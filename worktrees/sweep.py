"""The sweep's shell: it acts on `rules.plan` in the sweep's order, logs each action, and reports.

`sweep_repository` takes the repository's lock, fetches when asked,
collects the facts, plans, acts and writes `last-sweep.json`. Under a dry
run it takes no lock, writes nothing and marks each action `would`.
"""

from __future__ import annotations

import os
from typing import Dict, Iterable, Mapping, Optional, Sequence, Tuple

from . import core, gather, rules, system

Key = Tuple[str, str]
Outcome = Dict[str, object]

DONE: Outcome = {"done": True}
MAX_RUNS = 5


def failed(error: str) -> Outcome:
    return {"done": False, "error": error}


# The log ---------------------------------------------------------------------------


def record(common_dir: str, event: str, source: str, fields: Mapping) -> Dict:
    return {
        "time": rules.iso(system.now()),
        "repo": common_dir,
        "event": event,
        "source": source,
        **{k: v for k, v in fields.items() if v is not None and k != "event"},
    }


def log_at(state_dir: str, common_dir: str, event: str, source: str, **fields: object) -> None:
    system.append_event(state_dir, record(common_dir, event, source, fields))


def log(repo: rules.RepoFacts, event: str, source: str = "sweep", **fields: object) -> None:
    log_at(repo.state_dir, repo.common_dir, event, source, **fields)


# `clean` ------------------------------------------------------------------------------


def ignored_files(worktree: str, *paths: str) -> Tuple[str, ...]:
    """Ignored files, file by file; a nested repository shows as `path/`."""
    return core.split_nul(
        system.git(
            worktree, "ls-files", "-z", "--others", "--ignored", "--exclude-standard", "--", *paths
        )
    )


def kept_files(worktree: str, patterns: Tuple[str, ...], ignored: Tuple[str, ...]) -> frozenset:
    """Ignored files a keep pattern matches, at any depth."""
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


def clean_listing(
    env: Mapping[str, str], cfg: Mapping, main: str, common_dir: str, worktree: str, is_main: bool
) -> Dict:
    """What `clean` deletes in one worktree: its build folder, whole entries and single files."""
    root = core.repo_build_root(env, common_dir)
    build = os.path.join(root, "wt", core.wt_id(worktree, is_main))
    patterns = gather.keep_patterns(cfg, worktree, main)
    ignored = ignored_files(worktree)
    kept = kept_files(worktree, patterns, ignored)
    entries = core.parse_would_remove(system.git(worktree, "clean", "-ndX"))
    whole, mixed = core.clean_plan(entries, kept, tuple(p for p in ignored if p.endswith("/")))
    files = tuple(f for d in mixed for f in core.files_to_remove(ignored_files(worktree, d), kept))
    return {
        "worktree": worktree,
        "build_root": root,
        "build_dir": build if os.path.isdir(build) else None,
        "remove": list(whole + files),
        "mixed_dirs": list(mixed),
        "kept": sorted(kept),
    }


def prune_empty_dirs(top: str) -> None:
    for folder, _, _ in sorted(os.walk(top), key=lambda item: -len(item[0])):
        try:
            os.rmdir(folder)
        except OSError:
            pass


def apply_clean(listing: Mapping) -> None:
    worktree = listing["worktree"]
    if listing["build_dir"]:
        system.remove_tree(listing["build_dir"], listing["build_root"])
    for rel in listing["remove"]:
        system.remove_tree(os.path.join(worktree, rel.rstrip("/")), worktree)
    for rel in listing["mixed_dirs"]:
        prune_empty_dirs(os.path.join(worktree, rel))


# Acting ----------------------------------------------------------------------------


PUSHED_FLAGS = frozenset(" *+-=")


def parse_push(text: str) -> Dict[str, str]:
    """`git push --porcelain` lines, `<flag>\\t<from>:<to>\\t<summary>`, as `{to: flag}`."""
    rows = tuple(line.split("\t") for line in text.splitlines() if line.count("\t") >= 2)
    return {row[1].split(":", 1)[-1]: row[0] for row in rows if row[0]}


def push_refs(repo: rules.RepoFacts, refs: Sequence[str]) -> Dict[str, Optional[str]]:
    """Push `refs` to origin in one push, when `worktrees.archivePush` is set; each ref's problem."""
    if not refs or not repo.settings.archive_push:
        return {ref: None for ref in refs}
    pushed = system.git_result(
        repo.main, "push", "--porcelain", "origin", *("{0}:{0}".format(ref) for ref in refs)
    )
    flags = parse_push(pushed.stdout)
    problem = pushed.stderr.strip() or "not pushed"
    return {ref: None if flags.get(ref, "!") in PUSHED_FLAGS else problem for ref in refs}


def write_archives(
    repo: rules.RepoFacts, items: Sequence[Tuple[str, str, str]], event: str, source: str
) -> Dict[str, Outcome]:
    """`refs/archive/<branch>/<time>` at each tip, created only if absent, then pushed together.

    An archive that isn't on origin when `worktrees.archivePush` is set
    fails, so the branch it guards is kept.
    """
    made = {
        ref: system.git_result(repo.main, "update-ref", "-m", "worktrees " + event, ref, tip, "")
        for _, ref, tip in items
    }
    problems = push_refs(repo, tuple(ref for ref, result in made.items() if result.returncode == 0))
    for branch, ref, tip in items:
        if made[ref].returncode == 0:
            log(
                repo,
                event,
                source,
                ref=ref,
                new=tip,
                branch=branch,
                pushed=repo.settings.archive_push and problems[ref] is None,
            )
    return {
        ref: failed(made[ref].stderr.strip())
        if made[ref].returncode != 0
        else failed("archived, but not pushed: " + str(problems[ref]))
        if problems[ref]
        else DONE
        for _, ref, _ in items
    }


def write_archive(
    repo: rules.RepoFacts, branch: str, ref: str, tip: str, event: str, source: str
) -> Outcome:
    return write_archives(repo, ((branch, ref, tip),), event, source)[ref]


def delete_branch(repo: rules.RepoFacts, v: rules.BranchVerdict) -> Outcome:
    """`git update-ref -d` with the tip the rules tested, so a branch that moved stays."""
    ref = "refs/heads/" + v.facts.name
    deleted = system.git_result(
        repo.main, "update-ref", "-m", "worktrees sweep", "-d", ref, v.facts.tip
    )
    if deleted.returncode != 0:
        return failed(deleted.stderr.strip())
    log(repo, "branch-delete", ref=ref, old=v.facts.tip, verdict=v.state)
    return DONE


def archived_first(p: rules.Plan, v: rules.BranchVerdict, done: Mapping[Key, Outcome]) -> Outcome:
    """The branch's archive: the backfill's, or one written now."""
    name = v.facts.name
    return (
        done.get(("archive", name), failed("not archived"))
        if name in p.backfill_archives
        else write_archive(p.repo, name, p.archives[name], v.facts.tip, "archive", "sweep")
        if name in p.archives
        else DONE
    )


def end_branch(p: rules.Plan, v: rules.BranchVerdict, done: Mapping[Key, Outcome]) -> Outcome:
    archived = archived_first(p, v, done) if v.action == "archive+delete" else DONE
    return delete_branch(p.repo, v) if archived.get("done") else archived


def remove_worktree(
    p: rules.Plan,
    v: rules.WorktreeVerdict,
    branch: Optional[rules.BranchVerdict],
    size: Optional[int],
    done: Mapping[Key, Outcome],
) -> Tuple[Tuple[Key, Outcome], ...]:
    f = v.facts
    archived = (
        archived_first(p, branch, done)
        if branch is not None and branch.action == "archive+delete"
        else DONE
    )
    if not archived.get("done"):
        return ((("worktree", f.path), archived),)
    removed = system.git_result(p.repo.main, "worktree", "remove", f.path)
    if removed.returncode != 0:
        return ((("worktree", f.path), failed(removed.stderr.strip())),)
    log(
        p.repo,
        "remove",
        worktree=f.path,
        ref="refs/heads/" + f.branch if f.branch else None,
        old=f.head,
        bytes=size,
    )
    then = (
        ((("branch", branch.facts.name), end_branch(p, branch._replace(action="delete"), done)),)
        if branch is not None and branch.action in ("delete", "archive+delete")
        else ()
    )
    return ((("worktree", f.path), DONE),) + then


def clean_worktree(
    env: Mapping[str, str], cfg: Mapping, repo: rules.RepoFacts, v: rules.WorktreeVerdict
) -> Outcome:
    listing = clean_listing(env, cfg, repo.main, repo.common_dir, v.facts.path, False)
    apply_clean(listing)
    log(
        repo,
        "clean",
        worktree=v.facts.path,
        entries=len(listing["remove"]) + (1 if listing["build_dir"] else 0),
    )
    return {**DONE, "deleted": listing["remove"]}


def prune_worktrees(
    repo: rules.RepoFacts, gone: Sequence[rules.WorktreeVerdict]
) -> Tuple[Tuple[Key, Outcome], ...]:
    if not gone:
        return ()
    pruned = system.git_result(repo.main, "worktree", "prune")
    if pruned.returncode != 0:
        return tuple((("worktree", v.facts.path), failed(pruned.stderr.strip())) for v in gone)
    for v in gone:
        log(
            repo,
            "prune",
            worktree=v.facts.path,
            ref="refs/heads/" + v.facts.branch if v.facts.branch else None,
            old=v.facts.head,
        )
    return tuple((("worktree", v.facts.path), DONE) for v in gone)


def prune_archive(repo: rules.RepoFacts, ref: str, obj: str) -> Outcome:
    deleted = system.git_result(
        repo.main, "update-ref", "-m", "worktrees archive-prune", "-d", ref, obj
    )
    if deleted.returncode != 0:
        return failed(deleted.stderr.strip())
    log(repo, "archive-prune", ref=ref, old=obj)
    return DONE


def execute(
    env: Mapping[str, str], cfg: Mapping, p: rules.Plan, sizes: Mapping[str, Optional[int]]
) -> Dict[Key, Outcome]:
    """Every action of the plan, in the sweep's order; returns each one's outcome."""
    repo = p.repo
    tips = {v.facts.name: v.facts.tip for v in p.branches}
    by_branch = {v.facts.name: v for v in p.branches}
    for snapshot in p.snapshots:
        log(
            repo,
            "snapshot",
            "backfill" if p.backfill else "sweep",
            **{k: v for k, v in snapshot.items() if k != "event"},
        )
    written = write_archives(
        repo,
        tuple((name, p.archives[name], tips[name]) for name in p.backfill_archives),
        "archive",
        "backfill",
    )
    backfilled = {("archive", name): written[p.archives[name]] for name in p.backfill_archives}
    pruned = dict(prune_worktrees(repo, tuple(v for v in p.worktrees if v.action == "prune")))
    removed = dict(
        pair
        for v in p.worktrees
        if v.action == "remove"
        for pair in remove_worktree(
            p, v, by_branch.get(v.facts.branch or ""), sizes.get(v.facts.path), backfilled
        )
    )
    cleaned = {
        ("worktree", v.facts.path): clean_worktree(env, cfg, repo, v)
        for v in p.worktrees
        if v.action == "clean"
    }
    branches = {
        ("branch", v.facts.name): end_branch(p, v, backfilled)
        for v in p.branches
        if v.worktree is None and v.action in ("delete", "archive+delete")
    }
    archives_pruned = {
        ("archive-prune", ref): prune_archive(repo, ref, obj) for ref, obj in p.archive_prunes
    }
    saved = write_archives(
        repo, tuple((b, ref, old) for b, old, ref in p.rescues), "rescue", "sweep"
    )
    rescued = {("rescue", old): saved[ref] for _, old, ref in p.rescues}
    for v in p.worktrees:
        if v.lock_report and v.facts.lock is not None:
            log(
                repo,
                "lock-report",
                worktree=v.facts.path,
                lock=v.facts.lock.lock.reason,
                age=v.facts.lock.age,
                alive=v.facts.lock.alive,
            )
    return {**backfilled, **pruned, **removed, **cleaned, **branches, **archives_pruned, **rescued}


# The report ----------------------------------------------------------------------------


def iso_or_none(seconds: Optional[float]) -> Optional[str]:
    return rules.iso(seconds) if seconds is not None else None


def lock_entry(v: rules.WorktreeVerdict) -> Optional[Dict]:
    lf = v.facts.lock
    return (
        None
        if lf is None
        else {
            "reason": lf.lock.reason,
            "who": lf.lock.who,
            "pid": lf.lock.pid,
            "since": iso_or_none(lf.lock.since),
            "age_seconds": lf.age,
            "alive": lf.alive,
            "process_start": iso_or_none(lf.process_start),
            "report": v.lock_report,
        }
    )


def outcome_fields(dry: bool, action: str, outcome: Optional[Outcome]) -> Dict:
    return {"would": dry and action != "keep"} if outcome is None else {"would": False, **outcome}


def reasons(rs: Iterable[rules.Reason]) -> list:
    return [{"code": r.code, "detail": r.detail} for r in rs]


def archive_note(ref: str, dry: bool, push: bool, outcome: Optional[Outcome]) -> str:
    """`archive <ref> (would write, would push)`, or what became of it."""
    steps = (
        ("would write", "would push" if push else "not pushed: worktrees.archivePush is false")
        if dry or outcome is None
        else (str(outcome.get("error")),)
        if not outcome.get("done")
        else ("written", "pushed" if push else "not pushed: worktrees.archivePush is false")
    )
    return "archive {} ({})".format(ref, ", ".join(steps))


def branch_archive(
    p: rules.Plan, name: Optional[str], dry: bool, done: Mapping[Key, Outcome]
) -> Tuple[str, ...]:
    """The archive the sweep writes for a branch, as a `survives` entry."""
    ref = p.archives.get(name or "")
    outcome = done.get(("archive", name or "")) or done.get(("branch", name or ""))
    return (archive_note(ref, dry, p.repo.settings.archive_push, outcome),) if ref else ()


NOWHERE = "nowhere else: kept"


def worktree_survives(
    p: rules.Plan, v: rules.WorktreeVerdict, dry: bool, done: Mapping[Key, Outcome]
) -> list:
    f = v.facts
    commits = tuple(v.survives) + branch_archive(p, f.branch, dry, done)
    return (
        list(commits or ("HEAD {}: {}".format(f.head[:12], NOWHERE),))
        + (["{} uncommitted paths: {}".format(len(f.changes), NOWHERE)] if f.changes else [])
        + (["kept files {}: {}".format(", ".join(f.kept), NOWHERE)] if f.kept else [])
    )


def worktree_entry(
    v: rules.WorktreeVerdict,
    dry: bool,
    outcome: Optional[Outcome],
    size: Optional[int],
    tests: bool,
    survives: Sequence[str] = (),
) -> Dict:
    f = v.facts
    return {
        "survives": list(survives),
        "kind": "worktree",
        "path": f.path,
        "branch": f.branch,
        "head": f.head,
        "main": f.is_main,
        "action": v.action,
        **outcome_fields(dry, v.action, outcome),
        "reasons": reasons(v.reasons),
        "lock": lock_entry(v),
        "landed": f.landed,
        "idle_seconds": f.idle,
        "uncommitted": list(f.changes),
        "keep_files": list(f.kept),
        "clean": list(f.cleanable) if v.action == "clean" else [],
        "bytes": size if v.action == "remove" else None,
        **({"tests": [t._asdict() for t in v.tests]} if tests else {}),
    }


def branch_entry(
    p: rules.Plan,
    v: rules.BranchVerdict,
    dry: bool,
    outcome: Optional[Outcome],
    done: Optional[Mapping[Key, Outcome]] = None,
) -> Dict:
    b = v.facts
    found = tuple(v.survives) + branch_archive(p, b.name, dry, done or {})
    return {
        "survives": list(found or (NOWHERE,)),
        "kind": "branch",
        "branch": b.name,
        "tip": b.tip,
        "upstream": b.upstream,
        "track": b.track,
        "state": v.state,
        "action": v.action,
        **outcome_fields(dry, v.action, outcome),
        "reasons": reasons(v.reasons),
        "worktree": v.worktree,
        "archive_ref": p.archives.get(b.name) if v.action == "archive+delete" else None,
        "age_seconds": b.age,
    }


def totals(report: Mapping) -> Dict:
    wts, brs = report["worktrees"], report["branches"]
    return {
        "worktrees": rules.count(w["action"] for w in wts),
        "branches": rules.count(b["action"] for b in brs),
        "kept_by_reason": rules.count(
            r["code"]
            for e in tuple(wts) + tuple(brs)
            if e["action"] in ("keep", "clean")
            for r in e["reasons"]
        ),
        "archives_written": len(report["backfill"]["archives"])
        + sum(
            1 for b in brs if b["archive_ref"] and b["branch"] not in report["backfill"]["branches"]
        )
        + len(report["rescues"]),
        "archives_pruned": len(report["archives_pruned"]),
        "rescues": len(report["rescues"]),
        "lock_reports": len(report["lock_reports"]),
        "snapshots": report["backfill"]["snapshots"],
        "bytes_freed": sum(w["bytes"] or 0 for w in wts if w["action"] == "remove"),
    }


def repo_report(
    p: rules.Plan,
    dry: bool,
    done: Mapping[Key, Outcome],
    sizes: Mapping[str, Optional[int]],
    tests: bool = False,
) -> Dict:
    repo = p.repo
    tips = {v.facts.name: v.facts.tip for v in p.branches}
    body = {
        "repository": repo.main,
        "common_dir": repo.common_dir,
        "state_dir": repo.state_dir,
        "default_branch": repo.default_branch,
        "default_ref": repo.default_ref,
        "problem": repo.default_problem,
        "settings": repo.settings._asdict(),
        "worktrees": [
            worktree_entry(
                v,
                dry,
                done.get(("worktree", v.facts.path)),
                sizes.get(v.facts.path),
                tests,
                worktree_survives(p, v, dry, done),
            )
            for v in p.worktrees
        ],
        "branches": [
            branch_entry(p, v, dry, done.get(("branch", v.facts.name)), done) for v in p.branches
        ],
        "backfill": {
            "needed": p.backfill,
            "snapshots": len(p.snapshots),
            "branches": list(p.backfill_archives),
            "archives": [
                {
                    "branch": name,
                    "ref": p.archives[name],
                    "tip": tips[name],
                    "push": repo.settings.archive_push,
                    **outcome_fields(dry, "archive", done.get(("archive", name))),
                }
                for name in p.backfill_archives
            ],
        },
        "archives_pruned": [
            {
                "ref": ref,
                "object": obj,
                **outcome_fields(dry, "archive-prune", done.get(("archive-prune", ref))),
            }
            for ref, obj in p.archive_prunes
        ],
        "rescues": [
            {
                "branch": branch,
                "old": old,
                "ref": ref,
                "push": repo.settings.archive_push,
                **outcome_fields(dry, "rescue", done.get(("rescue", old))),
            }
            for branch, old, ref in p.rescues
        ],
        "lock_reports": [
            {"path": v.facts.path, **(lock_entry(v) or {})} for v in p.worktrees if v.lock_report
        ],
    }
    return {**body, "totals": totals(body)}


def overall_totals(reports: Sequence[Mapping]) -> Dict:
    good = tuple(r["totals"] for r in reports if "totals" in r)
    return {
        "repositories": len(reports),
        "errors": sum(1 for r in reports if r.get("error")),
        "skipped": sum(1 for r in reports if r.get("skipped")),
        "without_default_branch": sum(1 for r in reports if r.get("problem")),
        **{
            key: rules.sum_counts(t[key] for t in good)
            for key in ("worktrees", "branches", "kept_by_reason")
        },
        **{
            key: sum(t[key] for t in good)
            for key in (
                "archives_written",
                "archives_pruned",
                "rescues",
                "lock_reports",
                "snapshots",
                "bytes_freed",
            )
        },
    }


# Running -----------------------------------------------------------------------------


def fetch(checkout: str, timeout: float) -> Optional[str]:
    """`git fetch --prune origin`; a problem as a string."""
    try:
        result = system.run(
            system.git_argv(checkout, "fetch", "--prune", "--quiet", "origin"),
            check=False,
            timeout=timeout,
            extra_env={
                **system.GIT_ENV,
                "GIT_SSH_COMMAND": os.environ.get("GIT_SSH_COMMAND", "ssh -o BatchMode=yes"),
            },
        )
    except Exception as error:  # a timeout
        return "fetch: {}".format(error)
    return "fetch: {}".format(result.stderr.strip()) if result.returncode else None


def once(
    env: Mapping[str, str],
    cfg: Mapping,
    checkout: str,
    override: Optional[str],
    mode: str,
    do_fetch: bool,
    tests: bool = False,
) -> Dict:
    """One pass over one repository: `dry-run` and `report` only report; `apply` acts.

    A sweep started by a hook (`hook`) reads `worktrees.sweep` to choose
    between `report` and `apply`.
    """
    fetched = fetch(checkout, float(cfg["fetch_timeout_seconds"])) if do_fetch else None
    repo = gather.collect(env, cfg, checkout, override, system.now())
    acting = mode == "apply" or (mode == "hook" and repo.settings.sweep == "apply")
    p = rules.plan(repo, system.now())
    sizes = {
        v.facts.path: system.disk_usage(v.facts.path) for v in p.worktrees if v.action == "remove"
    }
    done = execute(env, cfg, p, sizes) if acting else {}
    return {
        **repo_report(p, not acting, done, sizes, tests),
        "mode": "apply" if acting else "dry-run" if mode == "dry-run" else "report",
        "fetch_problem": fetched,
    }


def sweep_repository(
    env: Mapping[str, str],
    cfg: Mapping,
    checkout: str,
    override: Optional[str],
    mode: str,
    do_fetch: bool,
) -> Dict:
    """The whole sweep of one repository, under its lock unless it is a dry run."""
    found = system.locate(checkout)
    if found is None:
        return {"repository": checkout, "error": "{} is not a git checkout".format(checkout)}
    try:
        if mode == "dry-run":
            return once(env, cfg, checkout, override, mode, do_fetch)
        state_dir = core.repo_state_dir(env, found[0])
        pending = os.path.join(state_dir, "sweep.pending")
        with system.sweep_lock(state_dir) as taken:
            if not taken:
                system.write_text(pending, rules.iso(system.now()) + "\n")
                return {
                    "repository": checkout,
                    "skipped": "another sweep holds {}; it runs again when it ends".format(
                        os.path.join(state_dir, "sweep.lock")
                    ),
                }
            report = rerun(env, cfg, checkout, override, mode, do_fetch, pending, 1)
            import json

            system.write_atomically(
                os.path.join(state_dir, "last-sweep.json"),
                json.dumps(report, indent=2, sort_keys=True) + "\n",
            )
            return report
    except (system.CommandFailed, gather.NotARepository) as error:
        return {"repository": checkout, "error": str(error)}


def rerun(
    env: Mapping[str, str],
    cfg: Mapping,
    checkout: str,
    override: Optional[str],
    mode: str,
    do_fetch: bool,
    pending: str,
    runs: int,
) -> Dict:
    """A pass, then another while a concurrent sweep has left `sweep.pending`."""
    report = once(env, cfg, checkout, override, mode, do_fetch)
    if os.path.exists(pending) and runs < MAX_RUNS:
        os.remove(pending)
        return rerun(env, cfg, checkout, override, mode, do_fetch, pending, runs + 1)
    return {**report, "runs": runs}


# Text ----------------------------------------------------------------------------------


def counted(counts: Mapping[str, int]) -> str:
    return ", ".join("{} {}".format(k, n) for k, n in counts.items())


def summary_line(r: Mapping) -> str:
    if r.get("error"):
        return "{}: error: {}\n".format(r["repository"], r["error"])
    if r.get("skipped"):
        return "{}: skipped: {}\n".format(r["repository"], r["skipped"])
    t = r["totals"]
    verb = "would " if r.get("mode") != "apply" else ""
    acts = {k: v for k, v in t["worktrees"].items() if k != "keep"}
    branch_acts = {k: v for k, v in t["branches"].items() if k != "keep"}
    return "{}{}: {}worktrees {}{}; {}branches {}; {}archives {} written{}, {} pruned, {} rescued; kept {}{}\n".format(
        r["repository"],
        " ({})".format(r["default_ref"]) if r.get("default_ref") else "",
        verb,
        counted(acts) or "none",
        " ({} freed)".format(core.human_bytes(t["bytes_freed"])) if t["bytes_freed"] else "",
        verb,
        counted(branch_acts) or "none",
        verb,
        t["archives_written"],
        " and pushed" if r.get("settings", {}).get("archive_push") else "",
        t["archives_pruned"],
        t["rescues"],
        counted(t["kept_by_reason"]) or "nothing",
        "; {}".format(r["problem"]) if r.get("problem") else "",
    )


def totals_line(t: Mapping, dry: bool) -> str:
    return "total: {} repositories; {}worktrees {}; branches {}; archives {} written, {} pruned; {} freed; kept {}\n".format(
        t["repositories"],
        "would: " if dry else "",
        counted(t["worktrees"]) or "none",
        counted(t["branches"]) or "none",
        t["archives_written"],
        t["archives_pruned"],
        core.human_bytes(t["bytes_freed"]),
        counted(t["kept_by_reason"]) or "nothing",
    )


def status_rows(r: Mapping) -> str:
    if r.get("error"):
        return "== {}\nerror: {}\n".format(r["repository"], r["error"])
    rows = tuple(
        (
            "worktree",
            w["path"],
            w["branch"] or "(detached)",
            w["action"],
            "; ".join(x["detail"] for x in w["reasons"]) or "-",
            "survives: " + "; ".join(w["survives"]),
        )
        for w in r["worktrees"]
    ) + tuple(
        (
            "branch",
            b["branch"],
            b["state"],
            b["action"],
            "; ".join(x["detail"] for x in b["reasons"]) or "-",
            "survives: " + "; ".join(b["survives"]),
        )
        for b in r["branches"]
    )
    return "== {}{}\n{}".format(
        r["repository"],
        "  ({})".format(r["problem"]) if r.get("problem") else "",
        core.table(rows),
    )
