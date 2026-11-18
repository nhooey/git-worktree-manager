//! A worktree's ignored entries, read through gitoxide, and their split.

mod sandbox;

use std::path::{Path, PathBuf};

use git_worktree_manager::keep::{self, Ignored, IgnoredKind};
use git_worktree_manager::system::Real;
use sandbox::{Sandbox, join};

fn entry(path: &str, kind: IgnoredKind) -> Ignored {
    Ignored {
        path: path.into(),
        kind,
    }
}

#[test]
fn test_ignored_entries_are_listed_file_by_file() {
    let s = Sandbox::new();
    let repo = s.repo(&[("src/main.rs", "fn main() {}\n")]);
    let wt = s.add_worktree(&repo, "feat");
    for (rel, text) in [
        (".env", "A=1\n"),
        (".claude/settings.local.json", "{}\n"),
        (".claude/other.json", "{}\n"),
        ("target/debug/app", "binary"),
        ("target/debug/.env", "nested\n"),
        ("untracked.txt", "not ignored\n"),
    ] {
        s.write(&join(&wt, rel), text);
    }
    s.sh(&["git", "init", "--quiet", &join(&wt, "nested/lib")], None);
    s.write(&join(&wt, "nested/lib/file"), "x");
    std::os::unix::fs::symlink("/nowhere", join(&wt, "target/latest")).expect("a link");

    let system = Real::new(s.environment());
    let entries = keep::ignored(&system, Path::new(&wt)).expect("readable");
    assert_eq!(
        entries,
        vec![
            entry(".claude/other.json", IgnoredKind::File),
            entry(".claude/settings.local.json", IgnoredKind::File),
            entry(".env", IgnoredKind::File),
            entry("nested/lib", IgnoredKind::Repository),
            entry("target/debug/.env", IgnoredKind::File),
            entry("target/debug/app", IgnoredKind::File),
            entry("target/latest", IgnoredKind::Link),
        ]
    );

    s.write(&join(&repo, ".env"), "A=1\n");
    let patterns = keep::read_patterns(
        &system,
        Path::new(&wt),
        Some(Path::new(&repo)),
        keep::ignore_case(Path::new(&wt)).expect("readable"),
    )
    .expect("patterns");
    let same = |e: &Ignored| {
        std::fs::read(Path::new(&wt).join(&e.path)).ok()
            == std::fs::read(Path::new(&repo).join(&e.path)).ok()
    };
    let split = keep::split(&entries, &patterns, same);
    let paths = |items: &[&str]| items.iter().map(PathBuf::from).collect::<Vec<_>>();
    assert_eq!(
        split,
        keep::Split {
            kept: paths(&[
                ".claude/settings.local.json",
                ".env",
                "nested/lib",
                "target/debug/.env"
            ]),
            own: paths(&[
                ".claude/settings.local.json",
                "nested/lib",
                "target/debug/.env"
            ]),
            build_output: paths(&[".claude/other.json", "target/debug/app", "target/latest"]),
        }
    );
}

#[test]
fn test_keep_patterns_fold_case_where_the_repository_ignores_it() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    s.write(&join(&repo, ".git/info/exclude"), "handoff.md\n");
    s.write(&join(&repo, "handoff.md"), "notes\n");
    let system = Real::new(s.environment());
    let split_with = |ignore_case: &str| {
        s.git(&repo, &["config", "core.ignoreCase", ignore_case]);
        let worktree = Path::new(&repo);
        let entries = keep::ignored(&system, worktree).expect("readable");
        let case = keep::ignore_case(worktree).expect("readable");
        let patterns = keep::read_patterns(&system, worktree, None, case).expect("patterns");
        keep::split(&entries, &patterns, |_| false)
    };
    assert_eq!(split_with("true").kept, [PathBuf::from("handoff.md")]);
    assert_eq!(
        split_with("false").build_output,
        [PathBuf::from("handoff.md")]
    );
}
