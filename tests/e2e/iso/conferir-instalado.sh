#!/bin/bash
# Roda (via SSH, como a usuária ana) no sistema INSTALADO a partir da ISO.
# uso: conferir-instalado.sh DISTRO MODO GRUPO
DISTRO=$1 MODO=$2 GRUPO=$3
ok=0; falha=0
check() { if eval "$2" >/dev/null 2>&1; then echo "✔ $1"; ok=$((ok+1)); else echo "✘ $1"; falha=$((falha+1)); fi; }
S() { echo Senha123 | sudo -S -p "" "$@"; }

case $MODO in
  bios) check "ligou em BIOS" '[ ! -d /sys/firmware/efi ]';;
  uefi) check "ligou em UEFI" '[ -d /sys/firmware/efi ]';;
  secureboot) check "ligou com Secure Boot ATIVO" \
    '[ "$(od -An -t u1 /sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c | awk "{print \$NF}")" = 1 ]';;
esac
check "hostname é o escolhido na instalação"      '[ "$(hostname)" = instalado ]'
check "raiz é ext4 no disco instalado"             '[ "$(findmnt -no FSTYPE /)" = ext4 ] && findmnt -no SOURCE / | grep -q vda'
# UUID (o padrão) ou PARTUUID (as imagens "cloud" do Debian usam esse), sempre do disco NOVO.
check "linha de boot aponta para o disco novo"     'grep -qE "root=(UUID=$(findmnt -no UUID /)|PARTUUID=$(lsblk -no PARTUUID "$(findmnt -no SOURCE /)"))( |$)" /proc/cmdline'
check "linha de boot sem nada da sessão live"      '! grep -qE "rd.live|boot=live|CDLABEL" /proc/cmdline'
check "usuária ana existe e está no grupo $GRUPO"  'id -nG ana | grep -qw $GRUPO'
check "senha da ana funciona no sudo"              'S true'
check "liveuser NÃO existe mais"                   '! id liveuser'
check "/home/liveuser NÃO existe"                  '[ ! -e /home/liveuser ]'
check "sudo sem senha da sessão live sumiu"        '! S test -e /etc/sudoers.d/maloki-live'
check "regra do polkit da sessão live sumiu"       '! S test -e /etc/polkit-1/rules.d/49-maloki-live.rules'
check "login automático do liveuser sumiu"         '! S grep -rqs "liveuser" /etc/gdm /etc/gdm3 /etc/sddm.conf.d /etc/lightdm'
check "configuração do Calamares e scripts sumiram" '[ ! -e /etc/calamares ] && [ ! -e /usr/libexec/maloki ] && [ ! -e /usr/lib/maloki ]'
check "atalho do instalador sumiu"                 '[ ! -e /usr/share/applications/maloki-instalador.desktop ]'
check "usuário da máquina original NÃO existe"     '! id daniel'
if [ "$DISTRO" != fedora ] && [ "$DISTRO" != rocky ]; then
  check "live-boot desinstalado"                   '! dpkg -s live-boot 2>/dev/null | grep -q "install ok installed"'
fi
check "sem serviços com falha"                     '[ -z "$(systemctl --failed --no-legend --plain)" ]'
check "sistema de arquivos sem erros no boot"      '! S dmesg | grep -q "EXT4-fs error"'
check "internet funciona (DNS + rede)"             'getent hosts debian.org'
if [ -f /etc/selinux/config ] && command -v getenforce >/dev/null; then
  check "SELinux Enforcing"                        '[ "$(getenforce)" = Enforcing ]'
fi
systemctl --failed --no-legend --plain | sed "s/^/   serviço com falha: /"
echo "$ok ok, $falha falha(s)"
