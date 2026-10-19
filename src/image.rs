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
    let mut f = std::fs::File::open(&image).map_err(|e| format!("could not open {shown}: {e}"))?;
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut o = Outcome { path: shown.clone(), size: len, reused: true, ..Default::default() };
    let mut sha = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let (mut done, mut last) = (0u64, std::time::Instant::now());
    amount(0, len);
    loop {
        if stop() {
            o.stopped = true;
            return Ok(o);
        }
        let n = f.read(&mut buf).map_err(|e| format!("could not read {shown}: {e}"))?;
        if n == 0 {
            break;
        }
        sha.update(&buf[..n]);
        done += n as u64;
        if last.elapsed().as_millis() > 300 {
            amount(done, len);
            progress(&format!("Checked {} of {}", size(done), size(len)));
            last = std::time::Instant::now();
        }
    }
    amount(done, len);
    o.sha256 = sha.finish_hex();
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
    let dir = crate::rescue::verbatim(Path::new(&opts.out_dir));
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", opts.out_dir))?;
    let final_path = dir.join(IMAGE_NAME);
    let partial = dir.join(format!("{IMAGE_NAME}.partial"));
    let errs_path = dir.join(format!("{IMAGE_NAME}.partial-unreadable.txt"));
    let sha_path = dir.join(format!("{IMAGE_NAME}.sha256"));
    let map_path = dir.join(format!("{IMAGE_NAME}{READ_ERRORS_SUFFIX}"));
    let shown = Path::new(&opts.out_dir).join(IMAGE_NAME).display().to_string();
    let mut o = Outcome { path: shown.clone(), size: opts.size, ..Default::default() };

    // Made before: check it against its SHA-256 (the SSD is not read).
    if final_path.exists() {
        progress(&format!("{shown} is already there: checking it against its checksum (the SSD is not needed for this)..."));
        let mut o = check_existing(&final_path, progress, amount, &stop)?;
        o.path = shown;
        o.verified &= o.size == opts.size;
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
    if have > 0 {
        let keep = have.min(opts.size) / (1 << 20) * (1 << 20);
        f.set_len(keep).map_err(|e| format!("could not use {}: {e}", partial.display()))?;
        progress(&format!(
            "Carrying on with the image a stopped run began ({} of {} done). Reading that part back first...",
            size(keep),
            size(opts.size)
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
        start = keep;
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
    amount(start, opts.size);

    // Reading and writing overlap: one thread reads the SSD, this one hashes
    // and writes.
    let watched = crate::disk::Watched::new(src);
    let quit = AtomicBool::new(false);
    let t0 = std::time::Instant::now();
    let mut pos = start;
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
    // All read: make it safe on the drive, give it its name and checksum.
    f.sync_all().map_err(|e| format!("could not finish writing the image: {e}"))?;
    drop(f);
    merge(&mut o.unreadable);
    std::fs::rename(&partial, &final_path).map_err(|e| format!("could not give the image its name: {e}"))?;
    o.sha256 = sha.finish_hex();
    std::fs::write(&sha_path, sha_line(&o.sha256)).map_err(|e| format!("could not write {}: {e}", sha_path.display()))?;
    if !o.unreadable.is_empty() {
        std::fs::write(&map_path, mapfile_text(opts.size, &o.unreadable, &opts.disk))
            .map_err(|e| format!("could not write {}: {e}", map_path.display()))?;
    }
    let _ = std::fs::remove_file(&errs_path);
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
    let _ = std::fs::write(dir.join(format!("{IMAGE_NAME}.txt")), info);
    progress("All of the SSD is read: it is not needed any more. Checking the image by reading it back...");
    let back = crate::sha256::file_hex(&final_path, &stop).map_err(|e| format!("could not read the image back: {e}"))?;
    match back {
        Some(b) => o.verified = b == o.sha256,
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
             help before using it.",
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

    fn hex(b: &[u8]) -> String {
        let mut s = Sha256::new();
        s.update(b);
        s.finish_hex()
    }

    #[test]
    fn an_image_is_exact_checked_and_carries_on_after_a_stop() {
        let dir = std::env::temp_dir().join(format!("rp-image-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let d = data(20 << 20);
        let mem = Memory::new(d.clone());
        let want = 18 << 20; // less than the whole disk, like the 1 TB copy on a 2 TB SSD
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
        f.seek(SeekFrom::Start(12345)).unwrap();
        f.write_all(b"x").unwrap();
        drop(f);
        let o3 = make(&mem, &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(o3.reused && !o3.verified, "{o3:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_place_the_ssd_cannot_read_is_zeros_and_recorded() {
        let dir = std::env::temp_dir().join(format!("rp-image-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let d = data(12 << 20);
        let mem = Memory::new(d.clone());
        let bad = BadSpot { inner: &mem, bad: (9 << 20, (9 << 20) + 5000) };
        let opts = Options { disk: "test".into(), out_dir: dir.display().to_string(), size: 12 << 20, stop: None };
        let o = make(&bad, &opts, &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(o.verified, "{o:?}");
        // Whole 4 KB units around the bad spot: 9 MiB .. 9 MiB + 8 KiB.
        assert_eq!(o.unreadable, vec![(9 << 20, 8192)]);
        let img = std::fs::read(dir.join(IMAGE_NAME)).unwrap();
        let mut want = d.clone();
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
