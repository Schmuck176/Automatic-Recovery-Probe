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
