# Changelog

Todas as mudanças importantes do maloki ficam registradas aqui.
O formato segue o [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/)
e o projeto usa [Versionamento Semântico](https://semver.org/lang/pt-BR/).

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

[0.1.0]: https://github.com/Daniel-Pereira-Linux/maloki/releases/tag/v0.1.0
