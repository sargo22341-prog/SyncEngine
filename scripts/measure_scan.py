"""Measure the release scanner on synthetic files in an isolated temporary tree.

Reports wall time and observed kernel high-water RSS on the available OS.
Sampling can miss the final peak; this is not a universal performance guarantee.
"""
import argparse
import json
import os
import subprocess
import tempfile
import time
from pathlib import Path


def memory_peak(pid):
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes
        class Counters(ctypes.Structure):
            _fields_ = [("cb", wintypes.DWORD), ("page_faults", wintypes.DWORD)] + [(name, ctypes.c_size_t) for name in
                ("peak_working_set", "working_set", "peak_paged", "paged", "peak_nonpaged", "nonpaged", "pagefile", "peak_pagefile")]
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        psapi = ctypes.WinDLL("psapi", use_last_error=True)
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        psapi.GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
        handle = kernel.OpenProcess(0x0410, False, pid)
        if not handle:
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            counters = Counters()
            counters.cb = ctypes.sizeof(counters)
            if not psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
                raise ctypes.WinError(ctypes.get_last_error())
            return counters.peak_working_set
        finally:
            kernel.CloseHandle(handle)
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        if line.startswith("VmHWM:"):
            return int(line.split()[1]) * 1024
    raise OSError("kernel high-water RSS unavailable")


def measure(binary, count, mode="both"):
    with tempfile.TemporaryDirectory(prefix="syncengine-perf-") as directory:
        root = Path(directory)
        for index in range(count):
            folder = root / str(index // 1000)
            folder.mkdir(exist_ok=True)
            (folder / f"fixture-{index}.txt").write_bytes(b"synthetic fixture\n")
        results = []
        for hashed in ({"both": (False, True), "metadata": (False,), "hash": (True,)}[mode]):
            start = time.perf_counter()
            command = [str(binary), "scan", str(root)]
            if hashed:
                command.append("--hash")
            peak = None
            errors = set()
            with subprocess.Popen(command, stdout=subprocess.DEVNULL) as process:
                while process.poll() is None:
                    try:
                        observed = memory_peak(process.pid)
                        peak = max(peak or 0, observed)
                    except OSError as error:
                        errors.add(type(error).__name__)
                    time.sleep(0.02)
                if process.returncode:
                    raise subprocess.CalledProcessError(process.returncode, command)
            results.append({"hash": hashed, "wall_seconds": time.perf_counter() - start,
                            "observed_peak_rss_bytes": peak, "sampling_errors": sorted(errors)})
        return {"files": count, "file_bytes": 18, "runs": results,
                "limits": "single warm-cache run; 20ms sampling of kernel high-water RSS can miss final peak"}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--files", type=int, default=10000)
    parser.add_argument("--mode", choices=["both", "metadata", "hash"], default="both")
    args = parser.parse_args()
    if not 1 <= args.files <= 1000000:
        parser.error("--files must be between 1 and 1000000")
    print(json.dumps(measure(args.binary.resolve(strict=True), args.files, args.mode), indent=2))
