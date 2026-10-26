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
use crate::exfat::{ClusterList, Entry, FolderIssue, Volume};
use crate::fmt::{commas, csv_row, json_str, size};
use crate::gpt::{self, Table};
use crate::mapfile::{self, Mapfile};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

pub const SECTOR: u64 = 512;

pub struct Options {
    pub map_path: String,
    /// Mapfiles of earlier attempts that copied onto the same disk: what
    /// they read is on the disk too.
    pub earlier_maps: Vec<String>,
    pub disk: String,
    pub out_dir: String,
    /// Where Windows shows the volume's files, to check this check against
    /// (see Compare); None: wherever the volume has a drive letter here.
    pub compare_with: Option<String>,
}

/// Checking this check against Windows' own reading of the volume: what
/// Windows lists against what this check says it can list. Windows reads
/// the volume with its own exFAT driver, so the two can only agree if this
/// check reads the volume right (it is the one independent check there is
/// on the recovered SSD itself; on test disks the oracle in
/// tools/test_mapcheck.py is another).
#[derive(Clone, Debug)]
pub enum Compare {
    /// Wherever the volume has a drive letter on this PC (found by its serial
    /// number): the image mounted read-only, or the recovered SSD switched on
    /// read-only. Not compared when it has none.
    Auto,
    /// This folder shows the volume's files: a drive letter (R:) or, for the
    /// tests, any folder.
    With(String),
}

/// One thing that claims clusters: the volume's own structures, or a file
/// or folder found.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Claim {
    pub kind: Kind,
    pub path: String,
    /// Listed after a blank record ddrescue did read (Report::leftover).
    pub leftover: bool,
}

impl Claim {
    pub fn describe(&self) -> String {
        let p = match self.kind {
            Kind::File => format!("the file {}", self.path),
            Kind::Folder => format!("the folder {}", self.path),
            Kind::Root => "the top folder".to_string(),
            k => k.describe(&self.path),
        };
        if self.leftover {
            format!("{p} (listed after a blank record ddrescue read: most likely a leftover)")
        } else {
            p
        }
    }
}

/// Windows' own reading of the volume against this check's (see Compare).
#[derive(Clone, Debug, Default)]
pub struct WindowsView {
    /// Where Windows' reading was taken: a drive (R:) or a folder.
    pub root: String,
    /// Whether that volume is read-only (None: cannot tell, or a folder).
    pub read_only: Option<bool>,
    /// Files and folders Windows lists there.
    pub listed: u64,
    /// Files and folders this check expects Windows to list.
    pub expected: u64,
    /// Listed by Windows, though this check says it cannot list them (or
    /// never found them). At most LIST_MAX kept; the count is of all.
    pub only_windows: Vec<String>,
    pub only_windows_count: u64,
    /// Expected, but Windows does not list them.
    pub only_here: Vec<String>,
    pub only_here_count: u64,
    /// Files Windows gives another size (or kind): (path, size here, size
    /// Windows gives; u64::MAX for a folder).
    pub other_size: Vec<(String, u64, u64)>,
    pub other_size_count: u64,
    /// Folders Windows would not let the program look into: left out of the
    /// comparison.
    pub unlistable: Vec<String>,
    /// Folders Windows reports as corrupted: it lists nothing in them (and
    /// this check expects it to list nothing there, or they differ).
    pub corrupt: Vec<String>,
    /// Why there is no comparison after all.
    pub failed: Option<String>,
}

impl WindowsView {
    pub fn agrees(&self) -> bool {
        self.failed.is_none() && self.only_windows_count == 0 && self.only_here_count == 0 && self.other_size_count == 0
    }
}

/// How many paths a list in the report keeps (the counts are of all).
const LIST_MAX: usize = 1000;
/// Up to how many files and folders their names are kept for the checks
/// that need them (Windows' reading, clusters claimed twice): about 100 MB.
const KEEP_NAMES_UP_TO: usize = 3_000_000;

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
    /// When ddrescue last saved the mapfile ("Current time:"), as written there.
    pub map_time: String,
    pub map_domain: (u64, u64),
    pub map_shift: i128,
    pub map_warnings: Vec<String>,
    /// Reads of the disk that failed while it still answered.
    pub disk_warnings: Vec<String>,
    /// Places (count, bytes) the SSD itself could not read when the image
    /// this check reads was made; counted as never read.
    pub ssd_unread: (u64, u64),
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
    /// The total size of every file found (what copying them all takes).
    pub bytes_seen: u64,
    pub areas: Vec<Area>,
    pub items: Vec<Item>,
    pub hidden: Vec<(Entry, Option<bool>)>,
    /// Files and folders found from records that lie in never-read places
    /// (they hold data, so an earlier attempt most likely read them).
    pub unread_records: Vec<Entry>,
    pub folder_problems: Vec<FolderIssue>,
    pub lost_clusters: u64,
    pub lost_runs: Vec<(u32, u32)>,
    /// Pieces of lost data (as the copying saves them, in "lost data (no
    /// name)") that sit on places ddrescue never read.
    pub lost_items: Vec<Item>,
    /// The newest date (created or modified) of any file or folder found,
    /// as an exFAT timestamp, with its path.
    pub newest: Option<(u32, String)>,
    /// Clusters two things found here both claim (or one claims twice: its
    /// cluster chain loops): who claimed them first, who again, how many.
    /// The volume's own records never do that; a record that does is
    /// damaged, or a leftover of something deleted or replaced long ago.
    pub claimed_twice: Vec<(Claim, Claim, u64)>,
    /// Clusters a file or folder claims that the volume marks as free (and
    /// how many), where that mark was read. Again only a damaged record or
    /// a leftover does that.
    pub claimed_free: Vec<(Claim, u64)>,
    /// Windows' own reading of the volume against this check's.
    pub windows: Option<WindowsView>,
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
    /// Of the hidden files and folders, how many Windows still opens by full path.
    pub fn hidden_maybe(&self) -> usize {
        self.hidden.iter().filter(|(e, _)| e.hidden_maybe).count()
    }
    /// Does [s, e) touch a place that was never read?
    pub fn in_unread(&self, s: u64, e: u64) -> bool {
        let i = self.areas.partition_point(|a| a.end <= s);
        i < self.areas.len() && self.areas[i].start < e
    }
    /// Is this file or folder hidden by a blank record that ddrescue did
    /// read? Then the old drive had that record blank too, and Windows never
    /// listed it there either: most likely a leftover of something deleted
    /// or replaced long ago (a folder's records past its end are not
    /// cleared), not something the damage hid.
    pub fn leftover(&self, e: &Entry) -> bool {
        !e.hidden_by_blank.is_empty() && !e.hidden_by_blank.iter().any(|&(s, x)| self.in_unread(s, x))
    }
    /// (files, folders) that are leftovers.
    pub fn leftovers(&self) -> (usize, usize) {
        let l: Vec<&Entry> = self.hidden.iter().map(|(e, _)| e).filter(|e| self.leftover(e)).collect();
        (l.iter().filter(|e| !e.is_dir).count(), l.iter().filter(|e| e.is_dir).count())
    }
    /// Why Windows cannot list it, with whether the damage is the reason.
    pub fn hidden_why(&self, e: &Entry) -> String {
        if self.leftover(e) {
            format!(
                "{}; that blank record is in a place ddrescue did read, so the old drive had it blank too and Windows \
                 never listed this there either: most likely a leftover of something deleted or replaced long ago, not \
                 something the damage hid",
                e.why_hidden
            )
        } else if !e.hidden_by_blank.is_empty() {
            format!("{} (the blank record is in a place ddrescue never read)", e.why_hidden)
        } else {
            e.why_hidden.clone()
        }
    }
    /// (Folders found by their contents, the files inside them.)
    pub fn rebuilt(&self) -> (u64, u64) {
        let top = format!("{}\\", crate::exfat::FOUND_FOLDERS);
        let mut folders = 0;
        let mut files = 0;
        for (e, _) in &self.hidden {
            if let Some(rest) = e.path.strip_prefix(&top) {
                if !e.is_dir {
                    files += 1;
                } else if !rest.contains('\\') {
                    folders += 1;
                }
            }
        }
        (folders, files)
    }
    /// Is the newest date on the volume more than a day after ddrescue
    /// finished? Then something changed the SSD after the copy (or that
    /// file's date is wrong).
    pub fn newer_than_copy(&self) -> bool {
        match (&self.newest, local_seconds_of(&self.map_time)) {
            (Some((ts, _)), Some(done)) => exfat_seconds(*ts).is_some_and(|t| t > done + 86_400),
            _ => false,
        }
    }
}

/// An exFAT timestamp as seconds (local time as stored, no time zone).
fn exfat_seconds(ts: u32) -> Option<i64> {
    let (sec, min, hour) = ((ts & 0x1F) * 2, (ts >> 5) & 0x3F, (ts >> 11) & 0x1F);
    let (day, month, year) = ((ts >> 16) & 0x1F, (ts >> 21) & 0x0F, 1980 + (ts >> 25));
    if !(1..=12).contains(&month) || day == 0 || hour > 23 || min > 59 || sec > 59 {
        return None;
    }
    Some(crate::exfat::days_from_civil(year as i64, month, day) * 86_400 + (hour * 3600 + min * 60 + sec) as i64)
}

/// "2024-04-24 21:38:08" as seconds (local time, no time zone).
fn local_seconds_of(text: &str) -> Option<i64> {
    let t = text.trim();
    let n = |a: usize, b: usize| t.get(a..b).and_then(|x| x.parse::<i64>().ok());
    let (y, mo, d, h, mi, s) = (n(0, 4)?, n(5, 7)?, n(8, 10)?, n(11, 13)?, n(14, 16)?, n(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    Some(crate::exfat::days_from_civil(y, mo as u32, d as u32) * 86_400 + h * 3600 + mi * 60 + s)
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
    // The recovered SSD holds a copy and is itself healthy: a read that
    // fails means it stopped answering (then the results would leave things
    // out, so there are none), or that it has a bad spot of its own.
    let watched = crate::disk::Watched::new(&*src);
    // Reading an image (step 4 makes one): the places the SSD itself could
    // not read when the image was made are zeros in it, so they count as
    // never read too.
    let mut ssd_errors: Vec<(u64, u64, char)> = Vec::new();
    if let Some(p) = crate::image::read_errors_map(&opts.disk) {
        let m = read_map(&p.display().to_string())?;
        ssd_errors = m.bad_areas().iter().map(|a| (a.start, a.end, '-')).collect();
        progress(&format!(
            "The image's own list says the SSD could not read {} places when the image was made: they count as never read",
            commas(ssd_errors.len() as u64)
        ));
    }
    let compare = match &opts.compare_with {
        Some(w) => Compare::With(w.clone()),
        None => Compare::Auto,
    };
    let mut report = analyse_with(&watched, &map, &earlier, &ssd_errors, &opts.map_path, &compare, progress)?;
    if watched.errors() > 0 {
        if !watched.answers(report.vol_offset) {
            return Err(disk_lost_text(&watched.last_error()));
        }
        report.disk_warnings.push(format!(
            "{} reads of this disk failed although it still answers (the last: {}). Folders that could not be read \
             are listed with the problems below.",
            commas(watched.errors()),
            watched.last_error()
        ));
    }
    write_outputs(&mut report, Path::new(&opts.out_dir))?;
    Ok(report)
}

/// What to tell someone whose recovered disk stopped answering during the check.
pub fn disk_lost_text(err: &str) -> String {
    format!(
        "The recovered SSD stopped answering during the check (Windows said: {err}), so it gives no results: they \
         would leave things out. Check its cable is in firmly. If it came out, plug it back in: it arrives switched \
         off, as Windows is still set to leave new disks switched off. Then choose it again in step 3, click \
         Protect, and run this step again."
    )
}

/// Find the exFAT volume: in a partition, at the start of the disk, or
/// from a backup boot sector when the main one is gone. As a last resort
/// (no readable partition table), look for a boot sector that records its
/// own position, which exFAT boot sectors do.
pub fn find_volume<'a>(src: &'a dyn Source, table: &Table) -> Result<(Volume<'a>, bool), String> {
    find_volume_in(src, table, true)
}

/// The same, without the slow last resort (reading the first 4 GiB of the
/// disk for a boot sector): for describing disks quickly.
pub fn find_volume_quick<'a>(src: &'a dyn Source, table: &Table) -> Result<(Volume<'a>, bool), String> {
    find_volume_in(src, table, false)
}

/// The rule `is_the_copy` follows, for what is already known about a disk:
/// the size its partition table was made for, its own size, and whether it
/// holds an exFAT volume (asked only when the rest does not settle it).
pub fn copy_rule(made_for: Option<u64>, disk_size: u64, has_exfat: impl FnOnce() -> bool, map_size: u64) -> bool {
    made_for == Some(map_size) && (disk_size > map_size || has_exfat())
}

/// Is this disk the copy ddrescue made (or a copy of that copy, or an image
/// of it), for a mapfile of a drive of `map_size` bytes?
///
/// Its partition table must have been made for a drive of exactly that size,
/// but that alone says little: drives sold as the same size are all exactly
/// the same size (every "1 TB" drive holds 1,000,204,886,016 bytes), so a
/// 1 TB drive Windows set up for itself, such as SPARE, has a partition
/// table made for that size too. So it must also be bigger than the old
/// drive (ddrescue copied it onto the start of a bigger SSD), or hold the
/// exFAT volume (an image of the copy is exactly the old drive's size).
pub fn is_the_copy(src: &dyn Source, table: &Table, map_size: u64) -> bool {
    copy_rule(table.original_disk_size(), src.size(), || find_volume_quick(src, table).is_ok(), map_size)
}

/// Why a disk is not the copy (see `is_the_copy`), in plain words; None
/// when it is.
pub fn not_the_copy(src: &dyn Source, table: &Table, map_size: u64) -> Option<String> {
    if is_the_copy(src, table, map_size) {
        return None;
    }
    Some(match table.original_disk_size() {
        Some(made_for) if made_for != map_size => format!(
            "its partition table was made for a {}-byte drive, but the mapfile describes a {}-byte drive",
            commas(made_for),
            commas(map_size)
        ),
        Some(_) => format!(
            "its partition table was made for a {}-byte drive, as the old drive's was - but so is every drive of that \
             size, and this one is no bigger and holds no exFAT volume: it is another drive of the same size (SPARE?)",
            commas(map_size)
        ),
        None => "it has no partition table made for the drive the mapfile describes".to_string(),
    })
}

fn find_volume_in<'a>(src: &'a dyn Source, table: &Table, scan: bool) -> Result<(Volume<'a>, bool), String> {
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
    if let Some((start, backup, bps)) = if scan { scan_for_boot_sector(src, 4 << 30) } else { None } {
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
    analyse_with(src, map, earlier, &[], map_path, &Compare::Auto, progress)
}

/// `bad` and `more` (both sorted lists of (start, end, status)) together:
/// overlapping places become one.
fn union(bad: Vec<(u64, u64, char)>, more: &[(u64, u64, char)]) -> Vec<(u64, u64, char)> {
    let mut all: Vec<(u64, u64, char)> = bad.into_iter().chain(more.iter().copied()).collect();
    all.sort_by_key(|a| (a.0, a.1));
    let mut out: Vec<(u64, u64, char)> = Vec::new();
    for a in all {
        match out.last_mut() {
            Some(l) if a.0 < l.1 => l.1 = l.1.max(a.1),
            _ => out.push(a),
        }
    }
    out
}

/// The same, with `ssd_errors`: places the recovered SSD itself could not
/// read when its image was made (positions on the disk).
pub fn analyse_with(
    src: &dyn Source,
    map: &Mapfile,
    earlier: &[(String, Mapfile)],
    ssd_errors: &[(u64, u64, char)],
    map_path: &str,
    compare: &Compare,
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
    let ssd_unread = (ssd_errors.len() as u64, ssd_errors.iter().map(|a| a.1 - a.0).sum::<u64>());
    if !ssd_errors.is_empty() {
        bad = union(bad, ssd_errors);
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
    // Clusters claimed again by something found later (the same bits).
    let mut again: Vec<u64> = vec![0; owned.len()];
    let mut any_again = false;
    // Everything that claims clusters, in the order found (its number in
    // this list), and the runs of clusters each claims.
    let mut claimants: Vec<Claimant> = Vec::new();
    let mut claim_runs: Vec<(u32, u32, u32)> = Vec::new();
    fn mark(owned: &mut [u64], again: &mut [u64], any_again: &mut bool, runs: &mut Vec<(u32, u32, u32)>, id: u32, list: &ClusterList) {
        for &(first, count) in &list.runs {
            runs.push((first, count, id));
            for c in first..first.saturating_add(count) {
                let i = c as usize - 2;
                if i / 64 < owned.len() {
                    let bit = 1u64 << (i % 64);
                    if owned[i / 64] & bit != 0 {
                        again[i / 64] |= bit;
                        *any_again = true;
                    }
                    owned[i / 64] |= bit;
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
        mark(&mut owned, &mut again, &mut any_again, &mut claim_runs, claimants.len() as u32, &list);
        claimants.push(Claimant { kind, path: "".into(), size: 0, listed: false, leftover: false });
        let len = list.total() * vol.cluster_size;
        claim(&mut owners, &mut owner_of, &mut items, kind, "", &list, None, len);
    }
    // Where Windows shows the volume's files, to compare with (see Compare).
    let drive = crate::sys::drive_for_serial(vol.serial);
    let compare_root = match compare {
        Compare::With(w) => Some(w.clone()),
        Compare::Auto => drive.clone(),
    };

    let mut hidden: Vec<(Entry, Option<bool>)> = Vec::new();
    let mut unread_records: Vec<Entry> = Vec::new();
    let area_spans: Vec<(u64, u64)> = areas.iter().map(|a| (a.start, a.end)).collect();
    let in_unread = |s: u64, e: u64| -> bool {
        // areas are sorted and do not overlap
        let i = area_spans.partition_point(|&(_, ae)| ae <= s);
        i < area_spans.len() && area_spans[i].0 < e
    };
    // (The same as Report::leftover, before there is a report.)
    let leftover =
        |e: &Entry| !e.hidden_by_blank.is_empty() && !e.hidden_by_blank.iter().any(|&(s, x)| in_unread(s, x));
    let mut seen = (0u64, 0u64);
    let mut bytes_seen = 0u64;
    let mut newest: Option<(u32, String)> = None;
    let mut last_report = std::time::Instant::now();
    let folder_problems = {
        // (Both the walk and the search for lost folders report as they go.)
        let say = std::cell::RefCell::new(&mut *progress);
        let mut visit = |e: &Entry, list: &ClusterList| {
            if e.is_dir {
                seen.0 += 1;
            } else {
                seen.1 += 1;
                bytes_seen += e.data_length;
            }
            if last_report.elapsed().as_millis() > 500 {
                (*say.borrow_mut())(&format!(
                    "Reading folder records: {} folders, {} files so far",
                    commas(seen.0),
                    commas(seen.1)
                ));
                last_report = std::time::Instant::now();
            }
            if e.hidden_from_windows {
                hidden.push((e.clone(), None));
            }
            for ts in [e.modified, e.created] {
                if exfat_seconds(ts).is_some() && newest.as_ref().is_none_or(|(n, _)| ts > *n) {
                    newest = Some((ts, e.path.clone()));
                }
            }
            let id = claimants.len() as u32;
            mark(&mut owned, &mut again, &mut any_again, &mut claim_runs, id, list);
            let kind = if e.is_dir { Kind::Folder } else { Kind::File };
            claimants.push(Claimant {
                kind,
                path: if claimants.len() < KEEP_NAMES_UP_TO { e.path.as_str().into() } else { "".into() },
                size: e.data_length,
                listed: !e.hidden_from_windows,
                leftover: leftover(e),
            });
            let before = items.len();
            claim(&mut owners, &mut owner_of, &mut items, kind, &e.path, list, Some(e), e.data_length);
            if e.record_extents.iter().any(|&(s, x)| in_unread(s, x)) {
                unread_records.push(e.clone());
                if items.len() > before {
                    items[before].record_unread = true;
                }
            }
        };
        crate::exfat::walk_until(&vol, &mut visit, &|| false, &mut |m: &str| (*say.borrow_mut())(m))
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

    // Clusters claimed twice, and clusters claimed that the volume marks as
    // free (where that mark was read).
    let claim_of = |id: u32| {
        let c = &claimants[id as usize];
        Claim { kind: c.kind, path: c.path.to_string(), leftover: c.leftover }
    };
    let mut claimed_twice: Vec<(Claim, Claim, u64)> = Vec::new();
    if any_again {
        // Who claims each of those clusters, in the order claimed.
        let mut by: HashMap<u32, Vec<u32>> = HashMap::new();
        for &(first, count, id) in &claim_runs {
            for c in first..first.saturating_add(count) {
                let i = c as usize - 2;
                if i / 64 < again.len() && again[i / 64] & (1 << (i % 64)) != 0 {
                    by.entry(c).or_default().push(id);
                }
            }
        }
        let mut pairs: BTreeMap<(u32, u32), u64> = BTreeMap::new();
        for ids in by.values() {
            for &other in &ids[1..] {
                *pairs.entry((ids[0], other)).or_default() += 1;
            }
        }
        claimed_twice = pairs.into_iter().map(|((a, b), n)| (claim_of(a), claim_of(b), n)).collect();
        claimed_twice.sort();
        progress(&format!("{} clusters are claimed by more than one file or folder", commas(by.len() as u64)));
    }
    let mut claimed_free: Vec<(Claim, u64)> = Vec::new();
    if !vol.bitmap.is_empty() {
        let mut free: Vec<u64> = vec![0; owned.len()];
        let mut any_free = false;
        for (w, &own) in owned.iter().enumerate() {
            let mut alloc = 0u64;
            for b in 0..8 {
                if let Some(&byte) = vol.bitmap.get(w * 8 + b) {
                    alloc |= (byte as u64) << (8 * b);
                }
            }
            let mut bits = own & !alloc;
            let valid = (vol.cluster_count as u64).saturating_sub(w as u64 * 64);
            if valid < 64 {
                bits &= (1u64 << valid) - 1;
            }
            // Only where the mark was read: a never-read place in the
            // bitmap reads as "free".
            let mut k = bits;
            while k != 0 {
                let bit = k.trailing_zeros() as u64;
                k &= k - 1;
                let byte = (w as u64 * 64 + bit) / 8;
                if vol.disk_offset_in(&vol.bitmap_clusters, byte).is_none_or(|d| in_unread(d, d + 1)) {
                    bits &= !(1u64 << bit);
                }
            }
            if bits != 0 {
                free[w] = bits;
                any_free = true;
            }
        }
        if any_free {
            let mut count: BTreeMap<u32, u64> = BTreeMap::new();
            for &(first, n, id) in &claim_runs {
                for c in first..first.saturating_add(n) {
                    let i = c as usize - 2;
                    if i / 64 < free.len() && free[i / 64] & (1 << (i % 64)) != 0 {
                        *count.entry(id).or_default() += 1;
                    }
                }
            }
            claimed_free = count.into_iter().map(|(id, n)| (claim_of(id), n)).collect();
            claimed_free.sort();
        }
    }

    // Windows' own reading of the volume, against this check's.
    let windows = compare_root.map(|root| {
        progress(&format!("Comparing with what Windows lists on {root}..."));
        let kept = claimants.len() <= KEEP_NAMES_UP_TO;
        let mut v = windows_view(&root, &claimants[3..], kept, progress);
        v.read_only = crate::sys::volume_read_only(&root);
        v
    });

    // Pieces of lost data on never-read places: named as the copying names
    // them, so that the check of the copies marks them NEVER_READ too.
    let mut lost_items: Vec<Item> = Vec::new();
    for &(first, count) in &lost_runs {
        let mut ranges: Vec<(u64, u64, u64)> = Vec::new();
        for (c, pieces) in bad_clusters.range(first..first.saturating_add(count)) {
            let at = (*c - first) as u64 * vol.cluster_size;
            for p in pieces {
                ranges.push((at + p.start, at + p.end, vol.cluster_offset(*c) + p.start));
            }
        }
        if ranges.is_empty() {
            continue;
        }
        ranges.sort();
        let mut merged: Vec<(u64, u64, u64)> = Vec::new();
        for r in ranges {
            match merged.last_mut() {
                Some(last) if last.1 == r.0 && last.2 + (last.1 - last.0) == r.2 => last.1 = r.1,
                _ => merged.push(r),
            }
        }
        let mut content = Content::default();
        for &(fs, fe, disk) in &merged {
            content.add(content_of(src, disk, disk + (fe - fs)));
        }
        let head = src.read_at(vol.cluster_offset(first), 64).unwrap_or_default();
        let len = count as u64 * vol.cluster_size;
        lost_items.push(Item {
            path: format!("{}\\{}", crate::rescue::LOST_DIR, crate::rescue::lost_piece_name(first, count, &head)),
            is_dir: false,
            size: len,
            valid_data_length: len,
            ranges: merged,
            chain_problem: None,
            hidden: false,
            hidden_maybe: false,
            why_hidden: String::new(),
            content,
            can_see: None,
            record_unread: false,
        });
    }

    // Can Windows see these paths? Only when the volume has a drive letter here.
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
        map_time: map.current_time.clone(),
        map_domain: (map.domain_start, map.domain_end),
        map_shift: map.output_shift,
        map_warnings: map.warnings.clone(),
        disk_warnings: Vec::new(),
        ssd_unread,
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
        bytes_seen,
        areas,
        items,
        hidden,
        unread_records,
        folder_problems,
        lost_clusters,
        lost_runs,
        lost_items,
        newest,
        claimed_twice,
        claimed_free,
        windows,
        written: Vec::new(),
    })
}

/// A file or folder found, as the checks after the walk need it.
struct Claimant {
    kind: Kind,
    /// Empty past KEEP_NAMES_UP_TO.
    path: Box<str>,
    size: u64,
    /// Windows lists it (this check says).
    listed: bool,
    leftover: bool,
}

/// What Windows lists under a folder (list_tree).
struct Listing {
    /// Path (folders separated by '\') -> (is a folder, size).
    entries: HashMap<String, (bool, u64)>,
    /// Folders Windows does not let the program look into (as System Volume
    /// Information): left out of the comparison.
    shut: Vec<String>,
    /// Folders Windows fails to list part-way (it reports them as corrupted):
    /// to Windows' own listing (File Explorer, a copy) they hold nothing, so
    /// nothing of theirs counts as listed - as the check predicts for a folder
    /// holding a damaged record (exfat.rs, Stop::Damaged).
    corrupt: Vec<String>,
}

/// Every file and folder under `root`, as Windows lists them.
fn list_tree(root: &Path, say: &mut dyn FnMut(&str)) -> Result<Listing, String> {
    let mut out = Listing { entries: HashMap::new(), shut: Vec::new(), corrupt: Vec::new() };
    let mut stack = vec![String::new()];
    let mut last = std::time::Instant::now();
    while let Some(rel) = stack.pop() {
        let dir = if rel.is_empty() { root.to_path_buf() } else { root.join(rel.replace('\\', std::path::MAIN_SEPARATOR_STR)) };
        // A folder's entries are taken only once Windows has listed all of it.
        let mut here: Vec<(String, bool, u64)> = Vec::new();
        let failed = match std::fs::read_dir(&dir) {
            Err(e) => Some(e),
            Ok(rd) => {
                let mut failed = None;
                for ent in rd {
                    let ent = match ent {
                        Ok(ent) => ent,
                        Err(e) => {
                            failed = Some(e);
                            break;
                        }
                    };
                    let name = ent.file_name().to_string_lossy().into_owned();
                    let path = if rel.is_empty() { name } else { format!("{rel}\\{name}") };
                    let is_dir = ent.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    let len = if is_dir { 0 } else { ent.metadata().map(|m| m.len()).unwrap_or(0) };
                    here.push((path, is_dir, len));
                }
                failed
            }
        };
        match failed {
            Some(e) if rel.is_empty() => return Err(format!("could not list {}: {e}", root.display())),
            Some(e) if e.kind() == std::io::ErrorKind::PermissionDenied => out.shut.push(rel),
            Some(_) => out.corrupt.push(rel),
            None => {
                for (path, is_dir, len) in here {
                    if is_dir {
                        stack.push(path.clone());
                    }
                    out.entries.insert(path, (is_dir, len));
                }
            }
        }
        if last.elapsed().as_millis() > 500 {
            say(&format!("Listing what Windows shows: {} files and folders so far", commas(out.entries.len() as u64)));
            last = std::time::Instant::now();
        }
    }
    out.shut.sort();
    out.corrupt.sort();
    Ok(out)
}

/// What Windows lists under `root` against what this check says it can
/// list (`found`: every file and folder found, in the order found).
fn windows_view(root: &str, found: &[Claimant], names_kept: bool, say: &mut dyn FnMut(&str)) -> WindowsView {
    let mut v = WindowsView { root: root.to_string(), ..Default::default() };
    if !names_kept {
        v.failed = Some(format!("the volume holds more than {} files and folders: too many to compare", commas(KEEP_NAMES_UP_TO as u64)));
        return v;
    }
    // A drive letter is read literally, so names ending in a space or a dot
    // are not trimmed on the way (exFAT allows them; Windows' usual paths do not).
    let r = root.trim_end_matches(['\\', '/']);
    let path = if cfg!(windows) && r.len() == 2 && r.ends_with(':') {
        PathBuf::from(format!(r"\\?\{r}\"))
    } else if cfg!(windows) && !root.starts_with(r"\\") {
        // A folder: taken literally too (a full path, with \\?\ before it).
        match std::path::absolute(root) {
            Ok(a) => PathBuf::from(format!(r"\\?\{}", a.display())),
            Err(_) => PathBuf::from(root),
        }
    } else {
        PathBuf::from(root)
    };
    let Listing { entries: mut listed, shut: unlistable, corrupt } = match list_tree(&path, say) {
        Ok(x) => x,
        Err(e) => {
            v.failed = Some(e);
            return v;
        }
    };
    v.listed = listed.len() as u64;
    v.corrupt = corrupt;
    let shut: std::collections::HashSet<&str> = unlistable.iter().map(|s| s.as_str()).collect();
    let inside_shut = |p: &str| !shut.is_empty() && p.match_indices('\\').any(|(i, _)| shut.contains(&p[..i]));
    let mut only_here = Vec::new();
    let mut other = Vec::new();
    for c in found.iter().filter(|c| c.listed) {
        if inside_shut(&c.path) {
            continue;
        }
        v.expected += 1;
        match listed.remove(&*c.path) {
            None => only_here.push(c.path.to_string()),
            Some((is_dir, len)) => {
                let here_dir = c.kind != Kind::File;
                if is_dir != here_dir || (!is_dir && len != c.size) {
                    other.push((c.path.to_string(), if here_dir { u64::MAX } else { c.size }, if is_dir { u64::MAX } else { len }));
                }
            }
        }
    }
    let mut only_windows: Vec<String> = listed.into_keys().collect();
    only_windows.sort();
    only_here.sort();
    other.sort();
    v.only_windows_count = only_windows.len() as u64;
    v.only_here_count = only_here.len() as u64;
    v.other_size_count = other.len() as u64;
    only_windows.truncate(LIST_MAX);
    only_here.truncate(LIST_MAX);
    other.truncate(LIST_MAX);
    v.only_windows = only_windows;
    v.only_here = only_here;
    v.other_size = other;
    v.unlistable = unlistable;
    v
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
    for w in &r.disk_warnings {
        t.push(format!("          note: {w}"));
    }
    t.push(format!(
        "Mapfile:  {}  (ddrescue {}, {})",
        r.map_path,
        if r.map_version.is_empty() { "?" } else { &r.map_version },
        if r.map_finished { "finished" } else { "NOT finished - the copy may be incomplete" }
    ));
    for w in &r.map_warnings {
        t.push(format!("          note: {w}"));
    }
    if r.ssd_unread.0 > 0 {
        t.push(format!(
            "          The image's own list: the SSD could not read {} places ({} bytes) when the image was made.",
            commas(r.ssd_unread.0),
            commas(r.ssd_unread.1)
        ));
        t.push("          They are zeros in the image, and count as never read below.".to_string());
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
    t.push(format!(
        "          {} folders and {} files found ({} in all)",
        commas(r.folders_seen),
        commas(r.files_seen),
        size(r.bytes_seen)
    ));
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
    let (left_files, left_folders) = r.leftovers();
    t.push(format!(
        "  inside folder records      {:>6}  -> {} folders affected; {} files and {} folders Windows cannot list{}",
        commas(in_folders as u64),
        commas(r.folders().count() as u64),
        commas((r.hidden_files() - left_files) as u64),
        commas((r.hidden.len() - r.hidden_files() - left_folders) as u64),
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
    if left_files + left_folders > 0 {
        t.push(String::new());
        t.push(format!(
            "{} files and {} folders are listed after a blank record that ddrescue DID read: the old drive had",
            commas(left_files as u64),
            commas(left_folders as u64)
        ));
        t.push("that record blank too, so Windows never listed them there either. They are most likely leftovers".to_string());
        t.push("of files deleted or replaced long ago (a folder's records past its end are not cleared), not".to_string());
        t.push("files the damage hid. The copying still copies them; treat those copies as doubtful (they are".to_string());
        t.push("marked in map-check-hidden-files.csv).".to_string());
    }
    if !r.claimed_twice.is_empty() || !r.claimed_free.is_empty() {
        t.push(String::new());
        t.push("CHECK OF THE VOLUME'S RECORDS: some records point at space that is not theirs:".to_string());
        for (a, b, n) in r.claimed_twice.iter().take(10) {
            if a == b {
                t.push(format!("  {} claims {} clusters twice (its cluster chain loops)", a.describe(), commas(*n)));
            } else {
                t.push(format!("  {} and {} claim the same {} clusters", a.describe(), b.describe(), commas(*n)));
            }
        }
        if r.claimed_twice.len() > 10 {
            t.push(format!("  ...and {} more pairs", r.claimed_twice.len() - 10));
        }
        for (a, n) in r.claimed_free.iter().take(10) {
            t.push(format!("  {} claims {} clusters the volume marks as free", a.describe(), commas(*n)));
        }
        if r.claimed_free.len() > 10 {
            t.push(format!("  ...and {} more", r.claimed_free.len() - 10));
        }
        t.push("The volume's own records never do that: such a record is damaged, or a leftover of something".to_string());
        t.push("deleted or replaced long ago, and what it points at may belong to another file. Tell Claude".to_string());
        t.push("before relying on those files (map-check-consistency.csv lists them all).".to_string());
    }
    let (rebuilt_folders, rebuilt_files) = r.rebuilt();
    if rebuilt_folders > 0 {
        t.push(String::new());
        let one = rebuilt_folders == 1;
        t.push(format!(
            "{} {} whose own record was lost {} found by {} contents: the {} {} inside keep their",
            commas(rebuilt_folders),
            if one { "folder" } else { "folders" },
            if one { "was" } else { "were" },
            if one { "its" } else { "their" },
            commas(rebuilt_files),
            if rebuilt_files == 1 { "file" } else { "files" }
        ));
        t.push(format!(
            "names, dates and folders, under \"{}\" (the copying puts them there too).",
            crate::exfat::FOUND_FOLDERS
        ));
    }
    if r.lost_clusters > 0 {
        t.push(String::new());
        t.push(format!(
            "{} of data is marked as in use but belongs to no file this check could find. That is",
            size(r.lost_bytes())
        ));
        t.push("usually files whose folder records were lost; their contents are still on the disk.".to_string());
    }
    if let Some((ts, path)) = &r.newest {
        t.push(String::new());
        t.push(format!("Newest date on the volume: {} ({}).", crate::exfat::timestamp_string(*ts), shown(path)));
        if r.newer_than_copy() {
            t.push(format!(
                "WARNING: that is after ddrescue finished ({}). Something may have changed this SSD after the copy",
                r.map_time
            ));
            t.push("(a computer it was plugged into, say) - or that file's date is simply wrong. Ask before going on.".to_string());
        } else if !r.map_time.is_empty() {
            t.push(format!("That is before ddrescue finished ({}): nothing on the SSD is dated after the copy.", r.map_time));
        }
    }
    t.push(String::new());
    match &r.drive {
        Some(d) => t.push(format!("The volume is drive {d} on this PC; the CSV files say whether Windows can see each file.")),
        None => t.push("The volume has no drive letter here, so whether Windows can see each file was not checked.".to_string()),
    }
    t.extend(windows_lines(r));
    t
}

/// What Windows' own reading of the volume says (see Compare).
pub fn windows_lines(r: &Report) -> Vec<String> {
    let mut t = Vec::new();
    let Some(w) = &r.windows else {
        t.push("Not checked against Windows' own reading of the volume: it has no drive letter here. To check,".to_string());
        t.push("open the image read-only (AIM Toolkit, read-only ticked) and run this check again.".to_string());
        return t;
    };
    if w.read_only == Some(false) {
        t.push(format!("WARNING: {} is NOT read-only: Windows can write to it. If it is the image, unmount it and", w.root));
        t.push("open it again read-only (in AIM Toolkit, tick read-only). If it is the recovered SSD, switch it off".to_string());
        t.push("in step 3 and protect it again.".to_string());
    }
    if let Some(e) = &w.failed {
        t.push(format!("Could not check against what Windows lists on {}: {e}.", w.root));
        return t;
    }
    if w.agrees() {
        t.push(format!(
            "Checked against Windows' own reading of the volume (drive {}): Windows lists {} files and folders,",
            w.root,
            commas(w.listed)
        ));
        t.push("exactly the ones this check says it can list, with the same sizes. (Windows and this check read".to_string());
        t.push("the volume independently, so this is a check of the check.)".to_string());
    } else {
        t.push(format!("Checked against Windows' own reading of the volume (drive {}): they DIFFER.", w.root));
        t.push(format!(
            "  Windows lists {} files and folders this check says it cannot list (or never found){}",
            commas(w.only_windows_count),
            w.only_windows.first().map(|p| format!(", such as {p}")).unwrap_or_default()
        ));
        t.push(format!(
            "  and does not list {} that this check says it can{}; {} have another size there.",
            commas(w.only_here_count),
            w.only_here.first().map(|p| format!(", such as {p}")).unwrap_or_default(),
            commas(w.other_size_count)
        ));
        t.push("Tell Claude before going on: this check may read part of the volume wrongly".to_string());
        t.push("(map-check-windows-compare.csv lists every difference).".to_string());
    }
    if !w.unlistable.is_empty() {
        t.push(format!(
            "(Windows would not let the program look into {} folders, such as {}: left out of the comparison.)",
            commas(w.unlistable.len() as u64),
            w.unlistable[0]
        ));
    }
    if !w.corrupt.is_empty() {
        t.push(format!(
            "(Windows reports {} folders as corrupted and lists nothing in them, such as {}.)",
            commas(w.corrupt.len() as u64),
            shown(&w.corrupt[0])
        ));
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
    for it in &r.lost_items {
        body.push_str(&csv_row(&[
            "LOST DATA".to_string(),
            it.path.clone(),
            it.size.to_string(),
            it.damaged_bytes().to_string(),
            ranges_text(&it.ranges),
            it.content.word().to_string(),
            "no".to_string(),
            "data no folder record names, saved by the copying under this name".to_string(),
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
        let mut body =
            csv_row(&["kind", "path", "size_bytes", "windows_lists_it", "windows_can_see", "the_damage_hid_it", "why"]);
        for (e, see) in &r.hidden {
            body.push_str(&csv_row(&[
                if e.is_dir { "FOLDER" } else { "FILE" }.to_string(),
                e.path.clone(),
                e.data_length.to_string(),
                "no".to_string(),
                yes_no(*see).to_string(),
                if r.leftover(e) { "no - most likely a leftover" } else { "yes" }.to_string(),
                r.hidden_why(e),
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

    if !r.claimed_twice.is_empty() || !r.claimed_free.is_empty() {
        let p = out_dir.join("map-check-consistency.csv");
        let mut body = csv_row(&["problem", "claimed_by", "also_claimed_by", "clusters"]);
        for (a, b, n) in &r.claimed_twice {
            body.push_str(&csv_row(&[
                if a == b { "claims clusters twice (its cluster chain loops)" } else { "two claim the same clusters" }.to_string(),
                a.describe(),
                if a == b { String::new() } else { b.describe() },
                n.to_string(),
            ]));
        }
        for (a, n) in &r.claimed_free {
            body.push_str(&csv_row(&[
                "claims clusters the volume marks as free".to_string(),
                a.describe(),
                String::new(),
                n.to_string(),
            ]));
        }
        write_bom(&p, &body)?;
        written.push(p.display().to_string());
    }
    if let Some(w) = r.windows.as_ref().filter(|w| !w.agrees() && w.failed.is_none()) {
        let p = out_dir.join("map-check-windows-compare.csv");
        let mut body = csv_row(&["difference", "path", "size_here", "size_windows_gives"]);
        let sz = |n: u64| if n == u64::MAX { "folder".to_string() } else { n.to_string() };
        for x in &w.only_windows {
            body.push_str(&csv_row(&["Windows lists it; this check says it cannot".to_string(), x.clone(), String::new(), String::new()]));
        }
        for x in &w.only_here {
            body.push_str(&csv_row(&["this check says Windows lists it; it does not".to_string(), x.clone(), String::new(), String::new()]));
        }
        for (x, a, b) in &w.other_size {
            body.push_str(&csv_row(&["another size".to_string(), x.clone(), sz(*a), sz(*b)]));
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
    for it in &r.lost_items {
        tsv.push_str(&crate::join::tsv_line(&crate::join::MapItem {
            kind: "DAMAGED".to_string(),
            path: it.path.clone(),
            size: it.size,
            damaged_bytes: it.damaged_bytes(),
            first_damaged: it.ranges.iter().map(|r| r.0).min().unwrap_or(0),
            only_past_end: false,
            content: it.content.word().to_string(),
            maybe: false,
            ranges: ranges_text(&it.ranges),
        }));
    }
    for (e, _) in &r.hidden {
        // (Leftovers were never files Windows listed: whether a copy has them is no matter.)
        if e.is_dir || r.leftover(e) {
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
        "\"volume\":{{\"offset\":{},\"label\":{},\"serial\":{},\"cluster_size\":{},\"cluster_count\":{},\"boot_checksum_ok\":{},\"used_backup_boot\":{},\"drive\":{},\"folders\":{},\"files\":{},\"bytes\":{}}},\n",
        r.vol_offset,
        json_str(&r.vol_label),
        r.vol_serial,
        r.cluster_size,
        r.cluster_count,
        r.boot_checksum_ok,
        r.used_backup_boot,
        r.drive.as_deref().map(json_str).unwrap_or("null".into()),
        r.folders_seen,
        r.files_seen,
        r.bytes_seen
    );
    let _ = write!(
        s,
        "\"totals\":{{\"bad_bytes\":{},\"bad_areas\":{},\"files_damaged\":{},\"folders_damaged\":{},\"hidden_files\":{},\"hidden_folders\":{},\"hidden_maybe\":{},\"hidden_leftovers\":{},\"lost_clusters\":{},\"lost_bytes\":{},\"folder_problems\":{}}},\n",
        r.bad_bytes(),
        r.areas.len(),
        r.files().count(),
        r.folders().count(),
        r.hidden_files(),
        r.hidden.len() - r.hidden_files(),
        r.hidden_maybe(),
        r.leftovers().0 + r.leftovers().1,
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
            "\n{{\"path\":{},\"is_dir\":{},\"size\":{},\"maybe\":{},\"leftover\":{},\"why\":{},\"windows_can_see\":{}}}",
            json_str(&e.path),
            e.is_dir,
            e.data_length,
            e.hidden_maybe,
            r.leftover(e),
            json_str(&r.hidden_why(e)),
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
    s.push_str("],\n\"claimed_twice\":[");
    let claim = |c: &Claim| format!("{{\"kind\":{},\"path\":{},\"leftover\":{}}}", json_str(c.kind.code()), json_str(&c.path), c.leftover);
    let twice: Vec<String> =
        r.claimed_twice.iter().map(|(a, b, n)| format!("{{\"first\":{},\"again\":{},\"clusters\":{n}}}", claim(a), claim(b))).collect();
    s.push_str(&twice.join(","));
    s.push_str("],\n\"claimed_free\":[");
    let free: Vec<String> = r.claimed_free.iter().map(|(a, n)| format!("{{\"by\":{},\"clusters\":{n}}}", claim(a))).collect();
    s.push_str(&free.join(","));
    s.push_str("],\n\"windows_compare\":");
    s.push_str(&windows_json(r.windows.as_ref(), usize::MAX));
    s.push_str(",\n\"lost_runs\":[");
    let runs: Vec<String> = r.lost_runs.iter().map(|(c, n)| format!("[{c},{n}]")).collect();
    s.push_str(&runs.join(","));
    s.push_str("]}\n");
    s
}

/// Windows' reading of the volume against this check's, as JSON (null when
/// not compared), with at most `most` paths in each list.
fn windows_json(w: Option<&WindowsView>, most: usize) -> String {
    let Some(w) = w else { return "null".to_string() };
    let list = |v: &[String]| v.iter().take(most).map(|p| json_str(p)).collect::<Vec<_>>().join(",");
    let sizes: Vec<String> = w.other_size.iter().take(most).map(|(p, a, b)| {
        let n = |x: &u64| if *x == u64::MAX { "null".to_string() } else { x.to_string() };
        format!("[{},{},{}]", json_str(p), n(a), n(b))
    }).collect();
    format!(
        "{{\"root\":{},\"read_only\":{},\"agrees\":{},\"failed\":{},\"listed\":{},\"expected\":{},\"only_windows\":[{}],\"only_windows_count\":{},\"only_here\":[{}],\"only_here_count\":{},\"other_size\":[{}],\"other_size_count\":{},\"unlistable\":[{}],\"corrupt\":[{}]}}",
        json_str(&w.root),
        opt_bool(w.read_only),
        w.agrees(),
        w.failed.as_deref().map(json_str).unwrap_or_else(|| "null".to_string()),
        w.listed,
        w.expected,
        list(&w.only_windows),
        w.only_windows_count,
        list(&w.only_here),
        w.only_here_count,
        sizes.join(","),
        w.other_size_count,
        list(&w.unlistable),
        list(&w.corrupt)
    )
}

/// What the front end shows of a map check (JSON): the totals, the files
/// that lost data (most damage first), what Windows cannot list, and each
/// unread place with what it hits.
/// What the map check found, in one line (for "what just happened").
pub fn found_words(r: &Report) -> String {
    let losing: Vec<&Item> =
        r.files().filter(|i| !i.only_past_valid_data() && i.damaged_bytes() > 0).chain(r.lost_items.iter()).collect();
    let bytes: u64 = losing.iter().map(|i| i.size).sum();
    let hidden = r.hidden.iter().filter(|(e, _)| !e.is_dir && !r.leftover(e)).count() as u64;
    let n = losing.len() as u64;
    let mut t = if n == 0 {
        "No file lost data".to_string()
    } else {
        format!("{} {} lost data ({} in all)", commas(n), if n == 1 { "file" } else { "files" }, size(bytes))
    };
    if hidden > 0 {
        t.push_str(&format!(", and Windows cannot list {} more (they are copied too)", commas(hidden)));
    }
    format!("{t}, of the {} files on the SSD ({})", commas(r.files_seen), size(r.bytes_seen))
}

pub fn page_json(r: &Report) -> String {
    use crate::fmt::json_str;
    use std::collections::BTreeMap;
    let files: Vec<&Item> = r.files().collect();
    let mut losing: Vec<&Item> =
        files.iter().copied().filter(|i| !i.only_past_valid_data() && i.damaged_bytes() > 0).collect();
    // Pieces of data no folder names any more count too: they are copied,
    // and lost bytes the same way.
    losing.extend(r.lost_items.iter());
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
                "{{\"path\":{},\"is_dir\":{},\"size\":{},\"by_path\":{},\"leftover\":{},\"why\":{}}}",
                json_str(&e.path),
                e.is_dir,
                e.data_length,
                e.hidden_maybe,
                r.leftover(e),
                json_str(&r.hidden_why(e))
            )
        })
        .collect();
    // The same, grouped: one line per folder and reason (a damaged folder
    // can hide thousands of files, all for the one reason).
    let mut groups: BTreeMap<(bool, String, String, bool), (u64, u64, Vec<String>)> = BTreeMap::new();
    for (e, _) in &r.hidden {
        let parent = e.path.rsplit_once('\\').map(|(p, _)| p.to_string()).unwrap_or_default();
        let place = if e.why_hidden.starts_with("inside the folder") { String::new() } else { parent };
        // (Leftovers last.)
        let g = groups.entry((r.leftover(e), place, r.hidden_why(e), e.hidden_maybe)).or_default();
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
        .map(|((leftover, place, why, by_path), (files, folders, examples))| {
            format!(
                "{{\"where\":{},\"why\":{},\"by_path\":{},\"leftover\":{},\"files\":{},\"folders\":{},\"examples\":[{}]}}",
                json_str(place),
                json_str(why),
                by_path,
                leftover,
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
    let consistency: Vec<String> = r
        .claimed_twice
        .iter()
        .map(|(a, b, n)| {
            if a == b {
                format!("{} claims {} clusters twice (its cluster chain loops)", a.describe(), commas(*n))
            } else {
                format!("{} and {} claim the same {} clusters", a.describe(), b.describe(), commas(*n))
            }
        })
        .chain(r.claimed_free.iter().map(|(a, n)| format!("{} claims {} clusters the volume marks as free", a.describe(), commas(*n))))
        .take(6)
        .map(|x| json_str(&x))
        .collect();
    let newest = match &r.newest {
        Some((ts, path)) => format!(
            "{{\"date\":{},\"path\":{},\"after_copy\":{},\"copy_finished\":{}}}",
            json_str(&crate::exfat::timestamp_string(*ts)),
            json_str(path),
            r.newer_than_copy(),
            json_str(&r.map_time)
        ),
        None => "null".to_string(),
    };
    // What those files take in all: the room their copies need in REPAIR.
    let losing_bytes: u64 = losing.iter().map(|i| i.size).sum();
    format!(
        "{{\"bad_bytes\":{},\"bad_areas\":{},\"files_touched\":{},\"files_losing_data\":{},\"losing_bytes\":{},\"folders_affected\":{},\
         \"hidden_files\":{},\"hidden_folders\":{},\"hidden_by_path\":{},\"lost_bytes\":{},\"folder_problems\":{},\
         \"files_seen\":{},\"folders_seen\":{},\"bytes_seen\":{},\"disk_size\":{},\"table_disk_size\":{},\"matches_mapfile\":{},\
         \"volume\":{{\"label\":{},\"offset\":{},\"cluster_size\":{},\"clusters\":{}}},\"drive\":{},\
         \"newest\":{},\"rebuilt_folders\":{},\"rebuilt_files\":{},\"files\":[{}],\"hidden\":[{}],\
         \"hidden_groups\":[{}],\"leftover_files\":{},\"leftover_folders\":{},\"claimed_twice\":{},\"claimed_free\":{},\
         \"consistency_examples\":[{}],\"windows\":{},\"windows_lines\":[{}],\"areas\":[{}]}}",
        r.bad_bytes(),
        r.areas.len(),
        files.len(),
        losing.len(),
        losing_bytes,
        r.folders().count(),
        r.hidden_files(),
        r.hidden.len() - r.hidden_files(),
        r.hidden_maybe(),
        r.lost_bytes(),
        r.folder_problems.len(),
        r.files_seen,
        r.folders_seen,
        r.bytes_seen,
        r.disk_size,
        opt(r.table_disk_size),
        r.matches_mapfile.map(|b| b.to_string()).unwrap_or_else(|| "null".to_string()),
        json_str(&r.vol_label),
        r.vol_offset,
        r.cluster_size,
        r.cluster_count,
        r.drive.as_deref().map(json_str).unwrap_or_else(|| "null".to_string()),
        newest,
        r.rebuilt().0,
        r.rebuilt().1,
        file_list.join(","),
        hidden.join(","),
        hidden_groups.join(","),
        r.leftovers().0,
        r.leftovers().1,
        r.claimed_twice.len(),
        r.claimed_free.len(),
        consistency.join(","),
        windows_json(r.windows.as_ref(), 20),
        windows_lines(r).iter().map(|l| json_str(l)).collect::<Vec<_>>().join(","),
        areas.join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::Memory;

    /// Every "1 TB" drive holds exactly this many bytes (1,953,525,168
    /// sectors), and every "2 TB" drive this many.
    const TB1: u64 = 1_000_204_886_016;
    const TB2: u64 = 2_000_398_934_016;

    /// A disk for the tests that holds nothing but the pieces given.
    struct Sparse {
        size: u64,
        pieces: Vec<(u64, Vec<u8>)>,
    }

    impl Source for Sparse {
        fn size(&self) -> u64 {
            self.size
        }
        fn read_at(&self, off: u64, len: usize) -> std::io::Result<Vec<u8>> {
            let end = off.saturating_add(len as u64).min(self.size);
            if end <= off {
                return Ok(Vec::new());
            }
            let mut out = vec![0u8; (end - off) as usize];
            for (at, data) in &self.pieces {
                let (s, e) = ((*at).max(off), (at + data.len() as u64).min(end));
                if s < e {
                    out[(s - off) as usize..(e - off) as usize].copy_from_slice(&data[(s - at) as usize..(e - at) as usize]);
                }
            }
            Ok(out)
        }
        fn describe(&self) -> String {
            "a test disk".to_string()
        }
    }

    /// The partition table Windows writes on a new drive of `size` bytes: GPT
    /// with one basic data partition, its header and entries at the start
    /// and its backup in the drive's last sectors; and the start of NTFS.
    fn set_up_by_windows(size: u64) -> Vec<(u64, Vec<u8>)> {
        let last = size / 512 - 1;
        let mut entries = vec![0u8; 128 * 128];
        let basic_data = [0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7];
        entries[0..16].copy_from_slice(&basic_data);
        entries[16] = 1;
        entries[32..40].copy_from_slice(&2048u64.to_le_bytes());
        entries[40..48].copy_from_slice(&(last - 34).to_le_bytes());
        let header = |me: u64, other: u64, at: u64| {
            let mut h = vec![0u8; 512];
            h[0..8].copy_from_slice(b"EFI PART");
            h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
            h[12..16].copy_from_slice(&92u32.to_le_bytes());
            h[24..32].copy_from_slice(&me.to_le_bytes());
            h[32..40].copy_from_slice(&other.to_le_bytes());
            h[40..48].copy_from_slice(&34u64.to_le_bytes());
            h[48..56].copy_from_slice(&(last - 33).to_le_bytes());
            h[72..80].copy_from_slice(&at.to_le_bytes());
            h[80..84].copy_from_slice(&128u32.to_le_bytes());
            h[84..88].copy_from_slice(&128u32.to_le_bytes());
            h[88..92].copy_from_slice(&gpt::crc32(&entries).to_le_bytes());
            let c = gpt::crc32(&h[..92]);
            h[16..20].copy_from_slice(&c.to_le_bytes());
            h
        };
        let mut ntfs = vec![0u8; 512];
        ntfs[0..3].copy_from_slice(&[0xEB, 0x52, 0x90]);
        ntfs[3..11].copy_from_slice(b"NTFS    ");
        ntfs[510..512].copy_from_slice(&[0x55, 0xAA]);
        let (main, backup) = (header(1, last, 2), header(last, 1, last - 32));
        vec![(512, main), (1024, entries.clone()), (2048 * 512, ntfs), ((last - 32) * 512, entries), (last * 512, backup)]
    }

    #[test]
    fn what_windows_lists_is_compared_name_by_name() {
        let dir = std::env::temp_dir().join(format!("rp-compare-{}", std::process::id()));
        // Taken literally on Windows, so a name ending in a space can be made:
        // the folder made a full path once (that would trim such a name), with
        // \\?\ before it, and the names joined on after.
        let base = if cfg!(windows) {
            PathBuf::from(format!(r"\\?\{}", std::path::absolute(&dir).unwrap().display()))
        } else {
            dir.clone()
        };
        let _ = std::fs::remove_dir_all(&base);
        let file = |rel: &str, n: usize| {
            let p = base.join(rel.replace('\\', std::path::MAIN_SEPARATOR_STR));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, vec![7u8; n]).unwrap();
        };
        file(r"Photos\IMG_0001.JPG", 100);
        file(r"Photos\name ending in a space ", 7);
        file(r"Docs\a.txt", 5);
        std::fs::create_dir_all(base.join("Empty")).unwrap();
        let c = |kind: Kind, path: &str, size: u64, listed: bool| Claimant { kind, path: path.into(), size, listed, leftover: false };
        let found = vec![
            c(Kind::Folder, "Photos", 4096, true),
            c(Kind::File, r"Photos\IMG_0001.JPG", 100, true),
            c(Kind::File, r"Photos\name ending in a space ", 7, true),
            c(Kind::Folder, "Docs", 4096, true),
            c(Kind::File, r"Docs\a.txt", 5, true),
            // Hidden from Windows: not expected in its listing.
            c(Kind::File, r"Docs\after the damage.txt", 9, false),
            c(Kind::Folder, "Empty", 4096, true),
        ];
        let root = dir.display().to_string();
        let v = windows_view(&root, &found, true, &mut |_| {});
        assert!(v.agrees(), "{v:?}");
        assert_eq!((v.listed, v.expected), (6, 6));
        // One file gone, one added, one a byte longer, and a folder where a file was.
        std::fs::remove_file(base.join("Docs").join("a.txt")).unwrap();
        file("extra.bin", 1);
        file(r"Photos\IMG_0001.JPG", 101);
        std::fs::remove_dir(base.join("Empty")).unwrap();
        file("Empty", 3);
        let v = windows_view(&root, &found, true, &mut |_| {});
        assert!(!v.agrees());
        assert_eq!(v.only_here, vec![r"Docs\a.txt".to_string()]);
        assert_eq!(v.only_windows, vec!["extra.bin".to_string()]);
        assert_eq!(
            v.other_size,
            vec![("Empty".to_string(), u64::MAX, 3), (r"Photos\IMG_0001.JPG".to_string(), 100, 101)]
        );
        // Nothing to compare with.
        let gone = windows_view(&dir.join("not there").display().to_string(), &found, true, &mut |_| {});
        assert!(gone.failed.is_some() && !gone.agrees());
        let too_many = windows_view(&root, &found, false, &mut |_| {});
        assert!(too_many.failed.is_some());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_drive_as_big_as_the_old_one_is_not_taken_for_the_copy() {
        // SPARE: a 1 TB drive Windows set up for itself. Its partition table
        // was made for the old drive's size too: every 1 TB drive has it.
        let spare = Sparse { size: TB1, pieces: set_up_by_windows(TB1) };
        let t = gpt::read(&spare, &[]);
        assert_eq!(t.original_disk_size(), Some(TB1));
        assert!(!is_the_copy(&spare, &t, TB1));
        let why = not_the_copy(&spare, &t, TB1).unwrap();
        assert!(why.contains("another drive of the same size"), "{why}");
        // The recovered SSD: the old drive's partition table at the start of
        // a 2 TB SSD (whatever its volume holds, or if it could not be read).
        let mut pieces = set_up_by_windows(TB1);
        pieces.retain(|p| p.0 != 2048 * 512);
        let ssd = Sparse { size: TB2, pieces };
        let t = gpt::read(&ssd, &[]);
        assert!(is_the_copy(&ssd, &t, TB1) && not_the_copy(&ssd, &t, TB1).is_none());
        // ...but not with the mapfile of another drive.
        assert!(!is_the_copy(&ssd, &t, TB2));
        let why = not_the_copy(&ssd, &t, TB2).unwrap();
        assert!(why.contains("made for a 1,000,204,886,016-byte drive"), "{why}");
        // A 2 TB drive Windows set up for itself, and one with no partition table.
        let two = Sparse { size: TB2, pieces: set_up_by_windows(TB2) };
        assert!(!is_the_copy(&two, &gpt::read(&two, &[]), TB1));
        let blank = Sparse { size: TB2, pieces: Vec::new() };
        let t = gpt::read(&blank, &[]);
        assert!(!is_the_copy(&blank, &t, TB1));
        assert!(not_the_copy(&blank, &t, TB1).unwrap().contains("no partition table"));
    }

    #[test]
    fn an_image_of_the_copy_is_the_copy_by_its_exfat_volume() {
        // The self-test's disk: a partition table made for it, and an exFAT
        // volume - as an image of the copy has.
        let img = crate::selftest::disk_image().unwrap();
        let size = img.len() as u64;
        let disk = Memory::new(img.clone());
        let t = gpt::read(&disk, &[]);
        assert_eq!(t.original_disk_size(), Some(size));
        assert!(is_the_copy(&disk, &t, size));
        // Only its partition table, no exFAT volume: another drive that size.
        let mut table_only = vec![0u8; img.len()];
        table_only[..34 * 512].copy_from_slice(&img[..34 * 512]);
        let end = img.len() - 33 * 512;
        table_only[end..].copy_from_slice(&img[end..]);
        let other = Memory::new(table_only);
        let t = gpt::read(&other, &[]);
        assert_eq!(t.original_disk_size(), Some(size));
        assert!(!is_the_copy(&other, &t, size));
        // The rule itself; the volume is only looked for when it matters.
        assert!(copy_rule(Some(size), size, || true, size));
        assert!(!copy_rule(Some(size), size, || false, size));
        assert!(copy_rule(Some(size), size + 512, || panic!("not needed"), size));
        assert!(!copy_rule(None, size + 512, || panic!("not needed"), size));
    }
}
