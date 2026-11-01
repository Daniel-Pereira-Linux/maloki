#!/usr/bin/env python3
# "Robô" que usa o maloki pelos menus, como uma pessoa faria:
# espera cada pergunta aparecer na tela e digita a resposta.
#
# uso: robo.py LOG CHAVE_PUB HOSTNAME [--cancelar-em TEXTO] -- COMANDO...
#   --cancelar-em: aperta Ctrl+C alguns segundos depois de TEXTO aparecer
import os, pty, re, select, sys, time

args = sys.argv[1:]
log, chave, hostname = args[0], args[1], args[2]
cancelar_em = None
if args[3] == "--cancelar-em":
    cancelar_em = args[4]
    args = args[5:]
else:
    args = args[3:]
comando = args[1:] if args[0] == "--" else args

# Pergunta (um pedaço do texto) -> resposta. "\r" = Enter (aceita o padrão).
respostas = [
    ("Nome do arquivo da imagem", "\r"),
    ("Incluir os apps Flatpak", "\r"),
    ("Tamanho do disco da VM", "\r"),
    ("Nome da máquina (hostname)", hostname + "\r"),
    ("Criar um usuário na imagem", "\r"),
    ("Nome de login", "maria\r"),
    ("Senha", "Senha123\r"),
    ("Repita a senha", "Senha123\r"),
    ("pode usar sudo", "\r"),
    ("Obrigar a trocar", "\r"),
    ("Chave SSH", chave + "\r"),
    ("Criar mais um usuário", "\r"),
    ("Bloquear o login do root", "\r"),
    ("Parar esses serviços", "\r"),  # só aparece se houver banco de dados ligado
    ("Compactar a imagem", "\r"),
    ("Começar?", "\r"),
]

pid, fd = pty.fork()
if pid == 0:
    os.execvp(comando[0], comando)

bruto, pos, cancelou, viu_cancelar = b"", 0, False, None
inicio = time.time()
with open(log, "wb") as arquivo:
    while time.time() - inicio < 3600:
        pronto, _, _ = select.select([fd], [], [], 0.5)
        if pronto:
            try:
                dado = os.read(fd, 65536)
            except OSError:
                break
            if not dado:
                break
            arquivo.write(dado)
            arquivo.flush()
            bruto += dado
        tela = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", bruto.decode("utf-8", "replace"))
        # Responde a primeira pergunta que apareceu depois da última respondida.
        achadas = [(tela.find(p, pos), p, r) for p, r in respostas if tela.find(p, pos) >= 0]
        if achadas:
            onde, pergunta, resposta = min(achadas)
            time.sleep(0.4)
            os.write(fd, resposta.encode())
            pos = onde + len(pergunta)
            respostas = [(p, r) for p, r in respostas if p != pergunta]
        if cancelar_em and not cancelou and cancelar_em in tela:
            viu_cancelar = viu_cancelar or time.time()
            if time.time() - viu_cancelar > 8:  # deixa o rsync trabalhar um pouco
                os.write(fd, b"\x03")  # Ctrl+C
                cancelou = True

_, status = os.waitpid(pid, 0)
codigo = os.waitstatus_to_exitcode(status)
print(f"robô: código de saída {codigo}" + (" (Ctrl+C enviado)" if cancelou else ""))
sys.exit(codigo)
