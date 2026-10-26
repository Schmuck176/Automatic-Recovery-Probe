//! What the job running now is getting through, for the page to show as it
//! goes: files and bytes done of all there are to do, kind by kind (photos,
//! videos, music...), and the file it is on now.
//!
//! The steps that go file by file (copying, checking the copies, checking
//! every file, the extra checks, the repairs, the second copy) count each
//! file here twice: once when they list what there is to do, once when it is
//! done. One job runs at a time (web.rs), so there is one board. Nothing
//! watches it outside the page (the command line, the tests): then every
//! call is a quick no-op.

use crate::fmt::{commas, json_str, size};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The kinds of file, in the order the page shows them.
pub const KINDS: [&str; 6] = ["Photos", "Videos", "Music and sound", "Documents", "Archives", "Other files"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Count {
    pub files: u64,
    pub bytes: u64,
}

impl Count {
    fn add(&mut self, bytes: u64) {
        self.files += 1;
        self.bytes += bytes;
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Board {
    /// What the numbers count, in a word the page puts after them
    /// ("copied", "checked", "repaired").
    pub what: String,
    pub done: [Count; 6],
    pub total: [Count; 6],
    /// The file being worked on now (as the step names it: a path).
    pub now: String,
}

impl Board {
    pub fn new(what: &str) -> Board {
        Board { what: what.to_string(), ..Default::default() }
    }
    pub fn to_do(&mut self, name: &str, bytes: u64) {
        self.total[kind_of(name)].add(bytes);
    }
    pub fn done(&mut self, name: &str, bytes: u64) {
        let k = kind_of(name);
        self.done[k].add(bytes);
        // (Never more done than there was to do: a file the listing missed
        // counts in both.)
        if self.done[k].files > self.total[k].files {
            self.total[k].files = self.done[k].files;
        }
        if self.done[k].bytes > self.total[k].bytes {
            self.total[k].bytes = self.done[k].bytes;
        }
    }
    pub fn sum(c: &[Count; 6]) -> Count {
        c.iter().fold(Count::default(), |a, b| Count { files: a.files + b.files, bytes: a.bytes + b.bytes })
    }
    pub fn is_empty(&self) -> bool {
        Board::sum(&self.total).files == 0 && Board::sum(&self.done).files == 0
    }

    /// For the page: {"what":..,"files":[done,total],"bytes":[done,total],
    /// "kinds":[{"name":..,"files":[d,t],"bytes":[d,t]}...],"now":..}.
    pub fn json(&self) -> String {
        let pair = |d: u64, t: u64| format!("[{d},{t}]");
        let (d, t) = (Board::sum(&self.done), Board::sum(&self.total));
        let kinds: Vec<String> = (0..KINDS.len())
            .filter(|&k| self.total[k].files > 0)
            .map(|k| {
                format!(
                    "{{\"name\":{},\"files\":{},\"bytes\":{}}}",
                    crate::fmt::json_str(KINDS[k]),
                    pair(self.done[k].files, self.total[k].files),
                    pair(self.done[k].bytes, self.total[k].bytes)
                )
            })
            .collect();
        format!(
            "{{\"what\":{},\"files\":{},\"bytes\":{},\"kinds\":[{}],\"now\":{}}}",
            crate::fmt::json_str(&self.what),
            pair(d.files, t.files),
            pair(d.bytes, t.bytes),
            kinds.join(","),
            crate::fmt::json_str(&self.now)
        )
    }
}

/// Which kind a file is, by its name.
pub fn kind_of(name: &str) -> usize {
    let base = name.rsplit(['\\', '/']).next().unwrap_or(name);
    let ext = match base.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => e.to_ascii_lowercase(),
        _ => return 5,
    };
    let e = ext.as_str();
    if crate::extra::PHOTO_EXTS.contains(&e) || crate::extra::HEIF_EXTS.contains(&e) || crate::formats::is_raw_ext(&format!(".{e}")) || e == "psd" {
        0
    } else if crate::extra::VIDEO_EXTS.contains(&e) {
        1
    } else if crate::extra::SOUND_EXTS.contains(&e) {
        2
    } else if matches!(
        e,
        "pdf" | "doc" | "docx" | "docm" | "dot" | "dotx" | "xls" | "xlsx" | "xlsm" | "xltx" | "ppt" | "pptx" | "pptm"
            | "potx" | "ppsx" | "odt" | "ods" | "odp" | "odg" | "rtf" | "txt" | "md" | "csv" | "pages" | "numbers"
            | "key" | "epub" | "xps" | "one"
    ) {
        3
    } else if crate::extra::ARCHIVE_EXTS.contains(&e) || matches!(e, "dmg" | "iso" | "sitx" | "sit") {
        4
    } else {
        5
    }
}

static WATCHED: AtomicBool = AtomicBool::new(false);
static BOARD: Mutex<Option<Board>> = Mutex::new(None);

fn with(f: impl FnOnce(&mut Board)) {
    if !WATCHED.load(Ordering::Relaxed) {
        return;
    }
    let mut b = BOARD.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(b) = b.as_mut() {
        f(b);
    }
}

/// The page starts (true) or ends (false) watching: a job began or ended.
pub fn watch(on: bool) {
    let mut b = BOARD.lock().unwrap_or_else(|e| e.into_inner());
    *b = None;
    WATCHED.store(on, Ordering::Relaxed);
}

/// A step begins counting afresh (what its numbers count, in a word).
pub fn begin(what: &str) {
    if !WATCHED.load(Ordering::Relaxed) {
        return;
    }
    *BOARD.lock().unwrap_or_else(|e| e.into_inner()) = Some(Board::new(what));
}

/// Nothing being counted (between the stages of Start).
pub fn clear() {
    if WATCHED.load(Ordering::Relaxed) {
        *BOARD.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// One more file to do.
pub fn to_do(name: &str, bytes: u64) {
    with(|b| b.to_do(name, bytes));
}

/// One file done.
pub fn done(name: &str, bytes: u64) {
    with(|b| b.done(name, bytes));
}

/// The file being worked on now.
pub fn now(name: &str) {
    with(|b| {
        if b.now != name {
            b.now = name.to_string();
        }
    });
}

/// The numbers now, if anything is being counted.
pub fn snapshot() -> Option<Board> {
    if !WATCHED.load(Ordering::Relaxed) {
        return None;
    }
    BOARD.lock().unwrap_or_else(|e| e.into_inner()).clone().filter(|b| !b.is_empty())
}

// ---------------------------------------------------------------- what just happened

/// One line of "what just happened".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// Local date and time, "2026-10-26 07:12:03".
    pub when: String,
    /// "info", "good", "warn" or "bad".
    pub tone: &'static str,
    pub text: String,
}

/// How many lines the page can be given.
const FEED_KEEP: usize = 300;
static FEED: Mutex<VecDeque<Event>> = Mutex::new(VecDeque::new());
/// Where every line is also kept (results\what-happened.txt), so the page
/// still has them after the program is closed and opened again.
static FEED_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

fn tone_of(word: &str) -> &'static str {
    match word {
        "good" => "good",
        "warn" => "warn",
        "bad" => "bad",
        _ => "info",
    }
}

/// Keep the lines in this file from now on, and take in its latest ones.
pub fn feed_file(path: &Path) {
    let mut events: VecDeque<Event> = VecDeque::new();
    if let Ok(t) = std::fs::read_to_string(path) {
        for line in t.lines() {
            let f: Vec<&str> = line.splitn(3, '\t').collect();
            if let [when, tone, text] = f.as_slice() {
                events.push_back(Event { when: when.to_string(), tone: tone_of(tone), text: text.to_string() });
                if events.len() > FEED_KEEP {
                    events.pop_front();
                }
            }
        }
    }
    *FEED.lock().unwrap_or_else(|e| e.into_inner()) = events;
    *FEED_FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.to_path_buf());
}

fn now_local() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as f64).unwrap_or(0.0);
    crate::fmt::local_datetime(now)
}

/// Something happened: one line, in plain words.
pub fn event(tone: &'static str, text: &str) {
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return;
    }
    let e = Event { when: now_local(), tone, text };
    if let Some(p) = FEED_FILE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = write!(f, "{}\t{}\t{}\r\n", e.when, e.tone, e.text);
        }
    }
    let mut feed = FEED.lock().unwrap_or_else(|e| e.into_inner());
    feed.push_back(e);
    while feed.len() > FEED_KEEP {
        feed.pop_front();
    }
}

/// The latest `n` lines, newest first: [{"when":..,"tone":..,"text":..}].
pub fn feed_json(n: usize) -> String {
    let feed = FEED.lock().unwrap_or_else(|e| e.into_inner());
    let items: Vec<String> = feed
        .iter()
        .rev()
        .take(n)
        .map(|e| format!("{{\"when\":{},\"tone\":{},\"text\":{}}}", json_str(&e.when), json_str(e.tone), json_str(&e.text)))
        .collect();
    format!("[{}]", items.join(","))
}

/// "412 photos, 3 videos and 2 other files".
pub fn kinds_words(c: &[Count; 6]) -> String {
    const ONE: [&str; 6] = ["photo", "video", "music or sound file", "document", "archive", "other file"];
    const MANY: [&str; 6] = ["photos", "videos", "music and sound files", "documents", "archives", "other files"];
    let mut ks: Vec<usize> = (0..6).filter(|&k| c[k].files > 0).collect();
    ks.sort_by(|a, b| c[*b].files.cmp(&c[*a].files));
    let words: Vec<String> =
        ks.iter().map(|&k| format!("{} {}", commas(c[k].files), if c[k].files == 1 { ONE[k] } else { MANY[k] })).collect();
    match words.len() {
        0 => "no files".to_string(),
        1 => words[0].clone(),
        n => format!("{} and {}", words[..n - 1].join(", "), words[n - 1]),
    }
}

// ---------------------------------------------------------------- copying, folder by folder

/// The folders copied lately, told at most every 10 seconds ("Copied
/// DCIM\2019 (412 photos, 2.1 GB)", or several at once with the biggest).
struct Copied {
    /// The folder being copied now, and what it holds so far.
    now: Option<(String, [Count; 6])>,
    /// Folders finished, not told yet: (folder, what it holds).
    done: Vec<(String, [Count; 6])>,
    told: Option<Instant>,
}

static COPIED: Mutex<Copied> = Mutex::new(Copied { now: None, done: Vec::new(), told: None });

fn folder_of(path: &str) -> String {
    path.rsplit_once(['\\', '/']).map(|(d, _)| d.to_string()).unwrap_or_default()
}

/// A file copied (its path in the copies, as the copying names it).
pub fn copied(path: &str, bytes: u64) {
    let folder = folder_of(path);
    let mut c = COPIED.lock().unwrap_or_else(|e| e.into_inner());
    let same = c.now.as_ref().is_some_and(|(d, _)| *d == folder);
    if !same {
        if let Some(f) = c.now.take() {
            c.done.push(f);
        }
        c.now = Some((folder, [Count::default(); 6]));
        let due = c.told.is_none_or(|t| t.elapsed() >= Duration::from_secs(10));
        if due && !c.done.is_empty() {
            let done = std::mem::take(&mut c.done);
            c.told = Some(Instant::now());
            drop(c);
            tell_copied(&done);
            c = COPIED.lock().unwrap_or_else(|e| e.into_inner());
        }
    }
    if let Some((_, k)) = c.now.as_mut() {
        k[kind_of(path)].add(bytes);
    }
}

/// The copying is over (or stopped): what it copied last.
pub fn copied_end() {
    let done = {
        let mut c = COPIED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(f) = c.now.take() {
            c.done.push(f);
        }
        c.told = None;
        std::mem::take(&mut c.done)
    };
    tell_copied(&done);
}

fn tell_copied(done: &[(String, [Count; 6])]) {
    let shown = |d: &str| if d.is_empty() { "the top folder".to_string() } else { d.to_string() };
    match done {
        [] => {}
        [(d, k)] => event("info", &format!("Copied {} ({}, {})", shown(d), kinds_words(k), size(Board::sum(k).bytes))),
        _ => {
            let mut all = [Count::default(); 6];
            for (_, k) in done {
                for i in 0..6 {
                    all[i].files += k[i].files;
                    all[i].bytes += k[i].bytes;
                }
            }
            let big = done.iter().max_by_key(|(_, k)| Board::sum(k).bytes).map(|(d, _)| shown(d)).unwrap_or_default();
            event(
                "info",
                &format!(
                    "Copied {} folders, the biggest {} ({}, {} in all)",
                    commas(done.len() as u64),
                    big,
                    kinds_words(&all),
                    size(Board::sum(&all).bytes)
                ),
            );
        }
    }
}

// ---------------------------------------------------------------- damage found, repairs made

/// Damage found by the step running now: the first few told one by one,
/// then the rest every minute, counted by kind.
struct Found {
    told: u64,
    waiting: [Count; 6],
    since: Option<Instant>,
}

static FOUND: Mutex<Found> = Mutex::new(Found { told: 0, waiting: [Count { files: 0, bytes: 0 }; 6], since: None });

/// A step begins looking for damage afresh.
pub fn found_begin() {
    *FOUND.lock().unwrap_or_else(|e| e.into_inner()) = Found { told: 0, waiting: [Count::default(); 6], since: None };
}

/// A damaged file found, and in a few words how.
pub fn found(path: &str, how: &str) {
    let mut f = FOUND.lock().unwrap_or_else(|e| e.into_inner());
    if f.told < 10 {
        f.told += 1;
        drop(f);
        let how = how.trim();
        event("warn", &format!("Found damage: {}{}", path, if how.is_empty() { String::new() } else { format!(" ({})", short(how, 90)) }));
        return;
    }
    f.waiting[kind_of(path)].add(0);
    let since = *f.since.get_or_insert_with(Instant::now);
    if since.elapsed() >= Duration::from_secs(60) {
        let w = std::mem::replace(&mut f.waiting, [Count::default(); 6]);
        f.since = None;
        drop(f);
        event("warn", &format!("Found {} more damaged: {}", commas(Board::sum(&w).files), kinds_words(&w)));
    }
}

/// The step is over: the damage not told yet.
pub fn found_end() {
    let w = {
        let mut f = FOUND.lock().unwrap_or_else(|e| e.into_inner());
        f.since = None;
        std::mem::replace(&mut f.waiting, [Count::default(); 6])
    };
    if Board::sum(&w).files > 0 {
        event("warn", &format!("Found {} more damaged: {}", commas(Board::sum(&w).files), kinds_words(&w)));
    }
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

/// A repair tried, for the page's photos before and after.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repair {
    /// Counted up from 1 as repairs are tried (the page's pictures of one
    /// attempt are told from another's by it).
    pub n: u64,
    pub id: String,
    /// In WORK.
    pub path: String,
    /// REPAIRED, SALVAGED or NOT REPAIRED.
    pub result: String,
    pub how: String,
    /// What was made, in REPAIR ("" if nothing).
    pub output: String,
}

/// The latest repairs tried, newest last.
static REPAIRS: Mutex<VecDeque<Repair>> = Mutex::new(VecDeque::new());
static REPAIRS_SEEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A repair tried: its number, the file in WORK, REPAIRED / SALVAGED / NOT
/// REPAIRED, how, and what was made (in REPAIR).
pub fn repaired(id: &str, path: &str, result: &str, how: &str, output: &str) {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let how = short(how.trim(), 110);
    match result {
        "REPAIRED" => event("good", &format!("Repaired {name}: {how}")),
        "SALVAGED" => event("info", &format!("Saved what could be saved of {name}: {how}")),
        _ => event("warn", &format!("Could not repair {name}: {how}")),
    }
    let n = REPAIRS_SEEN.fetch_add(1, Ordering::Relaxed) + 1;
    let mut r = REPAIRS.lock().unwrap_or_else(|e| e.into_inner());
    r.push_back(Repair { n, id: id.to_string(), path: path.to_string(), result: result.to_string(), how, output: output.to_string() });
    while r.len() > 24 {
        r.pop_front();
    }
}

/// The repairs tried lately, newest first.
pub fn repairs_json() -> String {
    let r = REPAIRS.lock().unwrap_or_else(|e| e.into_inner());
    let items: Vec<String> = r
        .iter()
        .rev()
        .map(|x| {
            format!(
                "{{\"n\":{},\"id\":{},\"path\":{},\"result\":{},\"how\":{},\"output\":{}}}",
                x.n,
                json_str(&x.id),
                json_str(&x.path),
                json_str(&x.result),
                json_str(&x.how),
                json_str(&x.output)
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// A picture the page can show (a browser shows these kinds; not HEIC, not
/// a camera's RAW).
pub fn page_shows(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    Some(match ext.as_str() {
        "jpg" | "jpeg" | "jpe" | "jfif" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        "avif" => "image/avif",
        _ => return None,
    })
}

// ---------------------------------------------------------------- the SSD, while the image is made

/// How many cells the strip of the drive has.
pub const CELLS: usize = 400;

/// The SSD as the image reads it: a strip of cells from its start to its
/// end, how fast it reads, and how warm it is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drive {
    pub size: u64,
    /// How far it has read.
    pub pos: u64,
    /// Per cell: 0 not read yet, 1 read, 2 read slowly, 3 the SSD could not
    /// read some of it (left as zeros, or stepped over).
    pub cells: Vec<u8>,
    /// MB/s, oldest first, one per `every` seconds (halved when full).
    pub speeds: Vec<f32>,
    pub every: u32,
    /// Pieces read slowly; places it could not read, and how much.
    pub slow: u64,
    pub bad_places: u64,
    pub bad_bytes: u64,
    /// °C now, the warmest seen, and where reading pauses (None: not known).
    pub temp: Option<i16>,
    pub temp_max: Option<i16>,
    pub temp_limit: Option<i16>,
    /// Why there is no temperature, or "".
    pub temp_note: String,
    /// Reading paused while the SSD cools down; how many times it was.
    pub paused: bool,
    pub pauses: u32,
    /// The image is being read back to check it: how far.
    pub check: Option<(u64, u64)>,
    /// How fast it read lately (8 MB pieces, MB/s), to tell a slow one.
    recent: VecDeque<f32>,
    /// For the speed samples: when and where the last one was taken.
    mark: Option<(Instant, u64)>,
    /// The last tenth told.
    told_tenth: u64,
    /// Pieces with places it could not read (told now and then).
    bad_pieces: u64,
}

static DRIVE: Mutex<Option<Drive>> = Mutex::new(None);

impl Drive {
    fn cell(&self, at: u64) -> usize {
        if self.size == 0 {
            return 0;
        }
        ((at as u128 * CELLS as u128 / self.size as u128) as usize).min(CELLS - 1)
    }

    fn mark_range(&mut self, at: u64, len: u64, v: u8) {
        if len == 0 || self.cells.is_empty() {
            return;
        }
        let (a, b) = (self.cell(at), self.cell(at + len - 1));
        for c in &mut self.cells[a..=b] {
            if *c < v {
                *c = v;
            }
        }
    }

    /// A piece read in `secs`, with the places in it the SSD could not read.
    /// Returns what to tell about it, if anything.
    pub fn piece(&mut self, at: u64, len: u64, secs: f64, bad: &[(u64, u64)]) -> Vec<(&'static str, String)> {
        let mut say = Vec::new();
        self.pos = self.pos.max(at + len);
        self.mark_range(at, len, 1);
        if bad.is_empty() {
            let speed = (len as f64 / (1 << 20) as f64 / secs.max(0.000_1)) as f32;
            let mut typical: Vec<f32> = self.recent.iter().copied().collect();
            typical.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let median = typical.get(typical.len() / 2).copied();
            // Slow: well under its usual speed, and long enough to matter.
            if let Some(m) = median.filter(|_| typical.len() >= 16) {
                if secs >= 0.5 && speed < m / 8.0 {
                    self.slow += 1;
                    self.mark_range(at, len, 2);
                    if self.slow == 1 || self.slow % 25 == 0 {
                        say.push((
                            "warn",
                            format!(
                                "The SSD read slowly at {} ({:.0} MB/s there, {:.0} MB/s usually){}",
                                size(at),
                                speed,
                                m,
                                if self.slow > 1 { format!(": {} slow places so far", commas(self.slow)) } else { String::new() }
                            ),
                        ));
                    }
                }
            }
            self.recent.push_back(speed);
            if self.recent.len() > 64 {
                self.recent.pop_front();
            }
        } else {
            for &(s, n) in bad {
                self.mark_range(s, n, 3);
                self.bad_places += 1;
                self.bad_bytes += n;
            }
            self.bad_pieces += 1;
            // The first few told one by one, then now and then (a big
            // damaged stretch is thousands of pieces).
            if self.bad_pieces <= 10 || self.bad_pieces % 100 == 0 {
                say.push((
                    "bad",
                    format!(
                        "The SSD could not read {} at {}: left as zeros, and listed ({} so far)",
                        size(bad.iter().map(|b| b.1).sum()),
                        size(bad[0].0),
                        size(self.bad_bytes)
                    ),
                ));
            }
        }
        let tenth = if self.size > 0 { self.pos.min(self.size) * 10 / self.size } else { 0 };
        if tenth > self.told_tenth && tenth < 10 {
            self.told_tenth = tenth;
            let now = self.speeds.last().map(|s| format!(" · {s:.0} MB/s")).unwrap_or_default();
            let bad = if self.bad_bytes > 0 { format!(" · {} could not be read so far", size(self.bad_bytes)) } else { String::new() };
            say.push(("info", format!("Read {}% of the SSD ({}){now}{bad}", tenth * 10, size(self.pos))));
        }
        say
    }

    /// Paused (or not) while it cools down.
    pub fn set_paused(&mut self, on: bool) {
        if on && !self.paused {
            self.pauses += 1;
            // (The time paused is not reading time: the speed is measured
            // afresh from when it carries on.)
            self.mark = None;
        }
        self.paused = on;
    }

    /// A speed sample, about every `every` seconds.
    pub fn sample(&mut self, now: Instant) {
        let Some((t, p)) = self.mark else {
            self.mark = Some((now, self.pos));
            return;
        };
        let secs = now.duration_since(t).as_secs_f64();
        if secs < self.every.max(1) as f64 {
            return;
        }
        self.speeds.push((self.pos.saturating_sub(p) as f64 / (1 << 20) as f64 / secs) as f32);
        self.mark = Some((now, self.pos));
        if self.speeds.len() >= 300 {
            // The whole run, at half the detail.
            self.speeds = self.speeds.chunks(2).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
            self.every *= 2;
        }
    }

    pub fn json(&self) -> String {
        let cells: String = self.cells.iter().map(|c| (b'0' + c) as char).collect();
        let speeds: Vec<String> = self.speeds.iter().map(|s| format!("{s:.1}")).collect();
        let opt = |v: Option<i16>| v.map(|t| t.to_string()).unwrap_or_else(|| "null".to_string());
        format!(
            "{{\"size\":{},\"pos\":{},\"cells\":{},\"speeds\":[{}],\"every\":{},\"slow\":{},\"bad_places\":{},\"bad_bytes\":{},\
             \"temp\":{},\"temp_max\":{},\"temp_limit\":{},\"temp_note\":{},\"paused\":{},\"pauses\":{},\"check\":{}}}",
            self.size,
            self.pos,
            json_str(&cells),
            speeds.join(","),
            self.every,
            self.slow,
            self.bad_places,
            self.bad_bytes,
            opt(self.temp),
            opt(self.temp_max),
            opt(self.temp_limit),
            json_str(&self.temp_note),
            self.paused,
            self.pauses,
            self.check.map(|(d, t)| format!("[{d},{t}]")).unwrap_or_else(|| "null".to_string())
        )
    }
}

fn with_drive(f: impl FnOnce(&mut Drive) -> Vec<(&'static str, String)>) {
    let say = {
        let mut d = DRIVE.lock().unwrap_or_else(|e| e.into_inner());
        match d.as_mut() {
            Some(d) => f(d),
            None => Vec::new(),
        }
    };
    for (tone, text) in say {
        event(tone, &text);
    }
}

/// The image begins reading the SSD (`size` bytes), from `start` (what an
/// earlier run read is shown read, with the places it could not).
pub fn drive_begin(size: u64, start: u64, bad: &[(u64, u64)]) {
    let mut d = Drive { size, pos: start, cells: vec![0; CELLS], every: 2, ..Default::default() };
    if start > 0 {
        d.mark_range(0, start, 1);
    }
    for &(s, n) in bad {
        d.mark_range(s, n, 3);
        d.bad_places += 1;
        d.bad_bytes += n;
    }
    // (The temperature, if it was being watched already, stays.)
    if let Some(old) = DRIVE.lock().unwrap_or_else(|e| e.into_inner()).take() {
        d.temp = old.temp;
        d.temp_max = old.temp_max;
        d.temp_limit = old.temp_limit;
        d.temp_note = old.temp_note;
    }
    *DRIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(d);
}

/// A piece read (see Drive::piece).
pub fn drive_piece(at: u64, len: u64, secs: f64, bad: &[(u64, u64)]) {
    with_drive(|d| {
        let say = d.piece(at, len, secs, bad);
        d.sample(Instant::now());
        say
    });
}

/// Reading the image back to check it: how far.
pub fn drive_check(done: u64, total: u64) {
    with_drive(|d| {
        d.check = Some((done, total));
        Vec::new()
    });
}

/// The SSD's temperature (None: not known, and why), and where reading pauses.
pub fn drive_temp(t: Option<i16>, limit: Option<i16>, note: &str) {
    with_drive(|d| {
        d.temp = t;
        if let Some(t) = t {
            d.temp_max = Some(d.temp_max.map_or(t, |m| m.max(t)));
        }
        d.temp_limit = limit;
        d.temp_note = note.to_string();
        Vec::new()
    });
}

/// Reading paused (or not) while the SSD cools down.
pub fn drive_paused(on: bool) {
    with_drive(|d| {
        d.set_paused(on);
        Vec::new()
    });
}

/// Is the SSD being read now (the strip begun, not all of it read yet, and
/// not being checked)?
pub fn drive_reading() -> bool {
    DRIVE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|d| d.size > 0 && d.pos < d.size && d.check.is_none())
}

/// Is reading paused now, while the SSD cools down?
pub fn drive_is_paused() -> bool {
    DRIVE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|d| d.paused)
}

/// Ready for another image (or nothing to show).
pub fn drive_clear() {
    *DRIVE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The strip and the numbers, while there are some.
pub fn drive_json() -> Option<String> {
    DRIVE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|d| d.json())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_name() {
        assert_eq!(KINDS[kind_of(r"Photos\2019\IMG_0001.JPG")], "Photos");
        assert_eq!(KINDS[kind_of("IMG_0002.HEIC")], "Photos");
        assert_eq!(KINDS[kind_of("DSC_0003.NEF")], "Photos");
        assert_eq!(KINDS[kind_of("clip.MOV")], "Videos");
        assert_eq!(KINDS[kind_of("song.flac")], "Music and sound");
        assert_eq!(KINDS[kind_of("letter.docx")], "Documents");
        assert_eq!(KINDS[kind_of("Report.PDF")], "Documents");
        assert_eq!(KINDS[kind_of("backup.7z")], "Archives");
        assert_eq!(KINDS[kind_of(".DS_Store")], "Other files");
        assert_eq!(KINDS[kind_of("README")], "Other files");
        assert_eq!(KINDS[kind_of("folder.d/thing")], "Other files");
    }

    #[test]
    fn the_board_adds_up() {
        let mut b = Board::new("copied");
        b.to_do("a.jpg", 100);
        b.to_do("b.jpg", 50);
        b.to_do("c.mp4", 1000);
        b.done("a.jpg", 100);
        // A file the listing missed counts as to do as well.
        b.done("d.pdf", 7);
        assert_eq!(Board::sum(&b.total), Count { files: 4, bytes: 1157 });
        assert_eq!(Board::sum(&b.done), Count { files: 2, bytes: 107 });
        let j = b.json();
        assert!(j.contains("\"files\":[2,4]") && j.contains("\"bytes\":[107,1157]"), "{j}");
        assert!(j.contains("{\"name\":\"Photos\",\"files\":[1,2],\"bytes\":[100,150]}"), "{j}");
        assert!(j.contains("{\"name\":\"Documents\",\"files\":[1,1],\"bytes\":[7,7]}"), "{j}");
        assert!(!j.contains("Archives"), "kinds with nothing to do are left out: {j}");
    }

    #[test]
    fn kinds_in_words() {
        let mut c = [Count::default(); 6];
        assert_eq!(kinds_words(&c), "no files");
        c[0] = Count { files: 412, bytes: 0 };
        assert_eq!(kinds_words(&c), "412 photos");
        c[1] = Count { files: 3, bytes: 0 };
        c[5] = Count { files: 1, bytes: 0 };
        assert_eq!(kinds_words(&c), "412 photos, 3 videos and 1 other file");
        c[1].files = 1000;
        assert_eq!(kinds_words(&c), "1,000 videos, 412 photos and 1 other file", "the most first");
    }

    #[test]
    fn the_drive_strip_and_its_speed() {
        let mb = 1u64 << 20;
        let mut d = Drive { size: 3000 * mb, cells: vec![0; CELLS], every: 2, ..Default::default() };
        // 40 pieces of 8 MB at 400 MB/s, then one at 2 MB/s (4 s), then one
        // the SSD could not read 4 KB of.
        let mut said = Vec::new();
        let mut at = 0;
        for _ in 0..40 {
            said.extend(d.piece(at, 8 * mb, 0.02, &[]));
            at += 8 * mb;
        }
        assert_eq!(d.slow, 0);
        said.extend(d.piece(at, 8 * mb, 4.0, &[]));
        assert_eq!(d.slow, 1, "well under its usual speed, and long enough to matter");
        assert_eq!(d.cells[d.cell(at)], 2);
        assert!(said.iter().any(|(t, s)| *t == "warn" && s.contains("read slowly") && s.contains("400 MB/s usually")), "{said:?}");
        at += 8 * mb;
        said.extend(d.piece(at, 8 * mb, 0.5, &[(at + 4096, 4096)]));
        assert_eq!((d.bad_places, d.bad_bytes, d.cells[d.cell(at)]), (1, 4096, 3));
        assert!(said.iter().any(|(t, s)| *t == "bad" && s.contains("could not read 4 KB")), "{said:?}");
        // A quick piece after a short one is not slow (it is not long enough).
        let before = d.slow;
        at += 8 * mb;
        d.piece(at, 8 * mb, 0.3, &[]);
        assert_eq!(d.slow, before);
        // Tenths are told once each.
        assert!(said.iter().any(|(_, s)| s.starts_with("Read 10% of the SSD")), "{said:?}");
        assert_eq!(said.iter().filter(|(_, s)| s.starts_with("Read 10%")).count(), 1);
        assert!(d.cells[..d.cell(d.pos - 1)].iter().all(|&c| c >= 1) && d.cells[CELLS - 1] == 0);
        // The speed samples: the whole run, at less detail as it goes on.
        let t0 = Instant::now();
        let mut e = Drive { size: 10, cells: vec![0; CELLS], every: 2, ..Default::default() };
        e.sample(t0);
        for i in 1..=700u64 {
            e.pos += 100 * mb;
            e.sample(t0 + Duration::from_secs(2 * i));
        }
        assert!(e.speeds.len() < 300 && e.every > 2, "{} samples, every {} s", e.speeds.len(), e.every);
        assert!(e.speeds.iter().all(|s| (s - 50.0).abs() < 0.5), "100 MB every 2 s: 50 MB/s");
        let j = d.json();
        assert!(j.contains("\"slow\":1") && j.contains("\"bad_places\":1") && j.contains("\"temp\":null"), "{j}");
        // A pause is not counted as reading time: the speed is measured
        // afresh when it carries on.
        e.set_paused(true);
        e.set_paused(true);
        assert_eq!((e.pauses, e.mark), (1, None));
        let n = e.speeds.len();
        e.sample(t0 + Duration::from_secs(5000));
        e.pos += 100 * mb;
        e.sample(t0 + Duration::from_secs(5000 + e.every as u64));
        e.set_paused(false);
        assert_eq!(e.speeds.len(), n + 1);
        assert!(e.speeds.last().is_some_and(|s| *s > 10.0), "not the pause's long wait: {:?}", e.speeds.last());
        assert!(e.json().contains("\"pauses\":1"));
        // A long damaged stretch: the first few places told one by one, then
        // now and then.
        let mut f = Drive { size: 100_000 * mb, cells: vec![0; CELLS], every: 2, ..Default::default() };
        let told: usize = (0..250u64).map(|i| f.piece(i * 8 * mb, 8 * mb, 1.0, &[(i * 8 * mb, 4096)]).iter().filter(|(t, _)| *t == "bad").count()).sum();
        assert_eq!((f.bad_places, told), (250, 12), "10, then the 100th and the 200th");
    }

    #[test]
    fn what_happened_is_kept() {
        let dir = std::env::temp_dir().join(format!("arp-feed-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("what-happened.txt");
        std::fs::write(&p, "2026-10-26 07:00:00\tgood\tAn earlier line\r\nnot a line\r\n").unwrap();
        feed_file(&p);
        event("info", "Copied   DCIM\\2019 (412 photos)\nand more");
        let j = feed_json(300);
        assert!(j.contains("\"text\":\"Copied DCIM\\\\2019 (412 photos) and more\""), "one line, its spaces tidied: {j}");
        assert!(j.contains("An earlier line"), "{j}");
        let t = std::fs::read_to_string(&p).unwrap();
        assert!(t.contains("\tinfo\tCopied DCIM\\2019 (412 photos) and more\r\n"), "{t}");
        // (Stop keeping the file: other tests make lines too.)
        *FEED_FILE.lock().unwrap() = None;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_is_kept_unless_the_page_watches() {
        // (The tests run side by side, and many go through steps that count
        // here: so this one never turns watching on. The page's own tests
        // see the numbers a real job shows.)
        begin("copied");
        to_do("a.jpg", 1);
        now("a.jpg");
        done("a.jpg", 1);
        assert_eq!(snapshot(), None);
    }
}
