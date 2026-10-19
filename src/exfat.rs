//! A read-only exFAT reader that keeps going where Windows stops.
//!
//! Windows lists a folder's files by reading its folder records until the
//! first blank record, and skips records whose checksum is wrong. On a disk
//! with unreadable sectors, a blank (zeroed) patch in the middle of a
//! folder's records therefore hides every file listed after it. This reader
//! reads every record a folder has room for, checks each one's checksum
//! itself, and notes which files Windows will not show.
//!
//! Layout and checksums follow Microsoft's exFAT specification.

use crate::disk::Source;
use std::collections::HashMap;
use std::sync::Mutex;

pub const EOC: u32 = 0xFFFF_FFFF;
pub const BAD_CLUSTER: u32 = 0xFFFF_FFF7;

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// The exFAT entry-set checksum (spec 6.3.3): every byte of the set except
/// bytes 2 and 3 of the first entry.
pub fn entry_set_checksum(set: &[u8]) -> u16 {
    let mut c: u16 = 0;
    for (i, &b) in set.iter().enumerate() {
        if i == 2 || i == 3 {
            continue;
        }
        c = (if c & 1 != 0 { 0x8000 } else { 0 }) + (c >> 1) + b as u16;
    }
    c
}

/// The boot-region checksum (spec 3.4): sectors 0-10, skipping the volume
/// flags and percent-in-use bytes of sector 0.
pub fn boot_checksum(sectors: &[u8]) -> u32 {
    let mut c: u32 = 0;
    for (i, &b) in sectors.iter().enumerate() {
        if i == 106 || i == 107 || i == 112 {
            continue;
        }
        c = (if c & 1 != 0 { 0x8000_0000u32 } else { 0 }).wrapping_add(c >> 1).wrapping_add(b as u32);
    }
    c
}

/// Is there an exFAT boot sector at this disk offset?
pub fn is_exfat_at(src: &dyn Source, offset: u64) -> bool {
    matches!(src.read_at(offset, 16), Ok(b) if b.len() >= 11 && &b[3..11] == b"EXFAT   ")
}

pub struct Volume<'a> {
    pub src: &'a dyn Source,
    /// Disk byte offset of the volume (the partition start).
    pub offset: u64,
    pub bytes_per_sector: u64,
    pub cluster_size: u64,
    pub fat_offset: u64, // bytes from the volume start
    pub fat_length: u64, // bytes, one FAT
    pub number_of_fats: u64,
    pub active_fat: u64,
    pub heap_offset: u64, // bytes from the volume start
    pub cluster_count: u32,
    pub root_cluster: u32,
    pub volume_length: u64, // bytes
    pub serial: u32,
    pub label: String,
    pub volume_flags: u16,
    pub boot_checksum_ok: bool,
    pub bitmap: Vec<u8>,
    pub bitmap_clusters: ClusterList,
    pub upcase_clusters: ClusterList,
    pub root_clusters: ClusterList,
    fat_cache: Mutex<HashMap<u64, Vec<u32>>>,
}

/// A file's clusters as runs of consecutive clusters: (first, count).
#[derive(Clone, Debug, Default)]
pub struct ClusterList {
    pub runs: Vec<(u32, u32)>,
    /// Set when the chain could not be followed to its full length.
    pub problem: Option<String>,
}

impl ClusterList {
    pub fn total(&self) -> u64 {
        self.runs.iter().map(|r| r.1 as u64).sum()
    }
    fn push(&mut self, c: u32) {
        if let Some(last) = self.runs.last_mut() {
            if last.0 as u64 + last.1 as u64 == c as u64 {
                last.1 += 1;
                return;
            }
        }
        self.runs.push((c, 1));
    }
}

const FAT_BLOCK_ENTRIES: u64 = 1 << 18; // 1 MiB of FAT per cache block

impl<'a> Volume<'a> {
    /// Open the volume whose boot sector is at `offset`.
    pub fn open(src: &'a dyn Source, offset: u64) -> Result<Volume<'a>, String> {
        Self::open_from(src, offset, 0)
    }

    /// Open the volume starting at `offset` from its backup boot sector
    /// (12 sectors on), for when the main one cannot be read.
    pub fn open_backup(src: &'a dyn Source, offset: u64, bytes_per_sector: u64) -> Result<Volume<'a>, String> {
        let v = Self::open_from(src, offset, 12 * bytes_per_sector)?;
        if v.bytes_per_sector != bytes_per_sector {
            return Err("the backup boot sector is not where its own sector size says".to_string());
        }
        Ok(v)
    }

    fn open_from(src: &'a dyn Source, offset: u64, boot_at: u64) -> Result<Volume<'a>, String> {
        let boot = src.read_at(offset + boot_at, 512).map_err(|e| format!("could not read the boot sector: {e}"))?;
        if boot.len() < 512 || &boot[3..11] != b"EXFAT   " {
            return Err("no exFAT boot sector here".to_string());
        }
        let bps_shift = boot[108] as u32;
        let spc_shift = boot[109] as u32;
        if !(9..=12).contains(&bps_shift) || bps_shift + spc_shift > 25 {
            return Err(format!("implausible exFAT sizes (sector shift {bps_shift}, cluster shift {spc_shift})"));
        }
        let bps = 1u64 << bps_shift;
        let cluster_size = bps << spc_shift;
        let number_of_fats = boot[110] as u64;
        let volume_flags = u16le(&boot, 106);
        let mut v = Volume {
            src,
            offset,
            bytes_per_sector: bps,
            cluster_size,
            fat_offset: u32le(&boot, 80) as u64 * bps,
            fat_length: u32le(&boot, 84) as u64 * bps,
            number_of_fats,
            active_fat: if number_of_fats == 2 && volume_flags & 1 != 0 { 1 } else { 0 },
            heap_offset: u32le(&boot, 88) as u64 * bps,
            cluster_count: u32le(&boot, 92),
            root_cluster: u32le(&boot, 96),
            volume_length: u64le(&boot, 72).saturating_mul(bps),
            serial: u32le(&boot, 100),
            label: String::new(),
            volume_flags,
            boot_checksum_ok: false,
            bitmap: Vec::new(),
            bitmap_clusters: ClusterList::default(),
            upcase_clusters: ClusterList::default(),
            root_clusters: ClusterList::default(),
            fat_cache: Mutex::new(HashMap::new()),
        };
        if v.cluster_count == 0 || !v.valid_cluster(v.root_cluster) {
            return Err("the exFAT boot sector's cluster count or root folder is out of range".to_string());
        }
        // Boot checksum: sectors 0-10, compared with sector 11.
        if let Ok(region) = src.read_at(offset + boot_at, (12 * bps) as usize) {
            if region.len() as u64 == 12 * bps {
                let want = boot_checksum(&region[..(11 * bps) as usize]);
                v.boot_checksum_ok = u32le(&region, (11 * bps) as usize) == want;
            }
        }
        // The root folder holds the allocation bitmap, the up-case table and the label.
        v.root_clusters = v.chain(v.root_cluster, None);
        let root = v.read_clusters(&v.root_clusters).map_err(|e| format!("could not read the root folder: {e}"))?;
        for slot in root.chunks_exact(32) {
            match slot[0] {
                0x81 if slot[1] & 1 == 0 && v.bitmap_clusters.runs.is_empty() => {
                    let len = u64le(slot, 24);
                    v.bitmap_clusters = v.chain(u32le(slot, 20), Some(len));
                    if let Ok(mut bm) = v.read_clusters(&v.bitmap_clusters) {
                        bm.truncate(len as usize);
                        v.bitmap = bm;
                    }
                }
                0x82 if v.upcase_clusters.runs.is_empty() => {
                    v.upcase_clusters = v.chain(u32le(slot, 20), Some(u64le(slot, 24)));
                }
                0x83 => {
                    let n = (slot[1] as usize).min(11);
                    let units: Vec<u16> = (0..n).map(|i| u16le(slot, 2 + i * 2)).collect();
                    v.label = String::from_utf16_lossy(&units);
                }
                _ => {}
            }
        }
        Ok(v)
    }

    pub fn valid_cluster(&self, c: u32) -> bool {
        c >= 2 && (c as u64) <= self.cluster_count as u64 + 1
    }

    /// Disk byte offset of a cluster.
    pub fn cluster_offset(&self, c: u32) -> u64 {
        self.offset + self.heap_offset + (c as u64 - 2) * self.cluster_size
    }

    /// The cluster holding a disk byte offset, if it is inside the cluster heap.
    pub fn cluster_at(&self, disk_off: u64) -> Option<u32> {
        let start = self.offset + self.heap_offset;
        if disk_off < start {
            return None;
        }
        let c = (disk_off - start) / self.cluster_size + 2;
        if c > self.cluster_count as u64 + 1 {
            return None;
        }
        Some(c as u32)
    }

    pub fn heap_end(&self) -> u64 {
        self.offset + self.heap_offset + self.cluster_count as u64 * self.cluster_size
    }

    pub fn is_allocated(&self, c: u32) -> Option<bool> {
        if !self.valid_cluster(c) {
            return None;
        }
        let i = (c - 2) as usize;
        self.bitmap.get(i / 8).map(|b| b & (1 << (i % 8)) != 0)
    }

    /// One FAT entry (cached in 1 MiB blocks).
    pub fn fat_entry(&self, c: u32) -> Option<u32> {
        let block = c as u64 / FAT_BLOCK_ENTRIES;
        let idx = (c as u64 % FAT_BLOCK_ENTRIES) as usize;
        let mut cache = self.fat_cache.lock().unwrap();
        if !cache.contains_key(&block) {
            if cache.len() > 64 {
                cache.clear();
            }
            let base = self.offset + self.fat_offset + self.active_fat * self.fat_length;
            let start = block * FAT_BLOCK_ENTRIES * 4;
            let len = (FAT_BLOCK_ENTRIES * 4).min(self.fat_length.saturating_sub(start));
            let raw = self.src.read_at(base + start, len as usize).ok()?;
            let entries: Vec<u32> = raw.chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect();
            cache.insert(block, entries);
        }
        cache.get(&block).and_then(|e| e.get(idx).copied())
    }

    /// Follow a FAT chain from `first`. With a known byte length, exactly
    /// that many clusters are followed; without (folders that have no
    /// length, like the root), until the end-of-chain mark.
    pub fn chain(&self, first: u32, length: Option<u64>) -> ClusterList {
        let mut out = ClusterList::default();
        let want = length.map(|l| l.div_ceil(self.cluster_size));
        if want == Some(0) {
            return out;
        }
        let limit = want.unwrap_or((256 << 20) / self.cluster_size + 1).min(self.cluster_count as u64);
        let mut c = first;
        let mut seen: u64 = 0;
        loop {
            if !self.valid_cluster(c) {
                out.problem = Some(format!("the cluster chain points outside the volume (cluster {c})"));
                break;
            }
            out.push(c);
            seen += 1;
            if want.is_some_and(|w| seen >= w) {
                break;
            }
            if seen >= limit {
                if want.is_some() {
                    out.problem = Some("the cluster chain is longer than the volume (a loop)".to_string());
                }
                break;
            }
            let next = match self.fat_entry(c) {
                Some(n) => n,
                None => {
                    out.problem = Some("the FAT could not be read".to_string());
                    break;
                }
            };
            if next == EOC {
                if want.is_some() {
                    out.problem = Some(format!(
                        "the cluster chain ends after {seen} of {} clusters",
                        want.unwrap_or(0)
                    ));
                }
                break;
            }
            if next == BAD_CLUSTER {
                out.problem = Some("the cluster chain runs into a cluster marked bad".to_string());
                break;
            }
            c = next;
        }
        out
    }

    /// A file's or folder's clusters from its stream extension.
    pub fn clusters_of(&self, first: u32, no_fat_chain: bool, length: u64) -> ClusterList {
        if length == 0 || first == 0 {
            return ClusterList::default();
        }
        if no_fat_chain {
            let n = length.div_ceil(self.cluster_size);
            let mut out = ClusterList::default();
            if !self.valid_cluster(first) || first as u64 + n - 1 > self.cluster_count as u64 + 1 {
                out.problem = Some("the file's clusters run past the end of the volume".to_string());
                return out;
            }
            out.runs.push((first, n as u32));
            return out;
        }
        self.chain(first, Some(length))
    }

    /// Read every cluster in a list, in order, as one buffer.
    pub fn read_clusters(&self, list: &ClusterList) -> std::io::Result<Vec<u8>> {
        let mut out = Vec::with_capacity((list.total() * self.cluster_size).min(1 << 28) as usize);
        for &(first, count) in &list.runs {
            let mut done = 0u64;
            while done < count as u64 {
                let n = (count as u64 - done).min((32 << 20) / self.cluster_size.max(1)).max(1);
                let off = self.cluster_offset(first) + done * self.cluster_size;
                let data = self.src.read_at(off, (n * self.cluster_size) as usize)?;
                let short = data.len() < (n * self.cluster_size) as usize;
                out.extend_from_slice(&data);
                if short {
                    return Ok(out);
                }
                done += n;
            }
        }
        Ok(out)
    }

    /// Map a byte position inside a cluster list to a disk offset.
    pub fn disk_offset_in(&self, list: &ClusterList, pos: u64) -> Option<u64> {
        let mut base = 0u64;
        for &(first, count) in &list.runs {
            let len = count as u64 * self.cluster_size;
            if pos < base + len {
                return Some(self.cluster_offset(first) + (pos - base));
            }
            base += len;
        }
        None
    }
}

/// One file or folder found while walking.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Path from the volume root, folders separated by '\'.
    pub path: String,
    pub is_dir: bool,
    pub attributes: u16,
    pub first_cluster: u32,
    pub no_fat_chain: bool,
    pub data_length: u64,
    pub valid_data_length: u64,
    pub modified: u32,
    pub modified_10ms: u8,
    pub modified_utc: u8,
    pub created: u32,
    pub created_10ms: u8,
    pub created_utc: u8,
    pub accessed: u32,
    pub accessed_utc: u8,
    /// Disk offset of the file's folder record.
    pub record_offset: u64,
    pub record_len: u64,
    /// Where the record's slots are on the disk: (start, end) pieces, more
    /// than one when the record crosses into the folder's next cluster.
    pub record_extents: Vec<(u64, u64)>,
    /// Windows will not list it: it sits after a blank record in its
    /// folder, in a folder Windows reports as corrupted, or inside a folder
    /// Windows cannot list.
    pub hidden_from_windows: bool,
    /// ...but Windows still opens it when given its full path: it comes
    /// before the damaged record of a folder Windows reports as corrupted.
    /// (Windows looks a name up by reading the folder's records in order
    /// until it finds it, so it never reaches the damage.)
    pub hidden_maybe: bool,
    pub why_hidden: String,
}

/// Something wrong in a folder's records.
#[derive(Clone, Debug)]
pub struct FolderIssue {
    pub folder: String,
    pub disk_offset: u64,
    pub what: String,
}

const WHY_AFTER_BLANK: &str =
    "listed after a blank (never-read) record in its folder, so Windows stops before reaching it";
const WHY_AFTER_DAMAGE: &str =
    "listed after a damaged record in its folder: Windows reports that folder as corrupted and stops there";
const WHY_BEFORE_DAMAGE: &str = "its folder holds a damaged record further on, so Windows reports the whole \
folder as corrupted and lists none of it (typing this file's full path still opens it)";

/// How Windows itself gets on reading one folder's records, as measured on
/// GitHub's Windows machines (tools/windows_check.py, which checks every
/// one of these on every build):
///   * it reads the records in order, and a blank record - even one halfway
///     through a file's record - quietly ends the folder: nothing after it
///     is listed, everything before it is;
///   * a file record that is well formed but whose checksum is wrong makes
///     it report the whole folder as corrupted: it lists none of it, though
///     files before that record still open when given their full path;
///   * a file record that is not even well formed (its parts are of the
///     wrong kinds, or it claims an impossible number of them) is skipped,
///     together with the slots it claims, and the folder goes on;
///   * any other kind of record is skipped: vendor and padding records,
///     kinds that do not exist, parts whose first part is gone. So leftover
///     junk in a folder mostly costs only the files whose records it covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    /// Windows reads the whole folder.
    None,
    /// A blank record at this slot quietly ends the folder.
    Blank(usize),
    /// A damaged record at this slot makes Windows give up on the folder.
    Damaged(usize),
}

/// Walk every folder from the root. `visit` gets every file and folder.
/// Folders whose own record has a bad checksum are reported and not
/// entered (their cluster numbers cannot be trusted).
pub fn walk(vol: &Volume, visit: &mut dyn FnMut(&Entry, &ClusterList)) -> Vec<FolderIssue> {
    walk_until(vol, visit, &|| false)
}

/// `walk`, giving up (before reading the next folder) once `stop` says so.
pub fn walk_until(vol: &Volume, visit: &mut dyn FnMut(&Entry, &ClusterList), stop: &dyn Fn() -> bool) -> Vec<FolderIssue> {
    let mut issues = Vec::new();
    // (path, clusters, hidden, maybe, why)
    let mut stack: Vec<(String, ClusterList, bool, bool, String)> =
        vec![(String::new(), vol.root_clusters.clone(), false, false, String::new())];
    let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
    while let Some((dir, clusters, dir_hidden, dir_maybe, dir_why)) = stack.pop() {
        if stop() {
            break;
        }
        if let Some(first) = clusters.runs.first() {
            if !visited.insert(first.0) {
                issues.push(FolderIssue {
                    folder: shown(&dir),
                    disk_offset: vol.cluster_offset(first.0),
                    what: "two folders claim the same records (a loop) - not followed twice".to_string(),
                });
                continue;
            }
        }
        if let Some(p) = &clusters.problem {
            issues.push(FolderIssue { folder: shown(&dir), disk_offset: 0, what: format!("folder records: {p}") });
        }
        let buf = match vol.read_clusters(&clusters) {
            Ok(b) => b,
            Err(e) => {
                issues.push(FolderIssue { folder: shown(&dir), disk_offset: 0, what: format!("could not read the folder: {e}") });
                continue;
            }
        };
        let slots = buf.len() / 32;
        let rec_off = |i: usize| vol.disk_offset_in(&clusters, (i * 32) as u64).unwrap_or(0);
        let mut stop = Stop::None;
        // Every intact file record, with its slot, in folder order.
        let mut found: Vec<(usize, Entry, ClusterList)> = Vec::new();
        let mut i = 0usize;
        while i < slots {
            let o = i * 32;
            let t = buf[o];
            let reading = stop == Stop::None;
            if t == 0x00 {
                if reading {
                    stop = Stop::Blank(i);
                }
                i += 1;
                continue;
            }
            if t & 0x80 == 0 {
                // A deleted record.
                i += 1;
                continue;
            }
            if t != 0x85 {
                // Any other kind of record is skipped (Windows does too,
                // measured): the top folder's bitmap, up-case table and label,
                // vendor and padding records, kinds that do not exist, and
                // parts of a record whose first part is gone.
                i += 1;
                continue;
            }
            let sc = buf[o + 1] as usize;
            let issue = |what: &str, issues: &mut Vec<FolderIssue>| {
                issues.push(FolderIssue { folder: shown(&dir), disk_offset: rec_off(i), what: what.to_string() });
            };
            if i + sc >= slots {
                issue("a file record cut off at the end of the folder - skipped", &mut issues);
                i += 1;
                continue;
            }
            if (1..=sc).any(|k| buf[o + 32 * k] == 0x00) {
                // The rest of the record is blank: to Windows that is the end of the folder.
                if reading {
                    stop = Stop::Blank(i);
                }
                issues.push(FolderIssue {
                    folder: shown(&dir),
                    disk_offset: rec_off(i),
                    what: "a damaged file record (parts missing) - the file it described cannot be identified; \
                           Windows stops reading the folder there"
                        .to_string(),
                });
                i += 1;
                continue;
            }
            if !(2..=18).contains(&sc) {
                // Windows skips it together with every slot it claims, however
                // many (measured for 0, 1, 19 and 200 parts).
                issue(
                    "a damaged file record (impossible number of parts) - Windows skips it, and the records it claims",
                    &mut issues,
                );
                i += sc + 1;
                continue;
            }
            let set = &buf[o..o + 32 * (sc + 1)];
            let stream = &set[32..64];
            // Name records first, then any vendor extension records.
            let name_entries = (stream[3] as usize).div_ceil(15);
            let structure_ok = stream[0] == 0xC0
                && stream[3] >= 1
                && name_entries < sc
                && (0..name_entries).all(|k| set[64 + k * 32] == 0xC1)
                && (name_entries..sc - 1).all(|k| set[64 + k * 32] & 0xC0 == 0xC0);
            if !structure_ok {
                // Windows skips the record and every slot it claims.
                issue(
                    "a damaged file record (its parts are of the wrong kinds) - the file it described cannot be \
                     identified; Windows skips it",
                    &mut issues,
                );
                i += sc + 1;
                continue;
            }
            if entry_set_checksum(set) != u16le(set, 2) {
                if reading {
                    stop = Stop::Damaged(i);
                }
                issue(
                    "a damaged file record (checksum wrong) - the file it described cannot be identified; Windows \
                     reports this folder as corrupted",
                    &mut issues,
                );
                i += 1;
                continue;
            }
            let name_len = stream[3] as usize;
            let mut units: Vec<u16> = Vec::with_capacity(name_len);
            for k in 0..name_entries {
                let e = &set[64 + k * 32..96 + k * 32];
                for u in 0..15 {
                    if units.len() < name_len {
                        units.push(u16le(e, 2 + u * 2));
                    }
                }
            }
            let name = String::from_utf16_lossy(&units);
            let attributes = u16le(set, 4);
            let is_dir = attributes & 0x10 != 0;
            let path = if dir.is_empty() { name.clone() } else { format!("{dir}\\{name}") };
            let mut record_extents: Vec<(u64, u64)> = Vec::new();
            for k in 0..=sc {
                if let Some(d) = vol.disk_offset_in(&clusters, (o + 32 * k) as u64) {
                    match record_extents.last_mut() {
                        Some(last) if last.1 == d => last.1 = d + 32,
                        _ => record_extents.push((d, d + 32)),
                    }
                }
            }
            let entry = Entry {
                path,
                is_dir,
                attributes,
                first_cluster: u32le(stream, 20),
                no_fat_chain: stream[1] & 2 != 0,
                data_length: u64le(stream, 24),
                valid_data_length: u64le(stream, 8),
                // File directory entry (spec 7.4): CreateTimestamp @8,
                // LastModifiedTimestamp @12, LastAccessedTimestamp @16, their
                // 10 ms parts @20 and @21, and UTC offsets @22, @23 and @24.
                modified: u32le(set, 12),
                modified_10ms: set[21],
                modified_utc: set[23],
                created: u32le(set, 8),
                created_10ms: set[20],
                created_utc: set[22],
                accessed: u32le(set, 16),
                accessed_utc: set[24],
                record_offset: rec_off(i),
                record_len: 32 * (sc as u64 + 1),
                record_extents,
                hidden_from_windows: false,
                hidden_maybe: false,
                why_hidden: String::new(),
            };
            let clist = if stream[1] & 1 != 0 {
                vol.clusters_of(entry.first_cluster, entry.no_fat_chain, entry.data_length)
            } else {
                ClusterList::default()
            };
            found.push((i, entry, clist));
            i += sc + 1;
        }
        // What Windows makes of each file, now the whole folder has been read.
        let mut subdirs: Vec<(String, ClusterList, bool, bool, String)> = Vec::new();
        for (slot, mut entry, clist) in found {
            let (hidden, maybe, why) = if dir_hidden && !dir_maybe {
                (true, false, dir_why.clone())
            } else {
                match stop {
                    Stop::Damaged(d) if slot > d => (true, false, WHY_AFTER_DAMAGE.to_string()),
                    Stop::Blank(b) if slot > b => (true, false, WHY_AFTER_BLANK.to_string()),
                    Stop::Damaged(_) => (true, true, WHY_BEFORE_DAMAGE.to_string()),
                    _ if dir_hidden => (true, true, dir_why.clone()),
                    _ => (false, false, String::new()),
                }
            };
            entry.hidden_from_windows = hidden;
            entry.hidden_maybe = maybe;
            entry.why_hidden = why;
            visit(&entry, &clist);
            if entry.is_dir {
                // Name the outermost folder Windows cannot (or may not) list.
                let (sub_hidden, sub_maybe, sub_why) = if dir_hidden && (!dir_maybe || entry.hidden_maybe) {
                    (true, dir_maybe, dir_why.clone())
                } else if entry.hidden_from_windows {
                    let how = if entry.hidden_maybe {
                        "cannot list (typing the full path still opens it)"
                    } else {
                        "cannot list"
                    };
                    (true, entry.hidden_maybe, format!("inside the folder {}, which Windows {how}", shown(&entry.path)))
                } else {
                    (false, false, String::new())
                };
                subdirs.push((entry.path.clone(), clist, sub_hidden, sub_maybe, sub_why));
            }
        }
        // Depth-first, in the order the folder lists them.
        for s in subdirs.into_iter().rev() {
            stack.push(s);
        }
    }
    issues
}

fn shown(path: &str) -> String {
    if path.is_empty() {
        "(the top folder)".to_string()
    } else {
        path.to_string()
    }
}

/// An exFAT timestamp as "YYYY-MM-DD HH:MM:SS" (local time as stored).
pub fn timestamp_string(ts: u32) -> String {
    let sec = (ts & 0x1F) * 2;
    let min = (ts >> 5) & 0x3F;
    let hour = (ts >> 11) & 0x1F;
    let day = (ts >> 16) & 0x1F;
    let month = (ts >> 21) & 0x0F;
    let year = 1980 + (ts >> 25);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}")
}

/// Days from 1970-01-01 to a date (proleptic Gregorian calendar).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12; // March = 0
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// This PC's offset from UTC right now, in seconds.
fn local_offset_now() -> i64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    match crate::sys::local_time(now) {
        Some(t) => {
            let local = days_from_civil(t[0] as i64, t[1] as u32, t[2] as u32) * 86_400
                + t[3] as i64 * 3600
                + t[4] as i64 * 60
                + t[5] as i64;
            local - now
        }
        None => 0,
    }
}

/// An exFAT timestamp (with its 10 ms part and UTC offset byte) as Unix
/// seconds and nanoseconds. exFAT stores local time; with a valid UTC
/// offset the result is exact, without one this PC's offset is assumed.
pub fn timestamp_to_unix(ts: u32, ms10: u8, utc: u8) -> Option<(i64, u32)> {
    let sec = ((ts & 0x1F) * 2) as i64;
    let min = ((ts >> 5) & 0x3F) as i64;
    let hour = ((ts >> 11) & 0x1F) as i64;
    let day = (ts >> 16) & 0x1F;
    let month = (ts >> 21) & 0x0F;
    let year = 1980 + (ts >> 25) as i64;
    if !(1..=12).contains(&month) || day == 0 || hour > 23 || min > 59 || sec > 59 || ms10 > 199 {
        return None;
    }
    let mut t = days_from_civil(year, month, day) * 86_400 + hour * 3600 + min * 60 + sec + (ms10 / 100) as i64;
    if utc & 0x80 != 0 {
        let v = (utc & 0x7F) as i64;
        let quarters = if v & 0x40 != 0 { v - 128 } else { v };
        t -= quarters * 15 * 60;
    } else {
        t -= local_offset_now();
    }
    Some((t, (ms10 % 100) as u32 * 10_000_000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_offsets() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2024, 2, 29), 19_782);
        // 2023-07-14 10:30:00 at UTC+02:00 (8 quarters) = 08:30:00 UTC.
        let ts = ((2023 - 1980) << 25) | (7 << 21) | (14 << 16) | (10 << 11) | (30 << 5);
        let (t, n) = timestamp_to_unix(ts, 150, 0x80 | 8).unwrap();
        assert_eq!(t, days_from_civil(2023, 7, 14) * 86_400 + 8 * 3600 + 30 * 60 + 1);
        assert_eq!(n, 500_000_000);
        // UTC-05:00 = -20 quarters, stored as 7-bit two's complement.
        let (t2, _) = timestamp_to_unix(ts, 0, 0x80 | (128 - 20) as u8).unwrap();
        assert_eq!(t2, days_from_civil(2023, 7, 14) * 86_400 + 15 * 3600 + 30 * 60);
        assert_eq!(timestamp_to_unix(0, 0, 0x80), None);
    }

    #[test]
    fn checksums_match_the_spec_examples() {
        // A set made by exfat_image.py for "x" (verified there against fsck.exfat).
        let mut set = [0u8; 96];
        set[0] = 0x85;
        set[1] = 2;
        set[32] = 0xC0;
        set[35] = 1;
        set[64] = 0xC1;
        set[66] = b'x';
        let c = entry_set_checksum(&set);
        set[2..4].copy_from_slice(&c.to_le_bytes());
        assert_eq!(entry_set_checksum(&set), c, "bytes 2-3 are not part of the checksum");
        set[66] = b'y';
        assert_ne!(entry_set_checksum(&set), c);
    }
}
