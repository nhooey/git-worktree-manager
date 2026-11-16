"""The functional core of `worktrees`: every decision, as pure functions.

Nothing here reads files, the clock, the environment or runs a command.
The shell (`worktrees.system` and `worktrees.cli`) gathers those as plain
values, passes them in, and acts on what comes back. Failures are values:
a reason string or a tuple of reasons, never an exception.

Compatible with Python 3.9, macOS's `/usr/bin/python3`.
"""

from __future__ import annotations

import fnmatch
import hashlib
import itertools
import posixpath
import re
import time
from typing import Dict, FrozenSet, Iterable, Mapping, NamedTuple, Optional, Tuple

# The build root -------------------------------------------------------------


def sha8(text: str) -> str:
    """The first 8 hex digits of the SHA-256 of `text`."""
    return hashlib.sha256(text.encode()).hexdigest()[:8]


def repo_name(common_dir: str) -> str:
    """The repository's name from its common git directory.

    `/x/nix-worktrees/.git` gives `nix-worktrees`; a bare `/x/foo.git`
    gives `foo`.
    """
    trimmed = common_dir.rstrip("/")
    base = posixpath.basename(trimmed)
    return (
        posixpath.basename(posixpath.dirname(trimmed))
        if base == ".git"
        else (base[: -len(".git")] if base.endswith(".git") else base)
    )


def cache_home(env: Mapping[str, str]) -> str:
    return env.get("XDG_CACHE_HOME") or posixpath.join(env.get("HOME", ""), ".cache")


def state_home(env: Mapping[str, str]) -> str:
    return env.get("XDG_STATE_HOME") or posixpath.join(env.get("HOME", ""), ".local", "state")


def config_home(env: Mapping[str, str]) -> str:
    return env.get("XDG_CONFIG_HOME") or posixpath.join(env.get("HOME", ""), ".config")


def builds_top(env: Mapping[str, str]) -> str:
    """`${XDG_CACHE_HOME:-$HOME/.cache}/worktree-builds`."""
    return posixpath.join(cache_home(env), "worktree-builds")


def repo_build_root(env: Mapping[str, str], common_dir: str) -> str:
    """`<builds top>/<repo>-<hash8>`, from the real path of the common dir."""
    return posixpath.join(builds_top(env), "{}-{}".format(repo_name(common_dir), sha8(common_dir)))


def wt_id(worktree: str, is_main: bool) -> str:
    """`main` for the main checkout, else the folder's name and its path's hash."""
    return (
        "main"
        if is_main
        else "{}-{}".format(posixpath.basename(worktree.rstrip("/")), sha8(worktree))
    )


class Layout(NamedTuple):
    """Where one worktree's build output goes."""

    root: str
    wt: str
    worktree: str
    common_dir: str
    git_dir: str
    is_main: bool


def layout(env: Mapping[str, str], common_dir: str, git_dir: str, worktree: str) -> Layout:
    """The layout of a worktree, from real paths of its git directories."""
    is_main = git_dir.rstrip("/") == common_dir.rstrip("/")
    root = repo_build_root(env, common_dir)
    return Layout(
        root=root,
        wt=posixpath.join(root, "wt", wt_id(worktree, is_main)),
        worktree=worktree,
        common_dir=common_dir,
        git_dir=git_dir,
        is_main=is_main,
    )


MARKER = "worktrees-provisioned"


def marker_path(lay: Layout) -> str:
    """The provisioning marker, in the worktree's own git directory."""
    return posixpath.join(lay.git_dir, MARKER)


# Languages and variables ----------------------------------------------------

LANGUAGES: Tuple[str, ...] = (
    "c",
    "clojure",
    "dotnet",
    "go",
    "gradle",
    "javascript",
    "maven",
    "nix",
    "python",
    "rust",
    "scala",
    "swift",
    "terraform",
    "xcode",
)

ALIASES: Mapping[str, str] = {
    "babashka": "clojure",
    "c++": "c",
    "cpp": "c",
    "csharp": "dotnet",
    "java": "gradle",
    "js": "javascript",
    "kotlin": "gradle",
    "opentofu": "terraform",
    "tofu": "terraform",
    "ts": "javascript",
    "typescript": "javascript",
}

# Files at a checkout's root that name its languages, as fnmatch patterns.
DETECT: Tuple[Tuple[str, Tuple[str, ...]], ...] = (
    ("rust", ("Cargo.toml",)),
    ("swift", ("Package.swift",)),
    ("xcode", ("*.xcodeproj", "*.xcworkspace")),
    ("javascript", ("package.json",)),
    ("gradle", ("build.gradle", "build.gradle.kts", "settings.gradle*", "gradlew")),
    ("maven", ("pom.xml",)),
    ("python", ("pyproject.toml", "setup.py", "setup.cfg", "requirements*.txt", "uv.lock")),
    ("go", ("go.mod",)),
    ("terraform", ("*.tf",)),
    ("clojure", ("deps.edn", "project.clj", "bb.edn", "shadow-cljs.edn")),
    ("dotnet", ("*.sln", "*.csproj", "*.fsproj")),
    ("scala", ("build.sbt", "build.mill", "build.sc")),
    ("c", ("CMakeLists.txt", "meson.build", "configure.ac")),
    ("nix", ("flake.nix", "default.nix", "shell.nix")),
)

GRADLE_CACHING = "-Dorg.gradle.caching=true"


def detect_languages(names: Iterable[str]) -> Tuple[str, ...]:
    """The languages whose marker files are among `names`, in table order."""
    listed = tuple(names)
    return tuple(
        lang
        for lang, patterns in DETECT
        if any(fnmatch.fnmatchcase(n, p) for n in listed for p in patterns)
    )


def parse_languages(text: str) -> Tuple[Tuple[str, ...], Tuple[str, ...]]:
    """`(languages, unknown)` from a comma- or space-separated list."""
    words = tuple(w.lower() for w in re.split(r"[,\s]+", text) if w)
    named = tuple(ALIASES.get(w, w) for w in words)
    return (
        tuple(dict.fromkeys(n for n in named if n in LANGUAGES)),
        tuple(w for w, n in zip(words, named) if n not in LANGUAGES),
    )


def language_variables(lang: str, lay: Layout, platform: str) -> Tuple[Tuple[str, str], ...]:
    """The variables one language sets, per the README's table."""
    root, wt = lay.root, lay.wt
    table: Mapping[str, Tuple[Tuple[str, str], ...]] = {
        "rust": (
            ("CARGO_BUILD_BUILD_DIR", root + "/shared/cargo-build"),
            ("CARGO_TARGET_DIR", wt + "/target"),
        ),
        "swift": (("CLANG_MODULE_CACHE_PATH", root + "/shared/clang-modules"),),
        "python": (
            ("PYTHONPYCACHEPREFIX", root + "/shared/pycache"),
            ("UV_PROJECT_ENVIRONMENT", wt + "/venv"),
            ("RUFF_CACHE_DIR", wt + "/ruff"),
            ("MYPY_CACHE_DIR", wt + "/mypy"),
        )
        + ((("UV_LINK_MODE", "clone"),) if platform == "darwin" else ()),
        "terraform": (("TF_DATA_DIR", wt + "/tf"),),
        "c": (
            ("CCACHE_DIR", root + "/shared/ccache"),
            ("CCACHE_BASEDIR", lay.worktree),
        ),
    }
    return table.get(lang, ())


def wanted_variables(
    languages: Iterable[str], lay: Layout, platform: str
) -> Tuple[Tuple[str, str], ...]:
    """`WORKTREE_BUILDS_ROOT`, `WORKTREE_BUILDS_WT` and each language's."""
    base = (("WORKTREE_BUILDS_ROOT", lay.root), ("WORKTREE_BUILDS_WT", lay.wt))
    every = base + tuple(
        itertools.chain.from_iterable(language_variables(lang, lay, platform) for lang in languages)
    )
    return tuple(dict(every).items())


class EnvPlan(NamedTuple):
    exports: Tuple[Tuple[str, str], ...]
    unsets: Tuple[str, ...]


SET_VAR = "WORKTREE_BUILDS_SET"
ALWAYS = frozenset({"WORKTREE_BUILDS_ROOT", "WORKTREE_BUILDS_WT"})


def env_plan(
    wanted: Tuple[Tuple[str, str], ...],
    languages: Iterable[str],
    env: Mapping[str, str],
) -> EnvPlan:
    """What `env` prints, given the variables wanted and the environment.

    A variable already set is left alone unless `env` set it itself, as
    recorded in `WORKTREE_BUILDS_SET`, for another worktree, which
    `WORKTREE_BUILDS_WT` names. Names it set before that are no longer
    wanted are unset. `GRADLE_OPTS` gains the build cache flag.
    """
    ours_before = frozenset(n for n in env.get(SET_VAR, "").split(":") if n)
    wanted_map = dict(wanted)
    moved = (
        env.get("WORKTREE_BUILDS_WT", wanted_map["WORKTREE_BUILDS_WT"])
        != wanted_map["WORKTREE_BUILDS_WT"]
    )
    take = tuple(
        (n, v) for n, v in wanted if n in ALWAYS or n not in env or (moved and n in ours_before)
    )
    owned = frozenset(n for n, _ in take) | (ours_before & frozenset(wanted_map))
    gradle_opts = env.get("GRADLE_OPTS", "")
    gradle = (
        (("GRADLE_OPTS", (gradle_opts + " " + GRADLE_CACHING).strip()),)
        if "gradle" in tuple(languages) and GRADLE_CACHING not in gradle_opts.split()
        else ()
    )
    return EnvPlan(
        exports=take + gradle + ((SET_VAR, ":".join(sorted(owned))),),
        unsets=tuple(sorted(n for n in ours_before - frozenset(wanted_map) if n in env)),
    )


def sh_quote(value: str) -> str:
    return "'" + value.replace("'", "'\\''") + "'"


def fish_quote(value: str) -> str:
    return "'" + value.replace("\\", "\\\\").replace("'", "\\'") + "'"


def render_env(plan: EnvPlan, shell: str) -> str:
    """The plan as `sh` or `fish` lines."""
    lines = (
        tuple("set -e {}".format(n) for n in plan.unsets)
        + tuple("set -gx {} {}".format(n, fish_quote(v)) for n, v in plan.exports)
        if shell == "fish"
        else tuple("unset {}".format(n) for n in plan.unsets)
        + tuple("export {}={}".format(n, sh_quote(v)) for n, v in plan.exports)
    )
    return "".join(line + "\n" for line in lines)


# Provisioning --------------------------------------------------------------


def provision_actions(
    is_main: bool,
    worktree_has_settings: bool,
    main_has_settings: bool,
    worktree_envrc: Optional[bytes],
    main_envrc: Optional[bytes],
    direnv_available: bool,
) -> Tuple[str, ...]:
    """`copy-settings` and `direnv-allow`, where each applies."""
    return tuple(
        action
        for action, applies in (
            (
                "copy-settings",
                not is_main and main_has_settings and not worktree_has_settings,
            ),
            (
                "direnv-allow",
                not is_main
                and direnv_available
                and worktree_envrc is not None
                and worktree_envrc == main_envrc,
            ),
        )
        if applies
    )


# Git's output ---------------------------------------------------------------


class Worktree(NamedTuple):
    path: str
    head: str
    branch: Optional[str]
    detached: bool
    bare: bool
    locked: Optional[str]
    prunable: bool
    is_main: bool


def _worktree_from_block(block: Tuple[str, ...], is_main: bool) -> Worktree:
    fields = dict(
        (line.split(" ", 1) + [""])[:2] for line in block
    )  # e.g. {"worktree": path, "HEAD": sha, "branch": ref, "locked": reason}
    branch_ref = fields.get("branch")
    return Worktree(
        path=fields.get("worktree", ""),
        head=fields.get("HEAD", ""),
        branch=branch_ref[len("refs/heads/") :]
        if branch_ref and branch_ref.startswith("refs/heads/")
        else branch_ref,
        detached="detached" in fields,
        bare="bare" in fields,
        locked=fields.get("locked"),
        prunable="prunable" in fields,
        is_main=is_main,
    )


def parse_worktree_list(text: str) -> Tuple[Worktree, ...]:
    """`git worktree list --porcelain`; the first entry is the main checkout."""
    blocks = tuple(
        tuple(group)
        for nonblank, group in itertools.groupby(text.splitlines(), key=bool)
        if nonblank
    )
    return tuple(_worktree_from_block(b, i == 0) for i, b in enumerate(blocks))


def parse_would_remove(text: str) -> Tuple[str, ...]:
    """The paths of `git clean -n`'s `Would remove <path>` lines."""
    prefix = "Would remove "
    return tuple(line[len(prefix) :] for line in text.splitlines() if line.startswith(prefix))


def split_nul(text: str) -> Tuple[str, ...]:
    return tuple(p for p in text.split("\0") if p)


def cherry_all_upstream(text: str) -> bool:
    """True when `git cherry` printed only `-` lines: every commit is upstream."""
    return all(line.startswith("-") for line in text.splitlines() if line.strip())


def stash_branches(text: str) -> Tuple[str, ...]:
    """Branch names from `git stash list --format=%gs` (`WIP on x: …`, `On x: …`)."""
    pattern = re.compile(r"^(?:WIP on|On) (.+?): ")
    return tuple(m.group(1) for m in (pattern.match(line) for line in text.splitlines()) if m)


def parse_pattern_file(text: str) -> Tuple[str, ...]:
    """Patterns of a gitignore-syntax file: no blanks, no comments."""
    return tuple(
        line.rstrip()
        for line in text.splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    )


# Processes and locks ---------------------------------------------------------


class Process(NamedTuple):
    pid: int
    ppid: int
    start: Optional[float]


LSTART_FORMAT = "%a %b %d %H:%M:%S %Y"


def parse_lstart(text: str) -> Optional[float]:
    """`ps -o lstart=` (`Tue Nov 17 03:42:00 2026`), local time, to epoch seconds."""
    try:
        return time.mktime(time.strptime(" ".join(text.split()), LSTART_FORMAT))
    except ValueError:
        return None


def parse_start(text: str) -> Optional[float]:
    """A lock's start time: epoch seconds or milliseconds, ISO 8601, or lstart."""
    stripped = text.strip()
    if re.fullmatch(r"\d+(\.\d+)?", stripped):
        number = float(stripped)
        return number / 1000.0 if number > 1e11 else number
    import calendar  # here, not at the top: `worktrees env` never needs them
    import datetime

    try:
        parsed = datetime.datetime.fromisoformat(stripped.replace("Z", "+00:00"))
        return parsed.timestamp() if parsed.tzinfo else float(calendar.timegm(parsed.timetuple()))
    except ValueError:
        return parse_lstart(stripped)


def parse_ps(text: str) -> Tuple[Process, ...]:
    """`ps -A -o pid= -o ppid= -o lstart=` rows."""
    rows = tuple(line.split(None, 2) for line in text.splitlines())
    return tuple(
        Process(int(r[0]), int(r[1]), parse_lstart(r[2]) if len(r) > 2 else None)
        for r in rows
        if len(r) >= 2 and r[0].isdigit() and r[1].isdigit()
    )


def parse_lsof_cwd(text: str, skip_children_of: int = 0) -> Tuple[Tuple[int, str], ...]:
    """`(pid, cwd)` pairs from `lsof -d cwd -F pRn` output.

    Processes whose parent is `skip_children_of` are left out, so `lsof`
    itself, started by this program, never counts as being inside.
    """
    marked = tuple((line[:1], line[1:]) for line in text.splitlines() if line)
    owners = tuple(
        itertools.accumulate(
            marked,
            lambda owner, item: (
                (int(item[1]), 0)
                if item[0] == "p"
                else (owner[0], int(item[1]))
                if item[0] == "R" and item[1].isdigit()
                else owner
            ),
            initial=(0, 0),
        )
    )[1:]
    return tuple(
        (pid, value)
        for (pid, ppid), (tag, value) in zip(owners, marked)
        if tag == "n" and not (skip_children_of and ppid == skip_children_of)
    )


def ancestors(pid: int, parents: Mapping[int, int]) -> FrozenSet[int]:
    """`pid` and every process above it, up to 64 levels."""
    chain = itertools.accumulate(range(64), lambda p, _: parents.get(p, 0), initial=pid)
    return frozenset(itertools.takewhile(lambda p: p > 1, chain))


def is_inside(path: str, folder: str) -> bool:
    base = folder.rstrip("/")
    return path == base or path.startswith(base + "/")


def pids_inside(
    cwds: Iterable[Tuple[int, str]], folder: str, exclude: FrozenSet[int]
) -> Tuple[int, ...]:
    """Processes whose working directory is in `folder`, other than `exclude`."""
    return tuple(
        sorted({pid for pid, cwd in cwds if pid not in exclude and is_inside(cwd, folder)})
    )


CLAUDE_LOCK = re.compile(r"^claude agent\b.*\(pid (\d+) start ([^)]*)\)\s*$")


def classify_lock(
    reason: Optional[str], processes: Mapping[int, Process], tolerance: float = 2.0
) -> str:
    """`none`, `person`, `claude-live` or `claude-stale`.

    A Claude Code lock is stale when its pid is gone or the pid's start time
    differs. A start time that can't be read keeps the lock live.
    """
    match = CLAUDE_LOCK.match(reason) if reason is not None else None
    process = processes.get(int(match.group(1))) if match else None
    lock_start = parse_start(match.group(2)) if match else None
    return (
        "none"
        if reason is None
        else "person"
        if match is None
        else "claude-stale"
        if process is None
        else "claude-live"
        if lock_start is None or process.start is None
        else "claude-live"
        if abs(lock_start - process.start) <= tolerance
        else "claude-stale"
    )


def claude_project_folder(path: str) -> str:
    """Claude Code's `~/.claude/projects/` folder name for a working directory."""
    return re.sub(r"[^A-Za-z0-9]", "-", path)


# Decisions ------------------------------------------------------------------


class Facts(NamedTuple):
    """What the shell found out about one worktree."""

    worktree: Worktree
    changes: Tuple[str, ...]  # `git status --porcelain` lines
    landed: Optional[str]  # the upstream ref it landed in
    own_commits: bool  # its tip is on no other ref
    holder: Optional[str]  # the ref it landed in, else another branch holding its tip
    stashes: int
    unique_kept: Tuple[str, ...]  # ignored, kept files the main checkout lacks
    lock: str  # classify_lock's verdict
    inside: Tuple[int, ...]  # pids with their working directory inside
    fresh_transcript: bool
    protected: bool  # its branch is the default or integration branch
    upstreams: Tuple[str, ...]


# `Facts.inside` when `lsof` is missing: in use, to be safe.
UNKNOWN_INSIDE: Tuple[int, ...] = (-1,)


def tracked_changes(changes: Iterable[str]) -> Tuple[str, ...]:
    return tuple(c for c in changes if not c.startswith("??") and not c.startswith("!!"))


def in_use_reasons(f: Facts, with_transcript: bool) -> Tuple[str, ...]:
    return tuple(
        reason
        for applies, reason in (
            (f.lock == "person", "locked by a person: {}".format(f.worktree.locked)),
            (f.lock == "claude-live", "locked by a live Claude Code process"),
            (
                f.inside == UNKNOWN_INSIDE,
                "lsof is missing, so whether a process is inside is unknown",
            ),
            (
                bool(f.inside) and f.inside != UNKNOWN_INSIDE,
                "in use by process {}".format(", ".join(map(str, f.inside))),
            ),
            (
                with_transcript and f.fresh_transcript,
                "a Claude Code session wrote its transcript recently",
            ),
        )
        if applies
    )


class Verdict(NamedTuple):
    action: str  # finish | clean | keep | skip
    reasons: Tuple[str, ...]
    unlock: bool  # clear a stale Claude Code lock first


def finish_reasons(f: Facts, log_given: bool, log_problem: Optional[str]) -> Tuple[str, ...]:
    """Why `finish` keeps a worktree; empty when it may go."""
    landed_or_log = f.landed is not None or (
        log_given and log_problem is None and not f.own_commits
    )
    unlanded = (
        "log: {}".format(log_problem)
        if log_given and log_problem
        else "branch has commits of its own"
        if log_given
        else "branch has not landed in {}".format(", ".join(f.upstreams) or "any upstream")
    )
    return tuple(
        reason
        for applies, reason in (
            (f.worktree.is_main, "the main checkout"),
            (f.worktree.bare, "a bare repository"),
            (
                f.protected,
                "branch {} is the default or integration branch".format(f.worktree.branch),
            ),
            (bool(f.changes), "{} uncommitted paths".format(len(f.changes))),
            (f.stashes > 0, "{} stashes on its branch".format(f.stashes)),
            (
                bool(f.unique_kept),
                "kept files: {}".format(", ".join(f.unique_kept)),
            ),
            (not landed_or_log, unlanded),
        )
        if applies
    ) + in_use_reasons(f, with_transcript=False)


def finish_verdict(f: Facts, log_given: bool, log_problem: Optional[str]) -> Verdict:
    reasons = finish_reasons(f, log_given, log_problem)
    return Verdict(
        action="keep" if reasons else "finish",
        reasons=reasons,
        unlock=not reasons and f.lock == "claude-stale",
    )


def clean_reasons(f: Facts) -> Tuple[str, ...]:
    """Why `clean` refuses a worktree; empty when it may clean."""
    tracked = tracked_changes(f.changes)
    return tuple(
        reason
        for applies, reason in (
            (f.worktree.is_main, "the main checkout"),
            (f.worktree.bare, "a bare repository"),
            (bool(tracked), "{} tracked files with changes".format(len(tracked))),
        )
        if applies
    ) + in_use_reasons(f, with_transcript=False)


def sweep_verdict(f: Facts) -> Verdict:
    """What the sweep does with one worktree."""
    stale = f.lock == "claude-stale"
    busy = in_use_reasons(f, with_transcript=True)
    finish = finish_reasons(f, False, None)
    clean = clean_reasons(f)
    return (
        Verdict("skip", ("the main checkout",), False)
        if f.worktree.is_main
        else Verdict("skip", busy, stale)
        if busy
        else Verdict("finish", (), stale)
        if not finish
        else Verdict("clean", finish, stale)
        if not clean
        else Verdict("keep", finish + clean, stale)
    )


# `clean` ---------------------------------------------------------------------


def under(path: str, entry: str) -> bool:
    return path == entry.rstrip("/") or (entry.endswith("/") and path.startswith(entry))


def clean_plan(
    entries: Iterable[str], kept: FrozenSet[str], nested: Iterable[str]
) -> Tuple[Tuple[str, ...], Tuple[str, ...]]:
    """`(whole, mixed)` from `git clean -ndX`'s entries.

    A whole entry holds nothing kept and no nested repository, and goes at
    once. A mixed one is a folder holding a kept file or a nested
    repository; only its own ignored, unkept files go. A kept file entry
    stays.
    """
    guarded = tuple(kept) + tuple(nested)
    listed = tuple(entries)
    return (
        tuple(e for e in listed if not any(under(g, e) for g in guarded)),
        tuple(
            e for e in listed if e.endswith("/") and any(under(g, e) and g != e for g in guarded)
        ),
    )


def files_to_remove(ignored: Iterable[str], kept: FrozenSet[str]) -> Tuple[str, ...]:
    """Ignored files, not nested repositories, that no keep pattern holds."""
    return tuple(p for p in ignored if not p.endswith("/") and p not in kept)


# The sweep's other steps ------------------------------------------------------


def orphan_wt_dirs(
    entries: Iterable[Tuple[str, Optional[str]]], live: FrozenSet[str]
) -> Tuple[str, ...]:
    """`wt/<id>/` folders whose `WORKTREE` names no current worktree."""
    return tuple(d for d, path in entries if path is not None and path not in live)


def over_cap(size_bytes: int, cap_gb: float) -> bool:
    return size_bytes > cap_gb * 1e9


# Disk and floors ---------------------------------------------------------------


def parse_df(text: str) -> Optional[int]:
    """Available bytes from `df -Pk <path>`."""
    lines = text.strip().splitlines()
    fields = lines[-1].split() if len(lines) >= 2 else []
    return int(fields[-3]) * 1024 if len(fields) >= 6 and fields[-3].isdigit() else None


class Floors(NamedTuple):
    build_gb: float
    cleanup_only_gb: float


FLOOR_BUILD = "WORKTREES_FLOOR_BUILD_GB"
FLOOR_CLEANUP = "WORKTREES_FLOOR_CLEANUP_ONLY_GB"


def _number(text: Optional[str]) -> Optional[float]:
    try:
        return float(text) if text is not None else None
    except ValueError:
        return None


def parse_floors(file_text: str, env: Mapping[str, str]) -> Floors:
    """The floors: the variables, then the floor file's `KEY=value` lines, then 70 and 35."""
    pairs = dict(
        (k.strip(), v.strip().strip("'\""))
        for k, _, v in (
            line.partition("=")
            for line in file_text.splitlines()
            if "=" in line and not line.lstrip().startswith("#")
        )
    )

    def pick(key: str, default: float) -> float:
        return next(
            (n for n in (_number(env.get(key)), _number(pairs.get(key))) if n is not None),
            default,
        )

    return Floors(pick(FLOOR_BUILD, 70.0), pick(FLOOR_CLEANUP, 35.0))


def floor_state(free_bytes: Optional[int], floors: Floors) -> str:
    gb = None if free_bytes is None else free_bytes / 1e9
    return (
        "unknown"
        if gb is None
        else "below-cleanup-floor"
        if gb < floors.cleanup_only_gb
        else "below-build-floor"
        if gb < floors.build_gb
        else "ok"
    )


# Formatting ---------------------------------------------------------------------


def human_bytes(n: Optional[int]) -> str:
    if n is None:
        return "?"
    units = ("B", "K", "M", "G", "T")
    steps = tuple(itertools.takewhile(lambda i: n >= 1024 ** (i + 1), range(len(units) - 1)))
    exp = len(steps)
    return "{:.0f}{}".format(n / 1024**exp, units[exp]) if exp else "{}B".format(n)


def human_age(seconds: Optional[float]) -> str:
    if seconds is None:
        return "?"
    return (
        "{}m".format(int(seconds // 60))
        if seconds < 3600
        else "{}h".format(int(seconds // 3600))
        if seconds < 86400
        else "{}d".format(int(seconds // 86400))
    )


def table(rows: Tuple[Tuple[str, ...], ...]) -> str:
    """Rows as left-aligned columns separated by two spaces."""
    widths = tuple(max(len(r[i]) for r in rows) for i in range(len(rows[0]))) if rows else ()
    return "".join(
        "  ".join(cell.ljust(w) for cell, w in zip(row, widths)).rstrip() + "\n" for row in rows
    )


def merge_config(defaults: Mapping, user: Mapping) -> Dict:
    """User settings over the defaults: maps merge, everything else replaces."""
    return {
        **defaults,
        **{
            k: merge_config(defaults[k], v)
            if isinstance(v, Mapping) and isinstance(defaults.get(k), Mapping)
            else v
            for k, v in user.items()
        },
    }


def unknown_keys(defaults: Mapping, user: Mapping, prefix: str = "") -> Tuple[str, ...]:
    return tuple(
        itertools.chain.from_iterable(
            unknown_keys(defaults[k], v, prefix + k + ".")
            if isinstance(v, Mapping) and isinstance(defaults.get(k), Mapping)
            else (() if k in defaults else (prefix + k,))
            for k, v in user.items()
        )
    )
