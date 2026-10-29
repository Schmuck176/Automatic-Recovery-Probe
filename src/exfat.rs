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
        // (16-bit, wrapping, as the specification's UInt16 arithmetic.)
        c = (if c & 1 != 0 { 0x8000u16 } else { 0 }).wrapping_add(c >> 1).wrapping_add(b as u16);
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
    /// FAT blocks read so far, and the order they were read in (the oldest
    /// goes first when there are FAT_BLOCKS_KEPT).
    fat_cache: Mutex<(HashMap<u64, Vec<u32>>, std::collections::VecDeque<u64>)>,
    /// What each round of the search for folders whose own record is lost
    /// found (the copying walks the folders twice: to count, then to copy).
    orphan_rounds: Mutex<Vec<Vec<(u32, ClusterList)>>>,
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

/// The most a folder can hold, by the exFAT rules.
const MAX_FOLDER: u64 = 256 << 20;

const FAT_BLOCK_ENTRIES: u64 = 1 << 18; // 1 MiB of FAT per cache block
/// FAT blocks kept: 128 MB, the whole FAT of a 1 TB volume with clusters of
/// 32 KB or more (one with 128 KB clusters, as Windows makes them, has a FAT
/// of 30 MB). Past that the oldest block goes, one at a time: a walk that
/// jumps about the FAT then reads again only what it has to.
const FAT_BLOCKS_KEPT: usize = 128;

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
            fat_cache: Mutex::new((HashMap::new(), std::collections::VecDeque::new())),
            orphan_rounds: Mutex::new(Vec::new()),
        };
        if v.cluster_count == 0 || !v.valid_cluster(v.root_cluster) {
            return Err("the exFAT boot sector's cluster count or root folder is out of range".to_string());
        }
        // Its sizes must fit together (the exFAT rules): the FAT between the
        // boot region and the clusters, big enough for every cluster, and the
        // clusters inside the volume. A boot sector that says otherwise is
        // damaged - believed, it could have the program go over billions of
        // clusters that are not there.
        let heap_bytes = v.cluster_count as u64 * cluster_size;
        if !(1..=2).contains(&number_of_fats)
            || v.fat_offset < 24 * bps
            || v.fat_offset + number_of_fats * v.fat_length > v.heap_offset
            || v.fat_length < (v.cluster_count as u64 + 2) * 4
            || (v.volume_length > 0 && v.heap_offset + heap_bytes > v.volume_length)
        {
            return Err(format!(
                "the exFAT boot sector's sizes do not fit together (FAT at {} for {} bytes, {} clusters of {} bytes from {}, in \
                 a volume of {} bytes)",
                v.fat_offset, v.fat_length, v.cluster_count, cluster_size, v.heap_offset, v.volume_length
            ));
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
                    // (One bit a cluster: never more, whatever a damaged
                    // record says - read whole, it is held in memory.)
                    let len = u64le(slot, 24).min((v.cluster_count as u64).div_ceil(8));
                    v.bitmap_clusters = v.chain(u32le(slot, 20), Some(len));
                    if let Ok(mut bm) = v.read_clusters(&v.bitmap_clusters) {
                        bm.truncate(len as usize);
                        v.bitmap = bm;
                    }
                }
                0x82 if v.upcase_clusters.runs.is_empty() => {
                    // (At most 65,536 two-byte letters.)
                    v.upcase_clusters = v.chain(u32le(slot, 20), Some(u64le(slot, 24).min(128 << 10)));
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
        let mut guard = self.fat_cache.lock().unwrap();
        let (cache, order) = &mut *guard;
        if !cache.contains_key(&block) {
            if cache.len() >= FAT_BLOCKS_KEPT {
                if let Some(oldest) = order.pop_front() {
                    cache.remove(&oldest);
                }
            }
            let base = self.offset + self.fat_offset + self.active_fat * self.fat_length;
            let start = block * FAT_BLOCK_ENTRIES * 4;
            let len = (FAT_BLOCK_ENTRIES * 4).min(self.fat_length.saturating_sub(start));
            let raw = self.src.read_at(base + start, len as usize).ok()?;
            let entries: Vec<u32> = raw.chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect();
            cache.insert(block, entries);
            order.push_back(block);
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
        // (A cluster met again: the chain loops. Followed on, a folder would
        // list its files over and over - each copied again, as "name (2)",
        // "name (3)"... - and a file would repeat a piece of itself.)
        let mut met: std::collections::HashSet<u32> = std::collections::HashSet::new();
        loop {
            if !self.valid_cluster(c) {
                out.problem = Some(format!("the cluster chain points outside the volume (cluster {c})"));
                break;
            }
            if !met.insert(c) {
                out.problem = Some(format!("the cluster chain loops back on itself (to cluster {c})"));
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
    /// When it is listed after a blank record in its folder (or is inside a
    /// folder that is), where that blank record is on the disk: (start, end)
    /// pieces. Empty for any other reason. A blank record in a place ddrescue
    /// never read is the damage; one in a place it did read was blank on the
    /// old drive too, so Windows never listed what follows there either (it
    /// is most likely a leftover of something deleted or replaced long ago).
    /// The map check, which knows the never-read places, tells the two apart.
    pub hidden_by_blank: Vec<(u64, u64)>,
}

/// Something wrong in a folder's records.
#[derive(Clone, Debug)]
pub struct FolderIssue {
    pub folder: String,
    pub disk_offset: u64,
    pub what: String,
}

const WHY_AFTER_BLANK: &str = "listed after a blank record in its folder, so Windows stops before reaching it";
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
    walk_until(vol, visit, &|| false, &mut |_| {})
}

/// Where folders found by their contents go: their own name is lost.
pub const FOUND_FOLDERS: &str = "found folders (name lost)";
const WHY_ORPHAN: &str = "its folder's own record (with the folder's name) was in a place ddrescue never read or is \
damaged, so Windows cannot reach the folder; the program found the folder by its contents";

/// Clusters something found by the walk claims (bit i = cluster i + 2).
struct Claimed {
    bits: Vec<u64>,
}

impl Claimed {
    fn new(vol: &Volume) -> Claimed {
        Claimed { bits: vec![0; (vol.cluster_count as usize).div_ceil(64)] }
    }
    fn mark(&mut self, list: &ClusterList) {
        for &(first, count) in &list.runs {
            for c in first..first.saturating_add(count) {
                let i = (c as usize).wrapping_sub(2);
                if i / 64 < self.bits.len() {
                    self.bits[i / 64] |= 1 << (i % 64);
                }
            }
        }
    }
    fn has(&self, c: u32) -> bool {
        let i = (c as usize).wrapping_sub(2);
        i / 64 < self.bits.len() && self.bits[i / 64] & (1 << (i % 64)) != 0
    }
}

/// A folder to walk: its path, its clusters, whether Windows cannot list
/// it (and still opens it by its full path), why, and where the blank
/// record that hides it is (see Entry::hidden_by_blank).
type Start = (String, ClusterList, bool, bool, String, Vec<(u64, u64)>);

/// `walk`, giving up (before reading the next folder) once `stop` says so;
/// `note` hears how a long search for lost folders is getting on.
///
/// After the folders reached from the top folder, it walks the folders
/// whose own record is lost (a never-read place, or a damaged record, in
/// their parent folder): their clusters are marked in use but nothing found
/// claims them, and they still hold valid folder records. Each such folder is
/// walked as "found folders (name lost)\folder at cluster N", so its files
/// keep their names, dates and folders.
pub fn walk_until(
    vol: &Volume,
    visit: &mut dyn FnMut(&Entry, &ClusterList),
    stop: &dyn Fn() -> bool,
    note: &mut dyn FnMut(&str),
) -> Vec<FolderIssue> {
    let mut issues = Vec::new();
    let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut claimed = Claimed::new(vol);
    claimed.mark(&vol.bitmap_clusters);
    claimed.mark(&vol.upcase_clusters);
    claimed.mark(&vol.root_clusters);
    let root: Start = (String::new(), vol.root_clusters.clone(), false, false, String::new(), Vec::new());
    walk_stack(vol, vec![root], visit, stop, &mut issues, &mut visited, &mut claimed);
    for round in 0..3 {
        if stop() || vol.bitmap.is_empty() {
            break;
        }
        // (The same every time the same volume is walked to the end.)
        let earlier = vol.orphan_rounds.lock().unwrap().get(round).cloned();
        let orphans = match earlier {
            Some(o) => o,
            None => {
                let Some(o) = find_orphan_folders(vol, &claimed, stop, note) else { break };
                let mut rounds = vol.orphan_rounds.lock().unwrap();
                if rounds.len() == round {
                    rounds.push(o.clone());
                }
                o
            }
        };
        if orphans.is_empty() {
            break;
        }
        let mut starts: Vec<Start> = Vec::new();
        for (first, list) in orphans {
            let path = format!("{FOUND_FOLDERS}\\folder at cluster {first}");
            // The folder itself, as if its record had been read.
            let entry = Entry {
                path: path.clone(),
                is_dir: true,
                attributes: 0x10,
                first_cluster: first,
                no_fat_chain: false,
                data_length: list.total() * vol.cluster_size,
                valid_data_length: list.total() * vol.cluster_size,
                modified: 0,
                modified_10ms: 0,
                modified_utc: 0,
                created: 0,
                created_10ms: 0,
                created_utc: 0,
                accessed: 0,
                accessed_utc: 0,
                record_offset: 0,
                record_len: 0,
                record_extents: Vec::new(),
                hidden_from_windows: true,
                hidden_maybe: false,
                why_hidden: WHY_ORPHAN.to_string(),
                hidden_by_blank: Vec::new(),
            };
            claimed.mark(&list);
            visit(&entry, &list);
            // (Everything inside gets one reason, naming this folder.)
            let why = format!(
                "inside the folder {path}, which the program found by its contents: the folder's own record \
                 (with its name) was in a place ddrescue never read or is damaged, so Windows cannot reach it"
            );
            starts.push((path, list, true, false, why, Vec::new()));
        }
        // Walked in cluster order.
        starts.reverse();
        walk_stack(vol, starts, visit, stop, &mut issues, &mut visited, &mut claimed);
    }
    issues
}

/// How one in-use file record (type 0x85) at slot `i` of a folder's records
/// reads, by the rules Windows follows (see `Stop`). The walk and the search
/// for folders whose own record is lost both go by this, so they always
/// agree on which records are sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rec {
    /// Its parts run past the end of the folder.
    CutOff,
    /// One of its parts is blank.
    BlankInside,
    /// It claims an impossible number of parts (skipped with all of them).
    BadCount(usize),
    /// Its parts are of the wrong kinds (skipped with all the slots it claims).
    BadParts(usize),
    /// Well formed, but its checksum is wrong.
    BadChecksum,
    /// Sound, with this many parts after the first.
    Sound(usize),
}

fn classify(buf: &[u8], i: usize) -> Rec {
    let slots = buf.len() / 32;
    let o = i * 32;
    let sc = buf[o + 1] as usize;
    if i + sc >= slots {
        return Rec::CutOff;
    }
    if (1..=sc).any(|k| buf[o + 32 * k] == 0x00) {
        return Rec::BlankInside;
    }
    if !(2..=18).contains(&sc) {
        return Rec::BadCount(sc);
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
        return Rec::BadParts(sc);
    }
    if entry_set_checksum(set) != u16le(set, 2) {
        return Rec::BadChecksum;
    }
    Rec::Sound(sc)
}

/// Is there a valid file record set (in use, or deleted) at the start of
/// `buf`? (A deleted set has the "in use" bit of each part cleared; the
/// checksum was made with it set.)
fn starts_valid_set(buf: &[u8]) -> bool {
    if buf.len() < 64 || (buf[0] != 0x85 && buf[0] != 0x05) {
        return false;
    }
    let sc = buf[1] as usize;
    if !(2..=18).contains(&sc) || buf.len() < 32 * (sc + 1) {
        return false;
    }
    let mut set = buf[..32 * (sc + 1)].to_vec();
    for k in 0..=sc {
        set[32 * k] |= 0x80;
    }
    classify(&set, 0) == Rec::Sound(sc)
}

/// One sound, in-use file record found in a folder's records.
struct SetInfo {
    slot: usize,
    is_dir: bool,
    first: u32,
    length: u64,
    alloc: bool,
}

/// The sound in-use file records in a folder's records, found the way the
/// walk finds them.
fn sets_in(buf: &[u8]) -> Vec<SetInfo> {
    let mut out = Vec::new();
    let slots = buf.len() / 32;
    let mut i = 0;
    while i < slots {
        let o = i * 32;
        if buf[o] != 0x85 {
            i += 1;
            continue;
        }
        match classify(buf, i) {
            Rec::Sound(sc) => {
                let stream = &buf[o + 32..o + 64];
                out.push(SetInfo {
                    slot: i,
                    is_dir: u16le(buf, o + 4) & 0x10 != 0,
                    first: u32le(stream, 20),
                    length: u64le(stream, 24),
                    alloc: stream[1] & 1 != 0,
                });
                i += sc + 1;
            }
            Rec::BadCount(sc) | Rec::BadParts(sc) => i += sc + 1,
            _ => i += 1,
        }
    }
    out
}

/// A sub-folder the walk would go into from this record.
fn names_folder(vol: &Volume, s: &SetInfo) -> bool {
    s.is_dir && s.alloc && s.length > 0 && vol.valid_cluster(s.first)
}

/// Folders whose own record is lost: among the clusters marked in use that
/// nothing found claims, those that begin with a valid file record set.
/// Each is followed through the FAT where the FAT chains it, otherwise
/// cluster after cluster while the records carry on (a record that runs on
/// from the cluster before, or one beginning the cluster that is no other
/// folder's start). Left out: folders another such folder names as a
/// sub-folder (they are walked from there), and ones whose records do not
/// point at data that is lost too. Returns (first cluster, clusters) of
/// each, in cluster order.
fn find_orphan_folders(
    vol: &Volume,
    claimed: &Claimed,
    stop: &dyn Fn() -> bool,
    note: &mut dyn FnMut(&str),
) -> Option<Vec<(u32, ClusterList)>> {
    let cs = vol.cluster_size as usize;
    let lost = |c: u32| vol.valid_cluster(c) && vol.is_allocated(c) == Some(true) && !claimed.has(c);
    let head = |c: u32| vol.src.read_at(vol.cluster_offset(c), 32 * 19).unwrap_or_default();
    // Clusters that begin with a valid record set. (A glance at the start of
    // every cluster nothing names: on a slow drive and a lot of such data,
    // that takes a while, so it says how far it has got.)
    let last = vol.cluster_count.saturating_add(1);
    let to_look = (2..=last).filter(|&c| lost(c)).count() as u64;
    let mut looked = 0u64;
    let mut told = std::time::Instant::now();
    let mut dirlike: Vec<u32> = Vec::new();
    for c in 2..=last {
        if !lost(c) {
            continue;
        }
        looked += 1;
        if looked % 4096 == 0 {
            if stop() {
                return None;
            }
            if told.elapsed().as_secs() >= 2 {
                note(&format!(
                    "Looking for folders whose own record is lost, in the data no folder record names: {}% done",
                    looked * 100 / to_look.max(1)
                ));
                told = std::time::Instant::now();
            }
        }
        if starts_valid_set(&head(c)) {
            dirlike.push(c);
        }
    }
    if dirlike.is_empty() {
        return Some(Vec::new());
    }
    let is_dirlike: std::collections::HashSet<u32> = dirlike.iter().copied().collect();
    // The folders each of those names as its sub-folders (in that cluster,
    // with what follows it on the disk to finish a record it begins).
    let mut named: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for &c in &dirlike {
        if let Ok(b) = vol.src.read_at(vol.cluster_offset(c), cs + 32 * 18) {
            for s in sets_in(&b) {
                if s.slot < cs / 32 && names_folder(vol, &s) {
                    named.insert(s.first);
                }
            }
        }
    }
    // Do the records carry on from cluster k - 1 into cluster k?
    let tail_slots = (cs / 32).min(18);
    let continues = |k: u32| -> bool {
        let next = head(k);
        if next.len() < 32 * 19 || next[0] == 0x00 {
            return false;
        }
        let Ok(tail) = vol.src.read_at(vol.cluster_offset(k - 1) + (cs - 32 * tail_slots) as u64, 32 * tail_slots)
        else {
            return false;
        };
        if tail.len() < 32 * tail_slots || tail[32 * (tail_slots - 1)] == 0x00 {
            // The folder's records ended in the cluster before.
            return false;
        }
        // A record set begun in the cluster before and finished in this one.
        for i in (0..tail_slots).rev() {
            let t = tail[32 * i];
            let sc = tail[32 * i + 1] as usize;
            if (t == 0x85 || t == 0x05) && (2..=18).contains(&sc) && i + sc >= tail_slots {
                let mut joined = tail[32 * i..].to_vec();
                joined.extend_from_slice(&next);
                if starts_valid_set(&joined) {
                    return true;
                }
            }
        }
        // Or records of its own, beginning no other folder.
        is_dirlike.contains(&k) && !named.contains(&k)
    };
    let limit = ((256u64 << 20) / vol.cluster_size + 1) as usize;
    let mut consumed: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut groups: Vec<(u32, ClusterList)> = Vec::new();
    for &c in &dirlike {
        if consumed.contains(&c) {
            continue;
        }
        let mut list = ClusterList::default();
        let f = vol.fat_entry(c).unwrap_or(0);
        // A FAT chain of two or more clusters, all of them lost, ending properly.
        let mut chain = vec![c];
        let mut chained = false;
        if f != EOC && vol.valid_cluster(f) {
            let mut cur = c;
            loop {
                let Some(n) = vol.fat_entry(cur) else { break };
                if n == EOC {
                    chained = chain.len() >= 2;
                    break;
                }
                if !vol.valid_cluster(n) || !lost(n) || consumed.contains(&n) || chain.contains(&n) || chain.len() >= limit {
                    break;
                }
                chain.push(n);
                cur = n;
            }
        }
        if chained {
            for &x in &chain {
                list.push(x);
                consumed.insert(x);
            }
        } else {
            list.push(c);
            consumed.insert(c);
            if f != EOC {
                let mut k = c + 1;
                while vol.valid_cluster(k)
                    && lost(k)
                    && !consumed.contains(&k)
                    && list.total() < limit as u64
                    && continues(k)
                {
                    list.push(k);
                    consumed.insert(k);
                    k += 1;
                }
            }
        }
        groups.push((c, list));
    }
    // Keep the folders whose records point at clusters marked in use that
    // nothing else claims, and note the sub-folders each names.
    let mut kept: Vec<(u32, ClusterList, Vec<u32>)> = Vec::new();
    for (first, list) in groups {
        let Ok(buf) = vol.read_clusters(&list) else { continue };
        let (mut good, mut bad) = (0u32, 0u32);
        let mut names = Vec::new();
        for s in sets_in(&buf) {
            if names_folder(vol, &s) {
                names.push(s.first);
            }
            if !s.alloc || s.length == 0 {
                continue;
            }
            if lost(s.first) {
                good += 1;
            } else {
                bad += 1;
            }
        }
        if good >= 1 && good >= bad {
            kept.push((first, list, names));
        }
    }
    let mut named_by: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, (_, _, names)) in kept.iter().enumerate() {
        for &n in names {
            named_by.entry(n).or_default().push(i);
        }
    }
    let mut out = Vec::new();
    for (i, (first, list, _)) in kept.into_iter().enumerate() {
        // Another of these folders names it: walked from there.
        if named_by.get(&first).is_some_and(|by| by.iter().any(|&j| j != i)) {
            continue;
        }
        // Its clusters end where a folder one of these names begins.
        let mut cut = ClusterList::default();
        'runs: for &(f, n) in &list.runs {
            for c in f..f + n {
                if !cut.runs.is_empty() && named_by.contains_key(&c) {
                    break 'runs;
                }
                cut.push(c);
            }
        }
        out.push((first, cut));
    }
    Some(out)
}

fn walk_stack(
    vol: &Volume,
    starts: Vec<Start>,
    visit: &mut dyn FnMut(&Entry, &ClusterList),
    stop: &dyn Fn() -> bool,
    mut issues: &mut Vec<FolderIssue>,
    visited: &mut std::collections::HashSet<u32>,
    claimed: &mut Claimed,
) {
    let mut stack: Vec<Start> = starts;
    while let Some((dir, clusters, dir_hidden, dir_maybe, dir_why, dir_by)) = stack.pop() {
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
        // Where the blank record that ends the folder for Windows is.
        let mut blank_at: Vec<(u64, u64)> = Vec::new();
        let slot_at = |i: usize| vol.disk_offset_in(&clusters, (i * 32) as u64).map(|d| vec![(d, d + 32)]).unwrap_or_default();
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
                    blank_at = slot_at(i);
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
            let issue = |what: &str, issues: &mut Vec<FolderIssue>| {
                issues.push(FolderIssue { folder: shown(&dir), disk_offset: rec_off(i), what: what.to_string() });
            };
            let sc = match classify(&buf, i) {
                Rec::Sound(sc) => sc,
                Rec::CutOff => {
                    issue("a file record cut off at the end of the folder - skipped", &mut issues);
                    i += 1;
                    continue;
                }
                Rec::BlankInside => {
                    // The rest of the record is blank: to Windows that is the end of the folder.
                    if reading {
                        stop = Stop::Blank(i);
                        let sc = buf[o + 1] as usize;
                        blank_at = (1..=sc).find(|&k| buf[o + 32 * k] == 0x00).map(|k| slot_at(i + k)).unwrap_or_default();
                    }
                    issue(
                        "a damaged file record (parts missing) - the file it described cannot be identified; \
                         Windows stops reading the folder there",
                        &mut issues,
                    );
                    i += 1;
                    continue;
                }
                Rec::BadCount(sc) => {
                    // Windows skips it together with every slot it claims, however
                    // many (measured for 0, 1, 19 and 200 parts).
                    issue(
                        "a damaged file record (impossible number of parts) - Windows skips it, and the records it \
                         claims",
                        &mut issues,
                    );
                    i += sc + 1;
                    continue;
                }
                Rec::BadParts(sc) => {
                    // Windows skips the record and every slot it claims.
                    issue(
                        "a damaged file record (its parts are of the wrong kinds) - the file it described cannot be \
                         identified; Windows skips it",
                        &mut issues,
                    );
                    i += sc + 1;
                    continue;
                }
                Rec::BadChecksum => {
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
            };
            let set = &buf[o..o + 32 * (sc + 1)];
            let stream = &set[32..64];
            let name_entries = (stream[3] as usize).div_ceil(15);
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
                hidden_by_blank: Vec::new(),
            };
            let clist = if stream[1] & 1 == 0 {
                ClusterList::default()
            } else if entry.is_dir && entry.data_length > MAX_FOLDER {
                // A folder holds at most 256 MB of records (the exFAT rules):
                // a record that says more is damaged, and all it claims, read
                // into memory, could be more than the PC has.
                let mut l = vol.clusters_of(entry.first_cluster, entry.no_fat_chain, MAX_FOLDER);
                l.problem = Some(format!(
                    "its record says the folder holds {} bytes of records, more than a folder can (256 MB): only the \
                     first 256 MB are read",
                    entry.data_length
                ));
                l
            } else {
                vol.clusters_of(entry.first_cluster, entry.no_fat_chain, entry.data_length)
            };
            found.push((i, entry, clist));
            i += sc + 1;
        }
        // What Windows makes of each file, now the whole folder has been read.
        let mut subdirs: Vec<Start> = Vec::new();
        for (slot, mut entry, clist) in found {
            let (hidden, maybe, why, by) = if dir_hidden && !dir_maybe {
                (true, false, dir_why.clone(), dir_by.clone())
            } else {
                match stop {
                    Stop::Damaged(d) if slot > d => (true, false, WHY_AFTER_DAMAGE.to_string(), Vec::new()),
                    Stop::Blank(b) if slot > b => (true, false, WHY_AFTER_BLANK.to_string(), blank_at.clone()),
                    Stop::Damaged(_) => (true, true, WHY_BEFORE_DAMAGE.to_string(), Vec::new()),
                    _ if dir_hidden => (true, true, dir_why.clone(), dir_by.clone()),
                    _ => (false, false, String::new(), Vec::new()),
                }
            };
            entry.hidden_from_windows = hidden;
            entry.hidden_maybe = maybe;
            entry.why_hidden = why;
            entry.hidden_by_blank = by;
            claimed.mark(&clist);
            visit(&entry, &clist);
            if entry.is_dir {
                // Name the outermost folder Windows cannot (or may not) list.
                let (sub_hidden, sub_maybe, sub_why, sub_by) = if dir_hidden && (!dir_maybe || entry.hidden_maybe) {
                    (true, dir_maybe, dir_why.clone(), dir_by.clone())
                } else if entry.hidden_from_windows {
                    let how = if entry.hidden_maybe {
                        "cannot list (typing the full path still opens it)"
                    } else {
                        "cannot list"
                    };
                    (
                        true,
                        entry.hidden_maybe,
                        format!("inside the folder {}, which Windows {how}", shown(&entry.path)),
                        entry.hidden_by_blank.clone(),
                    )
                } else {
                    (false, false, String::new(), Vec::new())
                };
                subdirs.push((entry.path.clone(), clist, sub_hidden, sub_maybe, sub_why, sub_by));
            }
        }
        // Depth-first, in the order the folder lists them.
        for s in subdirs.into_iter().rev() {
            stack.push(s);
        }
    }
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

    /// Damage the self-test disk's volume and count what is found.
    fn found_on(patch: impl Fn(&mut Vec<u8>, usize)) -> (u64, u64, bool, Vec<String>) {
        let mut d = crate::selftest::disk_image().unwrap();
        let part = 2095 * 512;
        patch(&mut d, part);
        let src = crate::disk::Memory::new(d);
        let table = crate::gpt::read(&src, &[]);
        let (vol, backup) = crate::mapcheck::find_volume(&src, &table).unwrap();
        let (mut folders, mut files) = (0u64, 0u64);
        let issues = walk_until(
            &vol,
            &mut |e: &Entry, _| {
                if e.is_dir {
                    folders += 1
                } else {
                    files += 1
                }
            },
            &|| false,
            &mut |_| {},
        );
        (folders, files, backup, issues.iter().map(|i| i.what.clone()).collect())
    }

    #[test]
    fn a_loop_in_the_fat_is_read_once_and_said() {
        let (folders, files, _, before) = found_on(|_, _| {});
        assert!(!before.iter().any(|i| i.contains("loops")), "{before:?}");
        // The root folder's cluster (4) named as its own next: before, its
        // records were read over and over - every file and folder listed
        // thousands of times, each copied again as "name (2)", "name (3)"...
        let fat = |d: &mut Vec<u8>, part: usize, c: usize, v: u32| {
            let o = part + 128 * 512 + 4 * c;
            d[o..o + 4].copy_from_slice(&v.to_le_bytes());
        };
        let (f2, n2, _, issues) = found_on(|d, part| fat(d, part, 4, 4));
        assert_eq!((f2, n2), (folders, files));
        assert!(issues.iter().any(|i| i.contains("loops back on itself")), "{issues:?}");
    }

    #[test]
    fn a_boot_sector_whose_sizes_do_not_fit_is_passed_over_for_its_backup() {
        let (folders, files, backup, _) = found_on(|_, _| {});
        assert!(!backup);
        // The cluster count made 4 billion: believed, the program went over
        // billions of clusters that are not there (it seemed to hang).
        for (at, v) in [(92usize, u32::MAX), (92, 0x7FFF_FFFF), (84, 1), (80, 3)] {
            let (f2, n2, backup, _) = found_on(|d, part| d[part + at..part + at + 4].copy_from_slice(&v.to_le_bytes()));
            assert!(backup, "byte {at} = {v:#x}: the backup boot sector is used");
            assert_eq!((f2, n2), (folders, files), "byte {at} = {v:#x}");
        }
    }

    #[test]
    fn a_folder_record_claiming_more_than_a_folder_can_hold_is_read_only_so_far() {
        // A folder's record in the root (cluster 4) says it holds 1 GB of
        // records, with its checksum made right: read whole, a bigger claim
        // on a big volume could take more memory than the PC has.
        let (_, _, _, issues) = found_on(|d, part| {
            let root = part + 152 * 512 + (4 - 2) * 4096;
            let mut i = 0;
            while i < 4096 / 32 {
                let o = root + 32 * i;
                let sc = d[o + 1] as usize;
                if d[o] == 0x85 && u16le(&d[o..o + 32], 4) & 0x10 != 0 && d[o + 32] == 0xC0 {
                    let big = (1u64 << 30).to_le_bytes();
                    d[o + 32 + 24..o + 32 + 32].copy_from_slice(&big);
                    d[o + 32 + 8..o + 32 + 16].copy_from_slice(&big);
                    let c = entry_set_checksum(&d[o..o + 32 * (sc + 1)]);
                    d[o + 2..o + 4].copy_from_slice(&c.to_le_bytes());
                    break;
                }
                i += 1;
            }
            assert!(i < 4096 / 32, "a folder record in the root");
        });
        assert!(issues.iter().any(|i| i.contains("more than a folder can")), "{issues:?}");
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
