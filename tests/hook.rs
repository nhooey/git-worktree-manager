//! `hook reference-transaction`: through the fake system, in a real
//! repository whose hook is the shim, and timed as the built binary.

mod sandbox;

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use git_worktree_manager::cli;
use git_worktree_manager::commands::Status;
use git_worktree_manager::environment::Environment;
use git_worktree_manager::hook::{self, COMMITTED, HOOK, LINE_LIMIT};
use git_worktree_manager::names::{self, LAST_SWEEP_FILE, PROGRAM, Variable};
use git_worktree_manager::system::{
    Act, Completed, Failure, Fake, FakeFile, FileId, Metadata, System,
};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use sandbox::{BIN, Sandbox, join, real};
use serde_json::Value;

const ZERO: &str = "0000000000000000000000000000000000000000";
const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DEFAULT: &str = "refs/remotes/origin/main";
const LOG: &str = "/state/events.jsonl";

fn argv(state: &str) -> Vec<OsString> {
    [PROGRAM, "hook", HOOK, state]
        .into_iter()
        .map(OsString::from)
        .collect()
}

fn environment(log: Option<&str>, default_ref: Option<&str>) -> Environment {
    Environment::from_pairs(
        [(Variable::Log, log), (Variable::DefaultRef, default_ref)]
            .into_iter()
            .filter_map(|(variable, value)| value.map(|value| (variable.name(), value))),
    )
}

/// The folder git runs the hook in: a linked worktree.
const WORKTREE: &str = "/src/app.worktrees/fix";

fn fake(input: &str, log: Option<&str>, default_ref: Option<&str>) -> Fake {
    Fake {
        current_dir: PathBuf::from(WORKTREE),
        ..Fake::new("2026-11-19T09:12:00Z".parse().expect("a time"))
    }
    .with_environment(environment(log, default_ref))
    .with_stdin(input)
}

fn three_refs(last: &str) -> String {
    format!("{ZERO} {A} refs/heads/new\n{A} {B} refs/heads/moved\n{A} {B} {last}\n")
}

/// Each line appended, with its log; every write is whole lines under the
/// limit.
fn appended(acts: &[Act]) -> Vec<(PathBuf, Value)> {
    acts.iter()
        .filter_map(|act| match act {
            Act::Append { path, line } => Some((path, line)),
            _ => None,
        })
        .flat_map(|(path, write)| {
            assert!(write.len() < LINE_LIMIT, "{} bytes", write.len());
            assert!(write.ends_with(b"\n"));
            String::from_utf8_lossy(write)
                .lines()
                .map(|line| {
                    (
                        path.clone(),
                        serde_json::from_str(line).expect("a JSON line"),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn writes(acts: &[Act]) -> usize {
    acts.iter()
        .filter(|act| matches!(act, Act::Append { .. }))
        .count()
}

fn spawned(acts: &[Act]) -> Vec<&Act> {
    acts.iter()
        .filter(|act| matches!(act, Act::Spawn { .. }))
        .collect()
}

fn the_sweep() -> Act {
    Act::Spawn {
        dir: PathBuf::from("/"),
        program: PathBuf::from("/bin").join(PROGRAM),
        args: ["sweep", "--unattended", "--repository", WORKTREE]
            .map(OsString::from)
            .to_vec(),
        env: vec![],
    }
}

// Through the fake system -----------------------------------------------------

#[test]
fn test_any_state_but_committed_touches_nothing() {
    for state in ["preparing", "prepared", "aborted", "", "Committed"] {
        let mut system = fake(&three_refs(DEFAULT), Some(LOG), Some(DEFAULT));
        assert_eq!(cli::run(argv(state), &mut system), Status::Done, "{state}");
        assert_eq!(system.acts, vec![], "{state}");
        assert_eq!((system.stdout.as_str(), system.stderr.as_str()), ("", ""));
    }
}

#[test]
fn test_committed_appends_one_line_per_ref() {
    let mut system = fake(&three_refs("refs/heads/other"), Some(LOG), Some(DEFAULT));
    assert_eq!(cli::run(argv(COMMITTED), &mut system), Status::Done);
    let lines = appended(&system.acts);
    assert_eq!(
        lines
            .iter()
            .map(|(path, line)| (path.to_str().expect("UTF-8"), line["ref"].as_str()))
            .collect::<Vec<_>>(),
        [
            (LOG, Some("refs/heads/new")),
            (LOG, Some("refs/heads/moved")),
            (LOG, Some("refs/heads/other")),
        ]
    );
    assert_eq!(lines[0].1["pid"], 4242);
    assert_eq!(lines[0].1["time"], "2026-11-19T09:12:00Z");
    assert_eq!(spawned(&system.acts), Vec::<&Act>::new());
    assert_eq!((system.stdout.as_str(), system.stderr.as_str()), ("", ""));
}

#[test]
fn test_a_1000_ref_transaction_is_logged_in_whole_lines_under_the_limit() {
    let input: String = (0..1000)
        .map(|i| format!("{A} {B} refs/remotes/origin/b{i:04}\n"))
        .collect();
    let mut system = fake(&input, Some(LOG), Some(DEFAULT));
    assert_eq!(cli::run(argv(COMMITTED), &mut system), Status::Done);
    let lines = appended(&system.acts);
    assert_eq!(
        lines
            .iter()
            .map(|(_, line)| line["ref"].clone())
            .collect::<Vec<_>>(),
        (0..1000)
            .map(|i| Value::from(format!("refs/remotes/origin/b{i:04}")))
            .collect::<Vec<_>>()
    );
    assert!(
        writes(&system.acts) < 100,
        "{} writes",
        writes(&system.acts)
    );
}

#[test]
fn test_the_default_ref_moving_starts_the_sweep_after_the_lines() {
    let mut system = fake(&three_refs(DEFAULT), Some(LOG), Some(DEFAULT));
    assert_eq!(cli::run(argv(COMMITTED), &mut system), Status::Done);
    assert_eq!(appended(&system.acts).len(), 3);
    assert_eq!(system.acts.last(), Some(&the_sweep()));
    assert_eq!(spawned(&system.acts).len(), 1);
    assert_eq!((system.stdout.as_str(), system.stderr.as_str()), ("", ""));
}

#[test]
fn test_an_unwritable_log_still_starts_the_sweep_and_exits_0() {
    let mut system = fake(&three_refs(DEFAULT), Some(LOG), Some(DEFAULT));
    system.files.insert(PathBuf::from(LOG), FakeFile::Dir);
    assert_eq!(cli::run(argv(COMMITTED), &mut system), Status::Done);
    assert_eq!(system.acts.last(), Some(&the_sweep()));
    assert_eq!(system.stdout, "");
    assert_eq!(system.stderr.lines().count(), 1, "{}", system.stderr);
}

#[test]
fn test_a_missing_log_variable_logs_nothing_and_exits_0() {
    let mut system = fake(&three_refs(DEFAULT), None, Some(DEFAULT));
    assert_eq!(cli::run(argv(COMMITTED), &mut system), Status::Done);
    assert_eq!(system.acts, vec![the_sweep()]);
    assert!(
        system.stderr.contains(&Variable::Log.name()),
        "{}",
        system.stderr
    );
    assert_eq!(system.stdout, "");
}

#[test]
fn test_a_missing_default_ref_starts_no_sweep() {
    let mut system = fake(&three_refs(DEFAULT), Some(LOG), None);
    assert_eq!(cli::run(argv(COMMITTED), &mut system), Status::Done);
    assert_eq!(appended(&system.acts).len(), 3);
    assert_eq!(spawned(&system.acts), Vec::<&Act>::new());
    assert_eq!((system.stdout.as_str(), system.stderr.as_str()), ("", ""));
}

#[test]
fn test_malformed_input_is_said_on_standard_error_and_exits_0() {
    let input = format!("nonsense\n{ZERO} {A} refs/heads/ok\n");
    let mut system = fake(&input, Some(LOG), Some(DEFAULT));
    assert_eq!(cli::run(argv(COMMITTED), &mut system), Status::Done);
    assert_eq!(appended(&system.acts).len(), 1);
    assert_eq!(system.stderr.lines().count(), 1, "{}", system.stderr);
    assert_eq!(system.stdout, "");
}

#[test]
fn test_input_that_isnt_utf8_is_still_logged() {
    let mut system = fake("", Some(LOG), None)
        .with_stdin([format!("{ZERO} {A} refs/heads/").as_bytes(), b"\xff\n"].concat());
    assert_eq!(cli::run(argv(COMMITTED), &mut system), Status::Done);
    assert_eq!(appended(&system.acts)[0].1["ref"], "refs/heads/\u{fffd}");
}

/// The fake system, with the parts the hook acts through failing as
/// chosen.
struct Broken {
    fake: Fake,
    stdin: bool,
    executable: bool,
    spawn: bool,
    append: bool,
}

impl Broken {
    fn new(fake: Fake) -> Self {
        Self {
            fake,
            stdin: false,
            executable: false,
            spawn: false,
            append: false,
        }
    }

    fn fails(broken: bool, what: &str) -> Result<(), Failure> {
        match broken {
            true => Err(Failure::step_failed(format!("{what} is broken"))),
            false => Ok(()),
        }
    }
}

impl System for Broken {
    type Held = ();

    fn environment(&self) -> &Environment {
        self.fake.environment()
    }
    fn now(&self) -> Timestamp {
        self.fake.now()
    }
    fn time_zone(&self) -> TimeZone {
        self.fake.time_zone()
    }
    fn current_dir(&self) -> Result<PathBuf, Failure> {
        self.fake.current_dir()
    }
    fn executable(&self) -> Result<PathBuf, Failure> {
        Self::fails(self.executable, "the executable")?;
        self.fake.executable()
    }
    fn pid(&self) -> u32 {
        self.fake.pid()
    }
    fn stdin(&self) -> Result<Vec<u8>, Failure> {
        Self::fails(self.stdin, "standard input")?;
        self.fake.stdin()
    }
    fn git(&self, dir: &Path, args: &[OsString]) -> Result<Completed, Failure> {
        self.fake.git(dir, args)
    }
    fn process_start(&self, pid: u32) -> Result<Option<Timestamp>, Failure> {
        self.fake.process_start(pid)
    }
    fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Failure> {
        self.fake.read_file(path)
    }
    fn metadata(&self, path: &Path) -> Result<Option<Metadata>, Failure> {
        self.fake.metadata(path)
    }
    fn file_id(&self, path: &Path) -> Result<Option<FileId>, Failure> {
        self.fake.file_id(path)
    }
    fn read_link(&self, path: &Path) -> Result<Option<PathBuf>, Failure> {
        self.fake.read_link(path)
    }
    fn list_dir(&self, path: &Path) -> Result<Vec<OsString>, Failure> {
        self.fake.list_dir(path)
    }
    fn real_path(&self, path: &Path) -> Result<PathBuf, Failure> {
        self.fake.real_path(path)
    }
    fn free_bytes(&self, path: &Path) -> Result<u64, Failure> {
        self.fake.free_bytes(path)
    }
    fn volume(&self, path: &Path) -> Result<u64, Failure> {
        self.fake.volume(path)
    }
    fn git_act(&mut self, dir: &Path, args: &[OsString]) -> Result<Completed, Failure> {
        self.fake.git_act(dir, args)
    }
    fn run(
        &mut self,
        dir: &Path,
        program: &OsStr,
        args: &[OsString],
    ) -> Result<Completed, Failure> {
        self.fake.run(dir, program, args)
    }
    fn spawn_detached(
        &mut self,
        dir: &Path,
        program: &Path,
        args: &[OsString],
        env: &[(String, String)],
    ) -> Result<(), Failure> {
        Self::fails(self.spawn, "spawning")?;
        self.fake.spawn_detached(dir, program, args, env)
    }
    fn write_file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), Failure> {
        self.fake.write_file(path, bytes)
    }
    fn copy_new(&mut self, from: &Path, to: &Path) -> Result<(), Failure> {
        self.fake.copy_new(from, to)
    }
    fn append(&mut self, path: &Path, line: &[u8]) -> Result<(), Failure> {
        Self::fails(self.append, "the log")?;
        self.fake.append(path, line)
    }
    fn append_chunks(&mut self, path: &Path, chunks: &[&[u8]]) -> Result<(), Failure> {
        Self::fails(self.append, "the log")?;
        self.fake.append_chunks(path, chunks)
    }
    fn remove(&mut self, path: &Path, inside: &Path) -> Result<bool, Failure> {
        self.fake.remove(path, inside)
    }
    fn lock(&mut self, path: &Path) -> Result<Option<()>, Failure> {
        self.fake.lock(path)
    }
    fn wait_lock(&mut self, path: &Path) -> Result<(), Failure> {
        self.fake.wait_lock(path)
    }
    fn print(&mut self, text: &str) -> Result<(), Failure> {
        self.fake.print(text)
    }
    fn warn(&mut self, message: &str) {
        self.fake.warn(message)
    }
}

#[test]
fn test_every_failure_still_exits_0() {
    type Breaks = fn(&mut Broken);
    let cases: [(&str, Breaks); 5] = [
        ("standard input", |b| b.stdin = true),
        ("the executable", |b| b.executable = true),
        ("spawning", |b| b.spawn = true),
        ("the log", |b| b.append = true),
        ("everything", |b| {
            b.stdin = true;
            b.executable = true;
            b.spawn = true;
            b.append = true;
        }),
    ];
    for (what, breaks) in cases {
        let mut system = Broken::new(fake(&three_refs(DEFAULT), Some(LOG), Some(DEFAULT)));
        breaks(&mut system);
        assert_eq!(
            cli::run(argv(COMMITTED), &mut system),
            Status::Done,
            "{what}"
        );
        assert_eq!(system.fake.stdout, "", "{what}");
        assert_eq!(
            system.fake.stderr.lines().count(),
            1,
            "{what}: {}",
            system.fake.stderr
        );
        assert!(system.fake.stderr.contains("broken"), "{what}");
    }
}

#[test]
fn test_a_failed_append_stops_after_one_warning_and_still_sweeps() {
    let mut system = Broken::new(fake(&three_refs(DEFAULT), Some(LOG), Some(DEFAULT)));
    system.append = true;
    assert_eq!(cli::run(argv(COMMITTED), &mut system), Status::Done);
    assert_eq!(system.fake.acts, vec![the_sweep()]);
}

#[test]
fn test_prepared_reads_nothing() {
    let mut system = Broken::new(fake(&three_refs(DEFAULT), Some(LOG), Some(DEFAULT)));
    system.stdin = true;
    system.executable = true;
    assert_eq!(cli::run(argv("prepared"), &mut system), Status::Done);
    assert_eq!(system.fake.stderr, "");
}

// In a real repository whose hook is the shim ---------------------------------

/// Installs the shim running `program` as `repo`'s hook, and returns the
/// log it appends to.
fn install(s: &Sandbox, repo: &str, program: &str, default_ref: &str) -> String {
    let log = join(&s.var("XDG_STATE_HOME"), "it's/events.jsonl");
    let path = hook_path(repo);
    s.write(
        &path,
        &hook::shim(Path::new(program), Path::new(&log), default_ref),
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
    log
}

fn hook_path(repo: &str) -> String {
    join(repo, &format!(".git/hooks/{HOOK}"))
}

/// Runs the hook file at `path` as git would, with `args` and `input`.
fn run_file(s: &Sandbox, path: &str, args: &[&str], input: &[u8]) -> sandbox::Ran {
    let mut child = Command::new(path)
        .args(args)
        .current_dir(&s.tmp)
        .env_clear()
        .envs(s.env.iter().map(|(n, v)| (n.as_str(), v.as_str())))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the hook runs");
    // A hook that reads nothing closes the pipe, which git passes over.
    let _ = child
        .stdin
        .take()
        .map(|mut stdin| std::io::Write::write_all(&mut stdin, input));
    let output = child.wait_with_output().expect("the hook ends");
    sandbox::Ran {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

#[test]
fn test_a_shim_whose_program_wont_run_never_fails_git() {
    let s = Sandbox::new();
    let folder = join(&s.tmp, "a-folder");
    fs::create_dir_all(&folder).expect("a folder");
    for program in [join(&s.tmp, "gone"), folder] {
        let repo = s.repo(&[]);
        let log = install(&s, &repo, &program, DEFAULT);
        s.git(&repo, &["update-ref", "refs/heads/x", "HEAD"]);
        assert!(!sandbox::exists(&log), "{program}");
        assert_eq!(
            s.git(&repo, &["rev-parse", "x"]),
            s.git(&repo, &["rev-parse", "HEAD"]),
            "{program}"
        );
        fs::remove_dir_all(&repo).expect("the repository goes");
    }
}

#[test]
fn test_the_shim_runs_nothing_but_in_the_committed_state() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let log = install(&s, &repo, BIN, DEFAULT);
    let line = format!("{ZERO} {A} refs/heads/x\n");
    for args in [
        &["preparing"][..],
        &["prepared"],
        &["aborted"],
        &["prepared", "extra"],
        &["preparing", "x", "y"],
        &[],
    ] {
        let ran = run_file(&s, &hook_path(&repo), args, line.as_bytes());
        assert_eq!(
            (ran.code, ran.stdout.as_str(), ran.stderr.as_str()),
            (0, "", ""),
            "{args:?}"
        );
    }
    assert!(!sandbox::exists(&log));
    let ran = run_file(&s, &hook_path(&repo), &[COMMITTED], line.as_bytes());
    assert_eq!(ran.code, 0, "{}", ran.stderr);
    assert_eq!(logged(&log).len(), 1);
}

#[test]
fn test_the_binary_given_an_extra_argument_exits_2_and_logs_nothing() {
    let s = Sandbox::new();
    let log = join(&s.tmp, "events.jsonl");
    let ran = s.run_with_input(
        &["hook", HOOK, COMMITTED, "extra"],
        None,
        &[(&Variable::Log.name(), &log)],
        format!("{ZERO} {A} refs/heads/x\n").as_bytes(),
    );
    assert_eq!(ran.code, 2, "{}", ran.stderr);
    assert!(!sandbox::exists(&log));
}

/// Two hooks append to one log at once, many times. Each reads all its
/// input before it writes, so both are fed in full and their inputs closed
/// back to back, to start their writes together. Nothing can force the
/// writes to overlap, so the test also counts the rounds whose lines
/// interleave, and needs one.
#[test]
fn test_two_hooks_appending_at_once_never_split_a_line() {
    const ROUNDS: usize = 10;
    const LINES: usize = 5000;
    let s = Sandbox::new();
    let input = |who: &str| -> String {
        (0..LINES)
            .map(|i| format!("{A} {B} refs/heads/{who}/{i:04}\n"))
            .collect()
    };
    let inputs = [input("first"), input("second")];
    let interleaved = (0..ROUNDS)
        .filter(|round| {
            let log = join(&s.tmp, &format!("events-{round}.jsonl"));
            let children = inputs.each_ref().map(|text| {
                let mut child = Command::new(BIN)
                    .args(["hook", HOOK, COMMITTED])
                    .env_clear()
                    .envs(s.env.iter().map(|(n, v)| (n.as_str(), v.as_str())))
                    .env(Variable::Log.name(), &log)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("the hook runs");
                let stdin = child.stdin.take().map(|mut stdin| {
                    std::io::Write::write_all(&mut stdin, text.as_bytes()).expect("its input");
                    stdin
                });
                (child, stdin)
            });
            let children = children.map(|(child, stdin)| {
                drop(stdin);
                child
            });
            children.into_iter().for_each(|mut child| {
                assert!(child.wait().expect("the hook ends").success());
            });
            let lines = logged(&log);
            assert_eq!(lines.len(), 2 * LINES);
            let whose: Vec<bool> = lines
                .iter()
                .map(|line| {
                    line["ref"]
                        .as_str()
                        .is_some_and(|r| r.starts_with("refs/heads/first/"))
                })
                .collect();
            assert_eq!(whose.iter().filter(|first| **first).count(), LINES);
            whose.windows(2).filter(|pair| pair[0] != pair[1]).count() > 1
        })
        .count();
    let _ = std::io::Write::write_all(
        &mut std::io::stderr(),
        format!("two hooks' lines interleaved in {interleaved} of {ROUNDS} rounds\n").as_bytes(),
    );
    assert!(interleaved > 0, "the hooks never wrote at once");
}

/// Runs `change` in `repo`, and checks that the last line `log` holds for
/// `reference` deletes it.
fn deletion_logged(s: &Sandbox, repo: &str, log: &str, reference: &str, change: &[&str]) {
    s.git(repo, change);
    let lines = logged(log);
    let deleted = for_ref(&lines, reference);
    assert_eq!(
        deleted.last().map(|line| &line["new"]),
        Some(&Value::from(ZERO)),
        "{change:?}: {lines:?}"
    );
}

#[test]
fn test_every_kind_of_deletion_is_logged() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let log = install(&s, &repo, BIN, DEFAULT);
    s.git(&repo, &["branch", "by-update-ref"]);
    s.git(&repo, &["branch", "by-branch"]);
    s.git(
        &repo,
        &["push", "--quiet", "origin", "HEAD:refs/heads/pruned"],
    );
    s.git(&repo, &["fetch", "--quiet", "origin"]);
    deletion_logged(
        &s,
        &repo,
        &log,
        "refs/heads/by-update-ref",
        &["update-ref", "-d", "refs/heads/by-update-ref"],
    );
    deletion_logged(
        &s,
        &repo,
        &log,
        "refs/heads/by-branch",
        &["branch", "--quiet", "--delete", "--force", "by-branch"],
    );
    let remote = join(&s.tmp, "repo.remote.git");
    s.git(&remote, &["update-ref", "-d", "refs/heads/pruned"]);
    deletion_logged(
        &s,
        &repo,
        &log,
        "refs/remotes/origin/pruned",
        &["fetch", "--quiet", "--prune", "origin"],
    );
}

fn logged(log: &str) -> Vec<Value> {
    sandbox::read(log)
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON line"))
        .collect()
}

fn for_ref<'a>(lines: &'a [Value], reference: &str) -> Vec<&'a Value> {
    lines
        .iter()
        .filter(|line| line["ref"] == reference)
        .collect()
}

#[test]
fn test_update_ref_and_a_fetch_write_the_log() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let log = install(&s, &repo, BIN, DEFAULT);
    let head = s.git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();

    s.git(&repo, &["update-ref", "refs/heads/x", "HEAD"]);
    let lines = logged(&log);
    let created = for_ref(&lines, "refs/heads/x");
    assert_eq!(created.len(), 1, "{lines:?}");
    assert_eq!(
        (created[0]["event"].as_str(), created[0]["old"].as_str()),
        (Some("ref"), Some(ZERO))
    );
    assert_eq!(created[0]["new"], head.as_str());

    let other = join(&s.tmp, "other");
    s.sh(
        &[
            "git",
            "clone",
            "--quiet",
            &join(&s.tmp, "repo.remote.git"),
            &other,
        ],
        None,
    );
    let theirs = s.commit(&other, "theirs.txt", "theirs\n");
    s.git(&other, &["push", "--quiet", "origin", "main"]);
    let report = sweep_report(&s, &repo);
    s.git(&repo, &["fetch", "--quiet", "origin"]);
    let lines = logged(&log);
    let fetched = for_ref(&lines, DEFAULT);
    assert_eq!(
        fetched.last().map(|line| &line["new"]),
        Some(&Value::from(theirs))
    );
    assert_swept(&report, &repo);
}

/// Makes `repo`'s last-sweep report a FIFO, read whole by a thread, so
/// the sweep the hook starts blocks there until the test reads what it
/// planned. Under `--unattended` with no setting it only plans, in the
/// sandbox's state folder.
fn sweep_report(s: &Sandbox, repo: &str) -> mpsc::Receiver<String> {
    let common = s.git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    );
    let state = names::state_dir(&s.environment(), Path::new(&real(common.trim())))
        .expect("a state folder");
    assert!(state.starts_with(&s.tmp), "{}", state.display());
    fs::create_dir_all(&state).expect("the state folder");
    let fifo = state.join(LAST_SWEEP_FILE);
    s.sh(&["mkfifo", &fifo.to_string_lossy()], None);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(fs::read_to_string(&fifo).unwrap_or_default());
    });
    receiver
}

/// The detached sweep planned `repo`, and nothing else, without acting.
fn assert_swept(report: &mpsc::Receiver<String>, repo: &str) {
    let text = report
        .recv_timeout(Duration::from_secs(60))
        .expect("the detached sweep's report");
    let report: Value = serde_json::from_str(&text).expect("a JSON report");
    assert_eq!(report["command"], "sweep");
    assert_eq!(report["dry_run"], true, "{report:#}");
    let paths: Vec<&Value> = report["repositories"]
        .as_array()
        .expect("repositories")
        .iter()
        .map(|r| &r["path"])
        .collect();
    assert_eq!(paths, [&Value::from(real(repo))]);
}

// Timed as the built binary ---------------------------------------------------

/// The time one run of the hook takes, from its start to its exit.
fn timed(s: &Sandbox, state: &str, input: &[u8], log: &str) -> Duration {
    let start = Instant::now();
    let mut child = Command::new(BIN)
        .args(["hook", HOOK, state])
        .current_dir(&s.tmp)
        .env_clear()
        .envs(s.env.iter().map(|(n, v)| (n.as_str(), v.as_str())))
        .env(Variable::Log.name(), log)
        .env(Variable::DefaultRef.name(), DEFAULT)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the program runs");
    // A run that reads nothing closes the pipe, which git passes over too.
    let _ = child
        .stdin
        .take()
        .map(|mut stdin| std::io::Write::write_all(&mut stdin, input));
    let status = child.wait().expect("the program ends");
    let elapsed = start.elapsed();
    assert!(status.success());
    elapsed
}

/// The binary's own cost. A timing budget, run by name in a release
/// build: `cargo test --release --test hook -- --ignored --nocapture`.
#[test]
#[ignore = "a timing budget, run by name with --ignored in a release build"]
fn test_a_1000_ref_transaction_costs_the_hook_under_5_ms() {
    const RUNS: usize = 15;
    const BUDGET: Duration = Duration::from_millis(5);
    let s = Sandbox::new();
    let log = join(&s.tmp, "events.jsonl");
    let input: String = (0..1000)
        .map(|i| format!("{A} {B} refs/remotes/origin/b{i:04}\n"))
        .collect();
    // Each figure is the whole hook process, from its start to its exit,
    // the fastest of RUNS, since other work on the machine only ever adds
    // to a run. The budget is the `committed` run's; in the other states
    // the shim never starts the binary, whose early return they test.
    let fastest = |state: &str| {
        (0..RUNS)
            .map(|_| timed(&s, state, input.as_bytes(), &log))
            .min()
            .unwrap_or(Duration::MAX)
    };
    let states = ["preparing", "prepared", COMMITTED].map(|state| (state, fastest(state)));
    // The floor: a process that does nothing, as `true` does.
    let floor = (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            let ran = Command::new("true").envs(s.env.iter().cloned()).status();
            assert!(matches!(ran, Ok(status) if status.success()));
            start.elapsed()
        })
        .min()
        .unwrap_or(Duration::MAX);
    // Written past the test harness's capture, so a passing run's build
    // log holds the figures too.
    let _ = std::io::Write::write_all(
        &mut std::io::stderr(),
        format!(
            "a 1,000-ref transaction's hook runs, fastest of {RUNS}: {states:?}; true: {floor:?}\n"
        )
        .as_bytes(),
    );
    assert_eq!(sandbox::read(&log).lines().count(), 1000 * RUNS);
    let committed = states[2].1;
    assert!(committed < BUDGET, "{committed:?} for {COMMITTED}");
}

// A 1,000-ref fetch, timed with and without the hook ---------------------------

const BRANCHES: usize = 1000;

/// `git <args>` in `repo` with `input` on standard input.
fn git_with_input(s: &Sandbox, repo: &str, args: &[&str], input: &str) {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env_clear()
        .envs(s.env.iter().map(|(n, v)| (n.as_str(), v.as_str())))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("git runs");
    let written = child
        .stdin
        .take()
        .map(|mut stdin| std::io::Write::write_all(&mut stdin, input.as_bytes()));
    let output = child.wait_with_output().expect("git ends");
    assert!(matches!(written, Some(Ok(()))));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Points the remote's branches `b0000`... at `target`, in one
/// transaction; `verb` is `create` or `update`.
fn point_branches(s: &Sandbox, remote: &str, verb: &str, target: &str) {
    let input: String = (0..BRANCHES)
        .map(|i| format!("{verb} refs/heads/b{i:04} {target}\n"))
        .collect();
    git_with_input(s, remote, &["update-ref", "--stdin"], &input);
}

/// The time `git fetch` takes in `clone`, with git's auto maintenance,
/// which packs refs, off.
fn fetch(s: &Sandbox, clone: &str) -> Duration {
    let start = Instant::now();
    let status = Command::new("git")
        .args(["-C", clone, "-c", "maintenance.auto=false"])
        .args(["fetch", "--quiet", "origin"])
        .env_clear()
        .envs(s.env.iter().map(|(n, v)| (n.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let took = start.elapsed();
    assert!(matches!(status, Ok(status) if status.success()), "{clone}");
    took
}

fn line_count(log: &str) -> usize {
    fs::read_to_string(log).map_or(0, |text| text.lines().count())
}

/// Three clones of one remote: one with no hook, one with an empty hook,
/// and one with the shim, which appends to `log`.
struct Bench {
    s: Sandbox,
    seed: String,
    remote: String,
    chain: Vec<String>,
    clones: [String; 3],
    log: String,
}

/// The bench, its remote holding the 1,000 branches and a chain of
/// `rounds` + 2 commits, with each clone warmed by a first fetch, which
/// also lets macOS check a new hook.
fn bench(rounds: usize) -> Bench {
    let s = Sandbox::new();
    let seed = s.cloned();
    // A chain of commits, so each round moves every branch forward, as a
    // normal fetch does: git batches fast-forwards into one transaction.
    let chain: Vec<String> =
        std::iter::once(s.git(&seed, &["rev-parse", "HEAD"]).trim().to_owned())
            .chain((0..rounds + 1).map(|i| s.commit(&seed, &format!("c{i}.txt"), "c\n")))
            .collect();
    s.git(&seed, &["push", "--quiet", "origin", "main"]);
    let remote = join(&s.tmp, "repo.remote.git");
    point_branches(&s, &remote, "create", &chain[0]);
    let clones = ["plain", "empty", "hooked"].map(|name| {
        let path = join(&s.tmp, name);
        s.sh(&["git", "clone", "--quiet", &remote, &path], None);
        path
    });
    let empty_hook = hook_path(&clones[1]);
    s.write(&empty_hook, "#!/bin/sh\nexit 0\n");
    fs::set_permissions(&empty_hook, fs::Permissions::from_mode(0o755)).expect("executable");
    let log = install(&s, &clones[2], BIN, DEFAULT);
    point_branches(&s, &remote, "update", &chain[1]);
    clones.iter().for_each(|clone| {
        fetch(&s, clone);
    });
    Bench {
        s,
        seed,
        remote,
        chain,
        clones,
        log,
    }
}

impl Bench {
    /// Moves every branch to the chain's commit for `round`, then fetches
    /// in each clone, in an order that rotates so no clone always fetches
    /// first. Gives each clone's time, and the lines the shim logged.
    fn round(&self, round: usize) -> ([Duration; 3], Vec<Value>) {
        point_branches(&self.s, &self.remote, "update", &self.chain[round + 2]);
        let before = line_count(&self.log);
        let times: std::collections::BTreeMap<usize, Duration> = (0..3)
            .map(|k| (round + k) % 3)
            .map(|clone| (clone, fetch(&self.s, &self.clones[clone])))
            .collect();
        (
            [0, 1, 2].map(|clone| times[&clone]),
            logged(&self.log).into_iter().skip(before).collect(),
        )
    }
}

/// Each line's ref and new object name.
fn moves(lines: &[Value]) -> std::collections::BTreeSet<(String, String)> {
    lines
        .iter()
        .map(|line| (line["ref"].to_string(), line["new"].to_string()))
        .collect()
}

#[test]
fn test_a_1000_ref_fetch_logs_each_ref_once() {
    let bench = bench(2);
    for round in 0..2 {
        let (_, lines) = bench.round(round);
        let target = &bench.chain[round + 2];
        let expected: Vec<Value> = (0..BRANCHES)
            .map(|i| serde_json::json!({"ref": format!("refs/remotes/origin/b{i:04}"), "new": target}))
            .collect();
        assert_eq!(lines.len(), BRANCHES, "round {round}");
        assert_eq!(moves(&lines), moves(&expected), "round {round}");
    }
    // Only the default branch's line starts the sweep, which plans the
    // clone it ran in.
    let tip = bench.s.commit(&bench.seed, "moved.txt", "moved\n");
    bench
        .s
        .git(&bench.seed, &["push", "--quiet", "origin", "main"]);
    let before = line_count(&bench.log);
    let report = sweep_report(&bench.s, &bench.clones[2]);
    fetch(&bench.s, &bench.clones[2]);
    let lines: Vec<Value> = logged(&bench.log).into_iter().skip(before).collect();
    assert_eq!(
        moves(&lines),
        moves(&[serde_json::json!({"ref": DEFAULT, "new": tip})])
    );
    assert_swept(&report, &bench.clones[2]);
}

/// The shim's cost on a fetch. A timing budget, run by name in a release
/// build: `cargo test --release --test hook -- --ignored --nocapture`.
///
/// It holds the budget as the Python test did: the shim's fetch is slower
/// than the empty hook's by under 5 ms plus two process starts, the
/// Python's allowance for its two process images, here the shim's `sh`
/// and the binary it execs. A process start is the mean of 20 runs of
/// `true`, as the Python took it of `awk`.
#[test]
#[ignore = "a timing budget, run by name with --ignored in a release build"]
fn test_the_shim_adds_under_5_ms_to_a_1000_ref_fetch() {
    const ROUNDS: usize = 12;
    const STARTS: u32 = 20;
    const BUDGET: Duration = Duration::from_millis(5);
    let bench = bench(ROUNDS);
    let rounds: Vec<([Duration; 3], usize)> = (0..ROUNDS)
        .map(|round| {
            let (times, lines) = bench.round(round);
            (times, lines.len())
        })
        .collect();
    let s = &bench.s;

    let start = (0..STARTS)
        .map(|_| {
            let begun = Instant::now();
            let ran = Command::new("true").envs(s.env.iter().cloned()).status();
            assert!(matches!(ran, Ok(status) if status.success()));
            begun.elapsed()
        })
        .sum::<Duration>()
        / STARTS;
    // A fetch takes hundreds of ms and other work on the machine only ever
    // adds to it, so each clone's figure is its fastest fetch.
    let fastest = [0, 1, 2].map(|clone| {
        rounds
            .iter()
            .map(|(times, _)| times[clone])
            .min()
            .unwrap_or(Duration::MAX)
    });
    let over_plain = fastest[2].saturating_sub(fastest[0]);
    let over_empty = fastest[2].saturating_sub(fastest[1]);
    let allowed = 2 * start + BUDGET;
    let _ = std::io::Write::write_all(
        &mut std::io::stderr(),
        format!(
            "a {BRANCHES}-ref fetch, fastest of {ROUNDS}: {:?} with no hook, {:?} with an empty hook, {:?} with the shim: {over_plain:?} over no hook, {over_empty:?} over the empty hook, of {allowed:?} allowed (a process start: {start:?})\n",
            fastest[0], fastest[1], fastest[2]
        )
        .as_bytes(),
    );
    assert!(
        rounds.iter().all(|(_, lines)| *lines == BRANCHES),
        "lines per fetch: {:?}",
        rounds.iter().map(|(_, lines)| lines).collect::<Vec<_>>()
    );
    assert!(
        over_empty < allowed,
        "the shim added {over_empty:?} over the empty hook, of {allowed:?} allowed"
    );
}
