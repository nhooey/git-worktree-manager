//! The sweep's lock: an attended sweep and `finish` wait for it; an
//! unattended sweep leaves `sweep.pending` and the holder runs again; a
//! crashed sweep's lock blocks nothing.
//!
//! The first sweep is held at a known point without sleeps: the event log
//! is a FIFO, which an acting sweep opens at its rescue step, under the
//! lock, and reads until every writer has closed it.

mod sandbox;

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use git_worktree_manager::names::{
    self, EVENTS_FILE, LAST_SWEEP_FILE, SWEEP_LOCK_FILE, SWEEP_PENDING_FILE,
};
use sandbox::{BIN, Sandbox, exists, real};
use serde_json::Value;

const WAIT: Duration = Duration::from_secs(60);

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

/// A clone whose worktrees are idle at once, and its event log made a
/// FIFO.
fn clone_with_fifo_log(s: &Sandbox) -> (String, PathBuf) {
    let repo = s.cloned();
    s.git(&repo, &["config", "worktreeManager.idle", "0s"]);
    let state = state(s, &repo);
    fs::create_dir_all(&state).expect("the state folder");
    s.sh(
        &["mkfifo", &state.join(EVENTS_FILE).to_string_lossy()],
        None,
    );
    (repo, state)
}

/// The program with `args`, started and not waited for.
fn start(s: &Sandbox, args: &[&str]) -> Child {
    Command::new(BIN)
        .args(args)
        .current_dir(&s.tmp)
        .env_clear()
        .envs(s.env.iter().map(|(n, v)| (n.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the program runs")
}

/// `sweep --repository <repo> --format json`, started.
fn start_sweep(s: &Sandbox, repo: &str) -> Child {
    start(s, &["sweep", "--repository", repo, "--format", "json"])
}

fn bounded<T: Send + 'static>(what: &str, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver
        .recv_timeout(WAIT)
        .unwrap_or_else(|_| panic!("{what} took over {WAIT:?}"))
}

/// The FIFO opened for writing, once a sweep opens it to read: from then
/// on that sweep holds its lock and waits until the file is dropped.
fn hold(fifo: &Path) -> fs::File {
    let fifo = fifo.to_owned();
    bounded("a sweep reaching its rescue", move || {
        fs::OpenOptions::new()
            .write(true)
            .open(&fifo)
            .expect("the FIFO opens")
    })
}

/// Lets every later read of the FIFO end at once, by opening and closing
/// it for each reader, for as long as the test runs.
fn release(fifo: &Path) {
    let fifo = fifo.to_owned();
    std::thread::spawn(
        move || {
            while fs::OpenOptions::new().write(true).open(&fifo).is_ok() {}
        },
    );
}

fn finish(child: Child) -> Output {
    bounded("the program's end", move || {
        child.wait_with_output().expect("the program ends")
    })
}

/// The child's standard error, read in a thread: its first line as soon
/// as it is written, and all of it once the child closes it.
fn watch_stderr(child: &mut Child) -> (mpsc::Receiver<String>, mpsc::Receiver<String>) {
    let stderr = child.stderr.take().expect("standard error");
    let (first_sender, first) = mpsc::channel();
    let (all_sender, all) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = first_sender.send(line.clone());
        let mut rest = String::new();
        let _ = reader.read_to_string(&mut rest);
        let _ = all_sender.send(line + &rest);
    });
    (first, all)
}

fn has_done(report: &Value, path: &str) -> bool {
    report["repositories"][0]["worktrees"]
        .as_array()
        .is_some_and(|w| {
            w.iter()
                .any(|w| w["path"] == path && w["outcome"] == "done")
        })
}

#[test]
fn test_an_unattended_sweep_leaves_pending_and_the_holder_runs_again() {
    let s = Sandbox::new();
    let (repo, state) = clone_with_fifo_log(&s);
    let landed = s.add_worktree(&repo, "landed");
    let first = start_sweep(&s, &repo);
    let held = hold(&state.join(EVENTS_FILE));
    assert!(
        !exists(&landed),
        "the first sweep removed it before its rescue"
    );

    let later = s.add_worktree(&repo, "later");
    let second = finish(start(
        &s,
        &[
            "sweep",
            "--unattended",
            "--repository",
            &repo,
            "--format",
            "json",
        ],
    ));
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert_eq!(second.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains(&format!("another sweep of {repo} is running")),
        "{stderr}"
    );
    let report: Value = serde_json::from_slice(&second.stdout).expect("JSON");
    assert_eq!(report["dry_run"], false, "it planned nothing");
    let skipped = &report["repositories"][0];
    assert_eq!(skipped["path"], repo.as_str(), "{report:#}");
    assert_eq!(skipped["problem"]["code"], "skipped", "{report:#}");
    assert!(
        skipped["problem"]["detail"]
            .as_str()
            .is_some_and(|d| d.contains("another sweep")),
        "{report:#}"
    );
    assert!(exists(&later), "the second sweep acted");
    assert!(state.join(SWEEP_PENDING_FILE).exists());

    drop(held);
    release(&state.join(EVENTS_FILE));
    let ran = finish(first);
    assert_eq!(
        ran.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(!exists(&later), "the first sweep ran again, attended");
    assert!(!state.join(SWEEP_PENDING_FILE).exists());
    // Both rounds are in the report printed and the report written.
    let printed: Value = serde_json::from_slice(&ran.stdout).expect("JSON");
    let written: Value = serde_json::from_str(&sandbox::read(
        &state.join(LAST_SWEEP_FILE).to_string_lossy(),
    ))
    .expect("the report written");
    for report in [&printed, &written] {
        for path in [&landed, &later] {
            assert!(has_done(report, path), "{path}: {report:#}");
        }
    }
}

#[test]
fn test_an_attended_sweep_waits_for_the_lock_then_acts_itself() {
    let s = Sandbox::new();
    let (repo, state) = clone_with_fifo_log(&s);
    let first = start_sweep(&s, &repo);
    let held = hold(&state.join(EVENTS_FILE));
    let later = s.add_worktree(&repo, "later");
    let mut second = start_sweep(&s, &repo);
    let (first_line, all) = watch_stderr(&mut second);
    let line = first_line
        .recv_timeout(WAIT)
        .expect("the second sweep says it waits");
    assert!(
        line.contains(&format!("waiting for another sweep of {repo}")),
        "{line}"
    );
    assert!(exists(&later), "the second sweep waits");
    assert!(!state.join(SWEEP_PENDING_FILE).exists());

    drop(held);
    release(&state.join(EVENTS_FILE));
    assert_eq!(finish(first).status.code(), Some(0));
    let ran = finish(second);
    let stderr = all.recv_timeout(WAIT).expect("its standard error");
    assert_eq!(ran.status.code(), Some(0), "{stderr}");
    assert_eq!(stderr.matches("waiting for another sweep").count(), 1);
    let report: Value = serde_json::from_slice(&ran.stdout).expect("JSON");
    assert!(has_done(&report, &later), "{report:#}");
    assert!(!exists(&later));
}

#[test]
fn test_finish_waits_for_the_lock_then_finishes() {
    let s = Sandbox::new();
    let (repo, state) = clone_with_fifo_log(&s);
    let first = start_sweep(&s, &repo);
    let held = hold(&state.join(EVENTS_FILE));
    let landed = s.add_worktree(&repo, "landed");
    let mut finishing = start(&s, &["finish", &landed, "--format", "json"]);
    let (first_line, all) = watch_stderr(&mut finishing);
    let line = first_line.recv_timeout(WAIT).expect("finish says it waits");
    assert!(
        line.contains(&format!("waiting for another sweep of {repo}")),
        "{line}"
    );
    assert!(exists(&landed), "finish waits");

    drop(held);
    release(&state.join(EVENTS_FILE));
    assert_eq!(finish(first).status.code(), Some(0));
    let ran = finish(finishing);
    let stderr = all.recv_timeout(WAIT).expect("its standard error");
    assert_eq!(ran.status.code(), Some(0), "{stderr}");
    let report: Value = serde_json::from_slice(&ran.stdout).expect("JSON");
    assert_eq!(report["command"], "finish");
    assert!(has_done(&report, &landed), "{report:#}");
    assert!(!exists(&landed));
}

/// The sweep's lock on the repository whose state folder is `state`,
/// taken by the test, as a sweep holding it would.
fn take_lock(state: &Path) -> fs::File {
    fs::create_dir_all(state).expect("the state folder");
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join(SWEEP_LOCK_FILE))
        .expect("the lock file");
    file.try_lock().expect("the lock is free");
    file
}

/// `finish` on a worktree that a sweep removes while `finish` waits for
/// it ends as when it starts after that sweep: the path is no worktree of
/// the repository, exit 2.
#[test]
fn test_finish_on_a_worktree_removed_while_it_waits_exits_2() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let state = state(&s, &repo);
    let old = s.add_worktree(&repo, "old");
    let lock = take_lock(&state);
    let mut finishing = start(&s, &["finish", &old, "--format", "json"]);
    let (first_line, all) = watch_stderr(&mut finishing);
    let line = first_line.recv_timeout(WAIT).expect("finish says it waits");
    assert!(line.contains("waiting for another sweep"), "{line}");
    s.git(&repo, &["worktree", "remove", &old]);
    drop(lock);
    let ran = finish(finishing);
    let stderr = all.recv_timeout(WAIT).expect("its standard error");
    assert_eq!(ran.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("is no worktree of"), "{stderr}");
}

/// A dry run acts on nothing, so it doesn't wait for a sweep.
#[test]
fn test_a_dry_run_doesnt_wait_for_the_lock() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let state = state(&s, &repo);
    let landed = s.add_worktree(&repo, "landed");
    let _lock = take_lock(&state);
    for args in [
        vec!["sweep", "--dry-run", "--repository", &repo],
        vec!["finish", "--dry-run", &landed],
    ] {
        let ran = finish(start(&s, &args));
        let stderr = String::from_utf8_lossy(&ran.stderr);
        assert_eq!(ran.status.code(), Some(0), "{args:?}: {stderr}");
        assert!(!stderr.contains("waiting"), "{args:?}: {stderr}");
    }
    assert!(exists(&landed));
}

#[test]
fn test_a_crashed_sweeps_lock_blocks_nothing() {
    let s = Sandbox::new();
    let (repo, state) = clone_with_fifo_log(&s);
    let mut first = start_sweep(&s, &repo);
    let held = hold(&state.join(EVENTS_FILE));
    first.kill().expect("the first sweep is killed");
    first.wait().expect("the first sweep ends");
    drop(held);
    fs::remove_file(state.join(EVENTS_FILE)).expect("the FIFO goes");
    assert!(state.join(SWEEP_LOCK_FILE).exists());

    let later = s.add_worktree(&repo, "later");
    let ran = finish(start(&s, &["sweep", "--repository", &repo]));
    let stderr = String::from_utf8_lossy(&ran.stderr);
    assert_eq!(ran.status.code(), Some(0), "{stderr}");
    assert!(!stderr.contains("another sweep"), "{stderr}");
    assert!(!exists(&later));
    assert!(!state.join(SWEEP_PENDING_FILE).exists());
}

/// A `sweep.pending` that can't be cleared ends the sweep's rounds after
/// one, and says so, rather than running every round there is.
#[test]
fn test_a_pending_file_that_cant_be_cleared_stops_the_rounds() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let first = finish(start(&s, &["sweep", "--repository", &repo]));
    assert_eq!(first.status.code(), Some(0));
    let state = state(&s, &repo);
    s.write(&state.join(SWEEP_PENDING_FILE).to_string_lossy(), "");
    let mode = |m: u32| {
        fs::set_permissions(&state, fs::Permissions::from_mode(m)).expect("the folder's mode");
    };
    mode(0o555);
    let ran = finish(start(&s, &["sweep", "--repository", &repo]));
    mode(0o755);
    let stderr = String::from_utf8_lossy(&ran.stderr);
    assert_eq!(stderr.matches("clearing").count(), 1, "{stderr}");
}
