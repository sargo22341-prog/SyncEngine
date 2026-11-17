"""C2 : publication locale sur exFAT monté par FUSE (pas de liens physiques).

Linux, root requis (losetup, montage FUSE). Paquets : exfat-fuse, exfatprogs.
Tout se passe dans une image jetable sous target/ ; aucun dossier personnel.
"""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


def run(workspace):
    for tool in ("mkfs.exfat", "mount.exfat-fuse", "losetup", "fusermount3"):
        if shutil.which(tool) is None and not (tool == "fusermount3" and shutil.which("fusermount")):
            print(f"non exécuté : {tool} absent", file=sys.stderr)
            return 2
    with tempfile.TemporaryDirectory(prefix="c2-exfat-", dir=workspace / "target") as directory:
        folder = Path(directory)
        image = folder / "exfat.img"
        mount = folder / "mnt"
        mount.mkdir()
        with open(image, "wb") as handle:
            handle.truncate(64 * 1024 * 1024)
        subprocess.run(["mkfs.exfat", str(image)], check=True, stdout=subprocess.DEVNULL)
        loop = subprocess.run(["losetup", "-f", "--show", str(image)], check=True,
                              capture_output=True, text=True).stdout.strip()
        try:
            subprocess.run(["mount.exfat-fuse", loop, str(mount)], check=True)
            try:
                env = os.environ.copy()
                env["SYNCENGINE_EXFAT_ROOT"] = str(mount)
                return subprocess.run(
                    ["cargo", "test", "--locked", "-p", "connector-local", "--test", "write",
                     "create_new_publishes_on_a_filesystem_without_hard_links", "--", "--ignored"],
                    cwd=workspace, env=env, timeout=600).returncode
            finally:
                fusermount = shutil.which("fusermount3") or "fusermount"
                subprocess.run([fusermount, "-u", str(mount)], check=False)
        finally:
            subprocess.run(["losetup", "-d", loop], check=False)


if __name__ == "__main__":
    raise SystemExit(run(Path(__file__).resolve().parent.parent))
