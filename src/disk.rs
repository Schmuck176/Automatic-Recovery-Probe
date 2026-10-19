//! Read-only access to a whole disk: a disk image file, or on Windows the
//! physical drive itself (\\.\PhysicalDriveN, which needs administrator
//! rights). Nothing here can write: files and drives are opened read-only.

use std::fs::File;
use std::io;
use std::path::Path;

/// Something that can be read like a disk.
pub trait Source: Send + Sync {
    /// Size in bytes (0 if unknown).
    fn size(&self) -> u64;
    /// Read up to `len` bytes at `off`; fewer only at the end of the disk.
    fn read_at(&self, off: u64, len: usize) -> io::Result<Vec<u8>>;
    /// A human description ("Disk 2, 2.0 TB, Realtek RTL9210").
    fn describe(&self) -> String;
}

/// Reads through to a disk and counts the reads that fail. The recovered
/// SSD is a healthy drive holding a copy, so a read that fails there almost
/// always means it stopped answering (a loose cable, a USB hiccup), and work
/// that goes on regardless would quietly leave things out.
pub struct Watched<'a> {
    inner: &'a dyn Source,
    errors: std::sync::atomic::AtomicU64,
    last: std::sync::Mutex<String>,
}

impl<'a> Watched<'a> {
    pub fn new(inner: &'a dyn Source) -> Watched<'a> {
        Watched { inner, errors: std::sync::atomic::AtomicU64::new(0), last: std::sync::Mutex::new(String::new()) }
    }
    /// How many reads have failed so far.
    pub fn errors(&self) -> u64 {
        self.errors.load(std::sync::atomic::Ordering::Relaxed)
    }
    /// What the last failed read said.
    pub fn last_error(&self) -> String {
        self.last.lock().map(|s| s.clone()).unwrap_or_default()
    }
    /// Does the disk still answer? (Reads 512 bytes at `off` again.)
    pub fn answers(&self, off: u64) -> bool {
        self.inner.read_at(off, 512).is_ok_and(|b| !b.is_empty())
    }
}

impl Source for Watched<'_> {
    fn size(&self) -> u64 {
        self.inner.size()
    }
    fn read_at(&self, off: u64, len: usize) -> io::Result<Vec<u8>> {
        let r = self.inner.read_at(off, len);
        if let Err(e) = &r {
            self.errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Ok(mut l) = self.last.lock() {
                *l = e.to_string();
            }
        }
        r
    }
    fn describe(&self) -> String {
        self.inner.describe()
    }
}

/// A disk held in memory (for the tests).
pub struct Memory {
    data: Vec<u8>,
}

impl Memory {
    pub fn new(data: Vec<u8>) -> Memory {
        Memory { data }
    }
}

impl Source for Memory {
    fn size(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, off: u64, len: usize) -> io::Result<Vec<u8>> {
        let s = (off as usize).min(self.data.len());
        let e = s.saturating_add(len).min(self.data.len());
        Ok(self.data[s..e].to_vec())
    }
    fn describe(&self) -> String {
        "a test disk in memory".to_string()
    }
}

/// A disk for the tests that stops answering after a number of reads, the
/// way a USB disk does when its cable comes out.
pub struct Unplugged<'a> {
    pub inner: &'a dyn Source,
    pub reads_left: std::sync::atomic::AtomicI64,
}

impl Source for Unplugged<'_> {
    fn size(&self) -> u64 {
        self.inner.size()
    }
    fn read_at(&self, off: u64, len: usize) -> io::Result<Vec<u8>> {
        if self.reads_left.fetch_sub(1, std::sync::atomic::Ordering::Relaxed) <= 0 {
            return Err(io::Error::other("the device is not connected"));
        }
        self.inner.read_at(off, len)
    }
    fn describe(&self) -> String {
        self.inner.describe()
    }
}

/// Reads are done in whole 4 KiB pages at 4 KiB-aligned offsets into a
/// 4 KiB-aligned buffer, which every disk driver accepts.
const ALIGN: u64 = 4096;

struct AlignedBuf {
    ptr: *mut u8,
    layout: std::alloc::Layout,
}

impl AlignedBuf {
    fn new(len: usize) -> AlignedBuf {
        let layout = std::alloc::Layout::from_size_align(len.max(1), ALIGN as usize).expect("buffer layout");
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        AlignedBuf { ptr, layout }
    }
    fn as_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.layout.size()) }
    }
}

impl Drop for AlignedBuf {
    fn drop(&mut self) {
        unsafe { std::alloc::dealloc(self.ptr, self.layout) }
    }
}

#[cfg(unix)]
fn pread(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(f, buf, off)
}

#[cfg(windows)]
fn pread(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(f, buf, off)
}

/// Aligned positioned read of [off, off+len) clipped to `size` (if known).
fn read_aligned(f: &File, size: u64, off: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut end = off.saturating_add(len as u64);
    if size > 0 {
        end = end.min(size);
    }
    if end <= off {
        return Ok(Vec::new());
    }
    let a0 = off / ALIGN * ALIGN;
    let mut a1 = end.div_ceil(ALIGN) * ALIGN;
    if size > 0 && a1 > size {
        // The last partial page: devices have sizes in whole sectors, so
        // reading up to the size itself is still sector-aligned.
        a1 = size.div_ceil(512) * 512;
    }
    let total = (a1 - a0) as usize;
    let mut buf = AlignedBuf::new(total.div_ceil(ALIGN as usize) * ALIGN as usize);
    let dst = &mut buf.as_mut()[..total];
    let mut got = 0usize;
    while got < total {
        // Read in pieces of at most 8 MiB, each starting on a page boundary.
        let piece = (total - got).min(8 << 20);
        let n = pread(f, &mut dst[got..got + piece], a0 + got as u64)?;
        if n == 0 {
            break;
        }
        got += n;
    }
    let s = (off - a0) as usize;
    let e = ((end - a0) as usize).min(got);
    if e <= s {
        return Ok(Vec::new());
    }
    Ok(dst[s..e].to_vec())
}

/// A disk image file (used by the tests, and for a ddrescue image file).
pub struct ImageFile {
    f: File,
    size: u64,
    name: String,
}

impl ImageFile {
    pub fn open(path: &Path) -> io::Result<ImageFile> {
        let f = File::open(path)?;
        let size = f.metadata()?.len();
        Ok(ImageFile { f, size, name: path.display().to_string() })
    }
}

impl Source for ImageFile {
    fn size(&self) -> u64 {
        self.size
    }
    fn read_at(&self, off: u64, len: usize) -> io::Result<Vec<u8>> {
        // Image files have no alignment rules; read exactly.
        let end = off.saturating_add(len as u64).min(self.size);
        if end <= off {
            return Ok(Vec::new());
        }
        let mut buf = vec![0u8; (end - off) as usize];
        let mut got = 0;
        while got < buf.len() {
            let n = pread(&self.f, &mut buf[got..], off + got as u64)?;
            if n == 0 {
                break;
            }
            got += n;
        }
        buf.truncate(got);
        Ok(buf)
    }
    fn describe(&self) -> String {
        format!("disk image {}", self.name)
    }
}

/// A disk device opened as a file (/dev/sdb on Linux, /dev/rdisk4 on a
/// Mac): devices want whole, aligned sectors.
#[cfg(unix)]
pub struct DeviceFile {
    f: File,
    size: u64,
    name: String,
}

#[cfg(unix)]
impl Source for DeviceFile {
    fn size(&self) -> u64 {
        self.size
    }
    fn read_at(&self, off: u64, len: usize) -> io::Result<Vec<u8>> {
        read_aligned(&self.f, self.size, off, len)
    }
    fn describe(&self) -> String {
        format!("disk {} ({})", self.name, crate::fmt::size(self.size))
    }
}

/// Open a disk: an image file path, a disk device, or on Windows
/// "\\.\PhysicalDriveN" / "disk:N".
pub fn open_source(spec: &str) -> io::Result<Box<dyn Source>> {
    #[cfg(windows)]
    {
        let n = spec
            .strip_prefix(r"\\.\PhysicalDrive")
            .or_else(|| spec.strip_prefix(r"\\.\physicaldrive"))
            .or_else(|| spec.strip_prefix("disk:"));
        if let Some(n) = n.and_then(|n| n.parse::<u32>().ok()) {
            return Ok(Box::new(win::PhysicalDrive::open(n)?));
        }
    }
    #[cfg(unix)]
    {
        use std::io::{Seek, SeekFrom};
        use std::os::unix::fs::FileTypeExt;
        let ft = std::fs::metadata(spec)?.file_type();
        if ft.is_block_device() || ft.is_char_device() {
            let mut f = File::open(spec)?;
            let size = f.seek(SeekFrom::End(0)).unwrap_or(0);
            return Ok(Box::new(DeviceFile { f, size, name: spec.to_string() }));
        }
    }
    Ok(Box::new(ImageFile::open(Path::new(spec))?))
}

/// One disk the computer can see.
#[derive(Clone, Debug)]
pub struct DiskInfo {
    pub number: u32,
    pub spec: String,
    pub size: u64,
    pub model: String,
    pub bus: String,
    pub error: Option<String>,
}

/// Every physical disk (Windows). Needs administrator rights to open them;
/// without, each comes back with an error saying so.
pub fn list_disks() -> Vec<DiskInfo> {
    #[cfg(windows)]
    {
        win::list()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(windows)]
pub mod win {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    extern "system" {
        fn DeviceIoControl(
            h: *mut c_void,
            code: u32,
            inbuf: *const c_void,
            inlen: u32,
            outbuf: *mut c_void,
            outlen: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
    }

    const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_405C;
    const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;

    pub struct PhysicalDrive {
        f: File,
        size: u64,
        number: u32,
        model: String,
    }

    fn ioctl(f: &File, code: u32, input: &[u8], out_len: usize) -> Option<Vec<u8>> {
        let mut out = vec![0u8; out_len];
        let mut returned: u32 = 0;
        let ok = unsafe {
            DeviceIoControl(
                f.as_raw_handle() as *mut c_void,
                code,
                if input.is_empty() { std::ptr::null() } else { input.as_ptr() as *const c_void },
                input.len() as u32,
                out.as_mut_ptr() as *mut c_void,
                out_len as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return None;
        }
        out.truncate(returned as usize);
        Some(out)
    }

    fn length(f: &File) -> u64 {
        ioctl(f, IOCTL_DISK_GET_LENGTH_INFO, &[], 8)
            .filter(|b| b.len() >= 8)
            .map(|b| u64::from_le_bytes(b[..8].try_into().unwrap()))
            .unwrap_or(0)
    }

    fn cstr_at(b: &[u8], off: u32) -> String {
        let off = off as usize;
        if off == 0 || off >= b.len() {
            return String::new();
        }
        let end = b[off..].iter().position(|&c| c == 0).map(|e| off + e).unwrap_or(b.len());
        String::from_utf8_lossy(&b[off..end]).trim().to_string()
    }

    /// (model, bus, removable media) from IOCTL_STORAGE_QUERY_PROPERTY /
    /// StorageDeviceProperty.
    fn identity(f: &File) -> (String, String, bool) {
        // STORAGE_PROPERTY_QUERY { PropertyId = 0, QueryType = 0, AdditionalParameters[1] }
        let query = [0u8; 12];
        let b = match ioctl(f, IOCTL_STORAGE_QUERY_PROPERTY, &query, 1024) {
            Some(b) if b.len() >= 36 => b,
            _ => return (String::new(), String::new(), false),
        };
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        // STORAGE_DEVICE_DESCRIPTOR: Version, Size, DeviceType(1), DeviceTypeModifier(1),
        // RemovableMedia(1), CommandQueueing(1), VendorIdOffset@12, ProductIdOffset@16,
        // ProductRevisionOffset@20, SerialNumberOffset@24, BusType@28
        let vendor = cstr_at(&b, u32_at(12));
        let product = cstr_at(&b, u32_at(16));
        let bus = match u32_at(28) {
            7 => "USB",
            11 => "SATA",
            17 => "NVMe",
            1 => "SCSI",
            3 => "ATA",
            8 => "RAID",
            10 => "SAS",
            12 => "SD card",
            _ => "",
        };
        let model = format!("{vendor} {product}").trim().to_string();
        (model, bus.to_string(), b[10] != 0)
    }

    impl PhysicalDrive {
        pub fn open(n: u32) -> io::Result<PhysicalDrive> {
            let path = format!(r"\\.\PhysicalDrive{n}");
            let f = std::fs::OpenOptions::new().read(true).open(&path)?;
            let size = length(&f);
            let (model, _, _) = identity(&f);
            Ok(PhysicalDrive { f, size, number: n, model })
        }
    }

    impl Source for PhysicalDrive {
        fn size(&self) -> u64 {
            self.size
        }
        fn read_at(&self, off: u64, len: usize) -> io::Result<Vec<u8>> {
            read_aligned(&self.f, self.size, off, len)
        }
        fn describe(&self) -> String {
            format!("Disk {} ({}, {})", self.number, crate::fmt::size(self.size), self.model)
        }
    }

    pub fn list() -> Vec<DiskInfo> {
        let mut out = Vec::new();
        for n in 0..32u32 {
            let spec = format!(r"\\.\PhysicalDrive{n}");
            match std::fs::OpenOptions::new().read(true).open(&spec) {
                Ok(f) => {
                    let (model, bus, _) = identity(&f);
                    out.push(DiskInfo { number: n, spec, size: length(&f), model, bus, error: None });
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => out.push(DiskInfo {
                    number: n,
                    spec,
                    size: 0,
                    model: String::new(),
                    bus: String::new(),
                    error: Some(if e.kind() == io::ErrorKind::PermissionDenied {
                        "needs administrator rights".to_string()
                    } else {
                        e.to_string()
                    }),
                }),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligned_reads_return_exact_bytes() {
        let dir = std::env::temp_dir().join(format!("rp-disk-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("img.bin");
        let data: Vec<u8> = (0..20000u32).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        let f = File::open(&p).unwrap();
        for (off, len) in [(0u64, 10usize), (4095, 3), (4096, 4096), (19990, 100), (12345, 5000), (20000, 5)] {
            let got = read_aligned(&f, data.len() as u64, off, len).unwrap();
            let s = (off as usize).min(data.len());
            let e = (off as usize + len).min(data.len());
            assert_eq!(got, &data[s..e], "off {off} len {len}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
