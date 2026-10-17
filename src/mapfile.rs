//! Reading a GNU ddrescue mapfile: which byte ranges of the old disk were
//! read ('+') and which never were ('-' bad sector, '/' non-scraped,
//! '*' non-trimmed, '?' non-tried).

use crate::fmt::commas;

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub pos: u64,
    pub size: u64,
    pub status: char,
}

#[derive(Clone, Debug)]
pub struct Mapfile {
    /// The "# Command line:" comment, if present.
    pub command_line: String,
    pub version: String,
    pub start_time: String,
    pub current_time: String,
    pub finished: bool,
    pub blocks: Vec<Block>,
    /// The copied range, from `-i`/`-s` on the command line when present.
    pub domain_start: u64,
    pub domain_end: u64,
    /// Output position minus input position (`-o`), usually 0.
    pub output_shift: i128,
    pub warnings: Vec<String>,
}

/// One area that was never read, in input (old disk) offsets.
#[derive(Clone, Debug, PartialEq)]
pub struct BadArea {
    pub start: u64,
    pub end: u64,
    pub status: char,
}

impl BadArea {
    pub fn len(&self) -> u64 {
        self.end - self.start
    }
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

pub fn status_name(c: char) -> &'static str {
    match c {
        '-' => "bad sector",
        '/' => "non-scraped",
        '*' => "non-trimmed",
        '?' => "never tried",
        '+' => "read OK",
        _ => "unknown",
    }
}

fn parse_num(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(h, 16).ok()
    } else if s.len() > 1 && s.starts_with('0') && s.bytes().all(|c| c.is_ascii_digit()) {
        u64::from_str_radix(&s[1..], 8).ok()
    } else {
        s.parse().ok()
    }
}

/// A ddrescue size argument: plain number, or with s (sectors) / k M G T
/// (powers of 1000) / Ki Mi Gi Ti (powers of 1024).
fn parse_size(s: &str, sector: u64) -> Option<u64> {
    let s = s.trim();
    let digits_end = s.find(|c: char| !(c.is_ascii_hexdigit() || c == 'x' || c == 'X')).unwrap_or(s.len());
    let (num, suffix) = if s.starts_with("0x") || s.starts_with("0X") { (s, "") } else { s.split_at(digits_end) };
    let n = parse_num(num)?;
    let mult: u64 = match suffix {
        "" => 1,
        "s" => sector,
        "k" | "K" => 1000,
        "M" => 1_000_000,
        "G" => 1_000_000_000,
        "T" => 1_000_000_000_000,
        "Ki" => 1 << 10,
        "Mi" => 1 << 20,
        "Gi" => 1 << 30,
        "Ti" => 1 << 40,
        _ => return None,
    };
    n.checked_mul(mult)
}

pub fn parse(text: &str) -> Result<Mapfile, String> {
    let mut m = Mapfile {
        command_line: String::new(),
        version: String::new(),
        start_time: String::new(),
        current_time: String::new(),
        finished: false,
        blocks: Vec::new(),
        domain_start: 0,
        domain_end: u64::MAX,
        output_shift: 0,
        warnings: Vec::new(),
    };
    let mut saw_status_line = false;
    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(c) = line.strip_prefix('#') {
            let c = c.trim();
            if let Some(v) = c.strip_prefix("Mapfile. Created by GNU ddrescue version") {
                m.version = v.trim().to_string();
            } else if let Some(v) = c.strip_prefix("Command line:") {
                m.command_line = v.trim().to_string();
            } else if let Some(v) = c.strip_prefix("Start time:") {
                m.start_time = v.trim().to_string();
            } else if let Some(v) = c.strip_prefix("Current time:") {
                m.current_time = v.trim().to_string();
            } else if c == "Finished" {
                m.finished = true;
            }
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if !saw_status_line {
            // current_pos  current_status  [current_pass]
            saw_status_line = true;
            if parts.len() >= 2 && parse_num(parts[1]).is_none() {
                continue;
            }
        }
        if parts.len() < 3 {
            return Err(format!("line {}: expected 'position size status', found '{line}'", lineno + 1));
        }
        let pos = parse_num(parts[0]).ok_or_else(|| format!("line {}: bad position '{}'", lineno + 1, parts[0]))?;
        let size = parse_num(parts[1]).ok_or_else(|| format!("line {}: bad size '{}'", lineno + 1, parts[1]))?;
        let status = parts[2].chars().next().unwrap_or('?');
        if !"+-/*?".contains(status) {
            return Err(format!("line {}: unknown status '{}'", lineno + 1, parts[2]));
        }
        m.blocks.push(Block { pos, size, status });
    }
    if m.blocks.is_empty() {
        return Err("no data lines - this does not look like a ddrescue mapfile".to_string());
    }
    for w in m.blocks.windows(2) {
        if w[0].pos.saturating_add(w[0].size) != w[1].pos {
            m.warnings.push(format!(
                "the blocks are not contiguous at offset {} - the mapfile may be damaged",
                commas(w[1].pos)
            ));
            break;
        }
    }
    apply_command_line(&mut m);
    Ok(m)
}

/// Read -i/--input-position, -o/--output-position and -s/--size from the
/// command line. Only these change which part of the disk the map covers.
fn apply_command_line(m: &mut Mapfile) {
    let args: Vec<String> = m.command_line.split_whitespace().map(|s| s.to_string()).collect();
    let sector = {
        let mut b = 512;
        for (i, a) in args.iter().enumerate() {
            let v = if a == "-b" || a == "--sector-size" {
                args.get(i + 1).cloned()
            } else {
                a.strip_prefix("--sector-size=").or_else(|| a.strip_prefix("-b").filter(|r| !r.is_empty())).map(|s| s.to_string())
            };
            if let Some(n) = v.and_then(|v| parse_num(&v)) {
                b = n;
            }
        }
        b
    };
    let mut ipos: Option<u64> = None;
    let mut opos: Option<u64> = None;
    let mut size: Option<u64> = None;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let mut take = |long: &str, short: &str| -> Option<String> {
            if a == short || a == long {
                i += 1;
                return args.get(i).cloned();
            }
            if let Some(v) = a.strip_prefix(&format!("{long}=")) {
                return Some(v.to_string());
            }
            if a.len() > short.len() && a.starts_with(short) && !a.starts_with("--") {
                return Some(a[short.len()..].to_string());
            }
            None
        };
        if let Some(v) = take("--input-position", "-i") {
            ipos = parse_size(&v, sector);
        } else if let Some(v) = take("--output-position", "-o") {
            opos = parse_size(&v, sector);
        } else if let Some(v) = take("--size", "-s") {
            size = parse_size(&v, sector);
        }
        i += 1;
    }
    let start = ipos.unwrap_or(0);
    m.domain_start = start;
    if let Some(s) = size {
        m.domain_end = start.saturating_add(s);
    } else {
        // No -s: the domain ends where the last block that is not an open
        // "rest of the disk" placeholder ends.
        m.domain_end = m
            .blocks
            .iter()
            .filter(|b| b.size < (1u64 << 60))
            .map(|b| b.pos.saturating_add(b.size))
            .max()
            .unwrap_or(0);
    }
    if let Some(o) = opos {
        m.output_shift = o as i128 - start as i128;
        if m.output_shift != 0 {
            m.warnings.push(format!(
                "ddrescue wrote the copy {} bytes away from the original positions (-o); positions are adjusted for that",
                m.output_shift
            ));
        }
    }
}

impl Mapfile {
    /// Every area inside the copied range that was never read, merged
    /// where adjacent, in input offsets.
    pub fn bad_areas(&self) -> Vec<BadArea> {
        let mut out: Vec<BadArea> = Vec::new();
        for b in &self.blocks {
            if b.status == '+' {
                continue;
            }
            let s = b.pos.max(self.domain_start);
            let e = b.pos.saturating_add(b.size).min(self.domain_end);
            if e <= s {
                continue;
            }
            if let Some(last) = out.last_mut() {
                if last.end == s && last.status == b.status {
                    last.end = e;
                    continue;
                }
            }
            out.push(BadArea { start: s, end: e, status: b.status });
        }
        out
    }

    /// Every area inside the copied range that was read ('+'), merged, in
    /// input offsets.
    pub fn read_regions(&self) -> Vec<(u64, u64)> {
        let mut out: Vec<(u64, u64)> = Vec::new();
        for b in self.blocks.iter().filter(|b| b.status == '+') {
            let s = b.pos.max(self.domain_start);
            let e = b.pos.saturating_add(b.size).min(self.domain_end);
            if e <= s {
                continue;
            }
            match out.last_mut() {
                Some(last) if last.1 >= s => last.1 = last.1.max(e),
                _ => out.push((s, e)),
            }
        }
        out
    }

    /// Bytes inside the copied range that were read.
    pub fn rescued_bytes(&self) -> u64 {
        self.blocks
            .iter()
            .filter(|b| b.status == '+')
            .map(|b| {
                let s = b.pos.max(self.domain_start);
                let e = b.pos.saturating_add(b.size).min(self.domain_end);
                e.saturating_sub(s)
            })
            .sum()
    }

    pub fn domain_size(&self) -> u64 {
        self.domain_end.saturating_sub(self.domain_start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Mapfile. Created by GNU ddrescue version 1.25
# Command line: ddrescue -v -f -s 1000204886016 -r 2 -c 32 -b 512 /dev/rdisk5 /dev/disk4 /Users/x/map.log
# Start time:   2024-04-21 02:14:00
# Current time: 2024-04-24 21:38:08
# Finished
# current_pos  current_status  current_pass
0xB60C195200     +               2
#      pos        size  status
0x00000000  0x10000000  +
0x10000000  0x00000200  -
0x10000200  0x00001000  +
0x10001200  0x00002000  -
0x10003200  0xE8D0DB2E00  +
0xE8E0DB6000  0x7FFFFF171F249FFF  ?
";

    #[test]
    fn reads_the_real_layout() {
        let m = parse(SAMPLE).unwrap();
        assert!(m.finished);
        assert_eq!(m.version, "1.25");
        assert_eq!(m.domain_end, 1_000_204_886_016);
        let bad = m.bad_areas();
        assert_eq!(bad.len(), 2, "{bad:?}");
        assert_eq!(bad[0], BadArea { start: 0x1000_0000, end: 0x1000_0200, status: '-' });
        assert_eq!(bad[1].len(), 0x2000);
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    }

    #[test]
    fn sizes_with_suffixes() {
        assert_eq!(parse_size("1TB", 512), None);
        assert_eq!(parse_size("2Ki", 512), Some(2048));
        assert_eq!(parse_size("10s", 512), Some(5120));
        assert_eq!(parse_size("0x200", 512), Some(512));
    }
}
