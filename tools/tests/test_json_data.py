import unittest

from tools.nia_tools.common.json_data import decode_json, require_object


class JsonDataTests(unittest.TestCase):
    def test_decodes_recursive_json_value(self) -> None:
        value = require_object(
            decode_json('{"schema_version": 1, "items": [true, null]}'), "fixture"
        )

        self.assertEqual(value["schema_version"], 1)
        self.assertEqual(value["items"], [True, None])

    def test_requires_an_object_at_object_boundary(self) -> None:
        with self.assertRaisesRegex(ValueError, "not an object"):
            require_object(decode_json("[]"), "fixture")

    def test_rejects_non_standard_or_overflowed_numbers(self) -> None:
        for source in ("NaN", "Infinity", "-Infinity", "1e999"):
            with self.subTest(source=source):
                with self.assertRaisesRegex(ValueError, "non-finite number"):
                    decode_json(source, "fixture")


if __name__ == "__main__":
    unittest.main()
