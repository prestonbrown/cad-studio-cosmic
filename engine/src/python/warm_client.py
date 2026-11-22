"""cad-studio warm build client (engine/src/python/warm.rs embeds me).

Runs model.py through the app's warm builder server when one is live,
falling back to a normal interpreter run when it is not — so the agent's
validation runs skip the build123d/matplotlib import tax (seconds per
run) whenever the app is up, and behave identically when it is not.

Usage: from the project root, `python <CAD_STUDIO_WARM_DIR>/cad_warm_model.py`.
Streams the run's stdout/stderr as they arrive and exits with its code.
"""
import os
import socket
import sys

SOCKET = os.path.join(os.environ.get("CAD_STUDIO_WARM_DIR", ""), "warm.sock")


def cold():
    # plain interpreter run: os.execv replaces this process, so the exit
    # code and output are exactly a direct `python model.py`
    os.execv(sys.executable, [sys.executable, "model.py"])


if not os.environ.get("CAD_STUDIO_WARM_DIR") or not os.path.exists(SOCKET):
    cold()

try:
    conn = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    conn.connect(SOCKET)
    conn.sendall(f"RUN {os.getcwd()}\n".encode())
except OSError:
    cold()

pending = {"O": b"", "E": b""}
exit_code = None
buffer = b""
while True:
    chunk = conn.recv(65536)
    if not chunk:
        break  # server died mid-build: report failure
    buffer += chunk
    while b"\n" in buffer:
        raw, buffer = buffer.split(b"\n", 1)
        tag, rest = raw[:1], raw[2:]
        if tag in (b"O", b"E"):
            stream = sys.stdout.buffer if tag == b"O" else sys.stderr.buffer
            stream.write(rest + b"\n")
            stream.flush()
        elif tag == b"P":
            pass  # the fork child's pid; signals and the registry are the
            # server's business here
        elif tag == b"X":
            status = int(rest.strip() or (1 << 8))
            if status & 0x7F:  # the child died to a signal: die the same way
                os.kill(os.getpid(), status & 0x7F)
            exit_code = (status >> 8) & 0xFF
            break
    if exit_code is not None:
        break
conn.close()
sys.exit(0 if exit_code is None else exit_code)
