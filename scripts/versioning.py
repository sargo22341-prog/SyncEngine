"""Version bump and release notes of the automated GitHub release.

Run from any directory. Only Cargo.toml, Cargo.lock and RELEASE_NOTES.md are
rewritten; nothing is built, pushed or published here.

    python scripts/versioning.py bump patch|minor|major
    python scripts/versioning.py notes OUTPUT     # notes of the next release
    python scripts/versioning.py reset-notes      # keeps header and marker
    python scripts/versioning.py release patch|minor|major NOTES_OUTPUT

`release` chains the three steps and creates the local commit
`Version X.Y.Z [skip ci]`. Its author, committer and dates come from HEAD, so
every CI job that runs it on the same commit obtains the same commit id: the
binaries built by one job belong to the commit published by another.
"""
import argparse
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MARKER = "<!-- notes -->"
VERSION = re.compile(r"(\d+)\.(\d+)\.(\d+)")


def read(path):
    # Newlines normalized: a Windows checkout may hold CRLF files.
    return path.read_text(encoding="utf-8").replace("\r\n", "\n")


def write(path, text):
    path.write_text(text, encoding="utf-8", newline="\n")


def next_version(current, part):
    match = VERSION.fullmatch(current)
    if not match:
        raise ValueError(f"unreadable version: {current}")
    major, minor, patch = map(int, match.groups())
    if part == "patch":
        return f"{major}.{minor}.{patch + 1}"
    if part == "minor":
        return f"{major}.{minor + 1}.0"
    if part == "major":
        return f"{major + 1}.0.0"
    raise ValueError(f"unknown version part: {part} (patch, minor or major)")


def inheriting_packages(root, manifest):
    """Names of the workspace members whose version is the workspace one."""
    names = set()
    for member in manifest["workspace"]["members"]:
        package = tomllib.loads(read(root / member / "Cargo.toml"))["package"]
        if package.get("version") == {"workspace": True}:
            names.add(package["name"])
    return names


def bump(part, root=ROOT):
    """Bump [workspace.package] version and the matching local Cargo.lock entries.

    Registry and patched packages (which carry their own version) are left
    untouched, so the build stays `--locked`.
    """
    cargo_toml = root / "Cargo.toml"
    text = read(cargo_toml)
    manifest = tomllib.loads(text)
    current = manifest["workspace"]["package"]["version"]
    new = next_version(current, part)
    section = re.search(r"^\[workspace\.package\]\n(?:(?!\[).*\n)*", text, re.MULTILINE)
    if not section:
        raise ValueError("[workspace.package] missing from Cargo.toml")
    body, count = re.subn(rf'^version = "{re.escape(current)}"$', f'version = "{new}"',
                          section.group(0), count=1, flags=re.MULTILINE)
    if count != 1:
        raise ValueError("workspace version line not found")

    # Both files are checked before either is written.
    names = inheriting_packages(root, manifest)
    lock = root / "Cargo.lock"
    blocks = read(lock).split("\n[[package]]\n")
    changed = set()
    for index, block in enumerate(blocks):
        name = re.search(r'^name = "([^"]+)"$', block, re.MULTILINE)
        if not name or name.group(1) not in names or re.search(r"^source = ", block, re.MULTILINE):
            continue
        blocks[index], count = re.subn(rf'^version = "{re.escape(current)}"$',
                                       f'version = "{new}"', block, count=1, flags=re.MULTILINE)
        if count == 1:
            changed.add(name.group(1))
    if changed != names:
        raise ValueError(f"Cargo.lock out of date for: {', '.join(sorted(names - changed))}")
    write(cargo_toml, text[:section.start()] + body + text[section.end():])
    write(lock, "\n[[package]]\n".join(blocks))
    return current, new


def split_notes(path):
    lines = read(path).split("\n")
    if MARKER not in lines:
        raise ValueError(f"marker line {MARKER} missing from {path.name}")
    at = lines.index(MARKER)
    return lines[:at + 1], lines[at + 1:]


def notes(path):
    """Everything after the marker, without leading or trailing blank lines."""
    return "\n".join(split_notes(path)[1]).strip("\n")


def reset_notes(path):
    write(path, "\n".join(split_notes(path)[0]) + "\n\n")


def write_notes(path, output):
    text = notes(path)
    write(output, text + "\n" if text else "")


def github_output(**values):
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as out:
            out.writelines(f"{key}={value}\n" for key, value in values.items())


def release(part, notes_output, root=ROOT):
    """Bump, move the notes out and commit locally; returns (version, commit)."""
    # Files are rewritten with LF on purpose (identical commit on every OS); under
    # core.autocrlf=true Git would only warn about the future CRLF checkout.
    git = ["git", "-c", "core.safecrlf=false", "-C", str(root)]
    if subprocess.run([*git, "status", "--porcelain", "--untracked-files=no"], check=True,
                      capture_output=True, text=True).stdout.strip():
        raise ValueError("tracked files modified: the version commit must start from HEAD")
    head = subprocess.run([*git, "log", "-1", "--format=%cI"], check=True,
                          capture_output=True, text=True).stdout.strip()
    _, new = bump(part, root)
    release_notes = root / "RELEASE_NOTES.md"
    write_notes(release_notes, notes_output)
    reset_notes(release_notes)
    identity = {"GIT_AUTHOR_NAME": "github-actions[bot]",
                "GIT_AUTHOR_EMAIL": "41898282+github-actions[bot]@users.noreply.github.com",
                "GIT_AUTHOR_DATE": head, "GIT_COMMITTER_DATE": head}
    identity["GIT_COMMITTER_NAME"] = identity["GIT_AUTHOR_NAME"]
    identity["GIT_COMMITTER_EMAIL"] = identity["GIT_AUTHOR_EMAIL"]
    subprocess.run([*git, "-c", "commit.gpgsign=false", "commit", "--quiet", "-m",
                    f"Version {new} [skip ci]", "--", "Cargo.toml", "Cargo.lock", "RELEASE_NOTES.md"],
                   check=True, env={**os.environ, **identity})
    commit = subprocess.run([*git, "rev-parse", "HEAD"], check=True,
                            capture_output=True, text=True).stdout.strip()
    return new, commit


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("bump").add_argument("part", choices=["patch", "minor", "major"])
    commands.add_parser("notes").add_argument("output", type=Path)
    commands.add_parser("reset-notes")
    releaser = commands.add_parser("release")
    releaser.add_argument("part", choices=["patch", "minor", "major"])
    releaser.add_argument("notes_output", type=Path)
    args = parser.parse_args()
    release_notes = ROOT / "RELEASE_NOTES.md"
    try:
        if args.command == "bump":
            old, new = bump(args.part)
            print(f"Version {old} -> {new}")
            github_output(name=new)
        elif args.command == "release":
            new, commit = release(args.part, args.notes_output)
            print(f"Version {new} ({commit})")
            github_output(name=new, commit=commit)
        elif args.command == "notes":
            write_notes(release_notes, args.output)
        else:
            reset_notes(release_notes)
    except ValueError as error:
        print(error, file=sys.stderr)
        sys.exit(1)
