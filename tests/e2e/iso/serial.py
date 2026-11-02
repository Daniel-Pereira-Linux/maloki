#!/usr/bin/env python3
# Dirige a sessão live da ISO pela porta SERIAL da VM (não precisa de rede
# nem de tela): escolhe no menu do GRUB a opção "console serial", entra como
# liveuser e roda os testes do "CD de testes".
#
# uso: serial.py SOCKET LOG MODO
import re, socket, sys, time

caminho, log, modo = sys.argv[1:4]
s = None
for _ in range(60):  # o QEMU demora um pouco para criar o socket
    tentativa = socket.socket(socket.AF_UNIX)
    try:
        tentativa.connect(caminho)
        s = tentativa
        break
    except OSError:
        tentativa.close()
        time.sleep(1)
if s is None:
    sys.exit("não conectei na serial")
s.setblocking(False)

tela = ""
arquivo = open(log, "w", encoding="utf-8", errors="replace")

def ler(segundos=0.5):
    global tela
    fim = time.time() + segundos
    while time.time() < fim:
        try:
            dado = s.recv(65536)
            if not dado:
                return
            texto = dado.decode("utf-8", "replace")
            arquivo.write(texto); arquivo.flush()
            tela += texto
        except BlockingIOError:
            time.sleep(0.1)

def esperar(padrao, limite, desde=0):
    fim = time.time() + limite
    while time.time() < fim:
        ler()
        limpo = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", tela[desde:])
        m = re.search(padrao, limpo)
        if m:
            return m
    sys.exit(f"tempo esgotado esperando {padrao!r}")

def enviar(texto):
    s.sendall(texto.encode())

# 1. Menu do GRUB: 2x seta para baixo = "console serial"
esperar(r"console serial", 180)
time.sleep(1)
enviar("\x1b[B"); time.sleep(0.4); enviar("\x1b[B"); time.sleep(0.4); enviar("\r")
inicio = len(tela)

# 2. Login na serial
esperar(r"login:\s*$", 900, inicio)
time.sleep(2)
enviar("liveuser\r")
esperar(r"[Pp]assword:\s*$|[Ss]enha:\s*$", 60, inicio)
time.sleep(1)
enviar("live\r")
time.sleep(8)
ler(2)

# 3. Testes: o "CD de testes" tem o rótulo MALOKITESTE
inicio = len(tela)
enviar("stty cols 200; sudo mkdir -p /run/teste && sudo mount -o ro LABEL=MALOKITESTE /run/teste "
       f"&& sudo sh /run/teste/rodar-no-live.sh {modo}; echo FIM-$?\r")
esperar(r"FIM-(\d+)", 2400, inicio)
enviar("sudo systemctl poweroff\r")
ler(5)
print("serial: terminou")
