"""Prepare native Tauri companions and collect a verifiable private release.

Run from any directory. No publishing, installation or user-state mutation.
"""
import argparse
import hashlib
import json
import platform
import shutil
import subprocess
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def run(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def version():
    cargo = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    expected = cargo["workspace"]["package"]["version"]
    for name in ("apps/desktop/package.json", "apps/desktop/src-tauri/tauri.conf.json"):
        if json.loads((ROOT / name).read_text(encoding="utf-8"))["version"] != expected:
            raise ValueError(f"version mismatch: {name}")
    return expected


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def prepare():
    """Use the host triple, never silently package cross-target binaries."""
    version()
    host = next(line.split(": ", 1)[1] for line in run("rustc", "-vV").splitlines()
                if line.startswith("host: "))
    if host not in ("x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"):
        raise ValueError(f"unsupported release host: {host}")
    subprocess.run(["cargo", "build", "--release", "--locked", "--target", host,
                    "-p", "syncengine-cli", "-p", "syncengine-agent"], cwd=ROOT, check=True)
    suffix = ".exe" if "windows" in host else ""
    output = ROOT / "apps/desktop/src-tauri/binaries"
    output.mkdir(exist_ok=True)
    for name in ("syncengine", "syncengine-agent"):
        shutil.copy2(ROOT / "target" / host / "release" / (name + suffix),
                     output / f"{name}-{host}{suffix}")


def collect(bundle, output, target, formats):
    """Fail closed on missing formats; never mix a prior collection with this one."""
    current = version()
    allowed = {"x86_64-pc-windows-msvc": {"nsis"},
               "x86_64-unknown-linux-gnu": {"deb", "appimage"}}
    if not formats or target not in allowed or not set(formats) <= allowed[target]:
        raise ValueError("artifact formats do not match target")
    if output.exists():
        raise ValueError("output already exists; choose a new release directory")
    files = []
    for kind in formats:
        extension = {"nsis": ".exe", "deb": ".deb", "appimage": ".AppImage"}[kind]
        matches = sorted((bundle / kind).glob("*" + extension))
        if len(matches) != 1 or not matches[0].is_file() or matches[0].is_symlink():
            raise ValueError(f"expected exactly one {kind} artifact")
        files.extend(matches)
    names = [file.name for file in files]
    if len(names) != len(set(names)):
        raise ValueError("duplicate artifact names")
    # Read provenance before creating output, including lock hashes.
    manifest = {"schema_version": 1, "version": current, "target": target,
                "commit": run("git", "rev-parse", "HEAD"),
                "dirty": bool(run("git", "status", "--porcelain")),
                "rustc": run("rustc", "--version"), "node": run("node", "--version"),
                "build_host": platform.platform(), "signed": False,
                "locks": {name: digest(ROOT / name) for name in
                          ("Cargo.lock", "apps/desktop/package-lock.json")},
                "artifacts": [{"name": p.name, "bytes": p.stat().st_size,
                               "sha256": digest(p)} for p in files]}
    output.mkdir(parents=True)
    for file in files:
        shutil.copy2(file, output / file.name)
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    included = [*names, "manifest.json"]
    (output / "SHA256SUMS").write_text("".join(
        f"{digest(output / name)}  {name}\n" for name in sorted(included)), encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("prepare")
    commands.add_parser("check-version")
    collector = commands.add_parser("collect")
    collector.add_argument("--bundle", type=Path, required=True)
    collector.add_argument("--output", type=Path, required=True)
    collector.add_argument("--target", choices=["x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"], required=True)
    collector.add_argument("--formats", nargs="+", choices=["nsis", "deb", "appimage"], required=True)
    args = parser.parse_args()
    if args.command == "prepare":
        prepare()
    elif args.command == "collect":
        collect(args.bundle, args.output, args.target, args.formats)
    else:
        print(version())
