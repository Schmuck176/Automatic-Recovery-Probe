//! Checking copies against the list the copying made (rescue-log.csv).
//!
//! Every file and piece of lost data the copying writes gets its SHA-256 in
//! rescue-log.csv, computed from the bytes as they came off the disk. Reading
//! a copy back and comparing proves it is still exactly that - after a power
//! cut, after a trip to another computer, or years later. Nothing is changed:
//! the copies are only read.
//!
//! The safety check adds it all up, stage by stage: the files the map check
//! found on the SSD (step 5), the files the list of copies names (step 6),
//! and the copies here now. The same number of files, and the same size, at
//! every stage means nothing was lost on the way; a stage with fewer says
//! where something went missing.

use crate::fmt::{commas, csv_row, json_str, size};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// What the map check (step 5) found on the SSD, from map-check.json in its
/// results folder.
#[derive(Debug, Clone, PartialEq)]
pub struct OnSsd {
    pub files: u64,
    /// Their total size (None for a map check made before the program added
    /// it up).
    pub bytes: Option<u64>,
    pub folders: u64,
    /// The results folder's name: which map check this is.
    pub from: String,
}

/// The list of copies (rescue-log.csv), added up.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct ListTotals {
    /// Files copied in full or in part, or left as they were from an earlier
    /// run: their number, and their size on the SSD.
    pub files: u64,
    pub file_bytes: u64,
    /// Files that could not be copied.
    pub failed: u64,
    /// Pieces of data no folder names any more, saved, and their size.
    pub pieces: u64,
    pub piece_bytes: u64,
    /// The copying did not get to the end: the list says to run it again, or
    /// the list a run keeps while it copies is still there (a power cut?).
    pub unfinished: bool,
}

impl ListTotals {
    /// Counts one line of the list. True for a line naming a copy (a file or
    /// a piece of lost data, copied in full or in part, or left as it was).
    fn add(&mut self, row: &[String]) -> bool {
        if row.len() < 6 || row[0] == "kind" {
            return false;
        }
        if row[0] == "NOTE" {
            if row[5].contains(crate::rescue::RUN_AGAIN) {
                self.unfinished = true;
            }
            return false;
        }
        let file = match row[0].as_str() {
            "FILE" => true,
            "LOST DATA" => false,
            _ => return false,
        };
        let bytes: u64 = row[2].parse().unwrap_or(0);
        match row[3].as_str() {
            "copied" | "partly copied" | "skipped" => {
                if file {
                    self.files += 1;
                    self.file_bytes += bytes;
                } else {
                    self.pieces += 1;
                    self.piece_bytes += bytes;
                }
                true
            }
            "failed" => {
                if file {
                    self.failed += 1;
                }
                false
            }
            _ => false,
        }
    }

    /// The list in `folder`, added up (no copy is read).
    pub fn read(folder: &Path) -> Result<ListTotals, String> {
        let log = folder.join(crate::rescue::LOG_NAME);
        let f = std::fs::File::open(&log).map_err(|e| format!("could not open {}: {e}", log.display()))?;
        let mut t = ListTotals::default();
        crate::fmt::for_each_csv_row(std::io::BufReader::new(f), |row| {
            t.add(&row);
        })
        .map_err(|e| format!("could not read {}: {e}", log.display()))?;
        t.unfinished |= folder.join(crate::rescue::UNFINISHED_LOG_NAME).exists();
        Ok(t)
    }
}

#[derive(Default, Debug)]
pub struct Report {
    /// Copies the list has a checksum for.
    pub listed: u64,
    pub ok: u64,
    pub bytes: u64,
    /// Copies whose contents are not what was copied (path inside the folder).
    pub changed: Vec<String>,
    pub missing: Vec<String>,
    /// Files in the folder the list does not name (added since).
    pub extra: Vec<String>,
    /// Lines with no checksum (lists made before checksums were kept).
    pub no_checksum: u64,
    /// Of the copies checked, repaired versions put back (step 8): checked
    /// against the checksum written when each was put back.
    pub repaired: u64,
    /// The list, added up.
    pub list: ListTotals,
    /// Files the list has a checksum for (those inside a package put back
    /// whole included), and of them, the ones here now, exactly as copied or
    /// as repaired and put back: their number and their size now.
    pub checked_files: u64,
    pub here_files: u64,
    pub here_file_bytes: u64,
    /// Of the files here, repaired versions put back (step 8), every file
    /// of a package put back whole included.
    pub here_repaired: u64,
    /// The same for the pieces of data no folder names.
    pub checked_pieces: u64,
    pub here_pieces: u64,
    pub here_piece_bytes: u64,
    /// What the map check found on the SSD (when its results were given).
    pub on_ssd: Option<OnSsd>,
    pub stopped: bool,
    pub written: Option<String>,
}

struct Want {
    rel: String,
    size: u64,
    sha: String,
    /// A repaired version put back in place of the copy.
    repaired: bool,
    /// A piece of data no folder names (not a file).
    piece: bool,
}

fn components(rel: &str) -> impl Iterator<Item = &str> {
    rel.split('\\').filter(|c| !c.is_empty())
}

/// What the map check found on the SSD, from the map-check.json in its
/// results folder `dir`.
pub fn map_check_totals(dir: &Path) -> Option<OnSsd> {
    let text = std::fs::read_to_string(dir.join("map-check.json")).ok()?;
    let at = text.find("\"volume\":{")?;
    let part = &text[at..];
    // (The volume's label is the only text in it, and cannot hold a `"`
    // that is not escaped.)
    let part = &part[..part.find("\"totals\":").unwrap_or(part.len())];
    let num = |key: &str| -> Option<u64> {
        let i = part.find(&format!("\"{key}\":"))? + key.len() + 3;
        let digits: String = part[i..].chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().ok()
    };
    Some(OnSsd {
        files: num("files")?,
        bytes: num("bytes"),
        folders: num("folders")?,
        from: dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
    })
}

/// Check every copy in `folder` (which holds rescue-log.csv). `out_csv`
/// receives the problems found, if any.
pub fn run(
    folder: &Path,
    out_csv: Option<&Path>,
    progress: &mut dyn FnMut(&str),
    amount: &mut dyn FnMut(u64, u64),
    stop: &dyn Fn() -> bool,
) -> Result<Report, String> {
    run_checked(folder, None, out_csv, progress, amount, stop)
}

/// `run`, with what the map check found on the SSD (`map_check`: its
/// results folder) for the safety check.
pub fn run_checked(
    folder: &Path,
    map_check: Option<&Path>,
    out_csv: Option<&Path>,
    progress: &mut dyn FnMut(&str),
    amount: &mut dyn FnMut(u64, u64),
    stop: &dyn Fn() -> bool,
) -> Result<Report, String> {
    let log = folder.join(crate::rescue::LOG_NAME);
    let f = std::fs::File::open(&log).map_err(|e| {
        format!("{} has no {} (the list the copying makes): {e}", folder.display(), crate::rescue::LOG_NAME)
    })?;
    let mut wants: Vec<Want> = Vec::new();
    let mut r = Report { on_ssd: map_check.and_then(map_check_totals), ..Default::default() };
    crate::fmt::for_each_csv_row(std::io::BufReader::new(f), |row| {
        if !r.list.add(&row) {
            return;
        }
        let sha = row.get(6).map(|s| s.trim().to_lowercase()).unwrap_or_default();
        if sha.len() != 64 {
            r.no_checksum += 1;
            return;
        }
        let rel = if row[4].is_empty() { row[1].clone() } else { row[4].clone() };
        let piece = row[0] != "FILE";
        if piece {
            r.checked_pieces += 1;
        } else {
            r.checked_files += 1;
        }
        wants.push(Want { rel, size: row[2].parse().unwrap_or(0), sha, repaired: false, piece });
    })
    .map_err(|e| format!("could not read {}: {e}", log.display()))?;
    r.list.unfinished |= folder.join(crate::rescue::UNFINISHED_LOG_NAME).exists();
    // Repaired versions put back (step 8): each file is checked against the
    // checksum written when it was put back; a document package put back
    // whole is checked as a whole.
    let now = crate::repair::returned_now(folder);
    let packages: Vec<&crate::repair::Now> = now.values().filter(|n| n.package && n.repaired).collect();
    let package_of = |rel_lower: &str| {
        packages.iter().position(|p| {
            let k = p.path.to_lowercase();
            rel_lower.len() > k.len() && rel_lower.starts_with(&k) && rel_lower.as_bytes()[k.len()] == b'\\'
        })
    };
    // The files of each such package (each checked as part of it).
    let mut members = vec![0u64; packages.len()];
    wants.retain(|w| match package_of(&w.rel.to_lowercase()) {
        Some(i) => {
            if !w.piece {
                members[i] += 1;
            }
            false
        }
        None => true,
    });
    for w in &mut wants {
        if let Some(n) = now.get(&w.rel.to_lowercase()).filter(|n| !n.package) {
            w.sha = n.sha.clone();
            w.size = n.size;
            w.repaired = n.repaired;
        }
    }
    r.listed = wants.len() as u64 + packages.len() as u64;
    let total: u64 = wants.iter().map(|w| w.size).sum();
    progress(&format!("{} copies to check ({}), against {}", commas(r.listed), size(total), log.display()));
    let base = crate::rescue::verbatim(folder);
    let mut done = 0u64;
    let mut last = std::time::Instant::now();
    let mut listed: HashSet<String> = HashSet::new();
    for w in &wants {
        if stop() {
            r.stopped = true;
            break;
        }
        listed.insert(w.rel.to_lowercase());
        let mut p: PathBuf = base.clone();
        for c in components(&w.rel) {
            p.push(c);
        }
        match crate::sha256::file_hex(&p, stop) {
            Ok(Some(got)) if got == w.sha => {
                r.ok += 1;
                r.bytes += w.size;
                if w.repaired {
                    r.repaired += 1;
                }
                if w.piece {
                    r.here_pieces += 1;
                    r.here_piece_bytes += w.size;
                } else {
                    r.here_files += 1;
                    r.here_file_bytes += w.size;
                    if w.repaired {
                        r.here_repaired += 1;
                    }
                }
            }
            Ok(Some(_)) => r.changed.push(w.rel.clone()),
            Ok(None) => {
                r.stopped = true;
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => r.missing.push(w.rel.clone()),
            Err(_) => r.changed.push(w.rel.clone()),
        }
        done += w.size;
        if last.elapsed().as_millis() > 300 {
            amount(done, total);
            progress(&format!("Checked {} of {} copies", commas(r.ok + (r.changed.len() + r.missing.len()) as u64), commas(r.listed)));
            last = std::time::Instant::now();
        }
    }
    for (i, pk) in packages.iter().enumerate() {
        if r.stopped || stop() {
            r.stopped = true;
            break;
        }
        listed.insert(pk.path.to_lowercase());
        let mut p: PathBuf = base.clone();
        for c in components(&pk.path) {
            p.push(c);
        }
        match crate::repair::hash_of(&p, stop) {
            Ok(Some((bytes, got))) if got == pk.sha => {
                r.ok += 1;
                r.repaired += 1;
                r.bytes += bytes;
                r.here_files += members[i];
                r.here_file_bytes += bytes;
                r.here_repaired += members[i];
            }
            Ok(Some(_)) => r.changed.push(pk.path.clone()),
            Ok(None) => r.stopped = true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => r.missing.push(pk.path.clone()),
            Err(_) => r.changed.push(pk.path.clone()),
        }
    }
    amount(done, total);
    if !r.stopped {
        // Files the list does not name.
        let mut stack = vec![(base.clone(), String::new())];
        while let Some((dir, rel)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let here = if rel.is_empty() { name.clone() } else { format!("{rel}\\{name}") };
                match e.file_type() {
                    Ok(t) if t.is_dir() => {
                        if !listed.contains(&here.to_lowercase()) {
                            stack.push((e.path(), here));
                        }
                    }
                    Ok(_) => {
                        if rel.is_empty() && (name.starts_with("rescue-log") || name == crate::repair::RETURNS_NAME) {
                            continue;
                        }
                        if !listed.contains(&here.to_lowercase()) {
                            r.extra.push(here);
                        }
                    }
                    Err(_) => {}
                }
            }
        }
        r.extra.sort();
    }
    if let Some(out) = out_csv {
        if !r.changed.is_empty() || !r.missing.is_empty() || !r.extra.is_empty() {
            let mut body = csv_row(&["problem", "path"]);
            for p in &r.changed {
                body.push_str(&csv_row(&["CHANGED: not what was copied", p.as_str()]));
            }
            for p in &r.missing {
                body.push_str(&csv_row(&["MISSING", p.as_str()]));
            }
            for p in &r.extra {
                body.push_str(&csv_row(&["NOT IN THE LIST (added since)", p.as_str()]));
            }
            std::fs::write(out, format!("\u{FEFF}{body}")).map_err(|e| format!("could not write {}: {e}", out.display()))?;
            r.written = Some(out.display().to_string());
        }
    }
    Ok(r)
}

/// The report in plain words.
pub fn summary(r: &Report) -> Vec<String> {
    let mut t = Vec::new();
    let bad = r.changed.len() + r.missing.len();
    if r.stopped {
        t.push(format!("Stopped before the end: {} of {} copies checked.", commas(r.ok + bad as u64), commas(r.listed)));
    } else if bad == 0 {
        t.push(format!(
            "All {} copies are exactly what was copied off the disk ({}){}.",
            commas(r.ok),
            size(r.bytes),
            if r.repaired > 0 {
                format!(
                    " - or, for {} of them, exactly the repaired version put back in its place (step 8)",
                    commas(r.repaired)
                )
            } else {
                String::new()
            }
        ));
    } else {
        t.push(format!(
            "{} of {} copies are NOT what was copied off the disk: {} changed, {} missing.",
            commas(bad as u64),
            commas(r.listed),
            commas(r.changed.len() as u64),
            commas(r.missing.len() as u64)
        ));
        for p in r.changed.iter().take(10) {
            t.push(format!("  changed: {p}"));
        }
        for p in r.missing.iter().take(10) {
            t.push(format!("  missing: {p}"));
        }
    }
    if !r.extra.is_empty() {
        t.push(format!(
            "{} files in the folder are not in the list (added after the copying - by you, or by another program).",
            commas(r.extra.len() as u64)
        ));
        for p in r.extra.iter().take(10) {
            t.push(format!("  added: {p}"));
        }
    }
    if r.no_checksum > 0 {
        t.push(format!("{} lines of the list have no checksum to compare with.", commas(r.no_checksum)));
    }
    if let Some(w) = &r.written {
        t.push(format!("The full list of problems is in {w}"));
    }
    t
}

/// One stage of the safety check.
#[derive(Debug, Clone)]
pub struct Stage {
    pub name: &'static str,
    pub files: Option<u64>,
    pub bytes: Option<u64>,
    /// More about it, in a few words.
    pub words: String,
    /// As it should be (None: not known).
    pub ok: Option<bool>,
}

/// The safety check: from the SSD to the copies here now, stage by stage.
#[derive(Debug, Clone)]
pub struct Safety {
    pub stages: Vec<Stage>,
    /// Nothing missing and nothing changed, at every stage known.
    pub fine: bool,
    /// What it comes to, and what to do about anything wrong.
    pub verdict: Vec<String>,
}

const SSD: &str = "On the SSD (step 5)";
const COPIED: &str = "Copied (step 6)";
const HERE: &str = "Here now";

fn ssd_stage(on: Option<&OnSsd>) -> Stage {
    match on {
        Some(s) => Stage {
            name: SSD,
            files: Some(s.files),
            bytes: s.bytes,
            words: format!("in {} folders (step 5's results in {})", commas(s.folders), s.from),
            ok: Some(true),
        },
        None => Stage {
            name: SSD,
            files: None,
            bytes: None,
            words: "not known: step 5's results were not found".to_string(),
            ok: None,
        },
    }
}

fn list_stage(l: &ListTotals, on: Option<&OnSsd>, problems: &mut Vec<String>) -> Stage {
    let mut words = Vec::new();
    let mut ok = true;
    if l.pieces > 0 {
        words.push(format!("and {} of data no folder names ({})", crate::rescue::pieces(l.pieces), size(l.piece_bytes)));
    }
    if l.failed > 0 {
        ok = false;
        words.push(format!("{} could not be copied", commas(l.failed)));
    }
    if let Some(s) = on {
        let named = l.files + l.failed;
        if named < s.files {
            ok = false;
            words.push(format!("{} fewer than on the SSD", commas(s.files - named)));
            problems.push(format!(
                "The list of copies names {} fewer files than step 5 found on the SSD. Start step 6 again, into the same \
                 folder: it copies only what is missing. If the numbers still differ, ask before going on.",
                commas(s.files - named)
            ));
        } else if named > s.files {
            ok = false;
            words.push(format!("{} more than on the SSD", commas(named - s.files)));
            problems.push(format!(
                "The list of copies names {} more files than step 5 found on the SSD: was step 5 run on another disk, \
                 or were the copies made into a folder that was not empty? Ask before going on.",
                commas(named - s.files)
            ));
        } else if let Some(b) = s.bytes.filter(|&b| l.failed == 0 && b != l.file_bytes) {
            ok = false;
            words.push(format!("{} different in size", size(b.abs_diff(l.file_bytes))));
            problems.push(format!(
                "The files in the list of copies take {} {} than the files step 5 found on the SSD. Ask before going on.",
                size(b.abs_diff(l.file_bytes)),
                if l.file_bytes > b { "more" } else { "less" }
            ));
        }
    }
    if l.unfinished {
        ok = false;
        words.push("the copying did not get to the end".to_string());
        problems.push(
            "The copying did not get to the end. Start step 6 again, into the same folder: it carries on where it \
             stopped, and copies only what is missing."
                .to_string(),
        );
    }
    if l.failed > 0 {
        problems.push(format!(
            "{} could not be copied at all: rescue-log.csv names {}, with why (\"failed\"). Ask before going on.",
            if l.failed == 1 { "1 file".to_string() } else { format!("{} files", commas(l.failed)) },
            if l.failed == 1 { "it" } else { "them" }
        ));
    }
    Stage { name: COPIED, files: Some(l.files), bytes: Some(l.file_bytes), words: words.join("; "), ok: Some(ok) }
}

fn here_stage(r: &Report, problems: &mut Vec<String>) -> Stage {
    let bad = r.changed.len() + r.missing.len();
    let mut words = Vec::new();
    if bad == 0 {
        words.push("every one exactly as copied".to_string());
    } else {
        words.push(format!(
            "{} changed, {} missing",
            commas(r.changed.len() as u64),
            commas(r.missing.len() as u64)
        ));
    }
    if r.here_repaired > 0 {
        let diff = r.here_file_bytes.abs_diff(r.list.file_bytes);
        words.push(format!(
            "{} of them as the repaired version put back (step 8){}",
            commas(r.here_repaired),
            if bad == 0 && diff > 0 && r.checked_files == r.list.files {
                format!(
                    ", which {} {} in all",
                    if r.here_file_bytes > r.list.file_bytes { "adds" } else { "takes away" },
                    size(diff)
                )
            } else {
                String::new()
            }
        ));
    }
    if r.here_pieces > 0 || r.checked_pieces > 0 {
        words.push(format!(
            "and {} of data no folder names ({})",
            crate::rescue::pieces(r.here_pieces),
            size(r.here_piece_bytes)
        ));
    }
    if r.list.files > r.checked_files {
        words.push(format!("{} with no checksum to check", commas(r.list.files - r.checked_files)));
    }
    if !r.extra.is_empty() {
        words.push(if r.extra.len() == 1 {
            "1 file added since".to_string()
        } else {
            format!("{} files added since", commas(r.extra.len() as u64))
        });
    }
    if bad > 0 {
        problems.push(format!(
            "{} of the copies {} not what was copied: {} changed, {} missing (they are named below). Something changed \
             or deleted {} after step 6: stop, and find out what before going on.",
            commas(bad as u64),
            if bad == 1 { "is" } else { "are" },
            commas(r.changed.len() as u64),
            commas(r.missing.len() as u64),
            if bad == 1 { "it" } else { "them" }
        ));
    }
    if !r.extra.is_empty() {
        problems.push(format!(
            "{} added to the folder after the copying (named below): by you, or by another program - a Mac adds ._ \
             and .DS_Store files wherever it can write. Nothing is missing because of {}, but ask before going on.",
            if r.extra.len() == 1 { "1 file was".to_string() } else { format!("{} files were", commas(r.extra.len() as u64)) },
            if r.extra.len() == 1 { "it" } else { "them" }
        ));
    }
    Stage {
        name: HERE,
        files: Some(r.here_files),
        bytes: Some(r.here_file_bytes),
        words: words.join("; "),
        ok: Some(bad == 0 && r.extra.is_empty()),
    }
}

fn verdict(problems: Vec<String>, on: Option<&OnSsd>, all_good: &str) -> (bool, Vec<String>) {
    if !problems.is_empty() {
        return (false, problems);
    }
    let mut v = vec![all_good.to_string()];
    if on.is_none() {
        v.push("(Step 5's results were not found, so the SSD itself was not compared: run step 5, then this again.)".to_string());
    }
    (true, v)
}

/// The safety check after checking the copies (step 7): the SSD, the list
/// of copies, and the copies here now.
pub fn safety(r: &Report) -> Safety {
    let mut problems = Vec::new();
    let on = r.on_ssd.as_ref();
    let stages = vec![ssd_stage(on), list_stage(&r.list, on, &mut problems), here_stage(r, &mut problems)];
    let all_good = match (on.is_some(), r.here_repaired > 0) {
        (true, false) => "Nothing is missing, and nothing has changed: every file step 5 found on the SSD is here, exactly as copied.",
        (true, true) => {
            "Nothing is missing, and nothing has changed: every file step 5 found on the SSD is here, exactly as copied or \
             as repaired and put back."
        }
        (false, _) => "Nothing is missing from the list of copies, and nothing has changed.",
    };
    let (fine, verdict) = verdict(problems, on, all_good);
    Safety { stages, fine, verdict }
}

/// The safety check after copying (step 6): the SSD and the list of copies.
pub fn copy_safety(l: &ListTotals, on: Option<&OnSsd>) -> Safety {
    let mut problems = Vec::new();
    let stages = vec![ssd_stage(on), list_stage(l, on, &mut problems)];
    let all_good = if on.is_some() {
        "Every file step 5 found on the SSD is in the list of copies, with the same size in all."
    } else {
        "The copying got to the end, and every file it found is in the list of copies."
    };
    let (fine, verdict) = verdict(problems, on, all_good);
    Safety { stages, fine, verdict }
}

impl Safety {
    /// In plain words, one line per stage.
    pub fn lines(&self) -> Vec<String> {
        let mut t = vec!["Safety check, stage by stage:".to_string()];
        for (i, s) in self.stages.iter().enumerate() {
            let numbers = match (s.files, s.bytes) {
                (Some(f), Some(b)) => format!("{} files, {}", commas(f), size(b)),
                (Some(f), None) => format!("{} files", commas(f)),
                _ => String::new(),
            };
            let mark = match s.ok {
                Some(true) => "OK",
                Some(false) => "NOT OK",
                None => "?",
            };
            let sep = if numbers.is_empty() || s.words.is_empty() { "" } else { " - " };
            t.push(format!("  {}. {}: {numbers}{sep}{}  [{mark}]", i + 1, s.name, s.words));
        }
        for v in &self.verdict {
            t.push(format!("  {v}"));
        }
        t
    }

    /// For the page.
    pub fn json(&self) -> String {
        let n = |v: Option<u64>| v.map(|x| x.to_string()).unwrap_or_else(|| "null".to_string());
        let stages: Vec<String> = self
            .stages
            .iter()
            .map(|s| {
                format!(
                    "{{\"name\":{},\"files\":{},\"bytes\":{},\"words\":{},\"ok\":{}}}",
                    json_str(s.name),
                    n(s.files),
                    n(s.bytes),
                    json_str(&s.words),
                    match s.ok {
                        Some(true) => "true",
                        Some(false) => "false",
                        None => "null",
                    }
                )
            })
            .collect();
        let verdict: Vec<String> = self.verdict.iter().map(|v| json_str(v)).collect();
        format!("{{\"fine\":{},\"stages\":[{}],\"verdict\":[{}]}}", self.fine, stages.join(","), verdict.join(","))
    }

    /// Adds this check to the list of every check made (`HISTORY_NAME` in
    /// the program's results folder): when, which step, which folder, and the
    /// numbers at every stage. Run after every phase, a ✗ after a ✓ says
    /// that what was done in between is where something went wrong.
    pub fn record(&self, results: &Path, when: &str, step: &str, folder: &str, r: Option<&Report>) -> Result<(), String> {
        use std::io::Write;
        let path = results.join(HISTORY_NAME);
        let n = |v: Option<u64>| v.map(|x| x.to_string()).unwrap_or_default();
        let at = |name: &str| self.stages.iter().find(|s| s.name == name).map(|s| (s.files, s.bytes)).unwrap_or((None, None));
        let (ssd, listed, here) = (at(SSD), at(COPIED), at(HERE));
        let count = |f: fn(&Report) -> usize| r.map(|r| f(r).to_string()).unwrap_or_default();
        let result = if self.fine {
            "OK".to_string()
        } else {
            format!("NOT OK: {}", self.verdict.first().map(String::as_str).unwrap_or(""))
        };
        let mut body = String::new();
        if !path.exists() {
            body.push('\u{FEFF}');
            body.push_str(&csv_row(&HISTORY_HEADER));
        }
        body.push_str(&csv_row(&[
            when,
            step,
            folder,
            &n(ssd.0),
            &n(ssd.1),
            &n(listed.0),
            &n(listed.1),
            &n(here.0),
            &n(here.1),
            &count(|r| r.changed.len()),
            &count(|r| r.missing.len()),
            &count(|r| r.extra.len()),
            &result,
        ]));
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(body.as_bytes()))
            .map_err(|e| format!("could not write {}: {e}", path.display()))
    }
}

/// The list of every safety check made, in the program's results folder.
pub const HISTORY_NAME: &str = "safety-checks.csv";
const HISTORY_HEADER: [&str; 13] = [
    "when",
    "step",
    "folder",
    "ssd_files",
    "ssd_bytes",
    "listed_files",
    "listed_bytes",
    "here_files",
    "here_bytes",
    "changed",
    "missing",
    "added",
    "result",
];

/// The last `last` checks of that list (oldest first), for the page: each
/// with the files and size at its last stage (here now, or for the copying,
/// the list of copies).
pub fn history_json(results: &Path, last: usize) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    if let Ok(f) = std::fs::File::open(results.join(HISTORY_NAME)) {
        let _ = crate::fmt::for_each_csv_row(std::io::BufReader::new(f), |row| {
            if row.len() >= HISTORY_HEADER.len() && row[0] != HISTORY_HEADER[0] {
                rows.push(row);
            }
        });
    }
    let num = |s: &str| if s.is_empty() { "null".to_string() } else { s.parse::<u64>().map(|v| v.to_string()).unwrap_or_else(|_| "null".to_string()) };
    let items: Vec<String> = rows[rows.len().saturating_sub(last)..]
        .iter()
        .map(|r| {
            let (files, bytes) = if r[7].is_empty() { (&r[5], &r[6]) } else { (&r[7], &r[8]) };
            format!(
                "{{\"when\":{},\"step\":{},\"folder\":{},\"files\":{},\"bytes\":{},\"ok\":{},\"result\":{}}}",
                json_str(&r[0]),
                json_str(&r[1]),
                json_str(&r[2]),
                num(files),
                num(bytes),
                r[12] == "OK",
                json_str(&r[12])
            )
        })
        .collect();
    format!(
        "{{\"count\":{},\"file\":{},\"items\":[{}]}}",
        rows.len(),
        json_str(&results.join(HISTORY_NAME).display().to_string()),
        items.join(",")
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    fn h(b: &[u8]) -> String {
        let mut s = crate::sha256::Sha256::new();
        s.update(b);
        s.finish_hex()
    }

    fn check(dir: &Path, map_check: Option<&Path>) -> super::Report {
        super::run_checked(dir, map_check, None, &mut |_| {}, &mut |_, _| {}, &|| false).unwrap()
    }

    #[test]
    fn a_changed_or_missing_copy_is_found() {
        let dir = std::env::temp_dir().join(format!("rp-verify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Photos")).unwrap();
        std::fs::write(dir.join("Photos").join("a.jpg"), b"hello").unwrap();
        std::fs::write(dir.join("Photos").join("b_.jpg"), b"world").unwrap();
        std::fs::write(dir.join("c.txt"), b"gone soon").unwrap();
        let mut log = String::from("\u{FEFF}kind,path,size_bytes,result,copied_as,note,sha256\r\n");
        log.push_str(&format!("FILE,Photos\\a.jpg,5,copied,,,{}\r\n", h(b"hello")));
        log.push_str(&format!("FILE,Photos\\b..jpg,5,copied,Photos\\b_.jpg,,{}\r\n", h(b"world")));
        log.push_str(&format!("FILE,c.txt,9,copied,,,{}\r\n", h(b"gone soon")));
        log.push_str("FOLDER,Photos,,copied,,,\r\n");
        std::fs::write(dir.join("rescue-log.csv"), log).unwrap();
        let r = super::run(&dir, None, &mut |_| {}, &mut |_, _| {}, &|| false).unwrap();
        assert_eq!((r.listed, r.ok), (3, 3), "{r:?}");
        assert!(r.changed.is_empty() && r.missing.is_empty() && r.extra.is_empty(), "{r:?}");
        std::fs::write(dir.join("Photos").join("a.jpg"), b"hellO").unwrap();
        std::fs::remove_file(dir.join("c.txt")).unwrap();
        std::fs::write(dir.join("new.txt"), b"x").unwrap();
        let r = super::run(&dir, None, &mut |_| {}, &mut |_, _| {}, &|| false).unwrap();
        assert_eq!(r.ok, 1);
        assert_eq!(r.changed, vec!["Photos\\a.jpg".to_string()]);
        assert_eq!(r.missing, vec!["c.txt".to_string()]);
        assert_eq!(r.extra, vec!["new.txt".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_map_check_totals_are_read() {
        let dir = std::env::temp_dir().join(format!("rp-verify-mc-{}", std::process::id())).join("map-check-20261019-1402");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("map-check.json"),
            "{\"tool\":\"x\",\n\"volume\":{\"offset\":1,\"label\":\"a}\\\"files\\\":9\",\"serial\":2,\"drive\":null,\
             \"folders\":3,\"files\":12,\"bytes\":3456},\n\"totals\":{\"files\":99}}",
        )
        .unwrap();
        let t = super::map_check_totals(&dir).unwrap();
        assert_eq!(
            t,
            super::OnSsd { files: 12, bytes: Some(3456), folders: 3, from: "map-check-20261019-1402".to_string() }
        );
        // One made before the sizes were added up.
        std::fs::write(dir.join("map-check.json"), "{\"volume\":{\"folders\":3,\"files\":12},\n\"totals\":{}}").unwrap();
        assert_eq!(super::map_check_totals(&dir).unwrap().bytes, None);
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }

    #[test]
    fn the_safety_check_says_where_something_went_missing() {
        let top = std::env::temp_dir().join(format!("rp-verify-safety-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&top);
        let dir = top.join("WORK");
        let mc = top.join("map-check-1");
        std::fs::create_dir_all(dir.join("Photos")).unwrap();
        std::fs::create_dir_all(dir.join("lost data (no name)")).unwrap();
        std::fs::create_dir_all(&mc).unwrap();
        std::fs::write(dir.join("Photos").join("a.jpg"), b"hello").unwrap();
        std::fs::write(dir.join("Photos").join("b.jpg"), b"world!").unwrap();
        std::fs::write(dir.join("lost data (no name)").join("piece 1"), b"zz").unwrap();
        let mut log = String::from("\u{FEFF}kind,path,size_bytes,result,copied_as,note,sha256\r\n");
        log.push_str(&format!("FILE,Photos\\a.jpg,5,copied,,,{}\r\n", h(b"hello")));
        log.push_str(&format!("FILE,Photos\\b.jpg,6,partly copied,,never read,{}\r\n", h(b"world!")));
        log.push_str(&format!("LOST DATA,lost data (no name)\\piece 1,2,copied,,,{}\r\n", h(b"zz")));
        std::fs::write(dir.join("rescue-log.csv"), &log).unwrap();
        let ssd = |files: u64, bytes: u64| {
            std::fs::write(
                mc.join("map-check.json"),
                format!("{{\"volume\":{{\"folders\":1,\"files\":{files},\"bytes\":{bytes}}},\n\"totals\":{{}}}}"),
            )
            .unwrap()
        };

        // All there: the same at every stage.
        ssd(2, 11);
        let r = check(&dir, Some(&mc));
        let s = super::safety(&r);
        assert!(s.fine, "{:?}", s.lines());
        assert_eq!(s.stages.iter().map(|g| (g.files, g.bytes)).collect::<Vec<_>>(), vec![(Some(2), Some(11)); 3]);
        assert!(s.verdict[0].starts_with("Nothing is missing"), "{:?}", s.verdict);
        assert!(s.json().starts_with("{\"fine\":true,\"stages\":[{\"name\":\"On the SSD (step 5)\",\"files\":2"), "{}", s.json());
        let c = super::copy_safety(&super::ListTotals::read(&dir).unwrap(), r.on_ssd.as_ref());
        assert!(c.fine && c.stages.len() == 2, "{:?}", c.lines());

        // Step 5 found more files than the copying listed.
        ssd(3, 20);
        let s = super::safety(&check(&dir, Some(&mc)));
        assert!(!s.fine && s.stages[1].ok == Some(false) && s.stages[2].ok == Some(true), "{:?}", s.lines());
        assert!(s.verdict[0].contains("1 fewer files than step 5 found"), "{:?}", s.verdict);
        ssd(2, 11);

        // Deleted after the copying: stage 3 says so, and names it.
        std::fs::remove_file(dir.join("Photos").join("b.jpg")).unwrap();
        let r = check(&dir, Some(&mc));
        let s = super::safety(&r);
        assert!(!s.fine && s.stages[1].ok == Some(true) && s.stages[2].ok == Some(false), "{:?}", s.lines());
        assert_eq!((s.stages[2].files, s.stages[2].bytes), (Some(1), Some(5)));
        assert!(s.verdict[0].contains("1 missing") && s.verdict[0].contains("after step 6"), "{:?}", s.verdict);
        assert_eq!(r.missing, vec!["Photos\\b.jpg".to_string()]);
        std::fs::write(dir.join("Photos").join("b.jpg"), b"world!").unwrap();

        // A file added since (a Mac's ._ file, say).
        std::fs::write(dir.join("Photos").join("._a.jpg"), b"mac").unwrap();
        let s = super::safety(&check(&dir, Some(&mc)));
        assert!(!s.fine && s.stages[2].words.contains("1 file added since"), "{:?}", s.lines());
        std::fs::remove_file(dir.join("Photos").join("._a.jpg")).unwrap();

        // A copying that stopped before the end, and one a power cut stopped.
        std::fs::write(dir.join("rescue-log.csv"), format!("{log}NOTE,,,,,stopped on request before the end; run it again to copy the rest,\r\n")).unwrap();
        let s = super::safety(&check(&dir, Some(&mc)));
        assert!(!s.fine && s.verdict[0].contains("did not get to the end"), "{:?}", s.lines());
        std::fs::write(dir.join("rescue-log.csv"), &log).unwrap();
        std::fs::write(dir.join("rescue-log-unfinished.csv"), "").unwrap();
        assert!(super::ListTotals::read(&dir).unwrap().unfinished);
        std::fs::remove_file(dir.join("rescue-log-unfinished.csv")).unwrap();

        // No map check: the rest is still checked, and it says what it could not compare.
        let s = super::safety(&check(&dir, None));
        assert!(s.fine && s.stages[0].ok.is_none() && s.verdict.len() == 2, "{:?}", s.lines());
        std::fs::remove_dir_all(&top).ok();
    }

    #[test]
    fn every_check_is_recorded_in_order() {
        let top = std::env::temp_dir().join(format!("rp-verify-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&top);
        let dir = top.join("WORK");
        let results = top.join("results");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&results).unwrap();
        std::fs::write(dir.join("a.jpg"), b"hello").unwrap();
        let log = format!("\u{FEFF}kind,path,size_bytes,result,copied_as,note,sha256\r\nFILE,a.jpg,5,copied,,,{}\r\n", h(b"hello"));
        std::fs::write(dir.join("rescue-log.csv"), &log).unwrap();
        let file = crate::fmt::json_str(&results.join(super::HISTORY_NAME).display().to_string());
        assert_eq!(super::history_json(&results, 10), format!("{{\"count\":0,\"file\":{file},\"items\":[]}}"));

        // After the copying (step 6), and a check (step 7) that finds everything.
        let c = super::copy_safety(&super::ListTotals::read(&dir).unwrap(), None);
        c.record(&results, "2026-10-19 10:00:00", "Copied (step 6)", "E:\\WORK", None).unwrap();
        let r = check(&dir, None);
        super::safety(&r).record(&results, "2026-10-20 10:00:00", "Checked (step 7)", "E:\\WORK", Some(&r)).unwrap();
        // A copy deleted by mistake, then checked again.
        std::fs::remove_file(dir.join("a.jpg")).unwrap();
        let r = check(&dir, None);
        super::safety(&r).record(&results, "2026-10-21 10:00:00", "Checked (step 7)", "E:\\WORK", Some(&r)).unwrap();

        let csv = std::fs::read_to_string(results.join(super::HISTORY_NAME)).unwrap();
        assert!(csv.starts_with("\u{FEFF}when,step,folder,"), "{csv}");
        assert_eq!(csv.matches("when,step").count(), 1, "one header line: {csv}");
        let rows = crate::fmt::parse_csv(&csv);
        assert_eq!(rows.len(), 4, "{rows:?}");
        assert_eq!((rows[1][5].as_str(), rows[1][7].as_str(), rows[1][12].as_str()), ("1", "", "OK"));
        assert_eq!((rows[2][7].as_str(), rows[2][8].as_str(), rows[2][12].as_str()), ("1", "5", "OK"));
        assert_eq!((rows[3][7].as_str(), rows[3][10].as_str()), ("0", "1"));
        assert!(rows[3][12].starts_with("NOT OK: 1 of the copies is not what was copied"), "{:?}", rows[3]);

        let j = super::history_json(&results, 2);
        assert!(j.starts_with("{\"count\":3,"), "{j}");
        assert!(
            j.contains("{\"when\":\"2026-10-20 10:00:00\",\"step\":\"Checked (step 7)\",\"folder\":\"E:\\\\WORK\",\"files\":1,\"bytes\":5,\"ok\":true"),
            "{j}"
        );
        assert!(j.contains("\"files\":0,\"bytes\":0,\"ok\":false,\"result\":\"NOT OK: 1 of the copies"), "{j}");
        assert!(!j.contains("2026-10-19 10:00:00"), "only the last two: {j}");
        std::fs::remove_dir_all(&top).ok();
    }
}
