//! Remove git worktrees and branches once their work lands, archive what
//! would be lost, and keep worktrees' build output outside them and shared.
//!
//! The pure modules, `rules` and `build_root`, take values and return new
//! ones. `system` is the edge that reads git, the file system and the
//! clock, and `commands` joins the two.

#![forbid(unsafe_code)]

#[macro_use]
mod string_enum;

pub mod build_root;
mod clean;
pub mod cli;
pub mod commands;
pub mod names;
mod python;
pub mod rules;
pub mod system;

/// The program's name, from `Cargo.toml`, for its help and its messages.
pub const PROGRAM: &str = env!("CARGO_PKG_NAME");
