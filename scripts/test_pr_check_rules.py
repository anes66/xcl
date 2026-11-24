"""Tests of scripts/pr_check.py – the merge gate's rules, without GitHub or git:

    python -m unittest discover -s scripts -p "test_pr_check*.py"
"""
import os, sys, unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pr_check  # noqa: E402

HEAD = "0123456789abcdef0123456789abcdef01234567"
MAIN = "fedcba9876543210fedcba9876543210fedcba98"

SECTIONS = """
**Tested**
- RX 9070, `cargo test --workspace -- --test-threads=4`: 700 passed, 0 failed

**Not tested**
- NVIDIA

**AI use**
- Claude Code, Claude Opus 5.5: wrote the tests
"""


def smoke(commit, title="Game", seconds="120", exit="0x0", frames="1000 (PresentX 1000)",
          removals="0", crashes="0", peak="8000"):
    return (f"```\nxcl-smoke v1\ncommit: {commit}\ntitle: {title}\nseconds: {seconds}\nexit: {exit}\n"
            f"frames shown: {frames}\ndevice removals: {removals}\ncrashes: {crashes}\n"
            f"peak commit: {peak} MB\n```\n")


def file(path, status="modified", binary=False, size=100):
    return (path, status, binary, lambda: size)


def on_main(sha):
    return sha == MAIN[:len(sha)]


def check(body, files):
    return pr_check.check(body, HEAD, files, on_main)


RUNTIME = [file("crates/xcl/src/vfs.rs")]


class Sections(unittest.TestCase):
    def test_a_complete_description_passes(self):
        self.assertEqual(check(SECTIONS, [file("README.md")]), [])

    def test_each_missing_section_is_named(self):
        problems = check("Just a fix.", [file("README.md")])
        for section in ("Tested", "Not tested", "AI use"):
            self.assertTrue(any(f"**{section}**" in p for p in problems), section)

    def test_windows_line_endings_are_read(self):
        self.assertEqual(check(SECTIONS.replace("\n", "\r\n"), [file("README.md")]), [])


class Models(unittest.TestCase):
    def test_an_open_weight_model_in_ai_use_is_refused(self):
        body = SECTIONS.replace("Claude Code, Claude Opus 5.5", "Cline with Qwen3 32B")
        problems = check(body, [file("README.md")])
        self.assertTrue(any("qwen" in p for p in problems), problems)

    def test_a_model_named_outside_ai_use_is_not_refused(self):
        body = "Fixes a crash seen with a Llama-based launcher.\n" + SECTIONS
        self.assertEqual(check(body, [file("README.md")]), [])


class GameData(unittest.TestCase):
    def test_game_archives_and_executables_are_refused(self):
        for path in ("data/shaders.rpf", "game.exe", "movie.bk2", "dump.dmp"):
            self.assertTrue(check(SECTIONS, [file(path)]), path)

    def test_a_game_config_is_refused_except_the_test_titles(self):
        self.assertTrue(check(SECTIONS, [file("somewhere/MicrosoftGame.config")]))
        self.assertEqual(check(SECTIONS, [file("tests/data/testgame/MicrosoftGame.config")]), [])

    def test_small_test_data_is_allowed_and_large_is_not(self):
        self.assertEqual(check(SECTIONS, [file("tests/data/x.bin", binary=True, size=1000)]), [])
        self.assertTrue(check(SECTIONS, [file("tests/data/x.bin", binary=True, size=2 << 20)]))

    def test_a_binary_outside_test_data_is_refused(self):
        self.assertTrue(check(SECTIONS, [file("docs/picture.png", binary=True)]))

    def test_a_removed_file_is_not_checked(self):
        self.assertEqual(check(SECTIONS, [file("game.exe", status="removed")]), [])


class Runtime(unittest.TestCase):
    def test_what_counts_as_runtime(self):
        self.assertTrue(pr_check.runtime("crates/xcl/src/vfs.rs"))
        self.assertTrue(pr_check.runtime("crates/xcl/src/hle/d3d12x/real.rs"))
        self.assertFalse(pr_check.runtime("crates/xcl/src/gpu/recomp/tests.rs"))
        self.assertFalse(pr_check.runtime("crates/xcl/tests/e2e.rs"))
        self.assertFalse(pr_check.runtime("scripts/smoke.ps1"))
        self.assertFalse(pr_check.runtime("README.md"))

    def test_a_runtime_change_without_regression_runs_is_refused(self):
        problems = check(SECTIONS, RUNTIME)
        self.assertTrue(any(HEAD[:12] in p for p in problems), problems)
        self.assertTrue(any("commit of main" in p for p in problems), problems)

    def test_a_runtime_change_with_both_runs_passes(self):
        body = SECTIONS + smoke(MAIN) + smoke(HEAD)
        self.assertEqual(check(body, RUNTIME), [])

    def test_a_short_commit_names_the_head(self):
        body = SECTIONS + smoke(MAIN[:12]) + smoke(HEAD[:12])
        self.assertEqual(check(body, RUNTIME), [])

    def test_a_run_with_uncommitted_changes_does_not_count(self):
        body = SECTIONS + smoke(MAIN) + smoke(HEAD + " (uncommitted changes)")
        self.assertTrue(any("no uncommitted changes" in p for p in check(body, RUNTIME)))

    def test_the_base_run_must_be_on_main(self):
        other = "1111111111111111111111111111111111111111"
        body = SECTIONS + smoke(other) + smoke(HEAD)
        self.assertTrue(any("commit of main" in p for p in check(body, RUNTIME)))

    def test_device_removals_and_crashes_in_the_prs_run_are_refused(self):
        for key, kwargs in (("device removals", {"removals": "1"}), ("crashes", {"crashes": "2"})):
            body = SECTIONS + smoke(MAIN) + smoke(HEAD, **kwargs)
            self.assertTrue(any(key in p for p in check(body, RUNTIME)), key)

    def test_a_crash_in_mains_run_is_allowed(self):
        body = SECTIONS + smoke(MAIN, crashes="1") + smoke(HEAD)
        self.assertEqual(check(body, RUNTIME), [])

    def test_a_killed_run_is_refused(self):
        body = SECTIONS + smoke(MAIN) + smoke(HEAD, exit="killed (no exit)")
        self.assertTrue(any("killed" in p for p in check(body, RUNTIME)))

    def test_runs_of_different_titles_or_times_are_refused(self):
        for kwargs in ({"title": "Other"}, {"seconds": "60"}):
            body = SECTIONS + smoke(MAIN) + smoke(HEAD, **kwargs)
            self.assertTrue(any("same title" in p for p in check(body, RUNTIME)), kwargs)

    def test_frames_below_80_percent_of_main_are_refused(self):
        ok = SECTIONS + smoke(MAIN, frames="1000") + smoke(HEAD, frames="800")
        self.assertEqual(check(ok, RUNTIME), [])
        low = SECTIONS + smoke(MAIN, frames="1000") + smoke(HEAD, frames="799")
        self.assertTrue(any("fewer frames" in p for p in check(low, RUNTIME)))

    def test_a_run_that_logged_no_frames_is_refused(self):
        body = SECTIONS + smoke(MAIN) + smoke(HEAD, frames="none logged (the run did not reach its end)")
        self.assertTrue(any("fewer frames" in p for p in check(body, RUNTIME)))

    def test_peak_commit_above_125_percent_of_main_is_refused(self):
        ok = SECTIONS + smoke(MAIN, peak="8000") + smoke(HEAD, peak="10000")
        self.assertEqual(check(ok, RUNTIME), [])
        high = SECTIONS + smoke(MAIN, peak="8000") + smoke(HEAD, peak="10001")
        self.assertTrue(any("more memory" in p for p in check(high, RUNTIME)))


if __name__ == "__main__":
    unittest.main()
