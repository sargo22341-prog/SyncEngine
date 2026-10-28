"""Isolated Microsoft Graph-shaped HTTPS fixture; no Microsoft account or OS trust changes.

Requires cryptography from target/phase08-fixtures. All credentials are synthetic.
"""
import datetime
import http.server
import ipaddress
import json
import os
from pathlib import Path
import ssl
import subprocess
import tempfile
import threading
from urllib.parse import unquote, urlsplit, parse_qs

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID


class Handler(http.server.BaseHTTPRequestHandler):
    def handle(self):
        try:
            super().handle()
        except ConnectionResetError:
            # A rejected test certificate closes TLS before any HTTP request.
            # Preserve diagnostics for resets while processing a real request.
            if getattr(self, "raw_requestline", b""):
                raise

    counts = {}
    failures = []
    mutex = threading.Lock()

    def log_message(self, *_):
        pass

    def reply(self, status, body=b"", headers=None):
        if isinstance(body, dict):
            body = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        parsed = urlsplit(self.path)
        path = unquote(parsed.path)
        if path.startswith("/download/"):
            if self.headers.get("Authorization") or self.headers.get("Referer"):
                self.failures.append("authorization or referer leaked to download")
                return self.reply(403)
            drive = path.rsplit("/", 1)[-1]
            if drive == "short":
                self.send_response(200)
                self.end_headers()
                self.wfile.write(b"ab")
                return
            return self.reply(200, b"abc")
        if self.headers.get("Authorization") != "Bearer fixture-only-secret":
            self.failures.append("missing graph authentication")
            return self.reply(401)
        parts = path.split("/")
        if len(parts) < 5 or parts[1:3] != ["v1.0", "drives"]:
            self.failures.append("unexpected request path")
            return self.reply(400)
        drive = parts[3]
        with self.mutex:
            key = (drive, path)
            self.counts[key] = self.counts.get(key, 0) + 1
            count = self.counts[key]
        if drive in ("unauth", "denied", "missing", "retry"):
            return self.reply({"unauth": 401, "denied": 403, "missing": 404, "retry": 503}[drive],
                              b"fixture-only-secret signed-url-must-not-escape")
        if drive in ("throttle", "date"):
            return self.reply(429, headers={"Retry-After": "999" if drive == "throttle" else "Wed, 01 Jan 2031 00:00:00 GMT"})
        if drive == "backoff" and count == 1:
            return self.reply(503, headers={"Retry-After": "0"})
        root = {"id": "root", "name": "root", "folder": {}, "parentReference": {"driveId": drive}}
        item = {"id": "file", "name": "été #.txt", "size": 3, "file": {}, "eTag": "v1",
                "parentReference": {"id": "root", "driveId": drive}}
        if parts[4:] == ["root"]:
            return self.reply(200, root)
        if parts[4:] == ["items", "root:", "empty"]:
            return self.reply(200, {"id": "empty", "name": "empty", "folder": {},
                "parentReference": {"id": "root", "driveId": drive}})
        if parts[4:] == ["items", "empty", "children"]:
            return self.reply(200, {"value": []})
        if parts[4:] == ["items", "root:", "été #.txt"]:
            if drive == "changed" and count > 1:
                item["eTag"] = "v2"
            return self.reply(200, item)
        if parts[4:] == ["items", "file", "content"]:
            return self.reply(302, headers={"Location": f"https://localhost:{self.server.server_port}/download/{drive}?signed=fixture"})
        if parts[4:] == ["items", "root", "children"]:
            if drive == "malformed":
                return self.reply(200, b'{"value":[')
            if drive == "traversal":
                item["name"] = "../escape"
            if drive == "alias":
                item["remoteItem"] = {}
            second = "page" in parse_qs(parsed.query)
            if second and drive != "duplicate":
                item["name"] = "empty"; item["id"] = "empty"
                item.pop("file"); item["folder"] = {}
            result = {"value": [item]}
            if not second:
                result["@odata.nextLink"] = ("https://evil.invalid/steal?secret=fixture" if drive == "foreign"
                    else f"https://localhost:{self.server.server_port}{parsed.path}?page=2")
            return self.reply(200, result)
        self.failures.append("unhandled endpoint")
        return self.reply(400)


def main():
    workspace = Path(__file__).resolve().parent.parent
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "localhost")])
    now = datetime.datetime.now(datetime.timezone.utc)
    certificate = (x509.CertificateBuilder().subject_name(name).issuer_name(name)
        .public_key(key.public_key()).serial_number(x509.random_serial_number())
        .not_valid_before(now - datetime.timedelta(minutes=1))
        .not_valid_after(now + datetime.timedelta(hours=1))
        .add_extension(x509.SubjectAlternativeName([x509.DNSName("localhost"),
            x509.IPAddress(ipaddress.ip_address("127.0.0.1"))]), critical=False)
        .add_extension(x509.BasicConstraints(ca=True, path_length=None), critical=True)
        .sign(key, hashes.SHA256()))
    with tempfile.TemporaryDirectory(prefix="phase09-onedrive-", dir=workspace / "target") as directory:
        root = Path(directory)
        cert = root / "certificate.pem"
        private_key = root / "ephemeral-key.pem"
        cert.write_bytes(certificate.public_bytes(serialization.Encoding.PEM))
        private_key.write_bytes(key.private_bytes(serialization.Encoding.PEM,
            serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.minimum_version = ssl.TLSVersion.TLSv1_2
        context.load_cert_chain(cert, private_key)
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server.socket = context.wrap_socket(server.socket, server_side=True)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            env = os.environ.copy()
            env.update(SYNCENGINE_ONEDRIVE_URL=f"https://localhost:{server.server_port}/v1.0/",
                       SYNCENGINE_ONEDRIVE_CERT=str(cert))
            result = subprocess.run(["cargo", "test", "--locked", "-p", "connector-onedrive", "--features", "onedrive",
                "isolated_https_contract", "--", "--ignored"], cwd=workspace, env=env, timeout=120)
            assert not Handler.failures, Handler.failures
            assert Handler.counts.get(("retry", "/v1.0/drives/retry/root")) == 3
            for drive in ("throttle", "date"):
                assert Handler.counts.get((drive, f"/v1.0/drives/{drive}/root")) == 1
            return result.returncode
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)


if __name__ == "__main__":
    raise SystemExit(main())
