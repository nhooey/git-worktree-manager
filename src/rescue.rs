//! The rescue: branches deleted by hand, found in the log, the log's only
//! reader.

use crate::model::Oid;
use crate::model::event::Event;

/// A branch the log saw deleted, with the last tip it logged for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub branch: String,
    pub tip: Oid,
}

/// Every branch deleted by hand whose last logged tip the sweep should
/// check: it still exists, is reachable from no ref, and is missing from
/// the backup.
pub fn candidates(events: &[Event]) -> Vec<Candidate> {
    let _ = events;
    unimplemented!("rescue::candidates")
}
