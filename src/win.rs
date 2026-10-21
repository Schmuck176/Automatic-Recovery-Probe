//! What the double-click front end needs from Windows: whether this copy
//! runs with administrator rights, starting a copy of itself that has them
//! (Windows shows its usual "allow this app" question), the drives and disks
//! this PC can see without administrator rights, and opening the browser or
//! a folder. Each call is declared by hand, so no third-party code is used.
//!
//! On other systems the same functions exist and do the nearest sensible
//! thing, so the front end can be tested anywhere.

use std::path::{Path, PathBuf};

/// One drive letter (or, elsewhere, one mounted folder).
#[derive(Clone, Debug, Default)]
pub struct Drive {
    /// "E:" on Windows.
    pub letter: String,
    pub label: String,
    /// "NTFS", "exFAT", "FAT32"...
    pub fs: String,
    pub total: u64,
    pub free: u64,
    /// The physical disk it lives on, and where on that disk it starts.
    pub disk: Option<u32>,
    pub offset: u64,
    /// Removable media, network drive, CD...
    pub kind: &'static str,
}

/// One physical disk, as far as Windows says without administrator rights.
#[derive(Clone, Debug, Default)]
pub struct Disk {
    pub number: u32,
    pub size: u64,
    pub model: String,
    pub bus: String,
    /// It holds Windows itself: never the recovered disk.
    pub system: bool,
    /// Windows treats it as removable media (like a USB stick): its policy
    /// for new disks does not apply, so it cannot be kept switched off.
    pub removable: bool,
    pub drives: Vec<Drive>,
}

/// Quote one argument the way Windows programs split their command line
/// (the rules CommandLineToArgvW and Rust's own std::env::args follow).
pub fn quote_arg(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '\n', '"']) {
        return a.to_string();
    }
    let mut out = String::from("\"");
    let mut slashes = 0usize;
    for c in a.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(slashes));
                out.push(c);
                slashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}

#[cfg(windows)]
mod imp {
    use super::{Disk, Drive};
    use std::ffi::c_void;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;

    type Handle = *mut c_void;

    #[repr(C)]
    struct ShellExecuteInfoW {
        cb_size: u32,
        f_mask: u32,
        hwnd: *mut c_void,
        lp_verb: *const u16,
        lp_file: *const u16,
        lp_parameters: *const u16,
        lp_directory: *const u16,
        n_show: i32,
        h_inst_app: *mut c_void,
        lp_id_list: *mut c_void,
        lp_class: *const u16,
        hkey_class: *mut c_void,
        dw_hot_key: u32,
        h_icon_or_monitor: *mut c_void,
        h_process: Handle,
    }

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteExW(info: *mut ShellExecuteInfoW) -> i32;
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
        fn GetTokenInformation(token: Handle, class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
    }

    extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn CloseHandle(h: Handle) -> i32;
        fn GetLastError() -> u32;
        fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
        fn GetExitCodeProcess(h: Handle, code: *mut u32) -> i32;
        fn GetLogicalDrives() -> u32;
        fn GetDriveTypeW(root: *const u16) -> u32;
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
        fn GetDiskFreeSpaceExW(root: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
        fn GetSystemDirectoryW(buf: *mut u16, len: u32) -> u32;
        fn GetWindowsDirectoryW(buf: *mut u16, len: u32) -> u32;
        fn SetThreadErrorMode(mode: u32, old: *mut u32) -> i32;
        fn DeviceIoControl(
            h: Handle,
            code: u32,
            inbuf: *const c_void,
            inlen: u32,
            outbuf: *mut c_void,
            outlen: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn from_wide(b: &[u16]) -> String {
        String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())])
    }

    pub fn is_elevated() -> bool {
        const TOKEN_QUERY: u32 = 0x0008;
        const TOKEN_ELEVATION: u32 = 20;
        unsafe {
            let mut token: Handle = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return false;
            }
            let mut elevated: u32 = 0;
            let mut ret: u32 = 0;
            let ok = GetTokenInformation(token, TOKEN_ELEVATION, &mut elevated as *mut u32 as *mut c_void, 4, &mut ret);
            CloseHandle(token);
            ok != 0 && elevated != 0
        }
    }

    /// Start `exe` with administrator rights (Windows asks the person
    /// first) and wait for it to finish. Ok(exit code), or Err(why).
    pub fn run_elevated(exe: &Path, args: &[String]) -> Result<i32, String> {
        const SEE_MASK_NOCLOSEPROCESS: u32 = 0x0000_0040;
        const SEE_MASK_NOASYNC: u32 = 0x0000_0100;
        const SW_HIDE: i32 = 0;
        const ERROR_CANCELLED: u32 = 1223;
        let verb = wide("runas");
        let file = wide(&exe.to_string_lossy());
        let params = wide(&args.iter().map(|a| super::quote_arg(a)).collect::<Vec<_>>().join(" "));
        let dir = exe.parent().map(|d| wide(&d.to_string_lossy())).unwrap_or_else(|| wide("."));
        let mut info = ShellExecuteInfoW {
            cb_size: std::mem::size_of::<ShellExecuteInfoW>() as u32,
            f_mask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            hwnd: std::ptr::null_mut(),
            lp_verb: verb.as_ptr(),
            lp_file: file.as_ptr(),
            lp_parameters: params.as_ptr(),
            lp_directory: dir.as_ptr(),
            n_show: SW_HIDE,
            h_inst_app: std::ptr::null_mut(),
            lp_id_list: std::ptr::null_mut(),
            lp_class: std::ptr::null(),
            hkey_class: std::ptr::null_mut(),
            dw_hot_key: 0,
            h_icon_or_monitor: std::ptr::null_mut(),
            h_process: std::ptr::null_mut(),
        };
        unsafe {
            if ShellExecuteExW(&mut info) == 0 {
                let e = GetLastError();
                return Err(if e == ERROR_CANCELLED {
                    "Windows asked for permission and the answer was No, so the disk could not be read. \
                     Try again and click Yes."
                        .to_string()
                } else {
                    format!("Windows could not start the program with administrator rights (error {e})")
                });
            }
            if info.h_process.is_null() {
                return Err("Windows started the program but gave no way to follow it".to_string());
            }
            WaitForSingleObject(info.h_process, 0xFFFF_FFFF);
            let mut code: u32 = 0;
            GetExitCodeProcess(info.h_process, &mut code);
            CloseHandle(info.h_process);
            Ok(code as i32)
        }
    }

    /// Start `exe` with administrator rights in a window of its own (Windows
    /// asks the person first), without waiting for it.
    pub fn start_elevated(exe: &Path, args: &[String]) -> Result<(), String> {
        const SEE_MASK_NOASYNC: u32 = 0x0000_0100;
        const SW_SHOWNORMAL: i32 = 1;
        const ERROR_CANCELLED: u32 = 1223;
        let verb = wide("runas");
        let file = wide(&exe.to_string_lossy());
        let params = wide(&args.iter().map(|a| super::quote_arg(a)).collect::<Vec<_>>().join(" "));
        let dir = exe.parent().map(|d| wide(&d.to_string_lossy())).unwrap_or_else(|| wide("."));
        let mut info = ShellExecuteInfoW {
            cb_size: std::mem::size_of::<ShellExecuteInfoW>() as u32,
            f_mask: SEE_MASK_NOASYNC,
            hwnd: std::ptr::null_mut(),
            lp_verb: verb.as_ptr(),
            lp_file: file.as_ptr(),
            lp_parameters: params.as_ptr(),
            lp_directory: dir.as_ptr(),
            n_show: SW_SHOWNORMAL,
            h_inst_app: std::ptr::null_mut(),
            lp_id_list: std::ptr::null_mut(),
            lp_class: std::ptr::null(),
            hkey_class: std::ptr::null_mut(),
            dw_hot_key: 0,
            h_icon_or_monitor: std::ptr::null_mut(),
            h_process: std::ptr::null_mut(),
        };
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            let e = unsafe { GetLastError() };
            return Err(if e == ERROR_CANCELLED {
                "cancelled".to_string()
            } else {
                format!("Windows could not start the program with administrator rights (error {e})")
            });
        }
        Ok(())
    }

    /// Free space for this user on the drive holding `path`.
    pub fn free_space(path: &Path) -> Option<u64> {
        let mut p = path.to_path_buf();
        // The folder may not exist yet: ask about the nearest one that does.
        while !p.exists() {
            p = p.parent()?.to_path_buf();
        }
        let w = wide(&p.to_string_lossy());
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        (unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, &mut total, &mut free) } != 0).then_some(avail)
    }

    fn ioctl(f: &std::fs::File, code: u32, input: &[u8], out_len: usize) -> Option<Vec<u8>> {
        let mut out = vec![0u8; out_len];
        let mut returned: u32 = 0;
        let ok = unsafe {
            DeviceIoControl(
                f.as_raw_handle() as Handle,
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

    /// Open a disk or volume without asking to read it: enough for the
    /// questions below, and needs no administrator rights.
    fn open_query(path: &str) -> Option<std::fs::File> {
        std::fs::OpenOptions::new().access_mode(0).share_mode(3).open(path).ok()
    }

    const IOCTL_DISK_GET_DRIVE_GEOMETRY_EX: u32 = 0x0007_00A0;
    const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
    const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;

    fn cstr_at(b: &[u8], off: u32) -> String {
        let off = off as usize;
        if off == 0 || off >= b.len() {
            return String::new();
        }
        let end = b[off..].iter().position(|&c| c == 0).map(|e| off + e).unwrap_or(b.len());
        String::from_utf8_lossy(&b[off..end]).trim().to_string()
    }

    /// (model, bus, removable media): STORAGE_DEVICE_DESCRIPTOR's vendor
    /// and product, BusType, and RemovableMedia (byte 10).
    fn identity(f: &std::fs::File) -> (String, String, bool) {
        let query = [0u8; 12];
        let b = match ioctl(f, IOCTL_STORAGE_QUERY_PROPERTY, &query, 1024) {
            Some(b) if b.len() >= 36 => b,
            _ => return (String::new(), String::new(), false),
        };
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
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
        (format!("{vendor} {product}").trim().to_string(), bus.to_string(), b[10] != 0)
    }

    pub fn drives() -> Vec<Drive> {
        let mut old = 0u32;
        // No "insert a disk" pop-ups for empty card readers.
        unsafe { SetThreadErrorMode(0x0001, &mut old) };
        let mask = unsafe { GetLogicalDrives() };
        let mut out = Vec::new();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let letter = format!("{}:", (b'A' + i as u8) as char);
            let root = wide(&format!("{letter}\\"));
            let kind = match unsafe { GetDriveTypeW(root.as_ptr()) } {
                2 => "removable",
                3 => "fixed",
                4 => "network",
                5 => "cd",
                6 => "ram",
                _ => "other",
            };
            let mut d = Drive { letter: letter.clone(), kind, ..Default::default() };
            if kind != "cd" && kind != "network" {
                let mut label = [0u16; 261];
                let mut fs = [0u16; 64];
                let mut serial = 0u32;
                let ok = unsafe {
                    GetVolumeInformationW(
                        root.as_ptr(),
                        label.as_mut_ptr(),
                        label.len() as u32,
                        &mut serial,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        fs.as_mut_ptr(),
                        fs.len() as u32,
                    )
                };
                if ok != 0 {
                    d.label = from_wide(&label);
                    d.fs = from_wide(&fs);
                }
                let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
                if unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut avail, &mut total, &mut free) } != 0 {
                    d.total = total;
                    d.free = avail;
                }
                if let Some(f) = open_query(&format!(r"\\.\{letter}")) {
                    if let Some(b) = ioctl(&f, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, &[], 8 + 24 * 8) {
                        // VOLUME_DISK_EXTENTS: count, padding, then {disk u32, pad, start i64, length i64}
                        if b.len() >= 32 && u32::from_le_bytes(b[0..4].try_into().unwrap()) >= 1 {
                            d.disk = Some(u32::from_le_bytes(b[8..12].try_into().unwrap()));
                            d.offset = u64::from_le_bytes(b[16..24].try_into().unwrap());
                        }
                    }
                }
            }
            out.push(d);
        }
        unsafe { SetThreadErrorMode(old, std::ptr::null_mut()) };
        out
    }

    fn system_letter() -> String {
        let mut buf = [0u16; 260];
        let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
        let s = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
        s.get(..2).unwrap_or("C:").to_uppercase()
    }

    /// Windows' System32 folder.
    pub fn system_dir() -> Option<std::path::PathBuf> {
        let mut buf = [0u16; 260];
        let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
        (n > 0 && n < buf.len()).then(|| std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n])))
    }

    /// Windows' own folder (where explorer.exe is).
    pub fn windows_dir() -> Option<std::path::PathBuf> {
        let mut buf = [0u16; 260];
        let n = unsafe { GetWindowsDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
        (n > 0 && n < buf.len()).then(|| std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n])))
    }

    pub fn disks() -> Vec<Disk> {
        let drives = drives();
        let sys = system_letter();
        let mut out = Vec::new();
        for n in 0..32u32 {
            let f = match open_query(&format!(r"\\.\PhysicalDrive{n}")) {
                Some(f) => f,
                None => continue,
            };
            let size = ioctl(&f, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, &[], 256)
                .filter(|b| b.len() >= 32)
                .map(|b| u64::from_le_bytes(b[24..32].try_into().unwrap()))
                .unwrap_or(0);
            let (model, bus, removable) = identity(&f);
            let on_it: Vec<Drive> = drives.iter().filter(|d| d.disk == Some(n)).cloned().collect();
            let system = on_it.iter().any(|d| d.letter.eq_ignore_ascii_case(&sys));
            out.push(Disk { number: n, size, model, bus, system, removable, drives: on_it });
        }
        out
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Disk, Drive};
    use std::path::Path;

    pub fn is_elevated() -> bool {
        // Root, as near as other systems come.
        std::env::var("USER").map(|u| u == "root").unwrap_or(false)
    }

    /// No permission question elsewhere: just run it and wait.
    pub fn run_elevated(exe: &Path, args: &[String]) -> Result<i32, String> {
        std::process::Command::new(exe)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.code().unwrap_or(1))
            .map_err(|e| format!("could not start {}: {e}", exe.display()))
    }

    pub fn start_elevated(exe: &Path, args: &[String]) -> Result<(), String> {
        std::process::Command::new(exe)
            .args(args)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("could not start {}: {e}", exe.display()))
    }

    pub fn free_space(_path: &Path) -> Option<u64> {
        None
    }

    pub fn drives() -> Vec<Drive> {
        vec![Drive { letter: "/".to_string(), label: "the whole system".to_string(), kind: "fixed", ..Default::default() }]
    }

    pub fn disks() -> Vec<Disk> {
        Vec::new()
    }

    pub fn system_dir() -> Option<std::path::PathBuf> {
        None
    }

    pub fn windows_dir() -> Option<std::path::PathBuf> {
        None
    }
}

/// One of Windows' own programs in System32 (diskpart.exe, rundll32.exe), by
/// its full path: the program runs with administrator rights, and a program
/// of the same name anywhere else - in its own folder, say, which Windows
/// would look in first - must never be started in its place.
pub fn system_program(name: &str) -> PathBuf {
    imp::system_dir().map(|d| d.join(name)).filter(|p| p.is_file()).unwrap_or_else(|| PathBuf::from(name))
}

/// The same for a program in Windows' own folder (explorer.exe).
pub fn windows_program(name: &str) -> PathBuf {
    imp::windows_dir().map(|d| d.join(name)).filter(|p| p.is_file()).unwrap_or_else(|| PathBuf::from(name))
}

pub fn is_elevated() -> bool {
    imp::is_elevated()
}

/// Run `exe args` with administrator rights and wait for it.
pub fn run_elevated(exe: &Path, args: &[String]) -> Result<i32, String> {
    imp::run_elevated(exe, args)
}

/// Start `exe args` with administrator rights in its own window; Err("cancelled")
/// when the person says No.
pub fn start_elevated(exe: &Path, args: &[String]) -> Result<(), String> {
    imp::start_elevated(exe, args)
}

/// Free space on the drive holding `path` (None where it cannot be told).
pub fn free_space(path: &Path) -> Option<u64> {
    imp::free_space(path)
}

/// Every drive letter, with its label, file system, size and disk.
pub fn drives() -> Vec<Drive> {
    imp::drives()
}

/// Every physical disk, with the drive letters on it.
pub fn disks() -> Vec<Disk> {
    imp::disks()
}

/// Open a web address in the PC's usual browser. From a copy running with
/// administrator rights this goes through Explorer, so the browser itself
/// does not get those rights.
pub fn open_browser(url: &str) -> bool {
    #[cfg(windows)]
    let r = if is_elevated() {
        std::process::Command::new(windows_program("explorer.exe")).arg(url).spawn()
    } else {
        std::process::Command::new(system_program("rundll32.exe")).args(["url.dll,FileProtocolHandler", url]).spawn()
    };
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    r.is_ok()
}

/// Show a folder (or select a file) in Explorer.
pub fn open_folder(path: &Path) -> bool {
    #[cfg(windows)]
    {
        let arg = if path.is_file() { format!("/select,{}", path.display()) } else { path.display().to_string() };
        std::process::Command::new(windows_program("explorer.exe")).arg(arg).spawn().is_ok()
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn().is_ok()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(path).spawn().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::quote_arg;

    #[test]
    fn windows_own_programs_by_their_full_path() {
        let p = super::system_program("diskpart.exe");
        let e = super::windows_program("explorer.exe");
        if cfg!(windows) {
            assert!(p.is_absolute() && p.is_file(), "{}", p.display());
            assert!(p.to_string_lossy().to_lowercase().ends_with(r"\system32\diskpart.exe"), "{}", p.display());
            // (A Windows without File Explorer falls back to the name.)
            assert!(e.is_absolute() || e.as_os_str() == "explorer.exe", "{}", e.display());
        } else {
            assert_eq!(p, std::path::PathBuf::from("diskpart.exe"));
            assert_eq!(e, std::path::PathBuf::from("explorer.exe"));
        }
    }

    #[test]
    fn arguments_survive_windows_quoting() {
        assert_eq!(quote_arg("plain"), "plain");
        assert_eq!(quote_arg("with space"), "\"with space\"");
        assert_eq!(quote_arg(r"C:\RECOVERY-TOOLS\results\"), r"C:\RECOVERY-TOOLS\results\");
        assert_eq!(quote_arg(r"C:\My Files\"), "\"C:\\My Files\\\\\"");
        assert_eq!(quote_arg("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(quote_arg(r"\\.\PhysicalDrive2"), r"\\.\PhysicalDrive2");
    }
}
