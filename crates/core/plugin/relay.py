"""Lets an agent inside the isolated distribution talk to the RoVibe app,
and to the few internet hosts it is allowed.

The agent posts to 127.0.0.1 as it would on Windows. This process, started
by the app, hands each request to the app over its own stdin/stdout and
returns the answer. Nothing listens on a network interface, so neither the
Windows firewall nor other machines are involved.

It also runs the only way out to the internet the agent's user has once the
distribution's firewall is closed: a proxy that opens HTTPS tunnels to the
hosts listed in a file only root can write, and to nothing else.
"""
import http.server
import itertools
import json
import os
import select
import socket
import socketserver
import sys
import threading

port = int(sys.argv[1])
proxy_port = int(sys.argv[2])
hosts_file = sys.argv[3]
pending = {}
counter = itertools.count(1)
writing = threading.Lock()
refused = set()


def tell_app(message):
    with writing:
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()


def read_replies():
    for line in sys.stdin:
        try:
            reply = json.loads(line)
            done, box = pending.pop(reply["id"])
        except (ValueError, KeyError):
            continue
        box.append(reply)
        done.set()
    # The app is gone: there is nobody left to answer.
    os._exit(0)


class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length).decode("utf-8", "replace")

        request_id = next(counter)
        done, box = threading.Event(), []
        pending[request_id] = (done, box)
        tell_app({"id": request_id, "path": self.path, "body": body})

        # Longer than the slowest tool the app offers.
        if not done.wait(660):
            pending.pop(request_id, None)
            self.send_error(504)
            return

        data = box[0]["body"].encode("utf-8")
        self.send_response(box[0]["status"])
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


def allowed(host):
    """Whether `host` is on the list: an exact name, or `*.domain` for any
    name under a domain. Read on every connection, so a change of the app's
    settings applies to agents that are already running."""
    try:
        with open(hosts_file, encoding="utf-8") as file:
            patterns = file.read().split()
    except OSError:
        return False
    host = host.lower().rstrip(".")
    for pattern in patterns:
        if pattern.startswith("*."):
            if host.endswith(pattern[1:]):
                return True
        elif host == pattern:
            return True
    return False


class Tunnel(socketserver.BaseRequestHandler):
    def refuse(self, host):
        reason = f"RoVibe : {host} n'est pas dans les hôtes autorisés en mode isolé.\n".encode()
        self.request.sendall(
            b"HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain; charset=utf-8\r\n"
            + f"Content-Length: {len(reason)}\r\nConnection: close\r\n\r\n".encode()
            + reason
        )
        # Said once per host: the user learns what an agent tried to reach
        # without the journal filling up with retries.
        if host not in refused and len(refused) < 200:
            refused.add(host)
            tell_app({"refused": host})

    def handle(self):
        self.request.settimeout(15)
        head = b""
        while b"\r\n\r\n" not in head and len(head) < 16384:
            chunk = self.request.recv(4096)
            if not chunk:
                return
            head += chunk
        words = head.split(b"\r\n", 1)[0].decode("latin-1").split()
        if len(words) < 2:
            return
        method, target = words[0].upper(), words[1]

        # Only HTTPS tunnels: a plain request would let any content through
        # a host that merely redirects.
        host, _, number = target.rpartition(":")
        if method != "CONNECT" or number != "443" or not allowed(host):
            self.refuse(host if method == "CONNECT" else target[:120])
            return

        try:
            remote = socket.create_connection((host, 443), timeout=15)
        except OSError:
            self.request.sendall(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n")
            return
        self.request.sendall(b"HTTP/1.1 200 Connection established\r\n\r\n")
        self.request.settimeout(None)
        remote.settimeout(None)
        pair = {self.request: remote, remote: self.request}
        try:
            while True:
                ready, _, _ = select.select(list(pair), [], [], 900)
                if not ready:
                    break
                for source in ready:
                    data = source.recv(65536)
                    if not data:
                        return
                    pair[source].sendall(data)
        except OSError:
            pass
        finally:
            remote.close()


class Proxy(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


threading.Thread(target=read_replies, daemon=True).start()
threading.Thread(target=Proxy(("127.0.0.1", proxy_port), Tunnel).serve_forever, daemon=True).start()
http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
