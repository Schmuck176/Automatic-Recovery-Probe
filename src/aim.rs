//! Opening the image read-only with AIM Toolkit's command line (Arsenal Image
//! Mounter's aim_cli), so that the map check can check its own reading of the
//! volume against Windows' (mapcheck.rs, Compare::Auto finds the drive by the
//! volume's serial number), and closing it again.
//!
//! `aim_cli --mount --readonly --online --filename=<image> --background`
//! mounts the image as a read-only disk and brings it online (Windows'
//! setting that leaves new disks switched off would otherwise keep it from
//! getting a drive letter; read-only, it can't be changed); a copy of aim_cli
//! keeps serving the image in the background until
//! `aim_cli --dismount=<device number>`. Arsenal Image Mounter opens a raw
//! image so that other programs can still read it, which the map check
//! needs. The image is also marked read-only in Windows (image.rs), whatever
//! AIM does.

use std::path::Path;
use std::time::Duration;

/// What aim_cli said when it mounted the image.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mounted {
    /// "000001": what --dismount= takes.
    pub device: String,
    /// "\\?\PhysicalDrive3", if it said.
    pub disk: String,
    /// "R:\" and the like.
    pub letters: Vec<String>,
    pub read_only: bool,
}

/// Read aim_cli's answer to --mount.
pub fn parse_mount(text: &str) -> Option<Mounted> {
    let mut m = Mounted::default();
    for l in text.lines().map(|l| l.trim()) {
        if let Some(n) = l.strip_prefix("Device number ") {
            m.device = n.trim().to_string();
        } else if let Some(n) = l.strip_prefix("Device is ") {
            m.disk = n.trim().to_string();
        } else if let Some(n) = l.strip_prefix("Mounted at ") {
            m.letters.push(n.trim().to_string());
        } else if l == "Mounted read only" {
            m.read_only = true;
        } else if let Some(i) = l.find("--dismount=") {
            if m.device.is_empty() {
                m.device = l[i + "--dismount=".len()..].trim().to_string();
            }
        }
    }
    (!m.device.is_empty()).then_some(m)
}

/// Mount `image` read-only with aim_cli.
pub fn mount_read_only(exe: &Path, image: &Path, stop: &dyn Fn() -> bool) -> Result<Mounted, String> {
    let mut f = std::ffi::OsString::from("--filename=");
    f.push(image.as_os_str());
    let r = crate::tools::run(
        exe,
        &[
            std::ffi::OsString::from("--mount"),
            std::ffi::OsString::from("--readonly"),
            std::ffi::OsString::from("--online"),
            f,
            std::ffi::OsString::from("--background"),
        ],
        None,
        Duration::from_secs(180),
        stop,
    )
    .map_err(|e| format!("could not start AIM Toolkit's aim_cli: {e}"))?;
    let text = r.text();
    match parse_mount(&text) {
        Some(m) if m.read_only || !text.contains("Mounted writable") => Ok(m),
        Some(m) => {
            // Never leave it mounted writable.
            let _ = dismount(exe, &m.device);
            Err("AIM mounted the image writable, not read-only, so it was closed again at once".to_string())
        }
        None => Err(format!(
            "AIM Toolkit could not open the image: {}",
            text.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("no reason given").trim()
        )),
    }
}

/// Close it again.
pub fn dismount(exe: &Path, device: &str) -> Result<(), String> {
    let r = crate::tools::run(
        exe,
        &[std::ffi::OsString::from(format!("--dismount={device}")), std::ffi::OsString::from("--force")],
        None,
        Duration::from_secs(120),
        &|| false,
    )
    .map_err(|e| format!("could not start aim_cli: {e}"))?;
    if r.code == Some(0) {
        Ok(())
    } else {
        Err(format!("AIM Toolkit could not close it: {}", r.text().lines().last().unwrap_or("").trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aim_clis_answer_is_read() {
        let out = "Opening image file 'D:\\IMAGE\\recovered-ssd.img'...\r\nImage file size is 931.5 GB\r\nMounting as virtual disk...\r\n\
                   Device number 000002\r\nDevice is \\\\?\\PhysicalDrive3\r\nMounted offline\r\nMounted read only\r\n\
                   Setting device policy to online\r\n\r\nContains volume \\\\?\\Volume{1234}\\\r\n  Mounted at R:\\\r\n\r\n\
                   Virtual disk mounted. To dismount, type aim_cli --dismount=000002\r\n";
        let m = parse_mount(out).unwrap();
        assert_eq!(m.device, "000002");
        assert_eq!(m.disk, "\\\\?\\PhysicalDrive3");
        assert_eq!(m.letters, vec!["R:\\".to_string()]);
        assert!(m.read_only);
        assert_eq!(parse_mount("Cannot access Arsenal Image Mounter driver."), None);
    }
}
