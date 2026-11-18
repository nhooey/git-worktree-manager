//! The rescue's reads: the log's candidates, each kept only when its tip
//! still exists, is reachable from no ref, and no archive in the backup
//! that passes its check now holds it.

use std::collections::BTreeSet;
use std::path::Path;

use crate::backup;
use crate::model::Oid;
use crate::rescue::{self, Candidate};
use crate::system::{Failure, System, args};

/// Whether the commit `tip` is in the repository.
fn exists(system: &impl System, repository: &Path, tip: Oid) -> Result<bool, Failure> {
    let commit = format!("{tip}^{{commit}}");
    system
        .git(repository, &args(["cat-file", "-e", commit.as_str()]))
        .map(|done| done.success())
}

/// Whether no ref, and no worktree's `HEAD`, reaches `tip`: walking from
/// it, past everything they reach, still lists a commit. A walk that
/// fails says nothing, and the tip is taken as reached.
fn unreachable(system: &impl System, repository: &Path, tip: Oid) -> Result<bool, Failure> {
    let tip = tip.to_string();
    let done = system.git(
        repository,
        &args(["rev-list", "--max-count=1", tip.as_str(), "--not", "--all"]),
    )?;
    Ok(done.success() && !done.stdout.iter().all(u8::is_ascii_whitespace))
}

/// The branches the log at `log` saw deleted by hand that the rescue
/// archives: [`rescue::candidates`], less each whose tip is gone or a ref
/// reaches, and each whose tip an archive in the backup at `backup` holds
/// and passes its check now. An archive that fails it, as one whose
/// backup lost an object does, is no hold on the commit.
pub fn found(
    system: &impl System,
    log: &Path,
    repository: &Path,
    backup: &Path,
) -> Result<Vec<Candidate>, Failure> {
    let Some(bytes) = system.read_file(log)? else {
        return Ok(Vec::new());
    };
    let candidates: Vec<Candidate> = rescue::candidates(&rescue::events(&bytes))
        .into_iter()
        .map(|c| {
            let keep =
                exists(system, repository, c.tip)? && unreachable(system, repository, c.tip)?;
            Ok(keep.then_some(c))
        })
        .filter_map(Result::transpose)
        .collect::<Result<_, Failure>>()?;
    if candidates.is_empty() {
        return Ok(candidates);
    }
    let tips: BTreeSet<Oid> = candidates.iter().map(|c| c.tip).collect();
    let held: BTreeSet<Oid> = backup::read(system, backup, &tips)?
        .archives
        .iter()
        .filter(|a| a.compared == Some(true))
        .map(|a| a.oid)
        .collect();
    Ok(candidates
        .into_iter()
        .filter(|c| !held.contains(&c.tip))
        .collect())
}
