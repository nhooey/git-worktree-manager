//! The rescue: a branch deleted by hand whose last logged tip nothing
//! holds is archived; one that is merged, already in the backup or gone,
//! or whose logged name was cut or held invalid bytes, is not.

mod sandbox;

use std::path::{Path, PathBuf};

use git_worktree_manager::hook::CUT_MARK;
use git_worktree_manager::names::{self, EVENTS_FILE};
use sandbox::{Sandbox, real};
use serde_json::Value;

const ZERO: &str = "0000000000000000000000000000000000000000";

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

/// A commit on `parent` that no ref holds.
fn loose(s: &Sandbox, repo: &str, parent: &str, message: &str) -> String {
    s.git(
        repo,
        &["commit-tree", "HEAD^{tree}", "-p", parent, "-m", message],
    )
    .trim()
    .to_owned()
}

fn line(reference: &str, old: &str, new: &str) -> String {
    serde_json::json!({
        "time": "2026-11-19T09:00:00Z",
        "pid": 1,
        "event": "ref",
        "ref": reference,
        "old": old,
        "new": new,
    })
    .to_string()
        + "\n"
}

fn log(s: &Sandbox, repo: &str, lines: &[String]) {
    s.write(
        &state(s, repo).join(EVENTS_FILE).to_string_lossy(),
        &lines.concat(),
    );
}

fn sweep(s: &Sandbox, repo: &str) -> Value {
    s.run_json(&["sweep", "--repository", repo], None, 0)
}

fn rescued(report: &Value) -> Vec<(String, String)> {
    report["repositories"][0]["rescues"]
        .as_array()
        .expect("rescues")
        .iter()
        .map(|r| {
            (
                r["branch"].as_str().unwrap_or_default().to_owned(),
                r["tip"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// An archive at the tip counts only once it passes its compare in this
/// run: one whose backup lost a blob is no hold on the commit, so the
/// rescue tries again, and says it failed.
#[test]
fn test_a_tip_whose_archive_fails_its_compare_is_not_taken_as_backed_up() {
    let s = Sandbox::new();
    let repo = s.cloned();
    s.git(&repo, &["checkout", "--quiet", "-b", "solo"]);
    let tip = s.commit(&repo, "solo.txt", "only on solo\n");
    s.git(&repo, &["checkout", "--quiet", "main"]);
    let written = s.run_json(
        &["archive", "write", "solo", "--repository", &repo],
        None,
        0,
    );
    let backup = written["repositories"][0]["backup"]["path"]
        .as_str()
        .expect("a backup")
        .to_owned();
    let blob = s.git(&repo, &["rev-parse", "solo:solo.txt"]);
    let blob = blob.trim();
    std::fs::remove_file(sandbox::join(
        &backup,
        &format!("objects/{}/{}", &blob[..2], &blob[2..]),
    ))
    .expect("the blob goes from the backup");
    s.git(&repo, &["update-ref", "-d", "refs/heads/solo", &tip]);
    log(&s, &repo, &[line("refs/heads/solo", &tip, ZERO)]);
    let planned = s.run_json(&["sweep", "--dry-run", "--repository", &repo], None, 0);
    assert_eq!(rescued(&planned), [("solo".to_owned(), tip.clone())]);
    let ran = s.run(
        &["sweep", "--repository", &repo, "--format", "json"],
        None,
        &[],
    );
    let report: Value = serde_json::from_str(&ran.stdout).expect("JSON");
    assert_eq!(rescued(&report), [("solo".to_owned(), tip.clone())]);
    // The push can't mend the backup: it holds a ref at the tip already,
    // so git sends nothing under it, and the new archive fails its compare.
    let rescue = &report["repositories"][0]["rescues"][0];
    assert_eq!(rescue["outcome"], "failed", "{rescue:#}");
    assert_eq!(ran.code, 1, "{}", ran.stderr);
    assert_eq!(
        s.git(&repo, &["cat-file", "-t", &tip]).trim(),
        "commit",
        "the commit is still in the clone"
    );
}

/// A rescue whose push the backup refuses fails, says why, and leaves
/// the commit where it was.
#[test]
fn test_a_rescue_whose_push_is_refused_fails() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let tip = loose(&s, &repo, "HEAD", "only in the reflog");
    let status = s.run_json(&["status", "--repository", &repo], None, 0);
    let backup = status["repositories"][0]["backup"]["path"]
        .as_str()
        .expect("a backup")
        .to_owned();
    s.sh(&["git", "init", "--quiet", "--bare", &backup], None);
    let hook = sandbox::join(&backup, "hooks/pre-receive");
    s.write(&hook, "#!/bin/sh\necho refused >&2\nexit 1\n");
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("executable");
    log(&s, &repo, &[line("refs/heads/gone", &tip, ZERO)]);
    let ran = s.run(
        &["sweep", "--repository", &repo, "--format", "json"],
        None,
        &[],
    );
    assert_eq!(ran.code, 1, "{}", ran.stderr);
    let report: Value = serde_json::from_str(&ran.stdout).expect("JSON");
    let rescue = &report["repositories"][0]["rescues"][0];
    assert_eq!(rescue["branch"], "gone", "{rescue:#}");
    assert_eq!(rescue["outcome"], "failed", "{rescue:#}");
    assert!(
        rescue["error"].as_str().is_some_and(|e| !e.is_empty()),
        "{rescue:#}"
    );
    assert_eq!(s.git(&repo, &["cat-file", "-t", &tip]).trim(), "commit");
    assert_eq!(
        s.sh(
            &[
                "git",
                "--git-dir",
                &backup,
                "for-each-ref",
                "--format=%(refname)"
            ],
            None
        )
        .trim(),
        ""
    );
}

#[test]
fn test_a_deleted_branch_is_rescued_from_its_last_logged_tip() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let first = loose(&s, &repo, "HEAD", "first");
    let last = loose(&s, &repo, &first, "last");
    log(
        &s,
        &repo,
        &[
            line("refs/heads/lost", ZERO, &first),
            line("refs/heads/lost", &first, &last),
            line("refs/heads/lost", &last, ZERO),
        ],
    );
    let planned = s.run_json(&["sweep", "--dry-run", "--repository", &repo], None, 0);
    assert_eq!(
        planned["repositories"][0]["rescues"][0]["outcome"],
        "planned"
    );
    let report = sweep(&s, &repo);
    assert_eq!(rescued(&report), [("lost".to_owned(), last.clone())]);
    let rescue = &report["repositories"][0]["rescues"][0];
    assert_eq!(
        (&rescue["outcome"], &rescue["archive"]["state"]),
        (&Value::from("done"), &Value::from("verified")),
        "{rescue:#}"
    );
    let reference = rescue["archive"]["ref"].as_str().expect("an archive ref");
    assert!(reference.starts_with("refs/archive/lost/"), "{reference}");
    let backup = report["repositories"][0]["backup"]["path"]
        .as_str()
        .expect("a backup");
    assert_eq!(
        s.sh(&["git", "--git-dir", backup, "rev-parse", reference], None)
            .trim(),
        last
    );
    let again = sweep(&s, &repo);
    assert_eq!(rescued(&again), [], "the backup holds it now");
}

#[test]
fn test_no_rescue_for_a_merged_tip_one_in_the_backup_or_one_gone() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let merged = s.git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    let held = loose(&s, &repo, "HEAD", "held");
    s.git(&repo, &["update-ref", "refs/heads/held", &held]);
    s.run_json(
        &["archive", "write", "held", "--repository", &repo],
        None,
        0,
    );
    s.git(&repo, &["update-ref", "-d", "refs/heads/held", &held]);
    let gone = "1234567812345678123456781234567812345678";
    log(
        &s,
        &repo,
        &[
            line("refs/heads/merged", &merged, ZERO),
            line("refs/heads/held", &held, ZERO),
            line("refs/heads/gone", gone, ZERO),
        ],
    );
    assert_eq!(rescued(&sweep(&s, &repo)), []);
}

#[test]
fn test_a_cut_or_invalid_name_is_skipped() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let cut = loose(&s, &repo, "HEAD", "cut");
    let invalid = loose(&s, &repo, "HEAD", "invalid");
    let kept = loose(&s, &repo, "HEAD", "kept");
    log(
        &s,
        &repo,
        &[
            line(&format!("refs/heads/a-long-na{CUT_MARK}"), &cut, ZERO),
            line("refs/heads/bad\u{FFFD}name", &invalid, ZERO),
            line("refs/heads/kept", &kept, ZERO),
        ],
    );
    assert_eq!(rescued(&sweep(&s, &repo)), [("kept".to_owned(), kept)]);
}
