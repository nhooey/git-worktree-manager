//! Where a commit survives besides its branch or worktree, and whether
//! that allows a deletion.

use crate::model::facts::{CommitFacts, RepositoryFacts};
use crate::model::ids::SurvivalKind;
use crate::model::report::Survival;

/// Every place `commit` survives: merged into the default branch, pushed
/// to a remote-tracking ref, or archived in the backup by an archive that
/// passed the compare pass. A local ref, a tag or an archive ref inside
/// the clone goes with the clone, so none counts.
pub fn survives(commit: &CommitFacts, repository: &RepositoryFacts) -> Vec<Survival> {
    let merged = repository
        .default
        .as_ref()
        .filter(|_| commit.merged == Some(true))
        .map(|default| Survival {
            kind: SurvivalKind::Merged,
            reference: default.reference.clone(),
            oid: default.oid,
        });
    let pushed = repository
        .remote_refs
        .iter()
        .filter(|r| r.oid == commit.oid)
        .map(|r| Survival {
            kind: SurvivalKind::Pushed,
            reference: r.name.clone(),
            oid: r.oid,
        });
    let archived = repository
        .backup
        .archives
        .iter()
        .filter(|a| a.oid == commit.oid && a.compared == Some(true))
        .map(|a| Survival {
            kind: SurvivalKind::Archived,
            reference: a.reference.clone(),
            oid: a.oid,
        });
    merged.into_iter().chain(pushed).chain(archived).collect()
}

/// Whether a commit may lose its branch: it is merged or archived. Pushed
/// proves nothing, since the ref is only as new as the last fetch.
pub fn allows_deletion(survives: &[Survival]) -> bool {
    survives
        .iter()
        .any(|s| matches!(s.kind, SurvivalKind::Merged | SurvivalKind::Archived))
}
