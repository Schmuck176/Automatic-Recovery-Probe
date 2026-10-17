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
use std::collections::HashSet;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

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
    pub log: String,
}

/// An absolute path Windows will not shorten or reinterpret (long paths,
/// names ending in a dot or a space, names like CON).
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
        p.push(part);
    }
    p
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

/// Copy one file's data from the disk. Returns a note when something could
/// not be copied (the missing parts are left as zeros).
fn copy_file(vol: &Volume, e: &Entry, list: &ClusterList, dest: &Path) -> Result<Option<String>, String> {
    let mut f = File::create(verbatim(dest)).map_err(|err| format!("could not create the copy: {err}"))?;
    let want = e.data_length;
    let vdl = e.valid_data_length.min(want);
    let mut written = 0u64;
    let mut note = list.problem.clone();
    'runs: for &(first, count) in &list.runs {
        let run_bytes = count as u64 * vol.cluster_size;
        let mut done = 0u64;
        while done < run_bytes && written < vdl {
            let n = (run_bytes - done).min(8 << 20).min(vdl - written);
            let data = vol
                .src
                .read_at(vol.cluster_offset(first) + done, n as usize)
                .map_err(|err| format!("could not read the recovered disk: {err}"))?;
            f.write_all(&data).map_err(|err| format!("could not write the copy: {err}"))?;
            written += data.len() as u64;
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
    Ok(note)
}

fn same_volume(out: &Path, vol_serial: u32) -> Option<String> {
    let drive = crate::sys::drive_for_serial(vol_serial)?;
    let abs = std::path::absolute(out).ok()?.to_string_lossy().to_uppercase();
    let d = drive.to_uppercase();
    (abs.starts_with(&d) || abs.starts_with(&format!(r"\\?\{d}"))).then_some(drive)
}

pub fn run(opts: &Options, progress: &mut dyn FnMut(&str)) -> Result<Outcome, String> {
    let src = open_source(&opts.disk).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!("Windows would not let the program read {} - it needs to run as administrator", opts.disk)
        } else {
            format!("could not open {}: {e}", opts.disk)
        }
    })?;
    rescue(&*src, opts, progress)
}

pub fn rescue(src: &dyn Source, opts: &Options, progress: &mut dyn FnMut(&str)) -> Result<Outcome, String> {
    let table = gpt::read(src, &[]);
    let (vol, _) = crate::mapcheck::find_volume(src, &table)?;
    let out = PathBuf::from(&opts.out_dir);
    if let Some(d) = same_volume(&out, vol.serial) {
        return Err(format!(
            "{} is on the recovered disk itself ({d}). Choose a folder on another drive - nothing is ever written to the recovered disk.",
            opts.out_dir
        ));
    }
    std::fs::create_dir_all(verbatim(&out)).map_err(|e| format!("could not create {}: {e}", out.display()))?;
    progress(&format!("Copying from the exFAT volume \"{}\" into {}", vol.label, out.display()));

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

    let mut o = Outcome::default();
    let mut log = csv_row(&["kind", "path", "size_bytes", "result", "note"]);
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
    let mut visit = |e: &Entry, list: &ClusterList| {
        mark(list);
        if !picked(e) {
            return;
        }
        let dest = dest_path(&out, &e.path);
        if e.is_dir {
            match std::fs::create_dir_all(verbatim(&dest)) {
                Ok(_) => {
                    o.folders += 1;
                    made_dirs.insert(dest);
                }
                Err(err) => {
                    o.failed.push((e.path.clone(), err.to_string()));
                    log.push_str(&csv_row(&["FOLDER", &e.path, "", "failed", &err.to_string()]));
                }
            }
            return;
        }
        if let Some(parent) = dest.parent() {
            if !made_dirs.contains(parent) {
                let _ = std::fs::create_dir_all(verbatim(parent));
                made_dirs.insert(parent.to_path_buf());
            }
        }
        if std::fs::symlink_metadata(verbatim(&dest)).is_ok() {
            o.already_there += 1;
            log.push_str(&csv_row(&[
                "FILE",
                &e.path,
                &e.data_length.to_string(),
                "skipped",
                "already in the folder (left as it was)",
            ]));
            return;
        }
        match copy_file(&vol, e, list, &dest) {
            Ok(note) => {
                o.files += 1;
                o.bytes += e.data_length;
                let (result, n) = match &note {
                    Some(n) => {
                        o.partial.push((e.path.clone(), n.clone()));
                        ("partly copied", n.clone())
                    }
                    None => ("copied", String::new()),
                };
                let n = if e.hidden_from_windows && n.is_empty() {
                    format!("Windows cannot list it: {}", e.why_hidden)
                } else {
                    n
                };
                log.push_str(&csv_row(&["FILE", &e.path, &e.data_length.to_string(), result, &n]));
            }
            Err(err) => {
                o.failed.push((e.path.clone(), err.clone()));
                log.push_str(&csv_row(&["FILE", &e.path, &e.data_length.to_string(), "failed", &err]));
            }
        }
        if last.elapsed().as_millis() > 500 {
            progress(&format!("Copied {} files ({})", commas(o.files), size(o.bytes)));
            last = std::time::Instant::now();
        }
    };
    let issues = walk(&vol, &mut visit);
    drop(visit);

    if opts.lost {
        // Data marked as in use that no file claims: one file per unbroken piece.
        let lost_dir = out.join("lost data (no name)");
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
            std::fs::create_dir_all(verbatim(&lost_dir)).map_err(|e| format!("could not create {}: {e}", lost_dir.display()))?;
        }
        for (first, count) in runs {
            let off = vol.cluster_offset(first);
            let len = count as u64 * vol.cluster_size;
            let head = vol.src.read_at(off, 64).unwrap_or_default();
            let name = format!("cluster {first} ({count} clusters).{}", extension_for(&head));
            let dest = lost_dir.join(&name);
            let result = (|| -> std::io::Result<()> {
                let mut f = File::create(verbatim(&dest))?;
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
                    log.push_str(&csv_row(&["LOST DATA", &shown, &len.to_string(), "copied", "data no folder record names"]));
                }
                Err(err) => {
                    o.failed.push((shown.clone(), err.to_string()));
                    log.push_str(&csv_row(&["LOST DATA", &shown, &len.to_string(), "failed", &err.to_string()]));
                }
            }
        }
    }
    for i in &issues {
        log.push_str(&csv_row(&["NOTE", &i.folder, "", "", &i.what]));
    }
    let log_path = out.join("rescue-log.csv");
    std::fs::write(verbatim(&log_path), format!("\u{FEFF}{log}")).map_err(|e| format!("could not write {}: {e}", log_path.display()))?;
    o.log = log_path.display().to_string();
    Ok(o)
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_for_lost_data() {
        assert_eq!(super::extension_for(b"\xff\xd8\xff\xe1"), "jpg");
        assert_eq!(super::extension_for(b"\x00\x00\x00\x14ftypqt  "), "mov");
        assert_eq!(super::extension_for(b"\x00\x00\x00\x18ftypheic"), "heic");
        assert_eq!(super::extension_for(b"RIFF\x00\x00\x00\x00WAVEfmt "), "wav");
        assert_eq!(super::extension_for(b"nothing"), "bin");
    }
}
