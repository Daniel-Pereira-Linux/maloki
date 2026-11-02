// O "plano" é tudo o que a pessoa escolheu: nome da imagem, tamanho, usuários...
//
// O maloki gera dois tipos de arquivo (o `Tipo`):
//   - template: uma imagem .qcow2 que liga direto numa VM (QEMU, Proxmox...)
//   - iso:      uma ISO "live" com o instalador Calamares, para instalar o
//               sistema em qualquer computador (ou VM) pelo pendrive/DVD
//
// Existem duas formas de montar o plano, e o resto do programa não sabe a diferença:
//   1. perguntar()     -> menus no terminal (modo interativo)
//   2. do_arquivo()    -> lê um arquivo .toml (modo automático, sem perguntas;
//                         bom para cron, scripts, Ansible...)

use crate::copia;
use crate::distro::Distro;
use crate::servicos;
use crate::usuarios::{self, Usuario};
use crate::Resultado;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Password, Select};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

/// O que gerar.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")] // no .toml: tipo = "template" ou tipo = "iso"
pub enum Tipo {
    Template,
    Iso,
}

/// Algoritmo de compressão. Cada tipo aceita dois:
///   template (.qcow2): zstd (padrão) ou zlib (para QEMU antigo, antes do 5.1)
///   iso (squashfs):    xz (padrão, a menor ISO) ou zstd (um pouco maior, liga mais rápido)
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compressao {
    Zstd,
    Zlib,
    Xz,
}

impl Compressao {
    pub fn nome(&self) -> &'static str {
        match self {
            Compressao::Zstd => "zstd",
            Compressao::Zlib => "zlib",
            Compressao::Xz => "xz",
        }
    }
}

pub struct Plano {
    pub tipo: Tipo,
    pub imagem: String, // caminho completo e REAL (sem "..", sem links) do arquivo final
    pub tamanho_gb: u64,
    pub usado_bytes: u64, // quanto o sistema ocupa (estimativa do `du`)
    pub hostname: String,
    pub incluir_flatpak: bool,
    pub usuarios: Vec<Usuario>,
    pub senha_root: Option<String>, // None = root bloqueado
    pub senha_root_e_hash: bool,
    pub compactar: bool,
    pub compressao: Compressao,
    pub parar_servicos: Vec<String>, // bancos de dados parados durante a cópia
}

/// Tamanho sugerido: o que já está usado + 30% + 5 GB de folga (mínimo de 20 GB).
fn tamanho_sugerido(usado_gb: u64) -> u64 {
    (usado_gb * 13 / 10 + 5).max(20)
}

/// Transforma bytes em GB arredondando para cima (2,1 GB vira 3).
fn em_gb(bytes: u64) -> u64 {
    bytes.div_ceil(1_000_000_000)
}

/// A pasta onde ficam os arquivos da ISO enquanto ela é montada:
/// fedora.iso -> fedora.iso.conteudo
pub fn pasta_conteudo(imagem: &str) -> String {
    format!("{}.conteudo", imagem)
}

/// O que o plano precisa saber do computador. Fica num struct para os testes
/// poderem inventar um computador qualquer.
pub struct Ambiente {
    pub usado_bytes: u64,
    pub pasta_atual: String,
    pub qemu_aceita_zstd: bool,
    pub passwd: String, // o /etc/passwd deste computador
    pub group: String,  // o /etc/group deste computador
}

impl Ambiente {
    fn deste_computador(incluir_flatpak: bool) -> Resultado<Ambiente> {
        Ok(Ambiente {
            usado_bytes: copia::estimar_tamanho(incluir_flatpak),
            pasta_atual: std::env::current_dir()?.to_string_lossy().to_string(),
            qemu_aceita_zstd: qemu_aceita_zstd(),
            passwd: fs::read_to_string("/etc/passwd").unwrap_or_default(),
            group: fs::read_to_string("/etc/group").unwrap_or_default(),
        })
    }
}

/// O .qcow2 com zstd precisa do QEMU 5.1 ou mais novo (de 2020).
/// `qemu-img --version` escreve "qemu-img version 10.2.2 (...)".
fn qemu_aceita_zstd() -> bool {
    let Ok(saida) = std::process::Command::new("qemu-img").arg("--version").output() else {
        return false;
    };
    versao_aceita_zstd(&String::from_utf8_lossy(&saida.stdout))
}

fn versao_aceita_zstd(texto: &str) -> bool {
    let versao = texto.split_whitespace().skip_while(|p| *p != "version").nth(1).unwrap_or("");
    let mut numeros = versao.split('.').map(|n| n.parse::<u32>().unwrap_or(0));
    let (maior, menor) = (numeros.next().unwrap_or(0), numeros.next().unwrap_or(0));
    (maior, menor) >= (5, 1)
}

/// Transforma o nome digitado no caminho completo e REAL do arquivo.
///
/// Por que "real"? O rsync precisa deixar de fora o arquivo que estamos criando
/// (senão ele copia a imagem para dentro dela mesma). Ele compara caminhos
/// reais: "/srv/imgs/../out/t.qcow2" ou um link simbólico no caminho não
/// casariam. Por isso resolvemos a pasta com `canonicalize`.
pub fn resolver_caminho(nome: &str, pasta_atual: &str) -> Result<String, String> {
    let nome = nome.trim();
    let caminho = Path::new(pasta_atual).join(nome);
    let arquivo = match caminho.file_name() {
        Some(arquivo) if !nome.is_empty() && !nome.ends_with('/') && !nome.ends_with("/..") && nome != ".." => arquivo,
        _ => return Err(format!("'{}' não é um nome de arquivo (ex.: template.qcow2)", nome)),
    };
    let pasta = caminho.parent().unwrap_or(Path::new("/"));
    let pasta_real: PathBuf = fs::canonicalize(pasta).map_err(|_| format!("a pasta {} não existe", pasta.display()))?;
    if !pasta_real.is_dir() {
        return Err(format!("{} não é uma pasta", pasta_real.display()));
    }
    let completo = pasta_real.join(arquivo);
    if completo.is_dir() {
        return Err(format!("{} é uma pasta; informe o nome de um arquivo", completo.display()));
    }
    Ok(completo.to_string_lossy().to_string())
}

// ======================= 1. MENUS (modo interativo) =======================

pub fn perguntar(distro: &Distro) -> Resultado<Plano> {
    let tema = ColorfulTheme::default();

    let tipo = match Select::with_theme(&tema)
        .with_prompt("O que você quer gerar?")
        .items(&[
            "Template para VMs (.qcow2): liga direto no QEMU, Proxmox, virt-manager...",
            "ISO instalável: sessão live + instalador Calamares (pendrive, DVD ou VM)",
        ])
        .default(0)
        .interact()?
    {
        0 => Tipo::Template,
        _ => Tipo::Iso,
    };

    let sugestao = match tipo {
        Tipo::Template => format!("template-{}.qcow2", distro.id),
        Tipo::Iso => format!("{}-maloki.iso", distro.id),
    };
    let pasta_atual = std::env::current_dir()?.to_string_lossy().to_string();
    let nome: String = Input::with_theme(&tema)
        .with_prompt("Nome do arquivo")
        .default(sugestao)
        .validate_with(|nome: &String| resolver_caminho(nome, &pasta_atual).map(|_| ()))
        .interact_text()?;
    let imagem = resolver_caminho(&nome, &pasta_atual)?;
    if Path::new(&imagem).exists() {
        let sobrescrever = Confirm::with_theme(&tema)
            .with_prompt(format!("{} já existe. Substituir? (a antiga só é trocada quando a nova ficar pronta)", nome))
            .default(false)
            .interact()?;
        if !sobrescrever {
            return Err("escolha outro nome para o arquivo".into());
        }
    }
    if tipo == Tipo::Iso {
        conferir_pasta_conteudo(&imagem)?;
    }

    let incluir_flatpak = Confirm::with_theme(&tema)
        .with_prompt("Incluir os apps Flatpak instalados no sistema?")
        .default(true)
        .interact()?;

    println!("Calculando o tamanho do sistema...");
    let ambiente = Ambiente::deste_computador(incluir_flatpak)?;
    let usado_bytes = ambiente.usado_bytes;
    let usado_gb = em_gb(usado_bytes);
    println!("O sistema ocupa cerca de {} GB.", usado_gb);

    // Na ISO, o tamanho é só o do disco de trabalho (não aparece para ninguém):
    // quem escolhe o tamanho do disco é quem instala, no Calamares.
    let tamanho_gb = match tipo {
        Tipo::Iso => tamanho_sugerido(usado_gb),
        Tipo::Template => Input::with_theme(&tema)
            .with_prompt("Tamanho do disco da VM em GB (o .qcow2 só ocupa o que for gravado)")
            .default(tamanho_sugerido(usado_gb))
            .validate_with(|n: &u64| validar_tamanho(*n, usado_gb))
            .interact_text()?,
    };

    let hostname: String = Input::with_theme(&tema)
        .with_prompt(match tipo {
            Tipo::Template => "Nome da máquina (hostname) da VM",
            Tipo::Iso => "Nome da máquina na sessão live (quem instala escolhe o definitivo)",
        })
        .default(String::from(match tipo {
            Tipo::Template => "template",
            Tipo::Iso => "live",
        }))
        .validate_with(|h: &String| validar_hostname(h))
        .interact_text()?;

    // Na ISO, os usuários e a senha do root são criados por quem instala (no Calamares).
    let (lista, senha_root) = match tipo {
        Tipo::Iso => {
            println!("\nNa ISO, quem instala cria os usuários no Calamares. Os usuários desta");
            println!("máquina NÃO vão para a ISO; a sessão live usa o usuário \"liveuser\" (senha: live).");
            (Vec::new(), None)
        }
        Tipo::Template => perguntar_usuarios(&tema, &ambiente)?,
    };

    // Bancos de dados ligados agora? Oferecemos parar durante a cópia.
    let ativos = servicos::ativos();
    let parar_servicos = if ativos.is_empty() {
        Vec::new()
    } else {
        println!("\nEstes serviços guardam dados que mudam o tempo todo: {}", ativos.join(", "));
        let parar = Confirm::with_theme(&tema)
            .with_prompt("Parar esses serviços durante a cópia? (recomendado; eles são religados no final)")
            .default(true)
            .interact()?;
        if parar { ativos } else { Vec::new() }
    };

    let (compactar, compressao) = match tipo {
        Tipo::Template => {
            let compactar = Confirm::with_theme(&tema)
                .with_prompt("Compactar a imagem no final? (arquivo menor, melhor para transportar; demora mais)")
                .default(true)
                .interact()?;
            let compressao = if compactar && ambiente.qemu_aceita_zstd {
                let escolha = Select::with_theme(&tema)
                    .with_prompt("Algoritmo de compressão")
                    .items(&[
                        "zstd: arquivo menor e a VM lê o disco mais rápido (QEMU 5.1+, de 2020)",
                        "zlib: o clássico, para quem vai usar a imagem num QEMU antigo",
                    ])
                    .default(0)
                    .interact()?;
                if escolha == 0 { Compressao::Zstd } else { Compressao::Zlib }
            } else {
                Compressao::Zlib
            };
            (compactar, compressao)
        }
        Tipo::Iso => {
            let escolha = Select::with_theme(&tema)
                .with_prompt("Algoritmo de compressão da ISO")
                .items(&[
                    "xz: a menor ISO possível (liga um pouco mais devagar)",
                    "zstd: ISO um pouco maior, mas a sessão live fica mais rápida",
                ])
                .default(0)
                .interact()?;
            (true, if escolha == 0 { Compressao::Xz } else { Compressao::Zstd })
        }
    };

    Ok(Plano {
        tipo,
        imagem,
        tamanho_gb,
        usado_bytes,
        hostname,
        incluir_flatpak,
        usuarios: lista,
        senha_root,
        senha_root_e_hash: false,
        compactar,
        compressao,
        parar_servicos,
    })
}

/// Os usuários e o root do template (só o template pergunta isso).
fn perguntar_usuarios(tema: &ColorfulTheme, ambiente: &Ambiente) -> Resultado<(Vec<Usuario>, Option<String>)> {
    println!("\n--- Usuários da imagem ---");
    println!("Os usuários atuais desta máquina serão REMOVIDOS da cópia.");
    let lista = usuarios::perguntar(tema, &ambiente.passwd, &ambiente.group)?;

    let bloquear_root = Confirm::with_theme(tema)
        .with_prompt("Bloquear o login do root? (recomendado; admins usam sudo)")
        .default(true)
        .interact()?;
    let senha_root = if bloquear_root {
        None
    } else {
        Some(
            Password::with_theme(tema)
                .with_prompt("Senha do root")
                .with_confirmation("Repita a senha", "As senhas não são iguais")
                .interact()?,
        )
    };

    if lista.iter().all(|u| !u.admin) && senha_root.is_none() {
        println!("⚠ Atenção: ninguém vai ter acesso de administrador nessa imagem.");
        if !Confirm::with_theme(tema).with_prompt("Continuar mesmo assim?").default(false).interact()? {
            return Err("cancelado: crie um usuário com sudo ou defina a senha do root".into());
        }
    }
    Ok((lista, senha_root))
}

/// A pasta de trabalho da ISO não pode existir: no fim ela é apagada inteira,
/// e não queremos apagar nada que não fomos nós que criamos.
fn conferir_pasta_conteudo(imagem: &str) -> Result<(), String> {
    let pasta = pasta_conteudo(imagem);
    if Path::new(&pasta).exists() {
        return Err(format!("{} já existe (sobra de uma execução interrompida?). Apague a pasta ou escolha outro nome", pasta));
    }
    Ok(())
}

// ======================= 2. ARQUIVO .toml (modo automático) =======================

// `#[derive(Deserialize)]` faz a crate serde ler o arquivo direto para dentro
// destes structs. `deny_unknown_fields` dá erro se tiver um campo com nome
// errado (ex.: "hostnme"), em vez de ignorar em silêncio.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arquivo {
    tipo: Option<Tipo>,
    imagem: String,
    tamanho_gb: Option<u64>,
    hostname: Option<String>,
    flatpak: Option<bool>,
    compactar: Option<bool>,
    compressao: Option<Compressao>,
    parar_servicos: Option<bool>,
    sobrescrever: Option<bool>,
    senha_root: Option<String>,
    senha_root_hash: Option<String>,
    #[serde(default)] // se não houver nenhum [[usuarios]], vira uma lista vazia
    usuarios: Vec<UsuarioDoArquivo>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UsuarioDoArquivo {
    nome: String,
    senha: Option<String>,
    senha_hash: Option<String>,
    admin: Option<bool>,
    trocar_senha: Option<bool>,
    chave_ssh: Option<String>,
}

pub fn do_arquivo(caminho: &str) -> Resultado<Plano> {
    let texto = std::fs::read_to_string(caminho).map_err(|e| format!("não consegui ler {}: {}", caminho, e))?;
    let arquivo: Arquivo = toml::from_str(&texto).map_err(|e| format!("erro no arquivo {}: {}", caminho, e))?;
    let ambiente = Ambiente::deste_computador(arquivo.flatpak.unwrap_or(true))?;
    montar_do_arquivo(arquivo, &ambiente)
}

/// Confere tudo e monta o Plano. Fica separada de `do_arquivo` para os testes
/// poderem inventar o computador (`Ambiente`).
fn montar_do_arquivo(arquivo: Arquivo, ambiente: &Ambiente) -> Resultado<Plano> {
    let tipo = arquivo.tipo.unwrap_or(Tipo::Template);
    let imagem = resolver_caminho(&arquivo.imagem, &ambiente.pasta_atual).map_err(|e| format!("imagem: {}", e))?;
    if Path::new(&imagem).exists() && !arquivo.sobrescrever.unwrap_or(false) {
        return Err(format!("{} já existe (use sobrescrever = true no arquivo)", imagem).into());
    }
    if tipo == Tipo::Iso {
        conferir_pasta_conteudo(&imagem)?;
        // Na ISO, quem cria os usuários é quem instala, no Calamares.
        if !arquivo.usuarios.is_empty() || arquivo.senha_root.is_some() || arquivo.senha_root_hash.is_some() {
            return Err("tipo = \"iso\": tire os [[usuarios]] e a senha do root; na ISO quem instala cria os usuários no Calamares".into());
        }
    }

    let usado_bytes = ambiente.usado_bytes;
    let usado_gb = em_gb(usado_bytes);
    let tamanho_gb = arquivo.tamanho_gb.unwrap_or(tamanho_sugerido(usado_gb));
    validar_tamanho(tamanho_gb, usado_gb)?;

    let padrao_hostname = if tipo == Tipo::Iso { "live" } else { "template" };
    let hostname = arquivo.hostname.unwrap_or_else(|| String::from(padrao_hostname));
    validar_hostname(&hostname).map_err(|e| format!("hostname '{}': {}", hostname, e))?;

    let mut lista: Vec<Usuario> = Vec::new();
    for u in arquivo.usuarios {
        usuarios::validar_nome(&u.nome)
            .and_then(|_| usuarios::nome_disponivel(&u.nome, &ambiente.passwd, &ambiente.group))
            .map_err(|e| format!("usuário '{}': {}", u.nome, e))?;
        if lista.iter().any(|existente| existente.nome == u.nome) {
            return Err(format!("usuário '{}' aparece duas vezes", u.nome).into());
        }
        let (senha, senha_e_hash) = escolher_senha(&u.nome, u.senha, u.senha_hash)?;
        let chave_ssh = match u.chave_ssh {
            Some(texto) => usuarios::ler_chave(&texto, &usuarios::pasta_pessoal())
                .map_err(|e| format!("chave SSH de '{}': {}", u.nome, e))?,
            None => None,
        };
        lista.push(Usuario {
            nome: u.nome,
            senha,
            senha_e_hash,
            admin: u.admin.unwrap_or(false),
            trocar_senha: u.trocar_senha.unwrap_or(false),
            chave_ssh,
        });
    }

    // Root: sem senha nenhuma no arquivo = bloqueado.
    let (senha_root, senha_root_e_hash) = match (arquivo.senha_root, arquivo.senha_root_hash) {
        (None, None) => (None, false),
        (senha, hash) => {
            let (s, e_hash) = escolher_senha("root", senha, hash)?;
            (Some(s), e_hash)
        }
    };

    if tipo == Tipo::Template && lista.iter().all(|u| !u.admin) && senha_root.is_none() {
        return Err("ninguém teria acesso de administrador: marque admin = true em um usuário ou defina senha_root".into());
    }

    let compressao = escolher_compressao(tipo, arquivo.compressao, ambiente.qemu_aceita_zstd)?;
    let parar_servicos = if arquivo.parar_servicos.unwrap_or(true) { servicos::ativos() } else { Vec::new() };

    Ok(Plano {
        tipo,
        imagem,
        tamanho_gb,
        usado_bytes,
        hostname,
        incluir_flatpak: arquivo.flatpak.unwrap_or(true),
        usuarios: lista,
        senha_root,
        senha_root_e_hash,
        // A ISO é sempre compactada (o squashfs é compactado por natureza).
        compactar: tipo == Tipo::Iso || arquivo.compactar.unwrap_or(true),
        compressao,
        parar_servicos,
    })
}

/// Confere se o algoritmo serve para o tipo e escolhe o padrão quando não foi informado.
fn escolher_compressao(tipo: Tipo, pedida: Option<Compressao>, qemu_aceita_zstd: bool) -> Result<Compressao, String> {
    match (tipo, pedida) {
        (Tipo::Template, None) if qemu_aceita_zstd => Ok(Compressao::Zstd),
        (Tipo::Template, None) => Ok(Compressao::Zlib),
        (Tipo::Template, Some(Compressao::Zstd)) if !qemu_aceita_zstd => {
            Err("compressao = \"zstd\" precisa do qemu-img 5.1 ou mais novo; use \"zlib\"".into())
        }
        (Tipo::Template, Some(Compressao::Xz)) => Err("o .qcow2 aceita compressao = \"zstd\" ou \"zlib\"".into()),
        (Tipo::Iso, None) => Ok(Compressao::Xz),
        (Tipo::Iso, Some(Compressao::Zlib)) => Err("a ISO aceita compressao = \"xz\" ou \"zstd\"".into()),
        (_, Some(c)) => Ok(c),
    }
}

/// Exatamente UMA das duas: `senha` (texto) ou `senha_hash` (já criptografada).
fn escolher_senha(dono: &str, senha: Option<String>, hash: Option<String>) -> Resultado<(String, bool)> {
    match (senha, hash) {
        (Some(s), None) if !s.is_empty() => Ok((s, false)),
        // Hashes do Linux sempre começam com $ (ex.: $6$ = SHA-512, $y$ = yescrypt).
        (None, Some(h)) if h.starts_with('$') && !h.contains([':', '\n']) => Ok((h, true)),
        (None, Some(_)) => Err(format!("senha_hash de '{}' não parece um hash (gere com: openssl passwd -6)", dono).into()),
        (Some(_), Some(_)) => Err(format!("'{}': use senha OU senha_hash, não as duas", dono).into()),
        _ => Err(format!("'{}' precisa de senha ou senha_hash", dono).into()),
    }
}

// ======================= Regras comuns aos dois modos =======================

fn validar_tamanho(tamanho_gb: u64, usado_gb: u64) -> Result<(), String> {
    if tamanho_gb < usado_gb + 2 {
        return Err(format!("o disco precisa ter pelo menos {} GB (o sistema ocupa {} GB)", usado_gb + 2, usado_gb));
    }
    Ok(())
}

/// Regras de hostname: 1 a 63 caracteres, letras, números e hífen,
/// sem começar nem terminar com hífen.
pub fn validar_hostname(nome: &str) -> Result<(), String> {
    if nome.is_empty() || nome.len() > 63 {
        return Err("use de 1 a 63 caracteres".into());
    }
    if nome.starts_with('-') || nome.ends_with('-') {
        return Err("não pode começar nem terminar com hífen".into());
    }
    if !nome.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("use só letras sem acento, números e hífen".into());
    }
    Ok(())
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::limpeza::apoio::*;

    const GB: u64 = 1_000_000_000;

    const PASSWD: &str = "root:x:0:0::/root:/bin/bash\ngdm:x:42:42::/var/lib/gdm:/sbin/nologin\ndaniel:x:1000:1000::/home/daniel:/bin/bash\n";
    const GROUP: &str = "root:x:0:\nwheel:x:10:daniel\ndocker:x:975:daniel\ndaniel:x:1000:\n";

    /// Um computador inventado: 5 GB usados, a pasta atual é `pasta`.
    fn ambiente(pasta: &str) -> Ambiente {
        Ambiente {
            usado_bytes: 5 * GB,
            pasta_atual: pasta.to_string(),
            qemu_aceita_zstd: true,
            passwd: PASSWD.into(),
            group: GROUP.into(),
        }
    }

    fn ler_toml_em(pasta: &str, texto: &str) -> Resultado<Plano> {
        let arquivo: Arquivo = toml::from_str(texto)?;
        montar_do_arquivo(arquivo, &ambiente(pasta))
    }

    fn ler_toml(texto: &str) -> Resultado<Plano> {
        ler_toml_em(&raiz_de_teste(), texto)
    }

    #[test]
    fn hostnames() {
        for bom in ["template", "web-01", "a", "VM2", &"a".repeat(63)] {
            assert!(validar_hostname(bom).is_ok(), "{}", bom);
        }
        for ruim in ["", "-template", "template-", "meu pc", "máquina", "a.b", "a_b", &"a".repeat(64)] {
            assert!(validar_hostname(ruim).is_err(), "{}", ruim);
        }
    }

    #[test]
    fn arquivo_completo() {
        let pasta = raiz_de_teste();
        let plano = ler_toml_em(&pasta,
            r#"
            imagem = "web.qcow2"
            tamanho_gb = 40
            hostname = "web-01"
            flatpak = false
            compactar = false
            parar_servicos = false
            senha_root_hash = "$6$sal$abc"

            [[usuarios]]
            nome = "maria"
            senha_hash = "$y$j9T$sal$hash"
            admin = true
            trocar_senha = true
            chave_ssh = "ssh-ed25519 AAAA maria@pc"

            [[usuarios]]
            nome = "joao"
            senha = "texto"
            "#,
        )
        .unwrap();
        assert_eq!(plano.imagem, format!("{}/web.qcow2", pasta));
        assert_eq!(plano.tipo, Tipo::Template);
        assert_eq!((plano.tamanho_gb, plano.hostname.as_str()), (40, "web-01"));
        assert!(!plano.incluir_flatpak && !plano.compactar && plano.parar_servicos.is_empty());
        assert_eq!(plano.senha_root.as_deref(), Some("$6$sal$abc"));
        assert!(plano.senha_root_e_hash);
        let maria = &plano.usuarios[0];
        assert!(maria.admin && maria.trocar_senha && maria.senha_e_hash);
        assert_eq!(maria.chave_ssh.as_deref(), Some("ssh-ed25519 AAAA maria@pc"));
        let joao = &plano.usuarios[1];
        assert!(!joao.admin && !joao.senha_e_hash && joao.chave_ssh.is_none());
    }

    #[test]
    fn arquivo_minimo_usa_os_padroes() {
        let plano = ler_toml("imagem = \"t.qcow2\"\n[[usuarios]]\nnome = \"maria\"\nsenha = \"x\"\nadmin = true\n").unwrap();
        assert_eq!(plano.tamanho_gb, tamanho_sugerido(5)); // 5 GB usados -> 20 GB (o mínimo)
        assert_eq!(plano.hostname, "template");
        assert!(plano.incluir_flatpak && plano.compactar);
        assert!(plano.senha_root.is_none()); // root bloqueado
        assert_eq!(plano.compressao, Compressao::Zstd);
    }

    #[test]
    fn erros_claros_no_arquivo() {
        let casos = [
            ("imagem = \"t.qcow2\"\nhostnme = \"x\"\n", "hostnme"), // campo com nome errado
            ("hostname = \"x\"\n", "imagem"),                       // falta a imagem
            ("imagem = \"t.qcow2\"\ntamanho_gb = 3\nsenha_root = \"x\"\n", "pelo menos 7 GB"),
            ("imagem = \"t.qcow2\"\nhostname = \"meu pc\"\nsenha_root = \"x\"\n", "hostname"),
            ("imagem = \"t.qcow2\"\n", "administrador"), // ninguém com sudo e root bloqueado
            ("imagem = \"t.qcow2\"\n[[usuarios]]\nnome = \"Maria\"\nsenha = \"x\"\nadmin = true\n", "Maria"),
            ("imagem = \"t.qcow2\"\n[[usuarios]]\nnome = \"maria\"\nadmin = true\n", "precisa de senha"),
            ("imagem = \"t.qcow2\"\n[[usuarios]]\nnome = \"maria\"\nsenha = \"a\"\nsenha_hash = \"$6$x\"\nadmin = true\n", "OU"),
            ("imagem = \"t.qcow2\"\n[[usuarios]]\nnome = \"maria\"\nsenha_hash = \"abc\"\nadmin = true\n", "openssl"),
            ("imagem = \"t.qcow2\"\n[[usuarios]]\nnome = \"m\"\nsenha = \"a\"\nadmin = true\n[[usuarios]]\nnome = \"m\"\nsenha = \"b\"\n", "duas vezes"),
            ("imagem = \"t.qcow2\"\n[[usuarios]]\nnome = \"m\"\nsenha = \"a\"\nadmin = true\nchave_ssh = \"/nao/existe.pub\"\n", "chave SSH"),
        ];
        for (texto, esperado) in casos {
            let erro = match ler_toml(texto) {
                Ok(_) => panic!("devia dar erro: {}", texto),
                Err(e) => e.to_string(),
            };
            assert!(erro.contains(esperado), "esperava '{}' no erro: {}", esperado, erro);
        }
    }

    #[test]
    fn nao_sobrescreve_sem_pedir() {
        let pasta = raiz_de_teste();
        criar(&pasta, "t.qcow2", "imagem antiga");
        let texto = "imagem = \"t.qcow2\"\nsenha_root = \"x\"\n";
        let arquivo: Arquivo = toml::from_str(texto).unwrap();
        assert!(montar_do_arquivo(arquivo, &ambiente(&pasta)).is_err());
        let arquivo: Arquivo = toml::from_str(&format!("{}sobrescrever = true\n", texto)).unwrap();
        assert!(montar_do_arquivo(arquivo, &ambiente(&pasta)).is_ok());
    }

    #[test]
    fn o_exemplo_do_projeto_e_valido() {
        // O arquivo de exemplo que acompanha o projeto tem que funcionar sempre.
        let texto = include_str!("../exemplo-plano.toml");
        let arquivo: Arquivo = toml::from_str(texto).unwrap();
        assert!(montar_do_arquivo(arquivo, &ambiente(&raiz_de_teste())).is_ok());
        let texto = include_str!("../exemplo-iso.toml");
        let arquivo: Arquivo = toml::from_str(texto).unwrap();
        assert_eq!(montar_do_arquivo(arquivo, &ambiente(&raiz_de_teste())).unwrap().tipo, Tipo::Iso);
    }

    #[test]
    fn caminho_da_imagem_e_resolvido() {
        let pasta = raiz_de_teste();
        criar(&pasta, "sub/x", "");
        // ".." e links viram o caminho real (é o que o rsync enxerga).
        assert_eq!(resolver_caminho("sub/../t.qcow2", &pasta).unwrap(), format!("{}/t.qcow2", pasta));
        std::os::unix::fs::symlink(format!("{}/sub", pasta), format!("{}/atalho", pasta)).unwrap();
        assert_eq!(resolver_caminho("atalho/t.qcow2", &pasta).unwrap(), format!("{}/sub/t.qcow2", pasta));
        assert_eq!(resolver_caminho("/caminho/absoluto/../", &pasta).unwrap_err(), "'/caminho/absoluto/../' não é um nome de arquivo (ex.: template.qcow2)");
        for ruim in ["", "  ", "sub/", "..", "sub/.."] {
            assert!(resolver_caminho(ruim, &pasta).unwrap_err().contains("não é um nome"), "{:?}", ruim);
        }
        assert!(resolver_caminho("sub", &pasta).unwrap_err().contains("é uma pasta"));
        assert!(resolver_caminho("nao/existe/t.qcow2", &pasta).unwrap_err().contains("não existe"));
    }

    #[test]
    fn iso_pelo_arquivo() {
        let plano = ler_toml("tipo = \"iso\"\nimagem = \"x.iso\"\n").unwrap();
        assert_eq!((plano.tipo, plano.compressao, plano.compactar), (Tipo::Iso, Compressao::Xz, true));
        assert_eq!(plano.hostname, "live");
        assert!(plano.usuarios.is_empty() && plano.senha_root.is_none());
        let plano = ler_toml("tipo = \"iso\"\nimagem = \"x.iso\"\ncompressao = \"zstd\"\ncompactar = false\n").unwrap();
        assert_eq!((plano.compressao, plano.compactar), (Compressao::Zstd, true));

        for (texto, esperado) in [
            ("tipo = \"iso\"\nimagem = \"x.iso\"\n[[usuarios]]\nnome = \"ana\"\nsenha = \"x\"\n", "Calamares"),
            ("tipo = \"iso\"\nimagem = \"x.iso\"\nsenha_root = \"x\"\n", "Calamares"),
            ("tipo = \"iso\"\nimagem = \"x.iso\"\ncompressao = \"zlib\"\n", "xz"),
            ("tipo = \"pendrive\"\nimagem = \"x.iso\"\n", "tipo"),
        ] {
            let erro = ler_toml(texto).err().map(|e| e.to_string()).unwrap_or_default();
            assert!(erro.contains(esperado), "esperava '{}' no erro: {}", esperado, erro);
        }
    }

    #[test]
    fn iso_nao_reaproveita_pasta_de_trabalho() {
        let pasta = raiz_de_teste();
        criar(&pasta, "x.iso.conteudo/LiveOS/squashfs.img", "sobra");
        let erro = ler_toml_em(&pasta, "tipo = \"iso\"\nimagem = \"x.iso\"\n").err().unwrap().to_string();
        assert!(erro.contains("já existe"), "{}", erro);
    }

    #[test]
    fn compressao_do_template() {
        let base = "imagem = \"t.qcow2\"\nsenha_root = \"x\"\n";
        assert_eq!(ler_toml(&format!("{}compressao = \"zlib\"\n", base)).unwrap().compressao, Compressao::Zlib);
        assert!(ler_toml(&format!("{}compressao = \"xz\"\n", base)).err().unwrap().to_string().contains("zlib"));
        // QEMU antigo: o padrão vira zlib, e pedir zstd é erro.
        let pasta = raiz_de_teste();
        let antigo = Ambiente { qemu_aceita_zstd: false, ..ambiente(&pasta) };
        let plano = montar_do_arquivo(toml::from_str(base).unwrap(), &antigo).unwrap();
        assert_eq!(plano.compressao, Compressao::Zlib);
        let pedido = format!("{}compressao = \"zstd\"\n", base);
        assert!(montar_do_arquivo(toml::from_str(&pedido).unwrap(), &antigo).is_err());
    }

    #[test]
    fn versao_do_qemu() {
        assert!(versao_aceita_zstd("qemu-img version 10.2.2 (qemu-10.2.2-1.fc44)\nCopyright"));
        assert!(versao_aceita_zstd("qemu-img version 5.1.0\n"));
        assert!(!versao_aceita_zstd("qemu-img version 5.0.1\n"));
        assert!(!versao_aceita_zstd("qemu-img version 4.2.1 (Debian 1:4.2-3ubuntu6)\n"));
        assert!(!versao_aceita_zstd(""));
    }

    #[test]
    fn usuario_com_nome_de_conta_do_sistema() {
        // gdm é conta do sistema; docker é um grupo: o useradd falharia lá no fim.
        for (nome, esperado) in [("gdm", "conta do sistema"), ("docker", "grupo")] {
            let texto = format!("imagem = \"t.qcow2\"\n[[usuarios]]\nnome = \"{}\"\nsenha = \"x\"\nadmin = true\n", nome);
            let erro = ler_toml(&texto).err().unwrap().to_string();
            assert!(erro.contains(esperado), "{}: {}", nome, erro);
        }
        // daniel é uma PESSOA: é removido da imagem, então o nome pode ser reusado.
        assert!(ler_toml("imagem = \"t.qcow2\"\n[[usuarios]]\nnome = \"daniel\"\nsenha = \"x\"\nadmin = true\n").is_ok());
    }

    #[test]
    fn tamanhos() {
        assert_eq!(tamanho_sugerido(0), 20);
        assert_eq!(tamanho_sugerido(30), 44);
        assert_eq!(em_gb(1), 1);
        assert_eq!(em_gb(2 * GB + 1), 3);
        assert!(validar_tamanho(10, 8).is_ok());
        assert!(validar_tamanho(9, 8).is_err());
    }
}
