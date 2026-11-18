"""Tests of `sweep`, `status`, `verdict`, `log` and `archive` over temporary repositories."""

from __future__ import annotations

import fcntl
import json
import os
import time
from typing import Dict, Tuple

from worktrees import rules
from worktrees.tests.helpers import Sandbox


class Removal(Sandbox):
    """Each removal rule, passing together and refusing on its own."""

    def removable(self) -> Tuple[str, str]:
        repo = self.cloned()
        self.git(repo, "config", "worktrees.idle", "0s")
        wt = self.landed_branch(repo, "feat", worktree=True)
        return repo, wt

    def sweep(self, repo: str, *args: str) -> Dict:
        return self.wt_json("sweep", *args, repo)

    def test_a_removable_worktree_and_its_branch_go(self):
        repo, wt = self.removable()
        report = self.sweep(repo)
        self.assertEqual(self.entry(report, "worktrees", wt)["action"], "remove")
        self.assertTrue(self.entry(report, "worktrees", wt)["done"])
        self.assertNotIn(wt, self.listed(repo))
        self.assertFalse(os.path.exists(wt))
        self.assertNotIn("feat", self.branches(repo))
        events = [e["event"] for e in self.events(repo)]
        self.assertIn("remove", events)
        self.assertIn("branch-delete", events)

    def test_a_broken_worktree_is_kept_and_the_rest_still_swept(self):
        repo, wt = self.removable()
        gone = self.landed_branch(repo, "gone-git", worktree=True)
        elsewhere = self.landed_branch(repo, "elsewhere", worktree=True)
        os.remove(os.path.join(gone, ".git"))
        self.write(os.path.join(elsewhere, ".git"), "gitdir: /nonexistent/worktrees/elsewhere\n")
        dry = self.sweep(repo, "--dry-run")
        self.assertNotIn("error", dry["repositories"][0])
        for path, branch in ((gone, "gone-git"), (elsewhere, "elsewhere")):
            entry = self.entry(dry, "worktrees", path)
            self.assertEqual(entry["action"], "keep", entry)
            self.assertEqual(self.codes(entry)[0], "broken", entry)
            self.assertIn("not a working worktree", entry["reasons"][0]["detail"])
            self.assertTrue(entry["survives"][0].startswith("merged into origin/main"), entry)
            self.assertEqual(self.entry(dry, "branches", branch)["action"], "keep")
        self.assertEqual(self.entry(dry, "worktrees", wt)["action"], "remove")
        self.assertEqual(self.entry(dry, "branches", "feat")["action"], "delete")
        report = self.sweep(repo)
        self.assertFalse(os.path.exists(wt))
        self.assertTrue(os.path.isdir(gone))
        self.assertTrue(os.path.isdir(elsewhere))
        self.assertIn("gone-git", self.branches(repo))
        self.assertIn("elsewhere", self.branches(repo))
        self.assertEqual(self.entry(report, "worktrees", gone)["action"], "keep")

    def assert_kept_for(self, repo: str, wt: str, code: str) -> None:
        report = self.sweep(repo)
        entry = self.entry(report, "worktrees", wt)
        self.assertEqual(entry["action"], "keep", entry)
        self.assertEqual(self.codes(entry), [code])
        self.assertIn(wt, self.listed(repo))
        self.assertIn("feat", self.branches(repo))

    def test_the_main_worktree_is_kept(self):
        repo, _ = self.removable()
        entry = self.entry(self.sweep(repo, "--dry-run"), "worktrees", os.path.realpath(repo))
        self.assertEqual(self.codes(entry), ["main-worktree"])

    def test_a_lock_keeps_it(self):
        repo, wt = self.removable()
        self.git(repo, "worktree", "lock", "--reason", "neil since 2026-11-18T09:12:00Z", wt)
        self.assert_kept_for(repo, wt, "locked")

    def test_an_unlanded_head_keeps_it(self):
        repo, wt = self.removable()
        self.commit(wt, "more.txt")
        self.assert_kept_for(repo, wt, "not-landed")

    def test_an_uncommitted_change_keeps_it(self):
        repo, wt = self.removable()
        self.write(os.path.join(wt, "feat.txt"), "changed\n")
        self.assert_kept_for(repo, wt, "uncommitted")

    def test_an_untracked_file_keeps_it(self):
        repo, wt = self.removable()
        self.write(os.path.join(wt, "deep", "new.txt"))
        self.assert_kept_for(repo, wt, "uncommitted")

    def test_a_kept_file_of_its_own_keeps_it(self):
        repo, wt = self.removable()
        self.write(os.path.join(repo, ".env"), "A=1\n")
        self.write(os.path.join(wt, ".env"), "A=2\n")
        self.assert_kept_for(repo, wt, "keep-pattern-files")
        entry = self.entry(self.sweep(repo, "--dry-run"), "worktrees", wt)
        self.assertEqual(entry["keep_files"], [".env"])

    def test_a_kept_file_alike_the_main_checkouts_does_not(self):
        repo, wt = self.removable()
        self.write(os.path.join(repo, ".env"), "A=1\n")
        self.write(os.path.join(wt, ".env"), "A=1\n")
        self.write(os.path.join(repo, ".claude", "settings.local.json"), "{}\n")
        self.write(os.path.join(wt, ".claude", "settings.local.json"), "{}\n")
        self.assertEqual(self.entry(self.sweep(repo), "worktrees", wt)["action"], "remove")

    def test_a_kept_file_inside_an_ignored_folder_keeps_it(self):
        repo, wt = self.removable()
        self.write(os.path.join(wt, ".claude", "settings.local.json"), "{}\n")
        self.assert_kept_for(repo, wt, "keep-pattern-files")

    def test_build_output_does_not_keep_it(self):
        repo, wt = self.removable()
        self.write(os.path.join(wt, "target", "debug", "app"))
        self.write(os.path.join(wt, "junk.o"))
        self.assertEqual(self.entry(self.sweep(repo), "worktrees", wt)["action"], "remove")

    def test_recent_activity_keeps_it(self):
        repo, wt = self.removable()
        self.git(repo, "config", "worktrees.idle", "1h")
        self.assert_kept_for(repo, wt, "not-idle")

    def test_a_folder_gone_is_pruned_and_logged(self):
        repo, wt = self.removable()
        self.git(repo, "config", "--unset", "worktrees.idle")
        os.rename(wt, wt + ".moved")
        report = self.sweep(repo)
        self.assertEqual(self.entry(report, "worktrees", wt)["action"], "prune")
        self.assertNotIn(wt, self.listed(repo))
        self.assertIn("prune", [e["event"] for e in self.events(repo)])


class Branches(Sandbox):
    """Every row of the table for a local branch with no worktree."""

    def setUp(self):
        super().setUp()
        repo = self.cloned()
        self.repo_path = repo
        self.landed_branch(repo, "landed")
        self.tips = {
            "preview/all": self.local_branch(repo, "preview/all"),
            "in-review": self.local_branch(repo, "in-review", push=True),
            "gone": self.local_branch(repo, "gone", push=True),
            "local": self.local_branch(repo, "local"),
            "diverged": self.local_branch(repo, "diverged", push=True),
            "rebased": self.local_branch(repo, "rebased"),
        }
        self.git(repo, "push", "--quiet", "origin", "--delete", "gone")
        self.git(repo, "fetch", "--quiet", "--prune", "origin")
        # `diverged`: the remote gains a commit the local branch lacks, and the reverse.
        other = self.other_clone(repo, "diverger")
        self.git(other, "checkout", "--quiet", "diverged")
        self.commit(other, "theirs.txt")
        self.git(other, "push", "--quiet", "origin", "diverged")
        self.git(repo, "checkout", "--quiet", "diverged")
        self.commit(repo, "ours.txt")
        self.git(repo, "checkout", "--quiet", "main")
        self.tips["diverged"] = self.git(repo, "rev-parse", "diverged").strip()
        # `rebased`: the same change reaches main as another commit.
        merger = self.other_clone(repo, "rebaser")
        self.commit(merger, "rebased.txt", message="the same change, rebased")
        self.git(merger, "push", "--quiet", "origin", "main")
        self.git(repo, "fetch", "--quiet", "origin")

    def test_every_row(self):
        repo = self.repo_path
        dry = self.wt_json("sweep", "--dry-run", repo)
        states = {
            b["branch"]: (b["state"], b["action"]) for b in dry["repositories"][0]["branches"]
        }
        self.assertEqual(
            states,
            {
                "main": ("default-branch", "keep"),
                "landed": ("landed", "delete"),
                "preview/all": ("preview", "archive+delete"),
                "rebased": ("content-landed", "archive+delete"),
                "in-review": ("in-review", "keep"),
                "gone": ("upstream-gone", "keep"),
                "local": ("local-only", "keep"),
                "diverged": ("diverged", "keep"),
            },
        )
        self.assertTrue(
            all(b["would"] for b in dry["repositories"][0]["branches"] if b["action"] != "keep")
        )
        survives = {b["branch"]: b["survives"] for b in dry["repositories"][0]["branches"]}
        self.assertEqual(survives["landed"][0], "merged into origin/main")
        self.assertIn(
            "pushed as origin/in-review at " + self.tips["in-review"][:12], survives["in-review"]
        )
        self.assertTrue(
            survives["local"][-1].endswith(
                "(would write to the local backup {}, would compare)".format(self.backup(repo))
            ),
            survives["local"],
        )
        self.assertTrue(survives["rebased"][0].startswith("archive refs/archive/rebased/"))
        self.wt_json("sweep", repo)
        self.assertEqual(
            sorted(self.branches(repo)), ["diverged", "gone", "in-review", "local", "main"]
        )
        archived = self.archives(repo)
        by_branch = {"/".join(ref.split("/")[2:-1]): obj for ref, obj in archived.items()}
        for name in ("rebased", "preview/all", "in-review", "gone", "local", "diverged"):
            self.assertEqual(by_branch[name], self.tips[name], name)
        self.assertNotIn("landed", by_branch)

    def test_archives_go_to_the_local_backup_and_never_to_origin(self):
        repo = self.repo_path
        report = self.wt_json("sweep", repo)["repositories"][0]
        self.assertEqual(
            self.backup(repo),
            os.path.join(
                self.env["HOME"],
                "backups",
                "git-archive",
                os.path.basename(self.tmp),
                "repo.remote.git",
            ),
        )
        self.assertEqual(report["backup"]["path"], self.backup(repo))
        self.assertTrue(self.archives(repo))
        for where in (self.remote(repo), repo):
            self.assertEqual(
                self.git(where, "for-each-ref", "--format=%(refname)", "refs/archive"), "", where
            )
        restored = self.other_clone(repo, "restored")
        self.git(restored, "fetch", "--quiet", self.backup(repo), "refs/archive/*:refs/archive/*")
        self.assertIn(self.tips["rebased"], self.archives(restored, local=True).values())
        top = os.path.join(self.env["HOME"], "backups")
        for readme in (
            os.path.join(top, "git-archive", "README.md"),
            os.path.join(top, "git-archive", os.path.basename(self.tmp), "README.md"),
        ):
            with open(readme) as handle:
                self.assertIn("git fetch", handle.read(), readme)
        with open(os.path.join(top, "README.md")) as handle:
            self.assertIn("`git-archive/`", handle.read())

    def test_an_archive_that_cannot_be_written_keeps_its_branch(self):
        repo = self.repo_path
        self.write(os.path.join(self.env["HOME"], "backups", "git-archive"), "not a folder\n")
        result = self.wt("sweep", "--json", repo)
        self.assertEqual(result.returncode, 1, result.stderr)
        rebased = self.entry(json.loads(result.stdout), "branches", "rebased")
        self.assertFalse(rebased["done"])
        self.assertIn("backup", rebased["error"])
        self.assertIn("rebased", self.branches(repo))
        self.assertIn("preview/all", self.branches(repo))
        self.assertNotIn("landed", self.branches(repo))

    def test_a_missing_object_in_the_backup_fails_the_compare_and_keeps_the_branch(self):
        repo = self.repo_path
        # A blob no other branch holds, so the sweep's own push can't bring it back.
        self.git(repo, "checkout", "--quiet", "-b", "preview/solo", "main")
        tip = self.commit(repo, "solo.txt", "only on preview/solo\n")
        self.git(repo, "checkout", "--quiet", "main")
        self.wt_json("archive", "preview/solo", cwd=repo)
        blob = self.git(repo, "rev-parse", "preview/solo:solo.txt").strip()
        os.remove(os.path.join(self.backup(repo), "objects", blob[:2], blob[2:]))
        result = self.wt("sweep", "--json", repo)
        self.assertEqual(result.returncode, 1, result.stderr)
        report = json.loads(result.stdout)
        self.assertIn("preview/solo", self.branches(repo))
        self.assertFalse(self.entry(report, "branches", "preview/solo")["done"])
        backup = report["repositories"][0]["backup"]
        solo = next(c for c in backup["compares"] if c["branch"] == "preview/solo")
        self.assertEqual((solo["ok"], solo["tip"]), (False, tip))
        self.assertEqual(solo["checks"], {"tip": True, "history": False, "fsck": False})
        self.assertIn("compare failed: history, fsck", solo["error"])
        self.assertGreaterEqual(backup["failed"], 1)
        self.assertNotIn("landed", self.branches(repo))

    def test_the_sweep_ends_by_asking_for_a_backup_of_the_new_archives(self):
        repo = self.repo_path
        top = os.path.join(self.env["HOME"], "backups")
        dry = self.wt("sweep", "--dry-run", repo)
        self.assertEqual(
            dry.stdout.splitlines()[-1],
            "6 new archives would be written in {0}/git-archive/; back up {0}/ once the sweep "
            "has run".format(top),
        )
        done = self.wt("sweep", repo)
        self.assertEqual(
            done.stdout.splitlines()[-1],
            "6 new archives in {0}/git-archive/; back up {0}/".format(top),
        )
        again = self.wt("sweep", repo)
        self.assertNotIn("new archives", again.stdout)

    def test_a_tip_only_in_a_local_ref_is_still_archived_before_it_goes(self):
        repo = self.repo_path
        self.git(repo, "tag", "rebased-tag", "rebased")
        dry = self.wt_json("sweep", "--dry-run", repo)
        self.assertEqual(self.entry(dry, "branches", "rebased")["action"], "archive+delete")
        self.wt_json("sweep", repo)
        self.assertNotIn("rebased", self.branches(repo))
        self.assertIn(self.tips["rebased"], self.archives(repo).values())

    def test_deletion_uses_the_tip_it_tested(self):
        repo = self.repo_path
        events = self.wt_json("sweep", repo)
        deleted = [e for e in self.events(repo) if e["event"] == "branch-delete"]
        self.assertEqual(
            sorted(e["ref"] for e in deleted),
            ["refs/heads/landed", "refs/heads/preview/all", "refs/heads/rebased"],
        )
        self.assertTrue(all(e["old"] for e in deleted))
        self.assertEqual(events["totals"]["branches"]["keep"], 5)

    def test_a_second_sweep_writes_no_second_backfill(self):
        repo = self.repo_path
        self.wt_json("sweep", repo)
        before = len(self.archives(repo))
        second = self.wt_json("sweep", repo)["repositories"][0]
        self.assertFalse(second["backfill"]["needed"])
        self.assertEqual(len(self.archives(repo)), before)


class Backfill(Sandbox):
    def test_snapshots_and_archives_come_before_any_removal(self):
        repo = self.cloned()
        self.git(repo, "config", "worktrees.idle", "0s")
        wt = self.landed_branch(repo, "feat", worktree=True)
        tip = self.local_branch(repo, "unmerged")
        report = self.wt_json("sweep", repo)["repositories"][0]
        self.assertTrue(report["backfill"]["needed"])
        self.assertEqual([a["branch"] for a in report["backfill"]["archives"]], ["unmerged"])
        kinds = [e["event"] for e in self.events(repo)]
        first_removal = min(kinds.index("remove"), kinds.index("branch-delete"))
        self.assertTrue(all(k in ("snapshot", "archive") for k in kinds[:first_removal]), kinds)
        snapshots = [e for e in self.events(repo) if e["event"] == "snapshot"]
        self.assertIn(wt, [e.get("worktree") for e in snapshots])
        self.assertIn("refs/heads/unmerged", [e.get("ref") for e in snapshots])
        self.assertIn(tip, self.archives(repo).values())

    def test_a_branch_missing_from_the_log_gets_a_snapshot_later(self):
        repo = self.cloned()
        self.wt_json("sweep", repo)
        self.local_branch(repo, "later")
        self.wt_json("sweep", repo)
        refs = [e.get("ref") for e in self.events(repo) if e["event"] == "snapshot"]
        self.assertEqual(refs.count("refs/heads/later"), 1)

    def test_an_archive_in_the_clone_the_default_branch_reaches_is_pruned(self):
        repo = self.cloned()
        tip = self.local_branch(repo, "work", push=True)
        self.git(repo, "update-ref", "refs/archive/work/2026-01-01T000000Z", tip)
        self.wt_json("sweep", repo)
        self.assertEqual(len(self.archives(repo, local=True)), 1)
        self.merge_into_main(repo, "work")
        report = self.wt_json("sweep", repo)["repositories"][0]
        self.assertEqual(len(report["archives_pruned"]), 1)
        self.assertEqual(self.archives(repo, local=True), {})
        self.assertIn(tip, self.archives(repo).values())


class Rescue(Sandbox):
    def test_a_branch_deleted_by_hand_is_archived_from_the_log(self):
        repo = self.cloned()
        self.wt_json("sweep", repo)
        self.install_shim(repo)
        tip = self.local_branch(repo, "doomed")
        self.git(repo, "branch", "--quiet", "-D", "doomed")
        updates = [
            e
            for e in self.events(repo)
            if e["event"] == "ref-update" and e["ref"] == "refs/heads/doomed"
        ]
        self.assertEqual(updates[-1]["new"], "0" * len(tip))
        self.assertIn(tip, [e["new"] for e in updates])
        dry = self.wt_json("sweep", "--dry-run", repo)["repositories"][0]
        self.assertEqual(
            [(r["branch"], r["old"], r["would"]) for r in dry["rescues"]], [("doomed", tip, True)]
        )
        self.wt_json("sweep", repo)
        self.assertIn(tip, self.archives(repo).values())
        self.assertIn("rescue", [e["event"] for e in self.events(repo)])
        again = self.wt_json("sweep", repo)["repositories"][0]
        self.assertEqual(again["rescues"], [])


class Locks(Sandbox):
    def test_live_and_dead_lock_holders_are_kept_and_reported(self):
        repo = self.cloned()
        self.git(repo, "config", "worktrees.idle", "0s")
        self.git(repo, "config", "worktrees.lockReport", "0s")
        live = self.landed_branch(repo, "live", worktree=True)
        dead = self.landed_branch(repo, "dead", worktree=True)
        person = self.landed_branch(repo, "person", worktree=True)
        sleeper = self.background(("sleep", "60"))
        start = rules.parse_lstart(self.lstart(sleeper.pid), utc=True)
        self.git(
            repo,
            "worktree",
            "lock",
            "--reason",
            "claude agent agent-a1b2 (pid {} start {})".format(sleeper.pid, int(start * 1000)),
            live,
        )
        self.git(
            repo,
            "worktree",
            "lock",
            "--reason",
            "claude agent agent-c3d4 (pid {} start {})".format(
                self.dead_pid(), int(time.time() * 1000)
            ),
            dead,
        )
        self.git(
            repo,
            "worktree",
            "lock",
            "--reason",
            "headless overnight2-03 pid {} since {}".format(sleeper.pid, rules.iso(start + 1)),
            person,
        )
        report = self.wt_json("sweep", repo)
        for path, alive in ((live, True), (dead, False), (person, True)):
            entry = self.entry(report, "worktrees", path)
            self.assertEqual(entry["action"], "keep")
            self.assertEqual(self.codes(entry), ["locked"])
            self.assertEqual(entry["lock"]["alive"], alive, (path, entry["lock"]))
            self.assertTrue(entry["lock"]["report"])
            self.assertIn(path, self.listed(repo))
        reported = report["repositories"][0]["lock_reports"]
        self.assertEqual(sorted(r["path"] for r in reported), sorted((live, dead, person)))
        self.assertEqual(
            sorted(e["worktree"] for e in self.events(repo) if e["event"] == "lock-report"),
            sorted((live, dead, person)),
        )
        listed = self.git(repo, "worktree", "list", "--porcelain")
        self.assertEqual(listed.count("\nlocked "), 3)


class Pending(Sandbox):
    def test_a_sweep_that_finds_the_lock_held_leaves_sweep_pending(self):
        repo = self.cloned()
        state = self.state_dir(repo)
        os.makedirs(state)
        fd = os.open(os.path.join(state, "sweep.lock"), os.O_RDWR | os.O_CREAT)
        self.addCleanup(os.close, fd)
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        report = self.wt_json("sweep", repo)["repositories"][0]
        self.assertIn("another sweep", report["skipped"])
        self.assertTrue(os.path.exists(os.path.join(state, "sweep.pending")))

    def test_sweep_pending_is_picked_up_with_one_more_run(self):
        repo = self.cloned()
        state = self.state_dir(repo)
        self.write(os.path.join(state, "sweep.pending"), "now\n")
        report = self.wt_json("sweep", repo)["repositories"][0]
        self.assertEqual(report["runs"], 2)
        self.assertFalse(os.path.exists(os.path.join(state, "sweep.pending")))
        self.assertTrue(os.path.exists(os.path.join(state, "last-sweep.json")))


class DryRun(Sandbox):
    def snapshot(self, repo: str, roots) -> Dict:
        def walk(root: str):
            return {
                os.path.join(folder, name): os.lstat(os.path.join(folder, name)).st_mtime_ns
                for folder, dirs, files in os.walk(root)
                for name in dirs + files + ["."]
            }

        return {
            "refs": self.git(repo, "for-each-ref", "--format=%(refname) %(objectname)"),
            "worktrees": self.git(repo, "worktree", "list", "--porcelain"),
            "files": {root: walk(root) for root in roots if os.path.exists(root)},
            "state": os.path.exists(self.state_dir(repo)),
        }

    def test_a_dry_run_changes_nothing(self):
        repo = self.cloned()
        self.git(repo, "config", "worktrees.idle", "0s")
        landed = self.landed_branch(repo, "feat", worktree=True)
        self.landed_branch(repo, "done")
        self.local_branch(repo, "rebased")
        merger = self.other_clone(repo, "rebaser")
        self.commit(merger, "rebased.txt")
        self.git(merger, "push", "--quiet", "origin", "main")
        self.git(repo, "fetch", "--quiet", "origin")
        idle = self.add_worktree(repo, "idle", start="rebased")
        self.write(os.path.join(idle, "junk.o"))
        two_days_ago = time.time() - 2 * 86400
        gitdir = self.git(idle, "rev-parse", "--absolute-git-dir").strip()
        for name in ("HEAD", "index", "logs/HEAD"):
            os.utime(os.path.join(gitdir, name), (two_days_ago, two_days_ago))
        self.write(os.path.join(idle, "untracked-in-idle.txt"))
        locked = self.landed_branch(repo, "locked", worktree=True)
        self.git(
            repo,
            "worktree",
            "lock",
            "--reason",
            "claude agent agent-x (pid {} start 1)".format(self.dead_pid()),
            locked,
        )
        roots = (
            repo,
            repo + ".worktrees",
            self.remote(repo),
            os.path.join(self.env["XDG_STATE_HOME"], "worktrees"),
        )
        before = self.snapshot(repo, roots)
        report = self.wt_json("sweep", "--dry-run", repo)
        self.wt_json("status", repo)
        self.wt_json("verdict", landed, cwd=repo)
        self.wt_json("verdict", "rebased", cwd=repo)
        after = self.snapshot(repo, roots)
        changed = sorted(
            path
            for root in roots
            for path in set(before["files"].get(root, {})) | set(after["files"].get(root, {}))
            if before["files"].get(root, {}).get(path) != after["files"].get(root, {}).get(path)
        )
        self.assertEqual(changed, [])
        self.assertEqual(after, before)
        self.assertFalse(before["state"])
        self.assertFalse(os.path.exists(os.path.join(self.env["HOME"], "backups")))
        r = report["repositories"][0]
        self.assertTrue(report["dry_run"])
        self.assertEqual(self.entry(report, "worktrees", landed)["action"], "remove")
        self.assertTrue(self.entry(report, "worktrees", landed)["would"])
        self.assertEqual(self.entry(report, "branches", "rebased")["action"], "archive+delete")
        self.assertEqual(self.codes(self.entry(report, "worktrees", idle)), ["uncommitted"])
        self.assertEqual(self.entry(report, "branches", "done")["action"], "delete")
        self.assertTrue(r["backfill"]["needed"])
        self.assertTrue(all(a["would"] for a in r["backfill"]["archives"]))
        self.assertGreater(r["totals"]["snapshots"], 0)


class Defaults(Sandbox):
    def test_no_default_branch_removes_nothing_and_says_why(self):
        repo = self.cloned()
        self.git(repo, "config", "worktrees.idle", "0s")
        wt = self.landed_branch(repo, "feat", worktree=True)
        self.git(repo, "remote", "set-head", "origin", "--delete")
        other = self.cloned("second")
        report = self.wt_json("sweep", repo, other)
        first, second = report["repositories"]
        self.assertIn("no default branch", first["problem"])
        self.assertEqual(self.codes(self.entry(report, "worktrees", wt)), ["no-default-branch"])
        self.assertIn(wt, self.listed(repo))
        self.assertIsNone(second["problem"])
        self.assertEqual(report["totals"]["without_default_branch"], 1)

    def test_default_branch_for_every_checkout_and_for_one(self):
        repo = self.cloned()
        self.git(repo, "config", "worktrees.idle", "0s")
        wt = self.landed_branch(repo, "feat", worktree=True)
        self.git(repo, "remote", "set-head", "origin", "--delete")
        everywhere = self.wt_json("sweep", "--dry-run", "--default-branch", "main", repo)
        self.assertEqual(self.entry(everywhere, "worktrees", wt)["action"], "remove")
        one = self.wt_json("sweep", "--dry-run", "--default-branch", repo + "=main", repo)
        self.assertEqual(one["repositories"][0]["default_ref"], "refs/remotes/origin/main")
        missing = self.wt_json("sweep", "--dry-run", "--default-branch", "trunk", repo)
        self.assertIn("does not exist", missing["repositories"][0]["problem"])

    def test_all_finds_main_checkouts_one_level_down(self):
        a, b = self.cloned("a"), self.cloned("b")
        os.makedirs(os.path.join(self.tmp, "plain"))
        report = self.wt_json("status", "--all", self.tmp)
        self.assertEqual(
            sorted(r["repository"] for r in report["repositories"]),
            sorted(os.path.realpath(p) for p in (a, b)),
        )

    def test_not_a_checkout_is_an_error_beside_the_others(self):
        repo = self.cloned()
        plain = os.path.join(self.tmp, "plain")
        os.makedirs(plain)
        report = self.wt_json("sweep", "--dry-run", plain, repo, code=1)
        self.assertIn("not a git checkout", report["repositories"][0]["error"])
        self.assertIn("worktrees", report["repositories"][1])
        self.assertEqual(report["totals"]["errors"], 1)

    def test_the_json_shape(self):
        repo = self.cloned()
        report = self.wt_json("sweep", "--dry-run", repo)
        self.assertEqual(
            sorted(report), ["backup_note", "dry_run", "mode", "repositories", "time", "totals"]
        )
        self.assertTrue(
            {
                "repository",
                "default_branch",
                "problem",
                "worktrees",
                "branches",
                "backfill",
                "archives_pruned",
                "rescues",
                "lock_reports",
                "backup",
                "totals",
            }
            <= set(report["repositories"][0])
        )


class LogAndArchive(Sandbox):
    def test_archive_then_query_the_log(self):
        repo = self.cloned()
        tip = self.local_branch(repo, "keepme")
        would = self.wt_json("archive", "--dry-run", "keepme", cwd=repo)
        self.assertTrue(would["would"])
        self.assertEqual(self.archives(repo), {})
        done = self.wt_json("archive", "keepme", cwd=repo)
        self.assertTrue(done["ref"].startswith("refs/archive/keepme/"))
        self.assertEqual(self.archives(repo), {done["ref"]: tip})
        found = self.wt_json("log", "--event", "archive", "--branch", "keepme", cwd=repo)
        self.assertEqual([e["ref"] for e in found], [done["ref"]])
        self.assertEqual(self.wt_json("log", "--event", "remove", cwd=repo), [])
        self.assertEqual(len(self.wt_json("log", "--since", "1h", cwd=repo)), 1)

    def test_verdict_lists_every_test(self):
        repo = self.cloned()
        wt = self.add_worktree(repo, "feat")
        result = self.wt_json("verdict", wt, cwd=repo)
        names = [t["name"] for t in result["tests"]]
        self.assertEqual(
            names,
            ["not-main", "default-branch", "unlocked", "landed", "clean", "keep-files", "idle"],
        )
        self.assertEqual(result["action"], "keep")
        self.assertEqual(self.codes(result), ["not-idle"])
