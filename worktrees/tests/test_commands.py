"""Tests of the commands, run as `python3 -m worktrees` over temporary repositories.

Every removal here happens in a temporary directory with HOME and the XDG
folders pointed inside it, never in a real checkout.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from typing import Dict, Optional, Sequence

from worktrees import core

SOURCE = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

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

GITIGNORE = "\n".join(
    (
        ".env*",
        ".envrc*",
        "*.local.*",
        ".claude/",
        ".idea/",
        "*.iml",
        ".superpowers/",
        "HANDOFF.md",
        "*.tfplan",
        "state/",
        "*.tfstate*",
        ".xtdb/",
        ".nrepl-port",
        "skills/",
        ".gstack/",
        "reports/",
        "secret.db",
        "build/",
        "*.o",
        "nested/",
        "",
    )
)


class Sandbox(unittest.TestCase):
    """A temporary HOME, cache, state and config, and helpers for repositories."""

    def setUp(self):
        self.tmp = os.path.realpath(tempfile.mkdtemp(prefix="worktrees-test-"))
        self.addCleanup(shutil.rmtree, self.tmp, True)
        home = os.path.join(self.tmp, "home")
        os.makedirs(home)
        gitconfig = os.path.join(self.tmp, "gitconfig")
        with open(gitconfig, "w") as handle:
            handle.write(
                "[user]\n\tname = Test\n\temail = test@example.invalid\n"
                "[commit]\n\tgpgsign = false\n[init]\n\tdefaultBranch = main\n"
                "[advice]\n\tdetachedHead = false\n"
            )
        self.env = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "HOME": home,
            "XDG_CACHE_HOME": os.path.join(self.tmp, "cache"),
            "XDG_STATE_HOME": os.path.join(self.tmp, "state"),
            "XDG_CONFIG_HOME": os.path.join(self.tmp, "config"),
            "CLAUDE_CONFIG_DIR": os.path.join(self.tmp, "claude"),
            "GIT_CONFIG_GLOBAL": gitconfig,
            "GIT_CONFIG_NOSYSTEM": "1",
            "PYTHONPATH": SOURCE,
            "TMPDIR": self.tmp,
        }
        self.processes = []

    def tearDown(self):
        for process in self.processes:
            process.kill()
            process.wait()

    # Running things ----------------------------------------------------------

    def sh(self, *args: str, cwd: Optional[str] = None) -> str:
        result = subprocess.run(
            args,
            cwd=cwd or self.tmp,
            env=self.env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
        )
        if result.returncode != 0:
            raise AssertionError("{} failed: {}".format(args, result.stderr))
        return result.stdout

    def git(self, repo: str, *args: str) -> str:
        return self.sh("git", "-C", repo, *args)

    def wt(
        self, *args: str, cwd: Optional[str] = None, env: Optional[Dict[str, str]] = None
    ) -> subprocess.CompletedProcess:
        return subprocess.run(
            (sys.executable, "-m", "worktrees") + args,
            cwd=cwd or self.tmp,
            env={**self.env, **(env or {})},
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
        )

    def wt_json(self, *args: str, cwd: Optional[str] = None, code: int = 0) -> Dict:
        result = self.wt(*(args + ("--json",)), cwd=cwd)
        self.assertEqual(result.returncode, code, result.stderr + result.stdout)
        return json.loads(result.stdout)

    def env_vars(
        self, cwd: str, extra: Optional[Dict[str, str]] = None, *args: str
    ) -> Dict[str, str]:
        result = self.wt("env", "--json", *args, cwd=cwd, env=extra)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)["exports"]

    def background(self, args: Sequence[str], cwd: str) -> subprocess.Popen:
        process = subprocess.Popen(args, cwd=cwd, env=self.env)
        self.processes.append(process)
        return process

    # Repositories ------------------------------------------------------------

    def write(self, path: str, text: str = "x\n") -> None:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as handle:
            handle.write(text)

    def commit(self, repo: str, name: str, text: str = "x\n", message: str = "") -> None:
        self.write(os.path.join(repo, name), text)
        self.git(repo, "add", "--", name)
        self.git(repo, "commit", "--quiet", "-m", message or "add " + name)

    def repo(self, name: str = "repo", files: Optional[Dict[str, str]] = None) -> str:
        path = os.path.join(self.tmp, name)
        self.sh("git", "init", "--quiet", path)
        self.commit(path, ".gitignore", GITIGNORE)
        for rel, text in (files or {}).items():
            self.commit(path, rel, text)
        return path

    def add_worktree(self, repo: str, branch: str, path: Optional[str] = None) -> str:
        target = path or os.path.join(repo + ".worktrees", branch)
        self.git(repo, "worktree", "add", "--quiet", "-b", branch, target)
        return os.path.realpath(target)

    def branches(self, repo: str) -> Sequence[str]:
        return self.git(repo, "for-each-ref", "--format=%(refname:short)", "refs/heads").split()

    def listed(self, repo: str) -> Sequence[str]:
        return tuple(
            os.path.realpath(line.split(" ", 1)[1])
            if os.path.exists(line.split(" ", 1)[1])
            else line.split(" ", 1)[1]
            for line in self.git(repo, "worktree", "list", "--porcelain").splitlines()
            if line.startswith("worktree ")
        )

    def lstart(self, pid: int) -> str:
        return " ".join(self.sh("ps", "-o", "lstart=", "-p", str(pid)).split())

    def dead_pid(self) -> int:
        process = subprocess.Popen(("true",))
        process.wait()
        return process.pid


class Env(Sandbox):
    def test_two_worktrees_share_a_root_and_differ_in_target(self):
        repo = self.repo(files={"Cargo.toml": "[package]\n"})
        a, b = self.add_worktree(repo, "a"), self.add_worktree(repo, "b")
        va, vb = self.env_vars(a), self.env_vars(b)
        self.assertEqual(va["WORKTREE_BUILDS_ROOT"], vb["WORKTREE_BUILDS_ROOT"])
        self.assertEqual(va["CARGO_BUILD_BUILD_DIR"], vb["CARGO_BUILD_BUILD_DIR"])
        self.assertNotEqual(va["CARGO_TARGET_DIR"], vb["CARGO_TARGET_DIR"])
        self.assertTrue(va["CARGO_TARGET_DIR"].startswith(va["WORKTREE_BUILDS_ROOT"] + "/wt/a-"))
        self.assertTrue(self.env_vars(repo)["WORKTREE_BUILDS_WT"].endswith("/wt/main"))

    def test_second_clone_gets_another_root(self):
        repo = self.repo(files={"Cargo.toml": "[package]\n"})
        os.makedirs(os.path.join(self.tmp, "other"))
        clone = os.path.join(self.tmp, "other", "repo")
        self.sh("git", "clone", "--quiet", repo, clone)
        self.assertNotEqual(
            self.env_vars(repo)["WORKTREE_BUILDS_ROOT"],
            self.env_vars(clone)["WORKTREE_BUILDS_ROOT"],
        )

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

    def test_inherited_from_another_worktree_is_recomputed(self):
        repo = self.repo(files={"Cargo.toml": "[package]\n"})
        a, b = self.add_worktree(repo, "a"), self.add_worktree(repo, "b")
        inherited = self.env_vars(a)
        got = self.env_vars(b, inherited)
        self.assertNotEqual(got["CARGO_TARGET_DIR"], inherited["CARGO_TARGET_DIR"])
        self.assertIn("/wt/b-", got["CARGO_TARGET_DIR"])

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
        with open(os.path.join(vars_["WORKTREE_BUILDS_ROOT"], "REPO")) as handle:
            self.assertEqual(handle.read().strip(), os.path.realpath(os.path.join(repo, ".git")))
        status = self.wt_json("status", "--no-size", cwd=repo)
        row = next(w for w in status["repositories"][0]["worktrees"] if w["path"] == wt)
        self.assertTrue(row["provisioned"])

    def test_provision_never_overwrites_settings(self):
        repo = self.repo()
        self.write(os.path.join(repo, ".claude", "settings.local.json"), "main\n")
        wt = self.add_worktree(repo, "feat")
        self.write(os.path.join(wt, ".claude", "settings.local.json"), "mine\n")
        self.wt_json("provision", wt)
        with open(os.path.join(wt, ".claude", "settings.local.json")) as handle:
            self.assertEqual(handle.read(), "mine\n")

    def test_env_is_fast_once_provisioned(self):
        load, cores = os.getloadavg()[0], os.cpu_count() or 1
        if load > cores:
            self.skipTest(
                "load average {:.0f} is above {} cores: timings would measure the load".format(
                    load, cores
                )
            )
        repo = self.repo(files={"Cargo.toml": "[package]\n"})
        wt = self.add_worktree(repo, "feat")
        self.wt("env", cwd=wt)
        # Apple's /usr/bin/python3 caches the standard library's bytecode under
        # $HOME/Library/Caches; an empty test HOME would recompile it every run.
        # Nothing `env` reads here comes from HOME: the XDG variables are set.
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

        # The budget is the program's own work, above starting the interpreter,
        # which a loaded machine or the Nix sandbox can slow by itself.
        # Interleaved, so both see the same load, and the fastest of each,
        # the run least slowed by anything else on the machine.
        pairs = tuple((once(("-c", "pass")), once(("-m", "worktrees", "env"))) for _ in range(15))
        bare = min(b for b, _ in pairs)
        env = min(e for _, e in pairs)
        self.assertLess(
            env - bare,
            0.050,
            "worktrees env took {:.0f} ms, {:.0f} ms above a bare interpreter".format(
                env * 1000, (env - bare) * 1000
            ),
        )


class Finish(Sandbox):
    def landed(self, repo: str, name: str = "feat") -> str:
        wt = self.add_worktree(repo, name)
        self.commit(wt, name + ".txt")
        self.git(repo, "merge", "--quiet", "--no-edit", "--no-ff", name)
        return wt

    def test_landed_worktree_is_removed(self):
        repo = self.repo(files={"Cargo.toml": "[package]\n"})
        wt = self.landed(repo)
        build = self.env_vars(wt)["WORKTREE_BUILDS_WT"]
        self.assertTrue(os.path.isdir(build))
        listing = self.wt_json("finish", wt, cwd=repo)
        self.assertFalse(listing["finished"])
        self.assertIn(wt, self.listed(repo))
        done = self.wt_json("finish", wt, "--apply", cwd=repo)
        self.assertTrue(done["finished"], done)
        self.assertNotIn(wt, self.listed(repo))
        self.assertNotIn("feat", self.branches(repo))
        self.assertFalse(os.path.exists(build))

    def test_landed_in_integration(self):
        repo = self.repo()
        self.git(repo, "branch", "integration")
        wt = self.add_worktree(repo, "feat")
        self.commit(wt, "f.txt")
        self.git(repo, "checkout", "--quiet", "integration")
        self.git(repo, "merge", "--quiet", "--ff-only", "feat")
        self.git(repo, "checkout", "--quiet", "main")
        done = self.wt_json("finish", wt, "--apply", cwd=repo)
        self.assertTrue(done["finished"], done)
        self.assertNotIn("feat", self.branches(repo))

    def test_unmerged_is_kept_with_its_reason(self):
        repo = self.repo()
        wt = self.add_worktree(repo, "feat")
        self.commit(wt, "f.txt")
        kept = self.wt_json("finish", wt, "--apply", cwd=repo, code=3)
        self.assertIn("branch has not landed in main", kept["reasons"])
        self.assertIn(wt, self.listed(repo))

    def test_dirty_tree_is_kept(self):
        repo = self.repo()
        wt = self.landed(repo)
        self.write(os.path.join(wt, "feat.txt"), "changed\n")
        kept = self.wt_json("finish", wt, "--apply", cwd=repo, code=3)
        self.assertIn("1 uncommitted paths", kept["reasons"])

    def test_failed_run_with_and_without_a_saved_log(self):
        """The job's commit is on another ref too, so it has no commits of its own."""
        repo = self.repo()
        base = self.git(repo, "rev-parse", "HEAD").strip()
        self.commit(repo, "later.txt")
        wt = os.path.join(repo + ".worktrees", "job")
        self.git(repo, "worktree", "add", "--quiet", "--detach", wt, base)
        wt = os.path.realpath(wt)
        self.git(wt, "switch", "--quiet", "-c", "job")
        self.commit(wt, "work.txt")
        self.git(repo, "branch", "keepref", "job")
        log = os.path.join(self.tmp, "job.log")
        self.write(log, "failed\n")
        without = self.wt_json("finish", wt, cwd=repo, code=3)
        self.assertIn("branch has not landed in main", without["reasons"])
        inside = os.path.join(wt, "job.log")
        self.write(inside)
        kept = self.wt_json("finish", wt, "--log", inside, cwd=repo, code=3)
        self.assertTrue(any(r.startswith("log: ") for r in kept["reasons"]), kept)
        os.remove(inside)
        done = self.wt_json("finish", wt, "--log", log, "--apply", cwd=repo)
        self.assertNotIn(wt, self.listed(repo))
        self.assertIn("remove worktree " + wt, done["actions"])

    def test_failed_run_with_commits_of_its_own_is_kept(self):
        repo = self.repo()
        wt = self.add_worktree(repo, "job")
        self.commit(wt, "work.txt")
        log = os.path.join(self.tmp, "job.log")
        self.write(log, "failed\n")
        kept = self.wt_json("finish", wt, "--log", log, "--apply", cwd=repo, code=3)
        self.assertIn("branch has commits of its own", kept["reasons"])

    def test_squash_merge_is_seen_by_git_cherry(self):
        repo = self.repo()
        wt = self.add_worktree(repo, "feat")
        self.commit(wt, "f.txt", "feature\n")
        self.git(repo, "merge", "--quiet", "--squash", "feat")
        self.git(repo, "commit", "--quiet", "-m", "squashed feat")
        done = self.wt_json("finish", wt, "--apply", cwd=repo, code=1)
        self.assertNotIn(wt, self.listed(repo))
        # git branch --delete refuses a branch that is not an ancestor; it stays.
        self.assertIn("feat", self.branches(repo))
        self.assertTrue(any("delete branch feat" in p for p in done["problems"]), done)

    def test_main_checkout_is_never_finished(self):
        repo = self.repo()
        kept = self.wt_json("finish", repo, "--apply", cwd=repo, code=3)
        self.assertIn("the main checkout", kept["reasons"])

    def test_unique_kept_file_keeps_a_landed_worktree(self):
        repo = self.repo()
        wt = self.landed(repo)
        self.write(os.path.join(wt, ".envrc.local"), "export SECRET=1\n")
        kept = self.wt_json("finish", wt, "--apply", cwd=repo, code=3)
        self.assertIn("kept files: .envrc.local", kept["reasons"])

    def test_copied_settings_do_not_keep_a_worktree(self):
        repo = self.repo()
        self.write(os.path.join(repo, ".claude", "settings.local.json"), "{}\n")
        wt = self.landed(repo)
        self.wt("env", cwd=wt)
        self.assertTrue(os.path.exists(os.path.join(wt, ".claude", "settings.local.json")))
        self.assertTrue(self.wt_json("finish", wt, "--apply", cwd=repo)["finished"])

    def test_stash_keeps_a_worktree(self):
        repo = self.repo()
        wt = self.landed(repo)
        self.write(os.path.join(wt, "feat.txt"), "wip\n")
        self.git(wt, "stash", "--quiet")
        kept = self.wt_json("finish", wt, "--apply", cwd=repo, code=3)
        self.assertIn("1 stashes on its branch", kept["reasons"])

    def test_process_inside_keeps_a_worktree(self):
        repo = self.repo()
        wt = self.landed(repo)
        self.background(("sleep", "60"), cwd=wt)
        time.sleep(0.2)
        kept = self.wt_json("finish", wt, "--apply", cwd=repo, code=3)
        self.assertTrue(any(r.startswith("in use by process") for r in kept["reasons"]), kept)


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
        build = self.env_vars(wt)["WORKTREE_BUILDS_WT"]
        self.write(os.path.join(build, "target", "debug", "app"))
        listing = self.wt_json("clean", wt)
        self.assertFalse(listing["applied"])
        self.assertTrue(os.path.exists(os.path.join(wt, "junk.o")))
        done = self.wt_json("clean", wt, "--apply")
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

    def test_listing_is_logged_before_deletion(self):
        wt = self.populated()
        self.wt_json("clean", wt, "--apply")
        with open(os.path.join(self.env["XDG_STATE_HOME"], "worktrees", "clean.log")) as handle:
            log = handle.read()
        self.assertIn("clean {}: junk.o".format(wt), log)
        self.assertNotIn(".envrc.local", log)

    def test_main_checkout_is_refused(self):
        repo = self.repo()
        self.write(os.path.join(repo, "junk.o"))
        refused = self.wt_json("clean", repo, "--apply", code=3)
        self.assertIn("the main checkout", refused["reasons"])
        self.assertTrue(os.path.exists(os.path.join(repo, "junk.o")))

    def test_dirty_tree_is_refused(self):
        repo = self.repo(files={"a.txt": "a\n"})
        wt = self.add_worktree(repo, "feat")
        self.write(os.path.join(wt, "a.txt"), "changed\n")
        self.write(os.path.join(wt, "junk.o"))
        refused = self.wt_json("clean", wt, "--apply", code=3)
        self.assertIn("1 tracked files with changes", refused["reasons"])
        self.assertTrue(os.path.exists(os.path.join(wt, "junk.o")))

    def test_env_and_envrc_local_survive(self):
        repo = self.repo()
        wt = self.add_worktree(repo, "feat")
        for rel in (".env", ".envrc.local", "junk.o"):
            self.write(os.path.join(wt, rel))
        self.wt_json("clean", wt, "--apply")
        self.assertTrue(os.path.exists(os.path.join(wt, ".env")))
        self.assertTrue(os.path.exists(os.path.join(wt, ".envrc.local")))
        self.assertFalse(os.path.exists(os.path.join(wt, "junk.o")))


class Sweep(Sandbox):
    def landed(self, repo: str, name: str, path: Optional[str] = None) -> str:
        wt = self.add_worktree(repo, name, path)
        self.commit(wt, name + ".txt")
        self.git(repo, "merge", "--quiet", "--no-edit", "--no-ff", name)
        return wt

    def sweep(self, repo: str, *args: str) -> Dict:
        return self.wt_json("sweep", "--no-fetch", *args, cwd=repo)["repositories"][0]

    def verdict(self, report: Dict, wt: str) -> Dict:
        return next(v for v in report["verdicts"] if v["path"] == wt)

    def lock(self, repo: str, wt: str, reason: str) -> None:
        self.git(repo, "worktree", "lock", "--reason", reason, wt)

    def test_listing_removes_nothing(self):
        repo = self.repo()
        wt = self.landed(repo, "feat")
        report = self.sweep(repo)
        self.assertEqual(self.verdict(report, wt)["action"], "finish")
        self.assertIn(wt, self.listed(repo))

    def test_three_places_found_and_landed_removed(self):
        repo = self.repo()
        scratch = os.path.join(self.tmp, "scratch", "job")
        places = (
            self.landed(repo, "a", os.path.join(repo, ".claude", "worktrees", "agent-a")),
            self.landed(repo, "b"),
            self.landed(repo, "c", scratch),
        )
        report = self.sweep(repo, "--apply")
        self.assertEqual({self.verdict(report, p)["action"] for p in places}, {"finish"})
        self.assertEqual(self.listed(repo), (os.path.realpath(repo),))

    def test_stale_claude_lock_cleared(self):
        repo = self.repo()
        wt = self.landed(repo, "agent")
        self.lock(
            repo,
            wt,
            "claude agent agent-1 (pid {} start Sun Nov 15 18:44:37 2026)".format(self.dead_pid()),
        )
        report = self.sweep(repo, "--apply")
        self.assertIn(wt, report["unlocked"])
        self.assertNotIn(wt, self.listed(repo))

    def test_live_claude_lock_respected(self):
        repo = self.repo()
        wt = self.landed(repo, "agent")
        live = self.background(("sleep", "60"), cwd=self.tmp)
        self.lock(
            repo,
            wt,
            "claude agent agent-1 (pid {} start {})".format(live.pid, self.lstart(live.pid)),
        )
        report = self.sweep(repo, "--apply")
        self.assertEqual(self.verdict(report, wt)["action"], "skip")
        self.assertIn(wt, self.listed(repo))

    def test_reused_pid_with_another_start_is_stale(self):
        repo = self.repo()
        wt = self.landed(repo, "agent")
        live = self.background(("sleep", "60"), cwd=self.tmp)
        self.lock(
            repo,
            wt,
            "claude agent agent-1 (pid {} start Mon Jan  1 00:00:00 2024)".format(live.pid),
        )
        report = self.sweep(repo, "--apply")
        self.assertIn(wt, report["unlocked"])
        self.assertNotIn(wt, self.listed(repo))

    def test_persons_lock_respected(self):
        repo = self.repo()
        wt = self.landed(repo, "usb")
        self.lock(repo, wt, "on a USB drive")
        report = self.sweep(repo, "--apply")
        self.assertEqual(self.verdict(report, wt)["action"], "skip")
        self.assertIn(wt, self.listed(repo))

    def test_process_inside_keeps_a_worktree(self):
        repo = self.repo()
        wt = self.landed(repo, "busy")
        self.background(("sleep", "60"), cwd=wt)
        time.sleep(0.2)
        report = self.sweep(repo, "--apply")
        self.assertEqual(self.verdict(report, wt)["action"], "skip")
        self.assertIn(wt, self.listed(repo))

    def test_fresh_transcript_keeps_a_worktree(self):
        repo = self.repo()
        wt = self.landed(repo, "named")
        self.write(
            os.path.join(
                self.env["CLAUDE_CONFIG_DIR"], "projects", core.claude_project_folder(wt), "s.jsonl"
            )
        )
        report = self.sweep(repo, "--apply")
        self.assertEqual(self.verdict(report, wt)["action"], "skip")

    def test_unmerged_idle_worktree_is_cleaned_and_reported(self):
        repo = self.repo()
        wt = self.add_worktree(repo, "wip")
        self.commit(wt, "w.txt")
        self.write(os.path.join(wt, "junk.o"))
        self.write(os.path.join(wt, ".env"))
        report = self.sweep(repo, "--apply")
        self.assertEqual(self.verdict(report, wt)["action"], "clean")
        self.assertEqual([u["worktree"] for u in report["unmerged"]], [wt])
        self.assertFalse(os.path.exists(os.path.join(wt, "junk.o")))
        self.assertTrue(os.path.exists(os.path.join(wt, ".env")))
        self.assertIn(wt, self.listed(repo))

    def test_orphan_wt_folder_deleted(self):
        repo = self.repo()
        wt = self.add_worktree(repo, "gone")
        build = self.env_vars(wt)["WORKTREE_BUILDS_WT"]
        self.git(repo, "worktree", "remove", wt)
        report = self.sweep(repo, "--apply")
        self.assertIn(build, report["orphans"])
        self.assertFalse(os.path.exists(build))
        self.assertTrue(os.path.isdir(self.env_vars(repo)["WORKTREE_BUILDS_WT"]))

    def test_cargo_cap(self):
        repo = self.repo()
        root = self.env_vars(repo)["WORKTREE_BUILDS_ROOT"]
        held = os.path.join(root, "shared", "cargo-build", "debug", "deps", "big.rlib")
        self.write(held, "x" * 100000)
        holder = self.background(
            (sys.executable, "-c", "import sys,time; f=open(sys.argv[1]); time.sleep(60)", held),
            cwd=self.tmp,
        )
        time.sleep(0.5)
        report = self.sweep(repo, "--apply", "--cargo-cap", "0.00001")
        self.assertEqual(report["cargo_build"]["action"], "kept: a process has a file open in it")
        self.assertTrue(os.path.exists(held))
        holder.kill()
        holder.wait()
        report = self.sweep(repo, "--apply", "--cargo-cap", "0.00001")
        self.assertEqual(report["cargo_build"]["action"], "deleted")
        self.assertFalse(os.path.exists(held))

    def test_landed_local_branch_deleted(self):
        repo = self.repo()
        self.git(repo, "branch", "old")
        self.git(repo, "branch", "integration")
        self.sweep(repo, "--apply")
        self.assertNotIn("old", self.branches(repo))
        self.assertIn("integration", self.branches(repo))

    def test_all_finds_repositories_under_roots(self):
        repo = self.repo()
        wt = self.landed(repo, "feat")
        config = os.path.join(self.env["XDG_CONFIG_HOME"], "worktrees", "config.json")
        self.write(config, json.dumps({"roots": [self.tmp]}))
        data = self.wt_json("sweep", "--all", "--no-fetch", cwd=self.tmp)
        self.assertIn(os.path.realpath(repo), [r["repository"] for r in data["repositories"]])
        self.assertIn(wt, self.listed(repo))


class StatusAndSessions(Sandbox):
    def test_status_rows_and_floors(self):
        repo = self.repo()
        wt = self.add_worktree(repo, "feat")
        self.write(os.path.join(wt, "new.txt"))
        self.write(
            os.path.join(self.env["XDG_CONFIG_HOME"], "worktrees", "floor"),
            "WORKTREES_FLOOR_BUILD_GB=1\nWORKTREES_FLOOR_CLEANUP_ONLY_GB=0.5\n",
        )
        data = self.wt_json("status", cwd=repo)
        row = next(w for w in data["repositories"][0]["worktrees"] if w["path"] == wt)
        self.assertEqual(row["uncommitted"], 1)
        self.assertEqual(row["landed"], "main")
        self.assertEqual(data["floors"], {"build_gb": 1.0, "cleanup_only_gb": 0.5})
        self.assertIn("free", data["disks"][0])
        self.assertIn("build root", self.wt("status", cwd=repo).stdout)

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


class Help(Sandbox):
    def test_every_command_has_help(self):
        for command in ("env", "provision", "clean", "finish", "sweep", "status", "sessions"):
            result = self.wt(command, "--help")
            self.assertEqual(result.returncode, 0, command)
            self.assertIn("Example:", result.stdout)

    def test_abbreviated_options_are_refused(self):
        self.assertEqual(self.wt("sweep", "--app").returncode, 2)


if __name__ == "__main__":
    unittest.main()
