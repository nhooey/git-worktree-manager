"""What the shell learns about a repository and its worktrees.

`gather` runs git, ps and lsof once per repository; `facts` runs the
per-worktree git commands. Both return `worktrees.core` records, which the
commands hand to the core's decisions.
"""

from __future__ import annotations

import filecmp
import os
import time
from typing import Dict, FrozenSet, Mapping, NamedTuple, Optional, Tuple

from . import core, system


class Repo(NamedTuple):
    main: str
    common_dir: str
    worktrees: Tuple[core.Worktree, ...]
    upstreams: Tuple[str, ...]
    protected: FrozenSet[str]
    merged: Mapping[str, FrozenSet[str]]  # upstream ref -> branch tips merged into it
    stashes: Tuple[str, ...]
    processes: Mapping[int, core.Process]
    cwds: Optional[Tuple[Tuple[int, str], ...]]
    exclude: FrozenSet[int]
    fetch_problem: Optional[str]


def default_branch(main: str, refs: FrozenSet[str]) -> Optional[str]:
    head = system.run(
        ("git", "-C", main, "symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"),
        check=False,
    ).stdout.strip()
    named = head.split("/", 1)[1] if "/" in head else None
    return named or next((n for n in ("main", "master") if "refs/heads/" + n in refs), None)


def fetch(main: str, timeout: float) -> Optional[str]:
    """`git fetch --prune --all` when there are remotes; a problem as a string."""
    if not system.git(main, "remote").strip():
        return None
    try:
        result = system.run(
            ("git", "-C", main, "fetch", "--prune", "--all", "--quiet"),
            check=False,
            timeout=timeout,
            extra_env={
                **system.GIT_ENV,
                "GIT_SSH_COMMAND": os.environ.get("GIT_SSH_COMMAND", "ssh -o BatchMode=yes"),
            },
        )
    except Exception as error:  # timeout
        return "fetch: {}".format(error)
    return "fetch: {}".format(result.stderr.strip()) if result.returncode else None


def gather(path: str, do_fetch: bool, timeout: float) -> Repo:
    found = system.locate(path)
    if found is None:
        raise system.CommandFailed(
            ("git", "rev-parse"), 128, "{} is not in a git worktree".format(path)
        )
    common_dir = found[0]
    listed = system.worktrees(found[2])
    main = listed[0].path
    fetch_problem = fetch(main, timeout) if do_fetch else None
    refs = frozenset(system.git(main, "for-each-ref", "--format=%(refname)").split())
    default = default_branch(main, refs)
    names = tuple(n for n in (default, "integration") if n)
    upstreams = tuple(
        ref
        for name in names
        for ref in sorted(refs)
        if ref == "refs/heads/" + name
        or (ref.startswith("refs/remotes/") and ref.endswith("/" + name) and ref.count("/") == 3)
    )
    merged = {
        u: frozenset(
            system.git(
                main, "for-each-ref", "--merged", u, "--format=%(objectname)", "refs/heads"
            ).split()
        )
        for u in upstreams
    }
    processes = system.process_table()
    by_pid = {p.pid: p for p in processes}
    return Repo(
        main=main,
        common_dir=common_dir,
        worktrees=listed,
        upstreams=upstreams,
        protected=frozenset(names),
        merged=merged,
        stashes=core.stash_branches(system.git(main, "stash", "list", "--format=%gs", check=False)),
        processes=by_pid,
        cwds=system.cwd_table(),
        exclude=core.ancestors(os.getpid(), {p.pid: p.ppid for p in processes}),
        fetch_problem=fetch_problem,
    )


def short(ref: str) -> str:
    return (
        ref[len("refs/heads/") :]
        if ref.startswith("refs/heads/")
        else ref[len("refs/remotes/") :]
        if ref.startswith("refs/remotes/")
        else ref
    )


def landed_in(repo: Repo, tip: str, branch: Optional[str]) -> Optional[str]:
    """The upstream ref `tip` has landed in: an ancestor, else all commits upstream by `git cherry`."""
    own = "refs/heads/" + branch if branch else None
    candidates = tuple(u for u in repo.upstreams if u != own)
    by_ancestry = next(
        (
            u
            for u in candidates
            if (
                tip in repo.merged[u]
                if branch
                else system.git_ok(repo.main, "merge-base", "--is-ancestor", tip, u)
            )
        ),
        None,
    )
    return by_ancestry or next(
        (
            u
            for u, out in (
                (u, system.git(repo.main, "cherry", u, tip, check=False)) for u in candidates
            )
            if out.strip() and core.cherry_all_upstream(out)
        ),
        None,
    )


def holder(repo: Repo, tip: str, branch: Optional[str]) -> Optional[str]:
    """Another branch holding `tip`, or None when the branch has commits of its own."""
    holders = system.git(
        repo.main,
        "for-each-ref",
        "--contains",
        tip,
        "--format=%(refname)",
        "refs/heads",
        "refs/remotes",
    ).split()
    return next(
        (
            short(h)
            for h in holders
            if h != "refs/heads/{}".format(branch) and not h.endswith("/HEAD")
        ),
        None,
    )


def keep_patterns(cfg: Mapping, worktree: str, main: str) -> Tuple[str, ...]:
    def from_file(name: str) -> Tuple[str, ...]:
        here = system.read_text(os.path.join(worktree, name))
        return core.parse_pattern_file(
            (here if here is not None else system.read_text(os.path.join(main, name))) or ""
        )

    return tuple(cfg["keep"]) + from_file(".worktreeinclude") + from_file(".worktree-keep")


def ignored_files(worktree: str, *paths: str) -> Tuple[str, ...]:
    """Ignored files, file by file; a nested repository shows as `path/`."""
    return core.split_nul(
        system.git(
            worktree, "ls-files", "-z", "--others", "--ignored", "--exclude-standard", "--", *paths
        )
    )


def kept_files(
    worktree: str, patterns: Tuple[str, ...], ignored: Tuple[str, ...]
) -> FrozenSet[str]:
    """Ignored files a keep pattern matches."""
    matching = core.split_nul(
        system.git(
            worktree,
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            *("--exclude=" + p for p in patterns),
        )
    )
    return frozenset(matching) & frozenset(p for p in ignored if not p.endswith("/"))


def unique_kept(worktree: str, main: str, kept: FrozenSet[str]) -> Tuple[str, ...]:
    """Kept files the main checkout lacks or holds with other bytes."""

    def same(rel: str) -> bool:
        theirs = os.path.join(main, rel)
        return os.path.isfile(theirs) and filecmp.cmp(
            os.path.join(worktree, rel), theirs, shallow=False
        )

    return tuple(sorted(rel for rel in kept if not same(rel)))


def facts(
    env: Mapping[str, str],
    cfg: Mapping,
    repo: Repo,
    wt: core.Worktree,
    now: float,
    with_kept: bool = True,
) -> core.Facts:
    exists = os.path.isdir(wt.path) and not wt.bare
    changes = (
        tuple(
            line
            for line in system.git(
                wt.path, "status", "--porcelain", "--untracked-files=normal"
            ).splitlines()
            if line
        )
        if exists
        else ()
    )
    patterns = keep_patterns(cfg, wt.path, repo.main)
    ignored = ignored_files(wt.path) if exists and with_kept and not wt.is_main else ()
    kept = kept_files(wt.path, patterns, ignored) if ignored else frozenset()
    landed = landed_in(repo, wt.head, wt.branch) if wt.head and not wt.is_main else None
    newest = system.newest_transcript(env, wt.path)
    cwds = repo.cwds if repo.cwds is not None else ()
    held_by = (
        holder(repo, wt.head, wt.branch) if wt.head and landed is None and not wt.is_main else None
    )
    return core.Facts(
        worktree=wt,
        changes=changes,
        landed=short(landed) if landed else None,
        own_commits=landed is None and held_by is None,
        holder=short(landed) if landed else held_by,
        stashes=sum(1 for b in repo.stashes if b == wt.branch) if wt.branch else 0,
        unique_kept=unique_kept(wt.path, repo.main, kept),
        lock=core.classify_lock(wt.locked, repo.processes),
        inside=core.pids_inside(cwds, wt.path, repo.exclude)
        if repo.cwds is not None
        else core.UNKNOWN_INSIDE,
        fresh_transcript=newest is not None
        and newest >= now - float(cfg["transcript_idle_minutes"]) * 60,
        protected=bool(wt.branch) and wt.branch in repo.protected,
        upstreams=tuple(short(u) for u in repo.upstreams),
    )


def find(repo: Repo, path: str) -> Optional[core.Worktree]:
    target = system.real(path)
    return next((w for w in repo.worktrees if w.path == target), None)


def wt_build_dir(env: Mapping[str, str], repo: Repo, wt: core.Worktree) -> Tuple[str, str]:
    """`(repository build root, the worktree's wt/<id>/)`."""
    root = core.repo_build_root(env, repo.common_dir)
    return root, os.path.join(root, "wt", core.wt_id(wt.path, wt.is_main))


def now() -> float:
    return time.time()


def git_dir(worktree: str) -> Optional[str]:
    result = system.run(("git", "-C", worktree, "rev-parse", "--absolute-git-dir"), check=False)
    return system.real(result.stdout.strip()) if result.returncode == 0 else None


def as_dict(f: core.Facts) -> Dict:
    return {
        "path": f.worktree.path,
        "branch": f.worktree.branch,
        "head": f.worktree.head,
        "main": f.worktree.is_main,
        "landed": f.landed,
        "uncommitted": len(f.changes),
        "stashes": f.stashes,
        "kept_files": list(f.unique_kept),
        "lock": f.lock,
        "lock_reason": f.worktree.locked,
        "processes_inside": list(f.inside),
        "recent_transcript": f.fresh_transcript,
    }
