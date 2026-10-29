#!/usr/bin/env python3
"""Exercise SignalR and polling fallback using a private fixture TLS endpoint.

Requires only Python's standard library, openssl and a built syncclipd.  The
real clipboard, keyring, desktop config, production server and network are not
used.  Remote fixture text is blocked by a download filter, so the test can
exercise daemon scheduling without requiring a Wayland compositor.
"""
import base64
import hashlib
import http.server
import json
import os
from pathlib import Path
import queue
import signal
import socket
import ssl
import struct
import subprocess
import sys
import tempfile
import threading
import time


AUTH = "Basic " + base64.b64encode(b"fixture:fixture-password").decode()
RS = b"\x1e"


def profile(text):
    return {"type": "Text", "hash": hashlib.sha256(text.encode()).hexdigest().upper(),
            "text": text, "hasData": False}


def read_exact(stream, size):
    data = b""
    while len(data) < size:
        chunk = stream.recv(size - len(data))
        if not chunk:
            raise EOFError("websocket closed")
        data += chunk
    return data


def read_frame(stream):
    first, second = read_exact(stream, 2)
    size = second & 127
    if size == 126:
        size = struct.unpack("!H", read_exact(stream, 2))[0]
    elif size == 127:
        size = struct.unpack("!Q", read_exact(stream, 8))[0]
    if size > 1024 * 1024:
        raise ValueError("fixture received an oversize frame")
    mask = read_exact(stream, 4) if second & 128 else None
    data = read_exact(stream, size)
    if mask:
        data = bytes(byte ^ mask[i % 4] for i, byte in enumerate(data))
    return first & 15, data


def write_frame(stream, payload, opcode=1):
    size = len(payload)
    header = bytes([0x80 | opcode])
    if size < 126:
        header += bytes([size])
    elif size <= 65535:
        header += bytes([126]) + struct.pack("!H", size)
    else:
        header += bytes([127]) + struct.pack("!Q", size)
    stream.sendall(header + payload)


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: signalr_e2e.py /absolute/path/to/syncclipd")
    binary = Path(sys.argv[1]).resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="omaclipsync-signalr-") as tmp:
        root = Path(tmp)
        cert, key = root / "cert.pem", root / "key.pem"
        subprocess.run(
            ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
             "-keyout", str(key), "-out", str(cert), "-days", "1",
             "-subj", "/CN=localhost", "-addext", "subjectAltName=IP:127.0.0.1"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True,
        )
        lock = threading.RLock()
        send_lock = threading.Lock()
        state = {"allow_ws": True, "profile": profile("fixture-initial-remote"),
                 "polls": [], "handshakes": [], "socket": None, "bad_auth": 0}

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args):
                pass

            def respond(self, status, body=b"", content_type="application/json"):
                self.send_response(status)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def authorized(self):
                if self.headers.get("Authorization") == AUTH:
                    return True
                with lock:
                    state["bad_auth"] += 1
                self.respond(401)
                return False

            def do_POST(self):
                if not self.authorized():
                    return
                if self.path != "/SyncClipboardHub/negotiate?negotiateVersion=1":
                    return self.respond(404)
                with lock:
                    allow = state["allow_ws"]
                if not allow:
                    return self.respond(503)
                body = json.dumps({"negotiateVersion": 1, "connectionId": "fixture-id",
                                   "connectionToken": "fixture-token",
                                   "availableTransports": [{"transport": "WebSockets",
                                                            "transferFormats": ["Text", "Binary"]}]}).encode()
                self.respond(200, body)

            def do_GET(self):
                if not self.authorized():
                    return
                if self.path == "/":
                    return self.respond(200, b"ready", "text/plain")
                if self.path == "/SyncClipboard.json":
                    with lock:
                        current = state["profile"]
                        state["polls"].append((time.monotonic(), current["hash"]))
                        data = json.dumps(current).encode()
                    return self.respond(200, data)
                if self.path != "/SyncClipboardHub?id=fixture-token":
                    return self.respond(404)
                with lock:
                    allow = state["allow_ws"]
                if not allow:
                    return self.respond(503)
                key_value = self.headers.get("Sec-WebSocket-Key", "")
                accept = base64.b64encode(hashlib.sha1(
                    (key_value + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()
                ).digest()).decode()
                self.send_response(101)
                self.send_header("Upgrade", "websocket")
                self.send_header("Connection", "Upgrade")
                self.send_header("Sec-WebSocket-Accept", accept)
                self.end_headers()
                try:
                    opcode, data = read_frame(self.connection)
                    assert opcode == 1 and json.loads(data.rstrip(RS)) == {"protocol": "json", "version": 1}
                    handshake_time = time.monotonic()
                    with send_lock:
                        write_frame(self.connection, b"{}" + RS)
                    with lock:
                        state["socket"] = self.connection
                        state["handshakes"].append(handshake_time)
                    self.connection.settimeout(3)
                    last_ping = time.monotonic()
                    while True:
                        if time.monotonic() - last_ping >= 10:
                            with send_lock:
                                write_frame(self.connection, b'{"type":6}' + RS)
                            last_ping = time.monotonic()
                        try:
                            opcode, payload = read_frame(self.connection)
                        except socket.timeout:
                            continue
                        if opcode == 8:
                            break
                        if opcode == 9:
                            with send_lock:
                                write_frame(self.connection, payload, opcode=10)
                except (OSError, EOFError, ValueError, AssertionError):
                    pass
                finally:
                    with lock:
                        if state["socket"] is self.connection:
                            state["socket"] = None
                    self.close_connection = True

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
        config.write_text(json.dumps({
            "server": {"url": f"https://127.0.0.1:{server.server_port}", "user": "fixture", "insecure": True},
            "sync": {"intervalSec": 1},
            "filter": {"mode": "blacklist", "patterns": ["^fixture-"], "alsoBlockDownloads": True},
            "logging": {"level": "debug"},
        }))
        config.chmod(0o600)
        env = os.environ.copy()
        env.update({"PATH": str(bindir) + os.pathsep + env.get("PATH", ""),
                    "XDG_CONFIG_HOME": str(root / "config"), "XDG_DATA_HOME": str(root / "data"),
                    "XDG_STATE_HOME": str(root / "state"), "WAYLAND_DISPLAY": "no-such-display"})
        env.pop("WAYLAND_SOCKET", None)
        daemon = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, env=env, start_new_session=True)
        messages = queue.Queue()

        def reader():
            for line in daemon.stdout:
                messages.put(json.loads(line))

        threading.Thread(target=reader, daemon=True).start()
        number = 0

        def status():
            nonlocal number
            number += 1
            daemon.stdin.write(json.dumps({"id": number, "method": "status"}).encode() + b"\n")
            daemon.stdin.flush()
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                message = messages.get(timeout=max(0.01, deadline - time.monotonic()))
                if message.get("id") == number:
                    return message["result"]
            raise RuntimeError("status timed out")

        def wait_for(predicate, timeout, what):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if daemon.poll() is not None:
                    raise RuntimeError("daemon exited")
                if predicate():
                    return
                time.sleep(0.1)
            raise RuntimeError(f"timed out waiting for {what}")

        def count_polls():
            with lock:
                return len(state["polls"])

        def send(message):
            with lock:
                peer = state["socket"]
            assert peer is not None, "no WebSocket available"
            with send_lock:
                write_frame(peer, json.dumps(message).encode() + RS)

        try:
            wait_for(lambda: len(state["handshakes"]) == 1, 20, "initial handshake")
            wait_for(lambda: count_polls() >= 1, 10, "initial pull")
            wait_for(lambda: status().get("transport") == "signalr", 10, "signalr status")
            print("SignalR negotiation and handshake: PASS", flush=True)
            time.sleep(2)
            before = count_polls()
            send({"type": 1, "target": "UnrelatedEvent", "arguments": []})
            time.sleep(4)
            assert count_polls() == before, "unknown event or connected idle caused a pull"
            with lock:
                state["profile"] = profile("fixture-notified")
            send({"type": 1, "target": "RemoteProfileChanged", "arguments": [{}]})
            wait_for(lambda: count_polls() > before, 8, "notification-triggered pull")
            with lock:
                assert state["polls"][-1][1] == profile("fixture-notified")["hash"]
            before_flood = count_polls()
            for i in range(30):
                if i == 29:
                    with lock:
                        state["profile"] = profile("fixture-after-flood")
                send({"type": 1, "target": "RemoteProfileChanged", "arguments": [{}]})
            time.sleep(2)
            flood_pulls = count_polls() - before_flood
            assert 1 <= flood_pulls <= 5, f"notification flood caused {flood_pulls} pulls"
            with lock:
                assert any(h == profile("fixture-after-flood")["hash"]
                           for _, h in state["polls"][before_flood:]), "latest notification lost"
                state["allow_ws"] = False
                old_socket = state["socket"]
            print("notification delivery and flood coalescing: PASS", flush=True)
            assert old_socket is not None
            old_socket.shutdown(socket.SHUT_RDWR)
            wait_for(lambda: status().get("transport") == "polling", 12, "fallback state")
            before_fallback = count_polls()
            time.sleep(35)
            with lock:
                fallback = state["polls"][before_fallback:]
            assert 1 <= len(fallback) <= 2, f"unexpected fallback pull count: {len(fallback)}"
            if len(fallback) == 2:
                assert fallback[1][0] - fallback[0][0] >= 29, "fallback repeated too frequently"
            print("low-frequency fallback for legacy 1s interval: PASS", flush=True)
            with lock:
                state["profile"] = profile("fixture-during-disconnect")
                state["allow_ws"] = True
            wait_for(lambda: len(state["handshakes"]) >= 2, 75, "reconnection")
            with lock:
                reconnected = state["handshakes"][-1]
            wait_for(lambda: any(t >= reconnected and h == profile("fixture-during-disconnect")["hash"]
                                 for t, h in state["polls"]), 10, "reconnect catch-up")
            assert status().get("transport") == "signalr"
            print("reconnect and latest-state catch-up: PASS", flush=True)
            with lock:
                oversized_peer = state["socket"]
            assert oversized_peer is not None
            with send_lock:
                write_frame(oversized_peer, b'x' * (300 * 1024))
            wait_for(lambda: state["socket"] is not oversized_peer, 12, "oversize frame rejection")
            with lock:
                assert state["bad_auth"] == 0
            logs = b"".join(p.read_bytes() for p in (root / "state").rglob("*.jsonl"))
            assert b"fixture-password" not in logs and b"fixture-notified" not in logs
            assert b'"filtered"' in logs
            print("PASS: SignalR handshake, ignored unknown event, notification pull and flood coalescing, low-frequency fallback, reconnect catch-up, oversize frame rejection, authentication and redacted logs")
        finally:
            if daemon.poll() is None:
                try:
                    daemon.stdin.write(b'{"id":999999,"method":"shutdown"}\n')
                    daemon.stdin.flush()
                    daemon.wait(timeout=8)
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
