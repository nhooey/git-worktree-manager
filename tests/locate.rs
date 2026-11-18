//! Locating a path's repository, over sandbox repositories, and the
//! repositories a manifest's entries name, through the binary.

mod sandbox;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use git_worktree_manager::environment::Environment;
use git_worktree_manager::locate::{Reach, locate};
use git_worktree_manager::system::Real;
use sandbox::{Sandbox, exists, join, real};
use serde_json::{Value, json};

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

const REACHES: [Reach; 2] = [Reach::Upward, Reach::Here];

#[test]
fn test_a_main_checkout_and_a_linked_worktree() {
    let s = Sandbox::new();
    let system = with(&s, &[]);
    let repo = s.repo(&[]);
    let feat = s.add_worktree(&repo, "feat");
    for reach in REACHES {
        let main = locate(&system, Path::new(&repo), reach)
            .expect("readable")
            .expect("a repository");
        assert_eq!(main.common_dir, PathBuf::from(join(&repo, ".git")));
        assert_eq!(main.git_dir, main.common_dir);
        assert_eq!(main.worktree, Some(PathBuf::from(&repo)));
        let linked = locate(&system, Path::new(&feat), reach)
            .expect("readable")
            .expect("a repository");
        assert_eq!(linked.common_dir, main.common_dir);
        assert_eq!(
            linked.git_dir,
            PathBuf::from(join(&repo, ".git/worktrees/feat"))
        );
        assert_eq!(linked.worktree, Some(PathBuf::from(&feat)));
        assert_eq!(
            locate(&system, Path::new(&join(&feat, "sub/..")), reach)
                .expect("readable")
                .and_then(|found| found.worktree),
            Some(PathBuf::from(&feat)),
            "`..` is taken after the parts before it"
        );
    }
}

#[test]
fn test_outside_a_repository_and_inside_a_git_directory() {
    let s = Sandbox::new();
    let system = with(&s, &[]);
    let repo = s.repo(&[]);
    let plain = join(&s.tmp, "plain");
    fs::create_dir_all(&plain).expect("a folder");
    for reach in REACHES {
        let none = |path: &str| locate(&system, Path::new(path), reach);
        assert_eq!(none(&plain), Ok(None), "{reach:?}");
        assert_eq!(none(&join(&repo, "missing")), Ok(None), "{reach:?}");
    }
    let git_dir = join(&repo, ".git");
    assert_eq!(
        locate(&system, Path::new(&git_dir), Reach::Upward),
        Ok(None)
    );
    assert!(
        locate(&system, Path::new(&git_dir), Reach::Here).is_err(),
        "a worktree's git directory names no clone of its own"
    );
}

#[test]
fn test_only_upward_finds_the_repository_around_a_folder() {
    let s = Sandbox::new();
    let system = with(&s, &[]);
    let repo = s.repo(&[]);
    let sub = join(&repo, "sub");
    fs::create_dir_all(&sub).expect("a folder");
    let common = |reach| {
        locate(&system, Path::new(&sub), reach)
            .expect("readable")
            .map(|found| found.common_dir)
    };
    assert_eq!(
        common(Reach::Upward),
        Some(PathBuf::from(join(&repo, ".git")))
    );
    assert_eq!(common(Reach::Here), None);
    s.write(&join(&sub, ".git"), "rubbish\n");
    assert!(locate(&system, Path::new(&sub), Reach::Here).is_err());
}

#[test]
fn test_git_dir_cannot_point_a_path_at_another_worktree() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let feat = s.add_worktree(&repo, "feat");
    let system = with(&s, &[("GIT_DIR", &join(&repo, ".git/worktrees/feat"))]);
    assert!(
        locate(&system, Path::new(&repo), Reach::Upward).is_err(),
        "GIT_DIR=<main>/.git/worktrees/feat with <main> is refused"
    );
    assert_eq!(
        locate(&system, Path::new(&feat), Reach::Upward)
            .expect("readable")
            .and_then(|found| found.worktree),
        Some(PathBuf::from(&feat))
    );
}

#[test]
fn test_a_bare_repository_has_no_worktree() {
    let s = Sandbox::new();
    let bare = s.bare(&join(&s.tmp, "bare.git"));
    for reach in REACHES {
        let found = locate(&with(&s, &[]), Path::new(&bare), reach)
            .expect("readable")
            .expect("a repository");
        assert_eq!(found.common_dir, PathBuf::from(&bare));
        assert_eq!(found.worktree, None);
    }
}

// Manifest entries ------------------------------------------------------------------

/// A clone, in the folder `name`, of a bare remote `<name>.remote.git`,
/// with `origin/HEAD` at `main`, a worktree `landed` on `main`'s tip that
/// a sweep removes, and no `idle` wait.
fn cloned_with_landed(s: &Sandbox, name: &str) -> String {
    let remote = s.bare(&join(&s.tmp, &format!("{name}.remote.git")));
    s.git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    let path = s.repo_named(name, &[]);
    s.git(&path, &["remote", "add", "origin", &remote]);
    s.git(
        &path,
        &["push", "--quiet", "--set-upstream", "origin", "main"],
    );
    s.git(&path, &["remote", "set-head", "origin", "main"]);
    s.git(&path, &["config", "worktreeManager.idle", "0s"]);
    s.add_worktree(&path, "landed");
    path
}

/// A bare clone of `from` at `path`, broken by `breaking`.
fn bare_broken_by(s: &Sandbox, from: &str, path: &str, breaking: impl FnOnce(&str)) -> String {
    s.sh(&["git", "clone", "--quiet", "--bare", from, path], None);
    breaking(path);
    path.to_owned()
}

/// A manifest's six entries: a clone, a path with no clone, a plain
/// folder inside another repository, a clone whose `.git` has no `HEAD`,
/// a bare clone with no `HEAD`, and a bare clone with only `objects` and
/// `packed-refs`. The other repository has a landed worktree a sweep
/// would remove and an unmerged branch `archive write` would archive, and
/// the folder inside it names `main` as its default branch, so a run that
/// read that repository for the folder would act there.
struct Entries {
    clone: String,
    missing: String,
    inside: String,
    headless: String,
    broken_bare: String,
    packed: String,
    outer: String,
    manifest: String,
}

fn entries(s: &Sandbox) -> Entries {
    let clone = s.cloned();
    let outer = cloned_with_landed(s, "outer");
    s.git(&outer, &["checkout", "--quiet", "-b", "wip"]);
    s.commit(&outer, "wip.txt", "only on wip\n");
    s.git(&outer, &["checkout", "--quiet", "main"]);
    let inside = join(&outer, "alpha");
    s.write(&join(&inside, "notes.txt"), "not a clone\n");
    let headless = s.repo_named("headless", &[]);
    fs::remove_file(join(&headless, ".git/HEAD")).expect("HEAD goes");
    let no_head = |bare: &str| fs::remove_file(join(bare, "HEAD")).expect("HEAD goes");
    let broken_bare = bare_broken_by(s, &clone, &join(&s.tmp, "broken.git"), no_head);
    let packed = bare_broken_by(s, &clone, &join(&s.tmp, "packed.git"), |bare| {
        s.git(bare, &["pack-refs", "--all"]);
        no_head(bare);
        fs::remove_dir_all(join(bare, "refs")).expect("refs go");
    });
    let missing = join(&s.tmp, "missing");
    let manifest = join(&s.tmp, "repositories.json");
    let listed = json!([
        { "path": clone },
        { "path": missing },
        { "path": inside, "default_branch": "main" },
        { "path": headless },
        { "path": broken_bare },
        { "path": packed },
    ]);
    s.write(&manifest, &listed.to_string());
    Entries {
        clone,
        missing,
        inside,
        headless,
        broken_bare,
        packed,
        outer,
        manifest,
    }
}

/// The other repository's refs and worktrees.
fn untouched(s: &Sandbox, outer: &str) -> (String, String) {
    (
        s.git(outer, &["for-each-ref"]),
        s.git(outer, &["worktree", "list", "--porcelain"]),
    )
}

/// Each repository in the report by its path, with its problem's code.
fn codes(report: &Value) -> BTreeMap<String, Value> {
    report["repositories"]
        .as_array()
        .expect("repositories")
        .iter()
        .map(|r| {
            let path = r["path"].as_str().expect("a path").to_owned();
            (path, r["problem"]["code"].clone())
        })
        .collect()
}

/// The clone read, the missing path and the folder skipped, the clones
/// with no `HEAD` unreadable, and the other repository nowhere.
fn assert_read_as_named(report: &Value, e: &Entries) {
    let expected: BTreeMap<String, Value> = [
        (&e.clone, Value::Null),
        (&e.missing, "skipped".into()),
        (&e.inside, "skipped".into()),
        (&e.headless, "unreadable".into()),
        (&e.broken_bare, "unreadable".into()),
        (&e.packed, "unreadable".into()),
    ]
    .into_iter()
    .map(|(path, code)| (path.clone(), code))
    .collect();
    assert_eq!(codes(report), expected, "{report:#}");
    let clone = report["repositories"]
        .as_array()
        .and_then(|all| all.iter().find(|r| r["path"] == e.clone.as_str()))
        .expect("the clone");
    assert_eq!(clone["common_dir"], join(&e.clone, ".git"));
    let text = report.to_string();
    assert!(!text.contains(&join(&e.outer, ".git")), "{report:#}");
}

#[test]
fn test_status_reads_each_manifest_entry_as_its_own_clone() {
    let s = Sandbox::new();
    let e = entries(&s);
    let before = untouched(&s, &e.outer);
    let report = s.run_json(&["status", "--repositories", &e.manifest], None, 1);
    assert_read_as_named(&report, &e);
    assert_eq!(untouched(&s, &e.outer), before);
}

#[test]
fn test_sweep_never_acts_on_a_repository_around_a_manifest_entry() {
    let s = Sandbox::new();
    let e = entries(&s);
    let before = untouched(&s, &e.outer);
    let report = s.run_json(&["sweep", "--repositories", &e.manifest], None, 1);
    assert_read_as_named(&report, &e);
    assert_eq!(untouched(&s, &e.outer), before);
}

#[test]
fn test_archive_reads_each_manifest_entry_as_its_own_clone() {
    let s = Sandbox::new();
    let e = entries(&s);
    let before = untouched(&s, &e.outer);
    for args in [
        &["archive", "write", "--dry-run"][..],
        &["archive", "write"],
        &["archive", "verify"],
    ] {
        let args: Vec<&str> = args
            .iter()
            .copied()
            .chain(["--repositories", &e.manifest])
            .collect();
        let report = s.run_json(&args, None, 1);
        assert_read_as_named(&report, &e);
    }
    assert_eq!(untouched(&s, &e.outer), before);
    let outer_backup = join(&s.var("HOME"), "backups/git-archive/local/outer.git");
    assert!(!exists(&outer_backup), "{outer_backup}");
}

/// A manifest of `paths`; its path.
fn manifest_of(s: &Sandbox, paths: &[&str]) -> String {
    let manifest = join(&s.tmp, "repositories.json");
    let listed: Vec<Value> = paths.iter().map(|path| json!({ "path": path })).collect();
    s.write(&manifest, &Value::from(listed).to_string());
    manifest
}

/// `status` over a manifest of `paths`, with `env` added: each
/// repository's path, common directory, problem code and worktrees, each
/// worktree as its path and whether it is the main one, in the manifest's
/// order.
fn status_of(s: &Sandbox, paths: &[&str], env: &[(&str, &str)]) -> Vec<Value> {
    let manifest = manifest_of(s, paths);
    let args = ["status", "--repositories", &manifest, "--format", "json"];
    let ran = s.run(&args, None, env);
    assert_eq!(ran.code, 0, "{}{}", ran.stderr, ran.stdout);
    let report: Value = serde_json::from_str(&ran.stdout).expect("JSON");
    report["repositories"]
        .as_array()
        .expect("repositories")
        .iter()
        .map(|r| {
            let worktrees: Vec<Value> = r["worktrees"]
                .as_array()
                .expect("worktrees")
                .iter()
                .map(|w| json!([w["path"], w["main"]]))
                .collect();
            json!({
                "path": r["path"],
                "common_dir": r["common_dir"],
                "problem": r["problem"]["code"],
                "worktrees": worktrees,
            })
        })
        .collect()
}

/// The `GIT_DIR` and `GIT_WORK_TREE` that name another clone, one with a
/// landed worktree, and that clone.
fn named_by_git_variables(s: &Sandbox) -> (Vec<(&'static str, String)>, String) {
    let other = cloned_with_landed(s, "other");
    let variables = vec![
        ("GIT_DIR", join(&other, ".git")),
        ("GIT_WORK_TREE", other.clone()),
    ];
    (variables, other)
}

#[test]
fn test_a_bare_clone_and_worktrees_named_in_a_manifest_are_read() {
    let s = Sandbox::new();
    let clone = s.cloned();
    let feat = s.add_worktree(&clone, "feat");
    let bare = real(&join(&s.tmp, "repo.remote.git"));
    let checkout = json!({
        "path": clone,
        "common_dir": join(&clone, ".git"),
        "problem": null,
        "worktrees": [[clone, true], [feat, false]],
    });
    let expected = [
        checkout.clone(),
        checkout,
        json!({
            "path": bare,
            "common_dir": bare,
            "problem": "no-default-branch",
            "worktrees": [],
        }),
    ];
    let paths = [clone.as_str(), &feat, &bare];
    assert_eq!(status_of(&s, &paths, &[]), expected);
    let (variables, _) = named_by_git_variables(&s);
    let named: Vec<(&str, &str)> = variables.iter().map(|(k, v)| (*k, v.as_str())).collect();
    assert_eq!(
        status_of(&s, &paths, &named),
        expected,
        "git's variables name no manifest entry's clone"
    );
}

#[test]
fn test_a_sweep_acts_on_each_entry_whatever_git_variables_name() {
    let s = Sandbox::new();
    let entry = cloned_with_landed(&s, "entry");
    let landed = join(&format!("{entry}.worktrees"), "landed");
    let (variables, other) = named_by_git_variables(&s);
    let before = untouched(&s, &other);
    let manifest = manifest_of(&s, &[&entry]);
    let named: Vec<(&str, &str)> = variables.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let ran = s.run(&["sweep", "--repositories", &manifest], None, &named);
    assert_eq!(ran.code, 0, "{}{}", ran.stderr, ran.stdout);
    assert_eq!(untouched(&s, &other), before);
    assert!(!exists(&landed), "the entry is swept: {}", ran.stdout);
}

#[test]
fn test_a_submodule_a_link_and_a_trailing_slash_name_their_clones() {
    let s = Sandbox::new();
    let clone = s.cloned();
    let library = s.repo_named("library", &[("lib.txt", "lib\n")]);
    let parent = s.repo_named("parent", &[]);
    s.git(
        &parent,
        &["submodule", "add", "--quiet", &library, "module"],
    );
    let module = join(&parent, "module");
    let link = join(&s.tmp, "link");
    std::os::unix::fs::symlink(&clone, &link).expect("a link");
    let slashed = format!("{clone}/");
    let common = Value::from(join(&clone, ".git"));
    let found: Vec<Value> = status_of(&s, &[&module, &link, &slashed], &[])
        .into_iter()
        .map(|summary| summary["common_dir"].clone())
        .collect();
    assert_eq!(
        found,
        [
            Value::from(join(&parent, ".git/modules/module")),
            common.clone(),
            common,
        ]
    );
}

#[test]
fn test_a_named_subfolder_of_a_clone_finds_the_clone() {
    let s = Sandbox::new();
    let clone = s.cloned();
    let sub = join(&clone, "sub");
    s.write(&join(&sub, "notes.txt"), "in the clone\n");
    let report = s.run_json(&["status", "--repository", &sub], None, 0);
    assert_eq!(
        report["repositories"][0]["common_dir"],
        join(&clone, ".git")
    );
}

// Configuration ---------------------------------------------------------------------

/// The action `status` plans for the worktree at `worktree` of `repo`,
/// with `env` added.
fn planned_action(s: &Sandbox, repo: &str, worktree: &str, env: &[(&str, &str)]) -> Value {
    let ran = s.run(
        &["status", "--repository", repo, "--format", "json"],
        None,
        env,
    );
    assert_eq!(ran.code, 0, "{}{}", ran.stderr, ran.stdout);
    let report: Value = serde_json::from_str(&ran.stdout).expect("JSON");
    report["repositories"][0]["worktrees"]
        .as_array()
        .and_then(|all| all.iter().find(|w| w["path"] == worktree))
        .map(|w| w["action"].clone())
        .expect("the worktree")
}

#[test]
fn test_git_config_variables_change_no_plan() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let landed = s.add_worktree(&repo, "landed");
    let elsewhere = join(&s.tmp, "elsewhere.gitconfig");
    s.write(&elsewhere, "[worktreeManager]\n\tidle = 0s\n");
    let kept = planned_action(&s, &repo, &landed, &[]);
    assert_eq!(kept, "keep");
    for env in [
        &[("GIT_CONFIG_GLOBAL", elsewhere.as_str())][..],
        &[
            ("GIT_CONFIG_COUNT", "1"),
            ("GIT_CONFIG_KEY_0", "worktreeManager.idle"),
            ("GIT_CONFIG_VALUE_0", "0s"),
        ],
    ] {
        assert_eq!(planned_action(&s, &repo, &landed, env), kept, "{env:?}");
    }
    s.git(&repo, &["config", "worktreeManager.idle", "0s"]);
    assert_eq!(planned_action(&s, &repo, &landed, &[]), "remove");
}

#[test]
fn test_a_git_child_reads_no_config_file_git_config_global_names() {
    use std::os::unix::fs::PermissionsExt;
    let s = Sandbox::new();
    let repo = cloned_with_landed(&s, "entry");
    let landed = join(&format!("{repo}.worktrees"), "landed");
    let marker = join(&s.tmp, "hook-ran");
    let hooks = join(&s.tmp, "hooks");
    let hook = join(&hooks, "reference-transaction");
    s.write(&hook, &format!("#!/bin/sh\ntouch '{marker}'\n"));
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).expect("runnable");
    let elsewhere = join(&s.tmp, "elsewhere.gitconfig");
    s.write(&elsewhere, &format!("[core]\n\thooksPath = {hooks}\n"));
    let ran = s.run(
        &["sweep", "--repository", &repo],
        None,
        &[("GIT_CONFIG_GLOBAL", &elsewhere)],
    );
    assert_eq!(ran.code, 0, "{}{}", ran.stderr, ran.stdout);
    assert!(!exists(&landed), "the sweep acted: {}", ran.stdout);
    assert!(!exists(&marker), "a git child read {elsewhere}");
}
