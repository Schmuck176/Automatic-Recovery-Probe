//! The places the program works in - the image's folder, WORK and REPAIR on
//! SPARE, the second copy - as the page shows them beside their icons: the
//! drive each is on and its free space, what goes in and how much of it is
//! there already, whether SPARE is switched off, and whether Defender
//! leaves it alone.
//!
//! Defender: while the program writes hundreds of thousands of files, its
//! scanning makes the writing 2-3 times slower, and it could quarantine a
//! recovered file. Start tells it to leave the image's folder and SPARE
//! alone, by itself, and then reads Defender's own list back to confirm it
//! took; 7. Put Windows back as it was takes those exclusions off again
//! (web/back.rs), once every drive is unplugged. Never Windows' own drive
//! or its system folders, and never a whole drive other than SPARE.

use super::*;

// ---------------------------------------------------------------- Defender

/// Defender's list of folders it leaves alone, read at most once a minute
/// (PowerShell takes a second or two): (when, the list - None when Defender
/// would not say).
static DEFENDER: Mutex<Option<(Instant, Option<Vec<String>>)>> = Mutex::new(None);

pub(super) fn defender_list(fresh: bool) -> Option<Vec<String>> {
    if !cfg!(windows) {
        return None;
    }
    {
        let d = DEFENDER.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((t, l)) = d.as_ref() {
            if !fresh && t.elapsed() < Duration::from_secs(60) {
                return l.clone();
            }
        }
    }
    let l = crate::winset::defender_exclusions();
    *DEFENDER.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), l.clone()));
    l
}

/// "C:\Windows\" -> "c:\windows" (lower case, no trailing '\').
fn norm(p: &str) -> String {
    p.trim().replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// A folder that may be left out of Defender's scanning: a folder, not a
/// whole drive; not Windows' own folders or programs; not the Users folder
/// or a whole profile.
fn may_exclude(folder: &str, system_drive: &str) -> bool {
    let f = norm(folder);
    let b = f.as_bytes();
    if b.len() < 4 || b[1] != b':' || b[2] != b'\\' {
        return false;
    }
    let sys = norm(system_drive);
    let under = |root: &str| f == root || f.starts_with(&format!("{root}\\"));
    if f.starts_with(&sys) {
        for r in ["\\windows", "\\program files", "\\program files (x86)", "\\programdata", "\\$recycle.bin", "\\system volume information"] {
            if under(&format!("{sys}{r}")) {
                return false;
            }
        }
        // C:\Users, or C:\Users\someone itself (a folder inside a profile is fine).
        let users = format!("{sys}\\users");
        if f == users || f.strip_prefix(&format!("{users}\\")).is_some_and(|rest| !rest.contains('\\')) {
            return false;
        }
    }
    true
}

/// The folders Defender should leave alone: the image's folder, and SPARE -
/// the whole drive when WORK is on SPARE (WORK, REPAIR and RECORDS are all
/// there), otherwise WORK and REPAIR themselves.
pub(super) fn defender_targets(image_dir: &str, work: &str, spare_letter: &str, system_drive: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |p: String| {
        if !out.iter().any(|o| norm(o) == norm(&p)) {
            out.push(p);
        }
    };
    let image_dir = image_dir.trim().trim_end_matches('\\');
    if may_exclude(image_dir, system_drive) {
        add(image_dir.to_string());
    }
    let work = work.trim().trim_end_matches('\\');
    let letter = work.get(..2).unwrap_or("").to_uppercase();
    let on_spare = letter.len() == 2
        && letter.ends_with(':')
        && !spare_letter.is_empty()
        && letter.eq_ignore_ascii_case(spare_letter.get(..2).unwrap_or(""))
        && !letter.eq_ignore_ascii_case(system_drive.get(..2).unwrap_or("C:"));
    if on_spare {
        add(format!("{letter}\\"));
    } else if may_exclude(work, system_drive) {
        add(work.to_string());
        if let Ok(r) = crate::repair::repair_dir(Path::new(work)) {
            let r = r.display().to_string();
            if may_exclude(&r, system_drive) {
                add(r);
            }
        }
    }
    out
}

/// SPARE's drive letter: the drive labelled SPARE, or the letter the
/// rehearsal saw it with (it may be switched off now).
fn spare_letter(app: &Arc<Mutex<App>>) -> String {
    crate::win::drives()
        .into_iter()
        .find(|d| d.label.eq_ignore_ascii_case("SPARE"))
        .map(|d| d.letter)
        .unwrap_or_else(|| lock(app).setting("spare_letter"))
}

fn system_drive() -> String {
    std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string()).to_uppercase()
}

/// The folders Defender should leave alone, from the folders chosen (or
/// suggested, before any is chosen).
pub(super) fn targets_now(app: &Arc<Mutex<App>>, image_dir: &str, work: &str) -> Vec<String> {
    defender_targets(image_dir, work, &spare_letter(app), &system_drive())
}

/// Tell Defender to leave the folders alone, then read its list back to
/// confirm it took. What it says goes into Start's lines; nothing here
/// stops Start (it is only slower with Defender scanning).
pub(super) fn defender_ensure(app: &Arc<Mutex<App>>, image_dir: &str, work: &str) -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    if !lock(app).elevated {
        return vec!["Defender was not asked to leave the folders alone: that needs administrator rights.".to_string()];
    }
    let targets = targets_now(app, image_dir, work);
    if targets.is_empty() {
        return Vec::new();
    }
    let Some(list) = defender_list(true) else {
        return vec![
            "Defender did not answer (it may be switched off, or another antivirus is in charge), so nothing was changed there."
                .to_string(),
        ];
    };
    let mut lines = Vec::new();
    let mut added = Vec::new();
    for t in &targets {
        if crate::winset::is_excluded(&list, Path::new(t)) {
            continue;
        }
        match crate::winset::set_defender_exclusion(Path::new(t), true) {
            Ok(()) => added.push(t.clone()),
            Err(e) => lines.push(format!("Defender would not leave {t} alone: {e}")),
        }
    }
    if !added.is_empty() {
        let mut a = lock(app);
        let mut l: Vec<String> = a.setting("defender_added").lines().filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
        for t in &added {
            if !l.iter().any(|x| norm(x) == norm(t)) {
                l.push(t.clone());
            }
        }
        a.settings.insert("defender_added".to_string(), l.join("\n"));
        a.save();
    }
    // Confirmed in Defender's own list.
    let now_list = defender_list(true).unwrap_or_default();
    let confirmed: Vec<&String> = targets.iter().filter(|t| crate::winset::is_excluded(&now_list, Path::new(t.as_str()))).collect();
    {
        let mut a = lock(app);
        let mut v = vec![local_now()];
        v.extend(confirmed.iter().map(|s| s.to_string()));
        a.settings.insert("defender_confirmed".to_string(), v.join("\n"));
        a.save();
    }
    if confirmed.len() == targets.len() {
        let what = targets.join(" and ");
        lines.insert(
            0,
            if added.is_empty() {
                format!("Defender leaves {what} alone (it did already): checked in its own list.")
            } else {
                format!(
                    "Defender now leaves {what} alone: checked in its own list (7. Put Windows back as it was takes this \
                     off again)."
                )
            },
        );
    } else {
        let missing: Vec<&String> = targets.iter().filter(|t| !confirmed.contains(t)).collect();
        lines.push(format!(
            "Defender's list does not show {} left alone, so it scans the files written there (2-3 times slower, and it could \
             quarantine a recovered file). Start carries on.",
            missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" and ")
        ));
    }
    lines
}

/// For the page: the folders Defender should leave alone and which it does.
pub(super) fn defender_json(app: &Arc<Mutex<App>>, image_dir: &str, work: &str, fresh: bool) -> String {
    let targets = targets_now(app, image_dir, work);
    let list = if cfg!(windows) && lock(app).elevated { defender_list(fresh) } else { None };
    let covered: Vec<String> = match &list {
        Some(l) => targets.iter().filter(|t| crate::winset::is_excluded(l, Path::new(t.as_str()))).cloned().collect(),
        None => Vec::new(),
    };
    let (confirmed_at, added) = {
        let a = lock(app);
        (a.setting("defender_confirmed").lines().next().unwrap_or("").to_string(), a.setting("defender_added"))
    };
    let added: Vec<String> = added.lines().filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
    Obj::new()
        .b("answered", list.is_some())
        .raw("targets", &strings(targets.iter()))
        .raw("covered", &strings(covered.iter()))
        .raw("added", &strings(added.iter()))
        .s("confirmed_at", &confirmed_at)
        .end()
}

// ---------------------------------------------------------------- the places

/// The bytes and files in a folder (REPAIR: never many), at most every 20
/// seconds per folder.
static SIZES: Mutex<Vec<(String, Instant, u64, u64)>> = Mutex::new(Vec::new());

fn folder_size(dir: &Path) -> Option<(u64, u64)> {
    if !dir.is_dir() {
        return None;
    }
    let key = dir.display().to_string();
    {
        let s = SIZES.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, t, f, b)) = s.iter().find(|x| x.0 == key) {
            if t.elapsed() < Duration::from_secs(20) {
                return Some((*f, *b));
            }
        }
    }
    let (mut files, mut bytes) = (0u64, 0u64);
    crate::walk::walk_sized(&key, |_, _, fs| {
        files += fs.len() as u64;
        bytes += fs.iter().map(|f| f.1).sum::<u64>();
        true
    });
    let mut s = SIZES.lock().unwrap_or_else(|e| e.into_inner());
    s.retain(|x| x.0 != key);
    s.push((key, Instant::now(), files, bytes));
    Some((files, bytes))
}

/// What the list of copies (rescue-log.csv) adds up to, read again only when
/// it changes: (files, bytes).
static COPIED: Mutex<Option<(PathBuf, Option<std::time::SystemTime>, u64, u64, u64)>> = Mutex::new(None);

fn copied_totals(work: &Path) -> Option<(u64, u64)> {
    let log = work.join(crate::rescue::LOG_NAME);
    let m = std::fs::metadata(&log).ok()?;
    let stamp = m.modified().ok();
    {
        let c = COPIED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((p, t, len, f, b)) = c.as_ref() {
            if *p == log && *t == stamp && *len == m.len() {
                return Some((*f, *b));
            }
        }
    }
    let l = crate::verify::ListTotals::read(work).ok()?;
    let (f, b) = (l.files, l.file_bytes + l.piece_bytes);
    *COPIED.lock().unwrap_or_else(|e| e.into_inner()) = Some((log, stamp, m.len(), f, b));
    Some((f, b))
}

/// SPARE: plugged in and switched on, plugged in but switched off, or not
/// plugged in - found by the identity the rehearsal remembered. Not looked
/// at while a job runs (that is not the moment to open disks): the last
/// answer stands.
static SPARE: Mutex<Option<(String, Option<u32>)>> = Mutex::new(None);

fn spare_state(app: &Arc<Mutex<App>>, running: bool) -> (String, Option<u32>) {
    if !cfg!(windows) {
        return (String::new(), None);
    }
    if running {
        if let Some(s) = SPARE.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return s;
        }
    }
    let id = lock(app).setting("spare_disk");
    if id.is_empty() {
        return (String::new(), None);
    }
    let found = crate::win::disks().into_iter().find(|d| format!("{}|{}", d.model, d.size) == id && !d.system);
    let s = match found {
        Some(d) => match crate::guard::state(d.number) {
            Ok(st) if st.offline => ("off".to_string(), Some(d.number)),
            Ok(_) => ("on".to_string(), Some(d.number)),
            Err(_) => (if d.drives.is_empty() { "off" } else { "on" }.to_string(), Some(d.number)),
        },
        None => ("gone".to_string(), None),
    };
    *SPARE.lock().unwrap_or_else(|e| e.into_inner()) = Some(s.clone());
    s
}

/// For the page (api/places): the drives, and what each place holds and needs.
pub(super) fn places_json(app: &Arc<Mutex<App>>, suggested_image: &str, suggested_work: &str) -> String {
    let (running, image_set, image_dir_set, work_set, copy2, image_sha) = {
        let a = lock(app);
        (
            a.job.as_ref().is_some_and(|j| j.state == "running"),
            a.setting("image"),
            a.setting("auto_image_dir"),
            a.setting("auto_work"),
            a.setting("auto_copy2"),
            a.setting("image_sha"),
        )
    };
    let image_dir = if image_dir_set.trim().is_empty() { suggested_image.to_string() } else { image_dir_set };
    let work = if work_set.trim().is_empty() { suggested_work.to_string() } else { work_set };
    let drives: Vec<String> = crate::win::drives()
        .iter()
        .map(|d| {
            let disk = d.disk.map(|n| n.to_string()).unwrap_or_else(|| "null".to_string());
            Obj::new()
                .s("letter", &d.letter)
                .s("label", &d.label)
                .s("fs", &d.fs)
                .s("kind", d.kind)
                .n("total", d.total)
                .n("free", d.free)
                .raw("disk", &disk)
                .end()
        })
        .collect();
    // The image, as it grows (.partial) and once made.
    let image_file = if !image_set.is_empty() {
        PathBuf::from(&image_set)
    } else {
        Path::new(image_dir.trim()).join(crate::image::IMAGE_NAME)
    };
    let partial = PathBuf::from(format!("{}.partial", image_file.display()));
    let image_bytes = std::fs::metadata(&image_file).or_else(|_| std::fs::metadata(&partial)).map(|m| m.len()).ok();
    let num = |v: Option<u64>| v.map(|n| n.to_string()).unwrap_or_else(|| "null".to_string());
    let work_p = PathBuf::from(work.trim());
    let copied = if work.trim().is_empty() { None } else { copied_totals(&work_p) };
    let repair = if work.trim().is_empty() { None } else { crate::repair::repair_dir(&work_p).ok() };
    let repair_size = repair.as_deref().and_then(folder_size);
    let (spare, spare_disk) = spare_state(app, running);
    Obj::new()
        .raw("drives", &arr(drives))
        .s("system", &system_drive())
        .raw(
            "image",
            &Obj::new()
                .s("dir", image_dir.trim())
                .s("file", &image_file.display().to_string())
                .raw("bytes", &num(image_bytes))
                .b("made", !image_set.is_empty() && !image_sha.is_empty())
                .end(),
        )
        .raw(
            "work",
            &Obj::new()
                .s("dir", work.trim())
                .raw("files", &num(copied.map(|c| c.0)))
                .raw("bytes", &num(copied.map(|c| c.1)))
                .end(),
        )
        .raw(
            "repair",
            &Obj::new()
                .s("dir", &repair.map(|r| r.display().to_string()).unwrap_or_default())
                .raw("files", &num(repair_size.map(|r| r.0)))
                .raw("bytes", &num(repair_size.map(|r| r.1)))
                .end(),
        )
        .raw("copy2", &Obj::new().s("dir", copy2.trim()).end())
        .raw(
            "spare",
            &Obj::new()
                .s("state", &spare)
                .raw("disk", &spare_disk.map(|n| n.to_string()).unwrap_or_else(|| "null".to_string()))
                .s("letter", &spare_letter(app))
                .end(),
        )
        .raw("defender", &defender_json(app, &image_dir, &work, false))
        // (What Start and Finalise would refuse about the folders, said now.)
        .s(
            "problem",
            &super::auto::check_folders(&image_dir, &work, copy2.trim(), !lock(app).test_disks.is_empty()).err().unwrap_or_default(),
        )
        .end()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defender_never_gets_told_to_skip_windows_or_a_whole_drive() {
        let t = |img: &str, work: &str, spare: &str| defender_targets(img, work, spare, "C:");
        // The PC: the image on C:, WORK on SPARE (E:).
        assert_eq!(t(r"C:\IMAGE", r"E:\WORK", "E:"), vec![r"C:\IMAGE".to_string(), r"E:\".to_string()]);
        // SPARE switched off: its letter remembered from the rehearsal.
        assert_eq!(t(r"C:\IMAGE\", r"e:\WORK", "E:"), vec![r"C:\IMAGE".to_string(), r"E:\".to_string()]);
        // WORK not on SPARE: only WORK and REPAIR, never the drive.
        let w = t(r"D:\IMAGE", r"D:\RECOVERY\WORK", "E:");
        assert!(w.contains(&r"D:\IMAGE".to_string()) && w.contains(&r"D:\RECOVERY\WORK".to_string()), "{w:?}");
        assert!(w.iter().all(|x| !x.ends_with(':') && x.len() > 3), "{w:?}");
        // Never Windows' own drive, folders or programs, or a whole profile.
        assert!(t(r"C:\", r"C:\WORK", "C:").iter().all(|x| x != r"C:\" && !x.is_empty()), "the system drive is never SPARE");
        assert_eq!(t(r"C:\Windows\Temp\IMAGE", "", ""), Vec::<String>::new());
        assert_eq!(t(r"C:\Program Files\X", "", ""), Vec::<String>::new());
        assert_eq!(t(r"C:\Users\someone", "", ""), Vec::<String>::new());
        assert_eq!(t(r"C:\Users", "", ""), Vec::<String>::new());
        assert_eq!(t(r"C:\Users\someone\IMAGE", "", ""), vec![r"C:\Users\someone\IMAGE".to_string()]);
        assert_eq!(t(r"D:\", "", ""), Vec::<String>::new());
        assert_eq!(t("", "", ""), Vec::<String>::new());
        // Two of the same are one.
        assert_eq!(t(r"E:\IMAGE", r"E:\WORK", "E:"), vec![r"E:\IMAGE".to_string(), r"E:\".to_string()]);
    }
}
