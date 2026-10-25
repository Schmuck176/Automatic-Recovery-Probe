//! Formatting that matches the Python version character for character.
//!
//! The Rust and Python probes are compared file by file, detail text
//! included, so numbers, sizes, dates and CSV quoting all follow Python's
//! rules. Rust's `{:.N}` rounding was checked against Python's `{:.Nf}` on
//! 32,000 values with no differences (both round exact ties to even).

/// `f"{n:,}"` for a non-negative integer.
pub fn commas(n: u64) -> String {
    group(&n.to_string())
}

fn group(digits: &str) -> String {
    let b = digits.as_bytes();
    let mut out = String::with_capacity(b.len() + b.len() / 3);
    for (i, c) in b.iter().enumerate() {
        if i > 0 && (b.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}

/// `f"{v:.{prec}f}"`
pub fn fixed(v: f64, prec: usize) -> String {
    format!("{:.*}", prec, v)
}

/// `f"{v:,.{prec}f}"`
pub fn fixed_commas(v: f64, prec: usize) -> String {
    let s = fixed(v, prec);
    let (neg, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.as_str()),
    };
    let (int, frac) = match body.find('.') {
        Some(i) => (&body[..i], &body[i..]),
        None => (body, ""),
    };
    format!("{}{}{}", if neg { "-" } else { "" }, group(int), frac)
}

/// The Python probe's `_size()`: 1.5 GB / 300 MB / 12 KB.
pub fn size(n: u64) -> String {
    if n >= 1 << 30 {
        format!("{} GB", fixed_commas(n as f64 / (1u64 << 30) as f64, 1))
    } else if n >= 1 << 20 {
        format!("{} MB", fixed_commas(n as f64 / (1u64 << 20) as f64, 0))
    } else {
        format!("{} KB", fixed_commas(n as f64 / 1024.0, 0))
    }
}

/// Python's `bytes.decode("ascii", "replace")`: ASCII kept, every other
/// byte becomes U+FFFD.
pub fn ascii_replace(b: &[u8]) -> String {
    b.iter()
        .map(|&c| if c < 0x80 { c as char } else { '\u{FFFD}' })
        .collect()
}

/// Seconds since 1970 as Python's `datetime.fromtimestamp` sees them: the
/// fraction is rounded to microseconds first, which can carry into the next
/// second.
fn python_seconds(ts: f64) -> i64 {
    let whole = ts.trunc();
    let mut secs = whole as i64;
    let us = ((ts - whole) * 1e6).round_ties_even();
    if us >= 1e6 {
        secs += 1;
    } else if us < 0.0 {
        secs -= 1;
    }
    secs
}

/// `datetime.fromtimestamp(ts).strftime("%Y-%m-%d")`, in local time.
pub fn local_date(ts: f64) -> String {
    match crate::sys::local_time(python_seconds(ts)) {
        Some(t) => format!("{:04}-{:02}-{:02}", t[0], t[1], t[2]),
        None => "unknown".to_string(),
    }
}

/// `datetime.fromtimestamp(ts).isoformat(" ", "seconds")`, in local time.
pub fn local_datetime(ts: f64) -> String {
    match crate::sys::local_time(python_seconds(ts)) {
        Some(t) => format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t[0], t[1], t[2], t[3], t[4], t[5]),
        None => String::new(),
    }
}

/// This PC's offset from UTC at a moment (seconds since 1970), in seconds.
fn offset_at(secs: i64) -> i64 {
    match crate::sys::local_time(secs) {
        Some(t) => {
            crate::exfat::days_from_civil(t[0] as i64, t[1] as u32, t[2] as u32) * 86_400
                + t[3] as i64 * 3600
                + t[4] as i64 * 60
                + t[5] as i64
                - secs
        }
        None => 0,
    }
}

/// A moment as the program records it (ticks, safety checks): local date
/// and time with its offset from UTC, "2026-10-25 01:30:00 +01:00". Local
/// time alone repeats an hour when the clocks go back; with the offset no
/// two moments are written the same, and `instant` puts them in order.
pub fn when_of(secs: i64) -> String {
    match crate::sys::local_time(secs) {
        Some(t) => {
            let off = offset_at(secs);
            let (sign, a) = if off < 0 { ('-', -off) } else { ('+', off) };
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {sign}{:02}:{:02}",
                t[0],
                t[1],
                t[2],
                t[3],
                t[4],
                t[5],
                a / 3600,
                a % 3600 / 60
            )
        }
        None => String::new(),
    }
}

/// Seconds since 1970-01-01 as a calendar date and time (no time zone):
/// (year, month, day, hour, minute, second). The inverse of
/// exfat::days_from_civil, with the time of day.
pub fn civil(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m as u32, d as u32, (rest / 3600) as u32, (rest % 3600 / 60) as u32, (rest % 60) as u32)
}

/// A moment in UTC as videos and FFmpeg write it: "2019-07-14T15:20:05Z".
pub fn utc_iso(secs: i64) -> String {
    let (y, m, d, hh, mm, ss) = civil(secs);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Now, as `when_of` writes it.
pub fn when_now() -> String {
    when_of(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0))
}

/// The moment a recorded date and time stands for, in seconds since 1970.
/// One without an offset (recorded before 22 October 2026, outside the hour
/// the clocks go back) is taken as this PC's local time.
pub fn instant(when: &str) -> Option<i64> {
    let w = when.trim();
    let b = w.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b' ' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |a: usize, z: usize| -> Option<i64> {
        let s = w.get(a..z)?;
        if s.bytes().all(|c| c.is_ascii_digit()) {
            s.parse().ok()
        } else {
            None
        }
    };
    let (y, mo, d, h, mi, s) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    let naive = crate::exfat::days_from_civil(y, mo as u32, d as u32) * 86_400 + h * 3600 + mi * 60 + s;
    let rest = w[19..].trim();
    if rest.is_empty() {
        // The offset in force then (found from a first guess).
        return Some(naive - offset_at(naive - offset_at(naive)));
    }
    let r = rest.as_bytes();
    if r.len() != 6 || !(r[0] == b'+' || r[0] == b'-') || r[3] != b':' {
        return None;
    }
    let digits = |s: &str| -> Option<i64> { s.bytes().all(|c| c.is_ascii_digit()).then(|| s.parse().ok()).flatten() };
    let oh = digits(rest.get(1..3)?)?;
    let om = digits(rest.get(4..6)?)?;
    let off = (oh * 3600 + om * 60) * if r[0] == b'-' { -1 } else { 1 };
    Some(naive - off)
}

/// A recorded moment as it is shown: its local date and time, without the
/// offset.
pub fn when_shown(when: &str) -> &str {
    let w = when.trim();
    match w.get(..19) {
        Some(day_and_time) if w.len() > 19 && instant(w).is_some() => day_and_time,
        _ => w,
    }
}

/// `datetime.now()` formatted as `%Y%m%d-%H%M`, for default file names.
pub fn now_stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    match crate::sys::local_time(now) {
        Some(t) => format!("{:04}{:02}{:02}-{:02}{:02}", t[0], t[1], t[2], t[3], t[4]),
        None => "00000000-0000".to_string(),
    }
}

/// One CSV field, quoted the way Python's `csv` module quotes it
/// (QUOTE_MINIMAL with the excel dialect).
pub fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Read back CSV text written with `csv_row` (quoted fields may hold commas,
/// quotes and line breaks). A byte-order mark at the start is skipped.
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let _ = for_each_csv_row(text.as_bytes(), |r| rows.push(r));
    rows
}

/// The same, one record at a time, without holding the whole text.
pub fn for_each_csv_row(mut r: impl std::io::BufRead, mut f: impl FnMut(Vec<String>)) -> std::io::Result<()> {
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut after_quote = false;
    let mut any = false;
    let mut first = true;
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 {
            break;
        }
        let mut s = line.as_str();
        if first {
            s = s.strip_prefix('\u{FEFF}').unwrap_or(s);
            first = false;
        }
        for c in s.chars() {
            any = true;
            if quoted {
                if c == '"' {
                    quoted = false;
                    after_quote = true;
                } else {
                    field.push(c);
                }
                continue;
            }
            if after_quote {
                after_quote = false;
                if c == '"' {
                    // A doubled quote inside a quoted field.
                    field.push('"');
                    quoted = true;
                    continue;
                }
            }
            match c {
                '"' => quoted = true,
                ',' => row.push(std::mem::take(&mut field)),
                '\r' => {}
                '\n' => {
                    row.push(std::mem::take(&mut field));
                    f(std::mem::take(&mut row));
                    any = false;
                }
                c => field.push(c),
            }
        }
    }
    if any {
        row.push(field);
        f(row);
    }
    Ok(())
}

/// One CSV row, ending in `\r\n` like Python's excel dialect.
pub fn csv_row<S: AsRef<str>>(fields: &[S]) -> String {
    if fields.len() == 1 && fields[0].as_ref().is_empty() {
        return "\"\"\r\n".to_string();
    }
    let mut out = String::new();
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&csv_field(f.as_ref()));
    }
    out.push_str("\r\n");
    out
}

/// A JSON string literal (used by the machine-readable output modes).
pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_moments_are_written_for_ffmpeg() {
        assert_eq!(utc_iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_iso(1_563_117_605), "2019-07-14T15:20:05Z");
        assert_eq!(utc_iso(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc_iso(-86_400), "1969-12-31T00:00:00Z");
        assert_eq!(crate::exfat::days_from_civil(2024, 2, 29) * 86_400, 1_709_164_800);
        assert_eq!(utc_iso(1_709_164_800), "2024-02-29T00:00:00Z");
        for s in [-1_000_000_000i64, -1, 0, 59, 86_399, 951_782_399, 4_102_444_800, 1_563_117_605] {
            let (y, m, d, hh, mm, ss) = civil(s);
            assert_eq!(crate::exfat::days_from_civil(y, m, d) * 86_400 + (hh * 3600 + mm * 60 + ss) as i64, s);
        }
    }

    #[test]
    fn commas_like_python() {
        assert_eq!(commas(0), "0");
        assert_eq!(commas(999), "999");
        assert_eq!(commas(1000), "1,000");
        assert_eq!(commas(1234567), "1,234,567");
        assert_eq!(fixed_commas(1234.5, 1), "1,234.5");
        assert_eq!(fixed_commas(999.96, 1), "1,000.0");
    }

    #[test]
    fn sizes_like_python() {
        assert_eq!(size(0), "0 KB");
        assert_eq!(size(1536), "2 KB");
        assert_eq!(size(5 << 20), "5 MB");
        assert_eq!(size(88 << 30), "88.0 GB");
    }

    #[test]
    fn moments_are_put_in_order_across_the_clocks_going_back() {
        // 25 October 2026 in London: 01:30 BST, then the clocks go back at
        // 02:00 BST to 01:00 GMT, and 01:10 GMT is later than 01:30 BST.
        let first = instant("2026-10-25 01:30:00 +01:00").unwrap();
        let later = instant("2026-10-25 01:10:00 +00:00").unwrap();
        assert!(later > first, "{first} {later}");
        assert_eq!(later - first, 40 * 60);
        assert_eq!(instant("2026-10-21 23:25:46 -05:30"), Some(instant("2026-10-21 23:25:46 +00:00").unwrap() + 5 * 3600 + 1800));
        // Written and read back, whatever this PC's time zone.
        for secs in [0i64, 1_792_592_502, 1_792_890_000, 1_792_893_600, 1_798_761_600] {
            let w = when_of(secs);
            assert_eq!(instant(&w), Some(secs), "{w}");
            assert_eq!(when_shown(&w), &w[..19]);
            assert_eq!(instant(when_shown(&w)).map(|t| (t - secs).abs() <= 3600), Some(true), "{w}");
        }
        // As recorded before 22 October 2026: local time, no offset.
        assert!(instant("2026-10-19 23:25:46").is_some());
        assert_eq!(when_shown("2026-10-19 23:25:46"), "2026-10-19 23:25:46");
        for bad in ["", "unknown", "2026-13-01 00:00:00", "2026-10-25 01:30:00 +1:00", "2026-10-25 01:30:00 x01:00", "2026-10-2٥ 01:30:00"] {
            assert_eq!(instant(bad), None, "{bad}");
        }
        assert_eq!(when_shown("whenever"), "whenever");
    }

    #[test]
    fn csv_quoting_like_python() {
        assert_eq!(csv_row(&["a", "b,c", "d\"e", "f"]), "a,\"b,c\",\"d\"\"e\",f\r\n");
        assert_eq!(csv_row(&[""]), "\"\"\r\n");
        assert_eq!(csv_row(&["", "x"]), ",x\r\n");
    }

    #[test]
    fn csv_reads_back() {
        let rows = vec![
            vec!["FILE", "Photos\\a, b \"c\".jpg", "12", "copied", "", ""],
            vec!["NOTE", "x", "", "", "", "two\r\nlines"],
            vec!["", "", "", "", "", ""],
        ];
        let mut text = "\u{FEFF}".to_string();
        for r in &rows {
            text.push_str(&csv_row(r));
        }
        let back = parse_csv(&text);
        assert_eq!(back.len(), 3);
        for (a, b) in rows.iter().zip(&back) {
            assert_eq!(a.iter().map(|s| s.to_string()).collect::<Vec<_>>(), *b);
        }
        assert_eq!(parse_csv("a,b"), vec![vec!["a".to_string(), "b".to_string()]]);
        assert!(parse_csv("").is_empty());
    }
}
