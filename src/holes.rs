//! The hole scan: find runs of zero bytes shaped like unreadable disk
//! sectors. A port of the Python version's scan_zero_run, checked against a
//! slow byte-by-byte reference on random files (see the tests below and
//! the built-in self-test).

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// Sector geometry and read size. Tests vary these; the probe uses DEFAULT.
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub chunk: usize,
    pub sector: u64,
    pub slop: u64,
}

/// A hole ddrescue leaves is whole 512-byte sectors of zeros starting on a
/// sector boundary of the file, give or take a few zero bytes of the file's
/// own on either side. Natural zero runs land anywhere.
pub const DEFAULT: Geometry = Geometry { chunk: crate::formats::CHUNK, sector: 512, slop: 8 };

/// The longest piece of [start, end) left after removing the skip ranges:
/// (length, start of that piece).
pub fn outside(start: u64, end: u64, skip: &[(u64, u64)]) -> (u64, u64) {
    let mut pieces = vec![(start, end)];
    for &(a, b) in skip {
        let mut kept = Vec::with_capacity(pieces.len() + 1);
        for &(x, y) in &pieces {
            if b <= x || a >= y {
                kept.push((x, y));
                continue;
            }
            if a > x {
                kept.push((x, a));
            }
            if b < y {
                kept.push((b, y));
            }
        }
        pieces = kept;
    }
    // max by (length, -start): the longest, and among equals the earliest.
    let mut best: Option<(u64, u64)> = None;
    for (x, y) in pieces {
        let len = y - x;
        match best {
            None => best = Some((len, x)),
            Some((bl, bx)) => {
                if len > bl || (len == bl && x < bx) {
                    best = Some((len, x));
                }
            }
        }
    }
    best.unwrap_or((0, start))
}

/// Bytes of a run lying on whole sectors, or 0 if it is not hole-shaped.
pub fn sector_core(start: u64, length: u64, g: Geometry) -> u64 {
    let end = start + length;
    let a = start.div_ceil(g.sector) * g.sector;
    let b = end / g.sector * g.sector;
    if b <= a || a - start > g.slop || end - b > g.slop {
        return 0;
    }
    b - a
}

fn leading_zeros_from(buf: &[u8], from: usize, to: usize) -> usize {
    buf[from..to].iter().take_while(|&&c| c == 0).count()
}

fn find_zero_run(buf: &[u8], from: usize, to: usize, len: usize) -> Option<usize> {
    // The first position in [from, to - len] that starts `len` zero bytes.
    if len == 0 || to < from || to - from < len {
        return None;
    }
    let mut i = from;
    while i + len <= to {
        // Jump to the next zero byte.
        match buf[i..to].iter().position(|&c| c == 0) {
            None => return None,
            Some(off) => i += off,
        }
        if i + len > to {
            return None;
        }
        // Measure this run of zeros (up to len).
        let run = leading_zeros_from(buf, i, i + len);
        if run == len {
            return Some(i);
        }
        i += run + 1; // the byte after the run is non-zero
    }
    None
}

/// Longest hole-shaped run of zeros with real data after it: (run, offset).
///
/// A run counts only if, once any known padding (`skip`) is taken out, at
/// least `threshold` bytes of it sit on whole disk sectors. A run that
/// reaches the end of the file is never counted: that is padding or a
/// truncation, not a hole.
pub fn scan_zero_run(path: &Path, threshold: u64, skip: &[(u64, u64)], g: Geometry) -> io::Result<(u64, u64)> {
    let mut fh = File::open(path)?;
    scan_reader(&mut fh, threshold, skip, g)
}

pub fn scan_reader<R: Read>(fh: &mut R, threshold: u64, skip: &[(u64, u64)], g: Geometry) -> io::Result<(u64, u64)> {
    let mut best: u64 = 0;
    let mut best_off: u64 = 0;
    let mut run: u64 = 0; // zeros at the end of what has been read so far
    let mut run_start: u64 = 0;
    let mut off: u64 = 0;
    let need = threshold.max(1) as usize;

    let consider = |length: u64, start: u64, best: &mut u64, best_off: &mut u64| {
        let (length, start) = if skip.is_empty() { (length, start) } else { outside(start, start + length, skip) };
        if sector_core(start, length, g) >= need as u64 && length > *best {
            *best = length;
            *best_off = start;
        }
    };

    let mut buf = vec![0u8; g.chunk.max(1)];
    loop {
        // Fill the buffer completely (a short read is not the end of file).
        let mut n = 0;
        while n < buf.len() {
            let got = fh.read(&mut buf[n..])?;
            if got == 0 {
                break;
            }
            n += got;
        }
        if n == 0 {
            break;
        }
        let b = &buf[..n];
        let lead = leading_zeros_from(b, 0, n);
        if lead == n {
            // All zeros: whatever run is open simply grows.
            if run == 0 {
                run_start = off;
            }
            run += n as u64;
            off += n as u64;
            continue;
        }
        // Real data starts at `lead`, so the open run plus this chunk's
        // leading zeros has data after it: a candidate.
        if run > 0 || lead > 0 {
            if run == 0 {
                run_start = off;
            }
            run += lead as u64;
            consider(run, run_start, &mut best, &mut best_off);
        }
        run = 0;
        let trail = n - crate::checks::rstrip_zeros(b).len();
        let (mut i, j) = (lead, n - trail);
        while let Some(k) = find_zero_run(b, i, j, need) {
            // b[k-1] is data (else the search would have stopped earlier) and
            // the run ends before j, which is data too: a closed run.
            let e = k + leading_zeros_from(b, k, j);
            consider((e - k) as u64, off + k as u64, &mut best, &mut best_off);
            i = e;
        }
        if trail > 0 {
            run = trail as u64;
            run_start = off + (n - trail) as u64;
        }
        off += n as u64;
        if n < buf.len() {
            break; // end of file
        }
    }
    // Anything still open here ran to the end of the file: not a hole.
    Ok((best, best_off))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slow, obviously-correct version: every maximal zero run with
    /// real data after it.
    fn reference(data: &[u8], thr: u64, skip: &[(u64, u64)], g: Geometry) -> (u64, u64) {
        let (mut best, mut best_off) = (0u64, 0u64);
        let n = data.len();
        let mut i = 0;
        while i < n {
            if data[i] != 0 {
                i += 1;
                continue;
            }
            let mut j = i;
            while j < n && data[j] == 0 {
                j += 1;
            }
            if j < n {
                let (mut length, mut start) = ((j - i) as u64, i as u64);
                if !skip.is_empty() {
                    let r = outside(start, start + length, skip);
                    length = r.0;
                    start = r.1;
                }
                if sector_core(start, length, g) >= thr && length > best {
                    best = length;
                    best_off = start;
                }
            }
            i = j;
        }
        (best, best_off)
    }

    /// A small deterministic random number generator (no crates needed).
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    #[test]
    fn agrees_with_slow_reference_on_random_files() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for trial in 0..6000 {
            let g = match rng.below(4) {
                0 => Geometry { chunk: 1, sector: 8, slop: 2 },
                1 => Geometry { chunk: 1, sector: 16, slop: 3 },
                2 => Geometry { chunk: 1, sector: 4, slop: 0 },
                _ => Geometry { chunk: 1, sector: 32, slop: 5 },
            };
            let chunk = [1usize, 2, 3, 5, 7, 16, 64, 100, 4096][rng.below(9) as usize];
            let g = Geometry { chunk, ..g };
            let mut data: Vec<u8> = Vec::new();
            for _ in 0..rng.below(41) {
                let r = rng.below(10);
                if r < 4 {
                    data.extend(std::iter::repeat(0).take(1 + rng.below(80) as usize));
                } else if r < 5 {
                    let pad = ((g.sector - data.len() as u64 % g.sector) % g.sector) as usize;
                    for _ in 0..pad {
                        data.push(1 + rng.below(255) as u8);
                    }
                    let z = g.sector * (1 + rng.below(6)) + rng.below(4);
                    data.extend(std::iter::repeat(0).take(z as usize));
                } else {
                    for _ in 0..1 + rng.below(30) {
                        data.push(1 + rng.below(255) as u8);
                    }
                }
            }
            let thr = [1u64, 2, 4, 8, 16, 24, 40][rng.below(7) as usize];
            let mut skip = Vec::new();
            if !data.is_empty() && rng.below(10) < 3 {
                for _ in 0..1 + rng.below(3) {
                    let a = rng.below(data.len() as u64 + 1);
                    skip.push((a, a + 1 + rng.below(60)));
                }
            }
            let got = scan_reader(&mut &data[..], thr, &skip, g).unwrap();
            let want = reference(&data, thr, &skip, g);
            assert_eq!(got, want, "trial {trial}: chunk {} sector {} thr {thr} skip {skip:?}", g.chunk, g.sector);
        }
    }
}
