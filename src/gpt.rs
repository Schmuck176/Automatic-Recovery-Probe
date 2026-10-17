//! Partition tables: GPT (with an MBR fallback), enough to find the exFAT
//! volume on the recovered disk and to recognise the disk as the copy that
//! a ddrescue mapfile describes.

use crate::disk::Source;

#[derive(Clone, Debug)]
pub struct Partition {
    pub index: u32,
    pub type_guid: String,
    pub start: u64, // bytes
    pub end: u64,   // bytes, exclusive
    pub name: String,
}

impl Partition {
    pub fn kind(&self) -> &'static str {
        match self.type_guid.as_str() {
            "C12A7328-F81F-11D2-BA4B-00A0C93EC93B" => "EFI system partition",
            "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7" => "basic data (exFAT, FAT or NTFS)",
            "E3C9E316-0B5C-4DB8-817D-F92DF00215AE" => "Microsoft reserved",
            "48465300-0000-11AA-AA11-00306543ECAC" => "Apple HFS+",
            "7C3457EF-0000-11AA-AA11-00306543ECAC" => "Apple APFS",
            "MBR-07" => "MBR exFAT/NTFS",
            "MBR-0B" | "MBR-0C" => "MBR FAT32",
            "MBR-EE" => "MBR protective",
            _ => "other",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Table {
    pub scheme: &'static str, // "GPT", "MBR" or "none"
    pub sector: u64,
    /// GPT only: the sector holding the backup header, i.e. the last
    /// sector of the disk the table was written for. On a ddrescue copy
    /// onto a bigger disk it still points at the OLD disk's last sector,
    /// which is how the copy is recognised.
    pub last_lba: u64,
    /// The main header could not be read and the backup was used.
    pub from_backup: bool,
    pub partitions: Vec<Partition>,
}

fn guid_string(b: &[u8]) -> String {
    format!(
        "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        u32::from_le_bytes(b[0..4].try_into().unwrap()),
        u16::from_le_bytes(b[4..6].try_into().unwrap()),
        u16::from_le_bytes(b[6..8].try_into().unwrap()),
        b[8],
        b[9],
        b[10],
        b[11],
        b[12],
        b[13],
        b[14],
        b[15]
    )
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// The standard CRC-32 (as zlib), used by GPT headers.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c: u32 = 0xFFFF_FFFF;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

/// A GPT header at `lba`, if one is there and its checksum is right.
fn header_at(src: &dyn Source, lba: u64, sector: u64) -> Option<Vec<u8>> {
    let h = src.read_at(lba.checked_mul(sector)?, 92).ok()?;
    if h.len() < 92 || &h[0..8] != b"EFI PART" || u32le(&h, 12) < 92 {
        return None;
    }
    let mut copy = h.clone();
    copy[16..20].fill(0);
    if crc32(&copy) != u32le(&h, 16) || u64le(&h, 24) != lba {
        return None;
    }
    Some(h)
}

fn entries(src: &dyn Source, hdr: &[u8], sector: u64) -> Vec<Partition> {
    let entries_lba = u64le(hdr, 72);
    let count = u32le(hdr, 80).min(1024) as u64;
    let esize = u32le(hdr, 84).max(128) as u64;
    let mut parts = Vec::new();
    if let Ok(raw) = src.read_at(entries_lba.saturating_mul(sector), (count * esize) as usize) {
        for i in 0..count {
            let o = (i * esize) as usize;
            if o + 128 > raw.len() {
                break;
            }
            let e = &raw[o..o + 128];
            if e[0..16].iter().all(|&c| c == 0) {
                continue;
            }
            let first = u64le(e, 32);
            let last = u64le(e, 40);
            let name_u16: Vec<u16> =
                e[56..128].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
            parts.push(Partition {
                index: i as u32 + 1,
                type_guid: guid_string(&e[0..16]),
                start: first.saturating_mul(sector),
                end: last.saturating_add(1).saturating_mul(sector),
                name: String::from_utf16_lossy(&name_u16),
            });
        }
    }
    parts
}

/// Read the partition table. `old_disk_sizes` are disk sizes where a
/// backup GPT header may sit (the end of this disk is always tried; a
/// mapfile gives the size of the disk that was copied).
pub fn read(src: &dyn Source, old_disk_sizes: &[u64]) -> Table {
    for sector in [512u64, 4096] {
        if let Some(h) = header_at(src, 1, sector) {
            let parts = entries(src, &h, sector);
            return Table { scheme: "GPT", sector, last_lba: u64le(&h, 32), from_backup: false, partitions: parts };
        }
    }
    // The main header is unreadable: the backup sits in the disk's last sector.
    let mut sizes: Vec<u64> = old_disk_sizes.to_vec();
    if src.size() > 0 {
        sizes.push(src.size());
    }
    for &size in &sizes {
        for sector in [512u64, 4096] {
            if size < 2 * sector {
                continue;
            }
            let lba = size / sector - 1;
            if let Some(h) = header_at(src, lba, sector) {
                let parts = entries(src, &h, sector);
                return Table { scheme: "GPT", sector, last_lba: lba, from_backup: true, partitions: parts };
            }
        }
    }
    // MBR
    if let Ok(mbr) = src.read_at(0, 512) {
        if mbr.len() == 512 && mbr[510] == 0x55 && mbr[511] == 0xAA {
            let mut parts = Vec::new();
            for i in 0..4 {
                let e = &mbr[446 + i * 16..446 + i * 16 + 16];
                let typ = e[4];
                let start = u32le(e, 8) as u64;
                let len = u32le(e, 12) as u64;
                if typ == 0 || len == 0 {
                    continue;
                }
                parts.push(Partition {
                    index: i as u32 + 1,
                    type_guid: format!("MBR-{typ:02X}"),
                    start: start * 512,
                    end: (start + len) * 512,
                    name: String::new(),
                });
            }
            if !parts.is_empty() && !(parts.len() == 1 && parts[0].type_guid == "MBR-EE") {
                return Table { scheme: "MBR", sector: 512, last_lba: 0, from_backup: false, partitions: parts };
            }
        }
    }
    Table { scheme: "none", sector: 512, last_lba: 0, from_backup: false, partitions: Vec::new() }
}

impl Table {
    /// The disk size the partition table was written for (GPT only).
    pub fn original_disk_size(&self) -> Option<u64> {
        if self.scheme == "GPT" && self.last_lba > 0 {
            Some((self.last_lba + 1) * self.sector)
        } else {
            None
        }
    }

    /// Where the partition table itself is stored.
    pub fn own_areas(&self) -> Vec<(u64, u64)> {
        match self.scheme {
            "GPT" => {
                let mut v = vec![(0, 34 * self.sector)];
                if self.last_lba > 33 {
                    v.push(((self.last_lba - 32) * self.sector, (self.last_lba + 1) * self.sector));
                }
                v
            }
            "MBR" => vec![(0, 512)],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn crc32_matches_zlib() {
        assert_eq!(super::crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(super::crc32(b""), 0);
    }
}
