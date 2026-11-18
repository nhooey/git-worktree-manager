"""What the shell learns about a repository, read-only, for `worktrees.rules`.

`collect` runs git, `ps` and `stat` and returns a `rules.RepoFacts`. It
writes nothing, not even a file's time: every git command runs with
`--no-optional-locks`, so `git status` leaves the index alone, and no
command writes an object.
"""

from __future__ import annotations

import filecmp
import glob
import os
from typing import Dict, FrozenSet, Mapping, Optional, Tuple

from . import core, rules, system


class NotARepository(Exception):
    pass


# Keep patterns and kept files ---------------------------------------------------


def keep_patterns(cfg: Mapping, worktree: str, main: str) -> Tuple[str, ...]:
    """The defaults' keep patterns, then the repository's `.worktreeinclude` and `.worktree-keep`."""

    def from_file(name: str) -> Tuple[str, ...]:
        here = system.read_text(os.path.join(worktree, name))
        return core.parse_pattern_file(
            (here if here is not None else system.read_text(os.path.join(main, name))) or ""
        )

    return tuple(cfg["keep"]) + from_file(".worktreeinclude") + from_file(".worktree-keep")


def ignored_entries(worktree: str) -> Tuple[str, ...]:
    """Ignored files and folders, without walking into an ignored folder such as `target/`."""
    return core.split_nul(
        system.git(
            worktree,
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "--no-empty-directory",
        )
    )


def same_content(mine: str, theirs: str) -> bool:
    """Whether `mine`, a file or a folder, is byte for byte `theirs`, file by file."""
    if os.path.islink(mine):
        return os.path.islink(theirs) and os.readlink(mine) == os.readlink(theirs)
    if os.path.isfile(mine):
        return os.path.isfile(theirs) and filecmp.cmp(mine, theirs, shallow=False)
    if os.path.isdir(mine):
        return os.path.isdir(theirs) and all(
            same_content(os.path.join(folder, name), os.path.join(theirs, rel, name))
            for folder, _, files in os.walk(mine)
            for rel in (os.path.relpath(folder, mine),)
            for name in files
        )
    return False


def kept_files(
    worktree: str, main: str, entries: Tuple[str, ...], patterns: Tuple[rules.Pattern, ...]
) -> Tuple[str, ...]:
    """Ignored entries a keep pattern holds, other than those the main checkout has alike."""
    direct = rules.kept_entries(entries, patterns)
    probed = tuple(
        os.path.relpath(hit, worktree) + ("/" if os.path.isdir(hit) else "")
        for probe in rules.probes(entries, patterns)
        for hit in sorted(glob.glob(os.path.join(worktree, probe)))
    )
    return tuple(
        rel
        for rel in dict.fromkeys(direct + probed)
        if not same_content(
            os.path.join(worktree, rel.rstrip("/")), os.path.join(main, rel.rstrip("/"))
        )
    )


# Branches -------------------------------------------------------------------------

HEADS_FORMAT = (
    "%(refname)%00%(objectname)%00%(upstream)%00%(upstream:track,nobracket)%00%(committerdate:unix)"
)


def heads(main: str) -> Tuple[Tuple[str, ...], ...]:
    return tuple(
        tuple(line.split("\0"))
        for line in system.git(
            main, "for-each-ref", "--format=" + HEADS_FORMAT, "refs/heads"
        ).splitlines()
        if line.count("\0") == 4
    )


def content_landed(
    main: str, default_ref: str, tip: str, tree: Mapping[str, Tuple[str, str]]
) -> bool:
    """Whether merging `tip` into the default branch would leave the default branch's tree as it is.

    Read-only: every path the branch changed since its merge base must hold
    the same object in the default branch. `git merge-tree --write-tree`
    would answer the same, but writing a tree the repository already has
    freshens that object's file, even through an alternate, which a dry
    run must not do.
    """
    base = system.git_result(main, "merge-base", default_ref, tip)
    if base.returncode != 0:
        return False
    changes = rules.parse_raw_diff(
        system.git(
            main,
            "diff-tree",
            "-r",
            "-z",
            "--no-renames",
            "--no-commit-id",
            base.stdout.strip(),
            tip,
        )
    )
    return rules.changes_present(changes, tree)


def branch_age(common_dir: str, name: str, committed: str, now: float) -> Optional[float]:
    """Seconds since the branch last moved: its reflog's time, else its tip's commit time."""
    times = tuple(
        t
        for t in (
            system.mtime(os.path.join(common_dir, "logs", "refs", "heads", name)),
            float(committed) if committed.isdigit() else None,
        )
        if t is not None
    )
    return max(0.0, now - max(times)) if times else None


# Defaults --------------------------------------------------------------------------


def default_branch(main: str, override: Optional[str]) -> Tuple[Optional[str], Optional[str]]:
    """`(branch, problem)`: `--default-branch`, else the target of `refs/remotes/origin/HEAD`."""
    head = (
        None
        if override
        else system.git_result(
            main, "symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"
        ).stdout.strip()
    )
    prefix = "refs/remotes/origin/"
    named = override or (head[len(prefix) :] if head and head.startswith(prefix) else None)
    return (
        (named, None)
        if named
        else (
            None,
            "no default branch: refs/remotes/origin/HEAD is unset and no --default-branch was given",
        )
    )


# Collecting ---------------------------------------------------------------------------


def lock_facts(
    reason: Optional[str], git_dir: Optional[str], now: float, starts: Dict[int, Optional[float]]
) -> Optional[rules.LockFacts]:
    if reason is None:
        return None
    lock = rules.parse_lock(reason)
    taken = (
        lock.since
        if lock.since is not None
        else (system.mtime(os.path.join(git_dir, "locked")) if git_dir else None)
    )
    start = (
        starts.setdefault(lock.pid, system.process_start(lock.pid))
        if lock.pid is not None
        else None
    )
    return rules.LockFacts(
        lock=lock,
        age=max(0.0, now - taken) if taken is not None else None,
        alive=rules.process_alive(lock, start),
        process_start=start,
    )


def idle_seconds(git_dir: Optional[str], now: float) -> Optional[float]:
    times = (
        tuple(
            t
            for t in (
                system.mtime(os.path.join(git_dir, name)) for name in ("HEAD", "logs/HEAD", "index")
            )
            if t is not None
        )
        if git_dir
        else ()
    )
    return max(0.0, now - max(times)) if times else None


def worktree_facts(
    wt: core.Worktree,
    main: str,
    common_dir: str,
    default_ref: Optional[str],
    tips: Mapping[str, str],
    merged: FrozenSet[str],
    patterns: Tuple[rules.Pattern, ...],
    now: float,
    starts: Dict[int, Optional[float]],
) -> rules.WorktreeFacts:
    exists = os.path.isdir(wt.path)
    git_dir = common_dir if wt.is_main else (system.linked_git_dir(wt.path) if exists else None)
    landed = (
        None
        if default_ref is None
        else ("refs/heads/" + wt.branch) in merged
        if wt.branch and tips.get(wt.branch) == wt.head
        else bool(wt.head)
        and system.git_ok(main, "merge-base", "--is-ancestor", wt.head, default_ref)
    )
    broken = worktree_problem(wt.path, common_dir) if exists and not wt.is_main else None
    inspect = exists and not wt.is_main and broken is None
    changes = (
        tuple(
            line
            for line in system.git(
                wt.path, "status", "--porcelain", "--untracked-files=all"
            ).splitlines()
            if line
        )
        if inspect
        else ()
    )
    entries = ignored_entries(wt.path) if inspect else ()
    kept = kept_files(wt.path, main, entries, patterns) if entries else ()
    held = frozenset(rules.kept_entries(entries, patterns))
    return rules.WorktreeFacts(
        path=wt.path,
        branch=wt.branch,
        head=wt.head,
        is_main=wt.is_main,
        exists=exists,
        lock=lock_facts(wt.locked, git_dir, now, starts),
        landed=landed,
        changes=changes,
        kept=kept,
        cleanable=tuple(e for e in entries if e not in held),
        idle=idle_seconds(git_dir, now),
        broken=broken,
    )


def worktree_problem(path: str, common_dir: str) -> Optional[str]:
    """Why git can't read `path` as a worktree of this repository; None when it can."""
    found = system.git_result(path, "rev-parse", "--path-format=absolute", "--git-common-dir")
    return (
        found.stderr.strip() or "git rev-parse failed with status {}".format(found.returncode)
        if found.returncode != 0
        else None
        if system.real(found.stdout.strip()) == system.real(common_dir)
        else "it belongs to another repository, {}".format(found.stdout.strip())
    )


def rescues(main: str, candidates: Tuple[Tuple[str, str], ...]) -> Tuple[Tuple[str, str], ...]:
    """The deleted tips still present that no ref reaches, `refs/archive/*` included."""
    return tuple(
        (branch, old)
        for branch, old in candidates
        if system.git_ok(main, "cat-file", "-e", old + "^{commit}")
        and system.git(
            main, "rev-list", "--max-count=1", old, "--not", "--all", check=False
        ).strip()
    )


def basic(env: Mapping[str, str], cfg: Mapping, checkout: str) -> rules.RepoFacts:
    """The repository's places and settings alone, for a command that needs no verdicts."""
    found = system.locate(checkout)
    if found is None:
        raise NotARepository("{} is not a git checkout".format(checkout))
    main = system.worktrees(found[2])[0].path
    config = rules.parse_config(
        system.git(main, "config", "--get-regexp", r"^worktrees\.", check=False)
    )
    return rules.RepoFacts(
        main=main,
        common_dir=found[0],
        state_dir=core.repo_state_dir(env, found[0]),
        default_branch=None,
        default_ref=None,
        default_problem=None,
        settings=rules.settings(cfg["settings"], config),
        worktrees=(),
        branches=(),
        archives=(),
        merged=frozenset(),
        events=(),
        rescues=(),
    )


def collect(
    env: Mapping[str, str], cfg: Mapping, checkout: str, override: Optional[str], now: float
) -> rules.RepoFacts:
    """Everything the rules need about the repository `checkout` belongs to."""
    found = system.locate(checkout)
    if found is None:
        raise NotARepository("{} is not a git checkout".format(checkout))
    common_dir = found[0]
    listed = tuple(w for w in system.worktrees(found[2]) if not w.bare)
    main = listed[0].path
    state_dir = core.repo_state_dir(env, common_dir)
    config = rules.parse_config(
        system.git(main, "config", "--get-regexp", r"^worktrees\.", check=False)
    )
    branch, problem = default_branch(main, override)
    default_ref = "refs/remotes/origin/" + branch if branch else None
    resolved = (
        system.git_result(main, "rev-parse", "--verify", "--quiet", default_ref + "^{tree}")
        if default_ref
        else None
    )
    tree = resolved.stdout.strip() if resolved is not None and resolved.returncode == 0 else None
    default_problem = problem or (
        None if tree else "no default branch: {} does not exist".format(default_ref)
    )
    known_ref = default_ref if default_problem is None else None
    rows = heads(main)
    tips = {ref[len("refs/heads/") :]: obj for ref, obj, *_ in rows}
    merged = frozenset(
        system.git(
            main,
            "for-each-ref",
            "--merged",
            known_ref,
            "--format=%(refname)",
            "refs/heads",
            "refs/archive",
        ).split()
        if known_ref
        else ()
    )
    archives = tuple(
        tuple(line.split(" ", 1))
        for line in system.git(
            main, "for-each-ref", "--format=%(refname) %(objectname)", "refs/archive"
        ).splitlines()
        if " " in line
    )
    archived = frozenset(obj for _, obj in archives)
    every_ref = tuple(
        (ref, peeled or obj)
        for ref, obj, peeled in (
            tuple(line.split("\0"))
            for line in system.git(
                main, "for-each-ref", "--format=%(refname)%00%(objectname)%00%(*objectname)"
            ).splitlines()
            if line.count("\0") == 2
        )
    )
    patterns = tuple(rules.parse_pattern(p) for p in keep_patterns(cfg, main, main))
    starts: Dict[int, Optional[float]] = {}
    default_head = "refs/heads/" + (branch or "")
    unlanded = tuple(r for r in rows if known_ref and r[0] not in merged and r[0] != default_head)
    default_tree = (
        rules.parse_ls_tree(system.git(main, "ls-tree", "-r", "-z", "--full-tree", known_ref))
        if unlanded and known_ref
        else {}
    )
    branches = tuple(
        rules.BranchFacts(
            name=ref[len("refs/heads/") :],
            tip=obj,
            upstream=upstream or None,
            track=track,
            landed=None if known_ref is None else ref in merged,
            content_landed=None
            if known_ref is None or ref in merged or ref == default_head
            else content_landed(main, known_ref, obj, default_tree),
            age=branch_age(common_dir, ref[len("refs/heads/") :], committed, now),
            archived=obj in archived,
            is_default=ref == default_head,
        )
        for ref, obj, upstream, track, committed in rows
    )
    worktrees = tuple(
        worktree_facts(
            wt,
            main,
            common_dir,
            known_ref,
            tips,
            merged,
            patterns
            + tuple(rules.parse_pattern(p) for p in keep_patterns({"keep": ()}, wt.path, main))
            if os.path.isdir(wt.path)
            else patterns,
            now,
            starts,
        )
        for wt in listed
    )
    events = system.read_events(state_dir)
    return rules.RepoFacts(
        main=main,
        common_dir=common_dir,
        state_dir=state_dir,
        default_branch=branch,
        default_ref=default_ref,
        default_problem=default_problem,
        settings=rules.settings(cfg["settings"], config),
        worktrees=worktrees,
        branches=branches,
        archives=archives,
        merged=merged,
        events=events,
        rescues=rescues(main, rules.rescue_candidates(events)),
        refs=every_ref,
    )
