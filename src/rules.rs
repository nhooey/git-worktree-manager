//! The sweep's rules: every decision about a worktree or a branch, as pure
//! functions.
//!
//! The edge reads git and the operating system into the records below;
//! [`plan`] turns them into what the sweep does, and why it keeps the rest.
//! Nothing here reads a file or the environment or runs a command; only
//! [`parse_lstart`] reads the local time zone, as Python's `time.mktime`
//! does. Failures are values: a reason, never a panic.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeDelta, TimeZone, Utc};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::names;
use crate::python;

/// One line of the events log, as JSON read it.
pub type Event = serde_json::Map<String, Value>;

pub const DAY: f64 = 86400.0;

/// How far a process's start time may stray from the time a lock names.
const START_TOLERANCE: f64 = 2.0;

/// `{:.0f}`, which rounds a tie to even as Python's formatting does.
fn whole(value: f64) -> String {
    format!("{value:.0}")
}

/// What Python's `str.format` shows for an optional string.
fn or_none(value: Option<&str>) -> &str {
    value.unwrap_or("None")
}

/// Python's `a or b` on an optional string: an empty one counts as missing.
fn nonempty(value: Option<&str>) -> Option<&str> {
    value.filter(|v| !v.is_empty())
}

// Times and durations ---------------------------------------------------------

/// True for git's all-zero object name, which stands for a missing ref.
pub fn is_zero(oid: Option<&str>) -> bool {
    oid.is_some_and(|o| !o.is_empty() && o.bytes().all(|b| b == b'0'))
}

/// The length of the `\d+(\.\d+)?` that starts `text`; 0 when none does.
fn decimal_len(text: &str) -> usize {
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let whole = digits(text);
    let fraction = text[whole..]
        .strip_prefix('.')
        .map_or(0, |rest| match digits(rest) {
            0 => 0,
            n => n + 1,
        });
    if whole == 0 { 0 } else { whole + fraction }
}

/// Seconds from `90`, `90s`, `30m`, `1h`, `1d` or `2w`; None when unreadable.
pub fn parse_duration(text: Option<&str>) -> Option<f64> {
    let stripped = python::strip(text.unwrap_or(""));
    let length = decimal_len(stripped);
    let number: f64 = stripped[..length].parse().ok()?;
    let unit = match python::lstrip(&stripped[length..]) {
        "" | "s" => 1.0,
        "m" => 60.0,
        "h" => 3600.0,
        "d" => 86400.0,
        "w" => 604800.0,
        _ => return None,
    };
    Some(number * unit)
}

/// Which clock a `ps` start time is read on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Local,
    Utc,
}

/// One or two ASCII digits between `low` and `high`, as `strptime` reads a
/// day, hour, minute or second.
fn bounded(text: &str, low: u32, high: u32) -> Option<u32> {
    (1..=2)
        .contains(&text.len())
        .then(|| text.parse().ok())
        .flatten()
        .filter(|v| text.bytes().all(|b| b.is_ascii_digit()) && (low..=high).contains(v))
}

const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// The local time `mktime` gives a wall-clock time: the earlier of two in
/// the hour a clock goes back, and the offset before the gap for a time
/// skipped when it goes forward.
fn local_seconds(naive: NaiveDateTime) -> Option<i64> {
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|t| t.timestamp())
        .or_else(|| {
            Local
                .offset_from_local_datetime(&(naive - TimeDelta::hours(3)))
                .earliest()
                .map(|offset| (naive - offset).and_utc().timestamp())
        })
}

/// `ps -o lstart=` (`Sat Nov 14 16:15:55 2026`), on the given clock, to
/// epoch seconds, as `time.strptime` with `%a %b %d %H:%M:%S %Y` reads it.
pub fn parse_lstart(text: &str, zone: Zone) -> Option<f64> {
    let words: Vec<&str> = python::split_whitespace(text).collect();
    let [weekday, month, day, clock, year] = words.as_slice() else {
        return None;
    };
    WEEKDAYS.iter().find(|w| w.eq_ignore_ascii_case(weekday))?;
    let month = MONTHS.iter().position(|m| m.eq_ignore_ascii_case(month))?;
    let day = bounded(day, 1, 31)?;
    let [hour, minute, second] = clock.split(':').collect::<Vec<_>>()[..] else {
        return None;
    };
    let (hour, minute, second) = (
        bounded(hour, 0, 23)?,
        bounded(minute, 0, 59)?,
        bounded(second, 0, 61)?,
    );
    let year: i32 = (year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit()))
        .then(|| year.parse().ok())
        .flatten()
        .filter(|&y| y >= 1)?;
    // A leap second carries into the next minute, as `mktime` and `timegm`
    // carry it.
    let naive = NaiveDate::from_ymd_opt(year, u32::try_from(month).ok()? + 1, day)?
        .and_hms_opt(hour, minute, 0)?
        + TimeDelta::seconds(i64::from(second));
    let seconds = match zone {
        Zone::Utc => naive.and_utc().timestamp(),
        Zone::Local => local_seconds(naive)?,
    };
    Some(seconds as f64)
}

/// Epoch seconds from epoch seconds or milliseconds, ISO 8601, or `ps`'s
/// lstart in local time.
pub fn parse_time(text: Option<&str>) -> Option<f64> {
    let stripped = python::strip(text.unwrap_or(""));
    if !stripped.is_empty() && decimal_len(stripped) == stripped.len() {
        let number: f64 = stripped.parse().ok()?;
        return Some(if number > 1e11 {
            number / 1000.0
        } else {
            number
        });
    }
    let zoned = stripped
        .strip_suffix('Z')
        .map_or_else(|| stripped.to_owned(), |rest| format!("{rest}+00:00"));
    match python::fromisoformat(&zoned) {
        // A naive time is UTC, to the second, as `calendar.timegm` reads it.
        Some(parsed) => Some(match parsed.offset_micros {
            None => parsed.naive.and_utc().timestamp() as f64,
            Some(offset) => {
                let local = parsed.naive.and_utc();
                let micros = local.timestamp() * 1_000_000
                    + i64::from(local.timestamp_subsec_micros())
                    - offset;
                // Python divides the integer by 10**6 rounding once; parsing
                // the exact decimal rounds once too, where `as f64` would
                // round first past 2**53.
                let sign = if micros < 0 { "-" } else { "" };
                let magnitude = micros.unsigned_abs();
                format!(
                    "{sign}{}.{:06}",
                    magnitude / 1_000_000,
                    magnitude % 1_000_000
                )
                .parse()
                .ok()?
            }
        }),
        None if stripped.is_empty() => None,
        None => parse_lstart(stripped, Zone::Local),
    }
}

/// The UTC time at `seconds`, floored to the second as `time.gmtime` does.
fn utc(seconds: f64) -> DateTime<Utc> {
    DateTime::from_timestamp(seconds.floor() as i64, 0).unwrap_or(if seconds < 0.0 {
        DateTime::<Utc>::MIN_UTC
    } else {
        DateTime::<Utc>::MAX_UTC
    })
}

/// UTC, to the second: `2026-11-18T09:12:00Z`.
pub fn iso(seconds: f64) -> String {
    utc(seconds).format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// `refs/archive/<branch>/<UTC time>`; a ref name can't hold a colon.
pub fn archive_ref(branch: &str, seconds: f64) -> String {
    format!(
        "refs/archive/{branch}/{}",
        utc(seconds).format("%Y-%m-%dT%H%M%SZ")
    )
}

/// `wanted` with `-2`, `-3`… added to any name that is taken or repeated.
pub fn unique_refs<S: AsRef<str>>(wanted: &[S], existing: &BTreeSet<String>) -> Vec<String> {
    let free = |name: &str, taken: &BTreeSet<String>| {
        (1..)
            .map(|n| {
                if n == 1 {
                    name.to_owned()
                } else {
                    format!("{name}-{n}")
                }
            })
            .find(|candidate| !taken.contains(candidate))
            .expect("a finite set leaves a suffix free")
    };
    wanted
        .iter()
        .fold(
            (Vec::new(), existing.clone()),
            |(mut chosen, mut taken), name| {
                let name = free(name.as_ref(), &taken);
                taken.insert(name.clone());
                chosen.push(name);
                (chosen, taken)
            },
        )
        .0
}

// The local backup ----------------------------------------------------------------

/// `(owner, repository)` for the backup: the last two parts of `origin`'s
/// URL.
///
/// A URL with a scheme is split as `urllib.parse.urlsplit` splits it, an
/// scp-like one (`git@host:owner/repo.git`) at its colon, anything else is
/// a path. With no `origin`, or a URL without two usable parts, it is
/// `local/<folder>`.
pub fn backup_name(url: Option<&str>, folder: &str) -> (String, String) {
    let text = python::strip(url.unwrap_or(""));
    let path = if text.contains("://") {
        python::urlsplit_path(text)
    } else {
        match text.split_once(':') {
            Some((host, rest)) if !host.contains('/') => rest.to_owned(),
            _ => text.to_owned(),
        }
    };
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let (owner, name) = match parts.as_slice() {
        [.., owner, name] => (*owner, name.strip_suffix(".git").unwrap_or(name)),
        _ => ("", ""),
    };
    let usable = [owner, name].iter().all(|p| !["", ".", ".."].contains(p));
    if usable {
        (owner.to_owned(), name.to_owned())
    } else {
        ("local".to_owned(), folder.to_owned())
    }
}

// Settings --------------------------------------------------------------------

string_enum! {
    /// What a sweep a hook starts does.
    pub enum SweepMode {
        Report => "report",
        Apply => "apply",
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Settings {
    pub sweep: SweepMode,
    /// Seconds a worktree's git directory must be unchanged.
    pub idle: f64,
    /// Seconds a landed worktree may stay locked before the report names it.
    pub lock_report: f64,
}

/// A default from `defaults.json` as Python's `str` shows it.
fn default_text(defaults: &serde_json::Map<String, Value>, key: &str) -> String {
    match defaults.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// The settings: `worktrees.*` from git config over `defaults.json`'s
/// `settings`. `config` has git's lower-cased keys, such as
/// `worktrees.lockreport`.
pub fn settings(
    defaults: &serde_json::Map<String, Value>,
    config: &BTreeMap<String, String>,
) -> Settings {
    let pick = |key: &str| {
        config
            .get(&format!("{}.{}", names::CONFIG_SECTION, key.to_lowercase()))
            .cloned()
            .unwrap_or_else(|| default_text(defaults, key))
    };
    let duration = |key: &str, fallback: f64| {
        parse_duration(Some(&pick(key)))
            .or_else(|| parse_duration(Some(&default_text(defaults, key))))
            .unwrap_or(fallback)
    };
    Settings {
        sweep: if python::strip(&pick("sweep")).to_lowercase() == "apply" {
            SweepMode::Apply
        } else {
            SweepMode::Report
        },
        idle: duration("idle", 3600.0),
        lock_report: duration("lockReport", DAY),
    }
}

/// `git config --get-regexp` lines, `key value`, as a mapping; the last one
/// wins.
pub fn parse_config(text: &str) -> BTreeMap<String, String> {
    python::splitlines(text)
        .into_iter()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            (key.to_owned(), value.to_owned())
        })
        .collect()
}

// Locks -------------------------------------------------------------------------

// Python's `\s` also matches U+001C to U+001F, which Rust's leaves out.
const CLAUDE_LOCK: &str = r"^(?P<who>claude agent [^\s\x1C-\x1F]+) \(pid (?P<pid>[0-9]+) start (?P<start>[^)]*)\)[\s\x1C-\x1F]*$";
const SINCE_LOCK: &str =
    r"^(?P<who>.*?)[\s\x1C-\x1F]+since[\s\x1C-\x1F]+(?P<since>[^\s\x1C-\x1F].*?)[\s\x1C-\x1F]*$";
const PID_IN: &str = r"\bpid ([0-9]+)\b";

fn pattern(text: &str) -> Regex {
    Regex::new(text).expect("the lock patterns are valid")
}

string_enum! {
    /// How a lock reason was written.
    pub enum LockForm {
        ClaudeAgent => "claude-agent",
        Since => "since",
        Other => "other",
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Lock {
    pub reason: String,
    pub who: String,
    pub pid: Option<u64>,
    /// When the lock was taken, or when its process started.
    pub since: Option<f64>,
    pub form: LockForm,
}

/// A lock reason: `<who> since <time>`, or Claude Code's
/// `claude agent <id> (pid <pid> start <time>)`.
pub fn parse_lock(reason: &str) -> Lock {
    let pid_in = pattern(PID_IN)
        .captures(reason)
        .and_then(|c| c[1].parse().ok());
    if let Some(claude) = pattern(CLAUDE_LOCK).captures(reason) {
        Lock {
            reason: reason.to_owned(),
            who: claude["who"].to_owned(),
            pid: claude["pid"].parse().ok(),
            since: parse_time(Some(&claude["start"])),
            form: LockForm::ClaudeAgent,
        }
    } else if let Some(since) = pattern(SINCE_LOCK).captures(reason) {
        Lock {
            reason: reason.to_owned(),
            who: since["who"].to_owned(),
            pid: pid_in,
            since: parse_time(Some(&since["since"])),
            form: LockForm::Since,
        }
    } else {
        Lock {
            reason: reason.to_owned(),
            who: reason.to_owned(),
            pid: pid_in,
            since: None,
            form: LockForm::Other,
        }
    }
}

/// Whether the process a lock names still runs; None when it names none.
///
/// `process_start` is what `ps -o lstart= -p <pid>` gives, None when no
/// such process runs. Claude Code's form names the process's own start
/// time, which must match; the `since` form names when the lock was taken,
/// which the process must predate. A pid reused by a later process fails
/// either test.
pub fn process_alive(lock: &Lock, process_start: Option<f64>) -> Option<bool> {
    lock.pid?;
    let Some(start) = process_start else {
        return Some(false);
    };
    Some(match (lock.since, lock.form) {
        (None, _) => true,
        (Some(since), LockForm::ClaudeAgent) => (start - since).abs() <= START_TOLERANCE,
        (Some(since), _) => start <= since + START_TOLERANCE,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct LockFacts {
    pub lock: Lock,
    /// Seconds since `since`, or since the lock file was written.
    pub age: Option<f64>,
    pub alive: Option<bool>,
    pub process_start: Option<f64>,
}

pub fn lock_detail(facts: &LockFacts) -> String {
    let pid = facts
        .lock
        .pid
        .map_or_else(|| "None".to_owned(), |p| p.to_string());
    let alive = match facts.alive {
        None => String::new(),
        Some(true) => format!("; pid {pid} is alive"),
        Some(false) => format!("; pid {pid} is gone"),
    };
    let reason = Some(facts.lock.reason.as_str())
        .filter(|r| !r.is_empty())
        .unwrap_or("(no reason)");
    format!("locked: {reason}{alive}")
}

// Keep patterns -------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    pub text: String,
    /// Without a leading or trailing slash.
    pub body: String,
    pub dir_only: bool,
    /// Holds a slash, so it matches from the worktree's root.
    pub anchored: bool,
}

pub fn parse_pattern(text: &str) -> Pattern {
    let stripped = python::strip(text);
    let body = stripped.trim_matches('/');
    Pattern {
        text: text.to_owned(),
        body: body.to_owned(),
        dir_only: stripped.ends_with('/'),
        anchored: stripped.starts_with('/') || body.contains('/'),
    }
}

fn path_match(path: &str, body: &str) -> bool {
    python::fnmatchcase(path, body) && path.matches('/').count() == body.matches('/').count()
}

/// Whether a keep pattern holds `entry`, a path from `git ls-files`,
/// folders ending in `/`.
///
/// As in gitignore, a pattern without a slash matches a name at any depth,
/// one with a slash matches from the root, and a trailing slash matches
/// only folders. An entry inside a matching folder matches too.
pub fn pattern_matches(pattern: &Pattern, entry: &str) -> bool {
    let is_dir = entry.ends_with('/');
    let parts: Vec<&str> = entry.split('/').filter(|p| !p.is_empty()).collect();
    (0..parts.len()).any(|i| {
        let folder = i + 1 < parts.len() || is_dir;
        (!pattern.dir_only || folder)
            && if pattern.anchored {
                path_match(&parts[..=i].join("/"), &pattern.body)
            } else {
                python::fnmatchcase(parts[i], &pattern.body)
            }
    })
}

/// The ignored entries a keep pattern holds.
pub fn kept_entries<S: AsRef<str>>(entries: &[S], patterns: &[Pattern]) -> Vec<String> {
    entries
        .iter()
        .map(AsRef::as_ref)
        .filter(|e| patterns.iter().any(|p| pattern_matches(p, e)))
        .map(str::to_owned)
        .collect()
}

/// `items` without repeats, each where it first appears.
fn first_of_each(items: impl IntoIterator<Item = String>) -> Vec<String> {
    items.into_iter().fold(Vec::new(), |mut seen, item| {
        if !seen.contains(&item) {
            seen.push(item);
        }
        seen
    })
}

/// Anchored keep patterns that reach inside an ignored folder no pattern
/// holds.
///
/// `git ls-files --directory` stops at an ignored folder, so a pattern such
/// as `.claude/settings.local.json` under an ignored `.claude/` is looked
/// up at its own path rather than by walking the folder.
pub fn probes<S: AsRef<str>>(entries: &[S], patterns: &[Pattern]) -> Vec<String> {
    let folders: Vec<&str> = entries
        .iter()
        .map(AsRef::as_ref)
        .filter(|e| e.ends_with('/') && !patterns.iter().any(|p| pattern_matches(p, e)))
        .collect();
    first_of_each(
        patterns
            .iter()
            .filter(|p| p.anchored && folders.iter().any(|f| p.body.starts_with(f)))
            .map(|p| p.body.clone()),
    )
}

// Content landed ------------------------------------------------------------------

/// A path on the new side of a diff; a deletion has mode `000000`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub mode: String,
    pub object: String,
}

/// The changes `git diff-tree -r -z --no-renames` prints.
pub fn parse_raw_diff(text: &str) -> Vec<Change> {
    let fields: Vec<&str> = text.split('\0').collect();
    fields
        .iter()
        .zip(fields.iter().skip(1))
        .filter_map(|(meta, path)| {
            let parts: Vec<&str> = meta.strip_prefix(':')?.split(' ').collect();
            (parts.len() >= 5).then(|| Change {
                path: (*path).to_owned(),
                mode: parts[1].to_owned(),
                object: parts[3].to_owned(),
            })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub mode: String,
    pub object: String,
}

/// `git ls-tree -r -z` by path; None when any entry isn't
/// `<mode> <type> <object>\t<path>`, since a path lost to a bad entry would
/// read as deleted.
pub fn parse_ls_tree(text: &str) -> Option<BTreeMap<String, TreeEntry>> {
    text.split('\0')
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (meta, path) = entry.split_once('\t')?;
            let [mode, _, object] = meta.split(' ').collect::<Vec<_>>()[..] else {
                return None;
            };
            [mode, object].iter().all(|f| !f.is_empty()).then(|| {
                (
                    path.to_owned(),
                    TreeEntry {
                        mode: mode.to_owned(),
                        object: object.to_owned(),
                    },
                )
            })
        })
        .collect()
}

/// Whether the default branch's tree already holds every change, path by
/// path; never when its tree couldn't be read, so the branch is kept.
///
/// Then a merge of the branch into the default branch leaves the default
/// branch's tree as it is, which is what `git merge-tree --write-tree`
/// would show, without writing or freshening any object.
pub fn changes_present(changes: &[Change], tree: Option<&BTreeMap<String, TreeEntry>>) -> bool {
    tree.is_some_and(|tree| {
        changes.iter().all(|c| match tree.get(&c.path) {
            None => c.mode == "000000",
            Some(entry) => c.mode != "000000" && entry.mode == c.mode && entry.object == c.object,
        })
    })
}

// Facts -----------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct WorktreeFacts {
    pub path: String,
    pub branch: Option<String>,
    pub head: String,
    pub is_main: bool,
    pub exists: bool,
    pub lock: Option<LockFacts>,
    /// HEAD is an ancestor of origin/<default>; None without one.
    pub landed: Option<bool>,
    /// `git status --porcelain --untracked-files=all` lines.
    pub changes: Vec<String>,
    /// Ignored entries a keep pattern holds, unlike the main checkout's.
    pub kept: Vec<String>,
    /// Ignored entries no keep pattern holds.
    pub cleanable: Vec<String>,
    /// Seconds since HEAD, logs/HEAD or the index last changed.
    pub idle: Option<f64>,
    /// Why its folder isn't a working worktree of this repository.
    pub broken: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BranchFacts {
    pub name: String,
    pub tip: String,
    pub upstream: Option<String>,
    /// `%(upstream:track,nobracket)`: ``, `gone`, `ahead 1`, `ahead 1, behind 2`.
    pub track: String,
    pub landed: Option<bool>,
    pub content_landed: Option<bool>,
    pub age: Option<f64>,
    /// An archive ref in the local backup points at its tip.
    pub archived: bool,
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RepoFacts {
    pub main: String,
    pub common_dir: String,
    pub state_dir: String,
    pub default_branch: Option<String>,
    pub default_ref: Option<String>,
    pub default_problem: Option<String>,
    pub settings: Settings,
    pub worktrees: Vec<WorktreeFacts>,
    pub branches: Vec<BranchFacts>,
    /// `(ref, object)`.
    pub archives: Vec<(String, String)>,
    /// Refs whose tip is an ancestor of origin/<default>.
    pub merged: BTreeSet<String>,
    pub events: Vec<Event>,
    /// `(branch, old tip)`: deleted, unreachable, present.
    pub rescues: Vec<(String, String)>,
    /// Every ref and the commit it points at.
    pub refs: Vec<(String, String)>,
    /// The bare repository under ~/backups/git-archive that holds its archives.
    pub backup: String,
    /// `(ref, object)`: `refs/archive/*` there.
    pub backup_refs: Vec<(String, String)>,
}

string_enum! {
    /// Why the sweep keeps a worktree or a branch: a branch's state, or the
    /// worktree rule it fails.
    pub enum ReasonCode {
        DefaultBranch => "default-branch",
        NoDefaultBranch => "no-default-branch",
        Landed => "landed",
        Preview => "preview",
        ContentLanded => "content-landed",
        UpstreamGone => "upstream-gone",
        Diverged => "diverged",
        InReview => "in-review",
        LocalOnly => "local-only",
        CheckedOut => "checked-out",
        FolderMissing => "folder-missing",
        Broken => "broken",
        MainWorktree => "main-worktree",
        Locked => "locked",
        NotLanded => "not-landed",
        Uncommitted => "uncommitted",
        KeepPatternFiles => "keep-pattern-files",
        NotIdle => "not-idle",
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Reason {
    pub code: ReasonCode,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Test {
    pub name: TestName,
    pub ok: bool,
    pub detail: String,
}

// Branches --------------------------------------------------------------------------

string_enum! {
    /// The row of the branch table a branch is in.
    pub enum BranchState {
        DefaultBranch => "default-branch",
        NoDefaultBranch => "no-default-branch",
        Landed => "landed",
        Preview => "preview",
        ContentLanded => "content-landed",
        UpstreamGone => "upstream-gone",
        Diverged => "diverged",
        InReview => "in-review",
        LocalOnly => "local-only",
    }
}

/// The states whose work hasn't reached the default branch by ancestry.
pub const NOT_LANDED: &[BranchState] = &[
    BranchState::Preview,
    BranchState::ContentLanded,
    BranchState::UpstreamGone,
    BranchState::Diverged,
    BranchState::InReview,
    BranchState::LocalOnly,
];

string_enum! {
    pub enum BranchAction {
        Delete => "delete",
        ArchiveDelete => "archive+delete",
        Keep => "keep",
    }
}

impl BranchState {
    /// What the sweep wants for a branch in this state, before asking
    /// whether its tip survives elsewhere.
    pub fn wanted_action(self) -> BranchAction {
        match self {
            BranchState::Landed | BranchState::Preview => BranchAction::Delete,
            BranchState::ContentLanded => BranchAction::ArchiveDelete,
            _ => BranchAction::Keep,
        }
    }

    pub fn reason_code(self) -> ReasonCode {
        match self {
            BranchState::DefaultBranch => ReasonCode::DefaultBranch,
            BranchState::NoDefaultBranch => ReasonCode::NoDefaultBranch,
            BranchState::Landed => ReasonCode::Landed,
            BranchState::Preview => ReasonCode::Preview,
            BranchState::ContentLanded => ReasonCode::ContentLanded,
            BranchState::UpstreamGone => ReasonCode::UpstreamGone,
            BranchState::Diverged => ReasonCode::Diverged,
            BranchState::InReview => ReasonCode::InReview,
            BranchState::LocalOnly => ReasonCode::LocalOnly,
        }
    }
}

pub fn branch_state(b: &BranchFacts, default_known: bool) -> BranchState {
    let upstream = nonempty(b.upstream.as_deref()).is_some();
    if b.is_default {
        BranchState::DefaultBranch
    } else if !default_known {
        BranchState::NoDefaultBranch
    } else if b.landed == Some(true) {
        BranchState::Landed
    } else if b.name.starts_with("preview/") {
        BranchState::Preview
    } else if b.content_landed == Some(true) {
        BranchState::ContentLanded
    } else if upstream && b.track == "gone" {
        BranchState::UpstreamGone
    } else if upstream && b.track.contains("ahead") && b.track.contains("behind") {
        BranchState::Diverged
    } else if upstream {
        BranchState::InReview
    } else {
        BranchState::LocalOnly
    }
}

pub fn state_detail(b: &BranchFacts, state: BranchState, default_ref: Option<&str>) -> String {
    let upstream = or_none(b.upstream.as_deref());
    match state {
        BranchState::DefaultBranch => "the default branch".to_owned(),
        BranchState::NoDefaultBranch => "no default branch, so nothing is removed".to_owned(),
        BranchState::Landed => format!("its tip is an ancestor of {}", or_none(default_ref)),
        BranchState::Preview => "a preview branch".to_owned(),
        BranchState::ContentLanded => {
            format!("merging it into {} changes nothing", or_none(default_ref))
        }
        BranchState::UpstreamGone => format!(
            "its upstream {upstream} is gone: its pull request was closed or its branch deleted"
        ),
        BranchState::Diverged => format!(
            "it and {upstream} each have commits the other lacks ({})",
            b.track
        ),
        BranchState::InReview => format!("its upstream {upstream} exists"),
        BranchState::LocalOnly => format!(
            "no upstream{}: open a pull request, or archive it",
            b.age.map_or_else(String::new, |age| format!(
                ", {} days old",
                whole(age / DAY)
            ))
        ),
    }
}

pub fn short_ref(reference: Option<&str>) -> &str {
    let reference = reference.unwrap_or("");
    reference.strip_prefix("refs/remotes/").unwrap_or(reference)
}

/// The first `n` characters of `text`, as Python's `text[:n]`.
fn prefix(text: &str, n: usize) -> &str {
    text.char_indices()
        .nth(n)
        .map_or(text, |(at, _)| &text[..at])
}

/// Where a commit survives off this clone: merged into the default branch,
/// or pushed.
///
/// Only a remote-tracking ref counts: a local ref, a tag or an archive ref
/// in the same clone goes when the clone does.
pub fn elsewhere(
    tip: &str,
    refs: &[(String, String)],
    merged: bool,
    default_ref: Option<&str>,
) -> Vec<String> {
    let merged_into = merged.then(|| format!("merged into {}", short_ref(default_ref)));
    merged_into
        .into_iter()
        .chain(
            refs.iter()
                .filter(|(reference, commit)| {
                    commit == tip
                        && reference.starts_with("refs/remotes/")
                        && !reference.ends_with("/HEAD")
                })
                .map(|(reference, _)| {
                    format!(
                        "pushed as {} at {}",
                        short_ref(Some(reference)),
                        prefix(tip, 12)
                    )
                }),
        )
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct BranchVerdict {
    pub facts: BranchFacts,
    pub state: BranchState,
    pub action: BranchAction,
    pub reasons: Vec<Reason>,
    /// The worktree it is checked out in.
    pub worktree: Option<String>,
    /// Where its tip survives already, before any archive.
    pub survives: Vec<String>,
}

/// What the sweep does with a branch, once any worktree it is checked out
/// in is gone.
///
/// Nothing is deleted unless its tip survives off this clone: merged into
/// the default branch, or pushed. Otherwise it is archived to the local
/// backup first, and deleted only once the backup's copy compares equal,
/// or kept.
pub fn branch_verdict(
    b: &BranchFacts,
    default_known: bool,
    default_ref: Option<&str>,
    worktree: Option<&str>,
    worktree_action: Option<WorktreeAction>,
    survives: Vec<String>,
) -> BranchVerdict {
    let state = branch_state(b, default_known);
    let held = worktree.is_some() && worktree_action != Some(WorktreeAction::Remove);
    let wanted = state.wanted_action();
    let action = if held || wanted == BranchAction::Keep {
        BranchAction::Keep
    } else if b.landed == Some(true) || !survives.is_empty() {
        BranchAction::Delete
    } else {
        BranchAction::ArchiveDelete
    };
    let checked_out = held.then(|| Reason {
        code: ReasonCode::CheckedOut,
        detail: format!("checked out in {}", or_none(worktree)),
    });
    let kept_state = (wanted == BranchAction::Keep).then(|| Reason {
        code: state.reason_code(),
        detail: state_detail(b, state, default_ref),
    });
    BranchVerdict {
        facts: b.clone(),
        state,
        action,
        reasons: checked_out.into_iter().chain(kept_state).collect(),
        worktree: worktree.map(str::to_owned),
        survives,
    }
}

// Worktrees ---------------------------------------------------------------------------

string_enum! {
    /// A removal rule, in the rules' order.
    pub enum TestName {
        Folder => "folder",
        Working => "working",
        NotMain => "not-main",
        DefaultBranch => "default-branch",
        Unlocked => "unlocked",
        Landed => "landed",
        Clean => "clean",
        KeepFiles => "keep-files",
        Idle => "idle",
    }
}

impl TestName {
    /// The reason a worktree that fails this rule is kept.
    pub fn reason_code(self) -> ReasonCode {
        match self {
            TestName::Folder => ReasonCode::FolderMissing,
            TestName::Working => ReasonCode::Broken,
            TestName::NotMain => ReasonCode::MainWorktree,
            TestName::DefaultBranch => ReasonCode::NoDefaultBranch,
            TestName::Unlocked => ReasonCode::Locked,
            TestName::Landed => ReasonCode::NotLanded,
            TestName::Clean => ReasonCode::Uncommitted,
            TestName::KeepFiles => ReasonCode::KeepPatternFiles,
            TestName::Idle => ReasonCode::NotIdle,
        }
    }
}

string_enum! {
    pub enum WorktreeAction {
        Remove => "remove",
        Prune => "prune",
        Clean => "clean",
        Keep => "keep",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorktreeVerdict {
    pub facts: WorktreeFacts,
    pub action: WorktreeAction,
    pub tests: Vec<Test>,
    pub reasons: Vec<Reason>,
    /// Landed, and locked longer than `worktrees.lockReport`.
    pub lock_report: bool,
    /// Where its HEAD survives already, before any archive.
    pub survives: Vec<String>,
}

fn test(name: TestName, ok: bool, detail: impl Into<String>) -> Test {
    Test {
        name,
        ok,
        detail: detail.into(),
    }
}

pub fn landed_test(
    f: &WorktreeFacts,
    state: Option<BranchState>,
    default_ref: Option<&str>,
    default_problem: Option<&str>,
) -> Test {
    match (f.landed, state) {
        (None, _) => test(
            TestName::Landed,
            false,
            nonempty(default_problem).unwrap_or("no default branch"),
        ),
        (Some(true), _) => test(
            TestName::Landed,
            true,
            format!("HEAD is an ancestor of {}", or_none(default_ref)),
        ),
        (Some(false), Some(BranchState::ContentLanded)) => test(
            TestName::Landed,
            true,
            "its branch's content has landed; it is archived first",
        ),
        (Some(false), Some(BranchState::Preview)) => test(
            TestName::Landed,
            true,
            "a preview branch, which never lands; archived first unless it survives elsewhere",
        ),
        (Some(false), _) => test(
            TestName::Landed,
            false,
            format!(
                "HEAD is not an ancestor of {}{}",
                or_none(default_ref),
                state.map_or_else(String::new, |s| format!(" ({s})"))
            ),
        ),
    }
}

/// Python's `text[n:]`.
fn suffix(text: &str, n: usize) -> &str {
    text.char_indices().nth(n).map_or("", |(at, _)| &text[at..])
}

/// Every removal rule's result for one worktree, in the rules' order.
pub fn worktree_tests(
    f: &WorktreeFacts,
    state: Option<BranchState>,
    default_ref: Option<&str>,
    default_problem: Option<&str>,
    idle_needed: f64,
) -> Vec<Test> {
    let folder = (!f.exists).then(|| {
        test(
            TestName::Folder,
            false,
            format!("its folder {} is gone", f.path),
        )
    });
    let working = f.broken.as_ref().map(|broken| {
        test(
            TestName::Working,
            false,
            format!("not a working worktree: {broken}"),
        )
    });
    let uncommitted = f
        .changes
        .iter()
        .take(5)
        .map(|c| suffix(c, 3))
        .collect::<Vec<_>>()
        .join(", ");
    folder
        .into_iter()
        .chain(working)
        .chain([
            test(
                TestName::NotMain,
                !f.is_main,
                if f.is_main {
                    "the main worktree"
                } else {
                    "a linked worktree"
                },
            ),
            test(
                TestName::DefaultBranch,
                default_problem.is_none(),
                nonempty(default_problem).map_or_else(
                    || format!("{} is the default", or_none(default_ref)),
                    str::to_owned,
                ),
            ),
            test(
                TestName::Unlocked,
                f.lock.is_none(),
                f.lock
                    .as_ref()
                    .map_or_else(|| "not locked".to_owned(), lock_detail),
            ),
            landed_test(f, state, default_ref, default_problem),
            test(
                TestName::Clean,
                f.changes.is_empty(),
                if f.changes.is_empty() {
                    "nothing uncommitted".to_owned()
                } else {
                    format!("{} uncommitted paths: {uncommitted}", f.changes.len())
                },
            ),
            test(
                TestName::KeepFiles,
                f.kept.is_empty(),
                if f.kept.is_empty() {
                    "no kept files of its own".to_owned()
                } else {
                    format!(
                        "kept files unlike the main checkout's: {}",
                        f.kept.join(", ")
                    )
                },
            ),
            test(
                TestName::Idle,
                f.idle.is_some_and(|idle| idle >= idle_needed),
                format!(
                    "idle {} of the {} needed",
                    age(f.idle),
                    age(Some(idle_needed))
                ),
            ),
        ])
        .collect()
}

fn age(seconds: Option<f64>) -> String {
    match seconds {
        None => "?".to_owned(),
        Some(s) if s < 60.0 => format!("{}s", whole(s)),
        Some(s) if s < 3600.0 => format!("{}m", whole(s / 60.0)),
        Some(s) if s < DAY => format!("{}h", whole(s / 3600.0)),
        Some(s) => format!("{}d", whole(s / DAY)),
    }
}

/// What the sweep does with one worktree, with every test's result.
///
/// Removable when it isn't the main worktree, is unlocked, has landed, is
/// clean, holds no kept file of its own and has been idle. A missing
/// folder is pruned unless locked. An unlocked worktree that hasn't landed
/// and has been idle a day has its ignored files cleaned.
pub fn worktree_verdict(
    f: &WorktreeFacts,
    state: Option<BranchState>,
    default_ref: Option<&str>,
    default_problem: Option<&str>,
    s: &Settings,
) -> WorktreeVerdict {
    let tests = worktree_tests(f, state, default_ref, default_problem, s.idle);
    let failing: Vec<&Test> = tests.iter().filter(|t| !t.ok).collect();
    let landed = tests.iter().any(|t| t.name == TestName::Landed && t.ok);
    let tracked = f.changes.iter().any(|c| !c.starts_with("??"));
    let action = if !f.exists {
        if f.lock.is_none() && !f.is_main {
            WorktreeAction::Prune
        } else {
            WorktreeAction::Keep
        }
    } else if failing.is_empty() {
        WorktreeAction::Remove
    } else if !f.is_main
        && f.broken.is_none()
        && f.lock.is_none()
        && default_problem.is_none()
        && !landed
        && f.idle.is_some_and(|idle| idle >= DAY)
        && !tracked
        && !f.cleanable.is_empty()
    {
        WorktreeAction::Clean
    } else {
        WorktreeAction::Keep
    };
    let reasons = if matches!(action, WorktreeAction::Remove | WorktreeAction::Prune) {
        Vec::new()
    } else if f.is_main {
        vec![Reason {
            code: ReasonCode::MainWorktree,
            detail: "the main worktree".to_owned(),
        }]
    } else {
        failing
            .iter()
            // Without a default branch, landing is unknown rather than refused.
            .filter(|t| !(t.name == TestName::Landed && f.landed.is_none()))
            .map(|t| Reason {
                code: t.name.reason_code(),
                detail: t.detail.clone(),
            })
            .collect()
    };
    let lock_report = landed
        && f.lock
            .as_ref()
            .and_then(|lock| lock.age)
            .is_some_and(|lock_age| lock_age >= s.lock_report);
    WorktreeVerdict {
        facts: f.clone(),
        action,
        tests,
        reasons,
        lock_report,
        survives: Vec::new(),
    }
}

// The log ---------------------------------------------------------------------------

fn text_field<'a>(event: &'a Event, key: &str) -> Option<&'a str> {
    event.get(key).and_then(Value::as_str)
}

pub fn logged_refs(events: &[Event]) -> BTreeSet<String> {
    events
        .iter()
        .filter_map(|e| text_field(e, "ref"))
        .map(str::to_owned)
        .collect()
}

pub fn has_snapshot(events: &[Event]) -> bool {
    events
        .iter()
        .any(|e| text_field(e, "event") == Some("snapshot"))
}

/// An object name, unless it is missing, empty or all zeros.
fn oid<'a>(event: &'a Event, key: &str) -> Option<&'a str> {
    text_field(event, key).filter(|v| !v.is_empty() && !is_zero(Some(v)))
}

type RescueScan = (BTreeMap<String, String>, Vec<(String, String)>);

fn rescue_step((last, found): RescueScan, e: &Event) -> RescueScan {
    let Some(reference) = text_field(e, "ref").filter(|r| r.starts_with("refs/heads/")) else {
        return (last, found);
    };
    let deleting = text_field(e, "event") == Some("ref-update") && is_zero(text_field(e, "new"));
    let tip = oid(e, "old")
        .map(str::to_owned)
        .or_else(|| last.get(reference).cloned());
    // A deletion keeps the last tip: git's ref packing deletes each loose ref
    // it packs, and a later `git branch -D` logs only an all-zero old value.
    let last = match oid(e, "new") {
        Some(new) => {
            let mut next = last;
            next.insert(reference.to_owned(), new.to_owned());
            next
        }
        None => last,
    };
    let found = match tip {
        Some(tip) if deleting => found
            .into_iter()
            .chain([(reference["refs/heads/".len()..].to_owned(), tip)])
            .collect(),
        _ => found,
    };
    (last, found)
}

/// `(branch, old tip)` for each `refs/heads/*` deletion the hooks logged.
///
/// git passes an all-zero old value for a deletion it doesn't check, which
/// git 2.56 does even for `git branch -d`, so the tip is the deletion's old
/// value when it has one, else the last value the log recorded for the ref.
/// A tip deleted twice is rescued once, under the last branch that held it.
pub fn rescue_candidates(events: &[Event]) -> Vec<(String, String)> {
    let (_, deleted) = events
        .iter()
        .fold((BTreeMap::new(), Vec::new()), rescue_step);
    deleted.into_iter().fold(
        Vec::new(),
        |rescues: Vec<(String, String)>, (branch, tip)| match rescues
            .iter()
            .position(|(_, seen)| *seen == tip)
        {
            Some(at) => rescues
                .into_iter()
                .enumerate()
                .map(|(i, entry)| {
                    if i == at {
                        (branch.clone(), entry.1)
                    } else {
                        entry
                    }
                })
                .collect(),
            None => rescues.into_iter().chain([(branch, tip)]).collect(),
        },
    )
}

/// A worktree as the log's backfill records it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "event", rename = "snapshot")]
pub struct WorktreeSnapshot {
    pub worktree: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub new: String,
    pub lock: Option<String>,
}

/// A branch as the log records it, at the backfill or when first seen.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "event", rename = "snapshot")]
pub struct BranchSnapshot {
    #[serde(rename = "ref")]
    pub reference: String,
    pub new: String,
    pub upstream: Option<String>,
    pub verdict: BranchState,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Snapshot {
    Worktree(WorktreeSnapshot),
    Branch(BranchSnapshot),
}

impl Snapshot {
    pub fn reference(&self) -> Option<&str> {
        match self {
            Snapshot::Worktree(w) => w.reference.as_deref(),
            Snapshot::Branch(b) => Some(&b.reference),
        }
    }
}

pub fn worktree_snapshot(f: &WorktreeFacts) -> WorktreeSnapshot {
    WorktreeSnapshot {
        worktree: f.path.clone(),
        reference: nonempty(f.branch.as_deref()).map(|b| format!("refs/heads/{b}")),
        new: f.head.clone(),
        lock: f.lock.as_ref().map(|l| l.lock.reason.clone()),
    }
}

pub fn branch_snapshot(v: &BranchVerdict) -> BranchSnapshot {
    BranchSnapshot {
        reference: format!("refs/heads/{}", v.facts.name),
        new: v.facts.tip.clone(),
        upstream: v.facts.upstream.clone(),
        verdict: v.state,
    }
}

// The plan --------------------------------------------------------------------------------

/// A deleted branch's tip, written to the local backup under `archive`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rescue {
    pub branch: String,
    pub tip: String,
    pub archive: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan<'a> {
    pub repo: &'a RepoFacts,
    pub worktrees: Vec<WorktreeVerdict>,
    pub branches: Vec<BranchVerdict>,
    pub backfill: bool,
    pub snapshots: Vec<Snapshot>,
    /// `(branch, archive ref)`: the archive written for a branch before
    /// anything goes, backfill first.
    pub archives: Vec<(String, String)>,
    /// The branches among `archives` the backfill archives.
    pub backfill_archives: Vec<String>,
    pub archive_prunes: Vec<(String, String)>,
    pub rescues: Vec<Rescue>,
}

impl Plan<'_> {
    /// The archive ref the sweep writes for `branch`, if it writes one.
    pub fn archive_for(&self, branch: &str) -> Option<&str> {
        self.archives
            .iter()
            .find(|(name, _)| name == branch)
            .map(|(_, archive)| archive.as_str())
    }
}

/// Everything the sweep does in one repository, in its order, and why it
/// keeps the rest.
pub fn plan(repo: &RepoFacts, now: f64) -> Plan<'_> {
    let known = repo.default_problem.is_none();
    let default_ref = repo.default_ref.as_deref();
    let default_problem = repo.default_problem.as_deref();
    let states: BTreeMap<&str, BranchState> = repo
        .branches
        .iter()
        .map(|b| (b.name.as_str(), branch_state(b, known)))
        .collect();
    let survives = |tip: &str, merged: bool| elsewhere(tip, &repo.refs, merged, default_ref);
    let worktrees: Vec<WorktreeVerdict> = repo
        .worktrees
        .iter()
        .map(|f| {
            let state = nonempty(f.branch.as_deref()).and_then(|b| states.get(b).copied());
            let verdict = worktree_verdict(f, state, default_ref, default_problem, &repo.settings);
            WorktreeVerdict {
                survives: survives(&f.head, f.landed == Some(true)),
                ..verdict
            }
        })
        .collect();
    let held: BTreeMap<&str, &WorktreeVerdict> = worktrees
        .iter()
        .filter(|v| v.facts.exists)
        .filter_map(|v| nonempty(v.facts.branch.as_deref()).map(|b| (b, v)))
        .collect();
    let branches: Vec<BranchVerdict> = repo
        .branches
        .iter()
        .map(|b| {
            let holder = held.get(b.name.as_str());
            branch_verdict(
                b,
                known,
                default_ref,
                holder.map(|v| v.facts.path.as_str()),
                holder.map(|v| v.action),
                survives(&b.tip, b.landed == Some(true)),
            )
        })
        .collect();
    let backfill = known && !has_snapshot(&repo.events);
    let logged = logged_refs(&repo.events);
    let backfill_names: Vec<String> = branches
        .iter()
        .filter(|v| backfill && NOT_LANDED.contains(&v.state) && !v.facts.archived)
        .map(|v| v.facts.name.clone())
        .collect();
    let content_names: Vec<String> = branches
        .iter()
        .filter(|v| {
            v.action == BranchAction::ArchiveDelete && !backfill_names.contains(&v.facts.name)
        })
        .map(|v| v.facts.name.clone())
        .collect();
    let archived_names: Vec<&String> = backfill_names.iter().chain(&content_names).collect();
    let archive_refs: Vec<String> = archived_names
        .iter()
        .map(|name| name.as_str())
        .chain(repo.rescues.iter().map(|(branch, _)| branch.as_str()))
        .map(|name| archive_ref(name, now))
        .collect();
    let refs = unique_refs(
        &archive_refs,
        &repo
            .backup_refs
            .iter()
            .map(|(reference, _)| reference.clone())
            .collect(),
    );
    let (archive_names_refs, rescue_refs) = refs.split_at(archived_names.len());
    let snapshots: Vec<Snapshot> = if known {
        worktrees
            .iter()
            .filter(|v| backfill && v.facts.exists)
            .map(|v| Snapshot::Worktree(worktree_snapshot(&v.facts)))
            .chain(
                branches
                    .iter()
                    .filter(|v| {
                        backfill || !logged.contains(&format!("refs/heads/{}", v.facts.name))
                    })
                    .map(|v| Snapshot::Branch(branch_snapshot(v))),
            )
            .collect()
    } else {
        Vec::new()
    };
    let archives = archived_names
        .iter()
        .zip(archive_names_refs)
        .map(|(name, archive)| ((*name).clone(), archive.clone()))
        .collect();
    let archive_prunes = repo
        .archives
        .iter()
        .filter(|(reference, _)| known && repo.merged.contains(reference))
        .cloned()
        .collect();
    let rescues = repo
        .rescues
        .iter()
        .zip(rescue_refs)
        .map(|((branch, tip), archive)| Rescue {
            branch: branch.clone(),
            tip: tip.clone(),
            archive: archive.clone(),
        })
        .collect();
    Plan {
        worktrees,
        branches,
        backfill,
        snapshots,
        archives,
        backfill_archives: backfill_names,
        archive_prunes,
        rescues,
        repo,
    }
}

// Totals ----------------------------------------------------------------------------------

/// How often each value appears, by value.
pub fn count<S: AsRef<str>>(values: impl IntoIterator<Item = S>) -> BTreeMap<String, usize> {
    values
        .into_iter()
        .fold(BTreeMap::new(), |mut counts, value| {
            *counts.entry(value.as_ref().to_owned()).or_insert(0) += 1;
            counts
        })
}

/// The counts of every map, added key by key.
pub fn sum_counts(maps: &[BTreeMap<String, usize>]) -> BTreeMap<String, usize> {
    maps.iter()
        .flatten()
        .fold(BTreeMap::new(), |mut sums, (key, n)| {
            *sums.entry(key.clone()).or_insert(0) += n;
            sums
        })
}

#[cfg(test)]
mod tests;
