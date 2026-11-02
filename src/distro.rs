// Descobre qual Linux está rodando, lendo o arquivo /etc/os-release.
//
// Distribuições da mesma "família" usam as mesmas ferramentas de boot:
//   Fedora: Fedora, RHEL, Rocky, AlmaLinux, CentOS -> dracut + grub2
//   Debian: Debian, Ubuntu, Mint, Pop!_OS          -> update-initramfs + grub

use crate::Resultado;
use std::fs;

// Um `enum` é um tipo que só pode ser UMA das opções listadas.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Familia {
    Fedora,
    Debian,
}

#[derive(Debug)]
pub struct Distro {
    pub nome: String, // ex.: "Fedora Linux 44 (Workstation Edition)"
    pub id: String,   // ex.: "fedora"
    pub familia: Familia,
}

impl Familia {
    /// Grupo que dá permissão de `sudo` em cada família.
    pub fn grupo_admin(&self) -> &'static str {
        // `match` em um enum obriga a tratar TODAS as opções; se um dia
        // adicionarmos uma família nova, o compilador avisa que falta aqui.
        match self {
            Familia::Fedora => "wheel",
            Familia::Debian => "sudo",
        }
    }

    /// Programas que precisam existir no sistema para o clone funcionar.
    pub fn ferramentas(&self) -> Vec<&'static str> {
        let mut lista = vec![
            "qemu-img", "losetup", "sfdisk", "mkfs.vfat",
            "mkfs.ext4", "e2fsck", "blockdev", "blkid", "rsync", "mount", "umount", "chroot", "useradd",
            "userdel", "usermod", "chage", "du",
        ];
        match self {
            Familia::Fedora => lista.extend(["dracut", "grub2-mkconfig"]),
            Familia::Debian => lista.extend(["update-initramfs", "grub-install", "update-grub"]),
        }
        lista
    }

    /// Programas a mais para gerar a ISO (no seu computador, fora da imagem).
    pub fn ferramentas_iso(&self) -> Vec<&'static str> {
        let mkrescue = match self {
            Familia::Fedora => "grub2-mkrescue",
            Familia::Debian => "grub-mkrescue",
        };
        vec!["mksquashfs", "xorriso", mkrescue, "calamares"]
    }

    /// Os pacotes que fornecem tudo o que a ISO precisa, para a mensagem de erro
    /// já dizer o comando certo de instalar.
    pub fn pacotes_iso(&self) -> &'static str {
        match self {
            Familia::Fedora => "sudo dnf install calamares dracut-live squashfs-tools xorriso mtools grub2-tools-extra grub2-pc-modules grub2-efi-x64-modules",
            Familia::Debian => "sudo apt install calamares live-boot squashfs-tools xorriso mtools grub-common grub-pc-bin grub-efi-amd64-bin",
        }
    }
}

/// Lê o /etc/os-release do sistema atual.
pub fn detectar() -> Resultado<Distro> {
    let texto = fs::read_to_string("/etc/os-release")?;
    detectar_de_texto(&texto)
}

/// A lógica fica separada da leitura do arquivo para dar para testar
/// com o os-release de qualquer distro, sem precisar instalá-la.
pub fn detectar_de_texto(texto: &str) -> Resultado<Distro> {
    let mut id = String::new();
    let mut id_like = String::new();
    let mut nome = String::new();

    // Cada linha tem o formato CHAVE=valor (com aspas duplas, simples ou sem).
    for linha in texto.lines() {
        if let Some((chave, valor)) = linha.split_once('=') {
            let valor = valor.trim().trim_matches(['"', '\'']).to_string();
            match chave.trim() {
                "ID" => id = valor.to_lowercase(),
                "ID_LIKE" => id_like = valor.to_lowercase(),
                "PRETTY_NAME" => nome = valor,
                _ => {} // `_` significa "qualquer outra coisa": ignoramos
            }
        }
    }
    if id.is_empty() {
        return Err("não encontrei o ID da distribuição no /etc/os-release".into());
    }
    if nome.is_empty() {
        nome = id.clone();
    }

    // Procuramos a família no ID e também no ID_LIKE ("parecido com").
    let todos = format!("{} {}", id, id_like);
    let tem = |nomes: &[&str]| todos.split_whitespace().any(|x| nomes.contains(&x));

    let familia = if tem(&["fedora", "rhel", "centos"]) {
        Familia::Fedora
    } else if tem(&["debian", "ubuntu"]) {
        Familia::Debian
    } else {
        return Err(format!(
            "a distribuição '{}' ainda não é suportada (suporto as famílias Fedora e Debian)",
            nome
        )
        .into());
    };

    Ok(Distro { nome, id, familia })
}

#[cfg(test)]
mod testes {
    use super::*;

    fn familia(texto: &str) -> Familia {
        detectar_de_texto(texto).unwrap().familia
    }

    #[test]
    fn fedora() {
        let d = detectar_de_texto("NAME=\"Fedora Linux\"\nID=fedora\nPRETTY_NAME=\"Fedora Linux 44 (Workstation Edition)\"\n").unwrap();
        assert_eq!(d.familia, Familia::Fedora);
        assert_eq!(d.id, "fedora");
        assert_eq!(d.nome, "Fedora Linux 44 (Workstation Edition)");
    }

    #[test]
    fn derivados_do_rhel() {
        assert_eq!(familia("ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n"), Familia::Fedora);
        assert_eq!(familia("ID=\"almalinux\"\nID_LIKE=\"rhel centos fedora\"\n"), Familia::Fedora);
        assert_eq!(familia("ID=\"rhel\"\nID_LIKE=\"fedora\"\n"), Familia::Fedora);
        assert_eq!(familia("ID=\"centos\"\nID_LIKE=\"rhel fedora\"\n"), Familia::Fedora);
    }

    #[test]
    fn ubuntu_e_derivados() {
        let d = detectar_de_texto("PRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nID=ubuntu\nID_LIKE=debian\n").unwrap();
        assert_eq!(d.familia, Familia::Debian);
        assert_eq!(d.id, "ubuntu");
        assert_eq!(familia("ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n"), Familia::Debian);
        assert_eq!(familia("ID=pop\nID_LIKE=\"ubuntu debian\"\n"), Familia::Debian);
        assert_eq!(familia("ID=debian\n"), Familia::Debian);
    }

    #[test]
    fn aceita_aspas_simples_espacos_e_maiusculas() {
        let d = detectar_de_texto("ID='Ubuntu'\nPRETTY_NAME='Ubuntu 22.04'\n").unwrap();
        assert_eq!(d.id, "ubuntu");
        assert_eq!(d.nome, "Ubuntu 22.04");
        assert_eq!(familia("ID = fedora \n"), Familia::Fedora);
    }

    #[test]
    fn sem_pretty_name_usa_o_id() {
        assert_eq!(detectar_de_texto("ID=debian\n").unwrap().nome, "debian");
    }

    #[test]
    fn distros_nao_suportadas_dao_erro() {
        assert!(detectar_de_texto("ID=arch\nPRETTY_NAME=\"Arch Linux\"\n").is_err());
        assert!(detectar_de_texto("ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n").is_err());
        assert!(detectar_de_texto("ID=alpine\n").is_err());
        assert!(detectar_de_texto("").is_err());
        // "debianzinho" não é "debian": comparamos palavras inteiras.
        assert!(detectar_de_texto("ID=debianzinho\n").is_err());
    }

    #[test]
    fn grupo_admin_por_familia() {
        assert_eq!(Familia::Fedora.grupo_admin(), "wheel");
        assert_eq!(Familia::Debian.grupo_admin(), "sudo");
    }

    #[test]
    fn ferramentas_por_familia() {
        assert!(Familia::Fedora.ferramentas().contains(&"dracut"));
        assert!(!Familia::Fedora.ferramentas().contains(&"update-grub"));
        assert!(Familia::Debian.ferramentas().contains(&"update-initramfs"));
        assert!(!Familia::Debian.ferramentas().contains(&"dracut"));
        assert!(Familia::Debian.ferramentas().contains(&"rsync"));
    }

    #[test]
    fn os_release_deste_computador_funciona() {
        // Se este PC for Fedora/Debian, tem que detectar sem erro.
        let texto = fs::read_to_string("/etc/os-release").unwrap_or_default();
        if texto.contains("fedora") || texto.contains("debian") || texto.contains("ubuntu") {
            assert!(detectar_de_texto(&texto).is_ok());
        }
    }
}
