//! A line of a repository's log, `events.jsonl` in its state folder: JSON
//! Lines, for `jq`, appended one `write` at a time.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::model::Oid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// When it was written.
    pub time: Timestamp,
    /// The process that wrote it.
    pub pid: u32,
    #[serde(flatten)]
    pub kind: EventKind,
}

/// What happened, tagged `"event"` in the line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum EventKind {
    /// A ref git changed, as the reference-transaction hook saw it
    /// committed. A null `old` is a creation, a null `new` a deletion.
    Ref {
        #[serde(rename = "ref")]
        reference: String,
        old: Oid,
        new: Oid,
    },
    /// A branch's tip archived into the backup.
    Archived {
        branch: String,
        tip: Oid,
        /// The archive ref written.
        #[serde(rename = "ref")]
        reference: String,
        /// Why, when `finish --abandon --reason` gave one.
        reason: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_ref_line() {
        let event = Event {
            time: "2026-11-19T09:12:00Z".parse().expect("a time"),
            pid: 42,
            kind: EventKind::Ref {
                reference: "refs/heads/fix".to_owned(),
                old: Oid::from_hex(&"0".repeat(40)).expect("null"),
                new: Oid::from_hex(&"b".repeat(40)).expect("an object name"),
            },
        };
        let line = serde_json::to_string(&event).expect("serialisable");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).expect("JSON"),
            serde_json::json!({
                "time": "2026-11-19T09:12:00Z",
                "pid": 42,
                "event": "ref",
                "ref": "refs/heads/fix",
                "old": "0".repeat(40),
                "new": "b".repeat(40),
            })
        );
        assert_eq!(
            serde_json::from_str::<Event>(&line).expect("readable"),
            event
        );
    }
}
