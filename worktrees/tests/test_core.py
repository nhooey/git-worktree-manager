"""Tests of the pure core behind env, provision and clean: values in, values out, no files."""

from __future__ import annotations

import unittest

from worktrees import core


def lay(wt: str = "/w/repo.worktrees/a", main: bool = False) -> core.Layout:
    return core.layout(
        {"HOME": "/home/u"},
        "/w/repo/.git",
        "/w/repo/.git" if main else "/w/repo/.git/worktrees/a",
        "/w/repo" if main else wt,
    )


class BuildRoot(unittest.TestCase):
    def test_root_is_keyed_by_common_dir(self):
        root = core.repo_build_root({"HOME": "/home/u"}, "/w/repo/.git")
        self.assertEqual(root, "/home/u/.cache/worktree-builds/repo-" + core.sha8("/w/repo/.git"))

    def test_state_dir_is_keyed_the_same_way(self):
        self.assertEqual(
            core.repo_state_dir({"HOME": "/home/u"}, "/w/repo/.git"),
            "/home/u/.local/state/worktrees/repo-" + core.sha8("/w/repo/.git"),
        )
        self.assertTrue(
            core.repo_state_dir({"HOME": "/h", "XDG_STATE_HOME": "/s"}, "/w/r/.git").startswith(
                "/s/worktrees/r-"
            )
        )

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

    def test_locked_without_a_reason_is_still_locked(self):
        (_, x) = core.parse_worktree_list(
            "worktree /w/r\nHEAD a\n\nworktree /w/x\nHEAD b\nlocked\n"
        )
        self.assertEqual(x.locked, "")

    def test_pattern_file(self):
        self.assertEqual(
            core.parse_pattern_file("# c\n\nsecret.db\n  \n*.key \n"), ("secret.db", "*.key")
        )


class Clean(unittest.TestCase):
    def test_refusals(self):
        self.assertEqual(core.clean_refusals(True, False, ()), ("the main checkout",))
        self.assertEqual(
            core.clean_refusals(False, False, (" M a.txt", "?? new.txt")),
            ("1 tracked files with changes",),
        )
        self.assertEqual(core.clean_refusals(False, False, ("?? new.txt",)), ())

    def test_plan_spares_kept_files_and_nested_repositories(self):
        whole, mixed = core.clean_plan(
            ("build/", "junk.o", ".claude/", "nested/"),
            frozenset({".claude/settings.local.json"}),
            ("nested/",),
        )
        self.assertEqual(whole, ("build/", "junk.o"))
        self.assertEqual(mixed, (".claude/",))
        self.assertEqual(
            core.files_to_remove(
                (".claude/settings.local.json", ".claude/cache.txt", ".claude/x/"),
                frozenset({".claude/settings.local.json"}),
            ),
            (".claude/cache.txt",),
        )


class Formatting(unittest.TestCase):
    def test_human(self):
        self.assertEqual(core.human_bytes(2048), "2K")
        self.assertEqual(core.human_bytes(None), "?")
        self.assertEqual(core.human_age(90), "1m")
        self.assertEqual(core.human_age(2 * 86400), "2d")

    def test_merge_config_and_unknown_keys(self):
        merged = core.merge_config({"a": 1, "s": {"x": 1, "y": 2}}, {"s": {"y": 3}})
        self.assertEqual(merged, {"a": 1, "s": {"x": 1, "y": 3}})
        self.assertEqual(core.unknown_keys({"s": {"x": 1}}, {"s": {"z": 1}, "q": 2}), ("s.z", "q"))


if __name__ == "__main__":
    unittest.main()
