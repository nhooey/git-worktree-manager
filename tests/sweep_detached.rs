//! The sweep the hook starts when the default branch moves: git doesn't
//! wait for it, and git's own variables don't reach it.
//!
//! The sweep's report is a FIFO, so the sweep blocks writing it until the
//! test reads it: git returning first shows git didn't wait.

mod sandbox;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use git_worktree_manager::hook::{self, HOOK};
use git_worktree_manager::names::{self, LAST_SWEEP_FILE};
use sandbox::{BIN, Sandbox, exists, join, real};
use serde_json::Value;

const DEFAULT: &str = "refs/remotes/origin/main";
const WAIT: Duration = Duration::from_secs(60);

fn bounded<T: Send + 'static>(what: &str, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver
        .recv_timeout(WAIT)
        .unwrap_or_else(|_| panic!("{what} took over {WAIT:?}"))
}

/// A clone with the shim installed, its worktrees idle at once, and its
/// last-sweep report made a FIFO.
fn hooked(s: &Sandbox) -> (String, PathBuf) {
    let repo = s.cloned();
    s.git(&repo, &["config", "worktreeManager.idle", "0s"]);
    let log = join(&s.tmp, "events.jsonl");
    let path = join(&repo, &format!(".git/hooks/{HOOK}"));
    s.write(&path, &hook::shim(Path::new(BIN), Path::new(&log), DEFAULT));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
    let common = s.git(
        &repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    );
    let state = names::state_dir(&s.environment(), Path::new(&real(common.trim())))
        .expect("a state folder");
    assert!(state.starts_with(&s.tmp), "{}", state.display());
    fs::create_dir_all(&state).expect("the state folder");
    let fifo = state.join(LAST_SWEEP_FILE);
    s.sh(&["mkfifo", &fifo.to_string_lossy()], None);
    (repo, fifo)
}

/// A commit on top of `HEAD` that no ref holds.
fn next_commit(s: &Sandbox, repo: &str) -> String {
    s.git(
        repo,
        &["commit-tree", "HEAD^{tree}", "-p", "HEAD", "-m", "upstream"],
    )
    .trim()
    .to_owned()
}

/// Moves the default ref with `git update-ref`, with `env` added and
/// standard output and error piped, as a caller capturing git's output
/// would, and gives git's exit code once it returns.
fn move_default(s: &Sandbox, repo: &str, to: &str, env: &[(&str, String)]) -> i32 {
    let mut command = Command::new("git");
    command
        .args(["-C", repo, "update-ref", DEFAULT, to])
        .current_dir(&s.tmp)
        .env_clear()
        .envs(s.env.iter().map(|(n, v)| (n.as_str(), v.as_str())))
        .envs(env.iter().map(|(n, v)| (*n, v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    bounded("git, while the sweep it started runs", move || {
        command
            .output()
            .expect("git runs")
            .status
            .code()
            .unwrap_or(-1)
    })
}

/// The report the detached sweep writes, read from the FIFO.
fn report(fifo: PathBuf) -> Value {
    let text = bounded("the detached sweep's report", move || {
        fs::read_to_string(&fifo).expect("the FIFO reads")
    });
    serde_json::from_str(&text).expect("a JSON report")
}

#[test]
fn test_git_returns_while_the_sweep_it_started_still_runs() {
    let s = Sandbox::new();
    let (repo, fifo) = hooked(&s);
    let to = next_commit(&s, &repo);
    // The sweep can't end before the FIFO is read, which is only after
    // git has returned.
    assert_eq!(move_default(&s, &repo, &to, &[]), 0);
    let report = report(fifo);
    assert_eq!(report["command"], "sweep");
    assert_eq!(report["repositories"][0]["path"], repo.as_str());
}

/// Git hands its hooks `GIT_DIR` and friends; the sweep it starts reads
/// the repository it was named, and its backup by the backup's own
/// objects, never the clone's.
#[test]
fn test_gits_variables_dont_reach_the_detached_sweep() {
    let s = Sandbox::new();
    let (repo, fifo) = hooked(&s);
    s.git(&repo, &["config", "worktreeManager.unattended", "sweep"]);
    let gone = join(&format!("{repo}.worktrees"), "gone");
    s.git(
        &repo,
        &["worktree", "add", "--quiet", "--detach", &gone, "HEAD"],
    );
    let gone = real(&gone);
    let head = s.commit(&gone, "gone.txt", "only here\n");
    fs::remove_dir_all(&gone).expect("the folder goes");
    let to = next_commit(&s, &repo);
    // Relative, as git may give them: the sweep runs in `/`, where
    // `.git` names no repository of the test's.
    let env = [
        ("GIT_DIR", ".git".to_owned()),
        ("GIT_OBJECT_DIRECTORY", ".git/objects".to_owned()),
    ];
    assert_eq!(move_default(&s, &repo, &to, &env), 0);
    let report = report(fifo);
    let repository = &report["repositories"][0];
    assert_eq!(repository["path"], repo.as_str());
    let pruned = repository["worktrees"]
        .as_array()
        .and_then(|w| w.iter().find(|w| w["path"] == gone.as_str()))
        .expect("the pruned worktree");
    assert_eq!(
        (&pruned["outcome"], &pruned["archive"]["state"]),
        (&Value::from("done"), &Value::from("verified")),
        "{pruned:#}"
    );
    assert!(!s.listed(&repo).contains(&gone));
    let backup = repository["backup"]["path"].as_str().expect("a backup");
    assert!(exists(backup));
    // Read with no variable pointing elsewhere: the backup itself holds
    // the commit and everything it needs.
    let reference = pruned["archive"]["ref"].as_str().expect("an archive ref");
    let held = s.sh(
        &[
            "git",
            "--git-dir",
            backup,
            "rev-list",
            "--objects",
            "--missing=print",
            reference,
        ],
        None,
    );
    assert!(held.starts_with(&head), "{held}");
    assert!(!held.lines().any(|line| line.starts_with('?')), "{held}");
}
