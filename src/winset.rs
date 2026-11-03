//! The Windows settings the recovery cares about, checked (and the few the
//! program may change, changed only when the person clicks for it, and put
//! back at the end):
//!
//! - The PC must not sleep while the program works: while any job runs
//!   (Start, Finalise, a step by itself), the program asks Windows to stay
//!   awake (SetThreadExecutionState), so no power setting needs changing; it
//!   lapses by itself when the program stops.
//! - A restart Windows is waiting to do (an update) must not happen with the
//!   recovered SSD plugged in: checked, and the person told to restart first.
//! - Windows restarting by itself after a crash would restart with the SSD
//!   plugged in: checked, and turned off on request (put back at the end).
//! - Defender scanning every file the program writes slows it 2-3 times and
//!   can quarantine a recovered file: an exclusion for the image folder and
//!   SPARE, on request (removed at the end).
//! - AIM Toolkit's driver, for opening the image read-only.

use std::path::Path;

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

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
        fn RegOpenKeyExW(key: isize, subkey: *const u16, options: u32, access: u32, result: *mut isize) -> i32;
        fn RegCloseKey(key: isize) -> i32;
        fn RegSetKeyValueW(key: isize, subkey: *const u16, value: *const u16, kind: u32, data: *const c_void, len: u32) -> i32;
        fn RegEnumKeyExW(
            key: isize,
            index: u32,
            name: *mut u16,
            name_len: *mut u32,
            reserved: *mut u32,
            class: *mut u16,
            class_len: *mut u32,
            last_write: *mut c_void,
        ) -> i32;
        fn RegEnumValueW(
            key: isize,
            index: u32,
            name: *mut u16,
            name_len: *mut u32,
            reserved: *mut u32,
            kind: *mut u32,
            data: *mut u8,
            data_len: *mut u32,
        ) -> i32;
    }

    extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
        fn GetTickCount64() -> u64;
    }

    /// Milliseconds since Windows started (a restart starts it again).
    pub fn up_ms() -> Option<u64> {
        Some(unsafe { GetTickCount64() })
    }

    const HKLM: isize = 0x8000_0002u32 as i32 as isize;
    const KEY_READ: u32 = 0x20019;
    const KEY_WOW64_64KEY: u32 = 0x0100;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn key_exists(path: &str) -> bool {
        let k = wide(path);
        let mut h: isize = 0;
        let r = unsafe { RegOpenKeyExW(HKLM, k.as_ptr(), 0, KEY_READ | KEY_WOW64_64KEY, &mut h) };
        if r == 0 {
            unsafe { RegCloseKey(h) };
            true
        } else {
            false
        }
    }

    /// The names of a key's subkeys and values (None: no such key).
    pub fn names_under(path: &str) -> Option<Vec<String>> {
        let k = wide(path);
        let mut h: isize = 0;
        if unsafe { RegOpenKeyExW(HKLM, k.as_ptr(), 0, KEY_READ | KEY_WOW64_64KEY, &mut h) } != 0 {
            return None;
        }
        let mut out = Vec::new();
        let mut buf = vec![0u16; 16_384];
        for values in [false, true] {
            for i in 0..10_000u32 {
                let mut len = buf.len() as u32;
                let r = unsafe {
                    if values {
                        RegEnumValueW(h, i, buf.as_mut_ptr(), &mut len, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
                    } else {
                        RegEnumKeyExW(h, i, buf.as_mut_ptr(), &mut len, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
                    }
                };
                if r != 0 {
                    break; // ERROR_NO_MORE_ITEMS, or one that cannot be read
                }
                let name = String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]);
                out.push(if values { format!("value {name}") } else { format!("key {name}") });
            }
        }
        unsafe { RegCloseKey(h) };
        out.sort();
        Some(out)
    }

    pub fn dword(path: &str, value: &str) -> Option<u32> {
        let k = wide(path);
        let v = wide(value);
        let mut data = 0u32;
        let mut len = 4u32;
        let r = unsafe {
            RegGetValueW(HKLM, k.as_ptr(), v.as_ptr(), 0x10 | 0x0001_0000, std::ptr::null_mut(), &mut data as *mut u32 as *mut c_void, &mut len)
        };
        (r == 0).then_some(data)
    }

    pub fn set_dword(path: &str, value: &str, data: u32) -> Result<(), String> {
        let k = wide(path);
        let v = wide(value);
        let r = unsafe { RegSetKeyValueW(HKLM, k.as_ptr(), v.as_ptr(), 4, &data as *const u32 as *const c_void, 4) };
        if r == 0 {
            Ok(())
        } else {
            Err(format!("Windows would not change the setting (error {r}); it needs administrator rights"))
        }
    }

    pub fn keep_awake(on: bool) {
        const ES_CONTINUOUS: u32 = 0x8000_0000;
        const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
        unsafe {
            SetThreadExecutionState(if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS });
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn key_exists(_path: &str) -> bool {
        false
    }
    pub fn names_under(_path: &str) -> Option<Vec<String>> {
        None
    }
    pub fn dword(_path: &str, _value: &str) -> Option<u32> {
        None
    }
    pub fn set_dword(_path: &str, _value: &str, _data: u32) -> Result<(), String> {
        Err("only on Windows".to_string())
    }
    pub fn keep_awake(_on: bool) {}
    pub fn up_ms() -> Option<u64> {
        None
    }
}

/// Ask Windows not to sleep while this thread works (and stop asking).
pub fn keep_awake(on: bool) {
    imp::keep_awake(on)
}

/// Where Windows says it is waiting to restart.
const RESTART_KEYS: [&str; 2] = [
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending",
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired",
];

/// Is Windows waiting to restart (an update installed, a restart due)?
pub fn restart_pending() -> Option<bool> {
    if !cfg!(windows) {
        return None;
    }
    Some(RESTART_KEYS.iter().any(|k| imp::key_exists(k)))
}

/// What Windows is waiting to restart for, as it says it (the updates, by
/// the names it keeps them under): the same after a restart, it is a restart
/// that a restart does not clear; another, it is a new one.
pub fn restart_pending_for() -> String {
    let mut t = Vec::new();
    for (i, k) in RESTART_KEYS.iter().enumerate() {
        if let Some(names) = imp::names_under(k) {
            t.push(format!("{i}: {}", names.join(", ")));
        }
    }
    t.join("; ")
}

/// A restart Windows is waiting for that a restart did not clear: asked for
/// (at `asked`, waiting for `asked_for`), Windows started again since, and
/// still waiting for the very same (`now_for`). Waiting for something else
/// since, it is a new restart to do (a failed update stays the same; a new
/// one comes under a name of its own).
pub fn restart_is_stuck(asked: Option<u64>, asked_for: &str, started: Option<u64>, now_for: &str) -> bool {
    restarted_since(asked, started) && asked_for == now_for
}

/// When Windows last started, in seconds since 1970 (None: not known).
pub fn started_at() -> Option<u64> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(now.saturating_sub(imp::up_ms()? / 1000))
}

/// Has Windows started again since `asked` (seconds since 1970) - is a
/// restart asked for then done? (A minute's margin: the two clocks read
/// at different moments.)
pub fn restarted_since(asked: Option<u64>, started: Option<u64>) -> bool {
    matches!((asked, started), (Some(a), Some(s)) if s > a + 60)
}

const CRASH_KEY: &str = r"SYSTEM\CurrentControlSet\Control\CrashControl";

/// Does Windows restart by itself after a crash ("Automatically restart")?
pub fn restarts_after_crash() -> Option<bool> {
    imp::dword(CRASH_KEY, "AutoReboot").map(|v| v != 0)
}

pub fn set_restarts_after_crash(on: bool) -> Result<(), String> {
    imp::set_dword(CRASH_KEY, "AutoReboot", on as u32)
}

/// Is AIM Toolkit's driver (Arsenal Image Mounter's) installed?
pub fn aim_driver_installed() -> Option<bool> {
    if !cfg!(windows) {
        return None;
    }
    Some(imp::key_exists(r"SYSTEM\CurrentControlSet\Services\phdskmnt"))
}

/// The folders Defender leaves alone (None when it would not say).
pub fn defender_exclusions() -> Option<Vec<String>> {
    if !cfg!(windows) {
        return None;
    }
    let ps = crate::win::system_program(r"WindowsPowerShell\v1.0\powershell.exe");
    let r = crate::tools::run(
        &ps,
        &crate::tools::os(&["-NoProfile", "-NonInteractive", "-Command", "(Get-MpPreference).ExclusionPath -join \"`n\""]),
        None,
        std::time::Duration::from_secs(60),
        &|| false,
    )
    .ok()?;
    if r.code != Some(0) {
        return None;
    }
    Some(r.out.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty() && !l.starts_with("N/A")).collect())
}

/// Disk `n`'s temperature (°C) from Windows' storage reliability counter:
/// for a disk whose driver does not answer the direct question (win.rs).
/// Asks for nothing to be written; None when Windows does not know it.
pub fn reliability_temperature(n: u32) -> Option<i16> {
    if !cfg!(windows) {
        return None;
    }
    let ps = crate::win::system_program(r"WindowsPowerShell\v1.0\powershell.exe");
    let cmd = format!(
        "$d = Get-PhysicalDisk | Where-Object {{ $_.DeviceId -eq '{n}' }} | Select-Object -First 1; \
         if ($d) {{ ($d | Get-StorageReliabilityCounter).Temperature }}"
    );
    let r = crate::tools::run(&ps, &crate::tools::os(&["-NoProfile", "-NonInteractive", "-Command", &cmd]), None, std::time::Duration::from_secs(60), &|| false)
        .ok()?;
    if r.code != Some(0) {
        return None;
    }
    r.out.lines().filter_map(|l| l.trim().parse::<i16>().ok()).find(|t| (1..=crate::win::PLAUSIBLE_MOST).contains(t))
}

/// Words in PowerShell's single quotes. PowerShell takes the curly single
/// quotes (‘ ’ ‚ ‛) for quotes too - "Mum’s photos", as a Mac or an iPhone
/// writes it, would otherwise end the quoted words early - so every one of
/// them is doubled, as PowerShell's own escaping does.
pub fn ps_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// Add (or remove) a Defender exclusion for a folder.
pub fn set_defender_exclusion(folder: &Path, add: bool) -> Result<(), String> {
    if !cfg!(windows) {
        return Err("only on Windows".to_string());
    }
    let ps = crate::win::system_program(r"WindowsPowerShell\v1.0\powershell.exe");
    let cmd = format!(
        "{} -ExclusionPath {}",
        if add { "Add-MpPreference" } else { "Remove-MpPreference" },
        ps_quote(&folder.display().to_string())
    );
    let r = crate::tools::run(&ps, &crate::tools::os(&["-NoProfile", "-NonInteractive", "-Command", &cmd]), None, std::time::Duration::from_secs(60), &|| false)
        .map_err(|e| format!("could not start PowerShell: {e}"))?;
    if r.code == Some(0) {
        Ok(())
    } else {
        Err(format!("Defender would not change it: {}", r.text().lines().next().unwrap_or("no reason given").trim()))
    }
}

/// Is `folder` (or a folder above it) excluded?
pub fn is_excluded(exclusions: &[String], folder: &Path) -> bool {
    let f = folder.display().to_string().to_lowercase().trim_end_matches('\\').to_string() + "\\";
    exclusions.iter().any(|e| {
        let e = e.to_lowercase().trim_end_matches('\\').to_string() + "\\";
        f.starts_with(&e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A restart asked for, done, and Windows still waiting for one: Start
    /// carries on (before, it stopped there for good). Not done: it stops.
    #[test]
    fn a_restart_done_since_it_was_asked_for_is_told() {
        assert!(restarted_since(Some(1_000_000), Some(1_000_100)));
        assert!(!restarted_since(Some(1_000_000), Some(1_000_030)), "the same start, read a little later");
        assert!(!restarted_since(Some(1_000_000), Some(900_000)), "started before it was asked for");
        assert!(!restarted_since(None, Some(1_000_000)), "never asked");
        assert!(!restarted_since(Some(1_000_000), None), "not known");
        // The same updates waiting after the restart: stuck. Others: a new
        // restart to do (before, any restart since the first time it was
        // asked for let Start carry on for good - a new update a day later
        // too, with the recovered SSD to be plugged in).
        let w = "1: value {0D1E2C3B-4A59-6877-8695-A4B3C2D1E0F9}";
        assert!(restart_is_stuck(Some(1_000_000), w, Some(1_000_100), w));
        assert!(!restart_is_stuck(Some(1_000_000), w, Some(1_000_100), "1: value {11111111-2222-3333-4444-555555555555}"));
        assert!(!restart_is_stuck(Some(1_000_000), w, Some(1_000_030), w), "not restarted since");
        assert!(!restart_is_stuck(None, "", Some(1_000_100), ""), "never asked");
        if cfg!(windows) {
            let s = started_at().expect("Windows says when it started");
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
            assert!(s <= now && s > 1_600_000_000, "{s}");
            // The names under a key Windows always has, read as they are.
            let names = imp::names_under(r"SOFTWARE\Microsoft\Windows\CurrentVersion").expect("the key is there");
            assert!(names.iter().any(|n| n == "key Explorer") && names.iter().any(|n| n.starts_with("value ")), "{names:?}");
            assert_eq!(imp::names_under(r"SOFTWARE\No Such Key Here 7f3a"), None);
            // What Windows waits for: said the same twice running.
            assert_eq!(restart_pending_for(), restart_pending_for());
            assert_eq!(restart_pending() == Some(true), !restart_pending_for().is_empty());
        } else {
            assert_eq!(started_at(), None);
            assert_eq!(restart_pending_for(), "");
        }
    }

    #[test]
    fn exclusions_cover_folders_below_them() {
        let ex = vec![r"E:\".to_string(), r"D:\IMAGE".to_string()];
        assert!(is_excluded(&ex, Path::new(r"E:\WORK")));
        assert!(is_excluded(&ex, Path::new(r"D:\IMAGE")));
        assert!(is_excluded(&ex, Path::new(r"d:\image\")));
        assert!(!is_excluded(&ex, Path::new(r"D:\IMAGES")));
        assert!(!is_excluded(&ex, Path::new(r"D:\COPY 2")));
        assert_eq!(ps_quote("D:\\it's"), "'D:\\it''s'");
        assert_eq!(ps_quote("Mum\u{2019}s \u{2018}best\u{201B}\u{201A}"), "'Mum\u{2019}\u{2019}s \u{2018}\u{2018}best\u{201B}\u{201B}\u{201A}\u{201A}'");
        assert_eq!(ps_quote("a \"b\" $c `d"), "'a \"b\" $c `d'", "nothing else is special in single quotes");
    }

    /// On Windows: PowerShell reads every kind of quote back as it was.
    #[cfg(windows)]
    #[test]
    fn powershell_reads_the_quoted_words_back() {
        let s = "D:\\Mum\u{2019}s \u{2018}photos\u{201A}\u{201B} it's \"x\" $y `z";
        let codes: Vec<String> = s.chars().map(|c| format!("0x{:X}", c as u32)).collect();
        let check = format!("$v = {}; $w = -join ([char[]]({})); [Console]::Out.Write($v -ceq $w)", ps_quote(s), codes.join(","));
        let ps = crate::win::system_program(r"WindowsPowerShell\v1.0\powershell.exe");
        let r = crate::tools::run(&ps, &crate::tools::os(&["-NoProfile", "-NonInteractive", "-Command", &check]), None, std::time::Duration::from_secs(60), &|| false)
            .unwrap();
        assert_eq!(r.out.trim(), "True", "{}", r.text());
    }

    /// On Windows: Defender takes a folder to leave alone, lists it, and
    /// takes it off again - the very commands Start, Finalise and 7. Put
    /// Windows back as it was use. (The test machines' own folders are left
    /// alone already, so Start never asks for one there.)
    #[cfg(windows)]
    #[test]
    fn defender_takes_a_folder_to_leave_alone_and_gives_it_back() {
        let Some(before) = defender_exclusions() else {
            eprintln!("(Defender does not answer here: nothing to check)");
            return;
        };
        let dir = std::env::temp_dir().join(format!("arp-defender-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shown = dir.display().to_string();
        let listed = |l: &[String]| l.iter().any(|e| e.trim_end_matches('\\').eq_ignore_ascii_case(shown.trim_end_matches('\\')));
        assert!(!listed(&before), "{shown} is left alone already");
        if let Err(e) = set_defender_exclusion(&dir, true) {
            eprintln!("(Defender here takes no folder to leave alone: {e})");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let during = defender_exclusions().unwrap_or_default();
        // (Taken off again whatever was listed.)
        let off = set_defender_exclusion(&dir, false);
        let after = defender_exclusions().unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(listed(&during), "not in Defender's list once asked for: {shown} ({during:?})");
        assert!(is_excluded(&during, &dir.join("WORK")), "a folder inside it is left alone too");
        off.unwrap();
        assert!(!listed(&after), "still in Defender's list once taken off: {shown}");
    }
}
