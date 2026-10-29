//! The automatic repairs: every damaged file copied out into
//! REPAIR\to-repair (repair.rs, "isolate") is tried, the most trustworthy way
//! first, with the open-source tools. A repaired version is saved into
//! REPAIR\repaired\auto, the same kind of file with the same [R....] number
//! in its name, so "put back" (repair.rs) puts it exactly where its original
//! is in WORK, keeping the original; what can't take the original's place
//! (a preview taken out of a damaged camera RAW, what could be read out of a
//! damaged archive) goes into REPAIR\salvaged\auto. Every attempt is listed
//! in REPAIR\auto-repair-log.csv.
//!
//! In order:
//!
//! 1. An intact copy of the same file, somewhere else in WORK (or in a folder
//!    the person named): the same size, and identical in every byte the old
//!    drive did give up (the map check knows exactly which bytes it never
//!    read). That is the file as it was: not a repair, a restore.
//! 2. The format's own repair: a photo cut short gets its end marker back; a
//!    video that lost its index (untrunc, with a good video from the same
//!    camera, picked by the program) or whose container broke (FFmpeg); an
//!    MP3's frames (MP3val); a FLAC re-encoded losslessly; a PDF rebuilt
//!    (qpdf); an Office document or zip rebuilt from every part that reads
//!    back right (7-Zip); a WAV's header.
//! 3. Salvage: the largest preview inside a camera RAW, the small preview
//!    inside a JPEG, whatever can be read out of an archive.
//!
//! A version only counts as repaired when it checks out: the program's own
//! check of its structure passes, and FFmpeg (or qpdf, or 7-Zip) reads it
//! through without the errors the damaged copy had. The copies in to-repair
//! are never changed: every tool works on a copy of the copy, in a folder of
//! its own, and nothing here ever writes into WORK (put back does that).
//!
//! What no tool can do: bring back bytes the old drive never gave up. Most
//! repairs here make a file open again; where the lost part was in the
//! middle of a picture or a video, it still shows (grey, or a glitch), and
//! the log says so.

use crate::fmt::{commas, csv_row};
use crate::repair::{self, Item};
use crate::tools::{self, Tool};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// REPAIR\repaired\auto and REPAIR\salvaged\auto.
pub const AUTO_DIR: &str = "auto";
pub const LOG_NAME: &str = "auto-repair-log.csv";
const TEMP_DIR: &str = "~auto-repair-working";
/// The program's own repairs, as the log names the tool.
const OWN: &str = "Automatic Recovery Probe";

pub struct Options {
    pub work: PathBuf,
    /// Other folders that may hold intact copies of the files.
    pub search: Vec<PathBuf>,
    /// The map check's results folder (which bytes of each file were never read).
    pub map_check: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fate {
    Repaired,
    Salvaged,
    NotRepaired,
}

impl Fate {
    pub fn word(self) -> &'static str {
        match self {
            Fate::Repaired => "REPAIRED",
            Fate::Salvaged => "SALVAGED",
            Fate::NotRepaired => "NOT REPAIRED",
        }
    }
}

/// Where a file no tool here could repair might go next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Nothing to do: it opens or plays, and the lost part cannot come back.
    Nothing,
    /// The Mac (iPhone photos, camera RAW, Pages/Numbers/Keynote).
    Mac,
    /// A paid repair tool (look at its free preview first).
    Paid,
    /// Its own program's repair (Word, Excel, PowerPoint: Open and Repair).
    OwnApp,
    /// Done.
    Done,
}

impl Next {
    pub fn key(self) -> &'static str {
        match self {
            Next::Nothing => "nothing",
            Next::Mac => "mac",
            Next::Paid => "paid",
            Next::OwnApp => "own-app",
            Next::Done => "done",
        }
    }
    pub fn from_key(k: &str) -> Next {
        match k {
            "mac" => Next::Mac,
            "paid" => Next::Paid,
            "own-app" => Next::OwnApp,
            "done" => Next::Done,
            _ => Next::Nothing,
        }
    }
}

/// One file tried.
#[derive(Clone, Debug)]
pub struct Attempt {
    pub id: String,
    /// In WORK.
    pub path: String,
    pub group: String,
    pub fate: Fate,
    pub how: String,
    pub tool: String,
    /// In REPAIR (repaired\auto\... or salvaged\auto\...).
    pub output: String,
    pub next: Next,
    pub note: String,
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub attempts: Vec<Attempt>,
    /// Already repaired (a version in "repaired", or put back) before this run.
    pub already: usize,
    pub stopped: bool,
    pub log: Option<PathBuf>,
}

impl Outcome {
    pub fn count(&self, f: Fate) -> usize {
        self.attempts.iter().filter(|a| a.fate == f).count()
    }
}

// ---------------------------------------------------------------- small helpers

/// A camera RAW, by its extension (without the dot).
fn is_raw(ext: &str) -> bool {
    crate::formats::is_raw_ext(&format!(".{}", ext.to_lowercase()))
}

fn ext_lower(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => e.to_lowercase(),
        _ => String::new(),
    }
}

fn stem_of(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    }
}

fn under(base: &Path, rel: &str) -> PathBuf {
    let mut p = crate::rescue::verbatim(base);
    for c in rel.split('\\').filter(|c| !c.is_empty()) {
        p.push(c);
    }
    p
}

fn sha_of(p: &Path) -> Option<String> {
    crate::sha256::file_hex(p, &|| false).ok().flatten()
}

const VIDEO_ISO: &[&str] = &["mp4", "m4v", "mov", "qt", "3gp", "3g2"];
const VIDEO_OTHER: &[&str] = &["mkv", "webm", "avi", "mts", "m2ts", "ts", "mpg", "mpeg", "wmv", "flv"];
const ZIP_LIKE: &[&str] = &[
    "zip", "docx", "docm", "dotx", "dotm", "xlsx", "xlsm", "xltx", "xltm", "pptx", "pptm", "potx", "ppsx", "odt", "ods",
    "odp", "odg", "epub", "jar",
];
const ARCHIVES: &[&str] = &["7z", "rar", "gz", "tgz", "bz2", "tbz2", "xz", "txz", "zst", "cab", "tar", "iso"];
const OLD_OFFICE: &[&str] = &["doc", "dot", "xls", "xlt", "ppt", "pps", "rtf"];
const MAC_DOCS: &[&str] = &["pages", "numbers", "key", "rtfd", "webarchive"];

// ---------------------------------------------------------------- JPEG

#[derive(Debug, PartialEq, Eq)]
pub enum JpegLayout {
    /// Every marker is in place; the picture ends at this offset.
    Complete(usize),
    /// The compressed picture data runs to the end of the file: its end was lost.
    CutShort,
    /// The picture data starts, then something that makes no sense follows it
    /// (a marker that is not one, a length past the end) while its end marker
    /// is still there further on: damaged in the middle, not cut short.
    Garbled,
    /// Not a JPEG, or broken before the picture data.
    Broken,
}

/// Where a JPEG's parts are (the same walk as checks::jpeg_image_end, on bytes).
pub fn jpeg_layout(d: &[u8]) -> JpegLayout {
    let mut unlimited = u64::MAX;
    layout_within(d, &mut unlimited, true).unwrap_or(JpegLayout::Broken)
}

/// Take `n` from what the walk may still look at; None once it is spent.
fn spend(budget: &mut u64, n: usize) -> Option<()> {
    *budget = budget.checked_sub(n as u64)?;
    Some(())
}

/// The walk, looking at no more of the file than `budget` (bytes and
/// markers; None once it is spent, before it could tell). `why`: tell a
/// photo cut short from one damaged in the middle (that looks at the rest of
/// the file); without it, either is CutShort.
fn layout_within(d: &[u8], budget: &mut u64, why: bool) -> Option<JpegLayout> {
    let n = d.len();
    if n < 4 || d[0] != 0xFF || d[1] != 0xD8 {
        return Some(JpegLayout::Broken);
    }
    // Where the walk loses its way once the picture data has started: cut
    // short when nothing of the photo is left from there on (the end, or only
    // zeros: never read), damaged in the middle when its end marker is still
    // there after it (putting one more on its end would mend nothing).
    let lost_way = |at: usize, budget: &mut u64| -> Option<JpegLayout> {
        if !why {
            return Some(JpegLayout::CutShort);
        }
        let rest = &d[at.min(n)..];
        let zeros = rest.iter().take_while(|&&b| b == 0).count();
        if zeros == rest.len() {
            spend(budget, zeros)?;
            return Some(JpegLayout::CutShort);
        }
        match rest.windows(2).position(|w| w == [0xFF, 0xD9]) {
            Some(i) => {
                spend(budget, i + 2)?;
                Some(JpegLayout::Garbled)
            }
            None => {
                spend(budget, rest.len())?;
                Some(JpegLayout::CutShort)
            }
        }
    };
    let mut pos = 2usize;
    let mut in_scan = false;
    for _ in 0..1_000_000 {
        spend(budget, 1)?;
        if pos >= n {
            return Some(if in_scan { JpegLayout::CutShort } else { JpegLayout::Broken });
        }
        if d[pos] != 0xFF {
            return if in_scan { lost_way(pos, budget) } else { Some(JpegLayout::Broken) };
        }
        let fill = d[pos..].iter().take_while(|&&b| b == 0xFF).count();
        spend(budget, fill)?;
        pos += fill;
        if pos >= n {
            return Some(if in_scan { JpegLayout::CutShort } else { JpegLayout::Broken });
        }
        let marker = d[pos];
        pos += 1;
        if marker == 0xD9 {
            return Some(JpegLayout::Complete(pos));
        }
        // (Looking for whole JPEGs inside a file: a JPEG's start further on
        // is another one's, looked at on its own - not walked through again
        // from each before it.)
        if marker == 0xD8 && !why {
            return Some(JpegLayout::Broken);
        }
        if (0xD0..=0xD8).contains(&marker) || marker == 0x01 {
            continue;
        }
        if pos + 2 > n {
            return Some(if in_scan { JpegLayout::CutShort } else { JpegLayout::Broken });
        }
        let seg = ((d[pos] as usize) << 8) | d[pos + 1] as usize;
        if seg < 2 || pos + seg > n {
            return if in_scan { lost_way(pos, budget) } else { Some(JpegLayout::Broken) };
        }
        pos += seg;
        if marker == 0xDA {
            in_scan = true;
            loop {
                let Some(i) = d[pos.min(n)..].iter().position(|&c| c == 0xFF) else {
                    spend(budget, n - pos.min(n))?;
                    return Some(JpegLayout::CutShort);
                };
                spend(budget, i + 1)?;
                let k = pos + i;
                if k + 1 >= n {
                    return Some(JpegLayout::CutShort);
                }
                let nxt = d[k + 1];
                if nxt == 0x00 || (0xD0..=0xD7).contains(&nxt) {
                    pos = k + 2;
                } else if nxt == 0xFF {
                    pos = k + 1;
                } else {
                    pos = k;
                    break;
                }
            }
        }
    }
    Some(JpegLayout::Broken)
}

/// A JPEG whose end was lost, with its end marker put back (and the zeros
/// where its last part was never read left off).
pub fn jpeg_with_end(d: &[u8]) -> Option<Vec<u8>> {
    if jpeg_layout(d) != JpegLayout::CutShort {
        return None;
    }
    let mut end = d.len();
    let zeros = d.iter().rev().take_while(|&&b| b == 0).count();
    if zeros >= 16 {
        end -= zeros;
    }
    // A marker cut in half at the very end.
    while end > 0 && d[end - 1] == 0xFF {
        end -= 1;
    }
    let mut out = d[..end].to_vec();
    out.extend_from_slice(&[0xFF, 0xD9]);
    Some(out)
}

/// Every whole JPEG inside a file (a camera RAW holds several previews; a
/// JPEG holds a small one): (start, end), largest first.
pub fn embedded_jpegs(d: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    // (A file made of what only looks like the starts of JPEGs - FF D8 over
    // and over - would have each one walked to its end, and the next, for
    // ever: no more is looked at than a few times the file.)
    let mut budget = 8 * d.len() as u64 + (1 << 20);
    while i + 3 < d.len() {
        let Some(k) = d[i..].windows(3).position(|w| w == [0xFF, 0xD8, 0xFF]) else { break };
        let start = i + k;
        match layout_within(&d[start..], &mut budget, false) {
            None => break,
            // (And inside it too: a photo's own small preview is in its
            // first part, the camera's notes.)
            Some(JpegLayout::Complete(len)) if len >= 2048 => out.push((start, start + len)),
            Some(_) => {}
        }
        i = start + 3;
    }
    out.sort_by_key(|(s, e)| std::cmp::Reverse(e - s));
    out
}

// ---------------------------------------------------------------- WAV

/// A WAV whose header sizes no longer match the file (cut short, or a size
/// field that was never read), with them set right. None: nothing to fix.
pub fn wav_fixed(d: &[u8]) -> Option<Vec<u8>> {
    if d.len() < 44 || &d[0..4] != b"RIFF" || &d[8..12] != b"WAVE" {
        return None;
    }
    let mut out = d.to_vec();
    let mut changed = false;
    let riff = (d.len() - 8) as u32;
    if u32::from_le_bytes([d[4], d[5], d[6], d[7]]) != riff {
        out[4..8].copy_from_slice(&riff.to_le_bytes());
        changed = true;
    }
    let mut pos = 12usize;
    while pos + 8 <= d.len() {
        let id = &d[pos..pos + 4];
        let len = u32::from_le_bytes([d[pos + 4], d[pos + 5], d[pos + 6], d[pos + 7]]) as usize;
        if id == b"data" {
            let room = d.len() - (pos + 8);
            if len != room && (len > room || len == 0) {
                out[pos + 4..pos + 8].copy_from_slice(&(room as u32).to_le_bytes());
                changed = true;
            }
            break;
        }
        pos += 8 + len + (len & 1);
    }
    changed.then_some(out)
}

// ---------------------------------------------------------------- an intact twin

/// The parts of a file of `size` bytes that were read (not in `lost`).
fn read_parts(size: u64, lost: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let mut l: Vec<(u64, u64)> = lost.iter().map(|&(a, b)| (a.min(size), b.min(size))).filter(|(a, b)| b > a).collect();
    l.sort_unstable();
    let mut out = Vec::new();
    let mut at = 0u64;
    for (a, b) in l {
        if a > at {
            out.push((at, a));
        }
        at = at.max(b);
    }
    if at < size {
        out.push((at, size));
    }
    out
}

/// Is `cand` the intact original of `damaged`: the same size, the same in
/// every byte that was read, and not the same where it was never read (a
/// copy of the damaged file would be the same there too)?
pub fn is_intact_twin(damaged: &Path, cand: &Path, lost: &[(u64, u64)]) -> std::io::Result<bool> {
    let size = std::fs::metadata(damaged)?.len();
    if std::fs::metadata(cand)?.len() != size || lost.is_empty() {
        return Ok(false);
    }
    // Enough of the file must have been read to tell it from any other file
    // of the same size: at least 16 KB and half of it, and not just zeros.
    let parts = read_parts(size, lost);
    let read: u64 = parts.iter().map(|(s, e)| e - s).sum();
    if read < 16 * 1024 || read * 2 < size {
        return Ok(false);
    }
    let mut a = std::fs::File::open(damaged)?;
    let mut b = std::fs::File::open(cand)?;
    let mut x = vec![0u8; 1 << 20];
    let mut y = vec![0u8; 1 << 20];
    let mut same_part = |a: &mut std::fs::File, b: &mut std::fs::File, s: u64, e: u64| -> std::io::Result<bool> {
        a.seek(SeekFrom::Start(s))?;
        b.seek(SeekFrom::Start(s))?;
        let mut left = e - s;
        while left > 0 {
            let n = left.min(x.len() as u64) as usize;
            a.read_exact(&mut x[..n])?;
            b.read_exact(&mut y[..n])?;
            if x[..n] != y[..n] {
                return Ok(false);
            }
            left -= n as u64;
        }
        Ok(true)
    };
    for &(s, e) in &parts {
        if !same_part(&mut a, &mut b, s, e)? {
            return Ok(false);
        }
    }
    // ... and the part read must hold something (4 KB that are not zeros).
    let mut nonzero = 0u64;
    let mut buf = vec![0u8; 1 << 16];
    'outer: for &(s, e) in &parts {
        a.seek(SeekFrom::Start(s))?;
        let mut left = e - s;
        while left > 0 {
            let n = left.min(buf.len() as u64) as usize;
            a.read_exact(&mut buf[..n])?;
            nonzero += buf[..n].iter().filter(|&&x| x != 0).count() as u64;
            if nonzero >= 4096 {
                break 'outer;
            }
            left -= n as u64;
        }
    }
    if nonzero < 4096 {
        return Ok(false);
    }
    // Where it was never read, the twin must hold something else.
    let mut all_same = true;
    for &(s, e) in lost {
        let (s, e) = (s.min(size), e.min(size));
        if e > s && !same_part(&mut a, &mut b, s, e)? {
            all_same = false;
            break;
        }
    }
    Ok(!all_same)
}

// ---------------------------------------------------------------- running the tools

struct Env<'a> {
    home: &'a Path,
    work: &'a Path,
    map_items: Vec<crate::join::MapItem>,
    /// The map check's list of the files the never-read places touched was
    /// read: a file not on it was read whole from the old drive.
    map_known: bool,
    /// Damaged files' paths in WORK (lower case): never a twin or a pattern.
    damaged: HashSet<String>,
    /// Same-size files, by size, for the twins (filled once).
    by_size: HashMap<u64, Vec<PathBuf>>,
    /// Videos in WORK that may serve untrunc as a pattern (filled when needed).
    videos: std::cell::RefCell<Option<Vec<(PathBuf, u64, i64)>>>,
    ffmpeg: Option<PathBuf>,
    ffprobe: Option<PathBuf>,
    sevenzip: Option<PathBuf>,
    mp3val: Option<PathBuf>,
    untrunc: Option<PathBuf>,
    qpdf: Option<PathBuf>,
    stop: &'a (dyn Fn() -> bool + Sync),
}

/// What one try made.
enum Try {
    Made { file: PathBuf, how: String, tool: String, salvage: bool, note: String },
    No { why: String, next: Next },
}

fn no(why: impl Into<String>, next: Next) -> Try {
    Try::No { why: why.into(), next }
}

fn run_tool(exe: &Path, args: Vec<OsString>, dir: Option<&Path>, secs: u64, stop: &dyn Fn() -> bool) -> Option<tools::Ran> {
    tools::run(exe, &args, dir, Duration::from_secs(secs), stop).ok()
}

/// How FFmpeg gets on playing a file through: (it could, errors).
fn plays(env: &Env, p: &Path) -> Option<(bool, Vec<String>)> {
    let ff = env.ffmpeg.as_ref()?;
    let size_mb = std::fs::metadata(p).map(|m| m.len() >> 20).unwrap_or(0);
    let r = run_tool(
        ff,
        vec![
            "-hide_banner".into(),
            "-nostdin".into(),
            "-v".into(),
            "error".into(),
            // One thread: decoding a damaged video on several, FFmpeg finds
            // one more error or one fewer from one time to the next, and the
            // errors are what a repaired version is judged by (fewer than the
            // damaged copy's) - so the same file came out repaired one time
            // and not the next.
            "-threads".into(),
            "1".into(),
            "-i".into(),
            p.as_os_str().to_os_string(),
            "-map".into(),
            "0:v?".into(),
            "-map".into(),
            "0:a?".into(),
            "-f".into(),
            "null".into(),
            "-".into(),
        ],
        None,
        900 + size_mb,
        env.stop,
    )?;
    if r.stopped || r.timed_out {
        return None;
    }
    Some((r.code == Some(0), crate::extra::ffmpeg_problems(&r.err)))
}

/// The duration FFmpeg sees in a file, in seconds (0 when it cannot tell).
fn duration_of(env: &Env, p: &Path) -> f64 {
    let Some(fp) = env.ffprobe.as_ref() else { return 0.0 };
    let Some(r) = run_tool(
        fp,
        vec![
            "-v".into(),
            "error".into(),
            "-show_entries".into(),
            "format=duration".into(),
            "-of".into(),
            "default=nw=1:nk=1".into(),
            p.as_os_str().to_os_string(),
        ],
        None,
        120,
        env.stop,
    ) else {
        return 0.0;
    };
    r.out.trim().parse().unwrap_or(0.0)
}

fn probe_ok(p: &Path) -> Result<(), String> {
    let v = crate::probe::probe_file(p, 4096, false);
    if crate::formats::is_damage(v.verdict) {
        Err(format!("{}: {}", v.verdict, v.detail))
    } else {
        Ok(())
    }
}

/// A version checks out: its structure is right, and FFmpeg plays it
/// through (when FFmpeg is there) with fewer errors than the damaged copy
/// (`before`: how that played) - or as few, where the damaged copy's
/// structure was broken and this one's is right (a container rebuilt round
/// the same streams: the errors where data was lost stay, the file opens).
fn media_checks_out(env: &Env, made: &Path, before: Option<&(bool, Vec<String>)>, damaged: Option<&Path>) -> Result<String, String> {
    probe_ok(made)?;
    let Some((ok, errs)) = plays(env, made) else { return Ok(String::new()) };
    if !ok {
        return Err(format!("FFmpeg cannot play it: {}", errs.first().cloned().unwrap_or_default()));
    }
    if let Some((was_ok, was)) = before {
        let mended_structure = || damaged.is_some_and(|d| probe_ok(d).is_err());
        if *was_ok && !was.is_empty() && (errs.len() > was.len() || (errs.len() == was.len() && !mended_structure())) {
            return Err("it plays no better than the damaged copy".to_string());
        }
    }
    Ok(if errs.is_empty() {
        "FFmpeg plays it through without an error".to_string()
    } else {
        format!(
            "FFmpeg plays it through; {} where data was lost ({})",
            if errs.len() == 1 { "1 error".to_string() } else { format!("{} errors", commas(errs.len() as u64)) },
            errs[0]
        )
    })
}

// ---------------------------------------------------------------- one file

fn fix_jpeg(env: &Env, src: &Path, tmp: &Path, read_whole: bool) -> Try {
    let Ok(d) = std::fs::read(src) else { return no("it could not be read", Next::Paid) };
    match jpeg_layout(&d) {
        JpegLayout::CutShort => {
            let Some(out) = jpeg_with_end(&d) else { return no("its end could not be found", Next::Paid) };
            // (The end put back must make it whole: every part in place.)
            if jpeg_layout(&out) != JpegLayout::Complete(out.len()) {
                return no(
                    "it was cut short, and its picture data is damaged before that too: an end marker put back does not \
                     make it whole",
                    Next::Paid,
                );
            }
            let file = tmp.join(format!("repaired.{}", ext_lower(&src.to_string_lossy())));
            if std::fs::write(&file, &out).is_err() {
                return no("the repaired version could not be written", Next::Paid);
            }
            match media_checks_out(env, &file, None, None) {
                Ok(note) => Try::Made {
                    file,
                    how: "the photo was cut short (its end was never read): its end marker was put back, so it opens; the \
                          part that was lost shows grey"
                        .to_string(),
                    tool: OWN.to_string(),
                    salvage: false,
                    note,
                },
                Err(e) => no(format!("its end marker was put back, but it still does not check out ({e})"), Next::Paid),
            }
        }
        JpegLayout::Complete(_) if read_whole => no(
            "every part of the photo is in place, and every byte of it was read from the old drive: it is exactly as it was \
             there",
            Next::Nothing,
        ),
        JpegLayout::Complete(_) => no(
            "every part of the photo is in place, but some of its picture data was never read: it opens, with part of the \
             picture grey or garbled from that point; no tool can bring that part back (a photo repair tool's free preview \
             shows whether it does better)",
            Next::Paid,
        ),
        JpegLayout::Garbled if read_whole => no(
            "its picture data has a mistake in it partway through, but every byte of it was read from the old drive: it \
             is exactly as it was there",
            Next::Nothing,
        ),
        JpegLayout::Garbled => no(
            "its picture data is damaged partway through, but its end is there (it was not cut short, so putting its end \
             marker back would mend nothing): it opens, with the picture grey or garbled from that point; no tool can \
             bring that part back (a photo repair tool's free preview shows whether it does better)",
            Next::Paid,
        ),
        JpegLayout::Broken => no("its structure is broken before the picture data starts", Next::Paid),
    }
}

/// The largest preview inside a JPEG or camera RAW, as a JPEG of its own.
fn salvage_preview(src: &Path, tmp: &Path, min: usize, what: &str) -> Option<Try> {
    let len = std::fs::metadata(src).ok()?.len();
    if len > 1 << 30 {
        return None;
    }
    let d = std::fs::read(src).ok()?;
    let found = embedded_jpegs(&d);
    // (For a JPEG, not the damaged picture itself, which starts at 0.)
    let (s, e) = found.into_iter().find(|&(s, e)| s > 0 && e - s >= min)?;
    let file = tmp.join("preview.jpg");
    std::fs::write(&file, &d[s..e]).ok()?;
    probe_ok(&file).ok()?;
    Some(Try::Made {
        file,
        how: format!("{what} ({} KB) was taken out of it, as a JPEG of its own", (e - s) / 1024),
        tool: OWN.to_string(),
        salvage: true,
        note: String::new(),
    })
}

fn ftyp_brand(p: &Path) -> Option<String> {
    let mut f = std::fs::File::open(p).ok()?;
    let mut b = [0u8; 12];
    f.read_exact(&mut b).ok()?;
    (&b[4..8] == b"ftyp").then(|| String::from_utf8_lossy(&b[8..12]).to_string())
}

/// "IMG_1234" -> "IMG_", "GOPR0012" -> "GOPR": what a camera starts its names with.
fn name_prefix(name: &str) -> String {
    stem_of(name).chars().take_while(|c| !c.is_ascii_digit()).collect::<String>().to_lowercase()
}

fn mtime_secs(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Good videos to give untrunc as a pattern, best first: the same kind of
/// file, from the same folder and camera (the start of its name), made
/// around the same time.
fn patterns(env: &Env, item: &Item, src: &Path) -> Vec<PathBuf> {
    if env.videos.borrow().is_none() {
        let mut all = Vec::new();
        crate::walk::walk(&env.work.display().to_string(), |dir, dirs, files| {
            dirs.retain(|d| !crate::formats::SYSTEM_DIRS.contains(&d.as_str()));
            for f in files {
                if f.starts_with("._") || !VIDEO_ISO.contains(&ext_lower(f).as_str()) && ext_lower(f) != "m4a" {
                    continue;
                }
                let full = PathBuf::from(crate::walk::join(dir, f));
                let len = std::fs::metadata(&full).map(|m| m.len()).unwrap_or(0);
                if len > 0 {
                    let t = mtime_secs(&full);
                    all.push((full, len, t));
                }
            }
            !(env.stop)()
        });
        *env.videos.borrow_mut() = Some(all);
    }
    let name = item.path.rsplit('\\').next().unwrap_or(&item.path);
    let ext = ext_lower(name);
    let folder = item.path.rsplit_once('\\').map(|(d, _)| d.to_lowercase()).unwrap_or_default();
    let prefix = name_prefix(name);
    let brand = ftyp_brand(src);
    let own_time = mtime_secs(&under(env.work, &item.path));
    let videos = env.videos.borrow();
    let mut scored: Vec<(i64, PathBuf)> = Vec::new();
    for (p, _len, t) in videos.as_ref().into_iter().flatten() {
        let rel = repair::relative_to(env.work, &p.display().to_string().replace('/', "\\")).unwrap_or_default();
        if rel.eq_ignore_ascii_case(&item.path) || env.damaged.contains(&rel.to_lowercase()) {
            continue;
        }
        let pname = rel.rsplit('\\').next().unwrap_or(&rel);
        let pext = ext_lower(pname);
        let same_family = pext == ext
            || (["mov", "qt"].contains(&pext.as_str()) && ["mov", "qt"].contains(&ext.as_str()))
            || (["mp4", "m4v"].contains(&pext.as_str()) && ["mp4", "m4v"].contains(&ext.as_str()));
        if !same_family {
            continue;
        }
        let mut s = 0i64;
        let pfolder = rel.rsplit_once('\\').map(|(d, _)| d.to_lowercase()).unwrap_or_default();
        if pfolder == folder {
            s += 30;
        }
        if !prefix.is_empty() && name_prefix(pname) == prefix {
            s += 30;
        }
        if brand.is_some() && ftyp_brand(p) == brand {
            s += 20;
        }
        let gap = (t - own_time).abs();
        s += if gap < 86_400 * 7 {
            20
        } else if gap < 86_400 * 90 {
            10
        } else if gap < 86_400 * 730 {
            4
        } else {
            0
        };
        scored.push((s, p.clone()));
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().take(12).map(|x| x.1).collect()
}

fn fix_isobmff(env: &Env, item: &Item, src: &Path, tmp: &Path, damage: &str) -> Try {
    let before = plays(env, src);
    let cut = damage.contains("TRUNCATED") || damage.contains("index") || damage.contains("moov") || damage.contains("MISMATCH");
    if let Some((true, errs)) = &before {
        if !cut {
            return no(
                format!(
                    "it plays; where data was lost it shows a short glitch{}. The lost part cannot be brought back by any tool",
                    if errs.is_empty() { String::new() } else { format!(" (FFmpeg: {})", errs[0]) }
                ),
                Next::Nothing,
            );
        }
    }
    let Some(untrunc) = env.untrunc.as_ref() else {
        return no("it does not play, and untrunc (which can repair it) was not found on this PC", Next::Paid);
    };
    let ext = ext_lower(&src.to_string_lossy());
    let mut tried = 0;
    for pattern in patterns(env, item, src) {
        if (env.stop)() || tried >= 4 {
            break;
        }
        // The pattern must be good itself.
        if let Some((false, _)) = plays(env, &pattern) {
            continue;
        }
        tried += 1;
        let dir = tmp.join(format!("untrunc-{tried}"));
        let _ = std::fs::create_dir_all(&dir);
        let broken = dir.join(format!("broken.{ext}"));
        if std::fs::copy(src, &broken).is_err() {
            return no("a working copy could not be made", Next::Paid);
        }
        let limit = 1800 + std::fs::metadata(src).map(|m| m.len() >> 20).unwrap_or(0);
        let Some(r) = run_tool(
            untrunc,
            vec!["-n".into(), "-s".into(), pattern.as_os_str().to_os_string(), broken.as_os_str().to_os_string()],
            Some(&dir),
            limit,
            env.stop,
        ) else {
            continue;
        };
        if r.stopped {
            break;
        }
        // untrunc writes "broken.mov_fixed.mov" (or "_fixed-s") beside it.
        let made = std::fs::read_dir(&dir).ok().and_then(|rd| {
            rd.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().contains("_fixed")))
        });
        let Some(made) = made else { continue };
        let named = tmp.join(format!("repaired.{ext}"));
        if std::fs::rename(&made, &named).is_err() {
            continue;
        }
        match media_checks_out(env, &named, before.as_ref(), Some(src)) {
            Ok(note) => {
                let secs = duration_of(env, &named);
                let note = format!("{note}{}", own_moments(&under(env.work, &item.path), &pattern, &named));
                return Try::Made {
                    file: named,
                    how: format!(
                        "its index was rebuilt by untrunc, using {} (a good video from the same camera) as the pattern",
                        repair::relative_to(env.work, &pattern.display().to_string().replace('/', "\\"))
                            .unwrap_or_else(|| pattern.display().to_string())
                    ),
                    tool: tools::version(Tool::Untrunc, untrunc),
                    salvage: false,
                    note: if secs > 0.0 { format!("{note}; {secs:.1} seconds long") } else { note },
                };
            }
            Err(_) => {
                let _ = std::fs::remove_file(&named);
            }
        }
    }
    if tried == 0 {
        return no(
            "it does not play, and there is no good video of the same kind in WORK for untrunc to use as a pattern",
            Next::Paid,
        );
    }
    // Last: FFmpeg copies what it can read into a new container.
    if let Some(Try::Made { file, how, tool, salvage, note }) = remux(env, src, tmp, before.as_ref()) {
        return Try::Made { file, how, tool, salvage, note };
    }
    no(format!("untrunc could not rebuild it with {tried} good videos as patterns"), Next::Paid)
}

/// A file's own moment, as the copies hold it: created (on Windows, where
/// the copies keep it - unless it is after the modified one, as for a file
/// copied since), modified elsewhere.
fn file_moment(p: &Path) -> Option<i64> {
    let m = std::fs::metadata(p).ok()?;
    let modified = m.modified().ok()?;
    #[cfg(windows)]
    let t = match m.created() {
        Ok(c) if c <= modified => c,
        _ => modified,
    };
    #[cfg(not(windows))]
    let t = modified;
    Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64)
}

/// untrunc builds the repaired video's index from the pattern's, and with
/// it the moments the pattern was made: moved to this video's own. Its own
/// is its file's date, turned into the index's moment the way the pattern
/// turns its file's date into its own (cameras differ: UTC or local time),
/// to the quarter hour. Says what it did, for the note.
fn own_moments(original: &Path, pattern: &Path, made: &Path) -> String {
    let (Some(own), Some(now)) = (file_moment(original), crate::mp4time::made(made)) else {
        return String::new();
    };
    let way = match (crate::mp4time::made(pattern), file_moment(pattern)) {
        (Some(inside), Some(file)) if (inside - file).abs() <= 14 * 3600 + 60 => (inside - file + 450).div_euclid(900) * 900,
        _ => 0,
    };
    let target = own + way;
    if (target - now).abs() < 2 {
        return String::new();
    }
    match crate::mp4time::shift(made, target - now) {
        Ok(n) if n > 0 => format!(
            "; the date inside it set to its own ({}), not the pattern's ({})",
            crate::fmt::utc_iso(target).replace('T', " ").replace('Z', ""),
            crate::fmt::utc_iso(now).replace('T', " ").replace('Z', "")
        ),
        _ => String::new(),
    }
}

/// FFmpeg copies every stream into a new container of the same kind (the
/// index made again, broken pieces left out).
fn remux(env: &Env, src: &Path, tmp: &Path, before: Option<&(bool, Vec<String>)>) -> Option<Try> {
    let ff = env.ffmpeg.as_ref()?;
    let ext = ext_lower(&src.to_string_lossy());
    let out = tmp.join(format!("remuxed.{ext}"));
    let mut args: Vec<OsString> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-v".into(),
        "error".into(),
        "-err_detect".into(),
        "ignore_err".into(),
        "-fflags".into(),
        "+genpts+discardcorrupt".into(),
        "-i".into(),
        src.as_os_str().to_os_string(),
        "-map".into(),
        "0".into(),
        "-c".into(),
        "copy".into(),
        "-ignore_unknown".into(),
    ];
    if VIDEO_ISO.contains(&ext.as_str()) {
        // Every tag kept as it was: the creation date and place an iPhone
        // writes are left out otherwise.
        args.extend(["-movflags".into(), "use_metadata_tags".into()]);
    }
    args.extend(["-y".into(), out.as_os_str().to_os_string()]);
    let r = run_tool(ff, args, None, 1800, env.stop)?;
    if r.code != Some(0) || !out.is_file() {
        return None;
    }
    let note = media_checks_out(env, &out, before, Some(src)).ok()?;
    Some(Try::Made {
        file: out,
        how: "its container was rebuilt by FFmpeg: every stream copied as it is into a new file, the index made again and \
              broken pieces left out"
            .to_string(),
        tool: env.ffmpeg.as_ref().map(|f| tools::version(Tool::Ffmpeg, f)).unwrap_or_default(),
        salvage: false,
        note,
    })
}

fn fix_other_video(env: &Env, src: &Path, tmp: &Path, damage: &str) -> Try {
    let before = plays(env, src);
    let cut = damage.contains("TRUNCATED") || damage.contains("MISMATCH");
    if let Some((true, errs)) = &before {
        if !cut {
            return no(
                format!(
                    "it plays; where data was lost it shows a short glitch{}. The lost part cannot be brought back",
                    if errs.is_empty() { String::new() } else { format!(" (FFmpeg: {})", errs[0]) }
                ),
                Next::Nothing,
            );
        }
    }
    match remux(env, src, tmp, before.as_ref()) {
        Some(t) => t,
        None => no("FFmpeg could not rebuild it", Next::Paid),
    }
}

fn fix_mp3(env: &Env, src: &Path, tmp: &Path) -> Try {
    let Some(mp3val) = env.mp3val.as_ref() else {
        return no("MP3val (which repairs MP3s) was not found on this PC", Next::Nothing);
    };
    let work = tmp.join("repaired.mp3");
    if std::fs::copy(src, &work).is_err() {
        return no("a working copy could not be made", Next::Nothing);
    }
    let before = plays(env, src);
    let Some(r) = run_tool(mp3val, vec![work.as_os_str().to_os_string(), "-f".into(), "-nb".into(), "-t".into()], Some(tmp), 600, env.stop)
    else {
        return no("MP3val could not be run", Next::Nothing);
    };
    if !r.text().contains("FIXED") && sha_of(src) == sha_of(&work) {
        return no(
            format!("MP3val found nothing it can repair{}", r.text().lines().find(|l| l.starts_with("WARNING")).map(|l| format!(" ({l})")).unwrap_or_default()),
            Next::Nothing,
        );
    }
    match media_checks_out(env, &work, before.as_ref(), Some(src)) {
        Ok(note) => Try::Made {
            file: work,
            how: "its frames and header were rebuilt by MP3val (the length it claims now matches the sound it holds)".to_string(),
            tool: tools::version(Tool::Mp3val, mp3val),
            salvage: false,
            note,
        },
        Err(e) => no(format!("MP3val's repair does not check out ({e})"), Next::Nothing),
    }
}

const PLAYS_WITH_SKIP: &str = "it plays as it is: where data was lost there is a short skip or click (FFmpeg plays \
through it without an error). Nothing can bring the lost part back";

fn fix_reencode_flac(env: &Env, src: &Path, tmp: &Path) -> Try {
    let Some(ff) = env.ffmpeg.as_ref() else { return no("FFmpeg was not found on this PC", Next::Nothing) };
    let before = plays(env, src);
    if let Some((true, e)) = &before {
        if e.is_empty() {
            return no(PLAYS_WITH_SKIP, Next::Nothing);
        }
    }
    let out = tmp.join("repaired.flac");
    let r = run_tool(
        ff,
        vec![
            "-hide_banner".into(),
            "-nostdin".into(),
            "-v".into(),
            "error".into(),
            "-err_detect".into(),
            "ignore_err".into(),
            "-i".into(),
            src.as_os_str().to_os_string(),
            "-map".into(),
            "0:a".into(),
            "-map".into(),
            "0:v?".into(),
            "-c:a".into(),
            "flac".into(),
            "-c:v".into(),
            "copy".into(),
            "-map_metadata".into(),
            "0".into(),
            "-y".into(),
            out.as_os_str().to_os_string(),
        ],
        None,
        1800,
        env.stop,
    );
    if !r.is_some_and(|r| r.code == Some(0)) || !out.is_file() {
        return no("FFmpeg could not re-encode it", Next::Nothing);
    }
    match media_checks_out(env, &out, before.as_ref(), Some(src)) {
        Ok(note) => Try::Made {
            file: out,
            how: "re-encoded losslessly by FFmpeg: every frame that could be decoded is exactly as it was, the damaged ones \
                  left out"
                .to_string(),
            tool: tools::version(Tool::Ffmpeg, ff),
            salvage: false,
            note,
        },
        Err(e) => no(format!("the re-encoded version does not check out ({e})"), Next::Nothing),
    }
}

fn fix_remux_audio(env: &Env, src: &Path, tmp: &Path) -> Try {
    let before = plays(env, src);
    if let Some((true, e)) = &before {
        if e.is_empty() {
            return no(PLAYS_WITH_SKIP, Next::Nothing);
        }
    }
    match remux(env, src, tmp, before.as_ref()) {
        Some(t) => t,
        None => no("FFmpeg could not rebuild it; it plays up to the damage", Next::Nothing),
    }
}

fn fix_wav(env: &Env, src: &Path, tmp: &Path) -> Try {
    let Ok(d) = std::fs::read(src) else { return no("it could not be read", Next::Nothing) };
    match wav_fixed(&d) {
        Some(out) => {
            let file = tmp.join("repaired.wav");
            if std::fs::write(&file, out).is_err() {
                return no("the repaired version could not be written", Next::Nothing);
            }
            match media_checks_out(env, &file, None, None) {
                Ok(note) => Try::Made {
                    file,
                    how: "its header said a different length than the sound it holds: set right".to_string(),
                    tool: OWN.to_string(),
                    salvage: false,
                    note,
                },
                Err(e) => no(format!("its header was set right, but it does not check out ({e})"), Next::Nothing),
            }
        }
        None => no(
            "its header is right; where data was lost it holds silence (WAV sound is stored as it is, so the rest is \
             untouched)",
            Next::Nothing,
        ),
    }
}

/// qpdf --check: its exit code (0 clean, 3 warnings only, 2 errors) and the
/// first thing it says is wrong.
fn qpdf_check(qpdf: &Path, p: &Path, stop: &dyn Fn() -> bool) -> (Option<i32>, String) {
    match run_tool(qpdf, vec!["--check".into(), p.as_os_str().to_os_string()], None, 600, stop) {
        Some(r) => (r.code, qpdf_first_problem(&r.text())),
        None => (None, String::new()),
    }
}

/// The first line qpdf gives about a problem, without the file's name.
fn qpdf_first_problem(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| l.starts_with("WARNING:") || (l.starts_with("qpdf:") && !l.contains("operation succeeded")))
        .map(|l| {
            let l = l.trim_start_matches("WARNING:").trim_start_matches("qpdf:").trim();
            // "input.pdf (object 14 0, offset 8723): expected endstream" -> "object 14 0, offset 8723: expected endstream"
            let l = match l.split_once(": ") {
                Some((head, rest)) if head.ends_with(".pdf") => rest.to_string(),
                Some((head, rest)) if head.contains(".pdf (") => {
                    format!("{}: {rest}", head.split_once(" (").map(|(_, w)| w.trim_end_matches(')')).unwrap_or(head))
                }
                _ => l.to_string(),
            };
            l.chars().take(160).collect()
        })
        .unwrap_or_default()
}

/// Whether a PDF's end, with its trailer (which says where the document
/// starts), was lost: no "startxref" left near its end.
fn pdf_trailer_lost(d: &[u8]) -> bool {
    let mut end = d.len();
    while end > 0 && d[end - 1] == 0 {
        end -= 1;
    }
    let tail = &d[end.saturating_sub(4096)..end];
    !tail.windows(9).any(|w| w == b"startxref")
}

/// Where `needle` first is in `hay`.
fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let (&first, rest) = needle.split_first()?;
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        let k = hay[i..hay.len() - rest.len()].iter().position(|&b| b == first)?;
        if &hay[i + k + 1..i + k + 1 + rest.len()] == rest {
            return Some(i + k);
        }
        i += k + 1;
    }
    None
}

/// Is this object's body the document's information dictionary (its title,
/// author, and the dates it was made and changed)? A dictionary with those
/// dates or its maker's name, that is nothing else (no type, no parent).
fn is_pdf_info(body: &[u8]) -> bool {
    let start = body.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(body.len());
    let body = &body[start..];
    body.starts_with(b"<<")
        && [&b"stream"[..], b"/Type", b"/Subtype", b"/Parent"].iter().all(|k| find_bytes(body, k).is_none())
        && [&b"/CreationDate"[..], b"/ModDate", b"/Producer"].iter().any(|k| find_bytes(body, k).is_some())
}

/// A PDF whose end (its trailer, which says where the document starts) was
/// lost, with a new trailer pointing at its catalogue, so qpdf can rebuild
/// the rest from the objects in it - and at its information dictionary, so
/// the dates it was made and changed (and its title and author) stay with
/// it. None: no catalogue found.
pub fn pdf_with_trailer(d: &[u8]) -> Option<Vec<u8>> {
    let mut end = d.len();
    while end > 0 && d[end - 1] == 0 {
        end -= 1;
    }
    let d = &d[..end];
    let (mut catalog, mut info, mut max_obj) = (None, None, 0u64);
    let mut i = 0;
    while let Some(k) = find_bytes(&d[i..], b" 0 obj") {
        let at = i + k;
        let mut s = at;
        while s > 0 && d[s - 1].is_ascii_digit() {
            s -= 1;
        }
        if let Some(n) = std::str::from_utf8(&d[s..at]).ok().filter(|t| !t.is_empty()).and_then(|t| t.parse::<u64>().ok()) {
            max_obj = max_obj.max(n);
            let window = &d[at + 6..d.len().min(at + 6 + 4096)];
            let body = &window[..find_bytes(window, b"endobj").unwrap_or(window.len())];
            let head = &body[..body.len().min(400)];
            if find_bytes(head, b"/Type /Catalog").is_some() || find_bytes(head, b"/Type/Catalog").is_some() {
                catalog = Some(n);
            } else if is_pdf_info(body) {
                // (The last one: a document changed since adds a new one.)
                info = Some(n);
            }
        }
        i = at + 6;
    }
    let catalog = catalog?;
    let info = info.map(|n| format!(" /Info {n} 0 R")).unwrap_or_default();
    let mut out = d.to_vec();
    out.extend_from_slice(
        format!("\ntrailer\n<< /Size {} /Root {catalog} 0 R{info} >>\nstartxref\n0\n%%EOF\n", max_obj.saturating_add(1)).as_bytes(),
    );
    Some(out)
}

fn fix_pdf(env: &Env, src: &Path, tmp: &Path) -> Try {
    let Some(qpdf) = env.qpdf.as_ref() else {
        return no("qpdf (which rebuilds PDFs) was not found on this PC", Next::Paid);
    };
    let (before, _) = qpdf_check(qpdf, src, env.stop);
    // qpdf makes the table of where everything is again, from the objects in
    // the file. Where the file's end was lost with its trailer, it is also
    // tried with a new trailer pointing at its catalogue: older qpdf versions
    // can't rebuild such a file without one, and newer ones rebuild it with a
    // trailer short of its /Size, so it still doesn't check out.
    struct Made {
        file: PathBuf,
        after: Option<i32>,
        problem: String,
        with_trailer: bool,
    }
    // Ok(what it made), or Err(why it could not).
    let rebuild = |input: &Path, name: &str, with_trailer: bool| -> Result<Made, String> {
        let out = tmp.join(name);
        let _ = std::fs::remove_file(&out);
        match run_tool(qpdf, vec![input.as_os_str().to_os_string(), out.as_os_str().to_os_string()], None, 900, env.stop) {
            Some(r) if matches!(r.code, Some(0) | Some(3)) && out.is_file() => {
                let (after, problem) = qpdf_check(qpdf, &out, env.stop);
                Ok(Made { file: out, after, problem, with_trailer })
            }
            Some(r) => Err(qpdf_first_problem(&r.text())),
            None => Err(String::new()),
        }
    };
    let mut made: Vec<Made> = Vec::new();
    let mut failed = String::new();
    match rebuild(src, "repaired.pdf", false) {
        Ok(m) => made.push(m),
        Err(e) => failed = e,
    }
    if !made.iter().any(|m| m.after == Some(0)) {
        if let Some(patched) = std::fs::read(src).ok().filter(|d| pdf_trailer_lost(d)).and_then(|d| pdf_with_trailer(&d)) {
            let p = tmp.join("with-trailer.pdf");
            if std::fs::write(&p, patched).is_ok() {
                match rebuild(&p, "repaired-with-trailer.pdf", true) {
                    Ok(m) => made.push(m),
                    Err(e) => {
                        if failed.is_empty() {
                            failed = e;
                        }
                    }
                }
            }
        }
    }
    // The best: one qpdf finds nothing wrong with; else one with warnings
    // only, where the damaged copy had errors.
    let best = made
        .iter()
        .position(|m| m.after == Some(0))
        .or_else(|| made.iter().position(|m| m.after == Some(3) && before == Some(2)));
    let Some(best) = best else {
        if made.is_empty() {
            return no(
                format!("qpdf could not rebuild it{}", if failed.is_empty() { String::new() } else { format!(": {failed}") }),
                Next::Paid,
            );
        }
        let still = made.iter().map(|m| m.problem.as_str()).find(|p| !p.is_empty()).unwrap_or("");
        return no(
            format!(
                "qpdf rebuilt it, but the rebuilt PDF does not check out better{}",
                if still.is_empty() { String::new() } else { format!(" (qpdf still finds: {still})") }
            ),
            Next::Paid,
        );
    };
    let m = made.swap_remove(best);
    if let Err(e) = probe_ok(&m.file) {
        return no(format!("qpdf rebuilt it, but the rebuilt PDF does not check out ({e})"), Next::Paid);
    }
    Try::Made {
        file: m.file,
        how: if m.with_trailer {
            "rebuilt by qpdf: its end (the trailer and the table of where everything is) was lost, so a new trailer was \
             written pointing at its catalogue, and qpdf made the table again from what is in the file; anything that was in \
             the lost part (often the last page's text) is missing"
                .to_string()
        } else {
            "rebuilt by qpdf: its table of where everything is (the cross-reference table) was made again from what is in \
             the file"
                .to_string()
        },
        tool: tools::version(Tool::Qpdf, qpdf),
        salvage: false,
        note: if m.after == Some(0) {
            "qpdf --check finds no errors".to_string()
        } else {
            format!("qpdf --check finds warnings only: {}", m.problem)
        },
    }
}

/// 7-Zip takes out what it can; Some(list of members it could not).
fn sevenzip_extract(env: &Env, src: &Path, into: &Path) -> Option<(tools::Ran, usize)> {
    let sz = env.sevenzip.as_ref()?;
    let _ = std::fs::create_dir_all(into);
    let mut o = OsString::from("-o");
    o.push(into.as_os_str());
    let r = run_tool(
        sz,
        vec!["x".into(), "-y".into(), "-bd".into(), "-pnot-a-password".into(), o, "--".into(), src.as_os_str().to_os_string()],
        None,
        1800,
        env.stop,
    )?;
    let mut files = 0usize;
    crate::walk::walk(&into.display().to_string(), |_, _, f| {
        files += f.len();
        true
    });
    Some((r, files))
}

fn sevenzip_ok(env: &Env, p: &Path) -> bool {
    env.sevenzip.as_ref().is_some_and(|sz| {
        run_tool(sz, vec!["t".into(), "-pnot-a-password".into(), "-bd".into(), "--".into(), p.as_os_str().to_os_string()], None, 900, env.stop)
            .is_some_and(|r| r.code == Some(0))
    })
}

/// The parts 7-Zip names as not read back right, from its own lines
/// ("ERROR: CRC Failed : word/document.xml", "ERROR: Data Error : ...").
/// The archive's own errors ("Headers Error", "Unexpected end of archive":
/// its index lost) are not about any one part, and are not counted.
fn sevenzip_failed(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text
        .lines()
        .filter_map(|l| {
            let rest = l.trim_end_matches('\r').strip_prefix("ERROR: ")?;
            let (_, name) = rest.split_once(" : ")?;
            let name = name.trim();
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect();
    v.dedup();
    v
}

/// A zip's parts (an Office document's, an ePub's), walked one after another
/// from its start, by their own headers, without its index (the list of its
/// parts at its end, which is what is usually lost when the end of the file
/// was never read): how many parts are files, and whether the walk got as far
/// as the index - which says that no part was lost whole. `lost`: the places
/// the old drive never gave up; the walk stops at a header in one of them,
/// or at anything it can't read as a part.
#[derive(Debug, Default, PartialEq)]
struct ZipWalk {
    /// The parts that are files (not folders).
    files: usize,
    /// What follows the last part is the index.
    reached_index: bool,
}

fn read_at(f: &mut std::fs::File, size: u64, off: u64, n: u64) -> Option<Vec<u8>> {
    if off.checked_add(n)? > size || n > (1 << 20) {
        return None;
    }
    f.seek(SeekFrom::Start(off)).ok()?;
    let mut b = vec![0u8; n as usize];
    f.read_exact(&mut b).ok()?;
    Some(b)
}

fn le16(b: &[u8], at: usize) -> u64 {
    u16::from_le_bytes([b[at], b[at + 1]]) as u64
}

fn le32(b: &[u8], at: usize) -> u64 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as u64
}

/// Where a part written as a stream ends (its sizes in a descriptor after
/// its data): at the descriptor whose signature is followed by the size of
/// the data before it.
fn descriptor_end(f: &mut std::fs::File, size: u64, data: u64) -> Option<u64> {
    const CHUNK: u64 = 1 << 20;
    let mut pos = data;
    while pos < size {
        let n = (size - pos).min(CHUNK + 16);
        let b = read_at(f, size, pos, n)?;
        let mut i = 0usize;
        while i + 16 <= b.len() {
            if &b[i..i + 4] == b"PK\x07\x08" && le32(&b, i + 8) == pos + i as u64 - data {
                return Some(pos + i as u64 + 16);
            }
            i += 1;
        }
        if n < CHUNK + 16 {
            break;
        }
        pos += CHUNK;
    }
    None
}

fn zip_walk(path: &Path, lost: &[(u64, u64)]) -> Option<ZipWalk> {
    let mut f = std::fs::File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    let touches = |a: u64, b: u64| lost.iter().any(|&(s, e)| s < b && a < e);
    let mut w = ZipWalk::default();
    let mut off = 0u64;
    loop {
        if touches(off, off + 4) {
            return Some(w);
        }
        let Some(sig) = read_at(&mut f, size, off, 4) else { return Some(w) };
        if [b"PK\x01\x02", b"PK\x05\x06", b"PK\x06\x06"].iter().any(|s| sig == s.as_slice()) {
            w.reached_index = true;
            return Some(w);
        }
        if sig != b"PK\x03\x04" {
            return Some(w);
        }
        let Some(h) = read_at(&mut f, size, off, 30) else { return Some(w) };
        let flags = le16(&h, 6);
        let (mut csize, usize_) = (le32(&h, 18), le32(&h, 22));
        let (name_len, extra_len) = (le16(&h, 26), le16(&h, 28));
        let Some(name) = read_at(&mut f, size, off + 30, name_len) else { return Some(w) };
        let Some(extra) = read_at(&mut f, size, off + 30 + name_len, extra_len) else { return Some(w) };
        let zip64 = csize == 0xFFFF_FFFF || usize_ == 0xFFFF_FFFF;
        if zip64 {
            // The sizes are in the extra field 0x0001: the size first, then
            // the compressed size, each only where its own field says so.
            let mut i = 0usize;
            let mut found = None;
            while i + 4 <= extra.len() {
                let (id, len) = (le16(&extra, i), le16(&extra, i + 2) as usize);
                let body = &extra[i + 4..(i + 4 + len).min(extra.len())];
                if id == 1 {
                    let k = if usize_ == 0xFFFF_FFFF { 8 } else { 0 };
                    if csize == 0xFFFF_FFFF && body.len() >= k + 8 {
                        found = body[k..k + 8].try_into().ok().map(u64::from_le_bytes);
                    }
                    break;
                }
                i += 4 + len;
            }
            if csize == 0xFFFF_FFFF {
                let Some(c) = found else { return Some(w) };
                csize = c;
            }
        }
        let data = off + 30 + name_len + extra_len;
        if touches(off, data) {
            // A header the old drive never gave up (whole or in part): its
            // part's name or size can't be vouched for.
            return Some(w);
        }
        let Some(mut end) = data.checked_add(csize) else { return Some(w) };
        if flags & 8 != 0 {
            if csize == 0 && !zip64 {
                let Some(e) = descriptor_end(&mut f, size, data) else { return Some(w) };
                end = e;
            } else {
                let Some(sig) = read_at(&mut f, size, end, 4) else { return Some(w) };
                end += if sig == b"PK\x07\x08" { 4 } else { 0 } + if zip64 { 20 } else { 12 };
            }
        }
        if end > size {
            return Some(w);
        }
        if !name.ends_with(b"/") {
            w.files += 1;
        }
        off = end;
    }
}

fn fix_zip_like(env: &Env, item: &Item, src: &Path, tmp: &Path) -> Try {
    let Some(sz) = env.sevenzip.as_ref() else {
        return no("7-Zip was not found on this PC", Next::OwnApp);
    };
    let ext = ext_lower(&src.to_string_lossy());
    let office = !matches!(ext.as_str(), "zip" | "jar");
    if sevenzip_ok(env, src) {
        return no(
            if office {
                "7-Zip reads every part of it back right; the damage is elsewhere in it. Its own program's repair may still \
                 help (Word, Excel or PowerPoint: Open and Repair)"
            } else {
                "7-Zip reads every part of it back right; the damage is outside the files inside it"
            },
            if office { Next::OwnApp } else { Next::Nothing },
        );
    }
    let parts = tmp.join("parts");
    let Some((r, files)) = sevenzip_extract(env, src, &parts) else {
        return no("7-Zip could not be run", Next::OwnApp);
    };
    if files == 0 {
        return no(
            "7-Zip could read nothing out of it",
            if office { Next::OwnApp } else { Next::Nothing },
        );
    }
    // The parts 7-Zip could not read back right stay in, as far as it read
    // them (they may hold most of the text or picture), and are named.
    let failed = sevenzip_failed(&r.text());
    // Rebuilt from what was read (an ePub's or OpenDocument's "mimetype"
    // goes first, uncompressed, as those formats require).
    let out = tmp.join(format!("rebuilt.{ext}"));
    let mut ok = true;
    if parts.join("mimetype").is_file() {
        ok &= run_tool(sz, vec!["a".into(), "-tzip".into(), "-mx=0".into(), out.as_os_str().to_os_string(), "mimetype".into()], Some(&parts), 600, env.stop)
            .is_some_and(|r| r.code == Some(0));
    }
    ok &= run_tool(
        sz,
        vec!["a".into(), "-tzip".into(), "-mx=6".into(), "-r".into(), out.as_os_str().to_os_string(), "*".into(), "-x!mimetype".into()],
        Some(&parts),
        1800,
        env.stop,
    )
    .is_some_and(|r| r.code == Some(0));
    if !ok || !out.is_file() || !sevenzip_ok(env, &out) {
        return no("7-Zip could not rebuild it from its parts", if office { Next::OwnApp } else { Next::Nothing });
    }
    // Every part, only if each read back right (7-Zip checks each against
    // its own checksum) and the zip's own headers, walked part by part, lead
    // from the first part to the index with none missing on the way.
    let lost = lost_ranges(env, item).unwrap_or_default();
    let walk = zip_walk(src, &lost);
    let every = failed.is_empty() && walk.as_ref().is_some_and(|w| w.reached_index && files >= w.files);
    if every {
        return Try::Made {
            file: out,
            how: format!(
                "rebuilt by 7-Zip from every part inside it: all {} read back right, and what was lost was only the zip's own \
                 index (the list of its parts, at its end)",
                commas(files as u64)
            ),
            tool: tools::version(Tool::SevenZip, sz),
            salvage: false,
            note: format!("{} parts", commas(files as u64)),
        };
    }
    let how = if failed.is_empty() {
        format!(
            "{} parts inside it read back right and are kept in a new zip; its own list of its parts was lost too, so whether \
             it held more can't be told",
            commas(files as u64)
        )
    } else {
        let shown: Vec<&str> = failed.iter().take(5).map(String::as_str).collect();
        format!(
            "{} parts inside it read back right and {} did not ({}{}): all are kept in a new zip, those as far as 7-Zip could \
             read them",
            commas(files.saturating_sub(failed.len()) as u64),
            commas(failed.len() as u64),
            shown.join(", "),
            if failed.len() > shown.len() { ", ..." } else { "" }
        )
    };
    Try::Made {
        file: out,
        how,
        tool: tools::version(Tool::SevenZip, sz),
        salvage: true,
        note: if office { "Word, Excel or PowerPoint's Open and Repair may still open the original".to_string() } else { String::new() },
    }
}

fn salvage_archive(env: &Env, src: &Path, tmp: &Path) -> Try {
    if env.sevenzip.is_none() {
        return no("7-Zip was not found on this PC", Next::Nothing);
    }
    if sevenzip_ok(env, src) {
        return no("7-Zip reads every part of it back right; the damage is outside the files inside it", Next::Nothing);
    }
    let parts = tmp.join("what could be read");
    match sevenzip_extract(env, src, &parts) {
        Some((_, n)) if n > 0 => Try::Made {
            file: parts,
            how: format!("7-Zip took out {} files that could still be read", commas(n as u64)),
            tool: env.sevenzip.as_ref().map(|s| tools::version(Tool::SevenZip, s)).unwrap_or_default(),
            salvage: true,
            note: String::new(),
        },
        _ => no("7-Zip could read nothing out of it", Next::Nothing),
    }
}

/// A PNG's chunks, as far as they are whole (each checked by its CRC):
/// (type, the whole chunk).
fn png_chunks(d: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    if !d.starts_with(b"\x89PNG\r\n\x1a\n") {
        return out;
    }
    let mut p = 8;
    while p + 12 <= d.len() {
        let len = u32::from_be_bytes(d[p..p + 4].try_into().unwrap()) as usize;
        if len > d.len() - p - 12 {
            break;
        }
        let kind: [u8; 4] = d[p + 4..p + 8].try_into().unwrap();
        let crc = u32::from_be_bytes(d[p + 8 + len..p + 12 + len].try_into().unwrap());
        if crate::gpt::crc32(&d[p + 4..p + 8 + len]) != crc {
            break;
        }
        out.push((kind, &d[p..p + 12 + len]));
        if &kind == b"IEND" {
            break;
        }
        p += 12 + len;
    }
    out
}

/// What a PNG said about itself - when it was made (tIME, and its text and
/// EXIF, which can hold the date it was taken), its text, its colour
/// profile - that FFmpeg's new version of it lacks: the new version with
/// those chunks put in after its header. None: nothing to add.
fn png_keep_chunks(original: &[u8], made: &[u8]) -> Option<Vec<u8>> {
    const TEXT: [&[u8; 4]; 5] = [b"tEXt", b"zTXt", b"iTXt", b"eXIf", b"tIME"];
    const COLOUR: [&[u8; 4]; 4] = [b"iCCP", b"sRGB", b"gAMA", b"cHRM"];
    let new = png_chunks(made);
    if new.first().map(|c| &c.0) != Some(b"IHDR") || new.last().map(|c| &c.0) != Some(b"IEND") {
        return None;
    }
    let has = |k: &[u8; 4]| new.iter().any(|c| &c.0 == k);
    let colour_there = COLOUR.iter().any(|k| has(k));
    let add: Vec<&[u8]> = png_chunks(original)
        .into_iter()
        .filter(|(k, _)| {
            (TEXT.contains(&k) && !(k == b"tIME" && has(b"tIME")) && !(k == b"eXIf" && has(b"eXIf")))
                || (COLOUR.contains(&k) && !colour_there)
                || (k == b"pHYs" && !has(b"pHYs"))
        })
        .map(|(_, c)| c)
        .collect();
    if add.is_empty() {
        return None;
    }
    let header_end = 8 + new[0].1.len();
    let mut out = made[..header_end].to_vec();
    for c in add {
        out.extend_from_slice(c);
    }
    out.extend_from_slice(&made[header_end..]);
    Some(out)
}

fn fix_png(env: &Env, src: &Path, tmp: &Path) -> Try {
    let Some(ff) = env.ffmpeg.as_ref() else { return no("FFmpeg was not found on this PC", Next::Paid) };
    let out = tmp.join("repaired.png");
    let r = run_tool(
        ff,
        vec![
            "-hide_banner".into(),
            "-nostdin".into(),
            "-v".into(),
            "error".into(),
            "-err_detect".into(),
            "ignore_err".into(),
            "-i".into(),
            src.as_os_str().to_os_string(),
            "-frames:v".into(),
            "1".into(),
            "-y".into(),
            out.as_os_str().to_os_string(),
        ],
        None,
        300,
        env.stop,
    );
    if !r.is_some_and(|r| r.code == Some(0)) || !out.is_file() {
        return no("FFmpeg could not decode it at all", Next::Paid);
    }
    // What the PNG said about itself (its dates, text, colour profile) goes
    // into the new one too, if the new one still opens with it.
    if let (Ok(o), Ok(m)) = (std::fs::read(src), std::fs::read(&out)) {
        if let Some(with) = png_keep_chunks(&o, &m) {
            let p = tmp.join("repaired-with-its-details.png");
            if std::fs::write(&p, with).is_ok() && media_checks_out(env, &p, None, None).is_ok() {
                let _ = std::fs::rename(&p, &out);
            }
        }
    }
    match media_checks_out(env, &out, None, None) {
        Ok(note) => Try::Made {
            file: out,
            how: "what FFmpeg could decode of it was saved again as a PNG (lossless), so it opens; the part that was lost \
                  shows as it decoded"
                .to_string(),
            tool: tools::version(Tool::Ffmpeg, ff),
            salvage: false,
            note,
        },
        Err(e) => no(format!("FFmpeg's version does not check out ({e})"), Next::Paid),
    }
}

/// The map check's line for this file, as (lost byte ranges). None: nothing
/// of it was lost (every place never read lies past its end, or it is not on
/// the map check's list at all).
fn lost_ranges(env: &Env, item: &Item) -> Option<Vec<(u64, u64)>> {
    let it = crate::join::item_for_copy(&env.map_items, &item.path, item.size)?;
    if it.only_past_end {
        return None;
    }
    let r: Vec<(u64, u64)> = crate::join::parse_ranges(&it.ranges).into_iter().filter(|&(a, b)| b > a && a < item.size).collect();
    (!r.is_empty()).then_some(r)
}

fn find_twin(env: &Env, item: &Item, src: &Path, lost: &[(u64, u64)]) -> Option<PathBuf> {
    let cands = env.by_size.get(&item.size)?;
    let name = item.path.rsplit('\\').next().unwrap_or(&item.path).to_lowercase();
    // The same name first.
    let mut order: Vec<&PathBuf> = cands.iter().collect();
    order.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase() != name).unwrap_or(true));
    for c in order.into_iter().take(400) {
        if (env.stop)() {
            return None;
        }
        if let Some(rel) = repair::relative_to(env.work, &c.display().to_string().replace('/', "\\")) {
            if rel.eq_ignore_ascii_case(&item.path) || env.damaged.contains(&rel.to_lowercase()) {
                continue;
            }
        }
        if is_intact_twin(src, c, lost).unwrap_or(false) {
            return Some(c.clone());
        }
    }
    None
}

fn try_item(env: &Env, item: &Item, src: &Path, tmp: &Path) -> Try {
    if item.package {
        return no(
            "a document kept as a folder (Pages, Numbers, Keynote...): open it in its own app on the Mac",
            Next::Mac,
        );
    }
    let lost = lost_ranges(env, item);
    let read_whole = env.map_known && lost.is_none();
    let only_tools = only_by_tools(env, item);
    let t = try_format(env, item, src, tmp, lost, read_whole, only_tools);
    match t {
        Try::No { why, .. } if only_tools => Try::No {
            why: format!(
                "{why}. {READ_WHOLE}, so it is exactly as it was there: what the check found was so already (or is the \
                 checking tool's own limit), not damage from the drive's failure"
            ),
            next: Next::Nothing,
        },
        t => t,
    }
}

/// What the log says of a file read whole from the old drive (found only by
/// a checking tool): handoff.rs tells those apart by it.
pub const READ_WHOLE: &str = "Every byte of it was read from the old drive";

/// Read whole from the old drive (the map check: no place of it was never
/// read), and found only by one of the extra checks' tools: what the tool
/// finds was in the file already on the old drive (or is the tool's own
/// limit). It is tried all the same, but what a tool makes of it is kept
/// beside it (in salvaged), not put in its place, and it is not handed off:
/// the drive's failure did not touch it.
fn only_by_tools(env: &Env, item: &Item) -> bool {
    env.map_known
        && lost_ranges(env, item).is_none()
        && !item.why.is_empty()
        && item.why.split("; ").all(|r| r.starts_with("on the list auto-"))
}

fn try_format(env: &Env, item: &Item, src: &Path, tmp: &Path, lost: Option<Vec<(u64, u64)>>, read_whole: bool, only_tools: bool) -> Try {
    // 1. An intact copy elsewhere: the file as it was.
    if let Some(lost) = lost {
        if let Some(t) = find_twin(env, item, src, &lost) {
            let shown = repair::relative_to(env.work, &t.display().to_string().replace('/', "\\"))
                .map(|r| format!("WORK\\{r}"))
                .unwrap_or_else(|| t.display().to_string());
            let file = tmp.join("intact");
            if std::fs::copy(&t, &file).is_ok() {
                return Try::Made {
                    file,
                    how: format!(
                        "an intact copy of the same file was found at {shown}: identical in every byte the old drive did give \
                         up, and holding the {} bytes it never did. This is the file as it was",
                        commas(lost.iter().map(|(a, b)| b - a).sum::<u64>())
                    ),
                    tool: OWN.to_string(),
                    salvage: false,
                    note: String::new(),
                };
            }
        }
    }
    // 2. The format's own repair. Every tool works on a copy of the copy,
    // with a short, plain name in a folder of its own (some older tools
    // cannot open long paths, or names in other alphabets).
    let name = item.path.rsplit('\\').next().unwrap_or(&item.path);
    let ext = ext_lower(name);
    let e = ext.as_str();
    let input = tmp.join(if e.is_empty() { "input".to_string() } else { format!("input.{e}") });
    if std::fs::copy(src, &input).is_err() {
        return no("a working copy of it could not be made in REPAIR", Next::Paid);
    }
    let src = input.as_path();
    let damage = item.why.as_str();
    if ["jpg", "jpeg", "jpe", "jfif"].contains(&e) {
        let t = fix_jpeg(env, src, tmp, read_whole);
        if let Try::No { why, .. } = &t {
            if only_tools || read_whole {
                return t;
            }
            if let Some(Try::Made { file, how, tool, note, .. }) = salvage_preview(src, tmp, 4096, "the small preview the camera saved inside it") {
                return Try::Made { file, how, tool, salvage: true, note: if note.is_empty() { why.clone() } else { note } };
            }
        }
        return t;
    }
    if VIDEO_ISO.contains(&e) || e == "m4a" {
        return fix_isobmff(env, item, src, tmp, damage);
    }
    if VIDEO_OTHER.contains(&e) {
        return fix_other_video(env, src, tmp, damage);
    }
    match e {
        "mp3" => return fix_mp3(env, src, tmp),
        "flac" => return fix_reencode_flac(env, src, tmp),
        "ogg" | "oga" | "opus" | "aac" | "wma" => return fix_remux_audio(env, src, tmp),
        "wav" => return fix_wav(env, src, tmp),
        "pdf" => return fix_pdf(env, src, tmp),
        "png" => return fix_png(env, src, tmp),
        _ => {}
    }
    if ZIP_LIKE.contains(&e) {
        return fix_zip_like(env, item, src, tmp);
    }
    if ARCHIVES.contains(&e) {
        return salvage_archive(env, src, tmp);
    }
    if is_raw(e) {
        if let Some(t) = salvage_preview(src, tmp, 64 * 1024, "the largest preview the camera saved inside it") {
            if let Try::Made { file, how, tool, .. } = t {
                return Try::Made {
                    file,
                    how,
                    tool,
                    salvage: true,
                    note: "the full picture: on the Mac, Preview can export what it decodes as a TIFF (the hand-off does this)".to_string(),
                };
            }
        }
        return no("a camera RAW: on the Mac, Preview can export what it decodes as a TIFF (the hand-off does this)", Next::Mac);
    }
    if ["heic", "heif", "hif", "avif"].contains(&e) {
        return no(
            "an iPhone photo: on the Mac, Preview can save again what it decodes (the hand-off does this)",
            Next::Mac,
        );
    }
    if OLD_OFFICE.contains(&e) {
        return no("Word, Excel or PowerPoint can try Open and Repair on it", Next::OwnApp);
    }
    if MAC_DOCS.contains(&e) {
        return no("a Mac document: open it in its own app on the Mac", Next::Mac);
    }
    no("no open-source tool here repairs this kind of file: open it in its own program", Next::Paid)
}

// ---------------------------------------------------------------- the run

const LOG_HEADER: [&str; 9] = ["when", "id", "path_in_work", "result", "how", "tool", "output", "next", "note"];

fn append_log(repair: &Path, rows: &[Attempt]) -> Result<PathBuf, String> {
    use std::io::Write;
    let p = repair.join(LOG_NAME);
    let new = !p.exists();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .map_err(|e| format!("could not write {} ({e}) - is it open in another program?", p.display()))?;
    let mut text = if new { format!("\u{FEFF}{}", csv_row(&LOG_HEADER)) } else { String::new() };
    let when = crate::fmt::when_now();
    for a in rows {
        text.push_str(&csv_row(&[
            when.clone(),
            a.id.clone(),
            a.path.clone(),
            a.fate.word().to_string(),
            a.how.clone(),
            a.tool.clone(),
            a.output.clone(),
            a.next.key().to_string(),
            a.note.clone(),
        ]));
    }
    f.write_all(text.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| format!("could not write {}: {e}", p.display()))?;
    Ok(p)
}

/// The latest attempt for each file (by id), from every run's log.
pub fn read_log(repair: &Path) -> Vec<Attempt> {
    let mut by: Vec<Attempt> = Vec::new();
    let Ok(f) = std::fs::File::open(repair.join(LOG_NAME)) else { return by };
    let _ = crate::fmt::for_each_csv_row(std::io::BufReader::new(f), |r| {
        if r.len() < 9 || r[0] == "when" {
            return;
        }
        let a = Attempt {
            id: r[1].clone(),
            path: r[2].clone(),
            group: String::new(),
            fate: match r[3].as_str() {
                "REPAIRED" => Fate::Repaired,
                "SALVAGED" => Fate::Salvaged,
                _ => Fate::NotRepaired,
            },
            how: r[4].clone(),
            tool: r[5].clone(),
            output: r[6].clone(),
            next: Next::from_key(&r[7]),
            note: r[8].clone(),
        };
        match by.iter_mut().find(|x| x.id == a.id) {
            Some(x) => *x = a,
            None => by.push(a),
        }
    });
    by
}

/// A name for something kept in salvaged: "IMG_0003 [R0012] (preview).jpg".
fn salvage_name(copy_name: &str, made: &Path) -> String {
    let made_ext = if made.is_dir() { String::new() } else { ext_lower(&made.to_string_lossy()) };
    let stem = stem_of(copy_name);
    let name = made.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let what = if made.is_dir() {
        "what could be read"
    } else if name.starts_with("preview") {
        "preview"
    } else if ["repaired", "remuxed", "rebuilt"].iter().any(|s| name.starts_with(s)) {
        "rebuilt"
    } else {
        "what could be read"
    };
    if made_ext.is_empty() {
        format!("{stem} ({what})")
    } else {
        format!("{stem} ({what}).{made_ext}")
    }
}

fn move_into(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(p) = to.parent() {
        std::fs::create_dir_all(p)?;
    }
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{} is there already", to.display())));
    }
    std::fs::rename(from, to)
}

/// Try every damaged file copied out and not repaired yet.
pub fn run(
    home: &Path,
    opts: &Options,
    say: &mut dyn FnMut(&str),
    amount: &mut dyn FnMut(u64, u64),
    stop: &(dyn Fn() -> bool + Sync),
) -> Result<Outcome, String> {
    let work = &opts.work;
    let repair = repair::repair_dir(work)?;
    let items = repair::read_list(&repair)?;
    let mut o = Outcome::default();
    if items.is_empty() {
        return Ok(o);
    }
    // What already has a repaired version, or was put back.
    let mut has_version: HashSet<String> = HashSet::new();
    let root = repair.join(repair::REPAIRED);
    crate::walk::walk(&root.display().to_string(), |_, dirs, files| {
        for n in files.iter().chain(dirs.iter()) {
            if n.starts_with("._") {
                continue;
            }
            if let Some(id) = repair::tag_in(n) {
                has_version.insert(id);
            }
        }
        true
    });
    let back = repair::returned_now(work);
    let previous: HashMap<String, Attempt> = read_log(&repair).into_iter().map(|a| (a.id.clone(), a)).collect();
    let todo: Vec<&Item> = items
        .iter()
        .filter(|i| {
            let done = has_version.contains(&i.id) || back.get(&i.path.to_lowercase()).is_some_and(|n| n.repaired);
            if done {
                o.already += 1;
            }
            !done
        })
        .collect();
    // Something salvaged before is not salvaged again.
    let salvaged_before: HashSet<String> =
        previous.values().filter(|a| a.fate == Fate::Salvaged && !a.output.is_empty()).map(|a| a.id.clone()).collect();
    if todo.is_empty() {
        return Ok(o);
    }
    let map_text = opts.map_check.as_ref().and_then(|d| std::fs::read_to_string(d.join(crate::join::TSV_NAME)).ok());
    let map_known = map_text.is_some();
    let map_items = map_text.map(|t| crate::join::parse_tsv(&t)).unwrap_or_default();
    let damaged: HashSet<String> = items.iter().map(|i| i.path.to_lowercase()).collect();
    let mut env = Env {
        home,
        work,
        map_items,
        map_known,
        damaged,
        by_size: HashMap::new(),
        videos: std::cell::RefCell::new(None),
        ffmpeg: tools::find(Tool::Ffmpeg, home),
        ffprobe: tools::find(Tool::Ffprobe, home),
        sevenzip: tools::find(Tool::SevenZip, home),
        mp3val: tools::find(Tool::Mp3val, home),
        untrunc: tools::find(Tool::Untrunc, home),
        qpdf: tools::find(Tool::Qpdf, home),
        stop,
    };
    let _ = env.home;
    for (t, p) in [
        (Tool::Ffmpeg, &env.ffmpeg),
        (Tool::SevenZip, &env.sevenzip),
        (Tool::Mp3val, &env.mp3val),
        (Tool::Untrunc, &env.untrunc),
        (Tool::Qpdf, &env.qpdf),
    ] {
        match p {
            Some(p) => say(&format!("{}: {}", t.name(), tools::version(t, p))),
            None => say(&format!("{}: not found (what it repairs is left for the hand-off)", t.name())),
        }
    }
    // The twins: every file of a size some damaged file has, in WORK and in
    // the folders named.
    let sizes: HashSet<u64> = todo.iter().filter(|i| !i.package && lost_ranges(&env, i).is_some()).map(|i| i.size).collect();
    if !sizes.is_empty() {
        say(&format!(
            "Looking for intact copies of {} damaged files, in WORK{}...",
            commas(sizes.len() as u64),
            if opts.search.is_empty() { String::new() } else { format!(" and {} more folders", opts.search.len()) }
        ));
        // (REPAIR is not looked in: its copies are of the damaged files.)
        let key = |p: &str| p.replace('/', "\\").trim_start_matches(r"\\?\").trim_end_matches('\\').to_lowercase();
        let repair_key = key(&repair.display().to_string());
        for root in std::iter::once(work.clone()).chain(opts.search.iter().cloned()) {
            crate::walk::walk(&root.display().to_string(), |dir, dirs, files| {
                dirs.retain(|d| !crate::formats::SYSTEM_DIRS.contains(&d.as_str()));
                let k = key(dir);
                if k == repair_key || k.starts_with(&format!("{repair_key}\\")) {
                    dirs.clear();
                    return !stop();
                }
                for f in files {
                    let full = crate::walk::literal(&crate::walk::join(dir, f));
                    if let Ok(m) = std::fs::metadata(&full) {
                        if sizes.contains(&m.len()) {
                            env.by_size.entry(m.len()).or_default().push(full);
                        }
                    }
                }
                !stop()
            });
        }
    }
    let temp_root = repair.join(TEMP_DIR);
    let total = todo.len() as u64;
    amount(0, total);
    // (The page shows damaged files tried of all there are, by kind.)
    crate::live::begin("tried");
    for item in &todo {
        crate::live::to_do(&item.path, item.size);
    }
    for (n, item) in todo.iter().enumerate() {
        if stop() {
            o.stopped = true;
            break;
        }
        crate::live::now(&item.path);
        let src = under(&repair, &item.copied_as);
        let copy_name = item.copied_as.rsplit('\\').next().unwrap_or(&item.copied_as).to_string();
        let tmp = temp_root.join(&item.id);
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::create_dir_all(&tmp);
        let mut a = Attempt {
            id: item.id.clone(),
            path: item.path.clone(),
            group: item.group.clone(),
            fate: Fate::NotRepaired,
            how: String::new(),
            tool: String::new(),
            output: String::new(),
            next: Next::Nothing,
            note: String::new(),
        };
        if !src.exists() {
            a.how = format!("its copy {} is not in REPAIR any more", item.copied_as);
            a.next = Next::Paid;
        } else {
            let whole_only = only_by_tools(&env, item);
            // (One file the repairs cannot cope with - a parser's mistake on
            // something no test had - is left for the hand-off, not the end
            // of every repair after it: the stage would stop there each time.)
            let tried = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| try_item(&env, item, &src, &tmp)))
                .unwrap_or_else(|p| {
                    let why = p
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_default();
                    no(format!("the repair hit an internal error on this file ({why}): left for the hand-off"), Next::Paid)
                });
            match tried {
                Try::Made { file, how, tool, salvage, note } if whole_only && !salvage => {
                    // Exactly as it was on the old drive: it stays in WORK.
                    let rel = format!("{}\\{AUTO_DIR}\\{}", repair::SALVAGED, salvage_name(&copy_name, &file));
                    let dest = under(&repair, &rel);
                    let kept = std::fs::symlink_metadata(&dest).is_ok() || move_into(&file, &dest).is_ok();
                    if kept {
                        repair::give_dates(&under(work, &item.path), &dest);
                        a.fate = Fate::Salvaged;
                        a.how = format!(
                            "{how}. {READ_WHOLE}, so it stays in WORK exactly as it was there, and this version is kept \
                             beside it in REPAIR"
                        );
                        a.tool = tool;
                        a.output = rel;
                        a.note = note;
                    } else {
                        a.how = format!("{how}, but it could not be saved into REPAIR");
                    }
                    a.next = Next::Nothing;
                }
                Try::Made { file, how, tool, salvage, note } => {
                    if salvage && salvaged_before.contains(&item.id) {
                        a.how = format!("{how} (kept in salvaged before)");
                        a.fate = Fate::Salvaged;
                        a.output = previous.get(&item.id).map(|p| p.output.clone()).unwrap_or_default();
                        a.next = Next::from_key(&previous.get(&item.id).map(|p| p.next.key()).unwrap_or("paid").to_string());
                    } else {
                        let rel = if salvage {
                            format!("{}\\{AUTO_DIR}\\{}", repair::SALVAGED, salvage_name(&copy_name, &file))
                        } else {
                            format!("{}\\{AUTO_DIR}\\{copy_name}", repair::REPAIRED)
                        };
                        let dest = under(&repair, &rel);
                        let same = !salvage && sha_of(&file).is_some_and(|h| h == item.sha256);
                        let next_after_salvage = || match ext_lower(&copy_name).as_str() {
                            e if is_raw(e) => Next::Mac,
                            e if ZIP_LIKE.contains(&e) && !matches!(e, "zip" | "jar") => Next::OwnApp,
                            "jpg" | "jpeg" | "jpe" | "jfif" => Next::Paid,
                            _ => Next::Nothing,
                        };
                        if same {
                            a.how = "the repair changed nothing".to_string();
                            a.next = Next::Paid;
                        } else if salvage && std::fs::symlink_metadata(&dest).is_ok() {
                            // Salvaged by a run that was cut off before it
                            // could write its log: what it kept stays.
                            a.fate = Fate::Salvaged;
                            a.how = format!("{how} (kept in salvaged before)");
                            a.tool = tool;
                            a.output = rel;
                            a.note = note;
                            a.next = next_after_salvage();
                        } else {
                            match move_into(&file, &dest) {
                                Ok(()) => {
                                    // The file's own dates (in REPAIR, and
                                    // in WORK once it is put back).
                                    repair::give_dates(&under(work, &item.path), &dest);
                                    a.fate = if salvage { Fate::Salvaged } else { Fate::Repaired };
                                    a.how = how;
                                    a.tool = tool;
                                    a.output = rel;
                                    a.note = note;
                                    a.next = if salvage { next_after_salvage() } else { Next::Done };
                                }
                                Err(e) => {
                                    a.how = format!("{how}, but it could not be saved into REPAIR ({e})");
                                    a.next = Next::Paid;
                                }
                            }
                        }
                    }
                }
                Try::No { why, next } => {
                    a.how = why;
                    a.next = next;
                }
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
        say(&format!("{}  {}  {}", a.id, a.fate.word(), a.path));
        // Logged at once: a run cut off (the window closed, a power cut)
        // keeps what it did, and carries on after it.
        o.log = Some(append_log(&repair, std::slice::from_ref(&a))?);
        // ("What just happened", and the page's photos before and after.)
        crate::live::repaired(&a.id, &a.path, a.fate.word(), &a.how, &a.output);
        o.attempts.push(a);
        crate::live::done(&item.path, item.size);
        amount(n as u64 + 1, total);
    }
    let _ = std::fs::remove_dir_all(&temp_root);
    Ok(o)
}

pub fn summary(o: &Outcome) -> Vec<String> {
    let mut t = Vec::new();
    let (r, s, n) = (o.count(Fate::Repaired), o.count(Fate::Salvaged), o.count(Fate::NotRepaired));
    if o.attempts.is_empty() {
        t.push(if o.already > 0 {
            format!("Nothing new to repair: {} damaged files were repaired before.", commas(o.already as u64))
        } else {
            "No damaged files to repair.".to_string()
        });
    } else {
        t.push(format!(
            "Tried {} damaged files: {} repaired, {} with something salvaged, {} not repaired.",
            commas(o.attempts.len() as u64),
            commas(r as u64),
            commas(s as u64),
            commas(n as u64)
        ));
        let twins = o.attempts.iter().filter(|a| a.how.starts_with("an intact copy")).count();
        if twins > 0 {
            t.push(format!(
                "{} of them were restored exactly, from an intact copy of the same file found elsewhere.",
                commas(twins as u64)
            ));
        }
        let as_before = o.attempts.iter().filter(|a| a.fate != Fate::Repaired && a.how.contains(READ_WHOLE)).count();
        let nothing =
            o.attempts.iter().filter(|a| a.fate == Fate::NotRepaired && a.next == Next::Nothing && !a.how.contains(READ_WHOLE)).count();
        if nothing > 0 {
            t.push(format!(
                "{} open or play as they are, with a glitch where data was lost: nothing can bring that part back.",
                commas(nothing as u64)
            ));
        }
        if as_before > 0 {
            t.push(format!(
                "{} were read whole from the old drive, so they are exactly as they were on it: what a checking tool found in \
                 them was so already. They stay as they are in WORK (anything a tool made of them is kept in REPAIR\\salvaged).",
                commas(as_before as u64)
            ));
        }
        let mac = o.attempts.iter().filter(|a| a.fate != Fate::Repaired && a.next == Next::Mac).count();
        let paid = o.attempts.iter().filter(|a| a.fate != Fate::Repaired && a.next == Next::Paid).count();
        let own = o.attempts.iter().filter(|a| a.fate != Fate::Repaired && a.next == Next::OwnApp).count();
        if mac + paid + own > 0 {
            t.push(format!(
                "Left for the hand-off: {} for the Mac, {} for a paid repair tool, {} for their own program's repair.",
                commas(mac as u64),
                commas(paid as u64),
                commas(own as u64)
            ));
        }
    }
    if o.already > 0 && !o.attempts.is_empty() {
        t.push(format!("{} were repaired before and left as they are.", commas(o.already as u64)));
    }
    if o.stopped {
        t.push("Stopped before the end: Start again to carry on.".to_string());
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_jpeg() -> Vec<u8> {
        // SOI, APP0, DQT-ish segment, SOS, some data with stuffing and a restart, EOI.
        let mut d = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        d.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        d.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 1, 1, 0, 0, 0x3F, 0]);
        d.extend_from_slice(&[0x12, 0x34, 0xFF, 0x00, 0x56, 0xFF, 0xD0, 0x78]);
        d.extend_from_slice(&[0xFF, 0xD9]);
        d
    }

    #[test]
    fn jpeg_parts_are_found() {
        let d = tiny_jpeg();
        assert_eq!(jpeg_layout(&d), JpegLayout::Complete(d.len()));
        // Its end marker lost, and zeros where its last part was never read.
        let cut = &d[..d.len() - 2];
        assert_eq!(jpeg_layout(cut), JpegLayout::CutShort);
        let mut zeros = cut.to_vec();
        zeros.extend(std::iter::repeat(0).take(4096));
        assert_eq!(jpeg_layout(&zeros), JpegLayout::CutShort);
        let fixed = jpeg_with_end(&zeros).unwrap();
        assert_eq!(fixed, d);
        // Cut in the middle of a marker: the half marker is left off.
        let half = &d[..d.len() - 4];
        assert_eq!(half.last(), Some(&0xFF));
        let fixed = jpeg_with_end(half).unwrap();
        assert_eq!(fixed.len(), half.len() + 1);
        assert_eq!(jpeg_layout(&fixed), JpegLayout::Complete(fixed.len()));
        assert_eq!(jpeg_layout(b"not a jpeg at all"), JpegLayout::Broken);
        assert!(jpeg_with_end(&d).is_none());
    }

    /// The tiny JPEG with two bytes of its picture data changed into a
    /// marker that is not one (FF C4, a length that lands mid-data), as a
    /// bad read or a flipped bit leaves it; its end still there.
    fn garbled_jpeg() -> Vec<u8> {
        let mut d = tiny_jpeg();
        let scan = d.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap() + 10;
        d.splice(scan..scan + 2, [0xFF, 0xC4, 0x00, 0x03]);
        d
    }

    fn bare_env<'a>(work: &'a Path, stop: &'a (dyn Fn() -> bool + Sync)) -> Env<'a> {
        Env {
            home: work,
            work,
            map_items: Vec::new(),
            map_known: false,
            damaged: HashSet::new(),
            by_size: HashMap::new(),
            videos: std::cell::RefCell::new(None),
            ffmpeg: None,
            ffprobe: None,
            sevenzip: None,
            mp3val: None,
            untrunc: None,
            qpdf: None,
            stop,
        }
    }

    /// A stand-in for FFmpeg: as many errors for a file as the ".errors"
    /// file beside it says, and it plays.
    #[cfg(unix)]
    fn fake_ffmpeg(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let ff = dir.join("ffmpeg");
        std::fs::write(
            &ff,
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do if [ \"$1\" = -i ]; then f=\"$2\"; fi; shift; done\n\
             n=$(cat \"$f.errors\" 2>/dev/null || echo 0)\ni=0\n\
             while [ $i -lt $n ]; do echo \"[mpeg4 @ 0x1] error $i in the picture data\" >&2; i=$((i+1)); done\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&ff, std::fs::Permissions::from_mode(0o755)).unwrap();
        ff
    }

    #[cfg(unix)]
    #[test]
    fn a_container_rebuilt_round_the_same_errors_counts_only_where_the_old_one_was_broken() {
        let dir = std::env::temp_dir().join(format!("rp-ar-same-errors-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let stop = || false;
        let mut env = bare_env(&dir, &stop);
        env.ffmpeg = Some(fake_ffmpeg(&dir));
        let (made, broken, whole) = (dir.join("made.jpg"), dir.join("broken.jpg"), dir.join("whole.jpg"));
        let d = tiny_jpeg();
        std::fs::write(&made, &d).unwrap();
        std::fs::write(&whole, &d).unwrap();
        std::fs::write(&broken, &d[..d.len() - 2]).unwrap();
        assert!(probe_ok(&made).is_ok() && probe_ok(&broken).is_err());
        let errors = |p: &Path, n: usize| std::fs::write(format!("{}.errors", p.display()), n.to_string()).unwrap();
        for p in [&made, &broken, &whole] {
            errors(p, 3);
        }
        let was_broken = plays(&env, &broken).unwrap();
        let was_whole = plays(&env, &whole).unwrap();
        assert_eq!((was_broken.0, was_broken.1.len()), (true, 3));
        // As many errors, where the damaged copy's structure was broken and
        // this one's is right: it counts.
        assert!(media_checks_out(&env, &made, Some(&was_broken), Some(&broken)).is_ok());
        // ...and not where the damaged copy's was right too: nothing gained.
        assert!(media_checks_out(&env, &made, Some(&was_whole), Some(&whole)).is_err());
        assert!(media_checks_out(&env, &made, Some(&was_broken), None).is_err());
        // More errors than the damaged copy: never; fewer: always.
        errors(&made, 4);
        assert!(media_checks_out(&env, &made, Some(&was_broken), Some(&broken)).is_err());
        errors(&made, 2);
        assert!(media_checks_out(&env, &made, Some(&was_whole), Some(&whole)).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_photo_damaged_in_the_middle_is_not_taken_for_one_cut_short() {
        let d = garbled_jpeg();
        assert_eq!(jpeg_layout(&d), JpegLayout::Garbled);
        assert!(jpeg_with_end(&d).is_none(), "its end is there: nothing to put back");
        // Its end lost as well: cut short, but an end put back does not make it whole.
        let cut = &d[..d.len() - 2];
        assert_eq!(jpeg_layout(cut), JpegLayout::CutShort);
        let fixed = jpeg_with_end(cut).unwrap();
        assert_ne!(jpeg_layout(&fixed), JpegLayout::Complete(fixed.len()));
        // The not-a-marker followed by zeros (never read) to the end: cut short.
        let mut zeros = d[..d.len() - 4].to_vec();
        zeros.extend(std::iter::repeat(0).take(4096));
        assert_eq!(jpeg_layout(&zeros), JpegLayout::CutShort);
        // What the repair says of each.
        let dir = std::env::temp_dir().join(format!("rp-ar-garbled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let stop = || false;
        let env = bare_env(&dir, &stop);
        for (bytes, read_whole, want) in [
            (d.clone(), false, "damaged partway through, but its end is there"),
            (d.clone(), true, "every byte of it was read from the old drive"),
            (cut.to_vec(), false, "an end marker put back does not make it whole"),
        ] {
            let src = dir.join("input.jpg");
            std::fs::write(&src, &bytes).unwrap();
            match fix_jpeg(&env, &src, &dir, read_whole) {
                Try::No { why, .. } => assert!(why.contains(want), "{why}"),
                Try::Made { how, .. } => panic!("taken as repaired: {how}"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn previews_inside_a_file_are_found() {
        let mut big = tiny_jpeg();
        // Make it count as a preview (at least 2 KB).
        let at = big.len() - 2;
        big.splice(at..at, std::iter::repeat(0x11).take(3000));
        let mut raw = b"II*\0 a camera RAW's header ".to_vec();
        raw.extend(std::iter::repeat(7).take(500));
        let start = raw.len();
        raw.extend_from_slice(&big);
        raw.extend(std::iter::repeat(9).take(100));
        let found = embedded_jpegs(&raw);
        assert_eq!(found, vec![(start, start + big.len())]);
    }

    #[test]
    fn a_photos_own_small_preview_is_found_inside_it() {
        // The camera's notes (APP1, EXIF) hold a small JPEG of the photo; the
        // photo itself is whole.
        let mut thumb = tiny_jpeg();
        let at = thumb.len() - 2;
        thumb.splice(at..at, std::iter::repeat(0x22).take(5000));
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&thumb);
        let mut photo = vec![0xFF, 0xD8, 0xFF, 0xE1];
        photo.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        photo.extend_from_slice(&app1);
        let mut own = tiny_jpeg();
        let at = own.len() - 2;
        own.splice(at..at, std::iter::repeat(0x33).take(20_000));
        photo.extend_from_slice(&own[2..]);
        assert_eq!(jpeg_layout(&photo), JpegLayout::Complete(photo.len()));
        assert_eq!(embedded_jpegs(&photo), vec![(0, photo.len()), (12, 12 + thumb.len())]);
        // What is salvaged of it is that preview, not the photo itself.
        let dir = std::env::temp_dir().join(format!("rp-ar-thumb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("input.jpg");
        std::fs::write(&src, &photo).unwrap();
        match salvage_preview(&src, &dir, 4096, "the small preview the camera saved inside it") {
            Some(Try::Made { file, salvage, .. }) => {
                assert!(salvage);
                assert_eq!(std::fs::read(file).unwrap(), thumb);
            }
            _ => panic!("no preview taken out"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_of_what_only_looks_like_jpeg_starts_is_not_searched_for_ever() {
        // FF D8 over and over: each one looks like a JPEG's start, and the
        // walk from it goes on to the end of them all (before: 4 MB took hours).
        let mut d = [0xFF, 0xD8].repeat(2 << 20);
        let t = std::time::Instant::now();
        assert!(embedded_jpegs(&d).is_empty());
        assert!(t.elapsed().as_secs() < 20, "{:?}", t.elapsed());
        // A real preview in a real file is still found (previews_inside_a_file_are_found),
        // and one after a run that is short enough too.
        let mut big = tiny_jpeg();
        let at = big.len() - 2;
        big.splice(at..at, std::iter::repeat(0x11).take(3000));
        d.truncate(20_000);
        let start = d.len() + 7;
        d.extend_from_slice(b"padding");
        d.extend_from_slice(&big);
        assert_eq!(embedded_jpegs(&d), vec![(start, start + big.len())]);
    }

    #[test]
    fn wav_headers_are_set_right() {
        let mut w = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&[1, 0, 1, 0, 0x44, 0xAC, 0, 0, 0x88, 0x58, 1, 0, 2, 0, 16, 0]);
        w.extend_from_slice(b"data");
        w.extend_from_slice(&1_000_000u32.to_le_bytes());
        w.extend(std::iter::repeat(3).take(1000));
        let f = wav_fixed(&w).unwrap();
        assert_eq!(u32::from_le_bytes([f[4], f[5], f[6], f[7]]) as usize, f.len() - 8);
        assert_eq!(u32::from_le_bytes([f[40], f[41], f[42], f[43]]), 1000);
        assert!(wav_fixed(&f).is_none());
    }

    #[test]
    fn an_intact_twin_is_told_from_a_copy_of_the_damage() {
        let dir = std::env::temp_dir().join(format!("arp-twin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let original: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let mut damaged = original.clone();
        for b in &mut damaged[50_000..54_096] {
            *b = 0;
        }
        let lost = [(50_000u64, 54_096u64)];
        let (d, t, c, other) = (dir.join("d"), dir.join("t"), dir.join("c"), dir.join("o"));
        std::fs::write(&d, &damaged).unwrap();
        std::fs::write(&t, &original).unwrap();
        std::fs::write(&c, &damaged).unwrap();
        let mut o = original.clone();
        o[10] ^= 1;
        std::fs::write(&other, &o).unwrap();
        assert!(is_intact_twin(&d, &t, &lost).unwrap());
        assert!(!is_intact_twin(&d, &c, &lost).unwrap(), "a copy of the damaged file is no help");
        assert!(!is_intact_twin(&d, &other, &lost).unwrap(), "a different file is not a twin");
        assert!(!is_intact_twin(&d, &t, &[]).unwrap());
        assert_eq!(read_parts(100, &[(10, 20), (15, 30), (90, 120)]), vec![(0, 10), (30, 90)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_lost_pdf_trailer_is_written_again() {
        let pdf = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n12 0 obj\n<< >>\nend";
        let mut d = pdf.to_vec();
        d.extend(std::iter::repeat(0).take(600));
        let out = String::from_utf8(pdf_with_trailer(&d).unwrap()).unwrap();
        assert!(out.ends_with("trailer\n<< /Size 13 /Root 1 0 R >>\nstartxref\n0\n%%EOF\n"), "{out}");
        assert!(!out.contains('\0'));
        assert!(pdf_with_trailer(b"%PDF-1.4\n3 0 obj\n<< /Type /Page >>\nendobj\n").is_none());
        // Its information dictionary (when it was made and changed) is
        // pointed at too; a bookmark or a note with a date is not it.
        let mut pdf = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n"
            .to_vec();
        pdf.extend_from_slice(b"5 0 obj\n<< /Title (Bank) /Parent 4 0 R /Dest [3 0 R /Fit] >>\nendobj\n");
        pdf.extend_from_slice(b"6 0 obj\n<< /Type /Annot /Subtype /Text /CreationDate (D:20190101) >>\nendobj\n");
        pdf.extend_from_slice("7 0 obj\n<< /Producer (Scanner \u{e9}) /CreationDate (D:20190714152005+01'00') >>\nendobj\n".as_bytes());
        pdf.extend_from_slice(b"8 0 obj\n<< /Length 3 >>\nstream\n\xff\xfe\x00\nendstream\nendobj\n");
        pdf.extend(std::iter::repeat_n(0u8, 300));
        let out = String::from_utf8_lossy(&pdf_with_trailer(&pdf).unwrap()).into_owned();
        assert!(out.ends_with("trailer\n<< /Size 9 /Root 1 0 R /Info 7 0 R >>\nstartxref\n0\n%%EOF\n"), "{out}");
        assert_eq!(find_bytes(b"abcabd", b"abd"), Some(3));
        assert_eq!(find_bytes(b"ab", b"abc"), None);
        assert_eq!(find_bytes(b"xx", b""), None);
        // An object number as big as can be (a damaged one): no sum past it.
        let big = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n18446744073709551615 0 obj\n<< >>\nendobj\n";
        let out = String::from_utf8(pdf_with_trailer(big).unwrap()).unwrap();
        assert!(out.contains("/Size 18446744073709551615 /Root 1 0 R"), "{out}");
    }

    #[test]
    fn a_rebuilt_png_keeps_what_the_png_said_about_itself() {
        fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
            let mut c = (data.len() as u32).to_be_bytes().to_vec();
            c.extend_from_slice(kind);
            c.extend_from_slice(data);
            let crc = crate::gpt::crc32(&c[4..]);
            c.extend_from_slice(&crc.to_be_bytes());
            c
        }
        let sig = b"\x89PNG\r\n\x1a\n".to_vec();
        let ihdr = chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]);
        let time = chunk(b"tIME", &[0x07, 0xE3, 7, 14, 15, 20, 5]);
        let text = chunk(b"tEXt", b"Creation Time\x002019:07:14 15:20:05");
        let icc = chunk(b"iCCP", b"sRGB\x00\x00x");
        let idat = chunk(b"IDAT", &[1, 2, 3]);
        let iend = chunk(b"IEND", &[]);
        // The original: its details, then image data cut short (a CRC that fails).
        let mut original = [sig.clone(), ihdr.clone(), time.clone(), text.clone(), icc.clone(), idat.clone()].concat();
        let n = original.len();
        original[n - 2] ^= 0xFF;
        original.extend_from_slice(&iend);
        // FFmpeg's version: header, its own colour chunk, data, end.
        let srgb = chunk(b"sRGB", &[0]);
        let made = [sig.clone(), ihdr.clone(), srgb.clone(), idat.clone(), iend.clone()].concat();
        let out = png_keep_chunks(&original, &made).unwrap();
        // tIME and the text go in after the header; the colour profile not
        // (FFmpeg's version has its own).
        assert_eq!(out, [sig.clone(), ihdr.clone(), time.clone(), text.clone(), srgb, idat.clone(), iend.clone()].concat());
        let kinds: Vec<[u8; 4]> = png_chunks(&out).into_iter().map(|c| c.0).collect();
        assert_eq!(kinds, [*b"IHDR", *b"tIME", *b"tEXt", *b"sRGB", *b"IDAT", *b"IEND"]);
        // Nothing to add: None.
        assert!(png_keep_chunks(&made, &made).is_none());
        assert!(png_keep_chunks(&original, b"not a png").is_none());
        // Without a colour chunk of its own, it gets the original's.
        let plain = [sig, ihdr, idat, iend].concat();
        let kinds: Vec<[u8; 4]> = png_chunks(&png_keep_chunks(&original, &plain).unwrap()).into_iter().map(|c| c.0).collect();
        assert_eq!(kinds, [*b"IHDR", *b"tIME", *b"tEXt", *b"iCCP", *b"IDAT", *b"IEND"]);
    }

    #[test]
    fn a_video_rebuilt_from_a_pattern_gets_its_own_date_inside() {
        let dir = std::env::temp_dir().join(format!("own-moments-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let qt = |unix: i64| ((unix + 2_082_844_800) as u32).to_be_bytes();
        let video = |p: &Path, made: i64| {
            let mut mvhd = vec![0u8; 4];
            mvhd.extend_from_slice(&qt(made));
            mvhd.extend_from_slice(&qt(made));
            mvhd.extend(vec![0u8; 88]);
            let mut moov = ((mvhd.len() + 8) as u32).to_be_bytes().to_vec();
            moov.extend_from_slice(b"mvhd");
            moov.extend(mvhd);
            let mut file = ((moov.len() + 8) as u32).to_be_bytes().to_vec();
            file.extend_from_slice(b"moov");
            file.extend(moov);
            std::fs::write(p, file).unwrap();
        };
        let dated = |p: &Path, unix: i64| {
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(unix as u64);
            std::fs::OpenOptions::new().write(true).open(p).unwrap().set_times(std::fs::FileTimes::new().set_modified(t)).unwrap();
        };
        // The camera writes UTC inside, and its files are dated an hour
        // later (local summer time).
        let (made_p, file_p) = (1_689_411_600, 1_689_411_600 + 3600); // 2023-07-15 09:00Z, 10:00
        let file_d = 1_689_411_600 + 86_400 + 3 * 3600; // 2023-07-16 12:00
        let (pattern, made, original) = (dir.join("p.mov"), dir.join("made.mov"), dir.join("d.mov"));
        video(&pattern, made_p);
        dated(&pattern, file_p);
        video(&made, made_p); // untrunc's: the pattern's header
        std::fs::write(&original, b"the damaged video").unwrap();
        dated(&original, file_d);
        let note = own_moments(&original, &pattern, &made);
        assert!(note.contains("2023-07-16 11:00:00") && note.contains("2023-07-15 09:00:00"), "{note}");
        assert_eq!(crate::mp4time::made(&made), Some(file_d - 3600));
        // Already right: nothing to do.
        assert_eq!(own_moments(&original, &pattern, &made), "");
        // A pattern whose file date says nothing of when it was made (copied
        // since): the video's own file date, as it is.
        dated(&pattern, file_p + 40 * 86_400);
        video(&made, made_p);
        own_moments(&original, &pattern, &made);
        assert_eq!(crate::mp4time::made(&made), Some(file_d));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn camera_name_starts_are_compared() {
        assert_eq!(name_prefix("IMG_1234.MOV"), "img_");
        assert_eq!(name_prefix("GOPR0012.MP4"), "gopr");
        assert_eq!(name_prefix("20190101_120000.mp4"), "");
        assert_eq!(salvage_name("IMG_1 [R0003].CR2", Path::new("/x/preview.jpg")), "IMG_1 [R0003] (preview).jpg");
    }

    /// A zip of stored parts, made by hand. `streamed`: each part's sizes
    /// after its data, in a descriptor, as a program writing a stream does.
    fn zip_of(parts: &[(&str, &[u8])], streamed: bool) -> (Vec<u8>, usize) {
        let mut z = Vec::new();
        let mut cd = Vec::new();
        for (name, data) in parts {
            let off = z.len() as u32;
            let sizes = if streamed { 0 } else { data.len() as u32 };
            z.extend_from_slice(b"PK\x03\x04\x14\x00");
            z.extend_from_slice(&(if streamed { 8u16 } else { 0 }).to_le_bytes());
            z.extend_from_slice(&[0u8; 10]);
            z.extend_from_slice(&sizes.to_le_bytes());
            z.extend_from_slice(&sizes.to_le_bytes());
            z.extend_from_slice(&(name.len() as u16).to_le_bytes());
            z.extend_from_slice(&0u16.to_le_bytes());
            z.extend_from_slice(name.as_bytes());
            z.extend_from_slice(data);
            if streamed {
                z.extend_from_slice(b"PK\x07\x08\0\0\0\0");
                z.extend_from_slice(&(data.len() as u32).to_le_bytes());
                z.extend_from_slice(&(data.len() as u32).to_le_bytes());
            }
            cd.extend_from_slice(b"PK\x01\x02");
            cd.extend_from_slice(&[0u8; 24]);
            cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&[0u8; 12]);
            cd.extend_from_slice(&off.to_le_bytes());
            cd.extend_from_slice(name.as_bytes());
        }
        let at = z.len();
        z.extend_from_slice(&cd);
        z.extend_from_slice(b"PK\x05\x06");
        z.extend_from_slice(&[0u8; 18]);
        (z, at)
    }

    #[test]
    fn a_zips_parts_are_walked_without_its_index() {
        let dir = std::env::temp_dir().join(format!("arp-zipwalk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.docx");
        let image = vec![7u8; 3000];
        let parts: Vec<(&str, &[u8])> = vec![
            ("[Content_Types].xml", b"<Types/>"),
            ("word/", b""),
            ("word/document.xml", b"<w:document>The report.</w:document>"),
            ("word/media/image1.png", &image),
        ];
        let walk = |z: &[u8], lost: &[(u64, u64)]| {
            std::fs::write(&f, z).unwrap();
            zip_walk(&f, lost).unwrap()
        };
        for streamed in [false, true] {
            let (z, cd) = zip_of(&parts, streamed);
            let len = z.len() as u64;
            let every = ZipWalk { files: 3, reached_index: true };
            assert_eq!(walk(&z, &[]), every, "streamed {streamed}");
            // Its index lost from just after its first bytes: still every part.
            let mut zz = z.clone();
            zz[cd + 4..].fill(0);
            assert_eq!(walk(&zz, &[(cd as u64 + 4, len)]), every);
            // The index lost from its very start: whether more parts followed can't be told.
            assert_eq!(walk(&zz, &[(cd as u64, len)]), ZipWalk { files: 3, reached_index: false });
            // A hole in a part's data: the walk goes on (7-Zip's checksum of the part tells).
            if !streamed {
                assert_eq!(walk(&z, &[(cd as u64 - 1000, cd as u64 - 900)]), every);
            }
            // A part's header lost: the walk stops before it.
            let third = z.windows(4).enumerate().filter(|(_, w)| *w == b"PK\x03\x04").nth(2).unwrap().0 as u64;
            assert_eq!(walk(&z, &[(third + 10, third + 12)]), ZipWalk { files: 1, reached_index: false });
            // Cut short in the middle of the last part.
            assert_eq!(walk(&z[..cd - 100], &[]), ZipWalk { files: 2, reached_index: false });
        }
        // Not a zip at all.
        assert_eq!(walk(b"%PDF-1.4 and the rest", &[]), ZipWalk::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sevenzips_errors_about_parts_are_told_from_its_errors_about_the_zip() {
        let out = "\n7-Zip 26.03 (x64)\n\nScanning the drive for archives:\n1 file, 41000 bytes\n\nExtracting archive: C:\\R\\input.docx\n\
                   ERROR: C:\\R\\input.docx\nC:\\R\\input.docx\nOpen WARNING: Can not open the file as [zip] archive\n\n\
                   ERRORS:\nHeaders Error\n\n--\nPath = C:\\R\\input.docx\nType = zip\n\n\
                   ERROR: CRC Failed : word\\document.xml\r\nERROR: Data Error : word\\media\\image1.png\n\
                   Sub items Errors: 2\n\nArchives with Errors: 1\n\nOpen Errors: 1\n";
        assert_eq!(sevenzip_failed(out), vec!["word\\document.xml".to_string(), "word\\media\\image1.png".to_string()]);
        assert!(sevenzip_failed("ERRORS:\nHeaders Error\nUnexpected end of archive\n\nOpen Errors: 1\n").is_empty());
    }

    #[test]
    fn qpdfs_answers_are_read() {
        let damaged = "WARNING: input.pdf: file is damaged\nWARNING: input.pdf: can't find startxref\n\
                       WARNING: input.pdf (object 14 0, offset 8723): expected endstream\n\
                       qpdf: input.pdf: unable to find trailer dictionary while recovering damaged file\n";
        assert_eq!(qpdf_first_problem(damaged), "file is damaged");
        assert_eq!(
            qpdf_first_problem("WARNING: C:\\R\\out put.pdf (trailer, offset 2269): trailer dictionary lacks /Size key\n"),
            "trailer, offset 2269: trailer dictionary lacks /Size key"
        );
        assert_eq!(qpdf_first_problem("qpdf: x.pdf: unable to find trailer dictionary while recovering damaged file\n"),
                   "unable to find trailer dictionary while recovering damaged file");
        assert_eq!(qpdf_first_problem("checking a.pdf\nPDF Version: 1.4\nqpdf: operation succeeded with warnings\n"), "");
        let mut pdf = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
        assert!(pdf_trailer_lost(&pdf));
        pdf.extend_from_slice(b"xref\n0 1\ntrailer\n<< /Root 1 0 R >>\nstartxref\n42\n%%EOF\n");
        assert!(!pdf_trailer_lost(&pdf));
        pdf.extend(std::iter::repeat(0).take(5000));
        assert!(!pdf_trailer_lost(&pdf), "zeros after the end are not the end");
        let mut cut = pdf[..40].to_vec();
        cut.extend(std::iter::repeat(0).take(600));
        assert!(pdf_trailer_lost(&cut));
    }
}
