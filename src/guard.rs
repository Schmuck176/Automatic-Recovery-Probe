//! Keeping Windows from writing to the recovered SSD.
//!
//! When a drive is plugged in, Windows mounts it straight away and may write
//! to it on its own: it marks the volume as in use, and often adds a "System
//! Volume Information" folder. On the only copy of someone's files that is
//! not acceptable, so the front end does this instead:
//!
//!  1. before the SSD is plugged in, Windows' policy for new disks is set to
//!     "leave them switched off" (the SAN policy, as `diskpart san` shows it);
//!  2. the SSD arrives switched off (offline): no volume, nothing mounted,
//!     nothing that could write to it;
//!  3. the program marks that disk read-only (Windows forgets the read-only
//!     mark of a virtual disk when it is unplugged - a real disk's it may
//!     remember - so this is done every time it is plugged in);
//!  3a. the policy stays "leave new disks switched off" until the person
//!     puts Windows back (7. Put Windows back as it was, web/back.rs), which
//!     is refused while any copy of the old drive or SPARE is plugged in -
//!     nothing puts it back by itself (web.rs: hold_new_disks_off) - so a
//!     SSD that comes unplugged and is plugged in again arrives switched off;
//!  4. if the person wants to browse it, the disk is switched on: Windows
//!     mounts it read-only.
//!
//! The program itself never needs the disk switched on: it reads it directly.
//! Every step is checked on GitHub's Windows machines (tools/guard_check.py),
//! including that a protected disk comes back byte for byte unchanged.
//! (Measured there too: Windows Server wrote nothing to an unprotected
//! exFAT disk in 20 seconds, and a disk kept switched off by the policy is
//! read-only as well.)

/// Windows' policy for disks it has not seen before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SanPolicy {
    OnlineAll,
    OfflineShared,
    OfflineAll,
    OfflineInternal,
}

impl SanPolicy {
    /// The word `diskpart san policy=` takes.
    pub fn keyword(self) -> &'static str {
        match self {
            SanPolicy::OnlineAll => "OnlineAll",
            SanPolicy::OfflineShared => "OfflineShared",
            SanPolicy::OfflineAll => "OfflineAll",
            SanPolicy::OfflineInternal => "OfflineInternal",
        }
    }
    pub fn from_keyword(s: &str) -> Option<SanPolicy> {
        let k: String = s.chars().filter(|c| c.is_ascii_alphabetic()).collect::<String>().to_lowercase();
        match k.as_str() {
            "onlineall" => Some(SanPolicy::OnlineAll),
            "offlineshared" => Some(SanPolicy::OfflineShared),
            "offlineall" => Some(SanPolicy::OfflineAll),
            "offlineinternal" => Some(SanPolicy::OfflineInternal),
            _ => None,
        }
    }
    /// In plain words.
    pub fn describe(self) -> &'static str {
        match self {
            SanPolicy::OnlineAll => "new disks are switched on as soon as they are plugged in",
            SanPolicy::OfflineShared => "new disks are switched on, except disks on a shared bus",
            SanPolicy::OfflineAll => "new disks stay switched off until something switches them on",
            SanPolicy::OfflineInternal => "new internal disks stay switched off",
        }
    }
}

/// What `diskpart san` printed: the policy, from the line "SAN Policy : ...".
pub fn parse_san(output: &str) -> Option<SanPolicy> {
    output
        .lines()
        .filter(|l| l.to_lowercase().contains("san"))
        .filter_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
        .find_map(|v| SanPolicy::from_keyword(&v))
}

/// Runs diskpart (`program`, its full path) with these commands, given to
/// it through its input - never through a script file: diskpart runs with
/// administrator rights, and a file in the temporary folder could be changed
/// by another program between being written and being read.
#[cfg_attr(not(windows), allow(dead_code))]
fn run_diskpart(program: &std::path::Path, lines: &[&str]) -> Result<String, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run diskpart: {e}"))?;
    let mut input = String::new();
    for l in lines {
        input.push_str(l);
        input.push_str("\r\n");
    }
    input.push_str("exit\r\n");
    // (Closed once written: diskpart then has all its input.)
    let wrote = child.stdin.take().map(|mut s| s.write_all(input.as_bytes()));
    let out = child.wait_with_output().map_err(|e| format!("could not run diskpart: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if let Some(Err(e)) = wrote {
        return Err(format!("could not give diskpart its commands ({e}): {}", text.trim()));
    }
    if !out.status.success() {
        return Err(format!("diskpart failed: {}", text.trim()));
    }
    Ok(text)
}

/// One disk's switches.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiskState {
    pub offline: bool,
    pub read_only: bool,
    /// What Windows answers when asked whether the disk can be written to.
    pub writable: Option<bool>,
}

#[cfg(windows)]
mod imp {
    use super::{parse_san, DiskState, SanPolicy};
    use std::ffi::c_void;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;

    type Handle = *mut c_void;

    extern "system" {
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
        fn GetLastError() -> u32;
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn RegGetValueW(
            key: isize,
            subkey: *const u16,
            value: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut c_void,
            len: *mut u32,
        ) -> i32;
    }

    const IOCTL_DISK_GET_DISK_ATTRIBUTES: u32 = 0x0007_00F0;
    const IOCTL_DISK_SET_DISK_ATTRIBUTES: u32 = 0x0007_C0F4;
    const IOCTL_DISK_IS_WRITABLE: u32 = 0x0007_0024;
    const IOCTL_DISK_UPDATE_PROPERTIES: u32 = 0x0007_0140;
    const DISK_ATTRIBUTE_OFFLINE: u64 = 1;
    const DISK_ATTRIBUTE_READ_ONLY: u64 = 2;
    const ERROR_WRITE_PROTECT: u32 = 19;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn open(n: u32, write: bool) -> Result<std::fs::File, String> {
        let mut o = std::fs::OpenOptions::new();
        o.read(true).share_mode(3);
        if write {
            // Needed to change a disk's switches; nothing is written to it.
            o.write(true);
        }
        o.open(format!(r"\\.\PhysicalDrive{n}")).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                "this needs administrator rights".to_string()
            } else {
                format!("could not open disk {n}: {e}")
            }
        })
    }

    fn ioctl(f: &std::fs::File, code: u32, input: &[u8], out: &mut [u8]) -> Result<u32, u32> {
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                f.as_raw_handle() as Handle,
                code,
                if input.is_empty() { std::ptr::null() } else { input.as_ptr() as *const c_void },
                input.len() as u32,
                if out.is_empty() { std::ptr::null_mut() } else { out.as_mut_ptr() as *mut c_void },
                out.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(unsafe { GetLastError() })
        } else {
            Ok(returned)
        }
    }

    pub fn state(n: u32) -> Result<DiskState, String> {
        let f = open(n, false)?;
        // GET_DISK_ATTRIBUTES: Version, Reserved1 (u32 each), Attributes (u64).
        let mut out = [0u8; 16];
        ioctl(&f, IOCTL_DISK_GET_DISK_ATTRIBUTES, &[], &mut out)
            .map_err(|e| format!("Windows would not say how disk {n} is set (error {e})"))?;
        let a = u64::from_le_bytes(out[8..16].try_into().unwrap());
        let writable = match ioctl(&f, IOCTL_DISK_IS_WRITABLE, &[], &mut []) {
            Ok(_) => Some(true),
            Err(ERROR_WRITE_PROTECT) => Some(false),
            Err(_) => None,
        };
        Ok(DiskState { offline: a & DISK_ATTRIBUTE_OFFLINE != 0, read_only: a & DISK_ATTRIBUTE_READ_ONLY != 0, writable })
    }

    fn set(n: u32, attributes: u64, mask: u64, persist: bool) -> Result<(), String> {
        // Windows' own disk is never switched off or marked read-only,
        // whatever asks (the page never offers it; this holds for the
        // command line, and for any mistake, too).
        if crate::win::disks().iter().any(|d| d.number == n && d.system) {
            return Err(format!(
                "disk {n} holds Windows itself: the program never switches it off or marks it read-only"
            ));
        }
        let f = open(n, true)?;
        // SET_DISK_ATTRIBUTES: Version (its size), Persist, 3 reserved bytes,
        // Attributes, AttributesMask, 4 reserved u32s: 40 bytes.
        let mut b = [0u8; 40];
        b[0..4].copy_from_slice(&40u32.to_le_bytes());
        b[4] = persist as u8;
        b[8..16].copy_from_slice(&attributes.to_le_bytes());
        b[16..24].copy_from_slice(&mask.to_le_bytes());
        ioctl(&f, IOCTL_DISK_SET_DISK_ATTRIBUTES, &b, &mut [])
            .map_err(|e| format!("Windows would not change disk {n}'s settings (error {e})"))?;
        let _ = ioctl(&f, IOCTL_DISK_UPDATE_PROPERTIES, &[], &mut []);
        Ok(())
    }

    pub fn set_read_only(n: u32) -> Result<(), String> {
        // Asked to be remembered (Windows forgets it for virtual disks anyway).
        set(n, DISK_ATTRIBUTE_READ_ONLY, DISK_ATTRIBUTE_READ_ONLY, true)
    }

    pub fn clear_read_only(n: u32) -> Result<(), String> {
        set(n, 0, DISK_ATTRIBUTE_READ_ONLY, true)
    }

    pub fn set_online(n: u32, online: bool) -> Result<(), String> {
        set(n, if online { 0 } else { DISK_ATTRIBUTE_OFFLINE }, DISK_ATTRIBUTE_OFFLINE, false)
    }

    /// The same, asked to be remembered: Windows keeps the setting for the
    /// disk (by the identity it sees - for a USB case, often the case's own)
    /// and gives it again when the disk is plugged in next time.
    pub fn set_online_remembered(n: u32, online: bool) -> Result<(), String> {
        set(n, if online { 0 } else { DISK_ATTRIBUTE_OFFLINE }, DISK_ATTRIBUTE_OFFLINE, true)
    }

    /// Windows' own diskpart (from System32, by its full path), given these
    /// commands.
    fn diskpart(lines: &[&str]) -> Result<String, String> {
        super::run_diskpart(&crate::win::system_program("diskpart.exe"), lines)
    }

    pub fn san_policy() -> Result<SanPolicy, String> {
        let text = diskpart(&["san"])?;
        parse_san(&text).ok_or_else(|| format!("could not understand diskpart's answer: {}", text.trim()))
    }

    pub fn set_san_policy(p: SanPolicy) -> Result<(), String> {
        diskpart(&[&format!("san policy={}", p.keyword())]).map(|_| ())
    }

    /// The SAN policy as the registry holds it (1 = online all ... 4), for the checks.
    pub fn san_policy_registry() -> Option<u32> {
        let key = wide(r"SYSTEM\CurrentControlSet\Services\partmgr\Parameters");
        let value = wide("SanPolicy");
        let mut data = 0u32;
        let mut len = 4u32;
        let r = unsafe {
            RegGetValueW(
                0x8000_0002u32 as i32 as isize, // HKEY_LOCAL_MACHINE
                key.as_ptr(),
                value.as_ptr(),
                0x10, // RRF_RT_REG_DWORD
                std::ptr::null_mut(),
                &mut data as *mut u32 as *mut c_void,
                &mut len,
            )
        };
        (r == 0).then_some(data)
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{DiskState, SanPolicy};

    const ONLY: &str = "only on Windows";

    pub fn state(_n: u32) -> Result<DiskState, String> {
        Err(ONLY.to_string())
    }
    pub fn set_read_only(_n: u32) -> Result<(), String> {
        Err(ONLY.to_string())
    }
    pub fn set_online(_n: u32, _online: bool) -> Result<(), String> {
        Err(ONLY.to_string())
    }
    pub fn set_online_remembered(_n: u32, _online: bool) -> Result<(), String> {
        Err(ONLY.to_string())
    }
    pub fn clear_read_only(_n: u32) -> Result<(), String> {
        Err(ONLY.to_string())
    }
    pub fn san_policy() -> Result<SanPolicy, String> {
        Err(ONLY.to_string())
    }
    pub fn set_san_policy(_p: SanPolicy) -> Result<(), String> {
        Err(ONLY.to_string())
    }
    pub fn san_policy_registry() -> Option<u32> {
        None
    }
}

pub use imp::{
    clear_read_only, san_policy, san_policy_registry, set_online, set_online_remembered, set_read_only, set_san_policy, state,
};

/// For a disk that is not the recovered SSD (the drive the copies go to,
/// plugged in after "Get Windows ready", or protected by mistake): switch
/// it on and make it writable again. The caller checks which disk it is.
pub fn release(n: u32) -> Result<DiskState, String> {
    clear_read_only(n)?;
    let s = state(n)?;
    if !s.offline {
        // Switched off and on again, so Windows mounts it writable.
        set_online(n, false)?;
    }
    // Remembered switched on: a disk the rehearsal switched off (remembered
    // so) comes back normal from now on.
    set_online_remembered(n, true)?;
    let s = state(n)?;
    if s.read_only || s.offline {
        return Err(format!("disk {n} is still switched off or read-only (Windows says: {s:?})"));
    }
    Ok(s)
}

/// SPARE, switched off and read-only, both remembered by Windows: plugged in
/// again the same way, it arrives switched off - and so does any disk Windows
/// takes for it in the same USB case. (A disk new to Windows arrives switched
/// off anyway, by Windows' setting for new disks.)
pub fn switch_off_remembered(n: u32) -> Result<DiskState, String> {
    set_read_only(n)?;
    set_online_remembered(n, false)?;
    let s = state(n)?;
    if !s.offline || !s.read_only {
        return Err(format!("disk {n} did not stay switched off and read-only (Windows says: {s:?})"));
    }
    Ok(s)
}

/// Mark a disk read-only, then switch it on if asked.
/// Checks each step took.
pub fn protect(n: u32, online: bool) -> Result<DiskState, String> {
    set_read_only(n)?;
    let s = state(n)?;
    if !s.read_only || s.writable == Some(true) {
        return Err(format!("disk {n} did not become read-only (Windows says: {s:?})"));
    }
    if online && s.offline {
        set_online(n, true)?;
    }
    let s = state(n)?;
    if !s.read_only {
        return Err(format!("disk {n} lost its read-only setting when switched on (Windows says: {s:?})"));
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_diskpart_says() {
        let out = "\r\nMicrosoft DiskPart version 10.0.26100.1\r\n\r\nCopyright (C) Microsoft Corporation.\r\n\
                   On computer: BIGGAMER\r\n\r\nSAN Policy  : Online All\r\n";
        assert_eq!(parse_san(out), Some(SanPolicy::OnlineAll));
        assert_eq!(parse_san("SAN Policy  : Offline Shared"), Some(SanPolicy::OfflineShared));
        assert_eq!(parse_san("nothing useful"), None);
        assert_eq!(SanPolicy::from_keyword("OfflineAll"), Some(SanPolicy::OfflineAll));
        assert_eq!(SanPolicy::OfflineInternal.keyword(), "OfflineInternal");
        // What diskpart prints when its commands come through its input: a
        // prompt before each answer.
        let piped = "Microsoft DiskPart version 10.0.26100.1\r\n\r\nOn computer: SANDBOX-PC\r\n\r\nDISKPART> \r\n\
                     SAN Policy  : Offline All\r\n\r\nDISKPART> \r\nLeaving DiskPart...\r\n";
        assert_eq!(parse_san(piped), Some(SanPolicy::OfflineAll));
        assert_eq!(parse_san("DISKPART> SAN Policy  : Online All"), Some(SanPolicy::OnlineAll));
    }

    #[cfg(unix)]
    #[test]
    fn the_commands_go_in_through_diskparts_input() {
        // `cat` gives back what it is given: the commands, then "exit".
        let out = run_diskpart(std::path::Path::new("cat"), &["san", "san policy=OfflineAll"]).unwrap();
        assert_eq!(out, "san\r\nsan policy=OfflineAll\r\nexit\r\n");
        assert!(run_diskpart(std::path::Path::new("/nonexistent/diskpart"), &["san"]).unwrap_err().contains("could not run"));
        assert!(run_diskpart(std::path::Path::new("false"), &["san"]).is_err());
    }
}
