//! Commits: their time, whether the default branch holds them, and whether
//! it holds a branch's content.

use std::collections::{BTreeMap, BTreeSet};

use gix::ObjectId;
use gix::bstr::{BString, ByteSlice};
use gix::object::tree::EntryMode;
use gix::object::tree::diff::ChangeDetached;
use jiff::Timestamp;

use crate::landed::{self, Change, EntryKind, TreeEntry};
use crate::model::facts::CommitFacts;

/// What was read of each tip: its time and its merge base with the
/// default branch, or why it couldn't be read.
pub struct Ancestry {
    default: Option<ObjectId>,
    tips: BTreeMap<ObjectId, Result<Tip, String>>,
}

struct Tip {
    time: Timestamp,
    /// None without a default branch, or with no history in common.
    base: Option<ObjectId>,
}

impl Ancestry {
    /// Each of `tips` read once, its merge base found through one graph
    /// that every tip shares.
    pub fn read(
        repository: &gix::Repository,
        default: Option<ObjectId>,
        tips: &BTreeSet<ObjectId>,
    ) -> gix::Result<Self> {
        let cache = repository.commit_graph_if_enabled()?;
        let mut graph = repository.revision_graph(cache.as_ref());
        let tips = tips
            .iter()
            .map(|&oid| {
                let tip = (|| -> gix::Result<Tip> {
                    let seconds = repository.find_commit(oid)?.time()?.seconds;
                    Ok(Tip {
                        time: Timestamp::from_second(seconds).map_err(gix::Error::from_error)?,
                        base: default
                            .map(|default| {
                                repository.merge_base_with_graph(oid, default, &mut graph)
                            })
                            .transpose()?
                            .flatten()
                            .map(gix::Id::detach),
                    })
                })();
                (oid, tip.map_err(|error| format!("{oid}: {error}")))
            })
            .collect();
        Ok(Self { default, tips })
    }

    /// The commit `oid`, one of the tips read; it is merged when it is its
    /// own merge base with the default branch.
    pub fn commit(&self, oid: ObjectId) -> Result<CommitFacts, String> {
        let tip = self.tip(oid)?;
        Ok(CommitFacts {
            oid: oid.into(),
            time: tip.time,
            merged: self.default.map(|_| tip.base == Some(oid)),
        })
    }

    /// Whether the default branch's tree holds every change `tip` made
    /// since their merge base; None without a default branch, false when
    /// they share no history.
    pub fn content_landed(
        &self,
        repository: &gix::Repository,
        tip: ObjectId,
    ) -> Result<Option<bool>, String> {
        let base = self.tip(tip)?.base;
        self.default
            .map(|default| match base {
                Some(base) => content_landed(repository, base, tip, default)
                    .map_err(|error| format!("{tip}: {error}")),
                None => Ok(false),
            })
            .transpose()
    }

    fn tip(&self, oid: ObjectId) -> Result<&Tip, String> {
        match self.tips.get(&oid) {
            Some(tip) => tip.as_ref().map_err(String::clone),
            None => Err(format!("{oid} was not read")),
        }
    }
}

fn content_landed(
    repository: &gix::Repository,
    base: ObjectId,
    tip: ObjectId,
    default: ObjectId,
) -> gix::Result<bool> {
    let tree = |oid: ObjectId| repository.find_commit(oid).and_then(|c| c.tree());
    // Rewrite tracking is off, as `landed::content_landed` needs.
    let changes: Vec<Change> = repository
        .diff_tree_to_tree(
            &tree(base)?,
            &tree(tip)?,
            gix::diff::Options::default().with_rewrites(None),
        )?
        .iter()
        .filter_map(change)
        .collect();
    let default_tree = tree(default)?;
    let entries: Option<BTreeMap<BString, TreeEntry>> = changes
        .iter()
        .map(|change| {
            default_tree
                .lookup_entry(change.path.split_str("/"))
                .map(|found| {
                    found.map(|entry| {
                        (
                            change.path.clone(),
                            tree_entry(entry.mode(), entry.object_id()),
                        )
                    })
                })
        })
        .collect::<gix::Result<Vec<_>>>()
        .ok()
        .map(|found| found.into_iter().flatten().collect());
    Ok(landed::content_landed(&changes, entries.as_ref()))
}

/// What a change left at a path that holds no folder afterwards; None for
/// a folder, whose files are changes of their own.
fn change(change: &ChangeDetached) -> Option<Change> {
    let path = change.location().to_owned();
    let (mode, id) = change.entry_mode_and_id();
    match change {
        ChangeDetached::Deletion { .. } if !mode.is_tree() => Some(Change { path, new: None }),
        ChangeDetached::Deletion { .. } => None,
        // A file that became a folder is gone at its own path.
        ChangeDetached::Modification {
            previous_entry_mode,
            ..
        } if mode.is_tree() && !previous_entry_mode.is_tree() => Some(Change { path, new: None }),
        _ if mode.is_tree() => None,
        _ => Some(Change {
            path,
            new: Some(tree_entry(mode, id.to_owned())),
        }),
    }
}

fn tree_entry(mode: EntryMode, oid: ObjectId) -> TreeEntry {
    TreeEntry {
        kind: if mode.is_tree() {
            EntryKind::Tree
        } else if mode.is_commit() {
            EntryKind::Commit
        } else if mode.is_link() {
            EntryKind::Link
        } else if mode.is_executable() {
            EntryKind::BlobExecutable
        } else {
            EntryKind::Blob
        },
        oid: oid.into(),
    }
}
