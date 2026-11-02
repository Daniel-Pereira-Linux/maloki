# Contribuindo com o maloki

Que bom que você quer ajudar! 🎉 Toda contribuição conta, inclusive as pequenas.

## Jeitos de ajudar (do mais fácil ao mais difícil)

1. **Testar numa distro e contar como foi.** Use o modelo
   ["Testei numa distro"](../../issues/new/choose). Contar que **funcionou** também ajuda muito.
2. **Relatar um bug**, com o modelo "Bug".
3. **Melhorar a documentação.** Achou algo confuso? Isso é um bug também.
4. **Mexer no código.** Veja as issues com
   [`boa primeira issue`](../../labels/boa%20primeira%20issue).

## Antes de mexer no código

Leia o **[GUIA-DO-CODIGO.md](GUIA-DO-CODIGO.md)**. Ele mostra por onde ler, qual
arquivo chama qual, os 6 padrões do projeto e receitas para as mudanças mais
comuns (inclusive "quero suportar uma distro nova").

```bash
cargo build
cargo test                     # tem que ficar tudo verde
cargo clippy --all-targets     # sem avisos
./target/debug/maloki --simular
```

## O Pull Request

- Um assunto por PR (fica mais fácil de revisar).
- Comentários no código em **português**, explicando o **porquê**.
- Siga o checklist do guia: [seção 12](GUIA-DO-CODIGO.md#12-checklist-antes-de-abrir-o-pull-request).
- Mexeu em boot, rede ou limpeza? Rode o e2e da distro: `tests/e2e/rodar.sh debian`.
- Mexeu na ISO (`live.rs`)? Rode `tests/e2e/rodar.sh iso-debian iso-fedora`.

## Convivência

Seja gentil. Muita gente aqui está aprendendo, e pergunta "boba" não existe.
