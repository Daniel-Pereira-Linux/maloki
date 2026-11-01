// Bancos de dados gravam o tempo todo. Copiar os arquivos deles com o
// programa rodando pode gerar uma cópia "pela metade" (corrompida).
// Por isso oferecemos parar esses serviços durante a cópia e ligá-los de novo.

use crate::executor::Executor;
use crate::Resultado;
use std::process::Command;

// Serviços que costumam guardar dados em arquivos que mudam o tempo todo.
const SENSIVEIS: &[&str] = &[
    "postgresql", "mariadb", "mysql", "mysqld", "mongod", "redis", "redis-server",
    "influxdb", "elasticsearch", "opensearch", "slapd",
];

/// Quais dos serviços sensíveis estão ligados agora.
pub fn ativos() -> Vec<String> {
    SENSIVEIS
        .iter()
        // `systemctl is-active --quiet` não escreve nada: só responde pelo código de saída.
        .filter(|s| {
            Command::new("systemctl")
                .args(["is-active", "--quiet", s])
                .status()
                .map(|st| st.success())
                .unwrap_or(false)
        })
        .map(|s| s.to_string())
        .collect()
}

/// Enquanto existir, os serviços ficam parados. Quando ela deixa de existir
/// (fim da cópia, erro ou Ctrl+C), o `Drop` liga os serviços de novo.
pub struct Pausa {
    exec: Executor,
    servicos: Vec<String>,
}

pub fn pausar(exec: &Executor, servicos: &[String]) -> Resultado<Pausa> {
    // Criamos a Pausa ANTES de parar: se o 2º stop falhar, o Drop religa o 1º.
    let mut pausa = Pausa { exec: *exec, servicos: Vec::new() };
    for servico in servicos {
        exec.rodar("systemctl", &["stop", servico])?;
        pausa.servicos.push(servico.clone());
    }
    Ok(pausa)
}

impl Drop for Pausa {
    fn drop(&mut self) {
        // Liga na ordem inversa (como tirar pratos de uma pilha).
        for servico in self.servicos.iter().rev() {
            let _ = self.exec.rodar("systemctl", &["start", servico]);
        }
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn pausa_em_simulacao_nao_falha() {
        let exec = Executor { simular: true };
        let pausa = pausar(&exec, &["postgresql".to_string(), "redis".to_string()]).unwrap();
        assert_eq!(pausa.servicos, ["postgresql", "redis"]);
        drop(pausa); // religaria os dois (em simulação, só mostra)
    }

    #[test]
    fn lista_de_servicos_tem_nomes_validos() {
        for s in SENSIVEIS {
            assert!(!s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c == '-'), "{}", s);
        }
        // Rodar a detecção de verdade não pode travar nem dar pânico.
        let _ = ativos();
    }
}
