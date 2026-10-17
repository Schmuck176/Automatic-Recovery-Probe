//! What the probe knows about file formats: magic numbers, which extension
//! expects which content, and which formats can or cannot be fully tested.
//! Every table here mirrors the Python version line for line.

/// Read size for the head of a file and for the hole scan.
pub const CHUNK: usize = 1 << 20;
/// Default zero-run threshold in KiB.
pub const DEFAULT_HOLE_KB: u64 = 4;

/// (offset, magic bytes, format key). First match wins.
pub static MAGIC: &[(usize, &[u8], &str)] = &[
    (0, b"\xff\xd8\xff", "jpeg"),
    (0, b"\x89PNG\r\n\x1a\n", "png"),
    (0, b"GIF87a", "gif"),
    (0, b"GIF89a", "gif"),
    (0, b"BM", "bmp"),
    (0, b"II*\x00", "tiff"),
    (0, b"MM\x00*", "tiff"),
    // TIFF-shaped camera RAW with a magic number of its own: Panasonic and
    // Leica (RW2, RWL, RAW) and Olympus (ORF).
    (0, b"IIU\x00", "tiff"),
    (0, b"IIRO", "tiff"),
    (0, b"IIRS", "tiff"),
    (0, b"MMOR", "tiff"),
    (6, b"HEAPCCDR", "crw"), // Canon CRW, before 2004
    (0, b"\x00MRM", "mrw"),  // Minolta / Konica Minolta RAW
    (0, b"%PDF-", "pdf"),
    (0, b"PK\x03\x04", "zip"),
    (0, b"PK\x05\x06", "zip"),
    (0, b"Rar!\x1a\x07", "rar"),
    (0, b"7z\xbc\xaf\x27\x1c", "7z"),
    (0, b"\x1f\x8b", "gzip"),
    (0, b"fLaC", "flac"),
    (0, b"OggS", "ogg"),
    (0, b"RIFF", "riff"),
    (0, b"\x1a\x45\xdf\xa3", "matroska"),
    (4, b"ftyp", "isobmff"), // mp4 / mov / m4a / heic / cr3
    // Older QuickTime files have no ftyp box and begin directly with one of these.
    (4, b"moov", "isobmff"),
    (4, b"mdat", "isobmff"),
    (4, b"wide", "isobmff"),
    (4, b"free", "isobmff"),
    (4, b"skip", "isobmff"),
    (4, b"pnot", "isobmff"),
    (0, b"SQLite format 3\x00", "sqlite"),
    (0, b"FUJIFILMCCD-RAW", "raf"), // Fujifilm RAW
    (0, b"FOVb", "x3f"),            // Sigma RAW
    // The '._' files a Mac writes beside every file it saves to a non-Mac disk.
    (0, b"\x00\x05\x16\x07", "appledouble"),
    (0, b"\x00\x05\x16\x00", "appledouble"),
];

/// Extension (lower case, with the dot) -> the format keys its content may have.
pub fn expected_formats(ext: &str) -> Option<&'static [&'static str]> {
    let v: &'static [&'static str] = match ext {
        ".jpg" | ".jpeg" | ".jpe" => &["jpeg"],
        ".png" => &["png"],
        ".gif" => &["gif"],
        ".bmp" => &["bmp"],
        ".tif" | ".tiff" => &["tiff"],
        ".cr2" | ".nef" | ".arw" | ".dng" | ".orf" | ".rw2" | ".pef" | ".srw" => &["tiff"],
        ".raf" => &["raf"],
        ".cr3" => &["isobmff"],
        ".x3f" => &["x3f"],
        ".webp" => &["riff"],
        ".avif" => &["isobmff"],
        // Not ".db": most .db files are not SQLite (Windows' Thumbs.db is an OLE file).
        ".sqlite" | ".sqlite3" => &["sqlite"],
        ".pdf" => &["pdf"],
        ".zip" | ".docx" | ".xlsx" | ".pptx" | ".epub" | ".odt" | ".ods" | ".jar" => &["zip"],
        ".rar" => &["rar"],
        ".7z" => &["7z"],
        ".gz" => &["gzip"],
        ".mp3" => &["mp3", "riff"],
        ".flac" => &["flac"],
        ".ogg" => &["ogg"],
        ".wav" | ".avi" => &["riff"],
        ".mp4" | ".m4v" | ".mov" | ".m4a" | ".heic" | ".heif" => &["isobmff"],
        ".mkv" | ".webm" => &["matroska"],
        _ => return None,
    };
    Some(v)
}

/// Formats where a long zero run is genuinely abnormal (compressed throughout).
pub fn is_compressed(fmt: &str) -> bool {
    matches!(
        fmt,
        "jpeg" | "png" | "gif" | "pdf" | "zip" | "rar" | "7z" | "gzip" | "mp3" | "flac" | "ogg" | "isobmff"
            | "matroska" | "raf"
    )
}

/// Camera RAW extensions.
pub fn is_raw_ext(ext: &str) -> bool {
    matches!(
        ext,
        ".cr2" | ".cr3" | ".crw" | ".nef" | ".nrw" | ".arw" | ".srf" | ".sr2" | ".dng" | ".raf" | ".orf"
            | ".rw2" | ".raw" | ".pef" | ".ptx" | ".srw" | ".3fr" | ".erf" | ".mef" | ".mos" | ".x3f"
            | ".iiq" | ".k25" | ".kdc" | ".dcr" | ".rwl" | ".fff" | ".mrw"
    )
}

/// Formats whose structure can be checked but whose pixels cannot.
pub fn is_structure_only(fmt: &str) -> bool {
    matches!(fmt, "tiff" | "raf" | "x3f")
}

/// Extensions nothing on Windows decodes: never reported OK.
pub fn is_undecodable_ext(ext: &str) -> bool {
    matches!(ext, ".heic" | ".heif" | ".avif" | ".hif")
}

/// Formats whose last bytes are always real data, so a sector-aligned zero
/// ending means the end was never read.
pub fn zero_tail_means_damage(fmt: &str) -> bool {
    matches!(fmt, "mp3" | "flac" | "ogg" | "isobmff")
}

/// Apple Photos / iPhoto / Aperture library bundle name endings.
pub const LIBRARY_BUNDLES: &[&str] = &[
    ".photoslibrary",
    ".photolibrary",
    ".migratedphotolibrary",
    ".aplibrary",
    ".pplibrary",
    ".aplibrary2",
    ".photolibrary2",
];

/// Folders never descended into (compared exactly, as in the Python version).
pub const SYSTEM_DIRS: &[&str] = &[
    ".Spotlight-V100",
    ".fseventsd",
    "$RECYCLE.BIN",
    "System Volume Information",
    ".Trashes",
    "found.000",
];

/// The verdicts, in the order the summaries list them.
pub const VERDICTS: &[&str] = &["DB_DAMAGED", "TRUNCATED", "HOLE", "MISMATCH", "UNREADABLE", "LIBRARY", "OK", "SKIPPED"];

/// The verdicts that mean "this file is damaged".
pub fn is_damage(verdict: &str) -> bool {
    matches!(verdict, "DB_DAMAGED" | "TRUNCATED" | "HOLE" | "MISMATCH" | "UNREADABLE")
}

/// Housekeeping files Windows and macOS scatter through folders (lower case).
pub fn is_system_file(name_lower: &str) -> bool {
    matches!(
        name_lower,
        "thumbs.db" | "ehthumbs.db" | "desktop.ini" | ".ds_store" | ".localized" | "icon\r" | "icon\u{f00d}"
    )
}

pub const SYSTEM_KEY: &str = "(system files)";
pub const SYSTEM_NOTE: &str = "Mac or Windows housekeeping file (folder settings, thumbnails or a Mac '._' metadata file), not your data - nothing to test";

/// Biggest JPEG walked to find where the picture really ends.
pub const JPEG_PARSE_LIMIT: u64 = 256 << 20;
/// Biggest MP4/MOV 'moov' box read when looking for padding and PCM audio.
pub const MOOV_LIMIT: u64 = 64 << 20;

/// Photos Library database file names (lower case).
pub const LIBRARY_DBS: &[&str] = &["photos.sqlite", "photos.db", "library.apdb", "librarydata.apdb"];

/// Where an ID3v2 tag at the very start of the file ends; 0 if there is none.
pub fn id3_end(head: &[u8]) -> usize {
    if head.len() < 10 || &head[..3] != b"ID3" {
        return 0;
    }
    let s = &head[6..10];
    let size = ((s[0] as usize & 0x7F) << 21)
        | ((s[1] as usize & 0x7F) << 14)
        | ((s[2] as usize & 0x7F) << 7)
        | (s[3] as usize & 0x7F);
    10 + size + if head[5] & 0x10 != 0 { 10 } else { 0 }
}

fn slice(b: &[u8], start: usize, len: usize) -> &[u8] {
    let s = start.min(b.len());
    let e = start.saturating_add(len).min(b.len());
    &b[s..e]
}

/// The format key for a file's first bytes, or "" when unrecognised.
pub fn sniff(head: &[u8]) -> &'static str {
    let id3 = id3_end(head);
    if id3 > 0 {
        // An ID3 tag usually fronts an MP3, but some taggers put one in front of FLAC too.
        if slice(head, id3, 4) == b"fLaC" {
            return "flac";
        }
        return "mp3";
    }
    for (off, magic, key) in MAGIC {
        if slice(head, *off, magic.len()) == *magic {
            return key;
        }
    }
    // bare MP3 frame sync (no ID3 tag)
    if head.len() > 1 && head[0] == 0xFF && (head[1] & 0xE0) == 0xE0 {
        return "mp3";
    }
    ""
}

/// Python's `os.path.splitext(path)[1]`: the extension of the last path
/// component, where leading dots never start an extension.
pub fn splitext_ext(path: &str) -> &str {
    let sep = if cfg!(windows) {
        path.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0)
    } else {
        path.rfind('/').map(|i| i + 1).unwrap_or(0)
    };
    let name = &path[sep..];
    let dot = match name.rfind('.') {
        Some(d) => d,
        None => return "",
    };
    // Every character before the last dot is a dot: no extension.
    if name[..dot].bytes().all(|c| c == b'.') {
        return "";
    }
    &name[dot..]
}

/// Python's `os.path.basename(path)`.
pub fn basename(path: &str) -> &str {
    let sep = if cfg!(windows) {
        path.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0)
    } else {
        path.rfind('/').map(|i| i + 1).unwrap_or(0)
    };
    &path[sep..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitext_matches_python() {
        assert_eq!(splitext_ext("/a/b/photo.JPG"), ".JPG");
        assert_eq!(splitext_ext("/a/b/._IMG_0001.JPG"), ".JPG");
        assert_eq!(splitext_ext("/a/b/.bashrc"), "");
        assert_eq!(splitext_ext("/a/b/..cr2"), "");
        assert_eq!(splitext_ext("/a/b.d/noext"), "");
        assert_eq!(splitext_ext("archive.tar.gz"), ".gz");
        assert_eq!(splitext_ext("name."), ".");
    }

    #[test]
    fn sniff_basics() {
        assert_eq!(sniff(b"\xff\xd8\xff\xe0"), "jpeg");
        assert_eq!(sniff(b"\x00\x00\x00\x18ftypheic"), "isobmff");
        assert_eq!(sniff(b"\xff\xfb\x90\x64"), "mp3");
        assert_eq!(sniff(b"hello"), "");
    }
}
