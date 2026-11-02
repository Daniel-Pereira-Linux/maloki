# Changelog

Todas as mudanças importantes do maloki ficam registradas aqui.
O formato segue o [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/)
e o projeto usa [Versionamento Semântico](https://semver.org/lang/pt-BR/).

## [0.2.0] - 2026-11-02

> **Testado de ponta a ponta em VMs:** o template no Debian 13, Fedora 44,
> Rocky 9 e Ubuntu 24.04 (BIOS, UEFI e Secure Boot); as proteções; a ISO do
> Debian 13 (BIOS, UEFI e Secure Boot) e a do Fedora 44 em BIOS. Pendentes: a
> ISO do Fedora em UEFI (corrigida), a do Ubuntu e a instalação clicando nas
> telas do Calamares (nos testes, um script faz os mesmos passos dele).

### Adicionado
- **ISO instalável** (`tipo = "iso"` ou a 2ª opção do menu): o sistema vira uma
  ISO híbrida (DVD/pendrive, BIOS e UEFI) com sessão live e o instalador gráfico
  **Calamares**. O maloki configura o Calamares inteiro (telas, particionamento,
  usuários, cara do instalador com o logo da distro) e um script de
  pós-instalação que remove a sessão live e instala o GRUB no disco (inclusive
  o shim assinado do Fedora, para o Secure Boot).
- **Compressão zstd** para o `.qcow2` (`compressao = "zstd"`, o novo padrão
  quando o `qemu-img` é 5.1 ou mais novo): arquivo do mesmo tamanho ou menor e
  leitura bem mais rápida na VM. Na ISO, `xz` (a menor ISO, padrão) ou `zstd`.
- Testes de ponta a ponta da ISO (`tests/e2e/rodar.sh iso-debian iso-fedora`):
  geram a ISO numa VM, ligam a sessão live em BIOS e UEFI, conferem o Calamares,
  instalam no disco como o Calamares faria e ligam o sistema instalado em BIOS,
  UEFI e Secure Boot.
- Teste de ponta a ponta "imagem antiga + falha no meio" nas proteções.


### Corrigido
- **Uma falha no meio apagava a imagem antiga** quando `sobrescrever = true` (ou
  "Sobrescrever? sim" no menu): o arquivo final, que ainda era a imagem antiga,
  era apagado. Agora o resultado é gravado em `ARQUIVO.tmp` e só troca de nome
  quando fica pronto.
- **Com `..` ou um link simbólico no caminho da imagem**, ou com `*`, `?` ou `[`
  no nome, o rsync copiava o `.raw` que estava sendo criado para dentro dele
  mesmo, até o disco encher. O caminho agora é resolvido para o caminho real e
  os curingas são escapados.
- Um usuário com o nome de uma conta ou grupo do sistema (`gdm`, `docker`...)
  passava na validação e só falhava no passo 7, depois de toda a cópia.
- Caminhos de imagem inválidos (`""`, uma pasta, uma pasta que não existe) eram
  aceitos e só falhavam no fim.
- A estimativa de tamanho só olhava `/usr`, `/etc`, `/var`... e não contava
  outras pastas da raiz nem outros discos montados, que o rsync copia: a imagem
  podia sair pequena demais. A conta agora bate com o que o rsync copia.
- O swap do Ubuntu Server (`/swap.img`) e do btrfs (`/swap/swapfile`) não entram
  mais na imagem.
- Os menus fora de um terminal davam o erro cru "IO error: not a terminal";
  agora a mensagem sugere o `--plano`.
- O `chpasswd` era exigido, mas não era mais usado.
- **Ubuntu 24.04: o SSH não ligava no template.** O serviço que recria as chaves
  do SSH no primeiro boot formava um ciclo de ordem com o `ssh.socket`, e o
  systemd descartava o socket. Agora ele roda cedo no boot (`DefaultDependencies=no`).
- **Ubuntu em UEFI: o template parava no `grub>`** ("no such device"). O GRUB
  assinado (o do Secure Boot) lê o `EFI/ubuntu/grub.cfg`, e a cópia trazia o da
  máquina original, apontando para o disco antigo. Agora o `grub-install` também
  reescreve a pasta da distro na partição EFI.
- **ISO do Fedora em UEFI: a cópia do sistema para o disco falhava.** O squashfs
  levava a pasta `/boot/efi` com a etiqueta do SELinux, e a partição EFI (FAT32)
  não aceita etiquetas. Agora a pasta inteira fica fora do squashfs.
- Testes e2e: no Fedora 44, `cloud-init status` precisa de root; o teste
  esperava à toa até o limite de tempo.

## [0.1.0] - 2026-11-01

Primeira versão pública.

### Adicionado
- Clona o Linux que está rodando para uma imagem `.qcow2` em 9 etapas, usando um
  disco *loop* do kernel (não precisa do módulo `nbd`).
- Imagem híbrida: liga em **BIOS**, **UEFI** e **UEFI com Secure Boot**
  (4 partições: BIOS boot, EFI, `/boot` e raiz).
- `e2fsck` nos sistemas de arquivos da imagem antes de entregar: se houver erro,
  a imagem é descartada em vez de sair "pronta".
- Limpeza de dados pessoais: `/home`, `/root`, usuários, Wi-Fi, VPN, chaves SSH,
  `machine-id`, logs, caches, regras de sudo, chaves de disco.
- Rede da imagem vira DHCP (NetworkManager, netplan, systemd-networkd, ifupdown).
- Criação de usuários pelos menus ou pelo modo automático `--plano arquivo.toml`
  (com suporte a `senha_hash`).
- Modo `--simular`, que mostra todos os comandos sem executar nada.
- Proteções: Ctrl+C seguro, conferência de espaço livre, trava contra execução
  dupla, trava de caminho, pausa de bancos de dados durante a cópia, `qemu-img check`.
- Aviso das chaves privadas TLS que continuam na imagem.
- Famílias Fedora (Fedora, Rocky, AlmaLinux, RHEL, CentOS) e Debian (Debian, Ubuntu...).
  Testado de ponta a ponta em **Fedora 44** e **Rocky 9** (BIOS, UEFI e Secure Boot).
- 111 testes rápidos (`cargo test`) e testes de ponta a ponta em VMs descartáveis
  (`tests/e2e/rodar.sh`).
- Manpage `maloki(1)`, instalador `install.sh`, site do projeto.

[0.2.0]: https://github.com/Daniel-Pereira-Linux/maloki/releases/tag/v0.2.0
[0.1.0]: https://github.com/Daniel-Pereira-Linux/maloki/releases/tag/v0.1.0
