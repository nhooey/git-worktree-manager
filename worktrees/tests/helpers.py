"""A sandbox for the command tests: a temporary HOME, and repositories with a bare remote.

Every removal in the tests happens here, in a temporary directory with HOME
and the XDG folders pointed inside it, never in a real checkout.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from typing import Dict, Optional

SOURCE = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
BIN = os.path.join(SOURCE, "bin", "worktrees")

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
        "target/",
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
                "[commit]\n\tgpgsign = false\n[tag]\n\tgpgsign = false\n"
                "[init]\n\tdefaultBranch = main\n"
                "[advice]\n\tdetachedHead = false\n"
                '[protocol "file"]\n\tallow = always\n'
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

    # Running things ----------------------------------------------------------

    def sh(self, *args: str, cwd: Optional[str] = None, stdin: Optional[str] = None) -> str:
        result = subprocess.run(
            args,
            cwd=cwd or self.tmp,
            env=self.env,
            input=stdin,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
        )
        if result.returncode != 0:
            raise AssertionError("{} failed: {}".format(args, result.stderr))
        return result.stdout

    def git(self, repo: str, *args: str, stdin: Optional[str] = None) -> str:
        return self.sh("git", "-C", repo, *args, stdin=stdin)

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

    # Repositories ------------------------------------------------------------

    def write(self, path: str, text: str = "x\n") -> None:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as handle:
            handle.write(text)

    def commit(self, repo: str, name: str, text: str = "x\n", message: str = "") -> str:
        self.write(os.path.join(repo, name), text)
        self.git(repo, "add", "--", name)
        self.git(repo, "commit", "--quiet", "-m", message or "add " + name)
        return self.git(repo, "rev-parse", "HEAD").strip()

    def repo(self, name: str = "repo", files: Optional[Dict[str, str]] = None) -> str:
        """A repository with no remote."""
        path = os.path.join(self.tmp, name)
        self.sh("git", "init", "--quiet", path)
        self.commit(path, ".gitignore", GITIGNORE)
        for rel, text in (files or {}).items():
            self.commit(path, rel, text)
        return path

    def cloned(self, name: str = "repo", files: Optional[Dict[str, str]] = None) -> str:
        """A clone of a bare remote, `<name>.remote.git`, with `origin/HEAD` at `main`."""
        remote = os.path.join(self.tmp, name + ".remote.git")
        self.sh("git", "init", "--quiet", "--bare", remote)
        self.git(remote, "symbolic-ref", "HEAD", "refs/heads/main")
        path = self.repo(name, files)
        self.git(path, "remote", "add", "origin", remote)
        self.git(path, "push", "--quiet", "--set-upstream", "origin", "main")
        self.git(path, "remote", "set-head", "origin", "main")
        return path

    def add_worktree(
        self, repo: str, branch: str, path: Optional[str] = None, start: str = "HEAD"
    ) -> str:
        target = path or os.path.join(repo + ".worktrees", branch.replace("/", "-"))
        self.git(repo, "worktree", "add", "--quiet", "-b", branch, target, start)
        return os.path.realpath(target)
