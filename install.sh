#!/bin/sh
# Instalador do maloki.
#
#   curl -fsSL https://daniel-pereira-linux.github.io/maloki/install.sh | sh
#
# O que este script faz (leia antes de rodar, é curto!):
#   1. baixa a última versão do GitHub Releases
#   2. confere o checksum SHA-256 (garante que o arquivo veio inteiro e sem alteração)
#   3. instala o programa em /usr/local/bin e o manual em /usr/local/share/man/man1
#
# Variáveis opcionais:
#   MALOKI_VERSAO=v0.1.0   instala uma versão específica (padrão: a mais nova)
#   PREFIXO=$HOME/.local   instala em outra pasta (sem precisar de sudo)
set -eu

REPO="Daniel-Pereira-Linux/maloki"
VERSAO="${MALOKI_VERSAO:-latest}"
PREFIXO="${PREFIXO:-/usr/local}"
ARQUIVO="maloki-x86_64-linux.tar.gz"

erro() { printf '\033[31merro:\033[0m %s\n' "$1" >&2; exit 1; }
info() { printf '\033[36m==>\033[0m %s\n' "$1"; }

[ "$(uname -s)" = Linux ] || erro "o maloki só funciona no Linux."
[ "$(uname -m)" = x86_64 ] || erro "por enquanto só há versão para x86_64 (este computador é $(uname -m))."
command -v curl >/dev/null || erro "precisa do curl."
command -v sha256sum >/dev/null || erro "precisa do sha256sum (pacote coreutils)."

if [ "$VERSAO" = latest ]; then
    URL="https://github.com/$REPO/releases/latest/download"
else
    URL="https://github.com/$REPO/releases/download/$VERSAO"
fi

TEMP=$(mktemp -d)
trap 'rm -rf "$TEMP"' EXIT

info "Baixando $ARQUIVO ($VERSAO)..."
curl -fsSL -o "$TEMP/$ARQUIVO" "$URL/$ARQUIVO" || erro "não consegui baixar $URL/$ARQUIVO"
curl -fsSL -o "$TEMP/$ARQUIVO.sha256" "$URL/$ARQUIVO.sha256" || erro "não consegui baixar o checksum"

info "Conferindo o checksum SHA-256..."
(cd "$TEMP" && sha256sum -c "$ARQUIVO.sha256" >/dev/null) || erro "o checksum NÃO confere: o arquivo pode estar corrompido."

tar -xzf "$TEMP/$ARQUIVO" -C "$TEMP"

# Precisa de sudo só se a pasta de destino não for sua.
SUDO=""
if [ ! -w "$PREFIXO" ] && [ "$(id -u)" != 0 ]; then
    command -v sudo >/dev/null || erro "sem permissão para $PREFIXO (use PREFIXO=\$HOME/.local)."
    SUDO="sudo"
    info "Instalar em $PREFIXO precisa de sudo:"
fi

$SUDO install -Dm755 "$TEMP/maloki/maloki" "$PREFIXO/bin/maloki"
$SUDO install -Dm644 "$TEMP/maloki/maloki.1" "$PREFIXO/share/man/man1/maloki.1"

info "Instalado: $("$PREFIXO/bin/maloki" --ajuda >/dev/null 2>&1 && echo "$PREFIXO/bin/maloki")"
cat <<'FIM'

  Próximos passos:
    maloki --simular        mostra o passo a passo, sem mudar nada
    sudo maloki             cria a imagem (respondendo os menus)
    man maloki              o manual completo

FIM
