// Copia o sistema que está rodando para dentro do disco virtual, usando o rsync.
//
// O que NÃO é copiado (lista EXCLUIR):
//   - pastas "virtuais" que o Linux cria na hora do boot (/proc, /sys, /dev, /run)
//   - os dados dos usuários (/home, /root)
//   - caches, logs, discos de VMs e containers (são grandes e pessoais)

use crate::executor::Executor;
use crate::plano::Tipo;
use crate::Resultado;
use std::path::Path;
use std::process::Command;

// `/proc/*` = "o conteúdo de /proc": a pasta vazia é copiada, o que tem dentro não.
// Os padrões começam com "/" para valer só a partir da raiz da cópia
// (sem a "/", "home/*" também pegaria /usr/share/home/...).
const EXCLUIR: &[&str] = &[
    "/dev/*", "/proc/*", "/sys/*", "/run/*", "/tmp/*", "/mnt/*", "/media/*",
    "/lost+found", "/home/*", "/root/*",
    // Arquivos de swap (memória extra no disco): o do Fedora/Mint, o do Ubuntu
    // Server e o do btrfs. São grandes e a VM não usa a swap da máquina original.
    "/swapfile", "/swap.img", "/swap/swapfile",
    "/var/tmp/*", "/var/cache/*", "/var/log/journal/*", "/var/lib/systemd/coredump/*",
    "/var/lib/libvirt/images/*", "/var/lib/containers/*", "/var/lib/docker/*",
    // Ubuntu: cada snap é um arquivo .snap montado em /snap/nome/versão.
    // Copiamos o .snap (em /var/lib/snapd/snaps) e deixamos de fora o conteúdo
    // montado, que seria uma segunda cópia do mesmo programa.
    "/snap/*/*/*", "/var/lib/snapd/cache/*",
    "/boot/efi/*", // a partição EFI é copiada à parte (ela é FAT32, não aceita permissões Linux)
];

const FLATPAK: &str = "/var/lib/flatpak/*";

// O que fica na raiz (/) e nunca entra na cópia: pastas virtuais, dados
// pessoais e swap. Tem que combinar com a lista EXCLUIR acima.
const RAIZ_FORA: &[&str] = &[
    "dev", "proc", "sys", "run", "tmp", "mnt", "media", "home", "root",
    "lost+found", "swapfile", "swap.img", "swap",
];

/// Estima quantos bytes serão copiados, usando o `du` (disk usage).
pub fn estimar_tamanho(incluir_flatpak: bool) -> u64 {
    // TUDO o que está na raiz, menos o que fica de fora. (Antes só contávamos
    // /usr, /etc, /var...; uma pasta como /opt2 ou /nix ficava de fora da conta
    // e a imagem saía pequena demais.)
    let mut pastas: Vec<String> = std::fs::read_dir("/")
        .map(|entradas| {
            entradas
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|nome| !RAIZ_FORA.contains(&nome.as_str()))
                .map(|nome| format!("/{}", nome))
                .collect()
        })
        .unwrap_or_default();
    pastas.sort();
    let pastas: Vec<&str> = pastas.iter().map(|p| p.as_str()).collect();
    let total = du(&pastas);

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
/// -c = total geral na última linha. Sem o -x: o rsync também entra em outros
/// discos montados dentro destas pastas (um /data, por exemplo), e a conta
/// precisa bater com o que ele vai copiar de verdade.
fn du(pastas: &[&str]) -> u64 {
    // Pastas que não existem fariam o du reclamar; passamos só as que existem.
    let existentes: Vec<&str> = pastas.iter().copied().filter(|p| Path::new(p).exists()).collect();
    if existentes.is_empty() {
        return 0;
    }
    let Ok(saida) = Command::new("du").arg("-sbc").args(&existentes).output() else {
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

/// Quanto espaço o arquivo final precisa para ser criado com segurança.
/// Por um tempo existem DOIS arquivos: o rascunho .raw e o resultado.
///   - o .raw: o sistema copiado + 20% de folga (arquivos novos, metadados)
///   - template: o .qcow2, do tamanho do sistema (~70% dele se for compactado)
///   - ISO: o squashfs (~50% do sistema) E a ISO, que leva uma cópia dele
///   - + 1 GB de margem
pub fn espaco_necessario(usado: u64, tipo: Tipo, compactar: bool) -> u64 {
    let resultado = match tipo {
        Tipo::Template if compactar => usado * 7 / 10,
        Tipo::Template => usado,
        Tipo::Iso => usado / 2 + usado / 2,
    };
    usado + usado / 5 + resultado + 1_000_000_000
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

/// O rsync trata `*`, `?` e `[` como curingas: um arquivo chamado "img[1].qcow2"
/// não casaria com ele mesmo e seria copiado. Com uma barra invertida antes,
/// o caractere vale "ao pé da letra". (O rsync só entende a barra como escape
/// quando o padrão tem algum curinga; sem curinga, deixamos como está.)
fn escapar_padrao(caminho: &str) -> String {
    if !caminho.contains(['*', '?', '[']) {
        return caminho.to_string();
    }
    let mut saida = String::new();
    for c in caminho.chars() {
        if matches!(c, '*' | '?' | '[' | '\\') {
            saida.push('\\');
        }
        saida.push(c);
    }
    saida
}

/// Monta a lista de argumentos do rsync. Fica separada para dar para testar.
/// `origem` é "/" no programa de verdade (nos testes é uma pasta temporária).
/// `nao_copiar` são os arquivos que o próprio maloki está criando (o .raw que
/// cresce durante a cópia, o arquivo final...): copiar eles seria copiar a
/// imagem para dentro dela mesma, até o disco encher.
/// Os caminhos precisam ser os REAIS (sem "..", sem links): é assim que o rsync
/// os enxerga. O plano.rs já entrega o caminho da imagem resolvido.
fn argumentos_rsync(origem: &str, destino: &str, nao_copiar: &[&str], incluir_flatpak: bool) -> Vec<String> {
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

    // O padrão precisa ser relativo à origem: /home/x/t.qcow2 vira "/home/x/t.qcow2"
    // quando a origem é "/", ou "/t.qcow2" quando a origem é /home/x.
    for arquivo in nao_copiar {
        if let Ok(relativo) = Path::new(arquivo).strip_prefix(origem) {
            args.push(format!("--exclude=/{}", escapar_padrao(&relativo.to_string_lossy())));
        }
    }

    // A barra no fim de "origem/" significa "o CONTEÚDO da origem".
    args.push(format!("{}/", origem.trim_end_matches('/')));
    args.push(format!("{}/", destino.trim_end_matches('/')));
    args
}

pub fn copiar_sistema(exec: &Executor, origem: &str, raiz: &str, nao_copiar: &[&str], incluir_flatpak: bool) -> Resultado<()> {
    let args = argumentos_rsync(origem, raiz, nao_copiar, incluir_flatpak);
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
        assert_eq!(espaco_necessario(10 * gb, Tipo::Template, false), 23 * gb); // 10 + 2 + 10 + 1
        assert_eq!(espaco_necessario(10 * gb, Tipo::Template, true), 20 * gb); // 10 + 2 + 7 + 1
        assert_eq!(espaco_necessario(10 * gb, Tipo::Iso, true), 23 * gb); // 10 + 2 + 5 + 5 + 1
        assert_eq!(espaco_necessario(0, Tipo::Template, true), gb);
        // O .qcow2 compactado é menor, então precisa de menos espaço.
        for usado in [1, 500 * gb, 3 * gb + 7] {
            assert!(espaco_necessario(usado, Tipo::Template, true) <= espaco_necessario(usado, Tipo::Template, false));
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
        let a = argumentos_rsync("/", "/mnt/maloki-1", &["/home/daniel/t.qcow2"], true);
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
        let a = argumentos_rsync("/", "/mnt/x", &["/home/t.qcow2"], false);
        assert!(a.contains(&format!("--exclude={}", FLATPAK)));
    }

    #[test]
    fn curingas_no_nome_sao_escapados() {
        assert_eq!(escapar_padrao("home/x/t.qcow2"), "home/x/t.qcow2");
        assert_eq!(escapar_padrao("srv/img[1].raw"), "srv/img\\[1].raw");
        assert_eq!(escapar_padrao("a*b?c\\d"), "a\\*b\\?c\\\\d");
        // Sem curinga, a barra invertida fica como está (o rsync não a trata como escape).
        assert_eq!(escapar_padrao("a\\b"), "a\\b");
    }

    #[test]
    fn a_lista_da_raiz_combina_com_as_exclusoes() {
        // Tudo o que a estimativa deixa de fora tem que estar excluído no rsync,
        // senão a conta de espaço fica menor do que a cópia de verdade.
        for nome in RAIZ_FORA {
            let excluido = EXCLUIR.iter().any(|p| *p == format!("/{}", nome) || p.starts_with(&format!("/{}/", nome)));
            assert!(excluido, "/{} fica fora da conta mas não do rsync", nome);
        }
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
        // Uma imagem fora do /home, com curingas do rsync no nome.
        let esquisita = format!("{}/srv/img[1]*.qcow2.raw", origem);
        criar(&origem, "srv/img[1]*.qcow2.raw", "outra imagem sendo criada");
        criar(&origem, "srv/img1x.qcow2.raw", "parecida, mas é outro arquivo: vai");
        criar(&origem, "swap.img", "swap do Ubuntu Server");

        let exec = Executor { simular: false };
        copiar_sistema(&exec, &origem, &destino, &[&imagem, &esquisita], true).unwrap();

        for vai in [
            "etc/hostname", "usr/bin/programa", "usr/share/home/arquivo",
            "var/lib/flatpak/app/org.app/x", "var/lib/snapd/snaps/firefox_1.snap",
            "var/log/messages", "boot/vmlinuz", "boot/efi/EFI/fedora/grubx64.efi",
            "srv/img1x.qcow2.raw",
        ] {
            assert!(existe(&destino, vai), "{} deveria ter sido copiado", vai);
        }
        for nao_vai in [
            "home/daniel", "root/.ssh", "proc/1", "tmp/arquivo", "var/cache/dnf",
            "var/tmp/x", "var/log/journal/abc", "var/lib/containers/storage",
            "var/lib/libvirt/images/vm.qcow2", "snap/firefox/1/usr", "mnt/maloki-1",
            "srv/img[1]*.qcow2.raw", "swap.img",
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
        copiar_sistema(&Executor { simular: false }, &origem, &destino, &["/fora/t.qcow2"], false).unwrap();
        assert!(existe(&destino, "etc/hostname"));
        assert!(existe(&destino, "var/lib/flatpak"));
        assert!(!existe(&destino, "var/lib/flatpak/app"));
    }
}
