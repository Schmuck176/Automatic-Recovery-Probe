//! Reports: the three CSV files, the library ranking and the text
//! summaries. Ports of the Python version's library_row,
//! format_library_survey, format_untestable and the CSV writers.

use crate::fmt::{commas, csv_row, fixed, local_date, local_datetime, size};
use crate::formats::{basename, is_raw_ext, is_system_file, is_undecodable_ext, splitext_ext, SYSTEM_KEY};
use crate::walk::{LibInfo, Row};
use std::io::Write;

/// An iPhone HEIC is normally 1.5-3 MB. A library whose typical original is
/// a few hundred KB is probably holding iCloud "Optimize Storage" proxies.
pub const PLACEHOLDER_MEDIAN: u64 = 500 * 1024;

pub fn is_placeholder(i: &LibInfo) -> bool {
    i.median_orig > 0 && i.median_orig < PLACEHOLDER_MEDIAN
}

/// Placeholder libraries last, then most originals, then biggest bundle.
/// A stable sort, like Python's sorted().
pub fn ranked(libs: &[LibInfo]) -> Vec<&LibInfo> {
    let mut v: Vec<&LibInfo> = libs.iter().collect();
    v.sort_by(|a, b| {
        (is_placeholder(a), std::cmp::Reverse(a.originals), std::cmp::Reverse(a.total_bytes)).cmp(&(
            is_placeholder(b),
            std::cmp::Reverse(b.originals),
            std::cmp::Reverse(b.total_bytes),
        ))
    });
    v
}

/// A library as an ordinary result row.
pub fn library_row(info: &LibInfo) -> Row {
    let db = if !info.db_verdict.is_empty() {
        info.db_verdict.clone()
    } else if info.db == "missing" {
        "no database".to_string()
    } else {
        "database ok".to_string()
    };
    let tag = if is_placeholder(info) { "  [likely iCloud placeholders]" } else { "" };
    let hit = if info.damaged > 0 { format!(", {} damaged inside", commas(info.damaged)) } else { String::new() };
    Row {
        verdict: "LIBRARY",
        detail: format!(
            "{} originals ({}), bundle {}{hit} - found by {} - {db}{tag}",
            commas(info.originals),
            size(info.orig_bytes),
            size(info.total_bytes),
            info.how
        ),
        size: info.total_bytes,
        path: info.path.clone(),
    }
}

/// How a not-testable file is grouped: by extension, except housekeeping files.
pub fn untestable_key(path: &str) -> String {
    let name = basename(path);
    if name.starts_with("._") || is_system_file(&name.to_lowercase()) {
        return SYSTEM_KEY.to_string();
    }
    splitext_ext(name).to_lowercase()
}

/// Counts by key, in the order first seen (like Python's Counter).
#[derive(Default, Clone)]
pub struct Tally {
    pub items: Vec<(String, u64)>,
}

impl Tally {
    pub fn add(&mut self, key: &str) {
        match self.items.iter_mut().find(|(k, _)| k == key) {
            Some((_, n)) => *n += 1,
            None => self.items.push((key.to_string(), 1)),
        }
    }
    /// Counter.most_common(n): count descending, ties in first-seen order.
    pub fn most_common(&self, n: usize) -> Vec<(String, u64)> {
        let mut v = self.items.clone();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v.truncate(n);
        v
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// The "not testable" pile, broken down by extension.
pub fn format_untestable(counter: &Tally, total: u64) -> Vec<String> {
    if counter.is_empty() {
        return Vec::new();
    }
    let mut out = vec![String::new(), format!("NOT TESTABLE: {} files, by extension", commas(total)), "-".repeat(46)];
    for (ext, n) in counter.most_common(12) {
        let note = if is_undecodable_ext(&ext) {
            "  <- needs the Mac (sips)"
        } else if is_raw_ext(&ext) {
            "  <- camera RAW, needs the Mac"
        } else if ext == SYSTEM_KEY {
            "  <- Mac/Windows housekeeping, not your files"
        } else {
            ""
        };
        let shown = if ext.is_empty() { "(no extension)".to_string() } else { ext.clone() };
        out.push(format!("  {shown:<16}{:>10}{note}", commas(n)));
    }
    if counter.items.len() > 12 {
        out.push(format!("  ...and {} other extensions", counter.items.len() - 12));
    }
    out.extend(
        [
            "",
            "These are not damaged - they are unverified. Everything above marked",
            "'needs the Mac' is what Phase 5 Job 1 has to decode.",
            "",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    out
}

/// Ranked, human-readable comparison of every library found.
pub fn format_library_survey(infos: &[LibInfo]) -> Vec<String> {
    if infos.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = vec![
        String::new(),
        "=".repeat(76),
        format!("APPLE PHOTOS LIBRARIES FOUND: {}", infos.len()),
        "=".repeat(76),
        String::new(),
        "Ranked by number of originals. MOVE EVERY ONE out of the scanning area".to_string(),
        "before you run any duplicate finder.".to_string(),
        String::new(),
    ];
    for (n, i) in ranked(infos).into_iter().enumerate() {
        let newest = if i.newest != 0.0 { local_date(i.newest) } else { "unknown".to_string() };
        out.push(format!("{}. {}", n + 1, i.path));
        out.push(format!("     found by     {}", i.how));
        out.push(format!("     originals    {:>9} files   {}", commas(i.originals), size(i.orig_bytes)));
        out.push(format!("     whole bundle {:>9} files   {}", commas(i.files), size(i.total_bytes)));
        out.push(format!(
            "     database     {}{}{}",
            i.db,
            if i.db_bytes > 0 { format!(", {}", size(i.db_bytes)) } else { String::new() },
            if i.hot_wal { "   [hot -wal present]" } else { "" }
        ));
        out.push(format!("     newest file  {newest}"));
        if i.damaged > 0 {
            let mut dmg = i.damage.clone();
            dmg.sort_by(|a, b| b.1.cmp(&a.1));
            let parts = dmg.iter().map(|(v, c)| format!("{} {}", commas(*c), v.to_lowercase())).collect::<Vec<_>>().join(", ");
            let pct = if i.files > 0 { 100.0 * i.damaged as f64 / i.files as f64 } else { 0.0 };
            out.push(format!("     damaged      {:>9} files   ({}% of the bundle)", commas(i.damaged), fixed(pct, 1)));
            out.push(format!("                  {parts}"));
        } else if i.files > 0 {
            out.push(format!("     damaged      {:>9} files   nothing flagged inside it", "0"));
        }
        if i.unverified > 0 {
            out.push(format!(
                "     unverified   {:>9} files   HEIC/RAW - this library's share of the Mac pass",
                commas(i.unverified)
            ));
        }
        if i.originals > 0 {
            out.push(format!("     typical photo{:>14}   (median of originals/)", size(i.median_orig)));
            if i.median_orig > 0 && i.median_orig < PLACEHOLDER_MEDIAN {
                out.push("     >> LIKELY iCLOUD PLACEHOLDERS. A typical original this small".to_string());
                out.push("        means 'Optimize Mac Storage' was on and the real photos are".to_string());
                out.push("        still in iCloud. Check icloud.com before spending effort on".to_string());
                out.push("        this one - you may not need to recover it at all.".to_string());
            }
        }
        if i.db_verdict == "DB_DAMAGED" {
            out.push("     >> DATABASE DAMAGED - the photos in originals/ may be perfectly".to_string());
            out.push("        fine, but the albums, dates and faces are at risk. Try Photos'".to_string());
            out.push("        own Repair first (hold Option+Command while launching it).".to_string());
            if !i.db_detail.is_empty() {
                out.push(format!("        {}", i.db_detail.chars().take(68).collect::<String>()));
            }
        } else if i.db == "missing" {
            out.push("     >> no database found - the photos may survive but the albums,".to_string());
            out.push("        dates and faces do not. Treat as a folder of loose files.".to_string());
        } else if i.db_verdict == "OK" {
            out.push("     >> database passed its integrity check".to_string());
        }
        out.push(String::new());
    }
    if infos.len() > 1 {
        for s in [
            "Comparing them:",
            "  - Most originals is usually the one to restore first - but check the dates.",
            "    A smaller library with a NEWER newest-file is often the one you were",
            "    actually still using, and the big one an old archive.",
            "  - Libraries overlap heavily. Do not assume the small ones are redundant",
            "    until you have opened the big one and found the photos in it.",
            "  - A damaged database does not mean lost photos. originals/ is separate.",
            "  - 'damaged' counts flagged files INSIDE that bundle, so you can see which",
            "    library actually took the hit rather than reading it off a million-row",
            "    CSV. It is reported, not ranked on: a library with the most photos is",
            "    still usually the one to restore, damage and all.",
            "  - 'unverified' is that library's share of the Phase 5 Mac pass.",
            "  - Libraries flagged as iCloud placeholders are listed last: they hold",
            "    proxies, not your photos, so they are rarely the one to restore.",
            "",
        ] {
            out.push(s.to_string());
        }
    }
    out
}

pub const RESULT_HEADER: [&str; 4] = ["verdict", "detail", "size_bytes", "path"];

fn write_with_bom(path: &str, body: &str) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    f.write_all("\u{FEFF}".as_bytes())?;
    f.write_all(body.as_bytes())?;
    f.flush()
}

/// The results file and the not-testable list share one layout.
pub fn write_results_csv(path: &str, rows: &[Row]) -> std::io::Result<()> {
    let mut body = csv_row(&RESULT_HEADER);
    for r in rows {
        body.push_str(&csv_row(&[r.verdict.to_string(), r.detail.clone(), r.size.to_string(), r.path.clone()]));
    }
    write_with_bom(path, &body)
}

fn stem(results_csv: &str) -> &str {
    // os.path.splitext(results_csv)[0]
    let ext = splitext_ext(results_csv);
    &results_csv[..results_csv.len() - ext.len()]
}

pub fn untestable_csv_path(results_csv: &str) -> String {
    format!("{}-not-testable.csv", stem(results_csv))
}

pub fn library_csv_path(results_csv: &str) -> String {
    format!("{}-photo-libraries.csv", stem(results_csv))
}

/// One row per Photos library, in the same ranked order as the summary.
pub fn write_library_csv(path: &str, libraries: &[LibInfo]) -> std::io::Result<()> {
    let mut body = csv_row(&[
        "rank",
        "path",
        "found_by",
        "originals",
        "originals_bytes",
        "total_files",
        "total_bytes",
        "damaged_inside",
        "damage_breakdown",
        "unverified_inside",
        "database",
        "database_bytes",
        "median_original_bytes",
        "likely_icloud_placeholders",
        "hot_wal",
        "database_verdict",
        "database_detail",
        "newest_file",
    ]);
    for (n, i) in ranked(libraries).into_iter().enumerate() {
        let mut breakdown = i.damage.clone();
        breakdown.sort_by(|a, b| a.0.cmp(b.0));
        let breakdown = breakdown.iter().map(|(v, c)| format!("{v}={c}")).collect::<Vec<_>>().join(" ");
        body.push_str(&csv_row(&[
            (n + 1).to_string(),
            i.path.clone(),
            i.how.to_string(),
            i.originals.to_string(),
            i.orig_bytes.to_string(),
            i.files.to_string(),
            i.total_bytes.to_string(),
            i.damaged.to_string(),
            breakdown,
            i.unverified.to_string(),
            i.db.to_string(),
            i.db_bytes.to_string(),
            i.median_orig.to_string(),
            if is_placeholder(i) { "yes" } else { "no" }.to_string(),
            if i.hot_wal { "yes" } else { "no" }.to_string(),
            i.db_verdict.clone(),
            i.db_detail.clone(),
            if i.newest != 0.0 { local_datetime(i.newest) } else { String::new() },
        ]));
    }
    write_with_bom(path, &body)
}
