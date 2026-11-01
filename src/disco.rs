// Cria o disco da imagem e o "pluga" no sistema como se fosse um disco de verdade.
//
// Truque usado: o disco "loop" do kernel. O comando `losetup` faz um ARQUIVO
// comum aparecer como um disco (/dev/loop0). A partir daí dá para particionar,
// formatar e montar normalmente.
//
// Trabalhamos com um arquivo "cru" (.raw): os dados vão direto do kernel para o
// arquivo, sem nenhum programa no meio guardando coisas na memória. No final, o
// `qemu-img convert` (no main.rs) transforma o .raw no .qcow2 de verdade.

use crate::executor::Executor;
use crate::Resultado;
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

// Toda pasta de montagem começa com isto. Usamos esse prefixo como trava de
// segurança: o programa só apaga arquivos que estejam dentro dele.
pub const PREFIXO_RAIZ: &str = "/mnt/maloki-";

// O disco da imagem tem 4 partições, para ligar tanto em BIOS quanto em UEFI:
//   1. "BIOS boot" (1 MB): onde o GRUB do modo BIOS (antigo) se instala
//   2. EFI (600 MB, FAT32): onde fica o GRUB do modo UEFI (moderno)
//   3. /boot (1 GB, ext4): o kernel e os arquivos do GRUB
//   4. raiz (o resto, ext4): o sistema
//
// Por que um /boot separado, no começo do disco? No modo BIOS, o GRUB às vezes
// só consegue ler os primeiros ~8 GB do disco. Com o /boot aqui, tudo o que ele
// precisa fica no primeiro 1,6 GB (é o mesmo motivo de as distros fazerem assim).
pub const PARTICAO_EFI: u32 = 2;
pub const PARTICAO_BOOT: u32 = 3;
pub const PARTICAO_RAIZ: u32 = 4;

// O sfdisk lê este "roteiro". Os códigos longos (GUIDs) são os tipos oficiais
// de partição GPT; usá-los funciona em qualquer versão do sfdisk.
pub const ROTEIRO_PARTICOES: &str = "label: gpt
size=1MiB, type=21686148-6449-6E6F-744E-656564454649, name=bios
size=600MiB, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name=EFI
size=1GiB, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name=boot
type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name=raiz
";

pub struct Disco {
    exec: Executor,
    pub dispositivo: String,  // ex.: "/dev/loop0"
    pub raiz: String,         // pasta onde o disco novo fica montado
    montagens: Vec<String>,   // tudo o que montamos, para desmontar no fim
    finalizado: bool,         // true depois de `finalizar()`: o Drop não faz mais nada
}

impl Disco {
    /// Liga o arquivo .raw a um /dev/loopN livre.
    pub fn conectar(exec: Executor, arquivo_raw: &str) -> Resultado<Disco> {
        // --find      o kernel escolhe um /dev/loopN livre (sem risco de dois
        //             programas pegarem o mesmo)
        // --show      escreve qual foi o escolhido (e nós guardamos)
        // --partscan  cria /dev/loopNp1, p2... para as partições
        let dispositivo = exec.capturar("losetup", &["--find", "--show", "--partscan", arquivo_raw], "/dev/loop0")?;

        // std::process::id() é o número do nosso processo: deixa o nome da pasta único.
        let raiz = format!("{}{}", PREFIXO_RAIZ, std::process::id());
        Ok(Disco { exec, dispositivo, raiz, montagens: Vec::new(), finalizado: false })
    }

    /// Caminho de uma partição: particao(1) -> "/dev/loop0p1".
    pub fn particao(&self, numero: u32) -> String {
        format!("{}p{}", self.dispositivo, numero)
    }

    /// Cria as 4 partições e formata as que precisam de sistema de arquivos.
    pub fn particionar_e_formatar(&self) -> Resultado<()> {
        self.exec.rodar_com_entrada("sfdisk", &["--wipe", "always", &self.dispositivo], ROTEIRO_PARTICOES, false)?;
        // O próprio sfdisk avisa o kernel da tabela nova; só esperamos as partições aparecerem.
        self.esperar_particoes()?;

        // A partição 1 (BIOS) não é formatada: o GRUB grava direto nela.
        self.exec.rodar("mkfs.vfat", &["-F", "32", "-n", "EFI", &self.particao(PARTICAO_EFI)])?;
        self.exec.rodar("mkfs.ext4", &["-F", "-q", "-L", "boot", &self.particao(PARTICAO_BOOT)])?;
        self.exec.rodar("mkfs.ext4", &["-F", "-q", "-L", "raiz", &self.particao(PARTICAO_RAIZ)])?;
        Ok(())
    }

    /// O kernel demora um pouquinho para criar as partições /dev/loop0p*; esperamos até 10s.
    fn esperar_particoes(&self) -> Resultado<()> {
        if self.exec.simular {
            return Ok(());
        }
        // O kernel cria os arquivos das partições fora de ordem (às vezes a 3
        // aparece antes da 2), então esperamos TODAS existirem.
        for _ in 0..20 {
            let todas = (1..=PARTICAO_RAIZ).all(|n| Path::new(&self.particao(n)).exists());
            if todas {
                return Ok(());
            }
            sleep(Duration::from_millis(500));
        }
        Err("as partições do disco virtual não apareceram".into())
    }

    /// Monta algo e guarda na lista para desmontar depois.
    /// Ex.: montar(&["-t", "proc", "proc"], "/mnt/x/proc")
    pub fn montar(&mut self, args: &[&str], destino: &str) -> Resultado<()> {
        self.exec.rodar("mkdir", &["-p", destino])?;
        let mut todos = args.to_vec();
        todos.push(destino);
        self.exec.rodar("mount", &todos)?;
        self.montagens.push(destino.to_string());
        Ok(())
    }

    /// Desmonta tudo, do último para o primeiro (como tirar pratos de uma pilha).
    /// Devolve `true` se TUDO foi desmontado direitinho.
    pub fn desmontar_tudo(&mut self) -> bool {
        if !self.exec.simular {
            // Um programa rodado no chroot pode ter deixado um processo "filho"
            // vivo lá dentro; enquanto ele existir, a imagem fica "ocupada".
            encerrar_processos_na_imagem(&self.raiz);
        }
        // Grava no disco virtual tudo o que ainda está só na memória.
        let _ = self.exec.rodar("sync", &[]);

        let mut tudo_certo = true;
        // `pop()` tira o último item da lista; o loop para quando ela fica vazia.
        while let Some(pasta) = self.montagens.pop() {
            if !self.desmontar(&pasta) {
                tudo_certo = false;
                // Último recurso: "-l" (lazy) solta a pasta e desmonta quando
                // ninguém mais usar. Os dados podem NÃO ter sido gravados ainda!
                let _ = self.exec.rodar("umount", &["-l", &pasta]);
            }
        }
        tudo_certo
    }

    /// Tenta desmontar algumas vezes (às vezes o sistema demora a liberar).
    ///
    /// `-R` (recursivo) desmonta também o que estiver montado DENTRO da pasta,
    /// mesmo que não tenha sido a gente que montou: programas rodados no chroot
    /// às vezes montam coisas sozinhos (ex.: o binfmt_misc dentro do /proc).
    fn desmontar(&self, pasta: &str) -> bool {
        for tentativa in 0..5 {
            if self.exec.rodar("umount", &["-R", pasta]).is_ok() {
                return true;
            }
            if self.exec.simular {
                return false;
            }
            if tentativa == 1 {
                encerrar_processos_na_imagem(&self.raiz);
            }
            sleep(Duration::from_secs(1));
        }
        mostrar_quem_esta_usando(pasta);
        false
    }

    /// O jeito "certo" de terminar: desmonta, confere e solta o disco.
    /// Se algo não desmontou direito, os dados podem não ter sido gravados:
    /// devolvemos erro para a imagem NÃO ser entregue como se estivesse boa.
    pub fn finalizar(mut self) -> Resultado<()> {
        println!("\n🧹 Desmontando e soltando o disco...");
        let desmontou = self.desmontar_tudo();
        self.gravar_cache();
        // Com tudo desmontado e gravado, conferimos os sistemas de arquivos da imagem.
        // e2fsck -f = confere mesmo que pareça limpo; -n = só olha, não muda nada.
        let integro = desmontou
            && [PARTICAO_BOOT, PARTICAO_RAIZ]
                .iter()
                .all(|n| self.exec.rodar("e2fsck", &["-f", "-n", &self.particao(*n)]).is_ok());
        self.soltar();
        self.remover_pasta();
        self.finalizado = true; // o Drop não precisa fazer mais nada
        if !desmontou {
            return Err("a imagem não pôde ser desmontada com segurança (pode estar corrompida) e foi descartada".into());
        }
        if !integro {
            return Err("o e2fsck achou erros no sistema de arquivos da imagem; ela foi descartada".into());
        }
        Ok(())
    }

    /// Mesmo depois do `umount`, parte dos dados pode estar só na MEMÓRIA do
    /// kernel (o "cache" do disco). Antes de conferir ou soltar o disco:
    ///   sync                 -> manda o kernel gravar tudo o que está pendente
    ///   blockdev --flushbufs -> grava E esvazia o cache deste disco; assim o
    ///                           e2fsck depois lê o que está DE FATO no arquivo
    fn gravar_cache(&self) {
        let _ = self.exec.rodar("sync", &[]);
        let _ = self.exec.rodar("blockdev", &["--flushbufs", &self.dispositivo]);
    }

    /// Solta o /dev/loopN do arquivo e espera o kernel terminar de soltar.
    fn soltar(&self) {
        self.gravar_cache();
        let _ = self.exec.rodar("losetup", &["--detach", &self.dispositivo]);
        if self.exec.simular {
            return;
        }
        // Enquanto o disco está ligado ao arquivo, existe /sys/block/loopN/loop.
        let pasta = format!("/sys/block/{}/loop", self.dispositivo.trim_start_matches("/dev/"));
        for _ in 0..40 {
            if !Path::new(&pasta).exists() {
                break;
            }
            sleep(Duration::from_millis(500));
        }
        let _ = self.exec.rodar("sync", &[]);
    }

    fn remover_pasta(&self) {
        // rmdir só apaga pasta VAZIA: seguro mesmo se algo continuar montado.
        if self.exec.simular || Path::new(&self.raiz).exists() {
            let _ = self.exec.rodar("rmdir", &[&self.raiz]);
        }
    }
}

/// Encerra os processos que estão rodando "dentro" da imagem.
///
/// Cada processo tem um link /proc/<número>/root que aponta para a raiz que ele
/// enxerga. Os que foram iniciados pelo nosso chroot apontam para a imagem
/// (/mnt/maloki-PID); os do seu sistema apontam para "/" e não são tocados.
fn encerrar_processos_na_imagem(raiz: &str) {
    let presos = processos_dentro(raiz);
    for (pid, nome) in &presos {
        println!("  ⚠ encerrando \"{}\" (processo {}), que ficou rodando dentro da imagem", nome, pid);
        let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
    }
    if !presos.is_empty() {
        sleep(Duration::from_secs(2));
        // Quem ignorou o pedido educado (TERM) é encerrado à força (KILL).
        for (pid, _) in processos_dentro(raiz) {
            let _ = std::process::Command::new("kill").args(["-KILL", &pid.to_string()]).status();
        }
        sleep(Duration::from_secs(1));
    }
}

/// Quando uma pasta não desmonta, mostra o que ainda está montado dentro dela
/// e quem está usando: ajuda MUITO a descobrir o motivo (e a relatar o bug).
fn mostrar_quem_esta_usando(pasta: &str) {
    println!("  ⚠ não consegui desmontar {}. O que ainda está lá:", pasta);
    let _ = std::process::Command::new("findmnt").args(["-R", pasta]).status();
    // O fuser (pacote psmisc) lista os processos que usam a pasta; pode não existir.
    let _ = std::process::Command::new("fuser").args(["-vm", pasta]).status();
}

/// Lista (número, nome) dos processos cuja raiz é a imagem.
fn processos_dentro(raiz: &str) -> Vec<(u32, String)> {
    let mut lista = Vec::new();
    let Ok(entradas) = std::fs::read_dir("/proc") else { return lista };
    for entrada in entradas.flatten() {
        // Só as pastas com nome numérico são processos.
        let Ok(pid) = entrada.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Ok(raiz_do_processo) = std::fs::read_link(entrada.path().join("root")) else { continue };
        if raiz_do_processo.to_string_lossy() == raiz {
            let nome = std::fs::read_to_string(entrada.path().join("comm")).unwrap_or_default();
            lista.push((pid, nome.trim().to_string()));
        }
    }
    lista
}

// `Drop` é executado automaticamente quando o `Disco` deixa de existir,
// INCLUSIVE quando o programa para no meio por causa de um erro.
// Assim o disco nunca fica montado/ligado "pendurado".
impl Drop for Disco {
    fn drop(&mut self) {
        if self.finalizado {
            return; // `finalizar()` já fez tudo
        }
        println!("\n🧹 Desmontando e soltando o disco...");
        self.desmontar_tudo();
        self.soltar();
        self.remover_pasta();
    }
}


#[cfg(test)]
mod testes {
    use super::*;
    use crate::limpeza::apoio::raiz_de_teste;
    use std::process::Command;

    #[test]
    fn acha_so_processos_dentro_da_imagem() {
        // Nenhum processo deste computador tem a raiz numa pasta inventada...
        assert!(processos_dentro("/mnt/maloki-pasta-que-nao-existe").is_empty());
        // ...e o próprio teste roda com a raiz "/" (ele aparece na lista de "/").
        let meu_pid = std::process::id();
        assert!(processos_dentro("/").iter().any(|(pid, _)| *pid == meu_pid));
    }

    #[test]
    fn finalizar_em_simulacao() {
        let exec = Executor { simular: true };
        let mut disco = Disco::conectar(exec, "/tmp/x.raw").unwrap();
        let raiz = disco.raiz.clone();
        disco.montar(&["/dev/loop0p4"], &raiz).unwrap();
        // Em simulação o "umount" sempre "dá certo".
        assert!(disco.finalizar().is_ok());
    }

    #[test]
    fn nomes_das_particoes() {
        let exec = Executor { simular: true };
        let disco = Disco::conectar(exec, "/tmp/x.raw").unwrap();
        assert_eq!(disco.particao(PARTICAO_EFI), "/dev/loop0p2");
        assert_eq!(disco.particao(PARTICAO_BOOT), "/dev/loop0p3");
        assert_eq!(disco.particao(PARTICAO_RAIZ), "/dev/loop0p4");
        assert!(disco.raiz.starts_with(PREFIXO_RAIZ));
    }

    /// Roda o sfdisk DE VERDADE num arquivo comum (não precisa de root)
    /// e confere a tabela de partições que ele criou.
    #[test]
    fn sfdisk_de_verdade_cria_as_4_particoes() {
        let pasta = raiz_de_teste();
        let arquivo = format!("{}/disco.raw", pasta);
        // Um "disco" vazio de 2 GB (o arquivo é esparso: quase não ocupa espaço).
        std::fs::File::create(&arquivo).unwrap().set_len(2 * 1024 * 1024 * 1024).unwrap();

        let exec = Executor { simular: false };
        exec.rodar_com_entrada("sfdisk", &["--quiet", "--wipe", "always", &arquivo], ROTEIRO_PARTICOES, false).unwrap();

        // --dump mostra a tabela num formato de texto fácil de conferir.
        let saida = Command::new("sfdisk").args(["--dump", &arquivo]).output().unwrap();
        let tabela = String::from_utf8_lossy(&saida.stdout).to_uppercase();
        let linhas: Vec<&str> = tabela.lines().filter(|l| l.contains(" : START=")).collect();
        assert_eq!(linhas.len(), 4, "{}", tabela);

        assert!(tabela.contains("LABEL: GPT"));
        assert!(linhas[0].contains("SIZE=        2048") || linhas[0].contains("SIZE=2048"), "BIOS = 1 MiB: {}", linhas[0]);
        assert!(linhas[1].contains("SIZE=     1228800") || linhas[1].contains("SIZE=1228800"), "EFI = 600 MiB: {}", linhas[1]);
        assert!(linhas[2].contains("SIZE=     2097152") || linhas[2].contains("SIZE=2097152"), "/boot = 1 GiB: {}", linhas[2]);
        assert!(tabela.contains("NAME=\"BOOT\"") && tabela.contains("NAME=\"RAIZ\""));

        // LIÇÃO: não basta comparar o GUID com o mesmo texto que escrevemos no
        // roteiro (se ele estiver errado, o teste erra junto!). Perguntamos ao
        // sfdisk o NOME de cada tipo: ele só sabe o nome se o GUID for o oficial.
        let saida = Command::new("sfdisk")
            .args(["--list", "--output", "Device,Type", &arquivo])
            .env("LC_ALL", "C") // nomes em inglês, iguais em qualquer computador
            .output()
            .unwrap();
        let lista = String::from_utf8_lossy(&saida.stdout);
        let tipos: Vec<&str> = lista.lines().filter(|l| l.contains("disco.raw")).skip(1).collect();
        assert_eq!(tipos.len(), 4, "{}", lista);
        assert!(tipos[0].ends_with("BIOS boot"), "{}", tipos[0]);
        assert!(tipos[1].ends_with("EFI System"), "{}", tipos[1]);
        assert!(tipos[2].ends_with("Linux filesystem"), "{}", tipos[2]);
        assert!(tipos[3].ends_with("Linux filesystem"), "{}", tipos[3]);
        // O /boot inteiro tem que caber no começo do disco (abaixo de 2 GiB).
        let fim_do_boot: u64 = 2048 + 2048 + 1_228_800 + 2_097_152; // setores de 512 bytes
        assert!(fim_do_boot * 512 < 2 * 1024 * 1024 * 1024); // u64: 2 GiB não cabe num i32
    }
}
