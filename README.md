<div align="center">

# 🐧 → 📦 maloki

[![CI](https://github.com/Daniel-Pereira-Linux/maloki/actions/workflows/ci.yml/badge.svg)](https://github.com/Daniel-Pereira-Linux/maloki/actions/workflows/ci.yml)
[![Versão](https://img.shields.io/github/v/release/Daniel-Pereira-Linux/maloki?label=vers%C3%A3o)](https://github.com/Daniel-Pereira-Linux/maloki/releases/latest)
[![Licença MIT](https://img.shields.io/badge/licen%C3%A7a-MIT-blue)](LICENSE)
[![Site](https://img.shields.io/badge/site-daniel--pereira--linux.github.io%2Fmaloki-e8590c)](https://daniel-pereira-linux.github.io/maloki)

**Transforme o Linux que está rodando numa imagem de disco portátil (`.qcow2`),
sem os seus dados pessoais e com os usuários que você escolher.**

`BIOS` · `UEFI` · `Secure Boot` · `rede automática` · `modo simulação` · `Ctrl+C seguro`

</div>

---

## Índice

- [O que é](#o-que-é)
- [O que vai e o que NÃO vai para a imagem](#o-que-vai-e-o-que-não-vai-para-a-imagem)
- [Distribuições suportadas](#distribuições-suportadas)
- [Instalação](#instalação)
- [Uso em 3 passos](#uso-em-3-passos)
- [Modo automático (`--plano`)](#modo-automático---plano)
- [Usando a imagem gerada](#usando-a-imagem-gerada)
- [Proteções](#proteções)
- [Problemas comuns](#problemas-comuns)
- [Limitações conhecidas](#limitações-conhecidas)
- [Encontrou um bug?](#encontrou-um-bug)
- [Quer contribuir?](#quer-contribuir)

---

## O que é

Você tem um Linux configurado do jeito que gosta: programas instalados,
serviços ajustados, tudo funcionando. O `maloki` faz uma **"foto"** desse
sistema e grava tudo num único arquivo de disco virtual. Esse arquivo é o **template**:

```
  seu Linux (ligado)                         template-fedora.qcow2
 ┌──────────────────────┐                 ┌──────────────────────────┐
 │ programas            │ ─── copia ────▶ │ programas                │
 │ configurações        │ ─── copia ────▶ │ configurações            │
 │ /home, Wi-Fi, VPN,   │ ─── NÃO vai ─╳  │ usuários que VOCÊ criar  │
 │ seu usuário, chaves  │                 │ rede automática (DHCP)   │
 └──────────────────────┘                 │ liga em BIOS/UEFI/SB     │
                                          └──────────────────────────┘
```

O arquivo pode ir para um HD externo, outro computador ou um servidor, e liga
no **QEMU/KVM**, no **Proxmox**, no **libvirt/virt-manager** e, convertido, no
**VirtualBox** e no **VMware**.

---

## O que vai e o que NÃO vai para a imagem

| ✅ Vai para a imagem | ❌ NÃO vai (é apagado ou fica de fora) |
|---|---|
| Programas instalados (pacotes, Flatpaks opcionais) | `/home` inteiro e `/root` (seus arquivos) |
| Configurações do sistema em `/etc` | **O seu usuário** e todos os usuários "humanos" |
| Serviços e o que eles precisam para rodar | Senhas de **Wi-Fi** (NetworkManager, netplan, iwd, wpa_supplicant) |
| Kernel, com boot ajustado para a VM | **VPN** (WireGuard, OpenVPN cliente) |
| Os usuários que **você** criar no menu | **Chaves SSH** do servidor (cada VM ganha as suas) |
| | `machine-id` (cada VM ganha o seu) |
| | **IP fixo** e placas amarradas ao MAC (vira DHCP) |
| | Logs, caches, histórico, e-mails locais, Bluetooth, digitais |
| | Discos de VMs, containers, swapfile |
| | Regras de `sudo` dos usuários removidos, chaves de disco (LUKS) |

> ⚠️ **Chaves privadas de certificados (HTTPS)** ficam na imagem, porque um
> servidor web pode precisar delas para ligar. No final, o programa **lista**
> as que encontrou, para você decidir. Veja as [limitações](#limitações-conhecidas).

---

## Distribuições suportadas

| Distribuição | Status | Testado em |
|---|---|---|
| **Fedora** | ✅ testado de ponta a ponta | Fedora 44 (btrfs, SELinux ativo) |
| **Rocky / AlmaLinux / RHEL / CentOS** | ✅ testado de ponta a ponta | Rocky 9 (SELinux ativo) |
| **Debian** | 🟡 deve funcionar | passou em uma versão anterior do maloki; [conte como foi](#encontrou-um-bug) |
| **Ubuntu / Mint / Pop!_OS** | 🟡 deve funcionar (mesma família do Debian) | ainda não testado: [conte como foi](#encontrou-um-bug) |
| Arch, openSUSE, Alpine... | ❌ ainda não | o programa avisa e não faz nada |

"Testado de ponta a ponta" significa: o programa clonou uma máquina de verdade,
a imagem foi ligada em **BIOS, UEFI e Secure Boot**, e mais de 20 itens foram
conferidos por dentro, via SSH.

---

## Instalação

### Jeito rápido: binário pronto (recomendado)

```bash
curl -fsSL https://daniel-pereira-linux.github.io/maloki/install.sh | sh
```

O instalador baixa a versão mais nova do [Releases](https://github.com/Daniel-Pereira-Linux/maloki/releases),
**confere o checksum SHA‑256** e instala o `maloki` em `/usr/local/bin` e o
manual (`man maloki`). [Leia o script](install.sh) antes, se quiser: ele é curto.
O binário é estático, então funciona em qualquer distro x86_64.

Depois, instale os programas que o maloki usa (passo 2 abaixo).

### Compilando você mesmo

**1. Rust** (se ainda não tiver):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**2. Programas que o maloki usa:**

```bash
# Fedora / Rocky / AlmaLinux / RHEL
sudo dnf install qemu-img rsync dosfstools grub2-pc-modules

# Debian / Ubuntu
sudo apt install qemu-utils rsync dosfstools grub-pc-bin grub-efi-amd64-bin
```

> `grub2-pc-modules` / `grub-pc-bin` servem para a imagem ligar também em
> **BIOS**. Sem eles, a imagem liga só em UEFI (o programa avisa).

**3. Compilar:**

```bash
git clone https://github.com/Daniel-Pereira-Linux/maloki.git
cd maloki
cargo build --release
```

O programa fica em `target/release/maloki`.

---

## Uso em 3 passos

### 1. Veja antes o que vai acontecer (sem risco nenhum)

```bash
./target/release/maloki --simular
```

O modo simulação faz as mesmas perguntas e **mostra cada comando** que seria
executado, mas **não executa nenhum**. Nem precisa de `sudo`.

### 2. Vá para uma pasta com espaço livre

A imagem é criada **na pasta onde você está**. Ela precisa de espaço para o
sistema todo, mais a compactação (o programa confere antes de começar).

```bash
cd /media/meu-hd-externo
```

### 3. Rode de verdade

```bash
sudo /caminho/para/maloki/target/release/maloki
```

Os menus perguntam:

```
? Nome do arquivo da imagem (template-fedora.qcow2)
? Incluir os apps Flatpak instalados no sistema? (y/n)
? Tamanho do disco da VM em GB (44)
? Nome da máquina (hostname) da VM (template)
--- Usuários da imagem ---
? Criar um usuário na imagem? (y/n)
? Nome de login › maria
? Senha › ********
? Este usuário pode usar sudo (administrador)? (y/n)
? Obrigar a trocar a senha no primeiro login? (y/n)
? Chave SSH pública (cole a chave ou o caminho do .pub; Enter para pular)
? Bloquear o login do root? (recomendado; admins usam sudo) (y/n)
? Parar esses serviços durante a cópia? (só se houver banco de dados ligado)
? Compactar a imagem no final? (y/n)
```

Depois de um resumo e da confirmação, o programa passa por 9 etapas:

```
━━━ [1/9] Criando o disco virtual ━━━
━━━ [2/9] Particionando e formatando ━━━
━━━ [3/9] Copiando o sistema (pode demorar bastante) ━━━
━━━ [4/9] Removendo dados pessoais e identidade da máquina ━━━
━━━ [5/9] Preparando para rodar comandos dentro da imagem (chroot) ━━━
━━━ [6/9] Configurando o boot e a rede ━━━
━━━ [7/9] Trocando os usuários ━━━
━━━ [8/9] Gerando o .qcow2 compactado ━━━
━━━ [9/9] Conferindo a imagem ━━━

✅ Imagem pronta: /media/meu-hd-externo/template-fedora.qcow2
```

---

## Modo automático (`--plano`)

Para scripts, cron, Ansible ou para gerar sempre o mesmo template, escreva as
respostas num arquivo `.toml`:

```toml
imagem = "template.qcow2"
hostname = "web-01"
compactar = true

[[usuarios]]
nome = "maria"
senha_hash = "$6$..."          # gere com: openssl passwd -6
admin = true
chave_ssh = "~/.ssh/id_ed25519.pub"
```

```bash
sudo ./target/release/maloki --plano meu-plano.toml
```

Nesse modo o programa **não faz nenhuma pergunta**. O arquivo
[`exemplo-plano.toml`](exemplo-plano.toml) explica cada opção e o valor padrão.
Algumas regras:

- Use `senha_hash` em vez de `senha`, para não deixar a senha escrita no arquivo.
- Campo com nome errado (ex.: `hostnme`) **dá erro**, em vez de ser ignorado.
- Sem nenhum usuário `admin = true` e sem senha de root, o programa recusa:
  ninguém conseguiria administrar a máquina.

---

## Usando a imagem gerada

**Ligar no QEMU sem estragar o template.** O truque é criar um disco "filho"
(*overlay*) que guarda só o que mudar:

```bash
qemu-img create -f qcow2 -b template-fedora.qcow2 -F qcow2 minha-vm.qcow2

# Modo BIOS:
qemu-system-x86_64 -enable-kvm -m 4G -cpu host \
  -drive file=minha-vm.qcow2,if=virtio -nic user,hostfwd=tcp::2222-:22

# Modo UEFI (Fedora: pacote edk2-ovmf; Debian: ovmf):
cp /usr/share/edk2/ovmf/OVMF_VARS.fd minha-vm-VARS.fd
qemu-system-x86_64 -enable-kvm -m 4G -cpu host -machine q35 \
  -drive if=pflash,format=raw,readonly=on,file=/usr/share/edk2/ovmf/OVMF_CODE.fd \
  -drive if=pflash,format=raw,file=minha-vm-VARS.fd \
  -drive file=minha-vm.qcow2,if=virtio -nic user,hostfwd=tcp::2222-:22

# Entrar por SSH:
ssh -p 2222 maria@localhost
```

**Converter para outros programas:**

```bash
qemu-img convert -O vdi  template.qcow2 template.vdi    # VirtualBox
qemu-img convert -O vmdk template.qcow2 template.vmdk   # VMware
qemu-img convert -O raw  template.qcow2 template.img    # disco "cru" (dd, pendrive)
```

> 💡 No **primeiro boot** de sistemas com SELinux (Fedora, Rocky...), a máquina
> reetiqueta os arquivos e **reinicia sozinha uma vez**. É normal.

---

## Proteções

O programa roda como root e mexe com discos, por isso tem várias travas:

| Proteção | O que evita |
|---|---|
| 🔍 **`--simular`** | Mostra todos os comandos sem executar nenhum |
| ⌨️ **Ctrl+C seguro** | Cancelar no meio desmonta tudo, desconecta o disco virtual, religa os serviços e apaga a imagem incompleta |
| 💾 **Espaço livre** | Confere **antes** de começar, para não encher o disco do seu computador |
| 🔒 **Uma cópia por vez** | `/run/maloki.lock` impede duas execuções simultâneas |
| 🛡️ **Trava de caminho** | O programa só apaga arquivos dentro de `/mnt/maloki-*`; qualquer outro caminho para o programa na hora |
| 🧹 **Limpeza automática** | Qualquer erro no meio desmonta e desconecta tudo |
| 🗄️ **Pausa de bancos** | PostgreSQL, MySQL/MariaDB, Redis, MongoDB... param durante a cópia e voltam no fim |
| 🩺 **`e2fsck` no final** | Os sistemas de arquivos da imagem são conferidos; se houver qualquer erro, a imagem é descartada em vez de entregue |
| ✔️ **`qemu-img check`** | A estrutura da imagem é conferida no final |

---

## Problemas comuns

<details>
<summary><b>❌ "faltam programas: ..."</b></summary>

Instale os programas da seção [Instalação](#instalação). No Debian, o
`sfdisk` e o `mkfs.ext4` ficam em `/usr/sbin`; o programa procura lá também.
</details>

<details>
<summary><b>❌ "espaço livre insuficiente"</b></summary>

Rode o programa de dentro de uma pasta num disco com mais espaço (um HD
externo, por exemplo), ou responda "não" para compactar, que precisa de menos
espaço temporário.
</details>

<details>
<summary><b>❌ "o maloki já está rodando (processo N)"</b></summary>

Outra cópia está mesmo rodando. Se você tem certeza de que não está (o PC
desligou no meio, por exemplo), apague a trava: `sudo rm /run/maloki.lock`.
</details>

<details>
<summary><b>❌ Erro no <code>losetup</code></b></summary>

O maloki usa um disco *loop* (`/dev/loopN`) para montar a imagem. Confira com
`losetup -l` se sobrou algum disco ligado a um arquivo `.raw` antigo e solte com
`sudo losetup -d /dev/loopN`.
</details>

<details>
<summary><b>❌ A imagem não liga / tela preta</b></summary>

Ligue com o console serial no terminal para ver as mensagens do boot:

```bash
qemu-system-x86_64 -enable-kvm -m 4G -drive file=minha-vm.qcow2,if=virtio -nographic
```

e [abra uma issue](#encontrou-um-bug) com o que aparecer.
</details>

<details>
<summary><b>❓ Quanto tempo demora?</b></summary>

Quase tudo é a cópia (passo 3). Um servidor com 3 GB leva de 1 a 2 minutos; um
desktop com 20 GB leva de 10 a 20 minutos, dependendo do disco. A compactação
(passo 8) costuma levar outro tanto.
</details>

---

## Limitações conhecidas

- **Copia o sistema ligado.** Os bancos de dados conhecidos são pausados, mas
  outros programas que gravam o tempo todo podem ficar inconsistentes na imagem.
- **Chaves privadas TLS** (`/etc/pki/tls/private`, `/etc/ssl/private`, Let's
  Encrypt, servidor OpenVPN) **ficam** e são listadas no final. Se a imagem for
  compartilhada, apague ou troque essas chaves.
- A imagem usa um layout fixo: BIOS boot, EFI, `/boot` (1 GB) e o sistema (ext4). LVM, RAID,
  criptografia e subvolumes btrfs da máquina original não são recriados.
- Precisa de **root** (para montar discos e usar o `chroot`).
- Só **x86_64** por enquanto.

---

## Encontrou um bug?

Abra uma **issue** no GitHub com:

1. A distribuição e a versão (`cat /etc/os-release`)
2. O comando que você rodou (`--simular`? `--plano`?)
3. A saída completa do programa (de preferência o trecho do erro)
4. Se a imagem não ligou: o que aparece com `-nographic` (veja [acima](#problemas-comuns))

**Testou numa distro que ainda não está na tabela?** Conte como foi, mesmo que
tenha dado certo! É assim que a lista de suportadas cresce.

---

## Quer contribuir?

O código foi escrito para ser **lido por quem está aprendendo Rust**: cada
arquivo tem comentários explicando o que cada bloco faz e por quê.

👉 **Comece pelo [GUIA-DO-CODIGO.md](GUIA-DO-CODIGO.md)**. Ele mostra por onde
ler, qual arquivo chama qual, os padrões do projeto e receitas passo a passo
para as mudanças mais comuns.

```bash
cargo test                 # 100+ testes rápidos, sem root
tests/e2e/rodar.sh         # testes reais em VMs descartáveis (~1 hora)
```
