"""Measure the release scanner on synthetic files in an isolated temporary tree.

Reports wall time only, not peak RAM or a universal performance guarantee.
"""
import argparse
import json
import subprocess
import tempfile
import time
from pathlib import Path


def measure(binary, count):
    with tempfile.TemporaryDirectory(prefix="syncengine-perf-") as directory:
        root = Path(directory)
        for index in range(count):
            folder = root / str(index // 1000)
            folder.mkdir(exist_ok=True)
            (folder / f"fixture-{index}.txt").write_bytes(b"synthetic fixture\n")
        results = []
        for hashed in (False, True):
            start = time.perf_counter()
            command = [str(binary), "scan", str(root)]
            if hashed:
                command.append("--hash")
            subprocess.run(command, check=True, stdout=subprocess.DEVNULL)
            results.append({"hash": hashed, "wall_seconds": time.perf_counter() - start})
        return {"files": count, "file_bytes": 18, "runs": results,
                "limits": "single warm-cache run; no peak RAM measurement"}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--files", type=int, default=10000)
    args = parser.parse_args()
    if not 1 <= args.files <= 1000000:
        parser.error("--files must be between 1 and 1000000")
    print(json.dumps(measure(args.binary.resolve(strict=True), args.files), indent=2))
