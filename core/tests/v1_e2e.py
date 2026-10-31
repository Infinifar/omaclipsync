#!/usr/bin/env python3
"""Isolated real-Wayland V1 integration test; never uses the user's clipboard.

Run with XDG_RUNTIME_DIR and WAYLAND_DISPLAY pointing to a disposable compositor:
    RCS_TEST_DISPOSABLE_WAYLAND=1 python3 core/tests/v1_e2e.py /path/to/syncclipd /path/to/rustclipsync-server
"""
import base64
import contextlib
import http.client
import json
import os
from pathlib import Path
import queue
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PASSWORD = "integration-password-only"
DB_KEY = "2a" * 32


def available_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def until(check, message, timeout=30):
    end = time.monotonic() + timeout
    last_error = None
    while time.monotonic() < end:
        try:
            value = check()
            if value:
                return value
        except (OSError, ValueError, urllib.error.URLError, subprocess.TimeoutExpired) as error:
            last_error = type(error).__name__
        time.sleep(0.1)
    raise AssertionError(f"{message} (last error: {last_error})")


def run():
    if (len(sys.argv) != 3 or os.environ.get("RCS_TEST_DISPOSABLE_WAYLAND") != "1"
            or not os.environ.get("WAYLAND_DISPLAY") or not os.environ.get("XDG_RUNTIME_DIR")):
        raise SystemExit(__doc__)
    daemon_binary, server_binary = map(lambda x: str(Path(x).resolve()), sys.argv[1:])
    with tempfile.TemporaryDirectory(prefix="rcs-v1-e2e-") as directory:
        root = Path(directory)
        backend_port = available_port()
        proxy_port = available_port()
        env = os.environ.copy()
        env.update({"RCS_USER": "fixture", "RCS_PASSWORD": PASSWORD, "RCS_DB_KEY": DB_KEY,
                    "RCS_DATA_DIR": str(root / "server"), "RCS_LISTEN": f"127.0.0.1:{backend_port}"})
        server = None
        daemon = None
        proxy = None

        def start_server():
            process = subprocess.Popen([server_binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            until(lambda: urllib.request.urlopen(f"http://127.0.0.1:{backend_port}/healthz", timeout=1).status == 200,
                  "isolated service did not start")
            return process

        try:
            server = start_server()
            key, cert = root / "key.pem", root / "cert.pem"
            subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                            "-subj", "/CN=localhost", "-keyout", str(key), "-out", str(cert)],
                           check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

            class Proxy(BaseHTTPRequestHandler):
                protocol_version = "HTTP/1.1"

                def log_message(self, *args):
                    pass

                def handle_request(self):
                    connection = http.client.HTTPConnection("127.0.0.1", backend_port, timeout=32)
                    try:
                        length = int(self.headers.get("Content-Length", "0"))
                        if length > 10 * 1024 * 1024 + 4096:
                            self.send_error(413)
                            return
                        body = self.rfile.read(length) if length else None
                        forwarded = {k: v for k, v in self.headers.items() if k.lower() not in {"host", "connection"}}
                        connection.request(self.command, self.path, body=body, headers=forwarded)
                        response = connection.getresponse()
                        self.send_response(response.status)
                        for name, value in response.getheaders():
                            if name.lower() not in {"connection", "transfer-encoding", "server", "date"}:
                                self.send_header(name, value)
                        self.end_headers()
                        while chunk := response.read(65536):
                            self.wfile.write(chunk)
                    except (OSError, http.client.HTTPException):
                        with contextlib.suppress(OSError):
                            self.send_error(502)
                    finally:
                        connection.close()

                do_GET = do_POST = do_PUT = do_DELETE = handle_request

            proxy = ThreadingHTTPServer(("127.0.0.1", proxy_port), Proxy)
            proxy.daemon_threads = True
            tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            tls.load_cert_chain(cert, key)
            proxy.socket = tls.wrap_socket(proxy.socket, server_side=True)
            threading.Thread(target=proxy.serve_forever, daemon=True).start()
            base = f"https://127.0.0.1:{proxy_port}"
            tls_client = ssl._create_unverified_context()  # Disposable test certificate only.

            def api(method, path, authorization, body=None, headers=None):
                request_headers = {"Authorization": authorization, **(headers or {})}
                request = urllib.request.Request(base + path, data=body, headers=request_headers, method=method)
                with urllib.request.urlopen(request, timeout=12, context=tls_client) as response:
                    return response.status, response.read()

            basic = "Basic " + base64.b64encode(f"fixture:{PASSWORD}".encode()).decode()
            device = str(uuid.uuid4())
            token = "4b" * 32
            bearer = f"Bearer {device}:{token}"
            body = json.dumps({"deviceId": device, "deviceToken": token}).encode()
            _, result = api("POST", "/v1/devices", basic, body, {"Content-Type": "application/json"})
            registration = json.loads(result)
            epoch = registration["epoch"]
            assert registration["ackRevision"] == 0

            def state():
                return json.loads(api("GET", "/v1/state", bearer)[1])

            def content(message_id):
                return api("GET", f"/v1/messages/{message_id}/content", bearer)[1].decode()

            def send(text, revision, message_id=None, sensitive=False):
                message_id = message_id or str(uuid.uuid4())
                _, result = api("PUT", f"/v1/messages/{message_id}", bearer, text.encode(),
                                {"X-RCS-Epoch": epoch, "X-RCS-Base-Revision": str(revision),
                                 "X-RCS-Sensitive": "1" if sensitive else "0", "Content-Type": "text/plain; charset=utf-8"})
                return message_id, json.loads(result)

            bindir = root / "bin"
            bindir.mkdir()
            secret_tool = bindir / "secret-tool"
            secret_tool.write_text("""#!/usr/bin/python3
import pathlib, sys
account = sys.argv[-1]
path = pathlib.Path(__file__).parent / 'fixture-key'
if sys.argv[1] == 'lookup':
    if account == 'fixture': print('integration-password-only')
    elif path.exists(): print(path.read_text())
    else: sys.exit(1)
elif sys.argv[1] == 'store':
    path.write_text(sys.stdin.read().strip())
    path.chmod(0o600)
else: sys.exit(1)
""")
            secret_tool.chmod(0o700)
            config = root / "config/omaclipsync/config.json"
            config.parent.mkdir(parents=True)
            config.write_text(json.dumps({"server": {"url": base, "user": "fixture", "insecure": True, "protocol": "v1"},
                                          "sync": {"intervalSec": 30, "enabled": True}}))
            config.chmod(0o600)
            daemon_env = os.environ.copy()
            daemon_env.update({"PATH": str(bindir) + os.pathsep + daemon_env.get("PATH", ""),
                               "XDG_CONFIG_HOME": str(root / "config"), "XDG_DATA_HOME": str(root / "data"),
                               "XDG_STATE_HOME": str(root / "state")})

            def copy(text):
                subprocess.run(["wl-copy"], input=text.encode(), env=daemon_env, check=True, timeout=4)

            def paste():
                result = subprocess.run(["wl-paste", "--no-newline"], env=daemon_env,
                                        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=4)
                return result.stdout.decode() if result.returncode == 0 else None

            copy("initial clipboard is not an event")
            def start_daemon():
                process = subprocess.Popen([daemon_binary], env=daemon_env, stdin=subprocess.PIPE,
                                           stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1)
                results = queue.Queue()
                threading.Thread(target=lambda: [results.put(json.loads(line)) for line in process.stdout], daemon=True).start()
                return process, results

            daemon, messages = start_daemon()
            next_id = 0

            def rpc(method, params=None):
                nonlocal next_id
                next_id += 1
                daemon.stdin.write(json.dumps({"id": next_id, "method": method, "params": params or {}}) + "\n")
                daemon.stdin.flush()
                deadline = time.monotonic() + 2
                while time.monotonic() < deadline:
                    try:
                        message = messages.get(timeout=0.2)
                        if message.get("id") == next_id:
                            return message
                    except queue.Empty:
                        pass
                return {}

            def status():
                return rpc("status").get("result", {})

            until(lambda: status().get("watching") and status().get("state") == "connected", "desktop did not connect")
            assert state()["revision"] == 0, "initial selection was incorrectly uploaded"
            copy("fixture-A")
            until(lambda: state()["revision"] == 1, "desktop copy was not uploaded")
            assert content(state()["messageId"]) == "fixture-A"
            _, result = send("fixture-B", 1)
            assert result["revision"] == 2
            until(lambda: paste() == "fixture-B", "remote text was not applied")
            time.sleep(1)
            assert state()["revision"] == 2, "remote write was echoed back"
            subprocess.run(["wl-copy", "--clear"], env=daemon_env, check=True, timeout=4)
            time.sleep(2)
            assert paste() is None and state()["revision"] == 2, "cleared clipboard was restored or uploaded"
            message_id, result = send("fixture-A", 2)
            assert result["revision"] == 3
            until(lambda: paste() == "fixture-A", "A→B→A was incorrectly suppressed")
            _, duplicate = send("fixture-A", 2, message_id)
            assert duplicate["duplicate"] and duplicate["revision"] == 3
            subprocess.run(["wl-copy", "--clear"], env=daemon_env, check=True, timeout=4)
            send("fixture-A", 2, message_id)
            time.sleep(2)
            assert paste() is None and state()["revision"] == 3, "duplicate restored a cleared clipboard"

            daemon.terminate()
            daemon.wait(timeout=5)
            copy("local-after-restart")
            daemon, messages = start_daemon()
            until(lambda: status().get("watching") and status().get("state") == "connected", "desktop did not reconnect")
            time.sleep(2)
            assert paste() == "local-after-restart" and state()["revision"] == 3, "daemon restart replayed an old message"

            subprocess.run(["wl-copy", "--sensitive"], input=b"local-sensitive", env=daemon_env, check=True, timeout=4)
            time.sleep(2)
            assert state()["revision"] == 3, "marked sensitive local content was uploaded by default"
            subprocess.run(["wl-copy", "--clear"], env=daemon_env, check=True, timeout=4)

            _, secret_result = send("marked-sensitive", 3, sensitive=True)
            assert secret_result["revision"] == 4 and state()["sensitive"] is True
            desktop_id = status()["deviceId"]
            def desktop_ack():
                devices = json.loads(api("GET", "/v1/devices", basic)[1])
                return any(item["deviceId"] == desktop_id and item["ackRevision"] >= 4 for item in devices)
            until(desktop_ack, "sensitive skip was not acknowledged")
            assert paste() is None, "sensitive content was received despite the default policy"
            updated_config = json.loads(config.read_text())
            updated_config["sync"]["allowSensitive"] = True
            config.write_text(json.dumps(updated_config))
            until(lambda: status().get("watching") and status().get("state") == "connected", "desktop did not adopt sensitive policy")
            # Wait for the file watcher to apply the update, not just the previous status.
            time.sleep(2)
            _, secret_result = send("marked-sensitive-allowed", 4, sensitive=True)
            assert secret_result["revision"] == 5
            until(lambda: paste() == "marked-sensitive-allowed", "enabled sensitive receive failed")
            types = subprocess.run(["wl-paste", "--list-types"], env=daemon_env, check=True,
                                   stdout=subprocess.PIPE, timeout=4).stdout.decode().splitlines()
            assert "x-kde-passwordManagerHint" in types, "sensitive marker was lost on receive"
            subprocess.run(["wl-copy", "--sensitive"], input=b"local-sensitive-allowed", env=daemon_env, check=True, timeout=4)
            until(lambda: state()["revision"] == 6, "enabled sensitive upload failed")
            assert state()["sensitive"] is True, "sensitive marker was lost on upload"

            server.terminate()
            server.wait(timeout=5)
            server = None
            for text in ("offline-first", "offline-middle", "offline-latest"):
                copy(text)
                time.sleep(0.5)
            server = start_server()
            until(lambda: state()["revision"] >= 7, "latest offline copy was not uploaded", timeout=45)
            latest = state()
            assert latest["revision"] == 7 and content(latest["messageId"]) == "offline-latest", "offline copies did not collapse to latest"
            server.terminate()
            server.wait(timeout=5)
            server = None
            copy("stale-pending")
            time.sleep(2)
            daemon.terminate()
            daemon.wait(timeout=5)
            copy("changed-while-daemon-stopped")
            daemon, messages = start_daemon()
            until(lambda: status().get("watching"), "restarted desktop did not watch clipboard")
            time.sleep(1)
            server = start_server()
            until(lambda: status().get("state") == "connected", "desktop did not recover from outage", timeout=45)
            time.sleep(2)
            assert state()["revision"] == 7 and paste() == "changed-while-daemon-stopped", "stale outbox survived a changed clipboard"
            copy("fresh-after-restart")
            until(lambda: state()["revision"] == 8, "new copy after recovery was not uploaded", timeout=30)
            assert content(state()["messageId"]) == "fresh-after-restart"
            old_device = status()["deviceId"]
            assert api("DELETE", f"/v1/devices/{old_device}", basic)[0] == 204
            until(lambda: status().get("state") == "re_pair", "revocation was not detected", timeout=40)
            assert rpc("re_pair", {"confirm": False}).get("error") == "confirmation_required"
            assert rpc("re_pair", {"confirm": True}).get("ok") is True
            until(lambda: status().get("state") == "connected", "re-pairing failed")
            assert status()["deviceId"] != old_device
            time.sleep(1)
            assert state()["revision"] == 8 and paste() == "fresh-after-restart", "re-pairing replayed old clipboard"
            print("PASS: baseline, A→B→A, clear/restart replay, sensitive policy, offline latest, stale-outbox recovery and device re-pair")
        finally:
            if daemon is not None:
                daemon.terminate()
                with contextlib.suppress(subprocess.TimeoutExpired): daemon.wait(timeout=5)
                if daemon.poll() is None: daemon.kill()
            if proxy is not None:
                proxy.shutdown()
                proxy.server_close()
            if server is not None:
                server.terminate()
                with contextlib.suppress(subprocess.TimeoutExpired): server.wait(timeout=5)
                if server.poll() is None: server.kill()


if __name__ == "__main__":
    run()
