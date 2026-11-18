"""The functional core of `worktrees` behind `env`, `provision` and `clean`.

The build root, the language variables, provisioning, git's porcelain
output, `clean`'s plan and the formatting helpers, as pure functions.
Nothing here reads files, the clock or the environment, or runs a command:
the shell passes those in as plain values. `worktrees.rules` holds the
sweep's decisions.

Compatible with Python 3.9, macOS's `/usr/bin/python3`.
"""

from __future__ import annotations

import fnmatch
import hashlib
import itertools
import posixpath
import re
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


def repo_key(common_dir: str) -> str:
    """`<repo>-<hash8>`, the per-repository folder name under the build and state roots."""
    return "{}-{}".format(repo_name(common_dir), sha8(common_dir))


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
    return posixpath.join(builds_top(env), repo_key(common_dir))


def archive_top(env: Mapping[str, str]) -> str:
    """`~/backups/git-archive`: one bare repository of archives per project, `<owner>/<repo>.git`."""
    return posixpath.join(env.get("HOME", ""), "backups", "git-archive")


def repo_state_dir(env: Mapping[str, str], common_dir: str) -> str:
    """`${XDG_STATE_HOME:-~/.local/state}/worktrees/<repo>-<hash8>`: the log and the sweep's files."""
    return posixpath.join(state_home(env), "worktrees", repo_key(common_dir))


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


def parse_pattern_file(text: str) -> Tuple[str, ...]:
    """Patterns of a gitignore-syntax file: no blanks, no comments."""
    return tuple(
        line.rstrip()
        for line in text.splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    )


def is_inside(path: str, folder: str) -> bool:
    base = folder.rstrip("/")
    return path == base or path.startswith(base + "/")


def claude_project_folder(path: str) -> str:
    """Claude Code's `~/.claude/projects/` folder name for a working directory."""
    return re.sub(r"[^A-Za-z0-9]", "-", path)


# `clean` ---------------------------------------------------------------------


def tracked_changes(changes: Iterable[str]) -> Tuple[str, ...]:
    """`git status --porcelain` lines other than untracked and ignored ones."""
    return tuple(c for c in changes if not c.startswith("??") and not c.startswith("!!"))


def clean_refusals(is_main: bool, bare: bool, changes: Iterable[str]) -> Tuple[str, ...]:
    """Why `clean` refuses a worktree; empty when it may clean."""
    tracked = tracked_changes(changes)
    return tuple(
        reason
        for applies, reason in (
            (is_main, "the main checkout"),
            (bare, "a bare repository"),
            (bool(tracked), "{} tracked files with changes".format(len(tracked))),
        )
        if applies
    )


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
        "{}s".format(int(seconds))
        if seconds < 60
        else "{}m".format(int(seconds // 60))
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
