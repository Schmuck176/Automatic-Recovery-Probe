//! Checking copies against the list the copying made (rescue-log.csv).
//!
//! Every file and piece of lost data the copying writes gets its SHA-256 in
//! rescue-log.csv, computed from the bytes as they came off the disk. Reading
//! a copy back and comparing proves it is still exactly that - after a power
//! cut, after a trip to another computer, or years later. Nothing is changed:
//! the copies are only read.

use crate::fmt::{commas, csv_row, size};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

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
    pub stopped: bool,
    pub written: Option<String>,
}

struct Want {
    rel: String,
    size: u64,
    sha: String,
}

fn components(rel: &str) -> impl Iterator<Item = &str> {
    rel.split('\\').filter(|c| !c.is_empty())
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
    let log = folder.join(crate::rescue::LOG_NAME);
    let f = std::fs::File::open(&log).map_err(|e| {
        format!("{} has no {} (the list the copying makes): {e}", folder.display(), crate::rescue::LOG_NAME)
    })?;
    let mut wants: Vec<Want> = Vec::new();
    let mut r = Report::default();
    crate::fmt::for_each_csv_row(std::io::BufReader::new(f), |row| {
        if row.len() < 6 || row[0] == "kind" || !matches!(row[0].as_str(), "FILE" | "LOST DATA") {
            return;
        }
        if !matches!(row[3].as_str(), "copied" | "partly copied" | "skipped") {
            return;
        }
        let sha = row.get(6).map(|s| s.trim().to_lowercase()).unwrap_or_default();
        if sha.len() != 64 {
            r.no_checksum += 1;
            return;
        }
        let rel = if row[4].is_empty() { row[1].clone() } else { row[4].clone() };
        wants.push(Want { rel, size: row[2].parse().unwrap_or(0), sha });
    })
    .map_err(|e| format!("could not read {}: {e}", log.display()))?;
    r.listed = wants.len() as u64;
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
                    Ok(t) if t.is_dir() => stack.push((e.path(), here)),
                    Ok(_) => {
                        if rel.is_empty() && name.starts_with("rescue-log") {
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
            "All {} copies are exactly what was copied off the disk ({}).",
            commas(r.ok),
            size(r.bytes)
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
    }
    if r.no_checksum > 0 {
        t.push(format!("{} lines of the list have no checksum to compare with.", commas(r.no_checksum)));
    }
    if let Some(w) = &r.written {
        t.push(format!("The full list of problems is in {w}"));
    }
    t
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_changed_or_missing_copy_is_found() {
        let dir = std::env::temp_dir().join(format!("rp-verify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Photos")).unwrap();
        std::fs::write(dir.join("Photos").join("a.jpg"), b"hello").unwrap();
        std::fs::write(dir.join("Photos").join("b_.jpg"), b"world").unwrap();
        std::fs::write(dir.join("c.txt"), b"gone soon").unwrap();
        let h = |b: &[u8]| {
            let mut s = crate::sha256::Sha256::new();
            s.update(b);
            s.finish_hex()
        };
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
}
