"""worktrees: remove git worktrees and branches once their work lands, and lose nothing.

Commands:

  env        print export lines for the worktree's shared build caches
  provision  copy local settings and allow direnv in a new worktree
  clean      delete a worktree's ignored build output, keeping the keep patterns
  sessions   the Claude Code sessions of a checkout and its worktrees

Every command that changes anything acts unless given --dry-run, under
which it is read-only and prints what it would do. Exit status: 0 done,
1 failed, 2 bad usage, 3 refused (clean, with the reasons printed).

Example: worktrees clean --dry-run --json ../repo.worktrees/feature
"""

from __future__ import annotations

import argparse
import os
import sys
import time
from typing import Mapping, Optional, Sequence, Tuple

from . import __version__, core, system
from . import sweep as sweeping

REFUSED = 3


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
