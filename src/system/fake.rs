//! A system for tests: reads answer from tables set when it is built, and
//! acts are recorded in order, with files written where later reads see
//! them. Nothing outside it is touched.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use jiff::tz::TimeZone;

use super::{Completed, Failure, FileKind, Metadata, System, is_inside};
use crate::environment::Environment;

/// What the fake holds at a path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FakeFile {
    File {
        bytes: Vec<u8>,
        modified: Option<Timestamp>,
    },
    Dir,
    /// A symbolic link to `target`, as written.
    Link {
        target: PathBuf,
    },
}

/// One act, as the fake received it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    Git {
        dir: PathBuf,
        args: Vec<OsString>,
    },
    Run {
        dir: PathBuf,
        program: OsString,
        args: Vec<OsString>,
    },
    Spawn {
        dir: PathBuf,
        program: PathBuf,
        args: Vec<OsString>,
        env: Vec<(String, String)>,
    },
    Write {
        path: PathBuf,
        bytes: Vec<u8>,
    },
    CopyNew {
        from: PathBuf,
        to: PathBuf,
    },
    Append {
        path: PathBuf,
        line: Vec<u8>,
    },
    Remove {
        path: PathBuf,
    },
    Lock {
        path: PathBuf,
    },
}

/// A command's key in the fake's tables: the folder, then the arguments.
type CommandKey = (PathBuf, Vec<OsString>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fake {
    pub environment: Environment,
    pub now: Timestamp,
    pub time_zone: TimeZone,
    pub current_dir: PathBuf,
    pub executable: PathBuf,
    pub pid: u32,
    /// What standard input holds.
    pub stdin: Vec<u8>,
    /// By absolute path; a path that is no key and no key's ancestor is
    /// missing, and an ancestor of a key is a folder.
    pub files: BTreeMap<PathBuf, FakeFile>,
    /// `git` answers, for reads and acts alike.
    pub git: BTreeMap<CommandKey, Completed>,
    /// Other programs' answers, keyed by the program then its arguments.
    pub programs: BTreeMap<(PathBuf, OsString, Vec<OsString>), Completed>,
    /// Running processes' start times, by pid.
    pub processes: BTreeMap<u32, Timestamp>,
    /// Free bytes by volume mount point; a path is on the longest that
    /// holds it.
    pub volumes: BTreeMap<PathBuf, u64>,
    /// Lock files another process holds.
    pub held: BTreeSet<PathBuf>,
    /// Every act, in order.
    pub acts: Vec<Act>,
    pub stdout: String,
    pub stderr: String,
}

impl Fake {
    /// A system at `now` with nothing in it, in `/` on the UTC clock.
    pub fn new(now: Timestamp) -> Self {
        Self {
            environment: Environment::default(),
            now,
            time_zone: TimeZone::UTC,
            current_dir: PathBuf::from("/"),
            executable: PathBuf::from("/bin").join(crate::names::PROGRAM),
            pid: 4242,
            stdin: Vec::new(),
            files: BTreeMap::new(),
            git: BTreeMap::new(),
            programs: BTreeMap::new(),
            processes: BTreeMap::new(),
            volumes: BTreeMap::new(),
            held: BTreeSet::new(),
            acts: Vec::new(),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    pub fn with_environment(self, environment: Environment) -> Self {
        Self {
            environment,
            ..self
        }
    }

    pub fn with_stdin(self, stdin: impl Into<Vec<u8>>) -> Self {
        Self {
            stdin: stdin.into(),
            ..self
        }
    }

    pub fn with_file(self, path: impl Into<PathBuf>, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            files: self
                .files
                .into_iter()
                .chain([(
                    path.into(),
                    FakeFile::File {
                        bytes: bytes.into(),
                        modified: Some(self.now),
                    },
                )])
                .collect(),
            ..self
        }
    }

    /// A symbolic link at `path` to `target`.
    pub fn with_link(self, path: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        Self {
            files: self
                .files
                .into_iter()
                .chain([(
                    path.into(),
                    FakeFile::Link {
                        target: target.into(),
                    },
                )])
                .collect(),
            ..self
        }
    }

    /// `git <args>` in `dir` answers `completed`.
    pub fn with_git<S: AsRef<OsStr>>(
        self,
        dir: impl Into<PathBuf>,
        args: impl IntoIterator<Item = S>,
        completed: Completed,
    ) -> Self {
        Self {
            git: self
                .git
                .into_iter()
                .chain([((dir.into(), super::args(args)), completed)])
                .collect(),
            ..self
        }
    }

    pub fn with_process(self, pid: u32, start: Timestamp) -> Self {
        Self {
            processes: self.processes.into_iter().chain([(pid, start)]).collect(),
            ..self
        }
    }

    pub fn with_volume(self, mount: impl Into<PathBuf>, free: u64) -> Self {
        Self {
            volumes: self
                .volumes
                .into_iter()
                .chain([(mount.into(), free)])
                .collect(),
            ..self
        }
    }

    fn absolute(&self, path: &Path) -> PathBuf {
        super::normal_path(path, &self.current_dir)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.files.get(path) == Some(&FakeFile::Dir)
            || self
                .files
                .keys()
                .any(|key| key != path && key.starts_with(path))
    }

    /// The longest volume mount holding `path`: its place among the mounts,
    /// which is its id, and its free bytes.
    fn mount(&self, path: &Path) -> Result<(usize, u64), Failure> {
        let path = self.absolute(path);
        self.volumes
            .iter()
            .enumerate()
            .filter(|(_, (mount, _))| path.starts_with(mount))
            .max_by_key(|(_, (mount, _))| mount.components().count())
            .map(|(index, (_, free))| (index, *free))
            .ok_or_else(|| Failure::unreadable(format!("{}: no volume", path.display())))
    }

    fn answer(&self, key: &CommandKey) -> Result<Completed, Failure> {
        self.git.get(key).cloned().ok_or_else(|| {
            Failure::unreadable(format!(
                "the fake system has no answer for git {:?} in {}",
                key.1,
                key.0.display()
            ))
        })
    }

    fn record(&mut self, act: Act) {
        self.acts.push(act);
    }

    fn store(&mut self, path: PathBuf, bytes: Vec<u8>) {
        let modified = Some(self.now);
        self.files.insert(path, FakeFile::File { bytes, modified });
    }
}

impl System for Fake {
    type Held = ();

    fn environment(&self) -> &Environment {
        &self.environment
    }

    fn now(&self) -> Timestamp {
        self.now
    }

    fn time_zone(&self) -> TimeZone {
        self.time_zone.clone()
    }

    fn current_dir(&self) -> Result<PathBuf, Failure> {
        Ok(self.current_dir.clone())
    }

    fn executable(&self) -> Result<PathBuf, Failure> {
        Ok(self.executable.clone())
    }

    fn pid(&self) -> u32 {
        self.pid
    }

    fn stdin(&self) -> Result<Vec<u8>, Failure> {
        Ok(self.stdin.clone())
    }

    fn git(&self, dir: &Path, args: &[OsString]) -> Result<Completed, Failure> {
        self.answer(&(self.absolute(dir), args.to_vec()))
    }

    fn process_start(&self, pid: u32) -> Result<Option<Timestamp>, Failure> {
        Ok(self.processes.get(&pid).copied())
    }

    fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Failure> {
        let path = self.absolute(path);
        match self.files.get(&path) {
            Some(FakeFile::File { bytes, .. }) => Ok(Some(bytes.clone())),
            Some(FakeFile::Dir) => Err(Failure::unreadable(format!(
                "{}: is a folder",
                path.display()
            ))),
            // A link is followed once, to a file.
            Some(FakeFile::Link { target }) => {
                let target = self.absolute(&path.parent().unwrap_or(&path).join(target));
                Ok(match self.files.get(&target) {
                    Some(FakeFile::File { bytes, .. }) => Some(bytes.clone()),
                    _ => None,
                })
            }
            None => Ok(None),
        }
    }

    fn metadata(&self, path: &Path) -> Result<Option<Metadata>, Failure> {
        let path = self.absolute(path);
        Ok(match self.files.get(&path) {
            Some(FakeFile::File { bytes, modified }) => Some(Metadata {
                kind: FileKind::File,
                size: bytes.len() as u64,
                modified: *modified,
            }),
            Some(FakeFile::Link { target }) => Some(Metadata {
                kind: FileKind::Symlink,
                size: target.as_os_str().len() as u64,
                modified: None,
            }),
            _ if self.is_dir(&path) => Some(Metadata {
                kind: FileKind::Dir,
                size: 0,
                modified: None,
            }),
            _ => None,
        })
    }

    fn list_dir(&self, path: &Path) -> Result<Vec<OsString>, Failure> {
        let path = self.absolute(path);
        let names: BTreeSet<OsString> = self
            .files
            .keys()
            .filter_map(|key| key.strip_prefix(&path).ok())
            .filter_map(|rest| rest.components().next())
            .map(|first| first.as_os_str().to_owned())
            .collect();
        Ok(names.into_iter().collect())
    }

    fn read_link(&self, path: &Path) -> Result<Option<PathBuf>, Failure> {
        Ok(match self.files.get(&self.absolute(path)) {
            Some(FakeFile::Link { target }) => Some(target.clone()),
            _ => None,
        })
    }

    fn real_path(&self, path: &Path) -> Result<PathBuf, Failure> {
        // The fake's links are never resolved.
        Ok(super::normal_path(path, &self.current_dir))
    }

    fn free_bytes(&self, path: &Path) -> Result<u64, Failure> {
        self.mount(path).map(|(_, free)| free)
    }

    fn volume(&self, path: &Path) -> Result<u64, Failure> {
        self.mount(path).map(|(index, _)| index as u64)
    }

    fn git_act(&mut self, dir: &Path, args: &[OsString]) -> Result<Completed, Failure> {
        let key = (self.absolute(dir), args.to_vec());
        self.record(Act::Git {
            dir: key.0.clone(),
            args: key.1.clone(),
        });
        self.answer(&key)
    }

    fn run(
        &mut self,
        dir: &Path,
        program: &OsStr,
        args: &[OsString],
    ) -> Result<Completed, Failure> {
        let key = (self.absolute(dir), program.to_owned(), args.to_vec());
        self.record(Act::Run {
            dir: key.0.clone(),
            program: key.1.clone(),
            args: key.2.clone(),
        });
        self.programs.get(&key).cloned().ok_or_else(|| {
            Failure::step_failed(format!(
                "the fake system has no answer for {program:?} {args:?}"
            ))
        })
    }

    fn spawn_detached(
        &mut self,
        dir: &Path,
        program: &Path,
        args: &[OsString],
        env: &[(String, String)],
    ) -> Result<(), Failure> {
        let act = Act::Spawn {
            dir: self.absolute(dir),
            program: program.to_owned(),
            args: args.to_vec(),
            env: env.to_vec(),
        };
        self.record(act);
        Ok(())
    }

    fn write_file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), Failure> {
        let path = self.absolute(path);
        self.record(Act::Write {
            path: path.clone(),
            bytes: bytes.to_vec(),
        });
        self.store(path, bytes.to_vec());
        Ok(())
    }

    fn copy_new(&mut self, from: &Path, to: &Path) -> Result<(), Failure> {
        let (from, to) = (self.absolute(from), self.absolute(to));
        self.record(Act::CopyNew {
            from: from.clone(),
            to: to.clone(),
        });
        match (self.read_file(&from)?, self.metadata(&to)?) {
            (Some(bytes), None) => {
                self.store(to, bytes);
                Ok(())
            }
            (None, _) => Err(Failure::step_failed(format!(
                "{}: no such file",
                from.display()
            ))),
            (_, Some(_)) => Err(Failure::step_failed(format!("{}: exists", to.display()))),
        }
    }

    fn append(&mut self, path: &Path, line: &[u8]) -> Result<(), Failure> {
        let path = self.absolute(path);
        self.record(Act::Append {
            path: path.clone(),
            line: line.to_vec(),
        });
        let bytes = self
            .read_file(&path)?
            .unwrap_or_default()
            .into_iter()
            .chain(line.iter().copied())
            .collect();
        self.store(path, bytes);
        Ok(())
    }

    fn remove(&mut self, path: &Path, inside: &Path) -> Result<bool, Failure> {
        let (path, inside) = (self.absolute(path), self.absolute(inside));
        if !is_inside(&path, &inside) || path == inside {
            return Err(Failure::step_failed(format!(
                "refusing to delete {} outside {}",
                path.display(),
                inside.display()
            )));
        }
        self.record(Act::Remove { path: path.clone() });
        let before = self.files.len();
        self.files.retain(|key, _| !key.starts_with(&path));
        Ok(self.files.len() < before)
    }

    fn lock(&mut self, path: &Path) -> Result<Option<Self::Held>, Failure> {
        let path = self.absolute(path);
        self.record(Act::Lock { path: path.clone() });
        Ok((!self.held.contains(&path)).then_some(()))
    }

    fn print(&mut self, text: &str) -> Result<(), Failure> {
        self.stdout.push_str(text);
        Ok(())
    }

    fn warn(&mut self, message: &str) {
        self.stderr
            .push_str(&format!("{}: {message}\n", crate::names::PROGRAM));
    }
}
