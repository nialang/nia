import tempfile
import unittest
from pathlib import Path

from tools.nia_tools.audit.compatibility import (
    fingerprint_domain_errors,
    global_identity_errors,
    release_version_errors,
    typed_fingerprint_domain_errors,
)


class CompatibilityAuditTests(unittest.TestCase):
    def repository(self) -> tuple[tempfile.TemporaryDirectory, Path]:
        temporary = tempfile.TemporaryDirectory()
        root = Path(temporary.name)
        (root / "crates/nia-compat/src").mkdir(parents=True)
        (root / "crates/owner/src").mkdir(parents=True)
        (root / "Cargo.toml").write_text(
            '[workspace]\nmembers = []\n[workspace.package]\nversion = "1.2.3"\n',
            encoding="utf-8",
        )
        (root / "crates/nia-compat/src/lib.rs").write_text(
            'pub const FORMAT: &[u8; 8] = b"NIAFMT01";\n',
            encoding="utf-8",
        )
        return temporary, root

    def test_accepts_versioned_fingerprint_domains(self):
        temporary, root = self.repository()
        self.addCleanup(temporary.cleanup)
        (root / "crates/owner/src/lib.rs").write_text(
            "const PRODUCT_DOMAIN: FingerprintDomain =\n"
            '    FingerprintDomain::new("nia.owner.product.v2");\n'
            "QueryFingerprintBuilder::new(PRODUCT_DOMAIN);\n",
            encoding="utf-8",
        )

        self.assertEqual(fingerprint_domain_errors(root), [])
        self.assertEqual(typed_fingerprint_domain_errors(root), [])

    def test_rejects_unversioned_constructor_domain(self):
        temporary, root = self.repository()
        self.addCleanup(temporary.cleanup)
        (root / "crates/owner/src/lib.rs").write_text(
            'QueryFingerprintBuilder::new("owner-product");\n',
            encoding="utf-8",
        )

        self.assertRegex(fingerprint_domain_errors(root)[0], "owner-product")

    def test_rejects_malformed_typed_domain(self):
        temporary, root = self.repository()
        self.addCleanup(temporary.cleanup)
        (root / "crates/owner/src/lib.rs").write_text(
            "const PRODUCT_DOMAIN: FingerprintDomain =\n"
            '    FingerprintDomain::new("nia.owner..product.v2");\n',
            encoding="utf-8",
        )

        self.assertRegex(fingerprint_domain_errors(root)[0], "owner..product")

    def test_rejects_raw_versioned_constructor_domain(self):
        temporary, root = self.repository()
        self.addCleanup(temporary.cleanup)
        (root / "crates/owner/src/lib.rs").write_text(
            'QueryFingerprintBuilder::new("nia.owner.product.v2");\n',
            encoding="utf-8",
        )

        self.assertRegex(typed_fingerprint_domain_errors(root)[0], "raw literal")

    def test_rejects_duplicate_typed_domain_declarations(self):
        temporary, root = self.repository()
        self.addCleanup(temporary.cleanup)
        (root / "crates/owner/src/lib.rs").write_text(
            "const FIRST_DOMAIN: FingerprintDomain =\n"
            '    FingerprintDomain::new("nia.owner.product.v2");\n'
            "const SECOND_DOMAIN: FingerprintDomain =\n"
            '    FingerprintDomain::new("nia.owner.product.v2");\n',
            encoding="utf-8",
        )

        self.assertRegex(typed_fingerprint_domain_errors(root)[0], "duplicate")

    def test_rejects_registered_magic_outside_registry(self):
        temporary, root = self.repository()
        self.addCleanup(temporary.cleanup)
        (root / "crates/owner/src/lib.rs").write_text(
            'const MAGIC: &[u8; 8] = b"NIAFMT01";\n',
            encoding="utf-8",
        )

        self.assertRegex(global_identity_errors(root)[0], "outside nia-compat")

    def test_rejects_workspace_version_outside_authorities(self):
        temporary, root = self.repository()
        self.addCleanup(temporary.cleanup)
        (root / "README.md").write_text("current version: 1.2.3\n", encoding="utf-8")

        self.assertRegex(release_version_errors(root)[0], "README.md")


if __name__ == "__main__":
    unittest.main()
