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
//! Carrying on is only done from a point known to be safe on the drive:
//! every few gigabytes the image (and the list of places the SSD could not
//! read) is made safe on the drive and that point is noted beside it. After
//! a crash or a power cut a file can come back the right length but with
//! zeros where its last-written part should be (Windows writes the data
//! later than it notes the size); the part after the noted point is read
//! from the SSD again, so no such zeros can end up in the image unnoticed.
//! The image is checked by reading it back from the drive itself, not from
//! what Windows still holds of it in memory.
//!
//! Only the copy ddrescue made is ever imaged (mapcheck::is_the_copy): not
//! another drive that happens to be the old drive's size. An image already
//! in the folder is only taken if it starts the way the disk does. Its
//! checksum and its list of places the SSD could not read are written, safe
//! on the drive, before the image gets its name: whenever
//! recovered-ssd.img is there, so are they.

use crate::disk::Source;
use crate::fmt::{commas, size};
use crate::sha256::Sha256;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const IMAGE_NAME: &str = "recovered-ssd.img";
/// Beside an image: the places the SSD itself could not read (if any).
pub const READ_ERRORS_SUFFIX: &str = ".ssd-read-errors.log";
const CHUNK: usize = 8 << 20;
const UNIT: usize = 4096;
/// How often what is made so far is made safe on the drive (see the top).
const SAFE_EVERY: u64 = 2 << 30;
/// Beside the image while it is made: how much of it is safe on the drive.
const SAFE_SUFFIX: &str = ".partial-safe.txt";
/// Stop rather than go on if the SSD cannot read this much: something is
/// wrong with it, and the person should ask before going on.
const TOO_MUCH_UNREADABLE: u64 = 64 << 20;

pub struct Options {
    /// The disk (\\.\PhysicalDriveN) or, for tests, an image file.
    pub disk: String,
    /// The folder the image goes into.
    pub out_dir: String,
    /// How many bytes to copy: the size of the disk the mapfile describes.
    pub size: u64,
    pub stop: Option<Arc<AtomicBool>>,
}

#[derive(Default, Debug)]
pub struct Outcome {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// (start, length) of the places the SSD would not give (zeros in the image).
    pub unreadable: Vec<(u64, u64)>,
    /// The image read back gives the same SHA-256.
    pub verified: bool,
    /// The image was already there: only checked.
    pub reused: bool,
    pub stopped: bool,
    pub disk_lost: Option<String>,
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

/// A ddrescue-style mapfile of [0, size) with `bad` marked '-'.
fn mapfile_text(size: u64, bad: &[(u64, u64)], disk: &str) -> String {
    let now = crate::fmt::local_datetime(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as f64).unwrap_or(0.0),
    );
    let mut t = format!(
        "# Mapfile. Created by {} {} (not GNU ddrescue): the places the recovered SSD itself could not read when its image was made\n\
         # Command line: recovery-probe image {disk}\n# Start time:   {now}\n# Current time: {now}\n# Finished\n\
         # current_pos  current_status  current_pass\n0x00000000     +               1\n#      pos        size  status\n",
        crate::APP,
        crate::VERSION
    );
    let mut pos = 0u64;
    for &(s, n) in bad {
        if s > pos {
            t.push_str(&format!("0x{pos:08X}  0x{:08X}  +\n", s - pos));
        }
        t.push_str(&format!("0x{s:08X}  0x{n:08X}  -\n"));
        pos = s + n;
    }
    if pos < size {
        t.push_str(&format!("0x{pos:08X}  0x{:08X}  +\n", size - pos));
    }
    t
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
    Data { data: Vec<u8>, bad: Vec<(u64, u64)> },
    Lost(String),
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
                o.unreadable = mf.bad_areas().iter().map(|a| (a.start, a.len())).collect();
            }
        }
    }
    Ok(o)
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
    let safe = std::fs::read_to_string(&safe_path)
        .ok()
        .and_then(|t| t.split_whitespace().next().and_then(|n| n.parse::<u64>().ok()))
        .unwrap_or(0);
    let keep = if safe <= have.min(opts.size) { safe } else { 0 };
    if keep == 0 {
        // Nothing known to be safe: start from the beginning.
        if have > 0 {
            f.set_len(0).map_err(|e| format!("could not use {}: {e}", partial.display()))?;
            progress("An earlier image was not finished, and none of it is known to be safe on the drive: starting again.");
        }
        let _ = std::fs::remove_file(&safe_path);
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
        progress(&format!(
            "Carrying on with the image a stopped run began ({} of {} done{}). Reading that part back first...",
            size(keep),
            size(opts.size),
            if have > keep { "; what was written after the last safe point is read from the SSD again" } else { "" }
        ));
        if !hash_file_part(&mut f, keep, &mut sha, &stop).map_err(|e| format!("could not read {}: {e}", partial.display()))? {
            o.stopped = true;
            return Ok(o);
        }
        if let Ok(t) = std::fs::read_to_string(&errs_path) {
            for line in t.lines() {
                let mut it = line.split_whitespace().filter_map(|x| x.parse::<u64>().ok());
                if let (Some(s), Some(n)) = (it.next(), it.next()) {
                    if s < keep {
                        o.unreadable.push((s, n.min(keep - s)));
                    }
                }
            }
        }
        // The list keeps only the places before the safe point: the rest is
        // read again.
        let kept: String = o.unreadable.iter().map(|(s, n)| format!("{s} {n}\n")).collect();
        crate::sys::write_safely(&errs_path, kept.as_bytes()).map_err(|e| format!("could not write {}: {e}", errs_path.display()))?;
        start = keep;
    } else {
        let _ = std::fs::remove_file(&errs_path);
    }
    // Make what is written so far safe on the drive, and note how far that is.
    let make_safe = |f: &std::fs::File, errs: &std::fs::File, at: u64| -> bool {
        errs.sync_all().is_ok()
            && f.sync_all().is_ok()
            && crate::sys::write_safely(&safe_path, format!("{at}\r\n").as_bytes()).is_ok()
    };
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
    amount(start, opts.size);

    // Reading and writing overlap: one thread reads the SSD, this one hashes
    // and writes.
    let watched = crate::disk::Watched::new(src);
    let quit = AtomicBool::new(false);
    let t0 = std::time::Instant::now();
    let mut pos = start;
    let mut safe_at = start;
    let mut unreadable_total: u64 = o.unreadable.iter().map(|r| r.1).sum();
    let mut write_error: Option<String> = None;
    let mut too_much = false;
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Piece>(3);
        let watched = &watched;
        let quit = &quit;
        let size_all = opts.size;
        scope.spawn(move || {
            let mut at = start;
            while at < size_all && !quit.load(Ordering::Relaxed) {
                let n = (size_all - at).min(CHUNK as u64) as usize;
                let piece = match watched.read_at(at, n) {
                    Ok(d) if d.len() == n => Piece::Data { data: d, bad: Vec::new() },
                    _ if !watched.answers(0) => Piece::Lost(watched.last_error()),
                    _ => {
                        // A bad spot: read it again in small pieces, and leave
                        // what will not read as zeros.
                        let mut data = vec![0u8; n];
                        let mut bad: Vec<(u64, u64)> = Vec::new();
                        let mut k = 0;
                        while k < n {
                            let m = UNIT.min(n - k);
                            match watched.read_at(at + k as u64, m) {
                                Ok(d) if d.len() == m => data[k..k + m].copy_from_slice(&d),
                                _ => match bad.last_mut() {
                                    Some(b) if b.0 + b.1 == at + k as u64 => b.1 += m as u64,
                                    _ => bad.push((at + k as u64, m as u64)),
                                },
                            }
                            k += m;
                        }
                        if !watched.answers(0) {
                            Piece::Lost(watched.last_error())
                        } else {
                            Piece::Data { data, bad }
                        }
                    }
                };
                let lost = matches!(piece, Piece::Lost(_));
                if tx.send(piece).is_err() || lost {
                    break;
                }
                at += n as u64;
            }
        });
        let mut last = std::time::Instant::now();
        for piece in rx {
            match piece {
                Piece::Lost(e) => {
                    o.disk_lost = Some(e);
                    break;
                }
                Piece::Data { data, bad } => {
                    if let Err(e) = f.write_all(&data) {
                        write_error = Some(e.to_string());
                        break;
                    }
                    sha.update(&data);
                    for &(s, n) in &bad {
                        let _ = writeln!(errs, "{s} {n}");
                        unreadable_total += n;
                    }
                    let _ = errs.flush();
                    o.unreadable.extend(bad);
                    pos += data.len() as u64;
                    if pos - safe_at >= SAFE_EVERY && pos < opts.size && make_safe(&f, &errs, pos) {
                        safe_at = pos;
                    }
                    if last.elapsed().as_millis() > 500 {
                        amount(pos, opts.size);
                        last = std::time::Instant::now();
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
    if write_error.is_none() && pos > safe_at && pos < opts.size {
        // Stopped part-way: so the next run carries on from here.
        let _ = make_safe(&f, &errs, pos);
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
    if o.disk_lost.is_some() || o.stopped || pos < opts.size {
        o.stopped = true;
        return Ok(o);
    }
    // All read: make it safe on the drive. Then everything that describes it
    // goes beside it, safe on the drive too, and only then does it get its
    // name - so recovered-ssd.img is never there without its checksum and
    // its list of places the SSD could not read (a power cut in between
    // leaves the .partial, which the next run finishes).
    f.sync_all().map_err(|e| format!("could not finish writing the image: {e}"))?;
    drop(f);
    merge(&mut o.unreadable);
    o.sha256 = sha.finish_hex();
    if o.unreadable.is_empty() {
        match std::fs::remove_file(&map_path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!("could not remove an earlier {}: {e}", map_path.display()))
            }
            _ => {}
        }
    } else {
        crate::sys::write_safely(&map_path, mapfile_text(opts.size, &o.unreadable, &opts.disk).as_bytes())
            .map_err(|e| format!("could not write {}: {e}", map_path.display()))?;
    }
    crate::sys::write_safely(&sha_path, sha_line(&o.sha256).as_bytes())
        .map_err(|e| format!("could not write {}: {e}", sha_path.display()))?;
    let info = format!(
        "Image of {} made by {} {} on {}.\r\nSize: {} bytes. SHA-256: {}\r\nPlaces the SSD could not read: {} ({} bytes){}.\r\n",
        src.describe(),
        crate::APP,
        crate::VERSION,
        crate::fmt::local_datetime(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as f64).unwrap_or(0.0)
        ),
        commas(opts.size),
        o.sha256,
        commas(o.unreadable.len() as u64),
        commas(o.unreadable.iter().map(|r| r.1).sum()),
        if o.unreadable.is_empty() { "" } else { " - listed in the .ssd-read-errors.log beside it, and left as zeros" }
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
    let back = crate::sha256::file_hex_from_drive(&final_path, amount, &stop)
        .map_err(|e| format!("could not read the image back: {e}"))?;
    match back {
        Some((_, b)) => o.verified = b == o.sha256,
        None => o.stopped = true,
    }
    Ok(o)
}

/// What to say about it.
pub fn summary(o: &Outcome) -> Vec<String> {
    let mut t = Vec::new();
    if let Some(e) = &o.disk_lost {
        t.push(format!(
            "The recovered SSD stopped answering (Windows said: {e}), so the image was stopped. Nothing is lost: what is made \
             so far is kept. Check its cable is in firmly. If it came out, plug it back in (it arrives switched off), choose \
             it again in step 3, click Protect, and start the image again: it carries on where it stopped."
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
    if o.unreadable.is_empty() {
        t.push("The SSD gave every byte.".to_string());
    } else {
        t.push(format!(
            "The SSD could not read {} places ({}): they are zeros in the image, and the map check counts them as never read.",
            commas(o.unreadable.len() as u64),
            size(o.unreadable.iter().map(|r| r.1).sum())
        ));
    }
    if !o.reused {
        t.push(format!("Reading the SSD took {}.", duration(o.read_seconds)));
    }
    t
}

fn duration(s: u64) -> String {
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min", s / 60)
    } else {
        format!("{} h {} min", s / 3600, (s % 3600) / 60)
    }
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

    #[test]
    fn only_the_copy_is_imaged() {
        let dir = std::env::temp_dir().join(format!("rp-image-other-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // A drive exactly as big as the old one, set up for itself (SPARE):
        // its partition table is made for that size too, but it is no bigger
        // and holds no exFAT volume.
        let spare = Memory::new(copy_disk(12 << 20, 12 << 20));
        let opts = Options { disk: "test".into(), out_dir: dir.display().to_string(), size: 12 << 20, stop: None };
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
        std::fs::remove_file(dir.join(IMAGE_NAME)).unwrap();
        std::fs::write(dir.join(format!("{IMAGE_NAME}.partial")), &d[..5 << 20]).unwrap();
        std::fs::write(dir.join(format!("{IMAGE_NAME}{SAFE_SUFFIX}")), "5242880\r\n").unwrap();
        let e = make(&Memory::new(d2), &opts, &mut |_| {}, &mut |_, _| {}).unwrap_err();
        assert!(e.contains("holds an unfinished image, but not one of"), "{e}");
        let o = make(&Memory::new(d.clone()), &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(o.verified && std::fs::read(dir.join(IMAGE_NAME)).unwrap() == d[..12 << 20], "{o:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_checksum_and_the_list_are_there_before_the_image_is() {
        let dir = std::env::temp_dir().join(format!("rp-image-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let d = copy_disk(13 << 20, 12 << 20);
        let mem = Memory::new(d.clone());
        let opts = Options { disk: "test".into(), out_dir: dir.display().to_string(), size: 12 << 20, stop: None };
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
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_image_is_exact_checked_and_carries_on_after_a_stop() {
        let dir = std::env::temp_dir().join(format!("rp-image-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
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
        let opts = Options { disk: "test".into(), out_dir: dir.display().to_string(), size: want, stop: Some(stop.clone()) };
        let o = make(&StopWhenRead { inner: &mem, stop: stop.clone() }, &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(o.stopped, "{o:?}");
        let part = std::fs::metadata(dir.join(format!("{IMAGE_NAME}.partial"))).unwrap().len();
        assert!(part > 0 && part < want, "{part}");
        // Stopped: all of it was made safe on the drive, and noted.
        let note = std::fs::read_to_string(dir.join(format!("{IMAGE_NAME}{SAFE_SUFFIX}"))).unwrap();
        assert_eq!(note.trim().parse::<u64>().unwrap(), part);
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
        let mut f = std::fs::OpenOptions::new().write(true).open(dir.join(IMAGE_NAME)).unwrap();
        f.seek(SeekFrom::Start(5 << 20)).unwrap();
        f.write_all(b"x").unwrap();
        drop(f);
        let o3 = make(&mem, &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(o3.reused && !o3.verified, "{o3:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn after_a_crash_it_carries_on_only_from_the_safe_point() {
        let dir = std::env::temp_dir().join(format!("rp-image-crash-{}", std::process::id()));
        let want = 18u64 << 20;
        let d = copy_disk(20 << 20, want);
        let mem = Memory::new(d.clone());
        let opts = Options { disk: "test".into(), out_dir: dir.display().to_string(), size: want, stop: None };
        let partial = dir.join(format!("{IMAGE_NAME}.partial"));
        let safe = dir.join(format!("{IMAGE_NAME}{SAFE_SUFFIX}"));
        // What a power cut can leave: the right data up to some point, then
        // zeros where the last-written data never reached the drive.
        let crashed = |safe_note: Option<&str>| {
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let mut left = d[..5 << 20].to_vec();
            left.extend(std::iter::repeat_n(0u8, 6 << 20));
            std::fs::write(&partial, &left).unwrap();
            if let Some(n) = safe_note {
                std::fs::write(&safe, n).unwrap();
            }
        };
        for note in [Some("5242880\r\n"), Some("3145728"), None, Some("999999999999"), Some("not a number")] {
            crashed(note);
            let o = make(&mem, &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
            assert!(o.verified && !o.stopped, "{note:?}: {o:?}");
            let img = std::fs::read(dir.join(IMAGE_NAME)).unwrap();
            assert!(img == d[..want as usize], "{note:?}: the image is not the disk");
            assert_eq!(o.sha256, hex(&d[..want as usize]), "{note:?}");
            assert!(!safe.exists() && !partial.exists(), "{note:?}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_place_the_ssd_cannot_read_is_zeros_and_recorded() {
        let dir = std::env::temp_dir().join(format!("rp-image-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let d = copy_disk(13 << 20, 12 << 20);
        let mem = Memory::new(d.clone());
        let bad = BadSpot { inner: &mem, bad: (9 << 20, (9 << 20) + 5000) };
        let opts = Options { disk: "test".into(), out_dir: dir.display().to_string(), size: 12 << 20, stop: None };
        let o = make(&bad, &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(o.verified, "{o:?}");
        // Whole 4 KB units around the bad spot: 9 MiB .. 9 MiB + 8 KiB.
        assert_eq!(o.unreadable, vec![(9 << 20, 8192)]);
        let img = std::fs::read(dir.join(IMAGE_NAME)).unwrap();
        let mut want = d[..12 << 20].to_vec();
        want[9 << 20..(9 << 20) + 8192].fill(0);
        assert_eq!(img, want);
        let map = std::fs::read_to_string(dir.join(format!("{IMAGE_NAME}{READ_ERRORS_SUFFIX}"))).unwrap();
        let m = crate::mapfile::parse(&map).unwrap();
        let areas: Vec<(u64, u64)> = m.bad_areas().iter().map(|a| (a.start, a.len())).collect();
        assert_eq!(areas, vec![(9 << 20, 8192)]);
        assert_eq!(m.domain_end, 12 << 20);
        std::fs::remove_dir_all(&dir).ok();
    }
}
