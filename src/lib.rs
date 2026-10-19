//! Recovery Probe.
//!
//! Finds damage in files recovered with ddrescue - the kind that opens fine
//! and looks normal - and sorts every file into damaged, not testable or
//! clean. A Rust port of recovery_probe.py (kept in `reference/`), checked
//! against it file by file.
//!
//! Only Rust's standard library is used: no third-party code runs over the
//! recovered files.

pub mod checklist;
pub mod checks;
pub mod cli;
pub mod disk;
pub mod exfat;
pub mod fmt;
pub mod formats;
pub mod gpt;
pub mod guard;
pub mod holes;
pub mod image;
pub mod join;
pub mod mapcheck;
pub mod mapfile;
pub mod probe;
pub mod repair;
pub mod report;
pub mod rescue;
pub mod selftest;
pub mod sha256;
pub mod sqlite;
pub mod sys;
pub mod verify;
pub mod walk;
pub mod web;
pub mod win;

pub const APP: &str = "Recovery Probe";
/// Shown in the window and console, so you can always tell which copy is running.
pub const VERSION: &str = "2026-10-19 (2)";
