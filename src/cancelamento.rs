// Ctrl+C seguro.
//
// Normalmente o Ctrl+C mata o programa NA HORA, sem rodar o `Drop` do Disco.
// Resultado: o /dev/loop0 ficaria ligado e as pastas montadas em /mnt.
//
// Aqui trocamos esse comportamento: o Ctrl+C só "levanta uma bandeira".
//   - O comando que está rodando (rsync, dracut...) também recebe o Ctrl+C,
//     para e devolve erro; o `?` sai da função e o Drop limpa tudo.
//   - Entre um passo e outro, `verificar()` olha a bandeira e para com erro.

use crate::Resultado;
use std::sync::atomic::{AtomicBool, Ordering};

// `static` = uma variável única para o programa inteiro.
// `AtomicBool` = um bool que pode ser mexido com segurança por duas "linhas de
// execução" ao mesmo tempo (o aviso do Ctrl+C chega numa linha separada).
static CANCELADO: AtomicBool = AtomicBool::new(false);

/// Passa a tratar o Ctrl+C. Chamado só quando começamos a mexer no disco:
/// durante os menus, o Ctrl+C continua saindo na hora (não há nada a desfazer).
pub fn instalar() -> Resultado<()> {
    ctrlc::set_handler(|| {
        // swap devolve o valor antigo: a mensagem aparece só no primeiro Ctrl+C.
        if !CANCELADO.swap(true, Ordering::SeqCst) {
            eprintln!("\n⚠ Ctrl+C: parando com segurança, aguarde a limpeza...");
        }
    })?;
    Ok(())
}

/// Dá erro se a pessoa apertou Ctrl+C.
pub fn verificar() -> Resultado<()> {
    if CANCELADO.load(Ordering::SeqCst) {
        return Err("cancelado com Ctrl+C (nada ficou montado)".into());
    }
    Ok(())
}
