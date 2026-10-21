"""Verify the absolute-position contract used by disc-count scripts."""

import io
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from stone_stats import summarize


INITIAL = "---------------------------OX------XO--------------------------- b"


class PositionStatisticsTests(unittest.TestCase):
    def test_turns_duplicates_and_selected_original_lines_are_preserved(self):
        white = INITIAL[:64] + " w"
        rows = [white, INITIAL, white]
        total, counts, matches = summarize(io.StringIO("\n".join(rows) + "\n"), 4)
        self.assertEqual(total, 3)
        self.assertEqual(dict(counts), {4: 3})
        self.assertEqual(matches, rows)

    def test_every_field_and_every_physical_line_is_validated(self):
        for invalid in [INITIAL[:64], "", INITIAL + " b", INITIAL[:65] + "x"]:
            with self.subTest(invalid=invalid):
                with self.assertRaisesRegex(ValueError, "line 2"):
                    summarize(io.StringIO(INITIAL + "\n" + invalid + "\n"))


if __name__ == "__main__":
    unittest.main()
