// Impede duas cópias do programa rodando ao mesmo tempo (as duas poderiam
// mexer na mesma pasta /mnt e uma estragaria o trabalho da outra).
//
// Como funciona: criamos um arquivo com o número do nosso processo (PID).
// Se o arquivo já existe e aquele processo ainda está vivo, outra cópia
// está rodando. Se o processo morreu (ex.: o PC desligou), a trava é velha
// e pode ser removida.

use crate::Resultado;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;

pub const ARQUIVO_TRAVA: &str = "/run/maloki.lock";

pub struct Trava {
    caminho: String,
}

pub fn pegar(caminho: &str) -> Resultado<Trava> {
    // Duas tentativas: a segunda só acontece depois de remover uma trava velha.
    for _ in 0..2 {
        // create_new só cria se o arquivo NÃO existir, e faz isso de uma vez só:
        // se dois programas tentarem juntos, só um consegue.
        match OpenOptions::new().write(true).create_new(true).open(caminho) {
            Ok(mut arquivo) => {
                writeln!(arquivo, "{}", std::process::id())?;
                return Ok(Trava { caminho: caminho.to_string() });
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                let pid = fs::read_to_string(caminho).unwrap_or_default().trim().to_string();
                // Todo processo vivo tem uma pasta /proc/<pid>.
                if !pid.is_empty() && Path::new(&format!("/proc/{}", pid)).exists() {
                    return Err(format!("o maloki já está rodando (processo {})", pid).into());
                }
                fs::remove_file(caminho)?; // trava velha: o processo não existe mais
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err("não consegui criar a trava".into())
}

// Ao terminar (bem ou mal), a trava some sozinha.
impl Drop for Trava {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.caminho);
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::limpeza::apoio::raiz_de_teste;

    #[test]
    fn segunda_copia_e_bloqueada_e_trava_some_no_fim() {
        let pasta = raiz_de_teste();
        let caminho = format!("{}/trava", pasta);
        let primeira = pegar(&caminho).unwrap();
        let erro = pegar(&caminho).err().unwrap().to_string();
        assert!(erro.contains("já está rodando"));
        drop(primeira);
        assert!(!Path::new(&caminho).exists());
        assert!(pegar(&caminho).is_ok()); // livre de novo
    }

    #[test]
    fn trava_velha_e_ignorada() {
        let pasta = raiz_de_teste();
        let caminho = format!("{}/trava", pasta);
        // Um PID que não existe (o máximo do Linux é bem menor que isso).
        fs::write(&caminho, "999999999\n").unwrap();
        let trava = pegar(&caminho).unwrap();
        assert_eq!(fs::read_to_string(&caminho).unwrap().trim(), std::process::id().to_string());
        drop(trava);
    }

    #[test]
    fn trava_vazia_ou_estragada_e_ignorada() {
        let pasta = raiz_de_teste();
        let caminho = format!("{}/trava", pasta);
        fs::write(&caminho, "").unwrap();
        assert!(pegar(&caminho).is_ok());
    }
}
