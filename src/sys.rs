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
