//! The rescue: branches deleted by hand, found in the log, the log's only
//! reader.

use std::collections::BTreeMap;

use crate::hook::CUT_MARK;
use crate::model::Oid;
use crate::model::event::{Event, EventKind};

/// The namespace of the branches the rescue looks for.
const BRANCHES: &str = "refs/heads/";

/// A branch the log saw deleted, with the last tip it logged for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub branch: String,
    pub tip: Oid,
}

/// The log's events, from its bytes: one JSON line each. A line that
/// isn't one is passed over.
pub fn events(log: &[u8]) -> Vec<Event> {
    log.split(|&byte| byte == b'\n')
        .filter_map(|line| serde_json::from_slice(line).ok())
        .collect()
}

/// The branch a logged ref name is, when the rescue may act on it: a name
/// cut to fit the log's line limit, or one that held bytes that aren't
/// UTF-8, may stand for another branch, so it is never acted on.
fn branch(reference: &str) -> Option<&str> {
    reference
        .strip_prefix(BRANCHES)
        .filter(|name| !name.is_empty())
        .filter(|name| !name.ends_with(CUT_MARK) && !name.contains('\u{FFFD}'))
}

/// What the log last said of one branch: whether its last change deleted
/// it, and the last object name it gave for it.
#[derive(Clone, Copy, Default)]
struct Last {
    deleted: bool,
    tip: Option<Oid>,
}

/// Every branch deleted by hand whose last logged tip the sweep should
/// check: it still exists, is reachable from no ref, and is missing from
/// the backup. A change's `new` is read first: a null `new` deletes the
/// branch, and its `old`, when git gave one, is the tip it had. One
/// candidate per branch, by name.
pub fn candidates(events: &[Event]) -> Vec<Candidate> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::Ref {
                reference,
                old,
                new,
            } => branch(reference).map(|name| (name, *old, *new)),
            EventKind::Archived { .. } => None,
        })
        .fold(
            BTreeMap::<&str, Last>::new(),
            |mut last, (name, old, new)| {
                let before = last.get(name).copied().unwrap_or_default();
                let now = match new.is_null() {
                    false => Last {
                        deleted: false,
                        tip: Some(new),
                    },
                    true => Last {
                        deleted: true,
                        tip: Some(old).filter(|old| !old.is_null()).or(before.tip),
                    },
                };
                last.insert(name, now);
                last
            },
        )
        .into_iter()
        .filter(|(_, last)| last.deleted)
        .filter_map(|(name, last)| {
            last.tip.map(|tip| Candidate {
                branch: name.to_owned(),
                tip,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
