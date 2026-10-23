"""Verify security backports against SHA256-pinned published crate archives.

No advisory suppression: RustSec still reports the original GLib version.
This independent check proves the exact upstream fix is in the compiled source.
"""
import argparse
import hashlib
import io
import json
import tarfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def expected(name, filename, content):
    if name == "gtk3-macros" and filename == "src/attribute_parser.rs":
        content = content.replace(b"Meta, Token, Type};", b"Meta, Token};")
        content = content.replace(b"                    path_span,\n                    span,\n", b"")
        for line in (b"use proc_macro2::Span;\n", b"    pub path_span: Span,\n", b"    pub span: Span,\n", b"    pub ty: Type,\n",
                     b"    let ty = &field.ty;\n", b"        let path_span = field_attr.path().span();\n",
                     b"            ty: ty.clone(),\n"):
            if content.count(line) != 1:
                raise ValueError("GTK unused-field patch context changed")
            content = content.replace(line, b"")
        return content
    if name != "glib" and filename == "src/lib.rs":
        return content.replace(b"use proc_macro::TokenStream;", b"// proc-macro-error3's attribute expansion uses its canonical crate name.\nextern crate proc_macro_error as proc_macro_error3;\nuse proc_macro::TokenStream;")
    if name == "glib" and filename == "src/variant_iter.rs":
        before = b"let p: *mut libc::c_char"
        argument = b"                &p,"
        if content.count(before) != 1 or content.count(argument) != 1:
            raise ValueError("upstream GLib patch context changed")
        return content.replace(before, b"let mut p: *mut libc::c_char").replace(argument, b"                &mut p,")
    if name != "glib" and filename == "Cargo.toml":
        before = b'[dependencies.proc-macro-error]\nversion = "1.0"'
        if content.count(before) != 1:
            raise ValueError("upstream macro patch context changed")
        return content.replace(before, b'[dependencies.proc-macro-error]\npackage = "proc-macro-error3"\nversion = "3.1.1"\ndefault-features = false\nfeatures = ["syn2-error"]')
    return content


def verify(name, cache=None):
    folder = ROOT / "vendor" / name
    provenance = json.loads((folder / "SYNCENGINE_PROVENANCE.json").read_text())
    version = provenance["version"]
    filename = f"{name}-{version}.crate"
    archives = list(cache.glob(f"*/{filename}")) if cache else []
    if archives:
        archive = archives[0].read_bytes()
    else:
        with urllib.request.urlopen(f"https://static.crates.io/crates/{name}/{filename}", timeout=60) as response:
            archive = response.read(16 * 1024 * 1024)
    if hashlib.sha256(archive).hexdigest() != provenance["crates_io_sha256"]:
        raise ValueError(f"upstream archive hash mismatch: {name}")
    names = {"SYNCENGINE_PROVENANCE.json"}
    fixes = {}
    if name == "glib":
        fixes = json.loads((folder / "SYNCENGINE_LIFETIME_FIXES.json").read_text())
        names.update(("SYNCENGINE_LIFETIME_FIXES.json", "SYNCENGINE_LIFETIME_FIXES.patch"))
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as source:
        for member in source:
            if not member.isfile():
                continue
            relative = member.name.removeprefix(f"{name}-{version}/")
            if relative == member.name or ".." in Path(relative).parts:
                raise ValueError("unsafe archive member")
            original = source.extractfile(member).read()
            actual = (folder / relative).read_bytes().replace(b"\r\n", b"\n")
            baseline = expected(name, relative, original.replace(b"\r\n", b"\n"))
            if actual != baseline:
                fix = fixes.get(relative, {})
                if hashlib.sha256(baseline).hexdigest() != fix.get("before_sha256") or hashlib.sha256(actual).hexdigest() != fix.get("after_sha256"):
                    raise ValueError(f"unreviewed vendor change: {name}/{relative}")
            names.add(relative)
    actual_names = {p.relative_to(folder).as_posix() for p in folder.rglob("*") if p.is_file()}
    if actual_names != names:
        raise ValueError(f"unexpected vendor files: {name}: {actual_names ^ names}")
    return {"crate": name, "version": version, "verified": True, "upstream_sha256": provenance["crates_io_sha256"]}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path)
    args = parser.parse_args()
    print(json.dumps([verify(name, args.cache) for name in ("glib", "glib-macros", "gtk3-macros")], indent=2))
