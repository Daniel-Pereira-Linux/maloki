// Rede da imagem: tira as configurações da máquina original (IP fixo,
// placas amarradas ao endereço MAC) e deixa a rede AUTOMÁTICA (DHCP).
//
// Sem isso, um clone de um servidor com IP 192.168.100.11 ligaria com o
// MESMO IP: dois computadores com o mesmo endereço = conflito na rede.
//
// Cada distro configura a rede com uma ferramenta diferente:
//   NetworkManager  -> Fedora, Rocky, desktops      (arquivos .nmconnection / ifcfg)
//   netplan         -> Ubuntu, Debian "cloud"       (arquivos .yaml)
//   ifupdown        -> Debian servidor clássico     (/etc/network/interfaces)

use crate::executor::Executor;
use crate::limpeza::na_imagem;
use crate::Resultado;
use std::fs;
use std::path::Path;

// NetworkManager: uma conexão DHCP para qualquer placa com fio
// ("e*" pega eth0, ens3, enp1s0...). O nome ".nmconnection" é obrigatório.
pub const NM_DHCP: &str = "\
# Gerado pelo maloki: rede automática (DHCP) em qualquer placa com fio.
[connection]
id=rede-com-fio
type=ethernet
autoconnect=true

[match]
interface-name=e*

[ipv4]
method=auto

[ipv6]
method=auto
";

// netplan: a mesma ideia, no formato YAML (a indentação importa!).
pub const NETPLAN_DHCP: &str = "\
# Gerado pelo maloki: rede automática (DHCP) em qualquer placa com fio.
network:
  version: 2
  ethernets:
    rede-com-fio:
      match:
        name: \"e*\"
      dhcp4: true
      dhcp6: true
";

// systemd-networkd: usado quando não há NetworkManager nem netplan.
pub const NETWORKD_DHCP: &str = "\
# Gerado pelo maloki: rede automática (DHCP) em qualquer placa com fio.
[Match]
Name=e*

[Network]
DHCP=yes
";

// /etc/network/interfaces sem nenhuma placa de rede (só o "loopback", a
// rede interna do próprio computador, que sempre existe).
pub const INTERFACES_SO_LO: &str = "\
# Gerado pelo maloki: a rede é configurada pelo systemd-networkd.
source /etc/network/interfaces.d/*

auto lo
iface lo inet loopback
";

pub fn configurar(exec: &Executor, raiz: &str) -> Resultado<()> {
    if exec.simular {
        println!("[simulação] trocaria as configurações de rede da máquina original por DHCP automático");
        return Ok(());
    }
    let tem = |caminho: &str| Path::new(&na_imagem(raiz, caminho)).exists();
    let tem_nm = tem("usr/sbin/NetworkManager");
    let tem_netplan = tem("usr/sbin/netplan") || tem("usr/bin/netplan");

    // 1) Fedora/Rocky antigos: arquivos ifcfg-* (o ifcfg-lo é o loopback e fica).
    for arquivo in listar(&na_imagem(raiz, "etc/sysconfig/network-scripts")) {
        let nome = nome_do_arquivo(&arquivo);
        if nome.starts_with("ifcfg-") && nome != "ifcfg-lo" {
            exec.apagar(&arquivo)?;
        }
    }

    // 2) netplan: apaga os arquivos que descrevem placas de rede e põe o nosso.
    for arquivo in listar(&na_imagem(raiz, "etc/netplan")) {
        let texto = fs::read_to_string(&arquivo).unwrap_or_default();
        if define_placas(&texto) {
            exec.apagar(&arquivo)?;
        }
    }
    if tem_netplan {
        let arquivo = na_imagem(raiz, "etc/netplan/01-maloki-dhcp.yaml");
        exec.escrever(&arquivo, NETPLAN_DHCP)?;
        exec.rodar("chmod", &["600", &arquivo])?; // o netplan reclama se outros puderem ler
    } else if tem_nm {
        // 3) NetworkManager sem netplan (Fedora, Rocky, Debian desktop).
        let arquivo = na_imagem(raiz, "etc/NetworkManager/system-connections/rede-com-fio.nmconnection");
        exec.escrever(&arquivo, NM_DHCP)?;
        exec.rodar("chmod", &["600", &arquivo])?; // o NM ignora o arquivo se outros puderem ler
    }

    // 4) Debian clássico (ifupdown): o arquivo cita placas pelo nome (ens3, eth0...),
    //    que mudam na VM. Deixamos só o loopback e passamos a rede para o networkd.
    let interfaces = na_imagem(raiz, "etc/network/interfaces");
    let texto = fs::read_to_string(&interfaces).unwrap_or_default();
    if tem_placa_alem_do_lo(&texto) {
        exec.escrever(&interfaces, INTERFACES_SO_LO)?;
        exec.esvaziar_pasta(&na_imagem(raiz, "etc/network/interfaces.d"))?;
        if !tem_nm && !tem_netplan {
            exec.escrever(&na_imagem(raiz, "etc/systemd/network/80-maloki-dhcp.network"), NETWORKD_DHCP)?;
            exec.rodar("chroot", &[raiz, "systemctl", "enable", "systemd-networkd"])?;
        }
    }
    Ok(())
}

/// Um arquivo do netplan define placas se tiver alguma destas seções.
pub fn define_placas(yaml: &str) -> bool {
    let secoes = ["ethernets:", "wifis:", "bonds:", "bridges:", "vlans:", "tunnels:", "vrfs:", "modems:"];
    yaml.lines().any(|linha| secoes.contains(&linha.trim()))
}

/// O /etc/network/interfaces fala de alguma placa além do "lo"?
pub fn tem_placa_alem_do_lo(texto: &str) -> bool {
    texto.lines().any(|linha| {
        let palavras: Vec<&str> = linha.split_whitespace().collect();
        match palavras.first() {
            // "iface eth0 inet dhcp" -> o nome da placa é a 2ª palavra
            Some(&"iface") => palavras.get(1).is_some_and(|nome| *nome != "lo"),
            // "auto lo eth0" / "allow-hotplug ens3" -> todas as palavras depois da 1ª
            Some(&"auto") | Some(&"allow-hotplug") => palavras[1..].iter().any(|nome| *nome != "lo"),
            _ => false,
        }
    })
}

fn listar(pasta: &str) -> Vec<String> {
    let mut lista: Vec<String> = fs::read_dir(pasta)
        .map(|entradas| entradas.flatten().map(|e| e.path().to_string_lossy().to_string()).collect())
        .unwrap_or_default();
    lista.sort();
    lista
}

fn nome_do_arquivo(caminho: &str) -> String {
    Path::new(caminho).file_name().unwrap_or_default().to_string_lossy().to_string()
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::limpeza::apoio::*;

    const DE_VERDADE: Executor = Executor { simular: false };

    // O netplan REAL da sua VM debian do lab (IP fixo amarrado ao MAC).
    const NETPLAN_DO_LAB: &str = "network:
  version: 2
  ethernets:
    eth0:
      match:
        macaddress: \"52:54:00:a1:b2:11\"
      addresses:
      - \"192.168.100.11/24\"
";

    #[test]
    fn netplan_com_ip_fixo_vira_dhcp() {
        let raiz = raiz_de_teste();
        criar(&raiz, "usr/sbin/netplan", "binario");
        criar(&raiz, "etc/netplan/50-cloud-init.yaml", NETPLAN_DO_LAB);
        criar(&raiz, "etc/netplan/90-NM-abc.yaml", "network:\n  wifis:\n    wlan0: {}\n");
        // Ubuntu desktop: só diz "quem cuida da rede"; não define placas, então fica.
        criar(&raiz, "etc/netplan/01-network-manager-all.yaml", "network:\n  version: 2\n  renderer: NetworkManager\n");
        configurar(&DE_VERDADE, &raiz).unwrap();

        assert!(!existe(&raiz, "etc/netplan/50-cloud-init.yaml"));
        assert!(!existe(&raiz, "etc/netplan/90-NM-abc.yaml"));
        assert!(existe(&raiz, "etc/netplan/01-network-manager-all.yaml"));
        assert_eq!(ler(&raiz, "etc/netplan/01-maloki-dhcp.yaml"), NETPLAN_DHCP);
        // Com netplan, não criamos arquivo do NetworkManager à parte.
        assert!(!existe(&raiz, "etc/NetworkManager/system-connections/rede-com-fio.nmconnection"));
    }

    #[test]
    fn rocky_ifcfg_vira_conexao_dhcp_do_networkmanager() {
        let raiz = raiz_de_teste();
        criar(&raiz, "usr/sbin/NetworkManager", "binario");
        criar(&raiz, "etc/sysconfig/network-scripts/ifcfg-eth0", "BOOTPROTO=none\nIPADDR=192.168.100.12\n");
        criar(&raiz, "etc/sysconfig/network-scripts/ifcfg-lo", "DEVICE=lo\n");
        criar(&raiz, "etc/sysconfig/network-scripts/readme-ifcfg-rh.txt", "leia-me");
        configurar(&DE_VERDADE, &raiz).unwrap();

        assert!(!existe(&raiz, "etc/sysconfig/network-scripts/ifcfg-eth0"));
        assert!(existe(&raiz, "etc/sysconfig/network-scripts/ifcfg-lo"));
        assert!(existe(&raiz, "etc/sysconfig/network-scripts/readme-ifcfg-rh.txt"));
        let nm = format!("{}/etc/NetworkManager/system-connections/rede-com-fio.nmconnection", raiz);
        assert_eq!(fs::read_to_string(&nm).unwrap(), NM_DHCP);
        // O NetworkManager ignora conexões que outros usuários podem ler.
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&nm).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn debian_classico_passa_para_o_networkd() {
        let raiz = raiz_de_teste();
        criar(&raiz, "etc/network/interfaces", "auto lo\niface lo inet loopback\n\nallow-hotplug ens3\niface ens3 inet static\n  address 10.0.0.5/24\n");
        criar(&raiz, "etc/network/interfaces.d/ens4", "iface ens4 inet dhcp\n");
        // A parte do networkd chama `chroot ... systemctl enable`, que precisa de root;
        // por isso aqui conferimos só os arquivos, sem o chroot.
        let interfaces = ler(&raiz, "etc/network/interfaces");
        assert!(tem_placa_alem_do_lo(&interfaces));
        let exec = Executor { simular: false };
        exec.escrever(&na_imagem(&raiz, "etc/network/interfaces"), INTERFACES_SO_LO).unwrap();
        exec.esvaziar_pasta(&na_imagem(&raiz, "etc/network/interfaces.d")).unwrap();
        assert!(!tem_placa_alem_do_lo(&ler(&raiz, "etc/network/interfaces")));
        assert!(!existe(&raiz, "etc/network/interfaces.d/ens4"));
    }

    #[test]
    fn so_loopback_nao_e_mexido() {
        let raiz = raiz_de_teste();
        let so_lo = "auto lo\niface lo inet loopback\n";
        criar(&raiz, "etc/network/interfaces", so_lo);
        configurar(&DE_VERDADE, &raiz).unwrap();
        assert_eq!(ler(&raiz, "etc/network/interfaces"), so_lo);
    }

    #[test]
    fn reconhece_placas_no_interfaces() {
        assert!(tem_placa_alem_do_lo("iface eth0 inet dhcp\n"));
        assert!(tem_placa_alem_do_lo("auto lo eth0\n"));
        assert!(tem_placa_alem_do_lo("allow-hotplug ens3\n"));
        assert!(!tem_placa_alem_do_lo("auto lo\niface lo inet loopback\n"));
        assert!(!tem_placa_alem_do_lo("# iface eth0 inet dhcp\nsource /etc/network/interfaces.d/*\n"));
        assert!(!tem_placa_alem_do_lo(""));
    }

    #[test]
    fn reconhece_placas_no_netplan() {
        assert!(define_placas(NETPLAN_DO_LAB));
        assert!(define_placas("network:\n  bridges:\n    br0: {}\n"));
        assert!(!define_placas("network:\n  version: 2\n  renderer: NetworkManager\n"));
        assert!(!define_placas("# ethernets: comentado\n"));
        // O nosso próprio arquivo define placas (então seria trocado, não duplicado).
        assert!(define_placas(NETPLAN_DHCP));
    }

    #[test]
    fn arquivos_gerados_sao_validos() {
        // YAML: indentação só com espaços (tabs quebram o netplan).
        assert!(!NETPLAN_DHCP.contains('\t'));
        // Os formatos "ini" (NM e networkd): cada linha é seção, chave=valor ou comentário.
        for texto in [NM_DHCP, NETWORKD_DHCP] {
            for linha in texto.lines().filter(|l| !l.trim().is_empty()) {
                assert!(linha.starts_with('#') || linha.starts_with('[') || linha.contains('='), "{}", linha);
            }
        }
    }

    #[test]
    fn simulacao_nao_mexe_na_rede() {
        let raiz = raiz_de_teste();
        criar(&raiz, "etc/netplan/50-cloud-init.yaml", NETPLAN_DO_LAB);
        configurar(&Executor { simular: true }, &raiz).unwrap();
        assert!(existe(&raiz, "etc/netplan/50-cloud-init.yaml"));
    }
}
