//! The rules: each a pure function over a repository's facts, and the
//! catalogue `rules` prints from the same data.

use jiff::Timestamp;

use crate::model::facts::RepositoryFacts;
use crate::model::plan::Plan;
use crate::model::settings::Settings;

/// What happens to every worktree and local branch of one repository, by
/// which rule, and where its commits survive. `status` and `sweep` both
/// print it.
pub fn plan(facts: &RepositoryFacts, settings: &Settings, now: Timestamp) -> Plan {
    let _ = (facts, settings, now);
    unimplemented!("rules::plan")
}
