//! Round trips over many random inputs (a fixed seed: the same every run),
//! for the small pieces everything else stands on: the lists (CSV), the
//! checksums and their saved state, the mapfile, the [R0012] numbers, names
//! made safe for Windows.

/// xorshift64*: enough randomness for tests, no crate needed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len() as u64) as usize]
    }
    fn text(&mut self, alphabet: &[&str], max: u64) -> String {
        (0..self.below(max + 1)).map(|_| *self.pick(alphabet)).collect()
    }
}

const ODD: &[&str] = &[
    "a", "Z", "0", "9", " ", ",", "\"", "\r", "\n", "\r\n", "\t", "é", "日", "\u{FEFF}", "'", ";", "\\", "/", ".", "[", "]",
    "R", "r", "(", ")", "😀", "\u{F00D}", "%", "*", "?", ":", "|", "<", ">",
];

#[test]
fn every_list_reads_back_as_it_was_written() {
    let mut r = Rng(0x5eed_0001);
    for _ in 0..3000 {
        let rows: Vec<Vec<String>> = (0..1 + r.below(6))
            .map(|_| (0..1 + r.below(7)).map(|_| r.text(ODD, 12)).collect())
            .collect();
        // As the program writes its lists: a byte-order mark, then the rows.
        let mut text = String::from("\u{FEFF}");
        for row in &rows {
            text.push_str(&crate::fmt::csv_row(row));
        }
        let mut back = Vec::new();
        crate::fmt::for_each_csv_row(std::io::BufReader::new(text.as_bytes()), |row| back.push(row)).unwrap();
        assert_eq!(back, rows, "{text:?}");
        // And read a few bytes at a time, as from a slow drive.
        let mut back2 = Vec::new();
        let slow = std::io::BufReader::with_capacity(3, text.as_bytes());
        crate::fmt::for_each_csv_row(slow, |row| back2.push(row)).unwrap();
        assert_eq!(back2, rows);
    }
}

#[test]
fn a_checksum_carried_on_from_its_saved_state_is_the_same() {
    let mut r = Rng(0x5eed_0002);
    for _ in 0..400 {
        let n = r.below(5000) as usize;
        let data: Vec<u8> = (0..n).map(|_| r.next() as u8).collect();
        let mut whole = crate::sha256::Sha256::new();
        whole.update(&data);
        let want = whole.finish_hex();
        // Fed in random pieces, zeros fed as zeros.
        let mut h = crate::sha256::Sha256::new();
        let mut at = 0;
        while at < n {
            let k = (1 + r.below(300) as usize).min(n - at);
            if data[at..at + k].iter().all(|&b| b == 0) {
                h.update_zeros(k as u64);
            } else {
                h.update(&data[at..at + k]);
            }
            at += k;
            // Saved and carried on wherever it may be (a whole block).
            if let Some(s) = h.export() {
                h = crate::sha256::Sha256::import(&s).unwrap();
                assert_eq!(h.total(), at as u64);
            }
        }
        assert_eq!(h.finish_hex(), want, "{n} bytes");
    }
    // A saved state that is not one: refused.
    for bad in ["", "zz", &"0".repeat(79), &"g".repeat(80), &format!("{}{:016x}", "0".repeat(64), 63)] {
        assert!(crate::sha256::Sha256::import(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn a_mapfile_reads_back_as_ddrescue_wrote_it() {
    let mut r = Rng(0x5eed_0003);
    for case in 0..500 {
        // Blocks that cover the disk from 0, each with a status.
        let size = 512 * (1 + r.below(1 << 14));
        let mut blocks: Vec<(u64, u64, char)> = Vec::new();
        let mut pos = 0;
        while pos < size {
            let len = (512 * (1 + r.below(64))).min(size - pos);
            let st = *r.pick(&['+', '+', '+', '-', '/', '*', '?']);
            match blocks.last_mut() {
                // (ddrescue never writes two blocks of one status in a row.)
                Some(l) if l.2 == st => l.1 += len,
                _ => blocks.push((pos, len, st)),
            }
            pos += len;
        }
        let hex = |v: u64, up: bool| if up { format!("0x{v:08X}") } else { format!("0x{v:x}") };
        let up = r.below(2) == 0;
        let gap = |r: &mut Rng| " ".repeat(1 + r.below(4) as usize);
        let mut t = String::from("# Mapfile. Created by GNU ddrescue version 1.25\n# Command line: ddrescue -f /dev/x /dev/y map\n");
        t.push_str("# current_pos  current_status  current_pass\n");
        t.push_str(&format!("{}{}+{}1\n", hex(0, up), gap(&mut r), gap(&mut r)));
        t.push_str("#      pos        size  status\n");
        for &(p, l, st) in &blocks {
            t.push_str(&format!("{}{}{}{}{}\n", hex(p, up), gap(&mut r), hex(l, up), gap(&mut r), st));
        }
        if case % 3 == 0 {
            t = t.replace('\n', "\r\n"); // saved on Windows
        }
        let m = crate::mapfile::parse(&t).unwrap_or_else(|e| panic!("{e}\n{t}"));
        assert_eq!(m.domain_size(), size, "{t}");
        let bad: Vec<(u64, u64, char)> = m.bad_areas().iter().map(|a| (a.start, a.len(), a.status)).collect();
        let want: Vec<(u64, u64, char)> = blocks.iter().filter(|b| b.2 != '+').copied().collect();
        assert_eq!(bad, want, "{t}");
        let read: u64 = blocks.iter().filter(|b| b.2 == '+').map(|b| b.1).sum();
        assert_eq!(m.rescued_bytes(), read);
    }
}

#[test]
fn every_copy_name_carries_exactly_its_own_number() {
    let mut r = Rng(0x5eed_0004);
    let alpha = &["a", "b", " ", ".", "[", "]", "R", "r", "1", "2", "0", "(", ")", "é", "日", "-", "_"];
    for i in 0..5000u64 {
        let name = r.text(alpha, 30);
        if name.is_empty() {
            continue;
        }
        let id = format!("R{:04}", 1 + i % 20000);
        let plain = crate::repair::untag(&name);
        assert_eq!(crate::repair::untag(&plain), plain, "untag is done once and for all: {name:?}");
        assert_eq!(crate::repair::tag_in(&plain), None, "{name:?} -> {plain:?}");
        let t = crate::repair::tagged(&plain, &id);
        assert_eq!(crate::repair::tag_in(&t).as_deref(), Some(id.as_str()), "{name:?} -> {t:?}");
        assert!(t.encode_utf16().count() <= 240);
    }
}

#[test]
fn a_name_made_safe_for_windows_is_safe_and_stays_so() {
    let mut r = Rng(0x5eed_0005);
    for _ in 0..5000 {
        let name = r.text(ODD, 16);
        let s = crate::rescue::safe_name(&name);
        assert_eq!(crate::rescue::safe_name(&s), s, "made safe once and for all: {name:?} -> {s:?}");
        assert!(!s.is_empty());
        assert!(!s.chars().any(|c| (c as u32) < 0x20 || "<>:\"/\\|?*".contains(c)), "{s:?}");
        assert!(!s.ends_with(' ') && !s.ends_with('.'), "{s:?}");
    }
}

/// Damage the way a place ddrescue never read can hold anything: a few
/// bytes, a field set to an extreme, a sector of zeros or of 0xFF - mostly
/// where `hot` says (the structures), sometimes anywhere.
fn mangle(r: &mut Rng, d: &mut [u8], hot: &[(usize, usize)], most: u64) {
    for _ in 0..1 + r.below(most) {
        let (a, b) = if !hot.is_empty() && r.below(4) != 0 { *r.pick(hot) } else { (0, d.len()) };
        if b <= a + 1 || b > d.len() {
            continue;
        }
        let at = a + r.below((b - a) as u64) as usize;
        let sector = |at: usize| (at / 512 * 512, (at / 512 * 512 + 512).min(d.len()));
        match r.below(7) {
            0 => d[at] ^= 1 << r.below(8),
            1 => d[at] = r.next() as u8,
            2 => {
                let n = (1 + r.below(64) as usize).min(d.len() - at);
                for x in &mut d[at..at + n] {
                    *x = r.next() as u8;
                }
            }
            3 => {
                let (s, e) = sector(at);
                d[s..e].fill(0);
            }
            4 => {
                let (s, e) = sector(at);
                d[s..e].fill(0xFF);
            }
            5 if at + 4 <= d.len() => {
                let v = *r.pick(&[0u32, 1, 2, 0x7FFF_FFFF, 0xFFFF_FFFF, 0x8000_0000, 0xFFFF_FFF7]);
                d[at..at + 4].copy_from_slice(&v.to_le_bytes());
            }
            _ if at + 8 <= d.len() => {
                let v = *r.pick(&[0u64, 1, u64::MAX, 1 << 63, 0x7FFF_FFFF_FFFF_FFFF, 1 << 40]);
                d[at..at + 8].copy_from_slice(&v.to_le_bytes());
            }
            _ => {}
        }
    }
}

/// The self-test's disk, damaged at random where its partition table, boot
/// region, FAT, root folder and the start of its clusters are: the map check
/// and the copying run to an end on every one (in the build with overflow
/// checks too: 18,000 such disks found two sums that ran past the largest
/// number there is).
#[test]
fn a_disk_damaged_anywhere_never_stops_the_map_check_or_the_copying() {
    let disk = crate::selftest::disk_image().unwrap();
    let map_text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/selftest/disk.map")).unwrap();
    let map = crate::mapfile::parse(&map_text).unwrap();
    let hot = {
        let src = crate::disk::Memory::new(disk.clone());
        let table = crate::gpt::read(&src, &[]);
        let (v, _) = crate::mapcheck::find_volume(&src, &table).unwrap();
        let (vo, heap) = (v.offset as usize, (v.offset + v.heap_offset) as usize);
        let fat = vo + v.fat_offset as usize;
        let root = v.cluster_offset(v.root_cluster) as usize;
        vec![(0, 34 * 512), (vo, vo + 24 * 512), (fat, fat + v.fat_length as usize), (heap, heap + 65_536), (root, root + 65_536), (disk.len() - 33 * 512, disk.len())]
    };
    let out = std::env::temp_dir().join(format!("arp-prop-disks-{}", std::process::id()));
    let mut r = Rng(0x5eed_0006);
    for round in 0..200 {
        let mut d = disk.clone();
        mangle(&mut r, &mut d, &hot, 6);
        let src = crate::disk::Memory::new(d);
        if let Ok(rep) = crate::mapcheck::analyse(&src, &map, &[], "map", &mut |_| {}) {
            let _ = (crate::mapcheck::summary(&rep), crate::mapcheck::to_json(&rep), crate::mapcheck::page_json(&rep));
        }
        if round % 10 == 0 {
            let opts = crate::rescue::Options {
                disk: "a damaged test disk".into(),
                out_dir: out.join(round.to_string()).display().to_string(),
                pick: crate::rescue::Pick::Everything,
                lost: true,
                stop: None,
                stop_after_files: None,
            };
            let _ = crate::rescue::rescue(&src, &opts, &mut |_| {}, &mut |_, _| {});
        }
    }
    let _ = std::fs::remove_dir_all(&out);
}

/// The sample files the self-test checks, damaged at random (and cut short,
/// and given a hole of zeros): the file checks, and reading and moving a
/// video's moments, come to a verdict on every one.
#[test]
fn a_file_damaged_anywhere_never_stops_the_file_checks() {
    let dir = std::env::temp_dir().join(format!("arp-prop-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let seeds: Vec<std::path::PathBuf> =
        std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/seeds")).unwrap().flatten().map(|e| e.path()).collect();
    let mut r = Rng(0x5eed_0007);
    for round in 0..60 {
        for s in &seeds {
            let mut d = std::fs::read(s).unwrap();
            let len = d.len();
            let hot = [(0, 4096.min(len)), (len.saturating_sub(4096), len)];
            mangle(&mut r, &mut d, &hot, 6);
            match r.below(5) {
                0 => d.truncate(r.below(len as u64 + 1) as usize),
                1 => {
                    let at = r.below(len as u64) as usize;
                    let n = (512 * (1 + r.below(8)) as usize).min(len - at);
                    d[at..at + n].fill(0);
                }
                _ => {}
            }
            let p = dir.join(format!("{round}-{}", s.file_name().unwrap().to_string_lossy()));
            std::fs::write(&p, &d).unwrap();
            let v = crate::probe::probe_file(&p, 4096, true);
            assert!(!v.verdict.is_empty(), "{}", p.display());
            let _ = crate::mp4time::made(&p);
            let _ = crate::mp4time::shift(&p, 3600);
            let _ = std::fs::remove_file(&p);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
