// Copia o sistema que está rodando para dentro do disco virtual, usando o rsync.
//
// O que NÃO é copiado (lista EXCLUIR):
//   - pastas "virtuais" que o Linux cria na hora do boot (/proc, /sys, /dev, /run)
//   - os dados dos usuários (/home, /root)
//   - caches, logs, discos de VMs e containers (são grandes e pessoais)

use crate::executor::Executor;
use crate::Resultado;
use std::path::Path;
use std::process::Command;

// `/proc/*` = "o conteúdo de /proc": a pasta vazia é copiada, o que tem dentro não.
// Os padrões começam com "/" para valer só a partir da raiz da cópia
// (sem a "/", "home/*" também pegaria /usr/share/home/...).
const EXCLUIR: &[&str] = &[
    "/dev/*", "/proc/*", "/sys/*", "/run/*", "/tmp/*", "/mnt/*", "/media/*",
    "/lost+found", "/swapfile", "/home/*", "/root/*",
    "/var/tmp/*", "/var/cache/*", "/var/log/journal/*", "/var/lib/systemd/coredump/*",
    "/var/lib/libvirt/images/*", "/var/lib/containers/*", "/var/lib/docker/*",
    // Ubuntu: cada snap é um arquivo .snap montado em /snap/nome/versão.
    // Copiamos o .snap (em /var/lib/snapd/snaps) e deixamos de fora o conteúdo
    // montado, que seria uma segunda cópia do mesmo programa.
    "/snap/*/*/*", "/var/lib/snapd/cache/*",
    "/boot/efi/*", // a partição EFI é copiada à parte (ela é FAT32, não aceita permissões Linux)
];

const FLATPAK: &str = "/var/lib/flatpak/*";

/// Estima quantos bytes serão copiados, usando o `du` (disk usage).
pub fn estimar_tamanho(incluir_flatpak: bool) -> u64 {
    let total = du(&["/usr", "/etc", "/opt", "/boot", "/srv", "/var"]);

    // Subtraímos as pastas grandes que ficam de fora.
    let mut fora = vec![
        "/var/cache", "/var/tmp", "/var/log/journal", "/var/lib/libvirt/images",
        "/var/lib/containers", "/var/lib/docker", "/var/lib/systemd/coredump",
    ];
    if !incluir_flatpak {
        fora.push("/var/lib/flatpak");
    }
    total.saturating_sub(du(&fora)) // saturating_sub: nunca fica negativo
}

/// Soma o tamanho das pastas. -s = só o total, -b = em bytes,
/// -x = não entra em outros discos montados, -c = total geral na última linha.
fn du(pastas: &[&str]) -> u64 {
    // Pastas que não existem fariam o du reclamar; passamos só as que existem.
    let existentes: Vec<&str> = pastas.iter().copied().filter(|p| Path::new(p).exists()).collect();
    if existentes.is_empty() {
        return 0;
    }
    let Ok(saida) = Command::new("du").arg("-sxbc").args(&existentes).output() else {
        return 0;
    };
    total_do_du(&String::from_utf8_lossy(&saida.stdout))
}

/// Quanto espaço livre (em bytes) existe no disco onde fica a pasta.
/// `df --output=avail -B1` mostra só o espaço livre, em bytes, numa 2ª linha.
pub fn espaco_livre(pasta: &str) -> Option<u64> {
    let saida = Command::new("df").args(["--output=avail", "-B1", pasta]).output().ok()?;
    let texto = String::from_utf8_lossy(&saida.stdout);
    texto.lines().nth(1)?.trim().parse().ok()
}

/// Quanto espaço a imagem precisa para ser criada com segurança.
/// Por um tempo existem DOIS arquivos: o rascunho .raw e o .qcow2 final.
///   - o .raw: o sistema copiado + 20% de folga (arquivos novos, metadados)
///   - o .qcow2: do tamanho do sistema, ou ~70% dele se for compactado
///   - + 1 GB de margem
pub fn espaco_necessario(usado: u64, compactar: bool) -> u64 {
    let qcow2 = if compactar { usado * 7 / 10 } else { usado };
    usado + usado / 5 + qcow2 + 1_000_000_000
}

/// A última linha do `du -c` é "12345\ttotal": pegamos o número dela.
fn total_do_du(texto: &str) -> u64 {
    texto
        .lines()
        .last()
        .and_then(|linha| linha.split_whitespace().next())
        .and_then(|numero| numero.parse().ok())
        .unwrap_or(0)
}

/// Monta a lista de argumentos do rsync. Fica separada para dar para testar.
/// `origem` é "/" no programa de verdade (nos testes é uma pasta temporária).
fn argumentos_rsync(origem: &str, destino: &str, imagem: &str, incluir_flatpak: bool) -> Vec<String> {
    // -a  mantém dono, permissões, datas e links
    // -A  mantém ACLs   -X mantém atributos estendidos (inclui rótulos do SELinux)
    // -H  mantém hard links   --numeric-ids  não traduz donos pelo nome
    // --info=progress2  mostra o progresso total
    let mut args: Vec<String> = vec!["-aAXH".into(), "--numeric-ids".into(), "--info=progress2".into()];

    for padrao in EXCLUIR {
        args.push(format!("--exclude={}", padrao));
    }
    if !incluir_flatpak {
        args.push(format!("--exclude={}", FLATPAK));
    }

    // Nunca copiar a própria imagem que estamos criando (ela está crescendo!).
    // O padrão precisa ser relativo à origem: /home/x/t.qcow2 vira "/home/x/t.qcow2"
    // quando a origem é "/", ou "/t.qcow2" quando a origem é /home/x.
    if let Ok(relativo) = Path::new(imagem).strip_prefix(origem) {
        args.push(format!("--exclude=/{}", relativo.to_string_lossy()));
    }

    // A barra no fim de "origem/" significa "o CONTEÚDO da origem".
    args.push(format!("{}/", origem.trim_end_matches('/')));
    args.push(format!("{}/", destino.trim_end_matches('/')));
    args
}

pub fn copiar_sistema(exec: &Executor, origem: &str, raiz: &str, imagem: &str, incluir_flatpak: bool) -> Resultado<()> {
    let args = argumentos_rsync(origem, raiz, imagem, incluir_flatpak);
    // O rsync quer &str; convertemos a lista de String para uma lista de &str.
    let args_str: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    // 24 = "alguns arquivos sumiram durante a cópia": normal num sistema ligado.
    exec.rodar_aceitando("rsync", &args_str, &[24])?;

    // A partição EFI: -r recursivo, -t mantém datas (FAT32 não guarda dono/permissão).
    let origem_efi = format!("{}/boot/efi/", origem.trim_end_matches('/'));
    let destino_efi = format!("{}/boot/efi/", raiz);
    if exec.simular || Path::new(&origem_efi).exists() {
        exec.rodar("rsync", &["-rt", "--info=progress2", &origem_efi, &destino_efi])?;
    }
    Ok(())
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::limpeza::apoio::*;

    #[test]
    fn le_o_total_do_du() {
        assert_eq!(total_do_du("100\t/usr\n200\t/etc\n300\ttotal\n"), 300);
        assert_eq!(total_do_du(""), 0);
        assert_eq!(total_do_du("lixo\n"), 0);
    }

    #[test]
    fn conta_do_espaco_necessario() {
        let gb = 1_000_000_000;
        assert_eq!(espaco_necessario(10 * gb, false), 23 * gb); // 10 + 2 + 10 + 1
        assert_eq!(espaco_necessario(10 * gb, true), 20 * gb); // 10 + 2 + 7 + 1
        assert_eq!(espaco_necessario(0, true), gb);
        // O .qcow2 compactado é menor, então precisa de menos espaço.
        for usado in [1, 500 * gb, 3 * gb + 7] {
            assert!(espaco_necessario(usado, true) <= espaco_necessario(usado, false));
        }
    }

    #[test]
    fn espaco_livre_de_verdade() {
        let livre = espaco_livre("/tmp").unwrap();
        assert!(livre > 0);
        assert_eq!(espaco_livre("/pasta/que/nao/existe"), None);
    }

    #[test]
    fn du_de_verdade() {
        let raiz = raiz_de_teste();
        criar(&raiz, "a", &"x".repeat(5000));
        assert!(du(&[raiz.as_str()]) >= 5000);
        assert_eq!(du(&["/caminho/que/nao/existe"]), 0);
    }

    #[test]
    fn argumentos_basicos() {
        let a = argumentos_rsync("/", "/mnt/maloki-1", "/home/daniel/t.qcow2", true);
        assert_eq!(a[0], "-aAXH");
        assert!(a.contains(&"--exclude=/home/*".to_string()));
        assert!(a.contains(&"--exclude=/proc/*".to_string()));
        assert!(a.contains(&"--exclude=/boot/efi/*".to_string()));
        assert!(a.contains(&"--exclude=/home/daniel/t.qcow2".to_string()));
        assert!(!a.contains(&format!("--exclude={}", FLATPAK)));
        // origem e destino são sempre os dois últimos, com barra no fim
        assert_eq!(a[a.len() - 2], "/");
        assert_eq!(a[a.len() - 1], "/mnt/maloki-1/");
    }

    #[test]
    fn flatpak_opcional() {
        let a = argumentos_rsync("/", "/mnt/x", "/home/t.qcow2", false);
        assert!(a.contains(&format!("--exclude={}", FLATPAK)));
    }

    #[test]
    fn todos_os_padroes_sao_ancorados_na_raiz() {
        for padrao in EXCLUIR {
            assert!(padrao.starts_with('/'), "o padrão {} precisa começar com /", padrao);
        }
    }

    /// Roda o rsync DE VERDADE numa "máquina" falsa e confere o que foi copiado.
    #[test]
    fn rsync_de_verdade_respeita_as_exclusoes() {
        let origem = raiz_de_teste();
        let destino = raiz_de_teste();
        let imagem = format!("{}/home/daniel/template.qcow2", origem);

        // O que DEVE ir para a imagem
        criar(&origem, "etc/hostname", "pc");
        criar(&origem, "usr/bin/programa", "binario");
        criar(&origem, "usr/share/home/arquivo", "não confundir com /home");
        criar(&origem, "var/lib/flatpak/app/org.app/x", "app");
        criar(&origem, "var/lib/snapd/snaps/firefox_1.snap", "snap");
        criar(&origem, "var/log/messages", "log (é limpo depois, na limpeza)");
        criar(&origem, "boot/vmlinuz", "kernel");
        criar(&origem, "boot/efi/EFI/fedora/grubx64.efi", "grub");

        // O que NÃO pode ir
        criar(&origem, "home/daniel/documentos/segredo.txt", "pessoal");
        criar(&origem, "home/daniel/.bash_history", "historico");
        criar(&origem, "root/.ssh/id_ed25519", "chave");
        criar(&origem, "proc/1/status", "x");
        criar(&origem, "tmp/arquivo", "x");
        criar(&origem, "var/cache/dnf/pacote.rpm", "x");
        criar(&origem, "var/tmp/x", "x");
        criar(&origem, "var/log/journal/abc/system.journal", "x");
        criar(&origem, "var/lib/containers/storage/imagem", "x");
        criar(&origem, "var/lib/libvirt/images/vm.qcow2", "x");
        criar(&origem, "snap/firefox/1/usr/lib/firefox", "conteúdo montado");
        criar(&origem, "mnt/maloki-1/etc/x", "a própria imagem montada");
        criar(&origem, "home/daniel/template.qcow2", "a imagem sendo criada");

        let exec = Executor { simular: false };
        copiar_sistema(&exec, &origem, &destino, &imagem, true).unwrap();

        for vai in [
            "etc/hostname", "usr/bin/programa", "usr/share/home/arquivo",
            "var/lib/flatpak/app/org.app/x", "var/lib/snapd/snaps/firefox_1.snap",
            "var/log/messages", "boot/vmlinuz", "boot/efi/EFI/fedora/grubx64.efi",
        ] {
            assert!(existe(&destino, vai), "{} deveria ter sido copiado", vai);
        }
        for nao_vai in [
            "home/daniel", "root/.ssh", "proc/1", "tmp/arquivo", "var/cache/dnf",
            "var/tmp/x", "var/log/journal/abc", "var/lib/containers/storage",
            "var/lib/libvirt/images/vm.qcow2", "snap/firefox/1/usr", "mnt/maloki-1",
        ] {
            assert!(!existe(&destino, nao_vai), "{} NÃO deveria ter sido copiado", nao_vai);
        }
        // As pastas em si existem (vazias), porque o sistema precisa delas.
        for pasta in ["home", "root", "proc", "tmp", "var/cache", "snap/firefox/1"] {
            assert!(existe(&destino, pasta), "a pasta {} deveria existir vazia", pasta);
        }
    }

    #[test]
    fn rsync_sem_flatpak() {
        let origem = raiz_de_teste();
        let destino = raiz_de_teste();
        criar(&origem, "var/lib/flatpak/app/org.app/x", "app");
        criar(&origem, "etc/hostname", "pc");
        copiar_sistema(&Executor { simular: false }, &origem, &destino, "/fora/t.qcow2", false).unwrap();
        assert!(existe(&destino, "etc/hostname"));
        assert!(existe(&destino, "var/lib/flatpak"));
        assert!(!existe(&destino, "var/lib/flatpak/app"));
    }
}
