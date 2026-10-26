//! "Show me in Windows": Windows' own pages and tools that show each setting
//! the program changes or relies on, so you can see it for yourself instead
//! of taking the program's word for it. Opening them changes nothing.
//!
//! - Defender's exclusions: Windows Security, Exclusions.
//! - New disks arriving switched off, the disks' state, "Automatically
//!   restart", a restart waiting, the PC kept awake: a window of Windows'
//!   own answers (Get-MpPreference, diskpart's SAN policy, Get-Disk,
//!   Win32_OSRecoveryConfiguration, powercfg /requests).
//! - The disks: Disk Management. "Automatically restart": System
//!   Properties, Advanced. Updates: Windows Update. Notifications: Windows'
//!   notification settings.

/// The window of Windows' own answers: only asks, changes nothing.
pub const ANSWERS: &str = r#"$Host.UI.RawUI.WindowTitle = 'What Windows itself says - Automatic Recovery Probe'
function Say($t) { Write-Host ''; Write-Host $t -ForegroundColor Cyan }
function Line($t) { Write-Host ('   ' + $t) }
Write-Host 'Automatic Recovery Probe: what Windows itself says about the settings the program changes or relies on.'
Write-Host 'This window only asks Windows. It changes nothing.'

Say '1. Defender: the folders and drives it leaves alone (Get-MpPreference, ExclusionPath)'
Line 'While the program works: the image folder (such as C:\IMAGE) and SPARE (such as E:\) are in this list.'
try {
    $x = (Get-MpPreference -ErrorAction Stop).ExclusionPath
    if ($x) { $x | ForEach-Object { Line $_ } } else { Line '(none)' }
} catch { Line ('Defender did not answer: ' + $_.Exception.Message) }

Say '2. New disks: do they arrive switched off? (diskpart, SAN Policy: Offline All = yes, Online All = no)'
try {
    $s = ('san' | diskpart.exe) | Select-String -CaseSensitive -Pattern 'SAN'
    if ($s) { $s | ForEach-Object { Line $_.Line.Trim() } } else { Line 'diskpart did not say.' }
} catch { Line ('diskpart did not answer: ' + $_.Exception.Message) }

Say '3. The disks: switched off (IsOffline) and read-only (IsReadOnly) (Get-Disk)'
Line 'The recovered SSD, while it is plugged in: IsReadOnly True (and IsOffline True when switched off).'
try {
    Get-Disk -ErrorAction Stop | Sort-Object Number |
        Format-Table Number, FriendlyName, @{ n = 'Size (GB)'; e = { [math]::Round($_.Size / 1GB) } }, BusType, IsOffline, IsReadOnly -AutoSize |
        Out-String -Width 160 | Write-Host
} catch { Line ('Get-Disk did not answer: ' + $_.Exception.Message) }

Say '4. After a crash: does Windows restart by itself? (Win32_OSRecoveryConfiguration, AutoReboot)'
Line 'False = it stops at the blue screen, so it cannot restart with the recovered SSD plugged in.'
try {
    Line ('AutoReboot: ' + (Get-CimInstance Win32_OSRecoveryConfiguration -ErrorAction Stop).AutoReboot)
} catch { Line ('Windows did not answer: ' + $_.Exception.Message) }

Say '5. Is Windows waiting to restart for an update?'
$p = (Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending') -or (Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired')
if ($p) { Line 'Yes: restart the PC, with the recovered SSD unplugged, before Start.' } else { Line 'No.' }

Say '6. Kept awake (powercfg /requests): while Start or Finalise runs, the program is listed under SYSTEM'
try { powercfg.exe /requests | ForEach-Object { Line $_ } } catch { Line ('powercfg did not answer: ' + $_.Exception.Message) }

Write-Host ''
[void](Read-Host 'Press Enter to close this window')
"#;

/// What can be shown: (key, what opens).
pub const WHAT: &[(&str, &str)] = &[
    ("defender", "Windows Security, at its list of exclusions"),
    ("answers", "a window of Windows' own answers"),
    ("disks", "Disk Management"),
    ("crash", "System Properties, Advanced"),
    ("update", "Windows Update"),
    ("notifications", "Windows' notification settings"),
];

/// Open it, and say in a sentence what to look at there.
pub fn open(what: &str) -> Result<String, String> {
    if !WHAT.iter().any(|(k, _)| *k == what) {
        return Err(format!("nothing to show called {what:?}"));
    }
    imp::open(what)?;
    Ok(match what {
        "defender" => {
            "Windows Security is open at Exclusions (Windows asks for Yes before it shows the list). While the \
                       program works, the image's folder (such as C:\\IMAGE) and SPARE (such as E:\\) are in it. Look \
                       only: the program takes off what it added when you put Windows back."
        }
        "answers" => "A window with what Windows itself says about each setting is open (it may be behind this one). It \
                      only asks Windows and changes nothing; press Enter in it to close it.",
        "disks" => "Disk Management is open. While it is plugged in, the recovered SSD shows Offline (switched off) or \
                    Read Only. Look only: change nothing there, and never click Online on the recovered SSD.",
        "crash" => "System Properties is open at Advanced. Under Startup and Recovery, click Settings: \"Automatically \
                    restart\" is unticked while the program has it off. Look only, then click Cancel.",
        "update" => "Windows Update is open. If it says a restart is needed, restart the PC with the recovered SSD \
                     unplugged.",
        _ => "Windows' notification settings are open: notifications must be on, and Windows PowerShell allowed (the \
              program's notifications come from it)."
    }
    .to_string())
}

#[cfg(windows)]
mod imp {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn started(r: std::io::Result<std::process::Child>, what: &str) -> Result<(), String> {
        r.map(|_| ()).map_err(|e| format!("could not open {what}: {e}"))
    }

    pub fn open(what: &str) -> Result<(), String> {
        match what {
            "defender" => uri("windowsdefender://exclusions/"),
            "update" => uri("ms-settings:windowsupdate"),
            "notifications" => uri("ms-settings:notifications"),
            "disks" => started(
                Command::new(crate::win::system_program("mmc.exe")).arg(crate::win::system_program("diskmgmt.msc")).spawn(),
                "Disk Management",
            ),
            "crash" => started(Command::new(crate::win::system_program("SystemPropertiesAdvanced.exe")).spawn(), "System Properties"),
            _ => answers(),
        }
    }

    fn uri(u: &str) -> Result<(), String> {
        if crate::win::open_browser(u) {
            Ok(())
        } else {
            Err(format!("Windows would not open {u}"))
        }
    }

    /// In a window of its own (through `start`: a console program started
    /// from this one would otherwise write into this program's window).
    fn answers() -> Result<(), String> {
        let script = std::env::temp_dir().join("arp-what-windows-says.ps1");
        let mut text = vec![0xEF, 0xBB, 0xBF];
        text.extend_from_slice(super::ANSWERS.replace('\n', "\r\n").as_bytes());
        std::fs::write(&script, text).map_err(|e| format!("could not write {}: {e}", script.display()))?;
        let ps = crate::win::system_program(r"WindowsPowerShell\v1.0\powershell.exe");
        started(
            Command::new(crate::win::system_program("cmd.exe"))
                .arg("/c")
                .raw_arg(format!(
                    "start \"What Windows itself says\" \"{}\" -NoProfile -ExecutionPolicy Bypass -File \"{}\"",
                    ps.display(),
                    script.display()
                ))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn(),
            "PowerShell",
        )
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn open(_what: &str) -> Result<(), String> {
        Err("Windows' own pages are only on Windows".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_is_listed_opens() {
        assert!(open("coreisolationreboot").is_err());
        assert!(open("windowsdefender://reboot/").is_err());
        assert!(WHAT.iter().all(|(k, w)| !k.is_empty() && !w.is_empty()));
    }

    /// The window of answers only asks: no command in it changes anything.
    #[test]
    fn the_answers_only_ask() {
        let s = ANSWERS.to_lowercase();
        for change in ["set-", "add-mp", "remove-", "new-", "clear-", "format-volume", "initialize-", "online disk", "attributes"] {
            assert!(!s.contains(change), "{change}");
        }
        assert!(s.contains("get-mppreference") && s.contains("'san' | diskpart") && s.contains("get-disk") && s.contains("powercfg.exe /requests"));
        assert!(ANSWERS.is_ascii());
    }

    /// On Windows: the window's script is good PowerShell.
    #[cfg(windows)]
    #[test]
    fn the_answers_are_good_powershell() {
        let script = std::env::temp_dir().join(format!("arp-answers-test-{}.ps1", std::process::id()));
        std::fs::write(&script, ANSWERS).unwrap();
        let check = format!(
            "$e = $null; [void][System.Management.Automation.Language.Parser]::ParseFile('{}', [ref]$null, [ref]$e); $e.Count",
            script.display().to_string().replace('\'', "''")
        );
        let ps = crate::win::system_program(r"WindowsPowerShell\v1.0\powershell.exe");
        let r = crate::tools::run(&ps, &crate::tools::os(&["-NoProfile", "-NonInteractive", "-Command", &check]), None, std::time::Duration::from_secs(60), &|| false)
            .unwrap();
        let _ = std::fs::remove_file(&script);
        assert_eq!(r.out.trim(), "0", "{}", r.text());
    }
}
