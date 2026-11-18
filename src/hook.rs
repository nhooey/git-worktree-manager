//! git's `reference-transaction` hook: the log's lines from git's input,
//! built without opening the repository.

use jiff::Timestamp;

/// What one committed transaction makes the hook do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hooked {
    /// One JSON line per ref, each under 4 KB, to append to the log.
    pub lines: Vec<String>,
    /// The default branch's ref moved, so `sweep --unattended` starts.
    pub sweep: bool,
}

/// The hook's lines from `input`, git's `<old> <new> <ref>` lines on
/// standard input, with `default_ref` the default branch's ref.
pub fn lines(input: &str, default_ref: Option<&str>, time: Timestamp, pid: u32) -> Hooked {
    let _ = (input, default_ref, time, pid);
    unimplemented!("hook::lines")
}
