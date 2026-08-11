import json
import tempfile
import unittest
from pathlib import Path

from tools.nia_tools.audit.std_build_host import (
    DEFAULT_SNAPSHOT,
    build_host_closure,
    snapshot,
)


class StdBuildHostAuditTests(unittest.TestCase):
    def test_maintained_snapshot_matches_source_closure(self):
        expected = json.loads(DEFAULT_SNAPSHOT.read_text(encoding="utf-8"))

        self.assertEqual(snapshot(), expected)

    def test_follows_package_imports_and_declared_provider_modules(self):
        with tempfile.TemporaryDirectory() as temporary:
            std = Path(temporary) / "std"
            (std / "build").mkdir(parents=True)
            (std / "builtin").mkdir()
            (std / "start").mkdir()
            (std / "support").mkdir()
            (std / "builtin.nia").write_text("", encoding="utf-8")
            (std / "start.nia").write_text("", encoding="utf-8")
            (std / "build.nia").write_text(
                "pub(pkg) module core;\nusing pkg::support;\n", encoding="utf-8"
            )
            (std / "build/core.nia").write_text("", encoding="utf-8")
            (std / "support.nia").write_text(
                "pub(pkg) module provider;\n", encoding="utf-8"
            )
            (std / "support/provider.nia").write_text("", encoding="utf-8")

            closure = build_host_closure(std)

            self.assertEqual(
                closure,
                [
                    "std/build.nia",
                    "std/build/core.nia",
                    "std/builtin.nia",
                    "std/start.nia",
                    "std/support.nia",
                    "std/support/provider.nia",
                ],
            )

    def test_rejects_missing_package_dependency(self):
        with tempfile.TemporaryDirectory() as temporary:
            std = Path(temporary) / "std"
            std.mkdir()
            (std / "builtin.nia").write_text("", encoding="utf-8")
            (std / "start.nia").write_text("", encoding="utf-8")
            (std / "build.nia").write_text("using pkg::missing;\n", encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "does not exist"):
                build_host_closure(std)


if __name__ == "__main__":
    unittest.main()
