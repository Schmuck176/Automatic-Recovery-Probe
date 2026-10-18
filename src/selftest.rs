//! The self-test anyone can run on their own PC (step 1 of the page, or
//! `recovery-probe self-test`): the program checks itself on files and a
//! disk it carries inside it, where every right answer is known, so a
//! problem with this PC (or this copy of the program) shows up before any
//! real file is looked at.
//!
//!  * file checks: sample photos, music, video, documents and a Photos
//!    database, intact and damaged the ways a ddrescue copy is (cut short,
//!    a hole of zeros, the start never read, the wrong name) - the intact
//!    ones must pass, the damaged ones must be caught;
//!  * the map check, on a small damaged copy of a disk like the real one
//!    (made by tools/make_selftest_disk.py, with the answers worked out
//!    independently by tools/test_mapcheck.py);
//!  * the rescue, from that disk: every file, byte for byte;
//!  * checking the rescued files together with the map check's results.
//!
//! Everything happens in a temporary folder, removed afterwards.

use crate::fmt::commas;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

const DISK: &[u8] = include_bytes!("selftest/disk.bin");
const MAP: &str = include_str!("selftest/disk.map");
const EXPECTED: &str = include_str!("selftest/expected.tsv");

macro_rules! seed {
    ($name:literal, $cut:expr, $hole:expr) => {
        ($name, include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/seeds/", $name)) as &[u8], $cut, $hole)
    };
}

/// (name, contents, a copy cut short is caught, a 4 KB hole is caught).
/// Not every kind of damage can be seen in every format: a cut-short MP3,
/// FLAC or Ogg file still plays to where it stops, a hole in a JPEG can be
/// a dark patch, and silence is zeros anyway. The map check covers those.
const SEEDS: &[(&str, &[u8], bool, bool)] = &[
    seed!("black-sky.jpg", true, false),
    seed!("night.jpg", true, false),
    seed!("photo.png", true, true),
    seed!("photo.gif", true, true),
    seed!("doc.pdf", true, false),
    seed!("report.docx", true, true),
    seed!("clip.webm", true, true),
    seed!("song.m4a", true, true),
    seed!("song.flac", false, true),
    seed!("song.mp3", false, true),
    seed!("song.ogg", false, true),
    seed!("silence.wav", true, false),
    seed!("Photos.sqlite", true, true),
];

/// The built-in disk, unpacked: "RPSPARSE1", size, then runs of sectors.
pub fn disk_image() -> Result<Vec<u8>, String> {
    let bad = || "the built-in test disk is damaged (this copy of the program is broken)".to_string();
    if !DISK.starts_with(b"RPSPARSE1") || DISK.len() < 21 {
        return Err(bad());
    }
    let size = u64::from_le_bytes(DISK[9..17].try_into().unwrap()) as usize;
    let runs = u32::from_le_bytes(DISK[17..21].try_into().unwrap()) as usize;
    let mut img = vec![0u8; size];
    let mut p = 21usize;
    for _ in 0..runs {
        let head = DISK.get(p..p + 12).ok_or_else(bad)?;
        let off = u64::from_le_bytes(head[0..8].try_into().unwrap()) as usize;
        let n = u32::from_le_bytes(head[8..12].try_into().unwrap()) as usize * 512;
        p += 12;
        let data = DISK.get(p..p + n).ok_or_else(bad)?;
        img.get_mut(off..off + n).ok_or_else(bad)?.copy_from_slice(data);
        p += n;
    }
    Ok(img)
}

struct Expected {
    items: BTreeMap<String, (bool, u64, String)>,
    hidden: BTreeMap<String, bool>,
    found: BTreeMap<String, (bool, u64, u32)>,
    lost: usize,
    problems: usize,
}

fn expected() -> Expected {
    let mut e = Expected { items: BTreeMap::new(), hidden: BTreeMap::new(), found: BTreeMap::new(), lost: 0, problems: 0 };
    for line in EXPECTED.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        match f.as_slice() {
            ["ITEM", path, dir, dmg, ranges] => {
                e.items.insert(path.to_string(), (*dir == "1", dmg.parse().unwrap_or(0), ranges.to_string()));
            }
            ["HIDDEN", path, maybe] => {
                e.hidden.insert(path.to_string(), *maybe == "1");
            }
            ["FOUND", path, dir, size, crc] => {
                e.found.insert(path.to_string(), (*dir == "1", size.parse().unwrap_or(0), crc.parse().unwrap_or(0)));
            }
            ["LOST", n] => e.lost = n.parse().unwrap_or(0),
            ["PROBLEMS", n] => e.problems = n.parse().unwrap_or(0),
            _ => {}
        }
    }
    e
}

/// What the self-test found.
pub struct Findings {
    pub summary: Vec<String>,
    pub problems: Vec<String>,
    /// Where the work was done (kept when something failed).
    pub folder: PathBuf,
}

fn write(p: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("could not create {}: {e}", d.display()))?;
    }
    std::fs::write(p, data).map_err(|e| format!("could not write {}: {e}", p.display()))
}

/// The file checks: (summary line, problems).
fn file_checks(dir: &Path, say: &mut dyn FnMut(&str)) -> Result<(String, Vec<String>), String> {
    say("Checking sample files, intact and damaged...");
    let mut want: HashMap<String, bool> = HashMap::new(); // file name -> should be flagged as damaged
    for (name, data, cut, hole) in SEEDS {
        write(&dir.join(format!("intact {name}")), data)?;
        want.insert(format!("intact {name}"), false);
        if *cut {
            write(&dir.join(format!("cut short {name}")), &data[..data.len() * 55 / 100])?;
            want.insert(format!("cut short {name}"), true);
        }
        if *hole {
            let k = (data.len() / 2) / 4096 * 4096;
            let mut d = data.to_vec();
            d[k..k + 4096].fill(0);
            write(&dir.join(format!("hole {name}")), &d)?;
            want.insert(format!("hole {name}"), true);
        }
        let mut d = data.to_vec();
        d[..4096].fill(0);
        write(&dir.join(format!("start never read {name}")), &d)?;
        want.insert(format!("start never read {name}"), true);
    }
    write(&dir.join("named wrongly black-sky.png"), SEEDS[0].1)?;
    want.insert("named wrongly black-sky.png".to_string(), true);
    // Names and paths Windows makes awkward: other alphabets, emoji, and
    // a path longer than old Windows programs can handle.
    let deep: PathBuf = dir
        .join("家族の写真 😀 Ünïcödé")
        .join("a folder with a long name to make the whole path longer than two hundred and sixty characters")
        .join("and another folder with a long name so that old Windows programs could not open it at all");
    write(&deep.join("intact photo with a long path.png"), SEEDS[2].1)?;
    want.insert("intact photo with a long path.png".to_string(), false);

    let mut got: HashMap<String, (&'static str, String)> = HashMap::new();
    crate::walk::walk_and_probe(&dir.display().to_string(), 4096, true, None, |ev| {
        if let crate::walk::Event::Row(r) = ev {
            let name = r.path.rsplit(['\\', '/']).next().unwrap_or("").to_string();
            got.insert(name, (r.verdict, r.detail));
        }
    });
    let mut problems = Vec::new();
    let mut caught: BTreeMap<&str, u64> = BTreeMap::new();
    let (mut intact, mut damaged) = (0u64, 0u64);
    for (name, bad) in &want {
        match got.get(name) {
            None => problems.push(format!("the file check never saw {name}")),
            Some((v, d)) => {
                if *bad {
                    damaged += 1;
                    if crate::formats::is_damage(v) {
                        *caught.entry(v).or_insert(0) += 1;
                    } else {
                        problems.push(format!("{name} is damaged, but the file check said {v} {d}"));
                    }
                } else {
                    intact += 1;
                    if *v != "OK" {
                        problems.push(format!("{name} is intact, but the file check said {v}: {d}"));
                    }
                }
            }
        }
    }
    let how: Vec<String> = caught.iter().map(|(k, n)| format!("{k} {n}")).collect();
    Ok((
        format!(
            "File checks: {intact} intact sample files passed, {} of {damaged} damaged copies were caught ({}).",
            caught.values().sum::<u64>(),
            how.join(", ")
        ),
        problems,
    ))
}

fn crc_of(p: &Path) -> Option<(u64, u32)> {
    let data = std::fs::read(p).ok()?;
    Some((data.len() as u64, crate::gpt::crc32(&data)))
}

/// The map check, the rescue and the join, on the built-in disk.
fn disk_checks(dir: &Path, say: &mut dyn FnMut(&str)) -> Result<(Vec<String>, Vec<String>), String> {
    let mut summary = Vec::new();
    let mut problems = Vec::new();
    let exp = expected();
    let img = dir.join("test disk.img");
    write(&img, &disk_image()?)?;
    let map = dir.join("test disk.map");
    write(&map, MAP.as_bytes())?;

    say("Map check on the built-in test disk...");
    let out = dir.join("map check");
    let opts = crate::mapcheck::Options {
        map_path: map.display().to_string(),
        earlier_maps: Vec::new(),
        disk: img.display().to_string(),
        out_dir: out.display().to_string(),
    };
    let report = crate::mapcheck::run(&opts, &mut |_s: &str| {})?;
    let items: BTreeMap<String, (bool, u64, String)> = report
        .items
        .iter()
        .filter(|i| !i.path.is_empty())
        .map(|i| {
            let ranges = i.ranges.iter().map(|(s, e, _)| format!("{s}-{e}")).collect::<Vec<_>>().join(";");
            (i.path.clone(), (i.is_dir, i.damaged_bytes(), ranges))
        })
        .collect();
    if items != exp.items {
        let a: BTreeSet<_> = items.iter().collect();
        let b: BTreeSet<_> = exp.items.iter().collect();
        problems.push(format!(
            "map check: damaged files and folders differ - found {:?}, expected {:?}",
            a.difference(&b).take(3).collect::<Vec<_>>(),
            b.difference(&a).take(3).collect::<Vec<_>>()
        ));
    }
    let hidden: BTreeMap<String, bool> = report.hidden.iter().map(|(e, _)| (e.path.clone(), e.hidden_maybe)).collect();
    if hidden != exp.hidden {
        let a: BTreeSet<_> = hidden.iter().collect();
        let b: BTreeSet<_> = exp.hidden.iter().collect();
        problems.push(format!(
            "map check: the files Windows cannot list differ - found {:?}, expected {:?}",
            a.difference(&b).take(3).collect::<Vec<_>>(),
            b.difference(&a).take(3).collect::<Vec<_>>()
        ));
    }
    if report.folder_problems.len() != exp.problems {
        problems.push(format!("map check: {} folder problems, expected {}", report.folder_problems.len(), exp.problems));
    }
    if report.lost_runs.len() != exp.lost {
        problems.push(format!("map check: {} pieces of lost data, expected {}", report.lost_runs.len(), exp.lost));
    }
    let by_path = exp.hidden.values().filter(|m| **m).count();
    summary.push(format!(
        "Map check: {} damaged files and folders, {} files and folders Windows cannot list ({by_path} still open by full \
         path), {} pieces of lost data - {}.",
        items.len(),
        hidden.len(),
        report.lost_runs.len(),
        if problems.is_empty() { "all as expected" } else { "NOT as expected" }
    ));

    say("Copying everything off the test disk...");
    let rescued = dir.join("rescued");
    let ropts = crate::rescue::Options {
        disk: img.display().to_string(),
        out_dir: rescued.display().to_string(),
        pick: crate::rescue::Pick::Everything,
        lost: true,
        stop: None,
    };
    let o = crate::rescue::run(&ropts, &mut |_s: &str| {}, &mut |_d, _t| {})?;
    let before = problems.len();
    for (path, (is_dir, size, crc)) in &exp.found {
        let p = path.split('\\').fold(rescued.clone(), |acc, part| acc.join(crate::rescue::safe_name(part)));
        if *is_dir {
            if !p.is_dir() {
                problems.push(format!("rescue: the folder {path} was not made"));
            }
        } else {
            match crc_of(&p) {
                Some((n, c)) if n == *size && c == *crc => {}
                Some(_) => problems.push(format!("rescue: the copy of {path} is not the same as the file on the disk")),
                None => problems.push(format!("rescue: {path} was not copied")),
            }
        }
    }
    if o.lost_pieces as usize != exp.lost {
        problems.push(format!("rescue: saved {} pieces of lost data, expected {}", o.lost_pieces, exp.lost));
    }
    if o.renamed == 0 {
        problems.push("rescue: the folder whose name ends in a space kept a name Windows cannot use".to_string());
    }
    summary.push(format!(
        "Rescue: {} files and {} folders copied and compared byte for byte, {} pieces of lost data saved, {} name made safe \
         for Windows - {}.",
        commas(o.files),
        commas(o.folders),
        o.lost_pieces,
        o.renamed,
        if problems.len() == before { "all as expected" } else { "NOT as expected" }
    ));

    say("Checking the copies together with the map check...");
    let tsv = std::fs::read_to_string(out.join(crate::join::TSV_NAME))
        .map_err(|e| format!("could not read the map check's list of files: {e}"))?;
    let items = crate::join::parse_tsv(&tsv);
    let mut c = crate::join::Collector::new(&items);
    let mut rows = Vec::new();
    crate::walk::walk_and_probe(&rescued.display().to_string(), 4096, true, None, |ev| {
        if let crate::walk::Event::Row(r) = ev {
            c.saw(&r);
            if r.verdict != "OK" && r.verdict != "SKIPPED" {
                rows.push(r);
            }
        }
    });
    let found = crate::join::find(&items, &c);
    let mut skipped = Vec::new();
    crate::join::apply(&found, &mut rows, &mut skipped);
    let never: Vec<&str> =
        rows.iter().filter(|r| r.verdict == crate::join::VERDICT).map(|r| r.path.as_str()).collect();
    let damaged_files = exp.items.iter().filter(|(_, (d, _, _))| !d).count();
    if never.len() != damaged_files {
        problems.push(format!(
            "joining: {} copies marked {}, expected {damaged_files} ({never:?})",
            never.len(),
            crate::join::VERDICT
        ));
    }
    summary.push(format!(
        "Checking the copies with the map check: {} damaged file marked {} - {}.",
        never.len(),
        crate::join::VERDICT,
        if never.len() == damaged_files { "as expected" } else { "NOT as expected" }
    ));
    Ok((summary, problems))
}

/// Run every check. Err only when the self-test could not run at all.
pub fn run_checks(say: &mut dyn FnMut(&str)) -> Result<Findings, String> {
    let dir = std::env::temp_dir().join(format!("recovery-probe-self-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let mut summary = Vec::new();
    let mut problems = Vec::new();
    match file_checks(&dir.join("files"), say) {
        Ok((s, p)) => {
            summary.push(s);
            problems.extend(p);
        }
        Err(e) => problems.push(format!("the file checks could not run: {e}")),
    }
    match disk_checks(&dir.join("disk"), say) {
        Ok((s, p)) => {
            summary.extend(s);
            problems.extend(p);
        }
        Err(e) => problems.push(format!("the disk checks could not run: {e}")),
    }
    if problems.is_empty() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    Ok(Findings { summary, problems, folder: dir })
}

/// For the page.
pub fn run(say: &mut dyn FnMut(&str), _stop: &AtomicBool) -> Result<crate::web::Outcome, String> {
    let r = run_checks(say)?;
    let mut summary = r.summary.clone();
    let ok = r.problems.is_empty();
    if ok {
        summary.insert(0, "Every check passed: the program works on this PC.".to_string());
    } else {
        summary.insert(0, format!("{} checks did NOT give the right answer:", r.problems.len()));
        for p in r.problems.iter().take(20) {
            summary.insert(1, format!("  {p}"));
        }
        summary.push(format!("The files it used are kept in {} for a closer look.", r.folder.display()));
    }
    let data = format!("{{\"passed\":{ok},\"problems\":{}}}", r.problems.len());
    Ok(crate::web::Outcome {
        summary,
        data,
        problem: (!ok).then(|| "some checks did not give the right answer".to_string()),
        ..Default::default()
    })
}

/// `recovery-probe self-test`
pub fn run_cli() -> i32 {
    println!("{} {} - self-test", crate::APP, crate::VERSION);
    let r = match run_checks(&mut |s| println!("  {s}")) {
        Ok(r) => r,
        Err(e) => {
            println!("The self-test could not run: {e}");
            return 1;
        }
    };
    println!();
    for s in &r.summary {
        println!("{s}");
    }
    if r.problems.is_empty() {
        println!();
        println!("Every check passed: the program works on this PC.");
        0
    } else {
        println!();
        println!("{} checks did NOT give the right answer:", r.problems.len());
        for p in &r.problems {
            println!("  {p}");
        }
        println!("The files it used are kept in {}", r.folder.display());
        1
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_built_in_disk_unpacks() {
        let img = super::disk_image().unwrap();
        assert_eq!(img.len(), 16 << 20);
        assert_eq!(&img[510..512], &[0x55, 0xAA], "protective MBR");
        assert_eq!(&img[512..520], b"EFI PART");
    }
}
