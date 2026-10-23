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
NOTICES = ROOT / "target/third-party"


def run(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True, encoding="utf-8").strip()


def source_inventory():
    """Bind a dirty candidate to actual nonignored source bytes, not only HEAD."""
    names = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=ROOT).decode("utf-8").split("\0")
    entries = []
    for name in sorted(set(names) - {""}):
        path = ROOT / name
        if path.is_symlink() or path.is_junction() or not path.resolve().is_relative_to(ROOT.resolve()):
            raise ValueError("source provenance refuses linked/outside files")
        entries.append({"path": name, "sha256": digest(path) if path.is_file() else None})
    return entries


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
    import third_party
    if NOTICES.exists():
        if NOTICES.is_symlink() or NOTICES.is_junction() or not NOTICES.resolve().is_relative_to(ROOT.resolve()):
            raise ValueError("linked/outside generated notice directory refused")
        if {p.name for p in NOTICES.iterdir()} - {"license-inventory.json", "THIRD_PARTY_NOTICES.txt"}:
            raise ValueError("unexpected files in generated notice directory")
        # Remove only the two known generated files, never a directory tree.
        for name in ("license-inventory.json", "THIRD_PARTY_NOTICES.txt"):
            (NOTICES / name).unlink(missing_ok=True)
        NOTICES.rmdir()
    result = third_party.generate(NOTICES)
    # A private candidate includes the inventory even when legal review remains.
    # No public release is produced by this tool; pending entries remain explicit.
    host = next(line.split(": ", 1)[1] for line in run("rustc", "-vV").splitlines()
                if line.startswith("host: "))
    if host not in ("x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"):
        raise ValueError(f"unsupported release host: {host}")
    subprocess.run(["cargo", "build", "--release", "--locked", "--target", host,
                    "-p", "syncengine-cli", "-p", "syncengine-agent", "--features", "network,onedrive"], cwd=ROOT, check=True)
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
    for name in ("license-inventory.json", "THIRD_PARTY_NOTICES.txt"):
        notice = NOTICES / name
        if not notice.is_file() or notice.is_symlink():
            raise ValueError("release notices missing; run prepare")
        files.append(notice)
    license_file = ROOT / "LICENSE"
    if not license_file.is_file() or license_file.is_symlink():
        raise ValueError("product license missing")
    files.append(license_file)
    names = [file.name for file in files]
    if len(names) != len(set(names)):
        raise ValueError("duplicate artifact names")
    # Read provenance before creating output, including lock hashes.
    source = json.dumps(source_inventory(), indent=2, ensure_ascii=False) + "\n"
    manifest = {"schema_version": 1, "version": current, "target": target,
                "commit": run("git", "rev-parse", "HEAD"),
                "dirty": bool(run("git", "status", "--porcelain")),
                "rustc": run("rustc", "--version"), "node": run("node", "--version"),
                "build_host": platform.platform(), "signed": False,
                "source_inventory_sha256": hashlib.sha256(source.encode("utf-8")).hexdigest(),
                "locks": {name: digest(ROOT / name) for name in
                          ("Cargo.lock", "apps/desktop/package-lock.json")},
                "artifacts": [{"name": p.name, "bytes": p.stat().st_size,
                               "sha256": digest(p)} for p in files]}
    output.mkdir(parents=True)
    for file in files:
        shutil.copy2(file, output / file.name)
    (output / "source-inventory.json").write_text(source, encoding="utf-8", newline="\n")
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    included = [*names, "manifest.json", "source-inventory.json"]
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
