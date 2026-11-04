"""Unit tests for the verified-list and issue-form checks of check_docs.py.

Run: python3 -m unittest discover -s tools -p 'test_check_docs.py'
"""
import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check_docs as c  # noqa: E402

MD = """# Verified TVs

## Verified

| Series | Model tested | webOS | Firmware | Unit |
| --- | --- | --- | --- | --- |
| LG OLED C3 (2023, webOS 8.x) | `OLED42C34LA` | 8.3.0 | `03.30.74` | c3-a |

## Reported working (unconfirmed)

| Model | webOS | Firmware |
| --- | --- | --- |
| `X` | 8 | `99.99.99` |
"""
TOML = """schema = 1
[[series]]
name = "LG OLED C3 (2023, webOS 8.x)"
models = ["OLED??C3*"]
[[series.firmware]]
version = "03.30.74"
"""


class VerifiedList(unittest.TestCase):
    def test_table_rows_only_from_the_verified_section(self):
        self.assertEqual(c.verified_table_rows(MD), [("LG OLED C3 (2023, webOS 8.x)", "03.30.74")])

    def test_data_rows(self):
        self.assertEqual(c.verified_data_rows(TOML), [("LG OLED C3 (2023, webOS 8.x)", "03.30.74")])

    def _root(self, md, toml):
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        root = Path(d.name)
        (root / "docs/compatibility").mkdir(parents=True)
        (root / "data").mkdir()
        (root / "docs/compatibility/verified-models.md").write_text(md)
        (root / "data/verified-models.toml").write_text(toml)
        return root

    def test_agree_and_differ(self):
        self.assertEqual(c.check_verified_list(self._root(MD, TOML)), [])
        extra = TOML + '[[series.firmware]]\nversion = "03.30.73"\n'
        self.assertEqual(len(c.check_verified_list(self._root(MD, extra))), 1)

    def test_repository_files_agree(self):
        self.assertEqual(c.check_verified_list(c.ROOT), [])
        self.assertEqual(c.check_issue_form(c.ROOT), [])


if __name__ == "__main__":
    unittest.main()
