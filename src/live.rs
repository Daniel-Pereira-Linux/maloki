// ISO instalável: transforma a cópia do sistema numa ISO "live" com o
// instalador Calamares.
//
// Como uma ISO live funciona (de baixo para cima):
//   1. O firmware (BIOS ou UEFI) liga o GRUB que está na ISO.
//   2. O GRUB carrega o kernel e um initramfs "live" (/boot/vmlinuz e
//      /boot/initrd.img da ISO).
//   3. O initramfs live procura a ISO, monta o sistema compactado (squashfs) e
//      põe uma camada gravável na memória por cima (overlay). O sistema liga
//      como se estivesse num disco, mas nada é gravado de verdade.
//        Fedora: módulo dmsquash-live do dracut (pacote dracut-live)
//        Debian: pacote live-boot
//   4. A tela de login entra sozinha no usuário "liveuser" e abre o Calamares.
//   5. O Calamares particiona o disco, copia o squashfs para lá, cria os
//      usuários e, no fim, roda o NOSSO script (pos-instalacao), que desfaz o
//      que era só da sessão live e instala o GRUB no disco.

use crate::boot;
use crate::distro::{Distro, Familia};
use crate::executor::Executor;
use crate::limpeza::na_imagem;
use crate::plano::Compressao;
use crate::usuarios;
use crate::Resultado;
use std::cmp::Ordering;
use std::fs;
use std::path::Path;

/// O usuário da sessão live. A senha só serve para destravar a tela.
pub const USUARIO_LIVE: &str = "liveuser";
pub const SENHA_LIVE: &str = "live";

/// O logo do instalador quando a distro não tem um (o `include_bytes!` põe o
/// arquivo DENTRO do programa na hora de compilar).
const LOGO_PADRAO: &[u8] = include_bytes!("../assets/logo.png");

/// O que a preparação descobriu e o resto da ISO precisa saber.
pub struct Live {
    pub kernel: String, // versão do kernel, ex.: "6.17.4-300.fc44.x86_64"
    pub rotulo: String, // nome do volume da ISO, ex.: "MALOKI_FEDORA"
    pub nome: String,   // nome do sistema, ex.: "Fedora Linux 44 (Workstation Edition)"
}

// ======================= O que a ISO precisa do sistema =======================

/// Confere se este sistema tem o que a ISO precisa (e que não é um programa
/// que dê para achar no PATH). Devolve a lista do que falta.
pub fn o_que_falta(familia: Familia, raiz: &str) -> Vec<String> {
    let mut falta = Vec::new();
    let tem = |caminho: &str| Path::new(&format!("{}/{}", raiz.trim_end_matches('/'), caminho)).exists();
    match familia {
        Familia::Fedora => {
            if !tem_modulo_dracut_live(raiz) {
                falta.push("o módulo dmsquash-live do dracut (pacote dracut-live)".to_string());
            }
        }
        Familia::Debian => {
            if !tem("usr/share/initramfs-tools/scripts/live") {
                falta.push("o live-boot (pacote live-boot)".to_string());
            }
        }
    }
    if telas_de_login(raiz).is_empty() {
        falta.push("uma tela de login gráfica (GDM, SDDM ou LightDM): o Calamares é um programa gráfico".to_string());
    }
    if !tem("usr/lib/grub/i386-pc") && !tem("usr/lib/grub/x86_64-efi") {
        falta.push("os módulos do GRUB para gerar a ISO (grub2-pc-modules / grub-pc-bin)".to_string());
    }
    falta
}

/// Em quais modos a ISO vai ligar: depende dos módulos do GRUB deste computador,
/// que o grub-mkrescue usa para montar a parte de boot da ISO.
pub fn modos_iso(raiz: &str) -> boot::Modos {
    let tem = |caminho: &str| Path::new(&format!("{}/{}", raiz.trim_end_matches('/'), caminho)).exists();
    boot::Modos { bios: tem("usr/lib/grub/i386-pc"), uefi: tem("usr/lib/grub/x86_64-efi") }
}

/// O nome da pasta muda de versão para versão (90dmsquash-live, 70dmsquash-live...).
fn tem_modulo_dracut_live(raiz: &str) -> bool {
    let pasta = format!("{}/usr/lib/dracut/modules.d", raiz.trim_end_matches('/'));
    fs::read_dir(pasta)
        .map(|entradas| entradas.flatten().any(|e| e.file_name().to_string_lossy().ends_with("dmsquash-live")))
        .unwrap_or(false)
}

/// As telas de login instaladas (o nome que o Calamares usa para cada uma).
fn telas_de_login(raiz: &str) -> Vec<&'static str> {
    let tem = |caminho: &str| Path::new(&format!("{}/{}", raiz.trim_end_matches('/'), caminho)).exists();
    let mut telas = Vec::new();
    if tem("usr/sbin/gdm") || tem("usr/sbin/gdm3") {
        telas.push("gdm");
    }
    if tem("usr/bin/sddm") {
        telas.push("sddm");
    }
    if tem("usr/sbin/lightdm") {
        telas.push("lightdm");
    }
    telas
}

// ======================= Preparar a sessão live =======================

/// Prepara a cópia (já limpa, e com o chroot montado) para virar a ISO.
/// `armazenamento_gb`: o mínimo de disco que o Calamares vai exigir de quem instala.
pub fn preparar(exec: &Executor, raiz: &str, distro: &Distro, hostname: &str, armazenamento_gb: u64) -> Resultado<Live> {
    let familia = distro.familia;
    let os_release = fs::read_to_string(na_imagem(raiz, "etc/os-release")).unwrap_or_default();
    let nome = valor_os_release(&os_release, "NAME").unwrap_or_else(|| distro.nome.clone());
    let rotulo = rotulo_do_volume(&distro.id);

    // Na sessão live não há nada para montar: o Calamares escreve o fstab do
    // sistema instalado. (O fstab copiado apontaria para os discos DESTE computador.)
    exec.escrever(
        &na_imagem(raiz, "etc/fstab"),
        "# Gerado pelo maloki: na sessão live não há discos para montar.\n# O instalador (Calamares) escreve o fstab do sistema instalado.\n",
    )?;
    exec.escrever(&na_imagem(raiz, "etc/hostname"), &format!("{}\n", hostname))?;
    // A limpeza pediu para o SELinux reetiquetar tudo no 1º boot. Na sessão live
    // isso faria a máquina reiniciar sozinha; quem pede de novo é o pos-instalacao.
    exec.apagar(&na_imagem(raiz, ".autorelabel"))?;
    // Os parâmetros de boot do sistema INSTALADO (o pos-instalacao completa o root=).
    exec.editar(&na_imagem(raiz, "etc/default/grub"), boot::limpar_default_grub)?;

    match familia {
        Familia::Fedora => guardar_arquivos_efi(exec, raiz)?,
        Familia::Debian => {
            // Igual ao template: o Debian não recria as chaves do SSH sozinho.
            exec.escrever(&na_imagem(raiz, "etc/systemd/system/gerar-chaves-ssh.service"), boot::SERVICO_CHAVES_SSH)?;
            exec.rodar("chroot", &[raiz, "systemctl", "enable", "gerar-chaves-ssh.service"])?;
        }
    }

    criar_usuario_live(exec, raiz, familia)?;
    entrar_sozinho(exec, raiz)?;
    instalar_atalhos(exec, raiz)?;
    configurar_calamares(exec, raiz, distro, &nome, &os_release, armazenamento_gb)?;
    exec.escrever(&na_imagem(raiz, "usr/libexec/maloki/pos-instalacao"), &script_pos_instalacao(familia))?;
    exec.rodar("chmod", &["755", &na_imagem(raiz, "usr/libexec/maloki/pos-instalacao")])?;

    let kernel = if exec.simular {
        "VERSAO-DO-KERNEL".to_string()
    } else {
        kernel_mais_novo(raiz).ok_or("não achei nenhum kernel em /usr/lib/modules da imagem")?
    };
    gerar_initramfs_live(exec, raiz, familia, &kernel)?;
    Ok(Live { kernel, rotulo, nome })
}

/// O Fedora não gera o GRUB UEFI na instalação: ele vem pronto (e assinado,
/// para o Secure Boot) nos pacotes shim-x64 e grub2-efi-x64, que gravam na
/// partição EFI. A partição EFI não vai para o squashfs, então guardamos uma
/// cópia desses arquivos dentro do sistema para o pos-instalacao usar.
fn guardar_arquivos_efi(exec: &Executor, raiz: &str) -> Resultado<()> {
    let pasta_efi = na_imagem(raiz, "boot/efi/EFI");
    if exec.simular {
        println!("[simulação] guardaria {}/<distro>/ (shim + grub UEFI) em /usr/lib/maloki/efi/", pasta_efi);
        return Ok(());
    }
    let Some(pasta_distro) = boot::achar_pasta_grub_efi(&pasta_efi) else {
        println!("  ⚠ sem GRUB UEFI na partição EFI: o sistema instalado só vai ligar em BIOS");
        return Ok(());
    };
    let nome = pasta_distro.file_name().unwrap_or_default().to_string_lossy().to_string();
    let destino = na_imagem(raiz, &format!("usr/lib/maloki/efi/{}", nome));
    exec.rodar("mkdir", &["-p", &destino])?;
    // "pasta/." = o conteúdo da pasta (como o "/" no fim, no rsync).
    exec.rodar("cp", &["-r", &format!("{}/.", pasta_distro.to_string_lossy()), &destino])
}

fn criar_usuario_live(exec: &Executor, raiz: &str, familia: Familia) -> Resultado<()> {
    exec.rodar("useradd", &[
        "-R", raiz, "-m", "-s", "/bin/bash", "-c", "Sessão live", "-G", familia.grupo_admin(), USUARIO_LIVE,
    ])?;
    usuarios::definir_senha(exec, raiz, USUARIO_LIVE, SENHA_LIVE, false)?;

    // sudo sem senha: o atalho do instalador roda o Calamares como root.
    let sudoers = na_imagem(raiz, "etc/sudoers.d/maloki-live");
    exec.escrever(&sudoers, &format!("# Gerado pelo maloki: só na sessão live.\n{} ALL=(ALL) NOPASSWD: ALL\n", USUARIO_LIVE))?;
    exec.rodar("chmod", &["440", &sudoers])?; // o sudo ignora o arquivo se outros puderem mexer

    // O polkit pergunta a senha para coisas como montar um pendrive ou mudar o
    // relógio. Na sessão live, o liveuser pode tudo sem senha.
    exec.escrever(&na_imagem(raiz, "etc/polkit-1/rules.d/49-maloki-live.rules"), &REGRA_POLKIT.replace("@USUARIO@", USUARIO_LIVE))
}

const REGRA_POLKIT: &str = "\
// Gerado pelo maloki: na sessão live, o @USUARIO@ não precisa de senha.
polkit.addRule(function(action, subject) {
    if (subject.user == \"@USUARIO@\") {
        return polkit.Result.YES;
    }
});
";

/// Login automático no liveuser, em cada tela de login instalada.
fn entrar_sozinho(exec: &Executor, raiz: &str) -> Resultado<()> {
    let sessao = sessao_grafica(raiz);
    for tela in telas_de_login(raiz) {
        match tela {
            "gdm" => {
                for arquivo in ["etc/gdm/custom.conf", "etc/gdm3/custom.conf"] {
                    let caminho = na_imagem(raiz, arquivo);
                    if Path::new(&caminho).parent().is_some_and(|p| p.exists()) || exec.simular {
                        let texto = fs::read_to_string(&caminho).unwrap_or_default();
                        exec.escrever(&caminho, &autologin_gdm(&texto, USUARIO_LIVE))?;
                    }
                }
            }
            "sddm" => exec.escrever(
                &na_imagem(raiz, "etc/sddm.conf.d/maloki-live.conf"),
                &format!("# Gerado pelo maloki: só na sessão live.\n[Autologin]\nUser={}\nSession={}\n", USUARIO_LIVE, sessao),
            )?,
            _ => exec.escrever(
                &na_imagem(raiz, "etc/lightdm/lightdm.conf.d/50-maloki-live.conf"),
                &format!("# Gerado pelo maloki: só na sessão live.\n[Seat:*]\nautologin-user={}\nautologin-session={}\n", USUARIO_LIVE, sessao),
            )?,
        }
    }
    Ok(())
}

/// Liga o login automático no custom.conf do GDM. As linhas AutomaticLogin
/// antigas (inclusive as que a limpeza comentou) saem; as novas entram logo
/// abaixo de [daemon].
fn autologin_gdm(texto: &str, usuario: &str) -> String {
    let novas = format!("AutomaticLoginEnable=True\nAutomaticLogin={}\n", usuario);
    let mut saida = String::new();
    let mut colocou = false;
    for linha in texto.lines() {
        let sem_comentario = linha.trim_start().trim_start_matches('#').trim_start();
        if sem_comentario.starts_with("AutomaticLogin") {
            continue;
        }
        saida.push_str(linha);
        saida.push('\n');
        if linha.trim() == "[daemon]" && !colocou {
            saida.push_str(&novas);
            colocou = true;
        }
    }
    if !colocou {
        if !saida.is_empty() && !saida.ends_with("\n\n") {
            saida.push('\n');
        }
        saida.push_str("[daemon]\n");
        saida.push_str(&novas);
    }
    saida
}

/// A sessão gráfica para o login automático do SDDM/LightDM (o GDM escolhe sozinho).
/// Preferimos as áreas de trabalho mais comuns; senão, a primeira em ordem alfabética.
fn sessao_grafica(raiz: &str) -> String {
    let mut sessoes = Vec::new();
    for pasta in ["usr/share/wayland-sessions", "usr/share/xsessions"] {
        if let Ok(entradas) = fs::read_dir(format!("{}/{}", raiz.trim_end_matches('/'), pasta)) {
            for entrada in entradas.flatten() {
                let nome = entrada.file_name().to_string_lossy().to_string();
                if let Some(sessao) = nome.strip_suffix(".desktop") {
                    sessoes.push(sessao.to_string());
                }
            }
        }
    }
    sessoes.sort();
    let preferidas = ["plasma", "gnome", "xfce", "cinnamon", "mate", "lxqt", "budgie-desktop", "openbox"];
    preferidas
        .iter()
        .find(|p| sessoes.iter().any(|s| s == *p))
        .map(|p| p.to_string())
        .or_else(|| sessoes.first().cloned())
        .unwrap_or_default()
}

/// O atalho "Instalar o sistema" no menu e abrindo sozinho ao entrar na sessão.
fn instalar_atalhos(exec: &Executor, raiz: &str) -> Resultado<()> {
    let lancador = na_imagem(raiz, "usr/libexec/maloki/instalar");
    exec.escrever(&lancador, LANCADOR)?;
    exec.rodar("chmod", &["755", &lancador])?;
    exec.escrever(&na_imagem(raiz, "usr/share/applications/maloki-instalador.desktop"), ATALHO)?;

    let casa = format!("home/{}", USUARIO_LIVE);
    exec.escrever(&na_imagem(raiz, &format!("{}/.config/autostart/maloki-instalador.desktop", casa)), ATALHO)?;
    // O GNOME mostraria o assistente de "primeiro uso" para o liveuser.
    exec.escrever(&na_imagem(raiz, &format!("{}/.config/gnome-initial-setup-done", casa)), "yes")?;
    // Os arquivos foram criados pelo root; o liveuser só existe DENTRO da imagem.
    let dono = format!("{0}:{0}", USUARIO_LIVE);
    exec.rodar("chroot", &[raiz, "chown", "-R", &dono, &format!("/{}", casa)])
}

const LANCADOR: &str = "\
#!/bin/sh
# Gerado pelo maloki: abre o instalador (Calamares) como root.
# O -E mantém as variáveis da sessão (DISPLAY, WAYLAND_DISPLAY...),
# senão o Calamares não acharia a tela para se desenhar.
exec sudo -E calamares \"$@\"
";

const ATALHO: &str = "\
[Desktop Entry]
Type=Application
Name=Instalar o sistema
Name[en]=Install System
Comment=Instala este sistema no computador
Comment[en]=Install this system on the computer
Exec=/usr/libexec/maloki/instalar
Icon=system-software-install
Terminal=false
Categories=System;
X-GNOME-Autostart-enabled=true
";

// ======================= Calamares =======================

/// Escreve a configuração completa do Calamares em /etc/calamares (ela tem
/// prioridade sobre a que vem com o pacote, em /usr/share/calamares).
fn configurar_calamares(exec: &Executor, raiz: &str, distro: &Distro, nome: &str, os_release: &str, armazenamento_gb: u64) -> Resultado<()> {
    let telas = telas_de_login(raiz);
    let fuso = fuso_horario(raiz);
    exec.escrever(&na_imagem(raiz, "etc/calamares/settings.conf"), SETTINGS)?;
    for (arquivo, conteudo) in modulos_calamares(distro.familia, &telas, &fuso, armazenamento_gb) {
        exec.escrever(&na_imagem(raiz, &format!("etc/calamares/modules/{}", arquivo)), &conteudo)?;
    }
    let branding = na_imagem(raiz, "etc/calamares/branding/maloki");
    exec.escrever(&format!("{}/branding.desc", branding), &branding_desc(nome, &distro.id))?;
    let logo = logo_da_distro(raiz, os_release).unwrap_or_else(|| LOGO_PADRAO.to_vec());
    exec.escrever_bytes(&format!("{}/logo.png", branding), &logo)
}

/// A ordem das telas (show) e das tarefas (exec) do Calamares.
/// O `shellprocess@maloki` é o nosso script, rodado no fim da instalação.
pub const SETTINGS: &str = "\
# Gerado pelo maloki: o instalador desta ISO.
modules-search: [ local ]

instances:
- id: maloki
  module: shellprocess
  config: shellprocess-maloki.conf

sequence:
- show:
  - welcome
  - locale
  - keyboard
  - partition
  - users
  - summary
- exec:
  - partition
  - mount
  - unpackfs
  - machineid
  - fstab
  - locale
  - keyboard
  - localecfg
  - users
  - displaymanager
  - networkcfg
  - hwclock
  - shellprocess@maloki
  - umount
- show:
  - finished

branding: maloki
prompt-install: true
dont-chroot: false
oem-setup: false
disable-cancel: false
disable-cancel-during-exec: false
hide-back-and-next-during-exec: true
quit-at-end: false
";

/// O arquivo de configuração de cada módulo do Calamares: (nome, conteúdo).
fn modulos_calamares(familia: Familia, telas: &[&str], fuso: &(String, String), armazenamento_gb: u64) -> Vec<(&'static str, String)> {
    // Onde o initramfs live deixa o squashfs montado durante a sessão live.
    let squashfs = match familia {
        Familia::Fedora => "/run/initramfs/live/LiveOS/squashfs.img",
        Familia::Debian => "/run/live/medium/live/filesystem.squashfs",
    };
    let grupo = familia.grupo_admin();
    let teclado_debian = familia == Familia::Debian;
    vec![
        ("welcome.conf", format!(
            "# Gerado pelo maloki\n\
             showSupportUrl: false\nshowKnownIssuesUrl: false\nshowReleaseNotesUrl: false\nshowDonateUrl: false\n\
             requirements:\n  requiredStorage: {}\n  requiredRam: 1.0\n  internetCheckUrl: \"https://example.com\"\n  check: [ storage, ram, root ]\n  required: [ storage, ram, root ]\n",
            armazenamento_gb
        )),
        ("locale.conf", format!(
            "# Gerado pelo maloki: o fuso sugerido é o do computador que gerou a ISO.\nregion: \"{}\"\nzone: \"{}\"\n",
            fuso.0, fuso.1
        )),
        ("keyboard.conf", format!(
            "# Gerado pelo maloki\nxOrgConfFileName: \"/etc/X11/xorg.conf.d/00-keyboard.conf\"\n\
             convertedKeymapPath: \"/lib/kbd/keymaps/xkb\"\nwriteEtcDefaultKeyboard: {}\n",
            teclado_debian
        )),
        // Só ext4 e sem criptografia: é o que o nosso pos-instalacao sabe ligar.
        ("partition.conf", "\
# Gerado pelo maloki
efi:
  mountPoint: \"/boot/efi\"
  recommendedSize: 512MiB
  minimumSize: 300MiB
efiSystemPartition: \"/boot/efi\"
userSwapChoices: [ none, file ]
initialSwapChoice: none
initialPartitioningChoice: erase
drawNestedPartitions: false
alwaysShowPartitionLabels: true
allowManualPartitioning: true
defaultFileSystemType: \"ext4\"
availableFileSystemTypes: [ \"ext4\" ]
enableLuksAutomatedPartitioning: false
luksGeneration: luks2
".to_string()),
        ("mount.conf", "\
# Gerado pelo maloki: o que o Calamares monta no sistema novo para rodar
# comandos lá dentro (igual ao chroot do maloki).
extraMounts:
  - device: proc
    fs: proc
    mountPoint: /proc
  - device: sys
    fs: sysfs
    mountPoint: /sys
  - device: /dev
    mountPoint: /dev
    options: [ bind ]
  - device: tmpfs
    fs: tmpfs
    mountPoint: /run
  - device: /run/udev
    mountPoint: /run/udev
    options: [ bind ]
  - device: efivarfs
    fs: efivarfs
    mountPoint: /sys/firmware/efi/efivars
    efi: true
mountOptions:
  - filesystem: default
    options: [ defaults ]
  - filesystem: efi
    options: [ defaults, umask=0077 ]
  - filesystem: ext4
    options: [ defaults ]
".to_string()),
        ("unpackfs.conf", format!(
            "# Gerado pelo maloki: copia o sistema da ISO para o disco.\nunpack:\n  - source: \"{}\"\n    sourcefs: \"squashfs\"\n    destination: \"\"\n",
            squashfs
        )),
        ("machineid.conf", "# Gerado pelo maloki: cada instalação ganha a sua identidade.\nsystemd: true\ndbus: true\ndbus-symlink: true\n".to_string()),
        ("fstab.conf", "# Gerado pelo maloki\ncrypttabOptions: luks\n".to_string()),
        ("users.conf", format!(
            "# Gerado pelo maloki\n\
             defaultGroups:\n  - name: users\n    must_exist: true\n    system: true\n  - {grupo}\n\
             autologinGroup: autologin\ndoAutologin: false\n\
             sudoersGroup: {grupo}\nsetRootPassword: false\ndoReusePassword: false\n\
             passwordRequirements:\n  minLength: 1\n  maxLength: -1\n\
             allowWeakPasswords: true\nallowWeakPasswordsDefault: true\nuserShell: /bin/bash\n\
             hostname:\n  location: EtcFile\n  writeHostsFile: true\n"
        )),
        ("displaymanager.conf", format!(
            "# Gerado pelo maloki\ndisplaymanagers: [ {} ]\nbasicSetup: false\nsysconfigSetup: false\n",
            telas.join(", ")
        )),
        ("shellprocess-maloki.conf", "\
# Gerado pelo maloki: roda o pos-instalacao dentro do sistema novo.
dontChroot: false
timeout: 3600
script:
  - command: \"/usr/libexec/maloki/pos-instalacao\"
    timeout: 3600
i18n:
  name: \"Finalizando a instalação\"
".to_string()),
        ("umount.conf", "# Gerado pelo maloki: desmonta o sistema novo no fim.\nemergency: false\n".to_string()),
        ("finished.conf", "# Gerado pelo maloki\nrestartNowMode: user-checked\nrestartNowCommand: \"systemctl -i reboot\"\n".to_string()),
    ]
}

/// A "cara" do instalador: nome do sistema, logo e cores.
fn branding_desc(nome: &str, id: &str) -> String {
    let nome = texto_yaml(nome);
    format!(
        "# Gerado pelo maloki\n---\ncomponentName: maloki\n\
         welcomeStyleCalamares: false\nwelcomeExpandingLogo: true\n\
         windowExpanding: normal\nwindowSize: 900px,620px\nwindowPlacement: center\n\
         sidebar: widget\nnavigation: widget\n\
         strings:\n    productName: {nome}\n    shortProductName: {nome}\n    version: \"\"\n    shortVersion: \"\"\n\
         \x20   versionedName: {nome}\n    shortVersionedName: {nome}\n    bootloaderEntryName: {id}\n\
         \x20   productUrl: \"\"\n    supportUrl: \"\"\n    knownIssuesUrl: \"\"\n    releaseNotesUrl: \"\"\n    donateUrl: \"\"\n\
         images:\n    productLogo: \"logo.png\"\n    productIcon: \"logo.png\"\n    productWelcome: \"logo.png\"\n\
         slideshow:\n    - \"logo.png\"\n\
         style:\n    SidebarBackground: \"#1d3f8a\"\n    SidebarText: \"#ffffff\"\n\
         \x20   SidebarTextCurrent: \"#1d3f8a\"\n    SidebarBackgroundCurrent: \"#ffffff\"\n",
        id = texto_yaml(id),
    )
}

/// Um texto entre aspas no YAML, com \ e " escapados.
fn texto_yaml(texto: &str) -> String {
    format!("\"{}\"", texto.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Lê um valor do /etc/os-release (ex.: NAME="Fedora Linux" -> Fedora Linux).
fn valor_os_release(texto: &str, chave: &str) -> Option<String> {
    texto.lines().find_map(|linha| {
        let (k, v) = linha.split_once('=')?;
        (k.trim() == chave).then(|| v.trim().trim_matches(['"', '\'']).to_string())
    })
}

/// O logo da distro (LOGO= no os-release), se houver um .png dele.
fn logo_da_distro(raiz: &str, os_release: &str) -> Option<Vec<u8>> {
    let logo = valor_os_release(os_release, "LOGO")?;
    let candidatos = [
        format!("usr/share/pixmaps/{}.png", logo),
        format!("usr/share/icons/hicolor/256x256/apps/{}.png", logo),
        format!("usr/share/icons/hicolor/128x128/apps/{}.png", logo),
        format!("usr/share/icons/hicolor/96x96/apps/{}.png", logo),
    ];
    candidatos.iter().find_map(|c| fs::read(format!("{}/{}", raiz.trim_end_matches('/'), c)).ok())
}

/// O fuso horário deste computador, para ser a sugestão do instalador.
/// O /etc/localtime é um link para .../zoneinfo/America/Sao_Paulo.
fn fuso_horario(raiz: &str) -> (String, String) {
    fs::read_link(format!("{}/etc/localtime", raiz.trim_end_matches('/')))
        .ok()
        .and_then(|link| fuso_do_link(&link.to_string_lossy()))
        .unwrap_or_else(|| ("America".into(), "Sao_Paulo".into()))
}

fn fuso_do_link(link: &str) -> Option<(String, String)> {
    let (_, fuso) = link.split_once("zoneinfo/")?;
    let (regiao, zona) = fuso.split_once('/')?;
    Some((regiao.to_string(), zona.to_string()))
}

/// O nome do volume da ISO: até 32 letras maiúsculas, números e "_".
/// O Fedora acha a ISO no boot por ele (root=live:CDLABEL=...).
pub fn rotulo_do_volume(id: &str) -> String {
    let id: String = id.to_uppercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    format!("MALOKI_{}", id).chars().take(32).collect()
}

// ======================= Kernel e initramfs =======================

/// A versão de kernel mais nova da imagem (a pasta em /usr/lib/modules).
fn kernel_mais_novo(raiz: &str) -> Option<String> {
    let mut versoes: Vec<String> = Vec::new();
    for pasta in ["usr/lib/modules", "lib/modules"] {
        if let Ok(entradas) = fs::read_dir(format!("{}/{}", raiz.trim_end_matches('/'), pasta)) {
            for entrada in entradas.flatten() {
                let versao = entrada.file_name().to_string_lossy().to_string();
                if arquivo_do_kernel(raiz, &versao).is_some() && !versoes.contains(&versao) {
                    versoes.push(versao);
                }
            }
        }
    }
    versoes.into_iter().max_by(|a, b| comparar_versoes(a, b))
}

/// Onde está o kernel daquela versão (no /boot ou junto dos módulos).
fn arquivo_do_kernel(raiz: &str, versao: &str) -> Option<String> {
    [format!("boot/vmlinuz-{}", versao), format!("usr/lib/modules/{}/vmlinuz", versao)]
        .into_iter()
        .map(|c| format!("{}/{}", raiz.trim_end_matches('/'), c))
        .find(|c| Path::new(c).exists())
}

/// Compara versões pelos NÚMEROS: "6.10" é mais novo que "6.9"
/// (comparando como texto, "6.9" pareceria maior).
fn comparar_versoes(a: &str, b: &str) -> Ordering {
    let pedacos = |v: &str| -> Vec<u64> {
        v.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).map(|p| p.parse().unwrap_or(0)).collect()
    };
    pedacos(a).cmp(&pedacos(b)).then_with(|| a.cmp(b))
}

/// O initramfs "live" sabe achar a ISO e montar o squashfs. Fica no /tmp da
/// imagem até ir para a ISO (o /tmp não entra no squashfs).
fn gerar_initramfs_live(exec: &Executor, raiz: &str, familia: Familia, kernel: &str) -> Resultado<()> {
    match familia {
        Familia::Fedora => exec.rodar("chroot", &[
            raiz, "dracut", "--force", "--no-hostonly", "--no-hostonly-cmdline",
            "--add", "dmsquash-live", "--add-drivers", "iso9660 squashfs overlay sr_mod cdrom loop",
            "/tmp/maloki-initrd.img", kernel,
        ]),
        // O live-boot instalado entra sozinho no initramfs.
        Familia::Debian => exec.rodar("chroot", &[raiz, "mkinitramfs", "-o", "/tmp/maloki-initrd.img", kernel]),
    }
}

/// Os parâmetros de boot da sessão live.
pub fn parametros_live(familia: Familia, rotulo: &str) -> String {
    match familia {
        // enforcing=0: o SELinux só avisa na sessão live (arquivos criados para a
        // sessão não têm as etiquetas finais). O sistema instalado é reetiquetado.
        Familia::Fedora => format!("root=live:CDLABEL={} rd.live.image rd.live.overlay.overlayfs=1 enforcing=0 quiet rhgb", rotulo),
        Familia::Debian => "boot=live quiet splash".to_string(),
    }
}

/// Onde cada família espera o squashfs dentro da ISO.
fn caminho_squashfs(familia: Familia) -> &'static str {
    match familia {
        Familia::Fedora => "LiveOS/squashfs.img",
        Familia::Debian => "live/filesystem.squashfs",
    }
}

/// O menu de boot da ISO. Também aparece na porta serial (útil em servidores
/// e nos testes automáticos, que ligam a ISO sem tela).
pub fn menu_grub(nome: &str, parametros: &str) -> String {
    let nome = nome.replace(['"', '\\', '$'], "");
    format!(
        "# Gerado pelo maloki: o menu de boot da ISO.\n\
         set timeout=5\nset default=0\n\
         insmod all_video\nset gfxpayload=keep\n\
         serial --unit=0 --speed=115200\nterminal_input --append serial\nterminal_output --append serial\n\n\
         menuentry \"Iniciar {nome} (sessão live)\" {{\n    linux /boot/vmlinuz {parametros}\n    initrd /boot/initrd.img\n}}\n\n\
         menuentry \"Iniciar {nome} (vídeo em modo seguro)\" {{\n    linux /boot/vmlinuz {parametros} nomodeset\n    initrd /boot/initrd.img\n}}\n\n\
         menuentry \"Iniciar {nome} (console serial, para máquinas sem tela)\" {{\n    linux /boot/vmlinuz {parametros} console=tty0 console=ttyS0,115200\n    initrd /boot/initrd.img\n}}\n"
    )
}

// ======================= Montar a ISO =======================

/// A pasta com os arquivos da ISO. Ela é apagada sozinha no fim (o `Drop`),
/// dando certo ou não: a ISO pronta já leva uma cópia de tudo.
pub struct PastaConteudo {
    pub caminho: String,
    simular: bool,
}

impl PastaConteudo {
    /// Cria a pasta. `create_dir` dá erro se ela já existir: nunca apagamos
    /// no fim uma pasta que não fomos nós que criamos.
    pub fn criar(exec: &Executor, caminho: &str) -> Resultado<PastaConteudo> {
        if exec.simular {
            println!("[simulação] criaria a pasta {}/", caminho);
        } else {
            fs::create_dir(caminho).map_err(|e| format!("não consegui criar {}: {}", caminho, e))?;
        }
        Ok(PastaConteudo { caminho: caminho.to_string(), simular: exec.simular })
    }
}

impl Drop for PastaConteudo {
    fn drop(&mut self) {
        if !self.simular {
            let _ = fs::remove_dir_all(&self.caminho);
        }
    }
}

/// Põe na pasta da ISO o kernel, o initramfs live, o menu e o squashfs.
/// Precisa ser chamada com o chroot JÁ desmontado (/dev, /proc... não podem
/// entrar no squashfs), mas com a raiz e o /boot ainda montados.
pub fn montar_conteudo(exec: &Executor, raiz: &str, familia: Familia, live: &Live, conteudo: &str, compressao: Compressao) -> Resultado<()> {
    exec.rodar("mkdir", &["-p", &format!("{}/boot/grub", conteudo)])?;
    let kernel = if exec.simular {
        na_imagem(raiz, &format!("boot/vmlinuz-{}", live.kernel))
    } else {
        arquivo_do_kernel(raiz, &live.kernel).ok_or("o kernel sumiu da imagem")?
    };
    exec.rodar("cp", &[&kernel, &format!("{}/boot/vmlinuz", conteudo)])?;
    exec.rodar("mv", &[&na_imagem(raiz, "tmp/maloki-initrd.img"), &format!("{}/boot/initrd.img", conteudo)])?;
    exec.escrever(&format!("{}/boot/grub/grub.cfg", conteudo), &menu_grub(&live.nome, &parametros_live(familia, &live.rotulo)))?;

    let squashfs = format!("{}/{}", conteudo, caminho_squashfs(familia));
    exec.rodar("mkdir", &["-p", Path::new(&squashfs).parent().unwrap_or(Path::new(conteudo)).to_str().unwrap_or(conteudo)])?;
    let args = argumentos_mksquashfs(raiz, &squashfs, compressao);
    let args: Vec<&str> = args.iter().map(|a| a.as_str()).collect();
    exec.rodar("mksquashfs", &args)
}

/// -b 1M      blocos grandes: comprime bem mais (é o que as distros usam)
/// -noappend  sempre um arquivo novo
/// -e ...     fica de fora: a partição EFI (o pos-instalacao cuida dela) e o /tmp
fn argumentos_mksquashfs(raiz: &str, destino: &str, compressao: Compressao) -> Vec<String> {
    let mut args: Vec<String> = vec![raiz.into(), destino.into(), "-noappend".into(), "-b".into(), "1M".into()];
    match compressao {
        // xz com o filtro "x86": entende código de máquina e comprime ainda mais
        // os programas. É o que dá a menor ISO.
        Compressao::Xz => args.extend(["-comp", "xz", "-Xbcj", "x86"].map(String::from)),
        Compressao::Zstd | Compressao::Zlib => args.extend(["-comp", "zstd", "-Xcompression-level", "19"].map(String::from)),
    }
    // -e precisa ser o ÚLTIMO: tudo depois dele é lista de exclusão.
    args.extend(["-wildcards", "-e", "boot/efi/*", "tmp/*"].map(String::from));
    args
}

/// Gera a ISO híbrida (DVD e pendrive; BIOS e UEFI) com o grub-mkrescue.
pub fn gerar_iso(exec: &Executor, familia: Familia, conteudo: &str, rotulo: &str, destino: &str) -> Resultado<()> {
    let mkrescue = match familia {
        Familia::Fedora => "grub2-mkrescue",
        Familia::Debian => "grub-mkrescue",
    };
    // Depois do "--" vêm comandos do xorriso (quem grava a ISO de verdade):
    //   -volid      o nome do volume, que o Fedora usa para achar a ISO no boot
    //   -compliance iso_9660_level=3  aceita arquivos maiores que 4 GB (o squashfs)
    exec.rodar(mkrescue, &["-o", destino, conteudo, "--", "-volid", rotulo, "-compliance", "iso_9660_level=3"])
}

/// Confere se a ISO tem o que precisa para ligar.
pub fn conferir_iso(exec: &Executor, familia: Familia, iso: &str) -> Resultado<()> {
    let squashfs = format!("/{}", caminho_squashfs(familia));
    // -lsl lista os arquivos; se algum não existir, o xorriso termina com erro.
    exec.rodar("xorriso", &["-indev", iso, "-lsl", "/boot/vmlinuz", "/boot/initrd.img", "/boot/grub/grub.cfg", &squashfs])?;
    if !exec.simular {
        let tamanho = fs::metadata(iso)?.len();
        println!("  ISO com {:.2} GB", tamanho as f64 / 1e9);
    }
    Ok(())
}

// ======================= O script de pós-instalação =======================

/// O script que o Calamares roda DENTRO do sistema instalado, no fim.
pub fn script_pos_instalacao(familia: Familia) -> String {
    let familia = match familia {
        Familia::Fedora => "fedora",
        Familia::Debian => "debian",
    };
    POS_INSTALACAO.replace("@FAMILIA@", familia).replace("@USUARIO@", USUARIO_LIVE)
}

const POS_INSTALACAO: &str = r#"#!/bin/sh
# Gerado pelo maloki. O Calamares roda este script DENTRO do sistema que
# acabou de ser instalado (num chroot), depois de copiar os arquivos e criar
# os usuários. Ele:
#   1. tira o que só servia para a sessão live (liveuser, login automático...)
#   2. aponta o boot para o disco novo e refaz o initramfs
#   3. instala o GRUB (em BIOS ou UEFI, o modo em que o computador ligou)
# Se algo falhar, o Calamares mostra a mensagem e avisa que a instalação falhou.
set -eu
FAMILIA=@FAMILIA@
. /etc/os-release
falhar() { echo "maloki: $*" >&2; exit 1; }
avisar() { echo "maloki: $*"; }

# ---------- 1. O que era só da sessão live ----------
userdel -r @USUARIO@ >/dev/null 2>&1 || userdel @USUARIO@ >/dev/null 2>&1 || true
rm -rf /home/@USUARIO@
rm -f /etc/sudoers.d/maloki-live /etc/polkit-1/rules.d/49-maloki-live.rules \
      /etc/sddm.conf.d/maloki-live.conf /etc/lightdm/lightdm.conf.d/50-maloki-live.conf \
      /usr/share/applications/maloki-instalador.desktop
for arquivo in /etc/gdm/custom.conf /etc/gdm3/custom.conf; do
    if [ -f "$arquivo" ]; then sed -i '/^AutomaticLogin=@USUARIO@$/d' "$arquivo"; fi
done
if [ "$FAMILIA" = debian ]; then
    # O live-boot só serve para ligar da ISO; no disco ele atrapalharia o boot.
    # O live-tools ainda "desvia" o update-initramfs (ele não faria nada).
    for pacote in live-boot live-boot-initramfs-tools live-tools live-config live-config-systemd; do
        dpkg --purge "$pacote" >/dev/null 2>&1 || true
    done
fi

# ---------- 2. Os discos do sistema novo ----------
RAIZ_UUID=$(findmnt -no UUID /) || falhar "não achei o disco da raiz"
RAIZ_DEV=$(findmnt -no SOURCE /)
DISCO=/dev/$(lsblk -no PKNAME "$RAIZ_DEV" | head -n1)
[ -b "$DISCO" ] || falhar "não achei o disco de $RAIZ_DEV"
if [ -d /sys/firmware/efi ]; then
    EFI_DEV=$(findmnt -no SOURCE /boot/efi) || falhar "o modo UEFI precisa de uma partição EFI montada em /boot/efi"
    EFI_DISCO=/dev/$(lsblk -no PKNAME "$EFI_DEV" | head -n1)
    EFI_PARTE=$(cat "/sys/class/block/$(basename "$EFI_DEV")/partition")
fi

# ---------- 3. Kernel: linha de boot e initramfs ----------
if [ "$FAMILIA" = fedora ]; then
    # O kernel-install recria a entrada de boot (BLS) e o initramfs de cada
    # kernel, já com a linha de boot do /etc/kernel/cmdline.
    echo "root=UUID=$RAIZ_UUID ro rhgb quiet" > /etc/kernel/cmdline
    rm -f /boot/loader/entries/*.conf
    for pasta in /usr/lib/modules/*/; do
        versao=$(basename "$pasta")
        [ -f "${pasta}vmlinuz" ] || continue
        avisar "preparando o kernel $versao"
        kernel-install add "$versao" "${pasta}vmlinuz" || falhar "o kernel-install falhou ($versao)"
    done
    rm -f /boot/grub2/grubenv
    grub2-editenv /boot/grub2/grubenv create || true
else
    update-initramfs -u -k all || falhar "o update-initramfs falhou"
fi

# ---------- 4. GRUB ----------
if [ -d /sys/firmware/efi ]; then
    if [ "$FAMILIA" = fedora ]; then
        # O GRUB UEFI do Fedora vem pronto e assinado (Secure Boot): só copiamos.
        ORIGEM=$(ls -d /usr/lib/maloki/efi/*/ 2>/dev/null | head -n1)
        [ -n "$ORIGEM" ] || falhar "a ISO foi gerada sem os arquivos de boot UEFI (shim-x64 e grub2-efi-x64)"
        PASTA=$(basename "$ORIGEM")
        mkdir -p "/boot/efi/EFI/$PASTA" /boot/efi/EFI/BOOT
        cp -r "$ORIGEM". "/boot/efi/EFI/$PASTA/"
        # Um grub.cfg "atalho" na EFI, que carrega o menu de verdade do /boot.
        if mountpoint -q /boot; then PREFIXO=/grub2; else PREFIXO=/boot/grub2; fi
        BOOT_UUID=$(findmnt -no UUID -T /boot)
        printf 'search --no-floppy --fs-uuid --set=dev %s\nset prefix=($dev)%s\nexport $prefix\nconfigfile $prefix/grub.cfg\n' \
            "$BOOT_UUID" "$PREFIXO" > "/boot/efi/EFI/$PASTA/grub.cfg"
        INICIAL=shimx64.efi
        [ -f "/boot/efi/EFI/$PASTA/$INICIAL" ] || INICIAL=grubx64.efi
        # EFI/BOOT/BOOTX64.EFI é o caminho que todo firmware UEFI procura.
        cp "/boot/efi/EFI/$PASTA/$INICIAL" /boot/efi/EFI/BOOT/BOOTX64.EFI
        cp "/boot/efi/EFI/$PASTA/grubx64.efi" /boot/efi/EFI/BOOT/grubx64.efi
        efibootmgr --create --disk "$EFI_DISCO" --part "$EFI_PARTE" --label "$NAME" \
            --loader "\\EFI\\$PASTA\\$INICIAL" >/dev/null 2>&1 \
            || avisar "não consegui criar a entrada no firmware; ele vai usar o caminho padrão EFI/BOOT"
    else
        grub-install --target=x86_64-efi --efi-directory=/boot/efi --bootloader-id="$ID" \
            || falhar "o grub-install (UEFI) falhou"
        # Uma cópia no caminho padrão, para firmwares que esquecem a entrada.
        grub-install --target=x86_64-efi --efi-directory=/boot/efi --removable --no-nvram || true
    fi
else
    if [ "$FAMILIA" = fedora ]; then GRUB_INSTALL=grub2-install; else GRUB_INSTALL=grub-install; fi
    $GRUB_INSTALL --target=i386-pc "$DISCO" || falhar "o $GRUB_INSTALL (BIOS) falhou"
fi
if [ "$FAMILIA" = fedora ]; then
    grub2-mkconfig -o /boot/grub2/grub.cfg || falhar "o grub2-mkconfig falhou"
else
    update-grub || falhar "o update-grub falhou"
fi

# ---------- 5. Arremates ----------
# SELinux: reetiquetar tudo no primeiro boot (a máquina reinicia sozinha uma vez).
if [ -f /etc/selinux/config ]; then touch /.autorelabel; fi
rm -rf /etc/calamares /usr/lib/maloki /usr/libexec/maloki
avisar "sistema pronto para ligar"
"#;

#[cfg(test)]
mod testes {
    use super::*;
    use crate::limpeza::apoio::*;

    #[test]
    fn rotulos() {
        assert_eq!(rotulo_do_volume("fedora"), "MALOKI_FEDORA");
        assert_eq!(rotulo_do_volume("opensuse-leap"), "MALOKI_OPENSUSE_LEAP");
        assert_eq!(rotulo_do_volume(&"x".repeat(50)).len(), 32);
    }

    #[test]
    fn versoes_de_kernel_comparadas_pelos_numeros() {
        assert_eq!(comparar_versoes("6.10.0-1", "6.9.12-1"), Ordering::Greater);
        assert_eq!(comparar_versoes("6.17.4-300.fc44.x86_64", "6.17.11-300.fc44.x86_64"), Ordering::Less);
        let raiz = raiz_de_teste();
        for v in ["6.9.12-200.fc44.x86_64", "6.10.3-200.fc44.x86_64"] {
            criar(&raiz, &format!("usr/lib/modules/{}/vmlinuz", v), "kernel");
        }
        criar(&raiz, "usr/lib/modules/6.99.0-sem-kernel/modules.dep", ""); // sem vmlinuz: não conta
        criar(&raiz, "boot/vmlinuz-6.1.0-13-amd64", "kernel do Debian fica no /boot");
        criar(&raiz, "usr/lib/modules/6.1.0-13-amd64/modules.dep", "");
        assert_eq!(kernel_mais_novo(&raiz).as_deref(), Some("6.10.3-200.fc44.x86_64"));
        assert!(kernel_mais_novo(&raiz_de_teste()).is_none());
    }

    #[test]
    fn autologin_do_gdm() {
        let fedora = "[daemon]\n#AutomaticLoginEnable=True\n#AutomaticLogin=daniel\nWaylandEnable=true\n\n[security]\n";
        let novo = autologin_gdm(fedora, "liveuser");
        assert_eq!(novo, "[daemon]\nAutomaticLoginEnable=True\nAutomaticLogin=liveuser\nWaylandEnable=true\n\n[security]\n");
        assert_eq!(autologin_gdm(&novo, "liveuser"), novo); // aplicar de novo não muda
        assert_eq!(autologin_gdm("", "liveuser"), "[daemon]\nAutomaticLoginEnable=True\nAutomaticLogin=liveuser\n");
        assert_eq!(autologin_gdm("[security]\n", "liveuser"), "[security]\n\n[daemon]\nAutomaticLoginEnable=True\nAutomaticLogin=liveuser\n");
    }

    #[test]
    fn o_que_falta_no_sistema() {
        let raiz = raiz_de_teste();
        let falta = o_que_falta(Familia::Fedora, &raiz);
        assert_eq!(falta.len(), 3, "{:?}", falta);
        criar(&raiz, "usr/lib/dracut/modules.d/70dmsquash-live/module-setup.sh", "");
        criar(&raiz, "usr/sbin/gdm", "");
        criar(&raiz, "usr/lib/grub/x86_64-efi/x", "");
        assert!(o_que_falta(Familia::Fedora, &raiz).is_empty());
        // O Debian usa o live-boot, não o dracut.
        assert!(o_que_falta(Familia::Debian, &raiz)[0].contains("live-boot"));
        criar(&raiz, "usr/share/initramfs-tools/scripts/live", "");
        assert!(o_que_falta(Familia::Debian, &raiz).is_empty());
        assert_eq!(modos_iso(&raiz), boot::Modos { bios: false, uefi: true });
    }

    #[test]
    fn sessao_para_o_login_automatico() {
        let raiz = raiz_de_teste();
        assert_eq!(sessao_grafica(&raiz), "");
        criar(&raiz, "usr/share/xsessions/openbox.desktop", "");
        criar(&raiz, "usr/share/xsessions/aaa.desktop", "");
        assert_eq!(sessao_grafica(&raiz), "openbox"); // preferida, mesmo não sendo a 1ª
        criar(&raiz, "usr/share/wayland-sessions/plasma.desktop", "");
        assert_eq!(sessao_grafica(&raiz), "plasma");
    }

    #[test]
    fn fuso_e_os_release() {
        assert_eq!(fuso_do_link("../usr/share/zoneinfo/America/Sao_Paulo"), Some(("America".into(), "Sao_Paulo".into())));
        assert_eq!(fuso_do_link("/usr/share/zoneinfo/America/Argentina/Cordoba"), Some(("America".into(), "Argentina/Cordoba".into())));
        assert_eq!(fuso_do_link("/usr/share/zoneinfo/UTC"), None);
        let os = "NAME=\"Fedora Linux\"\nLOGO=fedora-logo-icon\nID=fedora\n";
        assert_eq!(valor_os_release(os, "NAME").as_deref(), Some("Fedora Linux"));
        assert_eq!(valor_os_release(os, "LOGO").as_deref(), Some("fedora-logo-icon"));
        assert_eq!(valor_os_release(os, "VERSION"), None);
    }

    #[test]
    fn menu_da_iso() {
        let menu = menu_grub("Fedora \"Linux\" $x", &parametros_live(Familia::Fedora, "MALOKI_FEDORA"));
        assert!(menu.contains("menuentry \"Iniciar Fedora Linux x (sessão live)\""));
        assert!(menu.contains("root=live:CDLABEL=MALOKI_FEDORA rd.live.image"));
        assert!(menu.contains("nomodeset"));
        assert!(menu.contains("console=ttyS0,115200"));
        assert_eq!(menu.matches("menuentry").count(), 3);
        assert!(parametros_live(Familia::Debian, "X").contains("boot=live"));
    }

    #[test]
    fn compressao_do_squashfs() {
        let xz = argumentos_mksquashfs("/mnt/maloki-1", "/x/squashfs.img", Compressao::Xz).join(" ");
        assert!(xz.starts_with("/mnt/maloki-1 /x/squashfs.img -noappend -b 1M -comp xz -Xbcj x86"));
        assert!(xz.ends_with("-e boot/efi/* tmp/*"));
        let zstd = argumentos_mksquashfs("/r", "/s", Compressao::Zstd).join(" ");
        assert!(zstd.contains("-comp zstd -Xcompression-level 19"));
    }

    /// Os arquivos do Calamares são YAML: um erro de indentação e o instalador
    /// nem abre. Conferimos com um leitor de YAML de verdade (o do Python), se houver.
    #[test]
    fn configuracao_do_calamares_e_yaml_valido() {
        let mut arquivos: Vec<(String, String)> = vec![
            ("settings.conf".into(), SETTINGS.into()),
            ("branding.desc".into(), branding_desc("Fedora \"Linux\" 44", "fedora")),
        ];
        for familia in [Familia::Fedora, Familia::Debian] {
            for (nome, conteudo) in modulos_calamares(familia, &["gdm", "sddm"], &("America".into(), "Sao_Paulo".into()), 30) {
                arquivos.push((format!("{:?}-{}", familia, nome), conteudo));
            }
        }
        let tem_python = std::process::Command::new("python3").args(["-c", "import yaml"]).status().is_ok_and(|s| s.success());
        if !tem_python {
            eprintln!("(sem python3-yaml: pulando a conferência do YAML)");
            return;
        }
        for (nome, conteudo) in arquivos {
            let pasta = raiz_de_teste();
            criar(&pasta, "a.yaml", &conteudo);
            let ok = std::process::Command::new("python3")
                .args(["-I", "-c", "import sys, yaml; yaml.safe_load(open(sys.argv[1]))", &format!("{}/a.yaml", pasta)])
                .status()
                .unwrap();
            assert!(ok.success(), "{} não é YAML válido:\n{}", nome, conteudo);
        }
    }

    #[test]
    fn script_de_pos_instalacao() {
        for familia in [Familia::Fedora, Familia::Debian] {
            let script = script_pos_instalacao(familia);
            assert!(!script.contains('@'), "sobrou um @MARCADOR@ no script");
            assert!(script.contains("userdel -r liveuser"));
            // `sh -n` só confere a sintaxe, sem rodar nada.
            let pasta = raiz_de_teste();
            criar(&pasta, "pos", &script);
            let ok = std::process::Command::new("sh").args(["-n", &format!("{}/pos", pasta)]).status().unwrap();
            assert!(ok.success(), "erro de sintaxe no script de {:?}", familia);
        }
        assert!(script_pos_instalacao(Familia::Fedora).contains("FAMILIA=fedora"));
    }

    #[test]
    fn pasta_conteudo_nao_reaproveita_nem_sobra() {
        let base = raiz_de_teste();
        let caminho = format!("{}/x.iso.conteudo", base);
        let exec = Executor { simular: false };
        {
            let pasta = PastaConteudo::criar(&exec, &caminho).unwrap();
            criar(&pasta.caminho, "boot/vmlinuz", "kernel");
            assert!(PastaConteudo::criar(&exec, &caminho).is_err()); // já existe: recusa
        }
        assert!(!Path::new(&caminho).exists()); // o Drop apagou
    }
}
