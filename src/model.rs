//! The values the program passes between its parts, each serialisable:
//! the facts read from a repository, its settings, the plan made from them,
//! what carrying it out did, the report, and a line of the log.
//!
//! Every part reads and builds these; none but this module defines them.

pub mod event;
pub mod facts;
pub mod ids;
pub mod outcomes;
pub mod plan;
pub mod report;
pub mod settings;

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A git object's name, written as hex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Oid(pub gix::ObjectId);

impl Oid {
    /// The object name from its hex; None when it isn't one.
    pub fn from_hex(hex: &str) -> Option<Self> {
        gix::ObjectId::from_hex(hex.as_bytes()).ok().map(Self)
    }

    /// The all-zero name git gives a ref that doesn't exist.
    pub fn is_null(self) -> bool {
        self.0.is_null()
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<gix::ObjectId> for Oid {
    fn from(id: gix::ObjectId) -> Self {
        Self(id)
    }
}

impl Serialize for Oid {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Oid {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let hex = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Self::from_hex(&hex)
            .ok_or_else(|| serde::de::Error::custom(format!("not an object name: {hex:?}")))
    }
}

/// A coded fact about an item: why it is kept, what needs a person, or what
/// is wrong with a repository. `C` is the set of codes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note<C> {
    pub code: C,
    /// The same fact in words, with the values that decided it.
    pub detail: String,
}

impl<C> Note<C> {
    pub fn new(code: C, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}
