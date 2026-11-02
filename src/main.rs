// maloki: transforma o Linux que está rodando, sem os seus dados pessoais, em:
//   - um "template" para VMs (.qcow2), com os usuários que você escolher; ou
//   - uma ISO instalável (sessão live + instalador Calamares).
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
mod live;         // ISO instalável: sessão live + instalador Calamares
mod plano;        // o que fazer: pelos menus ou por um arquivo .toml
mod rede;         // troca IP fixo da máquina original por rede automática
mod servicos;     // para bancos de dados durante a cópia
mod trava;        // impede duas cópias do programa ao mesmo tempo
mod usuarios;     // menu e criação de usuários

use dialoguer::{theme::ColorfulTheme, Confirm};
use disco::{Disco, PARTICAO_BOOT, PARTICAO_EFI, PARTICAO_RAIZ};
use executor::Executor;
use plano::{Compressao, Plano, Tipo};
use std::io::IsTerminal;
use std::path::Path;
use std::process::Command;

// Um "apelido" de tipo: em vez de escrever esse tipo enorme em toda função,
// escrevemos Resultado<T>. Significa: "deu certo com um T, ou deu algum erro".
pub type Resultado<T> = Result<T, Box<dyn std::error::Error>>;

const AJUDA: &str = "\
maloki: transforma este Linux, sem os seus dados, num template para VMs
(.qcow2) ou numa ISO instalável (sessão live + instalador Calamares).

Uso:
  sudo maloki                      pergunta tudo com menus
  sudo maloki --plano ARQUIVO      sem perguntas (veja exemplo-plano.toml e exemplo-iso.toml)
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
    println!("   maloki 🐧 → 📦 imagem QEMU ou 💿 ISO");
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
    avisar_ou_parar(simular, &programas_faltando(&distro.familia.ferramentas()), "")?;

    // ---------- O que fazer: menus ou arquivo ----------
    let plano = match &opcoes.arquivo_plano {
        Some(arquivo) => {
            println!("Lendo o plano de {} (modo automático, sem perguntas)...", arquivo);
            plano::do_arquivo(arquivo)?
        }
        None => {
            // Os menus precisam de um teclado e uma tela de verdade.
            if !std::io::stdin().is_terminal() {
                return Err("os menus precisam de um terminal; para rodar sem perguntas use --plano ARQUIVO".into());
            }
            plano::perguntar(&distro)?
        }
    };

    // ---------- Em quais modos (BIOS/UEFI) o resultado vai ligar ----------
    let modos = match plano.tipo {
        Tipo::Template => {
            let mut modos = boot::detectar_modos(distro.familia, "/");
            if simular && !eh_root() && Path::new("/boot/efi").exists() {
                // Sem root não dá para olhar dentro da partição EFI; na simulação supomos que tem.
                modos.uefi = true;
            }
            modos
        }
        Tipo::Iso => {
            let mut faltando = programas_faltando(&distro.familia.ferramentas_iso());
            faltando.extend(live::o_que_falta(distro.familia, "/"));
            let modos = live::modos_iso("/");
            if modos.uefi && !existe_programa("mformat") {
                faltando.push("mformat (pacote mtools, para a parte UEFI da ISO)".to_string());
            }
            avisar_ou_parar(simular, &faltando, distro.familia.pacotes_iso())?;
            if modos.uefi {
                println!("Obs.: a ISO liga com o Secure Boot DESLIGADO (o sistema instalado funciona com ele ligado).");
            }
            modos
        }
    };
    if !modos.bios && !modos.uefi {
        return Err("não achei o GRUB para BIOS nem para UEFI neste sistema".into());
    }
    println!("O resultado vai ligar em: {}", modos.descricao());

    // ---------- Espaço livre: não deixar o disco deste computador encher ----------
    let pasta_da_imagem = Path::new(&plano.imagem).parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
    let necessario = copia::espaco_necessario(plano.usado_bytes, plano.tipo, plano.compactar);
    if let Some(livre) = copia::espaco_livre(&pasta_da_imagem) {
        let gb = |bytes: u64| bytes as f64 / 1e9;
        println!("Espaço: {:.1} GB livres em {}, preciso de até {:.1} GB.", gb(livre), pasta_da_imagem, gb(necessario));
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

    // O disco é montado num arquivo "cru" temporário (.raw). O resultado é
    // gravado num .tmp e só no FIM troca de nome: se algo der errado, uma
    // imagem antiga com o mesmo nome continua intacta.
    let raw = arquivo_raw(&plano.imagem);
    let temporario = arquivo_temporario(&plano.imagem);
    let resultado = match plano.tipo {
        Tipo::Template => gerar_template(&exec, &plano, &distro, modos, &raw, &temporario),
        Tipo::Iso => gerar_iso(&exec, &plano, &distro, &raw, &temporario),
    };

    // O .raw era só um rascunho: apagamos sempre, dando certo ou não.
    if !simular {
        let _ = std::fs::remove_file(&raw);
    }
    let avisos = match resultado {
        Ok(avisos) => avisos,
        Err(erro) => {
            // O resultado pela metade não serve para nada e só ocupa espaço.
            // (Apagamos só o .tmp: uma imagem antiga com o nome final fica intacta.)
            if !simular {
                let _ = std::fs::remove_file(&temporario);
            }
            // Se foi Ctrl+C, o comando que estava rodando "falhou" porque foi
            // interrompido; a mensagem certa para a pessoa é a do cancelamento.
            cancelamento::verificar()?;
            return Err(erro);
        }
    };
    exec.renomear(&temporario, &plano.imagem)?;

    // O arquivo foi criado pelo root; devolvemos para quem chamou o sudo,
    // assim você consegue copiar/mover o arquivo sem precisar de sudo.
    if let Ok(usuario) = std::env::var("SUDO_USER") {
        exec.rodar("chown", &[&format!("{}:", usuario), &plano.imagem])?;
    }

    let pronta = if plano.tipo == Tipo::Iso { "ISO pronta" } else { "Imagem pronta" };
    println!("\n✅ {}: {}", pronta, plano.imagem);
    if !avisos.is_empty() {
        println!("\n⚠ Estas chaves privadas continuam no resultado (podem ser necessárias para algum");
        println!("  serviço ligar). Se ele for compartilhado, avalie apagá-las ou trocá-las:");
        for arquivo in &avisos {
            println!("     {}", arquivo);
        }
    }
    match plano.tipo {
        Tipo::Template => mostrar_dicas_template(&plano, modos),
        Tipo::Iso => mostrar_dicas_iso(&plano, modos),
    }
    Ok(())
}

/// Os programas da lista que não existem neste computador.
fn programas_faltando(programas: &[&str]) -> Vec<String> {
    programas.iter().filter(|p| !existe_programa(p)).map(|p| p.to_string()).collect()
}

/// Se falta algo: na simulação só avisa; de verdade, para com erro.
/// `instalar` é o comando que resolve (ex.: "sudo dnf install ...").
fn avisar_ou_parar(simular: bool, faltando: &[String], instalar: &str) -> Resultado<()> {
    if faltando.is_empty() {
        return Ok(());
    }
    let mut msg = format!("faltam: {}", faltando.join("; "));
    if !instalar.is_empty() {
        msg.push_str(&format!("\n   Para instalar: {}", instalar));
    }
    if !simular {
        return Err(msg.into());
    }
    println!("⚠ {}", msg);
    Ok(())
}

fn mostrar_resumo(plano: &Plano, modos: boot::Modos) {
    println!("\n================ RESUMO ================");
    match plano.tipo {
        Tipo::Template => println!("Gerar:       template para VMs (.qcow2), disco de {} GB", plano.tamanho_gb),
        Tipo::Iso => println!("Gerar:       ISO instalável (sessão live + instalador Calamares)"),
    }
    println!("Arquivo:     {}", plano.imagem);
    println!("Hostname:    {}", plano.hostname);
    println!("Liga em:     {}", modos.descricao());
    println!("Rede:        automática (DHCP)");
    println!("Flatpaks:    {}", if plano.incluir_flatpak { "incluídos" } else { "fora" });
    if plano.compactar {
        println!("Compressão:  {}", plano.compressao.nome());
    } else {
        println!("Compressão:  nenhuma");
    }
    if !plano.parar_servicos.is_empty() {
        println!("Pausar:      {} (durante a cópia)", plano.parar_servicos.join(", "));
    }
    match plano.tipo {
        Tipo::Template => {
            println!("Root:        {}", if plano.senha_root.is_some() { "com senha nova" } else { "bloqueado" });
            println!("Usuários:    {}", plano.usuarios.len());
            for u in &plano.usuarios {
                println!("  - {}{}", u.nome, if u.admin { " (sudo)" } else { "" });
            }
        }
        Tipo::Iso => println!("Usuários:    criados por quem instala (sessão live: {} / senha {})", live::USUARIO_LIVE, live::SENHA_LIVE),
    }
    println!("Seus dados (/home, Wi-Fi, VPN, chaves SSH, histórico, logs) NÃO vão para o arquivo.");
    println!("========================================");
}

fn mostrar_dicas_template(plano: &Plano, modos: boot::Modos) {
    println!("\nÉ um arquivo comum: copie para um HD externo, outro PC ou servidor (scp, rsync...).");
    println!("Precisa em outro formato? O qemu-img converte:");
    println!("   VirtualBox: qemu-img convert -O vdi  {} imagem.vdi", plano.imagem);
    println!("   VMware:     qemu-img convert -O vmdk {} imagem.vmdk", plano.imagem);
    println!("   Disco cru:  qemu-img convert -O raw  {} imagem.img", plano.imagem);
    if plano.compactar && plano.compressao == Compressao::Zstd {
        println!("(Compactada com zstd: use QEMU 5.1 ou mais novo para ligar.)");
    }
    println!("\nA imagem liga em: {}. A rede é automática (DHCP).", modos.descricao());
    println!("Se o sistema usa SELinux, no primeiro boot ele reetiqueta os arquivos");
    println!("e a máquina reinicia sozinha uma vez (é normal).");
}

fn mostrar_dicas_iso(plano: &Plano, modos: boot::Modos) {
    println!("\nGravar num pendrive (CUIDADO: apaga o pendrive; confira o nome com `lsblk`):");
    println!("   sudo dd if={} of=/dev/sdX bs=4M status=progress oflag=sync", plano.imagem);
    println!("   (ou use o Fedora Media Writer, o balenaEtcher ou o Ventoy)");
    println!("Testar numa VM antes:");
    println!("   qemu-system-x86_64 -enable-kvm -m 4096 -cdrom {} -drive file=disco.qcow2,if=virtio", plano.imagem);
    println!("   (crie o disco com: qemu-img create -f qcow2 disco.qcow2 40G)");
    println!("\nA ISO liga em: {}, com o Secure Boot desligado.", modos.descricao());
    println!("A sessão live entra sozinha como {} (senha: {}) e abre o instalador.", live::USUARIO_LIVE, live::SENHA_LIVE);
}

/// Os passos 1 a 5, iguais para o template e para a ISO: cria o disco de
/// trabalho, copia o sistema, limpa os dados pessoais e prepara o chroot.
fn copiar_e_limpar(exec: &Executor, plano: &Plano, raw: &str, temporario: &str) -> Resultado<Disco> {
    passo(1, "Criando o disco de trabalho")?;
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
        // Os arquivos que o maloki está criando não podem entrar na cópia!
        let conteudo = plano::pasta_conteudo(&plano.imagem);
        copia::copiar_sistema(exec, "/", &raiz, &[raw, temporario, &conteudo], plano.incluir_flatpak)?;
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
    Ok(disco)
}

/// Template: configura o boot do disco, troca os usuários e gera o .qcow2.
/// Devolve a lista de avisos (chaves privadas que ficaram na imagem).
fn gerar_template(exec: &Executor, plano: &Plano, distro: &distro::Distro, modos: boot::Modos, raw: &str, temporario: &str) -> Resultado<Vec<String>> {
    let avisos = criar_imagem(exec, plano, distro, modos, raw, temporario)?;

    passo(8, if plano.compactar { "Gerando o .qcow2 compactado" } else { "Gerando o .qcow2" })?;
    // -p mostra o progresso; -c compacta (o arquivo fica menor, demora mais).
    let mut args = vec!["convert", "-p", "-f", "raw", "-O", "qcow2"];
    if plano.compactar {
        args.push("-c");
        if plano.compressao == Compressao::Zstd {
            // zstd: comprime tanto quanto o zlib (às vezes mais) e descomprime
            // várias vezes mais rápido, então a VM lê o disco mais rápido.
            args.extend(["-o", "compression_type=zstd"]);
        }
    }
    args.push(raw);
    args.push(temporario);
    exec.rodar("qemu-img", &args)?;

    passo(9, "Conferindo a imagem")?;
    // O `qemu-img check` confere a estrutura interna do arquivo .qcow2.
    exec.rodar("qemu-img", &["check", "-q", temporario])?;
    Ok(avisos)
}

/// Os passos que mexem no disco do template. Ficam numa função separada porque
/// o `disco` precisa ser desmontado e solto (`finalizar`) antes de gerar o .qcow2.
fn criar_imagem(exec: &Executor, plano: &Plano, distro: &distro::Distro, modos: boot::Modos, raw: &str, temporario: &str) -> Resultado<Vec<String>> {
    let disco = copiar_e_limpar(exec, plano, raw, temporario)?;
    let raiz = disco.raiz.clone();

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

/// ISO: prepara a sessão live e o Calamares, compacta o sistema (squashfs) e
/// grava a ISO. Os passos 1 a 5 são os mesmos do template.
fn gerar_iso(exec: &Executor, plano: &Plano, distro: &distro::Distro, raw: &str, temporario: &str) -> Resultado<Vec<String>> {
    let mut disco = copiar_e_limpar(exec, plano, raw, temporario)?;
    let raiz = disco.raiz.clone();

    passo(6, "Preparando a sessão live e o instalador (Calamares)")?;
    // Os usuários desta máquina saem; quem instala cria os seus no Calamares.
    limpeza::remover_usuarios(exec, &raiz)?;
    usuarios::configurar_root(exec, &raiz, None, false)?;
    rede::configurar(exec, &raiz)?;
    // O Calamares exige de quem instala um disco do tamanho do sistema + folga.
    let armazenamento_gb = plano.usado_bytes.div_ceil(1_000_000_000) + 5;
    let live = live::preparar(exec, &raiz, distro, &plano.hostname, armazenamento_gb)?;
    let avisos = if exec.simular { Vec::new() } else { limpeza::procurar_chaves_privadas(&raiz) };

    passo(7, "Compactando o sistema (squashfs; é o passo mais demorado)")?;
    // /dev, /proc, /sys e /run do chroot não podem entrar no squashfs:
    // ficam montadas só as 3 partições (raiz, /boot e EFI).
    disco.desmontar_ate(3)?;
    let conteudo = live::PastaConteudo::criar(exec, &plano::pasta_conteudo(&plano.imagem))?;
    live::montar_conteudo(exec, &raiz, distro.familia, &live, &conteudo.caminho, plano.compressao)?;
    disco.finalizar()?;

    passo(8, "Gravando a ISO (BIOS + UEFI, DVD e pendrive)")?;
    live::gerar_iso(exec, distro.familia, &conteudo.caminho, &live.rotulo, temporario)?;

    passo(9, "Conferindo a ISO")?;
    live::conferir_iso(exec, distro.familia, temporario)?;
    Ok(avisos)
}

/// O rascunho "cru" fica ao lado da imagem final: template.qcow2 -> template.qcow2.raw
fn arquivo_raw(imagem: &str) -> String {
    format!("{}.raw", imagem)
}

/// O resultado é gravado aqui e só troca para o nome final quando estiver pronto.
fn arquivo_temporario(imagem: &str) -> String {
    format!("{}.tmp", imagem)
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
            tipo: Tipo::Template,
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
            compressao: Compressao::Zstd,
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
                let (raw, tmp) = (arquivo_raw(&plano.imagem), arquivo_temporario(&plano.imagem));
                let avisos = gerar_template(&exec, &plano, &d, boot::Modos { bios, uefi }, &raw, &tmp).unwrap();
                assert!(avisos.is_empty());
            }
        }
        // A imagem não pode ter sido criada de verdade.
        assert!(!Path::new("/tmp/teste-template.qcow2").exists());
        assert!(!Path::new("/tmp/teste-template.qcow2.raw").exists());
        assert!(!Path::new("/tmp/teste-template.qcow2.tmp").exists());
    }

    /// O mesmo para a ISO, nas duas famílias.
    #[test]
    fn pipeline_da_iso_em_simulacao() {
        let exec = Executor { simular: true };
        for familia in [distro::Familia::Fedora, distro::Familia::Debian] {
            let d = distro::Distro { nome: "Teste".into(), id: "teste".into(), familia };
            let plano = Plano {
                tipo: Tipo::Iso,
                imagem: "/tmp/teste-maloki.iso".into(),
                usuarios: Vec::new(),
                compressao: Compressao::Xz,
                ..plano_de_teste(true)
            };
            let (raw, tmp) = (arquivo_raw(&plano.imagem), arquivo_temporario(&plano.imagem));
            assert!(gerar_iso(&exec, &plano, &d, &raw, &tmp).unwrap().is_empty());
        }
        for sobra in ["/tmp/teste-maloki.iso", "/tmp/teste-maloki.iso.raw", "/tmp/teste-maloki.iso.tmp", "/tmp/teste-maloki.iso.conteudo"] {
            assert!(!Path::new(sobra).exists(), "{} não podia existir", sobra);
        }
    }
}
