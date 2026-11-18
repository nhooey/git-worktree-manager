"""Tests of `worktrees hook-ref-update`, the `sh` function git's `reference-transaction` hook runs.

The shim is installed by hand, as `worktrees hooks install` will install it.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from typing import Dict, Sequence, Tuple

from worktrees.tests.helpers import BIN, Sandbox

ZERO = "0" * 40
ROUNDS = 12
BRANCHES = 1000
BUDGET_SECONDS = 0.005
FORCED = 20


class HookRefUpdate(Sandbox):
    def run_hook(self, state: str, stdin: str, cwd: str, **env: str) -> subprocess.CompletedProcess:
        return subprocess.run(
            (BIN, "hook-ref-update", state),
            cwd=cwd,
            input=stdin,
            env={**self.env, **env},
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
        )

    def lines(self, path: str) -> Sequence[Dict]:
        with open(path) as handle:
            return [json.loads(line) for line in handle]

    def test_a_committed_transaction_writes_one_line_per_ref(self):
        repo = self.cloned()
        log = self.install_shim(repo)
        before = self.git(repo, "rev-parse", "HEAD").strip()
        after = self.commit(repo, "a.txt")
        mine = [e for e in self.lines(log) if e["ref"] == "refs/heads/main"]
        self.assertEqual(mine[-1]["old"], before)
        self.assertEqual(mine[-1]["new"], after)
        self.assertEqual(mine[-1]["event"], "ref-update")
        self.assertEqual(mine[-1]["source"], "hook")
        self.assertEqual(mine[-1]["worktree"], os.path.realpath(repo))
        self.git(repo, "fetch", "--quiet", "origin")
        self.commit(self.other_clone(repo), "theirs.txt")
        self.git(os.path.join(self.tmp, "other"), "push", "--quiet", "origin", "main")
        self.git(repo, "fetch", "--quiet", "origin")
        fetched = [e for e in self.lines(log) if e["ref"] == "refs/remotes/origin/main"]
        self.assertEqual(fetched[-1]["new"], self.git(repo, "rev-parse", "origin/main").strip())

    def test_any_other_state_writes_nothing(self):
        log = os.path.join(self.tmp, "events.jsonl")
        for state in ("prepared", "aborted", "preparing"):
            result = self.run_hook(
                state, "{} {} refs/heads/x\n".format(ZERO, "1" * 40), self.tmp, WORKTREES_LOG=log
            )
            self.assertEqual(result.returncode, 0)
        self.assertFalse(os.path.exists(log))

    def test_a_path_with_a_quote_and_a_backslash(self):
        odd = os.path.join(self.tmp, 'we"ird\\path')
        os.makedirs(odd)
        log = os.path.join(self.tmp, "events.jsonl")
        self.run_hook(
            "committed", '{} {} refs/heads/a"b\n'.format(ZERO, "1" * 40), odd, WORKTREES_LOG=log
        )
        (line,) = self.lines(log)
        self.assertEqual(line["worktree"], odd)
        self.assertEqual(line["ref"], 'refs/heads/a"b')
        self.assertNotIn("old", line)

    def test_an_unwritable_log_never_fails_git(self):
        result = self.run_hook(
            "committed",
            "{} {} refs/heads/a\n".format(ZERO, "1" * 40),
            self.tmp,
            WORKTREES_LOG=os.path.join(self.tmp, "missing", "dir", "events.jsonl"),
        )
        self.assertEqual((result.returncode, result.stderr), (0, ""))
        self.assertEqual(self.run_hook("committed", "", self.tmp).returncode, 0)

    def test_moving_the_default_branch_starts_a_detached_sweep(self):
        repo = self.cloned()
        self.install_shim(repo, "refs/remotes/origin/main")
        other = self.other_clone(repo)
        self.commit(other, "theirs.txt")
        self.git(other, "push", "--quiet", "origin", "main")
        started = time.monotonic()
        self.git(repo, "fetch", "--quiet", "origin")
        self.assertLess(time.monotonic() - started, 5, "the fetch waited for the sweep")
        last = os.path.join(self.state_dir(repo), "last-sweep.json")
        deadline = time.monotonic() + 60
        while not os.path.exists(last) and time.monotonic() < deadline:
            time.sleep(0.1)
        with open(last) as handle:
            report = json.load(handle)
        self.assertEqual(report["mode"], "report")
        self.assertEqual(report["repository"], os.path.realpath(repo))


class ThousandRefFetch(Sandbox):
    """The hook's cost on a fetch that moves 1,000 branches, against the same fetch without it."""

    def move(self, remote: str, target: str) -> None:
        self.git(
            remote,
            "update-ref",
            "--stdin",
            stdin="".join(
                "update refs/heads/b{:04d} {}\n".format(i, target) for i in range(BRANCHES)
            ),
        )

    def fetch(self, clone: str, git: str = "git", maintenance: bool = False) -> float:
        """Time a fetch; without `maintenance`, git's auto maintenance (which packs refs) is off."""
        start = time.perf_counter()
        subprocess.run(
            (
                git,
                "-C",
                clone,
                "-c",
                "maintenance.auto={}".format(str(maintenance).lower()),
                "fetch",
                "--quiet",
                "origin",
            ),
            env=self.env,
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        return time.perf_counter() - start

    def transactions(self, log: str, since: int) -> Tuple[int, int]:
        with open(log) as handle:
            new = [json.loads(line) for line in handle.readlines()[since:]]
        return len({e["pid"] for e in new}), len(new)

    def line_count(self, log: str) -> int:
        if not os.path.exists(log):
            return 0
        with open(log) as handle:
            return sum(1 for _ in handle)

    def test_the_hook_adds_its_own_work_under_5_ms_to_a_1000_ref_fetch(self):
        load, cores = os.getloadavg()[0], os.cpu_count() or 1
        if load > cores:
            self.skipTest(
                "load average {:.0f} is above {} cores: timings would measure the load".format(
                    load, cores
                )
            )
        seed = self.cloned("seed")
        # A chain of commits, so each round moves every branch forward, as a
        # normal fetch does: git batches fast-forwards into one transaction.
        chain = (self.git(seed, "rev-parse", "HEAD").strip(),) + tuple(
            self.commit(seed, "c{}.txt".format(i)) for i in range(ROUNDS + 4)
        )
        self.git(seed, "push", "--quiet", "origin", "main")
        remote = self.remote(seed)
        self.git(
            remote,
            "update-ref",
            "--stdin",
            stdin="".join(
                "create refs/heads/b{:04d} {}\n".format(i, chain[0]) for i in range(BRANCHES)
            ),
        )
        plain = os.path.join(self.tmp, "plain")
        empty = os.path.join(self.tmp, "empty")
        hooked = os.path.join(self.tmp, "hooked")
        for clone in (plain, empty, hooked):
            self.sh("git", "clone", "--quiet", remote, clone)
        self.write(
            os.path.join(empty, ".git", "hooks", "reference-transaction"), "#!/bin/sh\nexit 0\n"
        )
        os.chmod(os.path.join(empty, ".git", "hooks", "reference-transaction"), 0o755)
        log = self.install_shim(hooked, "refs/remotes/origin/main")
        clones = (plain, empty, hooked)

        def round_(i: int) -> Tuple[Tuple[float, float, float], Tuple[int, int]]:
            self.move(remote, chain[i + 2])
            lines = self.line_count(log)
            order = clones[i % 3 :] + clones[: i % 3]
            times = {clone: self.fetch(clone) for clone in order}
            return tuple(times[c] for c in clones), self.transactions(log, lines)

        # A first fetch warms each clone, and macOS's check of a new hook.
        self.move(remote, chain[1])
        for clone in clones:
            self.fetch(clone)
        rounds = tuple(round_(i) for i in range(ROUNDS))
        # A fetch takes about 500 ms and other work on the machine only ever
        # adds to it, by more than the budget, so each clone's figure is its
        # fastest fetch.
        mean = tuple(min(t[k] for t, _ in rounds) for k in range(3))
        added, over_empty = mean[2] - mean[0], mean[2] - mean[1]
        start = self.process_start()
        counts = {c for _, c in rounds}
        version = self.sh("git", "--version").strip()
        sys.stderr.write(
            "\n{}: a {}-ref fetch takes {:.1f} ms with no hook, {:.1f} ms with an empty hook, {:.1f} ms with "
            "the worktrees hook: {:+.1f} ms added, {:+.1f} ms over the empty hook, fastest of {} runs; "
            "a process start takes {:.1f} ms; (transactions, lines) per fetch: {}\n".format(
                version,
                BRANCHES,
                *(1000 * m for m in mean),
                1000 * added,
                1000 * over_empty,
                len(rounds),
                1000 * start,
                sorted(counts),
            )
        )
        forced = self.forced_transactions(remote, hooked, log, chain[0])
        sys.stderr.write(
            "{}: a fetch forcing {} branches back runs (transactions, lines) {}\n".format(
                version, FORCED, forced
            )
        )
        self.report_maintenance(remote, plain, hooked, log, chain[ROUNDS + 2])
        self.report_system_git(remote, hooked, log, chain[ROUNDS + 3])
        self.assertTrue(all(lines == BRANCHES for _, lines in counts), counts)
        if os.environ.get("NIX_BUILD_TOP"):
            # Nix's build sandbox checks every write the hook makes, one per
            # line, which no installed hook pays: the figures above are recorded,
            # and the budget is held outside it, by `tests` or plain unittest.
            sys.stderr.write("in a Nix build sandbox: the budget is not checked here\n")
            return
        # git starts any reference-transaction hook three times per transaction
        # (preparing, prepared, committed): three process starts before the
        # hook does anything. The budget is the hook's own work on top: the
        # `worktrees` program and awk, two process images, plus 5 ms.
        self.assertLess(
            over_empty,
            2 * start + BUDGET_SECONDS,
            "the hook added {:.1f} ms over an empty hook".format(1000 * over_empty),
        )

    def process_start(self) -> float:
        """What one process start costs here, the floor of any hook's run."""
        runs = 20
        begin = time.perf_counter()
        for _ in range(runs):
            subprocess.run(("awk", "BEGIN { exit 0 }"), check=True)
        return (time.perf_counter() - begin) / runs

    def forced_transactions(self, remote: str, hooked: str, log: str, back: str) -> Tuple[int, int]:
        """The same count for a forced update, on a few branches."""
        self.git(
            remote,
            "update-ref",
            "--stdin",
            stdin="".join("update refs/heads/b{:04d} {}\n".format(i, back) for i in range(FORCED)),
        )
        lines = self.line_count(log)
        self.fetch(hooked)
        return self.transactions(log, lines)

    def report_maintenance(
        self, remote: str, plain: str, hooked: str, log: str, target: str
    ) -> None:
        """A fetch with git's auto maintenance on: packing 1,000 loose refs runs the hook once per ref."""
        self.move(remote, target)
        lines = self.line_count(log)
        plain_time = self.fetch(plain, maintenance=True)
        hooked_time = self.fetch(hooked, maintenance=True)
        sys.stderr.write(
            "with auto maintenance packing the refs: {:.0f} ms without the hook, {:.0f} ms with it; "
            "(transactions, lines) {}\n".format(
                1000 * plain_time, 1000 * hooked_time, self.transactions(log, lines)
            )
        )

    def report_system_git(self, remote: str, hooked: str, log: str, target: str) -> None:
        """How many transactions macOS's `/usr/bin/git` runs for a 1,000-ref fetch, when it runs here."""
        system_git = "/usr/bin/git"
        try:
            version = subprocess.run(
                (system_git, "--version"),
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                universal_newlines=True,
                timeout=30,
            )
        except (OSError, subprocess.SubprocessError):
            return
        if version.returncode != 0:
            return
        self.move(remote, target)
        lines = self.line_count(log)
        start = time.perf_counter()
        self.fetch(hooked, system_git)
        sys.stderr.write(
            "{} ({}): the same fast-forward fetch runs (transactions, lines) {} in {:.0f} ms\n".format(
                system_git,
                version.stdout.strip(),
                self.transactions(log, lines),
                1000 * (time.perf_counter() - start),
            )
        )
