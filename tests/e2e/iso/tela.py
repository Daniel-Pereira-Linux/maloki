#!/usr/bin/env python3
# Opera a TELA de uma VM pelo QMP (o "painel de controle" do QEMU): tira print,
# clica com o mouse e digita. É assim que testamos o Calamares DE VERDADE,
# clicando nos botões como uma pessoa faria.
#
# A VM precisa ser ligada com:
#   -qmp unix:CAMINHO,server=on,wait=off   (o painel de controle)
#   -device usb-ehci -device usb-tablet    (mouse com posição absoluta)
#
# uso:
#   tela.py SOCKET print ARQUIVO.png        tira um print da tela
#   tela.py SOCKET clicar X Y               clique esquerdo em (X, Y), em pixels do print
#   tela.py SOCKET digitar "texto"          digita o texto (letras, números, espaço, símbolos comuns)
#   tela.py SOCKET tecla NOME [NOME...]     aperta teclas juntas (ex.: tecla ctrl alt t; tecla ret)
#   tela.py SOCKET tamanho                  mostra a resolução da tela
import json, os, socket, struct, sys, tempfile, time


def qmp(caminho, comandos):
    s = socket.socket(socket.AF_UNIX)
    s.connect(caminho)
    arquivo = s.makefile("rw")
    json.loads(arquivo.readline())  # cumprimento do QEMU
    respostas = []
    for comando in [{"execute": "qmp_capabilities"}] + comandos:
        arquivo.write(json.dumps(comando) + "\n")
        arquivo.flush()
        while True:
            resposta = json.loads(arquivo.readline())
            if "event" in resposta:
                continue
            if "error" in resposta:
                sys.exit(f"erro do QMP: {resposta['error']}")
            respostas.append(resposta.get("return"))
            break
    s.close()
    return respostas[1:]


def tamanho_do_png(caminho):
    with open(caminho, "rb") as f:
        cabecalho = f.read(24)
    return struct.unpack(">II", cabecalho[16:24])


def print_da_tela(caminho_qmp, destino):
    # O QEMU grava o print como o usuário que roda a VM; usamos um caminho absoluto.
    destino = os.path.abspath(destino)
    qmp(caminho_qmp, [{"execute": "screendump", "arguments": {"filename": destino, "format": "png"}}])
    return tamanho_do_png(destino)


def tamanho(caminho_qmp):
    with tempfile.TemporaryDirectory() as pasta:
        return print_da_tela(caminho_qmp, os.path.join(pasta, "t.png"))


def clicar(caminho_qmp, x, y):
    largura, altura = tamanho(caminho_qmp)
    # O usb-tablet usa coordenadas de 0 a 32767, não pixels.
    ax, ay = int(x * 32767 / (largura - 1)), int(y * 32767 / (altura - 1))
    mover = [{"type": "abs", "data": {"axis": "x", "value": ax}}, {"type": "abs", "data": {"axis": "y", "value": ay}}]
    botao = lambda apertado: [{"type": "btn", "data": {"down": apertado, "button": "left"}}]
    qmp(caminho_qmp, [
        {"execute": "input-send-event", "arguments": {"events": mover}},
    ])
    time.sleep(0.2)
    qmp(caminho_qmp, [{"execute": "input-send-event", "arguments": {"events": botao(True)}}])
    time.sleep(0.1)
    qmp(caminho_qmp, [{"execute": "input-send-event", "arguments": {"events": botao(False)}}])


# Caracteres -> teclas do QEMU (teclado americano, que é o padrão da VM).
ESPECIAIS = {
    " ": ["spc"], "-": ["minus"], "_": ["shift", "minus"], ".": ["dot"], ",": ["comma"],
    "/": ["slash"], "@": ["shift", "2"], "!": ["shift", "1"], "#": ["shift", "3"],
    "$": ["shift", "4"], "%": ["shift", "5"], "&": ["shift", "7"], "*": ["shift", "8"],
    "(": ["shift", "9"], ")": ["shift", "0"], "=": ["equal"], "+": ["shift", "equal"],
    ":": ["shift", "semicolon"], ";": ["semicolon"], "'": ["apostrophe"], '"': ["shift", "apostrophe"],
    "?": ["shift", "slash"], "\n": ["ret"],
}


def teclas_do_caractere(c):
    if c in ESPECIAIS:
        return ESPECIAIS[c]
    if c.isdigit() or ("a" <= c <= "z"):
        return [c]
    if "A" <= c <= "Z":
        return ["shift", c.lower()]
    sys.exit(f"não sei digitar {c!r}")


def apertar(caminho_qmp, teclas):
    lista = [{"type": "qcode", "data": t} for t in teclas]
    qmp(caminho_qmp, [{"execute": "send-key", "arguments": {"keys": lista, "hold-time": 60}}])


def digitar(caminho_qmp, texto):
    for c in texto:
        apertar(caminho_qmp, teclas_do_caractere(c))
        time.sleep(0.05)


if __name__ == "__main__":
    if len(sys.argv) < 3:
        sys.exit(__doc__ or "veja o comentário no topo do arquivo")
    caminho, acao, args = sys.argv[1], sys.argv[2], sys.argv[3:]
    if acao == "print":
        largura, altura = print_da_tela(caminho, args[0])
        print(f"{args[0]} ({largura}x{altura})")
    elif acao == "clicar":
        clicar(caminho, int(args[0]), int(args[1]))
    elif acao == "digitar":
        digitar(caminho, args[0])
    elif acao == "tecla":
        apertar(caminho, args)
    elif acao == "tamanho":
        print("%dx%d" % tamanho(caminho))
    else:
        sys.exit(f"ação desconhecida: {acao}")
