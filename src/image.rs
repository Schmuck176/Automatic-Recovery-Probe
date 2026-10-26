//! Making an image: the recovered SSD read once, from start to end, into one
//! file on another drive - a second, complete copy. After that the SSD can go
//! back in its drawer: the map check and the copying read the image instead,
//! as often as needed.
//!
//! The image gets a SHA-256 worked out from the bytes as they came off the
//! SSD, and is then read back and checked against it. A place the SSD itself
//! cannot read (not expected: it is a healthy drive) is left as zeros and
//! recorded in a ddrescue-style mapfile beside the image, so the map check
//! counts it as never read too. A stopped image carries on where it stopped.
//!
//! A weak SSD is not worn down. The SSD has sat unpowered for years, so a
//! place it struggles to read is possible. When a piece will not read, it is
//! read again 4 KB at a time - but only so far: after a stretch of 4 KB
//! pieces in a row that will not read, or too many in one 8 MB piece, or a
//! few minutes on it, the rest of that piece is stepped over without being
//! tried, left as zeros and listed as such ('?' in the list beside the
//! image; a place tried and not read is '-'). Stop works at any moment, and
//! the page says how the careful reading is getting on. If the SSD stops
//! answering at a place (some USB cases reset when a drive takes too long),
//! the next run reads that piece 4 KB at a time, and if it stops answering
//! again while doing that, the run after steps over the rest of the piece
//! from there: the image always gets past a bad place, instead of stopping at
//! it every time.
//!
//! Carrying on is only done from a point known to be safe on the drive:
//! every few gigabytes the image (and the list of places the SSD could not
//! read) is made safe on the drive and that point is noted beside it,
//! together with how far the checksum had got. After a crash or a power cut a
//! file can come back the right length but with zeros where its last-written
//! part should be (Windows writes the data later than it notes the size); the
//! part after the noted point is read from the SSD again, so no such zeros can
//! end up in the image unnoticed. The checksum carries on from the noted
//! point too, so it is always worked out from the bytes as they came off the
//! SSD - also for an image made in several runs - and the read-back check
//! compares the whole image with the SSD's bytes. The image is checked by
//! reading it back from the drive itself, not from what Windows still holds
//! of it in memory. Once checked, the image is marked read-only.
//!
//! Only the copy ddrescue made is ever imaged (mapcheck::is_the_copy): not
//! another drive that happens to be the old drive's size. An image already
//! in the folder is only taken if it starts the way the disk does. Its
//! checksum and its list of places the SSD could not read are written, safe
//! on the drive, before the image gets its name: whenever
//! recovered-ssd.img is there, so are they.

use crate::disk::Source;
use crate::fmt::{commas, duration, size};
use crate::sha256::Sha256;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const IMAGE_NAME: &str = "recovered-ssd.img";
/// Beside an image: the places the SSD itself could not read (if any).
pub const READ_ERRORS_SUFFIX: &str = ".ssd-read-errors.log";
const CHUNK: usize = 8 << 20;
const UNIT: usize = 4096;
/// How often what is made so far is made safe on the drive (see the top).
const SAFE_EVERY: u64 = 2 << 30;
/// Beside the image while it is made: how much of it is safe on the drive.
const SAFE_SUFFIX: &str = ".partial-safe.txt";
/// Stop rather than go on if the SSD cannot read this much (tried or
/// stepped over): something is wrong with it, and the person should ask
/// before going on.
const TOO_MUCH_UNREADABLE: u64 = 64 << 20;
/// Reading a piece 4 KB at a time around a place the SSD cannot read: the
/// rest of the piece is stepped over after this many 4 KB pieces in a row
/// would not read (64 KB),
const GIVE_UP_IN_A_ROW: u32 = 16;
/// or after this many in the piece,
const GIVE_UP_IN_PIECE: u32 = 64;
/// or after this long on the piece.
const GIVE_UP_AFTER: Duration = Duration::from_secs(300);
/// How often the page is told how reading a piece 4 KB at a time is getting on.
const TELL_EVERY: Duration = if cfg!(test) { Duration::from_millis(300) } else { Duration::from_secs(5) };
/// In the list of places beside the image: tried, and the SSD would not give it.
pub const UNREADABLE: char = '-';
/// In that list: stepped over without being tried, to spare a weak SSD.
pub const STEPPED_OVER: char = '?';

pub struct Options {
    /// The disk (\\.\PhysicalDriveN) or, for tests, an image file.
    pub disk: String,
    /// The folder the image goes into.
    pub out_dir: String,
    /// How many bytes to copy: the size of the disk the mapfile describes.
    pub size: u64,
    pub stop: Option<Arc<AtomicBool>>,
    /// While set, the SSD is not read (it is cooling down): reading carries
    /// on when it is cleared.
    pub pause: Option<Arc<AtomicBool>>,
}

#[derive(Default, Debug)]
pub struct Outcome {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// (start, length) of the places the SSD would not give (zeros in the image).
    pub unreadable: Vec<(u64, u64)>,
    /// (start, length) of the places stepped over without being tried, to
    /// spare a weak SSD (zeros in the image too).
    pub stepped_over: Vec<(u64, u64)>,
    /// The image read back gives the same SHA-256.
    pub verified: bool,
    /// The image was already there: only checked.
    pub reused: bool,
    pub stopped: bool,
    pub disk_lost: Option<String>,
    /// Where the SSD stopped answering (when it did).
    pub lost_at: Option<u64>,
    pub read_seconds: u64,
}

/// The mapfile of places the SSD could not read, for an image file (if it has one).
pub fn read_errors_map(image: &str) -> Option<PathBuf> {
    let p = PathBuf::from(format!("{image}{READ_ERRORS_SUFFIX}"));
    p.is_file().then_some(p)
}

fn sha_line(hex: &str) -> String {
    // The format sha256sum and shasum read and write.
    format!("{hex} *{IMAGE_NAME}\n")
}

/// The SHA-256 a .sha256 file next to the image gives.
fn recorded_sha(path: &Path) -> Option<String> {
    let t = std::fs::read_to_string(path).ok()?;
    let h = t.split_whitespace().next()?.to_lowercase();
    (h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())).then_some(h)
}

/// Sorted, with places that touch or overlap made one (each kind on its own).
fn merge(ranges: &mut Vec<(u64, u64)>) {
    ranges.sort();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for &(s, n) in ranges.iter() {
        match out.last_mut() {
            Some(l) if l.0 + l.1 >= s => l.1 = (s + n).max(l.0 + l.1) - l.0,
            _ => out.push((s, n)),
        }
    }
    *ranges = out;
}

/// Adds a place to a list of (start, length, kind), joining it to the last
/// one when they touch and are of the same kind.
fn add_place(list: &mut Vec<(u64, u64, char)>, start: u64, len: u64, kind: char) {
    match list.last_mut() {
        Some(l) if l.0 + l.1 == start && l.2 == kind => l.1 += len,
        _ => list.push((start, len, kind)),
    }
}

/// A ddrescue-style mapfile of [0, size) with `bad` marked with their kind
/// ('-' could not be read, '?' stepped over without being tried).
fn mapfile_text(size: u64, bad: &[(u64, u64, char)], disk: &str) -> String {
    let now = crate::fmt::local_datetime(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as f64).unwrap_or(0.0),
    );
    let mut t = format!(
        "# Mapfile. Created by {} {} (not GNU ddrescue): the places the recovered SSD itself could not read when its image was made\n\
         # ('-': tried, and the SSD would not give it; '?': stepped over without being tried, to spare the SSD)\n\
         # Command line: recovery-probe image {disk}\n# Start time:   {now}\n# Current time: {now}\n# Finished\n\
         # current_pos  current_status  current_pass\n0x00000000     +               1\n#      pos        size  status\n",
        crate::APP,
        crate::VERSION
    );
    let mut pos = 0u64;
    for &(s, n, kind) in bad {
        if s > pos {
            t.push_str(&format!("0x{pos:08X}  0x{:08X}  +\n", s - pos));
        }
        t.push_str(&format!("0x{s:08X}  0x{n:08X}  {kind}\n"));
        pos = s + n;
    }
    if pos < size {
        t.push_str(&format!("0x{pos:08X}  0x{:08X}  +\n", size - pos));
    }
    t
}

/// The two kinds of place, one list, in disk order, for the mapfile.
fn both_kinds(unreadable: &[(u64, u64)], stepped_over: &[(u64, u64)]) -> Vec<(u64, u64, char)> {
    let mut all: Vec<(u64, u64, char)> = unreadable
        .iter()
        .map(|&(s, n)| (s, n, UNREADABLE))
        .chain(stepped_over.iter().map(|&(s, n)| (s, n, STEPPED_OVER)))
        .collect();
    all.sort();
    all
}

fn hash_file_part(f: &mut std::fs::File, len: u64, sha: &mut Sha256, stop: &dyn Fn() -> bool) -> std::io::Result<bool> {
    f.seek(SeekFrom::Start(0))?;
    let mut buf = vec![0u8; CHUNK];
    let mut left = len;
    while left > 0 {
        if stop() {
            return Ok(false);
        }
        let n = (left.min(CHUNK as u64)) as usize;
        f.read_exact(&mut buf[..n])?;
        sha.update(&buf[..n]);
        left -= n as u64;
    }
    Ok(true)
}

enum Piece {
    /// A piece read, with zeros where it would not read, and those places:
    /// (start, length, UNREADABLE or STEPPED_OVER).
    Data { data: Vec<u8>, bad: Vec<(u64, u64, char)>, secs: f64 },
    /// The SSD stopped answering while reading at `at` (`careful`: while the
    /// piece was being read 4 KB at a time).
    Lost { err: String, at: u64, careful: bool },
    /// Stop was clicked while a piece was being read 4 KB at a time (nothing
    /// of that piece is kept: the next run reads it again).
    Stopped,
}

/// How much of the start of an image is compared with the disk: the
/// partition table, which names its disk with a number of its own.
const START: usize = 64 << 10;

/// Does an image start the way the disk does? None if it does; otherwise
/// why not.
fn differs_at_start(src: &dyn Source, image: &Path, size: u64) -> Option<String> {
    let n = (START as u64).min(size) as usize;
    let disk = match src.read_at(0, n) {
        Ok(d) if d.len() == n => d,
        Ok(_) => return Some("the disk ended before its partition table did".to_string()),
        Err(e) => return Some(format!("the disk could not be read ({e})")),
    };
    let img = match crate::disk::ImageFile::open(image).and_then(|f| f.read_at(0, n)) {
        Ok(d) => d,
        Err(e) => return Some(format!("the image could not be read ({e})")),
    };
    (img != disk).then(|| "it does not start the way this disk does".to_string())
}

/// What the note beside an unfinished image says: how much of it is safe on
/// the drive, how far the checksum had got there, and where the SSD stopped
/// answering last time (and whether the piece was being read 4 KB at a time).
#[derive(Default)]
struct SafeNote {
    at: u64,
    sha: Option<Sha256>,
    lost: Option<(u64, bool)>,
}

fn read_note(path: &Path) -> SafeNote {
    let Ok(t) = std::fs::read_to_string(path) else { return SafeNote::default() };
    let mut note = SafeNote {
        at: t.split_whitespace().next().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0),
        ..Default::default()
    };
    for line in t.lines().skip(1) {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            ["sha256", state] => note.sha = Sha256::import(state),
            ["lost", at, how] => note.lost = at.parse().ok().map(|a| (a, *how == "careful")),
            _ => {}
        }
    }
    note
}

fn note_text(at: u64, sha: &Sha256, lost: Option<(u64, bool)>) -> String {
    let mut t = format!("{at}\r\n");
    if let Some(s) = sha.export() {
        t.push_str(&format!("sha256 {s}\r\n"));
    }
    if let Some((p, careful)) = lost {
        t.push_str(&format!("lost {p} {}\r\n", if careful { "careful" } else { "whole" }));
    }
    t
}

/// The list of places an unfinished image could not read, as kept beside it
/// while it is made: "start length kind" a line (older lists have no kind:
/// '-').
fn read_places(text: &str) -> Vec<(u64, u64, char)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        let (Some(s), Some(n)) = (w.first().and_then(|x| x.parse::<u64>().ok()), w.get(1).and_then(|x| x.parse::<u64>().ok()))
        else {
            continue;
        };
        let kind = match w.get(2) {
            Some(&"?") => STEPPED_OVER,
            _ => UNREADABLE,
        };
        out.push((s, n, kind));
    }
    out
}

/// Make the image read-only, so nothing - another program, a mount that is
/// not read-only - can change it by accident. (Its checksum would show it,
/// but the image is the copy everything after is made from.)
fn make_read_only(p: &Path) {
    if let Ok(m) = std::fs::metadata(p) {
        let mut perm = m.permissions();
        if !perm.readonly() {
            perm.set_readonly(true);
            let _ = std::fs::set_permissions(p, perm);
        }
    }
}

/// Check an image made earlier against the SHA-256 saved beside it
/// (`recovered-ssd.img.sha256`). Only reads; the SSD is not needed.
pub fn check_existing(
    image: &Path,
    progress: &mut dyn FnMut(&str),
    amount: &mut dyn FnMut(u64, u64),
    stop: &dyn Fn() -> bool,
) -> Result<Outcome, String> {
    let shown = image.display().to_string();
    let image = crate::rescue::verbatim(image);
    let mut sha_name = image.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    sha_name.push(".sha256");
    let want = recorded_sha(&image.with_file_name(&sha_name))
        .ok_or_else(|| format!("{shown} has no checksum file beside it ({}), so it cannot be checked", sha_name.to_string_lossy()))?;
    let len = std::fs::metadata(&image).map_err(|e| format!("could not open {shown}: {e}"))?.len();
    let mut o = Outcome { path: shown.clone(), size: len, reused: true, ..Default::default() };
    let mut last = std::time::Instant::now();
    let got = crate::sha256::file_hex_from_drive(
        &image,
        &mut |done, total| {
            amount(done, total);
            if last.elapsed().as_millis() > 1000 {
                progress(&format!("Checked {} of {}", size(done), size(total)));
                last = std::time::Instant::now();
            }
        },
        stop,
    )
    .map_err(|e| format!("could not read {shown}: {e}"))?;
    let Some((_, hex)) = got else {
        o.stopped = true;
        return Ok(o);
    };
    o.sha256 = hex;
    o.verified = o.sha256 == want;
    if let Some(m) = read_errors_map(&image.display().to_string()) {
        if let Ok(t) = std::fs::read_to_string(m) {
            if let Ok(mf) = crate::mapfile::parse(&t) {
                for a in mf.bad_areas() {
                    if a.status == STEPPED_OVER {
                        o.stepped_over.push((a.start, a.len()));
                    } else {
                        o.unreadable.push((a.start, a.len()));
                    }
                }
            }
        }
    }
    Ok(o)
}

/// Make the note beside the unfinished image say how far it is safe on the
/// drive: the image and the list of places first, then the note.
fn make_safe(
    f: &std::fs::File,
    errs: &std::fs::File,
    note: &Path,
    at: u64,
    sha: &Sha256,
    lost: Option<(u64, bool)>,
) -> bool {
    errs.sync_all().is_ok() && f.sync_all().is_ok() && crate::sys::write_safely(note, note_text(at, sha, lost).as_bytes()).is_ok()
}

/// Make (or carry on making, or just check) the image.
pub fn make(
    src: &dyn Source,
    opts: &Options,
    progress: &mut dyn FnMut(&str),
    amount: &mut dyn FnMut(u64, u64),
) -> Result<Outcome, String> {
    let stop_flag = opts.stop.clone();
    let stop = || stop_flag.as_ref().is_some_and(|s| s.load(Ordering::Relaxed));
    // Only ever the copy ddrescue made: never another drive that happens to
    // be the old drive's size (every "1 TB" drive is exactly that size).
    let table = crate::gpt::read(src, &[]);
    if let Some(why) = crate::mapcheck::not_the_copy(src, &table, opts.size) {
        return Err(format!(
            "{} is not the copy ddrescue made: {why}. Nothing was read or written. Choose the recovered SSD (marked \
             \"the copy ddrescue made\") in step 3.",
            src.describe()
        ));
    }
    let dir = crate::rescue::verbatim(Path::new(&opts.out_dir));
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", opts.out_dir))?;
    let final_path = dir.join(IMAGE_NAME);
    let partial = dir.join(format!("{IMAGE_NAME}.partial"));
    let errs_path = dir.join(format!("{IMAGE_NAME}.partial-unreadable.txt"));
    let safe_path = dir.join(format!("{IMAGE_NAME}{SAFE_SUFFIX}"));
    let sha_path = dir.join(format!("{IMAGE_NAME}.sha256"));
    let map_path = dir.join(format!("{IMAGE_NAME}{READ_ERRORS_SUFFIX}"));
    let info_path = dir.join(format!("{IMAGE_NAME}.txt"));
    let shown = Path::new(&opts.out_dir).join(IMAGE_NAME).display().to_string();
    let mut o = Outcome { path: shown.clone(), size: opts.size, ..Default::default() };

    // Made before: check it against its SHA-256 - if it is an image of this
    // disk at all (another disk's image would pass its own check).
    if final_path.exists() {
        if let Some(why) = differs_at_start(src, &final_path, opts.size) {
            return Err(format!(
                "{shown} is already there, but it is not an image of {}: {why}. Choose another folder for the \
                 image, and ask for help before deleting anything.",
                src.describe()
            ));
        }
        progress(&format!("{shown} is already there: checking it against its checksum (the SSD itself is not read for this)..."));
        let mut o = check_existing(&final_path, progress, amount, &stop)?;
        o.path = shown;
        o.verified &= o.size == opts.size;
        if o.verified {
            make_read_only(&final_path);
        }
        // What a run stopped just after naming the image leaves behind.
        let _ = std::fs::remove_file(&errs_path);
        let _ = std::fs::remove_file(&safe_path);
        return Ok(o);
    }

    if src.size() > 0 && src.size() < opts.size {
        return Err(format!(
            "{} holds {} bytes, fewer than the {} bytes of the drive the mapfile describes: is it the recovered SSD?",
            src.describe(),
            commas(src.size()),
            commas(opts.size)
        ));
    }

    // Carry on with an image a stopped run began.
    let mut start = 0u64;
    let mut sha = Sha256::new();
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&partial)
        .map_err(|e| format!("could not create {}: {e}", partial.display()))?;
    let have = f.metadata().map(|m| m.len()).unwrap_or(0);
    // Only as far as it is known to be safe on the drive (see the top).
    let note = read_note(&safe_path);
    let keep = if note.at <= have.min(opts.size) { note.at } else { 0 };
    let mut lost_before: Option<(u64, bool)> = None;
    if keep == 0 {
        // Nothing known to be safe: start from the beginning.
        if have > 0 {
            f.set_len(0).map_err(|e| format!("could not use {}: {e}", partial.display()))?;
            progress("An earlier image was not finished, and none of it is known to be safe on the drive: starting again.");
        }
        let _ = std::fs::remove_file(&safe_path);
        // (Where the SSD stopped answering is still worth knowing.)
        lost_before = note.lost;
    }
    if keep > 0 {
        // Carried on only if it was begun from this disk.
        if let Some(why) = differs_at_start(src, &partial, keep) {
            return Err(format!(
                "{} holds an unfinished image, but not one of {}: {why}. Choose another folder for the image, and \
                 ask for help before deleting anything.",
                opts.out_dir,
                src.describe()
            ));
        }
        f.set_len(keep).map_err(|e| format!("could not use {}: {e}", partial.display()))?;
        match note.sha.filter(|s| s.total() == keep) {
            // The checksum carries on from where it had got, as worked out
            // from the SSD's own bytes.
            Some(s) => {
                sha = s;
                progress(&format!(
                    "Carrying on with the image a stopped run began ({} of {} done{}).",
                    size(keep),
                    size(opts.size),
                    if have > keep { "; what was written after the last safe point is read from the SSD again" } else { "" }
                ));
            }
            // A note from an older version: the checksum is worked out again
            // from the part made so far.
            None => {
                progress(&format!(
                    "Carrying on with the image a stopped run began ({} of {} done{}). Reading that part back first...",
                    size(keep),
                    size(opts.size),
                    if have > keep { "; what was written after the last safe point is read from the SSD again" } else { "" }
                ));
                if !hash_file_part(&mut f, keep, &mut sha, &stop)
                    .map_err(|e| format!("could not read {}: {e}", partial.display()))?
                {
                    o.stopped = true;
                    return Ok(o);
                }
            }
        }
        if let Ok(t) = std::fs::read_to_string(&errs_path) {
            for (s, n, kind) in read_places(&t) {
                if s < keep {
                    let n = n.min(keep - s);
                    if kind == STEPPED_OVER {
                        o.stepped_over.push((s, n));
                    } else {
                        o.unreadable.push((s, n));
                    }
                }
            }
        }
        // The list keeps only the places before the safe point: the rest is
        // read again.
        let kept: String = both_kinds(&o.unreadable, &o.stepped_over).iter().map(|(s, n, k)| format!("{s} {n} {k}\n")).collect();
        crate::sys::write_safely(&errs_path, kept.as_bytes()).map_err(|e| format!("could not write {}: {e}", errs_path.display()))?;
        start = keep;
        // Where the SSD stopped answering last time, if this run has not got
        // past it yet.
        lost_before = note.lost.filter(|(p, _)| *p >= keep);
    } else {
        let _ = std::fs::remove_file(&errs_path);
    }
    if let Some(free) = crate::win::free_space(&dir) {
        let need = opts.size - start + (64 << 20);
        if need > free {
            return Err(format!(
                "the image needs {} more but the drive {} is on has only {} free. Choose a drive with more room.",
                size(need),
                opts.out_dir,
                size(free)
            ));
        }
    }
    f.seek(SeekFrom::Start(start)).map_err(|e| format!("could not use {}: {e}", partial.display()))?;
    let mut errs =
        std::fs::OpenOptions::new().create(true).append(true).open(&errs_path).map_err(|e| format!("could not write {}: {e}", errs_path.display()))?;
    progress(&format!("Reading {} into {shown}", src.describe()));
    if let Some((p, careful)) = lost_before {
        progress(&format!(
            "The SSD stopped answering at byte {} last time. That 8 MB piece is read 4 KB at a time this time{}.",
            commas(p),
            if careful {
                ", up to that place; the rest of the piece is stepped over without being tried, so the image gets past it"
            } else {
                ""
            }
        ));
    }
    amount(start, opts.size);
    // The page's strip of the SSD (live.rs): what is read, and how.
    let earlier_bad: Vec<(u64, u64)> = o.unreadable.iter().chain(o.stepped_over.iter()).copied().collect();
    crate::live::drive_begin(opts.size, start, &earlier_bad);

    // Reading and writing overlap: one thread reads the SSD, this one hashes
    // and writes.
    let watched = crate::disk::Watched::new(src);
    let quit = AtomicBool::new(false);
    // How the careful reading of a piece is getting on, for the page.
    let careful_on = AtomicBool::new(false);
    let careful_at = AtomicU64::new(0);
    let careful_done = AtomicU64::new(0);
    let careful_failed = AtomicU64::new(0);
    let t0 = Instant::now();
    let mut pos = start;
    let mut safe_at = start;
    let mut unreadable_total: u64 = o.unreadable.iter().chain(o.stepped_over.iter()).map(|r| r.1).sum();
    let mut write_error: Option<String> = None;
    let mut too_much = false;
    let mut lost_now: Option<(u64, bool)> = None;
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Piece>(3);
        let watched = &watched;
        let quit = &quit;
        let (careful_on, careful_at, careful_done, careful_failed) = (&careful_on, &careful_at, &careful_done, &careful_failed);
        let size_all = opts.size;
        let pause = opts.pause.clone();
        scope.spawn(move || {
            // A piece read again 4 KB at a time (see the top). `step_over_from`:
            // where to stop trying and step over the rest of the piece.
            let careful_read = |at: u64, n: usize, step_over_from: Option<u64>| -> Piece {
                let mut data = vec![0u8; n];
                let mut bad: Vec<(u64, u64, char)> = Vec::new();
                let began = Instant::now();
                let (mut in_a_row, mut failed) = (0u32, 0u32);
                let stop_k = step_over_from.map(|p| ((p.saturating_sub(at)) as usize / UNIT * UNIT).min(n)).unwrap_or(n);
                careful_at.store(at, Ordering::Relaxed);
                careful_done.store(0, Ordering::Relaxed);
                careful_failed.store(0, Ordering::Relaxed);
                careful_on.store(true, Ordering::Relaxed);
                let mut k = 0usize;
                let mut lost: Option<Piece> = None;
                while k < n {
                    if quit.load(Ordering::Relaxed) {
                        lost = Some(Piece::Stopped);
                        break;
                    }
                    if k >= stop_k || in_a_row >= GIVE_UP_IN_A_ROW || failed >= GIVE_UP_IN_PIECE || began.elapsed() >= GIVE_UP_AFTER {
                        add_place(&mut bad, at + k as u64, (n - k) as u64, STEPPED_OVER);
                        break;
                    }
                    let m = UNIT.min(n - k);
                    match watched.read_at(at + k as u64, m) {
                        Ok(d) if d.len() == m => {
                            data[k..k + m].copy_from_slice(&d);
                            in_a_row = 0;
                        }
                        _ => {
                            if !watched.answers(0) {
                                lost = Some(Piece::Lost { err: watched.last_error(), at: at + k as u64, careful: true });
                                break;
                            }
                            add_place(&mut bad, at + k as u64, m as u64, UNREADABLE);
                            in_a_row += 1;
                            failed += 1;
                            careful_failed.store(failed as u64, Ordering::Relaxed);
                        }
                    }
                    k += m;
                    careful_done.store(k as u64, Ordering::Relaxed);
                }
                careful_on.store(false, Ordering::Relaxed);
                lost.unwrap_or(Piece::Data { data, bad, secs: began.elapsed().as_secs_f64() })
            };
            let mut at = start;
            while at < size_all && !quit.load(Ordering::Relaxed) {
                // Cooling down: nothing is read until it may go on.
                while pause.as_ref().is_some_and(|p| p.load(Ordering::Relaxed)) && !quit.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(250));
                }
                if quit.load(Ordering::Relaxed) {
                    break;
                }
                let n = (size_all - at).min(CHUNK as u64) as usize;
                let began = Instant::now();
                let piece = match lost_before {
                    // The SSD stopped answering in this piece last time: read
                    // it 4 KB at a time - and if that is how it was being read
                    // when it stopped answering, only up to that place.
                    Some((p, careful)) if p >= at && p < at + n as u64 => careful_read(at, n, careful.then_some(p)),
                    _ => match watched.read_at(at, n) {
                        Ok(d) if d.len() == n => Piece::Data { data: d, bad: Vec::new(), secs: began.elapsed().as_secs_f64() },
                        _ if !watched.answers(0) => Piece::Lost { err: watched.last_error(), at, careful: false },
                        // A bad spot: read it again in small pieces, and leave
                        // what will not read as zeros.
                        _ => careful_read(at, n, None),
                    },
                };
                let last = !matches!(piece, Piece::Data { .. });
                if tx.send(piece).is_err() || last {
                    break;
                }
                at += n as u64;
            }
        });
        let mut last = Instant::now();
        let mut last_told = Instant::now();
        let mut was_paused = false;
        loop {
            let piece = match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(p) => p,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Stop works at any moment, even while a piece is being
                    // read 4 KB at a time. (Telling how far it has got lets
                    // the command line notice Ctrl+C too.)
                    amount(pos, opts.size);
                    if stop() {
                        o.stopped = true;
                        quit.store(true, Ordering::Relaxed);
                    }
                    let paused = opts.pause.as_ref().is_some_and(|p| p.load(Ordering::Relaxed));
                    if paused != was_paused {
                        was_paused = paused;
                        crate::live::drive_paused(paused);
                        progress(if paused {
                            "Paused while the SSD cools down: reading carries on by itself when it has."
                        } else {
                            "The SSD has cooled down: reading carries on."
                        });
                    }
                    if careful_on.load(Ordering::Relaxed) && last_told.elapsed() >= TELL_EVERY {
                        let done = careful_done.load(Ordering::Relaxed);
                        progress(&format!(
                            "Reading 4 KB at a time around a place the SSD cannot read (byte {}): {}% of this 8 MB piece \
                             done, {} small pieces would not read so far.",
                            commas(careful_at.load(Ordering::Relaxed)),
                            done * 100 / CHUNK as u64,
                            careful_failed.load(Ordering::Relaxed)
                        ));
                        last_told = Instant::now();
                    }
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };
            match piece {
                Piece::Lost { err, at, careful } => {
                    o.disk_lost = Some(err);
                    o.lost_at = Some(at);
                    lost_now = Some((at, careful));
                    break;
                }
                Piece::Stopped => {
                    o.stopped = true;
                    break;
                }
                Piece::Data { data, bad, secs } => {
                    // (A piece read with the pause lifted: it is over. A piece
                    // the SSD was reading as the pause began still arrives.)
                    if was_paused && !opts.pause.as_ref().is_some_and(|p| p.load(Ordering::Relaxed)) {
                        was_paused = false;
                        crate::live::drive_paused(false);
                        progress("The SSD has cooled down: reading carries on.");
                    }
                    let bad_ranges: Vec<(u64, u64)> = bad.iter().map(|b| (b.0, b.1)).collect();
                    crate::live::drive_piece(pos, data.len() as u64, secs, &bad_ranges);
                    if let Err(e) = f.write_all(&data) {
                        write_error = Some(e.to_string());
                        break;
                    }
                    sha.update(&data);
                    if !bad.is_empty() {
                        let tried: u64 = bad.iter().filter(|b| b.2 == UNREADABLE).map(|b| b.1).sum();
                        let over: u64 = bad.iter().filter(|b| b.2 == STEPPED_OVER).map(|b| b.1).sum();
                        progress(&format!(
                            "At byte {} the SSD could not read {}{}. Left as zeros in the image and listed beside it.",
                            commas(bad[0].0),
                            size(tried),
                            if over > 0 {
                                format!("; {} after it were stepped over without being tried, to spare the SSD", size(over))
                            } else {
                                String::new()
                            }
                        ));
                    }
                    for &(s, n, kind) in &bad {
                        let _ = writeln!(errs, "{s} {n} {kind}");
                        unreadable_total += n;
                        if kind == STEPPED_OVER {
                            o.stepped_over.push((s, n));
                        } else {
                            o.unreadable.push((s, n));
                        }
                    }
                    let _ = errs.flush();
                    pos += data.len() as u64;
                    // Past the place it stopped answering last time: that
                    // note is done with.
                    if lost_before.is_some_and(|(p, _)| p < pos) {
                        lost_before = None;
                    }
                    if pos - safe_at >= SAFE_EVERY && pos < opts.size && make_safe(&f, &errs, &safe_path, pos, &sha, lost_before) {
                        safe_at = pos;
                    }
                    if last.elapsed().as_millis() > 500 {
                        amount(pos, opts.size);
                        last = Instant::now();
                    }
                    if unreadable_total > TOO_MUCH_UNREADABLE {
                        too_much = true;
                        break;
                    }
                    if stop() {
                        o.stopped = true;
                        break;
                    }
                }
            }
        }
        quit.store(true, Ordering::Relaxed);
    });
    o.read_seconds = t0.elapsed().as_secs();
    amount(pos, opts.size);
    let _ = f.flush();
    if write_error.is_none() && pos < opts.size {
        // Stopped part-way: so the next run carries on from here (and knows
        // where the SSD stopped answering, if it did).
        let _ = make_safe(&f, &errs, &safe_path, pos, &sha, lost_now.or(lost_before));
    }
    if let Some(e) = write_error {
        return Err(format!(
            "could not write the image ({e}) - was the drive it goes on unplugged, or is it full? What is made so far is \
             kept: start again to carry on."
        ));
    }
    if too_much {
        return Err(format!(
            "The SSD could not read {} so far, so the image was stopped. That is not normal for an SSD: ask for help before \
             going on. (What is made so far is kept.)",
            size(unreadable_total)
        ));
    }
    if o.disk_lost.is_some() || pos < opts.size {
        o.stopped = true;
        return Ok(o);
    }
    // (Stop clicked just as the last piece came in: the image is finished
    // anyway, and the read-back check below sees the Stop.)
    o.stopped = false;
    // All read: make it safe on the drive. Then everything that describes it
    // goes beside it, safe on the drive too, and only then does it get its
    // name - so recovered-ssd.img is never there without its checksum and
    // its list of places the SSD could not read (a power cut in between
    // leaves the .partial, which the next run finishes).
    f.sync_all().map_err(|e| format!("could not finish writing the image: {e}"))?;
    drop(f);
    merge(&mut o.unreadable);
    merge(&mut o.stepped_over);
    o.sha256 = sha.finish_hex();
    if o.unreadable.is_empty() && o.stepped_over.is_empty() {
        match std::fs::remove_file(&map_path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!("could not remove an earlier {}: {e}", map_path.display()))
            }
            _ => {}
        }
    } else {
        crate::sys::write_safely(&map_path, mapfile_text(opts.size, &both_kinds(&o.unreadable, &o.stepped_over), &opts.disk).as_bytes())
            .map_err(|e| format!("could not write {}: {e}", map_path.display()))?;
    }
    crate::sys::write_safely(&sha_path, sha_line(&o.sha256).as_bytes())
        .map_err(|e| format!("could not write {}: {e}", sha_path.display()))?;
    let (bad_n, bad_b) = (o.unreadable.len() as u64, o.unreadable.iter().map(|r| r.1).sum::<u64>());
    let (over_n, over_b) = (o.stepped_over.len() as u64, o.stepped_over.iter().map(|r| r.1).sum::<u64>());
    let info = format!(
        "Image of {} made by {} {} on {}.\r\nSize: {} bytes. SHA-256: {}\r\nPlaces the SSD could not read: {} ({} bytes); \
         stepped over without being tried: {} ({} bytes){}.\r\n",
        src.describe(),
        crate::APP,
        crate::VERSION,
        crate::fmt::local_datetime(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as f64).unwrap_or(0.0)
        ),
        commas(opts.size),
        o.sha256,
        commas(bad_n),
        commas(bad_b),
        commas(over_n),
        commas(over_b),
        if bad_n + over_n == 0 { "" } else { " - listed in the .ssd-read-errors.log beside it, and left as zeros" }
    );
    let _ = crate::sys::write_safely(&info_path, info.as_bytes());
    std::fs::rename(&partial, &final_path).map_err(|e| format!("could not give the image its name: {e}"))?;
    let _ = std::fs::remove_file(&errs_path);
    let _ = std::fs::remove_file(&safe_path);
    // (The SSD is needed again if this check fails: it is not "done" yet.)
    progress(
        "All of the SSD is read. Now the image is read back from the drive and checked, which takes about as long: \
         keep the SSD plugged in until this step says Done.",
    );
    let back = crate::sha256::file_hex_from_drive(
        &final_path,
        &mut |d, t| {
            crate::live::drive_check(d, t);
            amount(d, t)
        },
        &stop,
    )
    .map_err(|e| format!("could not read the image back: {e}"))?;
    match back {
        Some((_, b)) => o.verified = b == o.sha256,
        None => o.stopped = true,
    }
    if o.verified {
        make_read_only(&final_path);
    }
    Ok(o)
}

/// What to say about it.
pub fn summary(o: &Outcome) -> Vec<String> {
    let mut t = Vec::new();
    if let Some(e) = &o.disk_lost {
        t.push(format!(
            "The recovered SSD stopped answering{} (Windows said: {e}), so the image was stopped. Nothing is lost: what \
             is made so far is kept. Check its cable is in firmly. If it came out, plug it back in (it arrives switched \
             off), choose it again in step 3, click Protect, and start the image again: it carries on where it stopped, \
             and reads that place 4 KB at a time. If it stops answering there again, the next start steps over that \
             place, so the image always gets past it.",
            o.lost_at.map(|p| format!(" at byte {}", commas(p))).unwrap_or_default()
        ));
        return t;
    }
    if o.stopped {
        t.push("Stopped before the end. Start it again to carry on: what is made so far is kept.".to_string());
        return t;
    }
    if o.verified {
        t.push(format!(
            "{} the image {} ({} bytes), and it reads back exactly as it was written.",
            if o.reused { "Checked" } else { "Made" },
            o.path,
            commas(o.size)
        ));
    } else {
        t.push(format!(
            "The image {} does NOT read back the same as it was written{}. The drive it is on may have a problem: ask for \
             help before using it. Keep the recovered SSD: it is still needed.",
            o.path,
            if o.reused { " (or its checksum file does not belong to it)" } else { "" }
        ));
    }
    t.push(format!("Its SHA-256 is {} (saved beside it in {IMAGE_NAME}.sha256).", o.sha256));
    if o.unreadable.is_empty() && o.stepped_over.is_empty() {
        t.push("The SSD gave every byte.".to_string());
    } else {
        let places = |n: usize| if n == 1 { "1 place".to_string() } else { format!("{} places", commas(n as u64)) };
        let (bad, over) = (o.unreadable.len(), o.stepped_over.len());
        let (bad_b, over_b): (u64, u64) = (o.unreadable.iter().map(|r| r.1).sum(), o.stepped_over.iter().map(|r| r.1).sum());
        let what = match (bad, over) {
            (_, 0) => format!("The SSD could not read {} ({})", places(bad), size(bad_b)),
            (0, _) => format!(
                "{} ({}) {} stepped over without being read, to get the image past where the SSD stopped answering",
                places(over),
                size(over_b),
                if over == 1 { "was" } else { "were" }
            ),
            _ => format!(
                "The SSD could not read {} ({}), and {} around {} ({}) {} stepped over without being read, to spare the SSD",
                places(bad),
                size(bad_b),
                places(over),
                if bad == 1 { "it" } else { "them" },
                size(over_b),
                if over == 1 { "was" } else { "were" }
            ),
        };
        t.push(format!(
            "{what}. They are zeros in the image, listed in {IMAGE_NAME}{READ_ERRORS_SUFFIX} beside it, and the map check \
             counts them as never read. Tell Claude before going on: the 4 TB copy may still hold them."
        ));
    }
    if !o.reused {
        t.push(format!("Reading the SSD took {}.", duration(o.read_seconds)));
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::{Memory, Source};
    use std::io;

    /// A disk with a bad spot that never reads.
    struct BadSpot<'a> {
        inner: &'a dyn Source,
        bad: (u64, u64),
    }
    impl Source for BadSpot<'_> {
        fn size(&self) -> u64 {
            self.inner.size()
        }
        fn read_at(&self, off: u64, len: usize) -> io::Result<Vec<u8>> {
            if off < self.bad.1 && off + len as u64 > self.bad.0 {
                return Err(io::Error::other("unreadable"));
            }
            self.inner.read_at(off, len)
        }
        fn describe(&self) -> String {
            "a test disk".to_string()
        }
    }

    fn data(n: usize) -> Vec<u8> {
        (0..n as u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 11) as u8 | 1).collect()
    }

    /// A disk like the copy ddrescue made: `n` bytes of data, starting with
    /// a partition table made for a drive of `made_for` bytes (the old one).
    fn copy_disk(n: usize, made_for: u64) -> Vec<u8> {
        let mut d = data(n);
        d[512..34 * 512].fill(0);
        let h = &mut d[512..1024];
        h[0..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&92u32.to_le_bytes());
        h[24..32].copy_from_slice(&1u64.to_le_bytes());
        h[32..40].copy_from_slice(&(made_for / 512 - 1).to_le_bytes());
        h[72..80].copy_from_slice(&2u64.to_le_bytes());
        h[80..84].copy_from_slice(&128u32.to_le_bytes());
        h[84..88].copy_from_slice(&128u32.to_le_bytes());
        let c = crate::gpt::crc32(&h[..92]);
        h[16..20].copy_from_slice(&c.to_le_bytes());
        d
    }

    fn hex(b: &[u8]) -> String {
        let mut s = Sha256::new();
        s.update(b);
        s.finish_hex()
    }

    /// The test's folder, removed whole (the image in it is read-only).
    fn clear(dir: &Path) {
        if let Ok(m) = std::fs::metadata(dir.join(IMAGE_NAME)) {
            let mut p = m.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            p.set_readonly(false);
            let _ = std::fs::set_permissions(dir.join(IMAGE_NAME), p);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_hot_ssd_is_left_to_cool() {
        // Paused from the start (the SSD too warm): nothing is read until the
        // pause is lifted, then the image is made as usual.
        let dir = std::env::temp_dir().join(format!("rp-image-cool-{}", std::process::id()));
        clear(&dir);
        let d = copy_disk(13 << 20, 12 << 20);
        let pause = Arc::new(AtomicBool::new(true));
        let lift = pause.clone();
        let t = Instant::now();
        let lifter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(800));
            lift.store(false, Ordering::Relaxed);
        });
        let opts = Options { disk: "test".into(), out_dir: dir.display().to_string(), size: 12 << 20, stop: None, pause: Some(pause) };
        let mut said = Vec::new();
        let o = make(&Memory::new(d.clone()), &opts, &mut |s| said.push(s.to_string()), &mut |_, _| {}).unwrap();
        lifter.join().unwrap();
        assert!(t.elapsed() >= Duration::from_millis(700), "it waited");
        assert!(o.verified && std::fs::read(dir.join(IMAGE_NAME)).unwrap() == d[..12 << 20], "{o:?}");
        assert!(said.iter().any(|s| s.starts_with("Paused while the SSD cools down")), "{said:?}");
        assert!(said.iter().any(|s| s.starts_with("The SSD has cooled down")), "{said:?}");
        clear(&dir);
    }

    #[test]
    fn only_the_copy_is_imaged() {
        let dir = std::env::temp_dir().join(format!("rp-image-other-{}", std::process::id()));
        clear(&dir);
        // A drive exactly as big as the old one, set up for itself (SPARE):
        // its partition table is made for that size too, but it is no bigger
        // and holds no exFAT volume.
        let spare = Memory::new(copy_disk(12 << 20, 12 << 20));
        let opts = Options { disk: "test".into(), out_dir: dir.display().to_string(), size: 12 << 20, stop: None, pause: None };
        let e = make(&spare, &opts, &mut |_| {}, &mut |_, _| {}).unwrap_err();
        assert!(e.contains("not the copy ddrescue made") && e.contains("another drive of the same size"), "{e}");
        assert!(!dir.exists(), "nothing was written");
        // A drive whose table was made for another size.
        let other = Memory::new(copy_disk(20 << 20, 16 << 20));
        let e = make(&other, &opts, &mut |_| {}, &mut |_, _| {}).unwrap_err();
        assert!(e.contains("made for a 16,777,216-byte drive"), "{e}");
        // The copy is imaged; another copy-like disk then finds that image is not its own.
        let d = copy_disk(13 << 20, 12 << 20);
        let o = make(&Memory::new(d.clone()), &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(o.verified, "{o:?}");
        let mut d2 = d.clone();
        d2[40_000] ^= 0xFF;
        let e = make(&Memory::new(d2.clone()), &opts, &mut |_| {}, &mut |_, _| {}).unwrap_err();
        assert!(e.contains("is already there, but it is not an image of"), "{e}");
        // Nor is an unfinished image of another disk carried on.
        clear(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{IMAGE_NAME}.partial")), &d[..5 << 20]).unwrap();
        std::fs::write(dir.join(format!("{IMAGE_NAME}{SAFE_SUFFIX}")), "5242880\r\n").unwrap();
        let e = make(&Memory::new(d2), &opts, &mut |_| {}, &mut |_, _| {}).unwrap_err();
        assert!(e.contains("holds an unfinished image, but not one of"), "{e}");
        let o = make(&Memory::new(d.clone()), &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(o.verified && std::fs::read(dir.join(IMAGE_NAME)).unwrap() == d[..12 << 20], "{o:?}");
        clear(&dir);
    }

    #[test]
    fn the_checksum_and_the_list_are_there_before_the_image_is() {
        let dir = std::env::temp_dir().join(format!("rp-image-order-{}", std::process::id()));
        clear(&dir);
        let d = copy_disk(13 << 20, 12 << 20);
        let mem = Memory::new(d.clone());
        let opts = Options { disk: "test".into(), out_dir: dir.display().to_string(), size: 12 << 20, stop: None, pause: None };
        // An earlier run's list of unreadable places, left by a run that
        // stopped just before naming its image: this run reads every byte.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{IMAGE_NAME}{READ_ERRORS_SUFFIX}")), "stale").unwrap();
        // Watched as it goes: when the read-back begins, the image has its
        // name, its checksum is beside it and the stale list is gone.
        let seen = std::cell::RefCell::new(None);
        let o = make(
            &mem,
            &opts,
            &mut |line: &str| {
                if line.contains("read back from the drive") {
                    *seen.borrow_mut() = Some((
                        dir.join(IMAGE_NAME).is_file(),
                        recorded_sha(&dir.join(format!("{IMAGE_NAME}.sha256"))),
                        dir.join(format!("{IMAGE_NAME}{READ_ERRORS_SUFFIX}")).exists(),
                        dir.join(format!("{IMAGE_NAME}.partial")).exists(),
                    ));
                }
            },
            &mut |_, _| {},
        )
        .unwrap();
        assert!(o.verified, "{o:?}");
        assert_eq!(seen.into_inner(), Some((true, Some(hex(&d[..12 << 20])), false, false)));
        // Checked, the image is read-only: nothing can change it by accident.
        assert!(std::fs::metadata(dir.join(IMAGE_NAME)).unwrap().permissions().readonly());
        clear(&dir);
    }

    #[test]
    fn an_image_is_exact_checked_and_carries_on_after_a_stop() {
        let dir = std::env::temp_dir().join(format!("rp-image-{}", std::process::id()));
        clear(&dir);
        let want = 18 << 20; // less than the whole disk, like the 1 TB copy on a 2 TB SSD
        let d = copy_disk(20 << 20, want);
        let mem = Memory::new(d.clone());
        // Stopped part-way (as soon as the first piece is read), then
        // started again.
        let stop = Arc::new(AtomicBool::new(false));
        struct StopWhenRead<'a> {
            inner: &'a dyn Source,
            stop: Arc<AtomicBool>,
        }
        impl Source for StopWhenRead<'_> {
            fn size(&self) -> u64 {
                self.inner.size()
            }
            fn read_at(&self, off: u64, len: usize) -> io::Result<Vec<u8>> {
                self.stop.store(true, Ordering::Relaxed);
                self.inner.read_at(off, len)
            }
            fn describe(&self) -> String {
                "a test disk".to_string()
            }
        }
        let opts = Options { disk: "test".into(), out_dir: dir.display().to_string(), size: want, stop: Some(stop.clone()), pause: None };
        let o = make(&StopWhenRead { inner: &mem, stop: stop.clone() }, &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(o.stopped, "{o:?}");
        let part = std::fs::metadata(dir.join(format!("{IMAGE_NAME}.partial"))).unwrap().len();
        assert!(part > 0 && part < want, "{part}");
        // Stopped: all of it was made safe on the drive, and noted, with how
        // far the checksum had got.
        let note = read_note(&dir.join(format!("{IMAGE_NAME}{SAFE_SUFFIX}")));
        assert_eq!(note.at, part);
        assert_eq!(note.sha.as_ref().map(|s| s.total()), Some(part));
        stop.store(false, Ordering::Relaxed);
        let o = make(&mem, &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(o.verified && !o.stopped && o.unreadable.is_empty(), "{o:?}");
        let img = std::fs::read(dir.join(IMAGE_NAME)).unwrap();
        assert_eq!(img, d[..want as usize]);
        assert_eq!(o.sha256, hex(&d[..want as usize]));
        assert_eq!(std::fs::read_to_string(dir.join(format!("{IMAGE_NAME}.sha256"))).unwrap(), sha_line(&o.sha256));
        // Asked again: only checked.
        let o2 = make(&mem, &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(o2.reused && o2.verified, "{o2:?}");
        // Changed on the drive: the check says so.
        let img_path = dir.join(IMAGE_NAME);
        let mut p = std::fs::metadata(&img_path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(false);
        std::fs::set_permissions(&img_path, p).unwrap();
        let mut f = std::fs::OpenOptions::new().write(true).open(&img_path).unwrap();
        f.seek(SeekFrom::Start(5 << 20)).unwrap();
        f.write_all(b"x").unwrap();
        drop(f);
        let o3 = make(&mem, &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(o3.reused && !o3.verified, "{o3:?}");
        clear(&dir);
    }

    #[test]
    fn after_a_crash_it_carries_on_only_from_the_safe_point() {
        let dir = std::env::temp_dir().join(format!("rp-image-crash-{}", std::process::id()));
        let want = 18u64 << 20;
        let d = copy_disk(20 << 20, want);
        let mem = Memory::new(d.clone());
        let opts = Options { disk: "test".into(), out_dir: dir.display().to_string(), size: want, stop: None, pause: None };
        let partial = dir.join(format!("{IMAGE_NAME}.partial"));
        let safe = dir.join(format!("{IMAGE_NAME}{SAFE_SUFFIX}"));
        // What a power cut can leave: the right data up to some point, then
        // zeros where the last-written data never reached the drive.
        let crashed = |safe_note: Option<&str>| {
            clear(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let mut left = d[..5 << 20].to_vec();
            left.extend(std::iter::repeat_n(0u8, 6 << 20));
            std::fs::write(&partial, &left).unwrap();
            if let Some(n) = safe_note {
                std::fs::write(&safe, n).unwrap();
            }
        };
        // The checksum's state at 4 MiB, as a run that stopped there notes it.
        let mut at4 = Sha256::new();
        at4.update(&d[..4 << 20]);
        let with_state = note_text(4 << 20, &at4, None);
        for note in [Some("5242880\r\n"), Some("3145728"), None, Some("999999999999"), Some("not a number"), Some(with_state.as_str())] {
            crashed(note);
            let o = make(&mem, &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
            assert!(o.verified && !o.stopped, "{note:?}: {o:?}");
            let img = std::fs::read(dir.join(IMAGE_NAME)).unwrap();
            assert!(img == d[..want as usize], "{note:?}: the image is not the disk");
            assert_eq!(o.sha256, hex(&d[..want as usize]), "{note:?}");
            assert!(!safe.exists() && !partial.exists(), "{note:?}");
        }
        clear(&dir);
    }

    #[test]
    fn a_place_the_ssd_cannot_read_is_zeros_and_recorded() {
        let dir = std::env::temp_dir().join(format!("rp-image-bad-{}", std::process::id()));
        clear(&dir);
        let d = copy_disk(13 << 20, 12 << 20);
        let mem = Memory::new(d.clone());
        let bad = BadSpot { inner: &mem, bad: (9 << 20, (9 << 20) + 5000) };
        let opts = Options { disk: "test".into(), out_dir: dir.display().to_string(), size: 12 << 20, stop: None, pause: None };
        let o = make(&bad, &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(o.verified, "{o:?}");
        // Whole 4 KB units around the bad spot: 9 MiB .. 9 MiB + 8 KiB.
        assert_eq!(o.unreadable, vec![(9 << 20, 8192)]);
        assert!(o.stepped_over.is_empty(), "{o:?}");
        let img = std::fs::read(dir.join(IMAGE_NAME)).unwrap();
        let mut want = d[..12 << 20].to_vec();
        want[9 << 20..(9 << 20) + 8192].fill(0);
        assert_eq!(img, want);
        let map = std::fs::read_to_string(dir.join(format!("{IMAGE_NAME}{READ_ERRORS_SUFFIX}"))).unwrap();
        let m = crate::mapfile::parse(&map).unwrap();
        let areas: Vec<(u64, u64)> = m.bad_areas().iter().map(|a| (a.start, a.len())).collect();
        assert_eq!(areas, vec![(9 << 20, 8192)]);
        assert_eq!(m.domain_end, 12 << 20);
        clear(&dir);
    }

    /// A weak stretch: every read touching it fails - slowly, as a drive
    /// retrying inside itself does - and the disk still answers elsewhere.
    /// `every`: only every so many 4 KB pieces in it fail (1: all of them).
    struct Weak<'a> {
        inner: &'a dyn Source,
        bad: (u64, u64),
        every: u64,
        delay: Duration,
        failed: AtomicU64,
        reads: AtomicU64,
    }
    impl Source for Weak<'_> {
        fn size(&self) -> u64 {
            self.inner.size()
        }
        fn read_at(&self, off: u64, len: usize) -> io::Result<Vec<u8>> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            let end = off + len as u64;
            let hit = (off.max(self.bad.0)..end.min(self.bad.1)).step_by(UNIT).any(|p| (p - self.bad.0) / UNIT as u64 % self.every == 0)
                && off < self.bad.1
                && end > self.bad.0;
            if hit {
                self.failed.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(self.delay);
                return Err(io::Error::other("unrecovered read error"));
            }
            self.inner.read_at(off, len)
        }
        fn describe(&self) -> String {
            "a test disk with a weak stretch".to_string()
        }
    }

    #[test]
    fn a_long_weak_stretch_is_stepped_over_not_worn_down() {
        let dir = std::env::temp_dir().join(format!("rp-image-weak-{}", std::process::id()));
        clear(&dir);
        let want = 32u64 << 20;
        let d = copy_disk(40 << 20, want);
        let mem = Memory::new(d.clone());
        // 1 MiB that will not read at all, starting 1 MiB into the third piece.
        let bad_at = (16u64 << 20) + (1 << 20);
        let weak = Weak {
            inner: &mem,
            bad: (bad_at, bad_at + (1 << 20)),
            every: 1,
            delay: Duration::ZERO,
            failed: AtomicU64::new(0),
            reads: AtomicU64::new(0),
        };
        let opts = Options { disk: "test".into(), out_dir: dir.display().to_string(), size: want, stop: None, pause: None };
        let mut lines = Vec::new();
        let o = make(&weak, &opts, &mut |l| lines.push(l.to_string()), &mut |_, _| {}).unwrap();
        assert!(o.verified && !o.stopped, "{o:?}");
        // 16 pieces of 4 KB in a row would not read; the rest of the piece is
        // stepped over without being tried, and the next pieces read as usual.
        assert_eq!(o.unreadable, vec![(bad_at, 16 * 4096)], "{o:?}");
        let piece_end = 24u64 << 20;
        assert_eq!(o.stepped_over, vec![(bad_at + 16 * 4096, piece_end - bad_at - 16 * 4096)], "{o:?}");
        // The whole piece once, then 16 tries of the bad stretch: not 256.
        assert_eq!(weak.failed.load(Ordering::Relaxed), 1 + 16);
        let img = std::fs::read(dir.join(IMAGE_NAME)).unwrap();
        let mut expect = d[..want as usize].to_vec();
        expect[bad_at as usize..piece_end as usize].fill(0);
        assert!(img == expect, "the image holds everything else");
        assert_eq!(o.sha256, hex(&expect));
        // The list beside it tells the two kinds apart, and the map check
        // counts both as never read.
        let map = std::fs::read_to_string(dir.join(format!("{IMAGE_NAME}{READ_ERRORS_SUFFIX}"))).unwrap();
        let m = crate::mapfile::parse(&map).unwrap();
        let kinds: Vec<(u64, u64, char)> = m.bad_areas().iter().map(|a| (a.start, a.len(), a.status)).collect();
        assert_eq!(kinds, vec![(bad_at, 16 * 4096, '-'), (bad_at + 16 * 4096, piece_end - bad_at - 16 * 4096, '?')]);
        assert!(lines.iter().any(|l| l.contains("stepped over without being tried")), "{lines:?}");
        let s = summary(&o).join(" ");
        assert!(s.contains("could not read 1 place (64 KB), and 1 place around it") && s.contains("4 TB copy"), "{s}");
        // Checked again later: both kinds are read back from the list.
        let c = check_existing(&dir.join(IMAGE_NAME), &mut |_| {}, &mut |_, _| {}, &|| false).unwrap();
        assert!(c.verified && c.unreadable == o.unreadable && c.stepped_over == o.stepped_over, "{c:?}");
        clear(&dir);
    }

    #[test]
    fn stop_works_at_once_while_a_weak_piece_is_read_slowly() {
        let dir = std::env::temp_dir().join(format!("rp-image-stopweak-{}", std::process::id()));
        clear(&dir);
        let want = 32u64 << 20;
        let d = copy_disk(40 << 20, want);
        let mem = Memory::new(d);
        // Every other 4 KB piece of the second 8 MB piece fails, slowly: never
        // 16 in a row, so only the limit per piece (64 of them, about 4 s
        // here; hours on a real weak drive) would end it.
        let weak = Weak {
            inner: &mem,
            bad: (8 << 20, 16 << 20),
            every: 2,
            delay: Duration::from_millis(60),
            failed: AtomicU64::new(0),
            reads: AtomicU64::new(0),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let opts = Options { disk: "test".into(), out_dir: dir.display().to_string(), size: want, stop: Some(stop.clone()), pause: None };
        let t = Instant::now();
        let mut lines: Vec<String> = Vec::new();
        let o = std::thread::scope(|s| {
            let stop2 = stop.clone();
            s.spawn(move || {
                std::thread::sleep(Duration::from_millis(900));
                stop2.store(true, Ordering::Relaxed);
            });
            make(&weak, &opts, &mut |l| lines.push(l.to_string()), &mut |_, _| {}).unwrap()
        });
        let took = t.elapsed();
        assert!(o.stopped, "{o:?}");
        // Without Stop, the piece would take 64 failed reads (about 4 s here).
        assert!(took < Duration::from_millis(3000), "Stop took {took:?}");
        assert!(weak.failed.load(Ordering::Relaxed) < 45, "{} failed reads before it stopped", weak.failed.load(Ordering::Relaxed));
        // Meanwhile the page was told how it was getting on.
        assert!(lines.iter().any(|l| l.contains("Reading 4 KB at a time around a place the SSD cannot read")), "{lines:?}");
        assert!(summary(&o).join(" ").contains("Stopped before the end"));
        // Nothing of the piece being read carefully was kept: the safe point
        // is the start of that piece, and the next run reads it again.
        let note = read_note(&dir.join(format!("{IMAGE_NAME}{SAFE_SUFFIX}")));
        assert_eq!(note.at, 8 << 20);
        clear(&dir);
    }

    /// A place that makes the USB case reset: the read fails and the disk
    /// stops answering until it is plugged in again (a new Source).
    struct Resets<'a> {
        inner: &'a dyn Source,
        bad: (u64, u64),
        gone: AtomicBool,
    }
    impl Source for Resets<'_> {
        fn size(&self) -> u64 {
            self.inner.size()
        }
        fn read_at(&self, off: u64, len: usize) -> io::Result<Vec<u8>> {
            if self.gone.load(Ordering::Relaxed) || (off < self.bad.1 && off + len as u64 > self.bad.0) {
                self.gone.store(true, Ordering::Relaxed);
                return Err(io::Error::other("the device is not connected"));
            }
            self.inner.read_at(off, len)
        }
        fn describe(&self) -> String {
            "a test disk whose case resets at one place".to_string()
        }
    }

    #[test]
    fn a_place_that_resets_the_case_is_got_past() {
        let dir = std::env::temp_dir().join(format!("rp-image-reset-{}", std::process::id()));
        clear(&dir);
        let want = 48u64 << 20;
        let d = copy_disk(64 << 20, want);
        let mem = Memory::new(d.clone());
        let opts = Options { disk: "test".into(), out_dir: dir.display().to_string(), size: want, stop: None, pause: None };
        let bad_at = (20u64 << 20) + 3 * 4096;
        let mut runs = Vec::new();
        let mut done = None;
        for _ in 0..5 {
            // Plugged in again, protected, "Make the image" again.
            let src = Resets { inner: &mem, bad: (bad_at, bad_at + 4096), gone: AtomicBool::new(false) };
            let o = make(&src, &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
            runs.push((o.disk_lost.is_some(), o.lost_at));
            if o.verified {
                done = Some(o);
                break;
            }
        }
        // First the whole 8 MB piece makes it stop answering; then, read 4 KB
        // at a time, the place itself; the third run steps over the rest of
        // that piece from there and finishes.
        assert_eq!(runs, vec![(true, Some(16 << 20)), (true, Some(bad_at)), (false, None)], "{runs:?}");
        let o = done.unwrap();
        assert!(o.unreadable.is_empty(), "{o:?}");
        let s = summary(&o).join(" ");
        assert!(s.contains("1 place (") && s.contains("was stepped over without being read, to get the image past"), "{s}");
        assert_eq!(o.stepped_over, vec![(bad_at, (24 << 20) - bad_at)]);
        let img = std::fs::read(dir.join(IMAGE_NAME)).unwrap();
        let mut expect = d[..want as usize].to_vec();
        expect[bad_at as usize..24 << 20].fill(0);
        assert!(img == expect, "everything but the place stepped over is in the image");
        clear(&dir);
    }

    #[test]
    fn a_stopped_image_is_still_checked_against_the_ssd_bytes() {
        // Made in two runs, with the part made in the first changed on the
        // PC's drive in between (as a failing drive might): the checksum came
        // from the SSD's bytes all along, so the read-back check catches it.
        let dir = std::env::temp_dir().join(format!("rp-image-twice-{}", std::process::id()));
        clear(&dir);
        let want = 24u64 << 20;
        let d = copy_disk(32 << 20, want);
        let mem = Memory::new(d.clone());
        let stop = Arc::new(AtomicBool::new(false));
        struct StopAfter<'a> {
            inner: &'a dyn Source,
            stop: Arc<AtomicBool>,
            reads: AtomicU64,
        }
        impl Source for StopAfter<'_> {
            fn size(&self) -> u64 {
                self.inner.size()
            }
            fn read_at(&self, off: u64, len: usize) -> io::Result<Vec<u8>> {
                if self.reads.fetch_add(1, Ordering::Relaxed) >= 2 {
                    self.stop.store(true, Ordering::Relaxed);
                }
                self.inner.read_at(off, len)
            }
            fn describe(&self) -> String {
                "a test disk".to_string()
            }
        }
        let opts = Options { disk: "test".into(), out_dir: dir.display().to_string(), size: want, stop: Some(stop.clone()), pause: None };
        let o = make(&StopAfter { inner: &mem, stop: stop.clone(), reads: AtomicU64::new(0) }, &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(o.stopped);
        let partial = dir.join(format!("{IMAGE_NAME}.partial"));
        let mut f = std::fs::OpenOptions::new().write(true).open(&partial).unwrap();
        f.seek(SeekFrom::Start(3 << 20)).unwrap();
        f.write_all(b"changed on the PC's drive").unwrap();
        drop(f);
        stop.store(false, Ordering::Relaxed);
        let o = make(&mem, &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(!o.stopped && !o.verified, "the change is caught: {o:?}");
        assert_eq!(o.sha256, hex(&d[..want as usize]), "the checksum is the SSD's");
        clear(&dir);
    }
}
