"""Two mock HTTPS servers for the demo.

:8443 is the Acme API. It accepts the token in .secrets/acme-token, read on
every request so a rotation takes effect at once.
:9443 is an attacker's collector. It logs whatever it is sent.
"""

import json
import pathlib
import ssl
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = pathlib.Path(__file__).resolve().parent.parent
CERTS = ROOT / "mock" / "certs"
TOKEN = ROOT / ".secrets" / "acme-token"

BOLD, GREEN, RED, YELLOW, RESET = "\033[1m", "\033[32m", "\033[31m", "\033[33m", "\033[0m"


def log(color, who, line):
    print(f"{color}{BOLD}[{who}]{RESET} {line}", flush=True)


class Acme(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, status, body, headers=()):
        data = (json.dumps(body) + "\n").encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        for name, value in headers:
            self.send_header(name, value)
        self.end_headers()
        self.wfile.write(data)

    def authorized(self):
        sent = self.headers.get("authorization", "")
        valid = sent == f"Bearer {TOKEN.read_text().strip()}"
        mark = f"{GREEN}valid{RESET}" if valid else f"{RED}rejected{RESET}"
        log(GREEN, "acme", f"{self.command} {self.path}  authorization: {sent or '(none)'}  {mark}")
        return valid

    def handle_request(self):
        if not self.authorized():
            return self.reply(401, {"error": "bad credentials"})
        if self.command == "GET" and self.path == "/v1/whoami":
            return self.reply(200, {"login": "acme-bot", "scopes": ["repo:read"]})
        if self.command == "GET" and self.path == "/v1/echo":
            echoed = self.headers.get("authorization", "")
            return self.reply(200, {"echoed": "see x-echo-authorization"},
                              [("x-echo-authorization", echoed)])
        if self.command == "DELETE" and self.path.startswith("/v1/repos/"):
            return self.reply(200, {"deleted": self.path.removeprefix("/v1/repos/")})
        self.reply(404, {"error": "not found"})

    do_GET = do_POST = do_DELETE = handle_request


def read_body(request):
    if request.headers.get("transfer-encoding", "").lower() != "chunked":
        return request.rfile.read(int(request.headers.get("content-length") or 0))
    body = b""
    while size := int(request.rfile.readline().split(b";")[0], 16):
        body += request.rfile.read(size)
        request.rfile.readline()
    request.rfile.readline()
    return body


class Attacker(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        body = read_body(self).decode(errors="replace")
        log(RED, "attacker", f"{self.command} {self.path}")
        log(RED, "attacker", f"  authorization: {self.headers.get('authorization', '(none)')}")
        log(RED, "attacker", f"  body: {body or '(empty)'}")
        self.send_response(204)
        self.end_headers()


def serve(port, handler):
    server = ThreadingHTTPServer(("127.0.0.1", port), handler)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(CERTS / "server.pem", CERTS / "server.key")
    server.socket = context.wrap_socket(server.socket, server_side=True)
    server.serve_forever()


if __name__ == "__main__":
    if not (CERTS / "server.pem").exists() or not TOKEN.exists():
        sys.exit("run ./mock/setup.sh first")
    threading.Thread(target=serve, args=(9443, Attacker), daemon=True).start()
    log(GREEN, "acme", "Acme API on https://127.0.0.1:8443")
    log(RED, "attacker", "collector on https://127.0.0.1:9443")
    serve(8443, Acme)
