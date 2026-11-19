//! Local branches: each tip, upstream and how far apart they are, and
//! whether the default branch holds the branch's content.

use gix::ObjectId;
use gix::refs::{FullName, FullNameRef};
use gix::remote::Direction;

use super::commits::Ancestry;
use crate::model::facts::{BranchFacts, UpstreamFacts};
use crate::system::Failure;

/// Every local branch and the commit it points at. A symbolic ref under
/// `refs/heads/` is no branch of its own and is left out: deleting it as a
/// branch would delete the branch it names.
pub fn tips(repository: &gix::Repository) -> Result<Vec<(FullName, ObjectId)>, Failure> {
    let failed = super::failing(repository.git_dir());
    repository
        .references()
        .map_err(|e| failed(&e))?
        .local_branches()
        .map_err(|e| failed(&e))?
        .map(|reference| {
            let mut reference = reference.map_err(|e| failed(&e))?;
            if reference.target().try_id().is_none() {
                return Ok(None);
            }
            let name = reference.name().to_owned();
            let tip = reference
                .peel_to_commit()
                .map_err(|e| Failure::unreadable(format!("{}: {e}", name.as_bstr())))?
                .id;
            Ok(Some((name, tip)))
        })
        .filter_map(Result::transpose)
        .collect()
}

/// The facts of each branch in `tips`, whose commits `ancestry` has read.
pub fn read(
    repository: &gix::Repository,
    tips: &[(FullName, ObjectId)],
    ancestry: &Ancestry,
) -> Result<Vec<BranchFacts>, Failure> {
    tips.iter()
        .map(|(name, tip)| {
            let at =
                |e: &dyn std::fmt::Display| Failure::unreadable(format!("{}: {e}", name.as_bstr()));
            Ok(BranchFacts {
                name: name.shorten().to_string(),
                tip: ancestry.commit(*tip).map_err(|e| at(&e))?,
                upstream: upstream(repository, name.as_ref(), *tip).map_err(|e| at(&e))?,
                content_landed: ancestry
                    .content_landed(repository, *tip)
                    .map_err(|e| at(&e))?,
            })
        })
        .collect()
}

/// The branch's upstream, as git's `%(upstream)` names it: the tracking
/// ref of `branch.<name>.merge` through the remote's fetch refspecs, or
/// the local branch itself when the remote is `.`. None when no refspec
/// maps it, as git has none then either.
fn upstream(
    repository: &gix::Repository,
    branch: &FullNameRef,
    tip: ObjectId,
) -> gix::Result<Option<UpstreamFacts>> {
    let local = repository
        .branch_remote_name(branch.shorten(), Direction::Fetch)
        .is_some_and(|remote| remote.as_bstr() == ".");
    let tracking = if local {
        repository.branch_remote_ref_name(branch, Direction::Fetch)
    } else {
        repository.branch_remote_tracking_ref_name(branch, Direction::Fetch)
    };
    let Some(Ok(tracking)) = tracking else {
        return Ok(None);
    };
    let found = repository
        .try_find_reference(tracking.as_ref())?
        .map(|mut found| found.peel_to_commit().map(|commit| commit.id))
        .transpose()?;
    let count = |from: ObjectId, hidden: ObjectId| -> gix::Result<u64> {
        repository
            .rev_walk([from])
            .with_hidden([hidden])
            .all()?
            .try_fold(0, |count, info| info.map(|_| count + 1))
    };
    Ok(Some(UpstreamFacts {
        reference: tracking.as_bstr().to_string(),
        gone: found.is_none(),
        ahead: found.map(|up| count(tip, up)).transpose()?.unwrap_or(0),
        behind: found.map(|up| count(up, tip)).transpose()?.unwrap_or(0),
    }))
}
