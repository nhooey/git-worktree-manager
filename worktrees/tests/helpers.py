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
from typing import Dict, Optional, Sequence

from worktrees import core, rules

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
        self.processes = []

    def tearDown(self):
        for process in self.processes:
            process.kill()
            process.wait()

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

    def background(self, args: Sequence[str], cwd: Optional[str] = None) -> subprocess.Popen:
        process = subprocess.Popen(args, cwd=cwd or self.tmp, env=self.env)
        self.processes.append(process)
        return process

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

    def remote(self, repo: str) -> str:
        return self.git(repo, "remote", "get-url", "origin").strip()

    def other_clone(self, repo: str, name: str = "other") -> str:
        path = os.path.join(self.tmp, name)
        self.sh("git", "clone", "--quiet", self.remote(repo), path)
        return path

    def add_worktree(
        self, repo: str, branch: str, path: Optional[str] = None, start: str = "HEAD"
    ) -> str:
        target = path or os.path.join(repo + ".worktrees", branch.replace("/", "-"))
        self.git(repo, "worktree", "add", "--quiet", "-b", branch, target, start)
        return os.path.realpath(target)

    def merge_into_main(self, repo: str, branch: str) -> None:
        """Merge `branch` into `origin/main` with a merge commit, as a pull request would."""
        other = os.path.join(self.tmp, "merger")
        if not os.path.isdir(other):
            self.sh("git", "clone", "--quiet", self.remote(repo), other)
        self.git(other, "fetch", "--quiet", "origin")
        self.git(other, "checkout", "--quiet", "main")
        self.git(other, "reset", "--quiet", "--hard", "origin/main")
        self.git(other, "merge", "--quiet", "--no-ff", "-m", "merge " + branch, "origin/" + branch)
        self.git(other, "push", "--quiet", "origin", "main")
        self.git(repo, "fetch", "--quiet", "origin")

    def landed_branch(self, repo: str, branch: str, worktree: bool = False) -> str:
        """A branch with a commit, pushed and merged; returns its worktree's path or its name."""
        path = self.add_worktree(repo, branch) if worktree else None
        where = path or repo
        if not worktree:
            self.git(repo, "branch", "--quiet", branch, "main")
            self.git(repo, "checkout", "--quiet", branch)
        self.commit(where, branch.replace("/", "-") + ".txt")
        self.git(where, "push", "--quiet", "--set-upstream", "origin", branch)
        if not worktree:
            self.git(repo, "checkout", "--quiet", "main")
        self.merge_into_main(repo, branch)
        return path or branch

    def local_branch(self, repo: str, branch: str, push: bool = False) -> str:
        """A branch with one commit of its own; returns its tip."""
        self.git(repo, "branch", "--quiet", branch, "main")
        self.git(repo, "checkout", "--quiet", branch)
        tip = self.commit(repo, branch.replace("/", "-") + ".txt")
        if push:
            self.git(repo, "push", "--quiet", "--set-upstream", "origin", branch)
        self.git(repo, "checkout", "--quiet", "main")
        return tip

    def state_dir(self, repo: str) -> str:
        common = os.path.realpath(
            self.git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir").strip()
        )
        return core.repo_state_dir(self.env, common)

    def events(self, repo: str) -> Sequence[Dict]:
        path = os.path.join(self.state_dir(repo), "events.jsonl")
        if not os.path.exists(path):
            return []
        with open(path) as handle:
            return [json.loads(line) for line in handle if line.strip()]

    def branches(self, repo: str) -> Sequence[str]:
        return self.git(repo, "for-each-ref", "--format=%(refname:short)", "refs/heads").split()

    def backup(self, repo: str) -> str:
        """The bare repository under `~/backups/git-archive` that holds `repo`'s archives."""
        url = self.git(repo, "config", "--default", "", "--get", "remote.origin.url").strip()
        owner, name = rules.backup_name(url or None, os.path.basename(repo))
        return os.path.join(self.env["HOME"], "backups", "git-archive", owner, name + ".git")

    def archives(self, repo: str, local: bool = False) -> Dict[str, str]:
        """`refs/archive/*` in `repo`'s backup, or in `repo` itself with `local`."""
        where = repo if local else self.backup(repo)
        if not os.path.isdir(where):
            return {}
        return dict(
            line.split(" ", 1)
            for line in self.git(
                where, "for-each-ref", "--format=%(refname) %(objectname)", "refs/archive"
            ).splitlines()
        )

    def listed(self, repo: str) -> Sequence[str]:
        return tuple(
            os.path.realpath(line.split(" ", 1)[1])
            if os.path.exists(line.split(" ", 1)[1])
            else line.split(" ", 1)[1]
            for line in self.git(repo, "worktree", "list", "--porcelain").splitlines()
            if line.startswith("worktree ")
        )

    def install_shim(self, repo: str, default_ref: str = "") -> str:
        """The `reference-transaction` hook, installed by hand as `worktrees hooks install` will."""
        hooks = os.path.join(
            self.git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir").strip(),
            "hooks",
        )
        os.makedirs(hooks, exist_ok=True)
        path = os.path.join(hooks, "reference-transaction")
        log = os.path.join(self.state_dir(repo), "events.jsonl")
        os.makedirs(os.path.dirname(log), exist_ok=True)
        self.write(
            path,
            "#!/bin/sh\n"
            '[ "$1" = committed ] || exit 0\n'
            "w={}\n"
            '[ -x "$w" ] || exit 0\n'
            "WORKTREES_LOG={} \\\n"
            "WORKTREES_DEFAULT_REF={} \\\n"
            'exec "$w" hook-ref-update "$@"\n'.format(
                core.sh_quote(BIN), core.sh_quote(log), core.sh_quote(default_ref)
            ),
        )
        os.chmod(path, 0o755)
        return log

    def lstart(self, pid: int) -> str:
        """The process's start, in UTC, as `ps -o lstart=` gives it."""
        return " ".join(
            subprocess.run(
                ("ps", "-o", "lstart=", "-p", str(pid)),
                env={**self.env, "LC_ALL": "C", "TZ": "UTC0"},
                stdout=subprocess.PIPE,
                universal_newlines=True,
                check=True,
            ).stdout.split()
        )

    def dead_pid(self) -> int:
        process = subprocess.Popen(("true",))
        process.wait()
        return process.pid

    def entry(self, report: Dict, kind: str, key: str) -> Dict:
        repo = report["repositories"][0]
        field = "path" if kind == "worktrees" else "branch"
        return next(e for e in repo[kind] if e[field] == key)

    def codes(self, entry: Dict) -> Sequence[str]:
        return [r["code"] for r in entry["reasons"]]
