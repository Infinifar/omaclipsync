#!/usr/bin/env python3
"""Exercise keyring and 401 recovery with an isolated TLS server.

Uses fixture credentials and a fake clipboard watcher; the real keyring,
clipboard, production daemon, server and system network remain untouched.
"""
import base64
import http.server
import json
import os
from pathlib import Path
import queue
import signal
import ssl
import subprocess
import sys
import tempfile
import threading
import time


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: auth_recovery.py /absolute/path/to/syncclipd")
    binary = Path(sys.argv[1]).resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="omaclipsync-auth-") as tmp:
        root = Path(tmp)
        cert, key = root / "cert.pem", root / "key.pem"
        subprocess.run(
            ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
             "-keyout", str(key), "-out", str(cert), "-days", "1",
             "-subj", "/CN=localhost", "-addext", "subjectAltName=IP:127.0.0.1"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True,
        )
        auth = "Basic " + base64.b64encode(b"fixture:fixture-password").decode()
        state = {"reject": True, "attempts": 0, "bad_auth": 0}
        lock = threading.Lock()

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                with lock:
                    valid_auth = self.headers.get("Authorization") == auth
                    reject = state["reject"] or not valid_auth
                    state["attempts"] += 1
                    state["bad_auth"] += not valid_auth
                if reject:
                    body = b""
                elif self.path == "/":
                    body = b"ready"
                else:
                    body = b'{"type":"Image","hash":"","text":"","hasData":false}'
                self.send_response(401 if reject else 200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        server.socket = context.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()

        bindir = root / "bin"
        bindir.mkdir()
        secret = bindir / "secret-tool"
        secret.write_text(
            '#!/bin/sh\n[ -f "$AUTH_KEYRING_READY" ] || exit 1\n'
            'printf "fixture-password\\n"\n'
        )
        secret.chmod(0o700)
        paste = bindir / "wl-paste"
        paste.write_text('''#!/bin/sh
if [ "$1" = "--no-newline" ]; then printf initial; exit 0; fi
if [ "$1" = "--type" ] && [ "$3" = "--watch" ]; then
  printf initial | CLIPBOARD_STATE=data "$4" "$5"
  while :; do sleep 1; done
fi
exit 1
''')
        paste.chmod(0o700)
        config = root / "config/omaclipsync/config.json"
        config.parent.mkdir(parents=True)
        cfg = {
            "server": {"url": f"https://127.0.0.1:{server.server_port}",
                       "user": "fixture", "insecure": True},
            "sync": {"intervalSec": 30},
            "logging": {"level": "info"},
        }
        config.write_text(json.dumps(cfg))
        config.chmod(0o600)
        env = os.environ.copy()
        env.update({
            "PATH": str(bindir) + os.pathsep + env["PATH"],
            "XDG_CONFIG_HOME": str(root / "config"),
            "XDG_DATA_HOME": str(root / "data"),
            "XDG_STATE_HOME": str(root / "state"),
            "WAYLAND_DISPLAY": "omaclipsync-test-no-display",
            "AUTH_KEYRING_READY": str(root / "ready"),
        })
        env.pop("WAYLAND_SOCKET", None)
        daemon = subprocess.Popen(
            [str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, env=env, start_new_session=True,
        )
        messages = queue.Queue()

        def reader():
            for line in daemon.stdout:
                messages.put(json.loads(line))

        threading.Thread(target=reader, daemon=True).start()
        number = 0

        def status():
            nonlocal number
            number += 1
            request_id = number
            daemon.stdin.write(json.dumps({"id": request_id, "method": "status"}).encode() + b"\n")
            daemon.stdin.flush()
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                message = messages.get(timeout=max(0.01, deadline - time.monotonic()))
                if message.get("id") == request_id:
                    return message["result"]
            raise RuntimeError("status timed out")

        def wait_for(predicate, seconds):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                if daemon.poll() is not None:
                    raise RuntimeError("daemon exited")
                current = status()
                if predicate(current):
                    return current
                time.sleep(0.25)
            raise RuntimeError("expected state not reached")

        try:
            wait_for(lambda s: s["state"] == "credentials" and not s["watching"], 8)
            with lock:
                assert state["attempts"] == 0, "network contacted with unavailable keyring"
            print("unavailable keyring: credentials state, no network request: PASS", flush=True)

            (root / "ready").touch()
            cfg["logging"]["level"] = "debug"
            config.write_text(json.dumps(cfg))
            wait_for(lambda s: s["state"] == "credentials" and
                     s["configRevision"] >= 2 and s["watching"], 12)
            with lock:
                assert state["attempts"] >= 1
                state["reject"] = False
            print("HTTP 401: credentials state and retry: PASS", flush=True)

            wait_for(lambda s: s["state"] == "connected" and s["watching"], 45)
            with lock:
                assert state["bad_auth"] == 0, "fixture Basic authentication was not sent"
            print("recovery after server accepts credentials: PASS", flush=True)
            logs = b"".join(f.read_bytes() for f in (root / "state").rglob("*.jsonl"))
            assert b"fixture-password" not in logs
            print("credential log redaction: PASS", flush=True)
        finally:
            if daemon.poll() is None:
                try:
                    daemon.stdin.write(b'{"id":999999,"method":"shutdown"}\n')
                    daemon.stdin.flush()
                    daemon.wait(timeout=5)
                except (BrokenPipeError, subprocess.TimeoutExpired):
                    daemon.kill()
                    daemon.wait()
            try:
                os.killpg(daemon.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
