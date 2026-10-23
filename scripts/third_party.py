"""Generate declared-license inventory and notice texts for the locked graph.

Includes build/optional/all-target dependencies, a superset of redistributed code.
This does not choose the product license or replace a distribution/legal review.
"""
import argparse
import base64
import hashlib
import io
import json
import subprocess
import tarfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def notice_name(name):
    return Path(name).name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT"))


def generate(output):
    if output.exists():
        raise ValueError("choose a new notice output directory")
    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--all-features", "--locked", "--format-version", "1"], cwd=ROOT, text=True, encoding="utf-8"))
    packages, sections, missing = [], [], []
    workspace = set(metadata["workspace_members"])
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if package["id"] in workspace:
            continue
        folder = Path(package["manifest_path"]).parent
        texts = []
        for path in sorted(folder.rglob("*")):
            if path.is_file() and not path.is_symlink() and notice_name(path.name):
                if path.stat().st_size > 4 * 1024 * 1024:
                    raise ValueError("notice text exceeds budget")
                texts.append((path.relative_to(folder).as_posix(), path.read_text(encoding="utf-8", errors="replace")))
        if not texts:
            for path in folder.glob("README*"):
                if path.is_file() and path.stat().st_size <= 1024 * 1024:
                    texts.append((path.name, path.read_text(encoding="utf-8", errors="replace")))
        if not texts or not package.get("license"):
            missing.append(f"cargo:{package['name']}@{package['version']}")
        identity = f"cargo:{package['name']}@{package['version']}"
        packages.append({"package": identity, "declared_license": package.get("license"), "source": package.get("source") or "reviewed vendor", "repository": package.get("repository"), "texts": [n for n, _ in texts]})
        vcs = folder / ".cargo_vcs_info.json"
        if vcs.is_file():
            packages[-1]["source_revision"] = json.loads(vcs.read_text()).get("git", {}).get("sha1")
        sections.append("\n\n" + "=" * 72 + f"\n{identity}\nDeclared license: {package.get('license')}\n")
        sections.extend(f"\n--- {name} ---\n{text}" for name, text in texts)
    lock = json.loads((ROOT / "apps/desktop/package-lock.json").read_text())
    for path, package in sorted(lock["packages"].items()):
        if not path:
            continue
        identity = f"npm:{path.removeprefix('node_modules/')}@{package['version']}"
        url = package.get("resolved", "")
        if not url.startswith("https://registry.npmjs.org/"):
            raise ValueError("untrusted npm archive URL")
        cache = ROOT / "target/notice-cache"
        cache.mkdir(parents=True, exist_ok=True)
        cached = cache / (hashlib.sha256(url.encode()).hexdigest() + ".tgz")
        if cached.exists():
            archive = cached.read_bytes()
        else:
            with urllib.request.urlopen(url, timeout=60) as response:
                archive = response.read(64 * 1024 * 1024)
        algorithm, encoded = package["integrity"].split("-", 1)
        if algorithm not in ("sha512", "sha256") or base64.b64encode(hashlib.new(algorithm, archive).digest()).decode() != encoded:
            raise ValueError("npm archive integrity mismatch")
        if not cached.exists():
            cached.write_bytes(archive)
        texts = []
        license_value = package.get("license")
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as source:
            for member in source:
                if member.isfile() and member.name == "package/package.json":
                    info = json.load(source.extractfile(member))
                    license_value = info.get("license", license_value)
                if member.isfile() and notice_name(member.name):
                    if member.size > 4 * 1024 * 1024:
                        raise ValueError("npm notice exceeds budget")
                    texts.append((member.name, source.extractfile(member).read().decode("utf-8", errors="replace")))
        if not texts or not license_value:
            missing.append(identity)
        packages.append({"package": identity, "declared_license": license_value, "source": url, "integrity": package["integrity"], "texts": [n for n, _ in texts]})
        packages[-1]["build_only"] = bool(package.get("dev"))
        sections.append("\n\n" + "=" * 72 + f"\n{identity}\nDeclared license: {license_value}\n")
        sections.extend(f"\n--- {name} ---\n{text}" for name, text in texts)
    # Architecture-specific npm binary packages publish the project license via
    # their same-version parent package. Cargo subcrates likewise use a shared
    # repository license. Record this relationship explicitly, without inventing
    # a copyright or choosing between declared alternative licenses.
    families = {"npm:@esbuild/": "npm:esbuild", "npm:@rollup/rollup-": "npm:rollup",
                "npm:@tauri-apps/cli-": "npm:@tauri-apps/cli", "npm:@napi-rs/lzma-": "npm:@napi-rs/lzma"}
    by_id = {p["package"]: p for p in packages}
    for identity in list(missing):
        package = by_id[identity]
        version = identity.rsplit("@", 1)[1]
        parent_id = next((parent + "@" + version for prefix, parent in families.items() if identity.startswith(prefix)), None)
        parent = by_id.get(parent_id)
        if parent is None and identity.startswith("cargo:"):
            parent = next((p for p in packages if p["package"] != identity
                           and ((p.get("repository") and package.get("repository") and p["repository"].split("/tree/")[0] == package["repository"].split("/tree/")[0])
                                or (p.get("source_revision") and p.get("source_revision") == package.get("source_revision")))
                           and p["declared_license"] == package["declared_license"] and p["texts"]), None)
        if parent and parent["texts"] and parent["declared_license"] and package["declared_license"] in (None, parent["declared_license"]):
            package["project_notice_source"] = parent["package"]
            if not package["declared_license"]:
                package["declared_license"] = parent["declared_license"]
            missing.remove(identity)
    inventory = {"schema_version": 1, "scope": "all locked Cargo/npm targets, including build tools and optional dependencies", "product_license": "GPL-3.0-only", "packages": packages, "requires_review": missing}
    output.mkdir(parents=True)
    (output / "license-inventory.json").write_text(json.dumps(inventory, indent=2) + "\n", encoding="utf-8")
    (output / "THIRD_PARTY_NOTICES.txt").write_text("Third-party licenses and notices. SyncEngine product license: GPL-3.0-only; see LICENSE.\n" + "".join(sections), encoding="utf-8")
    return {"packages": len(packages), "requires_review": missing, "output": str(output)}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    print(json.dumps(generate(parser.parse_args().output), indent=2))
