//! The map check: which files and folders sit on the patches ddrescue could
//! not read.
//!
//! ddrescue copied the old disk onto the recovered SSD sector for sector,
//! so a byte position in the mapfile is the same byte position on the SSD.
//! This reads the SSD's partition table and exFAT volume directly (read
//! only), works out what owns every byte that was never read, and checks
//! whether each patch on the SSD now holds only zeros (never read by any
//! attempt) or real data (most likely read by an earlier attempt).

use crate::disk::{open_source, Source};
use crate::exfat::{walk, ClusterList, Entry, FolderIssue, Volume};
use crate::fmt::{commas, csv_row, json_str, size};
use crate::gpt::{self, Table};
use crate::mapfile::{self, Mapfile};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

pub const SECTOR: u64 = 512;

pub struct Options {
    pub map_path: String,
    /// Mapfiles of earlier attempts that copied onto the same disk: what
    /// they read is on the disk too.
    pub earlier_maps: Vec<String>,
    pub disk: String,
    pub out_dir: String,
}

/// An earlier attempt's mapfile, and how much of the final attempt's
/// never-read places it did read.
#[derive(Clone, Debug)]
pub struct Earlier {
    pub path: String,
    pub finished: bool,
    pub current_time: String,
    pub read_of_bad: u64,
}

/// `areas` minus everything in `read` (both sorted, positions on the disk).
pub fn subtract(areas: &[(u64, u64, char)], read: &[(u64, u64)]) -> Vec<(u64, u64, char)> {
    let mut out = Vec::new();
    for &(s, e, st) in areas {
        let mut pos = s;
        let i = read.partition_point(|&(_, re)| re <= s);
        for &(rs, re) in &read[i..] {
            if rs >= e {
                break;
            }
            if rs > pos {
                out.push((pos, rs, st));
            }
            pos = pos.max(re);
            if pos >= e {
                break;
            }
        }
        if pos < e {
            out.push((pos, e, st));
        }
    }
    out
}

/// What a never-read patch falls on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    PartitionTable,
    Partition,
    Unpartitioned,
    Boot,
    Reserved,
    Fat,
    PastHeap,
    Bitmap,
    Upcase,
    Root,
    Folder,
    File,
    Free,
    Lost,
}

impl Kind {
    pub fn code(self) -> &'static str {
        match self {
            Kind::PartitionTable => "partition-table",
            Kind::Partition => "partition",
            Kind::Unpartitioned => "unpartitioned",
            Kind::Boot => "boot",
            Kind::Reserved => "reserved",
            Kind::Fat => "fat",
            Kind::PastHeap => "past-heap",
            Kind::Bitmap => "bitmap",
            Kind::Upcase => "upcase",
            Kind::Root => "root",
            Kind::Folder => "folder",
            Kind::File => "file",
            Kind::Free => "free",
            Kind::Lost => "lost",
        }
    }

    /// Damage here costs nothing: nothing is stored there.
    pub fn harmless(self) -> bool {
        matches!(self, Kind::Free | Kind::Unpartitioned | Kind::Reserved | Kind::PastHeap)
    }

    fn describe(self, path: &str) -> String {
        match self {
            Kind::PartitionTable => "the partition table".to_string(),
            Kind::Partition => format!("the {path} partition (not the exFAT volume; no personal files)"),
            Kind::Unpartitioned => "space outside any partition (no files)".to_string(),
            Kind::Boot => "the exFAT boot area".to_string(),
            Kind::Reserved => "reserved space inside the exFAT volume (no files)".to_string(),
            Kind::Fat => "the exFAT file allocation table (FAT)".to_string(),
            Kind::PastHeap => "space after the exFAT volume's last cluster (no files)".to_string(),
            Kind::Bitmap => "exFAT's map of which clusters are in use".to_string(),
            Kind::Upcase => "exFAT's upper-case letter table".to_string(),
            Kind::Root => "the folder records of the top folder".to_string(),
            Kind::Folder => format!("the folder records of {path}"),
            Kind::File => format!("the file {path}"),
            Kind::Free => "free space (nothing lost)".to_string(),
            Kind::Lost => {
                "data marked as in use that no file found here claims (probably a file whose folder record was lost)"
                    .to_string()
            }
        }
    }
}

/// One thing a patch falls on, with the byte ranges inside it (for files
/// and folders: positions in the file).
#[derive(Clone, Debug)]
pub struct Hit {
    pub kind: Kind,
    pub path: String,
    pub ranges: Vec<(u64, u64)>,
}

/// One patch ddrescue never read, on this disk's positions.
#[derive(Clone, Debug)]
pub struct Area {
    pub start: u64,
    pub end: u64,
    pub status: char,
    pub content: Content,
    pub hits: Vec<Hit>,
}

/// What a patch holds on the disk now, judged sector by sector: a sector
/// that is all zeros was most likely never read by any attempt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Content {
    pub sectors: u64,
    pub blank: u64,
    pub unreadable: bool,
}

impl Content {
    pub fn word(&self) -> &'static str {
        if self.unreadable {
            "unreadable"
        } else if self.sectors == 0 {
            "none"
        } else if self.blank == self.sectors {
            "zeros"
        } else if self.blank == 0 {
            "data"
        } else {
            "mixed"
        }
    }
    fn add(&mut self, o: Content) {
        self.sectors += o.sectors;
        self.blank += o.blank;
        self.unreadable |= o.unreadable;
    }
}

/// Count, sector by sector, how much of [start, end) is zeros. A sector
/// only partly inside the range is judged on the part inside.
pub fn content_of(src: &dyn Source, start: u64, end: u64) -> Content {
    let mut c = Content::default();
    let mut pos = start;
    while pos < end {
        let piece_end = end.min(pos + (8 << 20));
        match src.read_at(pos, (piece_end - pos) as usize) {
            Ok(buf) => {
                let mut s = pos;
                while s < piece_end {
                    let e = ((s / SECTOR + 1) * SECTOR).min(piece_end);
                    let lo = (s - pos) as usize;
                    let hi = (e - pos) as usize;
                    c.sectors += 1;
                    // Past the end of the disk counts as never read.
                    if hi > buf.len() || buf[lo..hi].iter().all(|&b| b == 0) {
                        c.blank += 1;
                    }
                    s = e;
                }
            }
            Err(_) => {
                c.unreadable = true;
                c.sectors += (piece_end - pos).div_ceil(SECTOR);
            }
        }
        pos = piece_end;
    }
    c
}

/// A damaged file or folder.
#[derive(Clone, Debug)]
pub struct Item {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub valid_data_length: u64,
    /// (start, end) in the file, and the disk offset of start.
    pub ranges: Vec<(u64, u64, u64)>,
    pub chain_problem: Option<String>,
    pub hidden: bool,
    /// Windows still opens it by its full path (see exfat::Entry::hidden_maybe).
    pub hidden_maybe: bool,
    pub why_hidden: String,
    pub content: Content,
    pub can_see: Option<bool>,
    /// Its folder record lies in a place ddrescue never read.
    pub record_unread: bool,
}

impl Item {
    pub fn damaged_bytes(&self) -> u64 {
        self.ranges.iter().map(|(s, e, _)| e - s).sum()
    }
    /// Every damaged byte is past the file's valid data, which reads as
    /// zeros anyway: nothing is lost.
    pub fn only_past_valid_data(&self) -> bool {
        !self.ranges.is_empty() && self.ranges.iter().all(|&(s, _, _)| s >= self.valid_data_length)
    }
}

pub struct Report {
    pub disk: String,
    pub disk_size: u64,
    pub scheme: &'static str,
    pub table_disk_size: Option<u64>,
    /// The partition table was written for exactly the disk the mapfile
    /// describes (None: cannot tell).
    pub matches_mapfile: Option<bool>,
    pub map_path: String,
    pub map_version: String,
    pub map_finished: bool,
    pub map_domain: (u64, u64),
    pub map_shift: i128,
    pub map_warnings: Vec<String>,
    pub earlier: Vec<Earlier>,
    pub vol_offset: u64,
    pub vol_label: String,
    pub vol_serial: u32,
    pub cluster_size: u64,
    pub cluster_count: u32,
    pub boot_checksum_ok: bool,
    pub used_backup_boot: bool,
    pub drive: Option<String>,
    pub folders_seen: u64,
    pub files_seen: u64,
    pub areas: Vec<Area>,
    pub items: Vec<Item>,
    pub hidden: Vec<(Entry, Option<bool>)>,
    /// Files and folders found from records that lie in never-read places
    /// (they hold data, so an earlier attempt most likely read them).
    pub unread_records: Vec<Entry>,
    pub folder_problems: Vec<FolderIssue>,
    pub lost_clusters: u64,
    pub lost_runs: Vec<(u32, u32)>,
    pub written: Vec<String>,
}

impl Report {
    pub fn bad_bytes(&self) -> u64 {
        self.areas.iter().map(|a| a.end - a.start).sum()
    }
    pub fn lost_bytes(&self) -> u64 {
        self.lost_clusters * self.cluster_size
    }
    pub fn files(&self) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(|i| !i.is_dir)
    }
    pub fn folders(&self) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(|i| i.is_dir)
    }
    pub fn hidden_files(&self) -> usize {
        self.hidden.iter().filter(|(e, _)| !e.is_dir).count()
    }
    /// Of the hidden files and folders, how many Windows may still list.
    pub fn hidden_maybe(&self) -> usize {
        self.hidden.iter().filter(|(e, _)| e.hidden_maybe).count()
    }
}

/// Owners of clusters found while walking.
#[derive(Clone, Debug)]
struct Owner {
    kind: Kind,
    path: String,
    /// Index into `items` once the owner is damaged.
    item: Option<usize>,
}

/// A piece of a patch inside one cluster.
#[derive(Clone, Copy, Debug)]
struct Piece {
    area: usize,
    start: u64, // within the cluster
    end: u64,
}

fn read_map(path: &str) -> Result<Mapfile, String> {
    let text = std::fs::read(path).map_err(|e| format!("could not read the mapfile {path}: {e}"))?;
    mapfile::parse(&String::from_utf8_lossy(&text)).map_err(|e| format!("{path}: {e}"))
}

pub fn run(opts: &Options, progress: &mut dyn FnMut(&str)) -> Result<Report, String> {
    let map = read_map(&opts.map_path)?;
    progress(&format!("Read the mapfile: {} places were never read", commas(map.bad_areas().len() as u64)));
    let mut earlier = Vec::new();
    for p in &opts.earlier_maps {
        earlier.push((p.clone(), read_map(p)?));
    }
    let src = open_source(&opts.disk).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!("Windows would not let the program read {} - it needs to run as administrator", opts.disk)
        } else {
            format!("could not open {}: {e}", opts.disk)
        }
    })?;
    let mut report = analyse(&*src, &map, &earlier, &opts.map_path, progress)?;
    write_outputs(&mut report, Path::new(&opts.out_dir))?;
    Ok(report)
}

/// Find the exFAT volume: in a partition, at the start of the disk, or
/// from a backup boot sector when the main one is gone. As a last resort
/// (no readable partition table), look for a boot sector that records its
/// own position, which exFAT boot sectors do.
pub fn find_volume<'a>(src: &'a dyn Source, table: &Table) -> Result<(Volume<'a>, bool), String> {
    let mut starts: Vec<u64> = table.partitions.iter().map(|p| p.start).collect();
    starts.push(0);
    for &s in &starts {
        if crate::exfat::is_exfat_at(src, s) {
            return Volume::open(src, s).map(|v| (v, false));
        }
    }
    // The main boot sector may be among the unread sectors: exFAT keeps a
    // copy 12 sectors further on.
    for &s in &starts {
        for bps in [512u64, 4096] {
            if crate::exfat::is_exfat_at(src, s + 12 * bps) {
                if let Ok(v) = Volume::open_backup(src, s, bps) {
                    return Ok((v, true));
                }
            }
        }
    }
    if let Some((start, backup, bps)) = scan_for_boot_sector(src, 4 << 30) {
        let v = if backup { Volume::open_backup(src, start, bps) } else { Volume::open(src, start) };
        if let Ok(v) = v {
            return Ok((v, backup));
        }
    }
    let kinds: Vec<String> = table.partitions.iter().map(|p| format!("{} ({})", p.index, p.kind())).collect();
    Err(format!(
        "no exFAT volume found on {} (partition table: {}, partitions: {}). Is this the recovered SSD?",
        src.describe(),
        table.scheme,
        if kinds.is_empty() { "none".to_string() } else { kinds.join(", ") }
    ))
}

/// Look through the first `limit` bytes for an exFAT boot sector (main or
/// backup) and work out where its volume starts: from the position the
/// boot sector records (PartitionOffset), or else from where the volume's
/// FAT turns out to be. Returns (volume start, found the backup, sector size).
fn scan_for_boot_sector(src: &dyn Source, limit: u64) -> Option<(u64, bool, u64)> {
    let end = if src.size() > 0 { src.size().min(limit) } else { limit };
    let fat_ok = |start: u64, boot: &[u8], bps: u64| -> bool {
        let fat_off = u32::from_le_bytes(boot[80..84].try_into().unwrap()) as u64 * bps;
        matches!(src.read_at(start + fat_off, 8), Ok(f) if f.len() == 8 && f[0] == 0xF8 && f[1..8].iter().all(|&b| b == 0xFF))
    };
    let mut pos = 0u64;
    while pos < end {
        let n = (end - pos).min(8 << 20);
        let buf = src.read_at(pos, n as usize).ok()?;
        if buf.is_empty() {
            break;
        }
        let mut o = 0usize;
        while o + 512 <= buf.len() {
            let b = &buf[o..o + 512];
            if &b[3..11] == b"EXFAT   " && b[510] == 0x55 && b[511] == 0xAA && (9..=12).contains(&b[108]) {
                let at = pos + o as u64;
                let bps = 1u64 << b[108];
                let recorded = u64::from_le_bytes(b[64..72].try_into().unwrap()).saturating_mul(bps);
                if recorded == at || (recorded == 0 && fat_ok(at, b, bps)) {
                    return Some((at, false, bps));
                }
                if recorded.checked_add(12 * bps) == Some(at) || (recorded == 0 && at >= 12 * bps && fat_ok(at - 12 * bps, b, bps)) {
                    return Some((at - 12 * bps, true, bps));
                }
            }
            o += 512;
        }
        pos += buf.len() as u64;
    }
    None
}

type Span = (u64, u64, Kind, String, Option<(u64, u64)>);

pub fn analyse(
    src: &dyn Source,
    map: &Mapfile,
    earlier: &[(String, Mapfile)],
    map_path: &str,
    progress: &mut dyn FnMut(&str),
) -> Result<Report, String> {
    let table = gpt::read(src, &[map.domain_end]);
    let table_disk_size = table.original_disk_size();
    let matches_mapfile = table_disk_size.map(|sz| map.domain_start == 0 && sz == map.domain_end);
    let (vol, used_backup_boot) = find_volume(src, &table)?;
    progress(&format!("Found the exFAT volume \"{}\"", vol.label));

    // Every never-read patch, moved by ddrescue's -o if it was used.
    let on_disk = |s: u64, e: u64, shift: i128| -> Option<(u64, u64)> {
        let (s, e) = (s as i128 + shift, e as i128 + shift);
        (e > 0).then(|| (s.max(0) as u64, e as u64))
    };
    let mut bad: Vec<(u64, u64, char)> = map
        .bad_areas()
        .iter()
        .filter_map(|a| on_disk(a.start, a.end, map.output_shift).map(|(s, e)| (s, e, a.status)))
        .collect();
    // What earlier attempts read onto the same disk is not lost.
    let mut earlier_info = Vec::new();
    for (path, m) in earlier {
        let read: Vec<(u64, u64)> =
            m.read_regions().iter().filter_map(|&(s, e)| on_disk(s, e, m.output_shift)).collect();
        let before: u64 = bad.iter().map(|a| a.1 - a.0).sum();
        bad = subtract(&bad, &read);
        let after: u64 = bad.iter().map(|a| a.1 - a.0).sum();
        earlier_info.push(Earlier {
            path: path.clone(),
            finished: m.finished,
            current_time: m.current_time.clone(),
            read_of_bad: before - after,
        });
    }
    let mut areas: Vec<Area> = Vec::new();
    for (s, e, status) in bad {
        areas.push(Area { start: s, end: e, status, content: content_of(src, s, e), hits: Vec::new() });
    }

    // Split each patch into what it falls on. Clusters are owned by files
    // and folders found later by the walk; everything else is known now.
    let bps = vol.bytes_per_sector;
    let vol_end = (vol.offset + vol.volume_length).max(vol.heap_end());
    let heap_start = vol.offset + vol.heap_offset;
    let heap_end = vol.heap_end();
    let fat_start = vol.offset + vol.fat_offset;
    let fat_end = fat_start + vol.number_of_fats * vol.fat_length;
    let boot_end = vol.offset + 24 * bps;
    let table_areas = table.own_areas();
    let mut bad_clusters: BTreeMap<u32, Vec<Piece>> = BTreeMap::new();
    let mut fixed_hits: Vec<Vec<Span>> = vec![Vec::new(); areas.len()];
    for (i, a) in areas.iter().enumerate() {
        let mut pos = a.start;
        while pos < a.end {
            let (end, kind, label) = if pos >= vol.offset && pos < vol_end {
                if pos < boot_end {
                    (boot_end, Some(Kind::Boot), String::new())
                } else if pos < fat_start {
                    (fat_start, Some(Kind::Reserved), String::new())
                } else if pos < fat_end {
                    (fat_end, Some(Kind::Fat), String::new())
                } else if pos < heap_start {
                    (heap_start, Some(Kind::Reserved), String::new())
                } else if pos < heap_end {
                    let c = vol.cluster_at(pos).expect("inside the heap");
                    let c_start = vol.cluster_offset(c);
                    let end = a.end.min(c_start + vol.cluster_size);
                    bad_clusters.entry(c).or_default().push(Piece { area: i, start: pos - c_start, end: end - c_start });
                    (end, None, String::new())
                } else {
                    (vol_end, Some(Kind::PastHeap), String::new())
                }
            } else if let Some(&(_, te)) = table_areas.iter().find(|&&(ts, te)| pos >= ts && pos < te) {
                (te, Some(Kind::PartitionTable), String::new())
            } else {
                // Outside the volume: in another partition, or in no partition.
                let mut next = u64::MAX;
                if pos < vol.offset {
                    next = vol.offset;
                }
                for &(ts, _) in &table_areas {
                    if ts > pos {
                        next = next.min(ts);
                    }
                }
                let mut found = None;
                for p in &table.partitions {
                    if pos >= p.start && pos < p.end {
                        found = Some(p);
                        next = next.min(p.end);
                    } else if p.start > pos {
                        next = next.min(p.start);
                    }
                }
                match found {
                    Some(p) if p.start == vol.offset => (next, Some(Kind::PastHeap), String::new()),
                    Some(p) => {
                        let label = if p.name.is_empty() { p.kind().to_string() } else { format!("\"{}\"", p.name) };
                        (next, Some(Kind::Partition), label)
                    }
                    None => (next, Some(Kind::Unpartitioned), String::new()),
                }
            };
            let end = end.min(a.end).max(pos + 1);
            if let Some(k) = kind {
                fixed_hits[i].push((pos, end, k, label, None));
            }
            pos = end;
        }
    }
    progress(&format!("{} clusters of the volume are touched by never-read places", commas(bad_clusters.len() as u64)));

    // Owners: the volume's own structures, then everything the walk finds.
    let mut owners: Vec<Owner> = Vec::new();
    let mut owner_of: HashMap<u32, usize> = HashMap::new();
    // Bit i = cluster i + 2, like exFAT's own allocation bitmap.
    let mut owned: Vec<u64> = vec![0; (vol.cluster_count as usize).div_ceil(64)];
    fn mark(owned: &mut [u64], list: &ClusterList) {
        for &(first, count) in &list.runs {
            for c in first..first.saturating_add(count) {
                let i = c as usize - 2;
                if i / 64 < owned.len() {
                    owned[i / 64] |= 1 << (i % 64);
                }
            }
        }
    }
    let mut items: Vec<Item> = Vec::new();
    // Record who owns the damaged clusters in a cluster list, and make an
    // item for a damaged file or folder.
    let claim = |owners: &mut Vec<Owner>,
                 owner_of: &mut HashMap<u32, usize>,
                 items: &mut Vec<Item>,
                 kind: Kind,
                 path: &str,
                 list: &ClusterList,
                 entry: Option<&Entry>,
                 data_length: u64| {
        let id = owners.len();
        let mut ranges: Vec<(u64, u64, u64)> = Vec::new();
        let mut base = 0u64; // clusters of the file before this run
        for &(first, count) in &list.runs {
            for (c, pieces) in bad_clusters.range(first..first.saturating_add(count)) {
                owner_of.insert(*c, id);
                let file_cluster = base + (*c - first) as u64;
                for p in pieces {
                    let fs = file_cluster * vol.cluster_size + p.start;
                    let fe = (file_cluster * vol.cluster_size + p.end).min(data_length);
                    if fe > fs {
                        ranges.push((fs, fe, vol.cluster_offset(*c) + p.start));
                    }
                }
            }
            base += count as u64;
        }
        let is_item = matches!(kind, Kind::File | Kind::Folder | Kind::Root);
        let problem = if is_item { list.problem.clone() } else { None };
        let mut item = None;
        if is_item && (!ranges.is_empty() || problem.is_some()) {
            ranges.sort();
            // Merge ranges that touch in the file and on the disk.
            let mut merged: Vec<(u64, u64, u64)> = Vec::new();
            for r in ranges {
                if let Some(last) = merged.last_mut() {
                    if last.1 == r.0 && last.2 + (last.1 - last.0) == r.2 {
                        last.1 = r.1;
                        continue;
                    }
                }
                merged.push(r);
            }
            let mut content = Content::default();
            for &(fs, fe, disk) in &merged {
                content.add(content_of(vol.src, disk, disk + (fe - fs)));
            }
            items.push(Item {
                path: path.to_string(),
                is_dir: kind != Kind::File,
                size: data_length,
                valid_data_length: entry.map(|e| e.valid_data_length).unwrap_or(data_length),
                ranges: merged,
                chain_problem: problem,
                hidden: entry.map(|e| e.hidden_from_windows).unwrap_or(false),
                hidden_maybe: entry.map(|e| e.hidden_maybe).unwrap_or(false),
                why_hidden: entry.map(|e| e.why_hidden.clone()).unwrap_or_default(),
                content,
                can_see: None,
                record_unread: false,
            });
            item = Some(items.len() - 1);
        }
        owners.push(Owner { kind, path: path.to_string(), item });
    };

    for (kind, list) in [
        (Kind::Bitmap, vol.bitmap_clusters.clone()),
        (Kind::Upcase, vol.upcase_clusters.clone()),
        (Kind::Root, vol.root_clusters.clone()),
    ] {
        mark(&mut owned, &list);
        let len = list.total() * vol.cluster_size;
        claim(&mut owners, &mut owner_of, &mut items, kind, "", &list, None, len);
    }

    let mut hidden: Vec<(Entry, Option<bool>)> = Vec::new();
    let mut unread_records: Vec<Entry> = Vec::new();
    let area_spans: Vec<(u64, u64)> = areas.iter().map(|a| (a.start, a.end)).collect();
    let in_unread = |s: u64, e: u64| -> bool {
        // areas are sorted and do not overlap
        let i = area_spans.partition_point(|&(_, ae)| ae <= s);
        i < area_spans.len() && area_spans[i].0 < e
    };
    let mut seen = (0u64, 0u64);
    let mut last_report = std::time::Instant::now();
    let folder_problems = {
        let mut visit = |e: &Entry, list: &ClusterList| {
            if e.is_dir {
                seen.0 += 1;
            } else {
                seen.1 += 1;
            }
            if last_report.elapsed().as_millis() > 500 {
                progress(&format!("Reading folder records: {} folders, {} files so far", commas(seen.0), commas(seen.1)));
                last_report = std::time::Instant::now();
            }
            if e.hidden_from_windows {
                hidden.push((e.clone(), None));
            }
            mark(&mut owned, list);
            let kind = if e.is_dir { Kind::Folder } else { Kind::File };
            let before = items.len();
            claim(&mut owners, &mut owner_of, &mut items, kind, &e.path, list, Some(e), e.data_length);
            if e.record_extents.iter().any(|&(s, x)| in_unread(s, x)) {
                unread_records.push(e.clone());
                if items.len() > before {
                    items[before].record_unread = true;
                }
            }
        };
        walk(&vol, &mut visit)
    };
    progress(&format!("Read every folder: {} folders, {} files", commas(seen.0), commas(seen.1)));

    // What each patch falls on, in disk order.
    let mut cluster_hits: Vec<Vec<Span>> = vec![Vec::new(); areas.len()];
    for (c, pieces) in &bad_clusters {
        let c_off = vol.cluster_offset(*c);
        for p in pieces {
            let (kind, path, file_range) = match owner_of.get(c) {
                Some(&o) => {
                    let ow = &owners[o];
                    // Where this piece is in the file, if inside the file's length.
                    let at = c_off + p.start;
                    let fr = ow.item.and_then(|it| {
                        items[it].ranges.iter().find(|&&(fs, fe, d)| at >= d && at < d + (fe - fs)).map(|&(fs, fe, d)| {
                            let s = fs + (at - d);
                            (s, (s + (p.end - p.start)).min(fe))
                        })
                    });
                    (ow.kind, ow.path.clone(), fr)
                }
                None => match vol.is_allocated(*c) {
                    Some(true) => (Kind::Lost, String::new(), None),
                    _ => (Kind::Free, String::new(), None),
                },
            };
            cluster_hits[p.area].push((c_off + p.start, c_off + p.end, kind, path, file_range));
        }
    }
    for (i, area) in areas.iter_mut().enumerate() {
        let mut all: Vec<Span> = std::mem::take(&mut fixed_hits[i]);
        all.append(&mut cluster_hits[i]);
        all.sort_by_key(|h| h.0);
        for (_, _, kind, path, fr) in all {
            match area.hits.iter_mut().find(|h| h.kind == kind && h.path == path) {
                Some(h) => {
                    if let Some(r) = fr {
                        match h.ranges.last_mut() {
                            Some(last) if last.1 == r.0 => last.1 = r.1,
                            _ => h.ranges.push(r),
                        }
                    }
                }
                None => area.hits.push(Hit { kind, path, ranges: fr.into_iter().collect() }),
            }
        }
    }

    // Data the volume marks as in use but nothing found here owns.
    let mut lost_clusters = 0u64;
    let mut lost_runs: Vec<(u32, u32)> = Vec::new();
    for (w, &own) in owned.iter().enumerate() {
        let mut alloc = 0u64;
        for b in 0..8 {
            if let Some(&byte) = vol.bitmap.get(w * 8 + b) {
                alloc |= (byte as u64) << (8 * b);
            }
        }
        let mut lost = alloc & !own;
        // Bits past the last cluster are not clusters.
        let first_index = w as u64 * 64;
        let valid = (vol.cluster_count as u64).saturating_sub(first_index);
        if valid < 64 {
            lost &= (1u64 << valid) - 1;
        }
        while lost != 0 {
            let bit = lost.trailing_zeros() as u64;
            lost &= lost - 1;
            lost_clusters += 1;
            let c = (first_index + bit + 2) as u32;
            let full = lost_runs.len() >= 100_000;
            match lost_runs.last_mut() {
                Some(r) if r.0 as u64 + r.1 as u64 == c as u64 => r.1 += 1,
                _ if full => {}
                _ => lost_runs.push((c, 1)),
            }
        }
    }

    // Can Windows see these paths? Only when the volume has a drive letter here.
    let drive = crate::sys::drive_for_serial(vol.serial);
    if let Some(d) = &drive {
        // Taken literally (\\?\\), so a name ending in a space or a dot is not trimmed.
        let see = |p: &str| std::fs::metadata(format!(r"\\?\{d}\{p}")).is_ok();
        for it in items.iter_mut() {
            if !it.path.is_empty() {
                it.can_see = Some(see(&it.path));
            }
        }
        for (e, c) in hidden.iter_mut() {
            *c = Some(see(&e.path));
        }
    }

    Ok(Report {
        disk: src.describe(),
        disk_size: src.size(),
        scheme: table.scheme,
        table_disk_size,
        matches_mapfile,
        map_path: map_path.to_string(),
        map_version: map.version.clone(),
        map_finished: map.finished,
        map_domain: (map.domain_start, map.domain_end),
        map_shift: map.output_shift,
        map_warnings: map.warnings.clone(),
        earlier: earlier_info,
        vol_offset: vol.offset,
        vol_label: vol.label.clone(),
        vol_serial: vol.serial,
        cluster_size: vol.cluster_size,
        cluster_count: vol.cluster_count,
        boot_checksum_ok: vol.boot_checksum_ok,
        used_backup_boot,
        drive,
        folders_seen: seen.0,
        files_seen: seen.1,
        areas,
        items,
        hidden,
        unread_records,
        folder_problems,
        lost_clusters,
        lost_runs,
        written: Vec::new(),
    })
}

fn shown(path: &str) -> String {
    if path.is_empty() {
        "(the top folder)".to_string()
    } else {
        path.to_string()
    }
}

fn yes_no(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    }
}

/// The plain-English summary.
pub fn summary(r: &Report) -> Vec<String> {
    let mut t = Vec::new();
    t.push(format!("MAP CHECK - {} version {}", crate::APP, crate::VERSION));
    t.push(String::new());
    t.push(format!("Disk:     {}", r.disk));
    t.push(format!(
        "Mapfile:  {}  (ddrescue {}, {})",
        r.map_path,
        if r.map_version.is_empty() { "?" } else { &r.map_version },
        if r.map_finished { "finished" } else { "NOT finished - the copy may be incomplete" }
    ));
    for w in &r.map_warnings {
        t.push(format!("          note: {w}"));
    }
    for e in &r.earlier {
        t.push(format!(
            "Earlier:  {}  ({}, last saved {})",
            e.path,
            if e.finished { "finished" } else { "not finished" },
            if e.current_time.is_empty() { "?" } else { &e.current_time }
        ));
        t.push(if e.read_of_bad == 0 {
            "          It read none of the places the final attempt could not read.".to_string()
        } else {
            format!(
                "          It read {} bytes of the places the final attempt could not read: those are not lost.",
                commas(e.read_of_bad)
            )
        });
    }
    match (r.table_disk_size, r.matches_mapfile) {
        (Some(sz), Some(true)) => {
            t.push(format!(
                "          This disk's partition table was written for a {}-byte disk: exactly the drive",
                commas(sz)
            ));
            t.push("          the mapfile describes. This is the copy ddrescue made.".to_string());
        }
        (Some(sz), _) => {
            t.push(format!(
                "          WARNING: this disk's partition table was written for a {}-byte disk, but the mapfile",
                commas(sz)
            ));
            t.push(format!(
                "          describes a {}-byte drive. Make sure this is the recovered SSD.",
                commas(r.map_domain.1.saturating_sub(r.map_domain.0))
            ));
        }
        _ => {}
    }
    t.push(format!(
        "Volume:   exFAT \"{}\", {} clusters, starting at byte {}{}",
        r.vol_label,
        size(r.cluster_size),
        commas(r.vol_offset),
        if r.used_backup_boot {
            " (its main boot sector is unreadable; the backup copy was used)"
        } else if !r.boot_checksum_ok {
            " (its boot checksum is wrong)"
        } else {
            ""
        }
    ));
    t.push(format!("          {} folders and {} files found", commas(r.folders_seen), commas(r.files_seen)));
    t.push(String::new());
    let bad = r.bad_bytes();
    t.push(format!("Never read: {} bytes ({}) in {} places.", commas(bad), size(bad), commas(r.areas.len() as u64)));

    let count = |f: &dyn Fn(&Area) -> bool| r.areas.iter().filter(|a| f(a)).count();
    let in_files = count(&|a| a.hits.iter().any(|h| h.kind == Kind::File));
    let in_folders = count(&|a| a.hits.iter().any(|h| matches!(h.kind, Kind::Folder | Kind::Root)));
    let in_lost = count(&|a| a.hits.iter().any(|h| h.kind == Kind::Lost));
    let in_system = count(&|a| {
        a.hits.iter().any(|h| matches!(h.kind, Kind::Boot | Kind::Fat | Kind::Bitmap | Kind::Upcase | Kind::PartitionTable))
    });
    let in_other_part = count(&|a| a.hits.iter().any(|h| h.kind == Kind::Partition));
    let harmless = count(&|a| !a.hits.is_empty() && a.hits.iter().all(|h| h.kind.harmless()));
    let files: Vec<&Item> = r.files().collect();
    let real_damage = files.iter().filter(|i| !i.only_past_valid_data() && i.damaged_bytes() > 0).count();
    t.push(String::new());
    t.push("Where those places are:".to_string());
    t.push(format!(
        "  inside files               {:>6}  -> {} files touched{}",
        commas(in_files as u64),
        commas(files.len() as u64),
        if real_damage != files.len() {
            format!(" ({} of them lose real data)", commas(real_damage as u64))
        } else {
            String::new()
        }
    ));
    t.push(format!(
        "  inside folder records      {:>6}  -> {} folders affected; {} files and {} folders Windows cannot list{}",
        commas(in_folders as u64),
        commas(r.folders().count() as u64),
        commas(r.hidden_files() as u64),
        commas((r.hidden.len() - r.hidden_files()) as u64),
        if r.hidden_maybe() > 0 {
            format!(" ({} of them still open if you type their full path)", commas(r.hidden_maybe() as u64))
        } else {
            String::new()
        }
    ));
    if in_lost > 0 {
        t.push(format!("  in data no file claims     {:>6}  (files whose folder record was lost)", commas(in_lost as u64)));
    }
    if in_system > 0 {
        t.push(format!("  in the disk's own tables   {:>6}", commas(in_system as u64)));
    }
    if in_other_part > 0 {
        t.push(format!("  in another partition       {:>6}  (no personal files there)", commas(in_other_part as u64)));
    }
    t.push(format!("  in free or unused space    {:>6}  -> nothing lost", commas(harmless as u64)));
    t.push(String::new());
    let zero = count(&|a| a.content.word() == "zeros");
    if r.areas.is_empty() {
        t.push("The mapfile names no place that was never read: ddrescue read the whole disk.".to_string());
    } else {
        t.push(format!(
            "On the disk now, {} of the {} places hold only zeros: nothing was ever copied there, so what",
            commas(zero as u64),
            commas(r.areas.len() as u64)
        ));
        t.push("the old disk held in those places is missing from the copy.".to_string());
    }
    if zero < r.areas.len() {
        let other = commas((r.areas.len() - zero) as u64);
        if r.earlier.is_empty() {
            t.push(format!("The other {other} hold data, although the final attempt never read them. That data is either"));
            t.push("from an earlier recovery attempt that did read those places (then it is right), or left over".to_string());
            t.push("from whatever the SSD held before the copy (then it is wrong). Run the check again with the".to_string());
            t.push("earlier attempt's mapfile (--earlier-map) to tell which. content_now in the CSV files says".to_string());
            t.push("which places hold zeros and which hold data.".to_string());
        } else {
            t.push(format!("The other {other} hold data, although no attempt read them: it is left over from whatever"));
            t.push("the SSD held before the copy, not the old disk's data. Treat those parts as damaged too.".to_string());
        }
    }
    if !r.folder_problems.is_empty() {
        t.push(String::new());
        t.push(format!("{} problems were found in folder records:", commas(r.folder_problems.len() as u64)));
        for i in r.folder_problems.iter().take(10) {
            t.push(format!("  {}: {}", i.folder, i.what));
        }
        if r.folder_problems.len() > 10 {
            t.push(format!("  ...and {} more (map-check-folder-problems.csv)", r.folder_problems.len() - 10));
        }
    }
    if !r.unread_records.is_empty() {
        t.push(String::new());
        t.push(format!(
            "{} files and folders were found from folder records that lie in never-read places. Those",
            commas(r.unread_records.len() as u64)
        ));
        t.push("records are valid, so an earlier attempt may have read them - or they are left over from".to_string());
        t.push("what the SSD held before the copy (map-check-unsure-records.csv).".to_string());
    }
    if r.lost_clusters > 0 {
        t.push(String::new());
        t.push(format!(
            "{} of data is marked as in use but belongs to no file this check could find. That is",
            size(r.lost_bytes())
        ));
        t.push("usually files whose folder records were lost; their contents are still on the disk.".to_string());
    }
    t.push(String::new());
    match &r.drive {
        Some(d) => t.push(format!("The volume is drive {d} on this PC; the CSV files say whether Windows can see each file.")),
        None => t.push("The volume has no drive letter here, so whether Windows can see each file was not checked.".to_string()),
    }
    t
}

fn ranges_text(r: &[(u64, u64, u64)]) -> String {
    r.iter().map(|(s, e, _)| format!("{s}-{e}")).collect::<Vec<_>>().join("; ")
}

fn write_bom(path: &Path, body: &str) -> Result<(), String> {
    let mut f = std::fs::File::create(path).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    f.write_all("\u{FEFF}".as_bytes())
        .and_then(|_| f.write_all(body.as_bytes()))
        .map_err(|e| format!("could not write {}: {e}", path.display()))
}

const UNREAD_RECORD_NOTE: &str = "Its folder record is in a place the final attempt never read, yet that place holds a \
valid record: either an earlier attempt read it (then it is right), or it is left over from what the SSD held \
before the copy (then this file may not be real). ";

fn item_note(it: &Item) -> String {
    let mut note = String::new();
    if it.is_dir {
        note.push_str("folder records: files listed in the damaged part may be missing or hidden. ");
    }
    if it.only_past_valid_data() {
        note.push_str("only space past the file's real end (nothing lost). ");
    }
    if let Some(p) = &it.chain_problem {
        note.push_str(&format!("{p}. "));
    }
    if it.hidden {
        let how = if it.hidden_maybe {
            "Windows cannot list it (typing its full path still opens it)"
        } else {
            "Windows cannot list it"
        };
        note.push_str(&format!("{how}: {}. ", it.why_hidden));
    }
    if it.record_unread {
        note.push_str(UNREAD_RECORD_NOTE);
    }
    note.trim().to_string()
}

pub fn write_outputs(r: &mut Report, out_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(out_dir).map_err(|e| format!("could not create {}: {e}", out_dir.display()))?;
    let mut written = Vec::new();

    let damaged_csv = out_dir.join("map-check-damaged.csv");
    let mut body = csv_row(&[
        "kind",
        "path",
        "size_bytes",
        "damaged_bytes",
        "damaged_ranges",
        "content_now",
        "windows_can_see",
        "note",
    ]);
    for it in &r.items {
        body.push_str(&csv_row(&[
            if it.is_dir { "FOLDER" } else { "FILE" }.to_string(),
            shown(&it.path),
            it.size.to_string(),
            it.damaged_bytes().to_string(),
            ranges_text(&it.ranges),
            it.content.word().to_string(),
            yes_no(it.can_see).to_string(),
            item_note(it),
        ]));
    }
    write_bom(&damaged_csv, &body)?;
    written.push(damaged_csv.display().to_string());

    let areas_csv = out_dir.join("map-check-bad-areas.csv");
    let mut body = csv_row(&["disk_offset", "length", "ddrescue_status", "content_now", "what_it_hits"]);
    for a in &r.areas {
        let hits: Vec<String> = a.hits.iter().map(|h| h.kind.describe(&h.path)).collect();
        body.push_str(&csv_row(&[
            a.start.to_string(),
            (a.end - a.start).to_string(),
            mapfile::status_name(a.status).to_string(),
            a.content.word().to_string(),
            hits.join(" | "),
        ]));
    }
    write_bom(&areas_csv, &body)?;
    written.push(areas_csv.display().to_string());

    if !r.hidden.is_empty() {
        let p = out_dir.join("map-check-hidden-files.csv");
        let mut body = csv_row(&["kind", "path", "size_bytes", "windows_lists_it", "windows_can_see", "why"]);
        for (e, see) in &r.hidden {
            body.push_str(&csv_row(&[
                if e.is_dir { "FOLDER" } else { "FILE" }.to_string(),
                e.path.clone(),
                e.data_length.to_string(),
                "no".to_string(),
                yes_no(*see).to_string(),
                e.why_hidden.clone(),
            ]));
        }
        write_bom(&p, &body)?;
        written.push(p.display().to_string());
    }
    if !r.folder_problems.is_empty() {
        let p = out_dir.join("map-check-folder-problems.csv");
        let mut body = csv_row(&["folder", "disk_offset", "problem"]);
        for i in &r.folder_problems {
            body.push_str(&csv_row(&[i.folder.clone(), i.disk_offset.to_string(), i.what.clone()]));
        }
        write_bom(&p, &body)?;
        written.push(p.display().to_string());
    }

    if !r.unread_records.is_empty() {
        let p = out_dir.join("map-check-unsure-records.csv");
        let mut body = csv_row(&["kind", "path", "size_bytes", "record_disk_offset", "note"]);
        for e in &r.unread_records {
            body.push_str(&csv_row(&[
                if e.is_dir { "FOLDER" } else { "FILE" }.to_string(),
                e.path.clone(),
                e.data_length.to_string(),
                e.record_offset.to_string(),
                UNREAD_RECORD_NOTE.trim().to_string(),
            ]));
        }
        write_bom(&p, &body)?;
        written.push(p.display().to_string());
    }

    // For joining with a scan of the copies (join.rs).
    let tsv_path = out_dir.join(crate::join::TSV_NAME);
    let mut tsv = format!("{}\r\n", crate::join::TSV_HEADER);
    for it in &r.items {
        if it.path.is_empty() {
            continue;
        }
        tsv.push_str(&crate::join::tsv_line(&crate::join::MapItem {
            kind: if it.is_dir { "FOLDER" } else { "DAMAGED" }.to_string(),
            path: it.path.clone(),
            size: it.size,
            damaged_bytes: it.damaged_bytes(),
            first_damaged: it.ranges.iter().map(|r| r.0).min().unwrap_or(0),
            only_past_end: it.only_past_valid_data(),
            content: it.content.word().to_string(),
            maybe: it.hidden_maybe,
            ranges: ranges_text(&it.ranges),
        }));
    }
    for (e, _) in &r.hidden {
        if e.is_dir {
            continue;
        }
        tsv.push_str(&crate::join::tsv_line(&crate::join::MapItem {
            kind: "HIDDEN".to_string(),
            path: e.path.clone(),
            size: e.data_length,
            maybe: e.hidden_maybe,
            ..Default::default()
        }));
    }
    std::fs::write(&tsv_path, tsv).map_err(|e| format!("could not write {}: {e}", tsv_path.display()))?;
    written.push(tsv_path.display().to_string());

    let json_path = out_dir.join("map-check.json");
    std::fs::write(&json_path, to_json(r)).map_err(|e| format!("could not write {}: {e}", json_path.display()))?;
    written.push(json_path.display().to_string());

    let summary_txt = out_dir.join("map-check-summary.txt");
    let mut text = summary(r).join("\r\n");
    text.push_str("\r\n\r\nFiles written:\r\n");
    text.push_str(&format!("  {}\r\n", summary_txt.display()));
    for w in &written {
        text.push_str(&format!("  {w}\r\n"));
    }
    std::fs::write(&summary_txt, text).map_err(|e| format!("could not write {}: {e}", summary_txt.display()))?;
    written.insert(0, summary_txt.display().to_string());
    r.written = written;
    Ok(())
}

fn opt_bool(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "true",
        Some(false) => "false",
        None => "null",
    }
}

/// Everything, machine-readable (used by the tests and the report page).
pub fn to_json(r: &Report) -> String {
    let mut s = String::new();
    let _ = write!(
        s,
        "{{\"tool\":{},\"version\":{},\n\"disk\":{{\"description\":{},\"size\":{},\"scheme\":{},\"table_disk_size\":{},\"matches_mapfile\":{}}},\n",
        json_str(crate::APP),
        json_str(crate::VERSION),
        json_str(&r.disk),
        r.disk_size,
        json_str(r.scheme),
        r.table_disk_size.map(|v| v.to_string()).unwrap_or("null".into()),
        opt_bool(r.matches_mapfile)
    );
    let _ = write!(
        s,
        "\"mapfile\":{{\"path\":{},\"version\":{},\"finished\":{},\"domain_start\":{},\"domain_end\":{},\"shift\":{}}},\n",
        json_str(&r.map_path),
        json_str(&r.map_version),
        r.map_finished,
        r.map_domain.0,
        r.map_domain.1,
        r.map_shift
    );
    s.push_str("\"earlier\":[");
    for (i, e) in r.earlier.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(
            s,
            "{{\"path\":{},\"finished\":{},\"current_time\":{},\"read_of_bad\":{}}}",
            json_str(&e.path),
            e.finished,
            json_str(&e.current_time),
            e.read_of_bad
        );
    }
    s.push_str("],\n");
    let _ = write!(
        s,
        "\"volume\":{{\"offset\":{},\"label\":{},\"serial\":{},\"cluster_size\":{},\"cluster_count\":{},\"boot_checksum_ok\":{},\"used_backup_boot\":{},\"drive\":{},\"folders\":{},\"files\":{}}},\n",
        r.vol_offset,
        json_str(&r.vol_label),
        r.vol_serial,
        r.cluster_size,
        r.cluster_count,
        r.boot_checksum_ok,
        r.used_backup_boot,
        r.drive.as_deref().map(json_str).unwrap_or("null".into()),
        r.folders_seen,
        r.files_seen
    );
    let _ = write!(
        s,
        "\"totals\":{{\"bad_bytes\":{},\"bad_areas\":{},\"files_damaged\":{},\"folders_damaged\":{},\"hidden_files\":{},\"hidden_folders\":{},\"hidden_maybe\":{},\"lost_clusters\":{},\"lost_bytes\":{},\"folder_problems\":{}}},\n",
        r.bad_bytes(),
        r.areas.len(),
        r.files().count(),
        r.folders().count(),
        r.hidden_files(),
        r.hidden.len() - r.hidden_files(),
        r.hidden_maybe(),
        r.lost_clusters,
        r.lost_bytes(),
        r.folder_problems.len()
    );
    s.push_str("\"areas\":[");
    for (i, a) in r.areas.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(
            s,
            "\n{{\"start\":{},\"length\":{},\"status\":{},\"content\":{},\"sectors\":{},\"blank_sectors\":{},\"hits\":[",
            a.start,
            a.end - a.start,
            json_str(&a.status.to_string()),
            json_str(a.content.word()),
            a.content.sectors,
            a.content.blank
        );
        for (j, h) in a.hits.iter().enumerate() {
            if j > 0 {
                s.push(',');
            }
            let ranges: Vec<String> = h.ranges.iter().map(|(x, y)| format!("[{x},{y}]")).collect();
            let _ = write!(
                s,
                "{{\"kind\":{},\"path\":{},\"ranges\":[{}]}}",
                json_str(h.kind.code()),
                json_str(&h.path),
                ranges.join(",")
            );
        }
        s.push_str("]}");
    }
    s.push_str("],\n\"items\":[");
    for (i, it) in r.items.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let ranges: Vec<String> = it.ranges.iter().map(|(a, b, d)| format!("[{a},{b},{d}]")).collect();
        let _ = write!(
            s,
            "\n{{\"path\":{},\"is_dir\":{},\"size\":{},\"valid_data_length\":{},\"ranges\":[{}],\"damaged_bytes\":{},\"content\":{},\"only_past_valid_data\":{},\"chain_problem\":{},\"hidden\":{},\"hidden_maybe\":{},\"why_hidden\":{},\"windows_can_see\":{},\"record_unread\":{}}}",
            json_str(&it.path),
            it.is_dir,
            it.size,
            it.valid_data_length,
            ranges.join(","),
            it.damaged_bytes(),
            json_str(it.content.word()),
            it.only_past_valid_data(),
            it.chain_problem.as_deref().map(json_str).unwrap_or("null".into()),
            it.hidden,
            it.hidden_maybe,
            json_str(&it.why_hidden),
            opt_bool(it.can_see),
            it.record_unread
        );
    }
    s.push_str("],\n\"hidden\":[");
    for (i, (e, see)) in r.hidden.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(
            s,
            "\n{{\"path\":{},\"is_dir\":{},\"size\":{},\"maybe\":{},\"why\":{},\"windows_can_see\":{}}}",
            json_str(&e.path),
            e.is_dir,
            e.data_length,
            e.hidden_maybe,
            json_str(&e.why_hidden),
            opt_bool(*see)
        );
    }
    s.push_str("],\n\"unread_records\":[");
    for (i, e) in r.unread_records.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(
            s,
            "\n{{\"path\":{},\"is_dir\":{},\"record_offset\":{}}}",
            json_str(&e.path),
            e.is_dir,
            e.record_offset
        );
    }
    s.push_str("],\n\"folder_problems\":[");
    for (i, p) in r.folder_problems.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(
            s,
            "\n{{\"folder\":{},\"disk_offset\":{},\"problem\":{}}}",
            json_str(&p.folder),
            p.disk_offset,
            json_str(&p.what)
        );
    }
    s.push_str("],\n\"lost_runs\":[");
    let runs: Vec<String> = r.lost_runs.iter().map(|(c, n)| format!("[{c},{n}]")).collect();
    s.push_str(&runs.join(","));
    s.push_str("]}\n");
    s
}

/// What the front end shows of a map check (JSON): the totals, the files
/// that lost data (most damage first), what Windows cannot list, and each
/// unread place with what it hits.
pub fn page_json(r: &Report) -> String {
    use crate::fmt::json_str;
    use std::collections::BTreeMap;
    let files: Vec<&Item> = r.files().collect();
    let mut losing: Vec<&Item> =
        files.iter().copied().filter(|i| !i.only_past_valid_data() && i.damaged_bytes() > 0).collect();
    losing.sort_by(|a, b| b.damaged_bytes().cmp(&a.damaged_bytes()).then(a.path.cmp(&b.path)));
    let file_list: Vec<String> = losing
        .iter()
        .take(500)
        .map(|i| {
            format!(
                "{{\"path\":{},\"size\":{},\"damaged\":{},\"content\":{},\"hidden\":{},\"note\":{}}}",
                json_str(&i.path),
                i.size,
                i.damaged_bytes(),
                json_str(i.content.word()),
                i.hidden,
                json_str(&item_note(i))
            )
        })
        .collect();
    let hidden: Vec<String> = r
        .hidden
        .iter()
        .take(500)
        .map(|(e, _)| {
            format!(
                "{{\"path\":{},\"is_dir\":{},\"size\":{},\"by_path\":{},\"why\":{}}}",
                json_str(&e.path),
                e.is_dir,
                e.data_length,
                e.hidden_maybe,
                json_str(&e.why_hidden)
            )
        })
        .collect();
    // The same, grouped: one line per folder and reason (a damaged folder
    // can hide thousands of files, all for the one reason).
    let mut groups: BTreeMap<(String, String, bool), (u64, u64, Vec<String>)> = BTreeMap::new();
    for (e, _) in &r.hidden {
        let parent = e.path.rsplit_once('\\').map(|(p, _)| p.to_string()).unwrap_or_default();
        let place = if e.why_hidden.starts_with("inside the folder") { String::new() } else { parent };
        let g = groups.entry((place, e.why_hidden.clone(), e.hidden_maybe)).or_default();
        if e.is_dir {
            g.1 += 1;
        } else {
            g.0 += 1;
        }
        if g.2.len() < 4 {
            g.2.push(e.path.rsplit('\\').next().unwrap_or("").to_string());
        }
    }
    let hidden_groups: Vec<String> = groups
        .iter()
        .map(|((place, why, by_path), (files, folders, examples))| {
            format!(
                "{{\"where\":{},\"why\":{},\"by_path\":{},\"files\":{},\"folders\":{},\"examples\":[{}]}}",
                json_str(place),
                json_str(why),
                by_path,
                files,
                folders,
                examples.iter().map(|x| json_str(x)).collect::<Vec<_>>().join(",")
            )
        })
        .collect();
    let areas: Vec<String> = r
        .areas
        .iter()
        .take(5000)
        .map(|a| {
            let what: Vec<String> = a.hits.iter().take(3).map(|h| h.kind.describe(&h.path)).collect();
            format!(
                "[{},{},{},{}]",
                a.start,
                a.end,
                json_str(a.content.word()),
                json_str(&what.join("; "))
            )
        })
        .collect();
    let opt = |v: Option<u64>| v.map(|x| x.to_string()).unwrap_or_else(|| "null".to_string());
    format!(
        "{{\"bad_bytes\":{},\"bad_areas\":{},\"files_touched\":{},\"files_losing_data\":{},\"folders_affected\":{},\
         \"hidden_files\":{},\"hidden_folders\":{},\"hidden_by_path\":{},\"lost_bytes\":{},\"folder_problems\":{},\
         \"files_seen\":{},\"folders_seen\":{},\"disk_size\":{},\"table_disk_size\":{},\"matches_mapfile\":{},\
         \"volume\":{{\"label\":{},\"offset\":{},\"cluster_size\":{},\"clusters\":{}}},\"drive\":{},\
         \"files\":[{}],\"hidden\":[{}],\"hidden_groups\":[{}],\"areas\":[{}]}}",
        r.bad_bytes(),
        r.areas.len(),
        files.len(),
        losing.len(),
        r.folders().count(),
        r.hidden_files(),
        r.hidden.len() - r.hidden_files(),
        r.hidden_maybe(),
        r.lost_bytes(),
        r.folder_problems.len(),
        r.files_seen,
        r.folders_seen,
        r.disk_size,
        opt(r.table_disk_size),
        r.matches_mapfile.map(|b| b.to_string()).unwrap_or_else(|| "null".to_string()),
        json_str(&r.vol_label),
        r.vol_offset,
        r.cluster_size,
        r.cluster_count,
        r.drive.as_deref().map(json_str).unwrap_or_else(|| "null".to_string()),
        file_list.join(","),
        hidden.join(","),
        hidden_groups.join(","),
        areas.join(",")
    )
}
