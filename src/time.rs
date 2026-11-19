//! Durations from settings, and the times lock reasons and `ps` give.

use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};

/// The length of the `[0-9]+(\.[0-9]+)?` that starts `text`; 0 when none
/// does.
fn decimal_len(text: &str) -> usize {
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    match digits(text) {
        0 => 0,
        whole => {
            whole
                + text[whole..]
                    .strip_prefix('.')
                    .map_or(0, |rest| match digits(rest) {
                        0 => 0,
                        n => n + 1,
                    })
        }
    }
}

/// `90`, `90s`, `30m`, `1h`, `1.5d` or `2w`; None when unreadable.
pub fn parse_duration(text: &str) -> Option<SignedDuration> {
    let text = text.trim();
    let length = decimal_len(text);
    let number: f64 = text[..length].parse().ok()?;
    let unit = match text[length..].trim_start() {
        "" | "s" => 1.0,
        "m" => 60.0,
        "h" => 3600.0,
        "d" => 86400.0,
        "w" => 604800.0,
        _ => return None,
    };
    SignedDuration::try_from_secs_f64(number * unit).ok()
}

/// `ps -o lstart=` (`Sat Nov 14 16:15:55 2026`) on the clock `zone`.
pub fn parse_lstart(text: &str, zone: &TimeZone) -> Option<Timestamp> {
    // `ps` pads a one-digit day with a second space.
    let words: Vec<&str> = text.split_whitespace().collect();
    let civil = jiff::fmt::strtime::parse("%a %b %d %H:%M:%S %Y", words.join(" "))
        .ok()?
        .to_datetime()
        .ok()?;
    civil
        .to_zoned(zone.clone())
        .ok()
        .map(|zoned| zoned.timestamp())
}

/// The time a lock reason names: epoch seconds or milliseconds, ISO 8601
/// (UTC when it has no offset), or `ps`'s lstart on the clock `zone`.
pub fn parse_time(text: &str, zone: &TimeZone) -> Option<Timestamp> {
    let text = text.trim();
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        let number: i64 = text.parse().ok()?;
        // Seconds since the epoch pass 10^11 in the year 5138.
        return if number > 100_000_000_000 {
            Timestamp::from_millisecond(number).ok()
        } else {
            Timestamp::from_second(number).ok()
        };
    }
    text.parse::<Timestamp>()
        .ok()
        .or_else(|| {
            text.parse::<DateTime>()
                .ok()
                .and_then(|civil| civil.to_zoned(TimeZone::UTC).ok())
                .map(|zoned| zoned.timestamp())
        })
        .or_else(|| parse_lstart(text, zone))
}

/// `time` to the second, as the report and archive names show it.
pub fn to_second(time: Timestamp) -> Timestamp {
    Timestamp::from_second(time.as_second()).unwrap_or(time)
}

#[cfg(test)]
mod tests;
