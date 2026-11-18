use std::path::PathBuf;

use super::*;
use crate::environment::Environment;
use crate::model::facts::{BackupFacts, BranchFacts, CommitFacts};
use crate::system::{Act, Fake};

const GIT_DIR: &str = "/src/app/.git";
const BACKUP: &str = "/h/backups/git-archive/o/app.git";

fn oid(hex_digit: char) -> Oid {
    Oid::from_hex(&hex_digit.to_string().repeat(40)).expect("an object name")
}

fn time(text: &str) -> Timestamp {
    text.parse().expect("a time")
}

fn failed(stderr: &str) -> Completed {
    Completed {
        code: 128,
        stdout: Vec::new(),
        stderr: stderr.to_owned(),
    }
}

fn archive(reference: &str, tip: Oid) -> ArchiveFacts {
    let (branch, time) = backup::parse_archive_ref(reference).expect("an archive ref");
    ArchiveFacts {
        reference: reference.to_owned(),
        branch,
        time,
        oid: tip,
        compared: None,
    }
}

fn checked(reference: &str, tip: Oid, compared: Option<bool>) -> ArchiveFacts {
    ArchiveFacts {
        compared,
        ..archive(reference, tip)
    }
}

fn to_write(name: &str, tip: Oid) -> RefFacts {
    RefFacts {
        name: name.to_owned(),
        oid: tip,
    }
}

fn git<S: AsRef<std::ffi::OsStr>>(dir: &str, args: impl IntoIterator<Item = S>) -> Act {
    Act::Git {
        dir: dir.into(),
        args: crate::system::args(args),
    }
}

fn system() -> Fake {
    Fake::new(time("2026-11-19T09:12:00Z"))
        .with_environment(Environment::from_pairs([("HOME", "/h")]))
}

// Choosing -------------------------------------------------------------------------

fn branch(name: &str, tip: Oid, merged: Option<bool>) -> BranchFacts {
    BranchFacts {
        name: name.to_owned(),
        tip: CommitFacts {
            oid: tip,
            time: time("2026-11-01T00:00:00Z"),
            merged,
        },
        upstream: None,
        content_landed: None,
    }
}

#[test]
fn test_unarchived_is_every_unmerged_branch_the_backup_lacks() {
    let facts = RepositoryFacts {
        path: "/src/app".into(),
        common_dir: GIT_DIR.into(),
        bare: false,
        default: None,
        worktrees: vec![],
        branches: vec![
            branch("main", oid('a'), Some(true)),
            branch("archived", oid('b'), Some(false)),
            branch("fix", oid('c'), Some(false)),
            branch("no-default", oid('d'), None),
            branch("failed-push", oid('e'), Some(false)),
            branch("unchecked", oid('f'), Some(false)),
        ],
        remote_refs: vec![],
        backup: BackupFacts {
            path: BACKUP.into(),
            archives: vec![
                checked("refs/archive/old/2026-11-01T000000Z", oid('b'), Some(true)),
                checked(
                    "refs/archive/failed-push/2026-11-01T000000Z",
                    oid('e'),
                    Some(false),
                ),
                checked("refs/archive/unchecked/2026-11-01T000000Z", oid('f'), None),
            ],
        },
    };
    assert_eq!(
        unarchived(&facts),
        [
            ("fix".to_owned(), oid('c')),
            ("no-default".to_owned(), oid('d')),
            ("failed-push".to_owned(), oid('e')),
            ("unchecked".to_owned(), oid('f')),
        ]
    );
}

#[test]
fn test_references_take_the_next_free_name() {
    let now = time("2026-11-19T09:12:00.5Z");
    let existing = [archive("refs/archive/fix/2026-11-19T091200Z", oid('a'))];
    let branches = [
        ("fix".to_owned(), oid('b')),
        ("fix".to_owned(), oid('c')),
        ("new".to_owned(), oid('d')),
    ];
    assert_eq!(
        references(&branches, &existing, now),
        [
            to_write("refs/archive/fix/2026-11-19T091200Z-2", oid('b')),
            to_write("refs/archive/fix/2026-11-19T091200Z-3", oid('c')),
            to_write("refs/archive/new/2026-11-19T091200Z", oid('d')),
        ]
    );
}

#[test]
fn test_newest_first_and_the_one_restored() {
    let archives = [
        archive("refs/archive/fix/2026-11-18T091200Z", oid('a')),
        archive("refs/archive/fix/2026-11-19T091200Z-10", oid('d')),
        archive("refs/archive/fix/2026-11-19T091200Z", oid('b')),
        archive("refs/archive/other/2026-11-20T000000Z", oid('e')),
        archive("refs/archive/fix/2026-11-19T091200Z-2", oid('c')),
    ];
    let names = |chosen: Vec<ArchiveFacts>| -> Vec<String> {
        chosen.into_iter().map(|a| a.reference).collect()
    };
    assert_eq!(
        names(newest_first(&archives, Some("fix"))),
        [
            "refs/archive/fix/2026-11-19T091200Z-10",
            "refs/archive/fix/2026-11-19T091200Z-2",
            "refs/archive/fix/2026-11-19T091200Z",
            "refs/archive/fix/2026-11-18T091200Z",
        ]
    );
    assert_eq!(newest_first(&archives, None).len(), 5);
    let restored = |at: Option<At>| chosen(&archives, "fix", at.as_ref()).map(|a| a.oid);
    assert_eq!(restored(None), Some(oid('d')));
    assert_eq!(
        restored(Some(At::Time(time("2026-11-18T09:12:00Z")))),
        Some(oid('a'))
    );
    assert_eq!(
        restored(Some(At::Stamp("2026-11-19T091200Z-2".to_owned()))),
        Some(oid('c'))
    );
    assert_eq!(
        restored(Some(At::Stamp("2026-11-19T091200Z".to_owned()))),
        Some(oid('b'))
    );
    assert_eq!(restored(Some(At::Time(time("2026-11-17T00:00:00Z")))), None);
    assert_eq!(chosen(&archives, "gone", None), None);
}

#[test]
fn test_the_backup_note() {
    let root = Path::new("/h/backups/git-archive");
    assert_eq!(backup_note(root, 0, &[]), None);
    assert_eq!(
        backup_note(root, 1, &[]).as_deref(),
        Some("1 new archive in /h/backups/git-archive/; back up /h/backups/")
    );
    let planned = [
        to_write("refs/archive/fix/2026-11-19T091200Z", oid('a')),
        to_write("refs/archive/new/2026-11-19T091200Z", oid('b')),
    ];
    assert_eq!(
        backup_note(root, 0, &planned).as_deref(),
        Some(
            "2 new archives planned, not written, in /h/backups/git-archive/: \
             refs/archive/fix/2026-11-19T091200Z at aaaaaaaaaaaa, \
             refs/archive/new/2026-11-19T091200Z at bbbbbbbbbbbb; \
             back up /h/backups/ once they are"
        )
    );
}

// The READMEs ------------------------------------------------------------------------

#[test]
fn test_the_readmes_say_what_is_there_and_how_to_restore_it() {
    let root = Path::new("/h/backups/git-archive");
    let program = format!("git {}", crate::names::subcommand());
    let restoring = |owner: &str| {
        format!(
            "## Restoring

From inside a clone of the project, list a branch's archives, then recreate
the branch from its newest archive or the one at a time:

```bash
{program} archive list <branch>
{program} archive restore <branch> [--at <time>]
```

Or with git alone:

```bash
git ls-remote /h/backups/git-archive/{owner}/<repo>.git 'refs/archive/<branch>/*'
git fetch /h/backups/git-archive/{owner}/<repo>.git refs/archive/<branch>/<time>
git branch <branch> FETCH_HEAD
```
"
        )
    };
    assert_eq!(
        root_readme(root, &["local".to_owned(), "o".to_owned()]),
        format!(
            "# Git archives

Branches archived before they were deleted from a clone, so that no commit
is lost. Nothing here is pushed to any remote.

- **Layout:** one bare repository per project, `<owner>/<repo>.git`, named
  from the clone's `origin` URL, or `local/<folder>.git` for a clone with no
  `origin`. Each owner folder's `README.md` lists its repositories.
- **Owners:** `local`, `o`.
- **Archives:** each is `refs/archive/<branch>/<UTC time>`, at the branch's
  tip when it was archived, with `-2`, `-3` added to a name that is taken.
- **Compare pass:** an archive counts once `git rev-parse` of its ref gives
  the tip, `git rev-list --objects --missing=error <tip>` passes and
  `git fsck --connectivity-only` passes. A branch whose archive fails stays.

{}",
            restoring("<owner>")
        )
    );
    assert_eq!(
        owner_readme(root, "o", &["app.git".to_owned()]),
        format!(
            "# Git archives in `o/`

The archived branches of the projects under `o/`, one bare repository
each; `../README.md` says how they are made and checked.

- **Repositories:** `app.git`.

{}",
            restoring("o")
        )
    );
    assert!(owner_readme(root, "o", &[]).contains("- **Repositories:** none yet.\n"));
}

// Writing ----------------------------------------------------------------------------

fn readmes(owners: &[&str], repositories: &[&str]) -> [Act; 2] {
    let root = Path::new("/h/backups/git-archive");
    let names = |names: &[&str]| -> Vec<String> { names.iter().map(|n| n.to_string()).collect() };
    [
        Act::Write {
            path: root.join("README.md"),
            bytes: root_readme(root, &names(owners)).into_bytes(),
        },
        Act::Write {
            path: root.join("o/README.md"),
            bytes: owner_readme(root, "o", &names(repositories)).into_bytes(),
        },
    ]
}

/// The one push of `archives` into the backup, as a person would type it.
fn push(archives: &[RefFacts]) -> Vec<String> {
    let leases = archives
        .iter()
        .map(|a| format!("--force-with-lease={}:", a.name));
    let refspecs = archives.iter().map(|a| format!("{}:{}", a.oid, a.name));
    ["push", "--quiet", "--no-verify", "--no-recurse-submodules"]
        .map(str::to_owned)
        .into_iter()
        .chain(leases)
        .chain([BACKUP.to_owned()])
        .chain(refspecs)
        .collect()
}

#[test]
fn test_the_push_can_only_create_each_archive() {
    let archives = [
        to_write("refs/archive/fix/2026-11-19T091200Z", oid('a')),
        to_write("refs/archive/new/2026-11-19T091200Z", oid('b')),
    ];
    assert_eq!(
        push_args(Path::new(BACKUP), &archives),
        crate::system::args([
            "push",
            "--quiet",
            "--no-verify",
            "--no-recurse-submodules",
            "--force-with-lease=refs/archive/fix/2026-11-19T091200Z:",
            "--force-with-lease=refs/archive/new/2026-11-19T091200Z:",
            BACKUP,
            &format!("{}:refs/archive/fix/2026-11-19T091200Z", oid('a')),
            &format!("{}:refs/archive/new/2026-11-19T091200Z", oid('b')),
        ])
    );
}

#[test]
fn test_a_branch_named_like_an_option_is_created_by_its_full_name() {
    assert_eq!(
        create_args("-m", oid('a')),
        crate::system::args(["update-ref", "refs/heads/-m", &oid('a').to_string(), ""])
    );
    assert_eq!(
        fetch_args(Path::new(BACKUP), "refs/archive/-m/2026-11-19T091200Z"),
        crate::system::args([
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            "--no-recurse-submodules",
            BACKUP,
            "refs/archive/-m/2026-11-19T091200Z",
        ])
    );
}

#[test]
fn test_the_first_write_makes_the_backup_and_its_readmes_then_pushes_once() {
    let archives = [
        to_write("refs/archive/fix/2026-11-19T091200Z", oid('a')),
        to_write("refs/archive/new/2026-11-19T091200Z", oid('b')),
    ];
    let init = ["init", "--bare", "--quiet", BACKUP];
    let pushed = push(&archives);
    let mut system = system()
        .with_file(
            "/h/backups/git-archive/local/x.git/HEAD",
            "ref: refs/heads/main\n",
        )
        .with_git(GIT_DIR, init, Completed::ok(""))
        .with_git(GIT_DIR, &pushed, Completed::ok(""));
    assert_eq!(
        write(
            &mut system,
            Path::new(GIT_DIR),
            Path::new(BACKUP),
            &archives
        ),
        Ok(())
    );
    let [root, owner] = readmes(&["local"], &[]);
    assert_eq!(
        system.acts,
        [git(GIT_DIR, init), root, owner, git(GIT_DIR, &pushed)]
    );
}

#[test]
fn test_a_second_write_rewrites_no_readme() {
    let archives = [to_write("refs/archive/fix/2026-11-19T091200Z", oid('a'))];
    let pushed = push(&archives);
    let [root, owner] = readmes(&["o"], &["app.git"]);
    let file = |act: Act| match act {
        Act::Write { path, bytes } => (path, bytes),
        _ => unreachable!("a write"),
    };
    let (root, owner) = (file(root), file(owner));
    let mut system = system()
        .with_file(format!("{BACKUP}/HEAD"), "ref: refs/heads/main\n")
        .with_file(root.0, root.1)
        .with_file(owner.0, owner.1)
        .with_git(GIT_DIR, &pushed, Completed::ok(""));
    assert_eq!(
        write(
            &mut system,
            Path::new(GIT_DIR),
            Path::new(BACKUP),
            &archives
        ),
        Ok(())
    );
    assert_eq!(system.acts, [git(GIT_DIR, &pushed)]);
}

#[test]
fn test_writing_nothing_does_nothing() {
    let mut system = system();
    assert_eq!(
        write(&mut system, Path::new(GIT_DIR), Path::new(BACKUP), &[]),
        Ok(())
    );
    assert_eq!(compare(&system, Path::new(BACKUP), &[]), []);
    assert_eq!(system.acts, []);
}

#[test]
fn test_a_backup_that_cannot_be_made_fails_every_archive() {
    let archives = [to_write("refs/archive/fix/2026-11-19T091200Z", oid('a'))];
    let init = ["init", "--bare", "--quiet", BACKUP];
    let mut system = system().with_git(GIT_DIR, init, failed("fatal: cannot mkdir\n"));
    let report = write_and_compare(
        &mut system,
        Path::new(GIT_DIR),
        Path::new(BACKUP),
        &archives,
    );
    assert_eq!(system.acts, [git(GIT_DIR, init)]);
    assert_eq!(
        report,
        BackupReport {
            path: BACKUP.into(),
            archived: 0,
            verified: 0,
            failed: 1,
            compares: vec![Compare {
                reference: archives[0].name.clone(),
                oid: oid('a'),
                ok: false,
                detail: Some(format!(
                    "not written: git init --bare --quiet {BACKUP} exited 128: fatal: cannot mkdir"
                )),
            }],
        }
    );
}

// The compare pass ---------------------------------------------------------------------

const FSCK: [&str; 6] = [
    "--git-dir",
    BACKUP,
    "fsck",
    "--connectivity-only",
    "--no-dangling",
    "--no-progress",
];

fn rev_list(tip: Oid) -> Vec<String> {
    [
        "--git-dir",
        BACKUP,
        "rev-list",
        "--objects",
        "--missing=error",
        "--quiet",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([tip.to_string()])
    .collect()
}

fn rev_parse(reference: &str) -> [&str; 6] {
    [
        "--git-dir",
        BACKUP,
        "rev-parse",
        "--verify",
        "--quiet",
        reference,
    ]
}

/// A backup holding `fix` and `same` at the tip a, and `other` at b, whose
/// checks pass but those `broken` names: `fsck`, `history <tip>`, a ref at
/// another tip, or `missing <ref>`.
fn backup_system(broken: &[&str]) -> (Fake, [RefFacts; 3]) {
    let archives = [
        to_write("refs/archive/fix/2026-11-19T091200Z", oid('a')),
        to_write("refs/archive/same/2026-11-19T091200Z", oid('a')),
        to_write("refs/archive/other/2026-11-19T091200Z", oid('b')),
    ];
    let answer = |what: &str, ok: Completed| match broken.contains(&what) {
        true => failed(&format!("fatal: {what} broke\n")),
        false => ok,
    };
    let tip_of = |reference: &str, tip: Oid| match (
        broken.contains(&reference),
        broken.contains(&format!("missing {reference}").as_str()),
    ) {
        (_, true) => failed(""),
        (true, _) => Completed::ok(format!("{}\n", oid('f'))),
        _ => Completed::ok(format!("{tip}\n")),
    };
    let system = archives.iter().fold(
        system()
            .with_git(BACKUP, FSCK, answer("fsck", Completed::ok("")))
            .with_git(
                BACKUP,
                rev_list(oid('a')),
                answer("history a", Completed::ok("")),
            )
            .with_git(
                BACKUP,
                rev_list(oid('b')),
                answer("history b", Completed::ok("")),
            ),
        |system, a| system.with_git(BACKUP, rev_parse(&a.name), tip_of(&a.name, a.oid)),
    );
    (system, archives)
}

#[test]
fn test_every_archive_passes() {
    let (system, archives) = backup_system(&[]);
    let compares = compare(&system, Path::new(BACKUP), &archives);
    assert!(
        compares.iter().all(|c| c.ok && c.detail.is_none()),
        "{compares:?}"
    );
    assert_eq!(compares.len(), 3);
    assert_eq!(system.acts, []);
}

#[test]
fn test_a_failed_fsck_fails_every_archive() {
    let (system, archives) = backup_system(&["fsck"]);
    let compares = compare(&system, Path::new(BACKUP), &archives);
    assert!(compares.iter().all(|c| !c.ok), "{compares:?}");
    assert_eq!(
        compares[0].detail,
        Some(format!(
            "git --git-dir {BACKUP} fsck --connectivity-only --no-dangling --no-progress \
             exited 128: fatal: fsck broke"
        ))
    );
}

#[test]
fn test_a_missing_history_fails_the_archives_of_that_tip() {
    let (system, archives) = backup_system(&["history a"]);
    let report = backup_report(
        Path::new(BACKUP),
        0,
        compare(&system, Path::new(BACKUP), &archives),
    );
    let oks: Vec<bool> = report.compares.iter().map(|c| c.ok).collect();
    assert_eq!(oks, [false, false, true]);
    assert_eq!((report.verified, report.failed), (1, 2));
    assert_eq!(
        report.compares[0].detail,
        Some(format!(
            "git --git-dir {BACKUP} rev-list --objects --missing=error --quiet {} \
             exited 128: fatal: history a broke",
            oid('a')
        ))
    );
}

#[test]
fn test_a_ref_at_another_tip_fails() {
    let other = "refs/archive/other/2026-11-19T091200Z";
    let (system, archives) = backup_system(&[other]);
    let compares = compare(&system, Path::new(BACKUP), &archives);
    assert!(compares[0].ok && compares[1].ok);
    assert_eq!(
        compares[2].detail,
        Some(format!("{other} is at {}, not {}", oid('f'), oid('b')))
    );
}

/// Writes the archives of [`backup_system`] with `broken` checks, the
/// push succeeding.
fn written(broken: &[&str]) -> (BackupReport, Fake) {
    let (system, archives) = backup_system(broken);
    let pushed = push(&archives);
    let mut system = system
        .with_file(format!("{BACKUP}/HEAD"), "ref: refs/heads/main\n")
        .with_git(GIT_DIR, &pushed, Completed::ok(""));
    let report = write_and_compare(
        &mut system,
        Path::new(GIT_DIR),
        Path::new(BACKUP),
        &archives,
    );
    assert_eq!(system.acts.last(), Some(&git(GIT_DIR, &pushed)));
    (report, system)
}

#[test]
fn test_written_archives_are_compared() {
    let (report, _) = written(&[]);
    assert_eq!((report.archived, report.verified, report.failed), (3, 3, 0));
}

#[test]
fn test_a_written_archive_the_backup_lacks_fails() {
    let fix = "refs/archive/fix/2026-11-19T091200Z";
    let (report, _) = written(&[&format!("missing {fix}")]);
    assert_eq!((report.archived, report.verified, report.failed), (3, 2, 1));
    assert_eq!(report.compares[0].detail, Some(format!("{fix} is missing")));
}

#[test]
fn test_a_written_archive_whose_history_is_missing_fails() {
    let (report, _) = written(&["history b"]);
    assert_eq!((report.archived, report.verified, report.failed), (3, 2, 1));
    assert!(!report.compares[2].ok);
}

// Restoring --------------------------------------------------------------------------

const FETCH: [&str; 7] = [
    "fetch",
    "--quiet",
    "--no-tags",
    "--no-write-fetch-head",
    "--no-recurse-submodules",
    BACKUP,
    "refs/archive/fix/2026-11-19T091200Z",
];

#[test]
fn test_restore_fetches_the_archive_then_creates_the_branch() {
    let chosen = archive("refs/archive/fix/2026-11-19T091200Z", oid('a'));
    let fetch = FETCH;
    let tip = oid('a').to_string();
    let branch = ["update-ref", "refs/heads/fix", &tip, ""];
    let mut system = system()
        .with_git(GIT_DIR, fetch, Completed::ok(""))
        .with_git(GIT_DIR, branch, Completed::ok(""));
    assert_eq!(
        restore(
            &mut system,
            Path::new(GIT_DIR),
            Path::new(BACKUP),
            "fix",
            &chosen
        ),
        Ok(())
    );
    assert_eq!(system.acts, [git(GIT_DIR, fetch), git(GIT_DIR, branch)]);
}

#[test]
fn test_a_failed_fetch_creates_no_branch() {
    let chosen = archive("refs/archive/fix/2026-11-19T091200Z", oid('a'));
    let fetch = FETCH;
    let mut system = system().with_git(GIT_DIR, fetch, failed("fatal: no such ref\n"));
    assert!(
        restore(
            &mut system,
            Path::new(GIT_DIR),
            Path::new(BACKUP),
            "fix",
            &chosen
        )
        .is_err()
    );
    assert_eq!(system.acts, [git(GIT_DIR, fetch)]);
}

#[test]
fn test_root_of_a_backup() {
    assert_eq!(
        root_of(Path::new(BACKUP)),
        Some(PathBuf::from("/h/backups/git-archive").as_path())
    );
}
