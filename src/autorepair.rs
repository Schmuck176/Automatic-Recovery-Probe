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
    /// Not a JPEG, or broken before the picture data.
    Broken,
}

/// Where a JPEG's parts are (the same walk as checks::jpeg_image_end, on bytes).
pub fn jpeg_layout(d: &[u8]) -> JpegLayout {
    let n = d.len();
    if n < 4 || d[0] != 0xFF || d[1] != 0xD8 {
        return JpegLayout::Broken;
    }
    let mut pos = 2usize;
    let mut in_scan = false;
    for _ in 0..1_000_000 {
        if pos >= n {
            return if in_scan { JpegLayout::CutShort } else { JpegLayout::Broken };
        }
        if d[pos] != 0xFF {
            return if in_scan { JpegLayout::CutShort } else { JpegLayout::Broken };
        }
        while pos < n && d[pos] == 0xFF {
            pos += 1;
        }
        if pos >= n {
            return if in_scan { JpegLayout::CutShort } else { JpegLayout::Broken };
        }
        let marker = d[pos];
        pos += 1;
        if marker == 0xD9 {
            return JpegLayout::Complete(pos);
        }
        if (0xD0..=0xD8).contains(&marker) || marker == 0x01 {
            continue;
        }
        if pos + 2 > n {
            return if in_scan { JpegLayout::CutShort } else { JpegLayout::Broken };
        }
        let seg = ((d[pos] as usize) << 8) | d[pos + 1] as usize;
        if seg < 2 || pos + seg > n {
            return if in_scan { JpegLayout::CutShort } else { JpegLayout::Broken };
        }
        pos += seg;
        if marker == 0xDA {
            in_scan = true;
            loop {
                let Some(i) = d[pos.min(n)..].iter().position(|&c| c == 0xFF) else { return JpegLayout::CutShort };
                let k = pos + i;
                if k + 1 >= n {
                    return JpegLayout::CutShort;
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
    JpegLayout::Broken
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
    while i + 3 < d.len() {
        let Some(k) = d[i..].windows(3).position(|w| w == [0xFF, 0xD8, 0xFF]) else { break };
        let start = i + k;
        if let JpegLayout::Complete(len) = jpeg_layout(&d[start..]) {
            if len >= 2048 {
                out.push((start, start + len));
                i = start + len;
                continue;
            }
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
    for (s, e) in read_parts(size, lost) {
        if !same_part(&mut a, &mut b, s, e)? {
            return Ok(false);
        }
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
/// through (when FFmpeg is there) with fewer errors than the damaged copy.
fn media_checks_out(env: &Env, made: &Path, before: Option<&(bool, Vec<String>)>) -> Result<String, String> {
    probe_ok(made)?;
    let Some((ok, errs)) = plays(env, made) else { return Ok(String::new()) };
    if !ok {
        return Err(format!("FFmpeg cannot play it: {}", errs.first().cloned().unwrap_or_default()));
    }
    if let Some((was_ok, was)) = before {
        if *was_ok && errs.len() >= was.len() && !was.is_empty() {
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

fn fix_jpeg(env: &Env, src: &Path, tmp: &Path) -> Try {
    let Ok(d) = std::fs::read(src) else { return no("it could not be read", Next::Paid) };
    match jpeg_layout(&d) {
        JpegLayout::CutShort => {
            let Some(out) = jpeg_with_end(&d) else { return no("its end could not be found", Next::Paid) };
            let file = tmp.join(format!("repaired.{}", ext_lower(&src.to_string_lossy())));
            if std::fs::write(&file, &out).is_err() {
                return no("the repaired version could not be written", Next::Paid);
            }
            match media_checks_out(env, &file, None) {
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
        JpegLayout::Complete(_) => no(
            "every part of the photo is in place, but some of its picture data was never read: it opens, with part of the \
             picture grey or garbled from that point; no tool can bring that part back (a photo repair tool's free preview \
             shows whether it does better)",
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
        match media_checks_out(env, &named, before.as_ref()) {
            Ok(note) => {
                let secs = duration_of(env, &named);
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

/// FFmpeg copies every stream into a new container of the same kind (the
/// index made again, broken pieces left out).
fn remux(env: &Env, src: &Path, tmp: &Path, before: Option<&(bool, Vec<String>)>) -> Option<Try> {
    let ff = env.ffmpeg.as_ref()?;
    let ext = ext_lower(&src.to_string_lossy());
    let out = tmp.join(format!("remuxed.{ext}"));
    let r = run_tool(
        ff,
        vec![
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
            "-y".into(),
            out.as_os_str().to_os_string(),
        ],
        None,
        1800,
        env.stop,
    )?;
    if r.code != Some(0) || !out.is_file() {
        return None;
    }
    let note = media_checks_out(env, &out, before).ok()?;
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
    match media_checks_out(env, &work, before.as_ref()) {
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

fn fix_reencode_flac(env: &Env, src: &Path, tmp: &Path) -> Try {
    let Some(ff) = env.ffmpeg.as_ref() else { return no("FFmpeg was not found on this PC", Next::Nothing) };
    let before = plays(env, src);
    if let Some((true, e)) = &before {
        if e.is_empty() {
            return no("FFmpeg plays it through without an error: nothing to repair", Next::Nothing);
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
    match media_checks_out(env, &out, before.as_ref()) {
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
            return no("FFmpeg plays it through without an error: nothing to repair", Next::Nothing);
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
            match media_checks_out(env, &file, None) {
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

fn qpdf_check(qpdf: &Path, p: &Path, stop: &dyn Fn() -> bool) -> Option<i32> {
    run_tool(qpdf, vec!["--check".into(), p.as_os_str().to_os_string()], None, 600, stop).and_then(|r| r.code)
}

fn fix_pdf(env: &Env, src: &Path, tmp: &Path) -> Try {
    let Some(qpdf) = env.qpdf.as_ref() else {
        return no("qpdf (which rebuilds PDFs) was not found on this PC", Next::Paid);
    };
    let before = qpdf_check(qpdf, src, env.stop);
    let out = tmp.join("repaired.pdf");
    let r = run_tool(qpdf, vec![src.as_os_str().to_os_string(), out.as_os_str().to_os_string()], None, 900, env.stop);
    let made = r.as_ref().is_some_and(|r| matches!(r.code, Some(0) | Some(3))) && out.is_file();
    if !made {
        return no(
            format!(
                "qpdf could not rebuild it{}",
                r.map(|r| format!(": {}", r.text().lines().next().unwrap_or(""))).unwrap_or_default()
            ),
            Next::Paid,
        );
    }
    let after = qpdf_check(qpdf, &out, env.stop);
    let better = match (before, after) {
        (_, Some(0)) => true,
        (Some(2), Some(3)) => true,
        _ => false,
    };
    if !better || probe_ok(&out).is_err() {
        return no("qpdf rebuilt it, but the rebuilt PDF does not check out better", Next::Paid);
    }
    Try::Made {
        file: out,
        how: "rebuilt by qpdf: its table of where everything is (the cross-reference table) was made again from what is \
              in the file"
            .to_string(),
        tool: tools::version(Tool::Qpdf, qpdf),
        salvage: false,
        note: if after == Some(0) { "qpdf --check finds no errors".to_string() } else { "qpdf --check finds warnings only".to_string() },
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
    let low = r.text().to_lowercase();
    let lost = low.matches("crc failed").count() + low.matches("data error").count() + low.matches("unexpected end").count();
    if files == 0 {
        return no(
            "7-Zip could read nothing out of it",
            if office { Next::OwnApp } else { Next::Nothing },
        );
    }
    // Rebuilt from what read back right (an ePub's or OpenDocument's
    // "mimetype" goes first, uncompressed, as those formats require).
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
    if lost == 0 && r.code == Some(0) {
        return Try::Made {
            file: out,
            how: "rebuilt by 7-Zip from every part inside it (all of them read back right; what was damaged was the zip's \
                  own index)"
                .to_string(),
            tool: tools::version(Tool::SevenZip, sz),
            salvage: false,
            note: format!("{} parts", commas(files as u64)),
        };
    }
    // Some parts were lost: what could be read is kept, the original stays.
    let name = item.path.rsplit('\\').next().unwrap_or(&item.path);
    let _ = name;
    Try::Made {
        file: out,
        how: format!(
            "{} parts inside it could be read and are kept in a new zip; {} could not",
            commas(files as u64),
            if lost == 0 { "some".to_string() } else { commas(lost as u64) }
        ),
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
    match media_checks_out(env, &out, None) {
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

/// The map check's line for this file, as (lost byte ranges).
fn lost_ranges(env: &Env, item: &Item) -> Option<Vec<(u64, u64)>> {
    let it = crate::join::item_for_copy(&env.map_items, &item.path, item.size)?;
    let r = crate::join::parse_ranges(&it.ranges);
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
    // 1. An intact copy elsewhere: the file as it was.
    if let Some(lost) = lost_ranges(env, item) {
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
    // 2. The format's own repair.
    let name = item.path.rsplit('\\').next().unwrap_or(&item.path);
    let ext = ext_lower(name);
    let e = ext.as_str();
    let damage = item.why.as_str();
    if ["jpg", "jpeg", "jpe", "jfif"].contains(&e) {
        let t = fix_jpeg(env, src, tmp);
        if let Try::No { why, .. } = &t {
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
    let what = if made.is_dir() {
        "what could be read"
    } else if made.file_name().is_some_and(|n| n.to_string_lossy().starts_with("preview")) {
        "preview"
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
    let map_items = opts
        .map_check
        .as_ref()
        .and_then(|d| std::fs::read_to_string(d.join(crate::join::TSV_NAME)).ok())
        .map(|t| crate::join::parse_tsv(&t))
        .unwrap_or_default();
    let damaged: HashSet<String> = items.iter().map(|i| i.path.to_lowercase()).collect();
    let mut env = Env {
        home,
        work,
        map_items,
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
        let repair_low = repair.display().to_string().to_lowercase();
        for root in std::iter::once(work.clone()).chain(opts.search.iter().cloned()) {
            crate::walk::walk(&root.display().to_string(), |dir, dirs, files| {
                dirs.retain(|d| !crate::formats::SYSTEM_DIRS.contains(&d.as_str()));
                if dir.to_lowercase().starts_with(&repair_low) {
                    dirs.clear();
                    return !stop();
                }
                for f in files {
                    let full = crate::walk::join(dir, f);
                    if let Ok(m) = std::fs::metadata(&full) {
                        if sizes.contains(&m.len()) {
                            env.by_size.entry(m.len()).or_default().push(PathBuf::from(full));
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
    for (n, item) in todo.iter().enumerate() {
        if stop() {
            o.stopped = true;
            break;
        }
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
            match try_item(&env, item, &src, &tmp) {
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
                        if same {
                            a.how = "the repair changed nothing".to_string();
                            a.next = Next::Paid;
                        } else {
                            match move_into(&file, &dest) {
                                Ok(()) => {
                                    a.fate = if salvage { Fate::Salvaged } else { Fate::Repaired };
                                    a.how = how;
                                    a.tool = tool;
                                    a.output = rel;
                                    a.note = note;
                                    a.next = if salvage {
                                        match ext_lower(&copy_name).as_str() {
                                            e if is_raw(e) => Next::Mac,
                                            e if ZIP_LIKE.contains(&e) && !matches!(e, "zip" | "jar") => Next::OwnApp,
                                            "jpg" | "jpeg" | "jpe" | "jfif" => Next::Paid,
                                            _ => Next::Nothing,
                                        }
                                    } else {
                                        Next::Done
                                    };
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
        o.attempts.push(a);
        amount(n as u64 + 1, total);
    }
    let _ = std::fs::remove_dir_all(&temp_root);
    if !o.attempts.is_empty() {
        o.log = Some(append_log(&repair, &o.attempts)?);
    }
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
        let nothing = o.attempts.iter().filter(|a| a.fate == Fate::NotRepaired && a.next == Next::Nothing).count();
        if nothing > 0 {
            t.push(format!(
                "{} open or play as they are, with a glitch where data was lost: nothing can bring that part back.",
                commas(nothing as u64)
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
    fn camera_name_starts_are_compared() {
        assert_eq!(name_prefix("IMG_1234.MOV"), "img_");
        assert_eq!(name_prefix("GOPR0012.MP4"), "gopr");
        assert_eq!(name_prefix("20190101_120000.mp4"), "");
        assert_eq!(salvage_name("IMG_1 [R0003].CR2", Path::new("/x/preview.jpg")), "IMG_1 [R0003] (preview).jpg");
    }
}
