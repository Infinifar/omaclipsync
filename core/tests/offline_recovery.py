#!/usr/bin/env python3
"""Exercise 60 seconds of endpoint outage without changing system networking.

The installed/built daemon runs against an isolated local TLS endpoint and a
fixture clipboard watcher. No real clipboard, keyring, or server is accessed.
"""
import base64
import http.server
import json
import os
from pathlib import Path
import queue
import signal
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time

AUTH = 'Basic ' + base64.b64encode(b'fixture:fixture-password').decode()
VALUE = 'omaclipsync-offline-fixture'


def main():
    if len(sys.argv) != 2:
        raise SystemExit('usage: offline_recovery.py /absolute/path/to/syncclipd')
    binary = Path(sys.argv[1]).resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix='omaclipsync-offline-') as tmp:
        root = Path(tmp)
        key, cert = root / 'key.pem', root / 'cert.pem'
        subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes',
                        '-keyout', str(key), '-out', str(cert), '-days', '1',
                        '-subj', '/CN=localhost', '-addext', 'subjectAltName=IP:127.0.0.1'],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
        lock = threading.Lock()
        state = {'offline': False, 'profile': {'type': 'Image', 'hash': '', 'text': '', 'hasData': False}, 'uploads': []}

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def blocked(self):
                with lock:
                    offline = state['offline']
                if offline:
                    try:
                        self.connection.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass
                    self.connection.close()
                return offline

            def respond(self, status, body=b''):
                self.send_response(status)
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self):
                if self.blocked():
                    return
                if self.headers.get('Authorization') != AUTH:
                    return self.respond(401)
                with lock:
                    body = json.dumps(state['profile']).encode()
                if self.path == '/':
                    return self.respond(200, b'ready')
                if self.path == '/SyncClipboard.json':
                    return self.respond(200, body)
                return self.respond(404)

            def do_PUT(self):
                if self.blocked():
                    return
                if self.headers.get('Authorization') != AUTH:
                    return self.respond(401)
                if self.path != '/SyncClipboard.json':
                    return self.respond(404)
                item = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                with lock:
                    state['profile'] = item
                    state['uploads'].append(item)
                self.respond(200)

        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        server.socket = context.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        bindir = root / 'bin'
        bindir.mkdir()
        secret = bindir / 'secret-tool'
        secret.write_text('#!/bin/sh\nprintf "%s\\n" fixture-password\n')
        secret.chmod(0o700)
        paste = bindir / 'wl-paste'
        paste.write_text('''#!/bin/sh
if [ "$1" = "--no-newline" ]; then printf '%s' initial; exit 0; fi
if [ "$1" = "--type" ] && [ "$3" = "--watch" ]; then
  printf '%s' initial | CLIPBOARD_STATE=data "$4" "$5"
  while [ ! -f "$TEST_SIGNAL" ]; do sleep 0.2; done
  printf '%s' omaclipsync-offline-fixture | CLIPBOARD_STATE=data "$4" "$5"
  while :; do sleep 1; done
fi
exit 1
''')
        paste.chmod(0o700)
        config = root / 'config/omaclipsync/config.json'
        config.parent.mkdir(parents=True)
        config.write_text(json.dumps({'server': {'url': f'https://127.0.0.1:{server.server_port}',
                                                'user': 'fixture', 'insecure': True},
                                      'sync': {'intervalSec': 30}}))
        config.chmod(0o600)
        env = os.environ.copy()
        env.update({'PATH': str(bindir) + os.pathsep + env.get('PATH', ''),
                    'XDG_CONFIG_HOME': str(root / 'config'), 'XDG_DATA_HOME': str(root / 'data'),
                    'XDG_STATE_HOME': str(root / 'state'), 'WAYLAND_DISPLAY': 'omaclipsync-test-no-display',
                    'TEST_SIGNAL': str(root / 'copy-event')})
        env.pop('WAYLAND_SOCKET', None)
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
            daemon.stdin.write(json.dumps({'id': number, 'method': 'status'}).encode() + b'\n')
            daemon.stdin.flush()
            end = time.monotonic() + 5
            while time.monotonic() < end:
                message = messages.get(timeout=max(0.01, end-time.monotonic()))
                if message.get('id') == number:
                    return message['result']
            raise RuntimeError('status timed out')
        def wait_for(predicate, timeout):
            end = time.monotonic() + timeout
            while time.monotonic() < end:
                if daemon.poll() is not None:
                    raise RuntimeError('daemon exited')
                current = status()
                if predicate(current):
                    return current
                time.sleep(0.5)
            raise RuntimeError('expected daemon state not reached')
        try:
            wait_for(lambda s: s['state'] == 'connected' and s['watching'], 15)
            with lock:
                state['offline'] = True
            start = time.monotonic()
            # With the hub unavailable, the daemon intentionally polls at most
            # every 30 seconds, so outage detection need not be instantaneous.
            wait_for(lambda s: s['state'] == 'offline', 40)
            (root / 'copy-event').touch()
            print('isolated outage: offline detected; clipboard event queued', flush=True)
            while time.monotonic() - start < 60:
                if daemon.poll() is not None:
                    raise RuntimeError('daemon exited during outage')
                time.sleep(0.5)
            with lock:
                assert not state['uploads'], 'upload occurred during outage'
                state['offline'] = False
            recovered_at = time.monotonic()
            wait_for(lambda s: s['state'] == 'connected', 40)
            with lock:
                assert len(state['uploads']) == 1, f"unexpected uploads: {len(state['uploads'])}"
                assert state['uploads'][0]['text'] == VALUE
            print(f'PASS: outage={recovered_at-start:.1f}s recovery={time.monotonic()-recovered_at:.1f}s; queued copy uploaded once', flush=True)
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


if __name__ == '__main__':
    main()
