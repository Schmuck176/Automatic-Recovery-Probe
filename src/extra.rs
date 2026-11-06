//! The extra checks, read only: what the person was to do by hand in
//! Krokiet, 7-Zip and MP3val (Phase 4 of the old checklist), done by the
//! program with the tools' command-line versions.
//!
//! - 7-Zip tests every archive, and every Office document and ePub (they are
//!   zip files inside): the only real test of those.
//! - MP3val reads every MP3 frame by frame: a cut-short MP3 still claims its
//!   full length, and only counting its frames shows it.
//! - qpdf checks every PDF's structure and every stream in it.
//! - FFmpeg plays every photo, song and (if asked: it takes hours) video
//!   through, the way Krokiet's broken-files check does, and lists every one
//!   it reports errors in. FFmpeg 8.1 and later also plays iPhone photos
//!   (HEIC), so on this PC they are checked too, not only on the Mac.
//!
//! Every tool is run only with options that read, on the copies in WORK,
//! never on the image or the SSD. What they find goes into lists in
//! REPAIR\damaged-lists (one per tool, "path<TAB>what it said"), which the
//! isolate step reads like any list the person puts there, and into a
//! report in the results folder. Photos libraries' previews and thumbnails
//! are not checked: the library's app makes them again (repair.rs).

use crate::fmt::{commas, csv_row};
use crate::tools::{self, Tool};
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Check {
    Archive,
    Mp3,
    Pdf,
    Photo,
    Sound,
    Video,
}

impl Check {
    pub const ALL: [Check; 6] = [Check::Archive, Check::Mp3, Check::Pdf, Check::Photo, Check::Sound, Check::Video];

    pub fn key(self) -> &'static str {
        match self {
            Check::Archive => "archives",
            Check::Mp3 => "mp3",
            Check::Pdf => "pdf",
            Check::Photo => "photos",
            Check::Sound => "sound",
            Check::Video => "videos",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Check::Archive => "Archives and Office documents (7-Zip test)",
            Check::Mp3 => "MP3s, frame by frame (MP3val)",
            Check::Pdf => "PDFs (qpdf --check)",
            Check::Photo => "Photos, decoded (FFmpeg)",
            Check::Sound => "Music and sound, played through (FFmpeg)",
            Check::Video => "Videos, played through (FFmpeg)",
        }
    }

    pub fn tool(self) -> Tool {
        match self {
            Check::Archive => Tool::SevenZip,
            Check::Mp3 => Tool::Mp3val,
            Check::Pdf => Tool::Qpdf,
            Check::Photo | Check::Sound | Check::Video => Tool::Ffmpeg,
        }
    }

    fn list_name(self) -> String {
        format!("auto-{}.txt", self.key())
    }
}

pub const ARCHIVE_EXTS: &[&str] = &[
    "zip", "docx", "docm", "dotx", "dotm", "xlsx", "xlsm", "xltx", "xltm", "pptx", "pptm", "potx", "ppsx", "odt", "ods",
    "odp", "odg", "epub", "jar", "apk", "7z", "rar", "gz", "tgz", "bz2", "tbz2", "xz", "txz", "zst", "cab", "tar",
];
pub const PHOTO_EXTS: &[&str] = &["jpg", "jpeg", "jpe", "jfif", "png", "gif", "webp", "tif", "tiff", "bmp"];
pub const HEIF_EXTS: &[&str] = &["heic", "heif", "hif", "avif"];
pub const SOUND_EXTS: &[&str] = &["mp3", "m4a", "aac", "flac", "ogg", "oga", "opus", "wma", "aif", "aiff", "wav", "caf"];
pub const VIDEO_EXTS: &[&str] =
    &["mp4", "m4v", "mov", "qt", "3gp", "3g2", "avi", "mkv", "webm", "mts", "m2ts", "ts", "mpg", "mpeg", "wmv", "flv"];

fn ext_lower(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => e.to_lowercase(),
        _ => String::new(),
    }
}

/// Which checks a file gets (an MP3 gets two: MP3val and playing it).
pub fn checks_for(name: &str, heif: bool, videos: bool) -> Vec<Check> {
    let e = ext_lower(name);
    let e = e.as_str();
    let mut out = Vec::new();
    if ARCHIVE_EXTS.contains(&e) {
        out.push(Check::Archive);
    }
    if e == "mp3" {
        out.push(Check::Mp3);
    }
    if e == "pdf" {
        out.push(Check::Pdf);
    }
    if PHOTO_EXTS.contains(&e) || (heif && HEIF_EXTS.contains(&e)) {
        out.push(Check::Photo);
    }
    if SOUND_EXTS.contains(&e) {
        out.push(Check::Sound);
    }
    if videos && VIDEO_EXTS.contains(&e) {
        out.push(Check::Video);
    }
    out
}

pub struct Options {
    pub work: PathBuf,
    /// Whether to play the videos through too (hours to days for a big
    /// collection): asked as each one's turn comes, and while it plays. The
    /// person can change their mind until the videos come (they come last),
    /// and unticked while they play, the rest are left out.
    pub videos: Arc<dyn Fn() -> bool + Send + Sync>,
    /// How many tools run at once to begin with,
    pub jobs: usize,
    /// and at most: one more is started each time the processor has had
    /// room for a while - every tool busy, the processor not - while memory
    /// is free.
    pub most: usize,
    /// What earlier runs found, file by file (a file of the same size and
    /// date is not checked again: stopped, the checks carry on where they
    /// stopped).
    pub done_list: Option<PathBuf>,
    /// The map check's list of the files the never-read places touched
    /// (map-check-files.tsv), when there is one: every other file was read
    /// whole from the old drive.
    pub map_items: Option<Vec<crate::join::MapItem>>,
    /// Only the videos, for this long, in no particular order: how fast this
    /// PC plays them through (right after the copying, so that the time it
    /// takes is known before their turn). What it got through is kept, as
    /// any run's is.
    pub trial: Option<Duration>,
    /// How fast the videos went before (bytes a second, all at once): the
    /// time they take is worked out from it until this run has measured its
    /// own.
    pub video_rate: Option<f64>,
}

/// Every video played through: what there is to play, what was, how fast.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoNumbers {
    /// Videos to play (not counting those an earlier run played, nor
    /// identical copies of others), and their bytes.
    pub to_play: u64,
    pub to_play_bytes: u64,
    /// Played through by this run.
    pub played: u64,
    pub played_bytes: u64,
    /// Left out: not to be played when their turn came, or unticked while
    /// they played.
    pub left: u64,
    pub left_bytes: u64,
    /// How fast they went, all at once together (bytes a second), when
    /// measured.
    pub rate: Option<f64>,
}

impl VideoNumbers {
    /// How long playing the rest through would take at the speed measured
    /// (or `rate`), in seconds.
    pub fn secs_for(bytes: u64, rate: Option<f64>) -> Option<u64> {
        rate.filter(|r| *r > 0.0).map(|r| (bytes as f64 / r).round() as u64)
    }
}

/// One line of the list of files checked: check, size, modified (seconds),
/// result (F fine, D damaged, L has a password), message, path.
fn done_line(c: Check, size: u64, mtime: i64, v: &Verdict, path: &str) -> Option<String> {
    let (code, msg) = match v {
        Verdict::Fine => ("F", String::new()),
        Verdict::Damaged(m) => ("D", m.replace(['\t', '\r', '\n'], " ")),
        Verdict::Locked => ("L", String::new()),
        Verdict::NotChecked(_) => return None,
    };
    Some(format!("{}\t{size}\t{mtime}\t{code}\t{msg}\t{path}\n", c.key()))
}

fn read_done(p: &Path) -> std::collections::HashMap<(Check, String), (u64, i64, Verdict)> {
    let mut out = std::collections::HashMap::new();
    let Ok(bytes) = std::fs::read(p) else { return out };
    let text = String::from_utf8_lossy(&bytes);
    for line in text.lines() {
        let f: Vec<&str> = line.splitn(6, '\t').collect();
        if f.len() < 6 {
            continue;
        }
        let Some(c) = Check::ALL.iter().copied().find(|c| c.key() == f[0]) else { continue };
        let (Ok(size), Ok(mtime)) = (f[1].parse::<u64>(), f[2].parse::<i64>()) else { continue };
        let v = match f[3] {
            "F" => Verdict::Fine,
            "D" => Verdict::Damaged(f[4].to_string()),
            "L" => Verdict::Locked,
            _ => continue,
        };
        out.insert((c, f[5].to_lowercase()), (size, mtime, v));
    }
    out
}

fn mtime_of(m: &std::fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// A file a tool found something wrong with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    pub check: Check,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct Outcome {
    /// Per check: (files checked, found damaged).
    pub counts: BTreeMap<Check, (u64, u64)>,
    pub findings: Vec<Finding>,
    /// Archives and PDFs with a password: they cannot be tested without it.
    pub locked: Vec<String>,
    /// Files a tool could not get through in its time limit, or could not start on.
    pub not_checked: Vec<(String, String)>,
    /// Checks left out, and why (a tool not found).
    pub left_out: Vec<(Check, String)>,
    /// A check that called too many files of one kind damaged to believe it
    /// (a tool that cannot read a kind of file well calls them all damaged).
    /// Those are not listed as damaged.
    pub doubtful: Vec<Doubt>,
    pub lists: Vec<PathBuf>,
    pub report: Option<PathBuf>,
    pub stopped: bool,
    /// Files that are identical copies of another one checked (the same size
    /// and SHA-256 in the list of copies, the same kind): its result is
    /// theirs.
    pub twins: u64,
    /// The most tools that ran at once.
    pub at_once: usize,
    pub videos: VideoNumbers,
}

/// A check's findings for one kind of file, set aside.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Doubt {
    pub check: Check,
    pub ext: String,
    /// Files of that kind it called damaged, and checked.
    pub flagged: u64,
    pub checked: u64,
    /// Counted among the files read whole from the old drive (the map check
    /// was known): those are exactly as they were on it.
    pub read_whole: bool,
}

/// What a tool's run of one file says.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Verdict {
    Fine,
    Damaged(String),
    Locked,
    NotChecked(String),
}

// ---------------------------------------------------------------- reading what each tool says

/// The lines of FFmpeg's error output (at `-v error`) that are damage, not
/// chatter some files cause however good they are.
pub fn ffmpeg_problems(err: &str) -> Vec<String> {
    const HARMLESS: &[&str] = &[
        "deprecated pixel format",
        "non monotonically increasing dts",
        "non monotonous dts",
        "timestamps are unset",
        "guessed channel layout",
        "estimating duration from bitrate",
        "last message repeated",
        "consider increasing the value for the 'analyzeduration'",
        "consider increasing the value for the 'probesize'",
        "could not find codec parameters",
        "no accelerated colorspace conversion",
        "discarding id3 tags",
        "incomplete utf-8 sequence",
        "invalid utf-8",
        "unknown cover type",
        "ignoring",
        // What a photo's own notes say (the camera's EXIF, its colour
        // profile, other APP segments): not the picture.
        "exif",
        "icc",
        "app fields",
        "xmp",
    ];
    err.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .filter(|l| {
            let low = l.to_lowercase();
            !HARMLESS.iter().any(|h| low.contains(h))
        })
        .map(|l| l.chars().take(240).collect())
        .collect()
}

fn ffmpeg_verdict(r: &tools::Ran) -> Verdict {
    if r.timed_out {
        return Verdict::NotChecked("FFmpeg did not get through it in its time limit".to_string());
    }
    let problems = ffmpeg_problems(&r.err);
    if r.code != Some(0) {
        let why = problems.first().cloned().unwrap_or_else(|| format!("FFmpeg stopped with code {:?}", r.code));
        return Verdict::Damaged(format!("could not be played through: {why}"));
    }
    if problems.is_empty() {
        Verdict::Fine
    } else {
        let n = problems.len();
        Verdict::Damaged(if n == 1 { problems[0].clone() } else { format!("{} ({} errors in all)", problems[0], commas(n as u64)) })
    }
}

/// 7-Zip's test (`7z t`): 0 means every member read back right.
fn sevenzip_verdict(r: &tools::Ran) -> Verdict {
    if r.timed_out {
        return Verdict::NotChecked("7-Zip did not get through it in its time limit".to_string());
    }
    let text = r.text();
    let low = text.to_lowercase();
    if low.contains("wrong password") || low.contains("can not open encrypted archive") || low.contains("cannot open encrypted archive") {
        return Verdict::Locked;
    }
    // 0: every part read back right (whatever the names in it say: a file
    // called "error.log" inside is not an error). 1: warnings only, such as
    // data after the end of the archive - not damage to anything in it.
    if r.code == Some(0) {
        return Verdict::Fine;
    }
    if r.code == Some(1) && !["crc failed", "data error", "unexpected end", "headers error"].iter().any(|w| low.contains(w)) {
        return Verdict::Fine;
    }
    let lines: Vec<String> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| {
            let w = l.to_lowercase();
            w.contains("error") || w.contains("crc failed") || w.contains("data error") || w.contains("unexpected end")
                || w.contains("is not archive") || w.contains("can not open") || w.contains("cannot open")
                || w.contains("headers error") || w.contains("unsupported")
        })
        .filter(|l| !l.to_lowercase().starts_with("errors:"))
        .map(|l| l.chars().take(200).collect())
        .collect();
    if r.code == Some(0) && lines.is_empty() {
        return Verdict::Fine;
    }
    let mut why = lines.into_iter().take(3).collect::<Vec<_>>().join("; ");
    if why.is_empty() {
        why = format!("7-Zip's test failed (code {:?})", r.code);
    }
    Verdict::Damaged(why)
}

/// MP3val's report: its WARNING lines name what is wrong.
fn mp3val_verdict(r: &tools::Ran) -> Verdict {
    if r.timed_out {
        return Verdict::NotChecked("MP3val did not get through it in its time limit".to_string());
    }
    let mut problems: Vec<String> = Vec::new();
    for l in r.text().lines() {
        let t = l.trim();
        let Some(rest) = t.strip_prefix("WARNING:").or_else(|| t.strip_prefix("ERROR:")) else { continue };
        // WARNING: "C:\...\song.mp3" (offset 0x2e5b4f): MPEG stream error, resynchronized successfully
        let msg = match rest.rfind("): ") {
            Some(i) => &rest[i + 3..],
            None => match rest.rfind("\": ") {
                Some(i) => &rest[i + 3..],
                None => rest,
            },
        };
        let msg: String = msg.trim().chars().take(200).collect();
        if !problems.contains(&msg) {
            problems.push(msg);
        }
    }
    if problems.is_empty() {
        if r.code.is_some_and(|c| c != 0) && r.text().trim().is_empty() {
            return Verdict::NotChecked(format!("MP3val stopped with code {:?}", r.code));
        }
        Verdict::Fine
    } else {
        Verdict::Damaged(problems.join("; "))
    }
}

/// qpdf --check: 0 fine, 3 warnings (it had to work round damage), 2 errors.
fn qpdf_verdict(r: &tools::Ran) -> Verdict {
    if r.timed_out {
        return Verdict::NotChecked("qpdf did not get through it in its time limit".to_string());
    }
    let text = r.text();
    let low = text.to_lowercase();
    if low.contains("invalid password") {
        return Verdict::Locked;
    }
    match r.code {
        Some(0) => Verdict::Fine,
        Some(2) | Some(3) => {
            let lines: Vec<String> = text
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty() && !l.starts_with("checking ") && !l.starts_with("PDF Version") && !l.starts_with("File is not"))
                .filter(|l| !l.to_lowercase().starts_with("qpdf: operation succeeded with warnings"))
                .map(|l| l.chars().take(200).collect())
                .collect();
            let why = if lines.is_empty() { format!("qpdf found problems (code {:?})", r.code) } else { lines.into_iter().take(2).collect::<Vec<_>>().join("; ") };
            Verdict::Damaged(why)
        }
        other => Verdict::NotChecked(format!("qpdf stopped with code {other:?}: {}", text.lines().next().unwrap_or(""))),
    }
}

// ---------------------------------------------------------------- running them

/// How long a tool may take on a file this big.
fn limit_for(check: Check, size: u64) -> Duration {
    let mb = size / (1 << 20);
    Duration::from_secs(match check {
        Check::Archive => 600 + mb / 10,
        Check::Mp3 => 180 + mb,
        Check::Pdf => 300 + mb,
        Check::Photo => 120 + mb * 2,
        Check::Sound => 600 + mb,
        Check::Video => 900 + mb / 2,
    })
}

/// A path every Windows program can open: plain ASCII, short, and given
/// the usual way (not as one Windows takes literally: walk::literal).
fn plain_path(p: &Path) -> bool {
    p.to_str().is_some_and(|s| s.is_ascii() && s.len() < 240 && !s.starts_with(r"\\?\"))
}

/// `watch` is told about once a second what the tool has read and the
/// processor time it has used (how fast the videos go, and whether there is
/// room for more at once).
fn check_one(check: Check, exe: &Path, path: &Path, size: u64, stop: &dyn Fn() -> bool, watch: &dyn Fn(tools::Usage)) -> Verdict {
    // MP3val and qpdf (older kinds of Windows program) can't open every
    // name: a file whose path has letters outside plain ASCII, or is long, is
    // checked as a short-named copy (which it can't tell from the file).
    let mut copy: Option<PathBuf> = None;
    if cfg!(windows) && matches!(check, Check::Mp3 | Check::Pdf) && !plain_path(path) {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("arp-extra-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let c = dir.join(format!("f{}.{}", N.fetch_add(1, Ordering::Relaxed), ext_lower(&path.to_string_lossy())));
        match std::fs::copy(path, &c) {
            Ok(_) => copy = Some(c),
            Err(e) => return Verdict::NotChecked(format!("a short-named copy of it could not be made to check ({e})")),
        }
    }
    let v = check_path(check, exe, copy.as_deref().unwrap_or(path), size, stop, watch);
    if let Some(c) = copy {
        let _ = std::fs::remove_file(c);
    }
    v
}

/// FFmpeg playing a file through, to nowhere, saying only what goes wrong
/// (the extra checks, and the automatic repairs' check of what they made).
/// Sound is decoded on one thread: on several, FFmpeg 6 says nothing of the
/// damage in a FLAC (5 damaged FLACs in 6 played "without an error" here,
/// that on one thread gave four errors each). Video keeps its threads (a
/// big one would take several times as long on one).
///
/// The deblocking filter is left out (`-skip_loop_filter all`): it only
/// smooths the edges of the picture's blocks, after FFmpeg has read the
/// stream - where every error is found. Played with and without it, damaged
/// H.264, HEVC, MPEG-2 and MPEG-4 videos (random bytes, holes of zeros,
/// scattered flipped bytes, cut short) gave the same errors, every one; and
/// the H.264 and HEVC videos played 15-20% faster without it.
pub fn play_through_args(path: &Path) -> Vec<OsString> {
    vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-v".into(),
        "error".into(),
        "-threads:a".into(),
        "1".into(),
        "-skip_loop_filter".into(),
        "all".into(),
        "-i".into(),
        path.as_os_str().to_os_string(),
        "-map".into(),
        "0:v?".into(),
        "-map".into(),
        "0:a?".into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ]
}

fn check_path(check: Check, exe: &Path, path: &Path, size: u64, stop: &dyn Fn() -> bool, watch: &dyn Fn(tools::Usage)) -> Verdict {
    let p = path.as_os_str().to_os_string();
    let args: Vec<OsString> = match check {
        // A made-up password: an archive that has one says "Wrong password"
        // instead of waiting for someone to type it.
        Check::Archive => vec!["t".into(), "-pnot-a-password".into(), "-y".into(), "-bd".into(), "--".into(), p],
        Check::Mp3 => vec![p],
        Check::Pdf => vec!["--check".into(), p],
        Check::Photo | Check::Sound | Check::Video => play_through_args(path),
    };
    let r = match tools::run_low_watching(exe, &args, None, limit_for(check, size), stop, watch) {
        Ok(r) => r,
        Err(e) => return Verdict::NotChecked(format!("could not start {}: {e}", check.tool().name())),
    };
    if r.stopped {
        return Verdict::NotChecked("stopped".to_string());
    }
    match check {
        Check::Archive => sevenzip_verdict(&r),
        Check::Mp3 => mp3val_verdict(&r),
        Check::Pdf => qpdf_verdict(&r),
        Check::Photo | Check::Sound | Check::Video => ffmpeg_verdict(&r),
    }
}

/// Inside a library's previews (which the library makes again), or a
/// housekeeping file: not checked.
fn skip(work: &Path, full: &str, name: &str) -> bool {
    if name.starts_with("._") || crate::formats::is_system_file(&name.to_lowercase()) {
        return true;
    }
    if name.eq_ignore_ascii_case(crate::rescue::LOG_NAME) || name.eq_ignore_ascii_case(crate::repair::RETURNS_NAME) {
        return true;
    }
    let Some(rel) = crate::repair::relative_to(work, &full.replace('/', "\\")) else { return false };
    match crate::repair::package_around(&rel) {
        Some((root, crate::repair::Kind::Library, _)) => {
            crate::repair::member_role(&rel[root.len().min(rel.len())..]) == crate::repair::Role::Previews
        }
        Some((_, crate::repair::Kind::Program, _)) => true,
        _ => false,
    }
}

/// One check to make: a file, and the identical copies of it whose result
/// is its own (the same size and SHA-256 in the list of copies, and the same
/// kind of file).
struct Task {
    check: Check,
    path: PathBuf,
    size: u64,
    twins: Vec<(PathBuf, u64)>,
}

impl Task {
    fn files(&self) -> u64 {
        1 + self.twins.len() as u64
    }
    fn bytes(&self) -> u64 {
        self.twins.iter().fold(self.size, |a, t| a.saturating_add(t.1))
    }
}

/// A tool running now, on one of the workers: whether it plays a video, its
/// file's size, what it has read and the processor time it has used (as last
/// told), and that time at the last look.
#[derive(Clone, Copy, Debug, Default)]
struct Busy {
    video: bool,
    size: u64,
    read: u64,
    cpu: f64,
    cpu_seen: f64,
}

/// A path's place in no particular order: the trial plays the videos in it,
/// so that what it measures is not only the biggest, nor one folder's.
fn scatter(p: &Path) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in p.to_string_lossy().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// How fast something gets on, over the last two minutes.
#[derive(Default)]
struct Pace {
    /// (seconds since the start, amount done then).
    samples: std::collections::VecDeque<(f64, u64)>,
}

impl Pace {
    fn note(&mut self, at: f64, done: u64) {
        if self.samples.back().is_some_and(|&(t, _)| at - t < 1.0) {
            return;
        }
        self.samples.push_back((at, done));
        while self.samples.len() > 2 && self.samples.front().is_some_and(|&(t, _)| at - t > 120.0) {
            self.samples.pop_front();
        }
    }
    /// Done a second lately, once there are 20 seconds to go on.
    fn rate(&self) -> Option<f64> {
        let (&(t0, n0), &(t1, n1)) = (self.samples.front()?, self.samples.back()?);
        (t1 - t0 >= 20.0 && n1 > n0).then(|| (n1 - n0) as f64 / (t1 - t0))
    }
}

/// The processor busy less than this, with every tool running,
const ROOM: f64 = 0.75;
/// while each video being played keeps at least this much of a thread busy
/// (less: it waits for the drive, and more at once would not go faster),
const KEEPS_BUSY: f64 = 0.3;
/// and this much memory free: one more tool at once.
const MEMORY_FOR_MORE: u64 = 2 << 30;
/// Less memory free than this: one fewer.
const MEMORY_LOW: u64 = 1 << 30;

/// Is there room for one more tool at once? `busy`: how busy the processor
/// was lately (0 to 1); `all_working`: every worker had a file; `per_video`:
/// threads' worth of the processor each video being played used, if one
/// was; `free`: memory free now.
fn room_for_more(busy: Option<f64>, all_working: bool, per_video: Option<f64>, free: Option<u64>) -> bool {
    all_working
        && busy.is_some_and(|b| b < ROOM)
        && per_video.is_none_or(|p| p >= KEEPS_BUSY)
        && free.is_none_or(|f| f >= MEMORY_FOR_MORE)
}

/// Run the extra checks over WORK.
pub fn run(
    home: &Path,
    opts: &Options,
    say: &mut dyn FnMut(&str),
    amount: &mut dyn FnMut(u64, u64),
    stop: &(dyn Fn() -> bool + Sync),
) -> Result<Outcome, String> {
    let work = &opts.work;
    if !work.is_dir() {
        return Err(format!("{} is not there", work.display()));
    }
    let trial = opts.trial.is_some();
    let mut o = Outcome::default();
    // Which tools there are.
    let mut exes: BTreeMap<Check, PathBuf> = BTreeMap::new();
    let mut heif = false;
    for c in Check::ALL {
        if trial && c != Check::Video {
            continue;
        }
        match tools::find(c.tool(), home) {
            Some(p) => {
                if c.tool() == Tool::Ffmpeg {
                    heif = tools::ffmpeg_decodes_heif(&tools::version(Tool::Ffmpeg, &p));
                }
                exes.insert(c, p);
            }
            None => o.left_out.push((c, format!("{} was not found on this PC", c.tool().name()))),
        }
    }
    check_all(opts, &exes, heif, o, say, amount, stop)
}

/// The checks, with the tools found (`exes`; `heif`: FFmpeg decodes iPhone
/// photos).
fn check_all(
    opts: &Options,
    exes: &BTreeMap<Check, PathBuf>,
    heif: bool,
    mut o: Outcome,
    say: &mut dyn FnMut(&str),
    amount: &mut dyn FnMut(u64, u64),
    stop: &(dyn Fn() -> bool + Sync),
) -> Result<Outcome, String> {
    let work = &opts.work;
    let trial = opts.trial.is_some();
    let can_play = exes.contains_key(&Check::Video);
    // What to check.
    say(&format!("Looking for files to check in {}...", work.display()));
    let earlier = opts.done_list.as_ref().map(|p| read_done(p)).unwrap_or_default();
    // A repaired version put back keeps its original's size (an intact twin)
    // and dates: what was found before is about the damaged copy, so it is
    // checked again.
    let back = crate::repair::returned_now(work);
    // Every copy's size and SHA-256, as the copying found them: identical
    // copies are checked once.
    let copies = crate::repair::rescue_log(work);
    // The files the never-read places touched, by size (for a quick look-up).
    let touched_by_size: Option<std::collections::HashMap<u64, Vec<crate::join::MapItem>>> = opts.map_items.as_ref().map(|m| {
        let mut by: std::collections::HashMap<u64, Vec<crate::join::MapItem>> = std::collections::HashMap::new();
        for it in m.iter().filter(|i| i.kind == "DAMAGED" && !i.only_past_end) {
            by.entry(it.size).or_default().push(it.clone());
        }
        by
    });
    let mut read_whole: HashSet<String> = HashSet::new();
    let mut remembered: Vec<(Check, String, Verdict)> = Vec::new();
    let mut mtimes: std::collections::HashMap<PathBuf, i64> = std::collections::HashMap::new();
    // (check, kind of file, content) -> what an earlier run found for a file
    // with that content.
    let mut known: std::collections::HashMap<(Check, String, String), Verdict> = std::collections::HashMap::new();
    // To check, in the order found: (check, path, size, content).
    let mut found: Vec<(Check, PathBuf, u64, Option<String>)> = Vec::new();
    let root = work.display().to_string();
    crate::walk::walk(&root, |dir, dirs, files| {
        dirs.retain(|d| !crate::formats::SYSTEM_DIRS.contains(&d.as_str()));
        for f in files {
            let full = crate::walk::join(dir, f);
            if skip(work, &full, f) {
                continue;
            }
            let mut wanted = checks_for(f, heif, can_play);
            if trial {
                wanted.retain(|c| *c == Check::Video);
            }
            if wanted.is_empty() {
                continue;
            }
            let Ok(meta) = std::fs::metadata(crate::walk::literal(&full)) else { continue };
            let size = meta.len();
            if size == 0 {
                continue;
            }
            let mtime = mtime_of(&meta);
            let rel = crate::repair::relative_to(work, &full.replace('/', "\\")).unwrap_or_default();
            if let Some(by) = &touched_by_size {
                let touched = by.get(&size).is_some_and(|c| crate::join::item_for_copy(c, &rel, size).is_some());
                if !touched {
                    read_whole.insert(full.to_lowercase());
                }
            }
            let put_back = back.contains_key(&rel.to_lowercase());
            // Its content as the copying found it - if it is still that size,
            // and nothing was put back in its place since.
            let content = if put_back {
                None
            } else {
                copies
                    .get(&rel.to_lowercase())
                    .filter(|(_, s, sha, _)| *s == size && sha.len() == 64)
                    .map(|(_, _, sha, _)| format!("{size}:{sha}"))
            };
            let kind = ext_lower(f);
            for c in wanted {
                if exes.contains_key(&c) {
                    // Checked before, as it is now: what was found then stands.
                    match earlier.get(&(c, full.to_lowercase())).filter(|_| !put_back) {
                        Some((s, m, v)) if *s == size && *m == mtime => {
                            if let Some(k) = &content {
                                known.entry((c, kind.clone(), k.clone())).or_insert_with(|| v.clone());
                            }
                            remembered.push((c, full.clone(), v.clone()));
                        }
                        _ => {
                            mtimes.insert(PathBuf::from(&full), mtime);
                            found.push((c, PathBuf::from(&full), size, content.clone()));
                        }
                    }
                }
            }
        }
        !stop()
    });
    // Identical copies: the first found is checked, and its result is the
    // others' too (or what an earlier run found for one of them).
    let mut tasks: Vec<Task> = Vec::new();
    let mut first_of: std::collections::HashMap<(Check, String, String), usize> = std::collections::HashMap::new();
    let mut by_twin: Vec<(Check, PathBuf, u64, Verdict)> = Vec::new();
    for (c, path, size, content) in found {
        let key = content.map(|k| (c, ext_of_path(&path.to_string_lossy()), k));
        if let Some(k) = &key {
            if let Some(v) = known.get(k) {
                by_twin.push((c, path, size, v.clone()));
                continue;
            }
            if let Some(&i) = first_of.get(k) {
                tasks[i].twins.push((path, size));
                continue;
            }
            first_of.insert(k.clone(), tasks.len());
        }
        tasks.push(Task { check: c, path, size, twins: Vec::new() });
    }
    if trial {
        tasks.sort_by_key(|t| scatter(&t.path));
    } else {
        // Quick checks first, the videos last (the long part, best left
        // running) - the biggest first, so that it does not end with one big
        // video playing on its own.
        tasks.sort_by(|a, b| {
            let (va, vb) = (a.check == Check::Video, b.check == Check::Video);
            va.cmp(&vb).then(a.check.cmp(&b.check)).then_with(|| if va { b.size.cmp(&a.size) } else { std::cmp::Ordering::Equal })
        });
    }
    let total: u64 = tasks.iter().map(Task::files).sum();
    // (The page shows checks made of all there are, by kind of file. An MP3
    // gets two: MP3val's and FFmpeg's.)
    crate::live::begin("checks");
    crate::live::found_begin();
    for t in &tasks {
        crate::live::to_do(&t.path.to_string_lossy(), t.size);
        for (p, s) in &t.twins {
            crate::live::to_do(&p.to_string_lossy(), *s);
        }
    }
    let bytes: u64 = tasks.iter().map(Task::bytes).sum();
    say(&format!("{} checks to make, over {} of files.", commas(total), crate::fmt::size(bytes)));
    o.twins = tasks.iter().map(|t| t.twins.len() as u64).sum::<u64>() + by_twin.len() as u64;
    if o.twins > 0 {
        say(&format!(
            "({} of the files are identical copies of others - the same size and SHA-256, the same kind of file: each is \
             checked once, and its result is theirs too.)",
            commas(o.twins)
        ));
    }
    if !remembered.is_empty() {
        say(&format!("({} more were made by an earlier run, on the files as they are now: what they found stands.)", commas(remembered.len() as u64)));
    }
    let done_file = opts.done_list.as_ref().and_then(|p| {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        std::fs::OpenOptions::new().create(true).append(true).open(p).ok()
    });
    let done_file = Mutex::new(done_file);
    let write_done = |line: Option<String>| {
        use std::io::Write;
        if let (Some(line), Some(f)) = (line, done_file.lock().unwrap_or_else(|e| e.into_inner()).as_mut()) {
            let _ = f.write_all(line.as_bytes());
        }
    };
    // (Identical to a file an earlier run checked: written down as checked,
    // so that the next run remembers them by name.)
    for (c, path, size, v) in &by_twin {
        write_done(done_line(*c, *size, mtimes.get(path).copied().unwrap_or(0), v, &path.display().to_string()));
    }
    for c in Check::ALL {
        let n: u64 = tasks.iter().filter(|t| t.check == c).map(Task::files).sum();
        if n > 0 {
            say(&format!("  {}: {} files", c.title(), commas(n)));
        }
        if exes.contains_key(&c) {
            o.counts.insert(c, (0, 0));
        }
    }
    o.videos.to_play = tasks.iter().filter(|t| t.check == Check::Video).count() as u64;
    o.videos.to_play_bytes = tasks.iter().filter(|t| t.check == Check::Video).map(|t| t.size).sum();
    let quick_total: u64 = tasks.iter().filter(|t| t.check != Check::Video).map(Task::files).sum();
    if !trial && o.videos.to_play > 0 && !(opts.videos)() {
        say("  (The videos come last: they are played through if Play every video through is ticked when their turn comes. It is not ticked now.)");
    }
    amount(0, total);
    // Several at once, each tool at a low priority; more while the processor
    // has room.
    let most = opts.most.max(opts.jobs).clamp(1, 16);
    let target = AtomicUsize::new(opts.jobs.clamp(1, most));
    let mut most_at_once = target.load(Ordering::Relaxed);
    let queue = Mutex::new(tasks.into_iter().rev().collect::<Vec<_>>());
    let results: Mutex<Vec<(Check, String, Verdict)>> =
        Mutex::new(by_twin.iter().map(|(c, p, _, v)| (*c, p.display().to_string(), v.clone())).collect());
    let done = AtomicU64::new(0);
    let lines: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let busy: Mutex<Vec<Option<Busy>>> = Mutex::new(vec![None; most]);
    let left: Mutex<Vec<Task>> = Mutex::new(Vec::new());
    let (videos_played, video_bytes_played, quick_made) = (AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0));
    // The trial's time is up.
    let over = AtomicBool::new(false);
    let started = Instant::now();
    let videos_began: Mutex<Option<Instant>> = Mutex::new(None);
    let wanted = || trial || (opts.videos)();
    let mut video_pace = Pace::default();
    let mut quick_pace = Pace::default();
    std::thread::scope(|s| {
        for w in 0..most {
            let (queue, results, done, lines, busy, left, target, over) = (&queue, &results, &done, &lines, &busy, &left, &target, &over);
            let (videos_played, video_bytes_played, quick_made, videos_began) = (&videos_played, &video_bytes_played, &quick_made, &videos_began);
            let (exes, mtimes, write_done, wanted) = (&exes, &mtimes, &write_done, &wanted);
            s.spawn(move || loop {
                if stop() || over.load(Ordering::Relaxed) {
                    break;
                }
                if w >= target.load(Ordering::Relaxed) {
                    // Not needed yet: more run at once only while the
                    // processor has room.
                    if queue.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                    continue;
                }
                let next = queue.lock().unwrap_or_else(|e| e.into_inner()).pop();
                let Some(t) = next else { break };
                let video = t.check == Check::Video;
                if video && !wanted() {
                    // Not to be played when its turn came: left out.
                    done.fetch_add(t.files(), Ordering::Relaxed);
                    left.lock().unwrap_or_else(|e| e.into_inner()).push(t);
                    continue;
                }
                if video {
                    videos_began.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Instant::now);
                }
                let shown = t.path.to_string_lossy().into_owned();
                crate::live::now(&shown);
                busy.lock().unwrap_or_else(|e| e.into_inner())[w] = Some(Busy { video, size: t.size, ..Default::default() });
                let unticked = AtomicBool::new(false);
                let stop_this = || {
                    if stop() || over.load(Ordering::Relaxed) {
                        return true;
                    }
                    if video && !wanted() {
                        unticked.store(true, Ordering::Relaxed);
                        return true;
                    }
                    false
                };
                let watch = |u: tools::Usage| {
                    if let Some(b) = busy.lock().unwrap_or_else(|e| e.into_inner())[w].as_mut() {
                        b.read = u.read.min(b.size);
                        b.cpu = u.cpu;
                    }
                };
                let v = check_one(t.check, &exes[&t.check], &crate::walk::literal(&shown), t.size, &stop_this, &watch);
                busy.lock().unwrap_or_else(|e| e.into_inner())[w] = None;
                if matches!(&v, Verdict::NotChecked(why) if why == "stopped") {
                    if unticked.load(Ordering::Relaxed) {
                        // Unticked while it played: left out, with the rest.
                        done.fetch_add(t.files(), Ordering::Relaxed);
                        left.lock().unwrap_or_else(|e| e.into_inner()).push(t);
                    }
                    continue;
                }
                // It and its identical copies: the same result.
                let mut all = vec![(t.path.clone(), t.size)];
                all.extend(t.twins.iter().cloned());
                for (path, size) in &all {
                    let shown = path.to_string_lossy().into_owned();
                    crate::live::done(&shown, *size);
                    write_done(done_line(t.check, *size, mtimes.get(path).copied().unwrap_or(0), &v, &path.display().to_string()));
                    if let Verdict::Damaged(m) = &v {
                        lines.lock().unwrap_or_else(|e| e.into_inner()).push(format!("{}: {}  ({m})", t.check.tool().name(), path.display()));
                        // ("What just happened": the path inside WORK.)
                        let inside = path.strip_prefix(work).map(|p| p.display().to_string()).unwrap_or_else(|_| shown.clone());
                        crate::live::found(&inside, &format!("{}: {m}", t.check.tool().name()));
                    }
                    results.lock().unwrap_or_else(|e| e.into_inner()).push((t.check, path.display().to_string(), v.clone()));
                }
                if video {
                    videos_played.fetch_add(1, Ordering::Relaxed);
                    video_bytes_played.fetch_add(t.size, Ordering::Relaxed);
                } else {
                    quick_made.fetch_add(all.len() as u64, Ordering::Relaxed);
                }
                done.fetch_add(all.len() as u64, Ordering::Relaxed);
            });
        }
        // Progress, from here. Stopped, the workers end after the file they
        // are on (its tool is stopped too), and the scope waits for them.
        let mut cpu_then = crate::sys::cpu_times();
        let mut looked = Instant::now();
        let mut room_seen = 0;
        loop {
            for l in lines.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
                say(&l);
            }
            let d = done.load(Ordering::Relaxed);
            amount(d, total);
            if opts.trial.is_some_and(|t| started.elapsed() >= t) {
                over.store(true, Ordering::Relaxed);
            }
            let at = started.elapsed().as_secs_f64();
            let n = target.load(Ordering::Relaxed);
            // What the videos playing now have got through, and (every five
            // seconds) the processor time each has used since the last look.
            let look = looked.elapsed() >= Duration::from_secs(5);
            let (working, playing_bytes, per_video) = {
                let mut b = busy.lock().unwrap_or_else(|e| e.into_inner());
                let working = b.iter().filter(|x| x.is_some()).count();
                let playing_bytes: u64 = b.iter().flatten().filter(|x| x.video).map(|x| x.read).sum();
                let mut per_video = None;
                if look {
                    let secs = looked.elapsed().as_secs_f64().max(0.001);
                    let used: Vec<f64> = b
                        .iter_mut()
                        .flatten()
                        .filter(|x| x.video)
                        .map(|x| {
                            let u = (x.cpu - x.cpu_seen).max(0.0);
                            x.cpu_seen = x.cpu;
                            u / secs
                        })
                        .collect();
                    if !used.is_empty() {
                        per_video = Some(used.iter().sum::<f64>() / used.len() as f64);
                    }
                }
                (working, playing_bytes, per_video)
            };
            let video_bytes = video_bytes_played.load(Ordering::Relaxed).saturating_add(playing_bytes);
            video_pace.note(at, video_bytes);
            quick_pace.note(at, quick_made.load(Ordering::Relaxed));
            // Room for one more at once?
            if look {
                looked = Instant::now();
                let cpu_now = crate::sys::cpu_times();
                let cpu_busy = match (cpu_then, cpu_now) {
                    (Some(a), Some(b)) => crate::sys::busy_between(a, b),
                    _ => None,
                };
                cpu_then = cpu_now;
                let free = crate::sys::memory_free();
                let more_to_do = !queue.lock().unwrap_or_else(|e| e.into_inner()).is_empty();
                if free.is_some_and(|f| f < MEMORY_LOW) && n > 1 {
                    target.store(n - 1, Ordering::Relaxed);
                    room_seen = 0;
                } else if more_to_do && n < most && room_for_more(cpu_busy, working >= n, per_video, free) {
                    // (Seen twice running: not a moment's lull.)
                    room_seen += 1;
                    if room_seen >= 2 {
                        room_seen = 0;
                        target.store(n + 1, Ordering::Relaxed);
                        most_at_once = most_at_once.max(n + 1);
                        say(&format!("  ({} at once now: the processor had room for more.)", n + 1));
                    }
                } else {
                    room_seen = 0;
                }
            }
            // The time left, for the page: the quick checks at the pace they
            // go, the videos at the speed they play (or were measured at).
            let want = wanted();
            let left_bytes: u64 = left.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|t| t.size).sum();
            let rate = video_pace.rate().or(opts.video_rate);
            let video_rest = if want { o.videos.to_play_bytes.saturating_sub(video_bytes.saturating_add(left_bytes)) } else { 0 };
            let video_left = if video_rest == 0 { Some(0) } else { VideoNumbers::secs_for(video_rest, rate) };
            let quick_rest = quick_total.saturating_sub(quick_made.load(Ordering::Relaxed));
            let quick_left = if quick_rest == 0 { Some(0) } else { quick_pace.rate().map(|r| (quick_rest as f64 / r).round() as u64) };
            crate::live::videos(crate::live::Videos {
                played: videos_played.load(Ordering::Relaxed),
                to_play: o.videos.to_play,
                bytes: video_bytes,
                total: o.videos.to_play_bytes,
                left: video_left,
                at_once: n as u64,
                wanted: want,
                stage_left: quick_left.zip(video_left).map(|(q, v)| q + v),
            });
            if d >= total || stop() || over.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    });
    for l in lines.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
        say(&l);
    }
    crate::live::found_end();
    // (The short-named copies' folder, empty again.)
    let _ = std::fs::remove_dir(std::env::temp_dir().join(format!("arp-extra-{}", std::process::id())));
    o.stopped = stop();
    o.at_once = most_at_once;
    let left = left.into_inner().unwrap_or_else(|e| e.into_inner());
    // (As to_play: the videos to play, not their identical copies - the
    // time it takes is theirs.)
    o.videos.left = left.len() as u64;
    o.videos.left_bytes = left.iter().map(|t| t.size).sum();
    o.videos.played = videos_played.load(Ordering::Relaxed);
    o.videos.played_bytes = video_bytes_played.load(Ordering::Relaxed);
    // How fast they went: in the trial, over its last two minutes (all of
    // it); otherwise over all the time they played.
    let overall = videos_began
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
        .map(|t| t.elapsed().as_secs_f64())
        .filter(|s| *s >= 0.1 && o.videos.played_bytes > 0)
        .map(|s| o.videos.played_bytes as f64 / s);
    o.videos.rate = if trial { video_pace.rate().or(overall) } else { overall.or(video_pace.rate()) };
    let mut results = results.into_inner().unwrap_or_else(|e| e.into_inner());
    results.extend(remembered);
    results.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    // Files checked, per check and kind: all of them, and those read whole.
    let mut checked_by_kind: std::collections::HashMap<(Check, String), (u64, u64)> = std::collections::HashMap::new();
    for (c, path, v) in &results {
        if !matches!(v, Verdict::NotChecked(_)) {
            let e = checked_by_kind.entry((*c, ext_of_path(path))).or_default();
            e.0 += 1;
            if read_whole.contains(&path.to_lowercase()) {
                e.1 += 1;
            }
        }
    }
    for (c, path, v) in results {
        let e = o.counts.entry(c).or_insert((0, 0));
        match v {
            Verdict::Fine => e.0 += 1,
            Verdict::Damaged(message) => {
                e.0 += 1;
                e.1 += 1;
                o.findings.push(Finding { path, check: c, message });
            }
            Verdict::Locked => {
                e.0 += 1;
                o.locked.push(path);
            }
            Verdict::NotChecked(why) => {
                if why != "stopped" {
                    o.not_checked.push((path, why));
                }
            }
        }
    }
    amount(done.load(Ordering::Relaxed), total);
    set_aside_doubtful(&mut o, &checked_by_kind, opts.map_items.is_some().then_some(&read_whole));
    // (The trial, like a run stopped part way, adds what it found to the
    // lists: it did not check the rest.)
    let partial = o.stopped || trial;
    write_lists(work, &mut o, partial, trial.then_some(Check::Video))?;
    Ok(o)
}

/// Without the map check: at least this many files of one kind flagged, and
/// more than this share of those checked, and a check's findings for that
/// kind are not believed.
const DOUBT_AT_LEAST: u64 = 30;
const DOUBT_SHARE: f64 = 0.3;
/// With it: among the files read whole from the old drive (exactly as they
/// were on it, so rarely damaged), at least this many flagged and more than
/// this share.
const DOUBT_WHOLE_AT_LEAST: u64 = 10;
const DOUBT_WHOLE_SHARE: f64 = 0.03;

/// A path's extension, from its file name only.
fn ext_of_path(p: &str) -> String {
    ext_lower(p.rsplit(['\\', '/']).next().unwrap_or(p))
}

/// `checked`: per (check, kind), the files checked (all, read whole).
/// `read_whole`: the files read whole from the old drive, when the map check
/// is known. Then only findings among those are weighed (and set aside): a
/// file the drive damaged is listed whatever any tool says of its kind.
fn set_aside_doubtful(
    o: &mut Outcome,
    checked: &std::collections::HashMap<(Check, String), (u64, u64)>,
    read_whole: Option<&HashSet<String>>,
) {
    let mut flagged: std::collections::HashMap<(Check, String), u64> = std::collections::HashMap::new();
    for f in &o.findings {
        if read_whole.is_none_or(|w| w.contains(&f.path.to_lowercase())) {
            *flagged.entry((f.check, ext_of_path(&f.path))).or_default() += 1;
        }
    }
    let mut out: Vec<(Check, String)> = Vec::new();
    for ((c, e), n) in &flagged {
        let (all, whole) = checked.get(&(*c, e.clone())).copied().unwrap_or((0, 0));
        let (of, at_least, share) =
            if read_whole.is_some() { (whole, DOUBT_WHOLE_AT_LEAST, DOUBT_WHOLE_SHARE) } else { (all, DOUBT_AT_LEAST, DOUBT_SHARE) };
        if *n >= at_least && of > 0 && (*n as f64) > share * of as f64 {
            o.doubtful.push(Doubt { check: *c, ext: e.clone(), flagged: *n, checked: of, read_whole: read_whole.is_some() });
            out.push((*c, e.clone()));
        }
    }
    o.doubtful.sort();
    if out.is_empty() {
        return;
    }
    o.findings.retain(|f| {
        !(out.contains(&(f.check, ext_of_path(&f.path))) && read_whole.is_none_or(|w| w.contains(&f.path.to_lowercase())))
    });
    for d in &o.doubtful {
        if let Some(x) = o.counts.get_mut(&d.check) {
            x.1 = x.1.saturating_sub(d.flagged);
        }
    }
}

/// The findings, into REPAIR\damaged-lists: one list per check, added to
/// what an earlier run found (so a run that was stopped loses nothing).
/// `partial`: the run did not get to every file (stopped, or the trial), so
/// what earlier runs listed stays. `only`: that check's list alone (the
/// trial's videos).
fn write_lists(work: &Path, o: &mut Outcome, partial: bool, only: Option<Check>) -> Result<(), String> {
    let repair = crate::repair::repair_dir(work)?;
    let dir = repair.join(crate::repair::LISTS);
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    for c in Check::ALL {
        if only.is_some_and(|x| x != c) {
            continue;
        }
        let mine: Vec<&Finding> = o.findings.iter().filter(|f| f.check == c).collect();
        let p = dir.join(c.list_name());
        let mut lines: BTreeMap<String, String> = BTreeMap::new();
        // (As bytes: a list saved again by another editor, in Windows' old
        // code page, must not lose what an earlier run found.)
        if let Ok(bytes) = std::fs::read(&p) {
            let old = String::from_utf8_lossy(&bytes);
            for l in old.lines() {
                if l.starts_with('#') || l.trim().is_empty() {
                    continue;
                }
                let path = l.split('\t').next().unwrap_or("").to_string();
                if Path::new(&path).exists() {
                    lines.insert(path.to_lowercase(), l.to_string());
                }
            }
        }
        // A file checked again and found fine this time comes off the list,
        // unless the run was stopped before it got there.
        if !partial {
            lines.clear();
        }
        for f in &mine {
            lines.insert(f.path.to_lowercase(), format!("{}\t{}: {}", f.path, c.tool().name(), f.message.replace(['\t', '\r', '\n'], " ")));
        }
        if lines.is_empty() {
            if !partial && p.exists() {
                let _ = std::fs::remove_file(&p);
            }
            continue;
        }
        let mut text = format!(
            "# Found by Automatic Recovery Probe's extra checks: {} ({}). One damaged file per line, then what the tool said.\r\n",
            c.title(),
            crate::fmt::when_now()
        );
        for l in lines.values() {
            text.push_str(l);
            text.push_str("\r\n");
        }
        crate::sys::write_safely(&p, text.as_bytes()).map_err(|e| format!("could not write {}: {e}", p.display()))?;
        o.lists.push(p);
    }
    Ok(())
}

/// The whole result as a CSV, for the results folder.
pub fn write_report(path: &Path, o: &Outcome) -> std::io::Result<()> {
    let mut t = format!("\u{FEFF}{}", csv_row(&["result", "check", "path", "what the tool said"]));
    for f in &o.findings {
        t.push_str(&csv_row(&["DAMAGED", f.check.title(), &f.path, &f.message]));
    }
    for p in &o.locked {
        t.push_str(&csv_row(&["HAS A PASSWORD", "", p, "cannot be tested without its password"]));
    }
    for (p, why) in &o.not_checked {
        t.push_str(&csv_row(&["NOT CHECKED", "", p, why]));
    }
    std::fs::write(path, t)
}

pub fn summary(o: &Outcome) -> Vec<String> {
    let mut t = Vec::new();
    let found = o.findings.len();
    t.push(if found == 0 {
        "The extra checks found nothing wrong.".to_string()
    } else {
        format!(
            "The extra checks found {} damaged {} (listed in REPAIR\\damaged-lists, so they are copied out to repair).",
            commas(found as u64),
            if found == 1 { "file" } else { "files" }
        )
    });
    for (c, (n, bad)) in &o.counts {
        t.push(format!("  {}: {} checked, {} damaged", c.title(), commas(*n), commas(*bad)));
    }
    for (c, why) in &o.left_out {
        t.push(format!("  {}: not checked - {why}", c.title()));
    }
    let v = &o.videos;
    if v.left > 0 {
        let at = VideoNumbers::secs_for(v.left_bytes, v.rate)
            .map(|s| format!(" - about {} at the speed they played", crate::fmt::duration(s)))
            .unwrap_or_default();
        t.push(format!(
            "  {}: {} {} ({}) left out, as Play every video through was not ticked when their turn came{at}.",
            Check::Video.title(),
            commas(v.left),
            if v.left == 1 { "video" } else { "videos" },
            crate::fmt::size(v.left_bytes)
        ));
    }
    if v.played > 0 {
        if let Some(r) = v.rate {
            t.push(format!(
                "  The videos played at {} a second, {} at most at once.",
                crate::fmt::size(r.round() as u64),
                o.at_once
            ));
        }
    }
    if o.twins > 0 {
        t.push(if o.twins == 1 {
            "  1 file is an identical copy of another (the same SHA-256): the other was checked, and its result stands for both."
                .to_string()
        } else {
            format!(
                "  {} files are identical copies of others (the same SHA-256): one of each was checked, and its result \
                 stands for every copy.",
                commas(o.twins)
            )
        });
    }
    for d in &o.doubtful {
        t.push(if d.read_whole {
            format!(
                "  {}: it reported errors in {} of the {} .{} files the old drive gave up whole (every byte read, so each is \
                 exactly as it was on it) - far too many to be damage, so {} most likely cannot read that kind of file \
                 fully. Those are not listed as damaged (every .{} file the drive did damage still is).",
                d.check.title(),
                commas(d.flagged),
                commas(d.checked),
                d.ext,
                d.check.tool().name(),
                d.ext
            )
        } else {
            format!(
                "  {}: it reported errors in {} of {} .{} files - far too many to be damage from the old drive (which \
                 touched few files), so {} most likely cannot read that kind of file fully. They are not listed as damaged \
                 (the map check still names every one the drive damaged).",
                d.check.title(),
                commas(d.flagged),
                commas(d.checked),
                d.ext,
                d.check.tool().name()
            )
        });
    }
    if !o.locked.is_empty() {
        t.push(format!("{} archives or PDFs have a password, so they could not be tested.", commas(o.locked.len() as u64)));
    }
    if !o.not_checked.is_empty() {
        t.push(format!("{} files could not be checked (see the report).", commas(o.not_checked.len() as u64)));
    }
    if o.stopped {
        t.push("Stopped before the end: what was found so far is kept; Start again to finish.".to_string());
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ran(code: i32, out: &str, err: &str) -> tools::Ran {
        tools::Ran { code: Some(code), out: out.to_string(), err: err.to_string(), ..Default::default() }
    }

    /// A list an earlier run wrote, saved again in Windows' old code page
    /// ("Café" as one byte): a run stopped before it got to those files keeps
    /// what that run found (before: none of it, the list read as text failed).
    #[test]
    fn a_list_saved_by_another_editor_keeps_what_was_found() {
        let base = std::env::temp_dir().join(format!("arp-extra-latin1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let work = base.join("WORK");
        std::fs::create_dir_all(&work).unwrap();
        for n in ["a.mp3", "b.mp3"] {
            std::fs::write(work.join(n), b"mp3").unwrap();
        }
        let lists = crate::repair::repair_dir(&work).unwrap().join(crate::repair::LISTS);
        std::fs::create_dir_all(&lists).unwrap();
        let p = lists.join(Check::Mp3.list_name());
        let a = work.join("a.mp3").display().to_string();
        let mut old = format!("# an earlier run\r\n{a}\tMP3val: bad frames\r\n").into_bytes();
        old.extend_from_slice(format!("{}\\Caf\u{e9}.mp3", work.display()).as_bytes());
        old.truncate(old.len() - "\u{e9}.mp3".len());
        old.extend_from_slice(b"\xe9.mp3\tMP3val: bad frames\r\n");
        std::fs::write(&p, &old).unwrap();
        let b = work.join("b.mp3").display().to_string();
        let mut o = Outcome { stopped: true, ..Default::default() };
        o.findings.push(Finding { path: b.clone(), check: Check::Mp3, message: "bad header".to_string() });
        write_lists(&work, &mut o, true, None).unwrap();
        let text = String::from_utf8_lossy(&std::fs::read(&p).unwrap()).into_owned();
        assert!(text.contains(&a), "what the earlier run found is kept: {text}");
        assert!(text.contains(&b), "{text}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn which_checks_a_file_gets() {
        assert_eq!(checks_for("a.docx", false, false), vec![Check::Archive]);
        assert_eq!(checks_for("a.MP3", false, false), vec![Check::Mp3, Check::Sound]);
        assert_eq!(checks_for("a.heic", false, false), Vec::<Check>::new());
        assert_eq!(checks_for("a.heic", true, false), vec![Check::Photo]);
        assert_eq!(checks_for("a.mov", true, false), Vec::<Check>::new());
        assert_eq!(checks_for("a.mov", true, true), vec![Check::Video]);
        assert_eq!(checks_for("a.pdf", false, false), vec![Check::Pdf]);
        assert_eq!(checks_for("noext", true, true), Vec::<Check>::new());
    }

    #[test]
    fn sound_is_played_through_on_one_thread() {
        let a: Vec<String> = play_through_args(Path::new("track.flac")).iter().map(|s| s.to_string_lossy().into_owned()).collect();
        let at = |w: &str| a.iter().position(|x| x == w).unwrap();
        // (An input option: before the file.)
        assert_eq!(a[at("-threads:a") + 1], "1");
        assert!(at("-threads:a") < at("-i") && a[at("-i") + 1] == "track.flac", "{a:?}");
        assert!(!a.iter().any(|x| x == "-threads"), "video keeps its threads: {a:?}");
        // The deblocking filter left out, for the file's decoders (an input
        // option too).
        assert_eq!(a[at("-skip_loop_filter") + 1], "all");
        assert!(at("-skip_loop_filter") < at("-i"), "{a:?}");
    }

    #[test]
    fn ffmpeg_errors_are_told_from_chatter() {
        assert_eq!(ffmpeg_verdict(&ran(0, "", "")), Verdict::Fine);
        assert_eq!(ffmpeg_verdict(&ran(0, "", "[swscaler @ 0x1] deprecated pixel format used, make sure you did set range correctly\n")), Verdict::Fine);
        match ffmpeg_verdict(&ran(0, "", "[h264 @ 0x55] error while decoding MB 12 30, bytestream -5\n[h264 @ 0x55] concealing 1200 DC, 1200 AC, 1200 MV errors in P frame\n")) {
            Verdict::Damaged(m) => assert!(m.starts_with("[h264 @ 0x55] error while decoding MB") && m.contains("2 errors"), "{m}"),
            v => panic!("{v:?}"),
        }
        match ffmpeg_verdict(&ran(1, "", "x.jpg: Invalid data found when processing input\n")) {
            Verdict::Damaged(m) => assert!(m.contains("Invalid data"), "{m}"),
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn sevenzip_results_are_read() {
        assert_eq!(sevenzip_verdict(&ran(0, "Testing archive: a.zip\nEverything is Ok\n", "")), Verdict::Fine);
        assert_eq!(sevenzip_verdict(&ran(0, "Testing archive: E:\\W\\Error reports\\crash.zip\nEverything is Ok\n", "")), Verdict::Fine);
        assert_eq!(
            sevenzip_verdict(&ran(1, "Testing archive: a.zip\nWARNINGS:\nThere are data after the end of archive\n", "")),
            Verdict::Fine
        );
        assert_eq!(sevenzip_verdict(&ran(2, "", "ERROR: Wrong password : a.txt\n")), Verdict::Locked);
        match sevenzip_verdict(&ran(2, "ERROR: CRC Failed : photos/a.jpg\nSub items Errors: 1\n", "")) {
            Verdict::Damaged(m) => assert!(m.contains("CRC Failed"), "{m}"),
            v => panic!("{v:?}"),
        }
        match sevenzip_verdict(&ran(2, "", "ERROR: a.zip\nCan not open the file as archive\n")) {
            Verdict::Damaged(m) => assert!(m.contains("Can not open the file as archive"), "{m}"),
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn mp3val_warnings_are_read() {
        assert_eq!(mp3val_verdict(&ran(0, "Analyzing file \"a.mp3\"...\nINFO: \"a.mp3\": 4576 MPEG frames (MPEG 1 Layer III), +ID3v2, CBR\nDone!\n", "")), Verdict::Fine);
        match mp3val_verdict(&ran(0, "WARNING: \"C:\\M\\a.mp3\" (offset 0x2e5b4f): MPEG stream error, resynchronized successfully\nWARNING: \"C:\\M\\a.mp3\": Wrong number of MPEG frames specified in Xing header (4576 instead of 3000)\nDone!\n", "")) {
            Verdict::Damaged(m) => {
                assert!(m.starts_with("MPEG stream error, resynchronized successfully"), "{m}");
                assert!(m.contains("Wrong number of MPEG frames"), "{m}");
            }
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn qpdf_results_are_read() {
        assert_eq!(qpdf_verdict(&ran(0, "checking a.pdf\nPDF Version: 1.4\nFile is not encrypted\nFile is not linearized\nNo syntax or stream encoding errors found\n", "")), Verdict::Fine);
        match qpdf_verdict(&ran(3, "checking a.pdf\nPDF Version: 1.4\n", "WARNING: a.pdf: file is damaged\nWARNING: a.pdf (offset 1234): xref not found\nqpdf: operation succeeded with warnings\n")) {
            Verdict::Damaged(m) => assert!(m.contains("file is damaged"), "{m}"),
            v => panic!("{v:?}"),
        }
        assert_eq!(qpdf_verdict(&ran(2, "", "a.pdf: invalid password\n")), Verdict::Locked);
    }

    #[test]
    fn a_tool_that_flags_too_many_of_a_kind_is_not_believed() {
        // Without the map check: by share of all files checked.
        let mut o = Outcome::default();
        let mut checked = std::collections::HashMap::new();
        for i in 0..100 {
            o.findings.push(Finding { path: format!("p{i}.heic"), check: Check::Photo, message: "error".into() });
        }
        for i in 0..3 {
            o.findings.push(Finding { path: format!("p{i}.jpg"), check: Check::Photo, message: "error".into() });
        }
        o.counts.insert(Check::Photo, (1200, 103));
        checked.insert((Check::Photo, "heic".to_string()), (200u64, 0u64));
        checked.insert((Check::Photo, "jpg".to_string()), (1000u64, 0u64));
        set_aside_doubtful(&mut o, &checked, None);
        assert_eq!(o.findings.len(), 3, "the three JPEGs stay");
        assert_eq!(
            o.doubtful,
            vec![Doubt { check: Check::Photo, ext: "heic".to_string(), flagged: 100, checked: 200, read_whole: false }]
        );
        assert_eq!(o.counts[&Check::Photo], (1200, 3));
        // A few of a kind, even all of them, are believed.
        let mut o = Outcome::default();
        for i in 0..5 {
            o.findings.push(Finding { path: format!("v{i}.mov"), check: Check::Video, message: "error".into() });
        }
        let mut c2 = std::collections::HashMap::new();
        c2.insert((Check::Video, "mov".to_string()), (5u64, 0u64));
        set_aside_doubtful(&mut o, &c2, None);
        assert_eq!(o.findings.len(), 5);
    }

    #[test]
    fn with_the_map_check_only_files_read_whole_are_weighed() {
        // 2,000 HEIC photos, 60 of them flagged among the 1,990 read whole (3%
        // is 59.7): FFmpeg's limit. The 10 the drive damaged stay listed, and
        // so do the 12 JPEGs flagged among 20,000 read whole.
        let mut o = Outcome::default();
        let mut whole = HashSet::new();
        for i in 0..70 {
            let p = format!("e:\\w\\h{i}.heic");
            if i >= 10 {
                whole.insert(p.clone());
            }
            o.findings.push(Finding { path: p, check: Check::Photo, message: "error".into() });
        }
        for i in 0..12 {
            let p = format!("e:\\w\\j{i}.jpg");
            whole.insert(p.clone());
            o.findings.push(Finding { path: p, check: Check::Photo, message: "error".into() });
        }
        o.counts.insert(Check::Photo, (22000, 82));
        let mut checked = std::collections::HashMap::new();
        checked.insert((Check::Photo, "heic".to_string()), (2000u64, 1990u64));
        checked.insert((Check::Photo, "jpg".to_string()), (20000u64, 20000u64));
        set_aside_doubtful(&mut o, &checked, Some(&whole));
        assert_eq!(
            o.doubtful,
            vec![Doubt { check: Check::Photo, ext: "heic".to_string(), flagged: 60, checked: 1990, read_whole: true }]
        );
        assert_eq!(o.findings.len(), 22, "the 10 damaged HEICs and the 12 JPEGs");
        assert!(o.findings.iter().filter(|f| f.path.ends_with(".heic")).all(|f| !whole.contains(&f.path)));
        assert_eq!(o.counts[&Check::Photo], (22000, 22));
    }

    #[test]
    fn what_was_checked_is_remembered() {
        let dir = std::env::temp_dir().join(format!("arp-extra-done-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("done.tsv");
        let mut t = String::new();
        t += &done_line(Check::Video, 10, 99, &Verdict::Fine, "E:\\W\\a.mov").unwrap();
        t += &done_line(Check::Pdf, 20, 98, &Verdict::Damaged("xref\tbroken".into()), "E:\\W\\b.pdf").unwrap();
        assert!(done_line(Check::Pdf, 1, 1, &Verdict::NotChecked("x".into()), "c").is_none());
        std::fs::write(&p, t).unwrap();
        let back = read_done(&p);
        assert!(matches!(back.get(&(Check::Video, "e:\\w\\a.mov".to_string())), Some((10, 99, Verdict::Fine))));
        assert!(matches!(back.get(&(Check::Pdf, "e:\\w\\b.pdf".to_string())), Some((20, 98, Verdict::Damaged(m))) if m == "xref broken"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_more_at_once_only_while_the_processor_has_room() {
        let gb = 1u64 << 30;
        assert!(room_for_more(Some(0.4), true, Some(3.5), Some(8 * gb)));
        assert!(room_for_more(Some(0.4), true, None, None), "photos (no video playing), memory not known");
        assert!(!room_for_more(Some(0.9), true, Some(3.5), Some(8 * gb)), "the processor is busy");
        assert!(!room_for_more(None, true, Some(3.5), Some(8 * gb)), "how busy is not known");
        assert!(!room_for_more(Some(0.4), false, Some(3.5), Some(8 * gb)), "a worker had nothing to do");
        assert!(!room_for_more(Some(0.2), true, Some(0.1), Some(8 * gb)), "the videos wait for the drive");
        assert!(!room_for_more(Some(0.4), true, Some(3.5), Some(gb)), "memory is short");
    }

    #[test]
    fn how_fast_is_taken_over_the_last_two_minutes() {
        let mut p = Pace::default();
        p.note(0.0, 0);
        p.note(10.0, 100);
        assert_eq!(p.rate(), None, "too soon to tell");
        p.note(20.0, 400);
        assert_eq!(p.rate(), Some(20.0));
        p.note(20.5, 900);
        assert_eq!(p.rate(), Some(20.0), "one a second at most");
        for s in 21..300 {
            p.note(s as f64, 400 + (s as u64 - 20) * 50);
        }
        assert!((p.rate().unwrap() - 50.0).abs() < 1e-9, "the start has dropped out: {:?}", p.rate());
        assert_eq!(VideoNumbers::secs_for(1000, Some(50.0)), Some(20));
        assert_eq!(VideoNumbers::secs_for(1000, None), None);
        assert_eq!(VideoNumbers::secs_for(1000, Some(0.0)), None);
    }

    /// A stand-in for FFmpeg: reads the file it plays, takes a moment, writes
    /// down which it played, and calls the ones named "bad..." damaged.
    #[cfg(unix)]
    fn fake_ffmpeg(dir: &Path, secs: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("ffmpeg");
        let log = dir.join("played.txt");
        std::fs::write(
            &p,
            format!(
                "#!/bin/sh\nf=\"\"; prev=\"\"\nfor a in \"$@\"; do if [ \"$prev\" = \"-i\" ]; then f=\"$a\"; fi; prev=\"$a\"; done\n\
                 cat \"$f\" > /dev/null\nsleep {secs}\necho \"$f\" >> \"{}\"\n\
                 case \"$(basename \"$f\")\" in bad*) echo \"[h264 @ 0x1] error while decoding MB 1 2, bytestream -5\" >&2;; esac\nexit 0\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn sha(d: &[u8]) -> String {
        let mut h = crate::sha256::Sha256::new();
        h.update(d);
        h.finish_hex()
    }

    /// WORK with videos and the copying's list of them (sizes and SHA-256s).
    #[cfg(unix)]
    fn work_with_videos(base: &Path, files: &[(&str, &[u8])]) -> PathBuf {
        let work = base.join("WORK");
        std::fs::create_dir_all(&work).unwrap();
        let mut log = crate::fmt::csv_row(&crate::rescue::LOG_HEADER);
        for (name, data) in files {
            let p = work.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, data).unwrap();
            let sha = sha(data);
            log.push_str(&crate::fmt::csv_row(&["FILE", &name.replace('/', "\\"), &data.len().to_string(), "OK", "", "", &sha]));
        }
        std::fs::write(work.join(crate::rescue::LOG_NAME), log).unwrap();
        work
    }

    #[cfg(unix)]
    fn options(work: &Path, results: &Path, videos: Arc<dyn Fn() -> bool + Send + Sync>) -> Options {
        Options {
            work: work.to_path_buf(),
            videos,
            jobs: 1,
            most: 1,
            done_list: Some(results.join("done.tsv")),
            map_items: None,
            trial: None,
            video_rate: None,
        }
    }

    fn played(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("played.txt"))
            .unwrap_or_default()
            .lines()
            .map(|l| l.rsplit('/').next().unwrap_or(l).to_string())
            .collect()
    }

    /// The biggest video is played first; an identical copy of one (the same
    /// size and SHA-256, the same kind) is not played again - the result is
    /// its too, damaged or not, and is remembered for it by name.
    #[cfg(unix)]
    #[test]
    fn identical_videos_are_played_once_and_the_biggest_first() {
        let base = std::env::temp_dir().join(format!("arp-extra-twins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let a = vec![1u8; 3000];
        let bad = vec![2u8; 2000];
        let work = work_with_videos(
            &base,
            &[("Camera/a.mp4", &a), ("Backup/a copy.mp4", &a), ("Camera/bad1.mov", &bad), ("Backup/bad1.mov", &bad), ("small.mp4", b"tiny")],
        );
        let exe = fake_ffmpeg(&base, "0.05");
        let exes = BTreeMap::from([(Check::Video, exe)]);
        let opts = options(&work, &base, Arc::new(|| true));
        let o = check_all(&opts, &exes, false, Outcome::default(), &mut |_| {}, &mut |_, _| {}, &|| false).unwrap();
        assert_eq!(played(&base), vec!["a.mp4", "bad1.mov", "small.mp4"], "the biggest first, each content once");
        assert_eq!(o.twins, 2);
        assert_eq!(o.counts[&Check::Video], (5, 2), "five checked, both bad1.mov damaged");
        let mut damaged: Vec<&str> = o.findings.iter().map(|f| f.path.as_str()).collect();
        damaged.sort();
        assert!(damaged.len() == 2 && damaged.iter().all(|p| p.ends_with("bad1.mov")), "{damaged:?}");
        assert_eq!((o.videos.to_play, o.videos.played, o.videos.left), (3, 3, 0));
        let list = std::fs::read_to_string(crate::repair::repair_dir(&work).unwrap().join(crate::repair::LISTS).join(Check::Video.list_name())).unwrap();
        assert!(list.contains("Camera") && list.contains("Backup"), "both copies listed to repair: {list}");
        // Run again: every one remembered, by name - none played again.
        std::fs::remove_file(base.join("played.txt")).unwrap();
        let o = check_all(&opts, &exes, false, Outcome::default(), &mut |_| {}, &mut |_, _| {}, &|| false).unwrap();
        assert!(played(&base).is_empty(), "{:?}", played(&base));
        assert_eq!(o.counts[&Check::Video], (5, 2));
        // A new identical copy: an earlier run's result for its content stands.
        std::fs::write(work.join("again.mp4"), &a).unwrap();
        let mut log = std::fs::read_to_string(work.join(crate::rescue::LOG_NAME)).unwrap();
        log.push_str(&crate::fmt::csv_row(&["FILE", "again.mp4", "3000", "OK", "", "", &sha(&a)]));
        std::fs::write(work.join(crate::rescue::LOG_NAME), log).unwrap();
        let o = check_all(&opts, &exes, false, Outcome::default(), &mut |_| {}, &mut |_, _| {}, &|| false).unwrap();
        assert!(played(&base).is_empty(), "{:?}", played(&base));
        assert_eq!(o.counts[&Check::Video], (6, 2));
        // A copy changed since it was copied (not its list's size) is played.
        std::fs::write(work.join("again.mp4"), vec![1u8; 2999]).unwrap();
        let o = check_all(&opts, &exes, false, Outcome::default(), &mut |_| {}, &mut |_, _| {}, &|| false).unwrap();
        assert_eq!(played(&base), vec!["again.mp4"]);
        assert_eq!(o.twins, 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Whether to play the videos is asked when their turn comes, and while
    /// they play: not ticked, they are left out (and counted, for later);
    /// unticked while one plays, it is stopped and left out with the rest.
    #[cfg(unix)]
    #[test]
    fn the_videos_are_played_only_while_asked_for() {
        let base = std::env::temp_dir().join(format!("arp-extra-ask-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let files: Vec<(String, Vec<u8>)> = (0..4).map(|i| (format!("v{i}.mp4"), vec![i as u8; 1000 + i * 10])).collect();
        let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
        let work = work_with_videos(&base, &refs);
        let exe = fake_ffmpeg(&base, "1");
        let exes = BTreeMap::from([(Check::Video, exe)]);
        // Not ticked: none played.
        let o = check_all(&options(&work, &base, Arc::new(|| false)), &exes, false, Outcome::default(), &mut |_| {}, &mut |_, _| {}, &|| false)
            .unwrap();
        assert!(played(&base).is_empty());
        assert_eq!((o.videos.to_play, o.videos.played, o.videos.left, o.videos.left_bytes), (4, 0, 4, 4060));
        assert!(summary(&o).iter().any(|l| l.contains("4 videos (") && l.contains("left out")), "{:?}", summary(&o));
        // Unticked while the first plays (the biggest, v3): it is stopped,
        // and it and the rest are left out.
        let asked = Arc::new(AtomicU64::new(0));
        let a2 = asked.clone();
        let t = Instant::now();
        let o = check_all(
            &options(&work, &base, Arc::new(move || {
                a2.fetch_add(1, Ordering::Relaxed);
                t.elapsed() < Duration::from_millis(400)
            })),
            &exes,
            false,
            Outcome::default(),
            &mut |_| {},
            &mut |_, _| {},
            &|| false,
        )
        .unwrap();
        assert!(t.elapsed() < Duration::from_secs(4), "the one playing was stopped: {:?}", t.elapsed());
        assert!(played(&base).is_empty(), "{:?}", played(&base));
        assert_eq!((o.videos.played, o.videos.left), (0, 4));
        assert!(asked.load(Ordering::Relaxed) > 2, "asked while it played");
        assert!(!o.stopped, "not a stop: the checks carry on to the end");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The trial plays only videos, for as long as it is given: what it got
    /// through is kept (listed, and remembered), the rest is for later, and
    /// the other lists are left as they are.
    #[cfg(unix)]
    #[test]
    fn the_trial_plays_videos_for_a_while_and_keeps_what_it_found() {
        let base = std::env::temp_dir().join(format!("arp-extra-trial-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let files: Vec<(String, Vec<u8>)> = (0..12).map(|i| (format!("bad{i}.mp4"), vec![i as u8; 500 + i])).collect();
        let mut refs: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
        refs.push(("doc.pdf", b"%PDF-1.4"));
        let work = work_with_videos(&base, &refs);
        let lists = crate::repair::repair_dir(&work).unwrap().join(crate::repair::LISTS);
        std::fs::create_dir_all(&lists).unwrap();
        let pdf_list = lists.join(Check::Pdf.list_name());
        std::fs::write(&pdf_list, format!("{}\tqpdf: broken\r\n", work.join("doc.pdf").display())).unwrap();
        let exe = fake_ffmpeg(&base, "0.4");
        let exes = BTreeMap::from([(Check::Video, exe), (Check::Pdf, PathBuf::from("/bin/false"))]);
        let mut opts = options(&work, &base, Arc::new(|| false));
        opts.trial = Some(Duration::from_millis(1500));
        let t = Instant::now();
        let o = check_all(&opts, &exes, false, Outcome::default(), &mut |_| {}, &mut |_, _| {}, &|| false).unwrap();
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
        let got = played(&base);
        assert!(!got.is_empty() && got.len() < 12, "{got:?}");
        assert_eq!(o.videos.played as usize, got.len());
        assert_eq!(o.videos.to_play, 12, "only the videos (the PDF is not the trial's)");
        assert!(o.videos.rate.is_some_and(|r| r > 0.0), "{:?}", o.videos);
        let list = std::fs::read_to_string(lists.join(Check::Video.list_name())).unwrap();
        assert_eq!(list.lines().filter(|l| !l.starts_with('#')).count(), got.len(), "{list}");
        assert!(std::fs::read_to_string(&pdf_list).unwrap().contains("qpdf: broken"), "the other lists stay as they were");
        // The extra checks after it play the others only.
        std::fs::remove_file(base.join("played.txt")).unwrap();
        let o2 = check_all(&options(&work, &base, Arc::new(|| true)), &BTreeMap::from([(Check::Video, base.join("ffmpeg"))]), false, Outcome::default(), &mut |_| {}, &mut |_, _| {}, &|| false)
            .unwrap();
        let again = played(&base);
        assert_eq!(again.len() + got.len(), 12, "{again:?} {got:?}");
        assert!(again.iter().all(|v| !got.contains(v)));
        assert_eq!(o2.counts[&Check::Video], (12, 12));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn library_previews_are_not_checked() {
        let work = Path::new(if cfg!(windows) { r"C:\W" } else { "/W" });
        let full = if cfg!(windows) {
            r"C:\W\Pics.photoslibrary\resources\derivatives\a.jpg".to_string()
        } else {
            "/W/Pics.photoslibrary/resources/derivatives/a.jpg".to_string()
        };
        assert!(skip(work, &full, "a.jpg"));
        let orig = full.replace("resources", "originals").replace("derivatives", "A");
        assert!(!skip(work, &orig, "a.jpg"));
        assert!(skip(work, &orig.replace("a.jpg", "._a.jpg"), "._a.jpg"));
    }
}
