#!/bin/sh
# Faz, sem tela, o que o Calamares faz na instalação, usando a configuração
# que o maloki escreveu: particiona, copia o squashfs (unpackfs), cria um
# usuário e roda o pos-instalacao do maloki dentro do sistema novo.
# uso: instalar-como-calamares.sh MODO PASTA_DO_CD   (bios | uefi)
MODO=$1 CD=$2
DISCO=/dev/vda ALVO=/tmp/calamares-root CONF=/etc/calamares/modules
ok=0; falha=0
check() { if eval "$2" >/dev/null 2>&1; then echo "✔ instalação: $1"; ok=$((ok+1)); else echo "✘ instalação: $1"; falha=$((falha+1)); fi; }
passo() { echo "--- $*"; }

passo "particionando ($MODO)"
wipefs -aq $DISCO
if [ "$MODO" = uefi ]; then
    printf 'label: gpt\nsize=512MiB, type=U\ntype=L\n' | sfdisk -q $DISCO
    EFI=${DISCO}1 RAIZ=${DISCO}2
else
    printf 'label: dos\ntype=83, bootable\n' | sfdisk -q $DISCO
    RAIZ=${DISCO}1
fi
udevadm settle
mkfs.ext4 -q -F $RAIZ
mkdir -p $ALVO && mount $RAIZ $ALVO
if [ "$MODO" = uefi ]; then
    mkfs.vfat -F 32 $EFI >/dev/null
    mkdir -p $ALVO/boot/efi && mount $EFI $ALVO/boot/efi
fi

passo "unpackfs: copiando o sistema"
SQUASH=$(sed -n 's/.*source: *"\(.*\)".*/\1/p' $CONF/unpackfs.conf)
check "unsquashfs do squashfs da ISO" 'unsquashfs -q -f -d $ALVO "$SQUASH"'

passo "machineid, fstab, usuário"
systemd-machine-id-setup --root=$ALVO >/dev/null
{
    echo "UUID=$(blkid -s UUID -o value $RAIZ)  /  ext4  defaults  0 1"
    [ "$MODO" = uefi ] && echo "UUID=$(blkid -s UUID -o value $EFI)  /boot/efi  vfat  umask=0077  0 2"
} > $ALVO/etc/fstab
echo instalado > $ALVO/etc/hostname
# As montagens que o mount.conf do maloki pede (iguais às do Calamares).
mount -t proc proc $ALVO/proc
mount -t sysfs sys $ALVO/sys
mount --bind /dev $ALVO/dev
mount -t tmpfs tmpfs $ALVO/run
mkdir -p $ALVO/run/udev && mount --bind /run/udev $ALVO/run/udev
[ "$MODO" = uefi ] && mount -t efivarfs efivarfs $ALVO/sys/firmware/efi/efivars
GRUPO=$(sed -n 's/^sudoersGroup: *//p' $CONF/users.conf)
chroot $ALVO useradd -m -s /bin/bash -G "users,$GRUPO" ana
chroot $ALVO usermod -p "$(cat $CD/hash)" ana
mkdir -p $ALVO/home/ana/.ssh && cp $CD/chave.pub $ALVO/home/ana/.ssh/authorized_keys
chroot $ALVO chown -R ana:ana /home/ana/.ssh

passo "shellprocess@maloki: o pos-instalacao do maloki"
COMANDO=$(sed -n 's/.*command: *"\(.*\)".*/\1/p' $CONF/shellprocess-maloki.conf)
chroot $ALVO /bin/sh -c "$COMANDO" > /tmp/pos.log 2>&1
codigo=$?
sed 's/^/   pos-instalacao: /' /tmp/pos.log | tail -25
check "pos-instalacao terminou sem erro (código $codigo)" '[ $codigo = 0 ]'

passo "umount"
sync
check "desmontou tudo" 'umount -R $ALVO'
echo "instalação: $ok ok, $falha falha(s)"
