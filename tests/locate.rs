//! Locating a path's repository, over sandbox repositories.

mod sandbox;

use std::path::{Path, PathBuf};

use git_worktree_manager::environment::Environment;
use git_worktree_manager::locate::locate;
use git_worktree_manager::system::Real;
use sandbox::{Sandbox, join};

fn with(s: &Sandbox, pairs: &[(&str, &str)]) -> Real {
    Real::new(Environment(
        s.environment()
            .0
            .into_iter()
            .chain(
                pairs
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
            )
            .collect(),
    ))
}

#[test]
fn test_a_main_checkout_and_a_linked_worktree() {
    let s = Sandbox::new();
    let system = with(&s, &[]);
    let repo = s.repo(&[]);
    let feat = s.add_worktree(&repo, "feat");
    let main = locate(&system, Path::new(&repo))
        .expect("readable")
        .expect("a repository");
    assert_eq!(main.common_dir, PathBuf::from(join(&repo, ".git")));
    assert_eq!(main.git_dir, main.common_dir);
    assert_eq!(main.worktree, Some(PathBuf::from(&repo)));
    let linked = locate(&system, Path::new(&feat))
        .expect("readable")
        .expect("a repository");
    assert_eq!(linked.common_dir, main.common_dir);
    assert_eq!(
        linked.git_dir,
        PathBuf::from(join(&repo, ".git/worktrees/feat"))
    );
    assert_eq!(linked.worktree, Some(PathBuf::from(&feat)));
    assert_eq!(
        locate(&system, Path::new(&join(&feat, "sub/..")))
            .expect("readable")
            .and_then(|found| found.worktree),
        Some(PathBuf::from(&feat)),
        "`..` is taken after the parts before it"
    );
}

#[test]
fn test_outside_a_repository_and_inside_a_git_directory() {
    let s = Sandbox::new();
    let system = with(&s, &[]);
    let repo = s.repo(&[]);
    let plain = join(&s.tmp, "plain");
    std::fs::create_dir_all(&plain).expect("a folder");
    let none = |path: &str| locate(&system, Path::new(path));
    assert_eq!(none(&plain), Ok(None));
    assert_eq!(none(&join(&repo, ".git")), Ok(None));
    assert_eq!(none(&join(&repo, "missing")), Ok(None));
}

#[test]
fn test_git_dir_cannot_point_a_path_at_another_worktree() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let feat = s.add_worktree(&repo, "feat");
    let system = with(&s, &[("GIT_DIR", &join(&repo, ".git/worktrees/feat"))]);
    assert!(
        locate(&system, Path::new(&repo)).is_err(),
        "GIT_DIR=<main>/.git/worktrees/feat with <main> is refused"
    );
    assert_eq!(
        locate(&system, Path::new(&feat))
            .expect("readable")
            .and_then(|found| found.worktree),
        Some(PathBuf::from(&feat))
    );
}

#[test]
fn test_a_bare_repository_has_no_worktree() {
    let s = Sandbox::new();
    let bare = s.bare(&join(&s.tmp, "bare.git"));
    let found = locate(&with(&s, &[]), Path::new(&bare))
        .expect("readable")
        .expect("a repository");
    assert_eq!(found.common_dir, PathBuf::from(&bare));
    assert_eq!(found.worktree, None);
}
