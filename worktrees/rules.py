"""The sweep's rules: every decision about a worktree or a branch, as pure functions.

The shell (`worktrees.gather`) reads git and the operating system into the
records below; `plan` turns them into what the sweep does, and why it keeps
the rest. Nothing here reads a file, the clock or the environment, or runs
a command. Failures are values: a reason, never an exception.

Compatible with Python 3.9, macOS's `/usr/bin/python3`.
"""

from __future__ import annotations

import calendar
import datetime
import fnmatch
import functools
import re
import time
from typing import Dict, FrozenSet, Iterable, Mapping, NamedTuple, Optional, Tuple

# Times and durations ----------------------------------------------------------

ZERO_OID = re.compile(r"0+\Z")
DURATION = re.compile(r"\s*(\d+(?:\.\d+)?)\s*([smhdw]?)\s*\Z")
UNITS: Mapping[str, float] = {"": 1, "s": 1, "m": 60, "h": 3600, "d": 86400, "w": 604800}
LSTART_FORMAT = "%a %b %d %H:%M:%S %Y"

DAY = 86400.0


def is_zero(oid: Optional[str]) -> bool:
    """True for git's all-zero object name, which stands for a missing ref."""
    return bool(oid) and ZERO_OID.match(oid or "") is not None


def parse_duration(text: Optional[str]) -> Optional[float]:
    """Seconds from `90`, `90s`, `30m`, `1h`, `1d` or `2w`; None when unreadable."""
    match = DURATION.match(text or "")
    return float(match.group(1)) * UNITS[match.group(2)] if match else None


def parse_lstart(text: str, utc: bool = False) -> Optional[float]:
    """`ps -o lstart=` (`Sat Nov 14 16:15:55 2026`), in local time or UTC, to epoch seconds."""
    try:
        parsed = time.strptime(" ".join(text.split()), LSTART_FORMAT)
    except ValueError:
        return None
    return float(calendar.timegm(parsed)) if utc else time.mktime(parsed)


def parse_time(text: Optional[str]) -> Optional[float]:
    """Epoch seconds from epoch seconds or milliseconds, ISO 8601, or `ps`'s lstart."""
    stripped = (text or "").strip()
    if re.fullmatch(r"\d+(\.\d+)?", stripped):
        number = float(stripped)
        return number / 1000.0 if number > 1e11 else number
    try:
        parsed = datetime.datetime.fromisoformat(re.sub(r"Z\Z", "+00:00", stripped))
    except ValueError:
        return parse_lstart(stripped) if stripped else None
    return parsed.timestamp() if parsed.tzinfo else float(calendar.timegm(parsed.timetuple()))


def iso(seconds: float) -> str:
    """UTC, to the second: `2026-11-18T09:12:00Z`."""
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(seconds))


def archive_ref(branch: str, seconds: float) -> str:
    """`refs/archive/<branch>/<UTC time>`; a ref name can't hold a colon."""
    return "refs/archive/{}/{}".format(
        branch, time.strftime("%Y-%m-%dT%H%M%SZ", time.gmtime(seconds))
    )


def unique_refs(wanted: Tuple[str, ...], existing: FrozenSet[str]) -> Tuple[str, ...]:
    """`wanted` with `-2`, `-3`… added to any name that is taken or repeated."""

    def free(ref: str, taken: FrozenSet[str]) -> str:
        return next(
            candidate
            for candidate in (ref if n == 1 else "{}-{}".format(ref, n) for n in range(1, 1000))
            if candidate not in taken
        )

    def step(acc: Tuple[Tuple[str, ...], FrozenSet[str]], ref: str):
        chosen = free(ref, acc[1])
        return acc[0] + (chosen,), acc[1] | {chosen}

    empty: Tuple[str, ...] = ()
    return functools.reduce(step, wanted, (empty, existing))[0]


# Settings ----------------------------------------------------------------------


class Settings(NamedTuple):
    sweep: str  # `report` or `apply`, for a sweep a hook starts
    idle: float  # seconds a worktree's git directory must be unchanged
    lock_report: float  # seconds a landed worktree may stay locked before the report names it
    archive_push: bool


TRUE_WORDS = frozenset({"true", "yes", "on", "1"})


def settings(defaults: Mapping, config: Mapping[str, str]) -> Settings:
    """The settings: `worktrees.*` from git config over `defaults.json`'s `settings`.

    `config` has git's lower-cased keys, such as `worktrees.lockreport`.
    """

    def pick(key: str) -> str:
        return str(config.get("worktrees." + key.lower(), defaults[key]))

    def duration(key: str, fallback: float) -> float:
        found = (parse_duration(pick(key)), parse_duration(str(defaults[key])))
        return next((d for d in found if d is not None), fallback)

    return Settings(
        sweep="apply" if pick("sweep").strip().lower() == "apply" else "report",
        idle=duration("idle", 3600.0),
        lock_report=duration("lockReport", DAY),
        archive_push=pick("archivePush").strip().lower() in TRUE_WORDS,
    )


def parse_config(text: str) -> Dict[str, str]:
    """`git config --get-regexp` lines, `key value`, as a mapping; the last one wins."""
    return dict((line.split(" ", 1) + [""])[:2] for line in text.splitlines() if line)


# Locks -----------------------------------------------------------------------------

CLAUDE_LOCK = re.compile(
    r"^(?P<who>claude agent \S+) \(pid (?P<pid>\d+) start (?P<start>[^)]*)\)\s*$"
)
SINCE_LOCK = re.compile(r"^(?P<who>.*?)\s+since\s+(?P<since>\S.*?)\s*$")
PID_IN = re.compile(r"\bpid (\d+)\b")


class Lock(NamedTuple):
    reason: str
    who: str
    pid: Optional[int]
    since: Optional[float]  # when the lock was taken, or when its process started
    form: str  # `claude-agent`, `since` or `other`


def parse_lock(reason: str) -> Lock:
    """A lock reason: `<who> since <time>`, or Claude Code's `claude agent <id> (pid <pid> start <time>)`."""
    claude = CLAUDE_LOCK.match(reason)
    since = SINCE_LOCK.match(reason)
    pid = PID_IN.search(reason)
    return (
        Lock(
            reason,
            claude.group("who"),
            int(claude.group("pid")),
            parse_time(claude.group("start")),
            "claude-agent",
        )
        if claude
        else Lock(
            reason,
            since.group("who"),
            int(pid.group(1)) if pid else None,
            parse_time(since.group("since")),
            "since",
        )
        if since
        else Lock(reason, reason, int(pid.group(1)) if pid else None, None, "other")
    )


def process_alive(
    lock: Lock, process_start: Optional[float], tolerance: float = 2.0
) -> Optional[bool]:
    """Whether the process a lock names still runs; None when it names none.

    `process_start` is what `ps -o lstart= -p <pid>` gives, None when no
    such process runs. Claude Code's form names the process's own start
    time, which must match; the `since` form names when the lock was taken,
    which the process must predate. A pid reused by a later process fails
    either test.
    """
    return (
        None
        if lock.pid is None
        else False
        if process_start is None
        else True
        if lock.since is None
        else abs(process_start - lock.since) <= tolerance
        if lock.form == "claude-agent"
        else process_start <= lock.since + tolerance
    )


class LockFacts(NamedTuple):
    lock: Lock
    age: Optional[float]  # seconds since `since`, or since the lock file was written
    alive: Optional[bool]
    process_start: Optional[float]


def lock_detail(facts: LockFacts) -> str:
    alive = (
        ""
        if facts.alive is None
        else "; pid {} is alive".format(facts.lock.pid)
        if facts.alive
        else "; pid {} is gone".format(facts.lock.pid)
    )
    return "locked: {}{}".format(facts.lock.reason or "(no reason)", alive)


# Keep patterns ------------------------------------------------------------------------


class Pattern(NamedTuple):
    text: str
    body: str  # without a leading or trailing slash
    dir_only: bool
    anchored: bool  # holds a slash, so it matches from the worktree's root


def parse_pattern(text: str) -> Pattern:
    stripped = text.strip()
    body = stripped.strip("/")
    return Pattern(
        text=text,
        body=body,
        dir_only=stripped.endswith("/"),
        anchored=stripped.startswith("/") or "/" in body,
    )


def _path_match(path: str, body: str) -> bool:
    return fnmatch.fnmatchcase(path, body) and path.count("/") == body.count("/")


def pattern_matches(pattern: Pattern, entry: str) -> bool:
    """Whether a keep pattern holds `entry`, a path from `git ls-files`, folders ending in `/`.

    As in gitignore, a pattern without a slash matches a name at any depth,
    one with a slash matches from the root, and a trailing slash matches
    only folders. An entry inside a matching folder matches too.
    """
    is_dir = entry.endswith("/")
    parts = tuple(p for p in entry.split("/") if p)
    prefixes = tuple(
        ("/".join(parts[: i + 1]), i < len(parts) - 1 or is_dir) for i in range(len(parts))
    )
    return any(
        (not pattern.dir_only or folder)
        and (
            _path_match(path, pattern.body)
            if pattern.anchored
            else fnmatch.fnmatchcase(path.rsplit("/", 1)[-1], pattern.body)
        )
        for path, folder in prefixes
    )


def kept_entries(entries: Iterable[str], patterns: Iterable[Pattern]) -> Tuple[str, ...]:
    """The ignored entries a keep pattern holds."""
    every = tuple(patterns)
    return tuple(e for e in entries if any(pattern_matches(p, e) for p in every))


def probes(entries: Iterable[str], patterns: Iterable[Pattern]) -> Tuple[str, ...]:
    """Anchored keep patterns that reach inside an ignored folder no pattern holds.

    `git ls-files --directory` stops at an ignored folder, so a pattern such
    as `.claude/settings.local.json` under an ignored `.claude/` is looked
    up at its own path rather than by walking the folder.
    """
    every = tuple(patterns)
    folders = tuple(
        e for e in entries if e.endswith("/") and not any(pattern_matches(p, e) for p in every)
    )
    return tuple(
        dict.fromkeys(
            p.body for p in every if p.anchored and any(p.body.startswith(f) for f in folders)
        )
    )


# Content landed -------------------------------------------------------------------------


def parse_raw_diff(text: str) -> Tuple[Tuple[str, str, str], ...]:
    """`(path, mode, object)` on the new side of `git diff-tree -r -z --no-renames`; a deletion has mode `000000`."""
    fields = tuple(text.split("\0"))
    metas = tuple(i for i, f in enumerate(fields) if f.startswith(":") and i + 1 < len(fields))
    return tuple(
        (fields[i + 1], parts[1], parts[3])
        for i in metas
        for parts in (fields[i][1:].split(" "),)
        if len(parts) >= 5
    )


def parse_ls_tree(text: str) -> Mapping[str, Tuple[str, str]]:
    """`git ls-tree -r -z` as `{path: (mode, object)}`."""
    rows = tuple(entry.split("\t", 1) for entry in text.split("\0") if "\t" in entry)
    return {path: (meta.split(" ")[0], meta.split(" ")[2]) for meta, path in rows}


def changes_present(
    changes: Iterable[Tuple[str, str, str]], tree: Mapping[str, Tuple[str, str]]
) -> bool:
    """Whether the default branch's tree already holds every change, path by path.

    Then a merge of the branch into the default branch leaves the default
    branch's tree as it is, which is what `git merge-tree --write-tree`
    would show, without writing or freshening any object.
    """
    return all(
        path not in tree if mode == "000000" else tree.get(path) == (mode, obj)
        for path, mode, obj in changes
    )


# Facts ---------------------------------------------------------------------------------


class WorktreeFacts(NamedTuple):
    path: str
    branch: Optional[str]
    head: str
    is_main: bool
    exists: bool
    lock: Optional[LockFacts]
    landed: Optional[bool]  # HEAD is an ancestor of origin/<default>; None without one
    changes: Tuple[str, ...]  # `git status --porcelain --untracked-files=all` lines
    kept: Tuple[str, ...]  # ignored entries a keep pattern holds, unlike the main checkout's
    cleanable: Tuple[str, ...]  # ignored entries no keep pattern holds
    idle: Optional[float]  # seconds since HEAD, logs/HEAD or the index last changed
    broken: Optional[str] = None  # why its folder isn't a working worktree of this repository


class BranchFacts(NamedTuple):
    name: str
    tip: str
    upstream: Optional[str]
    track: str  # `%(upstream:track,nobracket)`: ``, `gone`, `ahead 1`, `ahead 1, behind 2`
    landed: Optional[bool]
    content_landed: Optional[bool]
    age: Optional[float]
    archived: bool  # an archive ref points at its tip
    is_default: bool


class RepoFacts(NamedTuple):
    main: str
    common_dir: str
    state_dir: str
    default_branch: Optional[str]
    default_ref: Optional[str]
    default_problem: Optional[str]
    settings: Settings
    worktrees: Tuple[WorktreeFacts, ...]
    branches: Tuple[BranchFacts, ...]
    archives: Tuple[Tuple[str, str], ...]  # (ref, object)
    merged: FrozenSet[str]  # refs whose tip is an ancestor of origin/<default>
    events: Tuple[Mapping, ...]
    rescues: Tuple[Tuple[str, str], ...]  # (branch, old tip): deleted, unreachable, present
    refs: Tuple[Tuple[str, str], ...] = ()  # every ref and the commit it points at


class Reason(NamedTuple):
    code: str
    detail: str


class Test(NamedTuple):
    name: str
    ok: bool
    detail: str


# Branches -------------------------------------------------------------------------------

BRANCH_ACTIONS: Mapping[str, str] = {
    "landed": "delete",
    "preview": "delete",
    "content-landed": "archive+delete",
}


def branch_state(b: BranchFacts, default_known: bool) -> str:
    """The row of the branch table a branch is in."""
    track = b.track or ""
    return (
        "default-branch"
        if b.is_default
        else "no-default-branch"
        if not default_known
        else "landed"
        if b.landed
        else "preview"
        if b.name.startswith("preview/")
        else "content-landed"
        if b.content_landed
        else "upstream-gone"
        if b.upstream and track == "gone"
        else "diverged"
        if b.upstream and "ahead" in track and "behind" in track
        else "in-review"
        if b.upstream
        else "local-only"
    )


def state_detail(b: BranchFacts, state: str, default_ref: Optional[str]) -> str:
    details: Mapping[str, str] = {
        "default-branch": "the default branch",
        "no-default-branch": "no default branch, so nothing is removed",
        "landed": "its tip is an ancestor of {}".format(default_ref),
        "preview": "a preview branch",
        "content-landed": "merging it into {} changes nothing".format(default_ref),
        "upstream-gone": "its upstream {} is gone: its pull request was closed or its branch "
        "deleted".format(b.upstream),
        "diverged": "it and {} each have commits the other lacks ({})".format(b.upstream, b.track),
        "in-review": "its upstream {} exists".format(b.upstream),
        "local-only": "no upstream{}: open a pull request, or archive it".format(
            "" if b.age is None else ", {:.0f} days old".format(b.age / DAY)
        ),
    }
    return details[state]


def short_ref(ref: Optional[str]) -> str:
    return (
        (ref or "")[len("refs/remotes/") :]
        if (ref or "").startswith("refs/remotes/")
        else ref or ""
    )


def elsewhere(
    tip: str,
    own: Optional[str],
    refs: Iterable[Tuple[str, str]],
    merged: bool,
    default_ref: Optional[str],
    going: FrozenSet[str],
) -> Tuple[str, ...]:
    """Where a commit survives besides `own`: merged, pushed, or identical to a ref that stays.

    `going` names the refs this sweep deletes, which count for nothing.
    """
    listed = tuple(
        (ref, commit)
        for ref, commit in refs
        if commit == tip and ref != own and ref not in going and not ref.endswith("/HEAD")
    )
    return (
        (("merged into {}".format(short_ref(default_ref)),) if merged else ())
        + tuple(
            "pushed as {} at {}".format(short_ref(ref), tip[:12])
            for ref, _ in listed
            if ref.startswith("refs/remotes/")
        )
        + tuple(
            "identical to {}".format(ref)
            for ref, _ in listed
            if not ref.startswith("refs/remotes/")
        )
    )


class BranchVerdict(NamedTuple):
    facts: BranchFacts
    state: str
    action: str  # `delete`, `archive+delete` or `keep`
    reasons: Tuple[Reason, ...]
    worktree: Optional[str]  # the worktree it is checked out in
    survives: Tuple[str, ...]  # where its tip survives already, before any archive


def branch_verdict(
    b: BranchFacts,
    default_known: bool,
    default_ref: Optional[str],
    worktree: Optional[str],
    worktree_action: Optional[str],
    survives: Tuple[str, ...] = (),
) -> BranchVerdict:
    """What the sweep does with a branch, once any worktree it is checked out in is gone.

    Nothing is deleted unless its tip survives elsewhere: merged into the
    default branch, pushed, or identical to a ref that stays. Otherwise it
    is archived first, and the archive pushed, or kept.
    """
    state = branch_state(b, default_known)
    held = worktree is not None and worktree_action != "remove"
    wanted = BRANCH_ACTIONS.get(state, "keep")
    action = (
        "keep"
        if held or wanted == "keep"
        else "delete"
        if b.landed or survives
        else "archive+delete"
    )
    return BranchVerdict(
        facts=b,
        state=state,
        action=action,
        reasons=((Reason("checked-out", "checked out in {}".format(worktree)),) if held else ())
        + ((Reason(state, state_detail(b, state, default_ref)),) if wanted == "keep" else ()),
        worktree=worktree,
        survives=survives,
    )


# Worktrees ------------------------------------------------------------------------------

TEST_REASONS: Mapping[str, str] = {
    "folder": "folder-missing",
    "working": "broken",
    "not-main": "main-worktree",
    "default-branch": "no-default-branch",
    "unlocked": "locked",
    "landed": "not-landed",
    "clean": "uncommitted",
    "keep-files": "keep-pattern-files",
    "idle": "not-idle",
}


class WorktreeVerdict(NamedTuple):
    facts: WorktreeFacts
    action: str  # `remove`, `prune`, `clean` or `keep`
    tests: Tuple[Test, ...]
    reasons: Tuple[Reason, ...]
    lock_report: bool  # landed, and locked longer than `worktrees.lockReport`
    survives: Tuple[str, ...] = ()  # where its HEAD survives already, before any archive


def landed_test(
    f: WorktreeFacts,
    state: Optional[str],
    default_ref: Optional[str],
    default_problem: Optional[str],
) -> Test:
    return (
        Test("landed", False, default_problem or "no default branch")
        if f.landed is None
        else Test("landed", True, "HEAD is an ancestor of {}".format(default_ref))
        if f.landed
        else Test("landed", True, "its branch's content has landed; it is archived first")
        if state == "content-landed"
        else Test(
            "landed",
            True,
            "a preview branch, which never lands; archived first unless it survives elsewhere",
        )
        if state == "preview"
        else Test(
            "landed",
            False,
            "HEAD is not an ancestor of {}{}".format(
                default_ref, "" if state is None else " ({})".format(state)
            ),
        )
    )


def worktree_tests(
    f: WorktreeFacts,
    state: Optional[str],
    default_ref: Optional[str],
    default_problem: Optional[str],
    idle_needed: float,
) -> Tuple[Test, ...]:
    """Every removal rule's result for one worktree, in the rules' order."""
    folder = () if f.exists else (Test("folder", False, "its folder {} is gone".format(f.path)),)
    working = (
        (Test("working", False, "not a working worktree: {}".format(f.broken)),)
        if f.broken is not None
        else ()
    )
    return (
        folder
        + working
        + (
            Test(
                "not-main", not f.is_main, "the main worktree" if f.is_main else "a linked worktree"
            ),
            Test(
                "default-branch",
                default_problem is None,
                default_problem or "{} is the default".format(default_ref),
            ),
            Test("unlocked", f.lock is None, lock_detail(f.lock) if f.lock else "not locked"),
            landed_test(f, state, default_ref, default_problem),
            Test(
                "clean",
                not f.changes,
                "{} uncommitted paths: {}".format(
                    len(f.changes), ", ".join(c[3:] for c in f.changes[:5])
                )
                if f.changes
                else "nothing uncommitted",
            ),
            Test(
                "keep-files",
                not f.kept,
                "kept files unlike the main checkout's: {}".format(", ".join(f.kept))
                if f.kept
                else "no kept files of its own",
            ),
            Test(
                "idle",
                f.idle is not None and f.idle >= idle_needed,
                "idle {} of the {} needed".format(_age(f.idle), _age(idle_needed)),
            ),
        )
    )


def _age(seconds: Optional[float]) -> str:
    return (
        "?"
        if seconds is None
        else "{:.0f}s".format(seconds)
        if seconds < 60
        else "{:.0f}m".format(seconds / 60)
        if seconds < 3600
        else "{:.0f}h".format(seconds / 3600)
        if seconds < DAY
        else "{:.0f}d".format(seconds / DAY)
    )


def worktree_verdict(
    f: WorktreeFacts,
    state: Optional[str],
    default_ref: Optional[str],
    default_problem: Optional[str],
    s: Settings,
) -> WorktreeVerdict:
    """What the sweep does with one worktree, with every test's result.

    Removable when it isn't the main worktree, is unlocked, has landed, is
    clean, holds no kept file of its own and has been idle. A missing
    folder is pruned unless locked. An unlocked worktree that hasn't landed
    and has been idle a day has its ignored files cleaned.
    """
    tests = worktree_tests(f, state, default_ref, default_problem, s.idle)
    failing = tuple(t for t in tests if not t.ok)
    landed = next(t.ok for t in tests if t.name == "landed")
    tracked = tuple(c for c in f.changes if not c.startswith("??"))
    action = (
        ("prune" if f.lock is None and not f.is_main else "keep")
        if not f.exists
        else "remove"
        if not failing
        else "clean"
        if not f.is_main
        and f.broken is None
        and f.lock is None
        and default_problem is None
        and not landed
        and f.idle is not None
        and f.idle >= DAY
        and not tracked
        and bool(f.cleanable)
        else "keep"
    )
    reasons = (
        ()
        if action in ("remove", "prune")
        else (Reason("main-worktree", "the main worktree"),)
        if f.is_main
        else tuple(
            Reason(TEST_REASONS[t.name], t.detail)
            for t in failing
            # Without a default branch, landing is unknown rather than refused.
            if not (t.name == "landed" and f.landed is None)
        )
    )
    return WorktreeVerdict(
        facts=f,
        action=action,
        tests=tests,
        reasons=reasons,
        lock_report=f.lock is not None
        and landed
        and f.lock.age is not None
        and f.lock.age >= s.lock_report,
    )


# The log -------------------------------------------------------------------------------


def logged_refs(events: Iterable[Mapping]) -> FrozenSet[str]:
    return frozenset(e["ref"] for e in events if isinstance(e.get("ref"), str))


def has_snapshot(events: Iterable[Mapping]) -> bool:
    return any(e.get("event") == "snapshot" for e in events)


def _oid(value: object) -> Optional[str]:
    return value if isinstance(value, str) and value and not is_zero(value) else None


def _rescue_step(
    acc: Tuple[Mapping[str, str], Tuple[Tuple[str, str], ...]], e: Mapping
) -> Tuple[Mapping[str, str], Tuple[Tuple[str, str], ...]]:
    last, found = acc
    ref = e.get("ref")
    if not isinstance(ref, str) or not ref.startswith("refs/heads/"):
        return acc
    deleting = e.get("event") == "ref-update" and is_zero(e.get("new"))
    tip = _oid(e.get("old")) or last.get(ref)
    new = _oid(e.get("new"))
    # A deletion keeps the last tip: git's ref packing deletes each loose ref
    # it packs, and a later `git branch -D` logs only an all-zero old value.
    return (
        {**last, ref: new} if new else last,
        found + ((ref[len("refs/heads/") :], tip),) if deleting and tip else found,
    )


def rescue_candidates(events: Iterable[Mapping]) -> Tuple[Tuple[str, str], ...]:
    """`(branch, old tip)` for each `refs/heads/*` deletion the hooks logged.

    git passes an all-zero old value for a deletion it doesn't check, which
    git 2.56 does even for `git branch -d`, so the tip is the deletion's old
    value when it has one, else the last value the log recorded for the ref.
    """
    empty: Tuple[Mapping[str, str], Tuple[Tuple[str, str], ...]] = ({}, ())
    deleted = functools.reduce(_rescue_step, events, empty)[1]
    return tuple((branch, old) for old, branch in dict((o, b) for b, o in deleted).items())


def worktree_snapshot(f: WorktreeFacts) -> Dict:
    return {
        "event": "snapshot",
        "worktree": f.path,
        "ref": "refs/heads/" + f.branch if f.branch else None,
        "new": f.head,
        "lock": f.lock.lock.reason if f.lock else None,
    }


def branch_snapshot(v: BranchVerdict) -> Dict:
    return {
        "event": "snapshot",
        "ref": "refs/heads/" + v.facts.name,
        "new": v.facts.tip,
        "upstream": v.facts.upstream,
        "verdict": v.state,
    }


# The plan ----------------------------------------------------------------------------------

NOT_LANDED = frozenset(
    {"preview", "content-landed", "upstream-gone", "diverged", "in-review", "local-only"}
)


class Plan(NamedTuple):
    repo: RepoFacts
    worktrees: Tuple[WorktreeVerdict, ...]
    branches: Tuple[BranchVerdict, ...]
    backfill: bool
    snapshots: Tuple[Dict, ...]
    archives: Mapping[str, str]  # branch -> the archive ref written for it before anything goes
    backfill_archives: Tuple[str, ...]  # the branches among `archives` the backfill archives
    archive_prunes: Tuple[Tuple[str, str], ...]
    rescues: Tuple[Tuple[str, str, str], ...]  # (branch, old tip, archive ref)


def plan(repo: RepoFacts, now: float) -> Plan:
    """Everything the sweep does in one repository, in its order, and why it keeps the rest."""
    known = repo.default_problem is None
    states = {b.name: branch_state(b, known) for b in repo.branches}
    worktrees = tuple(
        worktree_verdict(
            f,
            states.get(f.branch) if f.branch else None,
            repo.default_ref,
            repo.default_problem,
            repo.settings,
        )
        for f in repo.worktrees
    )
    held = {v.facts.branch: v for v in worktrees if v.facts.branch and v.facts.exists}
    # A branch the sweep may delete is no place for another's commits to survive.
    going = frozenset(
        "refs/heads/" + b.name for b in repo.branches if states[b.name] in BRANCH_ACTIONS
    )

    def survives(tip: str, own: Optional[str], merged: bool) -> Tuple[str, ...]:
        return elsewhere(tip, own, repo.refs, merged, repo.default_ref, going)

    branches = tuple(
        branch_verdict(
            b,
            known,
            repo.default_ref,
            held[b.name].facts.path if b.name in held else None,
            held[b.name].action if b.name in held else None,
            survives(b.tip, "refs/heads/" + b.name, bool(b.landed)),
        )
        for b in repo.branches
    )
    worktrees = tuple(
        v._replace(
            survives=survives(
                v.facts.head,
                "refs/heads/" + v.facts.branch if v.facts.branch else None,
                bool(v.facts.landed),
            )
        )
        for v in worktrees
    )
    backfill = known and not has_snapshot(repo.events)
    logged = logged_refs(repo.events)
    backfill_names = tuple(
        v.facts.name
        for v in branches
        if backfill and v.state in NOT_LANDED and not v.facts.archived
    )
    content_names = tuple(
        v.facts.name
        for v in branches
        if v.action == "archive+delete" and v.facts.name not in backfill_names
    )
    rescue_names = tuple(branch for branch, _ in repo.rescues)
    refs = unique_refs(
        tuple(archive_ref(n, now) for n in backfill_names + content_names + rescue_names),
        frozenset(ref for ref, _ in repo.archives),
    )
    archive_count = len(backfill_names) + len(content_names)
    return Plan(
        repo=repo,
        worktrees=worktrees,
        branches=branches,
        backfill=backfill,
        snapshots=(
            tuple(worktree_snapshot(v.facts) for v in worktrees if backfill and v.facts.exists)
            + tuple(
                branch_snapshot(v)
                for v in branches
                if backfill or "refs/heads/" + v.facts.name not in logged
            )
        )
        if known
        else (),
        archives=dict(zip(backfill_names + content_names, refs[:archive_count])),
        backfill_archives=backfill_names,
        archive_prunes=tuple(
            (ref, obj) for ref, obj in repo.archives if known and ref in repo.merged
        ),
        rescues=tuple(
            (branch, old, ref) for (branch, old), ref in zip(repo.rescues, refs[archive_count:])
        ),
    )


# Totals ------------------------------------------------------------------------------------


def count(values: Iterable[str]) -> Dict[str, int]:
    listed = tuple(values)
    return {v: listed.count(v) for v in sorted(frozenset(listed))}


def sum_counts(maps: Iterable[Mapping[str, int]]) -> Dict[str, int]:
    every = tuple(maps)
    keys = sorted(frozenset(k for m in every for k in m))
    return {k: sum(m.get(k, 0) for m in every) for k in keys}
