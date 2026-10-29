//! The moments a QuickTime or MP4 video's index (its `moov`) says it was
//! made and changed: read, and moved in place.
//!
//! untrunc gives a video whose index was lost a new index built from a good
//! video from the same camera, and with it that video's moments: the date
//! Windows shows as "Media created", and that photo programs sort videos by.
//! They are moved to the repaired video's own, so it sorts where it belongs.
//!
//! Only fixed-size fields are changed (the made and changed moments of the
//! `mvhd`, and of every track's `tkhd` and `mdhd`; and a date written as text
//! in the `udta` and `meta` boxes, such as an iPhone's creation date), in
//! place: the file keeps its size, and nothing else in it moves.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Seconds from 1904-01-01 (QuickTime's start of time) to 1970-01-01.
const QT_EPOCH: i64 = 2_082_844_800;
/// An index bigger than this is not read (a real one is a few MB at most).
const MOOV_MAX: u64 = 256 << 20;
/// A date written as text is moved only within this of the video's own
/// made moment (any time zone, either way): it is the same moment, then.
const TEXT_NEAR: i64 = 26 * 3600;

/// Where the top-level `moov` box's contents are: (offset, length).
fn find_moov(f: &mut std::fs::File) -> Option<(u64, u64)> {
    let len = f.metadata().ok()?.len();
    let mut pos = 0u64;
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h[..8]).ok()?;
        let (size, header) = match u32::from_be_bytes(h[0..4].try_into().unwrap()) {
            0 => (len - pos, 8),
            1 => {
                f.read_exact(&mut h[8..16]).ok()?;
                (u64::from_be_bytes(h[8..16].try_into().unwrap()), 16)
            }
            s => (s as u64, 8),
        };
        if size < header || size > len - pos {
            return None;
        }
        if &h[4..8] == b"moov" {
            return Some((pos + header, size - header));
        }
        pos += size;
    }
    None
}

/// The boxes in `b[start..end]`: (type, contents start, end).
fn boxes(b: &[u8], start: usize, end: usize) -> Vec<([u8; 4], usize, usize)> {
    let mut out = Vec::new();
    let mut p = start;
    while p + 8 <= end {
        let kind: [u8; 4] = b[p + 4..p + 8].try_into().unwrap();
        let (size, header) = match u32::from_be_bytes(b[p..p + 4].try_into().unwrap()) {
            0 => ((end - p) as u64, 8usize),
            1 if p + 16 <= end => (u64::from_be_bytes(b[p + 8..p + 16].try_into().unwrap()), 16),
            1 => break,
            s => (s as u64, 8),
        };
        if size < header as u64 || size > (end - p) as u64 {
            break;
        }
        let size = size as usize;
        out.push((kind, p + header, p + size));
        p += size;
    }
    out
}

/// The moments in an index: (offset, width in bytes) of each made and
/// changed moment of the mvhd (first), and of each track's tkhd and mdhd.
fn moment_fields(b: &[u8]) -> Vec<(usize, usize)> {
    fn of(b: &[u8], c: usize, end: usize, out: &mut Vec<(usize, usize)>) {
        match b.get(c) {
            Some(0) if c + 12 <= end => out.extend([(c + 4, 4), (c + 8, 4)]),
            Some(1) if c + 20 <= end => out.extend([(c + 4, 8), (c + 12, 8)]),
            _ => {}
        }
    }
    let mut out = Vec::new();
    let top = boxes(b, 0, b.len());
    for (k, c, e) in &top {
        if k == b"mvhd" {
            of(b, *c, *e, &mut out);
        }
    }
    for (k, c, e) in &top {
        if k != b"trak" {
            continue;
        }
        for (k2, c2, e2) in boxes(b, *c, *e) {
            if &k2 == b"tkhd" {
                of(b, c2, e2, &mut out);
            } else if &k2 == b"mdia" {
                for (k3, c3, e3) in boxes(b, c2, e2) {
                    if &k3 == b"mdhd" {
                        of(b, c3, e3, &mut out);
                    }
                }
            }
        }
    }
    out
}

/// The `udta` and `meta` boxes of the index and its tracks: where dates
/// written as text are.
fn text_ranges(b: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (k, c, e) in boxes(b, 0, b.len()) {
        if &k == b"udta" || &k == b"meta" {
            out.push((c, e));
        } else if &k == b"trak" {
            for (k2, c2, e2) in boxes(b, c, e) {
                if &k2 == b"udta" || &k2 == b"meta" {
                    out.push((c2, e2));
                }
            }
        }
    }
    out
}

fn read_field(b: &[u8], at: usize, width: usize) -> u64 {
    if width == 4 {
        u32::from_be_bytes(b[at..at + 4].try_into().unwrap()) as u64
    } else {
        u64::from_be_bytes(b[at..at + 8].try_into().unwrap())
    }
}

/// "2019-07-14T15:20:05" (or with a space for the T) at `b[at..]`, as
/// seconds (no time zone).
fn text_moment(b: &[u8], at: usize) -> Option<i64> {
    let s = b.get(at..at + 19)?;
    let digits = |r: std::ops::Range<usize>| -> Option<u32> {
        let mut v = 0u32;
        for &c in &s[r] {
            if !c.is_ascii_digit() {
                return None;
            }
            v = v * 10 + (c - b'0') as u32;
        }
        Some(v)
    };
    if s[4] != b'-' || s[7] != b'-' || !(s[10] == b'T' || s[10] == b' ') || s[13] != b':' || s[16] != b':' {
        return None;
    }
    let (y, m, d) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
    let (hh, mm, ss) = (digits(11..13)?, digits(14..16)?, digits(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    Some(crate::exfat::days_from_civil(y as i64, m, d) * 86_400 + (hh * 3600 + mm * 60 + ss) as i64)
}

/// The moment the video's index says it was made (seconds since 1970, as
/// the index holds it: UTC by the rules, local time for some cameras), if
/// it holds one.
pub fn made(p: &Path) -> Option<i64> {
    let mut f = std::fs::File::open(p).ok()?;
    let (at, len) = find_moov(&mut f)?;
    if len > MOOV_MAX {
        return None;
    }
    let mut b = vec![0u8; len as usize];
    f.seek(SeekFrom::Start(at)).ok()?;
    f.read_exact(&mut b).ok()?;
    let (at, width) = *moment_fields(&b).first()?;
    moment_of(read_field(&b, at, width))
}

/// A moment as the index holds it (seconds since 1904), as seconds since
/// 1970: None when it is not set (0), or past any real one (a damaged
/// 64-bit field, its top bit set).
fn moment_of(v: u64) -> Option<i64> {
    match v {
        0 => None,
        v => i64::try_from(v).ok().map(|v| v - QT_EPOCH),
    }
}

/// Move every moment in the video's index by `by` seconds, in place: the
/// made and changed moments (those that are set), and dates written as
/// text near the made moment. Returns how many were moved.
pub fn shift(p: &Path, by: i64) -> Result<usize, String> {
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(p).map_err(|e| format!("could not open it: {e}"))?;
    let (at, len) = find_moov(&mut f).ok_or("it has no index (moov) to change")?;
    if len > MOOV_MAX {
        return Err("its index is too big to change".to_string());
    }
    let mut b = vec![0u8; len as usize];
    f.seek(SeekFrom::Start(at)).and_then(|_| f.read_exact(&mut b)).map_err(|e| format!("could not read its index: {e}"))?;
    let fields = moment_fields(&b);
    let Some(&(m_at, m_width)) = fields.first() else { return Ok(0) };
    let made = moment_of(read_field(&b, m_at, m_width));
    let mut moved = 0;
    for (pos, width) in fields {
        // (Unset, or past any real moment: left as it is.)
        let Some(n) = i64::try_from(read_field(&b, pos, width)).ok().filter(|&v| v != 0).and_then(|v| v.checked_add(by)) else {
            continue;
        };
        if n <= 0 || (width == 4 && n > u32::MAX as i64) {
            continue;
        }
        if width == 4 {
            b[pos..pos + 4].copy_from_slice(&(n as u32).to_be_bytes());
        } else {
            b[pos..pos + 8].copy_from_slice(&(n as u64).to_be_bytes());
        }
        moved += 1;
    }
    for (start, end) in text_ranges(&b) {
        let mut i = start;
        while i + 19 <= end {
            match text_moment(&b, i) {
                Some(t) if made.and_then(|m| t.checked_sub(m)).is_some_and(|d| d.unsigned_abs() <= TEXT_NEAR as u64) => {
                    let (y, mo, d, hh, mm, ss) = crate::fmt::civil(t + by);
                    let text = format!("{y:04}-{mo:02}-{d:02}{}{hh:02}:{mm:02}:{ss:02}", b[i + 10] as char);
                    if text.len() == 19 {
                        b[i..i + 19].copy_from_slice(text.as_bytes());
                        moved += 1;
                    }
                    i += 19;
                }
                _ => i += 1,
            }
        }
    }
    if moved > 0 {
        f.seek(SeekFrom::Start(at)).and_then(|_| f.write_all(&b)).map_err(|e| format!("could not write its index: {e}"))?;
        f.sync_all().map_err(|e| format!("could not write its index: {e}"))?;
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn header_v0(kind: &[u8; 4], made: u32, changed: u32, rest: usize) -> Vec<u8> {
        let mut body = vec![0u8, 0, 0, 0];
        body.extend_from_slice(&made.to_be_bytes());
        body.extend_from_slice(&changed.to_be_bytes());
        body.extend(vec![0u8; rest]);
        bx(kind, &body)
    }

    fn header_v1(kind: &[u8; 4], made: u64, changed: u64, rest: usize) -> Vec<u8> {
        let mut body = vec![1u8, 0, 0, 0];
        body.extend_from_slice(&made.to_be_bytes());
        body.extend_from_slice(&changed.to_be_bytes());
        body.extend(vec![0u8; rest]);
        bx(kind, &body)
    }

    /// A video as a phone writes it: ftyp, mdat, then moov with an mvhd, two
    /// tracks and an iPhone-style creation date as text.
    fn video(dir: &Path, made_qt: u32) -> std::path::PathBuf {
        let mut moov = header_v0(b"mvhd", made_qt, made_qt + 5, 88);
        for v1 in [false, true] {
            let tkhd = if v1 { header_v1(b"tkhd", made_qt as u64, made_qt as u64 + 5, 72) } else { header_v0(b"tkhd", made_qt, made_qt + 5, 72) };
            let mdhd = header_v0(b"mdhd", made_qt, 0, 8);
            let mdia = bx(b"mdia", &[mdhd, bx(b"hdlr", &[0u8; 24])].concat());
            moov.extend(bx(b"trak", &[tkhd, mdia].concat()));
        }
        let (y, m, d, hh, mm, ss) = crate::fmt::civil(made_qt as i64 - QT_EPOCH + 3600);
        let text = format!("....com.apple.quicktime.creationdate....{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}+0100 and 1999-01-01T00:00:00");
        moov.extend(bx(b"meta", text.as_bytes()));
        let mut file = bx(b"ftyp", b"qt  \0\0\0\0qt  ");
        file.extend(bx(b"mdat", &[7u8; 5000]));
        file.extend(bx(b"moov", &moov));
        let p = dir.join("v.mov");
        std::fs::write(&p, file).unwrap();
        p
    }

    #[test]
    fn a_videos_moments_are_read_and_moved_in_place() {
        let dir = std::env::temp_dir().join(format!("mp4time-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 2023-07-15T09:00:00Z
        let at = 1_689_411_600i64;
        let p = video(&dir, (at + QT_EPOCH) as u32);
        let before = std::fs::read(&p).unwrap();
        assert_eq!(made(&p), Some(at));
        // A day and two hours later.
        let by = 26 * 3600;
        let n = shift(&p, by).unwrap();
        // mvhd 2, tkhd 2 + 2, mdhd 1 + 1 (the unset changed moments stay 0), the text date 1.
        assert_eq!(n, 9);
        let after = std::fs::read(&p).unwrap();
        assert_eq!(after.len(), before.len());
        assert_eq!(made(&p), Some(at + by));
        let text = String::from_utf8_lossy(&after);
        assert!(text.contains("creationdate....2023-07-16T12:00:00+0100"), "{text}");
        // A date far from the video's own is not touched.
        assert!(text.contains("1999-01-01T00:00:00"));
        // Nothing outside the index changed.
        assert_eq!(after[..5000 + 28], before[..5000 + 28]);
        // Moved back, it is exactly as it was.
        assert_eq!(shift(&p, -by).unwrap(), 9);
        assert_eq!(std::fs::read(&p).unwrap(), before);
        // No index: nothing to read or move.
        let q = dir.join("not.mov");
        std::fs::write(&q, bx(b"mdat", &[1u8; 100])).unwrap();
        assert_eq!(made(&q), None);
        assert!(shift(&q, 60).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_moment_past_any_real_one_is_left_alone() {
        // A damaged 64-bit field (its top bit set): not a moment, nothing to
        // work out from it (before: a sum past what the number can hold).
        let dir = std::env::temp_dir().join(format!("mp4time-big-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |v: u64| {
            let mut moov = header_v1(b"mvhd", v, v, 88);
            moov.extend(bx(b"meta", b"2019-07-14T15:20:05"));
            let mut file = bx(b"ftyp", b"qt  \0\0\0\0qt  ");
            file.extend(bx(b"moov", &moov));
            let p = dir.join("v.mov");
            std::fs::write(&p, &file).unwrap();
            (p, file)
        };
        for v in [u64::MAX, 1 << 63, (1 << 63) + 5] {
            let (p, file) = write(v);
            assert_eq!(made(&p), None, "{v}");
            for by in [i64::MAX, i64::MIN, 3600, -3600] {
                assert_eq!(shift(&p, by).unwrap(), 0, "{v} {by}");
            }
            assert_eq!(std::fs::read(&p).unwrap(), file);
        }
        // The largest moment there can be: moved only where the result can be held.
        let (p, file) = write(i64::MAX as u64);
        assert_eq!(made(&p), Some(i64::MAX - QT_EPOCH));
        assert_eq!(shift(&p, i64::MAX).unwrap(), 0);
        assert_eq!(std::fs::read(&p).unwrap(), file);
        assert_eq!(shift(&p, -3600).unwrap(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_dates_are_read_only_when_whole() {
        assert_eq!(text_moment(b"2019-07-14T15:20:05", 0), Some(1_563_117_605));
        assert_eq!(text_moment(b"2019-07-14 15:20:05", 0), Some(1_563_117_605));
        assert_eq!(text_moment(b"2019-13-14T15:20:05", 0), None);
        assert_eq!(text_moment(b"2019-07-14T15:20:0", 0), None);
        assert_eq!(text_moment(b"2019/07/14T15:20:05", 0), None);
    }
}
