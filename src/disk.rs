//! The disk floors, and whether a volume has room for a build.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::ids::CommandName;

/// The floors, in GB of 10^9 bytes: `WORKTREE_MANAGER_FLOOR_*_GB`, else
/// the floors file, else 70 and 35.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Floors {
    /// Below it, no build starts.
    pub build_gb: f64,
    /// Below it, only cleanup runs.
    pub cleanup_only_gb: f64,
}

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

/// A volume's state with `free_gb` free.
pub fn state(free_gb: f64, floors: &Floors) -> DiskState {
    let _ = (free_gb, floors);
    unimplemented!("disk::state")
}
