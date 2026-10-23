"""State maintenance tests use synthetic databases and isolated temporary trees."""
from contextlib import closing
import json
import sqlite3
import tempfile
import unittest
from pathlib import Path
import state_maintenance as tool


class Maintenance(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="syncengine-maintenance-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.state = self.root / "state"
        self.state.mkdir()
        with closing(sqlite3.connect(self.state / "profiles.sqlite")) as db:
            db.executescript("CREATE TABLE profiles(id TEXT PRIMARY KEY, config TEXT); PRAGMA user_version=1; PRAGMA application_id=1398361680;")
            db.execute("INSERT INTO profiles VALUES('fixture','original')")
            db.commit()

    def test_backup_restore_and_no_overwrite(self):
        (self.state / "versions").mkdir()
        (self.state / "versions/file").write_bytes(b"synthetic version")
        with tool.ownership(self.state, self.root / "locks"):
            tool.backup(self.state, self.root / "backup")
        tool.restore(self.root / "backup", self.root / "restored")
        self.assertEqual((self.root / "restored/versions/file").read_bytes(), b"synthetic version")
        with closing(sqlite3.connect(self.root / "restored/profiles.sqlite")) as db:
            self.assertEqual(db.execute("SELECT config FROM profiles").fetchone()[0], "original")
        with self.assertRaises(FileExistsError):
            tool.restore(self.root / "backup", self.root / "restored")

    def test_modified_backup_refused_before_output_creation(self):
        tool.backup(self.state, self.root / "backup")
        (self.root / "backup/profiles.sqlite").write_bytes(b"tampered")
        with self.assertRaises(ValueError):
            tool.restore(self.root / "backup", self.root / "restored")
        self.assertFalse((self.root / "restored").exists())

    def test_nested_output_and_unknown_database_are_refused(self):
        with self.assertRaises(ValueError):
            tool.backup(self.state, self.state / "backup")
        with closing(sqlite3.connect(self.state / "profiles.sqlite")) as db:
            db.execute("PRAGMA user_version=99")
        with self.assertRaises(ValueError):
            tool.backup(self.state, self.root / "backup")

    def test_ownership_rejects_concurrent_clients(self):
        with tool.ownership(self.state, self.root / "locks"):
            with self.assertRaises(OSError):
                with tool.ownership(self.state, self.root / "locks"):
                    self.fail("second owner acquired the lock")

    def journal(self, execution, status="succeeded", finish=True):
        lines = [{"format_version": 1, "sequence": 0, "at": {"secs": 10, "nanos": 0},
                  "record": {"type": "execution_started", "execution": execution, "profile": "fixture", "versions": f"versions/{execution}"}}]
        if finish:
            lines.append({"format_version": 1, "sequence": 1, "at": {"secs": 20, "nanos": 0},
                          "record": {"type": "execution_finished", "status": status, "totals": {}}})
        (self.state / f"{execution}.jsonl").write_text("".join(json.dumps(line) + "\n" for line in lines))
        folder = self.state / "versions" / execution
        folder.mkdir(parents=True)
        (folder / "version").write_bytes(b"synthetic recoverable content")

    def test_retention_preserves_failures_interruptions_and_latest_success(self):
        self.journal("success-a")
        self.journal("success-z")
        self.journal("failed", status="failed")
        self.journal("interrupted", finish=False)
        preview = tool.purge(self.state, 1, 1)
        self.assertEqual([c["execution"] for c in preview["candidates"]], ["success-a"])
        self.assertTrue((self.state / "versions/success-a/version").exists())
        tool.purge(self.state, 1, 1, apply=True)
        self.assertFalse((self.state / "success-a.jsonl").exists())
        for execution in ("success-z", "failed", "interrupted"):
            self.assertTrue((self.state / f"versions/{execution}/version").exists())
        self.assertTrue((self.state / "maintenance-log.jsonl").exists())

    def test_unfinished_application_intent_blocks_purge_for_profile(self):
        self.journal("success-a")
        self.journal("success-z")
        with closing(sqlite3.connect(self.state / "desktop-runs.sqlite")) as db:
            db.executescript("CREATE TABLE runs(profile TEXT,state TEXT); PRAGMA application_id=1398361681; PRAGMA user_version=1; INSERT INTO runs VALUES('fixture','running');")
        self.assertEqual(tool.purge(self.state, 1, 1, True)["candidates"], [])

    def test_completed_failure_keeps_its_versions_without_leaking_all_success_history(self):
        self.journal("success-a")
        self.journal("success-z")
        self.journal("failed", status="failed")
        with closing(sqlite3.connect(self.state / "desktop-runs.sqlite")) as db:
            db.executescript("CREATE TABLE runs(profile TEXT,state TEXT); PRAGMA application_id=1398361681; PRAGMA user_version=1; INSERT INTO runs VALUES('fixture','error');")
        result = tool.purge(self.state, 1, 1, True)
        self.assertEqual([c["execution"] for c in result["candidates"]], ["success-a"])
        self.assertTrue((self.state / "versions/failed/version").is_file())


if __name__ == "__main__":
    unittest.main()
