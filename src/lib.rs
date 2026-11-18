//! Remove git worktrees and branches once their work lands, archive what
//! would be lost, and keep worktrees' build output outside them.
//!
//! `model` holds the values every part passes along. The pure modules
//! decide; `system` and the readers are the edge that reads; `commands`
//! join the two and act through `system` again.

#![forbid(unsafe_code)]

#[macro_use]
mod string_enum;

// The base: shared values, names, the edge and the pure functions every
// later part calls.
pub mod backup;
pub mod cli;
pub mod commands;
pub mod environment;
pub mod keep;
pub mod landed;
pub mod locate;
pub mod lock;
pub mod model;
pub mod names;
pub mod system;
pub mod targets;
pub mod time;

// Each later part's own module.
pub mod archive;
pub mod build_root;
pub mod disk;
pub mod hook;
pub mod read;
pub mod render;
pub mod report;
pub mod rescue;
pub mod rules;
pub mod sweep;
