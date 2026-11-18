use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use jiff::Timestamp;

use super::*;

fn now() -> Timestamp {
    "2026-11-19T09:12:00Z".parse().expect("a time")
}

/// A temporary folder, as a real path.
fn scratch() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let real = fs::canonicalize(dir.path()).expect("a real path");
    (dir, real)
}

// Paths -----------------------------------------------------------------------

#[test]
fn test_real_path_agrees_with_canonicalize_where_the_path_exists() {
    let (_dir, root) = scratch();
    fs::create_dir_all(root.join("a/b")).expect("folders");
    symlink(root.join("a/b"), root.join("link")).expect("a link");
    assert_eq!(
        real_path(&root.join("link/../b/."), Path::new("/")),
        root.join("a/b")
    );
    assert_eq!(real_path(Path::new("b"), &root.join("a")), root.join("a/b"));
}

#[test]
fn test_a_missing_path_resolves_as_far_as_it_can() {
    let (_dir, root) = scratch();
    fs::create_dir_all(root.join("a")).expect("a folder");
    symlink(root.join("a"), root.join("link")).expect("a link");
    assert_eq!(
        real_path(&root.join("link/missing/x"), Path::new("/")),
        root.join("a/missing/x")
    );
}

#[test]
fn test_a_dangling_link_resolves_to_its_missing_target() {
    let (_dir, root) = scratch();
    let (inside, outside) = (root.join("inside"), root.join("outside"));
    fs::create_dir_all(&inside).expect("a folder");
    symlink(outside.join("gone"), inside.join("dangling")).expect("a link");
    let resolved = real_path(&inside.join("dangling"), Path::new("/"));
    assert_eq!(resolved, outside.join("gone"));
    assert!(
        !is_inside(&resolved, &inside),
        "a guard sees the link leave"
    );
}

#[test]
fn test_a_link_loop_ends() {
    let (_dir, root) = scratch();
    symlink(root.join("b"), root.join("a")).expect("a link");
    symlink(root.join("a"), root.join("b")).expect("a link");
    let resolved = real_path(&root.join("a"), Path::new("/"));
    assert!(resolved.starts_with(&root));
}

#[test]
fn test_is_inside_compares_whole_parts() {
    assert!(is_inside(Path::new("/a/b/c"), Path::new("/a/b")));
    assert!(is_inside(Path::new("/a/b"), Path::new("/a/b")));
    assert!(!is_inside(Path::new("/a/bc"), Path::new("/a/b")));
}

#[test]
fn test_real_remove_refuses_a_link_out_and_deletes_only_the_link() {
    let (_dir, root) = scratch();
    let (inside, outside) = (root.join("inside"), root.join("outside"));
    fs::create_dir_all(&inside).expect("a folder");
    fs::create_dir_all(&outside).expect("a folder");
    fs::write(outside.join("keep"), "kept").expect("a file");
    symlink(&outside, inside.join("out")).expect("a link");
    let mut system = Real::new(Environment::default());
    assert!(system.remove(&outside.join("keep"), &inside).is_err());
    assert_eq!(system.remove(&inside.join("out"), &inside), Ok(true));
    assert!(outside.join("keep").exists(), "the link's target stays");
    assert_eq!(system.remove(&inside.join("out"), &inside), Ok(false));
    assert!(system.remove(&inside, &inside).is_err());
}

#[test]
fn test_real_lock_is_exclusive() {
    let (_dir, root) = scratch();
    let path = root.join("state/sweep.lock");
    let mut system = Real::new(Environment::default());
    let held = system.lock(&path).expect("a lock file");
    assert!(held.is_some());
    assert!(system.lock(&path).expect("a lock file").is_none());
    drop(held);
    assert!(system.lock(&path).expect("a lock file").is_some());
}

#[test]
fn test_real_append_writes_whole_lines() {
    let (_dir, root) = scratch();
    let log = root.join("state/events.jsonl");
    let mut system = Real::new(Environment::default());
    system.append(&log, b"one\n").expect("appended");
    system.append(&log, b"two\n").expect("appended");
    assert_eq!(fs::read_to_string(&log).expect("the log"), "one\ntwo\n");
}

#[test]
fn test_real_free_bytes() {
    let (_dir, root) = scratch();
    let system = Real::new(Environment::default());
    assert!(system.free_bytes(&root).expect("free space") > 0);
    assert!(system.free_bytes(&root.join("missing")).is_err());
}

// The fake ----------------------------------------------------------------------

#[test]
fn test_the_fake_reads_its_tables() {
    let system = Fake::new(now())
        .with_environment(Environment::from_pairs([("HOME", "/h")]))
        .with_file("/repo/.worktree-keep", "*.local\n")
        .with_git("/repo", ["rev-parse", "HEAD"], Completed::ok("abc\n"))
        .with_process(42, now())
        .with_volume("/", 10)
        .with_volume("/Volumes/x", 20);
    assert_eq!(system.environment().var("HOME"), Some("/h"));
    assert_eq!(
        system.read_file(Path::new("/repo/.worktree-keep")),
        Ok(Some(b"*.local\n".to_vec()))
    );
    assert_eq!(system.read_file(Path::new("/repo/none")), Ok(None));
    assert_eq!(
        system
            .metadata(Path::new("/repo"))
            .map(|m| m.map(|m| m.kind)),
        Ok(Some(FileKind::Dir))
    );
    assert_eq!(system.list_dir(Path::new("/")), Ok(vec!["repo".into()]));
    assert_eq!(
        system
            .git(Path::new("/repo"), &args(["rev-parse", "HEAD"]))
            .map(|done| done.stdout),
        Ok(b"abc\n".to_vec())
    );
    assert!(system.git(Path::new("/repo"), &args(["status"])).is_err());
    assert_eq!(system.process_start(42), Ok(Some(now())));
    assert_eq!(system.process_start(43), Ok(None));
    assert_eq!(system.free_bytes(Path::new("/Volumes/x/a")), Ok(20));
    assert_eq!(system.free_bytes(Path::new("/home")), Ok(10));
}

#[test]
fn test_the_fake_records_acts_and_shows_writes_to_later_reads() {
    let mut system = Fake::new(now()).with_file("/repo/a", "a").with_git(
        "/repo",
        ["update-ref", "-d", "refs/heads/x"],
        Completed::ok(""),
    );
    system
        .append(Path::new("/state/events.jsonl"), b"one\n")
        .expect("appended");
    system
        .append(Path::new("/state/events.jsonl"), b"two\n")
        .expect("appended");
    system
        .copy_new(Path::new("/repo/a"), Path::new("/wt/a"))
        .expect("copied");
    assert!(
        system
            .copy_new(Path::new("/repo/a"), Path::new("/wt/a"))
            .is_err()
    );
    assert!(
        system
            .git_act(
                Path::new("/repo"),
                &args(["update-ref", "-d", "refs/heads/x"])
            )
            .expect("answered")
            .success()
    );
    assert!(
        system
            .remove(Path::new("/elsewhere"), Path::new("/wt"))
            .is_err()
    );
    assert_eq!(
        system.remove(Path::new("/wt/a"), Path::new("/wt")),
        Ok(true)
    );
    system.print("out\n").expect("printed");
    system.warn("careful");
    assert_eq!(
        system.read_file(Path::new("/state/events.jsonl")),
        Ok(Some(b"one\ntwo\n".to_vec()))
    );
    assert_eq!(system.read_file(Path::new("/wt/a")), Ok(None));
    assert_eq!(system.stdout, "out\n");
    assert_eq!(
        system.stderr,
        format!("{}: careful\n", crate::names::PROGRAM)
    );
    assert_eq!(
        system
            .acts
            .iter()
            .filter(|act| matches!(act, Act::Append { .. }))
            .count(),
        2
    );
    assert!(system.acts.contains(&Act::Remove {
        path: PathBuf::from("/wt/a")
    }));
}

#[test]
fn test_the_fake_gives_its_standard_input_and_pid() {
    let system = Fake {
        pid: 77,
        ..Fake::new(now()).with_stdin("0000 1111 refs/heads/main\n")
    };
    assert_eq!(system.stdin(), Ok(b"0000 1111 refs/heads/main\n".to_vec()));
    assert_eq!(system.pid(), 77);
}

#[test]
fn test_the_real_pid_is_this_process() {
    let system = Real::new(Environment::default());
    assert_eq!(system.pid(), std::process::id());
}

/// A writer whose first `write` takes only `short` bytes.
struct ShortOnce {
    short: Option<usize>,
    written: Vec<u8>,
}

impl std::io::Write for ShortOnce {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let taken = self
            .short
            .take()
            .map_or(buf.len(), |short| short.min(buf.len()));
        self.written.extend_from_slice(&buf[..taken]);
        Ok(taken)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn test_a_short_write_is_finished_before_it_fails() {
    let mut out = ShortOnce {
        short: Some(3),
        written: Vec::new(),
    };
    let chunks: [&[u8]; 2] = [b"one\ntwo\n", b"three\n"];
    let result = super::real::write_chunks(&mut out, Path::new("/log"), &chunks);
    assert!(
        matches!(&result, Err(failure) if failure.detail.contains("wrote 3 of 8")),
        "{result:?}"
    );
    assert_eq!(out.written, b"one\ntwo\n");
}

#[test]
fn test_failures_carry_a_code() {
    assert_eq!(
        Failure::unreadable("x").code,
        crate::model::ids::ErrorCode::Unreadable
    );
    let failure = Failure::step_failed("y");
    assert_eq!(
        (failure.note().code, failure.to_string()),
        (crate::model::ids::ErrorCode::StepFailed, "y".to_owned())
    );
}

#[test]
fn test_the_fake_lock_is_refused_where_another_holds_it() {
    let mut system = Fake {
        held: [PathBuf::from("/state/sweep.lock")].into(),
        ..Fake::new(now())
    };
    assert_eq!(system.lock(Path::new("/state/sweep.lock")), Ok(None));
    assert_eq!(system.lock(Path::new("/state/other.lock")), Ok(Some(())));
}
