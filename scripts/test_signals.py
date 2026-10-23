"""POSIX process-level SIGTERM regression; isolated sparse file and journal only."""
import argparse
import json
import os
import signal
import subprocess
import tempfile
import time
from pathlib import Path


def run(binary):
    if os.name != "posix":
        raise RuntimeError("this test requires POSIX SIGTERM; no Windows result claimed")
    with tempfile.TemporaryDirectory(prefix="syncengine-signal-fixture-") as folder:
        root = Path(folder)
        source, destination = root / "source", root / "destination"
        source.mkdir()
        destination.mkdir()
        with (source / "sparse.bin").open("wb") as stream:
            stream.truncate(512 * 1024 * 1024)
        account = root / "account"
        account.mkdir()
        env = os.environ.copy()
        env["XDG_STATE_HOME"] = str(account)
        journal = root / "run.jsonl"
        process = subprocess.Popen([str(binary), "execute", str(source), str(destination), "--confirm", "--journal", str(journal)],
                                   env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        sent = False
        try:
            deadline = time.monotonic() + 60
            while process.poll() is None and time.monotonic() < deadline:
                if journal.exists():
                    lines = journal.read_bytes().splitlines(keepends=True)
                    complete = [json.loads(line) for line in lines if line.endswith(b"\n")]
                    if any(line["record"]["type"] == "staging_created" for line in complete):
                        process.send_signal(signal.SIGTERM)
                        sent = True
                        break
                time.sleep(0.01)
            stdout, stderr = process.communicate(timeout=60)
            if not sent or process.returncode != 4:
                raise AssertionError(f"cooperative cancellation missing: sent={sent}, exit={process.returncode}, output={stdout!r}, error={stderr!r}")
            lines = [json.loads(line) for line in journal.read_bytes().splitlines()]
            if lines[-1]["record"]["type"] != "execution_finished" or lines[-1]["record"]["status"] != "cancelled":
                raise AssertionError("SIGTERM did not persist a cancelled execution")
            if list(destination.iterdir()):
                raise AssertionError("cancelled stage was published or not cleaned")
            if (source / "sparse.bin").stat().st_size != 512 * 1024 * 1024:
                raise AssertionError("source changed")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            for stream in (process.stdout, process.stderr):
                stream.close()
    print(json.dumps({"sigterm": "cooperative", "journal": "cancelled", "destination": "unchanged"}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    run(parser.parse_args().binary.resolve(strict=True))
