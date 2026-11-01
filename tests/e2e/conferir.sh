#!/bin/bash
# Roda DENTRO da VM clonada (enviado pelo rodar.sh via SSH) e confere se o
# clone ficou certo. Cada linha "check" é uma verificação: descrição + teste.
# uso: conferir.sh DISTRO MODO GRUPO_ADMIN CHAVE_TLS MACHINE_ID_ORIGINAL CHAVE_SSH_ORIGINAL
DISTRO=$1 MODO=$2 GRUPO=$3 CHAVE_TLS=$4 MID_ORIG=$5 FP_ORIG=$6
ok=0; falha=0
check() { if eval "$2" >/dev/null 2>&1; then echo "✔ $1"; ok=$((ok+1)); else echo "✘ $1"; falha=$((falha+1)); fi; }
S() { echo Senha123 | sudo -S -p "" "$@"; }   # sudo com a senha da maria

# Firmware: o modo em que a VM ligou
case $MODO in
  bios) check "ligou em BIOS" '[ ! -d /sys/firmware/efi ]';;
  uefi) check "ligou em UEFI" '[ -d /sys/firmware/efi ]';;
  secureboot) check "ligou com Secure Boot ATIVO" \
    '[ "$(od -An -t u1 /sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c | awk "{print \$NF}")" = 1 ]';;
esac

# Disco e boot
check "hostname é clone-$DISTRO"                  '[ "$(hostname)" = "clone-$DISTRO" ]'
check "raiz é ext4 (não btrfs/xfs/LUKS)"          '[ "$(findmnt -no FSTYPE /)" = ext4 ]'
check "/boot/efi montado (vfat)"                  '[ "$(findmnt -no FSTYPE /boot/efi)" = vfat ]'
check "linha de boot sem luks/subvol"             '! grep -qE "luks|subvol" /proc/cmdline'
check "sem serviços com falha"                    '[ -z "$(systemctl --failed --no-legend --plain)" ]'
# (Só "EXT4-fs error" conta. O aviso "orphan cleanup on readonly fs" é normal em
#  sistemas formatados com o recurso orphan_file, como o Fedora 44.)
check "sistema de arquivos sem erros no boot"     '! S dmesg | grep -q "EXT4-fs error"'

# Usuários
check "usuária maria existe"                      'id maria'
check "maria está no grupo $GRUPO"                'id -nG maria | grep -qw $GRUPO'
check "senha da maria funciona no sudo"           'S true'
check "usuário daniel NÃO existe"                 '! id daniel'
check "/home/daniel NÃO existe"                   '[ ! -e /home/daniel ]'
check "maria é o único usuário humano"            '[ "$(awk -F: "\$3>=1000 && \$3<60000 {print \$1}" /etc/passwd)" = maria ]'
check "root bloqueado"                            'S passwd -S root | grep -qE "^root (L|LK) "'
check "nenhuma regra de sudo do daniel"           '! S grep -rqsw daniel /etc/sudoers /etc/sudoers.d'

# Identidade da máquina
check "machine-id novo"                           '[ -s /etc/machine-id ] && [ "$(cat /etc/machine-id)" != "$MID_ORIG" ]'
check "chave SSH do servidor nova"                '[ "$(ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub | cut -d" " -f2)" != "$FP_ORIG" ]'

# Rede: a original tinha IP FIXO amarrado a outra placa (MAC)
check "IP veio do DHCP (rede automática)"         'ip -4 addr show | grep -q dynamic'
check "nenhum arquivo com o IP fixo antigo"       '! S grep -rqs "10.0.2.15/24" /etc/netplan /etc/NetworkManager /etc/sysconfig/network-scripts /etc/network /etc/systemd/network'
check "internet funciona (DNS + rede)"            'getent hosts debian.org'

# Segredos
check "VPN (wireguard) sumiu"                     '! S test -e /etc/wireguard/wg0.conf'
check "chave TLS ficou (só avisada, não apagada)" 'S test -e $CHAVE_TLS'
check "crypttab vazio"                            '[ ! -s /etc/crypttab ]'

if [ -f /etc/selinux/config ] && command -v getenforce >/dev/null; then
  check "SELinux Enforcing"                       '[ "$(getenforce)" = Enforcing ]'
fi

systemctl --failed --no-legend --plain | sed "s/^/   serviço com falha: /"
echo "$ok ok, $falha falha(s)"
