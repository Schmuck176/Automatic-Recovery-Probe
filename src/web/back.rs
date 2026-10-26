//! Putting Windows back as it was: the settings the program changed to
//! protect the recovered SSD and to work faster - new disks arriving
//! switched off, "Automatically restart" after a crash (if it was turned
//! off here), Defender's exclusions it added - go back only when you click
//! for it (7. Put Windows back as it was), and only once every drive is
//! unplugged: every copy of the old drive (the recovered SSD, the 4 TB SSD)
//! and SPARE. Nothing puts them back by itself any more (not Start, not
//! Finalise, not closing the program): with a drive still plugged in, the
//! recovered SSD could otherwise come back switched on one day.

use super::*;

/// A setting the program changed: what it is, how it is now, how it goes back.
pub(super) struct Change {
    pub key: &'static str,
    pub title: String,
    pub now: String,
    pub back: String,
}

/// The settings the program changed and has not put back yet.
pub(super) fn changes(app: &Arc<Mutex<App>>) -> Vec<Change> {
    let mut v = Vec::new();
    if let Some(p) = crate::cli::policy_to_restore() {
        v.push(Change {
            key: "newdisks",
            title: "New disks arrive switched off".to_string(),
            now: "Windows leaves every newly plugged-in disk switched off: the program set it, so the recovered SSD can't \
                  arrive switched on."
                .to_string(),
            back: format!("Goes back to how it was: {}.", p.describe()),
        });
    }
    let (crash, added) = {
        let a = lock(app);
        (a.setting("crash_restart_changed") == "1", a.setting("defender_added"))
    };
    if crash {
        v.push(Change {
            key: "crash",
            title: "After a crash, Windows stops at the blue screen".to_string(),
            now: "\"Automatically restart\" is off: the program turned it off, so a crash can't restart the PC with the \
                  recovered SSD plugged in."
                .to_string(),
            back: "Goes back to how it was: Windows restarts by itself after a crash.".to_string(),
        });
    }
    let added: Vec<&str> = added.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    if !added.is_empty() {
        v.push(Change {
            key: "defender",
            title: format!("Defender leaves {} alone", added.join(" and ")),
            now: "The program asked Defender to leave these alone while it writes there (faster, and nothing recovered \
                  can be quarantined)."
                .to_string(),
            back: "Goes back to how it was: Defender checks them again. (Folders you had excluded yourself stay as they are.)"
                .to_string(),
        });
    }
    v
}

/// What is still plugged in that must be unplugged first: every copy of the
/// old drive (the recovered SSD, the 4 TB SSD) and SPARE - in words.
pub(super) fn still_plugged(app: &Arc<Mutex<App>>) -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let (map, spare_id, elevated) = {
        let a = lock(app);
        (a.setting("mapfile"), a.setting("spare_disk"), a.elevated)
    };
    let size = mapfile_domain(&map).map(|(s, e)| e.saturating_sub(s));
    let mut out = Vec::new();
    for d in crate::win::disks() {
        // (The image opened by AIM Toolkit is a file opened read-only, not a
        // drive to unplug - and a check stopped half-way can leave it open.)
        if d.system || d.model.to_ascii_lowercase().contains("arsenal") {
            continue;
        }
        let letters: Vec<String> = d.drives.iter().map(|v| v.letter.clone()).collect();
        let at = if letters.is_empty() { format!("disk {}", d.number) } else { format!("disk {}, {}", d.number, letters.join(" ")) };
        let copy = match crate::disk::open_source(&format!(r"\\.\PhysicalDrive{}", d.number)) {
            Ok(src) => {
                let table = crate::gpt::read(&*src, &[]);
                match size {
                    Some(m) => crate::mapcheck::is_the_copy(&*src, &table, m),
                    // (No mapfile: any disk holding an exFAT volume might be one.)
                    None => crate::mapcheck::find_volume_quick(&*src, &table).is_ok(),
                }
            }
            // A disk that holds something but does not let itself be read
            // could be the recovered SSD. (An empty card reader holds
            // nothing; and without administrator rights no disk can be read.)
            Err(_) if elevated && d.size > 0 => {
                out.push(format!("A disk the program cannot read ({at}): it could be the recovered SSD"));
                continue;
            }
            Err(_) => false,
        };
        if copy {
            out.push(format!("A copy of the old drive: the recovered SSD or the 4 TB SSD ({at})"));
        } else if (!spare_id.is_empty() && format!("{}|{}", d.model, d.size) == spare_id)
            || d.drives.iter().any(|v| v.label.eq_ignore_ascii_case("SPARE"))
        {
            out.push(format!("SPARE ({at})"));
        }
    }
    out
}

fn running_title(app: &Arc<Mutex<App>>) -> Option<String> {
    lock(app).job.as_ref().filter(|j| j.state == "running").map(|j| j.title.clone())
}

/// For the page (api/windows-back): what goes back, and what must be
/// unplugged first (not looked at while a job runs).
pub(super) fn back_json(app: &Arc<Mutex<App>>) -> String {
    let changes = changes(app);
    let running = running_title(app);
    let plugged = if changes.is_empty() || running.is_some() { Vec::new() } else { still_plugged(app) };
    let rows: Vec<String> = changes
        .iter()
        .map(|c| Obj::new().s("key", c.key).s("title", &c.title).s("now", &c.now).s("back", &c.back).end())
        .collect();
    let done_at = lock(app).setting("windows_back_at");
    Obj::new()
        .raw("changes", &arr(rows))
        .raw("plugged", &arr(plugged.iter().map(|p| json_str(p)).collect()))
        .s("running", running.as_deref().unwrap_or(""))
        .b("windows", cfg!(windows))
        .s("done_at", &done_at)
        .end()
}

/// "a, b and c".
fn and_list(v: &[String]) -> String {
    match v {
        [] => String::new(),
        [a] => a.clone(),
        _ => format!("{} and {}", v[..v.len() - 1].join(", "), v[v.len() - 1]),
    }
}

/// Put Windows back as it was - only with every drive unplugged.
pub(super) fn put_back(app: &Arc<Mutex<App>>) -> Result<String, String> {
    // (Nothing changed: nothing to do, and no administrator rights needed to
    // say so.)
    if changes(app).is_empty() {
        return Ok("Nothing to put back: Windows is as it was.".to_string());
    }
    need_admin(app)?;
    if let Some(t) = running_title(app) {
        return Err(format!("\"{t}\" is still running: put Windows back once it has finished."));
    }
    let plugged = still_plugged(app);
    if !plugged.is_empty() {
        return Err(format!(
            "Still plugged in: {}. Switch {} off and unplug {} first. Windows is put back as it was only once every drive \
             is unplugged, so the recovered SSD can never arrive switched on.",
            and_list(&plugged),
            if plugged.len() == 1 { "it" } else { "them" },
            if plugged.len() == 1 { "it" } else { "them" }
        ));
    }
    let mut said = Vec::new();
    let mut failed = Vec::new();
    if crate::cli::policy_to_restore().is_some() {
        match crate::cli::guard_restore() {
            Ok(m) => said.push(format!("New disks: {m}")),
            Err(e) => failed.push(format!("New disks: could not be put back ({e})")),
        }
    }
    if lock(app).setting("crash_restart_changed") == "1" {
        match crate::winset::set_restarts_after_crash(true) {
            Ok(()) => {
                said.push("After a crash: Windows restarts by itself again, as it did before.".to_string());
                let mut a = lock(app);
                a.settings.remove("crash_restart_changed");
                a.save();
            }
            Err(e) => failed.push(format!("\"Automatically restart\" could not be put back: {e}")),
        }
    }
    let added: Vec<String> = lock(app).setting("defender_added").lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if !added.is_empty() {
        let mut left = Vec::new();
        for f in &added {
            match crate::winset::set_defender_exclusion(Path::new(f), false) {
                Ok(()) => said.push(format!("Defender: checks {f} again, as before.")),
                Err(e) => {
                    failed.push(format!("Defender's exclusion of {f} could not be taken off: {e}"));
                    left.push(f.clone());
                }
            }
        }
        let mut a = lock(app);
        a.settings.insert("defender_added".to_string(), left.join("\n"));
        a.save();
        drop(a);
        // (Defender's list, as the page shows it, read afresh.)
        let _ = places::defender_list(true);
    }
    if said.is_empty() && failed.is_empty() {
        return Ok("Nothing to put back: Windows is as it was.".to_string());
    }
    if failed.is_empty() {
        let mut a = lock(app);
        a.settings.insert("windows_back_at".to_string(), local_now());
        a.save();
        drop(a);
        crate::live::event("good", "Windows is put back as it was (the drives were all unplugged)");
        Ok(said.join("\n"))
    } else {
        crate::live::event("bad", &format!("Windows could not all be put back: {}", failed.join("; ")));
        Err(said.into_iter().chain(failed).collect::<Vec<_>>().join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drives_are_named_in_a_list() {
        assert_eq!(and_list(&[]), "");
        assert_eq!(and_list(&["SPARE (disk 4, E:)".to_string()]), "SPARE (disk 4, E:)");
        assert_eq!(and_list(&["a".to_string(), "b".to_string(), "c".to_string()]), "a, b and c");
    }
}
