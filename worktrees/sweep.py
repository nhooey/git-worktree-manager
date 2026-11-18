"""The shell of `clean`: it lists a worktree's build output and deletes it."""

from __future__ import annotations

import os
from typing import Dict, Mapping, Tuple

from . import core, gather, system

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
