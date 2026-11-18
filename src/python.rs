//! Python's text, path and time semantics where the program's output depends
//! on them: the Rust port reads lock reasons, git's output and patterns as
//! the Python program did, so a caller sees the same results.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

/// `str.isspace`: Unicode's White_Space, and the ASCII information
/// separators U+001C to U+001F, which Python also counts.
pub(crate) fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `str.strip()`.
pub(crate) fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// `str.rstrip()`.
pub(crate) fn rstrip(text: &str) -> &str {
    text.trim_end_matches(is_space)
}

/// `str.lstrip()`.
pub(crate) fn lstrip(text: &str) -> &str {
    text.trim_start_matches(is_space)
}

/// `str.split()`, with no separator.
pub(crate) fn split_whitespace(text: &str) -> impl Iterator<Item = &str> {
    text.split(is_space).filter(|word| !word.is_empty())
}

fn is_line_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r'
            | '\u{0b}'
            | '\u{0c}'
            | '\u{1c}'
            | '\u{1d}'
            | '\u{1e}'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

fn split_first_line(text: &str) -> (&str, Option<&str>) {
    match text.char_indices().find(|&(_, c)| is_line_break(c)) {
        None => (text, None),
        Some((at, c)) => {
            let next = if text[at..].starts_with("\r\n") {
                at + 2
            } else {
                at + c.len_utf8()
            };
            (&text[..at], Some(&text[next..]))
        }
    }
}

/// `str.splitlines()`: every line break Python knows, and no empty last line.
pub(crate) fn splitlines(text: &str) -> Vec<&str> {
    let first = Some(text).filter(|t| !t.is_empty()).map(split_first_line);
    std::iter::successors(first, |&(_, rest)| {
        rest.filter(|r| !r.is_empty()).map(split_first_line)
    })
    .map(|(line, _)| line)
    .collect()
}

/// Python's `a // b` on floats, which rounds as `float_floor_div` does.
pub(crate) fn floor_div(a: f64, b: f64) -> f64 {
    let remainder = a % b;
    let quotient = (a - remainder) / b;
    let adjusted = if remainder != 0.0 && ((b < 0.0) != (remainder < 0.0)) {
        quotient - 1.0
    } else {
        quotient
    };
    let floor = adjusted.floor();
    if adjusted - floor > 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

// fnmatch ---------------------------------------------------------------------

enum Token {
    Star,
    Any,
    Char(char),
    Class {
        negated: bool,
        items: Vec<(char, char)>,
    },
}

impl Token {
    fn matches(&self, c: char) -> bool {
        match self {
            Token::Star | Token::Any => true,
            Token::Char(expected) => c == *expected,
            Token::Class { negated, items } => {
                items.iter().any(|&(low, high)| low <= c && c <= high) != *negated
            }
        }
    }
}

/// The items of a bracket expression's body: `a-z` is a range, a `-` that
/// starts or ends it is itself. A reversed range matches nothing.
fn class_items(body: &[char]) -> Vec<(char, char)> {
    match body {
        [] => Vec::new(),
        [low, '-', high, rest @ ..] => std::iter::once((*low, *high))
            .chain(class_items(rest))
            .collect(),
        [c, rest @ ..] => std::iter::once((*c, *c)).chain(class_items(rest)).collect(),
    }
}

/// The bracket expression opening at `pattern[0]`, a `[`, and what follows
/// it; None when no `]` closes it, so the `[` stands for itself.
fn class(pattern: &[char]) -> Option<(Token, &[char])> {
    let negated = pattern.get(1) == Some(&'!');
    let start = 1 + usize::from(negated);
    let first_close = start + usize::from(pattern.get(start) == Some(&']'));
    let close = first_close + pattern.get(first_close..)?.iter().position(|&c| c == ']')?;
    Some((
        Token::Class {
            negated,
            items: class_items(&pattern[start..close]),
        },
        &pattern[close + 1..],
    ))
}

fn tokens(pattern: &[char]) -> Vec<Token> {
    let (token, rest) = match pattern {
        [] => return Vec::new(),
        ['*', rest @ ..] => (Token::Star, rest),
        ['?', rest @ ..] => (Token::Any, rest),
        ['[', rest @ ..] => class(pattern).unwrap_or((Token::Char('['), rest)),
        [c, rest @ ..] => (Token::Char(*c), rest),
    };
    let after = tokens(rest);
    // Consecutive stars are one star.
    if matches!(token, Token::Star) && matches!(after.first(), Some(Token::Star)) {
        after
    } else {
        std::iter::once(token).chain(after).collect()
    }
}

/// Whether `segment`, star-free, matches the start of `text`.
fn segment_at(segment: &[Token], text: &[char]) -> bool {
    segment.len() <= text.len() && segment.iter().zip(text).all(|(t, c)| t.matches(*c))
}

/// The text between stars is matched without backtracking, as CPython's
/// translation has done since bpo-40480: the part before the first star
/// anchors at the start, the part after the last at the end, and each part
/// between takes its leftmost place, since a later one leaves less room
/// for the rest. Time stays linear in the parts however many stars there
/// are.
fn match_tokens(tokens: &[Token], text: &[char]) -> bool {
    let segments: Vec<&[Token]> = tokens.split(|t| matches!(t, Token::Star)).collect();
    match segments.as_slice() {
        [] => text.is_empty(),
        [only] => only.len() == text.len() && segment_at(only, text),
        [first, middle @ .., last] => {
            let Some(end) = text.len().checked_sub(last.len()) else {
                return false;
            };
            first.len() <= end
                && segment_at(first, text)
                && segment_at(last, &text[end..])
                && middle
                    .iter()
                    .try_fold(first.len(), |from, segment| {
                        let latest = end.checked_sub(segment.len())?;
                        (from..=latest)
                            .find(|&at| segment_at(segment, &text[at..]))
                            .map(|at| at + segment.len())
                    })
                    .is_some()
        }
    }
}

/// `fnmatch.fnmatchcase`: `*` matches any run of characters, `/` and line
/// breaks among them, `?` any one, `[...]` and `[!...]` a set.
pub(crate) fn fnmatchcase(name: &str, pattern: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    match_tokens(&tokens(&pattern), &name)
}

// posixpath -------------------------------------------------------------------

/// `posixpath.join`: an absolute part restarts the path.
pub(crate) fn path_join(base: &str, parts: &[&str]) -> String {
    parts.iter().fold(base.to_owned(), |path, part| {
        if part.starts_with('/') {
            (*part).to_owned()
        } else if path.is_empty() || path.ends_with('/') {
            path + part
        } else {
            path + "/" + part
        }
    })
}

/// `posixpath.basename`.
pub(crate) fn basename(path: &str) -> &str {
    &path[path.rfind('/').map_or(0, |at| at + 1)..]
}

/// `posixpath.dirname`.
pub(crate) fn dirname(path: &str) -> &str {
    let head = &path[..path.rfind('/').map_or(0, |at| at + 1)];
    if head.chars().all(|c| c == '/') {
        head
    } else {
        head.trim_end_matches('/')
    }
}

// urllib.parse ------------------------------------------------------------------

fn is_scheme_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')
}

/// The path of `urllib.parse.urlsplit(url)`.
pub(crate) fn urlsplit_path(url: &str) -> String {
    let cleaned: String = url
        .trim_start_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let after_scheme = match cleaned.find(':') {
        Some(at) if at > 0 && cleaned[..at].chars().all(is_scheme_char) => &cleaned[at + 1..],
        _ => cleaned.as_str(),
    };
    let after_netloc = match after_scheme.strip_prefix("//") {
        Some(rest) => &rest[rest.find(['/', '?', '#']).unwrap_or(rest.len())..],
        None => after_scheme,
    };
    let without_fragment = after_netloc.split('#').next().unwrap_or("");
    without_fragment.split('?').next().unwrap_or("").to_owned()
}

// datetime.fromisoformat ------------------------------------------------------

/// A time `datetime.fromisoformat` read: naive, or with its UTC offset.
pub(crate) struct IsoDateTime {
    pub(crate) naive: NaiveDateTime,
    pub(crate) offset_micros: Option<i64>,
}

/// `text`, ASCII digits only, as a number; `get` has already fixed its length.
fn component(text: Option<&str>) -> Option<u32> {
    text.filter(|t| t.bytes().all(|b| b.is_ascii_digit()))?
        .parse()
        .ok()
}

/// Where the date ends and the one-character separator before the time
/// sits, as `_find_isoformat_datetime_separator` guesses it.
fn date_end(text: &[char]) -> Option<usize> {
    if text.len() == 7 {
        return Some(7);
    }
    let digit_at = |at: usize| text.get(at).is_some_and(char::is_ascii_digit);
    Some(match (text[4], text[5]) {
        ('-', 'W') if text.get(8) == Some(&'-') => match text.len() {
            9 => return None,
            n if n > 10 && digit_at(10) => 8,
            _ => 10,
        },
        ('-', 'W') => 8,
        ('-', _) => 10,
        ('W', _) => {
            let digits_end = 7 + text[7..].iter().take_while(|c| c.is_ascii_digit()).count();
            match digits_end {
                end if end < 9 => end,
                end if end % 2 == 0 => 7,
                _ => 8,
            }
        }
        _ => 8,
    })
}

/// `YYYY-MM-DD`, `YYYYMMDD`, `YYYY-Www[-D]` or `YYYYWww[D]`.
fn date(text: &str) -> Option<NaiveDate> {
    if ![7, 8, 10].contains(&text.len()) || !text.is_ascii() {
        return None;
    }
    let year = component(text.get(0..4))?;
    let dashed = text.get(4..5) == Some("-");
    let at = 4 + usize::from(dashed);
    let date = if text.get(at..at + 1) == Some("W") {
        let week = component(text.get(at + 1..at + 3))?;
        let day_at = at + 3;
        let day = if text.len() > day_at {
            if (text.get(day_at..day_at + 1) == Some("-")) != dashed {
                return None;
            }
            let at = day_at + usize::from(dashed);
            component(text.get(at..at + 1))?
        } else {
            1
        };
        let weekday = chrono::Weekday::try_from(u8::try_from(day.checked_sub(1)?).ok()?).ok()?;
        NaiveDate::from_isoywd_opt(i32::try_from(year).ok()?, week, weekday)?
    } else {
        let month = component(text.get(at..at + 2))?;
        let day_at = at + 2;
        if (text.get(day_at..day_at + 1) == Some("-")) != dashed {
            return None;
        }
        let at = day_at + usize::from(dashed);
        let day = component(text.get(at..at + 2))?;
        NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, day)?
    };
    // `datetime`'s years run from 1 to 9999.
    Some(date).filter(|d| (1..=9999).contains(&chrono::Datelike::year(d)))
}

// The time is read as CPython's `_datetimemodule.c` reads it, which is what
// runs, and is laxer than `_pydatetime`: a clock with a zone may end in one
// stray character, and the zone starts at the first `Z`, `+` or `-`.

/// The byte at `at`, and NUL past the end, as C reads a string.
fn byte_at(text: &[u8], at: usize) -> u8 {
    text.get(at).copied().unwrap_or(0)
}

/// `n` ASCII digits from `at`, as C's `parse_digits` reads them.
fn digits_at(text: &[u8], at: usize, n: usize) -> Option<u32> {
    (at..at + n).try_fold(0u32, |value, i| {
        let b = byte_at(text, i);
        b.is_ascii_digit().then(|| value * 10 + u32::from(b - b'0'))
    })
}

/// A clock's fraction from `at`: one to six digits read, the rest skipped.
fn fraction(text: &[u8], end: usize, at: usize, [h, m, s]: [u32; 3]) -> Option<([u32; 4], bool)> {
    let wanted = end.checked_sub(at).filter(|&n| n > 0)?.min(6);
    let micros = digits_at(text, at, wanted)? * 10u32.pow(6 - wanted as u32);
    let stop = (at + wanted..)
        .find(|&i| !byte_at(text, i).is_ascii_digit())
        .unwrap_or(at + wanted);
    Some(([h, m, s, micros], byte_at(text, stop) != 0))
}

/// `parse_hh_mm_ss_ff` from field `index` at `at`: `HH[:?MM[:?SS[{.,}f+]]]`
/// up to `end`, as hours, minutes, seconds and microseconds, and whether
/// anything follows. Reading at `end` sees the zone's first byte, as C does.
fn clock_from(
    text: &[u8],
    end: usize,
    at: usize,
    index: usize,
    colons: Option<bool>,
    fields: [u32; 3],
) -> Option<([u32; 4], bool)> {
    let value = digits_at(text, at, 2)?;
    let fields: [u32; 3] = std::array::from_fn(|i| if i == index { value } else { fields[i] });
    let c = byte_at(text, at + 2);
    let next = at + 3;
    let colons = colons.unwrap_or(c == b':');
    match c {
        b'.' | b',' if index == 2 => return fraction(text, end, next, fields),
        b'.' | b',' => return None,
        _ if next >= end => {
            let [h, m, s] = fields;
            return Some(([h, m, s, 0], c != 0));
        }
        _ => {}
    }
    match c {
        b':' if colons && index < 2 => clock_from(text, end, next, index + 1, Some(colons), fields),
        _ if colons => None,
        _ if index < 2 => clock_from(text, end, next - 1, index + 1, Some(colons), fields),
        _ => fraction(text, end, next - 1, fields),
    }
}

fn clock(text: &[u8], start: usize, end: usize) -> Option<([u32; 4], bool)> {
    clock_from(text, end, start, 0, None, [0; 3])
}

/// The time after the date's separator: its clock, whether it was `24:00`
/// and so the next day's midnight, and its zone.
fn time(text: &str) -> Option<(NaiveTime, bool, Option<i64>)> {
    let bytes = text.as_bytes();
    let zone_at = bytes
        .iter()
        .position(|b| matches!(b, b'Z' | b'+' | b'-'))
        .unwrap_or(bytes.len());
    let ([hour, minute, second, micros], trailing) = clock(bytes, 0, zone_at)?;
    let next_day = hour == 24;
    if next_day && [minute, second, micros] != [0, 0, 0] {
        return None;
    }
    let offset = match bytes.get(zone_at) {
        None if trailing => return None,
        None => None,
        Some(b'Z') if zone_at + 1 == bytes.len() => Some(0),
        Some(b'Z') => return None,
        Some(sign) => {
            let ([h, m, s, us], trailing) = clock(bytes, zone_at + 1, bytes.len())?;
            let size = i64::from((h * 60 + m) * 60 + s) * 1_000_000 + i64::from(us);
            // `timezone` takes any offset under a day, whatever its fields.
            if trailing || size >= 86_400 * 1_000_000 {
                return None;
            }
            Some(if *sign == b'-' { -size } else { size })
        }
    };
    let clock =
        NaiveTime::from_hms_micro_opt(if next_day { 0 } else { hour }, minute, second, micros)?;
    Some((clock, next_day, offset))
}

/// `datetime.datetime.fromisoformat` as Python 3.14 reads a string: a date
/// in any of [`date`]'s forms, then any one character and a [`time`].
pub(crate) fn fromisoformat(text: &str) -> Option<IsoDateTime> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 7 {
        return None;
    }
    let end = date_end(&chars)?.min(chars.len());
    let day = date(&chars[..end].iter().collect::<String>())?;
    // Anything after the date is a separator and a time: a separator alone fails.
    if chars.len() == end {
        return Some(IsoDateTime {
            naive: day.and_time(NaiveTime::MIN),
            offset_micros: None,
        });
    }
    let time_text: String = chars[end + 1..].iter().collect();
    let (clock, next_day, offset_micros) = time(&time_text)?;
    let day = if next_day {
        day.succ_opt()
            .filter(|d| chrono::Datelike::year(d) <= 9999)?
    } else {
        day
    };
    Some(IsoDateTime {
        naive: day.and_time(clock),
        offset_micros,
    })
}

// json.dumps ------------------------------------------------------------------

/// `repr(float)`: the shortest digits that read back as `x`, in positional
/// notation from 1e-4 up to 1e16 and in exponent notation outside it.
/// `json.dumps` writes the non-finite values as `NaN` and `Infinity`.
pub(crate) fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_owned();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    // Rust's `{:e}` gives the same shortest digits as Python's repr.
    let scientific = format!("{:e}", x.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let sign = if x.is_sign_negative() { "-" } else { "" };
    // Where the decimal point falls after the first `point` digits.
    let point = exponent + 1;
    let body = if !(-3..=16).contains(&point) {
        let (first, rest) = digits.split_at(1);
        let dot = if rest.is_empty() { "" } else { "." };
        let exponent_sign = if exponent < 0 { '-' } else { '+' };
        format!("{first}{dot}{rest}e{exponent_sign}{:02}", exponent.abs())
    } else if point <= 0 {
        format!("0.{}{digits}", "0".repeat(point.unsigned_abs() as usize))
    } else {
        let point = point as usize;
        if point >= digits.len() {
            format!("{digits}{}.0", "0".repeat(point - digits.len()))
        } else {
            format!("{}.{}", &digits[..point], &digits[point..])
        }
    };
    format!("{sign}{body}")
}

/// A JSON string as `json.dumps` writes it with `ensure_ascii`: every
/// character outside printable ASCII as `\uXXXX`, in surrogate pairs above
/// the Basic Multilingual Plane.
fn json_string(text: &str) -> String {
    let escaped: String = text
        .chars()
        .map(|c| match c {
            '"' => "\\\"".to_owned(),
            '\\' => "\\\\".to_owned(),
            '\n' => "\\n".to_owned(),
            '\r' => "\\r".to_owned(),
            '\t' => "\\t".to_owned(),
            '\u{08}' => "\\b".to_owned(),
            '\u{0c}' => "\\f".to_owned(),
            ' '..='~' => c.to_string(),
            _ => c
                .encode_utf16(&mut [0; 2])
                .iter()
                .map(|unit| format!("\\u{unit:04x}"))
                .collect(),
        })
        .collect();
    format!("\"{escaped}\"")
}

fn json_number(n: &serde_json::Number) -> String {
    match n.as_f64() {
        Some(x) if n.is_f64() => float_repr(x),
        _ => n.to_string(),
    }
}

/// A container's items, on one line, or one per line indented a level
/// deeper than the container.
fn json_container(
    open: char,
    close: char,
    items: &[String],
    indent: Option<usize>,
    level: usize,
) -> String {
    if items.is_empty() {
        return format!("{open}{close}");
    }
    match indent {
        None => format!("{open}{}{close}", items.join(",")),
        Some(width) => {
            let inner = format!("\n{}", " ".repeat(width * (level + 1)));
            let outer = format!("\n{}", " ".repeat(width * level));
            format!(
                "{open}{inner}{}{outer}{close}",
                items.join(&format!(",{inner}"))
            )
        }
    }
}

fn json_value(value: &serde_json::Value, indent: Option<usize>, level: usize) -> String {
    use serde_json::Value;
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => json_number(n),
        Value::String(s) => json_string(s),
        Value::Array(items) => {
            let written: Vec<String> = items
                .iter()
                .map(|item| json_value(item, indent, level + 1))
                .collect();
            json_container('[', ']', &written, indent, level)
        }
        Value::Object(map) => {
            let colon = if indent.is_some() { ": " } else { ":" };
            let sorted: std::collections::BTreeMap<&String, &Value> = map.iter().collect();
            let written: Vec<String> = sorted
                .into_iter()
                .map(|(key, item)| {
                    format!(
                        "{}{colon}{}",
                        json_string(key),
                        json_value(item, indent, level + 1)
                    )
                })
                .collect();
            json_container('{', '}', &written, indent, level)
        }
    }
}

/// `json.dumps(value, sort_keys=True, indent=indent)`, or with no indent,
/// `json.dumps(value, sort_keys=True, separators=(",", ":"))`.
pub(crate) fn json_dumps(value: &serde_json::Value, indent: Option<usize>) -> String {
    json_value(value, indent, 0)
}

#[cfg(test)]
mod tests;
