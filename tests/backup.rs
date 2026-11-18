//! Reading the local backup's archive refs through gitoxide, and checking
//! the archives of live tips with git.

mod sandbox;

use std::collections::BTreeSet;
use std::path::Path;

use git_worktree_manager::backup;
use git_worktree_manager::model::Oid;
use git_worktree_manager::system::Real;
use sandbox::{Sandbox, join};

#[test]
fn test_archive_refs_are_read_and_live_tips_checked() {
    let s = Sandbox::new();
    let repo = s.repo(&[("a.txt", "a\n")]);
    let whole = s.git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    let broken = s.commit(&repo, "b.txt", "b\n");
    let blob = s.git(&repo, &["rev-parse", "HEAD:b.txt"]).trim().to_owned();
    let old = s.commit(&repo, "c.txt", "c\n");
    let backup_path = s.bare(&join(&s.tmp, "home/backups/git-archive/local/repo.git"));
    let time = "2026-11-18T09:12:00Z".parse().expect("a time");
    let first = backup::archive_ref("fix/one", time);
    let second = format!("{first}-2");
    let gone = backup::archive_ref("gone", time);
    let retired = backup::archive_ref("old", time);
    s.git(
        &repo,
        &[
            "push",
            "--quiet",
            "--no-verify",
            &backup_path,
            &format!("{whole}:{first}"),
            &format!("{whole}:{second}"),
            &format!("{broken}:{gone}"),
            &format!("{old}:{retired}"),
            &format!("{whole}:refs/heads/not-an-archive"),
        ],
    );
    // A small push lands as loose objects; losing one breaks every archive
    // above it.
    std::fs::remove_file(join(
        &backup_path,
        &format!("objects/{}/{}", &blob[..2], &blob[2..]),
    ))
    .expect("the blob is a loose object");

    let oid = |hex: &str| Oid::from_hex(hex).expect("an object name");
    let tips = BTreeSet::from([oid(&whole), oid(&broken)]);
    let facts = backup::read(&Real::new(s.environment()), Path::new(&backup_path), &tips)
        .expect("readable");
    let read: Vec<(String, String, Oid, Option<bool>)> = facts
        .archives
        .iter()
        .map(|a| (a.reference.clone(), a.branch.clone(), a.oid, a.compared))
        .collect();
    assert_eq!(
        read,
        vec![
            (first, "fix/one".to_owned(), oid(&whole), Some(true)),
            (second, "fix/one".to_owned(), oid(&whole), Some(true)),
            (gone, "gone".to_owned(), oid(&broken), Some(false)),
            (retired, "old".to_owned(), oid(&old), None),
        ]
    );
    assert!(facts.archives.iter().all(|a| a.time == time));
}
