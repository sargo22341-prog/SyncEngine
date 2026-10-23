"""Verify Python maintenance locks exclude the actual native CLI and facade."""
import argparse
import os
import subprocess
import tempfile
from pathlib import Path
from state_maintenance import ownership


def run(binary):
    with tempfile.TemporaryDirectory(prefix="syncengine-lock-fixture-") as folder:
        root = Path(folder)
        for name in ("state", "source", "destination", "account"):
            (root / name).mkdir()
        (root / "source/file").write_bytes(b"synthetic")
        env = os.environ.copy()
        env.update(LOCALAPPDATA=str(root / "account"), XDG_STATE_HOME=str(root / "account"))
        common = root / "account/SyncEngine/job-locks"
        with ownership(root / "state", common):
            result = subprocess.run([str(binary), "execute", str(root / "source"), str(root / "destination"),
                                     "--confirm", "--journal", str(root / "run.jsonl")], env=env, capture_output=True, timeout=30)
            if result.returncode == 0 or (root / "run.jsonl").exists() or list((root / "destination").iterdir()):
                raise AssertionError("maintenance did not exclude direct native execution")
            result = subprocess.run([str(binary), "profile", "list", str(root / "state")], env=env, capture_output=True, timeout=30)
            if result.returncode == 0 or (root / "state/profiles.sqlite").exists():
                raise AssertionError("maintenance did not exclude facade state initialization")
        # The exact same command must work after lock release; a missing binary
        # or broken invocation cannot masquerade as a successful exclusion test.
        subprocess.run([str(binary), "profile", "list", str(root / "state")], env=env, check=True, capture_output=True, timeout=30)
    print("Native CLI/facade and maintenance share exclusive OS locks; release verified.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    run(parser.parse_args().binary.resolve(strict=True))
