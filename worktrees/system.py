"""The thin shell of `worktrees`: git, ps, lsof, df, du and the file system.

Each function here runs one command or touches one path and returns plain
values for `worktrees.core` to decide on. Nothing here decides.
"""

from __future__ import annotations

# json, shutil and datetime are imported where used: `worktrees env` runs on
# every directory change and loads neither.
import os
import subprocess
import sys
from typing import Dict, Iterable, Mapping, Optional, Tuple

from . import core

GIT_ENV = {"GIT_TERMINAL_PROMPT": "0", "LC_ALL": "C"}


class CommandFailed(Exception):
    def __init__(self, args: Tuple[str, ...], code: int, stderr: str):
        super().__init__("{} exited {}: {}".format(" ".join(args), code, stderr.strip()))
        self.code = code
        self.stderr = stderr


def run(
    args: Iterable[str],
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


def git(repo: str, *args: str, check: bool = True, timeout: Optional[float] = None) -> str:
    return run(
        ("git", "-C", repo, "-c", "core.quotePath=false") + args,
        check=check,
        timeout=timeout,
        extra_env=GIT_ENV,
    ).stdout


def git_ok(repo: str, *args: str) -> bool:
    return run(("git", "-C", repo) + args, check=False, extra_env=GIT_ENV).returncode == 0


def which(name: str) -> Optional[str]:
    import shutil

    return shutil.which(name)


def now_iso() -> str:
    import datetime

    return datetime.datetime.now().astimezone().isoformat(timespec="seconds")


def real(path: str) -> str:
    return os.path.realpath(path)


# Locations -------------------------------------------------------------------


def locate(path: str) -> Optional[Tuple[str, str, str]]:
    """`(common_dir, git_dir, toplevel)` as real paths, or None outside a worktree."""
    result = run(
        (
            "git",
            "-C",
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
    lines = result.stdout.splitlines()
    return (
        (real(lines[0]), real(lines[1]), real(lines[2]))
        if result.returncode == 0 and len(lines) == 3
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


def append_log(env: Mapping[str, str], name: str, lines: Iterable[str]) -> str:
    path = os.path.join(core.state_home(env), "worktrees", name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    stamp = now_iso()
    with open(path, "a", encoding="utf-8") as handle:
        handle.write("".join("{} {}\n".format(stamp, line) for line in lines))
    return path


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


def disk_usage(path: str) -> Optional[int]:
    """Bytes on disk under `path`, from `du -sk`."""
    if not os.path.exists(path):
        return 0
    result = run(("du", "-sk", path), check=False)
    first = result.stdout.split()
    return int(first[0]) * 1024 if first and first[0].isdigit() else None


def free_bytes(path: str) -> Optional[int]:
    result = run(("df", "-Pk", path), check=False)
    return core.parse_df(result.stdout) if result.returncode == 0 else None


def floors(env: Mapping[str, str]) -> core.Floors:
    text = read_text(os.path.join(core.config_home(env), "worktrees", "floor")) or ""
    return core.parse_floors(text, env)


# Processes ---------------------------------------------------------------------


def process_table() -> Tuple[core.Process, ...]:
    result = run(("ps", "-A", "-o", "pid=", "-o", "ppid=", "-o", "lstart="), check=False)
    return core.parse_ps(result.stdout)


def cwd_table() -> Optional[Tuple[Tuple[int, str], ...]]:
    """Every process's working directory, or None when `lsof` is missing."""
    if not which("lsof"):
        return None
    result = run(("lsof", "-n", "-P", "-w", "-d", "cwd", "-F", "pRn"), check=False)
    return tuple((pid, real(path)) for pid, path in core.parse_lsof_cwd(result.stdout, os.getpid()))


def files_open_under(folder: str) -> Optional[bool]:
    """Whether any process has a file open under `folder`; None without `lsof`."""
    if not which("lsof"):
        return None
    result = run(("lsof", "-n", "-P", "-w", "+D", folder, "-F", "p"), check=False)
    return any(line.startswith("p") for line in result.stdout.splitlines())


def newest_transcript(env: Mapping[str, str], worktree: str) -> Optional[float]:
    """The newest transcript mtime of the Claude Code project for `worktree`."""
    folder = os.path.join(claude_projects(env), core.claude_project_folder(worktree))
    try:
        with os.scandir(folder) as entries:
            times = tuple(e.stat().st_mtime for e in entries if e.name.endswith(".jsonl"))
    except OSError:
        return None
    return max(times) if times else None


def claude_projects(env: Mapping[str, str]) -> str:
    base = env.get("CLAUDE_CONFIG_DIR") or os.path.join(env.get("HOME", ""), ".claude")
    return os.path.join(base, "projects")
