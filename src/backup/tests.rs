use std::collections::BTreeSet;
use std::path::PathBuf;

use super::*;
use crate::system::{Completed, Fake};

fn pair(owner: &str, repository: &str) -> (String, String) {
    (owner.to_owned(), repository.to_owned())
}

#[test]
fn test_backup_names() {
    let cases = [
        (
            Some("git@github.com:nhooey/nix-worktrees.git"),
            pair("nhooey", "nix-worktrees"),
        ),
        (
            Some("https://github.com/nhooey/ansible"),
            pair("nhooey", "ansible"),
        ),
        (
            Some("ssh://git@github.com/nhooey/amqp.git/"),
            pair("nhooey", "amqp"),
        ),
        (Some("file:///srv/git/xtdb.git"), pair("git", "xtdb")),
        (Some("/tmp/x/repo.remote.git"), pair("x", "repo.remote")),
        (None, pair("local", "scratch")),
        (Some(""), pair("local", "scratch")),
        (Some("https://github.com/../.."), pair("local", "scratch")),
        (Some("repo.git"), pair("local", "scratch")),
    ];
    for (url, expected) in cases {
        assert_eq!(name(url, "scratch"), expected, "{url:?}");
    }
}

#[test]
fn test_location() {
    let environment = Environment::from_pairs([("HOME", "/h")]);
    assert_eq!(
        location(&environment, Some("git@github.com:o/r.git"), "clone"),
        Some(PathBuf::from("/h/backups/git-archive/o/r.git"))
    );
    assert_eq!(
        location(&environment, None, "clone"),
        Some(PathBuf::from("/h/backups/git-archive/local/clone.git"))
    );
    assert_eq!(location(&Environment::default(), None, "clone"), None);
}

#[test]
fn test_archive_refs() {
    let time: Timestamp = "2026-11-18T09:12:00.7Z".parse().expect("a time");
    let reference = archive_ref("a/b", time);
    assert_eq!(reference, "refs/archive/a/b/2026-11-18T091200Z");
    let whole: Timestamp = "2026-11-18T09:12:00Z".parse().expect("a time");
    assert_eq!(
        parse_archive_ref(&reference),
        Some(("a/b".to_owned(), whole))
    );
    assert_eq!(
        parse_archive_ref(&format!("{reference}-2")),
        Some(("a/b".to_owned(), whole))
    );
    assert_eq!(parse_archive_ref("refs/archive/a/b/yesterday"), None);
    assert_eq!(parse_archive_ref("refs/heads/a/2026-11-18T091200Z"), None);
    assert_eq!(parse_archive_ref("refs/archive/2026-11-18T091200Z"), None);
}

#[test]
fn test_archive_refs_are_unique() {
    assert_eq!(
        unique_refs(&["r", "r", "s"], &BTreeSet::from(["s".to_owned()])),
        ["r", "r-2", "s-2"]
    );
}

#[test]
fn test_a_missing_backup_has_no_archives() {
    let system = Fake::new("2026-11-19T09:12:00Z".parse().expect("a time"));
    let missing = PathBuf::from("/nonexistent/backup.git");
    assert_eq!(
        read(&system, &missing, &BTreeSet::new()),
        Ok(BackupFacts {
            path: missing,
            archives: vec![]
        })
    );
    assert!(system.acts.is_empty());
}

// Comparing the archives of live tips -------------------------------------------

const BACKUP: &str = "/b/repo.git";

fn oid(digit: char) -> Oid {
    Oid::from_hex(&digit.to_string().repeat(40)).expect("an object name")
}

fn archive(branch: &str, at: char) -> ArchiveFacts {
    ArchiveFacts {
        reference: format!("{ARCHIVE_PREFIX}{branch}/2026-11-18T091200Z"),
        branch: branch.to_owned(),
        time: "2026-11-18T09:12:00Z".parse().expect("a time"),
        oid: oid(at),
        compared: None,
    }
}

fn lines(oids: &[Oid]) -> Completed {
    Completed::ok(
        oids.iter()
            .map(|o| format!("{}\n", o.0))
            .collect::<String>(),
    )
}

fn failed() -> Completed {
    Completed {
        code: 128,
        stdout: Vec::new(),
        stderr: "fatal\n".to_owned(),
    }
}

fn walk(oids: &[Oid]) -> Vec<String> {
    ["rev-list", "--objects", "--missing=error", "--quiet"]
        .into_iter()
        .map(str::to_owned)
        .chain(oids.iter().map(|o| o.0.to_string()))
        .collect()
}

fn compared(archives: &[ArchiveFacts]) -> Vec<(String, Option<bool>)> {
    archives
        .iter()
        .map(|a| (a.branch.clone(), a.compared))
        .collect()
}

#[test]
fn test_only_the_archives_of_live_tips_are_compared() {
    let archives = vec![archive("a", 'a'), archive("old", 'c'), archive("b", 'b')];
    let tips = BTreeSet::from([oid('a'), oid('b')]);
    let refs = |names: &[&str]| -> Vec<String> {
        ["rev-parse"]
            .into_iter()
            .chain(names.iter().copied())
            .map(str::to_owned)
            .collect()
    };
    let system = Fake::new("2026-11-19T09:12:00Z".parse().expect("a time"))
        .with_git(
            BACKUP,
            refs(&[&archives[0].reference, &archives[2].reference]),
            lines(&[oid('a'), oid('b')]),
        )
        .with_git(BACKUP, walk(&[oid('a'), oid('b')]), Completed::ok(""));
    assert_eq!(
        compared(&check(&system, Path::new(BACKUP), archives, &tips).expect("compared")),
        [
            ("a".to_owned(), Some(true)),
            ("old".to_owned(), None),
            ("b".to_owned(), Some(true)),
        ]
    );
    assert!(system.acts.is_empty(), "the check is read-only");
}

#[test]
fn test_a_failed_walk_falls_back_to_one_per_tip() {
    let archives = vec![archive("a", 'a'), archive("b", 'b'), archive("moved", 'a')];
    let tips = BTreeSet::from([oid('a'), oid('b')]);
    let names: Vec<String> = archives.iter().map(|a| a.reference.clone()).collect();
    let system = Fake::new("2026-11-19T09:12:00Z".parse().expect("a time"))
        .with_git(
            BACKUP,
            ["rev-parse"].into_iter().map(str::to_owned).chain(names),
            // The last ref names another commit than the one read.
            lines(&[oid('a'), oid('b'), oid('d')]),
        )
        .with_git(BACKUP, walk(&[oid('a'), oid('b')]), failed())
        .with_git(BACKUP, walk(&[oid('a')]), Completed::ok(""))
        .with_git(BACKUP, walk(&[oid('b')]), failed());
    assert_eq!(
        compared(&check(&system, Path::new(BACKUP), archives, &tips).expect("compared")),
        [
            ("a".to_owned(), Some(true)),
            ("b".to_owned(), Some(false)),
            ("moved".to_owned(), Some(false)),
        ]
    );
}

#[test]
fn test_a_failed_rev_parse_falls_back_to_one_per_ref() {
    let archives = vec![archive("a", 'a'), archive("gone", 'b')];
    let tips = BTreeSet::from([oid('a'), oid('b')]);
    let names: Vec<String> = archives.iter().map(|a| a.reference.clone()).collect();
    let verify = |name: &str| ["rev-parse", "--verify", "--quiet", name].map(str::to_owned);
    let system = Fake::new("2026-11-19T09:12:00Z".parse().expect("a time"))
        .with_git(
            BACKUP,
            ["rev-parse"]
                .into_iter()
                .map(str::to_owned)
                .chain(names.clone()),
            failed(),
        )
        .with_git(BACKUP, verify(&names[0]), lines(&[oid('a')]))
        .with_git(BACKUP, verify(&names[1]), failed())
        .with_git(BACKUP, walk(&[oid('a'), oid('b')]), Completed::ok(""));
    assert_eq!(
        compared(&check(&system, Path::new(BACKUP), archives, &tips).expect("compared")),
        [
            ("a".to_owned(), Some(true)),
            ("gone".to_owned(), Some(false))
        ]
    );
}
