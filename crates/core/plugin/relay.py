"""Lets an agent inside the isolated distribution talk to the Essaim app.

The agent posts to 127.0.0.1 as it would on Windows. This process, started
by the app, hands each request to the app over its own stdin/stdout and
returns the answer. Nothing listens on a network interface, so neither the
Windows firewall nor other machines are involved.
"""
import http.server
import itertools
import json
import os
import sys
import threading

port = int(sys.argv[1])
pending = {}
counter = itertools.count(1)
writing = threading.Lock()


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
        with writing:
            sys.stdout.write(json.dumps({"id": request_id, "path": self.path, "body": body}) + "\n")
            sys.stdout.flush()

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


threading.Thread(target=read_replies, daemon=True).start()
http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
