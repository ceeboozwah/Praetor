#!/usr/bin/env python3
"""Minimal `ourc lsp` client for the N43 task.

    python3 lsp_client.py diag  <file.lang>
    python3 lsp_client.py hover <file.lang> <line> <col>
    python3 lsp_client.py def   <file.lang> <line> <col>
    python3 lsp_client.py comp  <file.lang> <line> <col>

`line`/`col` are 0-based, the same convention LSP editors use.
Find `ourc` on PATH, else at target/release/ourc inside the repo.
"""
import json
import os
import select
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
OURC = os.environ.get("OURC", os.path.join(REPO, "target", "release", "ourc"))


def frame(msg):
    b = json.dumps(msg).encode()
    return b"Content-Length: %d\r\n\r\n" % len(b) + b


def recv(p, timeout=5.0):
    fd = p.stdout.fileno()
    hdr = b""
    while b"\r\n\r\n" not in hdr:
        if not select.select([fd], [], [], timeout)[0]:
            return None
        c = os.read(fd, 1)
        if not c:
            return None
        hdr += c
    n = next((int(l.split(b":", 1)[1]) for l in hdr.split(b"\r\n")
              if l.lower().startswith(b"content-length")), None)
    if n is None:
        return None
    body = b""
    while len(body) < n:
        if not select.select([fd], [], [], timeout)[0]:
            return None
        body += os.read(fd, n - len(body))
    return json.loads(body)


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    cmd, path = sys.argv[1], sys.argv[2]
    lno = int(sys.argv[3]) if len(sys.argv) > 3 else 0
    col = int(sys.argv[4]) if len(sys.argv) > 4 else 0

    p = subprocess.Popen([OURC, "lsp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    send = lambda m: (p.stdin.write(frame(m)), p.stdin.flush())
    uri = "file://" + os.path.abspath(path)
    text = open(path).read()

    send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "rootUri": "file://" + os.path.dirname(os.path.abspath(path)),
        "capabilities": {}}})
    recv(p)
    send({"jsonrpc": "2.0", "method": "initialized", "params": {}})
    send({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
        "textDocument": {"uri": uri, "languageId": "lang", "version": 1,
                         "text": text}}})

    if cmd == "diag":
        found = False
        for _ in range(10):
            m = recv(p)
            if m is None:
                break
            if m.get("method") == "textDocument/publishDiagnostics":
                print(json.dumps(m["params"], indent=2))
                found = True
                break
        if not found:
            print("[]  # no diagnostics published (clean file)")
    else:
        method = {"hover": "textDocument/hover",
                  "def": "textDocument/definition",
                  "comp": "textDocument/completion"}[cmd]
        send({"jsonrpc": "2.0", "id": 2, "method": method, "params": {
            "textDocument": {"uri": uri},
            "position": {"line": lno, "character": col}}})
        while True:
            m = recv(p)
            if m is None or m.get("id") == 2:
                print(json.dumps(m.get("result") if m else None, indent=2))
                break

    send({"jsonrpc": "2.0", "method": "exit"})
    p.wait(timeout=5)


if __name__ == "__main__":
    main()
