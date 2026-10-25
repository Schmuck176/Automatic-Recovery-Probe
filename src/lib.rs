//! Automatic Recovery Probe: Recovery Probe (2026-10-22 (1)) with the
//! recovery automated.
//!
//! What Recovery Probe was:
//!
//! Finds damage in files recovered with ddrescue - the kind that opens fine
//! and looks normal - and sorts every file into damaged, not testable or
//! clean - and, around that, everything a one-chance recovery needs: an
//! image of the recovered disk, the map check, the copying with a checksum
//! per file, the safety checks, the repairs, and the checklist the person
//! follows. (The file checks began as a Python program; the port was checked
//! against it file by file until the Python was retired.)
//!
//! Only Rust's standard library is used: no third-party code runs over the
//! recovered files.

pub mod aim;
pub mod autorepair;
pub mod checklist;
pub mod checks;
pub mod cli;
pub mod disk;
pub mod exfat;
pub mod fmt;
pub mod extra;
pub mod formats;
pub mod gpt;
pub mod guard;
pub mod handoff;
pub mod holes;
pub mod image;
pub mod join;
pub mod mapcheck;
pub mod mapfile;
pub mod mp4time;
pub mod probe;
pub mod repair;
pub mod report;
pub mod rescue;
pub mod selftest;
pub mod sha256;
pub mod sqlite;
pub mod sys;
pub mod tools;
pub mod verify;
pub mod walk;
pub mod web;
pub mod win;
pub mod winset;

pub const APP: &str = "Automatic Recovery Probe";
/// Shown in the window and console, so you can always tell which copy is running.
pub const VERSION: &str = "2026-10-25 (2)";
