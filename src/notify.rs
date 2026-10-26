//! Windows notifications: Start tells you when it needs you (plug in the
//! recovered SSD), when it stops on a problem, and when it is done - so it
//! can run while you do something else, or are in another room.
//!
//! Raised by a hidden Windows PowerShell, as one of its own notifications
//! (they stay in the notification centre until dismissed, so one sent
//! while you were away is still there when you come back); where Windows
//! will not take that, as a notification-area balloon instead. Nothing to
//! install and no browser permission to give, and it works with the page's
//! window closed. Do Not Disturb (Focus) holds them back, as it does every
//! notification. The page can turn them off (the setting "notify").

use std::sync::atomic::{AtomicBool, Ordering};

static ON: AtomicBool = AtomicBool::new(true);

/// Notifications on (the default) or off.
pub fn set_on(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

pub fn is_on() -> bool {
    ON.load(Ordering::Relaxed)
}

fn ps_quote(s: &str) -> String {
    // One line, no control characters, PowerShell's single quotes doubled.
    let one: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    format!("'{}'", one.split_whitespace().collect::<Vec<_>>().join(" ").replace('\'', "''"))
}

/// Windows shows at most this much of a notification's text.
fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

/// Windows PowerShell's own name for its notifications (the System32
/// folder's id, then its path there): Windows knows it without any setup.
const POWERSHELL_APP: &str = r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";

/// The PowerShell that shows it, and prints how it did: "toast" (one of
/// PowerShell's own notifications) or "balloon" (an icon in the notification
/// area for `secs`, then taken away).
fn script(title: &str, body: &str, warn: bool, secs: u32) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'; $t = {title}; $b = {body}; \
         try {{ \
         [void][Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]; \
         $x = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
         $e = $x.GetElementsByTagName('text'); \
         [void]$e.Item(0).AppendChild($x.CreateTextNode($t)); \
         [void]$e.Item(1).AppendChild($x.CreateTextNode($b)); \
         $n = [Windows.UI.Notifications.ToastNotification]::new($x); \
         [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier({app}).Show($n); \
         'toast' \
         }} catch {{ \
         Add-Type -AssemblyName System.Windows.Forms; Add-Type -AssemblyName System.Drawing; \
         $i = New-Object System.Windows.Forms.NotifyIcon; \
         $i.Icon = [System.Drawing.SystemIcons]::{icon}; \
         $i.Text = 'Automatic Recovery Probe'; \
         $i.BalloonTipIcon = [System.Windows.Forms.ToolTipIcon]::{tip}; \
         $i.BalloonTipTitle = $t; \
         $i.BalloonTipText = $b; \
         $i.Visible = $true; \
         $i.ShowBalloonTip({ms}); \
         'balloon'; \
         Start-Sleep -Seconds {secs}; \
         $i.Dispose() \
         }}",
        app = ps_quote(POWERSHELL_APP),
        icon = if warn { "Warning" } else { "Information" },
        tip = if warn { "Warning" } else { "Info" },
        title = ps_quote(&cut(title, 60)),
        // (Never empty: Windows will not show a balloon without words.)
        body = ps_quote(&cut(if body.trim().is_empty() { "Automatic Recovery Probe" } else { body }, 240)),
        ms = secs * 1000,
    )
}

/// Show a notification (unless they are turned off). Returns at once: the
/// PowerShell that shows it runs by itself.
pub fn toast(title: &str, body: &str, warn: bool) {
    if !is_on() {
        return;
    }
    let _ = spawn(&script(title, body, warn, 20));
}

#[cfg(windows)]
fn spawn(script: &str) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new(crate::win::system_program(r"WindowsPowerShell\v1.0\powershell.exe"))
        .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
}

#[cfg(not(windows))]
fn spawn(_script: &str) -> std::io::Result<std::process::Child> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "notifications are only shown on Windows"))
}

/// Show one and wait for PowerShell to finish with it: how it was shown
/// ("toast" or "balloon").
pub fn try_now(title: &str, body: &str) -> Result<String, String> {
    #[cfg(windows)]
    {
        let ps = crate::win::system_program(r"WindowsPowerShell\v1.0\powershell.exe");
        let r = crate::tools::run(
            &ps,
            &crate::tools::os(&["-NoProfile", "-NonInteractive", "-Command", &script(title, body, false, 8)]),
            None,
            std::time::Duration::from_secs(60),
            &|| false,
        )
        .map_err(|e| format!("could not start PowerShell: {e}"))?;
        let way = r.out.lines().map(|l| l.trim()).find(|l| *l == "toast" || *l == "balloon").map(|l| l.to_string());
        match way {
            Some(w) if r.code == Some(0) => Ok(w),
            _ => Err(format!("Windows would not show it: {}", r.text().lines().next().unwrap_or("no reason given").trim())),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (title, body);
        Err("notifications are only shown on Windows".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_go_in_safely() {
        let s = script("Your turn", "Plug in the recovered SSD now - the 2 TB one.\nIt's in the drawer.", false, 20);
        assert!(s.contains("$t = 'Your turn';"), "{s}");
        assert!(s.contains("$b = 'Plug in the recovered SSD now - the 2 TB one. It''s in the drawer.';"), "{s}");
        assert!(s.contains(r"CreateToastNotifier('{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe')"), "{s}");
        assert!(s.contains("ShowBalloonTip(20000)") && s.contains("Start-Sleep -Seconds 20") && s.contains("SystemIcons]::Information"));
        assert!(!s.contains('\n'), "one line: {s}");
        let w = script("Start stopped", "", true, 5);
        assert!(w.contains("SystemIcons]::Warning") && w.contains("ToolTipIcon]::Warning") && w.contains("$b = 'Automatic Recovery Probe';"), "{w}");
        // Nothing can end the quoted words early.
        let evil = script("a'; Remove-Item C:\\ -Recurse; '", "b", false, 1);
        assert!(evil.contains("$t = 'a''; Remove-Item C:\\ -Recurse; ''';"), "{evil}");
        assert_eq!(cut(&"x".repeat(300), 240).chars().count(), 240);
    }

    /// On Windows: the script is good PowerShell, and Windows shows it
    /// (where there is a desktop to show it on).
    #[cfg(windows)]
    #[test]
    fn windows_takes_it() {
        let s = script("Automatic Recovery Probe", "A test of its notifications (from its own tests).", false, 1);
        let check = format!(
            "$e = $null; [void][System.Management.Automation.Language.Parser]::ParseInput({}, [ref]$null, [ref]$e); $e.Count",
            ps_quote(&s)
        );
        let ps = crate::win::system_program(r"WindowsPowerShell\v1.0\powershell.exe");
        let r = crate::tools::run(&ps, &crate::tools::os(&["-NoProfile", "-NonInteractive", "-Command", &check]), None, std::time::Duration::from_secs(60), &|| false)
            .unwrap();
        assert_eq!(r.out.trim(), "0", "{}", r.text());
        match try_now("Automatic Recovery Probe", "A test of its notifications (from its own tests).") {
            Ok(way) => assert!(way == "toast" || way == "balloon", "{way}"),
            Err(e) => eprintln!("(not shown here: {e})"),
        }
    }
}
