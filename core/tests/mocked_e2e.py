#!/usr/bin/env python3
"""End-to-end daemon smoke test with synthetic data and an isolated TLS server.

Requires openssl and a built syncclipd. Does not access the real clipboard,
keyring, desktop configuration, or user server.
"""
import base64
import hashlib
import http.server
import json
import os
from pathlib import Path
import selectors
import ssl
import subprocess
import sys
import tempfile
import threading
import time

INITIAL = "fixture-initial"
LOCAL = "fixture-local"
REMOTE = "fixture-remote"
AUTH = "Basic " + base64.b64encode(b"fixture:fixture-password").decode()


def profile(text):
    return {"type": "Text", "hash": hashlib.sha256(text.encode()).hexdigest().upper(),
            "text": text, "hasData": False}


def main():
    if len(sys.argv) not in (2, 3) or (len(sys.argv) == 3 and sys.argv[2] != "--blacklist"):
        raise SystemExit("usage: mocked_e2e.py /absolute/path/to/syncclipd [--blacklist]")
    binary = Path(sys.argv[1]).resolve(strict=True)
    blacklist = len(sys.argv) == 3
    with tempfile.TemporaryDirectory(prefix="omaclipsync-e2e-") as tmp:
        root = Path(tmp)
        key, cert = root / "key.pem", root / "cert.pem"
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                        "-keyout", str(key), "-out", str(cert), "-days", "1",
                        "-subj", "/CN=localhost", "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
        state = {"profile": profile(REMOTE), "uploads": [], "unauthorized": 0}
        lock = threading.Lock()

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def respond(self, status, data=b"", content_type="application/json"):
                self.send_response(status)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def authorized(self):
                if self.headers.get("Authorization") != AUTH:
                    with lock:
                        state["unauthorized"] += 1
                    self.respond(401)
                    return False
                return True

            def do_GET(self):
                if not self.authorized():
                    return
                if self.path == "/":
                    return self.respond(200, b"Server is running.", "text/plain")
                if self.path == "/SyncClipboard.json":
                    with lock:
                        data = json.dumps(state["profile"]).encode()
                    return self.respond(200, data)
                return self.respond(404)

            def do_PUT(self):
                if not self.authorized():
                    return
                body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                if self.path != "/SyncClipboard.json":
                    return self.respond(404)
                item = json.loads(body)
                with lock:
                    state["uploads"].append(item)
                    state["profile"] = item
                self.respond(200)

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        server.socket = context.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()

        bindir = root / "bin"
        bindir.mkdir()
        secret = bindir / "secret-tool"
        secret.write_text("#!/bin/sh\nprintf '%s\\n' fixture-password\n")
        secret.chmod(0o700)
        paste = bindir / "wl-paste"
        paste.write_text("""#!/bin/sh
if [ "$1" = "--no-newline" ]; then printf '%s' fixture-initial; exit 0; fi
if [ "$1" = "--type" ] && [ "$3" = "--watch" ]; then
  printf '%s' fixture-initial | CLIPBOARD_STATE=data "$4" "$5"
  sleep 0.2
  printf '%s' fixture-blocked | CLIPBOARD_STATE=data "$4" "$5"
  sleep 0.2
  printf '%s' fixture-local | CLIPBOARD_STATE=data "$4" "$5"
  sleep 8
  exit 0
fi
exit 1
""")
        paste.chmod(0o700)
        config = root / "config/omaclipsync/config.json"
        config.parent.mkdir(parents=True)
        config.write_text(json.dumps({
            "server": {"url": f"https://127.0.0.1:{server.server_port}", "user": "fixture", "insecure": True},
            "filter": {"mode": "blacklist" if blacklist else "whitelist",
                       "patterns": ["^fixture-(blocked|remote)$"] if blacklist else ["^fixture-local$"],
                       "alsoBlockDownloads": True},
        }))
        config.chmod(0o600)
        env = os.environ.copy()
        env.update({"PATH": str(bindir) + os.pathsep + env.get("PATH", ""),
                    "XDG_CONFIG_HOME": str(root / "config"), "XDG_DATA_HOME": str(root / "data"),
                    "XDG_STATE_HOME": str(root / "state"), "WAYLAND_DISPLAY": "omaclipsync-test-no-display"})
        env.pop("WAYLAND_SOCKET", None)
        daemon = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, env=env)
        messages = []
        try:
            daemon.stdin.write(b'{"id":1,"method":"hello"}\n')
            daemon.stdin.flush()
            selector = selectors.DefaultSelector()
            selector.register(daemon.stdout, selectors.EVENT_READ)
            buffer = b""
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline and daemon.poll() is None:
                for key_obj, _ in selector.select(0.25):
                    chunk = os.read(key_obj.fd, 65536)
                    if not chunk:
                        break
                    buffer += chunk
                    while b"\n" in buffer:
                        line, buffer = buffer.split(b"\n", 1)
                        messages.append(json.loads(line))
                with lock:
                    uploaded = len(state["uploads"])
                if uploaded:
                    break
            with lock:
                uploads = list(state["uploads"])
                unauthorized = state["unauthorized"]
            assert any(m.get("id") == 1 and m.get("result", {}).get("protocol") == 1 for m in messages)
            assert len(uploads) == 1, f"unexpected upload count: {len(uploads)}"
            assert uploads[0] == profile(LOCAL) | {"size": len(LOCAL)}
            assert unauthorized == 0
            daemon.stdin.write(b'{"id":2,"method":"toggle"}\n')
            daemon.stdin.flush()
            paused = False
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline and daemon.poll() is None and not paused:
                for key_obj, _ in selector.select(0.25):
                    chunk = os.read(key_obj.fd, 65536)
                    if not chunk:
                        break
                    buffer += chunk
                    while b"\n" in buffer:
                        line, buffer = buffer.split(b"\n", 1)
                        message = json.loads(line)
                        messages.append(message)
                        if message.get("id") == 2 and message.get("result", {}).get("enabled") is False:
                            paused = True
            assert paused, "pause command was not applied"
            assert any(m.get("event") == "activity" and m.get("data", {}).get("active") is True for m in messages)
            assert any(m.get("event") == "activity" and m.get("data", {}).get("active") is False for m in messages)
            with lock:
                assert len(state["uploads"]) == 1
            daemon.stdin.write(b'{"id":3,"method":"shutdown"}\n')
            daemon.stdin.flush()
            assert daemon.wait(timeout=8) == 0
            logs = b"".join(p.read_bytes() for p in (root / "state").rglob("*.jsonl"))
            assert LOCAL.encode() not in logs and b"fixture-blocked" not in logs and b"fixture-password" not in logs
            assert b'"filtered"' in logs
            print(f"mocked TLS + fallback watcher + {'blacklist' if blacklist else 'whitelist'} + upload + pause + redacted logs: PASS")
        finally:
            if daemon.poll() is None:
                daemon.kill()
                daemon.wait()
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
