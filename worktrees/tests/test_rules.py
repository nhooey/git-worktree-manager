"""Tests of the sweep's pure rules: values in, values out, no files."""

from __future__ import annotations

import time
import unittest

from worktrees import rules

SETTINGS = rules.Settings(sweep="report", idle=3600.0, lock_report=86400.0, archive_push=False)
REF = "refs/remotes/origin/main"


def wt(**over) -> rules.WorktreeFacts:
    base = rules.WorktreeFacts(
        path="/w/repo.worktrees/feat",
        branch="feat",
        head="a" * 40,
        is_main=False,
        exists=True,
        lock=None,
        landed=True,
        changes=(),
        kept=(),
        cleanable=(),
        idle=7200.0,
    )
    return base._replace(**over)


def branch(**over) -> rules.BranchFacts:
    base = rules.BranchFacts(
        name="feat",
        tip="a" * 40,
        upstream=None,
        track="",
        landed=False,
        content_landed=False,
        age=2 * 86400.0,
        archived=False,
        is_default=False,
    )
    return base._replace(**over)


def lock(
    reason: str = "neil since 2026-11-18T09:12:00Z", age: float = 10.0, alive=None
) -> rules.LockFacts:
    return rules.LockFacts(rules.parse_lock(reason), age, alive, None)


def verdict(f: rules.WorktreeFacts, state=None, problem=None) -> rules.WorktreeVerdict:
    return rules.worktree_verdict(f, state, REF, problem, SETTINGS)


def codes(v) -> tuple:
    return tuple(r.code for r in v.reasons)


class Times(unittest.TestCase):
    def test_durations(self):
        self.assertEqual(rules.parse_duration("1h"), 3600)
        self.assertEqual(rules.parse_duration("1d"), 86400)
        self.assertEqual(rules.parse_duration("30m"), 1800)
        self.assertEqual(rules.parse_duration("0s"), 0)
        self.assertEqual(rules.parse_duration("90"), 90)
        self.assertIsNone(rules.parse_duration("soon"))

    def test_times(self):
        self.assertEqual(rules.parse_time("2026-11-18T09:12:00Z"), 1794993120.0)
        self.assertEqual(rules.parse_time("1794993120000"), 1794993120.0)
        self.assertEqual(rules.parse_time("1794993120"), 1794993120.0)
        self.assertEqual(rules.parse_time("2026-11-18T09:12:00"), 1794993120.0)
        local = time.mktime(time.strptime("Sat Nov 14 16:15:55 2026", rules.LSTART_FORMAT))
        self.assertEqual(rules.parse_time("Sat Nov 14 16:15:55 2026"), local)
        self.assertIsNone(rules.parse_time("whenever"))

    def test_archive_refs_are_unique(self):
        self.assertEqual(
            rules.archive_ref("a/b", 1794993120.0), "refs/archive/a/b/2026-11-18T091200Z"
        )
        self.assertEqual(rules.unique_refs(("r", "r", "s"), frozenset({"s"})), ("r", "r-2", "s-2"))

    def test_settings_from_git_config(self):
        defaults = {"sweep": "report", "idle": "1h", "lockReport": "1d", "archivePush": False}
        s = rules.settings(
            defaults, rules.parse_config("worktrees.idle 0s\nworktrees.archivepush true\n")
        )
        self.assertEqual(s, rules.Settings("report", 0.0, 86400.0, True))
        self.assertEqual(rules.settings(defaults, {"worktrees.sweep": "apply"}).sweep, "apply")
        self.assertEqual(rules.settings(defaults, {"worktrees.idle": "junk"}).idle, 3600.0)


class Locks(unittest.TestCase):
    def test_since_form(self):
        parsed = rules.parse_lock("headless overnight2-03 pid 4411 since 2026-11-18T09:12:00Z")
        self.assertEqual(parsed.form, "since")
        self.assertEqual(parsed.pid, 4411)
        self.assertEqual(parsed.since, 1794993120.0)
        self.assertEqual(rules.parse_lock("neil since 2026-11-18T09:12:00Z").pid, None)

    def test_claude_form(self):
        parsed = rules.parse_lock("claude agent agent-a1b2 (pid 93171 start 1794993120000)")
        self.assertEqual(
            (parsed.form, parsed.pid, parsed.since), ("claude-agent", 93171, 1794993120.0)
        )
        self.assertEqual(parsed.who, "claude agent agent-a1b2")

    def test_other_form(self):
        self.assertEqual(rules.parse_lock("busy").form, "other")
        self.assertEqual(rules.parse_lock("").reason, "")

    def test_alive(self):
        claude = rules.parse_lock("claude agent agent-a (pid 1 start 1000)")
        self.assertTrue(rules.process_alive(claude, 1000.5))
        self.assertFalse(rules.process_alive(claude, 2000.0), "a pid reused by a later process")
        self.assertFalse(rules.process_alive(claude, None))
        since = rules.parse_lock("w pid 1 since 1970-01-01T00:16:40Z")
        self.assertTrue(rules.process_alive(since, 900.0))
        self.assertFalse(rules.process_alive(since, 1100.0))
        self.assertIsNone(
            rules.process_alive(rules.parse_lock("neil since 2026-11-18T09:12:00Z"), None)
        )


class Patterns(unittest.TestCase):
    def patterns(self, *texts):
        return tuple(rules.parse_pattern(t) for t in texts)

    def test_a_name_matches_at_any_depth(self):
        p = self.patterns(".env", "*.local.*")
        self.assertEqual(
            rules.kept_entries((".env", "a/.env", "x.local.toml", "build/"), p),
            (".env", "a/.env", "x.local.toml"),
        )

    def test_folders_and_anchors(self):
        p = self.patterns(".superpowers/", ".claude/settings.local.json")
        self.assertEqual(
            rules.kept_entries(
                (
                    ".superpowers/",
                    ".superpowers",
                    "a/.superpowers/x",
                    ".claude/settings.local.json",
                    "b/.claude/settings.local.json",
                ),
                p,
            ),
            (".superpowers/", "a/.superpowers/x", ".claude/settings.local.json"),
        )

    def test_probes_inside_ignored_folders(self):
        p = self.patterns(".claude/settings.local.json", ".env", ".idea/")
        self.assertEqual(
            rules.probes((".claude/", ".idea/", "target/"), p), (".claude/settings.local.json",)
        )


class Worktrees(unittest.TestCase):
    def test_removable(self):
        v = verdict(wt())
        self.assertEqual((v.action, v.reasons), ("remove", ()))
        self.assertTrue(all(t.ok for t in v.tests))

    def test_each_rule_refuses_on_its_own(self):
        cases = (
            (wt(is_main=True), "main-worktree"),
            (wt(lock=lock()), "locked"),
            (wt(landed=False), "not-landed"),
            (wt(changes=(" M a.txt",)), "uncommitted"),
            (wt(kept=(".env",)), "keep-pattern-files"),
            (wt(idle=60.0), "not-idle"),
        )
        for facts, code in cases:
            v = verdict(facts, state="in-review")
            self.assertEqual((v.action, codes(v)), ("keep", (code,)), code)

    def test_no_default_branch(self):
        v = verdict(wt(landed=None), problem="no default branch")
        self.assertEqual(codes(v), ("no-default-branch",))

    def test_content_landed_and_previews_count_as_landed(self):
        self.assertEqual(verdict(wt(landed=False), state="content-landed").action, "remove")
        self.assertEqual(verdict(wt(landed=False), state="preview").action, "remove")

    def test_a_missing_folder_is_pruned_unless_locked(self):
        self.assertEqual(verdict(wt(exists=False)).action, "prune")
        locked = verdict(wt(exists=False, lock=lock()))
        self.assertEqual((locked.action, codes(locked)), ("keep", ("folder-missing", "locked")))

    def test_an_unlanded_worktree_idle_a_day_is_cleaned(self):
        idle = wt(landed=False, idle=2 * 86400.0, cleanable=("target/",), changes=("?? x",))
        self.assertEqual(verdict(idle, state="local-only").action, "clean")
        self.assertEqual(verdict(idle._replace(lock=lock()), state="local-only").action, "keep")
        self.assertEqual(
            verdict(idle._replace(changes=(" M a",)), state="local-only").action, "keep"
        )
        self.assertEqual(verdict(idle._replace(cleanable=()), state="local-only").action, "keep")

    def test_a_landed_lock_held_past_the_limit_is_reported(self):
        self.assertTrue(verdict(wt(lock=lock(age=2 * 86400.0))).lock_report)
        self.assertFalse(verdict(wt(lock=lock(age=60.0))).lock_report)
        self.assertFalse(verdict(wt(landed=False, lock=lock(age=2 * 86400.0))).lock_report)


class Branches(unittest.TestCase):
    def state(self, **over) -> str:
        return rules.branch_state(branch(**over), True)

    def test_the_table(self):
        self.assertEqual(self.state(is_default=True), "default-branch")
        self.assertEqual(rules.branch_state(branch(), False), "no-default-branch")
        self.assertEqual(self.state(landed=True), "landed")
        self.assertEqual(self.state(name="preview/x"), "preview")
        self.assertEqual(self.state(content_landed=True), "content-landed")
        self.assertEqual(
            self.state(upstream="refs/remotes/origin/feat", track="gone"), "upstream-gone"
        )
        self.assertEqual(
            self.state(upstream="refs/remotes/origin/feat", track="ahead 1, behind 2"), "diverged"
        )
        self.assertEqual(
            self.state(upstream="refs/remotes/origin/feat", track="ahead 1"), "in-review"
        )
        self.assertEqual(self.state(), "local-only")

    def test_actions(self):
        def action(survives=(), **over):
            return rules.branch_verdict(branch(**over), True, REF, None, None, survives).action

        self.assertEqual(action(landed=True), "delete")
        self.assertEqual(action(name="preview/x"), "archive+delete")
        self.assertEqual(action(("pushed as origin/preview/x at aaa",), name="preview/x"), "delete")
        self.assertEqual(action(content_landed=True), "archive+delete")
        self.assertEqual(
            action(("identical to refs/archive/feat/t",), content_landed=True), "delete"
        )
        self.assertEqual(action(), "keep")
        self.assertEqual(action(("pushed as origin/feat at aaa",)), "keep")

    def test_where_a_tip_survives(self):
        refs = (
            ("refs/heads/feat", "t"),
            ("refs/remotes/origin/feat", "t"),
            ("refs/remotes/origin/HEAD", "t"),
            ("refs/tags/v1", "t"),
            ("refs/heads/going", "t"),
            ("refs/heads/other", "u"),
        )
        self.assertEqual(
            rules.elsewhere(
                "t", "refs/heads/feat", refs, True, REF, frozenset({"refs/heads/going"})
            ),
            ("merged into origin/main", "pushed as origin/feat at t", "identical to refs/tags/v1"),
        )
        self.assertEqual(
            rules.elsewhere("u", "refs/heads/other", refs, False, REF, frozenset()), ()
        )

    def test_a_branch_in_a_kept_worktree_stays(self):
        kept = rules.branch_verdict(branch(landed=True), True, REF, "/w/x", "keep")
        self.assertEqual(
            (kept.action, tuple(r.code for r in kept.reasons)), ("keep", ("checked-out",))
        )
        freed = rules.branch_verdict(branch(landed=True), True, REF, "/w/x", "remove")
        self.assertEqual(freed.action, "delete")


class ContentLanded(unittest.TestCase):
    def test_raw_diff_and_tree(self):
        raw = ":100644 100644 aaa bbb M\x00a.txt\x00:100644 000000 ccc 000000 D\x00gone.txt\x00"
        changes = rules.parse_raw_diff(raw)
        self.assertEqual(changes, (("a.txt", "100644", "bbb"), ("gone.txt", "000000", "000000")))
        tree = rules.parse_ls_tree("100644 blob bbb\ta.txt\x00100644 blob ddd\tother.txt\x00")
        self.assertTrue(rules.changes_present(changes, tree))
        self.assertFalse(rules.changes_present(changes, {**tree, "gone.txt": ("100644", "ccc")}))
        self.assertFalse(rules.changes_present(changes, {"a.txt": ("100644", "aaa")}))


class Log(unittest.TestCase):
    def test_rescue_takes_the_tip_from_the_deletion_or_the_last_value(self):
        zero = "0" * 40
        events = (
            {"event": "ref-update", "ref": "refs/heads/a", "new": "1" * 40},
            {"event": "ref-update", "ref": "refs/heads/a", "old": "1" * 40, "new": "2" * 40},
            {"event": "ref-update", "ref": "refs/heads/a", "new": zero},
            {"event": "ref-update", "ref": "refs/heads/b", "old": "3" * 40, "new": zero},
            {"event": "snapshot", "ref": "refs/heads/c", "new": "4" * 40},
            {"event": "ref-update", "ref": "refs/heads/c", "new": zero},
            {"event": "ref-update", "ref": "refs/heads/preview/p", "old": "5" * 40, "new": zero},
            {"event": "ref-update", "ref": "refs/heads/never", "new": zero},
            {"event": "ref-update", "ref": "refs/remotes/origin/x", "old": "6" * 40, "new": zero},
        )
        self.assertEqual(
            rules.rescue_candidates(events),
            (("a", "2" * 40), ("b", "3" * 40), ("c", "4" * 40), ("preview/p", "5" * 40)),
        )

    def test_backfill_only_once(self):
        self.assertFalse(rules.has_snapshot(({"event": "remove"},)))
        self.assertTrue(rules.has_snapshot(({"event": "snapshot"},)))


class Plan(unittest.TestCase):
    def repo(self, **over) -> rules.RepoFacts:
        base = rules.RepoFacts(
            main="/w/repo",
            common_dir="/w/repo/.git",
            state_dir="/s",
            default_branch="main",
            default_ref=REF,
            default_problem=None,
            settings=SETTINGS,
            worktrees=(wt(is_main=True, path="/w/repo", branch="main"), wt()),
            branches=(
                branch(name="main", is_default=True, landed=True),
                branch(name="feat", landed=True),
                branch(name="rebased", content_landed=True, tip="b" * 40),
                branch(name="open", upstream="refs/remotes/origin/open", tip="c" * 40),
            ),
            archives=(("refs/archive/old/2026-01-01T000000Z", "d" * 40),),
            merged=frozenset({"refs/archive/old/2026-01-01T000000Z"}),
            events=(),
            rescues=(("doomed", "e" * 40),),
        )
        return base._replace(**over)

    def test_backfill_archives_every_unlanded_branch_first(self):
        p = rules.plan(self.repo(), 1794993120.0)
        self.assertTrue(p.backfill)
        self.assertEqual(p.backfill_archives, ("rebased", "open"))
        self.assertEqual(p.archives["rebased"], "refs/archive/rebased/2026-11-18T091200Z")
        self.assertEqual([v.action for v in p.worktrees], ["keep", "remove"])
        self.assertEqual(
            [v.action for v in p.branches], ["keep", "delete", "archive+delete", "keep"]
        )
        self.assertEqual(p.archive_prunes, (("refs/archive/old/2026-01-01T000000Z", "d" * 40),))
        self.assertEqual(
            p.rescues, (("doomed", "e" * 40, "refs/archive/doomed/2026-11-18T091200Z"),)
        )
        self.assertEqual(len(p.snapshots), 2 + 4)

    def test_later_sweeps_snapshot_only_branches_missing_from_the_log(self):
        events = (
            {"event": "snapshot", "ref": "refs/heads/feat"},
            {"event": "snapshot", "ref": "refs/heads/main"},
        )
        p = rules.plan(self.repo(events=events), 1794993120.0)
        self.assertFalse(p.backfill)
        self.assertEqual(p.backfill_archives, ())
        self.assertEqual(
            sorted(s["ref"] for s in p.snapshots), ["refs/heads/open", "refs/heads/rebased"]
        )
        self.assertEqual(p.archives, {"rebased": "refs/archive/rebased/2026-11-18T091200Z"})

    def test_no_default_branch_plans_nothing_but_rescues(self):
        p = rules.plan(
            self.repo(default_problem="no default branch", worktrees=(wt(landed=None),)), 0.0
        )
        self.assertFalse(p.backfill)
        self.assertEqual(p.snapshots, ())
        self.assertEqual(p.archive_prunes, ())
        self.assertTrue(all(v.action == "keep" for v in p.worktrees + p.branches))
        self.assertEqual(len(p.rescues), 1)


if __name__ == "__main__":
    unittest.main()
