# Segurança

O maloki roda como **root** e decide o que **não** pode ir para uma imagem que
será compartilhada. Por isso, dois tipos de problema são tratados como falha
de segurança:

1. **Vazamento:** um dado pessoal ou segredo (senha, chave, token...) que
   deveria ser removido e vai parar na imagem.
2. **Dano ao sistema:** qualquer caminho em que o maloki altere ou apague
   arquivos **fora** da imagem que está criando.

## Como relatar

**Não abra uma issue pública** para isso. Use o relato privado do GitHub:
aba **Security → Report a vulnerability** deste repositório.

Inclua a distribuição, o que vazou ou foi alterado e como reproduzir.
Você recebe uma resposta assim que possível, e o crédito no CHANGELOG (se quiser).

## Versões com correções

Só a versão mais recente recebe correções.
