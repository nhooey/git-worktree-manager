//! The reading edge: a repository's facts and settings, through gitoxide,
//! with `ps` and the clock behind the system edge. It writes nothing.

use std::path::Path;

use crate::model::facts::RepositoryFacts;
use crate::model::settings::Settings;
use crate::system::{Failure, System};
use crate::targets::Target;

/// Everything the rules read about `target`'s repository; bytes on disk
/// only when `size` is set.
pub fn repository(
    system: &impl System,
    target: &Target,
    size: bool,
) -> Result<RepositoryFacts, Failure> {
    let _ = (system, target, size);
    unimplemented!("read::repository")
}

/// `worktreeManager.*` from the git config of the repository whose common
/// git directory is `common_dir`.
pub fn settings(common_dir: &Path) -> Result<Settings, Failure> {
    let _ = common_dir;
    unimplemented!("read::settings")
}
