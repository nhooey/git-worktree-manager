use std::path::{Path, PathBuf};

use super::*;

fn environment(pairs: &[(&str, &str)]) -> Environment {
    Environment::from_pairs(pairs.iter().copied())
}

#[test]
fn test_the_case_conversions() {
    assert_eq!(screaming_snake("worktree-manager"), "WORKTREE_MANAGER");
    assert_eq!(lower_camel("worktree-manager"), "worktreeManager");
    assert_eq!(lower_camel("a-b-c"), "aBC");
    assert_eq!(lower_camel("single"), "single");
}

#[test]
fn test_names_derive_from_the_program() {
    assert_eq!(format!("git-{}", subcommand()), PROGRAM);
    assert_eq!(
        Variable::Log.name(),
        format!("{}_LOG", screaming_snake(subcommand()))
    );
    assert_eq!(
        Variable::FloorCleanupOnlyGb.name(),
        format!("{}_FLOOR_CLEANUP_ONLY_GB", screaming_snake(subcommand()))
    );
    assert_eq!(
        SettingKey::LocalOnlyReport.config_key(),
        format!("{}.localOnlyReport", lower_camel(subcommand()))
    );
}

#[test]
fn test_repository_folders() {
    assert_eq!(repository_name(Path::new("/x/app/.git")), "app");
    assert_eq!(repository_name(Path::new("/x/app.git")), "app");
    assert_eq!(repository_name(Path::new("/x/app")), "app");
    let common = Path::new("/x/app/.git");
    let key = repository_key(common);
    assert_eq!(key, format!("app-{}", hash8(common)));
    assert_eq!(hash8(common).len(), 8);
    assert_ne!(hash8(common), hash8(Path::new("/y/app/.git")));

    let home = environment(&[("HOME", "/h")]);
    assert_eq!(
        state_dir(&home, common),
        Some(PathBuf::from(format!("/h/.local/state/{PROGRAM}/{key}")))
    );
    assert_eq!(
        build_root(&home, common),
        Some(PathBuf::from(format!("/h/.cache/{PROGRAM}/{key}")))
    );
    assert_eq!(
        floor_file(&home),
        Some(PathBuf::from(format!("/h/.config/{PROGRAM}/floor")))
    );
    assert_eq!(
        backup_root(&home),
        Some(PathBuf::from("/h/backups/git-archive"))
    );
}

#[test]
fn test_xdg_folders_win_and_an_empty_one_counts_as_unset() {
    let common = Path::new("/x/app/.git");
    let key = repository_key(common);
    let xdg = environment(&[
        ("HOME", "/h"),
        ("XDG_STATE_HOME", "/s"),
        ("XDG_CACHE_HOME", ""),
    ]);
    assert_eq!(
        state_dir(&xdg, common),
        Some(PathBuf::from(format!("/s/{PROGRAM}/{key}")))
    );
    assert_eq!(
        build_root(&xdg, common),
        Some(PathBuf::from(format!("/h/.cache/{PROGRAM}/{key}")))
    );
    assert_eq!(state_dir(&environment(&[]), common), None);
}
