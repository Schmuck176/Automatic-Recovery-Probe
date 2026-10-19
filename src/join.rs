//! Joining a scan with the map check.
//!
//! The scan looks at the copies (on SPARE); the map check knows, from the
//! ddrescue mapfile and the recovered disk itself, exactly which files had
//! bytes that were never read. A file the map check names has lost data even
//! when its structure looks fine to the scan (a few zeros in the middle of a
//! photo, a HEIC or RAW the scan cannot test), so the scan reports it as
//! NEVER_READ. And a file the scan calls damaged that the map check does not
//! name was not damaged by the failing drive: the note says so.
//!
//! The map check writes map-check-files.tsv for this: one line per damaged
//! file, file Windows cannot list, and damaged folder, with paths from the
//! top of the recovered volume. The copies are found by name and size, and
//! the one whose folders match the most is taken.

use crate::fmt::{commas, csv_row};
use crate::walk::Row;
use std::collections::{HashMap, HashSet};

pub const VERDICT: &str = "NEVER_READ";
pub const TSV_NAME: &str = "map-check-files.tsv";

/// One line of map-check-files.tsv.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapItem {
    /// DAMAGED, HIDDEN or FOLDER.
    pub kind: String,
    /// From the top of the recovered volume, folders separated by '\'.
    pub path: String,
    pub size: u64,
    pub damaged_bytes: u64,
    pub first_damaged: u64,
    /// Every damaged byte is past the file's real end: nothing lost.
    pub only_past_end: bool,
    /// What the unread places hold on the disk now: zeros, data or mixed.
    pub content: String,
    /// HIDDEN: Windows still opens it by its full path.
    pub maybe: bool,
    pub ranges: String,
}

impl MapItem {
    pub fn loses_data(&self) -> bool {
        self.kind == "DAMAGED" && self.damaged_bytes > 0 && !self.only_past_end
    }
    /// The name its copy has (made safe for Windows, see rescue.rs), in any case.
    fn name_key(&self) -> String {
        copy_name(self.path.rsplit('\\').next().unwrap_or(""))
    }
}

/// A name as the rescue gives it to a copy, in lower case: names Windows
/// cannot use get an underscore (rescue::safe_name).
fn copy_name(name: &str) -> String {
    crate::rescue::safe_name(name).to_lowercase()
}

/// "img (2).jpg" -> "img.jpg", "folder (2)" -> "folder": the name a copy
/// had before the rescue numbered it to keep it apart from another one
/// Windows would take for the same name.
fn without_number(name: &str) -> Option<String> {
    let cut = |s: &str| -> Option<String> {
        let s = s.strip_suffix(')')?;
        let open = s.rfind(" (")?;
        let num = &s[open + 2..];
        (!num.is_empty() && num.bytes().all(|b| b.is_ascii_digit())).then(|| s[..open].to_string())
    };
    if let Some(i) = name.rfind('.').filter(|&i| i > 0) {
        if let Some(stem) = cut(&name[..i]) {
            return Some(format!("{stem}{}", &name[i..]));
        }
    }
    cut(name)
}

pub const TSV_HEADER: &str =
    "kind\tpath\tsize_bytes\tdamaged_bytes\tfirst_damaged_byte\tonly_past_end\tcontent_now\topens_by_full_path\tdamaged_ranges";

/// One line of the file (paths cannot hold tabs: exFAT forbids them).
pub fn tsv_line(it: &MapItem) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\r\n",
        it.kind,
        it.path,
        it.size,
        it.damaged_bytes,
        it.first_damaged,
        if it.only_past_end { 1 } else { 0 },
        it.content,
        if it.maybe { 1 } else { 0 },
        it.ranges
    )
}

pub fn parse_tsv(text: &str) -> Vec<MapItem> {
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.trim_end_matches('\r').split('\t').collect();
        if f.len() < 9 {
            continue;
        }
        out.push(MapItem {
            kind: f[0].to_string(),
            path: f[1].to_string(),
            size: f[2].parse().unwrap_or(0),
            damaged_bytes: f[3].parse().unwrap_or(0),
            first_damaged: f[4].parse().unwrap_or(0),
            only_past_end: f[5] == "1",
            content: f[6].to_string(),
            maybe: f[7] == "1",
            ranges: f[8].to_string(),
        });
    }
    out
}

/// A file the scan saw that might be a copy of a map-check file.
#[derive(Clone, Debug)]
pub struct Seen {
    pub path: String,
    pub size: u64,
    pub verdict: &'static str,
    pub detail: String,
}

/// Collects, during a scan, the files whose names the map check mentions.
pub struct Collector {
    names: HashSet<String>,
    pub seen: HashMap<String, Vec<Seen>>,
}

impl Collector {
    pub fn new(items: &[MapItem]) -> Collector {
        let names = items.iter().filter(|i| i.kind != "FOLDER").map(|i| i.name_key()).collect();
        Collector { names, seen: HashMap::new() }
    }
    /// Call for every file the scan checks.
    pub fn saw(&mut self, r: &Row) {
        // Both separators: the copies may have been made on either system.
        let mut name = r.path.rsplit(['\\', '/']).next().unwrap_or("").to_lowercase();
        if !self.names.contains(&name) {
            match without_number(&name) {
                Some(n) if self.names.contains(&n) => name = n,
                _ => return,
            }
        }
        {
            self.seen.entry(name).or_default().push(Seen {
                path: r.path.clone(),
                size: r.size,
                verdict: r.verdict,
                detail: r.detail.clone(),
            });
        }
    }
}

fn components(p: &str) -> Vec<String> {
    p.split(['\\', '/']).filter(|c| !c.is_empty()).map(|c| c.to_lowercase()).collect()
}

/// How many trailing folders and name a path on the recovered disk (`a`)
/// and a copy's path (`b`) share, allowing for names the rescue changed.
fn common_tail(a: &[String], b: &[String]) -> usize {
    a.iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| {
            let x = copy_name(x);
            x == **y || without_number(y).is_some_and(|n| n == x)
        })
        .count()
}

/// What the join found.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Copies of files that lost data: (map item, copy, the scan's own verdict).
    pub matched: Vec<(MapItem, Seen)>,
    /// Copies found only by name and size (more than one candidate).
    pub ambiguous: Vec<(MapItem, Vec<Seen>)>,
    /// Files that lost data with no copy in the scanned folder.
    pub missing: Vec<MapItem>,
    /// Files Windows cannot list: with and without a copy here.
    pub hidden_found: usize,
    pub hidden_missing: Vec<MapItem>,
}

/// Find the copies.
pub fn find(items: &[MapItem], c: &Collector) -> Outcome {
    let mut o = Outcome::default();
    for it in items {
        if it.kind == "FOLDER" || (it.kind == "DAMAGED" && !it.loses_data()) {
            continue;
        }
        let want = components(&it.path);
        let same: Vec<&Seen> =
            c.seen.get(&it.name_key()).map(|v| v.iter().filter(|s| s.size == it.size).collect()).unwrap_or_default();
        let best = same.iter().map(|s| common_tail(&components(&s.path), &want)).max().unwrap_or(0);
        let picks: Vec<Seen> =
            same.iter().filter(|s| common_tail(&components(&s.path), &want) == best).map(|s| (*s).clone()).collect();
        match (it.kind.as_str(), picks.len()) {
            ("HIDDEN", 0) => o.hidden_missing.push(it.clone()),
            ("HIDDEN", _) => o.hidden_found += 1,
            (_, 0) => o.missing.push(it.clone()),
            (_, 1) => o.matched.push((it.clone(), picks[0].clone())),
            _ => o.ambiguous.push((it.clone(), picks)),
        }
    }
    o
}

/// The verdict a copy of a file that lost data gets.
pub fn never_read_row(it: &MapItem, s: &Seen, ambiguous: bool) -> Row {
    let pct = if it.size > 0 { 100.0 * it.first_damaged as f64 / it.size as f64 } else { 0.0 };
    let what = match it.content.as_str() {
        "zeros" => "the copy holds zeros there",
        "data" => "the copy holds whatever the SSD held there before, not the file's data",
        _ => "the copy holds zeros or leftover data there",
    };
    let mut detail = format!(
        "ddrescue never read {} bytes of this file (from byte {}, {}% in); {what}. The scan alone said: {}{}",
        commas(it.damaged_bytes),
        commas(it.first_damaged),
        crate::fmt::fixed(pct, 0),
        s.verdict,
        if s.detail.is_empty() { String::new() } else { format!(" ({})", s.detail) }
    );
    if ambiguous {
        detail.push_str(". Found by name and size only: more than one copy matches, so each is marked");
    }
    Row { verdict: VERDICT, detail, size: s.size, path: s.path.clone() }
}

/// Apply the join to a finished scan: flagged rows and the not-testable
/// list. Returns the lines for the summary.
pub fn apply(o: &Outcome, rows: &mut Vec<Row>, skipped: &mut Vec<Row>) -> Vec<String> {
    let mut marked: HashMap<String, Row> = HashMap::new();
    for (it, s) in &o.matched {
        marked.insert(s.path.clone(), never_read_row(it, s, false));
    }
    for (it, all) in &o.ambiguous {
        for s in all {
            marked.insert(s.path.clone(), never_read_row(it, s, true));
        }
    }
    // A file the map check names replaces whatever the scan said about it.
    skipped.retain(|r| !marked.contains_key(&r.path));
    let mut done: HashSet<String> = HashSet::new();
    for r in rows.iter_mut() {
        if let Some(m) = marked.get(&r.path) {
            *r = m.clone();
            done.insert(r.path.clone());
        }
    }
    let mut extra: Vec<Row> = marked.into_iter().filter(|(p, _)| !done.contains(p)).map(|(_, r)| r).collect();
    extra.sort_by(|a, b| a.path.cmp(&b.path));
    rows.extend(extra);
    // Everything else the scan flagged was not on an unread place.
    let named: HashSet<&str> = o
        .matched
        .iter()
        .map(|(_, s)| s.path.as_str())
        .chain(o.ambiguous.iter().flat_map(|(_, v)| v.iter().map(|s| s.path.as_str())))
        .collect();
    let mut not_on_unread = 0u64;
    for r in rows.iter_mut() {
        if crate::formats::is_damage(r.verdict) && !named.contains(r.path.as_str()) {
            r.detail.push_str(" [map check: this file was not on any place ddrescue failed to read]");
            not_on_unread += 1;
        }
    }
    let lost = o.matched.len() + o.ambiguous.len() + o.missing.len();
    let mut t = vec![format!(
        "Map check: {} files lost data where ddrescue could not read. {} of them are in this folder and are marked {VERDICT}{}.",
        commas(lost as u64),
        commas((o.matched.len() + o.ambiguous.len()) as u64),
        if o.ambiguous.is_empty() {
            String::new()
        } else {
            format!(" ({} found by name and size only)", commas(o.ambiguous.len() as u64))
        }
    )];
    if !o.missing.is_empty() {
        t.push(format!(
            "{} of them have no copy in this folder (listed in ...-map-check-not-here.csv): they may be in another \
             folder you check separately (WORK or PHOTOSLIB), or were never copied - step 6 copies everything.",
            commas(o.missing.len() as u64)
        ));
    }
    if not_on_unread > 0 {
        t.push(format!(
            "{} files the scan calls damaged were not on any unread place: they were like that before the drive failed, \
             or the scan is being cautious about them.",
            commas(not_on_unread)
        ));
    }
    let hidden_total = o.hidden_found + o.hidden_missing.len();
    if hidden_total > 0 {
        t.push(format!(
            "Of the {} files Windows cannot list, {} have a copy here{}.",
            commas(hidden_total as u64),
            commas(o.hidden_found as u64),
            if o.hidden_missing.is_empty() {
                String::new()
            } else {
                format!(
                    "; {} do not (they may be in another folder you check separately, or were never copied - step 6 \
                     copies them)",
                    commas(o.hidden_missing.len() as u64)
                )
            }
        ));
    }
    t
}

/// The files with no copy here, for map-check-not-here.csv.
pub fn missing_csv(o: &Outcome) -> String {
    let mut body = csv_row(&["kind", "path_on_recovered_disk", "size_bytes", "damaged_bytes", "note"]);
    for it in &o.missing {
        body.push_str(&csv_row(&[
            "LOST DATA".to_string(),
            it.path.clone(),
            it.size.to_string(),
            it.damaged_bytes.to_string(),
            "no copy with this name and size in the scanned folder".to_string(),
        ]));
    }
    for it in &o.hidden_missing {
        body.push_str(&csv_row(&[
            "WINDOWS CANNOT LIST".to_string(),
            it.path.clone(),
            it.size.to_string(),
            String::new(),
            if it.maybe {
                "Windows cannot list it (typing its full path still opens it)"
            } else {
                "Windows cannot list it"
            }
            .to_string(),
        ]));
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, path: &str, size: u64, dmg: u64) -> MapItem {
        MapItem {
            kind: kind.to_string(),
            path: path.to_string(),
            size,
            damaged_bytes: dmg,
            first_damaged: 4096,
            content: "zeros".to_string(),
            ..Default::default()
        }
    }

    fn row(verdict: &'static str, path: &str, size: u64) -> Row {
        Row { verdict, detail: String::new(), size, path: path.to_string() }
    }

    #[test]
    fn tsv_round_trip() {
        let mut it = item("DAMAGED", r"Photos\2019\IMG_0001.JPG", 300_000, 4096);
        it.ranges = "4096-8192".to_string();
        let text = format!("{TSV_HEADER}\r\n{}", tsv_line(&it));
        assert_eq!(parse_tsv(&text), vec![it]);
    }

    #[test]
    fn copies_are_found_by_name_size_and_folders() {
        let items = vec![
            item("DAMAGED", r"Photos\2019\IMG_0001.JPG", 300_000, 4096),
            item("DAMAGED", r"Photos\2020\IMG_0001.JPG", 200_000, 512),
            item("DAMAGED", r"Docs\gone.pdf", 9000, 512),
            item("HIDDEN", r"Big\after.bin", 100, 0),
            item("DAMAGED", r"Videos\partly.mov", 300_000, 0),
        ];
        let mut c = Collector::new(&items);
        let scanned = vec![
            row("OK", r"E:\WORK\Photos\2019\IMG_0001.JPG", 300_000),
            row("HOLE", r"E:\WORK\Photos\2020\IMG_0001.JPG", 200_000),
            row("OK", r"E:\WORK\Other\IMG_0001.JPG", 300_000),
            row("TRUNCATED", r"E:\WORK\Docs\other.pdf", 10),
        ];
        for r in &scanned {
            c.saw(r);
        }
        let o = find(&items, &c);
        assert_eq!(o.matched.len(), 2, "{o:?}");
        assert!(o.matched.iter().any(|(_, s)| s.path == r"E:\WORK\Photos\2019\IMG_0001.JPG"));
        assert_eq!(o.missing.len(), 1);
        assert_eq!(o.missing[0].path, r"Docs\gone.pdf");
        assert_eq!(o.hidden_missing.len(), 1);
        let mut rows: Vec<Row> = scanned.iter().filter(|r| r.verdict != "OK").cloned().collect();
        let mut skipped = Vec::new();
        let lines = apply(&o, &mut rows, &mut skipped);
        let v: HashMap<&str, &str> = rows.iter().map(|r| (r.path.as_str(), r.verdict)).collect();
        assert_eq!(v[r"E:\WORK\Photos\2019\IMG_0001.JPG"], VERDICT);
        assert_eq!(v[r"E:\WORK\Photos\2020\IMG_0001.JPG"], VERDICT);
        assert!(rows.iter().any(|r| r.path.ends_with("other.pdf") && r.detail.contains("not on any place")));
        assert!(lines[0].contains("3 files lost data"), "{lines:?}");
    }

    #[test]
    fn copies_whose_names_were_changed_are_found_too() {
        let items = vec![
            item("DAMAGED", r"Photos\Summer 2019 \IMG_0001.JPG", 300_000, 4096),
            item("DAMAGED", r"Docs\notes.", 900, 512),
            item("DAMAGED", r"Docs\con.txt", 800, 512),
            item("DAMAGED", r"Docs\Same.txt", 700, 512),
            item("DAMAGED", r"Lib\Photos Library.photoslibrary\originals\A\IMG_9.HEIC", 5000, 512),
        ];
        let mut c = Collector::new(&items);
        let scanned = vec![
            row("OK", r"E:\WORK\Photos\Summer 2019_\IMG_0001.JPG", 300_000),
            row("OK", r"E:\WORK\Docs\notes_", 900),
            row("OK", r"E:\WORK\Docs\_con.txt", 800),
            row("OK", r"E:\WORK\Docs\Same (2).txt", 700),
            row("SKIPPED", r"E:\PHOTOSLIB\1\Photos Library.photoslibrary\originals\A\IMG_9.HEIC", 5000),
        ];
        for r in &scanned {
            c.saw(r);
        }
        let o = find(&items, &c);
        assert_eq!(o.matched.len(), 5, "{o:?}");
        assert!(o.missing.is_empty());
        assert_eq!(without_number("img (2).jpg").as_deref(), Some("img.jpg"));
        assert_eq!(without_number("folder (12)").as_deref(), Some("folder"));
        assert_eq!(without_number("a (b).jpg"), None);
        assert_eq!(without_number("plain.jpg"), None);
        let t = common_tail(&components(r"Photos\Summer 2019 \IMG_0001.JPG"), &components(r"E:\WORK\Photos\Summer 2019_\IMG_0001.JPG"));
        assert_eq!(t, 3);
    }
}
