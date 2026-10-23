"""HTTPS WebDAV fixture with an ephemeral self-signed certificate.

Requires cryptography (installed with the SFTP fixture's pinned Paramiko).
Never changes the operating system certificate store.
"""
import datetime
import http.server
import ipaddress
import os
from pathlib import Path
import ssl
import subprocess
import tempfile
import threading

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID


class Handler(http.server.BaseHTTPRequestHandler):
    nodes = {"/dav": None, "/dav/file": b"abc"}
    mutex = threading.RLock()

    def log_message(self, *_):
        pass

    def reply(self, status, body=b"", path=None):
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        if path is not None:
            self.send_header("ETag", self.tag(path))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def tag(self, path):
        import hashlib
        value = self.nodes.get(path)
        content = value if value is not None else repr(sorted(p for p in self.nodes if p.startswith(path + "/"))).encode()
        return '"' + hashlib.sha256(content).hexdigest() + '"'

    def key(self):
        from urllib.parse import unquote, urlsplit
        return unquote(urlsplit(self.path).path).rstrip("/")

    def do_PROPFIND(self):
        from xml.sax.saxutils import escape
        with self.mutex:
            self.rfile.read(int(self.headers.get("Content-Length", "0")))
            path = self.key()
            if path not in self.nodes:
                return self.reply(404)
            paths = [path]
            if self.headers.get("Depth") == "1":
                paths += [p for p in self.nodes if p.startswith(path + "/") and "/" not in p[len(path)+1:]]
            parts = []
            for p in paths:
                collection = "<d:collection/>" if self.nodes[p] is None else ""
                size = len(self.nodes[p] or b"")
                href = p + "/" if self.nodes[p] is None else p
                parts.append(f'<d:response><d:href>{escape(href)}</d:href><d:propstat><d:prop>'
                             f'<d:resourcetype>{collection}</d:resourcetype>'
                             f'<d:getcontentlength>{size}</d:getcontentlength></d:prop>'
                             '<d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>')
            self.reply(207, ('<d:multistatus xmlns:d="DAV:">' + ''.join(parts) + '</d:multistatus>').encode())

    def do_GET(self):
        with self.mutex:
            path = self.key()
            if path not in self.nodes or self.nodes[path] is None:
                return self.reply(404)
            self.reply(200, self.nodes[path], path)

    def do_HEAD(self):
        with self.mutex:
            path = self.key()
            if path not in self.nodes:
                return self.reply(404)
            self.reply(200, self.nodes[path] or b"", path)

    def do_MKCOL(self):
        with self.mutex:
            path = self.key()
            if path in self.nodes:
                return self.reply(405)
            if path.rsplit("/", 1)[0] not in self.nodes:
                return self.reply(409)
            self.nodes[path] = None
            self.reply(201)

    def do_PUT(self):
        with self.mutex:
            content = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            path = self.key()
            if path in self.nodes and self.headers.get("If-None-Match") == "*":
                return self.reply(412)
            self.nodes[path] = content
            self.reply(201)

    def do_MOVE(self):
        from urllib.parse import unquote, urlsplit
        with self.mutex:
            path = self.key()
            target = unquote(urlsplit(self.headers["Destination"]).path).rstrip("/")
            if path not in self.nodes:
                return self.reply(404)
            if target in self.nodes:
                if self.headers.get("Overwrite") == "F":
                    return self.reply(412)
                if self.tag(target) not in self.headers.get("If", ""):
                    return self.reply(412)
            self.nodes[target] = self.nodes.pop(path)
            self.reply(201)

    def do_DELETE(self):
        with self.mutex:
            path = self.key()
            if path not in self.nodes:
                return self.reply(404)
            if self.headers.get("If-Match") != self.tag(path):
                return self.reply(412)
            if self.nodes[path] is None and any(p.startswith(path + "/") for p in self.nodes):
                return self.reply(409)
            del self.nodes[path]
            self.reply(204)


def main():
    workspace = Path(__file__).resolve().parent.parent
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "localhost")])
    now = datetime.datetime.now(datetime.timezone.utc)
    certificate = (x509.CertificateBuilder().subject_name(name).issuer_name(name)
                   .public_key(key.public_key()).serial_number(x509.random_serial_number())
                   .not_valid_before(now - datetime.timedelta(minutes=1))
                   .not_valid_after(now + datetime.timedelta(hours=1))
                   .add_extension(x509.SubjectAlternativeName([
                       x509.DNSName("localhost"), x509.IPAddress(ipaddress.ip_address("127.0.0.1"))]), critical=False)
                   .add_extension(x509.BasicConstraints(ca=True, path_length=None), critical=True)
                   .sign(key, hashes.SHA256()))
    with tempfile.TemporaryDirectory(prefix="phase08-webdav-", dir=workspace / "target") as directory:
        root = Path(directory)
        cert_path = root / "certificate.pem"
        key_path = root / "ephemeral-key.pem"
        cert_path.write_bytes(certificate.public_bytes(serialization.Encoding.PEM))
        key_path.write_bytes(key.private_bytes(serialization.Encoding.PEM,
                             serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert_path, key_path)
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server.socket = context.wrap_socket(server.socket, server_side=True)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            env = os.environ.copy()
            env.update(SYNCENGINE_WEBDAV_URL=f"https://127.0.0.1:{server.server_port}/dav/",
                       SYNCENGINE_WEBDAV_CERT=str(cert_path))
            return subprocess.run(["cargo", "test", "--locked", "-p", "connector-webdav", "--features", "webdav",
                                   "isolated_https_contract", "--", "--ignored"],
                                  cwd=workspace, env=env, timeout=120).returncode
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)


if __name__ == "__main__":
    raise SystemExit(main())
