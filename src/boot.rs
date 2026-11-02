// Faz o sistema copiado conseguir ligar dentro do QEMU.
//
// O sistema original sabe ligar no SEU computador: ele procura o seu disco
// (pelo UUID), destrava a criptografia (LUKS), usa btrfs... Nada disso existe
// na VM. Então precisamos:
//   1. escrever um /etc/fstab novo, com os UUIDs do disco virtual;
//   2. limpar a "linha de comando do kernel" (os parâmetros de boot);
//   3. gerar um initramfs genérico, que tenha os drivers das VMs (virtio);
//   4. configurar o GRUB para achar o disco novo.

use crate::distro::Familia;
use crate::executor::Executor;
use crate::limpeza::na_imagem;
use crate::Resultado;
use std::fs;
use std::path::Path;

/// Em quais modos de firmware a imagem vai conseguir ligar.
/// BIOS = computadores e VMs "antigos"; UEFI = o padrão atual.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Modos {
    pub bios: bool,
    pub uefi: bool,
}

impl Modos {
    pub fn descricao(&self) -> &'static str {
        match (self.bios, self.uefi) {
            (true, true) => "BIOS e UEFI",
            (true, false) => "só BIOS",
            (false, true) => "só UEFI",
            (false, false) => "nenhum",
        }
    }
}

/// Descobre o que o sistema tem instalado para cada modo de boot.
/// `raiz` é "/" no programa de verdade (nos testes, uma pasta falsa).
pub fn detectar_modos(familia: Familia, raiz: &str) -> Modos {
    let existe = |caminho: &str| Path::new(&format!("{}/{}", raiz.trim_end_matches('/'), caminho)).exists();

    // Os módulos do GRUB para BIOS ficam em /usr/lib/grub/i386-pc
    // (pacote grub2-pc-modules no Fedora, grub-pc-bin no Debian).
    let bios = existe("usr/lib/grub/i386-pc");

    let uefi = match familia {
        // No Fedora o GRUB UEFI pronto fica na partição EFI (pacote grub2-efi-x64).
        Familia::Fedora => achar_pasta_grub_efi(&format!("{}/boot/efi/EFI", raiz.trim_end_matches('/'))).is_some(),
        // No Debian o grub-install monta o GRUB UEFI a partir destes módulos.
        Familia::Debian => existe("usr/lib/grub/x86_64-efi"),
    };
    Modos { bios, uefi }
}

/// Os "RG" das 3 partições que têm sistema de arquivos. É assim (pelo UUID)
/// que o fstab e o GRUB encontram cada partição no boot.
pub struct Uuids {
    pub raiz: String,
    pub boot: String,
    pub efi: String,
}

/// `disco` é o dispositivo inteiro (ex.: /dev/loop0), onde o GRUB BIOS é gravado.
pub fn configurar(exec: &Executor, raiz: &str, familia: Familia, uuids: &Uuids, disco: &str, modos: Modos) -> Resultado<()> {
    ajustar_arquivos(exec, raiz, familia, uuids, modos.uefi)?;

    // `chroot` = rodar um comando "como se" a pasta da imagem fosse a raiz (/).
    // Assim o dracut/grub da imagem trabalham na imagem, não no seu sistema.
    match familia {
        Familia::Fedora => {
            // --no-hostonly: inclui drivers de todo tipo de máquina (inclusive virtio),
            // não só os do seu computador.
            exec.rodar("chroot", &[raiz, "dracut", "--force", "--no-hostonly", "--regenerate-all"])?;
            if modos.bios {
                // Grava o GRUB de BIOS no começo do disco e na partição "BIOS boot".
                exec.rodar("chroot", &[raiz, "grub2-install", "--target=i386-pc", disco])?;
            }
            // Um único menu de boot, usado pelos dois modos.
            exec.rodar("chroot", &[raiz, "grub2-mkconfig", "-o", "/boot/grub2/grub.cfg"])?;
        }
        Familia::Debian => {
            // Liga o serviço que recria as chaves SSH no primeiro boot.
            exec.rodar("chroot", &[raiz, "systemctl", "enable", "gerar-chaves-ssh.service"])?;
            // O Debian já gera o initramfs com os drivers mais comuns (MODULES=most).
            exec.rodar("chroot", &[raiz, "update-initramfs", "-u", "-k", "all"])?;
            if modos.bios {
                exec.rodar("chroot", &[raiz, "grub-install", "--target=i386-pc", disco])?;
            }
            if modos.uefi {
                // --removable instala em EFI/BOOT/BOOTX64.EFI, o caminho que todo
                // firmware UEFI procura; --no-nvram não mexe no firmware do seu PC.
                exec.rodar("chroot", &[
                    raiz, "grub-install", "--target=x86_64-efi", "--efi-directory=/boot/efi",
                    "--removable", "--no-nvram",
                ])?;
            }
            exec.rodar("chroot", &[raiz, "update-grub"])?;
        }
    }
    Ok(())
}

/// A parte que só mexe em arquivos (sem chroot): dá para testar sem ser root.
fn ajustar_arquivos(exec: &Executor, raiz: &str, familia: Familia, uuids: &Uuids, uefi: bool) -> Resultado<()> {
    exec.escrever(&na_imagem(raiz, "etc/fstab"), &fstab(uuids))?;

    // Parâmetros padrão do GRUB, usados quando ele gera o menu de boot.
    exec.editar(&na_imagem(raiz, "etc/default/grub"), ajustar_default_grub)?;

    match familia {
        Familia::Fedora => {
            // No Fedora cada kernel tem um arquivo em /boot/loader/entries (BLS)
            // com a linha "options ..." contendo os parâmetros de boot.
            // O /etc/kernel/cmdline é usado quando um kernel novo for instalado.
            let uuid = uuids.raiz.clone();
            exec.editar(&na_imagem(raiz, "etc/kernel/cmdline"), |texto| nova_cmdline(&uuid, texto) + "\n")?;
            for entrada in arquivos_bls(exec, raiz) {
                exec.editar(&entrada, |texto| ajustar_bls(texto, &uuid))?;
            }
            if uefi {
                preparar_efi_fedora(exec, raiz, &uuids.boot)?;
            }
        }
        Familia::Debian => {
            // Debian/Ubuntu guardam aqui qual swap usar para voltar da hibernação.
            // A swap do seu PC não existe na VM: sem isto o boot fica uns 30s
            // parado esperando por ela.
            exec.escrever(&na_imagem(raiz, "etc/initramfs-tools/conf.d/resume"), "RESUME=none\n")?;

            // A limpeza apagou as chaves do servidor SSH (cada máquina precisa das suas).
            // O Fedora recria sozinho no boot; o Debian/Ubuntu NÃO, e aí o SSH nem liga.
            // Este serviço do systemd recria as chaves no primeiro boot.
            exec.escrever(&na_imagem(raiz, "etc/systemd/system/gerar-chaves-ssh.service"), SERVICO_CHAVES_SSH)?;
        }
    }
    Ok(())
}

// Um "unit file" do systemd. Lendo de cima para baixo:
//   Before=        roda ANTES do servidor SSH
//   ConditionPathExists=!...  só roda se a chave NÃO existir (o "!" é "não")
//   ExecStart=     o comando: ssh-keygen -A cria todas as chaves que faltam
//   WantedBy=      liga junto com o sistema
pub const SERVICO_CHAVES_SSH: &str = "\
[Unit]
Description=Gera as chaves do servidor SSH no primeiro boot (maloki)
Before=ssh.service ssh.socket
ConditionPathExists=!/etc/ssh/ssh_host_ed25519_key

[Service]
Type=oneshot
ExecStart=/usr/bin/ssh-keygen -A

[Install]
WantedBy=multi-user.target
";

/// O /etc/fstab diz ao Linux quais partições montar e onde.
pub fn fstab(uuids: &Uuids) -> String {
    format!(
        "# Gerado pelo maloki\n\
         UUID={}  /          ext4  defaults                    1 1\n\
         UUID={}  /boot      ext4  defaults                    1 2\n\
         UUID={}  /boot/efi  vfat  umask=0077,shortname=winnt  0 2\n",
        uuids.raiz, uuids.boot, uuids.efi
    )
}

/// Tira da lista os parâmetros que só fazem sentido no computador original.
fn limpar_parametros(texto: &str) -> Vec<String> {
    filtrar_parametros(texto, true)
}

/// Tira os parâmetros que só valem na máquina original. Se `tirar_console`
/// for false, os `console=` ficam (usado no GRUB_CMDLINE_LINUX_DEFAULT).
fn filtrar_parametros(texto: &str, tirar_console: bool) -> Vec<String> {
    let especificos = ["root=", "rootflags=", "rd.luks", "rd.lvm", "rd.md", "resume="];
    texto
        .split_whitespace()
        .filter(|p| *p != "ro" && *p != "rw")
        .filter(|p| !especificos.iter().any(|e| p.starts_with(e)))
        .filter(|p| !(tirar_console && p.starts_with("console=")))
        .map(|p| p.to_string())
        .collect()
}

/// Monta a linha de boot nova: aponta para o disco virtual e liga o console serial
/// (assim dá para ver o boot até com `qemu -nographic`).
pub fn nova_cmdline(uuid_raiz: &str, antiga: &str) -> String {
    let mut partes = vec![format!("root=UUID={}", uuid_raiz), "ro".to_string()];
    partes.extend(limpar_parametros(antiga));
    partes.push("console=tty0".to_string());
    partes.push("console=ttyS0,115200".to_string());
    partes.join(" ")
}

/// Ajusta os parâmetros padrão do GRUB no /etc/default/grub.
///
/// Cada distro usa uma das duas variáveis (ou as duas):
///   GRUB_CMDLINE_LINUX         -> Rocky, Debian, o Fedora desktop
///   GRUB_CMDLINE_LINUX_DEFAULT -> o Fedora 44 "cloud" guarda tudo aqui
/// Nas duas tiramos o que é da máquina original (rootflags, rd.luks...).
/// Na primeira também trocamos os consoles; na _DEFAULT os consoles ficam,
/// porque ali eles são escolha da distro, não da máquina.
fn ajustar_default_grub(texto: &str) -> String {
    filtrar_default_grub(texto, true)
}

/// Igual, mas sem ligar o console serial: para a ISO, que é instalada em
/// computadores de verdade (lá o console é a tela, não uma porta serial).
pub fn limpar_default_grub(texto: &str) -> String {
    filtrar_default_grub(texto, false)
}

fn filtrar_default_grub(texto: &str, console_serial: bool) -> String {
    let mut saida = String::new();
    for linha in texto.lines() {
        // O valor pode vir com aspas duplas ou simples.
        let valor_de = |v: &str| v.trim().trim_matches(['"', '\'']).to_string();
        if let Some(valor) = linha.strip_prefix("GRUB_CMDLINE_LINUX=") {
            let mut partes = limpar_parametros(&valor_de(valor));
            if console_serial {
                partes.push("console=tty0".into());
                partes.push("console=ttyS0,115200".into());
            }
            saida.push_str(&format!("GRUB_CMDLINE_LINUX=\"{}\"\n", partes.join(" ")));
        } else if let Some(valor) = linha.strip_prefix("GRUB_CMDLINE_LINUX_DEFAULT=") {
            let partes = filtrar_parametros(&valor_de(valor), false);
            saida.push_str(&format!("GRUB_CMDLINE_LINUX_DEFAULT=\"{}\"\n", partes.join(" ")));
        } else {
            saida.push_str(linha);
            saida.push('\n');
        }
    }
    saida
}

/// Troca a linha "options ..." de uma entrada BLS do Fedora.
fn ajustar_bls(texto: &str, uuid_raiz: &str) -> String {
    let mut saida = String::new();
    for linha in texto.lines() {
        // split_once separa a primeira palavra (o "comando") do resto da linha.
        let (comando, resto) = linha.split_once(' ').unwrap_or((linha, ""));
        let nova = match comando {
            "options" => format!("options {}", nova_cmdline(uuid_raiz, resto)),
            "linux" | "initrd" | "devicetree" => format!("{} {}", comando, sem_prefixo_boot(resto)),
            _ => linha.to_string(),
        };
        saida.push_str(&nova);
        saida.push('\n');
    }
    saida
}

/// Na imagem o /boot é uma partição SEPARADA, então o GRUB enxerga o kernel
/// na "raiz" dela: o caminho certo é "/vmlinuz-...", não "/boot/vmlinuz-...".
/// Sistemas que tinham o /boot junto da raiz escrevem "/boot/vmlinuz"; aqui
/// tiramos esse "/boot". (É a regra do "BOOTPREFIX" do kernel-install do Fedora.)
fn sem_prefixo_boot(arquivos: &str) -> String {
    arquivos
        .split_whitespace()
        // strip_prefix devolve Some(resto) se começar com "/boot/", e None se não.
        .map(|arquivo| match arquivo.strip_prefix("/boot/") {
            Some(resto) => format!("/{}", resto),
            None => arquivo.to_string(), // "/vmlinuz", "$tuned_initrd"... ficam iguais
        })
        .collect::<Vec<String>>()
        .join(" ")
}

fn arquivos_bls(exec: &Executor, raiz: &str) -> Vec<String> {
    let pasta = na_imagem(raiz, "boot/loader/entries");
    if exec.simular {
        return vec![format!("{}/*.conf", pasta)];
    }
    let mut lista = Vec::new();
    if let Ok(entradas) = fs::read_dir(&pasta) {
        for entrada in entradas.flatten() {
            let caminho = entrada.path().to_string_lossy().to_string();
            if caminho.ends_with(".conf") {
                lista.push(caminho);
            }
        }
    }
    lista
}

/// Procura a pasta da distro (ex.: EFI/fedora ou EFI/rocky) que tem o grubx64.efi.
pub fn achar_pasta_grub_efi(pasta_efi: &str) -> Option<std::path::PathBuf> {
    let entradas = fs::read_dir(pasta_efi).ok()?; // `?` num Option: se for None, devolve None
    for entrada in entradas.flatten() {
        let nome = entrada.file_name().to_string_lossy().to_string();
        if nome != "BOOT" && entrada.path().join("grubx64.efi").exists() {
            return Some(entrada.path());
        }
    }
    None
}

/// No Fedora, o GRUB da partição EFI é só um "atalho": um grub.cfg pequeno que
/// procura a partição /boot pelo UUID e carrega o menu de verdade de lá.
fn preparar_efi_fedora(exec: &Executor, raiz: &str, uuid_boot: &str) -> Resultado<()> {
    // O menu de verdade fica na partição /boot, em /grub2/grub.cfg.
    let atalho = format!(
        "search --no-floppy --fs-uuid --set=dev {uuid_boot}\n\
         set prefix=($dev)/grub2\n\
         export $prefix\n\
         configfile $prefix/grub.cfg\n"
    );

    let pasta_efi = na_imagem(raiz, "boot/efi/EFI");
    if exec.simular {
        exec.escrever(&format!("{}/fedora/grub.cfg", pasta_efi), &atalho)?;
        println!("[simulação] garantiria {}/BOOT/BOOTX64.EFI (caminho padrão do UEFI)", pasta_efi);
        return Ok(());
    }

    let Some(pasta_distro) = achar_pasta_grub_efi(&pasta_efi) else {
        return Err("não achei o grubx64.efi na partição EFI (o pacote grub2-efi-x64 está instalado?)".into());
    };
    exec.escrever(&pasta_distro.join("grub.cfg").to_string_lossy(), &atalho)?;

    // Sem nada configurado, o firmware UEFI da VM procura EFI/BOOT/BOOTX64.EFI.
    // Colocamos lá o shim (que carrega o grub), ou o próprio grub se não houver shim.
    let boot = Path::new(&pasta_efi).join("BOOT");
    fs::create_dir_all(&boot)?;
    let shim = pasta_distro.join("shimx64.efi");
    let grub = pasta_distro.join("grubx64.efi");
    let inicial = if shim.exists() { &shim } else { &grub };
    if !boot.join("BOOTX64.EFI").exists() {
        fs::copy(inicial, boot.join("BOOTX64.EFI"))?;
    }
    fs::copy(&grub, boot.join("grubx64.efi"))?;
    Ok(())
}

#[cfg(test)]
mod testes {
    use super::*;

    // Esta é exatamente a linha de boot do seu Fedora (LUKS + btrfs).
    const SUA_CMDLINE: &str = "root=UUID=1b116ad6 ro rootflags=subvol=root rd.luks.uuid=luks-9da4 rhgb quiet";

    #[test]
    fn linha_de_boot_sem_luks_nem_btrfs() {
        assert_eq!(
            nova_cmdline("NOVO", SUA_CMDLINE),
            "root=UUID=NOVO ro rhgb quiet console=tty0 console=ttyS0,115200"
        );
    }

    #[test]
    fn ajusta_default_grub() {
        let texto = "GRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX=\"rd.luks.uuid=luks-9da4 rhgb quiet\"\n";
        assert_eq!(
            ajustar_default_grub(texto),
            "GRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX=\"rhgb quiet console=tty0 console=ttyS0,115200\"\n"
        );
    }

    #[test]
    fn ajusta_entrada_bls() {
        let texto = format!("title Fedora\nlinux /vmlinuz-6.17\noptions {}\n", SUA_CMDLINE);
        let novo = ajustar_bls(&texto, "NOVO");
        assert!(novo.contains("options root=UUID=NOVO ro rhgb quiet"));
        assert!(novo.contains("linux /vmlinuz-6.17\n")); // relativo ao /boot separado
        assert!(!novo.contains("luks"));
    }

    #[test]
    fn fstab_tem_as_tres_particoes() {
        let f = fstab(&uuids_de_teste());
        assert!(f.contains("UUID=RAIZ  /          ext4"));
        assert!(f.contains("UUID=BOOT  /boot      ext4"));
        assert!(f.contains("UUID=EFI  /boot/efi  vfat"));
        // Exatamente três linhas de montagem (a outra é comentário).
        assert_eq!(f.lines().filter(|l| l.starts_with("UUID=")).count(), 3);
        // A ordem importa: /boot antes de /boot/efi (que fica DENTRO dele).
        assert!(f.find("/boot ").unwrap() < f.find("/boot/efi").unwrap());
    }

    #[test]
    fn remove_todo_tipo_de_parametro_do_pc_original() {
        let antiga = "root=/dev/mapper/fedora-root rw rootflags=subvol=@ rd.luks.name=abc=cryptroot \
                      rd.lvm.lv=fedora/root rd.lvm.lv=fedora/swap rd.md.uuid=1234 resume=UUID=5678 \
                      console=ttyS1,9600 mitigations=off selinux=1 nomodeset";
        assert_eq!(
            nova_cmdline("U", antiga),
            "root=UUID=U ro mitigations=off selinux=1 nomodeset console=tty0 console=ttyS0,115200"
        );
    }

    #[test]
    fn linha_vazia_ou_so_com_espacos() {
        assert_eq!(nova_cmdline("U", ""), "root=UUID=U ro console=tty0 console=ttyS0,115200");
        assert_eq!(nova_cmdline("U", "   \n\t "), "root=UUID=U ro console=tty0 console=ttyS0,115200");
    }

    #[test]
    fn default_grub_variacoes() {
        // Aspas simples
        assert_eq!(
            ajustar_default_grub("GRUB_CMDLINE_LINUX='rd.luks.uuid=x quiet'\n"),
            "GRUB_CMDLINE_LINUX=\"quiet console=tty0 console=ttyS0,115200\"\n"
        );
        // Vazio (padrão do Debian/Ubuntu)
        assert_eq!(
            ajustar_default_grub("GRUB_CMDLINE_LINUX=\"\"\n"),
            "GRUB_CMDLINE_LINUX=\"console=tty0 console=ttyS0,115200\"\n"
        );
        // _DEFAULT e linhas comentadas não são tocadas
        let intocado = "GRUB_CMDLINE_LINUX_DEFAULT=\"quiet splash\"\n#GRUB_CMDLINE_LINUX=\"rd.luks.uuid=x\"\nGRUB_TIMEOUT=5\n";
        assert_eq!(ajustar_default_grub(intocado), intocado);
        // Sem a linha: nada muda
        assert_eq!(ajustar_default_grub("GRUB_TIMEOUT=5\n"), "GRUB_TIMEOUT=5\n");
    }

    #[test]
    fn default_grub_do_fedora_44_cloud() {
        // O /etc/default/grub REAL do Fedora 44 "cloud": tudo fica no _DEFAULT.
        let fedora = "GRUB_DEFAULT=saved\n\
                      GRUB_CMDLINE_LINUX_DEFAULT=\"no_timer_check console=tty1 console=ttyS0,115200n8 systemd.firstboot=off rootflags=subvol=root\"\n\
                      GRUB_ENABLE_BLSCFG=true\n";
        let novo = ajustar_default_grub(fedora);
        // O rootflags (btrfs) some; os consoles da distro ficam.
        assert!(novo.contains("GRUB_CMDLINE_LINUX_DEFAULT=\"no_timer_check console=tty1 console=ttyS0,115200n8 systemd.firstboot=off\"\n"));
        assert!(!novo.contains("rootflags"));
        assert!(novo.contains("GRUB_ENABLE_BLSCFG=true\n"));
        assert_eq!(ajustar_default_grub(&novo), novo); // aplicar de novo não muda
    }

    #[test]
    fn default_grub_duas_vezes_da_o_mesmo() {
        let uma = ajustar_default_grub("GRUB_CMDLINE_LINUX=\"rd.luks.uuid=x rhgb quiet\"\n");
        assert_eq!(ajustar_default_grub(&uma), uma);
    }

    #[test]
    fn bls_so_muda_a_linha_options() {
        let texto = "title Fedora Linux (6.17.1-300.fc44.x86_64) 44\n\
                     version 6.17.1-300.fc44.x86_64\n\
                     linux /vmlinuz-6.17.1-300.fc44.x86_64\n\
                     initrd /initramfs-6.17.1-300.fc44.x86_64.img $tuned_initrd\n\
                     options root=UUID=velho ro rootflags=subvol=root rd.luks.uuid=luks-1 rhgb quiet\n\
                     grub_users $grub_users\n\
                     grub_arg --unrestricted\n\
                     grub_class fedora\n";
        let novo = ajustar_bls(texto, "NOVO");
        let linhas_antigas: Vec<&str> = texto.lines().collect();
        let linhas_novas: Vec<&str> = novo.lines().collect();
        assert_eq!(linhas_antigas.len(), linhas_novas.len());
        for (antiga, nova) in linhas_antigas.iter().zip(&linhas_novas) {
            if antiga.starts_with("options ") {
                assert_eq!(*nova, "options root=UUID=NOVO ro rhgb quiet console=tty0 console=ttyS0,115200");
            } else {
                assert_eq!(antiga, nova);
            }
        }
        assert_eq!(ajustar_bls(&novo, "NOVO"), novo); // aplicar de novo não muda
    }

    #[test]
    fn caminho_do_kernel_fica_relativo_ao_boot() {
        // Entrada REAL do Rocky 9 (que já tinha /boot separado): não muda.
        let rocky = "linux /vmlinuz-5.14.0-687.10.1.el9_8.0.1.x86_64\n\
                     initrd /initramfs-5.14.0-687.10.1.el9_8.0.1.x86_64.img $tuned_initrd\n\
                     options root=UUID=velho ro console=ttyS0,115200n8 no_timer_check net.ifnames=0\n\
                     grub_users $grub_users\n";
        let novo = ajustar_bls(rocky, "U");
        assert!(novo.contains("linux /vmlinuz-5.14.0-687.10.1.el9_8.0.1.x86_64\n"));
        assert!(novo.contains("initrd /initramfs-5.14.0-687.10.1.el9_8.0.1.x86_64.img $tuned_initrd\n"));
        assert!(novo.contains("grub_users $grub_users\n"));
        assert!(novo.contains("options root=UUID=U ro no_timer_check net.ifnames=0 console=tty0 console=ttyS0,115200"));

        // Sistema com /boot junto da raiz: perde o "/boot".
        let junto = "linux /boot/vmlinuz-6.17\ninitrd /boot/initramfs-6.17.img\n";
        assert_eq!(ajustar_bls(junto, "U"), "linux /vmlinuz-6.17\ninitrd /initramfs-6.17.img\n");
    }

    #[test]
    fn tirar_o_prefixo_boot() {
        assert_eq!(sem_prefixo_boot("/boot/x /y $var"), "/x /y $var");
        assert_eq!(sem_prefixo_boot(""), "");
        // "/bootloader.img" não está DENTRO de /boot: fica igual.
        assert_eq!(sem_prefixo_boot("/bootloader.img"), "/bootloader.img");
        assert_eq!(sem_prefixo_boot("/boot/efi/x"), "/efi/x");
    }

    #[test]
    fn bls_aplicar_varias_vezes_da_o_mesmo() {
        let texto = "linux /boot/vmlinuz-1\ninitrd /initramfs-1.img\noptions rd.luks.uuid=x quiet\n";
        let uma = ajustar_bls(texto, "U");
        assert_eq!(ajustar_bls(&uma, "U"), uma);
        assert!(!uma.contains("/boot/"));
    }

    // ---------- Testes com arquivos de verdade ----------
    use crate::limpeza::apoio::*;

    fn uuids_de_teste() -> Uuids {
        Uuids { raiz: "RAIZ".into(), boot: "BOOT".into(), efi: "EFI".into() }
    }
    const DE_VERDADE: Executor = Executor { simular: false };

    fn fedora_falso() -> PastaTeste {
        let raiz = raiz_de_teste();
        criar(&raiz, "etc/fstab", "UUID=velho / btrfs subvol=root 0 0\n/dev/mapper/luks /home btrfs subvol=home 0 0\n");
        criar(&raiz, "etc/default/grub", "GRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX=\"rd.luks.uuid=luks-1 rhgb quiet\"\n");
        criar(&raiz, "etc/kernel/cmdline", &format!("{}\n", SUA_CMDLINE));
        criar(&raiz, "boot/loader/entries/abc-6.17.conf", &format!("title A\nlinux /vmlinuz-6.17\noptions {}\n", SUA_CMDLINE));
        criar(&raiz, "boot/loader/entries/abc-6.16.conf", &format!("title B\nlinux /vmlinuz-6.16\noptions {}\n", SUA_CMDLINE));
        criar(&raiz, "boot/loader/entries/LEIA-ME.txt", "não é .conf: não mexer rd.luks");
        criar(&raiz, "boot/efi/EFI/fedora/shimx64.efi", "SHIM");
        criar(&raiz, "boot/efi/EFI/fedora/grubx64.efi", "GRUB");
        raiz
    }

    #[test]
    fn fedora_arquivos_de_boot_ficam_certos() {
        let raiz = fedora_falso();
        ajustar_arquivos(&DE_VERDADE, &raiz, Familia::Fedora, &uuids_de_teste(), true).unwrap();

        let fstab = ler(&raiz, "etc/fstab");
        assert!(fstab.contains("UUID=RAIZ  /          ext4"));
        assert!(fstab.contains("UUID=BOOT  /boot      ext4"));
        assert!(fstab.contains("UUID=EFI  /boot/efi  vfat"));
        assert!(!fstab.contains("btrfs") && !fstab.contains("/home"));

        assert_eq!(
            ler(&raiz, "etc/kernel/cmdline"),
            "root=UUID=RAIZ ro rhgb quiet console=tty0 console=ttyS0,115200\n"
        );
        for entrada in ["abc-6.17.conf", "abc-6.16.conf"] {
            let texto = ler(&raiz, &format!("boot/loader/entries/{}", entrada));
            assert!(texto.contains("options root=UUID=RAIZ ro rhgb quiet"), "{}", entrada);
            assert!(!texto.contains("luks") && !texto.contains("subvol"), "{}", entrada);
        }
        assert!(ler(&raiz, "boot/loader/entries/LEIA-ME.txt").contains("rd.luks"));
        assert!(!ler(&raiz, "etc/default/grub").contains("luks"));

        // EFI: atalho do grub com o UUID novo + caminho padrão do UEFI
        let atalho = ler(&raiz, "boot/efi/EFI/fedora/grub.cfg");
        assert!(atalho.contains("--set=dev BOOT")); // procura a partição /boot
        assert!(atalho.contains("set prefix=($dev)/grub2"));
        assert_eq!(ler(&raiz, "boot/efi/EFI/BOOT/BOOTX64.EFI"), "SHIM");
        assert_eq!(ler(&raiz, "boot/efi/EFI/BOOT/grubx64.efi"), "GRUB");

        // Coisas do Debian não entram no Fedora (ele já recria as chaves SSH sozinho)
        assert!(!existe(&raiz, "etc/initramfs-tools/conf.d/resume"));
        assert!(!existe(&raiz, "etc/systemd/system/gerar-chaves-ssh.service"));
    }

    #[test]
    fn fedora_ajustar_duas_vezes_da_o_mesmo() {
        let raiz = fedora_falso();
        ajustar_arquivos(&DE_VERDADE, &raiz, Familia::Fedora, &uuids_de_teste(), true).unwrap();
        let antes = ler(&raiz, "boot/loader/entries/abc-6.17.conf");
        ajustar_arquivos(&DE_VERDADE, &raiz, Familia::Fedora, &uuids_de_teste(), true).unwrap();
        assert_eq!(ler(&raiz, "boot/loader/entries/abc-6.17.conf"), antes);
    }

    #[test]
    fn efi_sem_shim_usa_o_grub() {
        let raiz = raiz_de_teste();
        criar(&raiz, "boot/efi/EFI/fedora/grubx64.efi", "GRUB");
        preparar_efi_fedora(&DE_VERDADE, &raiz, "U").unwrap();
        assert_eq!(ler(&raiz, "boot/efi/EFI/BOOT/BOOTX64.EFI"), "GRUB");
    }

    #[test]
    fn efi_nao_sobrescreve_bootx64_existente() {
        let raiz = raiz_de_teste();
        criar(&raiz, "boot/efi/EFI/fedora/shimx64.efi", "SHIM");
        criar(&raiz, "boot/efi/EFI/fedora/grubx64.efi", "GRUB");
        criar(&raiz, "boot/efi/EFI/BOOT/BOOTX64.EFI", "SHIM-ORIGINAL-DO-PACOTE");
        preparar_efi_fedora(&DE_VERDADE, &raiz, "U").unwrap();
        assert_eq!(ler(&raiz, "boot/efi/EFI/BOOT/BOOTX64.EFI"), "SHIM-ORIGINAL-DO-PACOTE");
    }

    #[test]
    fn efi_sem_grub_da_erro_claro() {
        let raiz = raiz_de_teste();
        criar(&raiz, "boot/efi/EFI/BOOT/BOOTX64.EFI", "x");
        let erro = preparar_efi_fedora(&DE_VERDADE, &raiz, "U").unwrap_err().to_string();
        assert!(erro.contains("grubx64.efi"));
    }

    #[test]
    fn debian_arquivos_de_boot_ficam_certos() {
        let raiz = raiz_de_teste();
        criar(&raiz, "etc/default/grub", "GRUB_DEFAULT=0\nGRUB_CMDLINE_LINUX_DEFAULT=\"quiet splash\"\nGRUB_CMDLINE_LINUX=\"\"\n");
        criar(&raiz, "etc/initramfs-tools/conf.d/resume", "RESUME=UUID=minha-swap\n");
        ajustar_arquivos(&DE_VERDADE, &raiz, Familia::Debian, &uuids_de_teste(), true).unwrap();

        assert_eq!(ler(&raiz, "etc/initramfs-tools/conf.d/resume"), "RESUME=none\n");
        // Serviço que recria as chaves SSH (o Debian não faz isso sozinho).
        let servico = ler(&raiz, "etc/systemd/system/gerar-chaves-ssh.service");
        assert!(servico.contains("ExecStart=/usr/bin/ssh-keygen -A"));
        assert!(servico.contains("Before=ssh.service"));
        let grub = ler(&raiz, "etc/default/grub");
        assert!(grub.contains("GRUB_CMDLINE_LINUX_DEFAULT=\"quiet splash\""));
        assert!(grub.contains("GRUB_CMDLINE_LINUX=\"console=tty0 console=ttyS0,115200\""));
        assert!(ler(&raiz, "etc/fstab").contains("UUID=RAIZ  /"));
        // O Debian instala o grub com grub-install: não criamos atalho EFI.
        assert!(!existe(&raiz, "boot/efi/EFI"));
    }

    #[test]
    fn debian_sem_pasta_do_initramfs_cria_ela() {
        let raiz = raiz_de_teste();
        ajustar_arquivos(&DE_VERDADE, &raiz, Familia::Debian, &uuids_de_teste(), true).unwrap();
        assert_eq!(ler(&raiz, "etc/initramfs-tools/conf.d/resume"), "RESUME=none\n");
    }

    #[test]
    fn detecta_modos_de_boot() {
        // Fedora com tudo
        let raiz = raiz_de_teste();
        criar(&raiz, "usr/lib/grub/i386-pc/normal.mod", "x");
        criar(&raiz, "boot/efi/EFI/fedora/grubx64.efi", "x");
        assert_eq!(detectar_modos(Familia::Fedora, &raiz), Modos { bios: true, uefi: true });
        // O mesmo sistema visto como Debian: falta o x86_64-efi
        assert_eq!(detectar_modos(Familia::Debian, &raiz), Modos { bios: true, uefi: false });

        // Rocky só UEFI (o grubx64 fica em EFI/rocky)
        let raiz = raiz_de_teste();
        criar(&raiz, "boot/efi/EFI/rocky/grubx64.efi", "x");
        criar(&raiz, "boot/efi/EFI/BOOT/BOOTX64.EFI", "x");
        assert_eq!(detectar_modos(Familia::Fedora, &raiz), Modos { bios: false, uefi: true });

        // Só o EFI/BOOT não conta: precisa do grubx64.efi da distro
        let raiz = raiz_de_teste();
        criar(&raiz, "boot/efi/EFI/BOOT/grubx64.efi", "x");
        assert_eq!(detectar_modos(Familia::Fedora, &raiz), Modos { bios: false, uefi: false });

        // Debian com os dois pacotes de módulos
        let raiz = raiz_de_teste();
        criar(&raiz, "usr/lib/grub/i386-pc/normal.mod", "x");
        criar(&raiz, "usr/lib/grub/x86_64-efi/normal.mod", "x");
        assert_eq!(detectar_modos(Familia::Debian, &raiz), Modos { bios: true, uefi: true });

        // Sistema sem nada
        let raiz = raiz_de_teste();
        assert_eq!(detectar_modos(Familia::Debian, &raiz), Modos { bios: false, uefi: false });
    }

    #[test]
    fn descricao_dos_modos() {
        assert_eq!(Modos { bios: true, uefi: true }.descricao(), "BIOS e UEFI");
        assert_eq!(Modos { bios: true, uefi: false }.descricao(), "só BIOS");
        assert_eq!(Modos { bios: false, uefi: true }.descricao(), "só UEFI");
        assert_eq!(Modos { bios: false, uefi: false }.descricao(), "nenhum");
    }

    #[test]
    fn fedora_so_bios_nao_mexe_na_efi() {
        let raiz = fedora_falso();
        ajustar_arquivos(&DE_VERDADE, &raiz, Familia::Fedora, &uuids_de_teste(), false).unwrap();
        assert!(!existe(&raiz, "boot/efi/EFI/fedora/grub.cfg"));
        assert!(!existe(&raiz, "boot/efi/EFI/BOOT"));
        // Mas o resto do boot é ajustado normalmente.
        assert!(ler(&raiz, "boot/loader/entries/abc-6.17.conf").contains("root=UUID=RAIZ"));
    }

    #[test]
    fn fedora_so_bios_sem_grub_efi_nao_da_erro() {
        // Uma máquina BIOS pura não tem grubx64.efi: não pode falhar.
        let raiz = raiz_de_teste();
        criar(&raiz, "etc/default/grub", "GRUB_CMDLINE_LINUX=\"quiet\"\n");
        ajustar_arquivos(&DE_VERDADE, &raiz, Familia::Fedora, &uuids_de_teste(), false).unwrap();
    }

    #[test]
    fn configurar_em_simulacao_todas_as_combinacoes() {
        let exec = Executor { simular: true };
        for familia in [Familia::Fedora, Familia::Debian] {
            for (bios, uefi) in [(true, true), (true, false), (false, true)] {
                configurar(&exec, "/mnt/maloki-1", familia, &uuids_de_teste(), "/dev/loop0", Modos { bios, uefi }).unwrap();
            }
        }
    }

    #[test]
    fn servico_de_chaves_ssh_e_um_unit_valido() {
        // Confere a "gramática" básica de um unit file do systemd.
        let mut secoes = Vec::new();
        for linha in SERVICO_CHAVES_SSH.lines().filter(|l| !l.trim().is_empty()) {
            if linha.starts_with('[') {
                assert!(linha.ends_with(']'), "{}", linha);
                secoes.push(linha);
            } else {
                assert!(linha.contains('='), "linha sem '=': {}", linha);
            }
        }
        assert_eq!(secoes, ["[Unit]", "[Service]", "[Install]"]);
        // Se o systemd-analyze existir neste PC, ele confere o arquivo de verdade.
        let raiz = raiz_de_teste();
        criar(&raiz, "gerar-chaves-ssh.service", SERVICO_CHAVES_SSH);
        if let Ok(saida) = std::process::Command::new("systemd-analyze")
            .args(["verify", &format!("{}/gerar-chaves-ssh.service", raiz)])
            .output()
        {
            let erros = String::from_utf8_lossy(&saida.stderr);
            assert!(!erros.contains("gerar-chaves-ssh.service:"), "{}", erros);
        }
    }

    // ---------- Fuzz: 100 mil linhas de boot aleatórias ----------

    /// Gerador de números "aleatórios" simples (sem crate): sempre a mesma
    /// sequência, então se um teste falhar ele falha igual de novo.
    struct Aleatorio(u64);
    impl Aleatorio {
        fn proximo(&mut self, limite: usize) -> usize {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) as usize) % limite
        }
    }

    #[test]
    fn fuzz_100_mil_linhas_de_boot() {
        let especificos = [
            "root=UUID=abc", "root=/dev/mapper/x", "ro", "rw", "rootflags=subvol=root",
            "rootflags=subvol=@", "rd.luks.uuid=luks-1", "rd.luks.name=a=b", "rd.lvm.lv=fedora/root",
            "rd.md.uuid=1", "resume=UUID=2", "resume=/dev/sda2", "console=ttyS1", "console=tty0",
        ];
        let seguros = ["rhgb", "quiet", "splash", "mitigations=off", "selinux=1", "nomodeset", "audit=0", "rootwait"];
        let espacos = [" ", "  ", "\t", " \n "];
        let mut rng = Aleatorio(42);

        for _ in 0..100_000 {
            // Monta uma linha com até 12 parâmetros sorteados.
            let mut linha = String::new();
            let mut seguros_usados = Vec::new();
            for _ in 0..rng.proximo(13) {
                let parametro = if rng.proximo(2) == 0 {
                    especificos[rng.proximo(especificos.len())]
                } else {
                    let s = seguros[rng.proximo(seguros.len())];
                    seguros_usados.push(s);
                    s
                };
                linha.push_str(parametro);
                linha.push_str(espacos[rng.proximo(espacos.len())]);
            }

            let nova = nova_cmdline("NOVO", &linha);
            let partes: Vec<&str> = nova.split(' ').collect();

            assert!(nova.starts_with("root=UUID=NOVO ro "), "{:?} -> {}", linha, nova);
            assert!(nova.ends_with(" console=tty0 console=ttyS0,115200"), "{:?} -> {}", linha, nova);
            assert!(!nova.contains("  ") && !nova.contains('\t') && !nova.contains('\n'));
            assert_eq!(partes.iter().filter(|p| p.starts_with("root=")).count(), 1);
            assert_eq!(partes.iter().filter(|p| **p == "ro").count(), 1);
            assert_eq!(partes.iter().filter(|p| p.starts_with("console=")).count(), 2);
            for proibido in ["rw", "rootflags=", "rd.luks", "rd.lvm", "rd.md", "resume="] {
                assert!(!partes.iter().any(|p| p.starts_with(proibido)), "{} sobrou em {}", proibido, nova);
            }
            // Os parâmetros seguros continuam, na mesma ordem.
            let meio: Vec<&str> = partes[2..partes.len() - 2].to_vec();
            assert_eq!(meio, seguros_usados, "{:?}", linha);
            // Aplicar de novo não muda nada.
            assert_eq!(nova_cmdline("NOVO", &nova), nova);
        }
    }
}
