//! `provision`: a worktree's folders in the build root set up, the main
//! checkout's settings copied in, and its `.envrc` allowed when it is the
//! checkout's.
//!
//! Each step runs or is skipped on its own, and a failed one stops none
//! of the rest.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::build_root::{
    self, Layout, NEVER_INDEX_FILE, Platform, REPO_FILE, TIME_MACHINE_FILE, WORKTREE_FILE,
};
use crate::cli::{Format, FormatArg};
use crate::commands::{Status, print_json};
use crate::model::ids::CommandName;
use crate::model::report::SCHEMA;
use crate::system::{self, Failure, System};

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct Args {
    /// The worktree (default: the one the command runs in)
    #[arg(value_name = "worktree")]
    pub worktree: Option<PathBuf>,
    #[command(flatten)]
    pub format: FormatArg,
}

/// Whether `run` does the command's work yet.
pub const BUILT: bool = true;

/// The settings a new worktree takes from the main checkout.
pub const SETTINGS: &str = ".claude/settings.local.json";

/// The file `direnv allow` trusts.
pub const ENVRC: &str = ".envrc";

string_enum! {
    pub enum Step {
        /// Write [`REPO_FILE`] in the build root.
        Repo => "repo",
        /// Write [`WORKTREE_FILE`] in the worktree's own folder.
        Worktree => "worktree",
        /// Write [`NEVER_INDEX_FILE`] in the folder of every build root,
        /// on macOS.
        NeverIndex => "never-index",
        /// Exclude that folder from Time Machine, on macOS, and write
        /// [`TIME_MACHINE_FILE`] once it is.
        TimeMachine => "time-machine",
        /// Copy the main checkout's [`SETTINGS`].
        Settings => "settings",
        /// Run `direnv allow`.
        Direnv => "direnv",
    }
}

string_enum! {
    pub enum StepOutcome {
        Done => "done",
        Skipped => "skipped",
        Failed => "failed",
    }
}

/// What provisioning decides from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    pub layout: Layout,
    pub common_dir: PathBuf,
    /// None when the repository is bare.
    pub main_checkout: Option<PathBuf>,
    pub platform: Platform,
    pub repo_file: Option<Vec<u8>>,
    pub worktree_file: Option<Vec<u8>>,
    pub never_index: bool,
    /// Whether [`TIME_MACHINE_FILE`] says the exclusion is made.
    pub time_machine: bool,
    pub tmutil: bool,
    pub direnv: bool,
    pub worktree_has_settings: bool,
    pub main_has_settings: bool,
    pub worktree_envrc: Option<Vec<u8>>,
    pub main_envrc: Option<Vec<u8>>,
}

impl Facts {
    /// The folder holding every build root.
    pub fn top(&self) -> &Path {
        self.layout.root.parent().unwrap_or(&self.layout.root)
    }
}

/// `path`'s bytes and one newline.
pub fn line(path: &Path) -> Vec<u8> {
    [path.as_os_str().as_encoded_bytes(), b"\n"].concat()
}

/// Whether a step runs, or why not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Act,
    Skip(String),
}

/// Each step, and whether it runs. Settings are copied only where the
/// worktree has none, and `.envrc` is allowed only when it is byte for
/// byte the main checkout's.
pub fn decide(facts: &Facts) -> Vec<(Step, Decision)> {
    let skip = |why: &str| Decision::Skip(why.to_owned());
    let unless_same = |file: &Option<Vec<u8>>, path: &Path| {
        if file.as_deref() == Some(line(path).as_slice()) {
            skip("it is written already")
        } else {
            Decision::Act
        }
    };
    let macos = facts.platform == Platform::MacOs;
    let never_index = match (macos, facts.never_index) {
        (false, _) => skip("not macOS"),
        (true, true) => skip("it is written already"),
        (true, false) => Decision::Act,
    };
    let time_machine = match (macos, facts.time_machine, facts.tmutil) {
        (false, _, _) => skip("not macOS"),
        (true, true, _) => skip("it is excluded already"),
        (true, false, false) => skip("tmutil is not on PATH"),
        (true, false, true) => Decision::Act,
    };
    let neither = if facts.layout.main {
        Some(skip("the worktree is the main checkout"))
    } else if facts.main_checkout.is_none() {
        Some(skip("the repository has no main checkout"))
    } else {
        None
    };
    let settings = neither.clone().unwrap_or_else(|| {
        if !facts.main_has_settings {
            skip("the main checkout has none")
        } else if facts.worktree_has_settings {
            skip("the worktree has its own")
        } else {
            Decision::Act
        }
    });
    let direnv = neither.unwrap_or_else(|| match &facts.worktree_envrc {
        None => skip("the worktree has no .envrc"),
        Some(own) if Some(own) != facts.main_envrc.as_ref() => {
            skip("its .envrc is not the main checkout's")
        }
        Some(_) if !facts.direnv => skip("direnv is not on PATH"),
        Some(_) => Decision::Act,
    });
    vec![
        (Step::Repo, unless_same(&facts.repo_file, &facts.common_dir)),
        (
            Step::Worktree,
            unless_same(&facts.worktree_file, &facts.layout.worktree),
        ),
        (Step::NeverIndex, never_index),
        (Step::TimeMachine, time_machine),
        (Step::Settings, settings),
        (Step::Direnv, direnv),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StepReport {
    pub step: Step,
    pub outcome: StepOutcome,
    pub detail: String,
}

/// What `provision --format json` prints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Report {
    pub schema: u32,
    pub command: CommandName,
    pub worktree: PathBuf,
    pub steps: Vec<StepReport>,
}

/// The report as text, one line per step.
pub fn render(report: &Report) -> String {
    report
        .steps
        .iter()
        .map(|step| format!("{}: {}: {}\n", step.step, step.outcome, step.detail))
        .collect()
}

/// The facts of the worktree holding `path`.
fn read(system: &impl System, path: &Path) -> Result<Facts, Failure> {
    let place = build_root::place(system, path)?;
    let worktree = &place.layout.worktree;
    let theirs = |name: &str| place.main_checkout.as_ref().map(|main| main.join(name));
    let top = place.layout.root.parent().unwrap_or(&place.layout.root);
    Ok(Facts {
        repo_file: system.read_file(&place.layout.root.join(REPO_FILE))?,
        worktree_file: system.read_file(&place.layout.dir().join(WORKTREE_FILE))?,
        never_index: system.metadata(&top.join(NEVER_INDEX_FILE))?.is_some(),
        time_machine: system.metadata(&top.join(TIME_MACHINE_FILE))?.is_some(),
        tmutil: build_root::on_path(system, "tmutil"),
        direnv: build_root::on_path(system, "direnv"),
        worktree_has_settings: system.metadata(&worktree.join(SETTINGS))?.is_some(),
        main_has_settings: theirs(SETTINGS)
            .map(|path| system.metadata(&path))
            .transpose()?
            .flatten()
            .is_some(),
        worktree_envrc: system.read_file(&worktree.join(ENVRC))?,
        main_envrc: theirs(ENVRC)
            .map(|path| system.read_file(&path))
            .transpose()?
            .flatten(),
        platform: Platform::current(),
        main_checkout: place.main_checkout.clone(),
        common_dir: place.common_dir,
        layout: place.layout,
    })
}

/// `program args…` in `dir`, as a step's result.
fn ran(system: &mut impl System, dir: &Path, program: &str, args: &[&OsStr]) -> Result<(), String> {
    match system.run(dir, OsStr::new(program), &system::args(args)) {
        Ok(done) if done.success() => Ok(()),
        Ok(done) => Err(format!(
            "{program} {} exited {}: {}",
            args.first()
                .map(|arg| arg.to_string_lossy())
                .unwrap_or_default(),
            done.code,
            done.stderr.trim()
        )),
        Err(failure) => Err(failure.detail),
    }
}

/// `bytes` written to `path`, as a step's result, naming what they
/// replaced. `System::write_file` writes in place, so a reader can see the
/// file half-written.
fn write(
    system: &mut impl System,
    path: PathBuf,
    bytes: &[u8],
    before: Option<&[u8]>,
) -> Result<String, String> {
    let replaced = match before {
        Some(old) => format!(
            ", replacing {:?}",
            String::from_utf8_lossy(old).trim_end_matches('\n')
        ),
        None => String::new(),
    };
    system
        .write_file(&path, bytes)
        .map(|()| format!("wrote {}{replaced}", path.display()))
        .map_err(|failure| failure.detail)
}

fn act(system: &mut impl System, step: Step, facts: &Facts) -> Result<String, String> {
    let worktree = &facts.layout.worktree;
    match step {
        Step::Repo => write(
            system,
            facts.layout.root.join(REPO_FILE),
            &line(&facts.common_dir),
            facts.repo_file.as_deref(),
        ),
        Step::Worktree => write(
            system,
            facts.layout.dir().join(WORKTREE_FILE),
            &line(worktree),
            facts.worktree_file.as_deref(),
        ),
        Step::NeverIndex => write(system, facts.top().join(NEVER_INDEX_FILE), b"", None),
        Step::TimeMachine => {
            let top = facts.top();
            ran(
                system,
                top,
                "tmutil",
                &[OsStr::new("addexclusion"), top.as_os_str()],
            )?;
            write(system, top.join(TIME_MACHINE_FILE), b"", None)
                .map(|_| format!("excluded {}", top.display()))
                .map_err(|error| format!("excluded {}, but {error}", top.display()))
        }
        Step::Settings => {
            let from = facts
                .main_checkout
                .as_ref()
                .map(|main| main.join(SETTINGS))
                .ok_or("the repository has no main checkout")?;
            system
                .copy_new(&from, &worktree.join(SETTINGS))
                .map(|()| format!("copied {}", from.display()))
                .map_err(|failure| failure.detail)
        }
        Step::Direnv => ran(
            system,
            worktree,
            "direnv",
            &[OsStr::new("allow"), worktree.as_os_str()],
        )
        .map(|()| format!("allowed {}", worktree.join(ENVRC).display())),
    }
}

pub fn run(args: &Args, system: &mut impl System) -> Result<Status, Failure> {
    let path = match &args.worktree {
        Some(path) => path.clone(),
        None => system.current_dir()?,
    };
    let facts = read(system, &path)?;
    let steps: Vec<StepReport> = decide(&facts)
        .into_iter()
        .map(|(step, decision)| {
            let (outcome, detail) = match decision {
                Decision::Skip(detail) => (StepOutcome::Skipped, detail),
                Decision::Act => match act(system, step, &facts) {
                    Ok(detail) => (StepOutcome::Done, detail),
                    Err(detail) => (StepOutcome::Failed, detail),
                },
            };
            StepReport {
                step,
                outcome,
                detail,
            }
        })
        .collect();
    steps
        .iter()
        .filter(|step| step.outcome == StepOutcome::Failed)
        .for_each(|step| system.warn(&format!("{}: {}", step.step, step.detail)));
    let failed = steps.iter().any(|step| step.outcome == StepOutcome::Failed);
    let report = Report {
        schema: SCHEMA,
        command: CommandName::Provision,
        worktree: facts.layout.worktree,
        steps,
    };
    match args.format.format {
        Format::Text => system.print(&render(&report))?,
        Format::Json => print_json(system, &report)?,
    }
    Ok(if failed { Status::Failed } else { Status::Done })
}
