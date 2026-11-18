use jiff::Timestamp;
use jiff::tz::TimeZone;

use super::*;

fn read(reason: &str) -> Lock {
    parse(reason, &TimeZone::UTC)
}

fn second(n: i64) -> Timestamp {
    Timestamp::from_second(n).expect("in range")
}

#[test]
fn test_since_form() {
    let lock = read("headless overnight2-03 pid 4411 since 2026-11-18T09:12:00Z");
    assert_eq!(lock.form, LockForm::Since);
    assert_eq!(lock.who, "headless overnight2-03 pid 4411");
    assert_eq!(lock.pid, Some(4411));
    assert_eq!(lock.since, Some(second(1794993120)));
    assert_eq!(read("neil since 2026-11-18T09:12:00Z").pid, None);
}

#[test]
fn test_claude_form() {
    let lock = read("claude agent agent-a1b2 (pid 93171 start 1794993120000)");
    assert_eq!(
        (lock.form, lock.pid, lock.since),
        (LockForm::ClaudeAgent, Some(93171), Some(second(1794993120)))
    );
    assert_eq!(lock.who, "claude agent agent-a1b2");
}

#[test]
fn test_other_form() {
    assert_eq!(read("busy").form, LockForm::Other);
    assert_eq!(read("busy").since, None);
    assert_eq!(read("").reason, "");
    assert_eq!(read("held by pid 12").pid, Some(12));
}

#[test]
fn test_alive() {
    let claude = read("claude agent agent-a (pid 1 start 1000)");
    assert_eq!(alive(&claude, Some(second(1001))), Some(true));
    assert_eq!(
        alive(&claude, Some(second(2000))),
        Some(false),
        "a pid reused by a later process"
    );
    assert_eq!(alive(&claude, None), Some(false));
    let since = read("w pid 1 since 1970-01-01T00:16:40Z");
    assert_eq!(alive(&since, Some(second(900))), Some(true));
    assert_eq!(alive(&since, Some(second(1100))), Some(false));
    assert_eq!(alive(&read("neil since 2026-11-18T09:12:00Z"), None), None);
}
