//! The damaged files: isolate, repair, put back, evaluate.
//!
//! The copies in WORK are never changed by hand. This takes the damaged ones
//! out to be repaired and puts the repaired ones back, without ever splitting
//! a package or losing track of where anything belongs:
//!
//! 1. Isolate: each damaged file is copied (never moved) into
//!    REPAIR\to-repair\<what repairs it>\, its name tagged "[R0012]";
//!    REPAIR\repair-list.csv records where R0012 is in WORK and its SHA-256.
//! 2. Repair: done by the person, with the right tool, on those copies. A
//!    repaired version goes anywhere under REPAIR\repaired\, keeping its tag.
//!    What can't take the original's place (a TIFF made from a damaged RAW
//!    photo, what 7-Zip took out of a damaged archive) is kept in
//!    REPAIR\salvaged\, and the original stays in WORK as it is.
//! 3. Put back: a repaired file goes back to exactly where its original is in
//!    WORK, if the file there is still that original (by its SHA-256). The
//!    original moves to REPAIR\originals\ (same folders, never deleted), the
//!    dates stay the original's, and WORK\repair-returns.csv records both
//!    checksums, so checking the copies later knows about it. Undo puts the
//!    original back.
//! 4. Evaluate: every package (a Photos library, a Final Cut library, an app,
//!    a Pages document...): what is damaged inside it, of what kind, what has
//!    been put back and what is still damaged.
//!
//! Packages are never split. A damaged photo, video or sound file inside a
//! library is copied out and put back to the same place inside it; a document
//! kept as a folder goes out and back whole; a library's own data (its
//! database, a project file) and anything inside an app are not taken out at
//! all: the library's own app repairs it whole, on a copy, and an app is
//! reinstalled. Housekeeping files (.DS_Store, ._ files, Thumbs.db,
//! desktop.ini) are left exactly where they are.
//!
//! REPAIR sits beside WORK, on the same drive, so moving an original out of
//! WORK is a rename: nothing is ever deleted.

use crate::fmt::{commas, csv_row, size};
use crate::sha256::{file_hex, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const REPAIR_DIR: &str = "REPAIR";
pub const LIST_NAME: &str = "repair-list.csv";
pub const RETURNS_NAME: &str = "repair-returns.csv";
pub const EVALUATION_NAME: &str = "evaluation.csv";
pub const TO_REPAIR: &str = "to-repair";
pub const REPAIRED: &str = "repaired";
pub const ORIGINALS: &str = "originals";
pub const REPLACED: &str = "replaced";
pub const LISTS: &str = "damaged-lists";
pub const SALVAGED: &str = "salvaged";
const README_NAME: &str = "READ ME.txt";
const TMP_NAME: &str = "~recovery-probe-returning.tmp";
/// A document kept as a folder bigger than this is not copied out whole.
const PACKAGE_LIMIT: u64 = 4 << 30;

// ---------------------------------------------------------------- packages

/// What kind of package a folder is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A library or project: its own app repairs it whole; the photos,
    /// videos and sound inside can be repaired one by one.
    Library,
    /// A document kept as a folder (Pages, Numbers, Keynote...): out and back whole.
    Document,
    /// A program: reinstalled, never repaired.
    Program,
    /// A disk image or virtual machine kept as a folder.
    DiskImage,
}

const PACKAGES: &[(&str, Kind, &str)] = &[
    ("photoslibrary", Kind::Library, "Photos library"),
    ("photolibrary", Kind::Library, "iPhoto library"),
    ("migratedphotolibrary", Kind::Library, "Photos library (migrated)"),
    ("pplibrary", Kind::Library, "Photos library"),
    ("aplibrary", Kind::Library, "Aperture library"),
    ("aplibrary2", Kind::Library, "Aperture library"),
    ("photolibrary2", Kind::Library, "iPhoto library"),
    ("fcpbundle", Kind::Library, "Final Cut Pro library"),
    ("imovielibrary", Kind::Library, "iMovie library"),
    ("theater", Kind::Library, "iMovie Theater"),
    ("logicx", Kind::Library, "Logic Pro project"),
    ("band", Kind::Library, "GarageBand project"),
    ("musiclibrary", Kind::Library, "Music library"),
    ("tvlibrary", Kind::Library, "TV library"),
    ("cocatalog", Kind::Library, "Capture One catalogue"),
    ("lrlibrary", Kind::Library, "Lightroom library"),
    ("pages", Kind::Document, "Pages document"),
    ("numbers", Kind::Document, "Numbers spreadsheet"),
    ("key", Kind::Document, "Keynote presentation"),
    ("rtfd", Kind::Document, "TextEdit document with pictures"),
    ("scriv", Kind::Document, "Scrivener project"),
    ("graffle", Kind::Document, "OmniGraffle document"),
    ("oo3", Kind::Document, "OmniOutliner document"),
    ("app", Kind::Program, "app"),
    ("framework", Kind::Program, "program part"),
    ("bundle", Kind::Program, "program part"),
    ("plugin", Kind::Program, "plug-in"),
    ("kext", Kind::Program, "system extension"),
    ("prefpane", Kind::Program, "settings pane"),
    ("saver", Kind::Program, "screen saver"),
    ("qlgenerator", Kind::Program, "Quick Look plug-in"),
    ("mdimporter", Kind::Program, "Spotlight plug-in"),
    ("appex", Kind::Program, "app extension"),
    ("xpc", Kind::Program, "program part"),
    ("component", Kind::Program, "audio plug-in"),
    ("vst", Kind::Program, "audio plug-in"),
    ("vst3", Kind::Program, "audio plug-in"),
    ("aaxplugin", Kind::Program, "audio plug-in"),
    ("wdgt", Kind::Program, "widget"),
    ("dsym", Kind::Program, "debugging symbols"),
    ("sparsebundle", Kind::DiskImage, "disk image"),
    ("vmwarevm", Kind::DiskImage, "virtual machine"),
    ("pvm", Kind::DiskImage, "virtual machine"),
    ("utm", Kind::DiskImage, "virtual machine"),
];

/// A folder's package kind, from its name ("Holiday.photoslibrary").
pub fn package_of_name(name: &str) -> Option<(Kind, &'static str)> {
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    let ext = ext.to_lowercase();
    PACKAGES.iter().find(|p| p.0 == ext).map(|p| (p.1, p.2))
}

/// The outermost package a path in WORK is inside, from the folder names:
/// (its path in WORK, kind, what it is). The path's own last part is not
/// looked at (it is the file).
pub fn package_around(rel: &str) -> Option<(String, Kind, &'static str)> {
    let parts: Vec<&str> = rel.split('\\').filter(|c| !c.is_empty()).collect();
    for i in 0..parts.len().saturating_sub(1) {
        if let Some((k, what)) = package_of_name(parts[i]) {
            return Some((parts[..=i].join("\\"), k, what));
        }
    }
    None
}

/// The same, also recognising a Photos or iPhoto library by what is inside
/// it (old ones have no ".photolibrary" on their name).
fn package_around_on_disk(work: &Path, rel: &str) -> Option<(String, Kind, &'static str)> {
    if let Some(p) = package_around(rel) {
        return Some(p);
    }
    let parts: Vec<&str> = rel.split('\\').filter(|c| !c.is_empty()).collect();
    let mut dir = crate::rescue::verbatim(work);
    for i in 0..parts.len().saturating_sub(1) {
        dir.push(parts[i]);
        let subdirs: Vec<String> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        if !crate::walk::looks_like_library(&dir.to_string_lossy(), &subdirs).is_empty() {
            return Some((parts[..=i].join("\\"), Kind::Library, "Photos library"));
        }
    }
    None
}

/// What a file inside a library is to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Your photos, videos and sound: repaired one by one.
    Media,
    /// The library's own data (its database, project files).
    Core,
    /// Previews, thumbnails, caches: the app makes them again.
    Previews,
}

/// Folders inside libraries that hold only what the app makes again.
const PREVIEW_DIRS: &[&str] = &[
    "derivatives",
    "proxies",
    "proxy media",
    "render files",
    "transcoded media",
    "analysis files",
    "thumbnails",
    "thumbs",
    "previews",
    "caches",
    "cache",
];

/// The role of a file inside a library (`inside`: its path from the
/// library's top).
pub fn member_role(inside: &str) -> Role {
    let low = inside.to_lowercase();
    let parts: Vec<&str> = low.split('\\').filter(|c| !c.is_empty()).collect();
    let (dirs, name) = parts.split_at(parts.len().saturating_sub(1));
    if dirs.iter().any(|d| PREVIEW_DIRS.contains(d) || d.ends_with(".lrdata") || d.ends_with(".nosync")) {
        return Role::Previews;
    }
    if is_media(ext_of(name.first().copied().unwrap_or(""))) {
        Role::Media
    } else {
        Role::Core
    }
}

// ---------------------------------------------------------------- who repairs what

/// Where a damaged file goes in REPAIR\to-repair: the folder names say what
/// repairs what is in them.
pub const GROUPS: &[(&str, &[&str])] = &[
    ("1 Photos (photo repair tool)", &["jpg", "jpeg", "jpe", "jfif", "png", "gif", "bmp", "tif", "tiff", "webp"]),
    (
        "2 iPhone photos and RAW (on the Mac)",
        &[
            "heic", "heif", "hif", "avif", "dng", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2",
            "raw", "pef", "ptx", "srw", "3fr", "erf", "mef", "mos", "mrw", "x3f", "iiq", "k25", "kdc", "dcr", "rwl", "fff",
        ],
    ),
    (
        "3 Videos (untrunc or video repair tool)",
        &["mp4", "m4v", "mov", "qt", "3gp", "3g2", "avi", "mkv", "webm", "mts", "m2ts", "mpg", "mpeg", "wmv", "flv"],
    ),
    ("4 Music and sound (MP3val for MP3)", &["mp3", "m4a", "aac", "flac", "wav", "aif", "aiff", "ogg", "oga", "opus", "wma", "caf"]),
    (
        "5 Office documents (Open and Repair)",
        &["doc", "docx", "docm", "dot", "dotx", "xls", "xlsx", "xlsm", "xlt", "xltx", "ppt", "pptx", "pptm", "pps", "ppsx", "odt", "ods", "odp", "rtf"],
    ),
    ("6 PDF documents (PDF repair tool)", &["pdf"]),
    ("7 Archives and disk images (7-Zip)", &["zip", "7z", "rar", "gz", "tgz", "tar", "bz2", "xz", "zst", "cab", "iso", "dmg", "sparseimage"]),
    ("8 Mac documents (Pages, Numbers, Keynote on the Mac)", &["pages", "numbers", "key", "rtfd", "webarchive", "scriv", "graffle", "oo3"]),
];
pub const OTHER_GROUP: &str = "9 Everything else (by hand)";

/// What each group's tools are, in plain words (for the READ ME and the page).
const GROUP_HELP: &[&str] = &[
    "a photo repair tool (Stellar, Repairit), or an intact near-twin found with Krokiet's Similar Images",
    "Preview on the Mac: File > Export, in the same format (a HEIC stays a HEIC); a camera RAW can't be saved as RAW \
again, so a TIFF of it goes in \"salvaged\" (take the folder to the Mac on a USB stick: the Mac cannot write to SPARE)",
    "untrunc, with a good video from the same camera or phone, or a video repair tool",
    "MP3val's repair button for MP3s (on these copies only); other sound files: a sound editor",
    "Word, Excel or PowerPoint: File > Open, choose the file, then the arrow beside Open > Open and Repair",
    "a PDF repair tool, or open it in a PDF reader and save it again",
    "7-Zip: open it and take out what can still be read, into \"salvaged\"",
    "Pages, Numbers, Keynote or TextEdit on the Mac (on a USB stick: the Mac cannot write to SPARE)",
];

fn ext_of(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext,
        _ => "",
    }
}

fn is_media(ext: &str) -> bool {
    let e = ext.to_lowercase();
    GROUPS[..4].iter().any(|(_, exts)| exts.contains(&e.as_str()))
}

/// The group a file (or a document package) goes into, by its name.
pub fn group_of(name: &str) -> &'static str {
    let e = ext_of(name).to_lowercase();
    GROUPS.iter().find(|(_, exts)| exts.contains(&e.as_str())).map(|g| g.0).unwrap_or(OTHER_GROUP)
}

fn is_windows_program(name: &str) -> bool {
    matches!(ext_of(name).to_lowercase().as_str(), "exe" | "dll" | "msi" | "sys" | "ocx" | "scr")
}

fn is_housekeeping(name: &str) -> bool {
    name.starts_with("._") || crate::formats::is_system_file(&name.to_lowercase())
}

/// Folders a Mac or Windows keeps at the top of a drive (a USB stick the
/// repaired files came back on, say): none of them is a repaired file.
fn is_drive_housekeeping(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        ".spotlight-v100" | ".fseventsd" | ".trashes" | ".temporaryitems" | "$recycle.bin" | "system volume information"
    )
}

pub const LEAVE_HOUSEKEEPING: &str =
    "a housekeeping file (folder settings, thumbnails or Mac metadata): nothing to repair, and it stays where it is";
pub const LEAVE_PROGRAM: &str =
    "part of a program: reinstall the program (a program cannot be repaired, and nothing of yours is inside it)";
pub const LEAVE_WINDOWS_PROGRAM: &str = "a program or installer: download it again";
pub const LEAVE_LIBRARY_CORE: &str = "the library's own data (its database or a project file): repair the whole library \
with its own app, on a copy on the Mac";
pub const LEAVE_PREVIEWS: &str = "a preview the library's own app makes again by itself: nothing to do";
pub const LEAVE_DISK_IMAGE: &str =
    "inside a disk image or virtual machine: check a copy of the whole thing in its own app on the Mac";
pub const LEAVE_UNREADABLE: &str = "the check could not read it at all: look at it by hand (it may not be damaged)";
pub const LEAVE_TOO_BIG: &str = "a document package too big to copy out whole: open it on the Mac";

/// What to do with one damaged file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Copied out: on its own (a file) or whole (a document package).
    Isolate { unit: String, package: bool, group: &'static str },
    /// Not copied out, and why.
    Leave(&'static str),
}

/// The plan for a damaged file, given the package around it.
pub fn plan_for(rel: &str, around: Option<&(String, Kind, &'static str)>, only_unreadable: bool) -> Plan {
    let name = rel.rsplit('\\').next().unwrap_or(rel);
    if only_unreadable {
        return Plan::Leave(LEAVE_UNREADABLE);
    }
    if is_housekeeping(name) {
        return Plan::Leave(LEAVE_HOUSEKEEPING);
    }
    match around {
        Some((_, Kind::Program, _)) => Plan::Leave(LEAVE_PROGRAM),
        Some((_, Kind::DiskImage, _)) => Plan::Leave(LEAVE_DISK_IMAGE),
        Some((root, Kind::Document, _)) => Plan::Isolate {
            unit: root.clone(),
            package: true,
            group: group_of(root.rsplit('\\').next().unwrap_or(root)),
        },
        Some((root, Kind::Library, _)) => match member_role(&rel[root.len().min(rel.len())..]) {
            Role::Media => Plan::Isolate { unit: rel.to_string(), package: false, group: group_of(name) },
            Role::Core => Plan::Leave(LEAVE_LIBRARY_CORE),
            Role::Previews => Plan::Leave(LEAVE_PREVIEWS),
        },
        None if is_windows_program(name) => Plan::Leave(LEAVE_WINDOWS_PROGRAM),
        None => Plan::Isolate { unit: rel.to_string(), package: false, group: group_of(name) },
    }
}

// ---------------------------------------------------------------- tags

/// "IMG_0003.JPG" + "R0012" -> "IMG_0003 [R0012].JPG" (the tag goes before
/// the extension, so the file or package is still what it was).
pub fn tagged(name: &str, id: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} [{id}].{ext}"),
        _ => format!("{name} [{id}]"),
    }
}

/// The tag in a name, as "R0012": exactly one "[R...]" of digits.
pub fn tag_in(name: &str) -> Option<String> {
    let b = name.as_bytes();
    let mut found: Option<String> = None;
    let mut i = 0;
    while i + 3 < b.len() {
        if b[i] == b'[' && (b[i + 1] == b'R' || b[i + 1] == b'r') {
            let mut j = i + 2;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 2 && j < b.len() && b[j] == b']' {
                let n: u64 = name[i + 2..j].parse().ok()?;
                if found.is_some() {
                    return None;
                }
                found = Some(id_of(n));
                i = j;
            }
        }
        i += 1;
    }
    found
}

fn id_of(n: u64) -> String {
    format!("R{n:04}")
}

fn id_number(id: &str) -> u64 {
    id.trim_start_matches(['R', 'r']).parse().unwrap_or(0)
}

// ---------------------------------------------------------------- paths

/// REPAIR, beside WORK on the same drive.
pub fn repair_dir(work: &Path) -> Result<PathBuf, String> {
    let abs = std::path::absolute(work).unwrap_or_else(|_| work.to_path_buf());
    match abs.parent() {
        Some(p) if abs.file_name().is_some() => Ok(p.join(REPAIR_DIR)),
        _ => Err(format!(
            "{} is a whole drive: choose the folder the copies went into (such as E:\\WORK), so that REPAIR can go beside it",
            work.display()
        )),
    }
}

fn under(base: &Path, rel: &str) -> PathBuf {
    let mut p = crate::rescue::verbatim(base);
    for c in rel.split('\\').filter(|c| !c.is_empty()) {
        p.push(c);
    }
    p
}

fn norm(p: &str) -> String {
    let s = p.replace('/', "\\");
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    s.trim_end_matches('\\').to_string()
}

/// A path's part inside `work` ("E:\WORK\Photos\a.jpg" -> "Photos\a.jpg").
pub fn relative_to(work: &Path, path: &str) -> Option<String> {
    let w = norm(&std::path::absolute(work).unwrap_or_else(|_| work.to_path_buf()).to_string_lossy());
    let p = norm(path);
    if p.len() > w.len() + 1 && p[..w.len()].eq_ignore_ascii_case(&w) && p.as_bytes()[w.len()] == b'\\' {
        Some(p[w.len() + 1..].to_string())
    } else {
        None
    }
}

fn now_text() -> String {
    let now =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as f64).unwrap_or(0.0);
    crate::fmt::local_datetime(now)
}

// ---------------------------------------------------------------- hashing and copying

fn tree_files(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().to_lowercase();
        let here = if rel.is_empty() { name } else { format!("{rel}\\{name}") };
        if e.file_type()?.is_dir() {
            tree_files(&e.path(), &here, out)?;
        } else {
            out.push((here, e.path()));
        }
    }
    Ok(())
}

/// (bytes, SHA-256) of a file, or of a folder's files and their names.
pub(crate) fn hash_of(p: &Path, stop: &dyn Fn() -> bool) -> std::io::Result<Option<(u64, String)>> {
    hash_with(p, stop, false)
}

/// The same, read as the drive holds it, not from what Windows still keeps
/// in memory of something just written (sha256::file_hex_from_drive).
fn hash_on_drive(p: &Path, stop: &dyn Fn() -> bool) -> std::io::Result<Option<(u64, String)>> {
    hash_with(p, stop, true)
}

fn hash_with(p: &Path, stop: &dyn Fn() -> bool, on_drive: bool) -> std::io::Result<Option<(u64, String)>> {
    let one = |f: &Path| -> std::io::Result<Option<String>> {
        if on_drive {
            Ok(crate::sha256::file_hex_from_drive(f, &mut |_, _| {}, stop)?.map(|(_, h)| h))
        } else {
            file_hex(f, stop)
        }
    };
    let meta = std::fs::metadata(p)?;
    if !meta.is_dir() {
        return Ok(one(p)?.map(|h| (meta.len(), h)));
    }
    let mut files = Vec::new();
    tree_files(p, "", &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = Sha256::new();
    let mut total = 0u64;
    for (rel, f) in files {
        let Some(s) = one(&f)? else { return Ok(None) };
        total += std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
        h.update(format!("{rel}\t{s}\n").as_bytes());
    }
    Ok(Some((total, h.finish_hex())))
}

/// Make a file, or every file in a folder, safe on the drive. (Windows only
/// does that through a handle that may write; nothing is written.)
fn make_safe_on_drive(p: &Path) -> std::io::Result<()> {
    let sync = |f: &Path| std::fs::OpenOptions::new().write(true).open(f)?.sync_all();
    if std::fs::metadata(p)?.is_dir() {
        let mut files = Vec::new();
        tree_files(p, "", &mut files)?;
        for (_, f) in files {
            sync(&f)?;
        }
        Ok(())
    } else {
        sync(p)
    }
}

fn dir_bytes(p: &Path) -> u64 {
    let mut files = Vec::new();
    let _ = tree_files(p, "", &mut files);
    files.iter().map(|(_, f)| std::fs::metadata(f).map(|m| m.len()).unwrap_or(0)).sum()
}

fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &to)?;
        } else {
            std::fs::copy(e.path(), &to)?;
        }
    }
    Ok(())
}

fn copy_any(src: &Path, dst: &Path) -> std::io::Result<()> {
    if std::fs::metadata(src)?.is_dir() {
        copy_tree(src, dst)
    } else {
        std::fs::copy(src, dst).map(|_| ())
    }
}

/// Remove something the program itself left half-made (its own temporary copy).
fn remove_own_temp(p: &Path) {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => {
            let _ = std::fs::remove_dir_all(p);
        }
        Ok(_) => {
            let _ = std::fs::remove_file(p);
        }
        Err(_) => {}
    }
}

fn times_of(meta: &std::fs::Metadata) -> std::fs::FileTimes {
    let mut t = std::fs::FileTimes::new();
    if let Ok(m) = meta.modified() {
        t = t.set_modified(m);
    }
    if let Ok(a) = meta.accessed() {
        t = t.set_accessed(a);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTimesExt;
        if let Ok(c) = meta.created() {
            t = t.set_created(c);
        }
    }
    t
}

fn set_times(p: &Path, t: std::fs::FileTimes) {
    let f = if p.is_dir() {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::OpenOptions::new().write(true).custom_flags(0x0200_0000).open(p) // FILE_FLAG_BACKUP_SEMANTICS
        }
        #[cfg(not(windows))]
        {
            std::fs::File::open(p)
        }
    } else {
        std::fs::OpenOptions::new().write(true).open(p)
    };
    if let Ok(f) = f {
        let _ = f.set_times(t);
    }
}

/// A folder's dates, given back to it when this goes (however the work in
/// it ends): putting a file in a folder, or taking one out, changes the
/// folder's modified date.
struct KeepFolderDates {
    folder: PathBuf,
    times: Option<std::fs::FileTimes>,
}

impl KeepFolderDates {
    fn of(folder: &Path) -> KeepFolderDates {
        KeepFolderDates { folder: folder.to_path_buf(), times: std::fs::metadata(folder).ok().map(|m| times_of(&m)) }
    }
}

impl Drop for KeepFolderDates {
    fn drop(&mut self) {
        if let Some(t) = self.times.take() {
            set_times(&self.folder, t);
        }
    }
}

/// Give `to` the dates `from` has: created (on Windows), modified, last
/// opened. What the program makes of a file (a repaired version, a salvaged
/// preview, what could be read out of an archive) keeps the file's own
/// dates, so it sorts by date with everything else.
pub fn copy_dates(from: &Path, to: &Path) {
    if let Ok(m) = std::fs::metadata(from) {
        set_times(to, times_of(&m));
    }
}

/// The same for a document kept as a folder (a package): every file and
/// folder in `to` that is in `from` too, at the same place inside it, gets
/// its dates, and `to` itself the dates of `from`. A file `from` does not
/// have keeps its own. (A file's dates go to a file made of it whatever it
/// is: `from` may be a folder and `to` a file, or the other way round.)
pub fn give_dates(from: &Path, to: &Path) {
    if from.is_dir() && to.is_dir() {
        if let Ok(rd) = std::fs::read_dir(to) {
            for e in rd.flatten() {
                let f = from.join(e.file_name());
                if std::fs::symlink_metadata(&f).is_ok() {
                    give_dates(&f, &e.path());
                }
            }
        }
    }
    copy_dates(from, to);
}

// ---------------------------------------------------------------- the lists

/// One line of repair-list.csv: something copied out to be repaired.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    /// In WORK, folders separated by '\'.
    pub path: String,
    /// A document package, copied out whole.
    pub package: bool,
    pub group: String,
    pub size: u64,
    pub sha256: String,
    pub why: String,
    /// The package around it, if any (in WORK).
    pub inside: String,
    /// The copy, in REPAIR.
    pub copied_as: String,
}

const LIST_HEADER: [&str; 9] =
    ["id", "path_in_work", "kind", "repair_with", "size_bytes", "sha256", "why", "inside_package", "copied_out_as"];

pub fn read_list(repair: &Path) -> Result<Vec<Item>, String> {
    let p = repair.join(LIST_NAME);
    let f = match std::fs::File::open(&p) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("could not read {}: {e}", p.display())),
    };
    let mut out = Vec::new();
    crate::fmt::for_each_csv_row(std::io::BufReader::new(f), |r| {
        if r.len() < 9 || r[0] == "id" {
            return;
        }
        out.push(Item {
            id: r[0].clone(),
            path: r[1].clone(),
            package: r[2] == "package",
            group: r[3].clone(),
            size: r[4].parse().unwrap_or(0),
            sha256: r[5].clone(),
            why: r[6].clone(),
            inside: r[7].clone(),
            copied_as: r[8].clone(),
        });
    })
    .map_err(|e| format!("could not read {}: {e}", p.display()))?;
    Ok(out)
}

fn write_list(repair: &Path, items: &[Item]) -> Result<(), String> {
    let mut body = format!("\u{FEFF}{}", csv_row(&LIST_HEADER));
    for i in items {
        body.push_str(&csv_row(&[
            i.id.clone(),
            i.path.clone(),
            if i.package { "package" } else { "file" }.to_string(),
            i.group.clone(),
            i.size.to_string(),
            i.sha256.clone(),
            i.why.clone(),
            i.inside.clone(),
            i.copied_as.clone(),
        ]));
    }
    // Written so that a power cut leaves the old list or the new one, whole:
    // put back finds where everything belongs from this list alone.
    let p = repair.join(LIST_NAME);
    crate::sys::write_safely(&p, body.as_bytes())
        .map_err(|e| format!("could not write {} ({e}) - is it open in another program?", p.display()))
}

/// One line of repair-returns.csv (in WORK): a repaired file put back, or undone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Return {
    /// RETURNED or UNDONE.
    pub action: String,
    pub id: String,
    pub path: String,
    pub package: bool,
    pub size_before: u64,
    pub sha_before: String,
    pub size_after: u64,
    pub sha_after: String,
    pub when: String,
    pub from: String,
}

const RETURNS_HEADER: [&str; 10] = [
    "action",
    "id",
    "path_in_work",
    "kind",
    "size_before",
    "sha256_before",
    "size_after",
    "sha256_after",
    "when",
    "from",
];

pub fn read_returns(work: &Path) -> Vec<Return> {
    let mut out = Vec::new();
    let Ok(f) = std::fs::File::open(work.join(RETURNS_NAME)) else { return out };
    let _ = crate::fmt::for_each_csv_row(std::io::BufReader::new(f), |r| {
        if r.len() < 10 || r[0] == "action" {
            return;
        }
        out.push(Return {
            action: r[0].clone(),
            id: r[1].clone(),
            path: r[2].clone(),
            package: r[3] == "package",
            size_before: r[4].parse().unwrap_or(0),
            sha_before: r[5].clone(),
            size_after: r[6].parse().unwrap_or(0),
            sha_after: r[7].clone(),
            when: r[8].clone(),
            from: r[9].clone(),
        });
    });
    out
}

fn append_return(work: &Path, r: &Return) -> Result<(), String> {
    use std::io::Write;
    let p = work.join(RETURNS_NAME);
    let new = !p.exists();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .map_err(|e| format!("could not write {} ({e}) - is it open in another program?", p.display()))?;
    let mut text = if new { format!("\u{FEFF}{}", csv_row(&RETURNS_HEADER)) } else { String::new() };
    text.push_str(&csv_row(&[
        r.action.clone(),
        r.id.clone(),
        r.path.clone(),
        if r.package { "package" } else { "file" }.to_string(),
        r.size_before.to_string(),
        r.sha_before.clone(),
        r.size_after.to_string(),
        r.sha_after.clone(),
        r.when.clone(),
        r.from.clone(),
    ]));
    f.write_all(text.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| format!("could not write {}: {e}", p.display()))
}

/// What a put-back file (or package) in WORK should be now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Now {
    pub id: String,
    pub path: String,
    pub package: bool,
    pub size: u64,
    pub sha: String,
    pub original_size: u64,
    pub original_sha: String,
    /// The repaired version is in place (not undone).
    pub repaired: bool,
}

/// Every path the put-back log names, by its path in lower case.
pub fn returned_now(work: &Path) -> HashMap<String, Now> {
    let mut out: HashMap<String, Now> = HashMap::new();
    for r in read_returns(work) {
        let key = r.path.to_lowercase();
        let e = out.entry(key).or_insert_with(|| Now {
            id: r.id.clone(),
            path: r.path.clone(),
            package: r.package,
            size: r.size_before,
            sha: r.sha_before.clone(),
            original_size: r.size_before,
            original_sha: r.sha_before.clone(),
            repaired: false,
        });
        e.size = r.size_after;
        e.sha = r.sha_after.clone();
        e.repaired = r.action == "RETURNED";
    }
    out
}

// ---------------------------------------------------------------- what is damaged

/// A damaged file in WORK, and every reason given for it.
#[derive(Clone, Debug, Default)]
pub struct Damaged {
    pub path: String,
    pub why: Vec<String>,
    /// Every reason is "could not be read" (not proof of damage).
    pub only_unreadable: bool,
}

/// The copying's list: (path in WORK, size, sha256, result), by lower-case path.
fn rescue_log(work: &Path) -> HashMap<String, (String, u64, String, String)> {
    let mut out = HashMap::new();
    let Ok(f) = std::fs::File::open(work.join(crate::rescue::LOG_NAME)) else { return out };
    let _ = crate::fmt::for_each_csv_row(std::io::BufReader::new(f), |r| {
        if r.len() < 6 || !matches!(r[0].as_str(), "FILE" | "LOST DATA") {
            return;
        }
        let rel = if r[4].is_empty() { r[1].clone() } else { r[4].clone() };
        let sha = r.get(6).map(|s| s.trim().to_lowercase()).unwrap_or_default();
        out.insert(rel.to_lowercase(), (rel, r[2].parse().unwrap_or(0), sha, r[3].clone()));
    });
    out
}

/// The path a line of a list names in WORK: a full path, a path from WORK's
/// top, or one with WORK's own name in it (a Mac's "./WORK/a/b.heic"); with
/// anything after the path (" - error message") left off.
fn resolve_listed(work: &Path, line: &str) -> Option<String> {
    let s = line.trim().trim_matches('"').trim();
    if s.is_empty() || s.starts_with('#') {
        return None;
    }
    let s = s.replace('/', "\\");
    let work_name = work.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut tries: Vec<String> = Vec::new();
    if let Some(r) = relative_to(work, &s) {
        tries.push(r);
    }
    if !work_name.is_empty() {
        let low = s.to_lowercase();
        let pat = format!("\\{work_name}\\");
        if let Some(i) = low.find(&pat) {
            tries.push(s[i + pat.len()..].to_string());
        } else if let Some(rest) = low.strip_prefix(&format!("{work_name}\\")) {
            tries.push(s[s.len() - rest.len()..].to_string());
        }
    }
    tries.push(s.trim_start_matches(".\\").to_string());
    for t in tries {
        let mut cand = t.trim().to_string();
        loop {
            if !cand.is_empty() && std::fs::symlink_metadata(under(work, &cand)).is_ok() {
                return Some(cand);
            }
            let cut = [" - ", "\t", ": ", " ("].iter().filter_map(|sep| cand.rfind(sep)).max();
            match cut {
                Some(i) if i > 0 => cand = cand[..i].trim_end().to_string(),
                _ => break,
            }
        }
    }
    None
}

/// Every damaged file in WORK: from the copying's list (copied only in part),
/// the check of the copies (`scans`: its results CSV files), and any list in
/// REPAIR\damaged-lists. Also the lines of those lists that name nothing.
pub fn collect_damaged(work: &Path, scans: &[PathBuf], repair: &Path) -> (Vec<Damaged>, Vec<String>) {
    let mut by: BTreeMap<String, Damaged> = BTreeMap::new();
    let mut add = |rel: &str, why: String, unreadable: bool| {
        let e = by.entry(rel.to_lowercase()).or_insert_with(|| Damaged {
            path: rel.to_string(),
            why: Vec::new(),
            only_unreadable: true,
        });
        if !e.why.contains(&why) {
            e.why.push(why);
        }
        e.only_unreadable &= unreadable;
    };
    for (rel, _, _, result) in rescue_log(work).values() {
        if result == "partly copied" {
            add(rel, "copied only in part (some of it could not be read)".to_string(), false);
        }
    }
    for scan in scans {
        let Ok(f) = std::fs::File::open(scan) else { continue };
        let _ = crate::fmt::for_each_csv_row(std::io::BufReader::new(f), |r| {
            if r.len() < 4 || r[0] == "verdict" {
                return;
            }
            let v = r[0].as_str();
            if !(crate::formats::is_damage(v) || v == crate::join::VERDICT) {
                return;
            }
            let Some(rel) = relative_to(work, &r[3]) else { return };
            let mut detail: String = r[1].chars().take(160).collect();
            if r[1].chars().count() > 160 {
                detail.push('…');
            }
            add(&rel, if detail.is_empty() { v.to_string() } else { format!("{v}: {detail}") }, v == "UNREADABLE");
        });
    }
    let mut unmatched = Vec::new();
    if let Ok(rd) = std::fs::read_dir(repair.join(LISTS)) {
        // (Not a Mac's "._sips-failures.txt" beside the list, or .DS_Store.)
        let mut lists: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| !is_housekeeping(&e.file_name().to_string_lossy()))
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        lists.sort();
        for l in lists {
            let name = l.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let Ok(bytes) = std::fs::read(&l) else { continue };
            let text = String::from_utf8_lossy(&bytes).replace('\u{FEFF}', "");
            for line in text.lines() {
                let t = line.trim();
                if t.is_empty() || t.starts_with('#') {
                    continue;
                }
                match resolve_listed(work, t) {
                    Some(rel) => add(&rel, format!("on the list {name}"), false),
                    None => unmatched.push(format!("{name}: {t}")),
                }
            }
        }
    }
    (by.into_values().collect(), unmatched)
}

// ---------------------------------------------------------------- 1. isolate

#[derive(Debug, Default)]
pub struct Isolated {
    pub repair: PathBuf,
    /// Copied out this time.
    pub copied: Vec<Item>,
    /// Copied out before (or already repaired and put back).
    pub already: usize,
    /// Not copied out: (path in WORK, why).
    pub left: Vec<(String, &'static str)>,
    /// Could not be copied out: (path in WORK, why).
    pub problems: Vec<(String, String)>,
    /// Lines of REPAIR\damaged-lists files that name no file in WORK.
    pub unmatched: Vec<String>,
    pub stopped: bool,
}

const README: &str = "This folder is for repairing the damaged files, without ever changing the copies in WORK by hand.

1. ISOLATE (Automatic Recovery Probe does it by itself; or step 8): every damaged file is COPIED here, into to-repair, sorted
   into folders by what can repair it. Each copy has a number in its name, like
   \"IMG_0003 [R0012].JPG\". repair-list.csv says exactly where R0012 is in WORK.

2. REPAIR: repair the copies in to-repair (never the files in WORK), each with the tool its
   folder names. Save every repaired file anywhere in the folder \"repaired\", and keep its
   number in its name: \"IMG_0003 [R0012].JPG\", or \"IMG_0003 [R0012]_repaired.JPG\" - both
   are fine. If a tool drops the number, add it back by hand. A repaired file must be the
   same kind as the original (a HEIC stays a HEIC). What can't take the original's place - a
   TIFF made from a damaged RAW photo, what 7-Zip could take out of a damaged archive - goes
   in the folder \"salvaged\" instead: it is kept with the rest, and the original stays in
   WORK as it is. (The small \"._\" files a Mac adds beside each file on a USB stick may come
   along into \"repaired\": they are ignored.)

3. PUT BACK (Automatic Recovery Probe does it by itself; or step 8): each repaired file goes back to exactly where its
   original is in WORK, with the original's dates - but only if the file there is still
   the damaged original. The original moves into the folder \"originals\" here (the same
   folders as in WORK): nothing is deleted. WORK\\repair-returns.csv lists every file put
   back, so checking the copies later knows about it. A put-back can be undone.

4. EVALUATE (Automatic Recovery Probe does it by itself; or step 8): what is damaged inside each package (a Photos library,
   an app, a Pages document...), what has been put back, and what is still damaged:
   evaluation.csv.

Packages are never split. A damaged photo, video or sound file inside a library is copied
out and put back to the same place inside it. A Pages, Numbers or Keynote document kept as
a folder goes out and back whole. A library's own database, and anything inside an app, is
not copied out: the library's own app repairs it whole, on a copy on the Mac; an app is
reinstalled. Housekeeping files (.DS_Store, ._ files, Thumbs.db, desktop.ini) stay where
they are.

Other lists of damaged files (Krokiet's, 7-Zip's, sips-failures.txt from the Mac): put them
in the folder \"damaged-lists\", one file per line, and copy the damaged files out again.

The folders in to-repair, and what repairs what is in them:
";

fn write_readme(repair: &Path) {
    let mut t = README.to_string();
    for (i, (folder, _)) in GROUPS.iter().enumerate() {
        t.push_str(&format!("   {folder}: {}\n", GROUP_HELP[i]));
    }
    t.push_str(&format!("   {OTHER_GROUP}: open each in its own program; ask for help\n"));
    let _ = std::fs::write(repair.join(README_NAME), t.replace('\n', "\r\n"));
}

/// Step 1: copy every damaged file out to be repaired. Running it again
/// copies out only what is new.
pub fn isolate(
    work: &Path,
    scans: &[PathBuf],
    progress: &mut dyn FnMut(&str),
    stop: &dyn Fn() -> bool,
) -> Result<Isolated, String> {
    if !work.join(crate::rescue::LOG_NAME).is_file() {
        return Err(format!(
            "{} has no {}: choose the folder the program copied everything into (such as E:\\WORK)",
            work.display(),
            crate::rescue::LOG_NAME
        ));
    }
    let repair = repair_dir(work)?;
    for d in [TO_REPAIR, REPAIRED, ORIGINALS, LISTS, SALVAGED] {
        std::fs::create_dir_all(repair.join(d)).map_err(|e| format!("could not create {}: {e}", repair.join(d).display()))?;
    }
    write_readme(&repair);
    let mut o = Isolated { repair: repair.clone(), ..Default::default() };
    let log = rescue_log(work);
    let (damaged, unmatched) = collect_damaged(work, scans, &repair);
    o.unmatched = unmatched;
    let mut items = read_list(&repair)?;
    let known: HashSet<String> = items.iter().map(|i| i.path.to_lowercase()).collect();
    let mut next = items.iter().map(|i| id_number(&i.id)).max().unwrap_or(0) + 1;
    let returned = returned_now(work);
    let mut done_units: HashSet<String> = HashSet::new();
    progress(&format!("{} damaged files to look at", commas(damaged.len() as u64)));
    for d in &damaged {
        if stop() {
            o.stopped = true;
            break;
        }
        let around = package_around_on_disk(work, &d.path);
        let (unit, package, group) = match plan_for(&d.path, around.as_ref(), d.only_unreadable) {
            Plan::Leave(why) => {
                o.left.push((d.path.clone(), why));
                continue;
            }
            Plan::Isolate { unit, package, group } => (unit, package, group),
        };
        let key = unit.to_lowercase();
        if !done_units.insert(key.clone()) {
            continue;
        }
        if known.contains(&key) || returned.get(&key).is_some_and(|n| n.repaired) {
            o.already += 1;
            continue;
        }
        let src = under(work, &unit);
        let (bytes, sha) = if package {
            let b = dir_bytes(&src);
            if b > PACKAGE_LIMIT {
                o.left.push((unit.clone(), LEAVE_TOO_BIG));
                continue;
            }
            match hash_of(&src, stop) {
                Ok(Some(h)) => h,
                Ok(None) => {
                    o.stopped = true;
                    break;
                }
                Err(e) => {
                    o.problems.push((unit.clone(), format!("could not read it: {e}")));
                    continue;
                }
            }
        } else {
            match hash_of(&src, stop) {
                Ok(Some(h)) => h,
                Ok(None) => {
                    o.stopped = true;
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    o.problems.push((unit.clone(), "it is not in WORK".to_string()));
                    continue;
                }
                Err(e) => {
                    o.problems.push((unit.clone(), format!("could not read it: {e}")));
                    continue;
                }
            }
        };
        if !package {
            if let Some((_, _, logged, _)) = log.get(&key) {
                if logged.len() == 64 && *logged != sha {
                    o.problems.push((
                        unit.clone(),
                        "it is not what was copied any more (step 7's \"Check the copies are unchanged\" says more): not \
                         copied out"
                            .to_string(),
                    ));
                    continue;
                }
            }
        }
        let id = id_of(next);
        let name = unit.rsplit('\\').next().unwrap_or(&unit);
        let copied_as = format!("{TO_REPAIR}\\{group}\\{}", tagged(name, &id));
        let dest = under(&repair, &copied_as);
        if let Some(p) = dest.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        remove_own_temp(&dest);
        if let Err(e) = copy_any(&src, &dest) {
            o.problems.push((unit.clone(), format!("could not copy it out: {e}")));
            remove_own_temp(&dest);
            continue;
        }
        match hash_of(&dest, stop) {
            Ok(Some((_, h))) if h == sha => {}
            Ok(None) => {
                o.stopped = true;
                break;
            }
            _ => {
                o.problems.push((unit.clone(), "its copy did not read back the same: not copied out".to_string()));
                remove_own_temp(&dest);
                continue;
            }
        }
        next += 1;
        let item = Item {
            id,
            path: unit.clone(),
            package,
            group: group.to_string(),
            size: bytes,
            sha256: sha,
            why: d.why.join("; "),
            inside: around.as_ref().map(|a| a.0.clone()).unwrap_or_default(),
            copied_as,
        };
        progress(&format!("{}  {}", item.id, item.path));
        o.copied.push(item.clone());
        items.push(item);
    }
    items.sort_by_key(|i| id_number(&i.id));
    write_list(&repair, &items)?;
    Ok(o)
}

pub fn isolate_summary(o: &Isolated) -> Vec<String> {
    let mut t = Vec::new();
    if o.copied.is_empty() {
        t.push(if o.already > 0 {
            format!("Nothing new to copy out: {} damaged files were copied out before.", commas(o.already as u64))
        } else {
            "No damaged files to copy out.".to_string()
        });
    } else {
        let mut per: BTreeMap<&str, u64> = BTreeMap::new();
        for i in &o.copied {
            *per.entry(i.group.as_str()).or_default() += 1;
        }
        t.push(format!(
            "Copied {} damaged files out into {}, each with its number ([R…]) in its name; repair-list.csv says where each \
             belongs. Nothing in WORK was changed.",
            commas(o.copied.len() as u64),
            o.repair.join(TO_REPAIR).display()
        ));
        for (g, n) in per {
            t.push(format!("  {g}: {}", commas(n)));
        }
        if o.already > 0 {
            t.push(format!("{} were copied out before and are left as they are.", commas(o.already as u64)));
        }
    }
    if !o.left.is_empty() {
        let mut per: BTreeMap<&str, u64> = BTreeMap::new();
        for (_, why) in &o.left {
            *per.entry(why).or_default() += 1;
        }
        t.push(format!("Not copied out ({}):", commas(o.left.len() as u64)));
        for (why, n) in per {
            t.push(format!("  {}: {why}", commas(n)));
        }
    }
    for (p, why) in o.problems.iter().take(10) {
        t.push(format!("Could not copy out {p}: {why}"));
    }
    if o.problems.len() > 10 {
        t.push(format!("...and {} more that could not be copied out.", o.problems.len() - 10));
    }
    if !o.unmatched.is_empty() {
        t.push(format!(
            "{} lines of the lists in damaged-lists name no file in WORK, for example: {}",
            commas(o.unmatched.len() as u64),
            o.unmatched.iter().take(3).cloned().collect::<Vec<_>>().join(" | ")
        ));
    }
    if o.stopped {
        t.push("Stopped before the end: run it again to carry on.".to_string());
    }
    t
}

// ---------------------------------------------------------------- 3. put back

#[derive(Debug, Default)]
pub struct PutBack {
    /// Put back this time: (id, path in WORK, from).
    pub returned: Vec<(String, String, String)>,
    /// Already back as it is now.
    pub already: usize,
    /// Not put back: (what, why).
    pub skipped: Vec<(String, String)>,
    /// Copied out and not repaired yet (nothing for them in "repaired").
    pub waiting: usize,
    pub stopped: bool,
}

/// The repaired versions in REPAIR\repaired: (id, path, its name), and
/// what in there has no number.
fn candidates(repair: &Path) -> (Vec<(String, PathBuf, String)>, Vec<String>) {
    let mut found = Vec::new();
    let mut untagged = Vec::new();
    let root = repair.join(REPAIRED);
    let mut stack = vec![(root.clone(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let here = if rel.is_empty() { name.clone() } else { format!("{rel}\\{name}") };
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            // What a Mac writes beside every file on a USB stick ("._IMG_0003
            // [R0012].HEIC", .DS_Store), and the like: never a repaired file.
            if if is_dir { is_drive_housekeeping(&name) } else { is_housekeeping(&name) } {
                continue;
            }
            match tag_in(&name) {
                Some(id) => found.push((id, e.path(), format!("{REPAIRED}\\{here}"))),
                None if is_dir => stack.push((e.path(), here)),
                None => untagged.push(format!("{REPAIRED}\\{here}")),
            }
        }
    }
    found.sort_by(|a, b| a.2.cmp(&b.2));
    (found, untagged)
}

fn same_kind(original: &str, candidate: &str) -> bool {
    ext_of(original).eq_ignore_ascii_case(ext_of(candidate))
}

/// Step 3: put every repaired file back where its original is.
pub fn put_back(work: &Path, progress: &mut dyn FnMut(&str), stop: &dyn Fn() -> bool) -> Result<PutBack, String> {
    let repair = repair_dir(work)?;
    let items = read_list(&repair)?;
    if items.is_empty() {
        return Err("Nothing has been copied out to repair yet: copy the damaged files out first.".to_string());
    }
    let by_id: HashMap<&str, &Item> = items.iter().map(|i| (i.id.as_str(), i)).collect();
    let mut o = PutBack::default();
    let (found, untagged) = candidates(&repair);
    for u in untagged {
        o.skipped.push((u, "it has no number ([R…]) in its name, so where it belongs is not known".to_string()));
    }
    let mut per_id: BTreeMap<String, Vec<(PathBuf, String)>> = BTreeMap::new();
    for (id, p, shown) in found {
        per_id.entry(id).or_default().push((p, shown));
    }
    let mut seen: HashSet<String> = HashSet::new();
    for (id, cands) in &per_id {
        if stop() {
            o.stopped = true;
            break;
        }
        seen.insert(id.clone());
        let Some(item) = by_id.get(id.as_str()).copied() else {
            for (_, shown) in cands {
                o.skipped.push((shown.clone(), format!("{id} is not in repair-list.csv")));
            }
            continue;
        };
        if cands.len() > 1 {
            o.skipped.push((
                id.clone(),
                format!(
                    "more than one repaired version ({}): leave only one in \"repaired\"",
                    cands.iter().map(|c| c.1.as_str()).collect::<Vec<_>>().join(", ")
                ),
            ));
            continue;
        }
        let (cand, shown) = &cands[0];
        let name = item.path.rsplit('\\').next().unwrap_or(&item.path).to_string();
        let cand_name = cand.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !same_kind(&name, &cand_name) {
            o.skipped.push((
                shown.clone(),
                format!(
                    "it is a .{} file, and {} is a .{} file: not put back (save it as a .{} to put it back, or keep it in \"{SALVAGED}\")",
                    ext_of(&cand_name),
                    name,
                    ext_of(&name),
                    ext_of(&name)
                ),
            ));
            continue;
        }
        if !item.package && cand.is_dir() {
            o.skipped.push((shown.clone(), format!("it is a folder, and {name} is a file: not put back")));
            continue;
        }
        match put_one(work, &repair, item, cand, shown, stop) {
            Ok(Some(true)) => {
                progress(&format!("Put back {}  {}", item.id, item.path));
                o.returned.push((item.id.clone(), item.path.clone(), shown.clone()));
            }
            Ok(Some(false)) => o.already += 1,
            Ok(None) => {
                o.stopped = true;
                break;
            }
            Err(why) => o.skipped.push((shown.clone(), why)),
        }
    }
    let back = returned_now(work);
    o.waiting = items
        .iter()
        .filter(|i| !seen.contains(&i.id) && !back.get(&i.path.to_lowercase()).is_some_and(|n| n.repaired))
        .count();
    Ok(o)
}

/// Put one repaired version back. Some(true): done; Some(false): it is
/// already there; None: stopped.
fn put_one(
    work: &Path,
    repair: &Path,
    item: &Item,
    cand: &Path,
    shown: &str,
    stop: &dyn Fn() -> bool,
) -> Result<Option<bool>, String> {
    let read = |p: &Path| hash_of(p, stop).map_err(|e| format!("could not read {}: {e}", p.display()));
    let Some((cand_size, cand_sha)) = read(cand)? else { return Ok(None) };
    if cand_sha == item.sha256 {
        return Err(format!(
            "it is exactly the damaged original of {}: nothing was repaired, so there is nothing to put back",
            item.path
        ));
    }
    let target = under(work, &item.path);
    let kept = under(&repair.join(ORIGINALS), &item.path);
    let state = returned_now(work).remove(&item.path.to_lowercase());
    let last_returned = state.as_ref().filter(|n| n.repaired).map(|n| n.sha.clone());
    let current = match std::fs::symlink_metadata(&target) {
        Ok(_) => match read(&target)? {
            Some(h) => Some(h),
            None => return Ok(None),
        },
        Err(_) => None,
    };
    // What happens to what is in WORK now.
    enum Then {
        KeepOriginal,
        SetAside,
        Nothing,
    }
    let (then, size_before, sha_before) = match &current {
        Some((_, sha)) if *sha == cand_sha && last_returned.as_deref() == Some(cand_sha.as_str()) => {
            return Ok(Some(false))
        }
        Some((size, sha)) if *sha == cand_sha => {
            // In place, but not in repair-returns.csv: a put-back the PC never
            // finished (it stopped after putting it in place, before listing
            // it). Listed now - if the original is where that put-back moved
            // it; if not, the program did not put it there.
            match read(&kept) {
                Ok(Some((_, s))) if s == item.sha256 => {}
                Ok(None) => return Ok(None),
                _ => {
                    return Err(format!(
                        "{} in WORK is already this repaired version, but not put there by the program (the damaged \
                         original is not in {ORIGINALS}): it was replaced by hand. Ask before going on.",
                        item.path
                    ))
                }
            }
            let (size_before, sha_before) = match &state {
                Some(n) if n.repaired => (n.size, n.sha.clone()),
                _ => (item.size, item.sha256.clone()),
            };
            append_return(
                work,
                &Return {
                    action: "RETURNED".to_string(),
                    id: item.id.clone(),
                    path: item.path.clone(),
                    package: item.package,
                    size_before,
                    sha_before,
                    size_after: *size,
                    sha_after: cand_sha,
                    when: now_text(),
                    from: format!("{shown} (a put-back the PC never finished, listed now)"),
                },
            )?;
            return Ok(Some(true));
        }
        Some((size, sha)) if *sha == item.sha256 => (Then::KeepOriginal, *size, sha.clone()),
        Some((size, sha)) if last_returned.as_ref() == Some(sha) => (Then::SetAside, *size, sha.clone()),
        Some(_) => {
            return Err(format!(
                "{} in WORK has changed since it was copied out, so it was not replaced",
                item.path
            ))
        }
        None => match read(&kept) {
            // Moved out by a put-back that never finished (the PC stopped).
            Ok(Some((size, sha))) if sha == item.sha256 => (Then::Nothing, size, sha),
            Ok(None) => return Ok(None),
            _ => return Err(format!("{} is not in WORK", item.path)),
        },
    };
    let times = std::fs::metadata(&target).or_else(|_| std::fs::metadata(&kept)).ok().map(|m| times_of(&m));
    let Some(parent) = target.parent().map(|p| p.to_path_buf()) else {
        return Err(format!("{} has no folder", item.path));
    };
    let _folder_dates = KeepFolderDates::of(&parent);
    // The repaired version, next to where it goes, checked.
    let tmp = parent.join(TMP_NAME);
    remove_own_temp(&tmp);
    copy_any(cand, &tmp).map_err(|e| {
        remove_own_temp(&tmp);
        format!("could not copy it into WORK: {e}")
    })?;
    // Safe on the drive before it takes the original's place (a power cut
    // must not leave a version that never reached the drive in WORK), and
    // checked as the drive holds it.
    if let Err(e) = make_safe_on_drive(&tmp) {
        remove_own_temp(&tmp);
        return Err(format!("could not make its copy in WORK safe on the drive: {e}"));
    }
    match hash_on_drive(&tmp, stop) {
        Ok(Some((_, h))) if h == cand_sha => {}
        Ok(None) => {
            remove_own_temp(&tmp);
            return Ok(None);
        }
        _ => {
            remove_own_temp(&tmp);
            return Err("its copy in WORK did not read back the same: not put back".to_string());
        }
    }
    // What is there now moves out of the way (a rename: the same drive).
    let moved_to = match then {
        Then::KeepOriginal => {
            if let Some(p) = kept.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            if std::fs::symlink_metadata(&kept).is_ok() {
                let aside = repair.join(REPLACED).join(format!(
                    "{} {} {}",
                    item.id,
                    crate::fmt::now_stamp(),
                    kept.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                ));
                let _ = std::fs::create_dir_all(repair.join(REPLACED));
                std::fs::rename(&kept, &aside).map_err(|e| format!("could not move aside {}: {e}", kept.display()))?;
            }
            Some(kept.clone())
        }
        Then::SetAside => {
            let _ = std::fs::create_dir_all(repair.join(REPLACED));
            Some(repair.join(REPLACED).join(format!(
                "{} {} {}",
                item.id,
                crate::fmt::now_stamp(),
                target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            )))
        }
        Then::Nothing => None,
    };
    if let Some(to) = &moved_to {
        if let Err(e) = std::fs::rename(&target, to) {
            remove_own_temp(&tmp);
            return Err(format!(
                "could not move the file in WORK out of the way ({e}); REPAIR must be on the same drive as WORK"
            ));
        }
    }
    if let Err(e) = std::fs::rename(&tmp, &target) {
        if let Some(to) = &moved_to {
            let _ = std::fs::rename(to, &target);
        }
        remove_own_temp(&tmp);
        return Err(format!("could not put it in place: {e}"));
    }
    if !item.package {
        if let Some(t) = times {
            set_times(&target, t);
        }
    } else if std::fs::symlink_metadata(&kept).is_ok() {
        // A package: everything in it that the original has, its dates.
        give_dates(&kept, &target);
    }
    append_return(
        work,
        &Return {
            action: "RETURNED".to_string(),
            id: item.id.clone(),
            path: item.path.clone(),
            package: item.package,
            size_before,
            sha_before,
            size_after: cand_size,
            sha_after: cand_sha,
            when: now_text(),
            from: shown.to_string(),
        },
    )?;
    Ok(Some(true))
}

pub fn put_back_summary(o: &PutBack, repair: &Path) -> Vec<String> {
    let mut t = Vec::new();
    if o.returned.is_empty() {
        t.push("No repaired files to put back.".to_string());
    } else {
        t.push(format!(
            "Put back {} repaired files, each exactly where its original was in WORK, with the original's dates. The \
             originals are in {}; repair-returns.csv in WORK lists every one.",
            commas(o.returned.len() as u64),
            repair.join(ORIGINALS).display()
        ));
    }
    if o.already > 0 {
        t.push(format!("{} repaired files were already back.", commas(o.already as u64)));
    }
    if o.waiting > 0 {
        t.push(format!(
            "{} files copied out are not repaired yet (nothing for them in \"repaired\").",
            commas(o.waiting as u64)
        ));
    }
    for (what, why) in o.skipped.iter().take(12) {
        t.push(format!("Not put back: {what}: {why}"));
    }
    if o.skipped.len() > 12 {
        t.push(format!("...and {} more not put back.", o.skipped.len() - 12));
    }
    if o.stopped {
        t.push("Stopped before the end: run it again to carry on.".to_string());
    }
    t
}

/// Undo a put-back: the original goes back into WORK, and the repaired
/// version moves to REPAIR\replaced.
pub fn undo(work: &Path, id: &str) -> Result<String, String> {
    let typed = id.trim().trim_matches(['[', ']']);
    let typed = if typed.starts_with(['R', 'r']) { typed.to_string() } else { format!("R{typed}") };
    let id = tag_in(&format!("[{typed}]")).ok_or(format!("{id:?} is not a number like R0012"))?;
    let repair = repair_dir(work)?;
    let items = read_list(&repair)?;
    let item = items.iter().find(|i| i.id == id).ok_or(format!("{id} is not in repair-list.csv"))?;
    let state = returned_now(work).remove(&item.path.to_lowercase());
    let Some(now) = state.filter(|n| n.repaired) else {
        return Err(format!("{id} has not been put back, so there is nothing to undo"));
    };
    let no_stop = || false;
    let target = under(work, &item.path);
    let kept = under(&repair.join(ORIGINALS), &item.path);
    match hash_of(&target, &no_stop) {
        Ok(Some((_, h))) if h == now.sha => {}
        _ => return Err(format!("{} in WORK is not the version that was put back, so it was left as it is", item.path)),
    }
    let (size_orig, sha_orig) = match hash_of(&kept, &no_stop) {
        Ok(Some((s, h))) if h == item.sha256 => (s, h),
        _ => return Err(format!("the original is not in {} as it was, so it was left as it is", kept.display())),
    };
    let _ = std::fs::create_dir_all(repair.join(REPLACED));
    let aside = repair.join(REPLACED).join(format!(
        "{} {} {}",
        id,
        crate::fmt::now_stamp(),
        target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    let _folder_dates = target.parent().map(KeepFolderDates::of);
    std::fs::rename(&target, &aside).map_err(|e| format!("could not move the repaired version out of WORK: {e}"))?;
    if let Err(e) = std::fs::rename(&kept, &target) {
        let _ = std::fs::rename(&aside, &target);
        return Err(format!("could not put the original back: {e}"));
    }
    append_return(
        work,
        &Return {
            action: "UNDONE".to_string(),
            id: id.clone(),
            path: item.path.clone(),
            package: item.package,
            size_before: now.size,
            sha_before: now.sha.clone(),
            size_after: size_orig,
            sha_after: sha_orig,
            when: now_text(),
            from: format!("{ORIGINALS}\\{}", item.path),
        },
    )?;
    Ok(format!(
        "{id}: the original of {} is back in WORK; the repaired version is in {}.",
        item.path,
        aside.display()
    ))
}

// ---------------------------------------------------------------- 4. evaluate

#[derive(Clone, Debug)]
pub struct PackageReport {
    pub path: String,
    pub kind: Kind,
    pub what: &'static str,
    pub files: u64,
    pub bytes: u64,
    pub damaged: u64,
    pub media: u64,
    pub core: u64,
    pub previews: u64,
    pub put_back: u64,
    pub waiting: u64,
    pub verdict: String,
}

#[derive(Debug, Default)]
pub struct Evaluation {
    pub packages: Vec<PackageReport>,
    pub intact_packages: u64,
    /// Damaged files outside any package: (path, state).
    pub files: Vec<(String, String)>,
    pub put_back: u64,
    pub waiting: u64,
    pub left: u64,
    pub written: Option<PathBuf>,
    pub stopped: bool,
}

const STATE_BACK: &str = "repaired and put back";
const STATE_WAITING: &str = "copied out, waiting to be repaired";
const STATE_NOT_YET: &str = "not copied out yet: copy the damaged files out";
const STATE_AS_BEFORE: &str =
    "left as it was: every byte of it was read from the old drive, so it is exactly as it was on it (a checking tool found \
     something in it that was so already)";

fn verdict_for(p: &PackageReport) -> String {
    if p.damaged == 0 {
        return "nothing inside it is damaged".to_string();
    }
    match p.kind {
        Kind::Program => format!(
            "{} of its {} files are damaged: reinstall it (a program cannot be repaired, and nothing of yours is inside)",
            commas(p.damaged),
            commas(p.files)
        ),
        Kind::DiskImage => format!(
            "{} of its files are damaged: check a copy of the whole thing on the Mac (Disk Utility, or its own app)",
            commas(p.damaged)
        ),
        Kind::Document => {
            if p.put_back > 0 {
                "repaired and put back".to_string()
            } else if p.waiting > 0 {
                "copied out whole, waiting to be repaired (open it on the Mac)".to_string()
            } else {
                "damaged: copy the damaged files out, then repair it on the Mac".to_string()
            }
        }
        Kind::Library => {
            let mut parts = Vec::new();
            if p.core > 0 {
                parts.push(format!(
                    "{} of its own files are damaged (its database or project files): repair the whole library with its own \
                     app, on a copy on the Mac",
                    commas(p.core)
                ));
            }
            if p.media > 0 {
                let still = p.media.saturating_sub(p.put_back);
                parts.push(if still == 0 {
                    format!("all {} damaged photos, videos and sound files in it are repaired and put back", commas(p.media))
                } else {
                    format!(
                        "{} of its photos, videos and sound files are damaged; {} repaired and put back, {} still to repair",
                        commas(p.media),
                        commas(p.put_back),
                        commas(still)
                    )
                });
            }
            if p.previews > 0 && p.core == 0 && p.media == 0 {
                parts.push("only previews are damaged, and its own app makes them again: nothing to do".to_string());
            } else if p.previews > 0 {
                parts.push(format!("{} previews (its app makes them again)", commas(p.previews)));
            }
            parts.join("; ")
        }
    }
}

/// Step 4: what is damaged in every package, and in every file outside one;
/// what has been put back, and what is still damaged. Written to
/// REPAIR\evaluation.csv.
pub fn evaluate(
    work: &Path,
    scans: &[PathBuf],
    progress: &mut dyn FnMut(&str),
    stop: &dyn Fn() -> bool,
) -> Result<Evaluation, String> {
    let repair = repair_dir(work)?;
    let mut ev = Evaluation::default();
    let (mut damaged, _) = collect_damaged(work, scans, &repair);
    let items = read_list(&repair)?;
    let isolated: HashSet<String> = items.iter().map(|i| i.path.to_lowercase()).collect();
    // A file copied out to repair is one of the damaged files, even when the
    // latest check no longer finds anything wrong with it (because what was
    // put back in its place checks out).
    {
        let have: HashSet<String> = damaged.iter().map(|d| d.path.to_lowercase()).collect();
        for i in items.iter().filter(|i| !i.package && !have.contains(&i.path.to_lowercase())) {
            damaged.push(Damaged { path: i.path.clone(), why: vec![i.why.clone()], only_unreadable: false });
        }
    }
    let back = returned_now(work);
    // Read whole from the old drive, and found only by a checking tool: the
    // automatic repairs left those as they were (their log says so).
    let as_before: HashSet<String> = crate::autorepair::read_log(&repair)
        .into_iter()
        .filter(|a| a.fate != crate::autorepair::Fate::Repaired && a.how.contains(crate::autorepair::READ_WHOLE))
        .map(|a| a.path.to_lowercase())
        .collect();
    // Every package in WORK, with its size (the outermost one, where one is
    // inside another).
    progress("Looking for packages (libraries, apps, documents kept as folders) in WORK...");
    let mut pkgs: HashMap<String, PackageReport> = HashMap::new();
    let base = crate::rescue::verbatim(work);
    let base_s = base.to_string_lossy().into_owned();
    let mut halted = false;
    crate::walk::walk(&base_s, |dirpath, dirnames, filenames| {
        if stop() {
            halted = true;
            return false;
        }
        dirnames.retain(|d| !crate::formats::SYSTEM_DIRS.contains(&d.as_str()));
        let rel = relative_to(work, dirpath).unwrap_or_default();
        if rel.is_empty() {
            return true;
        }
        let low = rel.to_lowercase();
        // The package this folder is in (or is), looked up by each folder above it.
        let mut owner: Option<String> = None;
        let mut prefix = String::new();
        for part in low.split('\\') {
            if !prefix.is_empty() {
                prefix.push('\\');
            }
            prefix.push_str(part);
            if pkgs.contains_key(&prefix) {
                owner = Some(prefix.clone());
                break;
            }
        }
        if owner.is_none() {
            let name = rel.rsplit('\\').next().unwrap_or(&rel);
            let found = package_of_name(name).or_else(|| {
                (!crate::walk::looks_like_library(dirpath, dirnames).is_empty()).then_some((Kind::Library, "Photos library"))
            });
            if let Some((kind, what)) = found {
                pkgs.insert(
                    low.clone(),
                    PackageReport {
                        path: rel.clone(),
                        kind,
                        what,
                        files: 0,
                        bytes: 0,
                        damaged: 0,
                        media: 0,
                        core: 0,
                        previews: 0,
                        put_back: 0,
                        waiting: 0,
                        verdict: String::new(),
                    },
                );
                owner = Some(low.clone());
            }
        }
        if let Some(p) = owner.and_then(|k| pkgs.get_mut(&k)) {
            p.files += filenames.len() as u64;
            p.bytes += filenames
                .iter()
                .map(|f| std::fs::metadata(Path::new(dirpath).join(f)).map(|m| m.len()).unwrap_or(0))
                .sum::<u64>();
        }
        true
    });
    if halted {
        ev.stopped = true;
        return Ok(ev);
    }
    // Every damaged file, against its package.
    let state_of = |unit: &str| -> &'static str {
        let key = unit.to_lowercase();
        if back.get(&key).is_some_and(|n| n.repaired) {
            STATE_BACK
        } else if as_before.contains(&key) {
            STATE_AS_BEFORE
        } else if isolated.contains(&key) {
            STATE_WAITING
        } else {
            STATE_NOT_YET
        }
    };
    let mut file_rows: Vec<(String, String)> = Vec::new();
    for d in &damaged {
        let low = d.path.to_lowercase();
        let mut pkg_key: Option<String> = None;
        let mut prefix = String::new();
        let parts: Vec<&str> = low.split('\\').collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            if !prefix.is_empty() {
                prefix.push('\\');
            }
            prefix.push_str(part);
            if pkgs.contains_key(&prefix) {
                pkg_key = Some(prefix.clone());
                break;
            }
        }
        let around = pkg_key.as_ref().and_then(|k| pkgs.get(k)).map(|p| (p.path.clone(), p.kind, p.what));
        let plan = plan_for(&d.path, around.as_ref(), d.only_unreadable);
        let state: String = match &plan {
            Plan::Isolate { unit, .. } => state_of(unit).to_string(),
            Plan::Leave(why) => why.to_string(),
        };
        match &plan {
            Plan::Isolate { .. } if state == STATE_BACK => ev.put_back += 1,
            Plan::Isolate { .. } if state == STATE_AS_BEFORE => ev.left += 1,
            Plan::Isolate { .. } => ev.waiting += 1,
            Plan::Leave(_) => ev.left += 1,
        }
        if let Some(k) = pkg_key {
            let p = pkgs.get_mut(&k).unwrap();
            p.damaged += 1;
            if p.kind == Kind::Library {
                match member_role(&d.path[p.path.len().min(d.path.len())..]) {
                    Role::Media => p.media += 1,
                    Role::Core => p.core += 1,
                    Role::Previews => p.previews += 1,
                }
            }
            if state == STATE_BACK {
                p.put_back += 1;
            } else if state == STATE_WAITING {
                p.waiting += 1;
            }
        } else {
            file_rows.push((d.path.clone(), state));
        }
    }
    let mut reports: Vec<PackageReport> = pkgs.into_values().collect();
    for p in &mut reports {
        p.verdict = verdict_for(p);
    }
    ev.intact_packages = reports.iter().filter(|p| p.damaged == 0).count() as u64;
    reports.sort_by(|a, b| (b.damaged > 0).cmp(&(a.damaged > 0)).then(a.path.to_lowercase().cmp(&b.path.to_lowercase())));
    // The list.
    let mut body = csv_row(&[
        "kind",
        "path_in_work",
        "what_it_is",
        "files",
        "size_bytes",
        "damaged",
        "damaged_photos_videos_sound",
        "damaged_own_data",
        "damaged_previews",
        "put_back",
        "copied_out_waiting",
        "verdict",
    ]);
    for p in &reports {
        body.push_str(&csv_row(&[
            "PACKAGE".to_string(),
            p.path.clone(),
            p.what.to_string(),
            p.files.to_string(),
            p.bytes.to_string(),
            p.damaged.to_string(),
            p.media.to_string(),
            p.core.to_string(),
            p.previews.to_string(),
            p.put_back.to_string(),
            p.waiting.to_string(),
            p.verdict.clone(),
        ]));
    }
    for (path, state) in &file_rows {
        body.push_str(&csv_row(&[
            "FILE".to_string(),
            path.clone(),
            String::new(),
            "1".to_string(),
            String::new(),
            "1".to_string(),
            String::new(),
            String::new(),
            String::new(),
            (state == STATE_BACK).then_some("1").unwrap_or("0").to_string(),
            (state == STATE_WAITING).then_some("1").unwrap_or("0").to_string(),
            state.clone(),
        ]));
    }
    let _ = std::fs::create_dir_all(&repair);
    let out = repair.join(EVALUATION_NAME);
    std::fs::write(&out, format!("\u{FEFF}{body}"))
        .map_err(|e| format!("could not write {} ({e}) - is it open in another program?", out.display()))?;
    ev.written = Some(out);
    ev.packages = reports;
    ev.files = file_rows;
    Ok(ev)
}

pub fn evaluation_summary(ev: &Evaluation) -> Vec<String> {
    let mut t = Vec::new();
    let damaged_pkgs: Vec<&PackageReport> = ev.packages.iter().filter(|p| p.damaged > 0).collect();
    let total = ev.put_back + ev.waiting + ev.left;
    if total == 0 {
        t.push("No damaged files.".to_string());
    } else {
        let mut parts = Vec::new();
        if ev.put_back > 0 {
            parts.push(format!("{} repaired and put back", commas(ev.put_back)));
        }
        if ev.waiting > 0 {
            parts.push(format!("{} still to repair", commas(ev.waiting)));
        }
        if ev.left > 0 {
            parts.push(format!("{} not repaired here (evaluation.csv says why for each)", commas(ev.left)));
        }
        t.push(format!("{} damaged files: {}.", commas(total), parts.join(", ")));
    }
    if ev.packages.is_empty() {
        t.push("No packages (libraries, apps, documents kept as folders) in WORK.".to_string());
    } else {
        t.push(format!(
            "{} packages in WORK: {} with damage inside, {} intact.",
            commas(ev.packages.len() as u64),
            commas(damaged_pkgs.len() as u64),
            commas(ev.intact_packages)
        ));
    }
    for p in damaged_pkgs.iter().take(20) {
        t.push(format!("  {} ({}, {} files, {}): {}", p.path, p.what, commas(p.files), size(p.bytes), p.verdict));
    }
    if let Some(w) = &ev.written {
        t.push(format!("Everything is in {}.", w.display()));
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans() {
        let lib = "Pictures\\Photos Library.photoslibrary";
        let p = |rel: &str| plan_for(rel, package_around(rel).as_ref(), false);
        assert_eq!(
            p(&format!("{lib}\\originals\\A\\IMG_1.HEIC")),
            Plan::Isolate {
                unit: format!("{lib}\\originals\\A\\IMG_1.HEIC"),
                package: false,
                group: "2 iPhone photos and RAW (on the Mac)"
            }
        );
        assert_eq!(p(&format!("{lib}\\database\\Photos.sqlite")), Plan::Leave(LEAVE_LIBRARY_CORE));
        assert_eq!(p(&format!("{lib}\\resources\\derivatives\\0\\x.jpeg")), Plan::Leave(LEAVE_PREVIEWS));
        assert_eq!(
            p("Docs\\Report.pages\\Data\\pic.jpg"),
            Plan::Isolate { unit: "Docs\\Report.pages".to_string(), package: true, group: GROUPS[7].0 }
        );
        assert_eq!(p("Apps\\Thing.app\\Contents\\MacOS\\Thing"), Plan::Leave(LEAVE_PROGRAM));
        assert_eq!(p("Photos\\._IMG_1.JPG"), Plan::Leave(LEAVE_HOUSEKEEPING));
        assert_eq!(p("Photos\\.DS_Store"), Plan::Leave(LEAVE_HOUSEKEEPING));
        assert_eq!(p("Photos\\Thumbs.db"), Plan::Leave(LEAVE_HOUSEKEEPING));
        assert_eq!(p("setup.exe"), Plan::Leave(LEAVE_WINDOWS_PROGRAM));
        assert_eq!(
            p("Videos\\clip.MOV"),
            Plan::Isolate { unit: "Videos\\clip.MOV".to_string(), package: false, group: GROUPS[2].0 }
        );
        assert_eq!(
            p("Docs\\Report.pages"),
            Plan::Isolate { unit: "Docs\\Report.pages".to_string(), package: false, group: GROUPS[7].0 }
        );
        assert_eq!(p("Disk.sparsebundle\\bands\\1a"), Plan::Leave(LEAVE_DISK_IMAGE));
        assert_eq!(plan_for("a\\b.jpg", None, true), Plan::Leave(LEAVE_UNREADABLE));
        assert_eq!(p("notes\\odd.xyz"), Plan::Isolate { unit: "notes\\odd.xyz".to_string(), package: false, group: OTHER_GROUP });
    }

    #[test]
    fn tags() {
        assert_eq!(tagged("IMG_0003.JPG", "R0012"), "IMG_0003 [R0012].JPG");
        assert_eq!(tagged("Report.pages", "R0002"), "Report [R0002].pages");
        assert_eq!(tagged("README", "R0001"), "README [R0001]");
        assert_eq!(tag_in("IMG_0003 [R0012]_repaired.jpg"), Some("R0012".to_string()));
        assert_eq!(tag_in("IMG_0003 [r12].jpg"), Some("R0012".to_string()));
        assert_eq!(tag_in("IMG_0003.jpg"), None);
        assert_eq!(tag_in("a [R1] b [R2].jpg"), None);
        assert_eq!(tag_in("[R] x.jpg"), None);
        assert_eq!(tag_in("x [R00123].jpg"), Some("R0123".to_string()));
    }

    #[test]
    fn listed_paths() {
        let top = std::env::temp_dir().join(format!("rp-repair-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&top);
        let work = top.join("WORK");
        std::fs::create_dir_all(work.join("Photos")).unwrap();
        std::fs::write(work.join("Photos").join("IMG 1.heic"), b"x").unwrap();
        let full = format!("{}", work.join("Photos").join("IMG 1.heic").display());
        assert_eq!(resolve_listed(&work, &full).as_deref(), Some("Photos\\IMG 1.heic"));
        assert_eq!(resolve_listed(&work, "./WORK/Photos/IMG 1.heic").as_deref(), Some("Photos\\IMG 1.heic"));
        assert_eq!(resolve_listed(&work, "Photos/IMG 1.heic - Failed to decode").as_deref(), Some("Photos\\IMG 1.heic"));
        assert_eq!(resolve_listed(&work, "\"Photos\\IMG 1.heic\"").as_deref(), Some("Photos\\IMG 1.heic"));
        assert_eq!(resolve_listed(&work, "Photos/nothing.heic"), None);
        let _ = std::fs::remove_dir_all(&top);
    }

    fn sha(b: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(b);
        h.finish_hex()
    }

    /// WORK with a rescue-log.csv, and a scan's results naming the damage.
    fn setup(top: &Path) -> (PathBuf, PathBuf) {
        let _ = std::fs::remove_dir_all(top);
        let work = top.join("WORK");
        let files: Vec<(&str, &[u8])> = vec![
            ("Photos\\IMG_0003.JPG", b"damaged photo"),
            ("Photos\\IMG_0004.JPG", b"good photo"),
            ("Photos\\._IMG_0003.JPG", b"mac metadata"),
            ("Docs\\Report.pages\\Index.zip", b"index"),
            ("Docs\\Report.pages\\Data\\pic.jpg", b"damaged picture"),
            ("Lib.photoslibrary\\originals\\0\\IMG_9.HEIC", b"damaged heic"),
            ("Lib.photoslibrary\\database\\Photos.sqlite", b"damaged db"),
            ("Lib.photoslibrary\\resources\\derivatives\\0\\IMG_9.jpeg", b"damaged preview"),
            ("Tools\\Thing.app\\Contents\\MacOS\\Thing", b"damaged program"),
        ];
        let mut log = csv_row(&crate::rescue::LOG_HEADER);
        for (rel, data) in &files {
            let p = under(&work, rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, data).unwrap();
            log.push_str(&csv_row(&["FILE", rel, &data.len().to_string(), "copied", "", "", &sha(data)]));
        }
        std::fs::write(work.join(crate::rescue::LOG_NAME), format!("\u{FEFF}{log}")).unwrap();
        let mut scan = csv_row(&crate::report::RESULT_HEADER);
        for (rel, _) in &files {
            if rel.contains("IMG_0004") || rel.contains("Index.zip") || rel.starts_with("Photos\\._") {
                continue;
            }
            let full = format!("{}\\{}", work.display(), rel).replace('/', "\\");
            scan.push_str(&csv_row(&["NEVER_READ", "ddrescue never read 512 bytes of this file", "10", &full]));
        }
        let full = format!("{}\\Photos\\._IMG_0003.JPG", work.display()).replace('/', "\\");
        scan.push_str(&csv_row(&["TRUNCATED", "zero bytes", "0", &full]));
        let scan_csv = top.join("scan.csv");
        std::fs::write(&scan_csv, format!("\u{FEFF}{scan}")).unwrap();
        (work, scan_csv)
    }

    #[test]
    fn isolate_repair_put_back_undo_evaluate() {
        let top = std::env::temp_dir().join(format!("rp-repair-{}", std::process::id()));
        let (work, scan) = setup(&top);
        let no = || false;
        let o = isolate(&work, &[scan.clone()], &mut |_| {}, &no).unwrap();
        let repair = top.join(REPAIR_DIR);
        assert_eq!(o.repair, std::path::absolute(&repair).unwrap());
        let ids: Vec<(String, String)> = o.copied.iter().map(|i| (i.id.clone(), i.path.clone())).collect();
        assert_eq!(
            ids,
            vec![
                ("R0001".to_string(), "Docs\\Report.pages".to_string()),
                ("R0002".to_string(), "Lib.photoslibrary\\originals\\0\\IMG_9.HEIC".to_string()),
                ("R0003".to_string(), "Photos\\IMG_0003.JPG".to_string()),
            ],
            "{o:?}"
        );
        let whys: Vec<&str> = o.left.iter().map(|l| l.1).collect();
        assert!(whys.contains(&LEAVE_LIBRARY_CORE) && whys.contains(&LEAVE_PREVIEWS));
        assert!(whys.contains(&LEAVE_PROGRAM) && whys.contains(&LEAVE_HOUSEKEEPING));
        let photo_copy = repair.join(TO_REPAIR).join(GROUPS[0].0).join("IMG_0003 [R0003].JPG");
        assert_eq!(std::fs::read(&photo_copy).unwrap(), b"damaged photo");
        assert!(repair.join(TO_REPAIR).join(GROUPS[7].0).join("Report [R0001].pages").join("Data").join("pic.jpg").is_file());
        assert!(repair.join(README_NAME).is_file());
        // Nothing in WORK changed, and again copies nothing new.
        assert_eq!(std::fs::read(under(&work, "Photos\\IMG_0003.JPG")).unwrap(), b"damaged photo");
        let again = isolate(&work, &[scan.clone()], &mut |_| {}, &no).unwrap();
        assert!(again.copied.is_empty() && again.already == 3, "{again:?}");

        // Repair: a photo (the tool added "_repaired"), the package (whole),
        // and something with no number.
        let fixed = repair.join(REPAIRED);
        std::fs::write(fixed.join("IMG_0003 [R0003]_repaired.JPG"), b"repaired photo").unwrap();
        copy_tree(&repair.join(TO_REPAIR).join(GROUPS[7].0).join("Report [R0001].pages"), &fixed.join("Report [R0001].pages"))
            .unwrap();
        std::fs::write(fixed.join("Report [R0001].pages").join("Data").join("pic.jpg"), b"repaired picture").unwrap();
        std::fs::write(fixed.join("stray.jpg"), b"?").unwrap();
        // The HEIC came back as a JPEG: not put back.
        std::fs::write(fixed.join("IMG_9 [R0002].jpg"), b"converted").unwrap();
        let pb = put_back(&work, &mut |_| {}, &no).unwrap();
        assert_eq!(pb.returned.len(), 2, "{pb:?}");
        assert_eq!(pb.skipped.len(), 2, "{pb:?}");
        assert_eq!(std::fs::read(under(&work, "Photos\\IMG_0003.JPG")).unwrap(), b"repaired photo");
        assert_eq!(std::fs::read(under(&work, "Docs\\Report.pages\\Data\\pic.jpg")).unwrap(), b"repaired picture");
        assert_eq!(std::fs::read(under(&repair.join(ORIGINALS), "Photos\\IMG_0003.JPG")).unwrap(), b"damaged photo");
        assert_eq!(
            std::fs::read(under(&repair.join(ORIGINALS), "Docs\\Report.pages\\Data\\pic.jpg")).unwrap(),
            b"damaged picture"
        );
        assert!(!under(&work, &format!("Photos\\{TMP_NAME}")).exists());
        // Again: already back.
        let pb2 = put_back(&work, &mut |_| {}, &no).unwrap();
        assert!(pb2.returned.is_empty() && pb2.already == 2, "{pb2:?}");
        // The copies check: the repaired ones are what was put back.
        let r = crate::verify::run(&work, None, &mut |_| {}, &mut |_, _| {}, &no).unwrap();
        assert!(r.changed.is_empty() && r.missing.is_empty() && r.extra.is_empty(), "{r:?}");
        assert_eq!(r.repaired, 2, "{r:?}");

        // A newer repaired version replaces the earlier one (kept aside).
        std::fs::write(fixed.join("IMG_0003 [R0003]_repaired.JPG"), b"repaired photo, better").unwrap();
        let pb3 = put_back(&work, &mut |_| {}, &no).unwrap();
        assert_eq!(pb3.returned.len(), 1, "{pb3:?}");
        assert_eq!(std::fs::read(under(&work, "Photos\\IMG_0003.JPG")).unwrap(), b"repaired photo, better");
        assert_eq!(std::fs::read(under(&repair.join(ORIGINALS), "Photos\\IMG_0003.JPG")).unwrap(), b"damaged photo");

        // Something changed the file in WORK: it is not replaced.
        std::fs::write(under(&work, "Photos\\IMG_0003.JPG"), b"changed by hand").unwrap();
        std::fs::write(fixed.join("IMG_0003 [R0003]_repaired.JPG"), b"repaired photo, best").unwrap();
        let pb4 = put_back(&work, &mut |_| {}, &no).unwrap();
        assert!(pb4.returned.is_empty() && pb4.skipped.iter().any(|s| s.1.contains("has changed")), "{pb4:?}");
        std::fs::write(under(&work, "Photos\\IMG_0003.JPG"), b"repaired photo, better").unwrap();

        // Undo: the original is back, and the check of the copies agrees.
        let msg = undo(&work, "R0003").unwrap();
        assert!(msg.contains("back in WORK"), "{msg}");
        assert_eq!(std::fs::read(under(&work, "Photos\\IMG_0003.JPG")).unwrap(), b"damaged photo");
        assert!(undo(&work, "R0003").is_err());
        let r = crate::verify::run(&work, None, &mut |_| {}, &mut |_, _| {}, &no).unwrap();
        assert!(r.changed.is_empty() && r.missing.is_empty() && r.extra.is_empty(), "{r:?}");
        assert_eq!(r.repaired, 1, "{r:?}");

        // Evaluate.
        let ev = evaluate(&work, &[scan.clone()], &mut |_| {}, &no).unwrap();
        let get = |p: &str| ev.packages.iter().find(|x| x.path == p).unwrap().clone();
        let lib = get("Lib.photoslibrary");
        assert_eq!((lib.damaged, lib.media, lib.core, lib.previews, lib.put_back), (3, 1, 1, 1, 0), "{lib:?}");
        assert!(lib.verdict.contains("repair the whole library") && lib.verdict.contains("1 still to repair"), "{lib:?}");
        let doc = get("Docs\\Report.pages");
        assert_eq!(doc.verdict, "repaired and put back");
        let app = get("Tools\\Thing.app");
        assert!(app.verdict.contains("reinstall"), "{app:?}");
        assert_eq!(ev.files.len(), 2, "{:?}", ev.files);
        assert!(ev.files.iter().any(|f| f.0 == "Photos\\IMG_0003.JPG" && f.1 == STATE_WAITING), "{:?}", ev.files);
        assert!(repair.join(EVALUATION_NAME).is_file());
        let _ = std::fs::remove_dir_all(&top);
    }

    #[test]
    fn what_a_mac_adds_on_a_usb_stick_is_left_alone() {
        let top = std::env::temp_dir().join(format!("rp-repair-mac-{}", std::process::id()));
        let (work, scan) = setup(&top);
        let no = || false;
        isolate(&work, std::slice::from_ref(&scan), &mut |_| {}, &no).unwrap();
        let repair = top.join(REPAIR_DIR);
        assert!(repair.join(SALVAGED).is_dir());
        // Back from the Mac on a USB stick: the repaired photo, the "._" file
        // the Mac wrote beside it (same number in its name), Finder's
        // .DS_Store, and a folder from the top of the stick.
        let fixed = repair.join(REPAIRED);
        std::fs::write(fixed.join("IMG_0003 [R0003].JPG"), b"repaired photo").unwrap();
        std::fs::write(fixed.join("._IMG_0003 [R0003].JPG"), b"\x00\x05\x16\x07\x00\x02\x00\x00Mac OS X        ").unwrap();
        std::fs::write(fixed.join(".DS_Store"), b"\x00\x00\x00\x01Bud1").unwrap();
        std::fs::create_dir_all(fixed.join(".fseventsd")).unwrap();
        std::fs::write(fixed.join(".fseventsd").join("fseventsd-uuid"), b"x").unwrap();
        let pb = put_back(&work, &mut |_| {}, &no).unwrap();
        assert_eq!(pb.returned.len(), 1, "{pb:?}");
        assert!(pb.skipped.is_empty(), "{pb:?}");
        assert_eq!(std::fs::read(under(&work, "Photos\\IMG_0003.JPG")).unwrap(), b"repaired photo");
        // A list from the Mac, with its "._" file and a .DS_Store beside it:
        // only the list itself is read.
        let lists = repair.join(LISTS);
        std::fs::write(lists.join("sips-failures.txt"), "./WORK/Photos/IMG_0004.JPG\n").unwrap();
        std::fs::write(lists.join("._sips-failures.txt"), b"\x00\x05\x16\x07Mac OS X\ncom.apple.provenance\n").unwrap();
        std::fs::write(lists.join(".DS_Store"), b"\x00\x00\x00\x01Bud1\nnot a list\n").unwrap();
        let again = isolate(&work, &[scan], &mut |_| {}, &no).unwrap();
        assert!(again.unmatched.is_empty(), "{:?}", again.unmatched);
        assert_eq!(again.copied.len(), 1, "{again:?}");
        assert_eq!(again.copied[0].path, "Photos\\IMG_0004.JPG");
        let _ = std::fs::remove_dir_all(&top);
    }

    #[test]
    fn a_put_back_the_pc_never_finished_is_finished() {
        let top = std::env::temp_dir().join(format!("rp-repair-crash-{}", std::process::id()));
        let (work, scan) = setup(&top);
        let no = || false;
        isolate(&work, &[scan], &mut |_| {}, &no).unwrap();
        let repair = top.join(REPAIR_DIR);
        // The original was moved out, then the PC stopped.
        let kept = under(&repair.join(ORIGINALS), "Photos\\IMG_0003.JPG");
        std::fs::create_dir_all(kept.parent().unwrap()).unwrap();
        std::fs::rename(under(&work, "Photos\\IMG_0003.JPG"), &kept).unwrap();
        std::fs::write(under(&work, &format!("Photos\\{TMP_NAME}")), b"half").unwrap();
        std::fs::write(repair.join(REPAIRED).join("IMG_0003 [R0003].JPG"), b"repaired photo").unwrap();
        let pb = put_back(&work, &mut |_| {}, &no).unwrap();
        assert_eq!(pb.returned.len(), 1, "{pb:?}");
        assert_eq!(std::fs::read(under(&work, "Photos\\IMG_0003.JPG")).unwrap(), b"repaired photo");
        assert_eq!(std::fs::read(&kept).unwrap(), b"damaged photo");
        assert!(!under(&work, &format!("Photos\\{TMP_NAME}")).exists());
        let _ = std::fs::remove_dir_all(&top);
    }

    #[test]
    fn a_put_back_the_pc_stopped_before_listing_is_listed() {
        let top = std::env::temp_dir().join(format!("rp-repair-unlisted-{}", std::process::id()));
        let (work, scan) = setup(&top);
        let no = || false;
        isolate(&work, &[scan], &mut |_| {}, &no).unwrap();
        let repair = top.join(REPAIR_DIR);
        let fixed = repair.join(REPAIRED).join("IMG_0003 [R0003].JPG");
        std::fs::write(&fixed, b"repaired photo").unwrap();
        assert_eq!(put_back(&work, &mut |_| {}, &no).unwrap().returned.len(), 1);
        // The PC stopped after the repaired version was in place, before its
        // line in repair-returns.csv: the check of the copies sees a change.
        let returns = work.join(RETURNS_NAME);
        let header: String = std::fs::read_to_string(&returns).unwrap().lines().take(1).map(|l| format!("{l}\r\n")).collect();
        std::fs::write(&returns, header).unwrap();
        let r = crate::verify::run(&work, None, &mut |_| {}, &mut |_, _| {}, &no).unwrap();
        assert_eq!(r.changed, vec!["Photos\\IMG_0003.JPG".to_string()]);
        // Put back again: it is listed, and the check agrees.
        let pb = put_back(&work, &mut |_| {}, &no).unwrap();
        assert_eq!(pb.returned.len(), 1, "{pb:?}");
        let r = crate::verify::run(&work, None, &mut |_| {}, &mut |_, _| {}, &no).unwrap();
        assert!(r.changed.is_empty() && r.missing.is_empty() && r.extra.is_empty() && r.repaired == 1, "{r:?}");
        assert_eq!(put_back(&work, &mut |_| {}, &no).unwrap().already, 1, "and then it is simply back");
        // Undone, then the repaired version copied into WORK by hand: the
        // original is not in REPAIR\originals, so it is not listed as put back.
        undo(&work, "R0003").unwrap();
        std::fs::write(under(&work, "Photos\\IMG_0003.JPG"), b"repaired photo").unwrap();
        let pb = put_back(&work, &mut |_| {}, &no).unwrap();
        assert!(pb.returned.is_empty() && pb.skipped.iter().any(|s| s.1.contains("replaced by hand")), "{pb:?}");
        // A "repaired" version that is exactly the damaged original.
        std::fs::write(under(&work, "Photos\\IMG_0003.JPG"), b"damaged photo").unwrap();
        std::fs::write(&fixed, b"damaged photo").unwrap();
        let pb = put_back(&work, &mut |_| {}, &no).unwrap();
        assert!(pb.returned.is_empty() && pb.skipped.iter().any(|s| s.1.contains("nothing was repaired")), "{pb:?}");
        let _ = std::fs::remove_dir_all(&top);
    }
}
