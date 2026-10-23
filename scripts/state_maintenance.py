"""Offline state backup/restore and explicit successful-run retention.

Python 3.12+. Uses the same state/per-user OS locks as native clients. Never
purges interrupted/failed runs, baselines or tombstones. Restore only to a new
directory. Run preview before purge; --apply explicitly authorizes removal.
"""
import argparse
import contextlib
import hashlib
import json
import os
import re
import shutil
import sqlite3
import time
from pathlib import Path, PurePosixPath

DATABASES = {"profiles.sqlite": (1398361680, (1, 2)),
             "state.sqlite": (1398361669, (1,)),
             "desktop-runs.sqlite": (1398361681, (1,)),
             "scheduler.sqlite": (1398361682, (1,))}
IDENTIFIER = re.compile(r"[A-Za-z0-9_-]{1,128}\Z")


def lock_root():
    if os.name == "nt":
        base = Path(os.environ["LOCALAPPDATA"])
    else:
        base = Path(os.environ.get("XDG_STATE_HOME", str(Path.home() / ".local/state")))
    if not base.is_absolute():
        raise ValueError("absolute lock directory required")
    return base / "SyncEngine/job-locks"


@contextlib.contextmanager
def ownership(state, common):
    state = state.resolve(strict=True)
    common.mkdir(parents=True, exist_ok=True)
    common = common.resolve(strict=True)
    if state == common or state in common.parents or common in state.parents:
        raise ValueError("state and lock roots must be disjoint")
    with contextlib.ExitStack() as stack:
        for path in (state / "desktop.lock", common / "execution.lock"):
            stream = stack.enter_context(path.open("a+b"))
            stream.seek(0)
            if os.name == "nt":
                import msvcrt
                msvcrt.locking(stream.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield state


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def regular(path):
    if path.is_symlink() or path.is_junction():
        raise ValueError("links/junctions in state are refused")
    if not path.is_file():
        raise ValueError("ordinary file required")


def tree(root):
    """Never traverse symlinks/junctions, including on Windows."""
    def failed(error):
        raise error
    for directory, dirs, files in os.walk(root, followlinks=False, onerror=failed):
        for name in dirs:
            candidate = Path(directory) / name
            if candidate.is_symlink() or candidate.is_junction():
                raise ValueError("linked state directory refused")
        for name in files:
            candidate = Path(directory) / name
            regular(candidate)
            yield candidate


def connection(path):
    regular(path)
    db = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)
    try:
        app, versions = DATABASES[path.name]
        if db.execute("PRAGMA quick_check").fetchall() != [("ok",)]:
            raise ValueError("corrupt database")
        if db.execute("PRAGMA application_id").fetchone()[0] != app or db.execute("PRAGMA user_version").fetchone()[0] not in versions:
            raise ValueError("foreign or unsupported database")
    except BaseException:
        db.close()
        raise
    return db


def reserve_output(state, output):
    output = output.absolute()
    parent = output.parent.resolve(strict=True)
    output = parent / output.name
    if output == state or state in output.parents or output in state.parents:
        raise ValueError("backup/restore directories must be disjoint")
    output.mkdir(exist_ok=False)
    return output


def copy_new(source, target):
    regular(source)
    target.parent.mkdir(parents=True, exist_ok=True)
    with source.open("rb") as src, target.open("xb") as dst:
        shutil.copyfileobj(src, dst, 256 * 1024)
        dst.flush()
        os.fsync(dst.fileno())


def backup(state, output):
    output = reserve_output(state, output)
    files = {}
    for source in tree(state):
        relative = source.relative_to(state).as_posix()
        if source.name in ("desktop.lock", "cancel.request") or source.name.endswith(("-wal", "-shm", "-journal")):
            continue
        target = output / relative
        if relative in DATABASES:
            with contextlib.closing(connection(source)) as src, contextlib.closing(sqlite3.connect(target)) as dst:
                src.backup(dst)
            with target.open("r+b") as stream:
                os.fsync(stream.fileno())
        else:
            copy_new(source, target)
        files[relative] = {"bytes": target.stat().st_size, "sha256": digest(target)}
    if "profiles.sqlite" not in files:
        raise ValueError("application profile store missing; incomplete output retained")
    manifest = {"schema_version": 1, "created_at": int(time.time()), "files": files}
    with (output / "backup-manifest.json").open("x", encoding="utf-8") as stream:
        json.dump(manifest, stream, indent=2)
        stream.flush()
        os.fsync(stream.fileno())
    return manifest


def verify_backup(source):
    regular(source / "backup-manifest.json")
    if (source / "backup-manifest.json").stat().st_size > 64 * 1024 * 1024:
        raise ValueError("backup manifest exceeds budget")
    manifest = json.loads((source / "backup-manifest.json").read_text(encoding="utf-8"))
    if manifest["schema_version"] != 1 or "profiles.sqlite" not in manifest["files"]:
        raise ValueError("unsupported/incomplete backup")
    present = {p.relative_to(source).as_posix() for p in tree(source)} - {"backup-manifest.json"}
    if present != set(manifest["files"]):
        raise ValueError("backup inventory mismatch")
    for name, expected in manifest["files"].items():
        relative = PurePosixPath(name)
        if relative.is_absolute() or ".." in relative.parts or "\\" in name or ":" in name:
            raise ValueError("unsafe backup path")
        path = source / name
        regular(path)
        if path.stat().st_size != expected["bytes"] or digest(path) != expected["sha256"]:
            raise ValueError("backup hash mismatch")
        if name in DATABASES:
            connection(path).close()
    return manifest


def restore(source, output):
    source = source.resolve(strict=True)
    manifest = verify_backup(source)  # Reject alteration before reserving output.
    output = reserve_output(source, output)
    for name, expected in manifest["files"].items():
        copy_new(source / name, output / name)
        if digest(output / name) != expected["sha256"]:
            raise ValueError("restore integrity failure; incomplete output retained")
    for name in DATABASES:
        if (output / name).exists():
            connection(output / name).close()
    return {"restored": str(output), "files": len(manifest["files"])}


def successful_journal(path):
    """Strict sequential, complete success only; malformed/unfinished kept."""
    regular(path)
    first = last = None
    active = set()
    with path.open("rb") as stream:
        for sequence, raw in enumerate(iter(lambda: stream.readline(1024 * 1024 + 1), b"")):
            if len(raw) > 1024 * 1024 or not raw.endswith(b"\n"):
                return None
            line = json.loads(raw)
            if line["format_version"] != 1 or line["sequence"] != sequence or (last and last["record"]["type"] == "execution_finished"):
                return None
            record = line["record"]
            if first is None:
                first = record
            if record["type"] == "operation_started":
                active.add(record["operation"])
            elif record["type"] == "operation_finished":
                active.discard(record["operation"])
            last = line
    if not first or not last or active or first["type"] != "execution_started" or last["record"]["type"] != "execution_finished":
        return None
    execution = first["execution"]
    profile = first.get("profile")
    if not IDENTIFIER.fullmatch(execution) or path.name != execution + ".jsonl" or not profile:
        return None
    if last["record"]["status"] != "succeeded" or any(last["record"]["totals"].get(k, 0) for k in ("failed", "refused", "cancelled", "not_started")):
        return None
    if first.get("versions") not in (None, f"versions/{execution}"):
        return None
    return {"execution": execution, "profile": profile, "finished": last["at"]["secs"]}


def purge(state, keep, days, apply=False):
    if keep < 1 or days < 1:
        raise ValueError("keep >= 1 and days >= 1 required")
    versions = state / "versions"
    if versions.is_symlink() or versions.is_junction():
        raise ValueError("linked version root refused")
    blocked = set()
    for name, query in [("desktop-runs.sqlite", "SELECT profile FROM runs WHERE state='running'"),
                        ("state.sqlite", "SELECT profile FROM executions WHERE state='running'"),
                        ("scheduler.sqlite", "SELECT profile FROM schedules WHERE running_slot IS NOT NULL")]:
        if (state / name).exists():
            with contextlib.closing(connection(state / name)) as db:
                blocked.update(row[0] for row in db.execute(query))
    groups = {}
    for journal in state.glob("*.jsonl"):
        try:
            candidate = successful_journal(journal)
        except (ValueError, KeyError, TypeError):
            candidate = None
        if candidate and candidate["profile"] not in blocked:
            groups.setdefault(candidate["profile"], []).append(candidate)
    cutoff = time.time() - days * 86400
    candidates = []
    for values in groups.values():
        values.sort(key=lambda v: (v["finished"], v["execution"]), reverse=True)
        candidates.extend(v for v in values[keep:] if v["finished"] < cutoff)
    if apply:
        with (state / "maintenance-log.jsonl").open("a", encoding="utf-8") as log:
            log.write(json.dumps({"at": int(time.time()), "operation": "purge_authorized", "candidates": candidates, "keep": keep, "days": days}) + "\n")
            log.flush()
            os.fsync(log.fileno())
    for candidate in candidates:
        folder = state / "versions" / candidate["execution"]
        if folder.is_symlink() or folder.is_junction():
            raise ValueError("linked version store refused")
        content = list(tree(folder)) if folder.exists() else []
        if apply:
            # Only application-owned successful runs; never a synchronized tree.
            for file in content:
                regular(file)
                file.unlink()
            if folder.exists():
                for directory, _, _ in os.walk(folder, topdown=False):
                    Path(directory).rmdir()
            (state / (candidate["execution"] + ".jsonl")).unlink()
            # Baselines and tombstones are deliberately untouched. History rows
            # may already have been retained away by native count retention.
    return {"applied": apply, "keep_successful_per_profile": keep, "minimum_days": days, "candidates": candidates}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lock-root", type=Path)
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("backup", "restore"):
        command = sub.add_parser(name)
        command.add_argument("source", type=Path)
        command.add_argument("output", type=Path)
    command = sub.add_parser("purge")
    command.add_argument("source", type=Path)
    command.add_argument("--keep", type=int, required=True)
    command.add_argument("--days", type=int, required=True)
    command.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    if args.command == "restore":
        result = restore(args.source, args.output)
    else:
        with ownership(args.source, args.lock_root or lock_root()) as state:
            result = backup(state, args.output) if args.command == "backup" else purge(state, args.keep, args.days, args.apply)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
