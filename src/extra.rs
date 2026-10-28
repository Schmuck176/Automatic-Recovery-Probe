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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

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
    /// Play every video through too (hours to days for a big collection).
    pub videos: bool,
    /// How many tools run at once.
    pub jobs: usize,
    /// What earlier runs found, file by file (a file of the same size and
    /// date is not checked again: stopped, the checks carry on where they
    /// stopped).
    pub done_list: Option<PathBuf>,
    /// The map check's list of the files the never-read places touched
    /// (map-check-files.tsv), when there is one: every other file was read
    /// whole from the old drive.
    pub map_items: Option<Vec<crate::join::MapItem>>,
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
    let Ok(text) = std::fs::read_to_string(p) else { return out };
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
#[derive(Debug, PartialEq, Eq)]
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

fn check_one(check: Check, exe: &Path, path: &Path, size: u64, stop: &dyn Fn() -> bool) -> Verdict {
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
    let v = check_path(check, exe, copy.as_deref().unwrap_or(path), size, stop);
    if let Some(c) = copy {
        let _ = std::fs::remove_file(c);
    }
    v
}

fn check_path(check: Check, exe: &Path, path: &Path, size: u64, stop: &dyn Fn() -> bool) -> Verdict {
    let p = path.as_os_str().to_os_string();
    let args: Vec<OsString> = match check {
        // A made-up password: an archive that has one says "Wrong password"
        // instead of waiting for someone to type it.
        Check::Archive => vec!["t".into(), "-pnot-a-password".into(), "-y".into(), "-bd".into(), "--".into(), p],
        Check::Mp3 => vec![p],
        Check::Pdf => vec!["--check".into(), p],
        Check::Photo | Check::Sound | Check::Video => vec![
            "-hide_banner".into(),
            "-nostdin".into(),
            "-v".into(),
            "error".into(),
            "-i".into(),
            p,
            "-map".into(),
            "0:v?".into(),
            "-map".into(),
            "0:a?".into(),
            "-f".into(),
            "null".into(),
            "-".into(),
        ],
    };
    let r = match tools::run_low(exe, &args, None, limit_for(check, size), stop) {
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
    let mut o = Outcome::default();
    // Which tools there are.
    let mut exes: BTreeMap<Check, PathBuf> = BTreeMap::new();
    let mut heif = false;
    for c in Check::ALL {
        if c == Check::Video && !opts.videos {
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
    if !opts.videos {
        o.left_out.push((Check::Video, "playing every video through was not asked for (it takes hours)".to_string()));
    }
    // What to check.
    say(&format!("Looking for files to check in {}...", work.display()));
    let mut tasks: Vec<(Check, PathBuf, u64)> = Vec::new();
    let earlier = opts.done_list.as_ref().map(|p| read_done(p)).unwrap_or_default();
    // A repaired version put back keeps its original's size (an intact twin)
    // and dates: what was found before is about the damaged copy, so it is
    // checked again.
    let back = crate::repair::returned_now(work);
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
    let root = work.display().to_string();
    crate::walk::walk(&root, |dir, dirs, files| {
        dirs.retain(|d| !crate::formats::SYSTEM_DIRS.contains(&d.as_str()));
        for f in files {
            let full = crate::walk::join(dir, f);
            if skip(work, &full, f) {
                continue;
            }
            let wanted = checks_for(f, heif, opts.videos);
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
            for c in wanted {
                if exes.contains_key(&c) {
                    // Checked before, as it is now: what was found then stands.
                    match earlier.get(&(c, full.to_lowercase())).filter(|_| !put_back) {
                        Some((s, m, v)) if *s == size && *m == mtime => {
                            let v = match v {
                                Verdict::Fine => Verdict::Fine,
                                Verdict::Damaged(x) => Verdict::Damaged(x.clone()),
                                Verdict::Locked => Verdict::Locked,
                                Verdict::NotChecked(x) => Verdict::NotChecked(x.clone()),
                            };
                            remembered.push((c, full.clone(), v));
                        }
                        _ => {
                            mtimes.insert(PathBuf::from(&full), mtime);
                            tasks.push((c, PathBuf::from(&full), size));
                        }
                    }
                }
            }
        }
        !stop()
    });
    // Quick checks first, the videos last (the long part, best left running).
    tasks.sort_by_key(|t| (t.0 == Check::Video, t.0));
    let total = tasks.len() as u64;
    // (The page shows checks made of all there are, by kind of file. An MP3
    // gets two: MP3val's and FFmpeg's.)
    crate::live::begin("checks");
    crate::live::found_begin();
    for (_, path, size) in &tasks {
        crate::live::to_do(&path.to_string_lossy(), *size);
    }
    let bytes: u64 = tasks.iter().map(|t| t.2).sum();
    say(&format!("{} checks to make, over {} of files.", commas(total), crate::fmt::size(bytes)));
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
    for c in Check::ALL {
        let n = tasks.iter().filter(|t| t.0 == c).count() as u64;
        if n > 0 {
            say(&format!("  {}: {} files", c.title(), commas(n)));
        }
        if exes.contains_key(&c) {
            o.counts.insert(c, (0, 0));
        }
    }
    amount(0, total);
    // Several at once; each tool runs at a low priority.
    let queue = Mutex::new(tasks.into_iter().rev().collect::<Vec<_>>());
    let results: Mutex<Vec<(Check, String, Verdict)>> = Mutex::new(Vec::new());
    let done = AtomicU64::new(0);
    let jobs = opts.jobs.clamp(1, 8);
    let lines: Mutex<Vec<String>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..jobs {
            s.spawn(|| loop {
                if stop() {
                    break;
                }
                let next = queue.lock().unwrap_or_else(|e| e.into_inner()).pop();
                let Some((c, path, size)) = next else { break };
                let shown = path.to_string_lossy().into_owned();
                crate::live::now(&shown);
                let v = check_one(c, &exes[&c], &crate::walk::literal(&shown), size, stop);
                if !matches!(&v, Verdict::NotChecked(why) if why == "stopped") {
                    crate::live::done(&shown, size);
                }
                if let Some(line) = done_line(c, size, mtimes.get(&path).copied().unwrap_or(0), &v, &path.display().to_string()) {
                    use std::io::Write;
                    if let Some(f) = done_file.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                        let _ = f.write_all(line.as_bytes());
                    }
                }
                if let Verdict::Damaged(m) = &v {
                    lines.lock().unwrap_or_else(|e| e.into_inner()).push(format!("{}: {}  ({m})", c.tool().name(), path.display()));
                    // ("What just happened": the path inside WORK.)
                    let inside = path.strip_prefix(work).map(|p| p.display().to_string()).unwrap_or_else(|_| shown.clone());
                    crate::live::found(&inside, &format!("{}: {m}", c.tool().name()));
                }
                results.lock().unwrap_or_else(|e| e.into_inner()).push((c, path.display().to_string(), v));
                done.fetch_add(1, Ordering::Relaxed);
            });
        }
        // Progress, from here. Stopped, the workers end after the file they
        // are on (its tool is stopped too), and the scope waits for them.
        loop {
            for l in lines.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
                say(&l);
            }
            let d = done.load(Ordering::Relaxed);
            amount(d, total);
            if d >= total || stop() {
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
    write_lists(work, &mut o)?;
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
fn write_lists(work: &Path, o: &mut Outcome) -> Result<(), String> {
    let repair = crate::repair::repair_dir(work)?;
    let dir = repair.join(crate::repair::LISTS);
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    for c in Check::ALL {
        let mine: Vec<&Finding> = o.findings.iter().filter(|f| f.check == c).collect();
        let p = dir.join(c.list_name());
        let mut lines: BTreeMap<String, String> = BTreeMap::new();
        if let Ok(old) = std::fs::read_to_string(&p) {
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
        if !o.stopped {
            lines.clear();
        }
        for f in &mine {
            lines.insert(f.path.to_lowercase(), format!("{}\t{}: {}", f.path, c.tool().name(), f.message.replace(['\t', '\r', '\n'], " ")));
        }
        if lines.is_empty() {
            if !o.stopped && p.exists() {
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
