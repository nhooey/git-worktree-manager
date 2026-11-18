//! The edge: git, the file system, the clock, standard output and error.
//!
//! Each function here runs one command or touches one path and returns
//! plain values for [`crate::build_root`] and [`crate::rules`] to decide on.
//! Every git command runs with `--no-optional-locks`, so reading a
//! worktree's status never rewrites its index, whose time the idle rule
//! reads. Locating a repository and listing its worktrees read through
//! gitoxide, which writes nothing.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

use crate::PROGRAM;
use crate::build_root::{self, Env, Layout, Platform};
use crate::names;
use crate::python::{self, dirname, path_join};
use crate::rules;

/// The variables every git command runs with: no prompt for credentials,
/// and messages in English for the program to read.
const GIT_ENV: [(&str, &str); 2] = [("GIT_TERMINAL_PROMPT", "0"), ("LC_ALL", "C")];

/// The longest log line written whole; a longer one loses its lists and
/// objects, so one `write` with `O_APPEND` stays atomic.
const LOG_LINE_LIMIT: usize = 4000;

/// The settings the program ships with.
const DEFAULTS: &str = include_str!("../worktrees/defaults.json");

/// Why a command failed, as the message it prints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure(pub String);

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// An operating system error as Python prints it:
/// `[Errno 2] No such file or directory: '/x'`.
pub fn os_error(error: &io::Error, path: &str) -> Failure {
    let text = error.to_string();
    match error.raw_os_error() {
        Some(code) => {
            let reason = text
                .strip_suffix(&format!(" (os error {code})"))
                .unwrap_or(&text);
            Failure(format!("[Errno {code}] {reason}: '{path}'"))
        }
        None => Failure(format!("{text}: '{path}'")),
    }
}

// Output ----------------------------------------------------------------------

pub fn warn(message: &str) {
    // Nothing is left to report a failure to write standard error to.
    let _ = writeln!(io::stderr(), "{PROGRAM}: {message}");
}

/// Writes `text` to standard output. A reader that has gone away is no
/// failure: the program's work is done either way.
pub fn print(text: &str) -> Result<(), Failure> {
    let mut out = io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => {
            Err(Failure(format!("standard output: {error}")))
        }
        _ => Ok(()),
    }
}

// Commands --------------------------------------------------------------------

/// What a finished command returned.
pub struct Completed {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// The exit code, or the negated signal that ended the process, as Python's
/// `returncode` gives it.
fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|signal| -signal))
        .unwrap_or(-1)
}

/// Runs `argv` with no standard input, with `extra_env` over the
/// environment, and returns its status and output.
pub fn run(argv: &[&str], extra_env: &[(&str, &str)]) -> Result<Completed, Failure> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| Failure("no command".into()))?;
    let output = Command::new(program)
        .args(args)
        .envs(extra_env.iter().copied())
        .stdin(Stdio::null())
        .output()
        .map_err(|error| os_error(&error, program))?;
    Ok(Completed {
        code: exit_code(output.status),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Runs `argv` and fails unless it exits 0.
pub fn run_checked(argv: &[&str], extra_env: &[(&str, &str)]) -> Result<Completed, Failure> {
    let result = run(argv, extra_env)?;
    if result.code == 0 {
        Ok(result)
    } else {
        Err(Failure(format!(
            "{} exited {}: {}",
            argv.join(" "),
            result.code,
            python::strip(&result.stderr)
        )))
    }
}

/// `git` in `repo`, without optional locks and with paths unquoted.
fn git_argv<'a>(repo: &'a str, args: &[&'a str]) -> Vec<&'a str> {
    [
        "git",
        "--no-optional-locks",
        "-C",
        repo,
        "-c",
        "core.quotePath=false",
    ]
    .into_iter()
    .chain(args.iter().copied())
    .collect()
}

/// git's standard output; fails unless it exits 0.
pub fn git(repo: &str, args: &[&str]) -> Result<String, Failure> {
    run_checked(&git_argv(repo, args), &GIT_ENV).map(|done| done.stdout)
}

/// The first executable called `name` on the `PATH`, as `shutil.which`
/// finds it.
pub fn which(env: &Env, name: &str) -> Option<String> {
    env.get("PATH")?
        .split(':')
        .map(|folder| path_join(if folder.is_empty() { "." } else { folder }, &[name]))
        .find(|candidate| {
            fs::metadata(candidate)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

// The clock and the platform --------------------------------------------------

/// Seconds since the epoch.
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

/// The local time to the second, with its offset: `2026-11-18T09:12:00-05:00`.
pub fn now_iso() -> String {
    chrono::Local::now()
        .format("%Y-%m-%dT%H:%M:%S%:z")
        .to_string()
}

/// The local time at `seconds` as `time.strftime` writes it.
pub fn local_time(seconds: f64, format: &str) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(seconds.floor() as i64, 0)
        .earliest()
        .map_or_else(String::new, |at| at.format(format).to_string())
}

pub const fn platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Darwin
    } else {
        Platform::Other
    }
}

// Paths -----------------------------------------------------------------------

fn lossy(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub fn current_dir() -> Result<String, Failure> {
    std::env::current_dir()
        .map(|dir| lossy(&dir))
        .map_err(|error| os_error(&error, "."))
}

/// `os.path.realpath`: symbolic links resolved, and the parts that don't
/// exist kept as written, with `..` taken after the links before it.
pub fn real(path: &str) -> String {
    if let Ok(resolved) = fs::canonicalize(path) {
        return lossy(&resolved);
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.is_empty() {
            current_dir().unwrap_or_default()
        } else {
            "/".to_owned()
        };
    }
    let (parent, name) = match trimmed.rfind('/') {
        Some(0) => ("/", &trimmed[1..]),
        Some(at) => (&trimmed[..at], &trimmed[at + 1..]),
        None => ("", trimmed),
    };
    let base = real(parent);
    match name {
        "." => base,
        ".." => dirname(&base).to_owned(),
        _ => path_join(&base, &[name]),
    }
}

pub fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

pub fn is_dir(path: &str) -> bool {
    Path::new(path).is_dir()
}

pub fn is_file(path: &str) -> bool {
    Path::new(path).is_file()
}

/// The names in a folder, in the order the file system gives them.
pub fn list_dir(path: &str) -> Result<Vec<String>, Failure> {
    fs::read_dir(path)
        .and_then(|entries| {
            entries
                .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
                .collect()
        })
        .map_err(|error| os_error(&error, path))
}

/// A file's modification time and size, following symbolic links.
pub struct FileStat {
    pub modified: f64,
    pub size: u64,
}

/// The regular files in a folder, following symbolic links, with their
/// modification time computed as Python's `st_mtime` is.
pub fn files_in(path: &str) -> Result<Vec<(String, FileStat)>, Failure> {
    let names = list_dir(path)?;
    Ok(names
        .into_iter()
        .filter_map(|name| {
            let meta = fs::metadata(path_join(path, &[&name])).ok()?;
            meta.is_file().then(|| {
                let modified = meta.mtime() as f64 + meta.mtime_nsec() as f64 * 1e-9;
                (
                    name,
                    FileStat {
                        modified,
                        size: meta.size(),
                    },
                )
            })
        })
        .collect())
}

pub fn read_text(path: &str) -> Option<String> {
    fs::read(path)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

pub fn read_bytes(path: &str) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

fn make_dirs(path: &str) -> Result<(), Failure> {
    fs::create_dir_all(path).map_err(|error| os_error(&error, path))
}

pub fn write_text(path: &str, text: &str) -> Result<(), Failure> {
    make_dirs(dirname(path))?;
    fs::write(path, text).map_err(|error| os_error(&error, path))
}

/// Copies `source` to `target`, which must not exist yet.
pub fn copy_new(source: &str, target: &str) -> Result<(), Failure> {
    make_dirs(dirname(target))?;
    let bytes = fs::read(source).map_err(|error| os_error(&error, source))?;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .and_then(|mut file| file.write_all(&bytes))
        .map_err(|error| os_error(&error, target))
}

/// Deletes `path`, a file or folder the program found under `inside`;
/// false when it is gone already.
pub fn remove_tree(path: &str, inside: &str) -> Result<bool, Failure> {
    let resolved = real(path);
    let folder = real(inside);
    if !build_root::is_inside(&resolved, &folder) || resolved == folder {
        return Err(Failure(format!(
            "refusing to delete {path} outside {inside}"
        )));
    }
    let gone = |result: io::Result<()>| result.map(|()| true).map_err(|e| os_error(&e, path));
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_symlink() => gone(fs::remove_file(path)),
        Ok(_) if is_file(path) => gone(fs::remove_file(path)),
        Ok(_) if is_dir(path) => gone(fs::remove_dir_all(path)),
        _ => Ok(false),
    }
}

/// `top` and every folder under it, not following symbolic links.
fn folders_under(top: &str) -> Vec<String> {
    let children = fs::read_dir(top).map_or_else(
        |_| Vec::new(),
        |entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .flat_map(|entry| folders_under(&lossy(&entry.path())))
                .collect()
        },
    );
    std::iter::once(top.to_owned()).chain(children).collect()
}

/// Removes every empty folder under `top`, and `top` when it ends empty.
pub fn prune_empty_dirs(top: &str) {
    if !is_dir(top) || fs::symlink_metadata(top).is_ok_and(|meta| meta.is_symlink()) {
        return;
    }
    let mut folders = folders_under(top);
    folders.sort_by_key(|folder| std::cmp::Reverse(folder.len()));
    // A folder that isn't empty stays, which is the point.
    folders.iter().for_each(|folder| {
        let _ = fs::remove_dir(folder);
    });
}

// Locations -------------------------------------------------------------------

/// Where a worktree's repository is, as real paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Located {
    pub common_dir: String,
    pub git_dir: String,
    pub toplevel: String,
}

fn open(path: &str) -> Option<gix::Repository> {
    gix::ThreadSafeRepository::discover_with_environment_overrides(path)
        .ok()
        .map(gix::Repository::from)
}

/// A path gitoxide gave, made absolute against the directory it read from.
fn absolute(repo: &gix::Repository, path: &Path) -> String {
    real(&lossy(&repo.current_dir().join(path)))
}

/// The repository of the worktree holding `path`; None outside a worktree,
/// and in a bare repository.
pub fn locate(path: &str) -> Option<Located> {
    if !is_dir(path) {
        return None;
    }
    let repo = open(path)?;
    let found = Located {
        common_dir: absolute(&repo, repo.common_dir()),
        git_dir: absolute(&repo, repo.git_dir()),
        toplevel: absolute(&repo, repo.workdir()?),
    };
    // gitoxide gives a folder inside `.git` the worktree around it; git
    // says it is in no worktree.
    let here = real(path);
    let in_git_dir = [&found.git_dir, &found.common_dir]
        .iter()
        .any(|dir| build_root::is_inside(&here, dir));
    (!in_git_dir).then_some(found)
}

pub fn layout_for(env: &Env, path: &str) -> Option<Layout> {
    locate(path)
        .map(|found| build_root::layout(env, &found.common_dir, &found.git_dir, &found.toplevel))
}

/// A worktree of a repository, as these commands need it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub path: String,
    pub bare: bool,
    pub is_main: bool,
}

/// The main checkout, then each linked worktree by path, as `git worktree
/// list` gives them; a path that exists is a real path. git names the main
/// checkout by its common directory less a final `/.git`.
pub fn worktrees(repo_path: &str) -> Result<Vec<Listed>, Failure> {
    let failed = |error: gix::Error| Failure(format!("{repo_path}: {error}"));
    let repo =
        open(repo_path).ok_or_else(|| Failure(format!("{repo_path}: not a git repository")))?;
    let common_dir = absolute(&repo, repo.common_dir());
    let main = Listed {
        path: common_dir
            .strip_suffix("/.git")
            .unwrap_or(&common_dir)
            .to_owned(),
        bare: repo.main_repo().map_err(failed)?.is_bare(),
        is_main: true,
    };
    let mut linked: Vec<String> = repo
        .worktrees()
        .map_err(failed)?
        .iter()
        .map(|proxy| proxy.base().map(|base| lossy(&base)).map_err(failed))
        .collect::<Result<_, _>>()?;
    linked.sort();
    Ok(std::iter::once(main)
        .chain(linked.into_iter().map(|path| Listed {
            path,
            bare: false,
            is_main: false,
        }))
        .map(|w| Listed {
            path: if exists(&w.path) {
                real(&w.path)
            } else {
                w.path
            },
            ..w
        })
        .collect())
}

// The log ---------------------------------------------------------------------

/// Appends one event to the repository's log, stamped with the time and
/// this process: one JSON line, in one `write` to a file opened with
/// `O_APPEND`.
pub fn log_at(
    state_dir: &str,
    common_dir: &str,
    event: &str,
    source: &str,
    fields: Map<String, Value>,
) -> Result<(), Failure> {
    let stamped: Map<String, Value> = [
        ("time", Value::from(rules::iso(now()))),
        ("repo", Value::from(common_dir)),
        ("event", Value::from(event)),
        ("source", Value::from(source)),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .chain(
        fields
            .into_iter()
            .filter(|(key, value)| !value.is_null() && key != "event"),
    )
    .chain([("pid".to_owned(), Value::from(std::process::id()))])
    .collect();
    let line = python::json_dumps(&Value::Object(stamped.clone()), None);
    let fitted = if line.len() < LOG_LINE_LIMIT {
        line
    } else {
        let flat = stamped
            .into_iter()
            .filter(|(_, value)| !value.is_array() && !value.is_object())
            .collect();
        python::json_dumps(&Value::Object(flat), None)
    };
    make_dirs(state_dir)?;
    let path = path_join(state_dir, &[names::EVENTS_FILE]);
    fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o644)
        .open(&path)
        .and_then(|mut file| file.write_all(format!("{fitted}\n").as_bytes()))
        .map_err(|error| os_error(&error, &path))
}

// Configuration ---------------------------------------------------------------

/// The shipped defaults with `~/.config/worktrees/config.json` merged over
/// them; a setting the defaults lack, or a file that isn't a JSON object,
/// is reported.
pub fn load_config(env: &Env) -> Map<String, Value> {
    let defaults = match serde_json::from_str(DEFAULTS) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    let user_path = path_join(
        &build_root::config_home(env),
        &[names::CONFIG_DIR, names::CONFIG_FILE],
    );
    let user = match read_text(&user_path).filter(|text| !text.is_empty()) {
        None => Map::new(),
        Some(text) => match serde_json::from_str(&text) {
            Ok(Value::Object(map)) => map,
            Ok(_) => {
                warn(&format!(
                    "{user_path}: not a JSON object; using the defaults"
                ));
                Map::new()
            }
            Err(error) => {
                warn(&format!("{user_path}: {error}; using the defaults"));
                Map::new()
            }
        },
    };
    build_root::unknown_keys(&defaults, &user)
        .iter()
        .for_each(|key| warn(&format!("{user_path}: unknown setting {key}")));
    build_root::merge_config(&defaults, &user)
}

// The build root --------------------------------------------------------------

/// Creates the repository's and worktree's folders with their `REPO` and
/// `WORKTREE` files. The call that creates the top folder excludes it from
/// Time Machine and Spotlight on macOS.
pub fn ensure_build_dirs(env: &Env, lay: &Layout) -> Result<(), Failure> {
    let top = dirname(&lay.root);
    let fresh_top = !is_dir(top);
    make_dirs(&lay.wt)?;
    if fresh_top && platform() == Platform::Darwin {
        write_text(&path_join(top, &[".metadata_never_index"]), "")?;
        if let Some(tmutil) = which(env, "tmutil") {
            let result = run(&[&tmutil, "addexclusion", top], &[])?;
            if result.code != 0 {
                warn(&format!(
                    "tmutil addexclusion {top}: {}",
                    python::strip(&result.stderr)
                ));
            }
        }
    }
    [
        (path_join(&lay.root, &[names::REPO_FILE]), &lay.common_dir),
        (path_join(&lay.wt, &[names::WORKTREE_FILE]), &lay.worktree),
    ]
    .iter()
    .filter(|(path, value)| read_text(path).as_deref() != Some(&format!("{value}\n")))
    .try_for_each(|(path, value)| write_text(path, &format!("{value}\n")))
}

/// Claude Code's folder of projects, one per working directory.
pub fn claude_projects(env: &Env) -> String {
    let base = env
        .get("CLAUDE_CONFIG_DIR")
        .filter(|dir| !dir.is_empty())
        .cloned()
        .unwrap_or_else(|| path_join(env.get("HOME").map_or("", String::as_str), &[".claude"]));
    path_join(&base, &["projects"])
}
