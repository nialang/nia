import tempfile
import unittest
from pathlib import Path

from tools.nia_tools.report.crate_boundaries import (
    CargoMetadata,
    CargoPackage,
    rust_source_metrics,
    workspace_boundaries,
)


class CrateBoundaryTests(unittest.TestCase):
    def test_counts_non_empty_source_lines_and_public_items(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            crate_root = Path(directory)
            source_root = crate_root / "src"
            source_root.mkdir()
            (source_root / "lib.rs").write_text(
                "pub struct Public;\n\nstruct Private;\npub(crate) fn helper() {}\n",
                encoding="utf-8",
            )

            self.assertEqual(rust_source_metrics(crate_root), (3, 1))

    def test_separates_production_and_dev_only_dependents(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            packages: list[CargoPackage] = []
            for name in ("provider", "consumer", "dev-consumer"):
                crate_root = root / name
                (crate_root / "src").mkdir(parents=True)
                (crate_root / "src" / "lib.rs").write_text("", encoding="utf-8")
                packages.append(
                    {
                        "id": name,
                        "name": name,
                        "manifest_path": str(crate_root / "Cargo.toml"),
                        "dependencies": [],
                    }
                )
            packages[1]["dependencies"] = [
                {"name": "provider", "kind": None},
                {"name": "provider", "kind": "dev"},
            ]
            packages[2]["dependencies"] = [
                {"name": "provider", "kind": "dev"}
            ]
            metadata: CargoMetadata = {
                "packages": packages,
                "workspace_members": [package["id"] for package in packages],
            }

            boundaries = {
                boundary.name: boundary
                for boundary in workspace_boundaries(metadata)
            }

            self.assertEqual(
                boundaries["provider"].production_dependents,
                ("consumer",),
            )
            self.assertEqual(
                boundaries["provider"].dev_only_dependents,
                ("dev-consumer",),
            )
            self.assertEqual(
                boundaries["consumer"].production_dependencies,
                ("provider",),
            )


if __name__ == "__main__":
    unittest.main()
