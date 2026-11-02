#!/usr/bin/env bash
# Testes de ponta a ponta (e2e): usa o maloki DE VERDADE em VMs
# descartáveis e liga as imagens geradas para conferir o resultado por dentro.
#
# uso:   tests/e2e/rodar.sh [rocky] [debian] [fedora] [ubuntu] [protecoes]
#                          [iso-debian] [iso-fedora] [iso-ubuntu]
#        (sem nada = rocky, debian, fedora, ubuntu, protecoes, iso-debian e iso-fedora)
#
# Template: clona a VM "original" num .qcow2 e liga o clone em BIOS, UEFI e
#   Secure Boot, conferindo tudo por dentro (conferir.sh).
# ISO: gera a ISO na VM original, liga a ISO em BIOS e em UEFI, entra na
#   sessão live pela porta serial, confere o Calamares e instala no disco do
#   jeito que o Calamares instalaria (iso/). Depois liga o sistema instalado
#   (BIOS, UEFI e Secure Boot) e confere por SSH (iso/conferir-instalado.sh).
#
# Precisa de: qemu-system-x86_64, qemu-img, genisoimage, openssl, ssh, python3,
# cargo com o alvo x86_64-unknown-linux-musl e o firmware OVMF (edk2-ovmf).
# NÃO precisa de root: tudo roda dentro das VMs.
#
# Imagens base: baixadas para ~/.cache/maloki-e2e, ou informe o arquivo
# com BASE_ROCKY=/caminho.qcow2 (idem BASE_DEBIAN, BASE_UBUNTU, BASE_FEDORA).
set -uo pipefail

AQUI=$(cd "$(dirname "$0")" && pwd)
PROJETO=$(cd "$AQUI/../.." && pwd)
TRABALHO=${TRABALHO:-$HOME/.cache/maloki-e2e}
OVMF=/usr/share/edk2/ovmf
mkdir -p "$TRABALHO"
cd "$TRABALHO" || exit 1

# Os discos das VMs de teste ficam aqui e crescem vários GB. Numa pasta em
# MEMÓRIA (tmpfs, como o /tmp de muitas distros) ou com pouco espaço, o disco
# virtual das VMs começa a falhar em silêncio e as imagens saem corrompidas,
# parecendo um bug do maloki. (Aprendemos isso do jeito difícil.)
if [ "$(findmnt -no FSTYPE --target "$TRABALHO")" = tmpfs ]; then
  echo "A pasta de trabalho $TRABALHO fica na memória (tmpfs). Use uma pasta no disco (TRABALHO=...)."; exit 1
fi
livre_gb=$(df --output=avail -BG "$TRABALHO" | tail -1 | tr -dc 0-9)
if [ "$livre_gb" -lt 20 ]; then
  echo "Só ${livre_gb} GB livres em $TRABALHO; os testes precisam de pelo menos 20 GB."; exit 1
fi

# ---------------- Configuração de cada distro ----------------
declare -A URL=(
  [rocky]=https://dl.rockylinux.org/pub/rocky/9/images/x86_64/Rocky-9-GenericCloud-Base.latest.x86_64.qcow2
  [debian]=https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2
  [ubuntu]=https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img
  [fedora]=https://download.fedoraproject.org/pub/fedora/linux/releases/44/Cloud/x86_64/images/Fedora-Cloud-Base-Generic-44-1.7.x86_64.qcow2
)
# Pacotes que o maloki precisa e que as imagens "cloud" não trazem.
declare -A PACOTES=(
  [rocky]="qemu-img rsync dosfstools"
  [debian]="qemu-utils rsync dosfstools grub-pc-bin grub-efi-amd64-bin grub-efi-amd64-signed shim-signed redis-server"
  [ubuntu]="qemu-utils rsync dosfstools grub-pc-bin"
  [fedora]="qemu-img rsync dosfstools grub2-pc-modules"
)
# Para gerar a ISO: Calamares, o initramfs live e uma tela de login simples
# (LightDM + Openbox; as imagens "cloud" não têm área de trabalho).
declare -A PACOTES_ISO=(
  [debian]="calamares live-boot squashfs-tools xorriso mtools grub-common lightdm openbox xserver-xorg"
  [ubuntu]="calamares live-boot squashfs-tools xorriso mtools grub-common grub-efi-amd64-bin lightdm openbox xserver-xorg"
  [fedora]="calamares dracut-live squashfs-tools xorriso mtools grub2-tools-extra grub2-efi-x64-modules lightdm openbox xorg-x11-server-Xorg"
)
declare -A GRUPO=([rocky]=wheel [debian]=sudo [ubuntu]=sudo [fedora]=wheel)
# Como responder: pelos menus (robô) ou pelo arquivo --plano.
declare -A MODO=([rocky]=menus [debian]=plano [ubuntu]=plano [fedora]=menus)
# Onde cada família guarda chaves TLS (a "chave secreta" de teste vai para lá).
declare -A CHAVE_TLS=([rocky]=/etc/pki/tls/private/teste.key [fedora]=/etc/pki/tls/private/teste.key
                      [debian]=/etc/ssl/private/teste.key [ubuntu]=/etc/ssl/private/teste.key)

PORTA_ORIGINAL=2310
PORTA_TESTE=2320
MAC_ORIGINAL=52:54:00:12:34:56   # a VM original tem IP FIXO amarrado a este MAC
MAC_TESTE=52:54:00:ab:cd:ef      # a VM de teste usa outra placa (como num PC novo)

RESUMO=()
falhou=0
anotar() { RESUMO+=("$1"); case "$1" in *✘*) falhou=1;; esac; echo "$1"; }

# ---------------- Ferramentas ----------------

SSH_OPCOES=(-q -i "$TRABALHO/chave" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=5 -o BatchMode=yes)
ssh_vm() { local porta=$1 usuario=$2; shift 2; ssh "${SSH_OPCOES[@]}" -p "$porta" "$usuario@127.0.0.1" "$@"; }
scp_vm() { scp "${SSH_OPCOES[@]}" "$@"; }

# Nunca liga uma VM se o computador estiver com pouca memória
# (aprendemos isso do jeito difícil: o Linux mata VMs quando a memória acaba).
memoria_ok() {
  local livre_mb
  livre_mb=$(awk '/MemAvailable/ {print int($2/1024)}' /proc/meminfo)
  if [ "$livre_mb" -lt 3500 ]; then
    echo "Pouca memória livre (${livre_mb} MB); não vou ligar outra VM."; return 1
  fi
}

baixar_base() {
  local distro=$1 var="BASE_${1^^}"
  if [ -n "${!var:-}" ]; then echo "${!var}"; return; fi
  local arquivo="$TRABALHO/base-$distro.qcow2"
  if [ ! -f "$arquivo" ]; then
    echo "Baixando a imagem base do $distro..." >&2
    curl -fL --progress-bar -o "$arquivo.part" "${URL[$distro]}" && mv "$arquivo.part" "$arquivo"
  fi
  echo "$arquivo"
}

# Liga a VM "original" (a que vai ser clonada), com coisas pessoais de propósito:
# IP fixo, VPN, regra de sudo para o daniel e uma chave TLS.
# extras: pacotes a mais (os da ISO); com eles, o disco e o tempo de espera crescem.
ligar_original() {
  local distro=$1 base=$2 extras=${3:-}
  memoria_ok || return 1
  local seed="$TRABALHO/seed-$distro"
  rm -rf "$seed"; mkdir -p "$seed"
  cat > "$seed/user-data" <<EOF
#cloud-config
hostname: original-$distro
users:
  - name: daniel
    groups: [${GRUPO[$distro]}]
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    ssh_authorized_keys: ["$(cat "$TRABALHO/chave.pub")"]
packages: [$(echo "${PACOTES[$distro]} $extras" | xargs | sed 's/ /, /g')]
write_files:
  - {path: /etc/wireguard/wg0.conf, permissions: '0600', content: "[Interface]\nPrivateKey = SEGREDO-DA-VPN\n"}
  - {path: ${CHAVE_TLS[$distro]}, permissions: '0600', content: "-----BEGIN PRIVATE KEY-----\nteste\n"}
EOF
  printf 'instance-id: %s-1\nlocal-hostname: original-%s\n' "$distro" "$distro" > "$seed/meta-data"
  # IP fixo amarrado ao MAC, igual às VMs do lab (é isso que o item "rede" corrige).
  cat > "$seed/network-config" <<EOF
version: 2
ethernets:
  rede:
    match: {macaddress: "$MAC_ORIGINAL"}
    addresses: [10.0.2.15/24]
    routes: [{to: 0.0.0.0/0, via: 10.0.2.2}]   # "0.0.0.0/0" = rota padrão (o cloud-init antigo não entende "default")
    nameservers: {addresses: [10.0.2.3]}
EOF
  genisoimage -quiet -output "$seed.iso" -volid cidata -joliet -rock "$seed"/*
  qemu-img create -q -f qcow2 -b "$base" -F qcow2 "original-$distro.qcow2" 40G
  qemu-system-x86_64 -name "original-$distro" -enable-kvm -cpu host -smp 4 -m 3072 \
    -drive file="original-$distro.qcow2",if=virtio -drive file="$seed.iso",if=virtio,media=cdrom,readonly=on \
    -nic user,model=virtio-net-pci,mac=$MAC_ORIGINAL,hostfwd=tcp:127.0.0.1:$PORTA_ORIGINAL-:22 \
    -display none -serial file:"original-$distro.console" -daemonize -pidfile "original-$distro.pid"
  local inicio=$SECONDS
  # (com sudo: no Fedora 44, o `cloud-init status` sem root dá "Permission denied")
  until ssh_vm $PORTA_ORIGINAL daniel 'sudo cloud-init status 2>/dev/null | grep -qE "done|error"' 2>/dev/null; do
    sleep 5
    if [ $((SECONDS - inicio)) -gt 2400 ]; then echo "a VM original não ficou pronta"; return 1; fi
  done
  if ! ssh_vm $PORTA_ORIGINAL daniel 'sudo cloud-init status | grep -q done'; then
    echo "o cloud-init da VM original deu erro:"
    ssh_vm $PORTA_ORIGINAL daniel 'sudo cloud-init status --long | grep -A3 errors'
    return 1
  fi
  scp_vm -P $PORTA_ORIGINAL "$BINARIO" daniel@127.0.0.1:/var/tmp/maloki
}

desligar() {
  local nome=$1
  [ -f "$nome.pid" ] && kill "$(cat "$nome.pid")" 2>/dev/null
  sleep 2
  rm -f "$nome.pid" "$nome.qcow2"
}

# Liga a imagem clonada (num disco "filho", sem estragar a imagem) e confere tudo por SSH.
# modo: bios | uefi | secureboot
verificar() {
  local distro=$1 modo=$2 imagem="$TRABALHO/template-$1.qcow2"
  memoria_ok || return 1
  local nome="teste-$distro-$modo" firmware=() maquina=q35
  qemu-img create -q -f qcow2 -b "$imagem" -F qcow2 "$nome.qcow2"
  case $modo in
    uefi)
      cp "$OVMF/OVMF_VARS.fd" "$nome-VARS.fd"
      firmware=(-drive if=pflash,format=raw,readonly=on,file=$OVMF/OVMF_CODE.fd -drive if=pflash,format=raw,file="$nome-VARS.fd");;
    secureboot)
      # Firmware com as chaves da Microsoft já cadastradas, como um PC de verdade.
      cp "$OVMF/OVMF_VARS.secboot.fd" "$nome-VARS.fd"
      maquina=q35,smm=on
      firmware=(-global driver=cfi.pflash01,property=secure,value=on
                -drive if=pflash,format=raw,readonly=on,file=$OVMF/OVMF_CODE.secboot.fd -drive if=pflash,format=raw,file="$nome-VARS.fd");;
  esac
  qemu-system-x86_64 -enable-kvm -machine $maquina -cpu host -m 2048 -smp 2 "${firmware[@]}" \
    -drive file="$nome.qcow2",if=virtio -nic user,model=virtio-net-pci,mac=$MAC_TESTE,hostfwd=tcp:127.0.0.1:$PORTA_TESTE-:22 \
    -display none -serial file:"$nome.console" -daemonize -pidfile "$nome.pid"
  local inicio=$SECONDS
  until ssh_vm $PORTA_TESTE maria true 2>/dev/null; do
    sleep 5
    if [ $((SECONDS - inicio)) -gt 900 ]; then
      anotar "✘ $distro/$modo: a imagem não respondeu ao SSH em 15 min (veja $TRABALHO/$nome.console)"
      desligar "$nome"; rm -f "$nome-VARS.fd"; return 1
    fi
  done
  local resultado
  resultado=$(ssh_vm $PORTA_TESTE maria bash -s -- "$distro" "$modo" "${GRUPO[$distro]}" "${CHAVE_TLS[$distro]}" \
              "$(sed -n 1p "original-$distro.txt")" "$(sed -n 2p "original-$distro.txt")" < "$AQUI/conferir.sh")
  echo "$resultado" | sed "s/^/   /"
  local resumo
  resumo=$(echo "$resultado" | tail -1)
  if echo "$resumo" | grep -q " 0 falha"; then
    anotar "✔ $distro/$modo: $resumo (SSH em $((SECONDS - inicio))s)"
  else
    anotar "✘ $distro/$modo: $resumo"
  fi
  desligar "$nome"; rm -f "$nome-VARS.fd"
}

# ---------------- Teste de uma distro ----------------

testar_distro() {
  local distro=$1 base
  echo; echo "================ $distro ================"
  base=$(baixar_base "$distro") || { anotar "✘ $distro: não consegui a imagem base"; return; }
  ligar_original "$distro" "$base" || { anotar "✘ $distro: a VM original não ligou"; desligar "original-$distro"; return; }
  # Guarda o machine-id e a chave SSH da original, para provar que o clone tem outros.
  ssh_vm $PORTA_ORIGINAL daniel 'cat /etc/machine-id; sudo ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub | cut -d" " -f2' > "original-$distro.txt"

  local log="clone-$distro.log" comando="cd /var/tmp && sudo ./maloki"
  if [ "${MODO[$distro]}" = plano ]; then
    cat > "plano-$distro.toml" <<EOF
imagem = "template-$distro.qcow2"
hostname = "clone-$distro"
[[usuarios]]
nome = "maria"
senha_hash = "$(openssl passwd -6 Senha123)"
admin = true
chave_ssh = "$(cat "$TRABALHO/chave.pub")"
EOF
    scp_vm -P $PORTA_ORIGINAL "plano-$distro.toml" daniel@127.0.0.1:/var/tmp/plano.toml
    ssh_vm $PORTA_ORIGINAL daniel "$comando --plano plano.toml" > "$log" 2>&1
  else
    python3 -I "$AQUI/robo.py" "$log" "$(cat "$TRABALHO/chave.pub")" "clone-$distro" -- \
      ssh "${SSH_OPCOES[@]}" -tt -p $PORTA_ORIGINAL daniel@127.0.0.1 "$comando" > /dev/null
  fi
  if ! grep -aq "Imagem pronta" "$log"; then
    anotar "✘ $distro: o clone falhou (veja $TRABALHO/$log)"
    desligar "original-$distro"; return
  fi
  anotar "✔ $distro: clone pelos ${MODO[$distro]} terminou sem erro"
  grep -aq "teste.key" "$log" && anotar "✔ $distro: avisou sobre a chave TLS que ficou na imagem" \
                              || anotar "✘ $distro: não avisou sobre a chave TLS"
  if [ "$distro" = debian ]; then
    # O redis foi parado durante a cópia; tem que estar ligado de novo.
    ssh_vm $PORTA_ORIGINAL daniel 'systemctl is-active --quiet redis-server' \
      && anotar "✔ debian: redis parado na cópia e religado depois" || anotar "✘ debian: o redis não voltou a ligar"
  fi

  rm -f "template-$distro.qcow2"
  scp_vm -P $PORTA_ORIGINAL "daniel@127.0.0.1:/var/tmp/template-$distro.qcow2" .
  desligar "original-$distro"
  ls -la "template-$distro.qcow2" | awk '{print "   imagem: " $5/1e9 " GB"}'

  verificar "$distro" bios
  verificar "$distro" uefi
  verificar "$distro" secureboot
}

# ---------------- Proteções: trava, espaço e Ctrl+C ----------------

testar_protecoes() {
  echo; echo "================ proteções (no debian) ================"
  local base
  base=$(baixar_base debian) || return
  ligar_original debian "$base" || { anotar "✘ proteções: a VM não ligou"; desligar original-debian; return; }

  # 1. Trava: com outra cópia "rodando" (PID 1 sempre existe), tem que recusar.
  ssh_vm $PORTA_ORIGINAL daniel 'echo 1 | sudo tee /run/maloki.lock >/dev/null; cd /var/tmp; sudo ./maloki --plano /dev/null 2>&1; sudo rm -f /run/maloki.lock' > protecao-trava.log
  grep -q "já está rodando" protecao-trava.log && anotar "✔ trava: recusou rodar duas cópias juntas" \
                                               || anotar "✘ trava: não recusou (veja protecao-trava.log)"

  # 2. Espaço: imagem numa pasta de 100 MB tem que ser recusada ANTES de começar.
  printf 'imagem = "/mnt/pequena/t.qcow2"\n[[usuarios]]\nnome = "maria"\nsenha = "x"\nadmin = true\n' > plano-pequeno.toml
  scp_vm -P $PORTA_ORIGINAL plano-pequeno.toml daniel@127.0.0.1:/var/tmp/
  ssh_vm $PORTA_ORIGINAL daniel 'sudo mkdir -p /mnt/pequena && sudo mount -t tmpfs -o size=100M tmpfs /mnt/pequena; cd /var/tmp; sudo ./maloki --plano plano-pequeno.toml 2>&1; ls /mnt/pequena | wc -l; sudo umount /mnt/pequena' > protecao-espaco.log
  if grep -q "espaço livre insuficiente" protecao-espaco.log && [ "$(tail -1 protecao-espaco.log)" = 0 ]; then
    anotar "✔ espaço: recusou antes de começar (nada foi criado)"
  else
    anotar "✘ espaço: não recusou (veja protecao-espaco.log)"
  fi

  # 3. Ctrl+C no meio do rsync: nada pode ficar montado, conectado ou pela metade.
  python3 -I "$AQUI/robo.py" protecao-ctrlc.log "$(cat "$TRABALHO/chave.pub")" clone-debian --cancelar-em "[3/9]" -- \
    ssh "${SSH_OPCOES[@]}" -tt -p $PORTA_ORIGINAL daniel@127.0.0.1 "cd /var/tmp && sudo ./maloki" > /dev/null
  local estado
  estado=$(ssh_vm $PORTA_ORIGINAL daniel 'mount | grep -c maloki; losetup -l -n | grep -c "\.raw"; ls /var/tmp/*.qcow2 /var/tmp/*.raw /var/tmp/*.tmp 2>/dev/null | wc -l; ls /run/maloki.lock 2>/dev/null | wc -l; systemctl is-active redis-server')
  if grep -aq "cancelado com Ctrl+C" protecao-ctrlc.log && [ "$(echo $estado)" = "0 0 0 0 active" ]; then
    anotar "✔ Ctrl+C: parou, desmontou, soltou o disco, apagou a imagem pela metade e religou o redis"
  else
    anotar "✘ Ctrl+C: sobrou algo (montagens discos-loop imagens trava redis = $(echo $estado)); veja protecao-ctrlc.log"
  fi

  # 4. Imagem antiga + erro no meio: a antiga NÃO pode sumir (era um bug: o
  #    erro apagava o arquivo final, que ainda era a imagem antiga).
  ssh_vm $PORTA_ORIGINAL daniel 'cd /var/tmp && echo IMAGEM-ANTIGA > antiga.qcow2' >/dev/null
  # Forçamos o erro NO MEIO cancelando com Ctrl+C durante a cópia.
  printf 'imagem = "antiga.qcow2"\nsobrescrever = true\nsenha_root = "x"\n' > plano-antiga.toml
  scp_vm -P $PORTA_ORIGINAL plano-antiga.toml daniel@127.0.0.1:/var/tmp/
  python3 -I "$AQUI/robo.py" protecao-antiga.log "x" "x" --cancelar-em "[3/9]" -- \
    ssh "${SSH_OPCOES[@]}" -tt -p $PORTA_ORIGINAL daniel@127.0.0.1 "cd /var/tmp && sudo ./maloki --plano plano-antiga.toml" > /dev/null
  if [ "$(ssh_vm $PORTA_ORIGINAL daniel 'cat /var/tmp/antiga.qcow2')" = IMAGEM-ANTIGA ]; then
    anotar "✔ sobrescrever: a imagem antiga continuou intacta depois da falha"
  else
    anotar "✘ sobrescrever: a imagem antiga sumiu ou mudou (veja protecao-antiga.log)"
  fi
  desligar original-debian
}

# ---------------- ISO instalável (sessão live + Calamares) ----------------

# Liga a ISO numa VM com um disco vazio, roda os testes da sessão live pela
# serial e instala no disco (como o Calamares). modo: bios | uefi
testar_iso_modo() {
  local distro=$1 modo=$2 iso="$TRABALHO/maloki-$1.iso"
  memoria_ok || return 1
  local nome="iso-$distro-$modo" firmware=()
  rm -f "$nome.disco.qcow2" "$nome.sock"
  qemu-img create -q -f qcow2 "$nome.disco.qcow2" 25G
  if [ "$modo" = uefi ]; then
    cp "$OVMF/OVMF_VARS.fd" "$nome-VARS.fd"
    firmware=(-drive if=pflash,format=raw,readonly=on,file=$OVMF/OVMF_CODE.fd -drive if=pflash,format=raw,file="$nome-VARS.fd")
  fi
  qemu-system-x86_64 -enable-kvm -machine q35 -cpu host -m 4096 -smp 4 "${firmware[@]}" \
    -drive file="$nome.disco.qcow2",if=virtio \
    -drive file="$iso",media=cdrom,readonly=on,if=none,id=live -device ide-cd,drive=live,bus=ide.0,bootindex=0 \
    -drive file="$TRABALHO/cd-testes.iso",media=cdrom,readonly=on,if=none,id=testes -device ide-cd,drive=testes,bus=ide.1 \
    -nic user,model=virtio-net-pci -display none -serial unix:"$nome.sock",server=on,wait=off \
    -daemonize -pidfile "$nome.pid"
  python3 -I "$AQUI/iso/serial.py" "$nome.sock" "$nome.console" "$modo" || anotar "✘ $distro/iso/$modo: a sessão live não respondeu (veja $TRABALHO/$nome.console)"
  sleep 10; [ -f "$nome.pid" ] && kill "$(cat "$nome.pid")" 2>/dev/null; rm -f "$nome.pid" "$nome.sock"
  local linhas
  linhas=$(tr -d '\r' < "$nome.console" | sed 's/\x1b\[[0-9;?]*[A-Za-z]//g' | grep -aE '^(✔|✘|   (calamares|pos-instalacao):)')
  echo "$linhas" | sed 's/^/   /'
  local falhas
  falhas=$(echo "$linhas" | grep -c '^✘')
  if echo "$linhas" | grep -q 'pos-instalacao terminou sem erro' && [ "$falhas" = 0 ]; then
    anotar "✔ $distro/iso/$modo: sessão live e instalação ok ($(echo "$linhas" | grep -c '^✔') verificações)"
  else
    anotar "✘ $distro/iso/$modo: $falhas falha(s) na sessão live/instalação (veja $TRABALHO/$nome.console)"
    return 1
  fi
  verificar_instalado "$distro" "$modo" "$nome"
  [ "$modo" = uefi ] && verificar_instalado "$distro" secureboot "$nome"
  rm -f "$nome.disco.qcow2" "$nome-VARS.fd"
}

# Liga o disco onde a ISO instalou o sistema e confere por SSH (como a ana).
verificar_instalado() {
  local distro=$1 modo=$2 nome=$3 firmware=() maquina=q35
  memoria_ok || return 1
  case $modo in
    uefi) firmware=(-drive if=pflash,format=raw,readonly=on,file=$OVMF/OVMF_CODE.fd -drive if=pflash,format=raw,file="$nome-VARS.fd");;
    secureboot)
      # Firmware NOVO com Secure Boot: sem a entrada que o efibootmgr criou,
      # então também prova que o caminho padrão (EFI/BOOT/BOOTX64.EFI) funciona.
      cp "$OVMF/OVMF_VARS.secboot.fd" "$nome-SB.fd"
      maquina=q35,smm=on
      firmware=(-global driver=cfi.pflash01,property=secure,value=on
                -drive if=pflash,format=raw,readonly=on,file=$OVMF/OVMF_CODE.secboot.fd -drive if=pflash,format=raw,file="$nome-SB.fd");;
  esac
  qemu-img create -q -f qcow2 -b "$nome.disco.qcow2" -F qcow2 "$nome-filho.qcow2"
  qemu-system-x86_64 -enable-kvm -machine $maquina -cpu host -m 2048 -smp 2 "${firmware[@]}" \
    -drive file="$nome-filho.qcow2",if=virtio -nic user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:$PORTA_TESTE-:22 \
    -display none -serial file:"$nome-instalado-$modo.console" -daemonize -pidfile "$nome-filho.pid"
  local inicio=$SECONDS
  until ssh_vm $PORTA_TESTE ana true 2>/dev/null; do
    sleep 5
    if [ $((SECONDS - inicio)) -gt 900 ]; then
      anotar "✘ $distro/iso/instalado/$modo: não respondeu ao SSH em 15 min (veja $TRABALHO/$nome-instalado-$modo.console)"
      desligar "$nome-filho"; rm -f "$nome-SB.fd"; return 1
    fi
  done
  local resultado resumo
  resultado=$(ssh_vm $PORTA_TESTE ana bash -s -- "$distro" "$modo" "${GRUPO[$distro]}" < "$AQUI/iso/conferir-instalado.sh")
  echo "$resultado" | sed "s/^/   /"
  resumo=$(echo "$resultado" | tail -1)
  if echo "$resumo" | grep -q " 0 falha"; then
    anotar "✔ $distro/iso/instalado/$modo: $resumo (SSH em $((SECONDS - inicio))s)"
  else
    anotar "✘ $distro/iso/instalado/$modo: $resumo"
  fi
  desligar "$nome-filho"; rm -f "$nome-SB.fd"
}

testar_iso() {
  local distro=$1 base
  echo; echo "================ ISO: $distro ================"
  # REUSAR_ISO=1 pula a geração se a ISO já existir (útil para ajustar só os testes).
  if [ "${REUSAR_ISO:-0}" = 1 ] && [ -f "maloki-$distro.iso" ]; then
    echo "Reaproveitando $TRABALHO/maloki-$distro.iso"
  else
  base=$(baixar_base "$distro") || { anotar "✘ $distro/iso: não consegui a imagem base"; return; }
  ligar_original "$distro" "$base" "${PACOTES_ISO[$distro]}" \
    || { anotar "✘ $distro/iso: a VM original não ligou"; desligar "original-$distro"; return; }
  printf 'tipo = "iso"\nimagem = "maloki-%s.iso"\nsobrescrever = true\n' "$distro" > "plano-iso-$distro.toml"
  scp_vm -P $PORTA_ORIGINAL "plano-iso-$distro.toml" daniel@127.0.0.1:/var/tmp/plano-iso.toml
  local log="iso-$distro.log"
  ssh_vm $PORTA_ORIGINAL daniel "cd /var/tmp && sudo ./maloki --plano plano-iso.toml" > "$log" 2>&1
  if ! grep -aq "ISO pronta" "$log"; then
    anotar "✘ $distro/iso: a ISO não foi gerada (veja $TRABALHO/$log)"
    tail -15 "$log" | sed 's/^/   /'
    desligar "original-$distro"; return
  fi
  anotar "✔ $distro/iso: ISO gerada sem erro"
  rm -f "maloki-$distro.iso"
  scp_vm -P $PORTA_ORIGINAL "daniel@127.0.0.1:/var/tmp/maloki-$distro.iso" .
  desligar "original-$distro"
  ls -la "maloki-$distro.iso" | awk '{print "   ISO: " $5/1e9 " GB"}'
  fi

  # O "CD de testes": os scripts que rodam na sessão live + a chave SSH e a senha da ana.
  rm -rf cd-testes; mkdir cd-testes
  cp "$AQUI"/iso/*.sh cd-testes/; cp chave.pub cd-testes/; openssl passwd -6 Senha123 > cd-testes/hash
  genisoimage -quiet -output cd-testes.iso -volid MALOKITESTE -joliet -rock cd-testes

  testar_iso_modo "$distro" bios
  testar_iso_modo "$distro" uefi
}

# ---------------- Início ----------------

echo "Compilando a versão estática (roda em qualquer distro)..."
( cd "$PROJETO" && cargo build --quiet --release --target x86_64-unknown-linux-musl ) || exit 1
BINARIO="$PROJETO/target/x86_64-unknown-linux-musl/release/maloki"
[ -f "$TRABALHO/chave" ] || ssh-keygen -q -t ed25519 -N '' -C maloki-e2e -f "$TRABALHO/chave"

alvos=("$@")
# Padrão: tudo (as distros principais, as proteções e as ISOs).
[ ${#alvos[@]} -eq 0 ] && alvos=(rocky debian fedora ubuntu protecoes iso-debian iso-fedora)
for alvo in "${alvos[@]}"; do
  case $alvo in
    protecoes) testar_protecoes;;
    rocky|debian|ubuntu|fedora) testar_distro "$alvo";;
    iso-debian|iso-fedora|iso-ubuntu) testar_iso "${alvo#iso-}";;
    *) echo "alvo desconhecido: $alvo"; exit 2;;
  esac
done

echo; echo "================ RESUMO ================"
printf '%s\n' "${RESUMO[@]}"
exit $falhou
