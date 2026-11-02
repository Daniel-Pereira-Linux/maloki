// O "Executor" é quem roda os comandos do Linux e mexe nos arquivos.
//
// Todo comando passa por aqui por dois motivos:
//   1. Ele mostra na tela o que está sendo feito (você aprende vendo).
//   2. No modo --simular ele só MOSTRA, sem executar nada. Assim dá para
//      ver o passo a passo inteiro sem risco nenhum.

use crate::Resultado;
use std::fs;
use std::io::{ErrorKind, Write};
use std::process::{Command, Stdio};

// `#[derive(Clone, Copy)]` deixa copiar o Executor livremente (ele é só um bool),
// assim cada parte do programa pode ter a sua cópia.
#[derive(Clone, Copy)]
pub struct Executor {
    pub simular: bool,
}

impl Executor {
    /// Mostra o comando na tela, no formato que você digitaria no terminal.
    fn mostrar(&self, programa: &str, args: &[&str]) {
        let prefixo = if self.simular { "[simulação] $" } else { "  $" };
        println!("{} {} {}", prefixo, programa, args.join(" "));
    }

    /// Roda um comando e dá erro se ele falhar.
    pub fn rodar(&self, programa: &str, args: &[&str]) -> Resultado<()> {
        self.rodar_aceitando(programa, args, &[])
    }

    /// Igual a `rodar`, mas na tela o `segredo` (ex.: o hash de uma senha)
    /// aparece como "********".
    pub fn rodar_com_segredo(&self, programa: &str, args: &[&str], segredo: &str) -> Resultado<()> {
        let na_tela: Vec<&str> = args.iter().map(|a| if *a == segredo { "********" } else { a }).collect();
        self.mostrar(programa, &na_tela);
        if self.simular {
            return Ok(());
        }
        let status = Command::new(programa)
            .args(args)
            .status()
            .map_err(|e| format!("não consegui executar '{}': {}", programa, e))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("o comando '{}' falhou (código {})", programa, status.code().unwrap_or(-1)).into())
        }
    }

    /// Igual a `rodar`, mas aceita alguns códigos de saída como "deu certo".
    /// (O rsync, por exemplo, devolve 24 quando um arquivo sumiu durante a cópia,
    /// o que é normal ao copiar um sistema que está ligado.)
    pub fn rodar_aceitando(&self, programa: &str, args: &[&str], codigos_ok: &[i32]) -> Resultado<()> {
        self.mostrar(programa, args);
        if self.simular {
            return Ok(());
        }
        // status() roda o comando e espera ele terminar; a saída vai direto para a tela.
        let status = Command::new(programa)
            .args(args)
            .status()
            .map_err(|e| format!("não consegui executar '{}': {}", programa, e))?;

        let codigo = status.code().unwrap_or(-1);
        if status.success() || codigos_ok.contains(&codigo) {
            Ok(())
        } else {
            // `.into()` transforma a String no tipo de erro que a função devolve.
            Err(format!("o comando '{}' falhou (código {})", programa, codigo).into())
        }
    }

    /// Roda um comando enviando um texto para a entrada dele (como `echo texto | comando`).
    /// `secreto = true` esconde o texto na tela (usado para senhas).
    pub fn rodar_com_entrada(&self, programa: &str, args: &[&str], entrada: &str, secreto: bool) -> Resultado<()> {
        self.mostrar(programa, args);
        if !secreto {
            println!("      (entrada: {})", entrada.trim().replace('\n', " | "));
        }
        if self.simular {
            return Ok(());
        }
        let mut filho = Command::new(programa).args(args).stdin(Stdio::piped()).spawn()?;
        // Escrevemos na entrada do processo. Quando `stdin` sai do escopo (fim do
        // bloco), ele é fechado e o programa entende que a entrada acabou.
        {
            let mut stdin = filho.stdin.take().ok_or("sem stdin")?;
            stdin.write_all(entrada.as_bytes())?;
        }
        let status = filho.wait()?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("o comando '{}' falhou", programa).into())
        }
    }

    /// Roda um comando e devolve o que ele escreveu na tela (ex.: o UUID do blkid).
    /// No modo simulação devolve `valor_simulado`, porque o comando não rodou.
    pub fn capturar(&self, programa: &str, args: &[&str], valor_simulado: &str) -> Resultado<String> {
        self.mostrar(programa, args);
        if self.simular {
            return Ok(valor_simulado.to_string());
        }
        let saida = Command::new(programa).args(args).output()?;
        if !saida.status.success() {
            return Err(format!("o comando '{}' falhou", programa).into());
        }
        Ok(String::from_utf8_lossy(&saida.stdout).trim().to_string())
    }

    /// Cria (ou substitui) um arquivo com o conteúdo informado.
    pub fn escrever(&self, caminho: &str, conteudo: &str) -> Resultado<()> {
        if self.simular {
            println!("[simulação] escreveria {} ({} linhas)", caminho, conteudo.lines().count());
            return Ok(());
        }
        println!("  ✎ escrevendo {}", caminho);
        // Cria as pastas do caminho, se faltarem (como `mkdir -p`).
        if let Some(pasta) = std::path::Path::new(caminho).parent() {
            fs::create_dir_all(pasta)?;
        }
        fs::write(caminho, conteudo)?;
        Ok(())
    }

    /// Igual a `escrever`, para conteúdo que não é texto (ex.: uma imagem .png).
    pub fn escrever_bytes(&self, caminho: &str, conteudo: &[u8]) -> Resultado<()> {
        if self.simular {
            println!("[simulação] escreveria {} ({} bytes)", caminho, conteudo.len());
            return Ok(());
        }
        println!("  ✎ escrevendo {}", caminho);
        if let Some(pasta) = std::path::Path::new(caminho).parent() {
            fs::create_dir_all(pasta)?;
        }
        fs::write(caminho, conteudo)?;
        Ok(())
    }

    /// Troca o nome de um arquivo. Se `destino` existir, ele é substituído
    /// de uma vez só (nunca fica "meio velho, meio novo").
    pub fn renomear(&self, origem: &str, destino: &str) -> Resultado<()> {
        if self.simular {
            println!("[simulação] renomearia {} -> {}", origem, destino);
            return Ok(());
        }
        fs::rename(origem, destino).map_err(|e| format!("não consegui renomear {} para {}: {}", origem, destino, e))?;
        Ok(())
    }

    /// Apaga um arquivo ou uma pasta inteira. Se já não existir, tudo bem.
    pub fn apagar(&self, caminho: &str) -> Resultado<()> {
        if self.simular {
            println!("[simulação] apagaria {}", caminho);
            return Ok(());
        }
        // symlink_metadata não segue links: um link para pasta é apagado como arquivo,
        // sem entrar na pasta para onde ele aponta.
        let resultado = match fs::symlink_metadata(caminho) {
            Ok(info) if info.is_dir() => fs::remove_dir_all(caminho),
            Ok(_) => fs::remove_file(caminho),
            Err(e) => Err(e),
        };
        match resultado {
            Ok(()) => {
                println!("  ✗ apagado {}", caminho);
                Ok(())
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("não consegui apagar {}: {}", caminho, e).into()),
        }
    }

    /// Apaga tudo o que está DENTRO de uma pasta, mas mantém a pasta.
    pub fn esvaziar_pasta(&self, pasta: &str) -> Resultado<()> {
        if self.simular {
            println!("[simulação] esvaziaria {}/", pasta);
            return Ok(());
        }
        let Ok(entradas) = fs::read_dir(pasta) else {
            return Ok(()); // a pasta não existe: nada a fazer
        };
        for entrada in entradas.flatten() {
            self.apagar(&entrada.path().to_string_lossy())?;
        }
        Ok(())
    }

    /// Lê um arquivo, passa o texto pela função `mudar` e grava o resultado.
    /// Se o arquivo não existir, não faz nada.
    pub fn editar(&self, caminho: &str, mudar: impl Fn(&str) -> String) -> Resultado<()> {
        if self.simular {
            println!("[simulação] ajustaria {}", caminho);
            return Ok(());
        }
        let Ok(texto) = fs::read_to_string(caminho) else {
            return Ok(());
        };
        let novo = mudar(&texto);
        if novo != texto {
            println!("  ✎ ajustando {}", caminho);
            fs::write(caminho, novo)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::limpeza::apoio::*;

    const DE_VERDADE: Executor = Executor { simular: false };
    const SIMULANDO: Executor = Executor { simular: true };

    #[test]
    fn rodar_sucesso_e_falha() {
        assert!(DE_VERDADE.rodar("true", &[]).is_ok());
        assert!(DE_VERDADE.rodar("false", &[]).is_err());
        assert!(DE_VERDADE.rodar("programa-que-nao-existe-xyz", &[]).is_err());
    }

    #[test]
    fn rodar_aceitando_codigos() {
        assert!(DE_VERDADE.rodar_aceitando("sh", &["-c", "exit 24"], &[24]).is_ok());
        assert!(DE_VERDADE.rodar_aceitando("sh", &["-c", "exit 23"], &[24]).is_err());
        assert!(DE_VERDADE.rodar("sh", &["-c", "exit 24"]).is_err());
    }

    #[test]
    fn simulacao_nao_roda_nada() {
        // `false` sempre falha; na simulação ele nem roda, então "dá certo".
        assert!(SIMULANDO.rodar("false", &[]).is_ok());
        assert!(SIMULANDO.rodar("programa-que-nao-existe-xyz", &[]).is_ok());
        assert_eq!(SIMULANDO.capturar("false", &[], "VALOR").unwrap(), "VALOR");
    }

    #[test]
    fn entrada_chega_no_comando() {
        let ok = "read linha; [ \"$linha\" = \"maria:segredo\" ]";
        assert!(DE_VERDADE.rodar_com_entrada("sh", &["-c", ok], "maria:segredo\n", true).is_ok());
        assert!(DE_VERDADE.rodar_com_entrada("sh", &["-c", ok], "outra\n", true).is_err());
    }

    #[test]
    fn capturar_saida() {
        assert_eq!(DE_VERDADE.capturar("echo", &["  abc-123  "], "x").unwrap(), "abc-123");
        assert!(DE_VERDADE.capturar("false", &[], "x").is_err());
    }

    #[test]
    fn escrever_cria_pastas() {
        let raiz = raiz_de_teste();
        let arquivo = format!("{}/a/b/c.txt", raiz);
        DE_VERDADE.escrever(&arquivo, "oi").unwrap();
        assert_eq!(ler(&raiz, "a/b/c.txt"), "oi");
        DE_VERDADE.escrever(&arquivo, "trocado").unwrap(); // sobrescreve
        assert_eq!(ler(&raiz, "a/b/c.txt"), "trocado");
    }

    #[test]
    fn apagar_arquivo_pasta_e_inexistente() {
        let raiz = raiz_de_teste();
        criar(&raiz, "arquivo", "x");
        criar(&raiz, "pasta/sub/f", "x");
        DE_VERDADE.apagar(&format!("{}/arquivo", raiz)).unwrap();
        DE_VERDADE.apagar(&format!("{}/pasta", raiz)).unwrap();
        DE_VERDADE.apagar(&format!("{}/nao-existe", raiz)).unwrap(); // sem erro
        assert!(!existe(&raiz, "arquivo"));
        assert!(!existe(&raiz, "pasta"));
    }

    #[test]
    fn apagar_link_nao_apaga_o_destino() {
        let raiz = raiz_de_teste();
        let fora = raiz_de_teste();
        criar(&fora, "precioso", "x");
        std::os::unix::fs::symlink(fora.as_str(), format!("{}/link", raiz)).unwrap();
        DE_VERDADE.apagar(&format!("{}/link", raiz)).unwrap();
        assert!(!existe(&raiz, "link"));
        assert!(existe(&fora, "precioso"));
    }

    #[test]
    fn esvaziar_mantem_a_pasta() {
        let raiz = raiz_de_teste();
        criar(&raiz, "p/a", "x");
        criar(&raiz, "p/sub/b", "x");
        DE_VERDADE.esvaziar_pasta(&format!("{}/p", raiz)).unwrap();
        assert!(existe(&raiz, "p"));
        assert!(!existe(&raiz, "p/a"));
        assert!(!existe(&raiz, "p/sub"));
        // Pasta que não existe: nada a fazer, sem erro.
        DE_VERDADE.esvaziar_pasta(&format!("{}/nao-existe", raiz)).unwrap();
    }

    #[test]
    fn editar_muda_so_quando_existe() {
        let raiz = raiz_de_teste();
        criar(&raiz, "f", "abc");
        DE_VERDADE.editar(&format!("{}/f", raiz), |t| t.to_uppercase()).unwrap();
        assert_eq!(ler(&raiz, "f"), "ABC");
        DE_VERDADE.editar(&format!("{}/nao-existe", raiz), |t| t.to_uppercase()).unwrap();
        assert!(!existe(&raiz, "nao-existe")); // não cria arquivo novo
    }

    #[test]
    fn simulacao_nao_mexe_em_arquivos() {
        let raiz = raiz_de_teste();
        criar(&raiz, "f", "original");
        SIMULANDO.escrever(&format!("{}/novo", raiz), "x").unwrap();
        SIMULANDO.apagar(&format!("{}/f", raiz)).unwrap();
        SIMULANDO.editar(&format!("{}/f", raiz), |_| "mudou".into()).unwrap();
        SIMULANDO.esvaziar_pasta(&raiz).unwrap();
        assert!(!existe(&raiz, "novo"));
        assert_eq!(ler(&raiz, "f"), "original");
    }
}
