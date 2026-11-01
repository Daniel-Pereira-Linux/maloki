## O que muda

<!-- Uma ou duas frases. Fecha alguma issue? Escreva "Fecha #123". -->

## Como testei

- [ ] `cargo test` verde
- [ ] `cargo clippy --all-targets` sem avisos
- [ ] Testei em alguma distro de verdade (qual?):

## Checklist do projeto ([guia, seção 12](../blob/main/GUIA-DO-CODIGO.md#12-checklist-antes-de-abrir-o-pull-request))

- [ ] Toda remoção de arquivo usa `na_imagem()`
- [ ] Todo comando passa pelo `Executor`
- [ ] Código novo tem teste
- [ ] Atualizei README / manpage / exemplo-plano.toml, se o comportamento mudou
