//! Rescue: copy files straight off the recovered disk, past Windows.
//!
//! Files whose folder record comes after a blank (never-read) record are
//! invisible to Explorer and TeraCopy, so a normal copy leaves them behind
//! without a word. This copies them with the program's own exFAT reader.
//! It only ever reads the recovered disk, and writes only into the chosen
//! folder, which must be on a different drive.

use crate::disk::{open_source, Source};
use crate::exfat::{timestamp_to_unix, walk, ClusterList, Entry, Volume};
use crate::fmt::{commas, csv_row, size};
use crate::gpt;
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
    /// Stopped before the end (on request).
    pub stopped: bool,
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
fn verbatim(p: &Path) -> PathBuf {
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

/// Copy one file's data from the disk into `dest` (by way of a temporary
/// name beside it). Returns a note when something could not be copied (the
/// missing parts are left as zeros), or None for "stopped".
fn copy_file(
    vol: &Volume,
    e: &Entry,
    list: &ClusterList,
    dest: &Path,
    stop: &dyn Fn() -> bool,
    copied: &mut dyn FnMut(u64),
) -> Result<Option<Option<String>>, String> {
    let tmp = dest.with_file_name(PARTIAL);
    let mut f = File::create(&tmp).map_err(|err| format!("could not create the copy: {err}"))?;
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
        format!("could not give the copy its name: {err}")
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
) -> Result<Option<Option<String>>, String> {
    let want = e.data_length;
    let vdl = e.valid_data_length.min(want);
    let mut written = 0u64;
    let mut note = list.problem.clone();
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
                .map_err(|err| format!("could not read the recovered disk: {err}"))?;
            f.write_all(&data).map_err(|err| format!("could not write the copy: {err}"))?;
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
        f.set_len(want).map_err(|err| format!("could not write the copy: {err}"))?;
    }
    if let Some((secs, nanos)) = timestamp_to_unix(e.modified, e.modified_10ms, e.modified_utc) {
        let t = if secs >= 0 {
            std::time::UNIX_EPOCH + std::time::Duration::new(secs as u64, nanos)
        } else {
            std::time::UNIX_EPOCH
        };
        let _ = f.set_modified(t);
    }
    Ok(Some(note))
}

fn same_volume(out: &Path, vol_serial: u32) -> Option<String> {
    let drive = crate::sys::drive_for_serial(vol_serial)?;
    let abs = std::path::absolute(out).ok()?.to_string_lossy().to_uppercase();
    let d = drive.to_uppercase();
    (abs.starts_with(&d) || abs.starts_with(&format!(r"\\?\{d}"))).then_some(drive)
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

pub fn rescue(
    src: &dyn Source,
    opts: &Options,
    progress: &mut dyn FnMut(&str),
    amount: &mut dyn FnMut(u64, u64),
) -> Result<Outcome, String> {
    let table = gpt::read(src, &[]);
    let (vol, _) = crate::mapcheck::find_volume(src, &table)?;
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
    let stop = move || stop_flag.as_ref().is_some_and(|s| s.load(Ordering::Relaxed));

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

    // First the totals, so the progress can say how far along it is.
    let (mut total_files, mut total_bytes) = (0u64, 0u64);
    walk(&vol, &mut |e: &Entry, _: &ClusterList| {
        if picked(e) && !e.is_dir {
            total_files += 1;
            total_bytes += e.valid_data_length.min(e.data_length);
        }
    });
    progress(&format!("{} files to copy ({})", commas(total_files), size(total_bytes)));
    let mut done_bytes = 0u64;
    amount(0, total_bytes);

    let mut o = Outcome::default();
    let mut log = csv_row(&["kind", "path", "size_bytes", "result", "copied_as", "note"]);
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
    // Where each folder of the volume is copied to (names made safe for
    // Windows, and kept apart where two would end up the same).
    let mut dirs: HashMap<String, PathBuf> = HashMap::new();
    dirs.insert(String::new(), base.clone());
    let mut cur_parent: Option<String> = None;
    let mut used: HashSet<String> = HashSet::new();
    let mut visit = |e: &Entry, list: &ClusterList| {
        mark(list);
        if o.stopped {
            return;
        }
        if stop() {
            o.stopped = true;
            return;
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
                    made_dirs.insert(dest);
                    if !copied_as.is_empty() {
                        log.push_str(&csv_row(&[kind, &e.path, "", "copied", &copied_as, renamed_note.trim()]));
                    }
                }
                Err(err) => {
                    o.failed.push((e.path.clone(), err.to_string()));
                    log.push_str(&csv_row(&[kind, &e.path, "", "failed", &copied_as, &err.to_string()]));
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
        if std::fs::symlink_metadata(&dest).is_ok() {
            // The same size: a copy made by an earlier run.
            o.already_there += 1;
            done_bytes += e.valid_data_length.min(e.data_length);
            log.push_str(&csv_row(&[
                kind,
                &e.path,
                &size_text,
                "skipped",
                &copied_as,
                "already in the folder (left as it was)",
            ]));
            return;
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
            Ok(Some(note)) => {
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
                if e.hidden_from_windows && note.is_none() {
                    n.push_str(&format!("Windows cannot list it: {}", e.why_hidden));
                }
                log.push_str(&csv_row(&[kind, &e.path, &size_text, result, &copied_as, n.trim()]));
            }
            Err(err) => {
                o.failed.push((e.path.clone(), err.clone()));
                log.push_str(&csv_row(&[kind, &e.path, &size_text, "failed", &copied_as, &err]));
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
    let issues = walk(&vol, &mut visit);
    drop(visit);
    amount(done_bytes, total_bytes);

    if opts.lost && !o.stopped {
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
            let off = vol.cluster_offset(first);
            let len = count as u64 * vol.cluster_size;
            let head = vol.src.read_at(off, 64).unwrap_or_default();
            let name = format!("cluster {first} ({count} clusters).{}", extension_for(&head));
            let dest = lost_dir.join(&name);
            let result = (|| -> std::io::Result<()> {
                let mut f = File::create(&dest)?;
                let mut done = 0u64;
                while done < len {
                    let n = (len - done).min(8 << 20);
                    let data = vol.src.read_at(off + done, n as usize)?;
                    f.write_all(&data)?;
                    if (data.len() as u64) < n {
                        break;
                    }
                    done += n;
                }
                Ok(())
            })();
            let shown = format!("lost data (no name)\\{name}");
            match result {
                Ok(()) => {
                    o.lost_pieces += 1;
                    o.lost_bytes += len;
                    log.push_str(&csv_row(&["LOST DATA", &shown, &len.to_string(), "copied", "", "data no folder record names"]));
                }
                Err(err) => {
                    o.failed.push((shown.clone(), err.to_string()));
                    log.push_str(&csv_row(&["LOST DATA", &shown, &len.to_string(), "failed", "", &err.to_string()]));
                }
            }
        }
    }
    for i in &issues {
        log.push_str(&csv_row(&["NOTE", &i.folder, "", "", "", &i.what]));
    }
    if o.stopped {
        log.push_str(&csv_row(&["NOTE", "", "", "", "", "stopped on request before the end; run it again to copy the rest"]));
    }
    let log_path = base.join("rescue-log.csv");
    std::fs::write(&log_path, format!("\u{FEFF}{log}")).map_err(|e| format!("could not write {}: {e}", out.join("rescue-log.csv").display()))?;
    o.log = out.join("rescue-log.csv").display().to_string();
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

    #[test]
    fn names_for_lost_data() {
        assert_eq!(super::extension_for(b"\xff\xd8\xff\xe1"), "jpg");
        assert_eq!(super::extension_for(b"\x00\x00\x00\x14ftypqt  "), "mov");
        assert_eq!(super::extension_for(b"\x00\x00\x00\x18ftypheic"), "heic");
        assert_eq!(super::extension_for(b"RIFF\x00\x00\x00\x00WAVEfmt "), "wav");
        assert_eq!(super::extension_for(b"nothing"), "bin");
    }
}
