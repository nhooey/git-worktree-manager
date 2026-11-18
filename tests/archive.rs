//! `archive write`, `verify`, `list` and `restore`, run as the binary in
//! sandbox repositories whose backup is under the sandbox's HOME.

mod sandbox;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use git_worktree_manager::archive::{self, owner_readme, root_readme};
use git_worktree_manager::backup;
use git_worktree_manager::model::Oid;
use git_worktree_manager::model::facts::RefFacts;
use git_worktree_manager::system::Real;
use sandbox::{Sandbox, join, read, real, utime};
use serde_json::Value;

/// The backup root under the sandbox's HOME.
fn root(s: &Sandbox) -> String {
    join(&s.var("HOME"), "backups/git-archive")
}

/// The backup of a clone of `repo.remote.git`: owned by the sandbox's
/// folder, as the remote's path names it.
fn cloned_backup(s: &Sandbox) -> String {
    let owner = Path::new(&s.tmp).file_name().expect("a name");
    join(
        &root(s),
        &format!("{}/repo.remote.git", owner.to_string_lossy()),
    )
}

/// A branch `name` off `main` with a commit of its own; its tip.
fn branch(s: &Sandbox, repo: &str, name: &str, file: &str) -> String {
    s.git(repo, &["checkout", "--quiet", "-b", name, "main"]);
    let tip = s.commit(repo, file, &format!("only on {name}\n"));
    s.git(repo, &["checkout", "--quiet", "main"]);
    tip
}

/// Every ref and the object it points at.
fn refs(s: &Sandbox, repo: &str) -> String {
    s.git(repo, &["for-each-ref", "--format=%(refname) %(objectname)"])
}

fn archives(s: &Sandbox, backup: &str) -> BTreeMap<String, String> {
    refs(s, backup)
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(name, _)| name.starts_with(backup::ARCHIVE_PREFIX))
        .map(|(name, oid)| (name.to_owned(), oid.to_owned()))
        .collect()
}

/// Deletes from the backup of a clone with no `origin` the blob `object`
/// names, which no other archive holds.
fn remove_blob(s: &Sandbox, repo: &str, object: &str) {
    let blob = s.git(repo, &["rev-parse", object]).trim().to_owned();
    let backup = join(&root(s), "local/repo.git");
    fs::remove_file(join(
        &backup,
        &format!("objects/{}/{}", &blob[..2], &blob[2..]),
    ))
    .expect("the blob is a loose object");
}

/// A manifest naming each of `paths`; its path.
fn manifest(s: &Sandbox, paths: &[&str]) -> String {
    let file = join(&s.tmp, "repositories.json");
    let entries: Vec<Value> = paths
        .iter()
        .map(|path| serde_json::json!({ "path": path }))
        .collect();
    s.write(&file, &Value::from(entries).to_string());
    file
}

/// Each repository's problem code, null for one with none.
fn problem_codes(report: &Value) -> Vec<&Value> {
    report["repositories"]
        .as_array()
        .expect("repositories")
        .iter()
        .map(|r| &r["problem"]["code"])
        .collect()
}

fn compares(report: &Value) -> &Vec<Value> {
    report["repositories"][0]["backup"]["compares"]
        .as_array()
        .expect("compares")
}

/// Every file and folder under `top`, with its bytes and modification
/// time.
fn snapshot(top: &str) -> BTreeMap<PathBuf, (Option<Vec<u8>>, SystemTime)> {
    fn walk(path: &Path, into: &mut BTreeMap<PathBuf, (Option<Vec<u8>>, SystemTime)>) {
        let meta = fs::symlink_metadata(path).expect("metadata");
        let modified = meta.modified().expect("a modification time");
        let bytes = meta.is_file().then(|| fs::read(path).expect("bytes"));
        into.insert(path.to_owned(), (bytes, modified));
        if meta.is_dir() {
            fs::read_dir(path)
                .expect("a folder")
                .for_each(|entry| walk(&entry.expect("an entry").path(), into));
        }
    }
    let mut found = BTreeMap::new();
    walk(Path::new(top), &mut found);
    found
}

#[test]
fn test_an_archive_lands_in_the_backup_and_never_on_origin() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let remote = real(&join(&s.tmp, "repo.remote.git"));
    let tip = branch(&s, &repo, "fix", "fix.txt");
    let origin = refs(&s, &remote);
    let backup = cloned_backup(&s);

    let written = s.run_json(&["archive", "write", "fix"], Some(&repo), 0);
    let made = archives(&s, &backup);
    let (reference, oid) = made.iter().next().expect("one archive");
    assert!(reference.starts_with("refs/archive/fix/"), "{reference}");
    assert_eq!(oid, &tip);
    assert_eq!(written["command"], "archive write");
    assert_eq!(written["dry_run"], false);
    let part = &written["repositories"][0]["backup"];
    assert_eq!(part["path"], backup.as_str());
    assert_eq!(
        (&part["archived"], &part["verified"], &part["failed"]),
        (&1.into(), &1.into(), &0.into())
    );
    assert_eq!(compares(&written)[0]["ref"], reference.as_str());
    assert_eq!(compares(&written)[0]["ok"], true);
    assert_eq!(
        written["backup_note"],
        format!(
            "1 new archive in {}/; back up {}/",
            root(&s),
            join(&s.var("HOME"), "backups")
        )
    );
    assert_eq!(refs(&s, &remote), origin);

    let verified = s.run_json(&["archive", "verify"], Some(&repo), 0);
    assert_eq!(compares(&verified)[0]["ok"], true);
    let listed = s.run_json(&["archive", "list"], Some(&repo), 0);
    assert_eq!(listed["archives"][0]["ref"], reference.as_str());
    s.git(&repo, &["branch", "--delete", "--force", "fix"]);
    s.run_json(&["archive", "restore", "fix"], Some(&repo), 0);
    assert_eq!(refs(&s, &remote), origin);
    assert!(
        !refs(&s, &repo).contains("refs/archive/"),
        "{}",
        refs(&s, &repo)
    );
}

#[test]
fn test_the_text_names_each_archive() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let tip = branch(&s, &repo, "fix", "fix.txt");
    let ran = s.run(&["archive", "write", "fix"], Some(&repo), &[]);
    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    let backup = join(&root(&s), "local/repo.git");
    let reference = archives(&s, &backup)
        .into_keys()
        .next()
        .expect("an archive");
    assert_eq!(
        ran.stdout,
        format!(
            "archived {reference} at {} in {backup}\n1 new archive in {}/; back up {}/\n",
            &tip[..12],
            root(&s),
            join(&s.var("HOME"), "backups")
        )
    );
}

#[test]
fn test_a_second_archive_in_the_same_second_gets_2() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let tip = branch(&s, &repo, "fix", "fix.txt");
    s.run_json(&["archive", "write", "fix"], Some(&repo), 0);
    let backup = join(&root(&s), "local/repo.git");
    let first = archives(&s, &backup)
        .into_keys()
        .next()
        .expect("an archive");
    let (_, at) = backup::parse_archive_ref(&first).expect("an archive ref");
    // The seconds after the first, so the second run's falls among them.
    (0..30).for_each(|n| {
        let taken = backup::archive_ref("fix", at + jiff::SignedDuration::from_secs(n));
        s.git(&backup, &["update-ref", &taken, &tip]);
    });
    let second = s.run_json(&["archive", "write", "fix"], Some(&repo), 0);
    let reference = compares(&second)[0]["ref"]
        .as_str()
        .expect("a ref")
        .to_owned();
    assert!(reference.ends_with("-2"), "{reference}");
    assert_eq!(archives(&s, &backup)[&reference], tip);
}

#[test]
fn test_a_missing_object_fails_the_compare_and_verify_exits_1() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    branch(&s, &repo, "solo", "solo.txt");
    s.run_json(&["archive", "write", "solo"], Some(&repo), 0);
    remove_blob(&s, &repo, "solo:solo.txt");
    let verified = s.run_json(&["archive", "verify"], Some(&repo), 1);
    let compare = &compares(&verified)[0];
    assert_eq!(compare["ok"], false);
    let detail = compare["detail"].as_str().expect("a detail");
    assert!(
        detail.contains(" rev-list --objects --missing=error"),
        "{detail}"
    );
    assert!(detail.contains(" fsck --connectivity-only"), "{detail}");
    assert_eq!(verified["repositories"][0]["backup"]["failed"], 1);
}

#[test]
fn test_a_backup_that_cannot_be_read_is_a_problem() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    branch(&s, &repo, "fix", "fix.txt");
    s.write(&root(&s), "not a folder\n");
    let written = s.run_json(&["archive", "write", "fix"], Some(&repo), 1);
    let problem = &written["repositories"][0]["problem"];
    assert_eq!(problem["code"], "unreadable");
    let detail = problem["detail"].as_str().expect("a detail");
    assert!(detail.contains("Not a directory"), "{detail}");
    assert_eq!(read(&root(&s)), "not a folder\n");
}

#[test]
fn test_an_archive_that_cannot_be_written_fails() {
    use std::os::unix::fs::PermissionsExt;
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    branch(&s, &repo, "fix", "fix.txt");
    let owner = join(&root(&s), "local");
    fs::create_dir_all(&owner).expect("the owner folder");
    fs::set_permissions(&owner, fs::Permissions::from_mode(0o555)).expect("read-only");
    let written = s.run_json(&["archive", "write", "fix"], Some(&repo), 1);
    let compare = &compares(&written)[0];
    assert_eq!(compare["ok"], false);
    let detail = compare["detail"].as_str().expect("a detail");
    assert!(
        detail.starts_with("not written: git init --bare --quiet"),
        "{detail}"
    );
    assert_eq!(written["repositories"][0]["backup"]["archived"], 0);
    assert_eq!(written["backup_note"], Value::Null);
    fs::set_permissions(&owner, fs::Permissions::from_mode(0o755)).expect("writable again");
}

#[test]
fn test_a_named_branch_the_clone_lacks_writes_nothing() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    branch(&s, &repo, "fix", "fix.txt");
    let written = s.run_json(&["archive", "write", "fix", "nope"], Some(&repo), 1);
    assert_eq!(
        written["error"],
        serde_json::json!({
            "code": "step-failed",
            "detail": format!("no local branch nope in {repo}"),
        })
    );
    assert!(!Path::new(&root(&s)).exists());
}

#[test]
fn test_a_written_archive_that_fails_its_compare_exits_1() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    branch(&s, &repo, "solo", "solo.txt");
    s.run_json(&["archive", "write", "solo"], Some(&repo), 0);
    remove_blob(&s, &repo, "solo:solo.txt");
    branch(&s, &repo, "next", "next.txt");
    let written = s.run_json(&["archive", "write", "next"], Some(&repo), 1);
    let compare = &compares(&written)[0];
    assert_eq!(compare["ok"], false);
    let detail = compare["detail"].as_str().expect("a detail");
    assert!(detail.contains(" fsck --connectivity-only"), "{detail}");
    assert!(!detail.contains("rev-list"), "{detail}");
    let part = &written["repositories"][0]["backup"];
    assert_eq!((&part["archived"], &part["failed"]), (&1.into(), &1.into()));
}

#[test]
fn test_an_inherited_object_directory_cannot_pass_the_compare() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    branch(&s, &repo, "solo", "solo.txt");
    s.run_json(&["archive", "write", "solo"], Some(&repo), 0);
    remove_blob(&s, &repo, "solo:solo.txt");
    let objects = join(&repo, ".git/objects");
    for variable in ["GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES"] {
        let ran = s.run(
            &["archive", "verify", "--format", "json"],
            Some(&repo),
            &[(variable, &objects)],
        );
        assert_eq!(ran.code, 1, "{variable}: {}", ran.stdout);
    }
}

#[test]
fn test_a_branch_named_like_an_option_is_archived_and_restored() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let tip = s.git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    s.git(&repo, &["update-ref", "refs/heads/-m", &tip]);
    let run = |command: &str| {
        let ran = s.run(&["archive", command, "--", "-m"], Some(&repo), &[]);
        assert_eq!((ran.code, ran.stderr.as_str()), (0, ""), "{command}");
    };
    run("write");
    s.git(&repo, &["update-ref", "-d", "refs/heads/-m"]);
    run("restore");
    assert_eq!(s.git(&repo, &["rev-parse", "refs/heads/-m"]).trim(), tip);
    assert_eq!(
        s.git(&repo, &["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/main"
    );
}

#[test]
fn test_a_push_never_moves_an_archive() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let first = branch(&s, &repo, "fix", "one.txt");
    s.run_json(&["archive", "write", "fix"], Some(&repo), 0);
    let backup = join(&root(&s), "local/repo.git");
    let taken = archives(&s, &backup)
        .into_keys()
        .next()
        .expect("an archive");
    s.git(&repo, &["checkout", "--quiet", "fix"]);
    let second = s.commit(&repo, "two.txt", "two\n");
    s.git(&repo, &["checkout", "--quiet", "main"]);
    let onto = [RefFacts {
        name: taken.clone(),
        oid: Oid::from_hex(&second).expect("an object name"),
    }];
    let written = archive::write(
        &mut Real::new(s.environment()),
        Path::new(&join(&repo, ".git")),
        Path::new(&backup),
        &onto,
    );
    assert!(written.is_err(), "{written:?}");
    assert_eq!(archives(&s, &backup)[&taken], first);
}

#[test]
fn test_restore_refuses_a_branch_a_worktree_has_checked_out_unborn() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    branch(&s, &repo, "fix", "fix.txt");
    s.run_json(&["archive", "write", "fix"], Some(&repo), 0);
    s.git(&repo, &["branch", "--delete", "--force", "fix"]);
    let worktree = join(&s.tmp, "orphan");
    s.git(
        &repo,
        &[
            "worktree", "add", "--quiet", "--orphan", "-b", "fix", &worktree,
        ],
    );
    let refused = s.run_json(&["archive", "restore", "fix"], Some(&repo), 1);
    assert_eq!(
        refused["error"]["detail"],
        "a worktree has the branch fix checked out"
    );
    assert!(!refs(&s, &repo).contains("refs/heads/fix"));
}

#[test]
fn test_write_with_no_branch_archives_every_unmerged_branch_the_backup_lacks() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let fix = branch(&s, &repo, "fix", "fix.txt");
    let written = s.run_json(&["archive", "write"], Some(&repo), 0);
    let found: Vec<(&str, &str)> = compares(&written)
        .iter()
        .map(|c| {
            (
                c["ref"].as_str().unwrap_or(""),
                c["oid"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].0.starts_with("refs/archive/fix/"), "{found:?}");
    assert_eq!(found[0].1, fix);
    let again = s.run_json(&["archive", "write"], Some(&repo), 0);
    assert_eq!(compares(&again).len(), 0);
}

#[test]
fn test_outside_a_repository_nothing_is_printed() {
    let s = Sandbox::new();
    for args in [
        &["archive", "list", "--format", "json"][..],
        &["archive", "verify"],
        &["archive", "write"],
        &["archive", "write", "--dry-run"],
        &["archive", "write", "fix", "--format", "json"],
        &["archive", "restore", "fix"],
    ] {
        let ran = s.run(args, None, &[]);
        assert_eq!((ran.code, ran.stdout.as_str()), (1, ""), "{args:?}");
        assert!(
            ran.stderr.contains("is not in a git repository"),
            "{}",
            ran.stderr
        );
    }
}

#[test]
fn test_a_dry_run_skips_a_manifest_entry_with_no_clone_and_plans_the_rest() {
    let s = Sandbox::new();
    let repo = s.cloned();
    branch(&s, &repo, "fix", "fix.txt");
    let gone = join(&s.tmp, "gone");
    let repositories = manifest(&s, &[&repo, &gone]);
    let args = [
        "archive",
        "write",
        "--dry-run",
        "--repositories",
        &repositories,
    ];
    let ran = s.run(&args, None, &[]);
    assert_eq!(ran.code, 0, "{}{}", ran.stderr, ran.stdout);
    assert!(
        ran.stdout.contains(&format!("{gone}: skipped: ")),
        "{}",
        ran.stdout
    );
    assert!(ran.stdout.contains(": refs/archive/fix/"), "{}", ran.stdout);
    let planned = s.run_json(&args, None, 0);
    assert_eq!(
        problem_codes(&planned),
        [&Value::Null, &Value::from("skipped")]
    );
    let note = planned["backup_note"].as_str().expect("a note");
    assert!(note.contains(": refs/archive/fix/"), "{note}");
    assert!(!Path::new(&root(&s)).exists());
}

#[test]
fn test_write_skips_a_manifest_entry_with_no_clone_and_archives_the_rest() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let tip = branch(&s, &repo, "fix", "fix.txt");
    let gone = join(&s.tmp, "gone");
    let repositories = manifest(&s, &[&repo, &gone]);
    let written = s.run_json(
        &["archive", "write", "--repositories", &repositories],
        None,
        0,
    );
    assert_eq!(
        problem_codes(&written),
        [&Value::Null, &Value::from("skipped")]
    );
    let made = archives(&s, &cloned_backup(&s));
    let (reference, oid) = made.iter().next().expect("one archive");
    assert!(reference.starts_with("refs/archive/fix/"), "{reference}");
    assert_eq!((made.len(), oid), (1, &tip));
}

#[test]
fn test_a_manifest_entry_that_cannot_be_read_fails_the_write() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    s.write(&join(&repo, ".git/refs/heads/broken"), "rubbish\n");
    let gone = join(&s.tmp, "gone");
    let repositories = manifest(&s, &[&repo, &gone]);
    let written = s.run_json(
        &["archive", "write", "--repositories", &repositories],
        None,
        1,
    );
    assert_eq!(
        problem_codes(&written),
        [&Value::from("unreadable"), &Value::from("skipped")]
    );
}

#[test]
fn test_restore_brings_a_branch_back_and_refuses_one_that_exists() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let first = branch(&s, &repo, "fix", "one.txt");
    s.run_json(&["archive", "write", "fix"], Some(&repo), 0);
    s.git(&repo, &["checkout", "--quiet", "fix"]);
    let second = s.commit(&repo, "two.txt", "two\n");
    s.git(&repo, &["checkout", "--quiet", "main"]);
    let backup = join(&root(&s), "local/repo.git");
    let older = archives(&s, &backup)
        .into_keys()
        .next()
        .expect("an archive");
    // The second archive is a second newer, whenever it is written.
    let (_, at) = backup::parse_archive_ref(&older).expect("an archive ref");
    let newer = backup::archive_ref("fix", at + jiff::SignedDuration::from_secs(1));
    s.git(
        &repo,
        &[
            "push",
            "--quiet",
            "--no-verify",
            &backup,
            &format!("{second}:{newer}"),
        ],
    );

    let refused = s.run_json(&["archive", "restore", "fix"], Some(&repo), 1);
    assert_eq!(refused["command"], "archive restore");
    assert_eq!(refused["error"]["code"], "step-failed");
    assert_eq!(refused["error"]["detail"], "the branch fix exists");

    let tip = |s: &Sandbox| {
        s.git(&repo, &["rev-parse", "refs/heads/fix"])
            .trim()
            .to_owned()
    };
    s.git(&repo, &["branch", "--delete", "--force", "fix"]);
    let restored = s.run_json(&["archive", "restore", "fix"], Some(&repo), 0);
    assert_eq!(
        (&restored["ref"], &restored["outcome"]),
        (&newer.as_str().into(), &"done".into())
    );
    assert_eq!(tip(&s), second);

    let stamp = older.rsplit_once('/').expect("a stamp").1;
    let iso = at.to_string();
    for at in [stamp, iso.as_str()] {
        s.git(&repo, &["branch", "--delete", "--force", "fix"]);
        s.run_json(&["archive", "restore", "fix", "--at", at], Some(&repo), 0);
        assert_eq!(tip(&s), first, "{at}");
    }
    s.git(&repo, &["branch", "--delete", "--force", "fix"]);
    let missing = s.run_json(
        &["archive", "restore", "fix", "--at", "2000-01-01T00:00:00Z"],
        Some(&repo),
        1,
    );
    assert!(
        missing["error"]["detail"]
            .as_str()
            .expect("a detail")
            .starts_with("no archive of fix at")
    );
    assert!(!refs(&s, &repo).contains("refs/heads/fix"));
}

#[test]
fn test_list_is_newest_first() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let tip = branch(&s, &repo, "fix", "fix.txt");
    s.run_json(&["archive", "write", "fix"], Some(&repo), 0);
    let backup = join(&root(&s), "local/repo.git");
    let names = [
        "refs/archive/fix/2026-01-01T000000Z",
        "refs/archive/fix/2026-01-01T000000Z-2",
        "refs/archive/other/2026-01-02T000000Z",
    ];
    names.iter().for_each(|name| {
        s.git(&backup, &["update-ref", name, &tip]);
    });
    let listed = s.run_json(&["archive", "list", "fix"], Some(&repo), 0);
    let found: Vec<&str> = listed["archives"]
        .as_array()
        .expect("archives")
        .iter()
        .map(|a| a["ref"].as_str().expect("a ref"))
        .collect();
    assert_eq!(found.len(), 3);
    assert!(found[0].starts_with("refs/archive/fix/20"), "{found:?}");
    assert_eq!(
        &found[1..],
        &names[..2].iter().rev().copied().collect::<Vec<_>>()
    );
    assert_eq!(listed["backup"], backup.as_str());
}

#[test]
fn test_a_dry_run_changes_nothing() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    branch(&s, &repo, "fix", "fix.txt");
    let home = s.var("HOME");
    let before = (snapshot(&home), snapshot(&repo));
    let planned = s.run_json(&["archive", "write", "--dry-run", "fix"], Some(&repo), 0);
    assert_eq!(planned["dry_run"], true);
    assert_eq!(compares(&planned).len(), 0);
    assert_eq!(planned["repositories"][0]["backup"]["archived"], 0);
    let note = planned["backup_note"].as_str().expect("a note");
    assert!(
        note.starts_with("1 new archive planned, not written, in ")
            && note.contains(": refs/archive/fix/"),
        "{note}"
    );
    assert_eq!((snapshot(&home), snapshot(&repo)), before);
    assert!(!Path::new(&root(&s)).exists());

    s.run_json(&["archive", "write", "fix"], Some(&repo), 0);
    s.git(&repo, &["branch", "--delete", "--force", "fix"]);
    let before = (snapshot(&home), snapshot(&repo));
    let restored = s.run_json(&["archive", "restore", "--dry-run", "fix"], Some(&repo), 0);
    assert_eq!(restored["outcome"], "planned");
    assert_eq!((snapshot(&home), snapshot(&repo)), before);
}

#[test]
fn test_the_readmes_and_a_second_run_rewrites_none() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    branch(&s, &repo, "fix", "fix.txt");
    s.run_json(&["archive", "write", "fix"], Some(&repo), 0);
    let top = root(&s);
    let readmes = [join(&top, "README.md"), join(&top, "local/README.md")];
    assert_eq!(
        read(&readmes[0]),
        root_readme(Path::new(&top), &["local".to_owned()])
    );
    assert_eq!(
        read(&readmes[1]),
        owner_readme(Path::new(&top), "local", &["repo.git".to_owned()])
    );
    readmes
        .iter()
        .for_each(|readme| utime(readme, 1_000_000_000.0));
    let before: Vec<SystemTime> = readmes
        .iter()
        .map(|r| fs::metadata(r).and_then(|m| m.modified()).expect("a time"))
        .collect();
    s.run_json(&["archive", "write", "fix"], Some(&repo), 0);
    let after: Vec<SystemTime> = readmes
        .iter()
        .map(|r| fs::metadata(r).and_then(|m| m.modified()).expect("a time"))
        .collect();
    assert_eq!(after, before);
}
