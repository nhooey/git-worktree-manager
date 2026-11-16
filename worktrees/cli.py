"""worktrees: a git worktree's shared build caches, provisioning and cleanup.

Commands:

  env        print export lines for the worktree's shared build caches
  provision  copy local settings and allow direnv in a new worktree
  clean      delete a finished agent's build output from a worktree
  finish     remove a landed worktree and its branch
  sweep      prune, unlock, finish, clean and report a repository's worktrees
  status     one row per worktree, then the build root and the free disk
  sessions   the Claude Code sessions of a checkout and its worktrees

Every command that removes anything lists what it would do and does it only
with --apply. Exit status: 0 done (or listed), 1 failed, 2 bad usage,
3 kept or refused (finish and clean, with the reasons printed).

Example: eval "$(worktrees env)"
"""

from __future__ import annotations

import argparse
import os
import sys
import time
from typing import Dict, List, Mapping, Optional, Sequence, Tuple

from . import __version__, core, system
from . import repo as repos

KEPT = 3


def emit(data: object, human: str, as_json: bool) -> None:
    if as_json:
        import json

        sys.stdout.write(json.dumps(data, indent=2, sort_keys=True) + "\n")
    else:
        sys.stdout.write(human)


# env and provision ---------------------------------------------------------------


def provision(env: Mapping[str, str], lay: core.Layout) -> Tuple[str, ...]:
    """Provision one worktree; returns what it did. Idempotent."""
    system.ensure_build_dirs(lay)
    main = system.worktrees(lay.worktree)[0].path if not lay.is_main else lay.worktree
    settings = os.path.join(".claude", "settings.local.json")
    actions = core.provision_actions(
        is_main=lay.is_main,
        worktree_has_settings=os.path.exists(os.path.join(lay.worktree, settings)),
        main_has_settings=os.path.isfile(os.path.join(main, settings)),
        worktree_envrc=system.read_bytes(os.path.join(lay.worktree, ".envrc")),
        main_envrc=system.read_bytes(os.path.join(main, ".envrc")),
        direnv_available=system.which("direnv") is not None,
    )
    for action in actions:
        if action == "copy-settings":
            target = os.path.join(lay.worktree, settings)
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with open(os.path.join(main, settings), "rb") as source, open(target, "xb") as out:
                out.write(source.read())
        elif action == "direnv-allow":
            system.run(("direnv", "allow", lay.worktree))
    system.write_text(
        core.marker_path(lay),
        system.now_iso() + "\n",
    )
    return actions


def provision_quietly(env: Mapping[str, str], lay: core.Layout) -> None:
    try:
        provision(env, lay)
    except Exception as error:  # never stop the exports
        system.warn("provisioning {} failed: {}".format(lay.worktree, error))


def cmd_env(args: argparse.Namespace, env: Mapping[str, str]) -> int:
    lay = system.layout_for(env, os.getcwd())
    if lay is None:
        return 0
    if not os.path.exists(core.marker_path(lay)) or not os.path.exists(
        os.path.join(lay.wt, "WORKTREE")
    ):
        provision_quietly(env, lay)
    languages, unknown = (
        core.parse_languages(args.languages)
        if args.languages is not None
        else (core.detect_languages(os.listdir(lay.worktree)), ())
    )
    if unknown:
        system.warn(
            "unknown languages: {}; known: {}".format(", ".join(unknown), ", ".join(core.LANGUAGES))
        )
        return 2
    plan = core.env_plan(core.wanted_variables(languages, lay, sys.platform), languages, env)
    emit(
        {"exports": dict(plan.exports), "unsets": list(plan.unsets), "languages": list(languages)},
        core.render_env(plan, args.shell),
        args.json,
    )
    return 0


def cmd_provision(args: argparse.Namespace, env: Mapping[str, str]) -> int:
    lay = system.layout_for(env, args.worktree or os.getcwd())
    if lay is None:
        system.warn("{} is not in a git worktree".format(args.worktree or os.getcwd()))
        return 1
    actions = provision(env, lay)
    emit(
        {"worktree": lay.worktree, "actions": list(actions), "marker": core.marker_path(lay)},
        "provisioned {}{}\n".format(lay.worktree, "".join(": " + a for a in actions)),
        args.json,
    )
    return 0


# clean ----------------------------------------------------------------------------


def clean_listing(
    env: Mapping[str, str], cfg: Mapping, repo: repos.Repo, wt: core.Worktree
) -> Dict:
    """What `clean` deletes in one worktree: its build folder, whole entries and single files."""
    root, build = repos.wt_build_dir(env, repo, wt)
    patterns = repos.keep_patterns(cfg, wt.path, repo.main)
    ignored = repos.ignored_files(wt.path)
    kept = repos.kept_files(wt.path, patterns, ignored)
    entries = core.parse_would_remove(system.git(wt.path, "clean", "-ndX"))
    whole, mixed = core.clean_plan(entries, kept, tuple(p for p in ignored if p.endswith("/")))
    files = tuple(
        f for d in mixed for f in core.files_to_remove(repos.ignored_files(wt.path, d), kept)
    )
    return {
        "worktree": wt.path,
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


def apply_clean(env: Mapping[str, str], listing: Mapping) -> None:
    worktree = listing["worktree"]
    system.append_log(
        env,
        "clean.log",
        (
            "clean {}: {}".format(worktree, path)
            for path in ([listing["build_dir"]] if listing["build_dir"] else []) + listing["remove"]
        ),
    )
    if listing["build_dir"]:
        system.remove_tree(listing["build_dir"], listing["build_root"])
    for rel in listing["remove"]:
        system.remove_tree(os.path.join(worktree, rel.rstrip("/")), worktree)
    for rel in listing["mixed_dirs"]:
        prune_empty_dirs(os.path.join(worktree, rel))


def render_clean(listing: Mapping, applied: bool) -> str:
    verb = "deleted" if applied else "would delete"
    lines = ([listing["build_dir"]] if listing["build_dir"] else []) + listing["remove"]
    return "".join("{} {}\n".format(verb, p) for p in lines) or "nothing to clean in {}\n".format(
        listing["worktree"]
    )


def cmd_clean(args: argparse.Namespace, env: Mapping[str, str]) -> int:
    cfg = system.load_config(env)
    repo = repos.gather(args.worktree, do_fetch=False, timeout=0)
    wt = repos.find(repo, args.worktree)
    if wt is None:
        system.warn("{} is not a worktree of {}".format(args.worktree, repo.main))
        return 1
    refusal = core.clean_reasons(repos.facts(env, cfg, repo, wt, repos.now(), with_kept=False))
    if refusal:
        emit(
            {"worktree": wt.path, "cleaned": False, "reasons": list(refusal)},
            "kept {}: {}\n".format(wt.path, "; ".join(refusal)),
            args.json,
        )
        return KEPT
    listing = clean_listing(env, cfg, repo, wt)
    if args.apply:
        apply_clean(env, listing)
    emit({**listing, "applied": args.apply}, render_clean(listing, args.apply), args.json)
    return 0


# finish -----------------------------------------------------------------------------


def log_problem(log: Optional[str], worktree: str) -> Optional[str]:
    return (
        None
        if log is None
        else "{} does not exist".format(log)
        if not os.path.isfile(log)
        else "{} is inside the worktree".format(log)
        if core.is_inside(system.real(log), worktree)
        else None
    )


def finish_actions(
    env: Mapping[str, str], repo: repos.Repo, f: core.Facts, unlock: bool
) -> Tuple[Tuple[str, Tuple[str, ...]], ...]:
    """`(description, command)` pairs; an empty command is done in Python."""
    wt = f.worktree
    root, build = repos.wt_build_dir(env, repo, wt)
    upstream = f.holder
    forget = system.which("nix-devshell-cached-exports")
    forgets = (
        forget is not None and "--forget" in system.run((forget, "--help"), check=False).stdout
    )
    return (
        (
            (
                (
                    "unlock the stale Claude Code lock",
                    ("git", "-C", repo.main, "worktree", "unlock", wt.path),
                ),
            )
            if unlock
            else ()
        )
        + (
            (
                "remove worktree {}".format(wt.path),
                ("git", "-C", repo.main, "worktree", "remove", wt.path),
            ),
        )
        + (
            (
                (
                    (
                        "record {} as the branch's upstream".format(upstream),
                        (
                            "git",
                            "-C",
                            repo.main,
                            "branch",
                            "--set-upstream-to=" + upstream,
                            wt.branch,
                        ),
                    ),
                )
                if upstream
                else ()
            )
            + (
                (
                    "delete branch {}".format(wt.branch),
                    ("git", "-C", repo.main, "branch", "--delete", wt.branch),
                ),
            )
            if wt.branch
            else ()
        )
        + ((("delete build folder {}".format(build), ()),) if os.path.isdir(build) else ())
        + (
            (("forget the dev shell cache of {}".format(wt.path), (forget, "--forget", wt.path)),)
            if forgets
            else ()
        )
    )


def apply_finish(
    env: Mapping[str, str],
    repo: repos.Repo,
    f: core.Facts,
    actions: Sequence[Tuple[str, Tuple[str, ...]]],
) -> List[str]:
    """Run the actions in order; returns problems. Stops when the removal fails."""
    root, build = repos.wt_build_dir(env, repo, f.worktree)
    problems: List[str] = []
    for description, command in actions:
        if not command:
            system.remove_tree(build, root)
            continue
        result = system.run(command, check=False, extra_env=system.GIT_ENV)
        if result.returncode != 0:
            problems.append("{}: {}".format(description, result.stderr.strip()))
            if "remove worktree" in description or "unlock" in description:
                break
    system.append_log(
        env,
        "sweep.log",
        ["finish {}: {}".format(f.worktree.path, d) for d, _ in actions]
        + ["finish {}: {}".format(f.worktree.path, p) for p in problems],
    )
    return problems


def finish_one(
    env: Mapping[str, str], repo: repos.Repo, f: core.Facts, verdict: core.Verdict, apply: bool
) -> Dict:
    actions = finish_actions(env, repo, f, verdict.unlock)
    problems = apply_finish(env, repo, f, actions) if apply else []
    return {
        "worktree": f.worktree.path,
        "branch": f.worktree.branch,
        "actions": [d for d, _ in actions],
        "applied": apply,
        "problems": problems,
    }


def cmd_finish(args: argparse.Namespace, env: Mapping[str, str]) -> int:
    cfg = system.load_config(env)
    repo = repos.gather(
        args.worktree, do_fetch=not args.no_fetch, timeout=float(cfg["fetch_timeout_seconds"])
    )
    wt = repos.find(repo, args.worktree)
    if wt is None:
        system.warn("{} is not a worktree of {}".format(args.worktree, repo.main))
        return 1
    f = repos.facts(env, cfg, repo, wt, repos.now())
    verdict = core.finish_verdict(f, args.log is not None, log_problem(args.log, wt.path))
    if verdict.action == "keep":
        emit(
            {"worktree": wt.path, "finished": False, "reasons": list(verdict.reasons)},
            "kept {}: {}\n".format(wt.path, "; ".join(verdict.reasons)),
            args.json,
        )
        return KEPT
    done = finish_one(env, repo, f, verdict, args.apply)
    verb = "done:" if args.apply else "would:"
    emit(
        {**done, "finished": args.apply and not done["problems"]},
        "".join("{} {}\n".format(verb, a) for a in done["actions"])
        + "".join("problem: {}\n".format(p) for p in done["problems"]),
        args.json,
    )
    return 1 if done["problems"] else 0


# sweep ---------------------------------------------------------------------------------


def take_lock(lock: str, stale_minutes: float) -> bool:
    os.makedirs(os.path.dirname(lock), exist_ok=True)
    try:
        os.mkdir(lock)
        return True
    except FileExistsError:
        age = repos.now() - os.stat(lock).st_mtime
        if age > stale_minutes * 60:
            os.rmdir(lock)
            return take_lock(lock, float("inf"))
        return False


def sweep_repo(
    env: Mapping[str, str], cfg: Mapping, path: str, apply: bool, fetch: bool, cargo_cap: float
) -> Dict:
    found = system.locate(path)
    if found is None:
        return {"repository": path, "problem": "not a git checkout"}
    root = core.repo_build_root(env, found[0])
    lock = os.path.join(root, ".sweep.lock")
    if not take_lock(lock, float(cfg["sweep_lock_stale_minutes"])):
        return {"repository": path, "problem": "another sweep holds {}".format(lock)}
    try:
        return sweep_locked(env, cfg, found[2], root, apply, fetch, cargo_cap)
    finally:
        os.rmdir(lock)


def sweep_locked(
    env: Mapping[str, str],
    cfg: Mapping,
    path: str,
    root: str,
    apply: bool,
    fetch: bool,
    cargo_cap: float,
) -> Dict:
    log: List[str] = []
    pruned = system.git(
        path, "worktree", "prune", "--verbose", *(() if apply else ("--dry-run",))
    ).splitlines()
    repo = repos.gather(path, do_fetch=fetch, timeout=float(cfg["fetch_timeout_seconds"]))
    now = repos.now()
    judged = tuple(
        (f, core.sweep_verdict(f))
        for f in (
            repos.facts(env, cfg, repo, wt, now)
            for wt in repo.worktrees
            if not wt.prunable and not wt.bare
        )
    )
    unlocked = [f.worktree.path for f, v in judged if f.lock == "claude-stale"]
    if apply:
        for wt_path in unlocked:
            system.git(repo.main, "worktree", "unlock", wt_path, check=False)
    finished = [
        finish_one(env, repo, f, v._replace(unlock=False), apply)
        for f, v in judged
        if v.action == "finish"
    ]
    cleaned = []
    for f, v in judged:
        if v.action == "clean":
            listing = clean_listing(env, cfg, repo, f.worktree)
            if apply:
                apply_clean(env, listing)
            cleaned.append(
                {
                    "worktree": f.worktree.path,
                    "remove": len(listing["remove"]) + (1 if listing["build_dir"] else 0),
                }
            )
    unmerged = [
        {
            "worktree": f.worktree.path,
            "branch": f.worktree.branch,
            "size": system.disk_usage(f.worktree.path),
            "reasons": list(v.reasons),
        }
        for f, v in judged
        if v.action in ("clean", "keep") and f.landed is None
    ]
    after = system.worktrees(repo.main)
    live = frozenset(w.path for w in after)
    wt_top = os.path.join(root, "wt")
    wt_entries = tuple(
        (
            os.path.join(wt_top, name),
            (system.read_text(os.path.join(wt_top, name, "WORKTREE")) or "").strip() or None,
        )
        for name in (sorted(os.listdir(wt_top)) if os.path.isdir(wt_top) else ())
    )
    orphans = core.orphan_wt_dirs(
        tuple((d, system.real(p) if p and os.path.exists(p) else p) for d, p in wt_entries), live
    )
    if apply:
        for d in orphans:
            system.remove_tree(d, root)
    cargo = os.path.join(root, "shared", "cargo-build")
    cargo_size = system.disk_usage(cargo) if os.path.isdir(cargo) else 0
    cargo_open = (
        system.files_open_under(cargo)
        if cargo_size and core.over_cap(cargo_size, cargo_cap)
        else None
    )
    cargo_action = (
        "under the cap"
        if not cargo_size or not core.over_cap(cargo_size, cargo_cap)
        else "kept: a process has a file open in it"
        if cargo_open is not False
        else "deleted"
        if apply
        else "would delete"
    )
    if apply and cargo_action == "deleted":
        system.remove_tree(cargo, root)
    branches = landed_branches(repo, frozenset(w.branch for w in after if w.branch))
    deleted_branches = []
    for b, upstream in branches:
        if apply:
            system.git(repo.main, "branch", "--set-upstream-to=" + upstream, b, check=False)
            ok = system.git_ok(repo.main, "branch", "--delete", b)
            deleted_branches.append({"branch": b, "landed": upstream, "deleted": ok})
        else:
            deleted_branches.append({"branch": b, "landed": upstream, "deleted": False})
    results = sorted(
        os.path.join(repo.main, n)
        for n in os.listdir(repo.main)
        if (n == "result" or n.startswith("result-")) and os.path.islink(os.path.join(repo.main, n))
    )
    report = {
        "repository": repo.main,
        "applied": apply,
        "fetch_problem": repo.fetch_problem,
        "pruned": pruned,
        "unlocked": unlocked,
        "verdicts": [
            {**repos.as_dict(f), "action": v.action, "reasons": list(v.reasons)} for f, v in judged
        ],
        "finished": finished,
        "cleaned": cleaned,
        "unmerged": unmerged,
        "orphans": list(orphans),
        "cargo_build": {"size": cargo_size, "cap_gb": cargo_cap, "action": cargo_action},
        "branches": deleted_branches,
        "result_links": results,
    }
    log.extend(
        "{} {}: {}".format(v.action, f.worktree.path, "; ".join(v.reasons)) for f, v in judged
    )
    log.extend("orphan {}".format(o) for o in orphans)
    log.append("cargo-build {}: {}".format(core.human_bytes(cargo_size), cargo_action))
    log.extend(unmerged_log(env, root, unmerged))
    system.append_log(
        env,
        "sweep.log",
        ["sweep {} ({}): {}".format(repo.main, "apply" if apply else "list", line) for line in log],
    )
    return report


def unmerged_log(env: Mapping[str, str], root: str, unmerged: Sequence[Mapping]) -> List[str]:
    """The unmerged worktrees, once a day per repository."""
    stamp = os.path.join(root, ".unmerged-reported")
    today = system.now_iso()[:10]
    if not unmerged or (system.read_text(stamp) or "").strip() == today:
        return []
    system.write_text(stamp, today + "\n")
    return ["unmerged {} ({})".format(u["worktree"], core.human_bytes(u["size"])) for u in unmerged]


def landed_branches(repo: repos.Repo, checked_out: frozenset) -> List[Tuple[str, str]]:
    heads = system.git(
        repo.main, "for-each-ref", "--format=%(refname:short) %(objectname)", "refs/heads"
    ).splitlines()
    pairs = [tuple(line.split(" ", 1)) for line in heads if " " in line]
    return [
        (name, repos.short(upstream))
        for name, upstream in (
            (name, repos.landed_in(repo, tip, name))
            for name, tip in pairs
            if name not in checked_out and name not in repo.protected
        )
        if upstream
    ]


def discover(env: Mapping[str, str], cfg: Mapping) -> Tuple[Tuple[str, ...], Tuple[str, ...]]:
    """`(main checkouts, orphan repository build folders)` for `--all`."""
    depth = int(cfg["scan_depth"])

    def walk(top: str) -> List[str]:
        found: List[str] = []
        frontier = [(os.path.expanduser(top), 0)]
        while frontier:
            folder, level = frontier.pop()
            if os.path.isdir(os.path.join(folder, ".git")):
                found.append(system.real(folder))
                continue
            if level >= depth:
                continue
            try:
                children = sorted(
                    e.path
                    for e in os.scandir(folder)
                    if e.is_dir(follow_symlinks=False) and not e.name.startswith(".")
                )
            except OSError:
                children = []
            frontier.extend((c, level + 1) for c in children)
        return found

    scanned = [p for top in cfg["roots"] for p in walk(top)]
    top = core.builds_top(env)
    folders = sorted(os.listdir(top)) if os.path.isdir(top) else []
    recorded = [
        (os.path.join(top, n), (system.read_text(os.path.join(top, n, "REPO")) or "").strip())
        for n in folders
        if os.path.isdir(os.path.join(top, n))
    ]
    from_repo_files = [
        os.path.dirname(common) if os.path.basename(common) == ".git" else common
        for _, common in recorded
        if common and os.path.isdir(common)
    ]
    orphans = tuple(folder for folder, common in recorded if common and not os.path.isdir(common))
    return tuple(
        dict.fromkeys(system.real(p) for p in scanned + from_repo_files if os.path.isdir(p))
    ), orphans


def render_sweep(reports: Sequence[Mapping], orphan_repos: Sequence[str], apply: bool) -> str:
    def one(r: Mapping) -> str:
        if "problem" in r:
            return "{}: {}\n".format(r["repository"], r["problem"])
        rows = tuple(
            (v["action"], v["path"], v["branch"] or "(detached)", "; ".join(v["reasons"]))
            for v in r["verdicts"]
        )
        extra = (
            ["pruned: {}".format(p) for p in r["pruned"]]
            + ["unlocked stale lock: {}".format(p) for p in r["unlocked"]]
            + ["finished: {}".format(", ".join(f["actions"])) for f in r["finished"]]
            + ["orphan build folder: {}".format(o) for o in r["orphans"]]
            + [
                "cargo build folder {}: {}".format(
                    core.human_bytes(r["cargo_build"]["size"]), r["cargo_build"]["action"]
                )
            ]
            + [
                "landed branch {} ({}): {}".format(
                    b["branch"],
                    b["landed"],
                    "deleted" if b["deleted"] else "would delete" if not apply else "kept by git",
                )
                for b in r["branches"]
            ]
            + ["result link in the main checkout: {}".format(p) for p in r["result_links"]]
            + (["fetch problem: {}".format(r["fetch_problem"])] if r["fetch_problem"] else [])
        )
        return (
            "== {}{}\n".format(r["repository"], "" if apply else " (listing; --apply to act)")
            + core.table(rows)
            + "".join("  {}\n".format(e) for e in extra)
        )

    return "".join(one(r) for r in reports) + "".join(
        "orphan repository build folder: {}\n".format(o) for o in orphan_repos
    )


def cmd_sweep(args: argparse.Namespace, env: Mapping[str, str]) -> int:
    cfg = system.load_config(env)
    cap = args.cargo_cap if args.cargo_cap is not None else float(cfg["cargo_cap_gb"])
    if args.all:
        checkouts, orphan_repos = discover(env, cfg)
    else:
        here = system.locate(os.getcwd())
        if here is None:
            system.warn("not in a git checkout; use --all")
            return 1
        checkouts, orphan_repos = (here[2],), ()
    reports = [sweep_repo(env, cfg, c, args.apply, not args.no_fetch, cap) for c in checkouts]
    if args.apply:
        for o in orphan_repos:
            system.remove_tree(o, core.builds_top(env))
    emit(
        {"repositories": reports, "orphan_repositories": list(orphan_repos), "applied": args.apply},
        render_sweep(reports, orphan_repos, args.apply),
        args.json,
    )
    return 1 if any("problem" in r and "another sweep" not in r["problem"] for r in reports) else 0


# status -------------------------------------------------------------------------------------


def status_repo(env: Mapping[str, str], cfg: Mapping, path: str, sizes: bool) -> Dict:
    repo = repos.gather(path, do_fetch=False, timeout=0)
    now = repos.now()
    rows = []
    for wt in repo.worktrees:
        if wt.prunable or wt.bare:
            rows.append({"path": wt.path, "branch": wt.branch, "prunable": wt.prunable})
            continue
        f = repos.facts(env, cfg, repo, wt, now, with_kept=False)
        gd = repos.git_dir(wt.path)
        # The worktree's reflog changes on each commit and checkout in it.
        activity = (
            next(
                (
                    p
                    for p in (os.path.join(gd, "logs", "HEAD"), os.path.join(gd, "HEAD"))
                    if os.path.exists(p)
                ),
                None,
            )
            if gd
            else None
        )
        rows.append(
            {
                **repos.as_dict(f),
                "provisioned": bool(gd) and os.path.exists(os.path.join(gd, core.MARKER)),
                "age_seconds": max(0.0, now - os.stat(activity).st_mtime) if activity else None,
                "size": system.disk_usage(wt.path) if sizes else None,
            }
        )
    root = core.repo_build_root(env, repo.common_dir)
    return {
        "repository": repo.main,
        "worktrees": rows,
        "build_root": root,
        "build_root_size": system.disk_usage(root) if sizes else None,
    }


def cmd_status(args: argparse.Namespace, env: Mapping[str, str]) -> int:
    cfg = system.load_config(env)
    if args.all:
        checkouts = discover(env, cfg)[0]
    else:
        here = system.locate(os.getcwd())
        if here is None:
            system.warn("not in a git checkout; use --all")
            return 1
        checkouts = (here[2],)
    reports = [status_repo(env, cfg, c, not args.no_size) for c in checkouts]
    floors = system.floors(env)
    disks = [
        {"path": p, "free": free, "state": core.floor_state(free, floors)}
        for p, free in (
            (p, system.free_bytes(p))
            for p in dict.fromkeys([checkouts[0] if checkouts else os.getcwd(), "/nix"])
            if os.path.exists(p)
        )
    ]
    data = {
        "repositories": reports,
        "disks": disks,
        "floors": floors._asdict(),
        "builds_top": core.builds_top(env),
    }

    def repo_text(r: Mapping) -> str:
        header = (
            "worktree",
            "branch",
            "landed",
            "uncommitted",
            "provisioned",
            "lock",
            "age",
            "size",
        )
        rows = tuple(
            (
                w["path"],
                w.get("branch") or "(detached)",
                "prunable"
                if w.get("prunable")
                else (w.get("landed") or "no")
                if not w.get("main")
                else "main",
                str(w.get("uncommitted", "")),
                "yes" if w.get("provisioned") else "no",
                w.get("lock", ""),
                core.human_age(w.get("age_seconds")),
                core.human_bytes(w.get("size")) if w.get("size") is not None else "-",
            )
            for w in r["worktrees"]
        )
        return "== {}\n{}build root {}: {}\n".format(
            r["repository"],
            core.table((header,) + rows),
            r["build_root"],
            core.human_bytes(r["build_root_size"]) if r["build_root_size"] is not None else "-",
        )

    disk_text = "".join(
        "free on {}: {} ({}; floors {:g} GB build, {:g} GB cleanup only)\n".format(
            d["path"],
            core.human_bytes(d["free"]),
            d["state"],
            floors.build_gb,
            floors.cleanup_only_gb,
        )
        for d in disks
    )
    emit(data, "".join(repo_text(r) for r in reports) + disk_text, args.json)
    return 0


# sessions ------------------------------------------------------------------------------------


def cmd_sessions(args: argparse.Namespace, env: Mapping[str, str]) -> int:
    here = system.locate(args.checkout or os.getcwd())
    checkout = (
        system.worktrees(here[2])[0].path if here else system.real(args.checkout or os.getcwd())
    )
    prefix = core.claude_project_folder(checkout)
    projects = system.claude_projects(env)
    folders = sorted(
        n for n in (os.listdir(projects) if os.path.isdir(projects) else ()) if n.startswith(prefix)
    )
    sessions = sorted(
        (
            {
                "session": e.name[: -len(".jsonl")],
                "project": folder,
                "modified": e.stat().st_mtime,
                "size": e.stat().st_size,
            }
            for folder in folders
            for e in os.scandir(os.path.join(projects, folder))
            if e.name.endswith(".jsonl") and e.is_file()
        ),
        key=lambda s: -s["modified"],
    )
    rows = tuple(
        (
            time.strftime("%Y-%m-%d %H:%M", time.localtime(s["modified"])),
            s["session"],
            s["project"],
            core.human_bytes(s["size"]),
        )
        for s in sessions
    )
    emit(
        {"checkout": checkout, "sessions": sessions},
        core.table(rows) if rows else "no sessions for {}\n".format(checkout),
        args.json,
    )
    return 0


# The parser ------------------------------------------------------------------------------------


def parser() -> argparse.ArgumentParser:
    top = argparse.ArgumentParser(
        prog="worktrees",
        description=__doc__.split("\n\n")[0],
        epilog="\n\n".join(__doc__.split("\n\n")[1:]),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        allow_abbrev=False,
    )
    top.add_argument("--version", action="version", version="worktrees " + __version__)
    commands = top.add_subparsers(dest="command", metavar="<command>")
    commands.required = True

    def command(name: str, help_text: str, example: str, func) -> argparse.ArgumentParser:
        sub = commands.add_parser(
            name,
            help=help_text,
            description=help_text,
            epilog="Example: " + example,
            formatter_class=argparse.RawDescriptionHelpFormatter,
            allow_abbrev=False,
        )
        sub.set_defaults(func=func)
        sub.add_argument("--json", action="store_true", help="print JSON for a program to read")
        return sub

    env = command(
        "env",
        "Print export lines for the shared build caches of the checkout or worktree in the "
        "current directory, provisioning the worktree first when it hasn't been. Prints "
        "nothing outside git. A variable already set is left alone unless env set it for "
        "another worktree.",
        'eval "$(worktrees env --languages rust,python)"',
        cmd_env,
    )
    env.add_argument(
        "--languages",
        metavar="<list>",
        help="comma-separated languages; detected from files at the root when omitted ({})".format(
            ", ".join(core.LANGUAGES)
        ),
    )
    env.add_argument(
        "--shell",
        choices=("sh", "fish"),
        default="sh",
        help="the syntax of the lines (default: sh)",
    )

    prov = command(
        "provision",
        "Provision a worktree as env does on first entry: copy the main checkout's "
        ".claude/settings.local.json when the worktree has none, run direnv allow when its "
        ".envrc matches the checkout's, and write a marker in its git directory.",
        "worktrees provision ../repo.worktrees/feature",
        cmd_provision,
    )
    prov.add_argument(
        "worktree",
        nargs="?",
        metavar="<worktree>",
        help="the worktree (default: the current directory)",
    )

    clean = command(
        "clean",
        "Delete a finished agent's build output: the worktree's folder under the build root, "
        "and its gitignored files except the kept ones (the defaults, .worktreeinclude and "
        ".worktree-keep). Refuses the main checkout, a tree with tracked changes, and a "
        "worktree in use. Exits 3 when it refuses.",
        "worktrees clean .claude/worktrees/agent-1234 --apply",
        cmd_clean,
    )
    clean.add_argument("worktree", metavar="<worktree>")
    clean.add_argument("--apply", action="store_true", help="delete; without it, only list")

    finish = command(
        "finish",
        "Remove a worktree and its branch when the branch has landed and the tree is clean, "
        "or, with --log naming a saved log outside the worktree, when the branch has no "
        "commits of its own. Never uses --force. Exits 3, with the reasons, when it keeps them.",
        "worktrees finish ../repo.worktrees/job-7 --log ~/.local/state/job-7.log --apply",
        cmd_finish,
    )
    finish.add_argument("worktree", metavar="<worktree>")
    finish.add_argument(
        "--log", metavar="<path>", help="a failed run's saved log, outside the worktree"
    )
    finish.add_argument("--apply", action="store_true", help="remove; without it, only list")
    finish.add_argument(
        "--no-fetch",
        action="store_true",
        help="skip git fetch --prune before deciding what has landed",
    )

    sweep = command(
        "sweep",
        "Sweep the current repository's worktrees, or every clone's under the configured "
        "roots with --all: prune, clear stale Claude Code locks, finish landed worktrees, "
        "clean idle ones, report unmerged ones, delete orphan build folders, cap the shared "
        "Cargo build folder and delete landed local branches. Logs to "
        "~/.local/state/worktrees/sweep.log.",
        "worktrees sweep --all",
        cmd_sweep,
    )
    sweep.add_argument(
        "--all",
        action="store_true",
        help="every clone under the configured roots and the build root",
    )
    sweep.add_argument("--apply", action="store_true", help="act; without it, only list")
    sweep.add_argument("--no-fetch", action="store_true", help="skip git fetch --prune")
    sweep.add_argument(
        "--cargo-cap",
        type=float,
        metavar="<GB>",
        help="delete shared/cargo-build past this size (default: the configuration's, 60)",
    )

    status = command(
        "status",
        "One row per worktree: branch, landed or not, uncommitted paths, provisioned or not, "
        "lock, age and size on disk; then the build root's size and the free space against "
        "the disk floors.",
        "worktrees status --all",
        cmd_status,
    )
    status.add_argument("--all", action="store_true", help="every clone under the configured roots")
    status.add_argument("--no-size", action="store_true", help="skip measuring sizes on disk")

    sessions = command(
        "sessions",
        "List the Claude Code sessions of a checkout and all its worktrees, newest first.",
        "worktrees sessions ~/git/github/nhooey/nix-worktrees",
        cmd_sessions,
    )
    sessions.add_argument(
        "checkout", nargs="?", metavar="<checkout>", help="the checkout (default: the current one)"
    )
    return top


def fast_env_args(argv: Sequence[str]) -> Optional[argparse.Namespace]:
    """`env`'s options without building the parser, or None to let argparse handle them.

    `env` runs on every directory change, and building every command's
    parser costs more than the rest of `env` together.
    """
    words = tuple(argv)
    split = tuple(
        part
        for w in words[1:]
        for part in (w.split("=", 1) if w.startswith("--") and "=" in w else (w,))
    )
    flags = frozenset(w for w in split if w == "--json")
    pairs = tuple(w for w in split if w != "--json")
    names, values = pairs[0::2], pairs[1::2]
    options = dict(zip(names, values))
    valid = (
        bool(words)
        and words[0] == "env"
        and len(names) == len(values)
        and len(options) == len(names)
        and set(options) <= {"--languages", "--shell"}
        and options.get("--shell", "sh") in ("sh", "fish")
    )
    return (
        argparse.Namespace(
            func=cmd_env,
            languages=options.get("--languages"),
            shell=options.get("--shell", "sh"),
            json="--json" in flags,
        )
        if valid
        else None
    )


def main(argv: Optional[Sequence[str]] = None) -> int:
    words = tuple(sys.argv[1:] if argv is None else argv)
    args = fast_env_args(words) or parser().parse_args(words)
    env = dict(os.environ)
    if not env.get("HOME"):
        system.warn("HOME is not set")
        return 1
    try:
        return int(args.func(args, env))
    except system.CommandFailed as error:
        system.warn(str(error))
        return 1
    except BrokenPipeError:
        return 0
