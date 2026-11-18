//! A worktree's lock reason, and whether the process it names still runs.
//!
//! A lock is never broken: these only say who holds one and since when.

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::time::parse_time;

/// How far a process's start time may stray from the time a lock names.
pub const START_TOLERANCE: SignedDuration = SignedDuration::from_secs(2);

const CLAUDE_AGENT: &str =
    r"^(?<who>claude agent \S+) \(pid (?<pid>[0-9]+) start (?<start>[^)]*)\)\s*$";
const SINCE: &str = r"^(?<who>.*?)\s+since\s+(?<since>\S.*?)\s*$";
const PID: &str = r"\bpid ([0-9]+)\b";

fn pattern(text: &str) -> Regex {
    Regex::new(text).expect("the lock patterns are valid")
}

string_enum! {
    /// How a lock reason was written.
    pub enum LockForm {
        /// Claude Code's `claude agent <id> (pid <pid> start <time>)`.
        ClaudeAgent => "claude-agent",
        /// `<who> since <ISO time>`, with any `pid <pid>` in `<who>`.
        Since => "since",
        /// Anything else, or no reason.
        Other => "other",
    }
}

/// A lock reason, read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lock {
    /// The reason as `git worktree lock --reason` wrote it.
    pub reason: String,
    /// Who holds it: the reason before `since`, the agent, or the whole reason.
    pub who: String,
    /// The process the reason names, if any.
    pub pid: Option<u32>,
    /// When the lock was taken (`since`), or when its process started
    /// (`claude-agent`).
    pub since: Option<Timestamp>,
    pub form: LockForm,
}

/// Reads a lock reason; a time without an offset is read on `zone`'s clock
/// only in `ps`'s lstart form.
pub fn parse(reason: &str, zone: &TimeZone) -> Lock {
    let pid_in = || {
        pattern(PID)
            .captures(reason)
            .and_then(|found| found[1].parse().ok())
    };
    if let Some(found) = pattern(CLAUDE_AGENT).captures(reason) {
        Lock {
            reason: reason.to_owned(),
            who: found["who"].to_owned(),
            pid: found["pid"].parse().ok(),
            since: parse_time(&found["start"], zone),
            form: LockForm::ClaudeAgent,
        }
    } else if let Some(found) = pattern(SINCE).captures(reason) {
        Lock {
            reason: reason.to_owned(),
            who: found["who"].to_owned(),
            pid: pid_in(),
            since: parse_time(&found["since"], zone),
            form: LockForm::Since,
        }
    } else {
        Lock {
            reason: reason.to_owned(),
            who: reason.to_owned(),
            pid: pid_in(),
            since: None,
            form: LockForm::Other,
        }
    }
}

/// Whether the process a lock names still runs; None when it names none.
///
/// `process_start` is when the process with the lock's pid started, as
/// `ps -o lstart= -p <pid>` gives it, None when no such process runs.
/// Claude Code's form names the process's own start time, which must match
/// within [`START_TOLERANCE`]; the `since` form names when the lock was
/// taken, which the process must predate. A pid reused by a later process
/// fails either test.
pub fn alive(lock: &Lock, process_start: Option<Timestamp>) -> Option<bool> {
    lock.pid?;
    let Some(start) = process_start else {
        return Some(false);
    };
    Some(match (lock.since, lock.form) {
        (None, _) => true,
        (Some(since), LockForm::ClaudeAgent) => {
            start.duration_since(since).abs() <= START_TOLERANCE
        }
        (Some(since), _) => start <= since + START_TOLERANCE,
    })
}

#[cfg(test)]
mod tests;
