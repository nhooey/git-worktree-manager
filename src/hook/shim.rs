//! The hook git runs, in `<common dir>/hooks/`: three lines of `sh` that
//! `exec` the program with the log and the default branch's ref set.
//!
//! A failing hook aborts git's transaction in the `preparing` and
//! `prepared` states, so the program runs only in the committed state,
//! where its exit status is ignored, and the shim exits 0 in every other
//! state and when the program is gone, as after a Nix garbage collection.

use std::path::Path;

use super::COMMITTED;
use crate::model::ids::CommandName;
use crate::names::Variable;

/// `text` as one `sh` word, in single quotes.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The hook's text, which runs `program`, appends to `log`, and starts the
/// sweep when `default_ref` moves; an empty `default_ref` starts none.
pub fn shim(program: &Path, log: &Path, default_ref: &str) -> String {
    let program = quoted(&program.to_string_lossy());
    format!(
        "#!/bin/sh\n\
         [ \"$1\" = {COMMITTED} ] && [ -x {program} ] || exit 0\n\
         {}={} {}={} exec {program} {} \"$@\"\n",
        Variable::Log.name(),
        quoted(&log.to_string_lossy()),
        Variable::DefaultRef.name(),
        quoted(default_ref),
        CommandName::Hook.as_str(),
    )
}
