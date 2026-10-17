//! Console modes: `scan` (a port of the Python version's run_cli) and
//! `probe-jsonl` (one machine-readable line per file, used to compare this
//! program with the Python version file by file).

use crate::fmt::{commas, json_str};
use crate::formats::{DEFAULT_HOLE_KB, VERDICTS};
use crate::report::{
    format_library_survey, format_untestable, library_csv_path, library_row, untestable_csv_path, untestable_key,
    write_library_csv, write_results_csv, Tally,
};
use crate::walk::{abspath, walk_and_probe, Event, LibInfo, Row};
use crate::{APP, VERSION};
use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set by Ctrl+C, so a scan can stop cleanly and still write its results.
pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
pub fn install_ctrl_c() {
    extern "system" fn handler(ctrl_type: u32) -> i32 {
        if ctrl_type == 0 {
            // CTRL_C_EVENT
            INTERRUPTED.store(true, Ordering::Relaxed);
            return 1;
        }
        0
    }
    extern "system" {
        fn SetConsoleCtrlHandler(handler: Option<extern "system" fn(u32) -> i32>, add: i32) -> i32;
    }
    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }
}

#[cfg(unix)]
pub fn install_ctrl_c() {
    extern "C" fn handler(_sig: i32) {
        INTERRUPTED.store(true, Ordering::Relaxed);
    }
    extern "C" {
        fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
    }
    unsafe {
        signal(2, handler); // SIGINT
    }
}

/// Options shared by the console modes.
pub struct ScanOpts {
    pub hole_bytes: u64,
    pub do_hole: bool,
    pub out: Option<String>,
}

impl Default for ScanOpts {
    fn default() -> Self {
        ScanOpts { hole_bytes: DEFAULT_HOLE_KB * 1024, do_hole: true, out: None }
    }
}

/// The console scan: prints problems as it finds them, then writes the
/// results CSV, the not-testable list and the library inventory.
pub fn run_scan(target: &str, opts: &ScanOpts) -> i32 {
    if !std::path::Path::new(target).is_dir() {
        println!("Not a folder: {target}");
        return 2;
    }
    let out = opts.out.clone().unwrap_or_else(|| {
        let cwd = std::env::current_dir().map(|c| c.to_string_lossy().into_owned()).unwrap_or_default();
        crate::walk::join(&cwd, &format!("probe-results-{}.csv", crate::fmt::now_stamp()))
    });
    let mut counts: HashMap<&'static str, u64> = VERDICTS.iter().map(|v| (*v, 0)).collect();
    let mut rows: Vec<Row> = Vec::new();
    let mut skipped: Vec<Row> = Vec::new();
    let mut libraries: Vec<LibInfo> = Vec::new();
    let mut untestable = Tally::default();
    let mut scanned: u64 = 0;
    println!("{APP} - version {VERSION}\nScanning {target}\nWriting to {out}\nCtrl+C to stop.\n");
    install_ctrl_c();
    walk_and_probe(target, opts.hole_bytes, opts.do_hole, Some(&INTERRUPTED), |ev| match ev {
        Event::Lib(_, info) => {
            println!("  PHOTOS LIBRARY  {}\n                  (detected by {})", info.path, info.how);
            libraries.push(info);
        }
        Event::LibDone(i, info) => {
            rows.push(library_row(&info));
            *counts.entry("LIBRARY").or_insert(0) += 1;
            if i < libraries.len() {
                libraries[i] = info;
            }
        }
        Event::Row(r) => {
            scanned += 1;
            *counts.entry(r.verdict).or_insert(0) += 1;
            if r.verdict == "SKIPPED" {
                untestable.add(&untestable_key(&r.path));
                skipped.push(r.clone());
            }
            if r.verdict != "OK" && r.verdict != "SKIPPED" {
                println!("  {:<11} {}\n              {}", r.verdict, r.path, r.detail);
                rows.push(r);
            }
            if scanned % 2000 == 0 {
                println!("  ... {} files, {} flagged", commas(scanned), commas(rows.len() as u64));
            }
        }
    });
    if INTERRUPTED.load(Ordering::Relaxed) {
        println!("\nStopped early.");
    }
    if let Err(e) = write_results_csv(&out, &rows) {
        println!("Could not write {out}: {e}");
        return 1;
    }
    let c = |k: &str| counts.get(k).copied().unwrap_or(0);
    println!("\n{} files examined, {} flagged.", commas(scanned), commas(rows.len() as u64));
    println!(
        "  db damaged {}   truncated {}   holes {}   mismatched {}   unreadable {}",
        commas(c("DB_DAMAGED")),
        commas(c("TRUNCATED")),
        commas(c("HOLE")),
        commas(c("MISMATCH")),
        commas(c("UNREADABLE"))
    );
    println!(
        "  clean {}   not testable {}   <- camera RAW and HEIC always land here",
        commas(c("OK")),
        commas(c("SKIPPED"))
    );
    for line in format_untestable(&untestable, c("SKIPPED")) {
        println!("{line}");
    }
    for line in format_library_survey(&libraries) {
        println!("{line}");
    }
    if !libraries.is_empty() {
        let libcsv = library_csv_path(&out);
        match write_library_csv(&libcsv, &libraries) {
            Ok(()) => println!("  Library inventory: {libcsv}"),
            Err(e) => println!("  (could not write the library inventory: {e})"),
        }
    }
    if !skipped.is_empty() {
        let ntcsv = untestable_csv_path(&out);
        match write_results_csv(&ntcsv, &skipped) {
            Ok(()) => println!("  Not-testable list: {ntcsv}"),
            Err(e) => println!("  (could not write the not-testable list: {e})"),
        }
    }
    println!("\nResults: {out}");
    0
}

/// One JSON line per file: {"path", "verdict", "detail", "size"}.
pub fn run_probe_jsonl(target: &str, opts: &ScanOpts) -> i32 {
    let stdout = std::io::stdout();
    let mut w = std::io::BufWriter::new(stdout.lock());
    let root = abspath(target);
    walk_and_probe(&root, opts.hole_bytes, opts.do_hole, None, |ev| {
        if let Event::Row(r) = ev {
            let _ = writeln!(
                w,
                "{{\"path\":{},\"verdict\":{},\"detail\":{},\"size\":{}}}",
                json_str(&r.path),
                json_str(r.verdict),
                json_str(&r.detail),
                r.size
            );
        }
    });
    let _ = w.flush();
    0
}

/// The console map check.
pub fn run_map_check(map_path: &str, earlier: &[String], disk: &str, out_dir: &str) -> i32 {
    let opts = crate::mapcheck::Options {
        map_path: map_path.to_string(),
        earlier_maps: earlier.to_vec(),
        disk: disk.to_string(),
        out_dir: out_dir.to_string(),
    };
    let mut progress = |s: &str| println!("  {s}");
    match crate::mapcheck::run(&opts, &mut progress) {
        Ok(report) => {
            println!();
            for line in crate::mapcheck::summary(&report) {
                println!("{line}");
            }
            println!();
            println!("Files written:");
            for w in &report.written {
                println!("  {w}");
            }
            0
        }
        Err(e) => {
            println!("The map check could not finish: {e}");
            1
        }
    }
}

/// Every disk this computer can see.
pub fn run_list_disks() -> i32 {
    let disks = crate::disk::list_disks();
    if disks.is_empty() {
        println!("No disks found (listing disks only works on Windows).");
        return 1;
    }
    for d in disks {
        match &d.error {
            Some(e) => println!("  {}  -  {}", d.spec, e),
            None => println!(
                "  {}  {:>9}  {}{}",
                d.spec,
                crate::fmt::size(d.size),
                d.model,
                if d.bus.is_empty() { String::new() } else { format!(" ({})", d.bus) }
            ),
        }
    }
    0
}

/// What a mapfile says, in plain words.
pub fn run_map_info(map_path: &str) -> i32 {
    let text = match std::fs::read(map_path) {
        Ok(t) => t,
        Err(e) => {
            println!("Could not read {map_path}: {e}");
            return 1;
        }
    };
    let m = match crate::mapfile::parse(&String::from_utf8_lossy(&text)) {
        Ok(m) => m,
        Err(e) => {
            println!("{map_path} is not a ddrescue mapfile I can read: {e}");
            return 1;
        }
    };
    let bad = m.bad_areas();
    let bad_bytes: u64 = bad.iter().map(|a| a.len()).sum();
    println!("Mapfile:      {map_path}");
    println!("ddrescue:     {}", if m.version.is_empty() { "?" } else { &m.version });
    println!("Started:      {}", m.start_time);
    println!("Last saved:   {}", m.current_time);
    println!("Finished:     {}", if m.finished { "yes" } else { "no - the copy was stopped before the end" });
    println!(
        "Drive copied: {} bytes ({}), from byte {}",
        commas(m.domain_size()),
        crate::fmt::size(m.domain_size()),
        commas(m.domain_start)
    );
    println!("Read:         {} bytes", commas(m.rescued_bytes()));
    println!(
        "Never read:   {} bytes ({}) in {} places",
        commas(bad_bytes),
        crate::fmt::size(bad_bytes),
        commas(bad.len() as u64)
    );
    let mut by_status: Vec<(char, u64, u64)> = Vec::new();
    for a in &bad {
        match by_status.iter_mut().find(|s| s.0 == a.status) {
            Some(s) => {
                s.1 += 1;
                s.2 += a.len();
            }
            None => by_status.push((a.status, 1, a.len())),
        }
    }
    for (st, n, bytes) in by_status {
        println!("  {:<13} {:>7} places, {:>15} bytes", crate::mapfile::status_name(st), commas(n), commas(bytes));
    }
    if let (Some(first), Some(last)) = (bad.first(), bad.last()) {
        println!("They lie between byte {} and byte {}.", commas(first.start), commas(last.end));
    }
    for w in &m.warnings {
        println!("Note: {w}");
    }
    0
}
