// Menu para criar os usuários da imagem, e os comandos que os criam.

use crate::executor::Executor;
use crate::limpeza::{na_imagem, usuarios_humanos};
use crate::Resultado;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Password};
use std::fs;

pub struct Usuario {
    pub nome: String,
    pub senha: String,
    pub senha_e_hash: bool,         // true = a senha já vem criptografada ($6$..., $y$...)
    pub admin: bool,                // pode usar sudo?
    pub trocar_senha: bool,         // obrigar a trocar a senha no 1º login?
    pub chave_ssh: Option<String>,  // Option: pode ter uma chave (Some) ou não (None)
}

/// Pergunta, em loop, os dados de cada usuário até a pessoa dizer que acabou.
/// `passwd` e `group` são os arquivos deste computador (para recusar nomes já usados).
pub fn perguntar(tema: &ColorfulTheme, passwd: &str, group: &str) -> Resultado<Vec<Usuario>> {
    let mut usuarios: Vec<Usuario> = Vec::new();

    loop {
        let pergunta = if usuarios.is_empty() { "Criar um usuário na imagem?" } else { "Criar mais um usuário?" };
        let continuar = Confirm::with_theme(tema)
            .with_prompt(pergunta)
            .default(usuarios.is_empty()) // sugere "sim" só para o primeiro
            .interact()?;
        if !continuar {
            break;
        }

        // Guardamos os nomes já usados para não deixar repetir.
        let ja_usados: Vec<String> = usuarios.iter().map(|u| u.nome.clone()).collect();

        // validate_with roda a função a cada tentativa; se ela devolver Err,
        // o dialoguer mostra a mensagem e pergunta de novo.
        let nome: String = Input::with_theme(tema)
            .with_prompt("Nome de login")
            .validate_with(|texto: &String| -> Result<(), String> {
                if ja_usados.contains(texto) {
                    return Err("esse usuário já está na lista".into());
                }
                validar_nome(texto)?;
                nome_disponivel(texto, passwd, group)
            })
            .interact_text()?;

        // Password esconde o que é digitado e pede para confirmar.
        let senha = Password::with_theme(tema)
            .with_prompt("Senha")
            .with_confirmation("Repita a senha", "As senhas não são iguais, tente de novo")
            .interact()?;

        let admin = Confirm::with_theme(tema)
            .with_prompt("Este usuário pode usar sudo (administrador)?")
            .default(true)
            .interact()?;

        let trocar_senha = Confirm::with_theme(tema)
            .with_prompt("Obrigar a trocar a senha no primeiro login?")
            .default(false)
            .interact()?;

        let chave_ssh = perguntar_chave_ssh(tema)?;

        usuarios.push(Usuario { nome, senha, senha_e_hash: false, admin, trocar_senha, chave_ssh });
        println!("✔ usuário adicionado à lista\n");
    }
    Ok(usuarios)
}

/// Regras de nome de usuário do Linux: começa com letra minúscula ou _,
/// depois letras minúsculas, números, _ ou -, até 32 caracteres.
pub fn validar_nome(nome: &str) -> Result<(), String> {
    let reservados = ["root", "bin", "daemon", "adm", "nobody", "sshd", "wheel", "sudo"];
    if reservados.contains(&nome) {
        return Err(format!("'{}' é um nome reservado do sistema", nome));
    }
    if nome.is_empty() || nome.len() > 32 {
        return Err("use de 1 a 32 caracteres".into());
    }
    // chars() percorre as letras; nth(0) é a primeira.
    let primeira = nome.chars().next().unwrap_or(' ');
    if !(primeira.is_ascii_lowercase() || primeira == '_') {
        return Err("comece com uma letra minúscula".into());
    }
    let valido = nome.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if !valido {
        return Err("use só letras minúsculas, números, _ ou -".into());
    }
    Ok(())
}

/// O nome não pode ser de uma conta do sistema (gdm, sshd, postgres...) nem de
/// um grupo que já existe (docker, libvirt...): o `useradd` falharia só no
/// passo 7, depois de toda a cópia. As contas de PESSOAS não contam: elas são
/// removidas da imagem antes, então o nome delas fica livre.
pub fn nome_disponivel(nome: &str, passwd: &str, group: &str) -> Result<(), String> {
    if usuarios_humanos(passwd).iter().any(|humano| humano == nome) {
        return Ok(());
    }
    // A primeira coluna (antes do ":") dos dois arquivos é o nome.
    let tem = |arquivo: &str| arquivo.lines().any(|linha| linha.split(':').next() == Some(nome));
    if tem(passwd) {
        return Err(format!("'{}' já é uma conta do sistema", nome));
    }
    if tem(group) {
        return Err(format!("já existe um grupo '{}' no sistema; escolha outro nome", nome));
    }
    Ok(())
}

/// Aceita a chave colada direto ou o caminho de um arquivo .pub.
fn perguntar_chave_ssh(tema: &ColorfulTheme) -> Resultado<Option<String>> {
    loop {
        let texto: String = Input::with_theme(tema)
            .with_prompt("Chave SSH pública (cole a chave ou o caminho do .pub; Enter para pular)")
            .allow_empty(true)
            .interact_text()?;
        match ler_chave(&texto, &pasta_pessoal()) {
            Ok(chave) => return Ok(chave),
            // Mostra o problema e volta para o início do loop (pergunta de novo).
            Err(problema) => println!("  {}", problema),
        }
    }
}

/// Transforma o que a pessoa digitou numa chave SSH:
///   ""                  -> Ok(None)        (pulou)
///   "ssh-ed25519 AAA.." -> Ok(Some(chave)) (colou a chave)
///   "~/.ssh/id.pub"     -> lê o arquivo e devolve a chave dentro dele
pub fn ler_chave(texto: &str, casa: &str) -> Result<Option<String>, String> {
    let texto = texto.trim();
    if texto.is_empty() {
        return Ok(None);
    }
    // Se não parece uma chave, tratamos como caminho de arquivo.
    let chave = if parece_chave(texto) {
        texto.to_string()
    } else {
        let caminho = expandir_til(texto, casa);
        match fs::read_to_string(&caminho) {
            Ok(conteudo) => conteudo.trim().to_string(),
            Err(e) => return Err(format!("não consegui ler {}: {}", caminho, e)),
        }
    };
    if !parece_chave(&chave) {
        return Err("isso não parece uma chave pública SSH (deveria começar com ssh-, ecdsa- ou sk-)".into());
    }
    if chave.contains("PRIVATE KEY") {
        return Err("essa é a chave PRIVADA! Use o arquivo .pub".into());
    }
    Ok(Some(chave))
}

fn parece_chave(texto: &str) -> bool {
    texto.starts_with("ssh-") || texto.starts_with("ecdsa-") || texto.starts_with("sk-")
}

/// Pasta pessoal de quem chamou o programa.
/// Com sudo, o $HOME vira o do root; o SUDO_USER diz quem chamou o sudo.
pub fn pasta_pessoal() -> String {
    match std::env::var("SUDO_USER") {
        Ok(usuario) => format!("/home/{}", usuario),
        Err(_) => std::env::var("HOME").unwrap_or_default(),
    }
}

/// "~/.ssh/id.pub" -> "/home/SEU_USUARIO/.ssh/id.pub".
fn expandir_til(caminho: &str, casa: &str) -> String {
    match caminho.strip_prefix("~/") {
        Some(resto) => format!("{}/{}", casa.trim_end_matches('/'), resto),
        None => caminho.to_string(),
    }
}

/// Cria os usuários DENTRO da imagem (`-R raiz` em cada comando).
pub fn criar(exec: &Executor, raiz: &str, usuarios: &[Usuario], grupo_admin: &str) -> Resultado<()> {
    for u in usuarios {
        // -m cria a pasta /home/nome copiando o /etc/skel da imagem.
        let mut args = vec!["-R", raiz, "-m", "-s", "/bin/bash"];
        if u.admin {
            args.push("-G");
            args.push(grupo_admin);
        }
        args.push(&u.nome);
        exec.rodar("useradd", &args)?;

        definir_senha(exec, raiz, &u.nome, &u.senha, u.senha_e_hash)?;

        if u.trocar_senha {
            // "última troca de senha = dia 0" força a troca no próximo login.
            exec.rodar("chage", &["-R", raiz, "-d", "0", &u.nome])?;
        }

        if let Some(chave) = &u.chave_ssh {
            let pasta = na_imagem(raiz, &format!("home/{}/.ssh", u.nome));
            exec.rodar("mkdir", &["-p", &pasta])?;
            exec.escrever(&format!("{}/authorized_keys", pasta), &format!("{}\n", chave))?;
            // O usuário só existe no /etc/passwd DA IMAGEM, então o chown roda via chroot.
            let dono = format!("{0}:{0}", u.nome);
            let pasta_na_vm = format!("/home/{}/.ssh", u.nome);
            exec.rodar("chroot", &[raiz, "chown", "-R", &dono, &pasta_na_vm])?;
            // O SSH recusa chaves se as permissões forem abertas demais.
            exec.rodar("chmod", &["700", &pasta])?;
            exec.rodar("chmod", &["600", &format!("{}/authorized_keys", pasta)])?;
        }
    }
    Ok(())
}

/// O /etc/shadow copiado tem o hash da senha do root do SEU computador.
/// Por isso a senha do root é sempre trocada ou bloqueada.
pub fn configurar_root(exec: &Executor, raiz: &str, senha: Option<&str>, senha_e_hash: bool) -> Resultado<()> {
    match senha {
        Some(senha) => definir_senha(exec, raiz, "root", senha, senha_e_hash),
        // "!" no lugar do hash = nenhuma senha funciona (e o hash antigo some).
        None => exec.rodar("usermod", &["-R", raiz, "-p", "!", "root"]),
    }
}

/// Grava a senha de um usuário DA IMAGEM.
///
/// O Linux nunca guarda a senha em si, só um "hash" dela (um resumo
/// criptográfico, ex.: "$6$sal$..."). Geramos o hash aqui mesmo, em Rust,
/// e entregamos pronto para o `usermod -p`.
///
/// Por que não o `chpasswd`? No Fedora 44 ele falha dentro de outra raiz
/// ("cannot open /etc/passwd"), tanto com `-R` quanto com `chroot`.
/// O `usermod -R` funciona igual em todas as distros testadas.
pub fn definir_senha(exec: &Executor, raiz: &str, nome: &str, senha: &str, senha_e_hash: bool) -> Resultado<()> {
    let hash = if senha_e_hash {
        senha.to_string()
    } else {
        // SHA-512 com "sal" aleatório: o mesmo formato $6$ do `openssl passwd -6`.
        pwhash::sha512_crypt::hash(senha).map_err(|e| format!("não consegui gerar o hash da senha: {}", e))?
    };
    // O hash vai na linha de comando; `rodar_com_segredo` mostra "********" no lugar dele.
    exec.rodar_com_segredo("usermod", &["-R", raiz, "-p", &hash, nome], &hash)
}

#[cfg(test)]
mod testes {
    use super::*;

    // Aqui já existe uma função `criar` (a que cria usuários), então trazemos a
    // dos testes com outro nome: `as criar_arquivo` dá um apelido para ela.
    use crate::limpeza::apoio::{criar as criar_arquivo, raiz_de_teste};

    #[test]
    fn nomes_validos_e_invalidos() {
        assert!(validar_nome("maria").is_ok());
        assert!(validar_nome("dev_ops-2").is_ok());
        assert!(validar_nome("_servico").is_ok());
        assert!(validar_nome(&"a".repeat(32)).is_ok());
        assert!(validar_nome(&"a".repeat(33)).is_err()); // longo demais
        assert!(validar_nome("Maria").is_err());   // maiúscula
        assert!(validar_nome("2maria").is_err());  // começa com número
        assert!(validar_nome("-maria").is_err());  // começa com hífen
        assert!(validar_nome("ma ria").is_err());  // espaço
        assert!(validar_nome("maria:x").is_err()); // ":" quebraria o /etc/passwd
        assert!(validar_nome("joão").is_err());    // acento
        assert!(validar_nome("ma.ria").is_err());
        assert!(validar_nome("").is_err());
        for reservado in ["root", "bin", "daemon", "adm", "nobody", "sshd", "wheel", "sudo"] {
            assert!(validar_nome(reservado).is_err(), "{}", reservado);
        }
    }

    #[test]
    fn todo_nome_aceito_e_seguro_para_o_passwd() {
        // Testa todas as combinações de 1 e 2 caracteres ASCII (mais de 16 mil nomes).
        // Qualquer nome aceito NÃO pode ter ":", espaço, "/" nem quebra de linha,
        // que quebrariam o /etc/passwd ou o comando chpasswd.
        let mut aceitos = 0;
        for a in 0u8..128 {
            for b in 0u8..128 {
                for nome in [String::from(a as char), format!("{}{}", a as char, b as char)] {
                    if validar_nome(&nome).is_ok() {
                        aceitos += 1;
                        assert!(!nome.contains([':', ' ', '/', '\n', '\t', '\0']), "{:?}", nome);
                        assert!(nome.chars().next().unwrap().is_ascii_lowercase() || nome.starts_with('_'));
                    }
                }
            }
        }
        assert!(aceitos > 1000); // e ainda aceita bastante coisa normal
    }

    #[test]
    fn til_vira_a_pasta_pessoal() {
        assert_eq!(expandir_til("~/.ssh/id.pub", "/home/daniel"), "/home/daniel/.ssh/id.pub");
        assert_eq!(expandir_til("~/.ssh/id.pub", "/home/daniel/"), "/home/daniel/.ssh/id.pub");
        assert_eq!(expandir_til("/etc/chave.pub", "/home/daniel"), "/etc/chave.pub");
        assert_eq!(expandir_til("~maria/x", "/home/daniel"), "~maria/x"); // só "~/" é expandido
    }

    #[test]
    fn chave_colada_direto() {
        let chave = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExemplo daniel@pc";
        assert_eq!(ler_chave(chave, "/x").unwrap().as_deref(), Some(chave));
        assert_eq!(ler_chave(&format!("  {}  \n", chave), "/x").unwrap().as_deref(), Some(chave));
        assert!(ler_chave("ecdsa-sha2-nistp256 AAAA x", "/x").unwrap().is_some());
        assert!(ler_chave("sk-ssh-ed25519@openssh.com AAAA x", "/x").unwrap().is_some());
    }

    #[test]
    fn enter_pula_a_chave() {
        assert_eq!(ler_chave("", "/x").unwrap(), None);
        assert_eq!(ler_chave("   ", "/x").unwrap(), None);
    }

    #[test]
    fn chave_lida_de_arquivo() {
        let casa = raiz_de_teste();
        criar_arquivo(&casa, ".ssh/id_ed25519.pub", "ssh-ed25519 AAAAarquivo daniel@pc\n");
        let lida = ler_chave("~/.ssh/id_ed25519.pub", &casa).unwrap();
        assert_eq!(lida.as_deref(), Some("ssh-ed25519 AAAAarquivo daniel@pc"));
        let lida = ler_chave(&format!("{}/.ssh/id_ed25519.pub", casa), "/outra").unwrap();
        assert!(lida.is_some());
    }

    #[test]
    fn arquivo_errado_da_mensagem_clara() {
        let casa = raiz_de_teste();
        criar_arquivo(&casa, ".ssh/id_ed25519", "-----BEGIN OPENSSH PRIVATE KEY-----\nxxx\n");
        criar_arquivo(&casa, "texto.txt", "isso não é chave\n");
        assert!(ler_chave("~/.ssh/nao-existe.pub", &casa).unwrap_err().contains("não consegui ler"));
        assert!(ler_chave("~/texto.txt", &casa).unwrap_err().contains("não parece"));
        assert!(ler_chave("~/.ssh/id_ed25519", &casa).is_err()); // chave privada recusada
        assert!(ler_chave("ssh-rsa -----BEGIN OPENSSH PRIVATE KEY-----", "/x").unwrap_err().contains("PRIVADA"));
    }

    #[test]
    fn hash_da_senha_funciona_e_e_aleatorio() {
        let h1 = pwhash::sha512_crypt::hash("Senha123").unwrap();
        let h2 = pwhash::sha512_crypt::hash("Senha123").unwrap();
        assert!(h1.starts_with("$6$"));             // formato SHA-512 do Linux
        assert_ne!(h1, h2);                          // "sal" diferente a cada vez
        assert!(pwhash::sha512_crypt::verify("Senha123", &h1));
        assert!(!pwhash::sha512_crypt::verify("senha123", &h1));
        // O hash vai para o /etc/shadow: não pode ter ":" nem quebra de linha.
        assert!(!h1.contains([':', '\n']));
        // Confere com uma fonte independente: o openssl, se existir neste PC.
        if let Ok(saida) = std::process::Command::new("openssl")
            .args(["passwd", "-6", "-salt", "abcdefgh", "Senha123"])
            .output()
        {
            let do_openssl = String::from_utf8_lossy(&saida.stdout).trim().to_string();
            if do_openssl.starts_with("$6$") {
                assert!(pwhash::sha512_crypt::verify("Senha123", &do_openssl));
            }
        }
    }

    #[test]
    fn hash_nunca_aparece_na_tela() {
        // Em simulação só imprime; o importante é não dar erro (o "********"
        // é conferido olhando a saída de `cargo test -- --nocapture`).
        let exec = Executor { simular: true };
        definir_senha(&exec, "/mnt/maloki-1", "maria", "Senha123", false).unwrap();
        definir_senha(&exec, "/mnt/maloki-1", "maria", "$6$sal$hash", true).unwrap();
    }

    #[test]
    fn criar_usuarios_em_simulacao_nao_falha() {
        let exec = Executor { simular: true };
        let lista = vec![
            Usuario { nome: "maria".into(), senha: "s".into(), senha_e_hash: false, admin: true, trocar_senha: true, chave_ssh: Some("ssh-ed25519 A".into()) },
            Usuario { nome: "joao".into(), senha: "$6$sal$hash".into(), senha_e_hash: true, admin: false, trocar_senha: false, chave_ssh: None },
        ];
        criar(&exec, "/mnt/maloki-1", &lista, "wheel").unwrap();
        configurar_root(&exec, "/mnt/maloki-1", None, false).unwrap();
        configurar_root(&exec, "/mnt/maloki-1", Some("x"), false).unwrap();
        configurar_root(&exec, "/mnt/maloki-1", Some("$6$sal$hash"), true).unwrap();
    }
}
