//! The system edge: one trait in front of `git`, `ps`, the clock, the
//! environment, the file system and free space.
//!
//! Reads take `&self` and acts take `&mut self`, so a function's signature
//! says whether it can change anything. [`Real`] is the system the binary
//! runs on; [`Fake`] answers from tables and records every act, for tests.
//! gitoxide reads repositories itself and stays outside the trait.

mod fake;
mod real;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::environment::Environment;
use crate::model::Note;
use crate::model::ids::ErrorCode;

pub use fake::{Act, Fake, FakeFile};
pub use real::Real;

/// Why something failed: a code a caller can match on, and the detail the
/// program prints. Every command's failure is printed by
/// [`crate::commands::failed`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: ErrorCode,
    pub detail: String,
}

impl Failure {
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    /// Something couldn't be read.
    pub fn unreadable(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unreadable, detail)
    }

    /// A step that changes something failed.
    pub fn step_failed(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::StepFailed, detail)
    }

    /// The failure as the JSON's `error`.
    pub fn note(&self) -> Note<ErrorCode> {
        Note::new(self.code, self.detail.clone())
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

/// What a finished command returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completed {
    /// The exit code, or the negated signal that ended it.
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Completed {
    /// Exit 0 with `stdout`.
    pub fn ok(stdout: impl Into<Vec<u8>>) -> Self {
        Self {
            code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    pub fn success(&self) -> bool {
        self.code == 0
    }
}

/// What a path is, without following a final symbolic link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metadata {
    pub kind: FileKind,
    pub size: u64,
    pub modified: Option<Timestamp>,
}

/// A command's arguments.
pub fn args<S: AsRef<OsStr>>(items: impl IntoIterator<Item = S>) -> Vec<OsString> {
    items
        .into_iter()
        .map(|item| item.as_ref().to_owned())
        .collect()
}

pub trait System {
    /// What a held [`System::lock`] is; dropping it releases the lock.
    type Held;

    // Reads -------------------------------------------------------------------

    fn environment(&self) -> &Environment;

    fn now(&self) -> Timestamp;

    /// The local clock, on which `ps` gives start times.
    fn time_zone(&self) -> TimeZone;

    fn current_dir(&self) -> Result<PathBuf, Failure>;

    /// The running binary, for starting it again.
    fn executable(&self) -> Result<PathBuf, Failure>;

    /// This process's own id, which the hook logs.
    fn pid(&self) -> u32;

    /// All of standard input, which git gives the hook. Standard input is
    /// read once: a second call gets what is left, which is nothing.
    fn stdin(&self) -> Result<Vec<u8>, Failure>;

    /// `git` in `dir`, for a read: never with an optional lock, so reading
    /// a status doesn't refresh the index whose time `idle` reads.
    fn git(&self, dir: &Path, args: &[OsString]) -> Result<Completed, Failure>;

    /// When process `pid` started, from `ps -o lstart= -p <pid>`; None when
    /// no such process runs.
    fn process_start(&self, pid: u32) -> Result<Option<Timestamp>, Failure>;

    /// A file's bytes; None when it doesn't exist.
    fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Failure>;

    /// None when nothing is at `path`.
    fn metadata(&self, path: &Path) -> Result<Option<Metadata>, Failure>;

    /// Where the symbolic link at `path` points, as written; None when
    /// `path` is no link.
    fn read_link(&self, path: &Path) -> Result<Option<PathBuf>, Failure>;

    /// The names in a folder, sorted.
    fn list_dir(&self, path: &Path) -> Result<Vec<OsString>, Failure>;

    /// `path` absolute, with every symbolic link resolved; see [`real_path`].
    fn real_path(&self, path: &Path) -> Result<PathBuf, Failure>;

    /// Bytes free to an unprivileged user on the volume holding `path`.
    fn free_bytes(&self, path: &Path) -> Result<u64, Failure>;

    /// An id of the volume holding `path`, the same for every path on it.
    fn volume(&self, path: &Path) -> Result<u64, Failure>;

    // Acts --------------------------------------------------------------------

    /// `git` in `dir`, for a write.
    fn git_act(&mut self, dir: &Path, args: &[OsString]) -> Result<Completed, Failure>;

    /// Any other program, such as `direnv allow`, in `dir`.
    fn run(&mut self, dir: &Path, program: &OsStr, args: &[OsString])
    -> Result<Completed, Failure>;

    /// Starts `program` in `dir` with `env` added, in a process group of its
    /// own with its standard streams on `/dev/null`, and doesn't wait.
    fn spawn_detached(
        &mut self,
        dir: &Path,
        program: &Path,
        args: &[OsString],
        env: &[(String, String)],
    ) -> Result<(), Failure>;

    /// Writes `bytes` to `path`, creating its folders.
    fn write_file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), Failure>;

    /// Copies `from` to `to`, which must not exist yet.
    fn copy_new(&mut self, from: &Path, to: &Path) -> Result<(), Failure>;

    /// Appends `line` to `path` in one `write` to a file opened with
    /// `O_APPEND`, creating its folders, so lines from processes running at
    /// once never interleave.
    fn append(&mut self, path: &Path, line: &[u8]) -> Result<(), Failure>;

    /// Deletes `path`, a file, link or folder strictly inside `inside`,
    /// both read as real paths; false when it was gone already.
    fn remove(&mut self, path: &Path, inside: &Path) -> Result<bool, Failure>;

    /// Takes `flock` on `path`, creating it; None when another process
    /// holds it.
    fn lock(&mut self, path: &Path) -> Result<Option<Self::Held>, Failure>;

    /// Writes to standard output. A reader that has gone away is no
    /// failure.
    fn print(&mut self, text: &str) -> Result<(), Failure>;

    /// Writes `<program>: <message>` to standard error.
    fn warn(&mut self, message: &str);
}

/// `path`, made absolute against `current_dir`, with every symbolic link
/// resolved and `.` and `..` taken after the links before them.
///
/// Unlike `canonicalize`, a path that doesn't exist resolves as far as it
/// can, and a dangling link resolves to its missing target, so a guard that
/// asks whether a path is inside a folder can't be led out of it by a link.
pub fn real_path(path: &Path, current_dir: &Path) -> PathBuf {
    resolve(&current_dir.join(path), MAX_LINKS, &|link| {
        std::fs::read_link(link).ok()
    })
}

/// How many links one resolution follows before it stops, as the kernel's
/// `ELOOP` limit does.
const MAX_LINKS: u32 = 40;

/// `path`, absolute, resolved part by part with `read_link`, which gives a
/// link's target and None for anything else.
fn resolve(path: &Path, links: u32, read_link: &dyn Fn(&Path) -> Option<PathBuf>) -> PathBuf {
    path.components()
        .fold(PathBuf::new(), |base, component| match component {
            Component::Prefix(_) | Component::RootDir => base.join(component),
            Component::CurDir => base,
            Component::ParentDir => base.parent().map(Path::to_path_buf).unwrap_or(base),
            Component::Normal(name) => {
                let candidate = base.join(name);
                match read_link(&candidate) {
                    Some(target) if links > 0 => resolve(&base.join(target), links - 1, read_link),
                    _ => candidate,
                }
            }
        })
}

/// `path` made absolute against `current_dir`, with `.` and `..` taken as
/// written, following no link.
pub fn normal_path(path: &Path, current_dir: &Path) -> PathBuf {
    resolve(&current_dir.join(path), 0, &|_| None)
}

/// Whether `path` is `folder` or inside it, part by part.
pub fn is_inside(path: &Path, folder: &Path) -> bool {
    path.starts_with(folder)
}

#[cfg(test)]
mod tests;
