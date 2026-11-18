//! `disk`: the floors, each volume's free space and its state. It reads,
//! and creates and writes nothing.

use std::path::{Path, PathBuf};

use crate::cli::{Format, FormatArg, RepositoryArg};
use crate::commands::{Status, print_json};
use crate::disk::{self, FloorsFile, Measured};
use crate::locate::{Located, Reach, locate};
use crate::model::ids::ErrorCode;
use crate::names;
use crate::system::{Failure, System};

#[derive(clap::Args, Clone, Debug, PartialEq, Eq)]
pub struct Args {
    #[command(flatten)]
    pub repository: RepositoryArg,
    #[command(flatten)]
    pub format: FormatArg,
}

/// Whether `run` does the command's work yet.
pub const BUILT: bool = true;

pub fn run(args: &Args, system: &mut impl System) -> Result<Status, Failure> {
    let here = system.real_path(
        args.repository
            .repository
            .as_deref()
            .unwrap_or(Path::new(".")),
    )?;
    let located = locate(system, &here, Reach::Upward)?.ok_or_else(|| {
        Failure::new(
            ErrorCode::NotARepository,
            format!("{} is not in a repository", here.display()),
        )
    })?;
    run_in(args, &located, system)
}

/// `disk` for the repository `located`.
pub fn run_in(args: &Args, located: &Located, system: &mut impl System) -> Result<Status, Failure> {
    let file = names::floor_file(system.environment())
        .map(|path| read_text(system, &path).map(|text| text.map(|text| (path, text))))
        .transpose()?
        .flatten();
    let sourced = disk::floors(
        system.environment(),
        file.as_ref().map(|(path, text)| FloorsFile { path, text }),
    )?;
    sourced
        .warnings
        .iter()
        .for_each(|warning| system.warn(warning));
    let nix = system.metadata(Path::new(disk::NIX))?.is_some();
    let measured = disk::places(system.environment(), located, nix)
        .into_iter()
        .map(|path| measure(system, path))
        .collect::<Result<Vec<_>, _>>()?;
    let report = disk::report(sourced.floors, &measured);
    match args.format.format {
        Format::Json => print_json(system, &report)?,
        Format::Text => system.print(&disk::text(&sourced, &report))?,
    }
    Ok(Status::Done)
}

/// A file's text; None when it doesn't exist.
fn read_text(system: &impl System, path: &Path) -> Result<Option<String>, Failure> {
    system
        .read_file(path)?
        .map(|bytes| {
            String::from_utf8(bytes)
                .map_err(|_| Failure::unreadable(format!("{}: not UTF-8", path.display())))
        })
        .transpose()
}

/// `path`'s volume, read at the nearest folder that exists, since a build
/// root may not be made yet.
fn measure(system: &impl System, path: PathBuf) -> Result<Measured, Failure> {
    let real = system.real_path(&path)?;
    let existing = real
        .ancestors()
        .find_map(|at| match system.metadata(at) {
            Ok(Some(_)) => Some(Ok(at)),
            Ok(None) => None,
            Err(failure) => Some(Err(failure)),
        })
        .unwrap_or_else(|| {
            Err(Failure::unreadable(format!(
                "{}: nothing on its path exists",
                path.display()
            )))
        })?;
    Ok(Measured {
        volume: system.volume(existing)?,
        free_bytes: system.free_bytes(existing)?,
        path,
    })
}
