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
