//! Finalise: keeping it safe (Phase 8 of the old checklist), by itself.
//!
//! 1. RECORDS, beside WORK on SPARE (not inside it: anything new in WORK
//!    shows as added in the safety check): the results folder, the mapfile,
//!    the checklists. In three years you'll want to know what was checked.
//! 2. WORK checked first: every file read back and compared with its
//!    checksum, and the safety check against Your numbers - before anything
//!    is copied from it, so a file changed in WORK since it was copied
//!    (opened and saved, say) never spreads over the second copy.
//! 3. COPY 2, in the second-copy folder (beside the image): WORK (with
//!    rescue-log.csv and repair-returns.csv), REPAIR (the damaged originals,
//!    the salvaged files) and RECORDS, with every file's dates. Copied in a
//!    way that carries on where it stopped. A file already in COPY 2 is
//!    replaced only when WORK has just passed its check; otherwise COPY 2 is
//!    only added to, so it keeps the files as they were checked.
//! 4. COPY 2's WORK checked the same way.
//! 5. The image read back against its checksum.
//!
//! Windows' settings are not touched here: they go back separately, when the
//! person clicks for it with every drive unplugged (back.rs).

use super::*;

const TEMP_NAME: &str = "~arp-copying.tmp";

/// What a copy_tree did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Copied {
    /// Files copied this time, and their bytes.
    pub files: u64,
    pub bytes: u64,
    /// Of those, files that were there already, different, and replaced.
    pub replaced: u64,
    /// Files there already, different, and left as they were (`overwrite`
    /// off).
    pub kept: u64,
}

/// Copy a folder's files into another, with their dates; a file already
/// there with the same size and modified time is left as it is (so a copying
/// that was stopped carries on), unless `always` says otherwise: a repaired
/// version put back into WORK has its original's size (an intact twin) and
/// dates, so only its contents tell it from the damaged copy. A file there
/// already that differs is replaced only with `overwrite`; without it, what
/// is there stays as it is (only what is missing is added).
#[allow(clippy::too_many_arguments)]
pub(super) fn copy_tree(
    from: &Path,
    to: &Path,
    progress: &mut dyn FnMut(u64, u64),
    stop: &dyn Fn() -> bool,
    total: u64,
    done: &mut u64,
    always: &dyn Fn(&Path) -> bool,
    overwrite: bool,
) -> Result<Copied, String> {
    let mut c = Copied::default();
    let src = crate::rescue::verbatim(from);
    let dst = crate::rescue::verbatim(to);
    std::fs::create_dir_all(&dst).map_err(|e| format!("could not make {}: {e}", to.display()))?;
    let mut entries: Vec<std::fs::DirEntry> =
        std::fs::read_dir(&src).map_err(|e| format!("could not read {}: {e}", from.display()))?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if stop() {
            return Ok(c);
        }
        let name = e.file_name();
        let s = e.path();
        let d = dst.join(&name);
        let ft = e.file_type().map_err(|err| format!("could not read {}: {err}", s.display()))?;
        if ft.is_dir() {
            let sub = copy_tree(&s, &d, progress, stop, total, done, always, overwrite)?;
            c.files += sub.files;
            c.bytes += sub.bytes;
            c.replaced += sub.replaced;
            c.kept += sub.kept;
            if let Ok(m) = std::fs::metadata(&s) {
                set_times(&d, &m);
            }
            continue;
        }
        if name.to_string_lossy() == TEMP_NAME {
            continue;
        }
        let m = std::fs::metadata(&s).map_err(|err| format!("could not read {}: {err}", s.display()))?;
        let shown = s.to_string_lossy().into_owned();
        crate::live::now(&shown);
        let mut replacing = false;
        if let Ok(dm) = std::fs::metadata(&d) {
            let looks_same = dm.len() == m.len() && dm.modified().ok() == m.modified().ok();
            // (Not replacing anything: no need to read both to compare.)
            let same = looks_same && (!overwrite || !always(&s) || same_contents(&s, &d));
            if same || !overwrite {
                if !same {
                    c.kept += 1;
                }
                *done += m.len();
                progress(*done, total);
                crate::live::done(&shown, m.len());
                continue;
            }
            replacing = true;
        }
        let tmp = dst.join(TEMP_NAME);
        let _ = std::fs::remove_file(&tmp);
        std::fs::copy(&s, &tmp).map_err(|err| format!("could not copy {}: {err}", s.display()))?;
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&tmp) {
            let _ = f.sync_all();
        }
        let _ = std::fs::remove_file(&d);
        std::fs::rename(&tmp, &d).map_err(|err| format!("could not put {} in place: {err}", d.display()))?;
        set_times(&d, &m);
        c.files += 1;
        c.bytes += m.len();
        if replacing {
            c.replaced += 1;
        }
        *done += m.len();
        progress(*done, total);
        crate::live::done(&shown, m.len());
    }
    Ok(c)
}

/// The same bytes in both files.
fn same_contents(a: &Path, b: &Path) -> bool {
    let (Ok(x), Ok(y)) = (crate::sha256::file_hex(a, &|| false), crate::sha256::file_hex(b, &|| false)) else { return false };
    x.is_some() && x == y
}

fn set_times(p: &Path, m: &std::fs::Metadata) {
    let mut t = std::fs::FileTimes::new();
    if let Ok(x) = m.modified() {
        t = t.set_modified(x);
    }
    if let Ok(x) = m.accessed() {
        t = t.set_accessed(x);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTimesExt;
        if let Ok(c) = m.created() {
            t = t.set_created(c);
        }
    }
    let f = if p.is_dir() {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::OpenOptions::new().write(true).custom_flags(0x0200_0000).open(p) // FILE_FLAG_BACKUP_SEMANTICS
        }
        #[cfg(not(windows))]
        {
            std::fs::File::open(p)
        }
    } else {
        std::fs::OpenOptions::new().write(true).open(p)
    };
    if let Ok(f) = f {
        let _ = f.set_times(t);
    }
}

fn tree_bytes(p: &Path) -> u64 {
    tree_bytes_counted(p, false)
}

/// The bytes in a folder; with `count`, each file is also counted as one
/// to do for the page (by kind and size).
fn tree_bytes_counted(p: &Path, count: bool) -> u64 {
    let mut n = 0u64;
    crate::walk::walk_sized(&p.display().to_string(), |_, _, files| {
        for (name, size) in files {
            n += size;
            if count && name != TEMP_NAME {
                crate::live::to_do(name, *size);
            }
        }
        true
    });
    n
}

pub(super) fn start(app: &Arc<Mutex<App>>) -> Result<u64, String> {
    let plan = auto::plan_of(app)?;
    auto::check_folders(&plan.image_dir, &plan.work, &plan.copy2, !lock(app).test_disks.is_empty())?;
    check_destination(app, &plan.work)?;
    if !plan.copy2.trim().is_empty() {
        check_destination(app, &plan.copy2)?;
        // FAT32 holds no file of 4 GB or more (a video, a Photos library's),
        // and keeps a file's time only to 2 seconds: no second copy there.
        if let Some(d) = drive_of(&plan.copy2).filter(|d| d.fs.eq_ignore_ascii_case("FAT32") || d.fs.eq_ignore_ascii_case("FAT")) {
            return Err(format!(
                "{} is formatted {}, which cannot hold a file of 4 GB or more, nor keep the files' dates exactly: the second \
                 copy needs a drive formatted NTFS (or exFAT). Choose another drive for it (3. The folders).",
                d.letter, d.fs
            ));
        }
    }
    let work = PathBuf::from(&plan.work);
    if !work.join(crate::rescue::LOG_NAME).is_file() {
        return Err(format!("{} has no {}: Start has not copied everything yet", plan.work, crate::rescue::LOG_NAME));
    }
    let app2 = app.clone();
    start_job(app, "finalise", "Finalising: the second copy and the final checks", move |ctx| {
        crate::winset::keep_awake(true);
        let r = run(&app2, ctx, &plan);
        crate::winset::keep_awake(false);
        r
    })
}

fn run(app: &Arc<Mutex<App>>, ctx: &Ctx, plan: &auto::Plan) -> Result<Outcome, String> {
    let work = PathBuf::from(&plan.work);
    let base = work.parent().map(|p| p.to_path_buf()).ok_or("WORK must be a folder, not a whole drive")?;
    let repair = crate::repair::repair_dir(&work)?;
    let (home, results, map) = {
        let a = lock(app);
        (a.home.clone(), a.results.clone(), a.setting("mapfile"))
    };
    let mut summary = Vec::new();
    let mut problem = None;
    // 1. RECORDS.
    let records = base.join("RECORDS");
    ctx.line(&format!("- RECORDS: the results, the mapfile and the checklists, into {}", records.display()));
    let mut none = |_: u64, _: u64| {};
    let mut d = 0u64;
    copy_tree(&results, &records.join("results"), &mut none, &|| ctx.stopping(), 0, &mut d, &|_| false, true)?;
    if Path::new(&map).is_file() {
        let _ = std::fs::copy(&map, records.join(Path::new(&map).file_name().unwrap_or_default()));
    }
    for name in [crate::checklist::COPY_NAME, "recovery-checklist.md", crate::checklist::MANIFEST_NAME, "download-manifest.md"] {
        if let Some(p) = find_named(&home, name) {
            let _ = std::fs::copy(&p, records.join(name));
        }
    }
    summary.push(format!("RECORDS is up to date: {}", records.display()));
    if ctx.stopping() {
        return Ok(Outcome { summary, stopped: true, ..Default::default() });
    }
    // 2. WORK, checked before anything is copied from it.
    let w = auto::step(app, ctx, prepare_verify(app, &work.display().to_string())?)?;
    if w.stopped {
        return Ok(Outcome { summary, stopped: true, ..Default::default() });
    }
    let work_ok = w.problem.is_none();
    summary.push(format!("{}: {}", work.display(), w.summary.first().cloned().unwrap_or_default()));
    if let Some(p) = w.problem {
        problem = Some(format!("{}: {p}", work.display()));
        summary.extend(w.summary.into_iter().skip(1).take(6));
    }
    // 3. COPY 2.
    let copy2 = plan.copy2.trim().to_string();
    let mut copy2_work = None;
    if copy2.is_empty() {
        summary.push("No second-copy folder was chosen (3. The folders), so there is no second copy yet.".to_string());
    } else {
        let dest = PathBuf::from(&copy2);
        let parts: Vec<(PathBuf, PathBuf)> = [(&work, "WORK"), (&repair, crate::repair::REPAIR_DIR), (&records, "RECORDS")]
            .iter()
            .filter(|(p, _)| p.is_dir())
            .map(|(p, n)| (p.to_path_buf(), dest.join(n)))
            .collect();
        ctx.line("- Adding up what goes into the second copy...");
        // (The page shows files copied of all there are, kind by kind.)
        crate::live::begin("in the second copy");
        let total: u64 = parts.iter().map(|(p, _)| tree_bytes_counted(p, true)).sum();
        let there: u64 = parts.iter().map(|(_, q)| if q.is_dir() { tree_bytes(q) } else { 0 }).sum();
        let _ = std::fs::create_dir_all(&dest);
        if let Some(free) = crate::win::free_space(&dest) {
            if free + there < total + (1 << 30) {
                return Ok(Outcome {
                    summary: vec![format!(
                        "The second copy needs {}, and {} has {} free: choose another folder for it (3. The folders), or ask \
                         Claude.",
                        crate::fmt::size(total),
                        copy2,
                        crate::fmt::size(free)
                    )],
                    problem: Some("not enough room for the second copy".to_string()),
                    ..Default::default()
                });
            }
        }
        ctx.line(&format!("- The second copy: {} into {copy2}", crate::fmt::size(total)));
        let mut done = 0u64;
        let mut copied = Copied::default();
        // Every file put back into WORK (or put back and undone): compared by
        // its contents, not by its size and dates, which are its original's.
        let returned: std::collections::HashSet<String> = crate::repair::returned_now(&work).into_keys().collect();
        let in_work = |p: &Path| {
            crate::repair::relative_to(&work, &p.display().to_string().replace('/', "\\"))
                .is_some_and(|r| returned.contains(&r.to_lowercase()))
        };
        for (from, to) in &parts {
            let always: &dyn Fn(&Path) -> bool = if from == &work { &in_work } else { &|_| false };
            let c = copy_tree(from, to, &mut |d, t| ctx.amount(d, t), &|| ctx.stopping(), total, &mut done, always, work_ok)?;
            copied.files += c.files;
            copied.bytes += c.bytes;
            copied.replaced += c.replaced;
            copied.kept += c.kept;
            if ctx.stopping() {
                summary.push("Stopped while making the second copy: Finalise again carries on where it stopped.".to_string());
                return Ok(Outcome { summary, stopped: true, ..Default::default() });
            }
        }
        summary.push(format!(
            "The second copy is in {copy2}: {} files ({}) copied this time, the rest were there already.",
            crate::fmt::commas(copied.files),
            crate::fmt::size(copied.bytes)
        ));
        if copied.replaced > 0 {
            summary.push(format!(
                "{} files in the second copy were not as WORK holds them (changed there since the last Finalise?): they were \
                 copied again from WORK, which had just passed its check. The second copy is a copy to keep, not to work in.",
                crate::fmt::commas(copied.replaced)
            ));
        }
        if !work_ok {
            summary.push(format!(
                "WORK is not as it was copied, so nothing already in the second copy was replaced{}: it still holds the files \
                 as they were checked. Ask Claude before anything else.",
                if copied.kept > 0 { format!(" ({} files there differ from WORK's)", crate::fmt::commas(copied.kept)) } else { String::new() }
            ));
        }
        copy2_work = Some(dest.join("WORK"));
    }
    // 4. The check of the second copy.
    if let Some(folder) = copy2_work.clone() {
        let o = auto::step(app, ctx, prepare_verify(app, &folder.display().to_string())?)?;
        if o.stopped {
            return Ok(Outcome { summary, stopped: true, ..Default::default() });
        }
        summary.push(format!("{}: {}", folder.display(), o.summary.first().cloned().unwrap_or_default()));
        if let Some(p) = o.problem {
            let this = format!("{}: {p}", folder.display());
            problem = Some(match problem.take() {
                Some(w) => format!("{w}; {this}"),
                None => this,
            });
            summary.extend(o.summary.into_iter().skip(1).take(6));
        }
    }
    // 5. The image.
    let mut image_checked = false;
    if !lock(app).setting("image").is_empty() {
        match prepare_check_image(app) {
            Ok(p) => {
                let o = auto::step(app, ctx, p)?;
                if o.stopped {
                    return Ok(Outcome { summary, stopped: true, ..Default::default() });
                }
                summary.push(o.summary.first().cloned().unwrap_or_default());
                image_checked = o.problem.is_none();
                if let Some(p) = o.problem {
                    problem.get_or_insert(p);
                }
            }
            Err(e) => summary.push(format!("The image was not checked: {e}")),
        }
    }
    // Windows' settings stay as they are: they go back only when the person
    // clicks for it, with every drive unplugged (back.rs).
    if !back::changes(app).is_empty() {
        summary.push(
            "Windows' settings the program changed stay as they are for now: put them back in 7. Put Windows back as it \
             was, once the recovered SSD, the 4 TB SSD and SPARE are all unplugged."
                .to_string(),
        );
    }
    if problem.is_none() {
        summary.insert(
            0,
            format!(
                "Finalised: {} every file exactly as copied (or as repaired and put back){}",
                if copy2_work.is_some() { "both copies hold" } else { "WORK holds (there is no second copy yet)" },
                if image_checked { ", and the image is as it was made." } else { ". (The image was not checked: see below.)" }
            ),
        );
        summary.push(
            "Now: keep SPARE somewhere else (a relative's, work), and twice a year plug it in and click Finalise again: it \
             checks both copies and the image, and names any file that has changed."
                .to_string(),
        );
    }
    let mut a = lock(app);
    a.settings.insert("finalised".to_string(), local_now());
    a.save();
    Ok(Outcome { summary, problem, ..Default::default() })
}

/// The newest file of this name in the program's folder (two folders down).
fn find_named(home: &Path, name: &str) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut look = |dir: &Path| {
        let p = dir.join(name);
        if let Ok(m) = std::fs::metadata(&p) {
            let t = m.modified().unwrap_or(std::time::UNIX_EPOCH);
            if best.as_ref().is_none_or(|b| t > b.0) {
                best = Some((t, p));
            }
        }
    };
    look(home);
    if let Ok(rd) = std::fs::read_dir(home) {
        for e in rd.flatten() {
            if e.path().is_dir() && e.file_name() != "results" {
                look(&e.path());
            }
        }
    }
    best.map(|b| b.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_second_copy_carries_on_and_keeps_dates() {
        let base = std::env::temp_dir().join(format!("arp-copy2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let a = base.join("WORK");
        std::fs::create_dir_all(a.join("Pics").join("2019")).unwrap();
        std::fs::write(a.join("Pics").join("2019").join("a.jpg"), vec![1u8; 5000]).unwrap();
        std::fs::write(a.join("b.txt"), b"hello").unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000);
        std::fs::File::options().write(true).open(a.join("b.txt")).unwrap().set_modified(old).unwrap();
        let b = base.join("COPY 2").join("WORK");
        let copy = |always: &dyn Fn(&Path) -> bool, overwrite: bool| {
            let mut done = 0;
            let c = copy_tree(&a, &b, &mut |_, _| {}, &|| false, 5005, &mut done, always, overwrite).unwrap();
            (c, done)
        };
        let (c, _) = copy(&|_| false, true);
        assert_eq!((c.files, c.bytes, c.replaced), (2, 5005, 0));
        assert_eq!(std::fs::read(b.join("b.txt")).unwrap(), b"hello");
        assert_eq!(std::fs::metadata(b.join("b.txt")).unwrap().modified().unwrap(), old);
        // Again: nothing to copy.
        let (c, done) = copy(&|_| false, true);
        assert_eq!(c.files, 0);
        assert_eq!(done, 5005);
        // A repaired version put back in place of b.txt: the same size and
        // dates as before, so only a file named as put back is compared, and
        // copied again.
        std::fs::write(a.join("b.txt"), b"HELLO").unwrap();
        std::fs::File::options().write(true).open(a.join("b.txt")).unwrap().set_modified(old).unwrap();
        let (c, _) = copy(&|_| false, true);
        assert_eq!(c.files, 0, "size and dates alone can't tell");
        let put_back = |p: &Path| p.file_name().is_some_and(|n| n == "b.txt");
        let (c, _) = copy(&put_back, true);
        assert_eq!((c.files, c.replaced), (1, 1));
        assert_eq!(std::fs::read(b.join("b.txt")).unwrap(), b"HELLO");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// WORK not as it was copied (a file opened and saved there): the second
    /// copy is only added to - what is in it already stays as it is.
    #[test]
    fn a_changed_work_never_spreads_over_the_second_copy() {
        let base = std::env::temp_dir().join(format!("arp-copy2-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let a = base.join("WORK");
        let b = base.join("COPY 2").join("WORK");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(a.join("photo.jpg"), b"as copied").unwrap();
        let mut done = 0;
        copy_tree(&a, &b, &mut |_, _| {}, &|| false, 0, &mut done, &|_| false, true).unwrap();
        // Changed in WORK (another size and date), and a new file there.
        std::fs::write(a.join("photo.jpg"), b"rotated and saved by an app").unwrap();
        std::fs::write(a.join("new.txt"), b"new").unwrap();
        let c = copy_tree(&a, &b, &mut |_, _| {}, &|| false, 0, &mut done, &|_| false, false).unwrap();
        assert_eq!((c.files, c.replaced, c.kept), (1, 0, 1));
        assert_eq!(std::fs::read(b.join("photo.jpg")).unwrap(), b"as copied", "the second copy keeps it as checked");
        assert_eq!(std::fs::read(b.join("new.txt")).unwrap(), b"new", "what is missing is still added");
        // With WORK checked and fine, a second copy that differs is put right.
        std::fs::write(a.join("photo.jpg"), b"as copied").unwrap();
        std::fs::write(b.join("photo.jpg"), b"changed in the second copy").unwrap();
        let c = copy_tree(&a, &b, &mut |_, _| {}, &|| false, 0, &mut done, &|_| false, true).unwrap();
        assert_eq!((c.files, c.replaced, c.kept), (1, 1, 0));
        assert_eq!(std::fs::read(b.join("photo.jpg")).unwrap(), b"as copied");
        let _ = std::fs::remove_dir_all(&base);
    }
}
