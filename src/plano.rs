// O "plano" é tudo o que a pessoa escolheu: nome da imagem, tamanho, usuários...
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
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Password};
use serde::Deserialize;
use std::path::Path;

pub struct Plano {
    pub imagem: String,
    pub tamanho_gb: u64,
    pub usado_bytes: u64, // quanto o sistema ocupa (estimativa do `du`)
    pub hostname: String,
    pub incluir_flatpak: bool,
    pub usuarios: Vec<Usuario>,
    pub senha_root: Option<String>, // None = root bloqueado
    pub senha_root_e_hash: bool,
    pub compactar: bool,
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

// ======================= 1. MENUS (modo interativo) =======================

pub fn perguntar(distro: &Distro) -> Resultado<Plano> {
    let tema = ColorfulTheme::default();

    // Caminho absoluto da imagem, na pasta onde o programa foi chamado.
    let nome: String = Input::with_theme(&tema)
        .with_prompt("Nome do arquivo da imagem")
        .default(format!("template-{}.qcow2", distro.id))
        .interact_text()?;
    let imagem = std::env::current_dir()?.join(&nome).to_string_lossy().to_string();
    if Path::new(&imagem).exists() {
        let sobrescrever = Confirm::with_theme(&tema)
            .with_prompt(format!("{} já existe. Sobrescrever?", nome))
            .default(false)
            .interact()?;
        if !sobrescrever {
            return Err("escolha outro nome para a imagem".into());
        }
    }

    let incluir_flatpak = Confirm::with_theme(&tema)
        .with_prompt("Incluir os apps Flatpak instalados no sistema?")
        .default(true)
        .interact()?;

    println!("Calculando o tamanho do sistema...");
    let usado_bytes = copia::estimar_tamanho(incluir_flatpak);
    let usado_gb = em_gb(usado_bytes);
    println!("O sistema ocupa cerca de {} GB (o .qcow2 só ocupa no disco o que for gravado).", usado_gb);

    let tamanho_gb: u64 = Input::with_theme(&tema)
        .with_prompt("Tamanho do disco da VM em GB")
        .default(tamanho_sugerido(usado_gb))
        .validate_with(|n: &u64| validar_tamanho(*n, usado_gb))
        .interact_text()?;

    let hostname: String = Input::with_theme(&tema)
        .with_prompt("Nome da máquina (hostname) da VM")
        .default(String::from("template"))
        .validate_with(|h: &String| validar_hostname(h))
        .interact_text()?;

    println!("\n--- Usuários da imagem ---");
    println!("Os usuários atuais desta máquina serão REMOVIDOS da cópia.");
    let lista = usuarios::perguntar(&tema)?;

    let bloquear_root = Confirm::with_theme(&tema)
        .with_prompt("Bloquear o login do root? (recomendado; admins usam sudo)")
        .default(true)
        .interact()?;
    let senha_root = if bloquear_root {
        None
    } else {
        Some(
            Password::with_theme(&tema)
                .with_prompt("Senha do root")
                .with_confirmation("Repita a senha", "As senhas não são iguais")
                .interact()?,
        )
    };

    if lista.iter().all(|u| !u.admin) && senha_root.is_none() {
        println!("⚠ Atenção: ninguém vai ter acesso de administrador nessa imagem.");
        if !Confirm::with_theme(&tema).with_prompt("Continuar mesmo assim?").default(false).interact()? {
            return Err("cancelado: crie um usuário com sudo ou defina a senha do root".into());
        }
    }

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

    let compactar = Confirm::with_theme(&tema)
        .with_prompt("Compactar a imagem no final? (arquivo menor, melhor para transportar; demora mais)")
        .default(true)
        .interact()?;

    Ok(Plano {
        imagem,
        tamanho_gb,
        usado_bytes,
        hostname,
        incluir_flatpak,
        usuarios: lista,
        senha_root,
        senha_root_e_hash: false,
        compactar,
        parar_servicos,
    })
}

// ======================= 2. ARQUIVO .toml (modo automático) =======================

// `#[derive(Deserialize)]` faz a crate serde ler o arquivo direto para dentro
// destes structs. `deny_unknown_fields` dá erro se tiver um campo com nome
// errado (ex.: "hostnme"), em vez de ignorar em silêncio.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arquivo {
    imagem: String,
    tamanho_gb: Option<u64>,
    hostname: Option<String>,
    flatpak: Option<bool>,
    compactar: Option<bool>,
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
    let incluir_flatpak = arquivo.flatpak.unwrap_or(true);
    let usado_bytes = copia::estimar_tamanho(incluir_flatpak);
    montar_do_arquivo(arquivo, usado_bytes, &std::env::current_dir()?.to_string_lossy())
}

/// Confere tudo e monta o Plano. Fica separada de `do_arquivo` para os testes
/// poderem informar o tamanho do sistema e a pasta atual.
fn montar_do_arquivo(arquivo: Arquivo, usado_bytes: u64, pasta_atual: &str) -> Resultado<Plano> {
    let imagem = Path::new(pasta_atual).join(&arquivo.imagem).to_string_lossy().to_string();
    if Path::new(&imagem).exists() && !arquivo.sobrescrever.unwrap_or(false) {
        return Err(format!("{} já existe (use sobrescrever = true no arquivo)", imagem).into());
    }

    let usado_gb = em_gb(usado_bytes);
    let tamanho_gb = arquivo.tamanho_gb.unwrap_or(tamanho_sugerido(usado_gb));
    validar_tamanho(tamanho_gb, usado_gb)?;

    let hostname = arquivo.hostname.unwrap_or_else(|| String::from("template"));
    validar_hostname(&hostname).map_err(|e| format!("hostname '{}': {}", hostname, e))?;

    let mut lista: Vec<Usuario> = Vec::new();
    for u in arquivo.usuarios {
        usuarios::validar_nome(&u.nome).map_err(|e| format!("usuário '{}': {}", u.nome, e))?;
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

    if lista.iter().all(|u| !u.admin) && senha_root.is_none() {
        return Err("ninguém teria acesso de administrador: marque admin = true em um usuário ou defina senha_root".into());
    }

    let parar_servicos = if arquivo.parar_servicos.unwrap_or(true) { servicos::ativos() } else { Vec::new() };

    Ok(Plano {
        imagem,
        tamanho_gb,
        usado_bytes,
        hostname,
        incluir_flatpak: arquivo.flatpak.unwrap_or(true),
        usuarios: lista,
        senha_root,
        senha_root_e_hash,
        compactar: arquivo.compactar.unwrap_or(true),
        parar_servicos,
    })
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

    fn ler_toml(texto: &str) -> Resultado<Plano> {
        let arquivo: Arquivo = toml::from_str(texto)?;
        montar_do_arquivo(arquivo, 5 * GB, "/tmp/pasta-que-nao-tem-imagens")
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
        let plano = ler_toml(
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
        assert_eq!(plano.imagem, "/tmp/pasta-que-nao-tem-imagens/web.qcow2");
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
        assert!(montar_do_arquivo(arquivo, GB, &pasta).is_err());
        let arquivo: Arquivo = toml::from_str(&format!("{}sobrescrever = true\n", texto)).unwrap();
        assert!(montar_do_arquivo(arquivo, GB, &pasta).is_ok());
    }

    #[test]
    fn o_exemplo_do_projeto_e_valido() {
        // O arquivo de exemplo que acompanha o projeto tem que funcionar sempre.
        let texto = include_str!("../exemplo-plano.toml");
        let arquivo: Arquivo = toml::from_str(texto).unwrap();
        assert!(montar_do_arquivo(arquivo, 5 * GB, "/tmp/pasta-que-nao-tem-imagens").is_ok());
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
