//! probe_file: the verdict for one file. A port of the Python version's
//! probe_file, step for step and in the same order, so the two give the
//! same answer for every file.

use crate::checks::{
    check_isobmff_index, check_matroska_size, check_trailer, jpeg_image_end, lstrip_len, padding_ranges, read_at,
    trailing_zeros,
};
use crate::fmt::{commas, fixed};
use crate::formats::{
    basename, expected_formats, is_compressed, is_raw_ext, is_structure_only, is_system_file, is_undecodable_ext,
    sniff, splitext_ext, zero_tail_means_damage, CHUNK, SYSTEM_NOTE,
};
use crate::holes::{outside, scan_zero_run, Geometry, DEFAULT};
use std::fs::File;
use std::io;
use std::path::Path;

/// One file's result.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub verdict: &'static str,
    pub detail: String,
    pub size: u64,
}

fn v(verdict: &'static str, detail: impl Into<String>, size: u64) -> Verdict {
    Verdict { verdict, detail: detail.into(), size }
}

/// The name Python gives an I/O error (FileNotFoundError, PermissionError...).
pub fn py_error_name(e: &io::Error) -> &'static str {
    match e.kind() {
        io::ErrorKind::NotFound => "FileNotFoundError",
        io::ErrorKind::PermissionDenied => "PermissionError",
        io::ErrorKind::IsADirectory => "IsADirectoryError",
        io::ErrorKind::TimedOut => "TimeoutError",
        _ => "OSError",
    }
}

fn unreadable(e: &io::Error, size: u64) -> Verdict {
    v("UNREADABLE", format!("{}: {e}", py_error_name(e)), size)
}

/// The verdict for one file. `hole_bytes` is the hole-size threshold;
/// `do_hole` turns the (slower) hole scan on.
pub fn probe_file(path: &Path, hole_bytes: u64, do_hole: bool) -> Verdict {
    probe_file_with(path, hole_bytes, do_hole, DEFAULT)
}

pub fn probe_file_with(path: &Path, hole_bytes: u64, do_hole: bool, geom: Geometry) -> Verdict {
    let size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) => return unreadable(&e, 0),
    };
    let path_str = path.to_string_lossy();
    let name = basename(&path_str);
    if is_system_file(&name.to_lowercase()) {
        return v("SKIPPED", SYSTEM_NOTE, size);
    }
    if size == 0 {
        return v("TRUNCATED", "zero bytes", 0);
    }

    let (head, tail) = match (|| -> io::Result<(Vec<u8>, Vec<u8>)> {
        let mut fh = File::open(path)?;
        let head = read_at(&mut fh, 0, (CHUNK as u64).min(size) as usize)?;
        let tail_len = 65600u64.min(size);
        let tail = read_at(&mut fh, size - tail_len, tail_len as usize)?;
        Ok((head, tail))
    })() {
        Ok(ht) => ht,
        Err(e) => return unreadable(&e, size),
    };

    let mut fmt = sniff(&head);
    let ext = splitext_ext(&path_str).to_lowercase();
    let is_raw = is_raw_ext(&ext);

    if fmt == "appledouble" || (fmt.is_empty() && name.starts_with("._")) {
        return v("SKIPPED", SYSTEM_NOTE, size);
    }

    if fmt.is_empty() {
        let lead = lstrip_len(&head);
        if lead >= 512 && (expected_formats(&ext).is_some() || is_raw) {
            return v(
                "MISMATCH",
                format!("the first {} bytes are zeros - the start of this file was never read off the disk", commas(lead as u64)),
                size,
            );
        }
        if is_raw {
            // A camera RAW type with no known signature: not evidence of damage.
            fmt = "raw";
        } else if expected_formats(&ext).is_some() {
            return v(
                "MISMATCH",
                format!("extension says {ext} but the header is not recognisable - destroyed header or misnamed file"),
                size,
            );
        } else {
            let shown = if ext.is_empty() { "no extension".to_string() } else { ext.clone() };
            return v("SKIPPED", format!("unrecognised format ({shown})"), size);
        }
    }

    if fmt == "sqlite" {
        let (verdict, detail) = crate::sqlite::check_sqlite(path);
        return v(if verdict.is_empty() { "OK" } else { verdict }, detail, size);
    }

    let (mut verdict, mut detail) = check_trailer(fmt, size, &head, &tail);
    let mut picture_end = size;
    if verdict == "TRUNCATED" && fmt == "jpeg" {
        if let Some(end) = jpeg_image_end(path, size) {
            // A complete picture with something stored after it.
            verdict = "";
            detail = String::new();
            picture_end = end;
        }
    }
    if verdict.is_empty() && fmt == "isobmff" {
        (verdict, detail) = check_isobmff_index(path, size);
    } else if verdict.is_empty() && fmt == "matroska" {
        (verdict, detail) = check_matroska_size(&head, size);
    }
    if !verdict.is_empty() {
        return v(verdict, detail, size);
    }

    // Extension-vs-content. RAW is exempt.
    if let Some(expected) = expected_formats(&ext) {
        if !expected.contains(&fmt) && !is_raw {
            return v("MISMATCH", format!("content is {fmt}, extension says {ext}"), size);
        }
    }

    if zero_tail_means_damage(fmt) {
        let zt = trailing_zeros(path, size, &tail);
        if zt >= hole_bytes {
            // Padding is excused here too, and so is PCM audio.
            let (pad, pcm) = padding_ranges(fmt, path, &head, size);
            let (run, start) = outside(size - zt, size, &pad);
            let aligned = start.div_ceil(geom.sector) * geom.sector;
            if pcm.is_none()
                && start + run == size
                && aligned - start <= geom.slop
                && aligned <= size
                && size - aligned >= hole_bytes
            {
                return v(
                    "TRUNCATED",
                    format!(
                        "the last {} bytes are zeros, starting on a disk-sector boundary - the end of this file was never read off the disk",
                        commas(run)
                    ),
                    size,
                );
            }
        }
    }

    let is_webp = fmt == "riff" && head.get(8..12) == Some(&b"WEBP"[..]);
    if do_hole && (is_compressed(fmt) || is_raw || is_webp) {
        let (mut skip, no_scan) = padding_ranges(fmt, path, &head, size);
        if picture_end < size {
            skip.push((picture_end, size));
        }
        if no_scan.is_none() {
            let (run, at) = match scan_zero_run(path, hole_bytes, &skip, geom) {
                Ok(r) => r,
                Err(e) => return unreadable(&e, size),
            };
            if run >= hole_bytes {
                let pct = 100.0 * at as f64 / size as f64;
                return v(
                    "HOLE",
                    format!("{} zero bytes at offset {} ({}% in)", commas(run), commas(at), fixed(pct, 0)),
                    size,
                );
            }
        }
    }

    // Nothing found - but for some formats "nothing found" is not "clean".
    if is_raw {
        return v(
            "SKIPPED",
            "camera RAW: header, strip offsets and zero-runs all pass, but the sensor data cannot be decoded here - confirm on the Mac",
            size,
        );
    }
    if is_undecodable_ext(&ext) {
        return v(
            "SKIPPED",
            "HEIC/AVIF: structure and zero-runs pass, but nothing on Windows decodes it - confirm on the Mac",
            size,
        );
    }
    if is_structure_only(fmt) {
        return v(
            "SKIPPED",
            format!("{fmt}: structure checked, pixel data cannot be verified here - open a sample to confirm"),
            size,
        );
    }
    v("OK", "", size)
}
