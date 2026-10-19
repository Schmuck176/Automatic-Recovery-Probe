//! Rescue: copy files straight off the recovered disk, past Windows.
//!
//! Files whose folder record comes after a blank (never-read) record are
//! invisible to Explorer and TeraCopy, so a normal copy leaves them behind
//! without a word. This copies them with the program's own exFAT reader.
//! It only ever reads the recovered disk, and writes only into the chosen
//! folder, which must be on a different drive.

use crate::disk::{open_source, Source};
use crate::exfat::{timestamp_to_unix, ClusterList, Entry, Volume};
use crate::fmt::{commas, csv_row, size};
use crate::gpt;
use crate::sha256::Sha256;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    /// Only what Windows cannot list.
    Hidden,
    /// Every file and folder.
    Everything,
    /// These paths (from the top of the volume, folders include everything in them).
    Paths(Vec<String>),
}

pub struct Options {
    pub disk: String,
    pub out_dir: String,
    pub pick: Pick,
    /// Also save the data no file claims (files whose folder record was
    /// lost), one file per unbroken piece.
    pub lost: bool,
    /// Set from elsewhere to stop between files (what is copied so far stays).
    pub stop: Option<Arc<AtomicBool>>,
    /// Stop after copying this many files (for the tests).
    pub stop_after_files: Option<u64>,
}

#[derive(Default, Debug)]
pub struct Outcome {
    pub files: u64,
    pub folders: u64,
    pub bytes: u64,
    pub already_there: u64,
    pub partial: Vec<(String, String)>,
    pub failed: Vec<(String, String)>,
    pub lost_pieces: u64,
    pub lost_bytes: u64,
    /// Copies whose name had to change for Windows (see `safe_name`).
    pub renamed: u64,
    /// Copies an earlier run left that did not hold what the disk holds,
    /// copied again.
    pub recopied: u64,
    /// Stopped before the end (on request, or for one of the reasons below).
    pub stopped: bool,
    /// The recovered disk stopped answering (what Windows said).
    pub disk_lost: Option<String>,
    /// The folder the copies go to went away (its drive was unplugged?).
    pub dest_lost: Option<String>,
    /// Copying failed for many files in a row (the last reason).
    pub keeps_failing: Option<String>,
    pub log: String,
}

/// Device names Windows keeps for itself in every folder.
fn reserved(stem: &str) -> bool {
    let s = stem.to_uppercase();
    let c: Vec<char> = s.chars().collect();
    matches!(s.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || (c.len() == 4 && (s.starts_with("COM") || s.starts_with("LPT")) && "0123456789¹²³".contains(c[3]))
}

/// A name Windows programs can use for a copy. exFAT allows a few names
/// Windows does not: names ending in a space or a dot (Windows quietly drops
/// that last character, so most programs could never open the copy), the old
/// device names (CON, NUL, COM1 ...), and - only in a damaged record -
/// characters such as ':' or '?'. Each of those gets an underscore, the way
/// 7-Zip does it; the rescue log lists every copy whose name changed.
pub fn safe_name(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| match c {
            '\0'..='\u{1f}' | '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c => c,
        })
        .collect();
    if s.ends_with(' ') || s.ends_with('.') {
        s.pop();
        s.push('_');
    }
    if s.is_empty() {
        s.push('_');
    }
    if reserved(s.split('.').next().unwrap_or("").trim_end_matches(' ')) {
        s.insert(0, '_');
    }
    s
}

/// `name`, or "name (2).ext", "name (3).ext"... if a copy in the same folder
/// already has that name (Windows does not tell capitals apart).
fn unique(used: &mut HashSet<String>, name: String, is_dir: bool) -> String {
    if used.insert(name.to_lowercase()) {
        return name;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if !is_dir && i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    (2..)
        .map(|k| format!("{stem} ({k}){ext}"))
        .find(|c| used.insert(c.to_lowercase()))
        .expect("a free name")
}

/// Where a file is copied to first; renamed to its real name once complete,
/// so a stopped or failed copy never looks like a finished one.
const PARTIAL: &str = "~recovery-probe-copying.tmp";

/// An absolute path Windows takes literally (no length limit). Only the
/// chosen folder goes through this; names added to it are used as they are.
pub(crate) fn verbatim(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    #[cfg(windows)]
    {
        let s = abs.to_string_lossy().replace('/', "\\");
        if s.starts_with(r"\\?\") {
            return PathBuf::from(s);
        }
        if let Some(unc) = s.strip_prefix(r"\\") {
            return PathBuf::from(format!(r"\\?\UNC\{unc}"));
        }
        return PathBuf::from(format!(r"\\?\{s}"));
    }
    #[cfg(not(windows))]
    abs
}

fn dest_path(out: &Path, path: &str) -> PathBuf {
    let mut p = out.to_path_buf();
    for part in path.split('\\') {
        p.push(safe_name(part));
    }
    p
}

/// "a\\b" for a copy's place inside the chosen folder.
fn shown_inside(base: &Path, p: &Path) -> String {
    match p.strip_prefix(base) {
        Ok(rel) => rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("\\"),
        Err(_) => p.display().to_string(),
    }
}

/// The name a piece of lost data gets (the map check names it the same way,
/// so that the check of the copies can find it).
pub fn lost_piece_name(first: u32, count: u32, head: &[u8]) -> String {
    format!("cluster {first} ({count} clusters).{}", extension_for(head))
}

/// The folder lost data goes into, inside the copies' folder.
pub const LOST_DIR: &str = "lost data (no name)";

/// A file extension for data whose name is lost, from what it starts with.
pub fn extension_for(head: &[u8]) -> &'static str {
    match crate::formats::sniff(head) {
        "jpeg" => "jpg",
        "png" => "png",
        "gif" => "gif",
        "bmp" => "bmp",
        "tiff" => "tif",
        "crw" => "crw",
        "mrw" => "mrw",
        "pdf" => "pdf",
        "zip" => "zip",
        "rar" => "rar",
        "7z" => "7z",
        "gzip" => "gz",
        "flac" => "flac",
        "ogg" => "ogg",
        "mp3" => "mp3",
        "riff" => match head.get(8..12) {
            Some(b"WAVE") => "wav",
            Some(b"AVI ") => "avi",
            Some(b"WEBP") => "webp",
            _ => "riff",
        },
        "matroska" => "mkv",
        "isobmff" => match head.get(8..12) {
            Some(b"qt  ") => "mov",
            Some(b"heic") | Some(b"heix") | Some(b"mif1") => "heic",
            Some(b"crx ") => "cr3",
            Some(b"M4A ") => "m4a",
            _ => "mp4",
        },
        "sqlite" => "sqlite",
        "raf" => "raf",
        "x3f" => "x3f",
        _ => "bin",
    }
}

/// Why copying one file did not work: the recovered disk did not give the
/// data, or the copy could not be written.
enum CopyError {
    Read(String),
    Write(String),
}

impl CopyError {
    fn text(&self) -> &str {
        match self {
            CopyError::Read(s) | CopyError::Write(s) => s,
        }
    }
}

/// Copy one file's data from the disk into `dest` (by way of a temporary
/// name beside it). Returns a note when something could not be copied (the
/// missing parts are left as zeros) and the copy's SHA-256, or None for
/// "stopped".
fn copy_file(
    vol: &Volume,
    e: &Entry,
    list: &ClusterList,
    dest: &Path,
    stop: &dyn Fn() -> bool,
    copied: &mut dyn FnMut(u64),
) -> Result<Option<(Option<String>, [u8; 32])>, CopyError> {
    let tmp = dest.with_file_name(PARTIAL);
    let mut f = File::create(&tmp).map_err(|err| CopyError::Write(format!("could not create the copy: {err}")))?;
    let note = copy_data(vol, e, list, &mut f, stop, copied);
    let note = match note {
        Ok(Some(n)) => n,
        Ok(None) => {
            drop(f);
            let _ = std::fs::remove_file(&tmp);
            return Ok(None);
        }
        Err(err) => {
            drop(f);
            let _ = std::fs::remove_file(&tmp);
            return Err(err);
        }
    };
    drop(f);
    std::fs::rename(&tmp, dest).map_err(|err| {
        let _ = std::fs::remove_file(&tmp);
        CopyError::Write(format!("could not give the copy its name: {err}"))
    })?;
    Ok(Some(note))
}

fn copy_data(
    vol: &Volume,
    e: &Entry,
    list: &ClusterList,
    f: &mut File,
    stop: &dyn Fn() -> bool,
    copied: &mut dyn FnMut(u64),
) -> Result<Option<(Option<String>, [u8; 32])>, CopyError> {
    let want = e.data_length;
    let vdl = e.valid_data_length.min(want);
    let mut written = 0u64;
    let mut note = list.problem.clone();
    // The SHA-256 of exactly what the copy holds, from the bytes as they
    // were read off the disk.
    let mut sha = Sha256::new();
    'runs: for &(first, count) in &list.runs {
        let run_bytes = count as u64 * vol.cluster_size;
        let mut done = 0u64;
        while done < run_bytes && written < vdl {
            let n = (run_bytes - done).min(8 << 20).min(vdl - written);
            if stop() {
                return Ok(None);
            }
            let data = vol
                .src
                .read_at(vol.cluster_offset(first) + done, n as usize)
                .map_err(|err| CopyError::Read(format!("could not read the recovered disk: {err}")))?;
            f.write_all(&data).map_err(|err| CopyError::Write(format!("could not write the copy: {err}")))?;
            sha.update(&data);
            written += data.len() as u64;
            copied(data.len() as u64);
            if (data.len() as u64) < n {
                note.get_or_insert_with(|| "the disk ended before the file did".to_string());
                break 'runs;
            }
            done += n;
        }
    }
    if written < vdl && note.is_none() {
        note = Some("part of the file's data could not be found".to_string());
    }
    // Past the valid data a file reads as zeros by definition; anything that
    // could not be found is left as zeros too (and noted).
    if written < want {
        f.set_len(want).map_err(|err| CopyError::Write(format!("could not write the copy: {err}")))?;
        sha.update_zeros(want - written);
    }
    let _ = f.set_times(times_of(e));
    Ok(Some((note, sha.finish())))
}

/// A copy's dates, from its folder record: modified, last opened and (on
/// Windows) created. A date the record does not hold properly is left as
/// it is.
fn times_of(e: &Entry) -> std::fs::FileTimes {
    let at = |ts: u32, ms10: u8, utc: u8| {
        timestamp_to_unix(ts, ms10, utc).map(|(secs, nanos)| {
            if secs >= 0 {
                std::time::UNIX_EPOCH + std::time::Duration::new(secs as u64, nanos)
            } else {
                std::time::UNIX_EPOCH
            }
        })
    };
    let mut t = std::fs::FileTimes::new();
    if let Some(m) = at(e.modified, e.modified_10ms, e.modified_utc) {
        t = t.set_modified(m);
    }
    if let Some(a) = at(e.accessed, 0, e.accessed_utc) {
        t = t.set_accessed(a);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTimesExt;
        if let Some(c) = at(e.created, e.created_10ms, e.created_utc) {
            t = t.set_created(c);
        }
    }
    #[cfg(not(windows))]
    let _ = (e.created, e.created_10ms, e.created_utc);
    t
}

/// Give a copied folder its dates (done at the end of a run: adding files
/// to a folder changes its modified date).
fn set_folder_times(p: &Path, t: std::fs::FileTimes) {
    #[cfg(windows)]
    let f = {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_WRITE_ATTRIBUTES, with FILE_FLAG_BACKUP_SEMANTICS to open a folder.
        std::fs::OpenOptions::new().access_mode(0x0000_0100).custom_flags(0x0200_0000).open(p)
    };
    #[cfg(not(windows))]
    let f = File::open(p);
    if let Ok(f) = f {
        let _ = f.set_times(t);
    }
}

/// The bytes [pos, pos + n) of a file's copy should hold: what copy_data
/// writes there (the file's clusters up to its valid data, zeros after).
fn expected_bytes(vol: &Volume, e: &Entry, list: &ClusterList, pos: u64, n: usize) -> std::io::Result<Vec<u8>> {
    let vdl = e.valid_data_length.min(e.data_length);
    let end = pos + n as u64;
    let mut out = vec![0u8; n];
    let mut file_off = 0u64;
    for &(first, count) in &list.runs {
        let run_len = count as u64 * vol.cluster_size;
        let s = pos.max(file_off);
        let t = end.min(file_off + run_len).min(vdl);
        if s < t {
            let data = vol.src.read_at(vol.cluster_offset(first) + (s - file_off), (t - s) as usize)?;
            let at = (s - pos) as usize;
            out[at..at + data.len()].copy_from_slice(&data);
            if (data.len() as u64) < t - s {
                break; // the disk ended: the copy has zeros from there on
            }
        }
        file_off += run_len;
        if file_off >= end.min(vdl) {
            break;
        }
    }
    Ok(out)
}

/// Does a copy an earlier run left hold what the disk holds, at its start
/// and its end? (A PC that lost power or crashed can leave copies of the
/// right size whose last-written data never reached the drive: zeros.)
fn copy_matches(vol: &Volume, e: &Entry, list: &ClusterList, dest: &Path) -> Result<bool, CopyError> {
    use std::io::{Read, Seek, SeekFrom};
    let size = e.data_length;
    if size == 0 {
        return Ok(true);
    }
    let mut f = File::open(dest).map_err(|err| CopyError::Write(err.to_string()))?;
    let mut spots = vec![0u64];
    if size > 4096 {
        spots.push(size - 4096);
    }
    for pos in spots {
        let n = (size - pos).min(4096) as usize;
        let want = expected_bytes(vol, e, list, pos, n).map_err(|err| CopyError::Read(err.to_string()))?;
        let mut got = vec![0u8; n];
        f.seek(SeekFrom::Start(pos)).and_then(|_| f.read_exact(&mut got)).map_err(|err| CopyError::Write(err.to_string()))?;
        if got != want {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The same for a piece of lost data (clusters from `off`, `len` bytes).
fn piece_matches(vol: &Volume, off: u64, len: u64, dest: &Path) -> std::io::Result<bool> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = File::open(dest)?;
    let mut spots = vec![0u64];
    if len > 4096 {
        spots.push(len - 4096);
    }
    for pos in spots {
        let n = (len - pos).min(4096) as usize;
        let mut want = vol.src.read_at(off + pos, n)?;
        want.resize(n, 0);
        let mut got = vec![0u8; n];
        f.seek(SeekFrom::Start(pos))?;
        f.read_exact(&mut got)?;
        if got != want {
            return Ok(false);
        }
    }
    Ok(true)
}

fn same_volume(out: &Path, vol_serial: u32) -> Option<String> {
    let drive = crate::sys::drive_for_serial(vol_serial)?;
    let abs = std::path::absolute(out).ok()?.to_string_lossy().to_uppercase();
    let d = drive.to_uppercase();
    (abs.starts_with(&d) || abs.starts_with(&format!(r"\\?\{d}"))).then_some(drive)
}

/// The list of every file copied, in the folder the copies go to.
pub const LOG_NAME: &str = "rescue-log.csv";
/// The list as it grows during a run; folded into rescue-log.csv at the end
/// (and by the next run, if this one never got to the end).
pub const UNFINISHED_LOG_NAME: &str = "rescue-log-unfinished.csv";
const LOG_HEADER: [&str; 7] = ["kind", "path", "size_bytes", "result", "copied_as", "note", "sha256"];

/// The lists earlier runs into the same folder wrote, read back: one line
/// per file, folder and piece of lost data (the latest wins, except that a
/// file a later run left as it was keeps the line saying how it was
/// copied), and the notes about folders.
#[derive(Default)]
struct Earlier {
    lines: Vec<String>,
    /// (kind, path, copied as) -> index in `lines`.
    index: HashMap<String, usize>,
    notes: Vec<String>,
    seen_notes: HashSet<String>,
}

fn log_key(kind: &str, path: &str, copied_as: &str) -> String {
    format!("{kind}\t{path}\t{copied_as}")
}

impl Earlier {
    /// Each list with whether to keep its notes.
    fn read(lists: &[(&Path, bool)]) -> Earlier {
        let mut e = Earlier::default();
        for (p, notes) in lists {
            let Ok(f) = File::open(p) else { continue };
            let _ = crate::fmt::for_each_csv_row(std::io::BufReader::new(f), |row| e.add(&row, *notes));
        }
        e
    }
    fn add(&mut self, row: &[String], notes: bool) {
        if row.len() < 6 || row[0] == "kind" {
            return;
        }
        // Seven fields (lists from before the sha256 column have six).
        let fields: Vec<&str> = (0..7).map(|i| row.get(i).map(|s| s.as_str()).unwrap_or("")).collect();
        let line = csv_row(&fields);
        if row[0] == "NOTE" {
            if notes && self.seen_notes.insert(line.clone()) {
                self.notes.push(line);
            }
            return;
        }
        let key = log_key(&row[0], &row[1], &row[4]);
        match self.index.get(&key) {
            // Left as it was: the earlier line says how it was copied.
            Some(_) if row[3] == "skipped" => {}
            Some(&i) => self.lines[i] = line,
            None => {
                self.index.insert(key, self.lines.len());
                self.lines.push(line);
            }
        }
    }
    fn text(&self, drop_stopped_notes: bool) -> String {
        let mut out = csv_row(&LOG_HEADER);
        for l in &self.lines {
            out.push_str(l);
        }
        for n in &self.notes {
            if !(drop_stopped_notes && n.contains("run it again")) {
                out.push_str(n);
            }
        }
        out
    }
}

/// The files an earlier list says were copied only in part: key -> note.
struct Partial {
    notes: HashMap<String, String>,
    /// The earlier list has anything in it.
    any: bool,
}

impl Partial {
    fn read(path: &Path) -> Partial {
        let mut p = Partial { notes: HashMap::new(), any: false };
        if let Ok(f) = File::open(path) {
            let _ = crate::fmt::for_each_csv_row(std::io::BufReader::new(f), |row| {
                if row.len() < 6 || row[0] == "kind" {
                    return;
                }
                p.any = true;
                if row[3] == "partly copied" {
                    p.notes.insert(log_key(&row[0], &row[1], &row[4]), row[5].clone());
                }
            });
        }
        p
    }
}

/// The SHA-256 each earlier list gives each copy, so that a copy an
/// earlier run left can be checked whole before it is kept. (Keyed by a
/// 64-bit hash of the list's key, to keep a million of them small.)
struct Hashes {
    map: HashMap<u64, [u8; 32]>,
}

fn key_hash(key: &str) -> u64 {
    // FNV-1a.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    let b = s.trim().as_bytes();
    if b.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        let pair = std::str::from_utf8(&b[i * 2..i * 2 + 2]).ok()?;
        out[i] = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

impl Hashes {
    fn read(path: &Path) -> Hashes {
        let mut h = Hashes { map: HashMap::new() };
        if let Ok(f) = File::open(path) {
            let _ = crate::fmt::for_each_csv_row(std::io::BufReader::new(f), |row| {
                if row.len() < 7 || row[0] == "kind" || row[0] == "NOTE" {
                    return;
                }
                if let Some(d) = parse_hex32(&row[6]) {
                    h.map.insert(key_hash(&log_key(&row[0], &row[1], &row[4])), d);
                }
            });
        }
        h
    }
    fn get(&self, key: &str) -> Option<&[u8; 32]> {
        self.map.get(&key_hash(key))
    }
}

/// The list of this run, written down as it goes (so that a run that never
/// gets to the end - the PC lost power, say - still leaves it behind), and
/// folded into rescue-log.csv at the end.
struct RunLog {
    file: Option<std::io::BufWriter<File>>,
    last_flush: std::time::Instant,
}

impl RunLog {
    fn new(path: &Path) -> std::io::Result<RunLog> {
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        Ok(RunLog { file: Some(std::io::BufWriter::new(file)), last_flush: std::time::Instant::now() })
    }
    fn add(&mut self, fields: &[&str]) {
        if let Some(f) = &mut self.file {
            // Every line has the seven fields of LOG_HEADER.
            let mut all: Vec<&str> = fields.to_vec();
            all.resize(LOG_HEADER.len().max(all.len()), "");
            let _ = f.write_all(csv_row(&all).as_bytes());
            if self.last_flush.elapsed().as_secs() >= 2 {
                let _ = f.flush();
                self.last_flush = std::time::Instant::now();
            }
        }
    }
    fn note(&mut self, path: &str, what: &str) {
        self.add(&["NOTE", path, "", "", "", what]);
    }
    fn finish(mut self) -> std::io::Result<()> {
        match self.file.take() {
            Some(mut f) => f.flush(),
            None => Err(std::io::Error::other("the list could not be written as the copying went")),
        }
    }
}

/// `progress` gets a line of text now and then; `amount` gets (bytes copied,
/// bytes to copy) as the copying goes.
pub fn run(opts: &Options, progress: &mut dyn FnMut(&str), amount: &mut dyn FnMut(u64, u64)) -> Result<Outcome, String> {
    let src = open_source(&opts.disk).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!("Windows would not let the program read {} - it needs to run as administrator", opts.disk)
        } else {
            format!("could not open {}: {e}", opts.disk)
        }
    })?;
    rescue(&*src, opts, progress, amount)
}

/// "1 piece", "2 pieces".
pub fn pieces(n: u64) -> String {
    if n == 1 {
        "1 piece".to_string()
    } else {
        format!("{} pieces", commas(n))
    }
}

/// What to tell someone whose copies could no longer be written.
pub fn dest_lost_text(err: &str) -> String {
    format!(
        "The folder the copies go to went away (Windows said: {err}), so the copying stopped - was its drive \
         unplugged? Nothing is lost: what was copied is kept. Plug the drive back in and check it shows in File \
         Explorer, then start the copying again into the same folder: it carries on where it stopped."
    )
}

/// What to tell someone when copying failed for many files in a row.
pub fn keeps_failing_text(err: &str) -> String {
    format!(
        "Copying failed for many files in a row, so it stopped (the last reason: {err}). What was copied is kept. \
         Take a screenshot of this and ask for help before starting it again."
    )
}

/// What to tell someone whose recovered disk stopped answering.
pub fn disk_lost_text(err: &str) -> String {
    format!(
        "The recovered SSD stopped answering (Windows said: {err}), so the copying stopped. Nothing is lost: what \
         was copied is kept. Check its cable is in firmly. If it came out, plug it back in: it arrives switched off, \
         as Windows is still set to leave new disks switched off. Then choose it again in step 3, click Protect, and \
         start the copying again: it carries on where it stopped."
    )
}

pub fn rescue(
    src: &dyn Source,
    opts: &Options,
    progress: &mut dyn FnMut(&str),
    amount: &mut dyn FnMut(u64, u64),
) -> Result<Outcome, String> {
    let watched = crate::disk::Watched::new(src);
    let src: &dyn Source = &watched;
    let table = gpt::read(src, &[]);
    let (vol, _) = crate::mapcheck::find_volume(src, &table)?;
    // Read again whenever a read fails, to tell a disk that stopped
    // answering from one bad spot on it.
    let probe_at = vol.offset;
    let gone = || watched.errors() > 0 && !watched.answers(probe_at);
    let out = PathBuf::from(&opts.out_dir);
    if let Some(d) = same_volume(&out, vol.serial) {
        return Err(format!(
            "{} is on the recovered disk itself ({d}). Choose a folder on another drive - nothing is ever written to the recovered disk.",
            opts.out_dir
        ));
    }
    let base = verbatim(&out);
    std::fs::create_dir_all(&base).map_err(|e| format!("could not create {}: {e}", out.display()))?;
    progress(&format!("Copying from the exFAT volume \"{}\" into {}", vol.label, out.display()));
    let stop_flag = opts.stop.clone();
    let halt = AtomicBool::new(false);
    let stop = || halt.load(Ordering::Relaxed) || stop_flag.as_ref().is_some_and(|s| s.load(Ordering::Relaxed));

    let wanted: Vec<String> = match &opts.pick {
        Pick::Paths(p) => p.iter().map(|s| s.replace('/', "\\").trim_matches('\\').to_lowercase()).collect(),
        _ => Vec::new(),
    };
    let picked = |e: &Entry| -> bool {
        match &opts.pick {
            Pick::Hidden => e.hidden_from_windows,
            Pick::Everything => true,
            Pick::Paths(_) => {
                let p = e.path.to_lowercase();
                wanted.iter().any(|w| p == *w || p.starts_with(&format!("{w}\\")))
            }
        }
    };

    // First the totals, so the progress can say how far along it is, and
    // whether the copies fit. (Copies an earlier run made need no room.)
    let resuming = std::fs::read_dir(&base).map(|mut r| r.next().is_some()).unwrap_or(false);
    let (mut total_files, mut total_bytes, mut need_bytes, mut need_files) = (0u64, 0u64, 0u64, 0u64);
    crate::exfat::walk_until(
        &vol,
        &mut |e: &Entry, _: &ClusterList| {
            if picked(e) && !e.is_dir {
                total_files += 1;
                total_bytes += e.valid_data_length.min(e.data_length);
                let there = resuming
                    && std::fs::metadata(dest_path(&base, &e.path)).is_ok_and(|m| m.is_file() && m.len() == e.data_length);
                if !there {
                    need_files += 1;
                    need_bytes += e.data_length;
                }
            }
        },
        &|| watched.errors() > 0 && gone(),
    );
    if gone() {
        return Err(disk_lost_text(&watched.last_error()));
    }
    progress(&format!("{} files to copy ({})", commas(total_files), size(total_bytes)));
    if let Some(free) = crate::win::free_space(&base) {
        // Each copy takes up at least a whole 4 KB cluster on the other drive.
        let need = need_bytes + need_files * 4096;
        if need > free {
            return Err(format!(
                "the copies need about {} but the drive {} is on has only {} free. Choose a folder on a drive with \
                 more room.",
                size(need),
                out.display(),
                size(free)
            ));
        }
    }
    let mut done_bytes = 0u64;
    amount(0, total_bytes);

    // The lists earlier runs into this folder left (a run that never got to
    // the end leaves its unfinished list).
    let log_path = base.join(LOG_NAME);
    let unfinished_path = base.join(UNFINISHED_LOG_NAME);
    if std::fs::metadata(&unfinished_path).is_ok() {
        // Folded into the list, which this run's goes into at the end.
        let earlier = Earlier::read(&[(&log_path, true), (&unfinished_path, true)]);
        std::fs::write(&log_path, format!("\u{FEFF}{}", earlier.text(false)))
            .map_err(|e| format!("could not write {}: {e}", out.join(LOG_NAME).display()))?;
        let _ = std::fs::remove_file(&unfinished_path);
    }
    // Of the earlier list, only the files copied in part are needed while
    // copying (a file left as it was is still one of them), and each copy's
    // SHA-256 (a copy is only left as it was if all of it still holds what
    // was copied).
    let earlier = Partial::read(&log_path);
    let hashes = Hashes::read(&log_path);
    if earlier.any {
        progress("Checking the copies an earlier run made: each is read back whole before it is kept.");
    }
    let mut runlog =
        RunLog::new(&unfinished_path).map_err(|e| format!("could not write {}: {e}", out.join(UNFINISHED_LOG_NAME).display()))?;

    let mut o = Outcome::default();
    let mut owned: Vec<u64> = vec![0; (vol.cluster_count as usize).div_ceil(64)];
    let mut mark = |list: &ClusterList| {
        for &(first, count) in &list.runs {
            for c in first..first.saturating_add(count) {
                let i = c as usize - 2;
                if i / 64 < owned.len() {
                    owned[i / 64] |= 1 << (i % 64);
                }
            }
        }
    };
    mark(&vol.bitmap_clusters);
    mark(&vol.upcase_clusters);
    mark(&vol.root_clusters);
    let mut last = std::time::Instant::now();
    let mut made_dirs: HashSet<PathBuf> = HashSet::new();
    // Each copied folder and the dates it gets at the end.
    let mut folder_times: Vec<(PathBuf, std::fs::FileTimes)> = Vec::new();
    // Where each folder of the volume is copied to (names made safe for
    // Windows, and kept apart where two would end up the same).
    let mut dirs: HashMap<String, PathBuf> = HashMap::new();
    dirs.insert(String::new(), base.clone());
    let mut cur_parent: Option<String> = None;
    let mut used: HashSet<String> = HashSet::new();
    let mut errors_seen = 0u64;
    let mut failures_in_a_row = 0u32;
    let mut copied_this_run = 0u64;
    let mut visit = |e: &Entry, list: &ClusterList| {
        mark(list);
        if o.stopped {
            return;
        }
        if stop() {
            o.stopped = true;
            return;
        }
        if watched.errors() > errors_seen {
            errors_seen = watched.errors();
            if !watched.answers(probe_at) {
                o.disk_lost = Some(watched.last_error());
                o.stopped = true;
                halt.store(true, Ordering::Relaxed);
                return;
            }
        }
        let (parent, name) = e.path.rsplit_once('\\').unwrap_or(("", e.path.as_str()));
        if cur_parent.as_deref() != Some(parent) {
            // A new folder's records (walk() gives each folder's in one go).
            cur_parent = Some(parent.to_string());
            used.clear();
        }
        let parent_dest = dirs.get(parent).cloned().unwrap_or_else(|| dest_path(&base, parent));
        let mut dest = parent_dest.join(unique(&mut used, safe_name(name), e.is_dir));
        if e.is_dir {
            dirs.insert(e.path.clone(), dest.clone());
        }
        if !picked(e) {
            return;
        }
        let kind = if e.is_dir { "FOLDER" } else { "FILE" };
        let size_text = if e.is_dir { String::new() } else { e.data_length.to_string() };
        let mut renamed_note = String::new();
        // Something else already has this name there: keep it, and give the
        // copy a name of its own.
        let clash = |p: &Path| match std::fs::symlink_metadata(p) {
            Ok(m) if e.is_dir => !m.is_dir(),
            Ok(m) => !(m.is_file() && m.len() == e.data_length),
            Err(_) => false,
        };
        if clash(&dest) {
            let stem = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let mut taken: HashSet<String> = HashSet::new();
            taken.insert(stem.to_lowercase());
            loop {
                let cand = parent_dest.join(unique(&mut taken, stem.clone(), e.is_dir));
                if !clash(&cand) {
                    dest = cand;
                    break;
                }
            }
            renamed_note = "something else of that name was already in the folder (left as it was). ".to_string();
            if e.is_dir {
                dirs.insert(e.path.clone(), dest.clone());
            }
        }
        let copied_as = shown_inside(&base, &dest);
        let copied_as = if copied_as == e.path { String::new() } else { copied_as };
        if dest.file_name().is_some_and(|n| n.to_string_lossy() != name) {
            o.renamed += 1;
        }
        if e.is_dir {
            match std::fs::create_dir_all(&dest) {
                Ok(_) => {
                    o.folders += 1;
                    folder_times.push((dest.clone(), times_of(e)));
                    made_dirs.insert(dest);
                    if !copied_as.is_empty() {
                        runlog.add(&[kind, &e.path, "", "copied", &copied_as, renamed_note.trim()]);
                    }
                }
                Err(err) => {
                    if std::fs::metadata(&base).is_err() {
                        o.dest_lost = Some(err.to_string());
                        o.stopped = true;
                        halt.store(true, Ordering::Relaxed);
                        return;
                    }
                    o.failed.push((e.path.clone(), err.to_string()));
                    runlog.add(&[kind, &e.path, "", "failed", &copied_as, &err.to_string()]);
                }
            }
            return;
        }
        if let Some(parent) = dest.parent() {
            if !made_dirs.contains(parent) {
                let _ = std::fs::create_dir_all(parent);
                // A copy left half-done by an earlier, stopped run.
                let _ = std::fs::remove_file(parent.join(PARTIAL));
                made_dirs.insert(parent.to_path_buf());
            }
        }
        let mut again = false;
        if std::fs::symlink_metadata(&dest).is_ok() {
            // The same size: a copy made by an earlier run - if its start and
            // end hold what the disk holds. Its line in the earlier list says
            // how it was copied.
            let key = log_key(kind, &e.path, &copied_as);
            let whole = |want: &[u8; 32]| {
                crate::sha256::file_hex(&dest, &stop).ok().flatten().is_some_and(|got| got == crate::sha256::hex(want))
            };
            match copy_matches(&vol, e, list, &dest) {
                // Its start and end hold what the disk holds, and all of it
                // what was copied before (its SHA-256 in the earlier list).
                Ok(true) if hashes.get(&key).is_some_and(whole) => {
                    o.already_there += 1;
                    done_bytes += e.valid_data_length.min(e.data_length);
                    if let Some(note) = earlier.notes.get(&key) {
                        o.partial.push((e.path.clone(), note.clone()));
                    }
                    let sha = hashes.get(&key).map(|d| crate::sha256::hex(d)).unwrap_or_default();
                    runlog.add(&[kind, &e.path, &size_text, "skipped", &copied_as, "already in the folder (left as it was)", &sha]);
                    return;
                }
                _ if stop() => {
                    o.stopped = true;
                    return;
                }
                Err(CopyError::Read(_)) if !watched.answers(probe_at) => {
                    o.disk_lost = Some(watched.last_error());
                    o.stopped = true;
                    halt.store(true, Ordering::Relaxed);
                    return;
                }
                _ => again = true,
            }
        }
        let mut copied = |n: u64| {
            done_bytes += n;
            if last.elapsed().as_millis() > 250 {
                amount(done_bytes, total_bytes);
                last = std::time::Instant::now();
            }
        };
        match copy_file(&vol, e, list, &dest, &stop, &mut copied) {
            Ok(None) => o.stopped = true,
            Ok(Some((note, sha))) => {
                let sha = crate::sha256::hex(&sha);
                failures_in_a_row = 0;
                copied_this_run += 1;
                o.files += 1;
                o.bytes += e.data_length;
                let (result, n) = match &note {
                    Some(n) => {
                        o.partial.push((e.path.clone(), n.clone()));
                        ("partly copied", n.clone())
                    }
                    None => ("copied", String::new()),
                };
                let mut n = format!("{renamed_note}{n}");
                if again {
                    o.recopied += 1;
                    n = format!(
                        "copied again: the copy an earlier run left did not hold what the disk holds (the PC may have \
                         lost power). {n}"
                    );
                }
                if e.hidden_from_windows && note.is_none() {
                    n.push_str(&format!("Windows cannot list it: {}", e.why_hidden));
                }
                runlog.add(&[kind, &e.path, &size_text, result, &copied_as, n.trim(), &sha]);
                if opts.stop_after_files.is_some_and(|k| copied_this_run >= k) {
                    o.stopped = true;
                    halt.store(true, Ordering::Relaxed);
                }
            }
            Err(err) => {
                // The disk or the drive the copies go to may have gone away:
                // then stop, rather than list every file after this one as
                // failed.
                if let CopyError::Read(_) = err {
                    errors_seen = watched.errors();
                    if !watched.answers(probe_at) {
                        o.disk_lost = Some(watched.last_error());
                        o.stopped = true;
                        halt.store(true, Ordering::Relaxed);
                        return;
                    }
                } else if std::fs::metadata(&base).is_err() {
                    o.dest_lost = Some(err.text().to_string());
                    o.stopped = true;
                    halt.store(true, Ordering::Relaxed);
                    return;
                }
                o.failed.push((e.path.clone(), err.text().to_string()));
                runlog.add(&[kind, &e.path, &size_text, "failed", &copied_as, err.text()]);
                failures_in_a_row += 1;
                if failures_in_a_row >= 25 {
                    o.keeps_failing = Some(err.text().to_string());
                    o.stopped = true;
                    halt.store(true, Ordering::Relaxed);
                }
            }
        }
        if o.files % 64 == 0 || last.elapsed().as_millis() > 500 {
            amount(done_bytes, total_bytes);
            progress(&format!(
                "Copied {} of {} files ({} of {})",
                commas(o.files + o.already_there),
                commas(total_files),
                size(done_bytes),
                size(total_bytes)
            ));
            last = std::time::Instant::now();
        }
    };
    // (Between folders too: a disk that stops answering while a folder is
    // read never gets as far as the next file.)
    let issues = crate::exfat::walk_until(&vol, &mut visit, &|| stop() || gone());
    drop(visit);
    amount(done_bytes, total_bytes);
    if o.disk_lost.is_none() && gone() {
        o.disk_lost = Some(watched.last_error());
        o.stopped = true;
    }
    let walked_all = !o.stopped && !halt.load(Ordering::Relaxed);

    if opts.lost && walked_all {
        // Data marked as in use that no file claims: one file per unbroken piece.
        let lost_dir = base.join("lost data (no name)");
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for (w, &own) in owned.iter().enumerate() {
            let mut alloc = 0u64;
            for b in 0..8 {
                if let Some(&byte) = vol.bitmap.get(w * 8 + b) {
                    alloc |= (byte as u64) << (8 * b);
                }
            }
            let first_index = w as u64 * 64;
            let valid = (vol.cluster_count as u64).saturating_sub(first_index);
            let mut lost = alloc & !own;
            if valid < 64 {
                lost &= (1u64 << valid) - 1;
            }
            while lost != 0 {
                let bit = lost.trailing_zeros() as u64;
                lost &= lost - 1;
                let c = (first_index + bit + 2) as u32;
                match runs.last_mut() {
                    Some(r) if r.0 as u64 + r.1 as u64 == c as u64 => r.1 += 1,
                    _ => runs.push((c, 1)),
                }
            }
        }
        if !runs.is_empty() {
            std::fs::create_dir_all(&lost_dir).map_err(|e| format!("could not create {}: {e}", lost_dir.display()))?;
        }
        for (first, count) in runs {
            if watched.errors() > errors_seen {
                errors_seen = watched.errors();
                if !watched.answers(probe_at) {
                    o.disk_lost = Some(watched.last_error());
                    o.stopped = true;
                    break;
                }
            }
            let off = vol.cluster_offset(first);
            let len = count as u64 * vol.cluster_size;
            let head = vol.src.read_at(off, 64).unwrap_or_default();
            let name = lost_piece_name(first, count, &head);
            let dest = lost_dir.join(&name);
            let shown = format!("lost data (no name)\\{name}");
            let earlier_sha = hashes.get(&log_key("LOST DATA", &shown, "")).map(|d| crate::sha256::hex(d));
            if std::fs::metadata(&dest).is_ok_and(|m| m.is_file() && m.len() == len)
                && piece_matches(&vol, off, len, &dest).unwrap_or(false)
                && earlier_sha.as_ref().is_some_and(|want| {
                    crate::sha256::file_hex(&dest, &|| false).ok().flatten().as_ref() == Some(want)
                })
            {
                // Saved by an earlier run, and all of it still what was saved.
                o.lost_pieces += 1;
                o.lost_bytes += len;
                runlog.add(&[
                    "LOST DATA",
                    &shown,
                    &len.to_string(),
                    "skipped",
                    "",
                    "already in the folder (left as it was)",
                    earlier_sha.as_deref().unwrap_or(""),
                ]);
                continue;
            }
            let tmp = lost_dir.join(PARTIAL);
            let result = (|| -> std::io::Result<String> {
                let mut f = File::create(&tmp)?;
                let mut sha = Sha256::new();
                let mut done = 0u64;
                while done < len {
                    let n = (len - done).min(8 << 20);
                    let data = vol.src.read_at(off + done, n as usize)?;
                    f.write_all(&data)?;
                    sha.update(&data);
                    if (data.len() as u64) < n {
                        break;
                    }
                    done += n;
                }
                drop(f);
                std::fs::rename(&tmp, &dest)?;
                Ok(sha.finish_hex())
            })();
            match result {
                Ok(sha) => {
                    o.lost_pieces += 1;
                    o.lost_bytes += len;
                    runlog.add(&["LOST DATA", &shown, &len.to_string(), "copied", "", "data no folder record names", &sha]);
                }
                Err(err) => {
                    let _ = std::fs::remove_file(&tmp);
                    o.failed.push((shown.clone(), err.to_string()));
                    runlog.add(&["LOST DATA", &shown, &len.to_string(), "failed", "", &err.to_string()]);
                }
            }
        }
    }
    if o.disk_lost.is_none() {
        for i in &issues {
            runlog.note(&i.folder, &i.what);
        }
    }
    if let Some(err) = &o.disk_lost {
        runlog.note("", &format!("the recovered disk stopped answering ({err}); run it again to copy the rest"));
    } else if let Some(err) = &o.dest_lost {
        runlog.note("", &format!("the folder the copies go to went away ({err}); run it again to copy the rest"));
    } else if let Some(err) = &o.keeps_failing {
        runlog.note("", &format!("stopped because copying kept failing ({err}); run it again to copy the rest"));
    } else if o.stopped {
        runlog.note("", "stopped on request before the end; run it again to copy the rest");
    }
    // The whole list: the earlier lists with this run's folded in. Notes
    // about folders: this run's when it read every folder, the earlier
    // ones too when it did not get that far.
    let wrote_all = runlog.finish();
    let had_earlier = earlier.any;
    drop(earlier);
    let tmp = base.join("rescue-log.csv.new");
    let walked_everything = walked_all && o.disk_lost.is_none();
    let result = wrote_all.and_then(|_| {
        if had_earlier {
            let merged = Earlier::read(&[(&log_path, !walked_everything), (&unfinished_path, true)]);
            std::fs::write(&tmp, format!("\u{FEFF}{}", merged.text(walked_everything)))
        } else {
            // The first run into this folder: its list is the whole list.
            let mut f = std::io::BufWriter::new(File::create(&tmp)?);
            f.write_all(format!("\u{FEFF}{}", csv_row(&LOG_HEADER)).as_bytes())?;
            std::io::copy(&mut File::open(&unfinished_path)?, &mut f)?;
            f.flush()
        }
    });
    result
        .and_then(|_| std::fs::rename(&tmp, &log_path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!(
                "could not write {} ({e}) - is it open in another program? Close it and run the copying again: \
                 nothing is copied twice",
                out.join(LOG_NAME).display()
            )
        })?;
    let _ = std::fs::remove_file(&unfinished_path);
    for (p, t) in &folder_times {
        set_folder_times(p, *t);
    }
    o.log = out.join(LOG_NAME).display().to_string();
    Ok(o)
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_windows_can_use() {
        use super::safe_name;
        assert_eq!(safe_name("dir 4 "), "dir 4_");
        assert_eq!(safe_name("notes."), "notes_");
        assert_eq!(safe_name("Holiday 2019"), "Holiday 2019");
        assert_eq!(safe_name("con"), "_con");
        assert_eq!(safe_name("NUL.txt"), "_NUL.txt");
        assert_eq!(safe_name("Com1.jpg"), "_Com1.jpg");
        assert_eq!(safe_name("LPT\u{b9}"), "_LPT\u{b9}");
        assert_eq!(safe_name("console.log"), "console.log");
        assert_eq!(safe_name("COM10"), "COM10");
        assert_eq!(safe_name("a:b?c"), "a_b_c");
        assert_eq!(safe_name(""), "_");
        assert_eq!(safe_name("家族の写真 1.jpg"), "家族の写真 1.jpg");
        let mut used = std::collections::HashSet::new();
        assert_eq!(super::unique(&mut used, "IMG.JPG".into(), false), "IMG.JPG");
        assert_eq!(super::unique(&mut used, "img.jpg".into(), false), "img (2).jpg");
        assert_eq!(super::unique(&mut used, "img.jpg".into(), false), "img (3).jpg");
        assert_eq!(super::unique(&mut used, "x.y".into(), true), "x.y");
        assert_eq!(super::unique(&mut used, "X.Y".into(), true), "X.Y (2)");
    }

    use super::{rescue, Options, Pick, LOG_NAME, UNFINISHED_LOG_NAME};
    use crate::disk::{Memory, Source, Unplugged};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicI64;

    fn opts(out: &Path, stop_after: Option<u64>) -> Options {
        Options {
            disk: String::new(),
            out_dir: out.display().to_string(),
            pick: Pick::Everything,
            lost: true,
            stop: None,
            stop_after_files: stop_after,
        }
    }

    fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                let rel = p.strip_prefix(dir).unwrap().display().to_string();
                if p.is_dir() {
                    out.insert(rel + "/", Vec::new());
                    stack.push(p);
                } else if !rel.starts_with("rescue-log") {
                    out.insert(rel, std::fs::read(&p).unwrap());
                }
            }
        }
        out
    }

    /// The list's lines, each as its fields (in any order).
    fn log_rows(dir: &Path) -> Vec<Vec<String>> {
        let text = std::fs::read_to_string(dir.join(LOG_NAME)).unwrap();
        let mut rows = crate::fmt::parse_csv(&text);
        rows.sort();
        rows
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rp-rescue-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_disk_that_stops_answering_stops_the_copy_and_nothing_is_lost() {
        let disk = Memory::new(crate::selftest::disk_image().unwrap());
        let dir = scratch("unplugged");
        let whole = dir.join("whole");
        let o = rescue(&disk, &opts(&whole, None), &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(!o.stopped && o.failed.is_empty() && o.files > 50, "{o:?}");
        // How many reads one whole run takes.
        let count = Unplugged { inner: &disk, reads_left: AtomicI64::new(i64::MAX) };
        rescue(&count, &opts(&dir.join("count"), None), &mut |_| {}, &mut |_, _| {}).unwrap();
        let reads = i64::MAX - count.reads_left.load(std::sync::atomic::Ordering::Relaxed);
        for k in [reads / 2, reads * 9 / 10] {
            let part = dir.join(format!("part-{k}"));
            let cut = Unplugged { inner: &disk, reads_left: AtomicI64::new(k) };
            let o1 = rescue(&cut, &opts(&part, None), &mut |_| {}, &mut |_, _| {}).unwrap();
            assert!(o1.stopped && o1.disk_lost.is_some(), "the copy stops when the disk stops answering: {o1:?}");
            assert!(o1.failed.is_empty(), "nothing is listed as failed: {:?}", o1.failed);
            assert!(o1.files <= o.files);
            // Plugged in again: the copy carries on and ends up the same.
            let o2 = rescue(&disk, &opts(&part, None), &mut |_| {}, &mut |_, _| {}).unwrap();
            assert!(!o2.stopped && o2.failed.is_empty() && o2.already_there > 0, "{o2:?}");
            assert_eq!(o1.files + o2.files, o.files);
            assert_eq!(tree(&part), tree(&whole), "the copies are the same as one whole run's");
            assert_eq!(log_rows(&part), log_rows(&whole), "the list is the same as one whole run's");
        }
        // The disk is gone before the copying even starts.
        let gone = Unplugged { inner: &disk, reads_left: AtomicI64::new(3) };
        let e = rescue(&gone, &opts(&dir.join("gone"), None), &mut |_| {}, &mut |_, _| {});
        assert!(e.is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stopping_and_starting_again_gives_the_same_copies_and_list() {
        let disk = Memory::new(crate::selftest::disk_image().unwrap());
        let dir = scratch("again");
        let whole = dir.join("whole");
        rescue(&disk, &opts(&whole, None), &mut |_| {}, &mut |_, _| {}).unwrap();
        let part = dir.join("part");
        let mut runs = 0;
        loop {
            runs += 1;
            let o = rescue(&disk, &opts(&part, Some(17)), &mut |_| {}, &mut |_, _| {}).unwrap();
            assert!(o.failed.is_empty());
            assert!(std::fs::metadata(part.join(UNFINISHED_LOG_NAME)).is_err(), "the unfinished list is folded in");
            if !o.stopped {
                break;
            }
            assert!(runs < 50);
        }
        assert!(runs > 3);
        assert_eq!(tree(&part), tree(&whole));
        assert_eq!(log_rows(&part), log_rows(&whole));

        // A run that never got to the end (the PC lost power) leaves its
        // unfinished list; the next run folds it in.
        let crash = dir.join("crash");
        rescue(&disk, &opts(&crash, Some(30)), &mut |_| {}, &mut |_, _| {}).unwrap();
        std::fs::rename(crash.join(LOG_NAME), crash.join(UNFINISHED_LOG_NAME)).unwrap();
        let o = rescue(&disk, &opts(&crash, None), &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(!o.stopped && o.failed.is_empty());
        assert_eq!(tree(&crash), tree(&whole));
        assert_eq!(log_rows(&crash), log_rows(&whole));
        let _ = disk.size();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn copies_the_power_cut_left_as_zeros_are_copied_again() {
        let disk = Memory::new(crate::selftest::disk_image().unwrap());
        let dir = scratch("power");
        let whole = dir.join("whole");
        rescue(&disk, &opts(&whole, None), &mut |_| {}, &mut |_, _| {}).unwrap();
        let part = dir.join("part");
        rescue(&disk, &opts(&part, None), &mut |_| {}, &mut |_, _| {}).unwrap();
        // The power went just after these were named: their data never
        // reached the drive (zeros), though their size is right.
        let mut spoilt = Vec::new();
        for (rel, data) in tree(&part) {
            if !rel.ends_with('/') && data.iter().any(|&b| b != 0) && spoilt.len() < 5 {
                std::fs::write(part.join(&rel), vec![0u8; data.len()]).unwrap();
                spoilt.push(rel);
            }
        }
        assert_eq!(spoilt.len(), 5);
        let o = rescue(&disk, &opts(&part, None), &mut |_| {}, &mut |_, _| {}).unwrap();
        assert!(!o.stopped && o.failed.is_empty(), "{o:?}");
        let lost_spoilt = spoilt.iter().filter(|r| r.starts_with("lost data")).count() as u64;
        assert_eq!(o.recopied + lost_spoilt, 5, "{o:?} {spoilt:?}");
        assert_eq!(tree(&part), tree(&whole), "every copy holds what the disk holds again");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn names_for_lost_data() {
        assert_eq!(super::extension_for(b"\xff\xd8\xff\xe1"), "jpg");
        assert_eq!(super::extension_for(b"\x00\x00\x00\x14ftypqt  "), "mov");
        assert_eq!(super::extension_for(b"\x00\x00\x00\x18ftypheic"), "heic");
        assert_eq!(super::extension_for(b"RIFF\x00\x00\x00\x00WAVEfmt "), "wav");
        assert_eq!(super::extension_for(b"nothing"), "bin");
    }
}
