"""Run separate FTP and explicit FTPS contracts on disposable loopback servers.
Requires pyftpdlib==2.2.0, pyopenssl==26.4.0 and cryptography. All data in target.
"""
import datetime
import ipaddress
import os
from pathlib import Path
import subprocess
import tempfile
import threading

from pyftpdlib.authorizers import DummyAuthorizer
from pyftpdlib.handlers import FTPHandler, TLS_FTPHandler
from pyftpdlib.servers import FTPServer
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID


def run(mode, workspace):
    with tempfile.TemporaryDirectory(prefix=f"phase08-{mode}-", dir=workspace / "target") as directory:
        folder = Path(directory)
        data = folder / "data"
        data.mkdir()
        for index in range(600):
            (data / f"f{index:04}").write_bytes(b"abc")
        (data / "file").write_bytes(b"abc")
        authorizer = DummyAuthorizer()
        authorizer.add_user("fixture", "fixture-only", str(data), perm="elradfmwMT")
        base = FTPHandler if mode == "ftp" else TLS_FTPHandler
        class Handler(base):
            def ftp_RETR(self, file):
                if Path(file).name == "short":
                    candidate=Path(file).resolve()
                    if not candidate.is_relative_to(data.resolve()):
                        raise RuntimeError("fixture path escaped")
                    candidate.write_bytes(b"ab")
                return super().ftp_RETR(file)
        Handler.authorizer = authorizer
        Handler.banner = "fixture-only (synthetic credential echoed by server)"
        if mode == "ftps":
            key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
            name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "localhost")])
            now = datetime.datetime.now(datetime.timezone.utc)
            cert = (x509.CertificateBuilder().subject_name(name).issuer_name(name).public_key(key.public_key())
                    .serial_number(x509.random_serial_number()).not_valid_before(now - datetime.timedelta(minutes=1))
                    .not_valid_after(now + datetime.timedelta(hours=1))
                    .add_extension(x509.SubjectAlternativeName([x509.DNSName("localhost"),x509.IPAddress(ipaddress.ip_address("127.0.0.1"))]),critical=False)
                    .add_extension(x509.BasicConstraints(ca=True,path_length=None),critical=True).sign(key,hashes.SHA256()))
            (folder / "cert.pem").write_bytes(cert.public_bytes(serialization.Encoding.PEM))
            (folder / "key.pem").write_bytes(key.private_bytes(serialization.Encoding.PEM,serialization.PrivateFormat.PKCS8,serialization.NoEncryption()))
            Handler.certfile = str(folder / "cert.pem")
            Handler.keyfile = str(folder / "key.pem")
            Handler.tls_control_required = Handler.tls_data_required = True
        server = FTPServer(("127.0.0.1",0),Handler)
        thread = threading.Thread(target=server.serve_forever, kwargs={"timeout":0.05},daemon=True)
        thread.start()
        try:
            env = os.environ.copy()
            env.update(SYNCENGINE_FTP_ADDR=f"127.0.0.1:{server.socket.getsockname()[1]}",SYNCENGINE_FTP_CERT=str(folder / "cert.pem"))
            result = subprocess.run(["cargo","test","-p",f"connector-{mode}","--features",mode,"--locked","--test","wire","--","--ignored"],
                                    cwd=workspace,env=env,timeout=180).returncode
            if result:
                return result
            (data / "file").write_bytes(b"abc")  # independent application fixture
            env["SYNCENGINE_SECRET_FTP_FIXTURE"] = "fixture-only"
            return subprocess.run(["cargo","test","-p","sync-orchestrator","--features",mode,"--locked",
                                   "--test","wire_profiles",f"isolated_{mode}_profile","--","--ignored"],
                                  cwd=workspace,env=env,timeout=180).returncode
        finally:
            server.close_all()
            thread.join(timeout=2)


if __name__ == "__main__":
    workspace = Path(__file__).resolve().parent.parent
    result = run("ftp", workspace)
    if not result:
        result = run("ftps", workspace)
    raise SystemExit(result)
