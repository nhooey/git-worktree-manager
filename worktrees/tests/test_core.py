"""Tests of the pure core: values in, values out, no files."""

from __future__ import annotations

import time
import unittest

from worktrees import core


def lay(wt: str = "/w/repo.worktrees/a", main: bool = False) -> core.Layout:
    return core.layout(
        {"HOME": "/home/u"},
        "/w/repo/.git",
        "/w/repo/.git" if main else "/w/repo/.git/worktrees/a",
        "/w/repo" if main else wt,
    )


def wt(**over) -> core.Worktree:
    base = core.Worktree("/w/a", "abc", "feat", False, False, None, False, False)
    return base._replace(**over)


def facts(**over) -> core.Facts:
    base = core.Facts(
        worktree=wt(),
        changes=(),
        landed="main",
        own_commits=False,
        holder="main",
        stashes=0,
        unique_kept=(),
        lock="none",
        inside=(),
        fresh_transcript=False,
        protected=False,
        upstreams=("main",),
    )
    return base._replace(**over)


class BuildRoot(unittest.TestCase):
    def test_root_is_keyed_by_common_dir(self):
        root = core.repo_build_root({"HOME": "/home/u"}, "/w/repo/.git")
        self.assertEqual(root, "/home/u/.cache/worktree-builds/repo-" + core.sha8("/w/repo/.git"))

    def test_xdg_cache_home_wins(self):
        env = {"HOME": "/home/u", "XDG_CACHE_HOME": "/c"}
        self.assertTrue(core.repo_build_root(env, "/w/r/.git").startswith("/c/worktree-builds/r-"))

    def test_two_clones_of_one_name_differ(self):
        env = {"HOME": "/h"}
        self.assertNotEqual(
            core.repo_build_root(env, "/a/public/hyperscale-rs/.git"),
            core.repo_build_root(env, "/a/schnorrific/hyperscale-rs/.git"),
        )

    def test_bare_repository_name(self):
        self.assertEqual(core.repo_name("/srv/foo.git"), "foo")

    def test_wt_id(self):
        self.assertEqual(core.wt_id("/w/repo", True), "main")
        self.assertEqual(core.wt_id("/w/x/feat", False), "feat-" + core.sha8("/w/x/feat"))

    def test_main_checkout_layout(self):
        self.assertTrue(lay(main=True).is_main)
        self.assertTrue(lay(main=True).wt.endswith("/wt/main"))
        self.assertFalse(lay().is_main)


class Languages(unittest.TestCase):
    def test_detect(self):
        self.assertEqual(
            core.detect_languages(("Cargo.toml", "pyproject.toml", "x.tf")),
            ("rust", "python", "terraform"),
        )
        self.assertEqual(core.detect_languages(("App.xcodeproj",)), ("xcode",))
        self.assertEqual(core.detect_languages(("README.md",)), ())

    def test_parse_with_aliases_and_unknown(self):
        self.assertEqual(
            core.parse_languages("rust, kotlin,java tofu"), (("rust", "gradle", "terraform"), ())
        )
        self.assertEqual(core.parse_languages("cobol,rust"), (("rust",), ("cobol",)))

    def test_rust_variables(self):
        got = dict(core.wanted_variables(("rust",), lay(), "linux"))
        self.assertEqual(got["CARGO_BUILD_BUILD_DIR"], lay().root + "/shared/cargo-build")
        self.assertEqual(got["CARGO_TARGET_DIR"], lay().wt + "/target")
        self.assertIn("WORKTREE_BUILDS_ROOT", got)

    def test_uv_link_mode_only_on_darwin(self):
        self.assertIn("UV_LINK_MODE", dict(core.wanted_variables(("python",), lay(), "darwin")))
        self.assertNotIn("UV_LINK_MODE", dict(core.wanted_variables(("python",), lay(), "linux")))


class EnvPlan(unittest.TestCase):
    def wanted(self, layout: core.Layout = None):
        return core.wanted_variables(("rust",), layout or lay(), "linux")

    def test_fresh_environment_exports_everything(self):
        plan = core.env_plan(self.wanted(), ("rust",), {})
        names = dict(plan.exports)
        self.assertEqual(names["CARGO_TARGET_DIR"], lay().wt + "/target")
        self.assertEqual(
            names[core.SET_VAR],
            "CARGO_BUILD_BUILD_DIR:CARGO_TARGET_DIR:WORKTREE_BUILDS_ROOT:WORKTREE_BUILDS_WT",
        )

    def test_a_persons_variable_is_kept(self):
        plan = core.env_plan(self.wanted(), ("rust",), {"CARGO_TARGET_DIR": "/mine"})
        self.assertNotIn("CARGO_TARGET_DIR", dict(plan.exports))
        self.assertNotIn("CARGO_TARGET_DIR", dict(plan.exports)[core.SET_VAR])

    def test_inherited_from_another_worktree_is_recomputed(self):
        other = lay("/w/repo.worktrees/b")
        first = dict(core.env_plan(self.wanted(other), ("rust",), {}).exports)
        plan = core.env_plan(self.wanted(), ("rust",), first)
        self.assertEqual(dict(plan.exports)["CARGO_TARGET_DIR"], lay().wt + "/target")

    def test_same_worktree_leaves_its_own_alone(self):
        first = dict(core.env_plan(self.wanted(), ("rust",), {}).exports)
        changed = {**first, "CARGO_TARGET_DIR": "/changed"}
        plan = core.env_plan(self.wanted(), ("rust",), changed)
        self.assertNotIn("CARGO_TARGET_DIR", dict(plan.exports))
        self.assertIn("CARGO_TARGET_DIR", dict(plan.exports)[core.SET_VAR])

    def test_names_no_longer_wanted_are_unset(self):
        other = lay("/w/repo.worktrees/b")
        first = dict(
            core.env_plan(
                core.wanted_variables(("python",), other, "linux"), ("python",), {}
            ).exports
        )
        plan = core.env_plan(self.wanted(), ("rust",), first)
        self.assertIn("RUFF_CACHE_DIR", plan.unsets)

    def test_gradle_flag_is_appended_once(self):
        wanted = core.wanted_variables(("gradle",), lay(), "linux")
        plan = core.env_plan(wanted, ("gradle",), {"GRADLE_OPTS": "-Xmx2g"})
        self.assertEqual(dict(plan.exports)["GRADLE_OPTS"], "-Xmx2g " + core.GRADLE_CACHING)
        again = core.env_plan(wanted, ("gradle",), {"GRADLE_OPTS": "-Xmx2g " + core.GRADLE_CACHING})
        self.assertNotIn("GRADLE_OPTS", dict(again.exports))

    def test_render_quotes(self):
        plan = core.EnvPlan((("A", "it's"),), ("B",))
        self.assertEqual(core.render_env(plan, "sh"), "unset B\nexport A='it'\\''s'\n")
        self.assertEqual(core.render_env(plan, "fish"), "set -e B\nset -gx A 'it\\'s'\n")


class Provisioning(unittest.TestCase):
    def test_copy_and_allow(self):
        self.assertEqual(
            core.provision_actions(False, False, True, b"use flake\n", b"use flake\n", True),
            ("copy-settings", "direnv-allow"),
        )

    def test_differing_envrc_is_not_allowed(self):
        self.assertEqual(core.provision_actions(False, True, True, b"a", b"b", True), ())

    def test_main_checkout_needs_nothing(self):
        self.assertEqual(core.provision_actions(True, False, True, b"a", b"a", True), ())


class Parsing(unittest.TestCase):
    def test_worktree_list(self):
        text = (
            "worktree /w/repo\nHEAD aaa\nbranch refs/heads/main\n\n"
            "worktree /w/x\nHEAD bbb\ndetached\nlocked claude agent a (pid 1 start x)\n\n"
            "worktree /w/y\nHEAD ccc\nbranch refs/heads/y\nprunable gitdir file points to non-existent location\n"
        )
        main, x, y = core.parse_worktree_list(text)
        self.assertTrue(main.is_main)
        self.assertEqual(main.branch, "main")
        self.assertTrue(x.detached)
        self.assertEqual(x.locked, "claude agent a (pid 1 start x)")
        self.assertTrue(y.prunable)
        self.assertFalse(y.is_main)

    def test_cherry(self):
        self.assertTrue(core.cherry_all_upstream("- abc\n- def\n"))
        self.assertFalse(core.cherry_all_upstream("- abc\n+ def\n"))

    def test_stash_branches(self):
        self.assertEqual(core.stash_branches("WIP on feat: 1 x\nOn main: y\n"), ("feat", "main"))

    def test_lsof(self):
        text = "p10\nR1\nn/w/a\np20\nR99\nn/w/b\np30\nR1\nn/w/c\n"
        self.assertEqual(core.parse_lsof_cwd(text, 99), ((10, "/w/a"), (30, "/w/c")))

    def test_ps(self):
        rows = core.parse_ps("  10     1 Tue Nov 17 03:42:00 2026\n")
        self.assertEqual((rows[0].pid, rows[0].ppid), (10, 1))
        self.assertIsNotNone(rows[0].start)

    def test_df(self):
        text = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk3s5 100 50 40 56% /System/Volumes/Data\n"
        self.assertEqual(core.parse_df(text), 40 * 1024)

    def test_patterns(self):
        self.assertEqual(
            core.parse_pattern_file("# c\n\n.env\n  \nstate/tfplan\n"), (".env", "state/tfplan")
        )


class Locks(unittest.TestCase):
    start = "Tue Nov 17 03:42:00 2026"

    def table(self, start: str):
        return {42: core.Process(42, 1, core.parse_lstart(start))}

    def test_kinds(self):
        reason = "claude agent agent-1 (pid 42 start {})".format(self.start)
        self.assertEqual(core.classify_lock(None, {}), "none")
        self.assertEqual(core.classify_lock("on a USB drive", {}), "person")
        self.assertEqual(core.classify_lock(reason, self.table(self.start)), "claude-live")
        self.assertEqual(core.classify_lock(reason, {}), "claude-stale")
        self.assertEqual(
            core.classify_lock(reason, self.table("Tue Nov 17 05:00:00 2026")), "claude-stale"
        )

    def test_unreadable_start_is_live(self):
        reason = "claude agent agent-1 (pid 42 start sometime)"
        self.assertEqual(core.classify_lock(reason, self.table(self.start)), "claude-live")

    def test_start_formats(self):
        epoch = time.time()
        self.assertAlmostEqual(core.parse_start(str(int(epoch * 1000))), epoch, delta=1)
        self.assertEqual(core.parse_start("1970-01-01T00:00:10Z"), 10)

    def test_ancestors_and_inside(self):
        self.assertEqual(core.ancestors(5, {5: 4, 4: 1}), frozenset({5, 4}))
        cwds = ((1, "/w/a"), (2, "/w/a/src"), (3, "/w/ab"), (4, "/w/a"))
        self.assertEqual(core.pids_inside(cwds, "/w/a", frozenset({4})), (1, 2))


class Decisions(unittest.TestCase):
    def test_landed_clean_tree_finishes(self):
        self.assertEqual(core.finish_verdict(facts(), False, None).action, "finish")

    def test_unlanded_is_kept_with_its_reason(self):
        verdict = core.finish_verdict(facts(landed=None, own_commits=True), False, None)
        self.assertEqual(verdict.action, "keep")
        self.assertIn("branch has not landed in main", verdict.reasons)

    def test_failed_run_with_log(self):
        base = facts(landed=None, own_commits=False)
        self.assertEqual(core.finish_verdict(base, True, None).action, "finish")
        self.assertEqual(core.finish_verdict(base, False, None).action, "keep")
        self.assertIn("log: gone", core.finish_verdict(base, True, "gone").reasons)
        self.assertIn(
            "branch has commits of its own",
            core.finish_verdict(base._replace(own_commits=True), True, None).reasons,
        )

    def test_kept_for_each_rule(self):
        for over in (
            {"changes": (" M a",)},
            {"stashes": 1},
            {"unique_kept": (".env",)},
            {"lock": "person"},
            {"lock": "claude-live"},
            {"inside": (7,)},
            {"inside": core.UNKNOWN_INSIDE},
            {"protected": True},
            {"worktree": wt(is_main=True)},
        ):
            self.assertEqual(core.finish_verdict(facts(**over), False, None).action, "keep", over)

    def test_stale_lock_unlocks_then_finishes(self):
        verdict = core.finish_verdict(facts(lock="claude-stale"), False, None)
        self.assertEqual((verdict.action, verdict.unlock), ("finish", True))

    def test_clean_refusals(self):
        self.assertEqual(core.clean_reasons(facts(changes=("?? new",))), ())
        self.assertTrue(core.clean_reasons(facts(changes=(" M a",))))
        self.assertTrue(core.clean_reasons(facts(worktree=wt(is_main=True))))
        self.assertTrue(core.clean_reasons(facts(inside=(9,))))

    def test_sweep(self):
        self.assertEqual(core.sweep_verdict(facts()).action, "finish")
        self.assertEqual(core.sweep_verdict(facts(landed=None, own_commits=True)).action, "clean")
        self.assertEqual(core.sweep_verdict(facts(landed=None, changes=(" M a",))).action, "keep")
        self.assertEqual(core.sweep_verdict(facts(fresh_transcript=True)).action, "skip")
        self.assertEqual(core.sweep_verdict(facts(worktree=wt(is_main=True))).action, "skip")


class Clean(unittest.TestCase):
    def test_plan(self):
        entries = (".claude/", ".env", "a.o", "build/", "x/")
        kept = frozenset({".claude/settings.local.json", ".env"})
        whole, mixed = core.clean_plan(entries, kept, ("build/nested/",))
        self.assertEqual(whole, ("a.o", "x/"))
        self.assertEqual(mixed, (".claude/", "build/"))

    def test_files_to_remove(self):
        self.assertEqual(
            core.files_to_remove(
                (".claude/a", ".claude/settings.local.json", "sub/"),
                frozenset({".claude/settings.local.json"}),
            ),
            (".claude/a",),
        )

    def test_orphans(self):
        entries = (("/r/wt/a", "/w/a"), ("/r/wt/b", "/w/b"), ("/r/wt/c", None))
        self.assertEqual(core.orphan_wt_dirs(entries, frozenset({"/w/a"})), ("/r/wt/b",))


class Floors(unittest.TestCase):
    def test_defaults_file_and_env(self):
        self.assertEqual(core.parse_floors("", {}), core.Floors(70.0, 35.0))
        text = "WORKTREES_FLOOR_BUILD_GB=80\nWORKTREES_FLOOR_CLEANUP_ONLY_GB=40\n"
        self.assertEqual(core.parse_floors(text, {}), core.Floors(80.0, 40.0))
        self.assertEqual(core.parse_floors(text, {"WORKTREES_FLOOR_BUILD_GB": "90"}).build_gb, 90.0)

    def test_state(self):
        floors = core.Floors(70.0, 35.0)
        self.assertEqual(core.floor_state(80 * 10**9, floors), "ok")
        self.assertEqual(core.floor_state(60 * 10**9, floors), "below-build-floor")
        self.assertEqual(core.floor_state(30 * 10**9, floors), "below-cleanup-floor")


class Config(unittest.TestCase):
    def test_merge_and_unknown(self):
        defaults = {"a": 1, "s": {"apply": False, "x": 2}, "keep": ["a"]}
        user = {"s": {"apply": True}, "keep": ["b"], "zz": 1}
        self.assertEqual(
            core.merge_config(defaults, user),
            {"a": 1, "s": {"apply": True, "x": 2}, "keep": ["b"], "zz": 1},
        )
        self.assertEqual(core.unknown_keys(defaults, user), ("zz",))


if __name__ == "__main__":
    unittest.main()
