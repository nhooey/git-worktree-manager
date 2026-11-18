"""The thin shell of `worktrees`: git and the file system.

Each function here runs one command or touches one path and returns plain
values for `worktrees.core` to decide on. Every git command runs with
`--no-optional-locks`, so reading a worktree's status never rewrites its
index.
"""

from __future__ import annotations

# json, shutil and datetime are imported where used: `worktrees env` runs on
# every directory change and loads neither.
import os
import subprocess
import sys
import time
from typing import Dict, Mapping, Optional, Sequence, Tuple

from . import core

GIT_ENV = {"GIT_TERMINAL_PROMPT": "0", "LC_ALL": "C"}


class CommandFailed(Exception):
    def __init__(self, args: Tuple[str, ...], code: int, stderr: str):
        super().__init__("{} exited {}: {}".format(" ".join(args), code, stderr.strip()))
        self.code = code
        self.stderr = stderr


def run(
    args: Sequence[str],
    cwd: Optional[str] = None,
    check: bool = True,
    timeout: Optional[float] = None,
    extra_env: Optional[Mapping[str, str]] = None,
) -> subprocess.CompletedProcess:
    argv = tuple(args)
    result = subprocess.run(
        argv,
        cwd=cwd,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        universal_newlines=True,
        timeout=timeout,
        env={**os.environ, **(extra_env or {})},
    )
    if check and result.returncode != 0:
        raise CommandFailed(argv, result.returncode, result.stderr)
    return result


def git_argv(repo: str, *args: str) -> Tuple[str, ...]:
    return ("git", "--no-optional-locks", "-C", repo, "-c", "core.quotePath=false") + args


def git(
    repo: str,
    *args: str,
    check: bool = True,
    timeout: Optional[float] = None,
    extra_env: Optional[Mapping[str, str]] = None,
) -> str:
    return run(
        git_argv(repo, *args),
        check=check,
        timeout=timeout,
        extra_env={**GIT_ENV, **(extra_env or {})},
    ).stdout


def git_ok(repo: str, *args: str) -> bool:
    return run(git_argv(repo, *args), check=False, extra_env=GIT_ENV).returncode == 0


def git_result(repo: str, *args: str) -> subprocess.CompletedProcess:
    return run(git_argv(repo, *args), check=False, extra_env=GIT_ENV)


def which(name: str) -> Optional[str]:
    import shutil

    return shutil.which(name)


def now() -> float:
    return time.time()


def now_iso() -> str:
    import datetime

    return datetime.datetime.now().astimezone().isoformat(timespec="seconds")


def real(path: str) -> str:
    return os.path.realpath(path)


def mtime(path: str) -> Optional[float]:
    try:
        return os.stat(path).st_mtime
    except OSError:
        return None


# Locations -------------------------------------------------------------------


def locate(path: str) -> Optional[Tuple[str, str, str]]:
    """`(common_dir, git_dir, toplevel)` as real paths, or None outside a worktree."""
    result = (
        run(
            git_argv(
                path,
                "rev-parse",
                "--path-format=absolute",
                "--git-common-dir",
                "--git-dir",
                "--show-toplevel",
            ),
            check=False,
            extra_env=GIT_ENV,
        )
        if os.path.isdir(path)
        else None
    )
    lines = result.stdout.splitlines() if result else []
    return (
        (real(lines[0]), real(lines[1]), real(lines[2]))
        if result is not None and result.returncode == 0 and len(lines) == 3
        else None
    )


def layout_for(env: Mapping[str, str], path: str) -> Optional[core.Layout]:
    found = locate(path)
    return core.layout(env, *found) if found else None


def worktrees(repo: str) -> Tuple[core.Worktree, ...]:
    listed = core.parse_worktree_list(git(repo, "worktree", "list", "--porcelain"))
    return tuple(
        w._replace(path=real(w.path) if os.path.exists(w.path) else w.path) for w in listed
    )


def linked_git_dir(worktree: str) -> Optional[str]:
    """A linked worktree's git directory, from the `gitdir:` line of its `.git` file."""
    text = read_text(os.path.join(worktree, ".git"))
    line = (text or "").strip()
    target = line[len("gitdir: ") :] if line.startswith("gitdir: ") else None
    return real(os.path.join(worktree, target)) if target else None


def read_text(path: str) -> Optional[str]:
    try:
        with open(path, encoding="utf-8", errors="replace") as handle:
            return handle.read()
    except OSError:
        return None


def read_bytes(path: str) -> Optional[bytes]:
    try:
        with open(path, "rb") as handle:
            return handle.read()
    except OSError:
        return None


def write_text(path: str, text: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)


def write_atomically(path: str, text: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    partial = "{}.{}.tmp".format(path, os.getpid())
    with open(partial, "w", encoding="utf-8") as handle:
        handle.write(text)
    os.replace(partial, path)


def warn(message: str) -> None:
    sys.stderr.write("worktrees: {}\n".format(message))


# Configuration -----------------------------------------------------------------


def defaults_path() -> str:
    return os.path.join(os.path.dirname(os.path.abspath(__file__)), "defaults.json")


def load_config(env: Mapping[str, str]) -> Dict:
    """The shipped defaults with `~/.config/worktrees/config.json` merged over them."""
    import json

    with open(defaults_path(), encoding="utf-8") as handle:
        defaults = json.load(handle)
    user_path = os.path.join(core.config_home(env), "worktrees", "config.json")
    text = read_text(user_path)
    try:
        user = json.loads(text) if text else {}
    except ValueError as error:
        warn("{}: {}; using the defaults".format(user_path, error))
        user = {}
    for key in core.unknown_keys(defaults, user):
        warn("{}: unknown setting {}".format(user_path, key))
    return core.merge_config(defaults, user)


# Build root -------------------------------------------------------------------


def ensure_build_dirs(lay: core.Layout) -> None:
    """Create the repository's and worktree's folders with their REPO and WORKTREE files.

    The first call that creates the top folder excludes it from Time Machine
    and Spotlight on macOS.
    """
    top = os.path.dirname(lay.root)
    fresh_top = not os.path.isdir(top)
    os.makedirs(lay.wt, exist_ok=True)
    if fresh_top and sys.platform == "darwin":
        write_text(os.path.join(top, ".metadata_never_index"), "")
        if which("tmutil"):
            result = run(("tmutil", "addexclusion", top), check=False)
            if result.returncode != 0:
                warn("tmutil addexclusion {}: {}".format(top, result.stderr.strip()))
    if read_text(os.path.join(lay.root, "REPO")) != lay.common_dir + "\n":
        write_text(os.path.join(lay.root, "REPO"), lay.common_dir + "\n")
    if read_text(os.path.join(lay.wt, "WORKTREE")) != lay.worktree + "\n":
        write_text(os.path.join(lay.wt, "WORKTREE"), lay.worktree + "\n")


def remove_tree(path: str, inside: str) -> bool:
    """Delete `path`, a folder the program made under `inside`; False when absent."""
    resolved = real(path)
    if not core.is_inside(resolved, real(inside)) or resolved == real(inside):
        raise ValueError("refusing to delete {} outside {}".format(path, inside))
    if os.path.islink(path) or os.path.isfile(path):
        os.remove(path)
        return True
    if os.path.isdir(path):
        import shutil

        shutil.rmtree(path)
        return True
    return False


# Processes ---------------------------------------------------------------------


def claude_projects(env: Mapping[str, str]) -> str:
    base = env.get("CLAUDE_CONFIG_DIR") or os.path.join(env.get("HOME", ""), ".claude")
    return os.path.join(base, "projects")
