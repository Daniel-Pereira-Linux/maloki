#!/bin/sh
# Confere a sessão live da ISO gerada pelo maloki.
ok=0; falha=0
check() { if eval "$2" >/dev/null 2>&1; then echo "✔ live: $1"; ok=$((ok+1)); else echo "✘ live: $1"; falha=$((falha+1)); fi; }

check "a raiz é a camada na memória (overlay) sobre o squashfs" '[ "$(findmnt -no FSTYPE /)" = overlay ] || findmnt -no SOURCE / | grep -q live'
SQUASH=$(sed -n 's/.*source: *"\(.*\)".*/\1/p' /etc/calamares/modules/unpackfs.conf)
check "o squashfs está onde o Calamares vai procurar ($SQUASH)" '[ -f "$SQUASH" ]'
check "liveuser existe e tem sudo sem senha"   'su - liveuser -c "sudo -n true"'
check "usuário da máquina original (daniel) NÃO existe" '! id daniel'
check "/home só tem o liveuser"               '[ "$(ls /home)" = liveuser ]'
check "VPN da original não veio"              '[ ! -e /etc/wireguard/wg0.conf ]'
check "configuração do Calamares instalada"   '[ -f /etc/calamares/settings.conf ] && [ -f /etc/calamares/branding/maloki/branding.desc ]'
check "pos-instalacao do maloki presente"     '[ -x /usr/libexec/maloki/pos-instalacao ]'
check "atalho do instalador no menu"          '[ -f /usr/share/applications/maloki-instalador.desktop ]'
check "login automático configurado"          'grep -rqs "liveuser" /etc/gdm/custom.conf /etc/gdm3/custom.conf /etc/sddm.conf.d /etc/lightdm/lightdm.conf.d'

# A tela de login sobe e entra sozinha no liveuser (pode demorar um pouco).
i=0; until loginctl list-sessions --no-legend 2>/dev/null | grep -v tty | grep -q liveuser || [ $i -ge 60 ]; do sleep 2; i=$((i+1)); done
check "sessão gráfica do liveuser aberta sozinha" 'loginctl list-sessions --no-legend | grep liveuser | grep -vq ttyS0'

# O Calamares abre e carrega TODOS os módulos sem erro de configuração.
# (offscreen = desenha numa tela invisível; -d = escreve o log detalhado.)
QT_QPA_PLATFORM=offscreen timeout 60 calamares -d > /tmp/calamares.log 2>&1
# (Os avisos do teclado e do disco são do ambiente sem tela, não da configuração.)
ERROS_DE_CONFIGURACAO="bad config|yaml|failed to load|could not load|configuration error|no config file|ERROR:|ERROR \(|not found or invalid|is undefined"
check "Calamares carregou a configuração sem erros" '! grep -E "$ERROS_DE_CONFIGURACAO" /tmp/calamares.log | grep -qvi "keyboard\|smartctl\|smart output"'
check "Calamares carregou o módulo do maloki (shellprocess@maloki)" 'grep -q "shellprocess@maloki" /tmp/calamares.log'
grep -iE "$ERROS_DE_CONFIGURACAO|warning" /tmp/calamares.log | head -15 | sed 's/^/   calamares: /'
echo "live: $ok ok, $falha falha(s)"
