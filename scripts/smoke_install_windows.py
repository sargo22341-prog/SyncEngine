"""NSIS install/reinstall/uninstall smoke test for disposable GitHub runners only.

Never execute against a personal Windows session: NSIS writes HKCU/shortcuts.
All application content/state used by this test is an isolated temporary fixture.
"""
import argparse
import json
import os
import sqlite3
import subprocess
import tempfile
from contextlib import closing
from pathlib import Path


def run(installer):
    if os.name != "nt" or os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted":
        raise RuntimeError("requires a disposable GitHub-hosted Windows runner")
    with tempfile.TemporaryDirectory(prefix="syncengine-install-", dir=os.environ["RUNNER_TEMP"]) as folder:
        root = Path(folder)
        fixture_env = os.environ.copy()
        fixture_env["LOCALAPPDATA"] = str(root / "account")
        install = root / "installed"
        state = root / "state"
        state.mkdir()
        for name in ("source", "destination"):
            (root / name).mkdir()
        config = json.dumps({"id": "fixture", "name": "Upgrade fixture", "group": "", "mode": "backup",
            "source": str(root / "source"), "destination": str(root / "destination"),
            "filter": {"include_files": [], "exclude_files": [], "exclude_directories": [], "case_insensitive": False}, "allow_delete": False})
        with closing(sqlite3.connect(state / "profiles.sqlite")) as db:
            db.executescript("CREATE TABLE profiles(id TEXT PRIMARY KEY, config TEXT NOT NULL); CREATE TABLE preferences(id INTEGER PRIMARY KEY, config TEXT); PRAGMA application_id=1398361680; PRAGMA user_version=1;")
            db.execute("INSERT INTO profiles VALUES('fixture',?)", (config,))
            db.commit()
        marker = state / "preserved-version.fixture"
        marker.write_bytes(b"synthetic recoverable version")
        subprocess.run([str(installer), "/S", f"/D={install}"], check=True, timeout=180)
        cli = install / "syncengine.exe"
        agent = install / "syncengine-agent.exe"
        for binary in (cli, agent):
            if not binary.is_file():
                raise RuntimeError("companion missing from installed package")
        subprocess.run([str(cli), "profile", "list", str(state)], env=fixture_env, check=True, timeout=30)
        subprocess.run([str(agent), "list", str(state)], env=fixture_env, check=True, timeout=30)
        subprocess.run([str(installer), "/S", f"/D={install}"], check=True, timeout=180)
        with closing(sqlite3.connect(state / "profiles.sqlite")) as db:
            if db.execute("SELECT config FROM profiles").fetchone()[0] != config or db.execute("PRAGMA user_version").fetchone()[0] != 2:
                raise RuntimeError("upgrade changed profile content or failed migration")
        uninstallers = list(install.glob("*ninstall*.exe"))
        if len(uninstallers) != 1:
            raise RuntimeError("uninstaller absent or ambiguous")
        subprocess.run([str(uninstallers[0]), "/S", f"_?={install}"], check=True, timeout=180)
        if cli.exists() or agent.exists() or marker.read_bytes() != b"synthetic recoverable version":
            raise RuntimeError("uninstall did not remove companions or preserve state")
        print(json.dumps({"install": True, "same_version_reinstall": True, "v1_migration": True, "uninstall": True, "state_preserved": True}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("installer", type=Path)
    run(parser.parse_args().installer.resolve(strict=True))
