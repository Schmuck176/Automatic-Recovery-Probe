//! Structural checks: does a file end where its own format says it should,
//! is its index intact, and which of its zero bytes are legitimate padding.
//! Each function is a line-for-line port of the Python version's twin.

use crate::fmt::{ascii_replace, commas};
use crate::formats::{id3_end, JPEG_PARSE_LIMIT, MOOV_LIMIT};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// A finding: (verdict, detail). An empty verdict means nothing is wrong.
pub type Finding = (&'static str, String);

pub fn ok() -> Finding {
    ("", String::new())
}

pub fn rstrip_zeros(b: &[u8]) -> &[u8] {
    let mut n = b.len();
    while n > 0 && b[n - 1] == 0 {
        n -= 1;
    }
    &b[..n]
}

pub fn lstrip_len(b: &[u8]) -> usize {
    b.iter().take_while(|&&c| c == 0).count()
}

pub fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.len() >= needle.len() && hay.windows(needle.len()).any(|w| w == needle)
}

fn be32(b: &[u8]) -> u64 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64
}

fn be64(b: &[u8]) -> u64 {
    u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

/// Read up to `n` bytes at `pos` (fewer at end of file), like Python's
/// seek() + read().
pub fn read_at(fh: &mut File, pos: u64, n: usize) -> io::Result<Vec<u8>> {
    fh.seek(SeekFrom::Start(pos))?;
    let mut buf = Vec::with_capacity(n.min(1 << 26));
    fh.by_ref().take(n as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Explain a missing end marker when the tail is zeros, not just absent.
pub fn zero_tail_note(tail: &[u8]) -> String {
    let n = tail.len() - rstrip_zeros(tail).len();
    if n >= 512 {
        let most = if n == tail.len() { "at least " } else { "" };
        return format!(
            " - the last {most}{} bytes are zeros, so the end of this file was never read off the disk",
            commas(n as u64)
        );
    }
    String::new()
}

/// Return (verdict, detail); the verdict is "" when nothing is wrong.
pub fn check_trailer(fmt: &str, size: u64, head: &[u8], tail: &[u8]) -> Finding {
    match fmt {
        "jpeg" => {
            if !rstrip_zeros(tail).ends_with(b"\xff\xd9") {
                return ("TRUNCATED", format!("no JPEG end-of-image marker (FFD9){}", zero_tail_note(tail)));
            }
        }
        "png" => {
            if !contains(tail, b"IEND\xaeB`\x82") {
                return ("TRUNCATED", format!("no PNG IEND chunk{}", zero_tail_note(tail)));
            }
        }
        "gif" => {
            if !rstrip_zeros(tail).ends_with(b"\x3b") {
                return ("TRUNCATED", format!("no GIF trailer byte (0x3B){}", zero_tail_note(tail)));
            }
        }
        "pdf" => {
            if !contains(tail, b"%%EOF") {
                return ("TRUNCATED", format!("no %%EOF in final bytes{}", zero_tail_note(tail)));
            }
        }
        "zip" => {
            if !contains(tail, b"PK\x05\x06") {
                return ("TRUNCATED", format!("no ZIP end-of-central-directory record{}", zero_tail_note(tail)));
            }
        }
        "riff" => {
            if head.len() >= 8 {
                let declared = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as u64 + 8;
                if declared > size {
                    let short = declared - size;
                    return (
                        "TRUNCATED",
                        format!(
                            "RIFF header declares {} bytes, file is {} (short {})",
                            commas(declared),
                            commas(size),
                            commas(short)
                        ),
                    );
                }
            }
        }
        "isobmff" => {
            let f = check_isobmff(head, size);
            if !f.0.is_empty() {
                return f;
            }
        }
        "tiff" => {
            let f = check_tiff(head, size);
            if !f.0.is_empty() {
                return f;
            }
        }
        _ => {}
    }
    ok()
}

/// Walk top-level MP4/MOV boxes using only the head already read.
fn check_isobmff(head: &[u8], size: u64) -> Finding {
    let mut pos: u64 = 0;
    let limit = head.len() as u64;
    while pos + 8 <= limit {
        let p = pos as usize;
        let mut boxlen = be32(&head[p..]);
        let name = ascii_replace(&head[p + 4..p + 8]);
        if boxlen == 1 {
            if pos + 16 > limit {
                break;
            }
            boxlen = be64(&head[p + 8..]);
        } else if boxlen == 0 {
            return ok();
        }
        if boxlen < 8 {
            return ("TRUNCATED", format!("malformed box '{name}' (size {boxlen})"));
        }
        let end = pos as u128 + boxlen as u128;
        if end > size as u128 {
            let over = end - size as u128;
            return ("TRUNCATED", format!("box '{name}' runs {} bytes past end of file", commas(over as u64)));
        }
        pos = end as u64;
    }
    ok()
}

/// Walk every top-level box of an MP4/MOV/HEIC file and require an index box.
pub fn check_isobmff_index(path: &Path, size: u64) -> Finding {
    let mut seen_index = false;
    let mut pos: u64 = 0;
    let mut fh = match File::open(path) {
        Ok(f) => f,
        Err(_) => return ok(),
    };
    for _ in 0..1_000_000 {
        if pos + 8 > size {
            break;
        }
        let h = match read_at(&mut fh, pos, 16) {
            Ok(h) => h,
            Err(_) => return ok(),
        };
        if h.len() < 8 {
            return ok();
        }
        let mut boxlen = be32(&h);
        let typ = &h[4..8];
        let mut hdr = 8;
        if boxlen == 1 && h.len() >= 16 {
            boxlen = be64(&h[8..]);
            hdr = 16;
        } else if boxlen == 0 {
            boxlen = size - pos;
        }
        if typ == b"\x00\x00\x00\x00" || boxlen < hdr {
            if seen_index {
                break; // zeros after a complete file: harmless
            }
            return (
                "TRUNCATED",
                format!(
                    "zeros where a box header should be, at offset {} of {} - this part of the file was never read, and the index box is missing",
                    commas(pos),
                    commas(size)
                ),
            );
        }
        let end = pos as u128 + boxlen as u128;
        if end > size as u128 {
            return (
                "TRUNCATED",
                format!(
                    "box '{}' runs {} bytes past end of file",
                    ascii_replace(typ),
                    commas((end - size as u128) as u64)
                ),
            );
        }
        if typ == b"moov" || typ == b"meta" || typ == b"moof" {
            seen_index = true;
        }
        pos = end as u64;
    }
    if !seen_index {
        return (
            "TRUNCATED",
            "no index box (moov/meta) anywhere in the file - without it the video or photo cannot be opened".to_string(),
        );
    }
    ok()
}

/// (value, length) of the EBML variable-length integer at buf[pos].
pub fn ebml_vint(buf: &[u8], pos: usize, keep_marker: bool) -> Option<(u64, usize)> {
    let first = *buf.get(pos)?;
    if first == 0 {
        return None;
    }
    let length = 9 - (8 - first.leading_zeros() as usize); // 1xxxxxxx is 1 byte ... 00000001 is 8
    if pos + length > buf.len() {
        return None;
    }
    let mut val: u64 = if keep_marker { first as u64 } else { (first as u64) & ((1u64 << (8 - length)) - 1) };
    for &b in &buf[pos + 1..pos + length] {
        val = (val << 8) | b as u64;
    }
    Some((val, length))
}

/// A Matroska/WebM Segment that declares more bytes than the file holds.
pub fn check_matroska_size(head: &[u8], size: u64) -> Finding {
    let parsed = (|| {
        let (_, n) = ebml_vint(head, 0, true)?;
        let (hlen, m) = ebml_vint(head, n, false)?;
        let pos = n as u64 + m as u64 + hlen;
        if pos > usize::MAX as u64 {
            return None;
        }
        let pos = pos as usize;
        let (seg, n2) = ebml_vint(head, pos, true)?;
        if seg != 0x1853_8067 {
            return Some(None);
        }
        let (length, m2) = ebml_vint(head, pos + n2, false)?;
        Some(Some((pos, n2, length, m2)))
    })();
    let (pos, n, length, m) = match parsed {
        Some(Some(v)) => v,
        _ => return ok(),
    };
    if length == (1u64 << (7 * m)) - 1 {
        return ok(); // "unknown size": live recordings
    }
    let end = pos as u128 + n as u128 + m as u128 + length as u128;
    if end > size as u128 {
        return (
            "TRUNCATED",
            format!(
                "the file says it is {} bytes long but stops at {} (short {})",
                commas(end as u64),
                commas(size),
                commas((end - size as u128) as u64)
            ),
        );
    }
    ok()
}

/// How many zero bytes the file ends with (reading further back if needed).
pub fn trailing_zeros(path: &Path, size: u64, tail: &[u8]) -> u64 {
    let t = (tail.len() - rstrip_zeros(tail).len()) as u64;
    if (t as usize) < tail.len() {
        return t;
    }
    let mut pos = size - tail.len() as u64;
    let mut fh = match File::open(path) {
        Ok(f) => f,
        Err(_) => return t,
    };
    while pos > 0 {
        let step = (crate::formats::CHUNK as u64).min(pos);
        let buf = match read_at(&mut fh, pos - step, step as usize) {
            Ok(b) => b,
            Err(_) => return t,
        };
        let kept = rstrip_zeros(&buf).len() as u64;
        if kept > 0 {
            return size - (pos - step + kept);
        }
        pos -= step;
    }
    size
}

fn tiff_typesize(typ: u16) -> u64 {
    match typ {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => 0,
    }
}

/// Validate a TIFF/camera-RAW container by following its own pointers.
fn check_tiff(head: &[u8], size: u64) -> Finding {
    if head.len() < 8 {
        return ("TRUNCATED", "TIFF header incomplete".to_string());
    }
    let le = &head[0..2] == b"II";
    let u16_at = |b: &[u8], o: usize| -> Option<u64> {
        let s = b.get(o..o + 2)?;
        Some(if le { u16::from_le_bytes([s[0], s[1]]) } else { u16::from_be_bytes([s[0], s[1]]) } as u64)
    };
    let u32_at = |b: &[u8], o: usize| -> Option<u64> {
        let s = b.get(o..o + 4)?;
        Some(if le {
            u32::from_le_bytes([s[0], s[1], s[2], s[3]])
        } else {
            u32::from_be_bytes([s[0], s[1], s[2], s[3]])
        } as u64)
    };
    let ifd = match u32_at(head, 4) {
        Some(v) => v,
        None => return ok(),
    };
    if ifd == 0 || ifd >= size {
        return ("TRUNCATED", format!("IFD0 offset {} lies outside a {} byte file", commas(ifd), commas(size)));
    }
    if &head[2..4] != b"*\x00" && &head[2..4] != b"\x00*" {
        // Panasonic and Olympus RAW: TIFF-shaped, but their strip tags do not
        // mean what TIFF's do. Only the header and IFD0 position are checked.
        return ok();
    }
    let len = head.len() as u64;
    if ifd + 2 > len {
        return ok(); // IFD beyond our read window
    }
    let ifd = ifd as usize;
    let count = match u16_at(head, ifd) {
        Some(c) => c as usize,
        None => return ok(),
    };
    if (ifd + 2 + count * 12) as u64 > len {
        return ok();
    }
    let values = |off: u64, typ: u16, cnt: u64, ts: u64| -> Vec<u64> {
        if off + ts * cnt > len {
            return Vec::new();
        }
        if typ != 3 && typ != 4 {
            return Vec::new();
        }
        let off = off as usize;
        (0..cnt as usize)
            .filter_map(|i| if typ == 3 { u16_at(head, off + i * 2) } else { u32_at(head, off + i * 4) })
            .collect()
    };
    let mut offsets: Vec<u64> = Vec::new();
    let mut lengths: Vec<u64> = Vec::new();
    for i in 0..count {
        let e = ifd + 2 + i * 12;
        let (tag, typ, cnt) = match (u16_at(head, e), u16_at(head, e + 2), u32_at(head, e + 4)) {
            (Some(a), Some(b), Some(c)) => (a, b as u16, c),
            _ => return ok(),
        };
        let ts = tiff_typesize(typ);
        if ts == 0 {
            continue;
        }
        let total = ts * cnt;
        let vals = if total > 4 {
            let voff = match u32_at(head, e + 8) {
                Some(v) => v,
                None => return ok(),
            };
            if voff + total > size {
                return (
                    "TRUNCATED",
                    format!("tag {tag} value block runs {} bytes past end of file", commas(voff + total - size)),
                );
            }
            values(voff, typ, cnt, ts)
        } else {
            values((e + 8) as u64, typ, cnt, ts)
        };
        if tag == 273 || tag == 324 {
            offsets = vals; // StripOffsets / TileOffsets
        } else if tag == 279 || tag == 325 {
            lengths = vals; // StripByteCounts / TileByteCounts
        }
    }
    if !offsets.is_empty()
        && !lengths.is_empty()
        && offsets.len() == lengths.len()
        && !offsets.contains(&0xFFFF_FFFF)
        && !lengths.contains(&0xFFFF_FFFF)
    {
        let end = offsets.iter().zip(&lengths).map(|(o, n)| o + n).max().unwrap_or(0);
        if end > size {
            return (
                "TRUNCATED",
                format!(
                    "image data ends at {} but the file stops at {} (short {} bytes)",
                    commas(end),
                    commas(size),
                    commas(end - size)
                ),
            );
        }
    }
    ok()
}

/// Offset just past the main picture's end-of-image marker, or None.
pub fn jpeg_image_end(path: &Path, size: u64) -> Option<u64> {
    if size > JPEG_PARSE_LIMIT {
        return None;
    }
    let d = std::fs::read(path).ok()?;
    let n = d.len();
    if n < 2 || d[0] != 0xFF || d[1] != 0xD8 {
        return None;
    }
    let mut pos = 2usize;
    for _ in 0..100_000 {
        if pos >= n || d[pos] != 0xFF {
            return None; // no marker where one must be
        }
        while pos < n && d[pos] == 0xFF {
            pos += 1; // fill bytes before a marker
        }
        if pos >= n {
            return None;
        }
        let marker = d[pos];
        pos += 1;
        if marker == 0xD9 {
            return Some(pos as u64);
        }
        if (0xD0..=0xD8).contains(&marker) || marker == 0x01 {
            continue; // markers with no length field
        }
        if pos + 2 > n {
            return None;
        }
        let seg = ((d[pos] as usize) << 8) | d[pos + 1] as usize;
        if seg < 2 {
            return None;
        }
        pos += seg;
        if marker == 0xDA {
            // Start of scan: compressed data runs to the next real marker.
            loop {
                if pos >= n {
                    return None;
                }
                let k = match d[pos..].iter().position(|&c| c == 0xFF) {
                    Some(i) => pos + i,
                    None => return None,
                };
                if k + 1 >= n {
                    return None;
                }
                let nxt = d[k + 1];
                if nxt == 0x00 || (0xD0..=0xD7).contains(&nxt) {
                    pos = k + 2;
                } else if nxt == 0xFF {
                    pos = k + 1;
                } else {
                    pos = k;
                    break;
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Padding: zeros that are part of a format rather than damage.
// ---------------------------------------------------------------------------

const MP4_PAD: &[&[u8]] = &[b"free", b"skip", b"wide"];
const MP4_CONTAINERS: &[&[u8]] = &[
    b"moov", b"trak", b"mdia", b"minf", b"stbl", b"udta", b"edts", b"dinf", b"mvex", b"moof", b"traf", b"meta",
];
/// Sample formats whose silence (or black) is literally zero bytes.
const MP4_UNCOMPRESSED: &[&[u8]] = &[b"sowt", b"twos", b"lpcm", b"in24", b"in32", b"fl32", b"fl64", b"NONE", b"raw "];

pub const PCM_REASON: &str = "uncompressed (PCM) audio - its silence is stored as zeros, so the hole check cannot be used here; Phase 4's video decode covers this file";

/// Where zeros are part of the format rather than damage.
///
/// Returns (ranges, reason): byte ranges the hole scan must not count, and a
/// reason when zeros are normal content right through the file. A structure
/// that cannot be followed simply excuses nothing; ranges found before a
/// read error are kept, exactly as the Python version keeps them.
pub fn padding_ranges(fmt: &str, path: &Path, head: &[u8], size: u64) -> (Vec<(u64, u64)>, Option<&'static str>) {
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    let mut pcm = false;
    if let Ok(mut fh) = File::open(path) {
        let id3 = id3_end(head) as u64;
        let _ = (|| -> Option<()> {
            if id3 > 0 {
                ranges.push(id3_padding(&mut fh, id3).ok()?);
            }
            if fmt == "flac" {
                flac_padding(&mut fh, id3, size, &mut ranges)?;
            } else if fmt == "isobmff" {
                pcm = mp4_padding(&mut fh, size, &mut ranges)?;
            } else if fmt == "matroska" {
                mkv_padding(&mut fh, size, &mut ranges)?;
            }
            Some(())
        })();
    }
    if pcm {
        return (Vec::new(), Some(PCM_REASON));
    }
    ranges.retain(|r| r.1 > r.0);
    (ranges, None)
}

/// The zeros at the end of an ID3v2 tag that ends at `end`.
fn id3_padding(fh: &mut File, end: u64) -> io::Result<(u64, u64)> {
    let start = 10u64.max(end.saturating_sub(1 << 20));
    let body = read_at(fh, start, (end - start) as usize)?;
    Ok((start + rstrip_zeros(&body).len() as u64, start + body.len() as u64))
}

/// PADDING metadata blocks of a FLAC stream whose 'fLaC' is at `base`.
fn flac_padding(fh: &mut File, base: u64, size: u64, out: &mut Vec<(u64, u64)>) -> Option<()> {
    let mut pos = base + 4;
    for _ in 0..1000 {
        let h = read_at(fh, pos, 4).ok()?;
        if h.len() < 4 {
            return Some(());
        }
        let length = ((h[1] as u64) << 16) | ((h[2] as u64) << 8) | h[3] as u64;
        if h[0] & 0x7F == 1 {
            out.push((pos + 4, (pos + 4 + length).min(size)));
        }
        pos += 4 + length;
        if h[0] & 0x80 != 0 || pos >= size {
            return Some(());
        }
    }
    Some(())
}

/// Collect padding boxes into `out`; Some(true) if any track is uncompressed.
fn mp4_padding(fh: &mut File, size: u64, out: &mut Vec<(u64, u64)>) -> Option<bool> {
    let mut pcm = false;
    let mut pos: u64 = 0;
    for _ in 0..100_000 {
        if pos + 8 > size {
            break;
        }
        let h = read_at(fh, pos, 16).ok()?;
        if h.len() < 8 {
            return None;
        }
        let mut boxlen = be32(&h);
        let typ = &h[4..8];
        let mut hdr = 8u64;
        if boxlen == 1 && h.len() >= 16 {
            boxlen = be64(&h[8..]);
            hdr = 16;
        } else if boxlen == 0 {
            boxlen = size - pos;
        }
        if boxlen < hdr {
            break;
        }
        if MP4_PAD.contains(&typ) {
            out.push((pos + hdr, pos.saturating_add(boxlen).min(size)));
        } else if typ == b"moov" && boxlen <= MOOV_LIMIT {
            let body = read_at(fh, pos + hdr, (boxlen - hdr) as usize).ok()?;
            pcm = mp4_walk(&body, pos + hdr, out, 0) || pcm;
        }
        match pos.checked_add(boxlen) {
            Some(p) => pos = p,
            None => break, // a nonsense 64-bit size: the walk simply ends
        }
    }
    Some(pcm)
}

/// Walk the boxes in buf (file offset `base`); true if PCM audio is seen.
fn mp4_walk(buf: &[u8], base: u64, out: &mut Vec<(u64, u64)>, depth: u32) -> bool {
    let mut pcm = false;
    let mut pos: usize = 0;
    let n = buf.len();
    while pos + 8 <= n && depth < 12 {
        let mut boxlen = be32(&buf[pos..]);
        let typ = &buf[pos + 4..pos + 8];
        let mut hdr = 8u64;
        if boxlen == 1 && pos + 16 <= n {
            boxlen = be64(&buf[pos + 8..]);
            hdr = 16;
        } else if boxlen == 0 {
            boxlen = (n - pos) as u64;
        }
        if boxlen < hdr || pos as u64 + boxlen > n as u64 {
            break;
        }
        let mut start = pos + hdr as usize;
        let end = pos + boxlen as usize;
        if MP4_PAD.contains(&typ) {
            out.push((base + start as u64, base + end as u64));
        } else if MP4_CONTAINERS.contains(&typ) {
            if typ == b"meta" && buf.get(start..start + 4) == Some(b"\x00\x00\x00\x00") {
                start += 4; // ISO 'meta' is a full box; QuickTime's is not
            }
            let inner = if start <= end { &buf[start..end] } else { &buf[0..0] };
            pcm = mp4_walk(inner, base + start as u64, out, depth + 1) || pcm;
        } else if typ == b"stsd" {
            let mut p = start + 8; // version/flags, entry count
            while p + 8 <= end {
                if MP4_UNCOMPRESSED.contains(&&buf[p + 4..p + 8]) {
                    pcm = true;
                }
                let esize = be32(&buf[p..]) as usize;
                if esize < 8 {
                    break;
                }
                p += esize;
            }
        }
        pos += boxlen as usize;
    }
    pcm
}

/// Void elements in a Matroska/WebM segment's first 4 MiB.
fn mkv_padding(fh: &mut File, size: u64, out: &mut Vec<(u64, u64)>) -> Option<()> {
    let buf = read_at(fh, 0, size.min(4 << 20) as usize).ok()?;
    let (_, n) = ebml_vint(&buf, 0, true)?;
    let (hlen, m) = ebml_vint(&buf, n, false)?;
    let mut pos = usize::try_from(n as u64 + m as u64 + hlen).ok()?;
    let (seg, n) = ebml_vint(&buf, pos, true)?;
    if seg != 0x1853_8067 {
        return Some(()); // Segment
    }
    let (_, m) = ebml_vint(&buf, pos + n, false)?;
    pos += n + m;
    while pos + 2 <= buf.len() {
        let (eid, n) = ebml_vint(&buf, pos, true)?;
        let (length, m) = ebml_vint(&buf, pos + n, false)?;
        if length == (1u64 << (7 * m)) - 1 {
            return Some(()); // unknown size: cannot step over it
        }
        let start = (pos + n + m) as u64;
        if eid == 0xEC {
            out.push((start, start.saturating_add(length).min(size)));
        }
        pos = usize::try_from(start.checked_add(length)?).ok()?;
    }
    Some(())
}
