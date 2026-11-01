// Remove da cópia tudo o que é pessoal ou que identifica ESTA máquina,
// para que cada VM criada a partir do template seja "nova".

use crate::disco::PREFIXO_RAIZ;
use crate::executor::Executor;
use crate::Resultado;
use std::fs;

/// Monta o caminho de um arquivo DENTRO da imagem: na_imagem(raiz, "etc/hostname").
///
/// Trava de segurança: se por algum bug a raiz não for a nossa pasta de montagem,
/// ou se o caminho tentar "subir" de pasta com "..", o programa para na hora
/// (`panic!`) em vez de apagar arquivos do seu sistema real.
pub fn na_imagem(raiz: &str, relativo: &str) -> String {
    let sobe_pasta = raiz.split('/').any(|p| p == "..") || relativo.split('/').any(|p| p == "..");
    if !raiz_permitida(raiz) || sobe_pasta {
        panic!("caminho inesperado '{}' + '{}': abortando para proteger o sistema", raiz, relativo);
    }
    format!("{}/{}", raiz.trim_end_matches('/'), relativo.trim_start_matches('/'))
}

fn raiz_permitida(raiz: &str) -> bool {
    if raiz.starts_with(PREFIXO_RAIZ) && raiz.len() > PREFIXO_RAIZ.len() {
        return true;
    }
    // Nos testes automáticos (`cargo test`) a "imagem" é uma pasta temporária.
    // `cfg!(test)` vale `false` no programa de verdade, então isso não existe nele.
    cfg!(test) && raiz.starts_with(&prefixo_de_teste())
}

fn prefixo_de_teste() -> String {
    std::env::temp_dir().join("maloki-teste-").to_string_lossy().to_string()
}

// Arquivos que identificam a máquina ou guardam segredos.
const ARQUIVOS: &[&str] = &[
    "var/lib/dbus/machine-id",       // cópia antiga do machine-id
    "var/lib/systemd/random-seed",   // semente de números aleatórios
    "var/lib/systemd/credential.secret",
    "etc/krb5.keytab",               // credencial de domínio (Kerberos)
];

// Pastas cujo conteúdo deve sumir (as pastas continuam existindo).
const PASTAS: &[&str] = &[
    "etc/NetworkManager/system-connections", // redes Wi-Fi e SENHAS do Wi-Fi
    "var/lib/NetworkManager",                // redes vistas, leases de IP
    "var/lib/bluetooth",                     // dispositivos pareados
    "var/lib/fprint",                        // impressões digitais
    "var/lib/AccountsService/users",         // configurações da tela de login
    "var/lib/AccountsService/icons",         // fotos dos usuários
    "var/lib/sss/db",                        // cache de login de rede
    "var/lib/cloud",                         // estado do cloud-init (roda de novo na VM)
    "var/spool/mail",                        // e-mails locais
    "var/lib/iwd",                           // redes Wi-Fi do iwd (com senha)
    "etc/wireguard",                         // chaves privadas de VPN WireGuard
    "etc/openvpn/client",                    // configurações/chaves de VPN como cliente
    "etc/cryptsetup-keys.d",                 // arquivos que destravam discos criptografados
];

// Pastas onde costumam ficar chaves PRIVADAS de certificados (sites HTTPS etc.).
// Essas NÃO são apagadas (o servidor web pode precisar delas para ligar),
// mas avisamos no final para a pessoa decidir.
const PASTAS_COM_CHAVES: &[&str] = &[
    "etc/pki/tls/private",
    "etc/ssl/private",
    "etc/letsencrypt/archive",
    "etc/openvpn/server",
];

pub fn limpar(exec: &Executor, raiz: &str, hostname: &str) -> Resultado<()> {
    // machine-id vazio = o systemd gera um novo no primeiro boot de cada VM.
    exec.escrever(&na_imagem(raiz, "etc/machine-id"), "")?;

    for arquivo in ARQUIVOS {
        exec.apagar(&na_imagem(raiz, arquivo))?;
    }
    for pasta in PASTAS {
        exec.esvaziar_pasta(&na_imagem(raiz, pasta))?;
    }

    // Chaves do servidor SSH: cada VM precisa das suas (são recriadas no boot).
    apagar_que_comecam_com(exec, &na_imagem(raiz, "etc/ssh"), "ssh_host_")?;

    // Ubuntu 23.10+: o NetworkManager guarda as redes (com senha do Wi-Fi!)
    // como arquivos 90-NM-*.yaml do netplan.
    apagar_que_comecam_com(exec, &na_imagem(raiz, "etc/netplan"), "90-NM-")?;

    // wpa_supplicant (Wi-Fi sem NetworkManager): apaga só os arquivos com senha.
    apagar_arquivos_com_senha(exec, &na_imagem(raiz, "etc/wpa_supplicant"))?;

    // Logs antigos: apagamos os arquivos mas mantemos as pastas,
    // porque alguns serviços não sobem se a pasta de log não existir.
    apagar_arquivos(exec, &na_imagem(raiz, "var/log"))?;

    // O disco da VM não é criptografado: não há nada para destravar no boot.
    exec.escrever(&na_imagem(raiz, "etc/crypttab"), "")?;
    exec.escrever(&na_imagem(raiz, "etc/hostname"), &format!("{}\n", hostname))?;

    // Login automático apontaria para um usuário que não existe mais.
    exec.editar(&na_imagem(raiz, "etc/gdm/custom.conf"), desligar_autologin)?;
    exec.editar(&na_imagem(raiz, "etc/gdm3/custom.conf"), desligar_autologin)?;

    // SELinux (Fedora): pede para reetiquetar os arquivos no primeiro boot.
    let selinux = fs::read_to_string(na_imagem(raiz, "etc/selinux/config")).unwrap_or_default();
    if selinux_ativo(&selinux) || exec.simular {
        exec.escrever(&na_imagem(raiz, ".autorelabel"), "")?;
    }
    Ok(())
}

/// Lê o /etc/selinux/config e diz se o SELinux está ligado.
fn selinux_ativo(config: &str) -> bool {
    config.lines().any(|linha| {
        let linha = linha.trim();
        linha == "SELINUX=enforcing" || linha == "SELINUX=permissive"
    })
}

/// Remove da imagem todas as contas de pessoas (as suas também!).
pub fn remover_usuarios(exec: &Executor, raiz: &str) -> Resultado<()> {
    // Na simulação a imagem não existe, então lemos o /etc/passwd do sistema
    // atual só para mostrar quem SERIA removido.
    let passwd = if exec.simular {
        fs::read_to_string("/etc/passwd")?
    } else {
        fs::read_to_string(na_imagem(raiz, "etc/passwd"))?
    };

    let removidos = usuarios_humanos(&passwd);
    for nome in &removidos {
        // -R = "a raiz do sistema é esta pasta": edita o /etc/passwd DA IMAGEM.
        exec.rodar("userdel", &["-R", raiz, nome])?;
    }

    // Regras de sudo só para esses usuários (ex.: /etc/sudoers.d/daniel) também saem.
    if !exec.simular {
        for arquivo in listar_arquivos(&na_imagem(raiz, "etc/sudoers.d")) {
            let texto = fs::read_to_string(&arquivo).unwrap_or_default();
            if removidos.iter().any(|nome| menciona_usuario(&texto, nome)) {
                exec.apagar(&arquivo)?;
            }
        }
    }
    Ok(())
}

/// O texto cita o usuário como palavra inteira? ("daniel ALL=..." sim; "danielle" não)
pub fn menciona_usuario(texto: &str, nome: &str) -> bool {
    texto
        .lines()
        .filter(|linha| !linha.trim_start().starts_with('#'))
        // Separa em "palavras" tudo que não for letra, número, _ ou -.
        .flat_map(|linha| linha.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')))
        .any(|palavra| palavra == nome)
}

/// Procura chaves privadas que ficaram na imagem (para avisar, não apagar).
pub fn procurar_chaves_privadas(raiz: &str) -> Vec<String> {
    let mut achadas = Vec::new();
    for pasta in PASTAS_COM_CHAVES {
        for arquivo in listar_tudo(&na_imagem(raiz, pasta)) {
            // A "snakeoil" é uma chave de exemplo que o Debian gera; não é segredo seu.
            if !arquivo.contains("snakeoil") {
                // Mostramos o caminho como ele é DENTRO da VM (sem a pasta de montagem).
                achadas.push(arquivo.trim_start_matches(raiz).to_string());
            }
        }
    }
    achadas
}

/// Arquivos de uma pasta (sem entrar em subpastas).
fn listar_arquivos(pasta: &str) -> Vec<String> {
    let Ok(entradas) = fs::read_dir(pasta) else { return Vec::new() };
    entradas
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.path().to_string_lossy().to_string())
        .collect()
}

/// Arquivos de uma pasta E de todas as subpastas (recursão, como em apagar_arquivos).
fn listar_tudo(pasta: &str) -> Vec<String> {
    let mut lista = Vec::new();
    let Ok(entradas) = fs::read_dir(pasta) else { return lista };
    for entrada in entradas.flatten() {
        let caminho = entrada.path().to_string_lossy().to_string();
        match entrada.file_type() {
            Ok(tipo) if tipo.is_dir() => lista.extend(listar_tudo(&caminho)),
            Ok(_) => lista.push(caminho),
            Err(_) => {}
        }
    }
    lista.sort();
    lista
}

/// Apaga os arquivos de uma pasta que contêm senha de Wi-Fi (psk= ou password=).
fn apagar_arquivos_com_senha(exec: &Executor, pasta: &str) -> Resultado<()> {
    if exec.simular {
        println!("[simulação] apagaria os arquivos com senha dentro de {}/", pasta);
        return Ok(());
    }
    for arquivo in listar_arquivos(pasta) {
        let texto = fs::read_to_string(&arquivo).unwrap_or_default();
        if texto.contains("psk=") || texto.contains("password=") {
            exec.apagar(&arquivo)?;
        }
    }
    Ok(())
}

/// Lê o texto do /etc/passwd e devolve os nomes dos usuários "humanos".
/// Cada linha é: nome:x:UID:GID:descrição:home:shell
/// Por convenção, pessoas têm UID de 1000 a 59999; abaixo disso são contas do sistema.
pub fn usuarios_humanos(passwd: &str) -> Vec<String> {
    let mut nomes = Vec::new();
    for linha in passwd.lines() {
        let campos: Vec<&str> = linha.split(':').collect();
        if campos.len() < 3 || campos[0].is_empty() {
            continue;
        }
        // parse() converte o texto do UID em número (0 se não for número).
        let uid: u32 = campos[2].parse().unwrap_or(0);
        if (1000..60000).contains(&uid) {
            nomes.push(campos[0].to_string());
        }
    }
    nomes
}

/// Comenta as linhas de login automático do GDM.
fn desligar_autologin(texto: &str) -> String {
    let mut saida = String::new();
    for linha in texto.lines() {
        if linha.trim_start().starts_with("AutomaticLogin") {
            saida.push('#'); // vira comentário
        }
        saida.push_str(linha);
        saida.push('\n');
    }
    saida
}

fn apagar_que_comecam_com(exec: &Executor, pasta: &str, prefixo: &str) -> Resultado<()> {
    if exec.simular {
        println!("[simulação] apagaria {}/{}*", pasta, prefixo);
        return Ok(());
    }
    let Ok(entradas) = fs::read_dir(pasta) else { return Ok(()) };
    for entrada in entradas.flatten() {
        if entrada.file_name().to_string_lossy().starts_with(prefixo) {
            exec.apagar(&entrada.path().to_string_lossy())?;
        }
    }
    Ok(())
}

/// Apaga todos os arquivos de uma pasta e das subpastas, mantendo as pastas.
/// A função chama a si mesma para cada subpasta (isso se chama recursão).
fn apagar_arquivos(exec: &Executor, pasta: &str) -> Resultado<()> {
    if exec.simular {
        println!("[simulação] apagaria os arquivos dentro de {}/ (mantendo as pastas)", pasta);
        return Ok(());
    }
    let Ok(entradas) = fs::read_dir(pasta) else { return Ok(()) };
    for entrada in entradas.flatten() {
        let caminho = entrada.path().to_string_lossy().to_string();
        let Ok(tipo) = entrada.file_type() else { continue };
        // file_type() não segue links: um link para pasta conta como arquivo
        // e é apagado sem entrar na pasta para onde ele aponta.
        if tipo.is_dir() {
            apagar_arquivos(exec, &caminho)?;
        } else {
            fs::remove_file(&caminho)?;
        }
    }
    Ok(())
}

// Ferramentas usadas pelos testes de vários módulos.
#[cfg(test)]
pub mod apoio {
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Uma pasta temporária que se apaga sozinha quando o teste termina
    /// (o mesmo truque do `Drop` usado no `Disco`).
    pub struct PastaTeste(String);

    // `Deref` deixa usar a PastaTeste como se fosse a String de dentro:
    // `&pasta` funciona onde se espera `&str`.
    impl std::ops::Deref for PastaTeste {
        type Target = String;
        fn deref(&self) -> &String {
            &self.0
        }
    }

    impl std::fmt::Display for PastaTeste {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl Drop for PastaTeste {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Cria uma pasta temporária nova que faz o papel da "imagem".
    pub fn raiz_de_teste() -> PastaTeste {
        // Os testes rodam ao mesmo tempo; o contador garante uma pasta por teste.
        static CONTADOR: AtomicUsize = AtomicUsize::new(0);
        let n = CONTADOR.fetch_add(1, Ordering::SeqCst);
        let raiz = format!("{}{}-{}", super::prefixo_de_teste(), std::process::id(), n);
        let _ = fs::remove_dir_all(&raiz);
        fs::create_dir_all(&raiz).unwrap();
        PastaTeste(raiz)
    }

    /// Cria um arquivo (e as pastas do caminho) dentro da raiz de teste.
    pub fn criar(raiz: &str, relativo: &str, conteudo: &str) {
        let caminho = format!("{}/{}", raiz, relativo);
        fs::create_dir_all(Path::new(&caminho).parent().unwrap()).unwrap();
        fs::write(caminho, conteudo).unwrap();
    }

    pub fn ler(raiz: &str, relativo: &str) -> String {
        fs::read_to_string(format!("{}/{}", raiz, relativo)).unwrap()
    }

    pub fn existe(raiz: &str, relativo: &str) -> bool {
        // symlink_metadata: um link quebrado também "existe".
        fs::symlink_metadata(format!("{}/{}", raiz, relativo)).is_ok()
    }
}

#[cfg(test)]
mod testes {
    use super::apoio::*;
    use super::*;

    const DE_VERDADE: Executor = Executor { simular: false };

    /// Uma "máquina" falsa cheia de dados pessoais, imitando um Fedora/Ubuntu real.
    fn sistema_sujo() -> PastaTeste {
        let raiz = raiz_de_teste();
        criar(&raiz, "etc/machine-id", "6f1e2d3c4b5a69788796a5b4c3d2e1f0\n");
        criar(&raiz, "var/lib/dbus/machine-id", "6f1e2d3c4b5a69788796a5b4c3d2e1f0\n");
        criar(&raiz, "var/lib/systemd/random-seed", "semente");
        criar(&raiz, "etc/krb5.keytab", "segredo");
        criar(&raiz, "etc/ssh/ssh_host_ed25519_key", "CHAVE PRIVADA");
        criar(&raiz, "etc/ssh/ssh_host_ed25519_key.pub", "ssh-ed25519 AAAA");
        criar(&raiz, "etc/ssh/ssh_host_rsa_key", "CHAVE PRIVADA");
        criar(&raiz, "etc/ssh/sshd_config", "PermitRootLogin no\n");
        criar(&raiz, "etc/NetworkManager/system-connections/CasaWifi.nmconnection", "psk=senha123");
        criar(&raiz, "etc/NetworkManager/NetworkManager.conf", "[main]\n");
        criar(&raiz, "etc/netplan/90-NM-1234abcd.yaml", "password: senha123");
        criar(&raiz, "etc/netplan/01-network-manager-all.yaml", "renderer: NetworkManager");
        criar(&raiz, "var/lib/NetworkManager/seen-bssids", "aa:bb:cc");
        criar(&raiz, "var/lib/bluetooth/AA:BB/info", "fone");
        criar(&raiz, "var/lib/fprint/daniel/digital", "dedo");
        criar(&raiz, "var/lib/AccountsService/users/daniel", "[User]");
        criar(&raiz, "var/lib/AccountsService/icons/daniel", "foto");
        criar(&raiz, "var/lib/cloud/instance/user-data.txt", "dados");
        criar(&raiz, "var/spool/mail/daniel", "email");
        criar(&raiz, "var/log/messages", "log");
        criar(&raiz, "var/log/audit/audit.log", "log");
        criar(&raiz, "var/log/gdm/old.log", "log");
        criar(&raiz, "etc/crypttab", "luks-9da4 UUID=9da4 none discard\n");
        criar(&raiz, "etc/hostname", "notebook-do-daniel\n");
        criar(&raiz, "etc/gdm/custom.conf", "[daemon]\nAutomaticLoginEnable=True\nAutomaticLogin=daniel\n");
        criar(&raiz, "etc/selinux/config", "# comentario\nSELINUX=enforcing\nSELINUXTYPE=targeted\n");
        criar(&raiz, "usr/bin/programa", "binario");
        criar(&raiz, "etc/wireguard/wg0.conf", "PrivateKey = segredo");
        criar(&raiz, "etc/openvpn/client/trabalho.conf", "<key>segredo</key>");
        criar(&raiz, "etc/cryptsetup-keys.d/dados.key", "chave do disco");
        criar(&raiz, "var/lib/iwd/CasaWifi.psk", "Passphrase=senha123");
        criar(&raiz, "etc/wpa_supplicant/wpa_supplicant-wlan0.conf", "network={\n psk=\"senha123\"\n}\n");
        criar(&raiz, "etc/wpa_supplicant/functions.sh", "# script do pacote, sem senha");
        raiz
    }

    #[test]
    fn limpa_tudo_que_e_pessoal() {
        let raiz = sistema_sujo();
        limpar(&DE_VERDADE, &raiz, "template").unwrap();

        // Identidade da máquina
        assert_eq!(ler(&raiz, "etc/machine-id"), "");
        assert!(!existe(&raiz, "var/lib/dbus/machine-id"));
        assert!(!existe(&raiz, "var/lib/systemd/random-seed"));
        assert!(!existe(&raiz, "etc/krb5.keytab"));
        assert_eq!(ler(&raiz, "etc/hostname"), "template\n");

        // SSH: chaves somem, configuração fica
        assert!(!existe(&raiz, "etc/ssh/ssh_host_ed25519_key"));
        assert!(!existe(&raiz, "etc/ssh/ssh_host_ed25519_key.pub"));
        assert!(!existe(&raiz, "etc/ssh/ssh_host_rsa_key"));
        assert_eq!(ler(&raiz, "etc/ssh/sshd_config"), "PermitRootLogin no\n");

        // Redes: senhas somem, configuração geral fica
        assert!(!existe(&raiz, "etc/NetworkManager/system-connections/CasaWifi.nmconnection"));
        assert!(existe(&raiz, "etc/NetworkManager/system-connections"));
        assert!(existe(&raiz, "etc/NetworkManager/NetworkManager.conf"));
        assert!(!existe(&raiz, "etc/netplan/90-NM-1234abcd.yaml"));
        assert!(existe(&raiz, "etc/netplan/01-network-manager-all.yaml"));
        assert!(!existe(&raiz, "var/lib/NetworkManager/seen-bssids"));

        // Dados de pessoas
        assert!(!existe(&raiz, "var/lib/bluetooth/AA:BB"));
        assert!(!existe(&raiz, "var/lib/fprint/daniel"));
        assert!(!existe(&raiz, "var/lib/AccountsService/users/daniel"));
        assert!(!existe(&raiz, "var/lib/AccountsService/icons/daniel"));
        assert!(!existe(&raiz, "var/lib/cloud/instance"));
        assert!(!existe(&raiz, "var/spool/mail/daniel"));
        assert!(existe(&raiz, "var/spool/mail"));

        // Logs: arquivos somem, pastas ficam
        assert!(!existe(&raiz, "var/log/messages"));
        assert!(!existe(&raiz, "var/log/audit/audit.log"));
        assert!(existe(&raiz, "var/log/audit"));
        assert!(existe(&raiz, "var/log/gdm"));

        // Boot e login
        assert_eq!(ler(&raiz, "etc/crypttab"), "");
        let gdm = ler(&raiz, "etc/gdm/custom.conf");
        assert!(gdm.contains("#AutomaticLogin=daniel"));
        assert!(gdm.contains("#AutomaticLoginEnable=True"));
        assert!(existe(&raiz, ".autorelabel"));

        // Programas continuam intactos
        assert_eq!(ler(&raiz, "usr/bin/programa"), "binario");

        // VPN, chave de disco e Wi-Fi (iwd e wpa_supplicant)
        assert!(!existe(&raiz, "etc/wireguard/wg0.conf"));
        assert!(!existe(&raiz, "etc/openvpn/client/trabalho.conf"));
        assert!(!existe(&raiz, "etc/cryptsetup-keys.d/dados.key"));
        assert!(!existe(&raiz, "var/lib/iwd/CasaWifi.psk"));
        assert!(!existe(&raiz, "etc/wpa_supplicant/wpa_supplicant-wlan0.conf"));
        assert!(existe(&raiz, "etc/wpa_supplicant/functions.sh")); // sem senha: fica
    }

    #[test]
    fn avisa_sobre_chaves_privadas_sem_apagar() {
        let raiz = raiz_de_teste();
        criar(&raiz, "etc/pki/tls/private/meusite.key", "-----BEGIN PRIVATE KEY-----");
        criar(&raiz, "etc/letsencrypt/archive/meusite.com/privkey1.pem", "-----BEGIN PRIVATE KEY-----");
        criar(&raiz, "etc/ssl/private/ssl-cert-snakeoil.key", "chave de exemplo do Debian");
        criar(&raiz, "etc/ssl/certs/publico.pem", "certificado público: não é segredo");
        let achadas = procurar_chaves_privadas(&raiz);
        assert_eq!(achadas, ["/etc/pki/tls/private/meusite.key", "/etc/letsencrypt/archive/meusite.com/privkey1.pem"]);
        // Avisar não é apagar: os arquivos continuam lá.
        assert!(existe(&raiz, "etc/pki/tls/private/meusite.key"));
        assert!(procurar_chaves_privadas(&raiz_de_teste()).is_empty());
    }

    #[test]
    fn reconhece_usuario_no_sudoers() {
        assert!(menciona_usuario("daniel ALL=(ALL) NOPASSWD:ALL\n", "daniel"));
        assert!(menciona_usuario("Defaults:daniel !requiretty\n", "daniel"));
        assert!(menciona_usuario("%wheel ALL=(ALL) ALL\ndaniel,maria ALL=(ALL) ALL\n", "maria"));
        assert!(!menciona_usuario("danielle ALL=(ALL) ALL\n", "daniel")); // outra pessoa
        assert!(!menciona_usuario("# daniel ALL=(ALL) ALL\n", "daniel")); // comentário
        assert!(!menciona_usuario("%wheel ALL=(ALL) ALL\n", "daniel"));
    }

    #[test]
    fn limpar_duas_vezes_da_o_mesmo_resultado() {
        let raiz = sistema_sujo();
        limpar(&DE_VERDADE, &raiz, "template").unwrap();
        let gdm1 = ler(&raiz, "etc/gdm/custom.conf");
        limpar(&DE_VERDADE, &raiz, "template").unwrap();
        // Não pode virar "##AutomaticLogin" na segunda vez.
        assert_eq!(ler(&raiz, "etc/gdm/custom.conf"), gdm1);
    }

    #[test]
    fn limpar_um_sistema_minimo_nao_da_erro() {
        // Uma imagem sem NetworkManager, sem SSH, sem GDM, sem SELinux...
        let raiz = raiz_de_teste();
        criar(&raiz, "etc/passwd", "root:x:0:0::/root:/bin/sh\n");
        limpar(&DE_VERDADE, &raiz, "minimo").unwrap();
        assert_eq!(ler(&raiz, "etc/hostname"), "minimo\n");
        assert!(!existe(&raiz, ".autorelabel")); // sem SELinux, sem relabel
        assert!(!existe(&raiz, "etc/gdm/custom.conf")); // não cria o que não existia
    }

    #[test]
    fn link_para_pasta_nao_e_seguido() {
        // Se /var/log tiver um link para fora, só o LINK pode sumir, nunca o destino.
        let raiz = raiz_de_teste();
        let fora = raiz_de_teste();
        criar(&fora, "precioso.txt", "não apague");
        criar(&raiz, "var/log/real.log", "log");
        std::os::unix::fs::symlink(fora.as_str(), format!("{}/var/log/link", raiz)).unwrap();
        apagar_arquivos(&DE_VERDADE, &na_imagem(&raiz, "var/log")).unwrap();
        assert!(!existe(&raiz, "var/log/link"));
        assert_eq!(ler(&fora, "precioso.txt"), "não apague");
    }

    #[test]
    fn simulacao_nao_mexe_em_nada() {
        let raiz = sistema_sujo();
        limpar(&Executor { simular: true }, &raiz, "template").unwrap();
        assert!(existe(&raiz, "etc/ssh/ssh_host_rsa_key"));
        assert!(existe(&raiz, "var/log/messages"));
        assert_eq!(ler(&raiz, "etc/hostname"), "notebook-do-daniel\n");
    }

    #[test]
    fn selinux_ligado_ou_desligado() {
        assert!(selinux_ativo("SELINUX=enforcing\n"));
        assert!(selinux_ativo("SELINUX=permissive\n"));
        assert!(!selinux_ativo("SELINUX=disabled\n"));
        assert!(!selinux_ativo("#SELINUX=enforcing\nSELINUX=disabled\n"));
        assert!(!selinux_ativo(""));
        // A linha SELINUXTYPE não pode confundir.
        assert!(!selinux_ativo("SELINUXTYPE=enforcing\n"));
    }

    #[test]
    fn sem_selinux_nao_cria_autorelabel() {
        let raiz = sistema_sujo();
        criar(&raiz, "etc/selinux/config", "SELINUX=disabled\n");
        limpar(&DE_VERDADE, &raiz, "t").unwrap();
        assert!(!existe(&raiz, ".autorelabel"));
    }

    #[test]
    fn acha_so_usuarios_humanos() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\n\
                      sshd:x:74:74::/usr/share/empty.sshd:/sbin/nologin\n\
                      daniel:x:1000:1000:Daniel:/home/daniel:/bin/zsh\n\
                      maria:x:1001:1001::/home/maria:/bin/bash\n\
                      nobody:x:65534:65534:Kernel Overflow User:/:/sbin/nologin\n";
        assert_eq!(usuarios_humanos(passwd), ["daniel", "maria"]);
    }

    #[test]
    fn usuarios_humanos_casos_dificeis() {
        // Limites: 999 é sistema, 1000 é pessoa, 59999 é pessoa, 60000 não.
        let passwd = "a:x:999:999::/:/bin/sh\nb:x:1000:1000::/:/bin/sh\n\
                      c:x:59999:1::/:/bin/sh\nd:x:60000:1::/:/bin/sh\n";
        assert_eq!(usuarios_humanos(passwd), ["b", "c"]);
        // Linhas vazias, quebradas, sem nome ou com UID que não é número.
        let lixo = "\n\nlinha-sem-dois-pontos\n:x:1000:1000::/:/bin/sh\ne:x:abc:1::/:/bin/sh\n+::::::\n";
        assert!(usuarios_humanos(lixo).is_empty());
        assert!(usuarios_humanos("").is_empty());
    }

    #[test]
    fn desliga_login_automatico() {
        let texto = "[daemon]\nAutomaticLoginEnable=True\nAutomaticLogin=daniel\n";
        assert_eq!(
            desligar_autologin(texto),
            "[daemon]\n#AutomaticLoginEnable=True\n#AutomaticLogin=daniel\n"
        );
        // Já comentado continua igual; outras linhas não mudam.
        assert_eq!(desligar_autologin("#AutomaticLogin=x\nWaylandEnable=false\n"), "#AutomaticLogin=x\nWaylandEnable=false\n");
        assert_eq!(desligar_autologin("  AutomaticLogin = x\n"), "#  AutomaticLogin = x\n");
    }

    #[test]
    #[should_panic]
    fn trava_bloqueia_a_raiz_do_sistema() {
        na_imagem("/", "etc/passwd");
    }

    #[test]
    #[should_panic]
    fn trava_bloqueia_pasta_vazia() {
        na_imagem("", "etc/passwd");
    }

    #[test]
    #[should_panic]
    fn trava_bloqueia_so_o_prefixo() {
        na_imagem("/mnt/maloki-", "etc/passwd");
    }

    #[test]
    #[should_panic]
    fn trava_bloqueia_subir_de_pasta_no_caminho() {
        na_imagem("/mnt/maloki-123", "../../etc/passwd");
    }

    #[test]
    #[should_panic]
    fn trava_bloqueia_subir_de_pasta_na_raiz() {
        na_imagem("/mnt/maloki-123/../..", "etc/passwd");
    }

    #[test]
    #[should_panic]
    fn trava_bloqueia_outras_pastas_do_mnt() {
        na_imagem("/mnt/outra-coisa", "etc/passwd");
    }

    #[test]
    fn trava_aceita_a_pasta_certa() {
        assert_eq!(na_imagem("/mnt/maloki-42", "etc/passwd"), "/mnt/maloki-42/etc/passwd");
        assert_eq!(na_imagem("/mnt/maloki-42/", "/etc/passwd"), "/mnt/maloki-42/etc/passwd");
        // Nomes que só CONTÊM pontos não são "..".
        assert_eq!(na_imagem("/mnt/maloki-42", "etc/..x"), "/mnt/maloki-42/etc/..x");
    }
}
