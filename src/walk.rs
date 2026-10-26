//! Walking a folder: every file gets a verdict, and every Apple Photos
//! Library found on the way is inventoried. A port of the Python version's
//! walk_and_probe, survey_library and looks_like_library, including its
//! folder order rules (top-down, system folders skipped, symlinks not
//! followed).

use crate::formats::{is_damage, LIBRARY_BUNDLES, LIBRARY_DBS, SYSTEM_DIRS};
use crate::probe::probe_file;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub const SEP: char = std::path::MAIN_SEPARATOR;

// ---------------------------------------------------------------------------
// Path strings, the way Python's os.path builds them.
// ---------------------------------------------------------------------------

/// Python's `os.path.join(dir, name)` for a single name.
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        return name.to_string();
    }
    let ends_with_sep = dir.ends_with('/') || (cfg!(windows) && (dir.ends_with('\\') || (dir.len() == 2 && dir.ends_with(':'))));
    if ends_with_sep {
        format!("{dir}{name}")
    } else {
        format!("{dir}{SEP}{name}")
    }
}

/// Python's `os.path.abspath`: absolute, with `.`, `..` and doubled
/// separators removed.
pub fn abspath(p: &str) -> String {
    let p = if is_absolute(p) {
        p.to_string()
    } else {
        let cwd = std::env::current_dir().map(|c| c.to_string_lossy().into_owned()).unwrap_or_default();
        join(&cwd, p)
    };
    normpath(&p)
}

fn is_absolute(p: &str) -> bool {
    if cfg!(windows) {
        let b = p.as_bytes();
        p.starts_with("\\\\") || p.starts_with("//") || (b.len() >= 3 && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
    } else {
        p.starts_with('/')
    }
}

/// Python's `os.path.normpath` (ntpath on Windows, posixpath elsewhere).
pub fn normpath(p: &str) -> String {
    if cfg!(windows) {
        if p.starts_with("\\\\?\\") || p.starts_with("\\\\.\\") {
            return p.to_string();
        }
        let s = p.replace('/', "\\");
        let (prefix, rest) = if let Some(unc) = s.strip_prefix("\\\\") {
            // \\server\share is the prefix.
            let mut parts = unc.splitn(3, '\\');
            let server = parts.next().unwrap_or("");
            let share = parts.next().unwrap_or("");
            let rest = parts.next().unwrap_or("");
            (format!("\\\\{server}\\{share}"), format!("\\{rest}"))
        } else if s.len() >= 2 && s.as_bytes()[1] == b':' {
            (s[..2].to_string(), s[2..].to_string())
        } else {
            (String::new(), s.clone())
        };
        let rooted = rest.starts_with('\\');
        let mut comps: Vec<&str> = Vec::new();
        for c in rest.split('\\') {
            match c {
                "" | "." => {}
                ".." => {
                    if comps.last().is_some_and(|l| *l != "..") {
                        comps.pop();
                    } else if !rooted {
                        comps.push("..");
                    }
                }
                c => comps.push(c),
            }
        }
        let out = format!("{prefix}{}{}", if rooted { "\\" } else { "" }, comps.join("\\"));
        if out.is_empty() {
            ".".to_string()
        } else {
            out
        }
    } else {
        let leading = p.len() - p.trim_start_matches('/').len();
        let root = match leading {
            0 => "",
            2 => "//",
            _ => "/",
        };
        let mut comps: Vec<&str> = Vec::new();
        for c in p.split('/') {
            match c {
                "" | "." => {}
                ".." => {
                    if comps.last().is_some_and(|l| *l != "..") {
                        comps.pop();
                    } else if root.is_empty() {
                        comps.push("..");
                    }
                }
                c => comps.push(c),
            }
        }
        let out = format!("{root}{}", comps.join("/"));
        if out.is_empty() {
            ".".to_string()
        } else {
            out
        }
    }
}

// ---------------------------------------------------------------------------
// os.walk, top-down, errors ignored, symlinked folders listed but not entered.
// ---------------------------------------------------------------------------

/// List one folder: (sub-folder names, which of them may be entered, file names).
fn list_dir(dir: &str) -> Option<(Vec<String>, Vec<bool>, Vec<String>)> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut dirs = Vec::new();
    let mut enter = Vec::new();
    let mut files = Vec::new();
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let (is_dir, is_link) = match entry.file_type() {
            Ok(ft) if ft.is_symlink() => (std::fs::metadata(entry.path()).map(|m| m.is_dir()).unwrap_or(false), true),
            Ok(ft) => (ft.is_dir(), false),
            Err(_) => (false, false),
        };
        if is_dir {
            dirs.push(name);
            enter.push(!is_link);
        } else {
            files.push(name);
        }
    }
    Some((dirs, enter, files))
}

/// `walk`, with each file's size, from the folder's own listing (on
/// Windows that costs nothing more: no file is opened).
pub fn walk_sized(root: &str, mut visit: impl FnMut(&str, &mut Vec<String>, &[(String, u64)]) -> bool) {
    let mut stack: Vec<String> = vec![root.to_string()];
    while let Some(top) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&top) else { continue };
        let mut dirs = Vec::new();
        let mut enter = Vec::new();
        let mut files = Vec::new();
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            match entry.file_type() {
                Ok(ft) if ft.is_symlink() => {
                    if std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()) {
                        dirs.push(name);
                        enter.push(false);
                    } else {
                        files.push((name, std::fs::metadata(entry.path()).map(|m| m.len()).unwrap_or(0)));
                    }
                }
                Ok(ft) if ft.is_dir() => {
                    dirs.push(name);
                    enter.push(true);
                }
                _ => files.push((name, entry.metadata().map(|m| m.len()).unwrap_or(0))),
            }
        }
        let before = dirs.clone();
        if !visit(&top, &mut dirs, &files) {
            return;
        }
        for d in dirs.iter().rev() {
            let may_enter = before.iter().position(|b| b == d).map(|i| enter[i]).unwrap_or(true);
            if may_enter {
                stack.push(join(&top, d));
            }
        }
    }
}

/// Walk `root` top-down. `visit(dirpath, dirnames, filenames)` may prune
/// `dirnames`; it returns false to stop the whole walk.
pub fn walk(root: &str, mut visit: impl FnMut(&str, &mut Vec<String>, &[String]) -> bool) {
    let mut stack: Vec<String> = vec![root.to_string()];
    while let Some(top) = stack.pop() {
        let (mut dirs, enter, files) = match list_dir(&top) {
            Some(v) => v,
            None => continue,
        };
        let before = dirs.clone();
        if !visit(&top, &mut dirs, &files) {
            return;
        }
        for d in dirs.iter().rev() {
            // A folder the caller kept; enter it unless it is a symlink.
            let may_enter = before.iter().position(|b| b == d).map(|i| enter[i]).unwrap_or(true);
            if may_enter {
                stack.push(join(&top, d));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Photos Libraries
// ---------------------------------------------------------------------------

/// Is this folder an Apple Photos / iPhoto / Aperture library bundle?
/// By name, or by shape (a database/ folder beside originals/ or masters/).
pub fn looks_like_library(dirpath: &str, dirnames: &[String]) -> &'static str {
    let base = crate::formats::basename(dirpath).to_lowercase();
    if LIBRARY_BUNDLES.iter().any(|b| base.ends_with(b)) {
        return "name";
    }
    let low: Vec<String> = dirnames.iter().map(|d| d.to_lowercase()).collect();
    if low.iter().any(|d| d == "database") && low.iter().any(|d| d == "originals" || d == "masters") {
        return "structure (database/ beside originals/)";
    }
    ""
}

/// One Photos Library's inventory.
#[derive(Clone, Debug, Default)]
pub struct LibInfo {
    pub path: String,
    pub how: &'static str,
    pub originals: u64,
    pub orig_bytes: u64,
    pub total_bytes: u64,
    pub files: u64,
    pub db: &'static str, // "missing" or "present"
    pub db_bytes: u64,
    pub hot_wal: bool,
    pub newest: f64,
    pub db_verdict: String,
    pub db_detail: String,
    pub median_orig: u64,
    // Filled in during the main walk:
    pub damaged: u64,
    pub unverified: u64,
    /// Damage by verdict, in the order first seen.
    pub damage: Vec<(&'static str, u64)>,
}

impl LibInfo {
    fn add_damage(&mut self, verdict: &'static str) {
        match self.damage.iter_mut().find(|(v, _)| *v == verdict) {
            Some((_, n)) => *n += 1,
            None => self.damage.push((verdict, 1)),
        }
    }
}

fn mtime_secs(m: &std::fs::Metadata) -> f64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Python's `os.path.relpath(dirpath, libpath)` for a folder inside the library.
fn relpath_inside(dirpath: &str, libpath: &str) -> String {
    if dirpath == libpath {
        return ".".to_string();
    }
    let base = if libpath.ends_with(SEP) { libpath.to_string() } else { format!("{libpath}{SEP}") };
    dirpath.strip_prefix(&base).unwrap_or(dirpath).to_string()
}

/// Inventory one Photos library in a single stat pass.
pub fn survey_library(libpath: &str, how: &'static str) -> LibInfo {
    let mut info = LibInfo { path: libpath.to_string(), how, db: "missing", ..Default::default() };
    let mut sizes: Vec<u64> = Vec::new();
    walk(libpath, |dirpath, dirnames, filenames| {
        dirnames.retain(|d| !SYSTEM_DIRS.contains(&d.as_str()));
        // A library nested inside another gets its own entry; not counted here.
        if dirpath != libpath && !looks_like_library(dirpath, dirnames).is_empty() {
            dirnames.clear();
            return true;
        }
        let rel = relpath_inside(dirpath, libpath).replace('\\', "/").to_lowercase();
        let first = rel.split('/').next().unwrap_or("");
        let in_orig = first == "originals" || first == "masters";
        for name in filenames {
            let full = join(dirpath, name);
            let m = match std::fs::metadata(&full) {
                Ok(m) => m,
                Err(_) => continue,
            };
            info.files += 1;
            info.total_bytes += m.len();
            let mt = mtime_secs(&m);
            if mt > info.newest {
                info.newest = mt;
            }
            if in_orig {
                info.originals += 1;
                info.orig_bytes += m.len();
                if sizes.len() < 20000 {
                    sizes.push(m.len());
                }
            }
            if LIBRARY_DBS.contains(&name.to_lowercase().as_str()) {
                info.db = "present";
                info.db_bytes = m.len();
                let w = format!("{full}-wal");
                info.hot_wal = std::fs::metadata(&w).map(|m| m.len() > 0).unwrap_or(false);
            }
        }
        true
    });
    sizes.sort_unstable();
    if !sizes.is_empty() {
        info.median_orig = sizes[sizes.len() / 2];
    }
    info
}

// ---------------------------------------------------------------------------
// The main walk
// ---------------------------------------------------------------------------

/// One file's row: verdict, detail, size, path.
#[derive(Clone, Debug)]
pub struct Row {
    pub verdict: &'static str,
    pub detail: String,
    pub size: u64,
    pub path: String,
}

/// What the walk reports, in order: a library when it is found, a row per
/// file, and each library's final inventory at the end.
pub enum Event {
    Lib(usize, LibInfo),
    Row(Row),
    LibDone(usize, LibInfo),
}

/// Probe every file under `root`. `stop` ends the walk early (and then no
/// LibDone events are sent, as in the Python version). Returns false when
/// `root` itself went away part-way (its drive unplugged?): the walk then
/// stops rather than calling every file after that UNREADABLE.
pub fn walk_and_probe(
    root: &str,
    hole_bytes: u64,
    do_hole: bool,
    stop: Option<&AtomicBool>,
    mut emit: impl FnMut(Event),
) -> bool {
    let root = abspath(root);
    let mut libs: Vec<LibInfo> = Vec::new();
    let mut stopped = false;
    let mut gone = false;
    walk(&root, |dirpath, dirnames, filenames| {
        dirnames.retain(|d| !SYSTEM_DIRS.contains(&d.as_str()));
        let how = looks_like_library(dirpath, dirnames);
        if !how.is_empty() {
            let info = survey_library(dirpath, how);
            libs.push(info.clone());
            emit(Event::Lib(libs.len() - 1, info));
        }
        // The innermost library containing this folder, resolved once per folder.
        let mut here: Option<usize> = None;
        for (idx, info) in libs.iter().enumerate() {
            let base = &info.path;
            let inside = dirpath == base
                || dirpath.strip_prefix(base.as_str()).is_some_and(|rest| rest.starts_with(SEP));
            if inside && here.is_none_or(|h| base.len() > libs[h].path.len()) {
                here = Some(idx);
            }
        }
        for name in filenames {
            if stop.is_some_and(|s| s.load(Ordering::Relaxed)) {
                stopped = true;
                return false;
            }
            let p = join(dirpath, name);
            let res = std::panic::catch_unwind(|| probe_file(Path::new(&p), hole_bytes, do_hole));
            let r = match res {
                Ok(r) => r,
                Err(_) => crate::probe::Verdict {
                    verdict: "UNREADABLE",
                    detail: "the probe hit an internal error on this file - check it by hand".to_string(),
                    size: 0,
                },
            };
            if r.verdict == "UNREADABLE" && std::fs::metadata(&root).is_err() {
                gone = true;
                stopped = true;
                return false;
            }
            if let Some(h) = here {
                let lib = &mut libs[h];
                if is_damage(r.verdict) {
                    lib.damaged += 1;
                    lib.add_damage(r.verdict);
                } else if r.verdict == "SKIPPED" {
                    lib.unverified += 1;
                }
                if LIBRARY_DBS.contains(&name.to_lowercase().as_str()) {
                    lib.db_verdict = r.verdict.to_string();
                    lib.db_detail = r.detail.clone();
                }
            }
            emit(Event::Row(Row { verdict: r.verdict, detail: r.detail, size: r.size, path: p }));
        }
        true
    });
    if stopped {
        return !gone;
    }
    for (i, info) in libs.into_iter().enumerate() {
        emit(Event::LibDone(i, info));
    }
    true
}

/// What to say when the folder being checked went away part-way.
pub const GONE: &str = "The folder being checked went away part-way through - was its drive unplugged? Nothing is \
lost: plug the drive back in, check it shows in File Explorer, and run this check again.";

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn normpath_like_python() {
        assert_eq!(normpath("/a/b/../c/./d/"), "/a/c/d");
        assert_eq!(normpath("//a//b"), "//a/b");
        assert_eq!(normpath("///a"), "/a");
        assert_eq!(normpath("/.."), "/");
        assert_eq!(normpath("a/../.."), "..");
        assert_eq!(join("/", "x"), "/x");
        assert_eq!(join("/a", "x"), "/a/x");
    }

    #[test]
    fn a_folder_that_goes_away_stops_the_check() {
        let dir = std::env::temp_dir().join(format!("rp-walk-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for i in 0..40 {
            std::fs::write(dir.join("sub").join(format!("f{i:02}.txt")), b"some text").unwrap();
        }
        let root = dir.display().to_string();
        let mut rows = 0;
        let mut unreadable = 0;
        let ok = walk_and_probe(&root, 4096, true, None, |ev| {
            if let Event::Row(r) = ev {
                rows += 1;
                if r.verdict == "UNREADABLE" {
                    unreadable += 1;
                }
                if rows == 3 {
                    // The drive is unplugged.
                    std::fs::remove_dir_all(&dir).unwrap();
                }
            }
        });
        assert!(!ok, "the check says the folder went away");
        assert_eq!(unreadable, 0, "and calls no file unreadable because of it");
        assert_eq!(rows, 3);
        // A folder that stays: all is well.
        let dir2 = std::env::temp_dir().join(format!("rp-walk-stays-{}", std::process::id()));
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::write(dir2.join("a.txt"), b"x").unwrap();
        assert!(walk_and_probe(&dir2.display().to_string(), 4096, true, None, |_| {}));
        std::fs::remove_dir_all(&dir2).ok();
    }

    #[test]
    fn library_by_name_and_by_shape() {
        assert_eq!(looks_like_library("/x/My Pics.photoslibrary", &[]), "name");
        let dirs = vec!["Database".to_string(), "originals".to_string()];
        assert_eq!(looks_like_library("/x/Renamed", &dirs), "structure (database/ beside originals/)");
        assert_eq!(looks_like_library("/x/plain", &["database".to_string()]), "");
    }
}
