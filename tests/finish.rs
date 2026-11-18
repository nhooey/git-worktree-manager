//! `finish`, run as the binary: a landed worktree ends; one with work of
//! its own stays with exit 3 unless abandoned; an abandoned one is
//! archived and compared before anything goes; the main and a locked
//! worktree stay.

mod sandbox;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use git_worktree_manager::names::{self, EVENTS_FILE, LAST_SWEEP_FILE, SWEEP_PENDING_FILE};
use sandbox::{Sandbox, exists, join, real};
use serde_json::Value;

fn state(s: &Sandbox, repo: &str) -> PathBuf {
    let common = s.git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    );
    let state = names::state_dir(&s.environment(), Path::new(&real(common.trim())))
        .expect("a state folder");
    assert!(state.starts_with(&s.tmp), "{}", state.display());
    state
}

/// `finish <args> --format json` in `cwd`, expecting `code`.
fn finish(s: &Sandbox, cwd: &str, args: &[&str], code: i32) -> (Value, String) {
    let args: Vec<&str> = ["finish"]
        .into_iter()
        .chain(args.iter().copied())
        .chain(["--format", "json"])
        .collect();
    let ran = s.run(&args, Some(cwd), &[]);
    assert_eq!(ran.code, code, "{}{}", ran.stderr, ran.stdout);
    (serde_json::from_str(&ran.stdout).expect("JSON"), ran.stderr)
}

fn has_branch(s: &Sandbox, repo: &str, name: &str) -> bool {
    !s.git(repo, &["branch", "--list", name]).trim().is_empty()
}

fn backup(report: &Value) -> String {
    report["repositories"][0]["backup"]["path"]
        .as_str()
        .expect("a backup")
        .to_owned()
}

#[test]
fn test_a_landed_worktree_and_its_branch_end_without_the_idle_test() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let landed = s.add_worktree(&repo, "landed");
    let (report, stderr) = finish(&s, &repo, &[&landed], 0);
    assert_eq!(stderr, "");
    let worktree = &report["repositories"][0]["worktrees"][0];
    assert_eq!(
        (&worktree["action"], &worktree["outcome"]),
        (&Value::from("remove"), &Value::from("done"))
    );
    assert!(!exists(&landed));
    assert!(!has_branch(&s, &repo, "landed"));
}

#[test]
fn test_finish_run_inside_the_worktree_finishes_it() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let landed = s.add_worktree(&repo, "landed");
    finish(&s, &landed, &[&landed], 0);
    assert!(!exists(&landed));
    assert!(!has_branch(&s, &repo, "landed"));
}

#[test]
fn test_finish_with_git_dir_naming_the_worktree_finishes_it() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let landed = s.add_worktree(&repo, "landed");
    let git_dir = join(&repo, ".git/worktrees/landed");
    let ran = s.run(
        &["finish", &landed, "--format", "json"],
        Some(&repo),
        &[("GIT_DIR", &git_dir)],
    );
    assert_eq!(ran.code, 0, "{}{}", ran.stderr, ran.stdout);
    assert!(!exists(&landed));
    assert!(!has_branch(&s, &repo, "landed"));
}

#[test]
fn test_a_dry_run_finish_changes_nothing() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let landed = s.add_worktree(&repo, "landed");
    let (report, _) = finish(&s, &repo, &[&landed, "--dry-run"], 0);
    assert_eq!(report["dry_run"], true);
    assert_eq!(
        report["repositories"][0]["worktrees"][0]["outcome"],
        "planned"
    );
    assert!(exists(&landed) && has_branch(&s, &repo, "landed"));
    assert!(!state(&s, &repo).exists());
}

#[test]
fn test_a_worktree_with_its_own_commit_exits_3_and_names_not_landed() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let feat = s.add_worktree(&repo, "feat");
    s.commit(&feat, "feat.txt", "feat\n");
    let (report, stderr) = finish(&s, &feat, &[&feat], 3);
    assert!(
        stderr.contains(&format!("{feat} stays: not-landed:")),
        "{stderr}"
    );
    assert_eq!(report["repositories"][0]["worktrees"][0]["action"], "keep");
    assert!(exists(&feat) && has_branch(&s, &repo, "feat"));
}

#[test]
fn test_abandon_archives_with_the_reason_compares_then_removes_and_deletes() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let feat = s.add_worktree(&repo, "feat");
    let tip = s.commit(&feat, "feat.txt", "feat\n");
    let (report, stderr) = finish(
        &s,
        &repo,
        &[&feat, "--abandon", "--reason", "went another way"],
        0,
    );
    assert_eq!(stderr, "");
    let branch = &report["repositories"][0]["branches"][0];
    assert_eq!(
        (
            &branch["action"],
            &branch["outcome"],
            &branch["archive"]["state"]
        ),
        (
            &Value::from("delete"),
            &Value::from("done"),
            &Value::from("verified")
        ),
        "{branch:#}"
    );
    let reference = branch["archive"]["ref"].as_str().expect("an archive ref");
    assert_eq!(
        s.sh(
            &["git", "--git-dir", &backup(&report), "rev-parse", reference],
            None
        )
        .trim(),
        tip
    );
    assert!(!exists(&feat));
    assert!(!has_branch(&s, &repo, "feat"));
    let logged: Vec<Value> = sandbox::read(&state(&s, &repo).join(EVENTS_FILE).to_string_lossy())
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON line"))
        .collect();
    assert!(
        logged.iter().any(|line| line["event"] == "archived"
            && line["branch"] == "feat"
            && line["tip"] == tip.as_str()
            && line["ref"] == reference
            && line["reason"] == "went another way"),
        "{logged:#?}"
    );
}

#[test]
fn test_a_failed_archive_under_abandon_leaves_everything() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let feat = s.add_worktree(&repo, "feat");
    let tip = s.commit(&feat, "feat.txt", "feat\n");
    let (planned, _) = finish(
        &s,
        &repo,
        &[&feat, "--abandon", "--reason", "r", "--dry-run"],
        0,
    );
    let backup = backup(&planned);
    s.sh(&["git", "init", "--quiet", "--bare", &backup], None);
    let hook = join(&backup, "hooks/post-receive");
    s.write(
        &hook,
        "#!/bin/sh\nwhile read -r old new ref; do git update-ref -d \"$ref\"; done\n",
    );
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).expect("executable");
    let (report, _) = finish(&s, &repo, &[&feat, "--abandon", "--reason", "r"], 1);
    assert_eq!(report["repositories"][0]["backup"]["failed"], 1);
    assert!(exists(&join(&feat, "feat.txt")));
    assert_eq!(s.git(&repo, &["rev-parse", "feat"]).trim(), tip);
}

#[test]
fn test_an_unmerged_detached_head_stays_not_landed_even_under_abandon() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let detached = join(&format!("{repo}.worktrees"), "detached");
    s.git(
        &repo,
        &["worktree", "add", "--quiet", "--detach", &detached, "HEAD"],
    );
    let detached = real(&detached);
    s.commit(&detached, "mine.txt", "mine\n");
    let (_, stderr) = finish(&s, &repo, &[&detached, "--abandon", "--reason", "r"], 3);
    assert!(stderr.contains("not-landed"), "{stderr}");
    assert!(exists(&join(&detached, "mine.txt")));
}

#[test]
fn test_the_main_and_a_locked_worktree_exit_3() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let (_, stderr) = finish(&s, &repo, &[&repo], 3);
    assert!(stderr.contains("stays: main-worktree:"), "{stderr}");
    let locked = s.add_worktree(&repo, "locked");
    s.git(&repo, &["worktree", "lock", &locked, "--reason", "busy"]);
    let (_, stderr) = finish(&s, &repo, &[&locked], 3);
    assert!(stderr.contains("stays: locked:"), "{stderr}");
    assert!(exists(&locked) && has_branch(&s, &repo, "locked"));
}

#[test]
fn test_finish_outside_a_repository_prints_nothing_and_exits_1() {
    let s = Sandbox::new();
    let plain = join(&s.tmp, "plain");
    fs::create_dir_all(&plain).expect("a folder");
    for format in ["text", "json"] {
        let ran = s.run(&["finish", &plain, "--format", format], Some(&plain), &[]);
        assert_eq!((ran.code, ran.stdout.as_str()), (1, ""), "{format}");
        assert!(
            ran.stderr.contains("not in a git repository"),
            "{}",
            ran.stderr
        );
    }
}

/// A sweep that found `finish` holding the lock left `sweep.pending`:
/// `finish` starts that sweep detached once it is done, and sweeps
/// nothing itself.
#[test]
fn test_a_sweep_left_pending_is_started_once_finish_ends() {
    let s = Sandbox::new();
    let repo = s.cloned();
    s.git(&repo, &["config", "worktreeManager.idle", "0s"]);
    let landed = s.add_worktree(&repo, "landed");
    let other = s.add_worktree(&repo, "other");
    let state = state(&s, &repo);
    fs::create_dir_all(&state).expect("the state folder");
    s.write(&state.join(SWEEP_PENDING_FILE).to_string_lossy(), "");
    let fifo = state.join(LAST_SWEEP_FILE);
    s.sh(&["mkfifo", &fifo.to_string_lossy()], None);
    let (report, _) = finish(&s, &repo, &[&landed], 0);
    assert_eq!(
        report["repositories"][0]["worktrees"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert!(exists(&other), "finish swept nothing else itself");
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(fs::read_to_string(&fifo).unwrap_or_default());
    });
    let text = receiver
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the detached sweep's report");
    let swept: Value = serde_json::from_str(&text).expect("a JSON report");
    assert_eq!(swept["command"], "sweep");
    assert_eq!(swept["repositories"][0]["path"], repo.as_str());
    assert!(!state.join(SWEEP_PENDING_FILE).exists());
}

#[test]
fn test_a_path_that_is_no_worktree_is_a_usage_error() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let elsewhere = join(&repo, "src");
    fs::create_dir_all(&elsewhere).expect("a folder");
    let ran = s.run(&["finish", &elsewhere], Some(&repo), &[]);
    assert_eq!((ran.code, ran.stdout.as_str()), (2, ""), "{}", ran.stderr);
    assert!(ran.stderr.contains("is no worktree of"), "{}", ran.stderr);
}
