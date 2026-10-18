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
//!  3. the program marks that disk read-only, which Windows remembers for it
//!     from then on, and puts the policy back the way it was;
//!  4. if the person wants to browse it, the disk is switched on: Windows
//!     mounts it read-only.
//!
//! The program itself never needs the disk switched on: it reads it directly.
//! Every step is checked on GitHub's Windows machines (tools/guard_check.py),
//! including that a protected disk comes back byte for byte unchanged.

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
        // Remembered for this disk from now on.
        set(n, DISK_ATTRIBUTE_READ_ONLY, DISK_ATTRIBUTE_READ_ONLY, true)
    }

    pub fn set_online(n: u32, online: bool) -> Result<(), String> {
        set(n, if online { 0 } else { DISK_ATTRIBUTE_OFFLINE }, DISK_ATTRIBUTE_OFFLINE, false)
    }

    fn diskpart(lines: &[&str]) -> Result<String, String> {
        let dir = std::env::temp_dir();
        let script = dir.join(format!("recovery-probe-diskpart-{}.txt", std::process::id()));
        std::fs::write(&script, lines.join("\r\n") + "\r\n").map_err(|e| format!("could not write {}: {e}", script.display()))?;
        let out = std::process::Command::new("diskpart").arg("/s").arg(&script).output();
        let _ = std::fs::remove_file(&script);
        let out = out.map_err(|e| format!("could not run diskpart: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        if !out.status.success() {
            return Err(format!("diskpart failed: {}", text.trim()));
        }
        Ok(text)
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

pub use imp::{san_policy, san_policy_registry, set_online, set_read_only, set_san_policy, state};

/// Mark a disk read-only (remembered for it), then switch it on if asked.
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
    }
}
