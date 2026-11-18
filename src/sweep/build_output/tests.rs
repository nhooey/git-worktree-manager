use super::*;

fn identity(inode: u64) -> FileId {
    FileId { device: 1, inode }
}

fn entry(id: &str, pointer: Pointer) -> Entry {
    Entry {
        path: PathBuf::from("/c/app/wt").join(id),
        id: id.to_owned(),
        pointer,
    }
}

fn names(path: &str, inode: Option<u64>) -> Pointer {
    Pointer::Names {
        path: path.into(),
        identity: inode.map(identity),
    }
}

fn owner(path: &str, id: &str, inode: Option<u64>, going: bool) -> Owner {
    Owner {
        path: path.into(),
        identity: Ok(inode.map(identity)),
        id: id.to_owned(),
        going,
    }
}

fn planned(entries: &[Entry], owners: &[Owner]) -> Vec<(String, BuildOutputAction, OutcomeKind)> {
    plan(entries, owners)
        .into_iter()
        .map(|b| (b.path.to_string_lossy().into_owned(), b.action, b.outcome))
        .collect()
}

#[test]
fn test_a_back_pointer_is_an_absolute_path_and_one_newline() {
    assert_eq!(pointer(b"/w/fix\n"), Some(PathBuf::from("/w/fix")));
    assert_eq!(pointer(b"/w/fix"), None);
    assert_eq!(pointer(b"\n"), None);
    assert_eq!(pointer(b"/w/fix\n\n"), None);
    assert_eq!(pointer(b"/w/a\n/w/b\n"), None);
    assert_eq!(pointer(b"w/fix\n"), None);
}

#[test]
fn test_only_what_no_staying_worktree_owns_is_reported() {
    let owners = [
        owner("/w/app", "main", Some(1), false),
        owner("/w/fix", "fix-11111111", Some(2), false),
        owner("/w/done", "done-22222222", None, true),
    ];
    let entries = [
        entry("main", Pointer::None),
        entry("fix-11111111", names("/w/fix", Some(2))),
        entry("moved-33333333", names("/w/fix", Some(2))),
        entry("done-22222222", names("/w/done", None)),
        entry("gone-44444444", names("/w/gone", None)),
        entry("stray", Pointer::None),
        entry("odd", Pointer::Unreadable("Is a directory".to_owned())),
    ];
    assert_eq!(
        planned(&entries, &owners),
        [
            (
                "/c/app/wt/done-22222222".to_owned(),
                BuildOutputAction::Delete,
                OutcomeKind::Planned
            ),
            (
                "/c/app/wt/gone-44444444".to_owned(),
                BuildOutputAction::Delete,
                OutcomeKind::Planned
            ),
            (
                "/c/app/wt/stray".to_owned(),
                BuildOutputAction::Keep,
                OutcomeKind::None
            ),
            (
                "/c/app/wt/odd".to_owned(),
                BuildOutputAction::Keep,
                OutcomeKind::Failed
            ),
        ]
    );
}

/// A back-pointer in other case names another path, and its id differs,
/// but the folder is the listed worktree's.
#[test]
fn test_a_back_pointer_to_a_listed_worktrees_folder_by_identity_is_owned() {
    let owners = [owner("/w/fix", "fix-11111111", Some(7), false)];
    let entries = [entry("FIX-99999999", names("/w/FIX", Some(7)))];
    assert_eq!(planned(&entries, &owners), []);
}

#[test]
fn test_an_unreadable_entry_keeps_why() {
    let reports = plan(
        &[entry("odd", Pointer::Unreadable("denied".to_owned()))],
        &[],
    );
    assert_eq!(reports[0].error.as_deref(), Some("denied"));
}

/// A staying worktree whose folder can't be identified might own any
/// entry, so none is deleted; one going doesn't matter.
#[test]
fn test_an_unidentified_staying_worktree_keeps_every_entry() {
    let unidentified = |going| Owner {
        identity: Err("Not a directory".to_owned()),
        ..owner("/w/odd", "odd-55555555", None, going)
    };
    let entries = [entry("gone-44444444", names("/w/gone", None))];
    assert_eq!(
        planned(&entries, &[unidentified(false)]),
        [(
            "/c/app/wt/gone-44444444".to_owned(),
            BuildOutputAction::Keep,
            OutcomeKind::Failed
        )]
    );
    assert!(
        plan(&entries, &[unidentified(false)])[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("/w/odd can't be identified"))
    );
    assert_eq!(
        planned(&entries, &[unidentified(true)]),
        [(
            "/c/app/wt/gone-44444444".to_owned(),
            BuildOutputAction::Delete,
            OutcomeKind::Planned
        )]
    );
}
