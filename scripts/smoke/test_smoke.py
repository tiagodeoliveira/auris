"""Unit tests for pure helpers in smoke.py.

Run with: python -m unittest discover -s scripts/smoke -p 'test_*.py' -v
"""

import tempfile
import unittest
from pathlib import Path

from smoke import parse_env_file


class ParseEnvFileTests(unittest.TestCase):
    def test_basic_key_value_pairs(self):
        text = "FOO=bar\nBAZ=qux\n"
        env = parse_env_file_from_string(text)
        self.assertEqual(env["FOO"], "bar")
        self.assertEqual(env["BAZ"], "qux")

    def test_skips_comments_and_blanks(self):
        text = "# a comment\n\nFOO=bar\n  # indented comment\n"
        env = parse_env_file_from_string(text)
        self.assertEqual(env, {"FOO": "bar"})

    def test_strips_inline_whitespace_but_not_value_internal(self):
        text = "FOO=bar baz\n  BAZ  =  qux  \n"
        env = parse_env_file_from_string(text)
        self.assertEqual(env["FOO"], "bar baz")
        self.assertEqual(env["BAZ"], "qux")

    def test_ignores_lines_without_equals(self):
        text = "FOO=bar\nnot_a_kv_line\nBAZ=qux\n"
        env = parse_env_file_from_string(text)
        self.assertEqual(env, {"FOO": "bar", "BAZ": "qux"})

    def test_missing_file_raises_clear_error(self):
        with self.assertRaises(FileNotFoundError) as cm:
            parse_env_file(Path("/nonexistent/path/.env.test"))
        self.assertIn(".env.test", str(cm.exception))


def parse_env_file_from_string(text: str) -> dict:
    """Test helper: writes text to a tempfile and parses it."""
    with tempfile.NamedTemporaryFile("w", delete=False, suffix=".env") as f:
        f.write(text)
        path = Path(f.name)
    try:
        return parse_env_file(path)
    finally:
        path.unlink()


from smoke import parse_keywords, log_contains_pattern, log_count_matching, strip_ansi


class StripAnsiTests(unittest.TestCase):
    def test_strips_csi_color_codes(self):
        # The exact byte shape tracing_subscriber emits — italics on
        # the field name, dim on the `=` separator, color on level.
        # If this regex stops matching, A1 silently flips back to
        # a false negative on every smoke run.
        raw = '\x1b[32m INFO\x1b[0m \x1b[3mpool\x1b[0m\x1b[2m=\x1b[0m"chat"'
        self.assertEqual(strip_ansi(raw), ' INFO pool="chat"')

    def test_passes_through_clean_text(self):
        clean = 'INFO LLM client initialised pool="chat" model_id=claude-opus-4-7'
        self.assertEqual(strip_ansi(clean), clean)

    def test_handles_multiline(self):
        raw = "\x1b[32mline 1\x1b[0m\n\x1b[31mline 2\x1b[0m\n"
        self.assertEqual(strip_ansi(raw), "line 1\nline 2\n")


class ParseKeywordsTests(unittest.TestCase):
    def test_extracts_keywords_block(self):
        text = (
            "=== transcript ===\n"
            "Hey everyone, welcome to the release sync.\n"
            "Engineering will draft the migration plan.\n"
            "\n"
            "=== keywords ===\n"
            "release sync\n"
            "engineering\n"
            "migration plan\n"
        )
        self.assertEqual(
            parse_keywords(text),
            ["release sync", "engineering", "migration plan"],
        )

    def test_skips_blank_lines_inside_block(self):
        text = "=== keywords ===\nrelease sync\n\nengineering\n"
        self.assertEqual(parse_keywords(text), ["release sync", "engineering"])

    def test_missing_keywords_block_raises(self):
        with self.assertRaises(ValueError):
            parse_keywords("no keywords here")


class LogPatternTests(unittest.TestCase):
    def test_contains_pattern_matches_substring(self):
        log = "INFO line 1\nINFO LLM client initialised pool=chat\nINFO done\n"
        self.assertTrue(log_contains_pattern(log, "LLM client initialised"))
        self.assertFalse(log_contains_pattern(log, "nonexistent string"))

    def test_count_matching_counts_lines(self):
        log = (
            "INFO LLM client initialised pool=chat\n"
            "INFO LLM client initialised pool=background\n"
            "INFO unrelated line\n"
        )
        self.assertEqual(log_count_matching(log, "LLM client initialised"), 2)
        self.assertEqual(log_count_matching(log, "unrelated"), 1)
        self.assertEqual(log_count_matching(log, "missing"), 0)

    def test_count_matching_handles_both_pools(self):
        log = (
            "INFO LLM client initialised pool=chat\n"
            "INFO LLM client initialised pool=background\n"
        )
        self.assertEqual(log_count_matching(log, 'pool="chat"'), 0)
        self.assertEqual(log_count_matching(log, "pool=chat"), 1)
        self.assertEqual(log_count_matching(log, "pool=background"), 1)


from smoke import llm_usage_api_ok


class LlmUsageApiOkTests(unittest.TestCase):
    def test_passes_with_calls_and_both_pools(self):
        ok, _ = llm_usage_api_ok(
            {
                "llm_usage": {"calls": 8},
                "llm_usage_by_pool": [
                    {"pool": "background", "calls": 5},
                    {"pool": "chat", "calls": 3},
                ],
            }
        )
        self.assertTrue(ok)

    def test_fails_on_the_pre_fix_regression_shape(self):
        # Exactly what the API returned before improvement #17: the
        # DB had pool rows (A7 green) but the endpoint read the dead
        # legacy columns → zero aggregate, no by-pool field.
        ok, detail = llm_usage_api_ok(
            {
                "llm_usage": {
                    "calls": 0,
                    "input_tokens": 0,
                    "output_tokens": 0,
                    "cached_input_tokens": 0,
                    "provider": None,
                    "model_id": None,
                }
            }
        )
        self.assertFalse(ok)
        self.assertIn("calls=0", detail)

    def test_fails_when_one_pool_missing(self):
        ok, _ = llm_usage_api_ok(
            {
                "llm_usage": {"calls": 3},
                "llm_usage_by_pool": [{"pool": "chat", "calls": 3}],
            }
        )
        self.assertFalse(ok)

    def test_fails_when_fields_absent_entirely(self):
        ok, _ = llm_usage_api_ok({})
        self.assertFalse(ok)


if __name__ == "__main__":
    unittest.main()
