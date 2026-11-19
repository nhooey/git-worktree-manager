//! The process environment as a value, so the code that reads it is pure
//! and its tests set it without touching the process's own.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Variables by name. Names and values that aren't UTF-8 are read lossily.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment(pub BTreeMap<String, String>);

impl Environment {
    pub fn from_pairs<K: Into<String>, V: Into<String>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        Self(
            pairs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }

    /// A variable's value; an empty one counts as unset, as the shell's
    /// `${VAR:-default}` reads it.
    pub fn var(&self, name: &str) -> Option<&str> {
        self.0
            .get(name)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    pub fn home(&self) -> Option<PathBuf> {
        self.var("HOME").map(PathBuf::from)
    }

    /// `${name:-$HOME/<fallback>}`.
    pub fn dir_or_home(&self, name: &str, fallback: &str) -> Option<PathBuf> {
        self.var(name)
            .map(PathBuf::from)
            .or_else(|| self.home().map(|home| home.join(fallback)))
    }
}
