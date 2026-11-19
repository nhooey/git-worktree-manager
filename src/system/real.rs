//! The system the binary runs on.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use jiff::Timestamp;
use jiff::tz::TimeZone;

use super::{Completed, Failure, FileId, FileKind, Metadata, System, is_inside, real_path};
use crate::environment::Environment;
use crate::names::PROGRAM;
use crate::time::parse_lstart;

/// The variables every command runs with: no prompt for credentials, and
/// messages and dates in English.
const QUIET_ENGLISH: [(&str, &str); 2] = [("GIT_TERMINAL_PROMPT", "0"), ("LC_ALL", "C")];

pub struct Real {
    environment: Environment,
}

impl Real {
    /// The system, with the environment the process started with.
    pub fn new(environment: Environment) -> Self {
        Self { environment }
    }
}

fn io_detail(error: &io::Error, path: &Path) -> String {
    format!("{}: {error}", path.display())
}

/// A read of `path` that failed.
fn unreadable(error: &io::Error, path: &Path) -> Failure {
    Failure::unreadable(io_detail(error, path))
}

/// A change to `path` that failed.
fn step_failed(error: &io::Error, path: &Path) -> Failure {
    Failure::step_failed(io_detail(error, path))
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| -signal))
        .unwrap_or(-1)
}

/// The variables that point git at a repository, as `git rev-parse
/// --local-env-vars` lists them. A hook inherits some of them, and they
/// would win over the repository a `git` child, or the program started
/// detached, is given: an inherited `GIT_OBJECT_DIRECTORY` would let the
/// compare pass find in the clone the objects the backup lacks.
const GIT_LOCAL_VARIABLES: [&str; 15] = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

/// The variables that move git's global and system configuration files,
/// which the program's own reads don't follow.
const GIT_CONFIG_FILE_VARIABLES: [&str; 3] = [
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_CONFIG_NOSYSTEM",
];

/// The variables a `git` child, or the program started detached, runs
/// without, so it reads the repository and configuration the program
/// reads.
fn cleared() -> impl Iterator<Item = &'static str> {
    GIT_LOCAL_VARIABLES
        .into_iter()
        .chain(GIT_CONFIG_FILE_VARIABLES)
}

/// `program` in `dir`, without the variables `unset`.
fn run_in<'a>(
    dir: &Path,
    program: &OsStr,
    args: &[OsString],
    unset: impl IntoIterator<Item = &'a str>,
) -> io::Result<Completed> {
    unset
        .into_iter()
        .fold(Command::new(program), |mut command, name| {
            command.env_remove(name);
            command
        })
        .args(args)
        .current_dir(dir)
        .envs(QUIET_ENGLISH)
        .stdin(Stdio::null())
        .output()
        .map(|output| Completed {
            code: exit_code(output.status),
            stdout: output.stdout,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
}

/// `git` in `dir`, on the repository there whatever the environment says,
/// without optional locks and with paths unquoted.
fn run_git(dir: &Path, args: &[OsString]) -> io::Result<Completed> {
    let args: Vec<OsString> = ["--no-optional-locks", "-c", "core.quotePath=false"]
        .into_iter()
        .map(OsString::from)
        .chain(args.iter().cloned())
        .collect();
    run_in(dir, OsStr::new("git"), &args, cleared())
}

fn make_parent(path: &Path) -> Result<(), Failure> {
    path.parent().map_or(Ok(()), |parent| {
        fs::create_dir_all(parent).map_err(|e| step_failed(&e, parent))
    })
}

/// The lock file at `path`, opened to take `flock` on, and made with its
/// folder when missing.
fn lock_file(path: &Path) -> Result<fs::File, Failure> {
    make_parent(path)?;
    fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|error| step_failed(&error, path))
}

/// Writes each chunk in one `write`, stopping at the first that fails. A
/// short write's rest is written before the failure is reported, so the
/// next line appended never joins a line cut short.
pub(super) fn write_chunks(
    out: &mut impl Write,
    path: &Path,
    chunks: &[&[u8]],
) -> Result<(), Failure> {
    chunks.iter().try_for_each(|chunk| match out.write(chunk) {
        Ok(written) if written == chunk.len() => Ok(()),
        Ok(written) => Err(Failure::step_failed(format!(
            "{}: wrote {written} of {} bytes in one write, the rest {}",
            path.display(),
            chunk.len(),
            match out.write_all(&chunk[written..]) {
                Ok(()) => "after it".to_owned(),
                Err(error) => format!("not: {error}"),
            }
        ))),
        Err(error) => Err(step_failed(&error, path)),
    })
}

/// A held `flock`, released when dropped. The file is opened close-on-exec,
/// but a child forked at that moment holds a copy of its descriptor until
/// it execs, and closing ours alone would leave the lock with that copy;
/// unlocking releases it whatever copies remain.
pub struct Held(fs::File);

impl Drop for Held {
    fn drop(&mut self) {
        // Closing the file releases the lock as well once every copy closes.
        let _ = self.0.unlock();
    }
}

impl System for Real {
    type Held = Held;

    fn environment(&self) -> &Environment {
        &self.environment
    }

    fn now(&self) -> Timestamp {
        Timestamp::now()
    }

    fn time_zone(&self) -> TimeZone {
        TimeZone::system()
    }

    fn current_dir(&self) -> Result<PathBuf, Failure> {
        std::env::current_dir().map_err(|error| unreadable(&error, Path::new(".")))
    }

    fn executable(&self) -> Result<PathBuf, Failure> {
        std::env::current_exe().map_err(|error| unreadable(&error, Path::new(PROGRAM)))
    }

    fn pid(&self) -> u32 {
        std::process::id()
    }

    fn stdin(&self) -> Result<Vec<u8>, Failure> {
        let mut bytes = Vec::new();
        io::stdin()
            .lock()
            .read_to_end(&mut bytes)
            .map(|_| bytes)
            .map_err(|error| Failure::unreadable(format!("standard input: {error}")))
    }

    fn git(&self, dir: &Path, args: &[OsString]) -> Result<Completed, Failure> {
        run_git(dir, args).map_err(|error| unreadable(&error, Path::new("git")))
    }

    fn process_start(&self, pid: u32) -> Result<Option<Timestamp>, Failure> {
        let done = run_in(
            Path::new("/"),
            OsStr::new("ps"),
            &super::args(["-o", "lstart=", "-p", &pid.to_string()]),
            [],
        )
        .map_err(|error| unreadable(&error, Path::new("ps")))?;
        // `ps` exits 1 when no process has the pid.
        Ok(done
            .success()
            .then(|| parse_lstart(&String::from_utf8_lossy(&done.stdout), &self.time_zone()))
            .flatten())
    }

    fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Failure> {
        match fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(unreadable(&error, path)),
        }
    }

    fn metadata(&self, path: &Path) -> Result<Option<Metadata>, Failure> {
        match fs::symlink_metadata(path) {
            Ok(meta) => Ok(Some(Metadata {
                kind: match meta.file_type() {
                    kind if kind.is_symlink() => FileKind::Symlink,
                    kind if kind.is_dir() => FileKind::Dir,
                    kind if kind.is_file() => FileKind::File,
                    _ => FileKind::Other,
                },
                size: meta.len(),
                modified: meta
                    .modified()
                    .ok()
                    .and_then(|at| Timestamp::try_from(at).ok()),
            })),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(unreadable(&error, path)),
        }
    }

    fn file_id(&self, path: &Path) -> Result<Option<FileId>, Failure> {
        match fs::metadata(path) {
            Ok(meta) => Ok(Some(FileId {
                device: meta.dev(),
                inode: meta.ino(),
            })),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(unreadable(&error, path)),
        }
    }

    fn read_link(&self, path: &Path) -> Result<Option<PathBuf>, Failure> {
        match fs::read_link(path) {
            Ok(target) => Ok(Some(target)),
            // Not a link, or nothing there.
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::InvalidInput
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(unreadable(&error, path)),
        }
    }

    fn list_dir(&self, path: &Path) -> Result<Vec<OsString>, Failure> {
        let names: Result<Vec<OsString>, io::Error> = fs::read_dir(path)
            .and_then(|entries| entries.map(|entry| entry.map(|e| e.file_name())).collect());
        names
            .map(|names| {
                let sorted: std::collections::BTreeSet<OsString> = names.into_iter().collect();
                sorted.into_iter().collect()
            })
            .map_err(|error| unreadable(&error, path))
    }

    fn real_path(&self, path: &Path) -> Result<PathBuf, Failure> {
        self.current_dir().map(|dir| real_path(path, &dir))
    }

    fn free_bytes(&self, path: &Path) -> Result<u64, Failure> {
        rustix::fs::statvfs(path)
            .map(|stats| stats.f_bavail.saturating_mul(stats.f_frsize))
            .map_err(|error| unreadable(&error.into(), path))
    }

    fn volume(&self, path: &Path) -> Result<u64, Failure> {
        fs::metadata(path)
            .map(|meta| meta.dev())
            .map_err(|error| unreadable(&error, path))
    }

    fn git_act(&mut self, dir: &Path, args: &[OsString]) -> Result<Completed, Failure> {
        run_git(dir, args).map_err(|error| step_failed(&error, Path::new("git")))
    }

    fn run(
        &mut self,
        dir: &Path,
        program: &OsStr,
        args: &[OsString],
    ) -> Result<Completed, Failure> {
        run_in(dir, program, args, []).map_err(|error| step_failed(&error, Path::new(program)))
    }

    fn spawn_detached(
        &mut self,
        dir: &Path,
        program: &Path,
        args: &[OsString],
        env: &[(String, String)],
    ) -> Result<(), Failure> {
        cleared()
            .fold(Command::new(program), |mut command, name| {
                command.env_remove(name);
                command
            })
            .args(args)
            .current_dir(dir)
            .envs(env.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map(drop)
            .map_err(|error| step_failed(&error, program))
    }

    fn write_file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), Failure> {
        make_parent(path)?;
        fs::write(path, bytes).map_err(|error| step_failed(&error, path))
    }

    fn copy_new(&mut self, from: &Path, to: &Path) -> Result<(), Failure> {
        make_parent(to)?;
        let bytes = fs::read(from).map_err(|error| unreadable(&error, from))?;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)
            .and_then(|mut file| file.write_all(&bytes))
            .map_err(|error| step_failed(&error, to))
    }

    fn append(&mut self, path: &Path, line: &[u8]) -> Result<(), Failure> {
        make_parent(path)?;
        let written = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .and_then(|mut file| file.write(line))
            .map_err(|error| step_failed(&error, path))?;
        if written == line.len() {
            Ok(())
        } else {
            Err(Failure::step_failed(format!(
                "{}: wrote {written} of {} bytes",
                path.display(),
                line.len()
            )))
        }
    }

    fn append_chunks(&mut self, path: &Path, chunks: &[&[u8]]) -> Result<(), Failure> {
        make_parent(path)?;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .map_err(|error| step_failed(&error, path))?;
        write_chunks(&mut file, path, chunks)
    }

    fn remove(&mut self, path: &Path, inside: &Path) -> Result<bool, Failure> {
        let (resolved, folder) = (self.real_path(path)?, self.real_path(inside)?);
        // The link itself goes, not what it points at, so the guard is on
        // the folder that holds it.
        let held = match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_symlink() => path
                .parent()
                .map(|parent| self.real_path(parent))
                .transpose()?
                .zip(path.file_name())
                .map(|(parent, name)| parent.join(name)),
            _ => Some(resolved),
        };
        match held {
            Some(target) if is_inside(&target, &folder) && target != folder => {
                let gone = |result: io::Result<()>| {
                    result.map(|()| true).map_err(|e| step_failed(&e, path))
                };
                match fs::symlink_metadata(path) {
                    Ok(meta) if meta.is_dir() => gone(fs::remove_dir_all(path)),
                    Ok(_) => gone(fs::remove_file(path)),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
                    Err(error) => Err(unreadable(&error, path)),
                }
            }
            _ => Err(Failure::step_failed(format!(
                "refusing to delete {} outside {}",
                path.display(),
                inside.display()
            ))),
        }
    }

    fn lock(&mut self, path: &Path) -> Result<Option<Self::Held>, Failure> {
        let file = lock_file(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Held(file))),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(error)) => Err(step_failed(&error, path)),
        }
    }

    fn wait_lock(&mut self, path: &Path) -> Result<Self::Held, Failure> {
        let file = lock_file(path)?;
        file.lock()
            .map(|()| Held(file))
            .map_err(|error| step_failed(&error, path))
    }

    fn print(&mut self, text: &str) -> Result<(), Failure> {
        let mut out = io::stdout().lock();
        match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
            Err(error) if error.kind() != io::ErrorKind::BrokenPipe => {
                Err(Failure::step_failed(format!("standard output: {error}")))
            }
            _ => Ok(()),
        }
    }

    fn warn(&mut self, message: &str) {
        // Nothing is left to report a failure to write standard error to.
        let _ = writeln!(io::stderr(), "{PROGRAM}: {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_local_variables_are_gits() {
        let listed = run_in(
            Path::new("/"),
            OsStr::new("git"),
            &super::super::args(["rev-parse", "--local-env-vars"]),
            [],
        )
        .expect("git runs");
        let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
        let set = |names: Vec<&str>| -> std::collections::BTreeSet<String> {
            names.into_iter().map(str::to_owned).collect()
        };
        assert_eq!(
            set(listed.lines().collect()),
            set(GIT_LOCAL_VARIABLES.to_vec())
        );
    }
}
