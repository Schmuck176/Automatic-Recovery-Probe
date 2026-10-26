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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

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
