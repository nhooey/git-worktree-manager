//! Remove git worktrees and branches once their work lands, archive what
//! would be lost, and keep worktrees' build output outside them and shared.
//!
//! The modules here are pure: values in, new values out. Reading git, the
//! file system and the environment belongs to the edge that calls them.

#![forbid(unsafe_code)]

#[macro_use]
mod string_enum;

pub mod build_root;
pub mod names;
mod python;
pub mod rules;
