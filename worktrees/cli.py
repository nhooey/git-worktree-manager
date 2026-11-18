"""worktrees: remove git worktrees and branches once their work lands, and lose nothing.

Commands:

  sweep      remove landed worktrees and branches, archive what would be lost, report the rest
  status     one row per worktree and local branch, with the sweep's verdict and why
  verdict    one worktree's or branch's verdict, with every test's result
  log        query the repository's event log
  archive    archive a branch's tip to the local backup, and compare it
  env        print export lines for the worktree's shared build caches
  provision  copy local settings and allow direnv in a new worktree
  clean      delete a worktree's ignored build output, keeping the keep patterns
  sessions   the Claude Code sessions of a checkout and its worktrees

Every command that changes anything acts unless given --dry-run, under
which it is read-only and prints what it would do. Exit status: 0 done,
1 failed, 2 bad usage, 3 refused (clean, with the reasons printed).

Example: worktrees sweep --dry-run --json --default-branch main ~/git/github/nhooey/*
"""

from __future__ import annotations

import argparse
import os
import sys
import time
from typing import Dict, Mapping, Optional, Sequence, Tuple

from . import __version__, core, gather, rules, system
from . import sweep as sweeping

REFUSED = 3


def emit(data: object, human: str, as_json: bool) -> None:
    if as_json:
        import json

        sys.stdout.write(json.dumps(data, indent=2, sort_keys=True) + "\n")
    else:
        sys.stdout.write(human)


# Which repositories --------------------------------------------------------------


def checkouts_under(root: str) -> Tuple[str, ...]:
    """Every main checkout one level under `root`: a folder holding a `.git` folder."""
    top = os.path.expanduser(root)
    names = sorted(os.listdir(top)) if os.path.isdir(top) else []
    return tuple(os.path.join(top, n) for n in names if os.path.isdir(os.path.join(top, n, ".git")))


def chosen_checkouts(paths: Sequence[str], roots: Sequence[str]) -> Tuple[str, ...]:
    """The checkouts named and those under each root, one per repository; else the current one."""
    named = tuple(os.path.expanduser(p) for p in paths) + tuple(
        c for r in roots for c in checkouts_under(r)
    )
    every = named or (os.getcwd(),)
    keyed = tuple(
        (found[0] if found else system.real(c), c)
        for c, found in ((c, system.locate(c)) for c in every)
    )
    return tuple(
        c for i, (key, c) in enumerate(keyed) if key not in frozenset(k for k, _ in keyed[:i])
    )


def default_overrides(values: Sequence[str]) -> Tuple[Optional[str], Dict[str, str]]:
    """`--default-branch <branch>` for every checkout, `<checkout>=<branch>` for one."""

    def per_checkout(value: str) -> Optional[Tuple[str, str]]:
        left, sep, right = value.partition("=")
        path = os.path.expanduser(left)
        return (system.real(path), right) if sep and right and os.path.isdir(path) else None

    pairs = tuple((v, per_checkout(v)) for v in values)
    plain = tuple(v for v, pair in pairs if pair is None)
    return (plain[-1] if plain else None, dict(pair for _, pair in pairs if pair is not None))


def override_for(checkout: str, everywhere: Optional[str], per: Mapping[str, str]) -> Optional[str]:
    found = system.locate(checkout) if per else None
    keys = (system.real(checkout),) + ((found[2], os.path.dirname(found[0])) if found else ())
    return next((per[k] for k in keys if k in per), everywhere)


# sweep, status, verdict ---------------------------------------------------------------


def cmd_sweep(args: argparse.Namespace, env: Mapping[str, str]) -> int:
    cfg = system.load_config(env)
    everywhere, per = default_overrides(args.default_branch)
    mode = "dry-run" if args.dry_run else "hook" if args.from_hook else "apply"
    reports = [
        sweeping.sweep_repository(env, cfg, c, override_for(c, everywhere, per), mode, args.fetch)
        for c in chosen_checkouts(args.checkouts, args.all)
    ]
    totals = sweeping.overall_totals(reports)
    note = sweeping.backup_note(env, totals, mode == "dry-run")
    data = {
        "mode": mode,
        "dry_run": mode == "dry-run",
        "time": rules.iso(system.now()),
        "repositories": reports,
        "totals": totals,
        "backup_note": note,
    }
    if not args.from_hook:
        emit(
            data,
            "".join(sweeping.summary_line(r) for r in reports)
            + sweeping.totals_line(data["totals"], mode == "dry-run")
            + (note + "\n" if note else ""),
            args.json,
        )
    return 1 if any(r.get("error") or r.get("backup", {}).get("failed") for r in reports) else 0


def status_one(
    env: Mapping[str, str], cfg: Mapping, checkout: str, override: Optional[str]
) -> Dict:
    try:
        return sweeping.once(env, cfg, checkout, override, "dry-run", False)
    except (system.CommandFailed, gather.NotARepository) as error:
        return {"repository": checkout, "error": str(error)}


def cmd_status(args: argparse.Namespace, env: Mapping[str, str]) -> int:
    cfg = system.load_config(env)
    everywhere, per = default_overrides(args.default_branch)
    reports = [
        status_one(env, cfg, c, override_for(c, everywhere, per))
        for c in chosen_checkouts(args.checkouts, args.all)
    ]
    data = {
        "mode": "dry-run",
        "dry_run": True,
        "time": rules.iso(system.now()),
        "repositories": reports,
        "totals": sweeping.overall_totals(reports),
    }
    emit(data, "".join(sweeping.status_rows(r) for r in reports), args.json)
    return 1 if any(r.get("error") for r in reports) else 0


def branch_tests(entry: Mapping, facts: Optional[rules.BranchFacts]) -> list:
    if facts is None:
        return []
    return [
        {
            "name": "landed",
            "ok": bool(facts.landed),
            "detail": "tip is an ancestor of the default branch",
        },
        {"name": "preview", "ok": facts.name.startswith("preview/"), "detail": "named preview/*"},
        {
            "name": "content-landed",
            "ok": bool(facts.content_landed),
            "detail": "merging it changes nothing",
        },
        {"name": "upstream", "ok": facts.upstream is not None, "detail": facts.upstream or "none"},
        {"name": "track", "ok": facts.track == "", "detail": facts.track or "in step"},
        {
            "name": "worktree",
            "ok": entry["worktree"] is None,
            "detail": entry["worktree"] or "none",
        },
    ]


def cmd_verdict(args: argparse.Namespace, env: Mapping[str, str]) -> int:
    cfg = system.load_config(env)
    everywhere, per = default_overrides(args.default_branch)
    is_path = os.path.isdir(args.target)
    checkout = args.target if is_path else (args.checkout or os.getcwd())
    try:
        repo = gather.collect(
            env, cfg, checkout, override_for(checkout, everywhere, per), system.now()
        )
    except (system.CommandFailed, gather.NotARepository) as error:
        system.warn(str(error))
        return 1
    report = sweeping.repo_report(rules.plan(repo, system.now()), True, {}, {}, tests=True)
    target = system.real(args.target) if is_path else args.target
    entry = next(
        (
            e
            for e in (report["worktrees"] if is_path else report["branches"])
            if (e.get("path") if is_path else e.get("branch")) == target
        ),
        None,
    )
    if entry is None:
        system.warn(
            "{} is no worktree or local branch of {}".format(args.target, report["repository"])
        )
        return 1
    full = (
        entry
        if is_path
        else {
            **entry,
            "tests": branch_tests(
                entry, next((b for b in repo.branches if b.name == target), None)
            ),
        }
    )
    human = "{} {}: {}\n{}".format(
        full["kind"],
        args.target,
        full["action"],
        "".join(
            "  {} {}: {}\n".format("ok  " if t["ok"] else "fail", t["name"], t["detail"])
            for t in full["tests"]
        ),
    )
    emit(full, human, args.json)
    return 0


# log and archive ------------------------------------------------------------------------


def since_seconds(text: Optional[str], now: float) -> Optional[float]:
    if text is None:
        return None
    ago = rules.parse_duration(text)
    return now - ago if ago is not None else rules.parse_time(text)


def matches(
    event: Mapping, branch: Optional[str], since: Optional[float], kinds: Sequence[str]
) -> bool:
    ref = event.get("ref") or ""
    at = rules.parse_time(event.get("time"))
    return (
        (
            branch is None
            or ref == "refs/heads/" + branch
            or ref.startswith("refs/archive/" + branch + "/")
            or event.get("branch") == branch
        )
        and (since is None or (at is not None and at >= since))
        and (not kinds or event.get("event") in kinds)
    )


def cmd_log(args: argparse.Namespace, env: Mapping[str, str]) -> int:
    found = system.locate(args.checkout or os.getcwd())
    if found is None:
        system.warn("{} is not a git checkout".format(args.checkout or os.getcwd()))
        return 1
    state_dir = core.repo_state_dir(env, found[0])
    since = since_seconds(args.since, system.now())
    if args.since is not None and since is None:
        system.warn("--since {}: not a time or a duration".format(args.since))
        return 2
    events = tuple(
        e for e in system.read_events(state_dir) if matches(e, args.branch, since, args.event)
    )
    rows = tuple(
        (
            str(e.get("time", "")),
            str(e.get("event", "")),
            str(e.get("ref") or e.get("worktree") or ""),
            "{}..{}".format((e.get("old") or "")[:10], (e.get("new") or "")[:10]),
            str(e.get("source", "")),
        )
        for e in events
    )
    emit(list(events), core.table(rows), args.json)
    return 0


def cmd_archive(args: argparse.Namespace, env: Mapping[str, str]) -> int:
    cfg = system.load_config(env)
    try:
        repo = gather.basic(env, cfg, args.checkout or os.getcwd())
    except gather.NotARepository as error:
        system.warn(str(error))
        return 1
    tip = system.git_result(
        repo.main, "rev-parse", "--verify", "--quiet", "refs/heads/" + args.branch + "^{commit}"
    )
    if tip.returncode != 0:
        system.warn("no local branch {}".format(args.branch))
        return 1
    existing = frozenset(ref for ref, _ in gather.backup_refs(repo.backup))
    ref = rules.unique_refs((rules.archive_ref(args.branch, system.now()),), existing)[0]
    outcome, compares = (
        ({"would": True}, [])
        if args.dry_run
        else sweeping.write_archive(
            repo, args.branch, ref, tip.stdout.strip(), "archive", "archive"
        )
    )
    emit(
        {
            "branch": args.branch,
            "ref": ref,
            "tip": tip.stdout.strip(),
            "backup": repo.backup,
            "compares": compares,
            **outcome,
        },
        "{} {} at {} in the local backup {}{}\n".format(
            "would write"
            if args.dry_run
            else "wrote and compared"
            if outcome.get("done")
            else "failed to write",
            ref,
            tip.stdout.strip()[:12],
            repo.backup,
            ": " + str(outcome["error"]) if outcome.get("error") else "",
        ),
        args.json,
    )
    return 0 if args.dry_run or outcome.get("done") else 1


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
    system.write_text(core.marker_path(lay), system.now_iso() + "\n")
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
    if args.dry_run:
        emit(
            {"worktree": lay.worktree, "would": True},
            "would provision {}\n".format(lay.worktree),
            args.json,
        )
        return 0
    actions = provision(env, lay)
    emit(
        {"worktree": lay.worktree, "actions": list(actions), "marker": core.marker_path(lay)},
        "provisioned {}{}\n".format(lay.worktree, "".join(": " + a for a in actions)),
        args.json,
    )
    return 0


# clean ----------------------------------------------------------------------------


def render_clean(listing: Mapping, applied: bool) -> str:
    verb = "deleted" if applied else "would delete"
    lines = ([listing["build_dir"]] if listing["build_dir"] else []) + listing["remove"]
    return "".join("{} {}\n".format(verb, p) for p in lines) or "nothing to clean in {}\n".format(
        listing["worktree"]
    )


def cmd_clean(args: argparse.Namespace, env: Mapping[str, str]) -> int:
    cfg = system.load_config(env)
    found = system.locate(args.worktree)
    if found is None:
        system.warn("{} is not in a git worktree".format(args.worktree))
        return 1
    common_dir, _, toplevel = found
    listed = system.worktrees(toplevel)
    wt = next((w for w in listed if w.path == toplevel), None)
    if wt is None:
        system.warn("{} is not a worktree of {}".format(args.worktree, listed[0].path))
        return 1
    changes = tuple(
        line for line in system.git(wt.path, "status", "--porcelain").splitlines() if line
    )
    refusal = core.clean_refusals(wt.is_main, wt.bare, changes)
    if refusal:
        emit(
            {"worktree": wt.path, "cleaned": False, "reasons": list(refusal)},
            "kept {}: {}\n".format(wt.path, "; ".join(refusal)),
            args.json,
        )
        return REFUSED
    listing = sweeping.clean_listing(env, cfg, listed[0].path, common_dir, wt.path, wt.is_main)
    if not args.dry_run:
        sweeping.apply_clean(listing)
        sweeping.log_at(
            core.repo_state_dir(env, common_dir),
            common_dir,
            "clean",
            "clean",
            worktree=wt.path,
            entries=len(listing["remove"]) + (1 if listing["build_dir"] else 0),
        )
    emit(
        {**listing, "applied": not args.dry_run, "would": args.dry_run},
        render_clean(listing, not args.dry_run),
        args.json,
    )
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

    def command(
        name: str, help_text: str, example: str, func, acts: bool = False
    ) -> argparse.ArgumentParser:
        sub = commands.add_parser(
            name,
            help=help_text.split(". ")[0],
            description=help_text,
            epilog="Example: " + example,
            formatter_class=argparse.RawDescriptionHelpFormatter,
            allow_abbrev=False,
        )
        sub.set_defaults(func=func)
        sub.add_argument("--json", action="store_true", help="print JSON for a program to read")
        if acts:
            sub.add_argument(
                "--dry-run",
                action="store_true",
                help="read-only: change nothing and print what would be done",
            )
        return sub

    def repositories(sub: argparse.ArgumentParser) -> None:
        sub.add_argument(
            "checkouts",
            nargs="*",
            metavar="<checkout>",
            help="checkouts of the repositories (default: the current one)",
        )
        sub.add_argument(
            "--all",
            action="append",
            default=[],
            metavar="<root>",
            help="every main checkout one level under <root>; repeatable",
        )
        sub.add_argument(
            "--default-branch",
            action="append",
            default=[],
            metavar="<branch>|<checkout>=<branch>",
            help="the default branch, for every checkout or for one; repeatable "
            "(default: the target of refs/remotes/origin/HEAD)",
        )

    sweep = command(
        "sweep",
        "Sweep each repository: back the log up with a snapshot the first time, prune "
        "worktrees whose folder is gone, remove landed, clean, idle, unlocked worktrees, "
        "clean the ignored files of unlanded ones idle a day, delete landed branches, "
        "delete preview and content-landed branches once merged or pushed, archiving them "
        "to ~/backups/git-archive and comparing the archive first when not, prune archives the default "
        "branch now holds, archive branches deleted by hand, and report the rest. Each "
        "action is a line in ~/.local/state/worktrees/<repo>-<hash8>/events.jsonl.",
        "worktrees sweep --dry-run --json --default-branch ~/src/app=master ~/src/app ~/src/lib",
        cmd_sweep,
        acts=True,
    )
    repositories(sweep)
    sweep.add_argument("--fetch", action="store_true", help="git fetch --prune origin first")
    sweep.add_argument("--from-hook", action="store_true", help=argparse.SUPPRESS)

    status = command(
        "status",
        "One row per worktree and per local branch, with the verdict the sweep would reach "
        "and every reason it keeps one. Read-only.",
        "worktrees status --all ~/git/github/nhooey",
        cmd_status,
    )
    repositories(status)

    verdict = command(
        "verdict",
        "The sweep's verdict for one worktree (a path) or local branch (a name), with every "
        "test's result. Read-only.",
        "worktrees verdict ../repo.worktrees/feature",
        cmd_verdict,
    )
    verdict.add_argument("target", metavar="<worktree-or-branch>")
    verdict.add_argument(
        "--checkout",
        metavar="<checkout>",
        help="the repository of a branch (default: the current one)",
    )
    verdict.add_argument(
        "--default-branch",
        action="append",
        default=[],
        metavar="<branch>|<checkout>=<branch>",
        help="the default branch (default: the target of refs/remotes/origin/HEAD)",
    )

    log = command(
        "log",
        "Query the repository's event log, so a figure comes from a query and not from "
        "reading lines.",
        "worktrees log --event archive --since 7d",
        cmd_log,
    )
    log.add_argument("--branch", metavar="<name>", help="events of this branch and its archives")
    log.add_argument(
        "--since", metavar="<time>", help="an ISO 8601 time, or a duration ago such as 7d"
    )
    log.add_argument(
        "--event", action="append", default=[], metavar="<kind>", help="only this kind; repeatable"
    )
    log.add_argument(
        "--checkout", metavar="<checkout>", help="the repository (default: the current one)"
    )

    archive = command(
        "archive",
        "Push the branch's tip to the local backup, ~/backups/git-archive/<owner>/<repo>.git, "
        "as refs/archive/<branch>/<time>, so its commits outlive the branch, then compare it "
        "there with git. Nothing goes to origin.",
        "worktrees archive detail-pane-counters",
        cmd_archive,
        acts=True,
    )
    archive.add_argument("branch", metavar="<branch>")
    archive.add_argument(
        "--checkout", metavar="<checkout>", help="the repository (default: the current one)"
    )

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
        acts=True,
    )
    prov.add_argument(
        "worktree",
        nargs="?",
        metavar="<worktree>",
        help="the worktree (default: the current directory)",
    )

    clean = command(
        "clean",
        "Delete a worktree's build output: its folder under the build root, and its "
        "gitignored files except the kept ones (the defaults, .worktreeinclude and "
        ".worktree-keep). Refuses the main checkout and a tree with tracked changes, and "
        "exits 3 when it refuses.",
        "worktrees clean .claude/worktrees/agent-1234",
        cmd_clean,
        acts=True,
    )
    clean.add_argument("worktree", metavar="<worktree>")

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
