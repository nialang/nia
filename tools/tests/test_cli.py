import contextlib
import io
import unittest

from tools.nia_tools.cli import main, usage
from tools.nia_tools.runtime import PythonVersion, required_python_version


class CliTests(unittest.TestCase):
    def test_root_usage_lists_owned_command_groups(self) -> None:
        output = usage()

        self.assertIn("audit compatibility", output)
        self.assertIn("report crate-boundaries", output)
        self.assertIn("baseline compiler", output)
        self.assertIn("check", output)

    def test_category_help_lists_only_that_category(self) -> None:
        stdout = io.StringIO()

        with contextlib.redirect_stdout(stdout):
            result = main(("audit", "--help"))

        self.assertEqual(result, 0)
        self.assertIn("compatibility", stdout.getvalue())
        self.assertNotIn("crate-boundaries", stdout.getvalue())

    def test_unknown_command_fails_without_running_a_domain_tool(self) -> None:
        stderr = io.StringIO()

        with contextlib.redirect_stderr(stderr):
            result = main(("unknown",))

        self.assertEqual(result, 2)
        self.assertIn("unknown maintenance command", stderr.getvalue())

    def test_repository_declares_python_3_14(self) -> None:
        self.assertEqual(required_python_version(), PythonVersion(3, 14))


if __name__ == "__main__":
    unittest.main()
