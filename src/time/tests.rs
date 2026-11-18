use jiff::tz::{Offset, TimeZone};
use jiff::{SignedDuration, Timestamp};

use super::*;

fn at(text: &str) -> Timestamp {
    text.parse().expect("a timestamp")
}

/// A fixed zone, so the tests don't depend on the machine's.
fn plus_seven() -> TimeZone {
    TimeZone::fixed(Offset::constant(7))
}

#[test]
fn test_durations() {
    let hours = |h: i64| Some(SignedDuration::from_hours(h));
    assert_eq!(parse_duration("1h"), hours(1));
    assert_eq!(parse_duration("1d"), hours(24));
    assert_eq!(parse_duration("2w"), hours(336));
    assert_eq!(parse_duration("1.5h"), Some(SignedDuration::from_mins(90)));
    assert_eq!(parse_duration("30m"), Some(SignedDuration::from_mins(30)));
    assert_eq!(parse_duration(" 0s "), Some(SignedDuration::ZERO));
    assert_eq!(parse_duration("90"), Some(SignedDuration::from_secs(90)));
    assert_eq!(parse_duration("90 s"), Some(SignedDuration::from_secs(90)));
    for unreadable in ["soon", "", "h", "1y", "-1h", ".5h", "1hh"] {
        assert_eq!(parse_duration(unreadable), None, "{unreadable:?}");
    }
}

#[test]
fn test_times() {
    let zone = plus_seven();
    let expected = Some(at("2026-11-18T09:12:00Z"));
    assert_eq!(parse_time("2026-11-18T09:12:00Z", &zone), expected);
    assert_eq!(parse_time("2026-11-18T16:12:00+07:00", &zone), expected);
    assert_eq!(parse_time("1794993120000", &zone), expected);
    assert_eq!(parse_time("1794993120", &zone), expected);
    assert_eq!(
        parse_time("2026-11-18T09:12:00", &zone),
        expected,
        "no offset is UTC"
    );
    assert_eq!(parse_time("Wed Nov 18 16:12:00 2026", &zone), expected);
    assert_eq!(
        parse_time("2026-11-18T09:12:00.5Z", &zone),
        Some(at("2026-11-18T09:12:00.5Z"))
    );
    assert_eq!(parse_time("whenever", &zone), None);
    assert_eq!(parse_time("", &zone), None);
}

#[test]
fn test_lstart_pads_a_one_digit_day() {
    assert_eq!(
        parse_lstart("Wed Nov  4 16:15:55 2026", &TimeZone::UTC),
        Some(at("2026-11-04T16:15:55Z"))
    );
    assert_eq!(parse_lstart("Nov 4 16:15:55 2026", &TimeZone::UTC), None);
}

#[test]
fn test_to_second() {
    assert_eq!(
        to_second(at("2026-11-18T09:12:00.9Z")),
        at("2026-11-18T09:12:00Z")
    );
}
