"""Tests of env, provision, clean, sessions and the entry point, over temporary repositories."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from typing import Dict, Optional, Sequence

from worktrees import core
from worktrees.tests.helpers import BIN, Sandbox

# Default keep patterns, each with a path it keeps.
KEPT_SAMPLES = {
    ".env": ".env",
    ".env.*": ".env.production",
    ".envrc": ".envrc",
    ".envrc.local": ".envrc.local",
    "*.local.*": "config.local.toml",
    ".claude/settings.local.json": ".claude/settings.local.json",
    ".idea/": ".idea/workspace.xml",
    "*.iml": "app.iml",
    ".superpowers/": ".superpowers/plan.md",
    "HANDOFF.md": "HANDOFF.md",
    "*.tfplan": "infra.tfplan",
    "tfplan*": "state/tfplan",
    "*.tfstate*": "terraform.tfstate.backup",
    ".xtdb/": ".xtdb/db",
    ".nrepl-port": ".nrepl-port",
    "*-workspace/": "skills/x-workspace/out.json",
    ".gstack/": ".gstack/session",
    "reports/": "reports/r.txt",
}


class Env(Sandbox):
    def env_vars(
        self, cwd: str, extra: Optional[Dict[str, str]] = None, *args: str
    ) -> Dict[str, str]:
        result = self.wt("env", "--json", *args, cwd=cwd, env=extra)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)["exports"]

    def test_two_worktrees_share_a_root_and_differ_in_target(self):
        repo = self.repo(files={"Cargo.toml": "[package]\n"})
        a, b = self.add_worktree(repo, "a"), self.add_worktree(repo, "b")
        va, vb = self.env_vars(a), self.env_vars(b)
        self.assertEqual(va["WORKTREE_BUILDS_ROOT"], vb["WORKTREE_BUILDS_ROOT"])
        self.assertEqual(va["CARGO_BUILD_BUILD_DIR"], vb["CARGO_BUILD_BUILD_DIR"])
        self.assertNotEqual(va["CARGO_TARGET_DIR"], vb["CARGO_TARGET_DIR"])
        self.assertTrue(va["CARGO_TARGET_DIR"].startswith(va["WORKTREE_BUILDS_ROOT"] + "/wt/a-"))
        self.assertTrue(self.env_vars(repo)["WORKTREE_BUILDS_WT"].endswith("/wt/main"))

    def test_outside_git_prints_nothing(self):
        outside = os.path.join(self.tmp, "plain")
        os.makedirs(outside)
        result = self.wt("env", cwd=outside)
        self.assertEqual((result.returncode, result.stdout), (0, ""))

    def test_a_persons_variable_is_kept(self):
        repo = self.repo(files={"Cargo.toml": "[package]\n"})
        got = self.env_vars(repo, {"CARGO_TARGET_DIR": "/mine"})
        self.assertNotIn("CARGO_TARGET_DIR", got)
        self.assertIn("CARGO_BUILD_BUILD_DIR", got)

    def test_languages_option_and_unknown(self):
        repo = self.repo()
        self.assertIn("TF_DATA_DIR", self.env_vars(repo, None, "--languages", "tofu"))
        self.assertEqual(self.wt("env", "--languages", "cobol", cwd=repo).returncode, 2)

    def test_fish_syntax(self):
        repo = self.repo()
        self.assertIn(
            "set -gx WORKTREE_BUILDS_ROOT ", self.wt("env", "--shell", "fish", cwd=repo).stdout
        )

    def test_first_entry_provisions(self):
        repo = self.repo()
        self.write(os.path.join(repo, ".claude", "settings.local.json"), '{"permissions": {}}\n')
        wt = self.add_worktree(repo, "feat")
        gitdir = self.git(wt, "rev-parse", "--absolute-git-dir").strip()
        self.assertFalse(os.path.exists(os.path.join(gitdir, core.MARKER)))
        vars_ = self.env_vars(wt)
        self.assertTrue(os.path.exists(os.path.join(gitdir, core.MARKER)))
        with open(os.path.join(wt, ".claude", "settings.local.json")) as handle:
            self.assertEqual(handle.read(), '{"permissions": {}}\n')
        with open(os.path.join(vars_["WORKTREE_BUILDS_WT"], "WORKTREE")) as handle:
            self.assertEqual(handle.read().strip(), wt)

    def test_provision_never_overwrites_settings_and_dry_run_writes_nothing(self):
        repo = self.repo()
        self.write(os.path.join(repo, ".claude", "settings.local.json"), "main\n")
        wt = self.add_worktree(repo, "feat")
        self.assertTrue(self.wt_json("provision", "--dry-run", wt)["would"])
        self.assertFalse(os.path.exists(os.path.join(wt, ".claude")))
        self.write(os.path.join(wt, ".claude", "settings.local.json"), "mine\n")
        self.wt_json("provision", wt)
        with open(os.path.join(wt, ".claude", "settings.local.json")) as handle:
            self.assertEqual(handle.read(), "mine\n")

    def test_env_is_fast_once_provisioned(self):
        load, cores = os.getloadavg()[0], os.cpu_count() or 1
        if load > cores:
            self.skipTest("load average {:.0f} is above {} cores".format(load, cores))
        repo = self.repo(files={"Cargo.toml": "[package]\n"})
        wt = self.add_worktree(repo, "feat")
        self.wt("env", cwd=wt)
        # Apple's /usr/bin/python3 caches the standard library's bytecode under
        # $HOME/Library/Caches; an empty test HOME would recompile it every run.
        timed_env = {**self.env, "HOME": os.environ.get("HOME", self.env["HOME"])}

        def once(args: Sequence[str]) -> float:
            start = time.perf_counter()
            subprocess.run(
                (sys.executable,) + tuple(args),
                cwd=wt,
                env=timed_env,
                stdout=subprocess.DEVNULL,
                check=True,
            )
            return time.perf_counter() - start

        pairs = tuple((once(("-c", "pass")), once(("-m", "worktrees", "env"))) for _ in range(15))
        bare = min(b for b, _ in pairs)
        env = min(e for _, e in pairs)
        self.assertLess(
            env - bare,
            0.050,
            "worktrees env took {:.0f} ms above a bare interpreter".format((env - bare) * 1000),
        )


class Clean(Sandbox):
    def populated(self) -> str:
        repo = self.repo(files={".worktree-keep": "secret.db\n"})
        self.write(os.path.join(repo, ".claude", "settings.local.json"), "main\n")
        wt = self.add_worktree(repo, "feat")
        self.commit(wt, "work.txt")
        for rel in KEPT_SAMPLES.values():
            self.write(os.path.join(wt, rel), "kept " + rel + "\n")
        for rel in (
            "secret.db",
            "build/out.o",
            "junk.o",
            ".claude/cache.txt",
            "state/other",
            "skills/notes.txt",
        ):
            self.write(os.path.join(wt, rel))
        nested = os.path.join(wt, "nested")
        self.sh("git", "init", "--quiet", nested)
        self.write(os.path.join(nested, "inner.o"), "inner\n")
        buried = os.path.join(wt, "build", "dep")
        self.sh("git", "init", "--quiet", buried)
        self.write(os.path.join(buried, "src.c"))
        return wt

    def test_keeps_defaults_and_worktree_keep_and_deletes_the_rest(self):
        wt = self.populated()
        build = json.loads(self.wt("env", "--json", cwd=wt).stdout)["exports"]["WORKTREE_BUILDS_WT"]
        self.write(os.path.join(build, "target", "debug", "app"))
        listing = self.wt_json("clean", "--dry-run", wt)
        self.assertTrue(listing["would"])
        self.assertTrue(os.path.exists(os.path.join(wt, "junk.o")))
        done = self.wt_json("clean", wt)
        for pattern, rel in KEPT_SAMPLES.items():
            self.assertTrue(
                os.path.exists(os.path.join(wt, rel)), "{} ({}) was deleted".format(rel, pattern)
            )
        for rel in ("secret.db", "work.txt", ".worktree-keep"):
            self.assertTrue(os.path.exists(os.path.join(wt, rel)), rel)
        for rel in (
            "build/out.o",
            "junk.o",
            ".claude/cache.txt",
            "state/other",
            "skills/notes.txt",
        ):
            self.assertFalse(os.path.exists(os.path.join(wt, rel)), rel)
        self.assertTrue(os.path.exists(os.path.join(wt, "nested", "inner.o")))
        self.assertTrue(os.path.exists(os.path.join(wt, "build", "dep", "src.c")))
        self.assertFalse(os.path.exists(build))
        self.assertIn("junk.o", done["remove"])
        cleaned = [e for e in self.events(wt) if e["event"] == "clean"]
        self.assertEqual([e["worktree"] for e in cleaned], [wt])

    def test_main_checkout_is_refused(self):
        repo = self.repo()
        self.write(os.path.join(repo, "junk.o"))
        refused = self.wt_json("clean", repo, code=3)
        self.assertIn("the main checkout", refused["reasons"])
        self.assertTrue(os.path.exists(os.path.join(repo, "junk.o")))

    def test_dirty_tree_is_refused(self):
        repo = self.repo(files={"a.txt": "a\n"})
        wt = self.add_worktree(repo, "feat")
        self.write(os.path.join(wt, "a.txt"), "changed\n")
        self.write(os.path.join(wt, "junk.o"))
        refused = self.wt_json("clean", wt, code=3)
        self.assertIn("1 tracked files with changes", refused["reasons"])
        self.assertTrue(os.path.exists(os.path.join(wt, "junk.o")))


class SweepCleansTheIdle(Sandbox):
    def test_an_unlanded_worktree_idle_a_day_loses_its_build_output(self):
        repo = self.cloned()
        wt = self.add_worktree(repo, "wip")
        self.commit(wt, "wip.txt")
        self.write(os.path.join(wt, "target", "debug", "app"))
        self.write(os.path.join(wt, ".env"), "SECRET=1\n")
        gitdir = self.git(wt, "rev-parse", "--absolute-git-dir").strip()
        old = time.time() - 2 * 86400
        for name in ("HEAD", "index", "logs/HEAD"):
            os.utime(os.path.join(gitdir, name), (old, old))
        report = self.wt_json("sweep", repo)
        entry = self.entry(report, "worktrees", wt)
        self.assertEqual(entry["action"], "clean")
        self.assertFalse(os.path.exists(os.path.join(wt, "target")))
        self.assertTrue(os.path.exists(os.path.join(wt, ".env")))
        self.assertIn(wt, self.listed(repo))


class Sessions(Sandbox):
    def test_sessions_newest_first(self):
        repo = self.repo()
        wt = self.add_worktree(repo, "feat")
        projects = os.path.join(self.env["CLAUDE_CONFIG_DIR"], "projects")
        old = os.path.join(
            projects, core.claude_project_folder(os.path.realpath(repo)), "old.jsonl"
        )
        new = os.path.join(projects, core.claude_project_folder(wt), "new.jsonl")
        self.write(old)
        self.write(new)
        os.utime(old, (1000, 1000))
        self.write(os.path.join(projects, "-unrelated", "x.jsonl"))
        data = self.wt_json("sessions", cwd=wt)
        self.assertEqual([s["session"] for s in data["sessions"]], ["new", "old"])


class EntryPoint(Sandbox):
    def test_every_command_has_help(self):
        for command in (
            "env",
            "provision",
            "clean",
            "sweep",
            "status",
            "verdict",
            "log",
            "archive",
            "sessions",
        ):
            result = self.wt(command, "--help")
            self.assertEqual(result.returncode, 0, command)
            self.assertIn("Example:", result.stdout)

    def test_abbreviated_options_are_refused(self):
        self.assertEqual(self.wt("sweep", "--dry").returncode, 2)

    def test_the_sh_entry_point_runs_the_package_through_a_symbolic_link(self):
        repo = self.cloned()
        link = os.path.join(self.tmp, "bin", "worktrees")
        os.makedirs(os.path.dirname(link))
        os.symlink(BIN, link)
        env = {**self.env, "PYTHONPATH": ""}
        result = subprocess.run(
            (link, "status", "--json"),
            cwd=repo,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(result.stdout)["repositories"][0]["repository"], os.path.realpath(repo)
        )
