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
    /// A map check's results folder: its files that lost data are marked
    /// NEVER_READ, as step 7 on the page does with "Use what step 5 found".
    pub map_check: Option<String>,
}

impl Default for ScanOpts {
    fn default() -> Self {
        ScanOpts { hole_bytes: DEFAULT_HOLE_KB * 1024, do_hole: true, out: None, map_check: None }
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
    let items = match &opts.map_check {
        Some(dir) => {
            let p = std::path::Path::new(dir).join(crate::join::TSV_NAME);
            match std::fs::read_to_string(&p) {
                Ok(t) => crate::join::parse_tsv(&t),
                Err(e) => {
                    println!("Could not read the map check's list {}: {e}", p.display());
                    return 2;
                }
            }
        }
        None => Vec::new(),
    };
    let mut collector = crate::join::Collector::new(&items);
    collector.note_returns(target);
    println!("{APP} - version {VERSION}\nScanning {target}\nWriting to {out}\nCtrl+C to stop.\n");
    install_ctrl_c();
    let still_there = walk_and_probe(target, opts.hole_bytes, opts.do_hole, Some(&INTERRUPTED), |ev| match ev {
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
            collector.saw(&r);
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
    if !still_there {
        println!("\n{}", crate::walk::GONE);
        return 1;
    }
    if INTERRUPTED.load(Ordering::Relaxed) {
        println!("\nStopped early.");
    }
    if !items.is_empty() {
        let o = crate::join::find(&items, &collector);
        for line in crate::join::apply(&o, &mut rows, &mut skipped) {
            println!("{line}");
        }
        counts.insert(crate::join::VERDICT, rows.iter().filter(|r| r.verdict == crate::join::VERDICT).count() as u64);
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

/// Progress for whoever started this run: the front end starts the map
/// check and the rescue as a separate copy of the program (Windows only
/// lets a copy with administrator rights read a disk directly) and follows
/// it through this file. Lines: "P progress", "S summary", then "OK" or
/// "ERR why".
pub struct Status {
    path: Option<String>,
}

impl Status {
    pub fn new(path: Option<String>) -> Status {
        Status { path }
    }
    fn write(&self, kind: &str, text: &str) {
        if let Some(p) = &self.path {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                for line in text.lines() {
                    let _ = writeln!(f, "{kind} {line}");
                }
                if text.is_empty() {
                    let _ = writeln!(f, "{kind}");
                }
            }
        }
    }
    pub fn progress(&self, text: &str) {
        self.write("P", text);
    }
    pub fn summary(&self, text: &str) {
        self.write("S", text);
    }
    pub fn amount(&self, done: u64, total: u64) {
        self.write("A", &format!("{done} {total}"));
    }
    pub fn ok(&self) {
        self.write("OK", "");
    }
    pub fn failed(&self, why: &str) {
        self.write("ERR", &why.replace('\n', " "));
    }
}

/// The console map check.
pub fn run_map_check(map_path: &str, earlier: &[String], disk: &str, out_dir: &str, status: &Status) -> i32 {
    let opts = crate::mapcheck::Options {
        map_path: map_path.to_string(),
        earlier_maps: earlier.to_vec(),
        disk: disk.to_string(),
        out_dir: out_dir.to_string(),
    };
    let mut progress = |s: &str| {
        println!("  {s}");
        status.progress(s);
    };
    match crate::mapcheck::run(&opts, &mut progress) {
        Ok(report) => {
            println!();
            for line in crate::mapcheck::summary(&report) {
                println!("{line}");
                status.summary(&line);
            }
            println!();
            println!("Files written:");
            for w in &report.written {
                println!("  {w}");
            }
            status.ok();
            0
        }
        Err(e) => {
            println!("The map check could not finish: {e}");
            status.failed(&e);
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

/// Copy files straight off the recovered disk, past Windows.
pub fn run_rescue(
    disk: &str,
    out_dir: &str,
    pick: crate::rescue::Pick,
    lost: bool,
    stop_after: Option<u64>,
    status: &Status,
) -> i32 {
    let opts = crate::rescue::Options {
        disk: disk.to_string(),
        out_dir: out_dir.to_string(),
        pick,
        lost,
        stop: None,
        stop_after_files: stop_after,
    };
    let mut progress = |s: &str| {
        println!("  {s}");
        status.progress(s);
    };
    let say = |s: String| {
        println!("{s}");
        status.summary(&s);
    };
    let mut amount = |done: u64, total: u64| status.amount(done, total);
    match crate::rescue::run(&opts, &mut progress, &mut amount) {
        Ok(o) => {
            println!();
            say(format!(
                "Copied {} files ({}) and {} folders into {}.",
                commas(o.files),
                crate::fmt::size(o.bytes),
                commas(o.folders),
                out_dir
            ));
            if o.lost_pieces > 0 {
                say(format!(
                    "Saved {} of data no folder record names ({}) in \"lost data (no name)\".",
                    crate::rescue::pieces(o.lost_pieces),
                    crate::fmt::size(o.lost_bytes)
                ));
            }
            if o.already_there > 0 {
                say(format!("{} files were already in that folder and were left as they were.", commas(o.already_there)));
            }
            if o.recopied > 0 {
                say(format!(
                    "{} files an earlier run had left did not hold what the disk holds, so they were copied again.",
                    commas(o.recopied)
                ));
            }
            if !o.partial.is_empty() {
                say(format!(
                    "{} files could only be copied in part (the missing parts are zeros):",
                    commas(o.partial.len() as u64)
                ));
                for (p, n) in o.partial.iter().take(10) {
                    say(format!("  {p}: {n}"));
                }
            }
            if !o.failed.is_empty() {
                say(format!("{} could not be copied:", commas(o.failed.len() as u64)));
                for (p, n) in o.failed.iter().take(10) {
                    say(format!("  {p}: {n}"));
                }
            }
            if let Some(e) = &o.disk_lost {
                say(crate::rescue::disk_lost_text(e));
            } else if let Some(e) = &o.dest_lost {
                say(crate::rescue::dest_lost_text(e));
            } else if let Some(e) = &o.keeps_failing {
                say(crate::rescue::keeps_failing_text(e));
            } else if o.stopped {
                say("Stopped before the end. Run it again to copy the rest: what is already copied is kept.".to_string());
            }
            say(format!("The full list is in {}", o.log));
            if o.disk_lost.is_some() || o.dest_lost.is_some() || o.keeps_failing.is_some() {
                status.failed("stopped before the end (see above)");
                1
            } else if o.failed.is_empty() {
                status.ok();
                0
            } else {
                status.failed(&format!("{} files could not be copied (see the list)", o.failed.len()));
                1
            }
        }
        Err(e) => {
            println!("The rescue could not run: {e}");
            status.failed(&e);
            1
        }
    }
}

/// Make (or carry on making, or check) an image of a disk.
pub fn run_image(disk: &str, map_path: &str, out: &str) -> i32 {
    let size = match std::fs::read(map_path).map_err(|e| e.to_string()).and_then(|t| crate::mapfile::parse(&String::from_utf8_lossy(&t))) {
        Ok(m) => m.domain_size(),
        Err(e) => {
            println!("Could not read the mapfile {map_path}: {e}");
            return 1;
        }
    };
    let src = match crate::disk::open_source(disk) {
        Ok(s) => s,
        Err(e) => {
            println!("could not open {disk}: {e}");
            return 1;
        }
    };
    install_ctrl_c();
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let opts = crate::image::Options { disk: disk.to_string(), out_dir: out.to_string(), size, stop: Some(stop.clone()) };
    let mut last = std::time::Instant::now();
    let r = crate::image::make(&*src, &opts, &mut |s| println!("  {s}"), &mut |d, t| {
        if INTERRUPTED.load(Ordering::Relaxed) {
            stop.store(true, Ordering::Relaxed);
        }
        if last.elapsed().as_secs() >= 5 && t > 0 {
            println!("  {} of {} ({:.1}%)", crate::fmt::size(d), crate::fmt::size(t), 100.0 * d as f64 / t as f64);
            last = std::time::Instant::now();
        }
    });
    match r {
        Ok(o) => {
            for l in crate::image::summary(&o) {
                println!("{l}");
            }
            if o.verified {
                0
            } else {
                1
            }
        }
        Err(e) => {
            println!("{e}");
            1
        }
    }
}

/// Check the copies in a folder against rescue-log.csv, and (with the map
/// check's results folder) the safety check.
pub fn run_verify(folder: &str, map_check: Option<&str>) -> i32 {
    install_ctrl_c();
    // (Not into the folder being checked: that would add a file to it.)
    let out = std::env::current_dir().unwrap_or_default().join(format!("verify-{}.csv", crate::fmt::now_stamp()));
    if let Some(mc) = map_check {
        if crate::verify::map_check_totals(std::path::Path::new(mc)).is_none() {
            println!("{mc} has no map-check.json with the numbers of files: is it a map check's results folder?");
            return 2;
        }
    }
    match crate::verify::run_checked(
        std::path::Path::new(folder),
        map_check.map(std::path::Path::new),
        Some(&out),
        &mut |s| println!("  {s}"),
        &mut |_, _| {},
        &|| INTERRUPTED.load(Ordering::Relaxed),
    ) {
        Ok(r) => {
            for l in crate::verify::summary(&r) {
                println!("{l}");
            }
            let fine = if r.stopped {
                false
            } else {
                let s = crate::verify::safety(&r);
                println!();
                for l in s.lines() {
                    println!("{l}");
                }
                s.fine
            };
            if r.changed.is_empty() && r.missing.is_empty() && fine {
                0
            } else {
                1
            }
        }
        Err(e) => {
            println!("{e}");
            1
        }
    }
}

/// The folder this program is in (where its notes and results go), or the
/// temporary folder if that cannot be written to.
pub fn home_dir() -> std::path::PathBuf {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(std::env::temp_dir);
    let probe = dir.join(".recovery-probe-write-test");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            dir
        }
        Err(_) => std::env::temp_dir(),
    }
}

/// Where the program notes a Windows setting it changed, so it can always
/// be put back - also after the program was closed in between.
pub fn settings_note() -> std::path::PathBuf {
    home_dir().join("recovery-probe-windows-settings.txt")
}

/// The SAN policy that was in force before `prepare`, if the program changed it.
pub fn policy_to_restore() -> Option<crate::guard::SanPolicy> {
    let text = std::fs::read_to_string(settings_note()).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("san-policy-before\t"))
        .and_then(crate::guard::SanPolicy::from_keyword)
}

/// Set Windows to leave new disks switched off, noting how it was.
pub fn guard_prepare() -> Result<String, String> {
    use crate::guard::{san_policy, set_san_policy, SanPolicy};
    let now = san_policy()?;
    if policy_to_restore().is_none() {
        std::fs::write(
            settings_note(),
            format!(
                "san-policy-before\t{}\r\nThis note is written by Recovery Probe: it changed how Windows treats newly \
                 plugged-in disks and will put it back. Delete it only after running: recovery-probe disk-guard restore\r\n",
                now.keyword()
            ),
        )
        .map_err(|e| format!("could not write {}: {e}", settings_note().display()))?;
    }
    set_san_policy(SanPolicy::OfflineAll)?;
    let after = san_policy()?;
    if after != SanPolicy::OfflineAll {
        return Err(format!("Windows still says: {}", after.describe()));
    }
    Ok(format!("Before: {}. Now: {}.", now.describe(), after.describe()))
}

/// Put back the SAN policy `guard_prepare` changed.
pub fn guard_restore() -> Result<String, String> {
    use crate::guard::{san_policy, set_san_policy};
    let Some(old) = policy_to_restore() else {
        return Ok("Nothing to put back: the program has not changed this setting.".to_string());
    };
    set_san_policy(old)?;
    let now = san_policy()?;
    if now != old {
        return Err(format!("Windows still says: {}", now.describe()));
    }
    let _ = std::fs::remove_file(settings_note());
    Ok(format!("Put back: {}.", now.describe()))
}

/// `disk-guard status | prepare | protect <disk> [--online] | online <disk> | offline <disk> | restore`
pub fn run_disk_guard(args: &[String]) -> i32 {
    use crate::guard::{protect, san_policy, san_policy_registry, set_online, state};
    let disk_arg = |i: usize| -> Result<u32, String> {
        args.get(i)
            .and_then(|s| s.trim_start_matches(r"\\.\PhysicalDrive").parse().ok())
            .ok_or_else(|| "which disk? (its number, as list-disks shows it)".to_string())
    };
    let r: Result<String, String> = match args.first().map(|s| s.as_str()) {
        Some("status") => {
            let mut t = String::new();
            match san_policy() {
                Ok(p) => t.push_str(&format!("SAN policy: {} ({})\n", p.keyword(), p.describe())),
                Err(e) => t.push_str(&format!("SAN policy: unknown ({e})\n")),
            }
            t.push_str(&format!("SAN policy in the registry: {:?}\n", san_policy_registry()));
            if let Some(p) = policy_to_restore() {
                t.push_str(&format!("To put back afterwards: {}\n", p.keyword()));
            }
            for d in crate::win::disks() {
                match state(d.number) {
                    Ok(s) => t.push_str(&format!(
                        "disk {}: offline {}, read-only {}, writable {:?}\n",
                        d.number, s.offline, s.read_only, s.writable
                    )),
                    Err(e) => t.push_str(&format!("disk {}: {e}\n", d.number)),
                }
            }
            Ok(t)
        }
        Some("prepare") => guard_prepare(),
        Some("restore") => guard_restore(),
        Some("protect") => disk_arg(1).and_then(|n| {
            protect(n, args.iter().any(|a| a == "--online"))
                .map(|s| format!("disk {n}: read-only {}, offline {}, writable {:?}", s.read_only, s.offline, s.writable))
        }),
        Some(w @ ("online" | "offline")) => {
            let on = w == "online";
            disk_arg(1).and_then(|n| set_online(n, on)).map(|_| format!("disk switched {}", if on { "on" } else { "off" }))
        }
        _ => Err("disk-guard status | prepare | protect <disk> [--online] | online <disk> | offline <disk> | restore"
            .to_string()),
    };
    match r {
        Ok(t) => {
            println!("{}", t.trim_end());
            0
        }
        Err(e) => {
            println!("{e}");
            1
        }
    }
}

/// The repair steps from the command line: isolate, put-back, undo, evaluate.
pub fn run_repair_step(step: &str, folder: &str, scans: &[String], id: Option<&str>) -> i32 {
    install_ctrl_c();
    let work = std::path::Path::new(folder);
    let scans: Vec<std::path::PathBuf> = scans.iter().map(std::path::PathBuf::from).collect();
    let stop = || INTERRUPTED.load(Ordering::Relaxed);
    let show = |lines: Vec<String>| {
        for l in lines {
            println!("{l}");
        }
    };
    let result: Result<i32, String> = match step {
        "isolate" => crate::repair::isolate(work, &scans, &mut |s| println!("  {s}"), &stop).map(|o| {
            show(crate::repair::isolate_summary(&o));
            i32::from(!o.problems.is_empty() || o.stopped)
        }),
        "put-back" => crate::repair::put_back(work, &mut |s| println!("  {s}"), &stop).map(|o| {
            let repair = crate::repair::repair_dir(work).unwrap_or_default();
            show(crate::repair::put_back_summary(&o, &repair));
            i32::from(!o.skipped.is_empty() || o.stopped)
        }),
        "undo" => crate::repair::undo(work, id.unwrap_or("")).map(|m| {
            println!("{m}");
            0
        }),
        _ => crate::repair::evaluate(work, &scans, &mut |s| println!("  {s}"), &stop).map(|o| {
            show(crate::repair::evaluation_summary(&o));
            i32::from(o.stopped)
        }),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            println!("{e}");
            1
        }
    }
}
