"""cad-studio warm builder server (engine/src/python/warm.rs embeds me).

Keeps build123d and matplotlib imported once, then forks per build so the
child inherits the warmed modules copy-on-write and model.py starts paying
only geometry, not imports.

Protocol (all records newline-terminated text on one stream socket):
  app  -> "RUN <project_root>\n"
  srv  -> "P <child pid>\n"
  srv  -> "O <line>\n"  for every model.py stdout line
  srv  -> "E <line>\n"  for every model.py stderr line
  srv  -> "X <waitpid status>\n"   (the raw 16-bit wait status)

argv: warm_server.py <socket path> [module ...] — the app passes build123d
and matplotlib; tests pass nothing (a bare interpreter still speaks the
protocol, so the fork/pipe machinery is testable without the venv).
"""
import os
import runpy
import signal
import socket
import sys
import threading

socket_path = sys.argv[1]

for name in sys.argv[2:]:
    __import__(name)
if "matplotlib" in sys.argv[2:]:
    import matplotlib
    matplotlib.use("Agg")

print("READY", flush=True)

srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
srv.bind(socket_path)
srv.listen(1)


def pump(conn, lock, fd, tag):
    """Forward one child pipe's lines to the socket, each tagged."""
    with os.fdopen(fd, "rb", closefd=True) as pipe:
        for chunk in pipe:  # buffered iteration yields newline-ended lines
            record = tag + b" " + chunk
            if not record.endswith(b"\n"):
                record += b"\n"  # a final line without its newline
            with lock:
                conn.sendall(record)


while True:
    try:
        conn, _ = srv.accept()
    except OSError:
        break  # socket torn down
    with conn:
        request = b""
        while not request.endswith(b"\n"):
            chunk = conn.recv(4096)
            if not chunk:
                request = None
                break
            request += chunk
        if not request or not request.startswith(b"RUN "):
            continue
        root = request[4:].strip().decode()

        out_read, out_write = os.pipe()
        err_read, err_write = os.pipe()
        pid = os.fork()
        if pid == 0:
            # child: run model.py fresh, writing to the pipes the parent
            # pumps. Never returns to the accept loop.
            os.close(out_read)
            os.close(err_read)
            srv.close()
            conn.close()
            os.dup2(out_write, 1)
            os.dup2(err_write, 2)
            # an ignored disposition would survive from the app; the build
            # cancel path needs default signal behavior (processes.rs rule)
            for sig in (signal.SIGINT, signal.SIGQUIT, signal.SIGTERM):
                signal.signal(sig, signal.SIG_DFL)
            code = 1
            try:
                os.chdir(root)
                sys.argv = ["model.py"]
                runpy.run_path("model.py", run_name="__main__")
                code = 0
            except SystemExit as exit_exc:
                code = exit_exc.code if isinstance(exit_exc.code, int) else 0
            except BaseException:
                import traceback
                traceback.print_exc()
            finally:
                sys.stdout.flush()
                sys.stderr.flush()
            os._exit(code)

        os.close(out_write)
        os.close(err_write)
        conn.sendall(b"P %d\n" % pid)
        lock = threading.Lock()
        readers = [
            threading.Thread(target=pump, args=(conn, lock, out_read, b"O")),
            threading.Thread(target=pump, args=(conn, lock, err_read, b"E")),
        ]
        for reader in readers:
            reader.start()
        for reader in readers:
            reader.join()
        _, status = os.waitpid(pid, 0)
        conn.sendall(b"X %d\n" % status)
