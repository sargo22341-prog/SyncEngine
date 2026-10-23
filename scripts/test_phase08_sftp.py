"""Run the SFTP wire contract against an ephemeral, memory-only Paramiko server.

Requires Python + paramiko (tested with 5.1.0). No SSH agent, personal account,
real files, persistent keys, port exposure or Docker daemon is used.
"""
import io
import os
from pathlib import Path
import socket
import stat
import subprocess
import tempfile
import threading
import struct

import paramiko


class Authentication(paramiko.ServerInterface):
    def check_auth_password(self, username, password):
        return (paramiko.AUTH_SUCCESSFUL if username == "fixture" and
                password == "fixture-only" else paramiko.AUTH_FAILED)

    def get_allowed_auths(self, username):
        return "password"

    def check_channel_request(self, kind, channel_id):
        return (paramiko.OPEN_SUCCEEDED if kind == "session" else
                paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED)


class ExtendedServer(paramiko.SFTPServer):
    def _send_server_version(self):
        # Advertise the standard OpenSSH extension implemented below by
        # Filesystem.posix_rename; no custom SSH or cryptography is implemented.
        from paramiko.sftp import CMD_INIT, CMD_VERSION
        from paramiko.message import Message
        kind, data = self._read_packet()
        if kind != CMD_INIT:
            raise paramiko.SSHException("expected SFTP INIT")
        message = Message()
        message.add_int(3)
        message.add_string("posix-rename@openssh.com")
        message.add_string("1")
        self._send_packet(CMD_VERSION, message)
        return struct.unpack(">I", data[:4])[0]


def attributes(name, mode=stat.S_IFREG | 0o400, size=3):
    result = paramiko.SFTPAttributes()
    result.filename = name
    result.st_mode = mode
    result.st_size = size
    result.st_mtime = result.st_atime = 1_700_000_000
    return result


class MemoryHandle(paramiko.SFTPHandle):
    def __init__(self, path, flags, initial):
        super().__init__(flags)
        self.path = path
        self.content = io.BytesIO(initial)
        self.writable = bool(flags & (os.O_WRONLY | os.O_RDWR))

    def read(self, offset, length):
        self.content.seek(offset)
        return self.content.read(length)

    def write(self, offset, data):
        self.content.seek(offset)
        self.content.write(data)
        mode, _, modified = Filesystem.nodes[self.path]
        Filesystem.nodes[self.path] = (mode, self.content.getvalue(), modified)
        return paramiko.SFTP_OK


class Filesystem(paramiko.SFTPServerInterface):
    fail_publication = False
    nodes = {"/": (stat.S_IFDIR | 0o700, b"", 1700000000),
             "/root": (stat.S_IFDIR | 0o700, b"", 1700000000)}
    nodes.update({f"/root/f{n:04}": (stat.S_IFREG | 0o600, b"abc", 1700000000) for n in range(600)})
    nodes.update({"/root/file": (stat.S_IFREG | 0o600, b"abc", 1700000000),
                  "/root/broken": (stat.S_IFREG | 0o600, b"ab", 1700000000),
                  "/root/link": (stat.S_IFLNK | 0o777, b"", 1700000000),
                  "/root/denied": (stat.S_IFDIR | 0o700, b"", 1700000000),
                  "/root/empty": (stat.S_IFDIR | 0o700, b"", 1700000000)})

    def list_folder(self, path):
        if path == "/root/denied":
            return paramiko.SFTP_PERMISSION_DENIED
        if path == "/root/nonutf8":
            return [attributes(b"\xff")]
        if path not in self.nodes or not stat.S_ISDIR(self.nodes[path][0]):
            return paramiko.SFTP_NO_SUCH_FILE
        prefix = path.rstrip("/") + "/"
        return [self.lstat(name, name[len(prefix):]) for name in list(self.nodes)
                if name.startswith(prefix) and "/" not in name[len(prefix):] and name != path]

    def lstat(self, path, name=None):
        if path == "/root/nonutf8":
            return attributes(path, stat.S_IFDIR | 0o700)
        if path not in self.nodes:
            return paramiko.SFTP_NO_SUCH_FILE
        mode, content, modified = self.nodes[path]
        result = attributes(name or path, mode, 4 if path == "/root/broken" else len(content))
        result.st_mtime = result.st_atime = modified
        return result

    stat = lstat

    def open(self, path, flags, attr):
        if flags & os.O_EXCL and path in self.nodes:
            return paramiko.SFTP_FAILURE
        if path not in self.nodes:
            if not flags & os.O_CREAT:
                return paramiko.SFTP_NO_SUCH_FILE
            self.nodes[path] = (stat.S_IFREG | 0o600, b"", 1700000000)
        if not stat.S_ISREG(self.nodes[path][0]):
            return paramiko.SFTP_PERMISSION_DENIED
        return MemoryHandle(path, flags, self.nodes[path][1])

    def mkdir(self, path, attr):
        if path in self.nodes:
            return paramiko.SFTP_FAILURE
        self.nodes[path] = (stat.S_IFDIR | 0o700, b"", 1700000000)
        return paramiko.SFTP_OK

    def rmdir(self, path):
        if path not in self.nodes:
            return paramiko.SFTP_NO_SUCH_FILE
        if any(p.startswith(path.rstrip("/") + "/") for p in self.nodes):
            return paramiko.SFTP_FAILURE
        del self.nodes[path]
        return paramiko.SFTP_OK

    def remove(self, path):
        if path not in self.nodes:
            return paramiko.SFTP_NO_SUCH_FILE
        if not stat.S_ISREG(self.nodes[path][0]):
            return paramiko.SFTP_PERMISSION_DENIED
        del self.nodes[path]
        return paramiko.SFTP_OK

    def rename(self, old, new):
        if old == "/root/fail-publish" and new.endswith("/previous"):
            self.fail_publication = True
        if self.fail_publication and old.endswith("/content") and new == "/root/fail-publish":
            return paramiko.SFTP_FAILURE
        if new in self.nodes:
            return paramiko.SFTP_FAILURE
        return self.posix_rename(old, new)

    def posix_rename(self, old, new):
        if old not in self.nodes:
            return paramiko.SFTP_NO_SUCH_FILE
        self.nodes[new] = self.nodes.pop(old)
        return paramiko.SFTP_OK

    def chattr(self, path, attr):
        if path not in self.nodes:
            return paramiko.SFTP_NO_SUCH_FILE
        mode, content, modified = self.nodes[path]
        self.nodes[path] = (mode, content, getattr(attr, "st_mtime", modified))
        return paramiko.SFTP_OK


def main():
    workspace = Path(__file__).resolve().parent.parent
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(16)
    listener.settimeout(0.1)
    port = listener.getsockname()[1]
    host_key = paramiko.RSAKey.generate(2048)
    stop = threading.Event()
    transports = []

    def serve():
        while not stop.is_set():
            try:
                client, _ = listener.accept()
            except socket.timeout:
                continue
            transport = paramiko.Transport(client)
            # Fixed strong group avoids Paramiko 5.1/libssh2 group-exchange
            # negotiation incompatibility; production client defaults unchanged.
            transport.get_security_options().kex = ("diffie-hellman-group14-sha256",)
            transports.append(transport)
            transport.add_server_key(host_key)
            transport.set_subsystem_handler("sftp", ExtendedServer, Filesystem)
            try:
                transport.start_server(server=Authentication())
            except (EOFError, paramiko.SSHException):
                transport.close()

    thread = threading.Thread(target=serve, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="phase08-sftp-", dir=workspace / "target") as directory:
            known = Path(directory) / "known_hosts"
            known.write_text(f"[fixture.test]:{port} {host_key.get_name()} {host_key.get_base64()}\n", encoding="utf-8")
            wrong = Path(directory) / "wrong_hosts"
            other = paramiko.RSAKey.generate(2048)
            wrong.write_text(f"[fixture.test]:{port} {other.get_name()} {other.get_base64()}\n", encoding="utf-8")
            env = os.environ.copy()
            env.update(SYNCENGINE_SFTP_ADDR=f"127.0.0.1:{port}",
                       SYNCENGINE_SFTP_HOSTS=str(known), SYNCENGINE_SFTP_BAD_HOSTS=str(wrong))
            result = subprocess.run(["cargo", "test", "--locked", "-p", "connector-sftp", "--features", "sftp",
                                     "--test", "contract", "--", "--ignored"], cwd=workspace, env=env,
                                    timeout=120)
            return result.returncode
    finally:
        stop.set()
        thread.join(timeout=2)
        listener.close()
        for transport in transports:
            transport.close()


if __name__ == "__main__":
    raise SystemExit(main())
