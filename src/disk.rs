//! The disk floors, and whether a volume has room for a build.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::environment::Environment;
use crate::locate::Located;
use crate::model::ids::CommandName;
use crate::model::report::SCHEMA;
use crate::names::{self, Variable};
use crate::system::Failure;

/// The floors, in GB of 10^9 bytes: `WORKTREE_MANAGER_FLOOR_*_GB`, else
/// the floors file, else 70 and 35.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Floors {
    /// Below it, no build starts.
    pub build_gb: f64,
    /// Below it, only cleanup runs.
    pub cleanup_only_gb: f64,
}

/// The floors when neither a variable nor the floors file sets them.
pub const DEFAULT_FLOORS: Floors = Floors {
    build_gb: 70.0,
    cleanup_only_gb: 35.0,
};

string_enum! {
    pub enum DiskState {
        Ok => "ok",
        BelowBuild => "below-build",
        CleanupOnly => "cleanup-only",
    }
}

/// What `disk --format json` prints.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiskReport {
    pub schema: u32,
    pub command: CommandName,
    pub floors: Floors,
    pub disks: Vec<Disk>,
}

/// One volume.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Disk {
    pub path: PathBuf,
    pub free_gb: f64,
    pub state: DiskState,
}

/// A volume's state with `free_gb` free: `ok` at or above the build floor,
/// `below-build` under it, and `cleanup-only` under the cleanup-only floor.
pub fn state(free_gb: f64, floors: &Floors) -> DiskState {
    if free_gb < floors.cleanup_only_gb {
        DiskState::CleanupOnly
    } else if free_gb < floors.build_gb {
        DiskState::BelowBuild
    } else {
        DiskState::Ok
    }
}

/// `bytes` in GB of 10^9 bytes.
pub fn gb(bytes: u64) -> f64 {
    bytes as f64 / 1e9
}

// The floors' sources -------------------------------------------------------------

/// Where a floor's value came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Variable(String),
    /// A line of the floors file, counted from 1.
    File {
        path: PathBuf,
        line: usize,
    },
    Default,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Variable(name) => f.write_str(name),
            Self::File { path, line } => write!(f, "{} line {line}", path.display()),
            Self::Default => f.write_str("the default"),
        }
    }
}

/// The floors, and where each came from.
#[derive(Clone, Debug, PartialEq)]
pub struct SourcedFloors {
    pub floors: Floors,
    pub build: Source,
    pub cleanup_only: Source,
    /// What the floors file holds that isn't a floor.
    pub warnings: Vec<String>,
}

/// The floors file: `KEY=value` lines whose keys are the variables that
/// override them. A line starting with `#` is a comment, one pair of
/// matching quotes around a value is dropped, and the last line of a key
/// wins. A line with `=` and any other key is ignored with a warning, and a
/// line without `=` is ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FloorsFile<'a> {
    pub path: &'a Path,
    pub text: &'a str,
}

/// The floors from `environment`, else `file`, else [`DEFAULT_FLOORS`].
/// A value that isn't a number of GB, zero or more, is a failure naming
/// where it was found, and so is a build floor below the cleanup-only
/// floor.
pub fn floors(
    environment: &Environment,
    file: Option<FloorsFile<'_>>,
) -> Result<SourcedFloors, Failure> {
    let (build_gb, build) = floor(
        Variable::FloorBuildGb,
        DEFAULT_FLOORS.build_gb,
        environment,
        file,
    )?;
    let (cleanup_only_gb, cleanup_only) = floor(
        Variable::FloorCleanupOnlyGb,
        DEFAULT_FLOORS.cleanup_only_gb,
        environment,
        file,
    )?;
    if build_gb < cleanup_only_gb {
        Err(Failure::unreadable(format!(
            "the build floor, {build_gb} GB from {build}, is below the cleanup-only floor, \
             {cleanup_only_gb} GB from {cleanup_only}"
        )))
    } else {
        Ok(SourcedFloors {
            floors: Floors {
                build_gb,
                cleanup_only_gb,
            },
            build,
            cleanup_only,
            warnings: file.map(unknown_keys).unwrap_or_default(),
        })
    }
}

fn floor(
    variable: Variable,
    default: f64,
    environment: &Environment,
    file: Option<FloorsFile<'_>>,
) -> Result<(f64, Source), Failure> {
    let name = variable.name();
    let found = environment
        .var(&name)
        .map(|value| (value.trim(), Source::Variable(name.clone())))
        .or_else(|| file.and_then(|file| file_line(file, &name)));
    match found {
        Some((value, source)) => number(value).map(|gb| (gb, source.clone())).ok_or_else(|| {
            let place = match &source {
                Source::Variable(_) => String::new(),
                _ => format!("{source}: "),
            };
            Failure::unreadable(format!("{place}{name} is {value:?}, not a number of GB"))
        }),
        None => Ok((default, Source::Default)),
    }
}

/// The value of the last line of `file` that sets `key`.
fn file_line<'a>(file: FloorsFile<'a>, key: &str) -> Option<(&'a str, Source)> {
    settings(file)
        .filter(|(_, name, _)| *name == key)
        .last()
        .map(|(line, _, value)| {
            (
                value,
                Source::File {
                    path: file.path.to_owned(),
                    line,
                },
            )
        })
}

/// A warning for each setting in `file` that is no floor.
fn unknown_keys(file: FloorsFile<'_>) -> Vec<String> {
    let known = [
        Variable::FloorBuildGb.name(),
        Variable::FloorCleanupOnlyGb.name(),
    ];
    settings(file)
        .filter(|(_, key, _)| !known.iter().any(|name| name == key))
        .map(|(line, key, _)| {
            format!(
                "{} line {line}: {key} is not a floor, and is ignored",
                file.path.display()
            )
        })
        .collect()
}

/// Each `KEY=value` line of `file` that isn't a comment: its number, from
/// 1, its key and its value, trimmed and unquoted.
fn settings<'a>(file: FloorsFile<'a>) -> impl Iterator<Item = (usize, &'a str, &'a str)> {
    file.text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with('#'))
        .filter_map(|(index, line)| {
            line.split_once('=')
                .map(|(key, value)| (index + 1, key.trim(), unquote(value.trim())))
        })
}

/// `value` without one pair of matching quotes around it.
fn unquote(value: &str) -> &str {
    ['"', '\'']
        .into_iter()
        .find_map(|quote| {
            value
                .strip_prefix(quote)
                .and_then(|rest| rest.strip_suffix(quote))
        })
        .unwrap_or(value)
}

fn number(text: &str) -> Option<f64> {
    text.parse::<f64>()
        .ok()
        .filter(|gb| gb.is_finite() && gb.is_sign_positive())
}

// The volumes ----------------------------------------------------------------------

/// Where Nix builds, reported when it exists, since its builds fill it.
pub const NIX: &str = "/nix";

/// The paths whose volumes `disk` reports: the repository's build root, its
/// checkout (the common git directory of a bare one), the backup, and
/// [`NIX`] when `nix` says it exists. A path the environment can't name is
/// left out.
pub fn places(environment: &Environment, located: &Located, nix: bool) -> Vec<PathBuf> {
    [
        names::build_root(environment, &located.common_dir),
        Some(
            located
                .worktree
                .clone()
                .unwrap_or_else(|| located.common_dir.clone()),
        ),
        names::backup_root(environment),
        nix.then(|| PathBuf::from(NIX)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// A path, with what the system said of the volume holding it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Measured {
    pub path: PathBuf,
    pub volume: u64,
    pub free_bytes: u64,
}

/// The report of `measured`, each volume once, under the first path on it.
pub fn report(floors: Floors, measured: &[Measured]) -> DiskReport {
    DiskReport {
        schema: SCHEMA,
        command: CommandName::Disk,
        floors,
        disks: measured
            .iter()
            .enumerate()
            .filter(|(index, one)| {
                !measured
                    .iter()
                    .take(*index)
                    .any(|before| before.volume == one.volume)
            })
            .map(|(_, one)| {
                let free_gb = gb(one.free_bytes);
                Disk {
                    path: one.path.clone(),
                    free_gb,
                    state: state(free_gb, &floors),
                }
            })
            .collect(),
    }
}

/// The report as text: the floors with their sources, then a line per
/// volume, its free space rounded down to two places so a volume just
/// under a floor never shows as at it. The JSON's `free_gb` isn't rounded.
pub fn text(sourced: &SourcedFloors, report: &DiskReport) -> String {
    let floors = [
        ("build floor", report.floors.build_gb, &sourced.build),
        (
            "cleanup-only floor",
            report.floors.cleanup_only_gb,
            &sourced.cleanup_only,
        ),
    ]
    .into_iter()
    .map(|(name, gb, source)| format!("{name}: {gb} GB ({source})\n"));
    let disks = report.disks.iter().map(|disk| {
        format!(
            "{}: {} GB free, {}\n",
            disk.path.display(),
            two_places_down(disk.free_gb),
            disk.state
        )
    });
    floors.chain(disks).collect()
}

/// `gb` with two decimal places, rounded down. It is printed to nine places
/// first, a byte's, which rounds away the float error of a whole number of
/// bytes, then cut.
fn two_places_down(gb: f64) -> String {
    let exact = format!("{gb:.9}");
    match exact.split_once('.') {
        Some((whole, fraction)) => format!("{whole}.{}", fraction.get(..2).unwrap_or(fraction)),
        None => exact,
    }
}
