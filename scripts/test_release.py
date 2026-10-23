"""Release tooling tests: isolated artifacts, no installer execution."""
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import release


class ReleaseTests(unittest.TestCase):
    def test_platform_mismatch_and_empty_release_are_refused(self):
        for formats in (["deb"], []):
            with self.assertRaises(ValueError):
                release.collect(Path("unused"), Path("unused"), "x86_64-pc-windows-msvc", formats)

    def test_versions_agree(self):
        self.assertEqual(release.version(), "0.1.0")

    def test_missing_artifact_leaves_no_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            with self.assertRaises(ValueError):
                release.collect(root, root / "out", "x86_64-unknown-linux-gnu", ["deb"])
            self.assertFalse((root / "out").exists())

    def test_checksums_detect_tampering_and_output_is_not_overwritten(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "deb").mkdir()
            (root / "deb/fixture.deb").write_bytes(b"isolated fixture")
            with patch.object(release, "run", return_value="fixture"):
                release.collect(root, root / "out", "x86_64-unknown-linux-gnu", ["deb"])
            output = root / "out"
            manifest = json.loads((output / "manifest.json").read_text())
            self.assertEqual(manifest["artifacts"][0]["sha256"],
                             hashlib.sha256(b"isolated fixture").hexdigest())
            for line in (output / "SHA256SUMS").read_text().splitlines():
                expected, name = line.split("  ", 1)
                self.assertEqual(release.digest(output / name), expected)
            (output / "fixture.deb").write_bytes(b"modified")
            self.assertNotEqual(release.digest(output / "fixture.deb"),
                                manifest["artifacts"][0]["sha256"])
            with self.assertRaises(ValueError):
                release.collect(root, output, "x86_64-unknown-linux-gnu", ["deb"])
            self.assertEqual((output / "fixture.deb").read_bytes(), b"modified")

    def test_ambiguous_artifacts_are_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "deb").mkdir()
            for name in ("old.deb", "new.deb"):
                (root / "deb" / name).write_bytes(b"fixture")
            with self.assertRaises(ValueError):
                release.collect(root, root / "out", "x86_64-unknown-linux-gnu", ["deb"])


if __name__ == "__main__":
    unittest.main()
