//! The few operating-system calls Rust's standard library does not offer,
//! each declared by hand so the program needs no third-party code.

use std::ffi::c_void;

// ---------------------------------------------------------------------------
// Local time. Python's datetime.fromtimestamp() uses the C runtime's
// localtime, so the same function is called here, and the two versions print
// the same dates.
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod time_imp {
    #[repr(C)]
    struct Tm {
        tm_sec: i32,
        tm_min: i32,
        tm_hour: i32,
        tm_mday: i32,
        tm_mon: i32,
        tm_year: i32,
        tm_wday: i32,
        tm_yday: i32,
        tm_isdst: i32,
        tm_gmtoff: i64,
        tm_zone: *const u8,
    }

    extern "C" {
        fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm;
        fn tzset();
    }

    pub fn local(secs: i64) -> Option<[i32; 6]> {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| unsafe { tzset() });
        unsafe {
            let mut tm: Tm = std::mem::zeroed();
            if localtime_r(&secs, &mut tm).is_null() {
                return None;
            }
            Some([tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec])
        }
    }
}

#[cfg(windows)]
mod time_imp {
    #[repr(C)]
    struct Tm {
        tm_sec: i32,
        tm_min: i32,
        tm_hour: i32,
        tm_mday: i32,
        tm_mon: i32,
        tm_year: i32,
        tm_wday: i32,
        tm_yday: i32,
        tm_isdst: i32,
    }

    extern "C" {
        fn _localtime64_s(out: *mut Tm, t: *const i64) -> i32;
    }

    pub fn local(secs: i64) -> Option<[i32; 6]> {
        unsafe {
            let mut tm: Tm = std::mem::zeroed();
            if _localtime64_s(&mut tm, &secs) != 0 {
                return None;
            }
            Some([tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec])
        }
    }
}

/// [year, month, day, hour, minute, second] in the PC's local time zone.
pub fn local_time(secs: i64) -> Option<[i32; 6]> {
    time_imp::local(secs)
}

// ---------------------------------------------------------------------------
// Loading a system library at run time. Used for SQLite: Windows 10 and 11
// carry their own copy (winsqlite3.dll), so nothing has to be bundled.
// ---------------------------------------------------------------------------

pub struct Library {
    handle: *mut c_void,
}

// The handle is only ever used to look up function addresses.
unsafe impl Send for Library {}
unsafe impl Sync for Library {}

#[cfg(unix)]
mod dl_imp {
    use std::ffi::c_void;
    #[link(name = "dl")]
    extern "C" {
        pub fn dlopen(name: *const u8, flags: i32) -> *mut c_void;
        pub fn dlsym(handle: *mut c_void, name: *const u8) -> *mut c_void;
    }
    pub const RTLD_NOW: i32 = 2;
}

#[cfg(windows)]
mod dl_imp {
    use std::ffi::c_void;
    extern "system" {
        pub fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        pub fn GetProcAddress(handle: *mut c_void, name: *const u8) -> *mut c_void;
    }
    /// Only ever load from System32, never from the folder the program is in.
    pub const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;
}

impl Library {
    #[cfg(unix)]
    pub fn open(names: &[&str]) -> Option<Library> {
        for n in names {
            let c = format!("{n}\0");
            let h = unsafe { dl_imp::dlopen(c.as_ptr(), dl_imp::RTLD_NOW) };
            if !h.is_null() {
                return Some(Library { handle: h });
            }
        }
        None
    }

    #[cfg(windows)]
    pub fn open(names: &[&str]) -> Option<Library> {
        for n in names {
            let w: Vec<u16> = n.encode_utf16().chain(std::iter::once(0)).collect();
            let h = unsafe {
                dl_imp::LoadLibraryExW(w.as_ptr(), std::ptr::null_mut(), dl_imp::LOAD_LIBRARY_SEARCH_SYSTEM32)
            };
            if !h.is_null() {
                return Some(Library { handle: h });
            }
        }
        None
    }

    /// The address of an exported function, or null.
    pub fn symbol(&self, name: &str) -> *mut c_void {
        let c = format!("{name}\0");
        #[cfg(unix)]
        unsafe {
            dl_imp::dlsym(self.handle, c.as_ptr())
        }
        #[cfg(windows)]
        unsafe {
            dl_imp::GetProcAddress(self.handle, c.as_ptr())
        }
    }
}

// ---------------------------------------------------------------------------
// Which drive letter a volume has, found by its serial number (the number
// Windows shows with `vol D:`). Used to check whether Windows can see a file.
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub fn drive_for_serial(serial: u32) -> Option<String> {
    extern "system" {
        fn GetLogicalDrives() -> u32;
        fn GetVolumeInformationW(
            root: *const u16,
            name: *mut u16,
            name_len: u32,
            serial: *mut u32,
            max_component: *mut u32,
            flags: *mut u32,
            fs_name: *mut u16,
            fs_name_len: u32,
        ) -> i32;
        fn SetThreadErrorMode(mode: u32, old: *mut u32) -> i32;
    }
    // No "insert a disk" pop-ups for empty card readers while asking.
    let mut old = 0u32;
    unsafe { SetThreadErrorMode(0x0001, &mut old) };
    let mask = unsafe { GetLogicalDrives() };
    let mut found = None;
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain(std::iter::once(0)).collect();
        let mut s = 0u32;
        let mut fs = [0u16; 32];
        let ok = unsafe {
            GetVolumeInformationW(
                root.as_ptr(),
                std::ptr::null_mut(),
                0,
                &mut s,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        };
        let fs_name = String::from_utf16_lossy(&fs[..fs.iter().position(|&c| c == 0).unwrap_or(fs.len())]);
        if ok != 0 && s == serial && fs_name.eq_ignore_ascii_case("exFAT") {
            found = Some(format!("{letter}:"));
            break;
        }
    }
    unsafe { SetThreadErrorMode(old, std::ptr::null_mut()) };
    found
}

#[cfg(not(windows))]
pub fn drive_for_serial(_serial: u32) -> Option<String> {
    None
}

/// Is the volume at this drive letter ("R:") read-only? None: cannot tell.
#[cfg(windows)]
pub fn volume_read_only(letter: &str) -> Option<bool> {
    extern "system" {
        fn GetVolumeInformationW(
            root: *const u16,
            name: *mut u16,
            name_len: u32,
            serial: *mut u32,
            max_component: *mut u32,
            flags: *mut u32,
            fs_name: *mut u16,
            fs_name_len: u32,
        ) -> i32;
    }
    const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;
    let l = letter.trim_end_matches(['\\', '/']);
    if l.len() != 2 || !l.ends_with(':') {
        return None;
    }
    let root: Vec<u16> = format!("{l}\\").encode_utf16().chain(std::iter::once(0)).collect();
    let mut flags = 0u32;
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut flags,
            std::ptr::null_mut(),
            0,
        )
    };
    (ok != 0).then_some(flags & FILE_READ_ONLY_VOLUME != 0)
}

#[cfg(not(windows))]
pub fn volume_read_only(_letter: &str) -> Option<bool> {
    None
}

// ---------------------------------------------------------------------------
// How busy the processor is, and how much memory is free: the extra checks
// run more tools at once while the processor has room (extra.rs).
// ---------------------------------------------------------------------------

/// The processor's time since the PC started, all its threads together, in
/// its own units: (busy, all). Two readings apart give how busy it was in
/// between.
#[cfg(windows)]
pub fn cpu_times() -> Option<(u64, u64)> {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    extern "system" {
        fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
    }
    let (mut idle, mut kernel, mut user) = (FileTime::default(), FileTime::default(), FileTime::default());
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return None;
    }
    let v = |t: &FileTime| ((t.high as u64) << 32) | t.low as u64;
    // (The kernel's time includes the idle time.)
    let all = v(&kernel).saturating_add(v(&user));
    Some((all.saturating_sub(v(&idle)), all))
}

#[cfg(not(windows))]
pub fn cpu_times() -> Option<(u64, u64)> {
    let text = std::fs::read_to_string("/proc/stat").ok()?;
    cpu_times_in(text.lines().next()?)
}

/// "cpu  user nice system idle iowait irq softirq steal ..." -> (busy, all).
#[cfg(not(windows))]
fn cpu_times_in(line: &str) -> Option<(u64, u64)> {
    let mut f = line.split_whitespace();
    if f.next()? != "cpu" {
        return None;
    }
    let n: Vec<u64> = f.take(8).map(|x| x.parse().unwrap_or(0)).collect();
    if n.len() < 4 {
        return None;
    }
    let all: u64 = n.iter().sum();
    let idle = n[3] + n.get(4).copied().unwrap_or(0);
    Some((all.saturating_sub(idle), all))
}

/// How busy the processor was between two readings of `cpu_times`, from 0 to 1.
pub fn busy_between(a: (u64, u64), b: (u64, u64)) -> Option<f64> {
    let all = b.1.checked_sub(a.1)?;
    let busy = b.0.checked_sub(a.0)?;
    (all > 0).then(|| (busy as f64 / all as f64).clamp(0.0, 1.0))
}

/// Memory free for programs to use now, in bytes.
#[cfg(windows)]
pub fn memory_free() -> Option<u64> {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        _memory_load: u32,
        _total_phys: u64,
        avail_phys: u64,
        _total_page_file: u64,
        _avail_page_file: u64,
        _total_virtual: u64,
        _avail_virtual: u64,
        _avail_extended_virtual: u64,
    }
    extern "system" {
        fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
    }
    let mut s: MemoryStatusEx = unsafe { std::mem::zeroed() };
    s.length = std::mem::size_of::<MemoryStatusEx>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut s) } == 0 {
        return None;
    }
    Some(s.avail_phys)
}

#[cfg(not(windows))]
pub fn memory_free() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = text.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

// ---------------------------------------------------------------------------
// Files that must survive a power cut whole.
// ---------------------------------------------------------------------------

/// Write `path` so that a crash or power cut at any moment leaves either
/// the old file or the new one, whole: the text goes into a file beside it,
/// is made safe on the drive, and only then takes the old one's place.
/// (Written straight over, a file can come back after a power cut as the
/// right length but full of zeros: Windows writes the data later than it
/// notes the file's size.)
pub fn write_safely(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".new");
    let tmp = path.with_file_name(name);
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    #[cfg(not(windows))]
    #[test]
    fn how_busy_the_processor_is_is_read() {
        let a = super::cpu_times_in("cpu  100 0 50 800 50 0 0 0 0 0").unwrap();
        assert_eq!(a, (150, 1000));
        let b = super::cpu_times_in("cpu  400 0 150 1100 150 0 0 0 0 0").unwrap();
        // 400 more busy of 800 more in all.
        assert_eq!(super::busy_between(a, b), Some(0.5));
        assert_eq!(super::busy_between(b, a), None, "readings the wrong way round say nothing");
        assert_eq!(super::busy_between(a, a), None);
        assert!(super::cpu_times_in("intr 1 2 3").is_none());
        // This PC's own, and its memory.
        let (busy, all) = super::cpu_times().expect("/proc/stat");
        assert!(busy <= all && all > 0);
        assert!(super::memory_free().is_some_and(|m| m > 0));
    }

    #[test]
    fn a_file_written_safely_replaces_the_old_one_whole() {
        let dir = std::env::temp_dir().join(format!("rp-sys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("state.txt");
        std::fs::write(&p, "old").unwrap();
        super::write_safely(&p, b"new and longer").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"new and longer");
        assert!(!dir.join("state.txt.new").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
