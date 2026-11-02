#!/bin/sh
# Roda DENTRO da sessão live da ISO (como root), vindo do "CD de testes".
# 1. confere a sessão live; 2. instala no disco /dev/vda como o Calamares faria.
# uso: rodar-no-live.sh MODO   (bios | uefi)
AQUI=$(dirname "$0")
sh "$AQUI/checar-live.sh"
sh "$AQUI/instalar-como-calamares.sh" "$1" "$AQUI"
