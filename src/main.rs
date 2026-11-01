// maloki: transforma o Linux que está rodando numa imagem QEMU (.qcow2)
// "template": sem os seus dados pessoais e com os usuários que você escolher.
//
// Uso:
//   cargo build --release
//   sudo ./target/release/maloki                     # com menus
//   sudo ./target/release/maloki --plano plano.toml  # sem perguntas
//   ./target/release/maloki --simular                # só mostra o que faria
//
// Cada parte do programa fica num arquivo (módulo) separado:
mod boot;         // faz a cópia conseguir ligar em BIOS e UEFI
mod cancelamento; // Ctrl+C seguro (desmonta tudo antes de sair)
mod copia;        // copia o sistema com o rsync; espaço livre
mod disco;        // cria o .qcow2, particiona e monta
mod distro;       // descobre qual Linux está rodando
mod executor;     // roda comandos (ou só mostra, no modo simulação)
mod limpeza;      // remove dados pessoais da cópia
mod plano;        // o que fazer: pelos menus ou por um arquivo .toml
mod rede;         // troca IP fixo da máquina original por rede automática
mod servicos;     // para bancos de dados durante a cópia
mod trava;        // impede duas cópias do programa ao mesmo tempo
mod usuarios;     // menu e criação de usuários

use dialoguer::{theme::ColorfulTheme, Confirm};
use disco::{Disco, PARTICAO_BOOT, PARTICAO_EFI, PARTICAO_RAIZ};
use executor::Executor;
use plano::Plano;
use std::path::Path;
use std::process::Command;

// Um "apelido" de tipo: em vez de escrever esse tipo enorme em toda função,
// escrevemos Resultado<T>. Significa: "deu certo com um T, ou deu algum erro".
pub type Resultado<T> = Result<T, Box<dyn std::error::Error>>;

const AJUDA: &str = "\
maloki: transforma este Linux numa imagem QEMU (.qcow2) sem os seus dados.

Uso:
  sudo maloki                      pergunta tudo com menus
  sudo maloki --plano ARQUIVO      sem perguntas (veja exemplo-plano.toml)
  maloki --simular                 só mostra o passo a passo, sem mudar nada
  maloki --ajuda                   mostra esta ajuda
";

// As opções digitadas depois do nome do programa.
#[derive(Debug, PartialEq)]
struct Opcoes {
    simular: bool,
    arquivo_plano: Option<String>,
}

/// Lê as opções. Devolve Ok(None) quando a pessoa pediu a ajuda.
fn ler_opcoes(args: &[String]) -> Result<Option<Opcoes>, String> {
    let mut opcoes = Opcoes { simular: false, arquivo_plano: None };
    // `iter()` percorre a lista; com um iterador dá para "pegar o próximo" (o nome do arquivo).
    let mut itens = args.iter();
    while let Some(arg) = itens.next() {
        match arg.as_str() {
            "--simular" => opcoes.simular = true,
            "--plano" => match itens.next() {
                Some(arquivo) => opcoes.arquivo_plano = Some(arquivo.clone()),
                None => return Err("--plano precisa do nome de um arquivo".into()),
            },
            "--ajuda" | "-h" | "--help" => return Ok(None),
            outro => return Err(format!("opção desconhecida: {}\n\n{}", outro, AJUDA)),
        }
    }
    Ok(Some(opcoes))
}

fn main() {
    // std::env::args() são as palavras digitadas; skip(1) pula o nome do programa.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opcoes = match ler_opcoes(&args) {
        Ok(Some(opcoes)) => opcoes,
        Ok(None) => {
            print!("{}", AJUDA);
            return;
        }
        Err(msg) => {
            eprintln!("{}", msg);
            std::process::exit(2);
        }
    };

    // Toda a lógica fica em `executar`. Se ela devolver erro, mostramos e saímos
    // com código 1 (o jeito padrão de um programa avisar que falhou).
    if let Err(erro) = executar(&opcoes) {
        eprintln!("\n❌ Erro: {}", erro);
        std::process::exit(1);
    }
}

fn executar(opcoes: &Opcoes) -> Resultado<()> {
    let simular = opcoes.simular;
    println!("==========================================");
    println!("   maloki 🐧 → 📦 imagem QEMU");
    println!("==========================================");
    if simular {
        println!("MODO SIMULAÇÃO: nada será alterado, só mostro os comandos.\n");
    }

    // ---------- Verificações iniciais ----------
    if !simular && !eh_root() {
        return Err("rode com sudo (ou use --simular para só ver o passo a passo)".into());
    }
    // A trava vive até o fim desta função (o `_` no nome diz "não vou usar, só guardar").
    let _trava = if simular { None } else { Some(trava::pegar(trava::ARQUIVO_TRAVA)?) };

    let distro = distro::detectar()?;
    println!("Sistema atual: {} (família {:?})", distro.nome, distro.familia);

    // Em quais modos (BIOS/UEFI) a imagem vai conseguir ligar.
    let mut modos = boot::detectar_modos(distro.familia, "/");
    if simular && !eh_root() && Path::new("/boot/efi").exists() {
        // Sem root não dá para olhar dentro da partição EFI; na simulação supomos que tem.
        modos.uefi = true;
    }
    if !modos.bios && !modos.uefi {
        return Err("não achei o GRUB para BIOS nem para UEFI neste sistema".into());
    }
    println!("A imagem vai ligar em: {}", modos.descricao());

    let faltando: Vec<&str> = distro.familia.ferramentas().into_iter().filter(|p| !existe_programa(p)).collect();
    if !faltando.is_empty() {
        let msg = format!("faltam programas: {}", faltando.join(", "));
        if !simular {
            return Err(msg.into());
        }
        println!("⚠ {}", msg);
    }

    // ---------- O que fazer: menus ou arquivo ----------
    let plano = match &opcoes.arquivo_plano {
        Some(arquivo) => {
            println!("Lendo o plano de {} (modo automático, sem perguntas)...", arquivo);
            plano::do_arquivo(arquivo)?
        }
        None => plano::perguntar(&distro)?,
    };

    // ---------- Espaço livre: não deixar o disco deste computador encher ----------
    let pasta_da_imagem = Path::new(&plano.imagem).parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
    let necessario = copia::espaco_necessario(plano.usado_bytes, plano.compactar);
    if let Some(livre) = copia::espaco_livre(&pasta_da_imagem) {
        let gb = |bytes: u64| bytes as f64 / 1e9;
        println!("Espaço: {:.1} GB livres em {}, a imagem precisa de até {:.1} GB.", gb(livre), pasta_da_imagem, gb(necessario));
        if livre < necessario {
            let msg = "espaço livre insuficiente: escolha outra pasta (ex.: um HD externo) ou não compacte";
            if !simular {
                return Err(msg.into());
            }
            println!("⚠ {}", msg);
        }
    }

    mostrar_resumo(&plano, modos);
    if opcoes.arquivo_plano.is_none() {
        let tema = ColorfulTheme::default();
        if !Confirm::with_theme(&tema).with_prompt("Começar?").default(true).interact()? {
            println!("Cancelado.");
            return Ok(());
        }
    }

    // ---------- Execução ----------
    // A partir daqui, Ctrl+C não mata mais o programa na hora: ele limpa tudo antes.
    cancelamento::instalar()?;
    let exec = Executor { simular };

    // O disco é montado num arquivo "cru" temporário; no passo 8 ele vira o .qcow2.
    let raw = arquivo_raw(&plano.imagem);
    let resultado = criar_imagem(&exec, &plano, &distro, modos, &raw).and_then(|avisos| {
        passo(8, if plano.compactar { "Gerando o .qcow2 compactado" } else { "Gerando o .qcow2" })?;
        // -p mostra o progresso; -c compacta (o arquivo fica menor, demora mais).
        let mut args = vec!["convert", "-p", "-f", "raw", "-O", "qcow2"];
        if plano.compactar {
            args.push("-c");
        }
        args.push(&raw);
        args.push(&plano.imagem);
        exec.rodar("qemu-img", &args)?;

        passo(9, "Conferindo a imagem")?;
        // O `qemu-img check` confere a estrutura interna do arquivo .qcow2.
        exec.rodar("qemu-img", &["check", "-q", &plano.imagem])?;
        Ok(avisos)
    });

    // O .raw era só um rascunho: apagamos sempre, dando certo ou não.
    if !simular {
        let _ = std::fs::remove_file(&raw);
    }
    let avisos = match resultado {
        Ok(avisos) => avisos,
        Err(erro) => {
            // A imagem pela metade não serve para nada e só ocupa espaço.
            if !simular {
                let _ = std::fs::remove_file(&plano.imagem);
            }
            // Se foi Ctrl+C, o comando que estava rodando "falhou" porque foi
            // interrompido; a mensagem certa para a pessoa é a do cancelamento.
            cancelamento::verificar()?;
            return Err(erro);
        }
    };

    // O arquivo foi criado pelo root; devolvemos para quem chamou o sudo,
    // assim você consegue copiar/mover a imagem sem precisar de sudo.
    if let Ok(usuario) = std::env::var("SUDO_USER") {
        exec.rodar("chown", &[&format!("{}:", usuario), &plano.imagem])?;
    }

    println!("\n✅ Imagem pronta: {}", plano.imagem);
    if !avisos.is_empty() {
        println!("\n⚠ Estas chaves privadas continuam na imagem (podem ser necessárias para algum");
        println!("  serviço ligar). Se a imagem for compartilhada, avalie apagá-las ou trocá-las:");
        for arquivo in &avisos {
            println!("     {}", arquivo);
        }
    }
    println!("\nÉ um arquivo comum: copie para um HD externo, outro PC ou servidor (scp, rsync...).");
    println!("Precisa em outro formato? O qemu-img converte:");
    println!("   VirtualBox: qemu-img convert -O vdi  {} imagem.vdi", plano.imagem);
    println!("   VMware:     qemu-img convert -O vmdk {} imagem.vmdk", plano.imagem);
    println!("   Disco cru:  qemu-img convert -O raw  {} imagem.img", plano.imagem);
    println!("\nA imagem liga em: {}. A rede é automática (DHCP).", modos.descricao());
    println!("Se o sistema usa SELinux, no primeiro boot ele reetiqueta os arquivos");
    println!("e a máquina reinicia sozinha uma vez (é normal).");
    Ok(())
}

fn mostrar_resumo(plano: &Plano, modos: boot::Modos) {
    println!("\n================ RESUMO ================");
    println!("Imagem:      {} ({} GB)", plano.imagem, plano.tamanho_gb);
    println!("Hostname:    {}", plano.hostname);
    println!("Boot:        {}", modos.descricao());
    println!("Rede:        automática (DHCP)");
    println!("Flatpaks:    {}", if plano.incluir_flatpak { "incluídos" } else { "fora" });
    println!("Root:        {}", if plano.senha_root.is_some() { "com senha nova" } else { "bloqueado" });
    if !plano.parar_servicos.is_empty() {
        println!("Pausar:      {} (durante a cópia)", plano.parar_servicos.join(", "));
    }
    println!("Usuários:    {}", plano.usuarios.len());
    for u in &plano.usuarios {
        println!("  - {}{}", u.nome, if u.admin { " (sudo)" } else { "" });
    }
    println!("Seus dados (/home, Wi-Fi, VPN, chaves SSH, histórico, logs) NÃO vão para a imagem.");
    println!("========================================");
}

/// Os passos que mexem no disco. Ficam numa função separada porque o `disco`
/// precisa ser desmontado e solto (`finalizar`) antes de gerar o .qcow2.
/// Devolve a lista de avisos (chaves privadas que ficaram na imagem).
fn criar_imagem(exec: &Executor, plano: &Plano, distro: &distro::Distro, modos: boot::Modos, raw: &str) -> Resultado<Vec<String>> {
    passo(1, "Criando o disco virtual")?;
    let tamanho = format!("{}G", plano.tamanho_gb);
    // Um arquivo "esparso": diz ter o tamanho todo, mas só ocupa o que for gravado.
    exec.rodar("qemu-img", &["create", "-q", "-f", "raw", raw, &tamanho])?;

    // A partir daqui, se qualquer passo der erro (ou Ctrl+C), o `?` sai da
    // função e o Drop do `disco` desmonta e desconecta tudo automaticamente.
    let mut disco = Disco::conectar(*exec, raw)?;

    passo(2, "Particionando e formatando")?;
    disco.particionar_e_formatar()?;
    let raiz = disco.raiz.clone();
    // A ordem importa: a raiz primeiro, o /boot DENTRO dela, e a EFI dentro do /boot.
    disco.montar(&[&disco.particao(PARTICAO_RAIZ)], &raiz)?;
    disco.montar(&[&disco.particao(PARTICAO_BOOT)], &format!("{}/boot", raiz))?;
    disco.montar(&[&disco.particao(PARTICAO_EFI)], &format!("{}/boot/efi", raiz))?;

    passo(3, "Copiando o sistema (pode demorar bastante)")?;
    {
        // Os bancos de dados ficam parados só dentro destas chaves { }:
        // quando o bloco termina (ou dá erro), a `_pausa` é destruída e o Drop
        // dela liga os serviços de novo.
        let _pausa = servicos::pausar(exec, &plano.parar_servicos)?;
        exec.rodar("sync", &[])?; // grava no disco o que ainda está só na memória
        // O .raw está crescendo enquanto copiamos: ele não pode entrar na cópia!
        copia::copiar_sistema(exec, "/", &raiz, raw, plano.incluir_flatpak)?;
    }

    passo(4, "Removendo dados pessoais e identidade da máquina")?;
    limpeza::limpar(exec, &raiz, &plano.hostname)?;

    passo(5, "Preparando para rodar comandos dentro da imagem (chroot)")?;
    // O chroot precisa das pastas "virtuais" do Linux para os programas funcionarem.
    disco.montar(&["--bind", "/dev"], &format!("{}/dev", raiz))?;
    disco.montar(&["--bind", "/dev/pts"], &format!("{}/dev/pts", raiz))?;
    disco.montar(&["-t", "proc", "proc"], &format!("{}/proc", raiz))?;
    disco.montar(&["-t", "sysfs", "sys"], &format!("{}/sys", raiz))?;
    disco.montar(&["-t", "tmpfs", "tmpfs"], &format!("{}/run", raiz))?;

    passo(6, "Configurando o boot e a rede")?;
    // O blkid mostra o UUID ("RG") de cada partição recém-formatada.
    let uuid = |numero: u32, simulado: &str| exec.capturar("blkid", &["-s", "UUID", "-o", "value", &disco.particao(numero)], simulado);
    let uuids = boot::Uuids {
        raiz: uuid(PARTICAO_RAIZ, "UUID-DA-RAIZ")?,
        boot: uuid(PARTICAO_BOOT, "UUID-DO-BOOT")?,
        efi: uuid(PARTICAO_EFI, "UUID-EFI")?,
    };
    boot::configurar(exec, &raiz, distro.familia, &uuids, &disco.dispositivo, modos)?;
    rede::configurar(exec, &raiz)?;

    passo(7, "Trocando os usuários")?;
    // Isto precisa vir DEPOIS de montar o /proc: com SELinux ligado, o userdel
    // usa o /proc para gravar o /etc/passwd com a etiqueta de segurança certa.
    limpeza::remover_usuarios(exec, &raiz)?;
    usuarios::configurar_root(exec, &raiz, plano.senha_root.as_deref(), plano.senha_root_e_hash)?;
    usuarios::criar(exec, &raiz, &plano.usuarios, distro.familia.grupo_admin())?;

    let avisos = if exec.simular { Vec::new() } else { limpeza::procurar_chaves_privadas(&raiz) };

    // Desmonta e desconecta. Se não der para desmontar com segurança, é erro:
    // a imagem poderia estar corrompida e não pode ser entregue como pronta.
    disco.finalizar()?;
    Ok(avisos)
}

/// O rascunho "cru" fica ao lado da imagem final: template.qcow2 -> template.qcow2.raw
fn arquivo_raw(imagem: &str) -> String {
    format!("{}.raw", imagem)
}

/// Mostra o título do passo. Antes, confere se a pessoa apertou Ctrl+C.
fn passo(numero: u32, texto: &str) -> Resultado<()> {
    cancelamento::verificar()?;
    println!("\n━━━ [{}/9] {} ━━━", numero, texto);
    Ok(())
}

/// `id -u` mostra o número do usuário atual; o root é sempre 0.
fn eh_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|saida| String::from_utf8_lossy(&saida.stdout).trim() == "0")
        .unwrap_or(false)
}

/// Procura o programa nas pastas do PATH (e nas de administrador, /usr/sbin e /sbin).
fn existe_programa(nome: &str) -> bool {
    let path = std::env::var("PATH").unwrap_or_default();
    path.split(':')
        .chain(["/usr/sbin", "/sbin"])
        .any(|pasta| Path::new(pasta).join(nome).exists())
}

#[cfg(test)]
mod testes {
    use super::*;

    fn args(texto: &str) -> Vec<String> {
        texto.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn opcoes_da_linha_de_comando() {
        assert_eq!(ler_opcoes(&args("")).unwrap(), Some(Opcoes { simular: false, arquivo_plano: None }));
        assert_eq!(ler_opcoes(&args("--simular")).unwrap(), Some(Opcoes { simular: true, arquivo_plano: None }));
        assert_eq!(
            ler_opcoes(&args("--plano p.toml --simular")).unwrap(),
            Some(Opcoes { simular: true, arquivo_plano: Some("p.toml".into()) })
        );
        assert_eq!(ler_opcoes(&args("--ajuda")).unwrap(), None);
        assert_eq!(ler_opcoes(&args("-h")).unwrap(), None);
        assert!(ler_opcoes(&args("--plano")).unwrap_err().contains("precisa"));
        assert!(ler_opcoes(&args("--formatar-tudo")).unwrap_err().contains("desconhecida"));
    }

    fn plano_de_teste(compactar: bool) -> Plano {
        Plano {
            imagem: "/tmp/teste-template.qcow2".into(),
            tamanho_gb: 30,
            usado_bytes: 5_000_000_000,
            hostname: "template".into(),
            incluir_flatpak: false,
            usuarios: vec![usuarios::Usuario {
                nome: "maria".into(),
                senha: "segredo".into(),
                senha_e_hash: false,
                admin: true,
                trocar_senha: true,
                chave_ssh: Some("ssh-ed25519 AAAA maria@pc".into()),
            }],
            senha_root: None,
            senha_root_e_hash: false,
            compactar,
            parar_servicos: vec!["postgresql".into()],
        }
    }

    /// Roda TODOS os passos em modo simulação, para as duas famílias.
    /// Garante que nenhum passo dá erro nem aciona a trava de segurança.
    #[test]
    fn pipeline_completo_em_simulacao() {
        let exec = Executor { simular: true };
        for familia in [distro::Familia::Fedora, distro::Familia::Debian] {
            // Todas as combinações de BIOS/UEFI que fazem sentido.
            for (bios, uefi) in [(true, true), (true, false), (false, true)] {
                let d = distro::Distro { nome: "Teste".into(), id: "teste".into(), familia };
                let plano = plano_de_teste(true);
                let avisos = criar_imagem(&exec, &plano, &d, boot::Modos { bios, uefi }, &arquivo_raw(&plano.imagem)).unwrap();
                assert!(avisos.is_empty());
            }
        }
        // A imagem não pode ter sido criada de verdade.
        assert!(!Path::new("/tmp/teste-template.qcow2").exists());
        assert!(!Path::new("/tmp/teste-template.qcow2.raw").exists());
    }
}
