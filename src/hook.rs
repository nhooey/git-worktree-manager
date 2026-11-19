//! git's `reference-transaction` hook: the log's lines from git's input,
//! built without opening the repository.

mod shim;

pub use shim::shim;

use jiff::Timestamp;

use crate::model::Oid;

/// The hook's name in git, and its subcommand's under `hook`.
pub const HOOK: &str = "reference-transaction";

/// A write to the log, and so each line in it, its newline included, is
/// shorter than this, so no other process's write splits it.
pub const LINE_LIMIT: usize = 4096;

/// What ends a ref name cut to fit [`LINE_LIMIT`]. A ref name can't hold
/// `..`, so a cut name is never a real one. The log's
/// [`crate::model::event::Event`] has no `ref_bytes`, so the rescue must
/// itself reject a name ending in the mark.
pub const CUT_MARK: &str = "...";

/// The one state in which git's refs have changed.
pub const COMMITTED: &str = "committed";

/// What one committed transaction makes the hook do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hooked {
    /// One JSON line per ref, each ending in a newline and shorter than
    /// [`LINE_LIMIT`], to append to the log.
    pub lines: Vec<String>,
    /// The default branch's ref moved, so `sweep --unattended` starts.
    pub sweep: bool,
    /// Input lines that aren't `<old> <new> <ref>`, which nothing is
    /// logged for.
    pub malformed: usize,
}

/// A ref's change, as git gives it.
#[derive(Clone, Copy)]
struct Change<'a> {
    old: Oid,
    new: Oid,
    reference: &'a str,
}

/// A line of git's input.
enum Input<'a> {
    Change(Change<'a>),
    /// A symbolic ref's change with no object name on either side. A
    /// deletion git gives no old value for, `<null> <null> <ref>`, is a
    /// change, which the rescue needs.
    Symbolic,
    Malformed,
}

/// One side of a change: an object name, or a symbolic ref's
/// `ref:<target>`, which the log holds as the null object name, so a line
/// with `ref:` on one side keeps the other side's object name.
fn side(word: &str) -> Option<Oid> {
    match word.starts_with("ref:") {
        true => Some(gix::ObjectId::null(gix::hash::Kind::Sha1).into()),
        false => Oid::from_hex(word),
    }
}

fn parse(line: &str) -> Input<'_> {
    let mut words = line.splitn(3, ' ');
    match (words.next(), words.next(), words.next()) {
        (Some(old), Some(new), Some(reference)) if !reference.is_empty() => {
            let symbolic = old.starts_with("ref:") || new.starts_with("ref:");
            match (side(old), side(new)) {
                (Some(old), Some(new)) if symbolic && old.is_null() && new.is_null() => {
                    Input::Symbolic
                }
                (Some(old), Some(new)) => Input::Change(Change {
                    old,
                    new,
                    reference,
                }),
                _ => Input::Malformed,
            }
        }
        _ => Input::Malformed,
    }
}

/// `text` as a JSON string, escaped as serde_json escapes it.
fn json_string(text: &str) -> String {
    let body = text
        .chars()
        .fold(String::with_capacity(text.len() + 2), |mut out, c| {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if c < ' ' => out.push_str(&format!("\\u{:04x}", u32::from(c))),
                c => out.push(c),
            }
            out
        });
    format!("\"{body}\"")
}

/// The change's line, naming `reference` in place of its ref: the log's
/// [`crate::model::event::Event`] as serde writes it, written by hand for
/// speed, then `ref_bytes`, the ref name's whole length, only on a line
/// whose ref name was cut to fit [`LINE_LIMIT`].
fn render(
    change: Change,
    reference: &str,
    ref_bytes: Option<usize>,
    time: Timestamp,
    pid: u32,
) -> String {
    let ref_bytes = ref_bytes.map_or(String::new(), |bytes| format!(",\"ref_bytes\":{bytes}"));
    format!(
        "{{\"time\":\"{time}\",\"pid\":{pid},\"event\":\"ref\",\"ref\":{},\"old\":\"{}\",\"new\":\"{}\"{ref_bytes}}}\n",
        json_string(reference),
        change.old,
        change.new,
    )
}

/// The bytes `c` takes inside a JSON string.
fn escaped_len(c: char) -> usize {
    json_string(c.encode_utf8(&mut [0; 4])).len() - 2
}

/// The longest start of `reference` whose escaped bytes fit in `budget`.
fn prefix(reference: &str, budget: usize) -> &str {
    let (_, end) = reference
        .chars()
        .try_fold((0, 0), |(used, end), c| {
            let used = used + escaped_len(c);
            match used <= budget {
                true => Ok((used, end + c.len_utf8())),
                false => Err((used, end)),
            }
        })
        .unwrap_or_else(|stopped| stopped);
    &reference[..end]
}

/// The change's log line. A ref name too long for [`LINE_LIMIT`] is cut,
/// ends in [`CUT_MARK`], and the line gains `ref_bytes`.
fn line(change: Change, time: Timestamp, pid: u32) -> String {
    let whole = render(change, change.reference, None, time, pid);
    if whole.len() < LINE_LIMIT {
        return whole;
    }
    let bytes = Some(change.reference.len());
    let frame = render(change, CUT_MARK, bytes, time, pid).len();
    let budget = (LINE_LIMIT - 1).saturating_sub(frame);
    let cut = format!("{}{CUT_MARK}", prefix(change.reference, budget));
    render(change, &cut, bytes, time, pid)
}

/// The hook's lines from `input`, git's `<old> <new> <ref>` lines on
/// standard input, with `default_ref` the default branch's ref. Blank
/// lines and symbolic refs' changes are passed over.
pub fn lines(input: &str, default_ref: Option<&str>, time: Timestamp, pid: u32) -> Hooked {
    let parsed: Vec<Input> = input
        .lines()
        .filter(|text| !text.is_empty())
        .map(parse)
        .collect();
    let changes: Vec<Change> = parsed
        .iter()
        .filter_map(|input| match input {
            Input::Change(change) => Some(*change),
            Input::Symbolic | Input::Malformed => None,
        })
        .collect();
    Hooked {
        lines: changes
            .iter()
            .map(|change| line(*change, time, pid))
            .collect(),
        sweep: default_ref
            .is_some_and(|default| changes.iter().any(|change| change.reference == default)),
        malformed: parsed
            .iter()
            .filter(|input| matches!(input, Input::Malformed))
            .count(),
    }
}

/// `lines` as the writes that append them: whole lines, in order, each
/// write shorter than [`LINE_LIMIT`], so a 1,000-ref fetch costs some 40
/// writes and not 1,000.
pub fn writes(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .fold(Vec::new(), |mut writes: Vec<String>, line| {
            match writes.last_mut() {
                Some(last) if last.len() + line.len() < LINE_LIMIT => last.push_str(line),
                _ => writes.push(line.clone()),
            }
            writes
        })
}

#[cfg(test)]
mod tests;
