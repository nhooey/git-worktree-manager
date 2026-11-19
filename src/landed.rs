//! Content landed: whether the default branch already holds every change a
//! branch made, path by path, so merging the branch would leave the
//! default branch's tree as it is. It reads trees and writes nothing.

use std::collections::BTreeMap;

use gix::bstr::BString;

use crate::model::Oid;

string_enum! {
    /// What a tree entry is, from its mode.
    pub enum EntryKind {
        Blob => "blob",
        BlobExecutable => "blob-executable",
        Link => "link",
        Tree => "tree",
        Commit => "commit",
    }
}

/// A tree entry's mode and object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub kind: EntryKind,
    pub oid: Oid,
}

/// A path a branch changed since its merge base with the default branch.
///
/// The changes must come from a diff with rename tracking off: a rename
/// is then a deletion at its source and an addition at its destination.
/// A rename reported only at its destination hides the source's deletion,
/// so a default branch that still holds the source would look landed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The path, from the tree's root.
    pub path: BString,
    /// What the branch left there; None when it deleted the path.
    pub new: Option<TreeEntry>,
}

/// Whether the default branch's tree holds every change: each changed
/// path has the mode and object the branch gave it, or is absent where the
/// branch deleted it. `tree` holds the default branch's entries at least
/// at every changed path; None when it couldn't be read, which keeps the
/// branch, since a path lost to a bad read would look deleted.
pub fn content_landed(changes: &[Change], tree: Option<&BTreeMap<BString, TreeEntry>>) -> bool {
    tree.is_some_and(|tree| {
        changes
            .iter()
            .all(|change| tree.get(&change.path) == change.new.as_ref())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: EntryKind, hex_digit: char) -> TreeEntry {
        TreeEntry {
            kind,
            oid: Oid::from_hex(&hex_digit.to_string().repeat(40)).expect("an object name"),
        }
    }

    fn change(path: &str, new: Option<TreeEntry>) -> Change {
        Change {
            path: path.into(),
            new,
        }
    }

    fn tree(entries: &[(&str, TreeEntry)]) -> BTreeMap<BString, TreeEntry> {
        entries
            .iter()
            .map(|(path, entry)| ((*path).into(), *entry))
            .collect()
    }

    #[test]
    fn test_every_change_present() {
        let b = entry(EntryKind::Blob, 'b');
        let changes = [change("a.txt", Some(b)), change("gone.txt", None)];
        let default = tree(&[("a.txt", b), ("other.txt", entry(EntryKind::Blob, 'd'))]);
        assert!(content_landed(&changes, Some(&default)));
    }

    #[test]
    fn test_a_change_missing_from_the_default_branch() {
        let b = entry(EntryKind::Blob, 'b');
        let changes = [change("a.txt", Some(b)), change("gone.txt", None)];
        let still_there = tree(&[("a.txt", b), ("gone.txt", entry(EntryKind::Blob, 'c'))]);
        assert!(!content_landed(&changes, Some(&still_there)));
        let older = tree(&[("a.txt", entry(EntryKind::Blob, 'a'))]);
        assert!(!content_landed(&changes, Some(&older)));
        let mode = tree(&[("a.txt", entry(EntryKind::BlobExecutable, 'b'))]);
        assert!(!content_landed(&changes, Some(&mode)));
    }

    #[test]
    fn test_an_unread_tree_is_not_content_landed() {
        assert!(!content_landed(&[change("gone.txt", None)], None));
        assert!(content_landed(&[], Some(&tree(&[]))));
    }
}
