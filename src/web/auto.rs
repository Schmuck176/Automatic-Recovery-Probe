//! Start: the whole recovery, by itself, as one job of the page.
//!
//! The stages are the program's own steps (each one exactly as tested in
//! Recovery Probe: the same code, through the same "prepare" functions as
//! the step buttons), chained, with the open-source checks and repairs
//! between them:
//!
//!   ready -> image -> open (AIM) -> mapcheck -> copy -> verify -> scan ->
//!   extra -> isolate -> repair -> putback -> verify2 -> scan2 -> evaluate ->
//!   handoff
//!
//! Each stage's result also goes where the step's own button would have put
//! it (the steps tab shows it, the results folder keeps it). Start carries on
//! from the first stage not done yet, so stopping, closing the program or a
//! power cut loses nothing: every step underneath carries on where it
//! stopped. Where the person must do something - plug in the recovered SSD,
//! unplug it, plug in SPARE - the job waits, says so, and goes on by itself
//! once it sees it done. Any check that finds something stops it, and says
//! why and what to do, as the step's own button would.

use super::*;
use crate::autorepair;
use crate::extra;
use crate::handoff;

pub(super) const STAGES: &[(&str, &str)] = &[
    ("ready", "Checking everything is ready"),
    ("image", "Making the image of the recovered SSD"),
    ("open", "Opening the image read-only, for the check against Windows"),
    ("mapcheck", "Finding what the damage touched"),
    ("copy", "Copying everything into WORK"),
    ("verify", "Checking the copies are exactly what was copied"),
    ("scan", "Checking every copied file"),
    ("extra", "The extra checks: 7-Zip, MP3val, qpdf, FFmpeg"),
    ("isolate", "Copying the damaged files out to repair"),
    ("repair", "Repairing them with the open-source tools"),
    ("putback", "Putting the repaired files back into WORK"),
    ("verify2", "Checking the copies again"),
    ("scan2", "Checking every file again"),
    ("evaluate", "Evaluating what is still damaged"),
    ("handoff", "Getting the rest ready for the Mac or a paid tool"),
];

#[derive(Clone, Debug, Default)]
pub(super) struct Stage {
    pub key: String,
    /// pending, running, waiting, done, skipped, failed, stopped
    pub state: String,
    pub summary: Vec<String>,
    pub started_at: String,
    pub seconds: u64,
}

#[derive(Clone, Debug, Default)]
pub(super) struct State {
    pub stages: Vec<Stage>,
    /// When the stage running now began (not kept: for the page's clock).
    pub stage_began: Option<Instant>,
    /// What the job waits for the person to do, now.
    pub waiting: String,
    /// Something the person may do now (unplug the SSD).
    pub banner: String,
}

const SEP_FIELD: char = '\u{1f}';
const SEP_LINE: char = '\u{1e}';

impl State {
    pub fn new() -> State {
        State {
            stages: STAGES.iter().map(|(k, _)| Stage { key: k.to_string(), state: "pending".to_string(), ..Default::default() }).collect(),
            ..Default::default()
        }
    }

    /// As kept in the settings ("auto_state").
    pub fn text(&self) -> String {
        self.stages
            .iter()
            .map(|s| {
                format!(
                    "{}{SEP_FIELD}{}{SEP_FIELD}{}{SEP_FIELD}{}{SEP_FIELD}{}",
                    s.key,
                    s.state,
                    s.started_at,
                    s.seconds,
                    s.summary.join(&SEP_LINE.to_string())
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn from_text(t: &str) -> State {
        let mut st = State::new();
        for line in t.lines() {
            let f: Vec<&str> = line.split(SEP_FIELD).collect();
            if f.len() < 5 {
                continue;
            }
            if let Some(s) = st.stages.iter_mut().find(|s| s.key == f[0]) {
                // A stage that was running when the program closed did not finish.
                s.state = if matches!(f[1], "running" | "waiting") { "stopped".to_string() } else { f[1].to_string() };
                s.started_at = f[2].to_string();
                s.seconds = f[3].parse().unwrap_or(0);
                s.summary = if f[4].is_empty() { Vec::new() } else { f[4].split(SEP_LINE).map(|x| x.to_string()).collect() };
            }
        }
        st
    }

    fn get(&mut self, key: &str) -> &mut Stage {
        self.stages.iter_mut().find(|s| s.key == key).expect("a known stage")
    }

    /// The first stage not done (or skipped) yet.
    pub fn resume_at(&self) -> usize {
        self.stages.iter().position(|s| !matches!(s.state.as_str(), "done" | "skipped")).unwrap_or(self.stages.len())
    }
}

pub(super) fn load(app: &mut App) {
    app.auto = State::from_text(&app.setting("auto_state"));
}

fn save_state(app: &Arc<Mutex<App>>) {
    let mut a = lock(app);
    let t = a.auto.text();
    a.settings.insert("auto_state".to_string(), t);
    a.save();
}

pub(super) fn state_json(a: &App) -> String {
    let stages: Vec<String> = a
        .auto
        .stages
        .iter()
        .map(|s| {
            let title = STAGES.iter().find(|x| x.0 == s.key).map(|x| x.1).unwrap_or("");
            Obj::new()
                .s("key", &s.key)
                .s("title", title)
                .s("state", &s.state)
                .s("started_at", &s.started_at)
                .n(
                    "seconds",
                    match (s.state.as_str(), a.auto.stage_began) {
                        ("running" | "waiting", Some(t)) => t.elapsed().as_secs(),
                        _ => s.seconds,
                    },
                )
                .raw("summary", &strings(s.summary.iter()))
                .end()
        })
        .collect();
    Obj::new()
        .raw("stages", &arr(stages))
        .s("waiting", &a.auto.waiting)
        .s("banner", &a.auto.banner)
        .n("resume_at", a.auto.resume_at() as u64)
        .end()
}

/// Begin again from the first stage (the files made so far are kept, and
/// each step re-uses them where it can).
pub(super) fn start_over(app: &Arc<Mutex<App>>) -> Result<(), String> {
    {
        let mut a = lock(app);
        if a.job.as_ref().is_some_and(|j| j.state == "running") {
            return Err("Wait for what is running to finish, or stop it first.".to_string());
        }
        a.auto = State::new();
    }
    save_state(app);
    Ok(())
}

// ---------------------------------------------------------------- the plan

/// What Start works with, from the settings.
#[derive(Clone, Debug)]
pub(super) struct Plan {
    pub map: String,
    pub image_dir: String,
    pub work: String,
    pub copy2: String,
    pub search: Vec<PathBuf>,
    pub videos: bool,
}

pub(super) fn plan_of(app: &Arc<Mutex<App>>) -> Result<Plan, String> {
    let a = lock(app);
    let map = a.setting("mapfile");
    let image_dir = a.setting("auto_image_dir");
    let work = a.setting("auto_work");
    let image_done = !a.setting("image").is_empty() && !a.setting("image_sha").is_empty();
    if map.is_empty() {
        return Err("Choose the mapfile first (Get ready finds it in the program's folder).".to_string());
    }
    if image_dir.trim().is_empty() && !image_done {
        return Err("Choose the folder the image goes in first (3. The folders).".to_string());
    }
    if work.trim().is_empty() {
        return Err("Choose the folder the copies go in first (3. The folders): WORK on SPARE.".to_string());
    }
    Ok(Plan {
        map,
        image_dir,
        work: work.trim().to_string(),
        copy2: a.setting("auto_copy2"),
        search: a.setting("auto_search").lines().map(|l| l.trim()).filter(|l| !l.is_empty()).map(PathBuf::from).collect(),
        videos: a.setting("auto_videos") != "0",
    })
}

/// Start (or carry on).
pub(super) fn start(app: &Arc<Mutex<App>>) -> Result<u64, String> {
    let plan = plan_of(app)?;
    {
        // The rehearsal first, before the recovered SSD is plugged in (not
        // needed once the image is made, or for the page's test disks).
        let a = lock(app);
        let image_made = !a.setting("image").is_empty() && !a.setting("image_sha").is_empty();
        if cfg!(windows) && a.test_disks.is_empty() && !image_made && a.setting("rehearsal") == "switch-off" {
            return Err(
                "Finish the rehearsal first (2. The rehearsal): switch SPARE off, unplug it, plug it back in the same way and \
                 check it again."
                    .to_string(),
            );
        }
        if cfg!(windows) && a.test_disks.is_empty() && !image_made && a.setting("rehearsal") != "passed" && a.setting("rehearsal") != "stop" {
            return Err(
                "Do the rehearsal with SPARE first (2. The rehearsal): it shows whether Windows will keep the recovered SSD \
                 switched off when it is plugged in that way."
                    .to_string(),
            );
        }
    }
    if lock(app).setting("rehearsal") == "stop" {
        return Err(
            "The rehearsal with SPARE said to stop: Windows would not keep a disk plugged in that way switched off. Use \
             another case or adapter (and do the rehearsal again), or ask Claude."
                .to_string(),
        );
    }
    let app2 = app.clone();
    start_job(app, "auto", "Start: the whole recovery", move |ctx| {
        crate::winset::keep_awake(true);
        let r = run(&app2, ctx, &plan);
        crate::winset::keep_awake(false);
        {
            let mut a = lock(&app2);
            a.auto.waiting.clear();
        }
        save_state(&app2);
        r
    })
}

// ---------------------------------------------------------------- running

/// How a stage ended.
enum End {
    Done(Vec<String>),
    Skipped(Vec<String>),
    /// Something needs the person: the pipeline stops here.
    Stop(String, Vec<String>),
    Stopped,
}

fn set_stage(app: &Arc<Mutex<App>>, key: &str, f: impl FnOnce(&mut Stage)) {
    {
        let mut a = lock(app);
        f(a.auto.get(key));
    }
    save_state(app);
}

fn waiting(app: &Arc<Mutex<App>>, ctx: &Ctx, key: &str, what: &str) {
    let changed = {
        let mut a = lock(app);
        let changed = a.auto.waiting != what;
        a.auto.waiting = what.to_string();
        a.auto.get(key).state = if what.is_empty() { "running".to_string() } else { "waiting".to_string() };
        changed
    };
    if changed && !what.is_empty() {
        ctx.line(&format!("Waiting for you: {what}"));
        crate::live::event("warn", &format!("Waiting for you: {what}"));
        crate::notify::toast("Start needs you", what, false);
    }
}

fn banner(app: &Arc<Mutex<App>>, text: &str) {
    let changed = {
        let mut a = lock(app);
        let changed = a.auto.banner != text;
        a.auto.banner = text.to_string();
        changed
    };
    if changed && !text.is_empty() {
        crate::live::event("good", text);
        crate::notify::toast("When you're ready", text, false);
    }
}

/// The first line of what a stage said, short enough for one line of "what
/// just happened".
fn first_words(lines: &[String]) -> String {
    let l = telling_line(lines);
    let l = l.as_str();
    if l.chars().count() > 170 {
        format!(": {}…", l.chars().take(169).collect::<String>().trim_end())
    } else if l.is_empty() {
        String::new()
    } else {
        format!(": {l}")
    }
}

/// Put a step's result where its own button would have.
fn record(app: &Arc<Mutex<App>>, kind: &str, title: &str, started_at: &str, secs: u64, o: &Outcome) {
    let mut a = lock(app);
    let j = Job {
        kind: kind.to_string(),
        title: title.to_string(),
        state: if o.stopped {
            "stopped"
        } else if o.problem.is_some() {
            "failed"
        } else {
            "done"
        }
        .to_string(),
        error: o.problem.clone().unwrap_or_default(),
        summary: o.summary.clone(),
        outputs: o.outputs.clone(),
        data: o.data.clone(),
        started_at: started_at.to_string(),
        seconds: secs,
        ..Default::default()
    };
    a.last.insert(kind.to_string(), j);
    a.save();
}

/// Run a prepared step inside Start, and record it.
pub(super) fn step(app: &Arc<Mutex<App>>, ctx: &Ctx, p: Prepared) -> Result<Outcome, String> {
    let started = local_now();
    let t = Instant::now();
    ctx.line(&format!("- {}", p.title));
    ctx.amount(0, 0);
    let (kind, title) = (p.kind, p.title);
    let o = (p.work)(ctx)?;
    record(app, kind, title, &started, t.elapsed().as_secs(), &o);
    Ok(o)
}

fn run(app: &Arc<Mutex<App>>, ctx: &Ctx, plan: &Plan) -> Result<Outcome, String> {
    let from = lock(app).auto.resume_at();
    if from >= STAGES.len() {
        return Ok(Outcome {
            summary: vec!["Every stage is done already. To run them again, click Start over first.".to_string()],
            ..Default::default()
        });
    }
    let mut summary = Vec::new();
    crate::live::event(
        "info",
        &if from == 0 {
            format!("Start began: {} stages", STAGES.len())
        } else {
            format!("Start carried on from stage {} of {}: {}", from + 1, STAGES.len(), STAGES[from].1)
        },
    );
    if from > 0 {
        // Carrying on, the ready stage is not run again - and what it set may
        // have been put back since (7. Put Windows back as it was, which only
        // needs every drive unplugged): Defender is asked again to leave the
        // image's folder and SPARE alone. (New disks: stage_image.)
        for l in super::places::defender_ensure(app, &plan.image_dir, &plan.work) {
            ctx.line(&l);
        }
    }
    for (i, (key, title)) in STAGES.iter().enumerate().skip(from) {
        if ctx.stopping() {
            return Ok(stopped_outcome(app, key));
        }
        let started = local_now();
        let t = Instant::now();
        lock(app).auto.stage_began = Some(t);
        // (Nothing counted for the page until the stage counts its own.)
        crate::live::clear();
        set_stage(app, key, |s| {
            s.state = "running".to_string();
            s.started_at = started.clone();
            s.summary.clear();
        });
        ctx.line(&format!("Stage {} of {}: {title}", i + 1, STAGES.len()));
        crate::live::event("info", &format!("Stage {} of {} began: {title}", i + 1, STAGES.len()));
        let end = match *key {
            "ready" => stage_ready(app, ctx, plan),
            "image" => stage_image(app, ctx, plan),
            "open" => stage_open(app, ctx),
            "mapcheck" => stage_mapcheck(app, ctx, plan),
            "copy" => stage_copy(app, ctx, plan),
            "verify" | "verify2" => stage_verify(app, ctx, plan, key),
            "scan" | "scan2" => stage_scan(app, ctx, plan, key),
            "extra" => stage_extra(app, ctx, plan),
            "isolate" => stage_isolate(app, ctx, plan),
            "repair" => stage_repair(app, ctx, plan),
            "putback" => stage_putback(app, ctx, plan),
            "evaluate" => stage_evaluate(app, ctx, plan),
            "handoff" => stage_handoff(app, ctx, plan),
            _ => Ok(End::Skipped(Vec::new())),
        };
        {
            let mut a = lock(app);
            a.auto.waiting.clear();
        }
        let secs = t.elapsed().as_secs();
        let end = end.unwrap_or_else(|e| End::Stop(e, Vec::new()));
        match end {
            End::Done(lines) => {
                // (What the map check found has a line of its own.)
                let said = if *key == "mapcheck" { String::new() } else { first_words(&lines) };
                crate::live::event("good", &format!("Stage {} done: {title} ({}){said}", i + 1, crate::fmt::duration(secs)));
                set_stage(app, key, |s| {
                    s.state = "done".to_string();
                    s.seconds = secs;
                    s.summary = lines.clone();
                });
            }
            End::Skipped(lines) => {
                crate::live::event("info", &format!("Stage {} not needed: {title}{}", i + 1, first_words(&lines)));
                set_stage(app, key, |s| {
                    s.state = "skipped".to_string();
                    s.seconds = secs;
                    s.summary = lines.clone();
                });
            }
            End::Stopped => return Ok(stopped_outcome(app, key)),
            End::Stop(why, lines) => {
                crate::live::event("bad", &format!("Start stopped at stage {} ({title}): {why}", i + 1));
                crate::notify::toast(&format!("Start stopped at stage {}: it needs you", i + 1), &why, true);
                set_stage(app, key, |s| {
                    s.state = "failed".to_string();
                    s.seconds = secs;
                    let mut l = vec![why.clone()];
                    l.extend(lines.iter().cloned());
                    s.summary = l;
                });
                let mut sm = vec![format!("Start stopped at \"{title}\": {why}")];
                sm.extend(lines);
                sm.push("When it is sorted out, click Start again: it carries on from here.".to_string());
                return Ok(Outcome { summary: sm, problem: Some(why), ..Default::default() });
            }
        }
    }
    // (Windows' settings stay as they are: they go back only when the person
    // clicks for it, with every drive unplugged - back.rs.)
    if !super::back::changes(app).is_empty() {
        summary.push(
            "Windows' settings the program changed stay as they are until you put them back (7. Put Windows back as it \
             was), once every drive is unplugged."
                .to_string(),
        );
    }
    banner(app, "");
    crate::live::event("good", "Start is done: every stage ran");
    crate::notify::toast("Start is done", "Every stage ran. The page says what is left to do (5. What is left).", false);
    let mut sm = vec!["Start is done: every stage ran.".to_string()];
    sm.extend(final_lines(app, plan));
    sm.extend(summary);
    Ok(Outcome { summary: sm, ..Default::default() })
}

fn stopped_outcome(app: &Arc<Mutex<App>>, key: &str) -> Outcome {
    if let Some(i) = STAGES.iter().position(|(k, _)| *k == key) {
        crate::live::event("info", &format!("Start was stopped at stage {} ({}): Carry on picks up from there", i + 1, STAGES[i].1));
    }
    set_stage(app, key, |s| s.state = "stopped".to_string());
    Outcome {
        summary: vec!["Stopped. Click Start again to carry on from here: nothing done so far is lost.".to_string()],
        stopped: true,
        ..Default::default()
    }
}

/// The numbers at the end.
fn final_lines(app: &Arc<Mutex<App>>, plan: &Plan) -> Vec<String> {
    let mut t = Vec::new();
    let a = lock(app);
    if let Some(j) = a.last.get("autorepair") {
        t.extend(j.summary.iter().take(4).cloned());
    }
    if let Some(j) = a.last.get("handoff") {
        t.extend(j.summary.iter().take(4).cloned());
    }
    drop(a);
    if let Ok(repair) = crate::repair::repair_dir(Path::new(&plan.work)) {
        t.push(format!("Everything about the repairs is in {}.", repair.display()));
    }
    t
}

// ---------------------------------------------------------------- the stages

fn image_done(app: &Arc<Mutex<App>>) -> Option<String> {
    let a = lock(app);
    let img = a.setting("image");
    (!img.is_empty() && !a.setting("image_sha").is_empty() && Path::new(&img).is_file()).then_some(img)
}

/// The disks that are the copy ddrescue made (the recovered SSD, or the
/// 4 TB SSD that holds a copy of it), and whether each is read-only.
fn copies_attached(app: &Arc<Mutex<App>>) -> Vec<(u32, bool, bool, bool)> {
    let map = lock(app).setting("mapfile");
    let Some((s, e)) = mapfile_domain(&map) else { return Vec::new() };
    let mut out = Vec::new();
    for d in crate::win::disks() {
        if d.system {
            continue;
        }
        let Ok(src) = crate::disk::open_source(&format!(r"\\.\PhysicalDrive{}", d.number)) else { continue };
        let table = crate::gpt::read(&*src, &[]);
        if crate::mapcheck::is_the_copy(&*src, &table, e - s) {
            let st = crate::guard::state(d.number).ok();
            out.push((d.number, st.as_ref().is_some_and(|x| x.read_only), st.as_ref().is_some_and(|x| x.offline), d.removable));
        }
    }
    out
}

fn sleep_or_stop(ctx: &Ctx, secs: u64) -> bool {
    for _ in 0..(secs * 5) {
        if ctx.stopping() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    true
}

fn stage_ready(app: &Arc<Mutex<App>>, ctx: &Ctx, plan: &Plan) -> Result<End, String> {
    let mut lines = Vec::new();
    let (elevated, home, testing) = {
        let a = lock(app);
        (a.elevated, a.home.clone(), !a.test_disks.is_empty())
    };
    let need_ssd = image_done(app).is_none();
    // (The page's tests read a disk image: no SSD, so nothing of Windows' to set.)
    let real_ssd = need_ssd && !testing;
    if real_ssd && !elevated && cfg!(windows) {
        return Ok(End::Stop(
            "making the image needs administrator rights: close the program's window, double-click it again and click Yes"
                .to_string(),
            Vec::new(),
        ));
    }
    let Some((s, e)) = mapfile_domain(&plan.map) else {
        return Ok(End::Stop(format!("the mapfile {} could not be read", plan.map), Vec::new()));
    };
    lines.push(format!("The mapfile: {} ({} drive).", plan.map, crate::fmt::size(e - s)));
    if need_ssd {
        if let Some(d) = drive_of(&plan.image_dir) {
            if d.fs.eq_ignore_ascii_case("FAT32") || d.fs.eq_ignore_ascii_case("FAT") || d.fs.eq_ignore_ascii_case("exFAT") {
                return Ok(End::Stop(
                    format!("{} is formatted {}: the image needs a drive formatted NTFS", d.letter, d.fs),
                    Vec::new(),
                ));
            }
            if d.free < (e - s) + (1 << 30) {
                return Ok(End::Stop(
                    format!(
                        "{} has {} free, and the image needs {}: choose a folder on a drive with more room",
                        d.letter,
                        crate::fmt::size(d.free),
                        crate::fmt::size(e - s)
                    ),
                    Vec::new(),
                ));
            }
            lines.push(format!("The image goes into {} ({} free on {}).", plan.image_dir, crate::fmt::size(d.free), d.letter));
        } else if cfg!(windows) {
            return Ok(End::Stop(format!("the image folder {} is not on a drive this PC has", plan.image_dir), Vec::new()));
        }
        if real_ssd && crate::winset::restart_pending() == Some(true) {
            return Ok(End::Stop(
                "Windows is waiting to restart (an update). Restart the PC now, with the recovered SSD NOT plugged in, \
                 then click Start again"
                    .to_string(),
                Vec::new(),
            ));
        }
        if real_ssd && crate::winset::restarts_after_crash() == Some(true) {
            lines.push(
                "Note: Windows is set to restart by itself after a crash (Get ready can turn that off), so a crash would \
                 restart the PC with the recovered SSD plugged in."
                    .to_string(),
            );
        }
        // Before the SSD is plugged in: new disks arrive switched off.
        if real_ssd && cfg!(windows) && crate::cli::policy_to_restore().is_none() {
            match crate::cli::guard_prepare() {
                Ok(m) => lines.push(m),
                Err(e) => return Ok(End::Stop(format!("Windows could not be set to leave new disks switched off: {e}"), Vec::new())),
            }
        }
    }
    // Defender leaves the image's folder and SPARE alone while the program
    // writes there: asked by Start itself, and confirmed in Defender's list.
    let defender = super::places::defender_ensure(app, &plan.image_dir, &plan.work);
    for l in &defender {
        ctx.line(l);
    }
    lines.extend(defender);
    for t in crate::tools::status(&home) {
        lines.push(match &t.path {
            Some(_) => format!("{}: {}", t.tool.name(), t.version),
            None => format!("{}: not found ({} will be left out)", t.tool.name(), t.tool.used_for()),
        });
    }
    ctx.line(&lines.join("\n"));
    Ok(End::Done(lines))
}

fn stage_image(app: &Arc<Mutex<App>>, ctx: &Ctx, plan: &Plan) -> Result<End, String> {
    if let Some(img) = image_done(app) {
        return Ok(End::Skipped(vec![format!("The image was made and checked before: {img}")]));
    }
    // New disks arrive switched off - set again if Windows was put back since
    // the ready stage (7. Put Windows back as it was works whenever every
    // drive is unplugged, as they are while Start waits here for the SSD).
    let testing = !lock(app).test_disks.is_empty();
    if cfg!(windows) && !testing && crate::cli::policy_to_restore().is_none() {
        match crate::cli::guard_prepare() {
            Ok(m) => ctx.line(&format!("Windows is set again to leave new disks switched off: {m}")),
            Err(e) => {
                return Ok(End::Stop(format!("Windows could not be set to leave new disks switched off: {e}"), Vec::new()))
            }
        }
    }
    // The recovered SSD: plugged in, the one copy, read-only.
    let disk = loop {
        let copies = copies_attached(app);
        let tests = lock(app).test_disks.clone();
        if copies.len() > 1 {
            waiting(
                app,
                ctx,
                "image",
                "Two copies of the old drive are plugged in (the recovered SSD and the 4 TB SSD?). Unplug the 4 TB SSD now: \
                 never plug both in at once.",
            );
        } else if let Some(&(n, ro, _off, removable)) = copies.first() {
            if !ro {
                ctx.line(&format!("The recovered SSD is disk {n}: marking it read-only..."));
                crate::guard::protect(n, false).map_err(|e| format!("disk {n} could not be protected: {e}"))?;
            }
            if removable {
                ctx.line(REMOVABLE_NOTE);
            }
            break format!(r"\\.\PhysicalDrive{n}");
        } else if let Some(t) = tests.first() {
            // (The page's tests: a disk image offered as a disk.)
            break t.clone();
        } else {
            waiting(
                app,
                ctx,
                "image",
                "Plug in the recovered SSD now - the 2 TB one, exactly the way you rehearsed with SPARE (the same case or \
                 adapter, USB port and cable). The 4 TB SSD stays in its drawer. It arrives switched off; the program \
                 marks it read-only by itself.",
            );
        }
        if !sleep_or_stop(ctx, 2) {
            return Ok(End::Stopped);
        }
    };
    waiting(app, ctx, "image", "");
    {
        let mut a = lock(app);
        a.settings.insert("disk".to_string(), disk.clone());
        a.save();
    }
    let o = step(app, ctx, prepare_image(app, &disk, &plan.image_dir)?)?;
    if o.stopped {
        return Ok(End::Stopped);
    }
    if let Some(p) = o.problem {
        return Ok(End::Stop(p, o.summary));
    }
    banner(
        app,
        "The image is made and checked: the recovered SSD is not needed any more. Switch it off and unplug it, and put it \
         back in its drawer (step 9 of the program's steps has Switch it off).",
    );
    Ok(End::Done(o.summary))
}

/// The image mounted read-only with AIM, dismounted when dropped.
struct Opened {
    exe: PathBuf,
    device: String,
}

impl Drop for Opened {
    fn drop(&mut self) {
        let _ = crate::aim::dismount(&self.exe, &self.device);
    }
}

static OPENED: Mutex<Option<Opened>> = Mutex::new(None);

fn stage_open(app: &Arc<Mutex<App>>, ctx: &Ctx) -> Result<End, String> {
    let (home, elevated, image) = {
        let a = lock(app);
        (a.home.clone(), a.elevated, a.setting("image"))
    };
    if !cfg!(windows) || image.is_empty() {
        return Ok(End::Skipped(vec!["(Only on Windows, with the image made.)".to_string()]));
    }
    let skip = |why: &str| {
        Ok(End::Skipped(vec![format!(
            "{why}, so this time the map check cannot check itself against Windows' own reading of the volume. Everything \
             else is the same (Get ready can install AIM Toolkit)."
        )]))
    };
    if !elevated {
        return skip("The program runs without administrator rights");
    }
    let Some(exe) = crate::tools::find(crate::tools::Tool::AimCli, &home) else { return skip("AIM Toolkit is not installed") };
    if crate::winset::aim_driver_installed() == Some(false) {
        return skip("AIM Toolkit's driver is not installed");
    }
    // Opened before, by a copy of the program that was then closed: closed
    // first, so the image is never open twice.
    close_image(app);
    ctx.line(&format!("Opening {image} read-only with AIM Toolkit..."));
    let m = match crate::aim::mount_read_only(&exe, Path::new(&image), &|| ctx.stopping()) {
        Ok(m) => m,
        Err(e) => return skip(&format!("AIM Toolkit could not open the image ({e})")),
    };
    {
        let mut a = lock(app);
        a.settings.insert("aim_device".to_string(), m.device.clone());
        a.save();
    }
    *OPENED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Opened { exe, device: m.device.clone() });
    // Windows gives the volume a letter in a moment.
    let serial = crate::disk::open_source(&image)
        .ok()
        .and_then(|src| {
            let table = crate::gpt::read(&*src, &[]);
            crate::mapcheck::find_volume_quick(&*src, &table).ok().map(|(v, _)| v.serial)
        });
    let mut letter = None;
    for _ in 0..30 {
        if let Some(s) = serial {
            letter = crate::sys::drive_for_serial(s);
        }
        if letter.is_some() {
            break;
        }
        if !sleep_or_stop(ctx, 1) {
            close_image(app);
            return Ok(End::Stopped);
        }
    }
    match letter {
        Some(l) => Ok(End::Done(vec![format!(
            "The image is open read-only as drive {l} (AIM Toolkit, device {}); the map check compares itself with it, \
             then it is closed again.",
            m.device
        )])),
        None => {
            close_image(app);
            skip("Windows gave the image's volume no drive letter")
        }
    }
}

/// Close the image AIM Toolkit opened: this copy of the program's, or one
/// left open by a copy that was closed before it could close it (aim_cli
/// keeps serving the image by itself until it is told to stop).
fn close_image(app: &Arc<Mutex<App>>) {
    let opened = OPENED.lock().unwrap_or_else(|e| e.into_inner()).take();
    let left = lock(app).setting("aim_device");
    match opened {
        // (Dropping it closes it.)
        Some(o) => drop(o),
        None if !left.is_empty() => {
            let home = lock(app).home.clone();
            if let Some(exe) = crate::tools::find(crate::tools::Tool::AimCli, &home) {
                let _ = crate::aim::dismount(&exe, &left);
            }
        }
        None => return,
    }
    let mut a = lock(app);
    a.settings.remove("aim_device");
    a.save();
}

fn stage_mapcheck(app: &Arc<Mutex<App>>, ctx: &Ctx, plan: &Plan) -> Result<End, String> {
    let image = image_done(app).ok_or("there is no checked image to read")?;
    let r = run_map_check(app, ctx, plan, &image);
    // Closed again whatever happened.
    close_image(app);
    if !matches!(r, Ok(End::Done(_))) {
        // Opened again when Start carries on, so the map check can still
        // check itself against Windows then.
        {
            let mut a = lock(app);
            let open = a.auto.get("open");
            if open.state == "done" {
                open.state = "pending".to_string();
            }
        }
        save_state(app);
    }
    r
}

fn run_map_check(app: &Arc<Mutex<App>>, ctx: &Ctx, plan: &Plan, image: &str) -> Result<End, String> {
    check_disk(app, image)?;
    let started = local_now();
    let t = Instant::now();
    let out = fresh(&lock(app).results, &format!("map-check-{}", stamp()), "");
    let opts = crate::mapcheck::Options {
        map_path: plan.map.clone(),
        earlier_maps: Vec::new(),
        disk: image.to_string(),
        out_dir: out.display().to_string(),
        compare_with: None,
    };
    ctx.line("- Finding what the damage touched");
    let report = crate::mapcheck::run(&opts, &mut |s: &str| ctx.line(s))?;
    {
        let mut a = lock(app);
        a.settings.insert("last_map_check".to_string(), opts.out_dir.clone());
        a.save();
    }
    let mut outputs = vec![opts.out_dir.clone()];
    outputs.extend(report.written.iter().cloned());
    let summary = crate::mapcheck::summary(&report);
    let mut problem = None;
    if report.newer_than_copy() {
        problem = Some(
            "the newest date on the volume is after ddrescue finished: something may have changed the SSD after the copy \
             (or a file just has a wrong date). Ask Claude before going on"
                .to_string(),
        );
    }
    if let Some(w) = &report.windows {
        if w.failed.is_none() && !w.agrees() {
            problem = Some(
                "the program's reading of the volume and Windows' own reading of it differ (see the lines below): ask Claude \
                 before anything is copied"
                    .to_string(),
            );
        }
    }
    let serious_twice = report.claimed_twice.iter().any(|(a, b, _)| !a.leftover && !b.leftover);
    let serious_free = report.claimed_free.iter().any(|(c, _)| !c.leftover);
    if serious_twice || serious_free {
        problem = Some(
            "some records point at space that is not theirs (see the lines below): the program may be reading part of the \
             volume wrongly. Ask Claude before anything is copied"
                .to_string(),
        );
    }
    // Will it fit?
    if problem.is_none() {
        if let Some(d) = drive_of(&plan.work) {
            let already = crate::verify::ListTotals::read(Path::new(&plan.work)).ok().map(|l| l.file_bytes + l.piece_bytes).unwrap_or(0);
            if d.free + already < report.bytes_seen + (1 << 30) {
                problem = Some(format!(
                    "the files take {}, and {} has {} free: they do not fit on SPARE. Ask Claude",
                    crate::fmt::size(report.bytes_seen),
                    d.letter,
                    crate::fmt::size(d.free)
                ));
            }
        }
    }
    let o = Outcome {
        summary: summary.clone(),
        outputs,
        data: crate::mapcheck::page_json(&report),
        problem: problem.clone(),
        ..Default::default()
    };
    record(app, "mapcheck", "Finding what the damage touched", &started, t.elapsed().as_secs(), &o);
    crate::live::event(if problem.is_some() { "warn" } else { "info" }, &format!("Found what the damage touched: {}", crate::mapcheck::found_words(&report)));
    let mut lines: Vec<String> = summary.iter().take(8).cloned().collect();
    lines.extend(crate::mapcheck::windows_lines(&report).into_iter().take(3));
    match problem {
        Some(p) => Ok(End::Stop(p, lines)),
        None => Ok(End::Done(lines)),
    }
}

/// SPARE, if it is plugged in but switched off (Windows' setting for new
/// disks): the disk the rehearsal remembered, or the one disk switched off
/// that is not a copy of the old drive and holds no exFAT volume.
fn switch_spare_on(app: &Arc<Mutex<App>>, ctx: &Ctx) {
    if !cfg!(windows) {
        return;
    }
    let remembered = lock(app).setting("spare_disk");
    let mut candidates = Vec::new();
    for d in crate::win::disks() {
        if d.system || !d.drives.is_empty() {
            continue;
        }
        let Ok(st) = crate::guard::state(d.number) else { continue };
        if !st.offline {
            continue;
        }
        if could_be_recovered(app, d.number) {
            continue;
        }
        let id = format!("{}|{}", d.model, d.size);
        candidates.push((d.number, id));
    }
    let pick = candidates.iter().find(|c| c.1 == remembered).or(if candidates.len() == 1 { candidates.first() } else { None });
    if let Some((n, _)) = pick {
        match crate::guard::release(*n) {
            Ok(_) => ctx.line(&format!("SPARE (disk {n}) arrived switched off; switched it on.")),
            Err(e) => ctx.line(&format!("(SPARE, disk {n}, could not be switched on: {e})")),
        }
    }
}

fn stage_copy(app: &Arc<Mutex<App>>, ctx: &Ctx, plan: &Plan) -> Result<End, String> {
    let image = image_done(app).ok_or("there is no checked image to copy from")?;
    let work = PathBuf::from(&plan.work);
    // The recovered SSD unplugged: nothing more to say about it. (Windows'
    // setting for new disks stays: SPARE, if it arrives switched off, is
    // switched on below.)
    if recovered_disk_attached(map_size_now()).is_none() {
        banner(app, "");
    }
    // WORK's drive must be there.
    loop {
        let root_ok = work.ancestors().last().is_some_and(|r| r.exists()) && drive_of(&plan.work).is_some_or_not_windows();
        if root_ok && std::fs::create_dir_all(&work).is_ok() {
            break;
        }
        switch_spare_on(app, ctx);
        let elsewhere = crate::win::drives()
            .into_iter()
            .find(|d| d.label.eq_ignore_ascii_case("SPARE") && !plan.work.to_uppercase().starts_with(&d.letter.to_uppercase()));
        let msg = match elsewhere {
            Some(d) => format!(
                "SPARE is plugged in, but as {} now, and the copies are to go into {}. Click Stop, change the copies folder to \
                 {}\\WORK (3. The folders), and click Carry on.",
                d.letter, plan.work, d.letter
            ),
            None => format!(
                "Plug in SPARE now (the copies go into {}). The recovered SSD is not needed for this: everything is read from \
                 the image.",
                plan.work
            ),
        };
        waiting(app, ctx, "copy", &msg);
        if !sleep_or_stop(ctx, 3) {
            return Ok(End::Stopped);
        }
    }
    waiting(app, ctx, "copy", "");
    let o = step(app, ctx, prepare_rescue(app, &image, &plan.work, true, true)?)?;
    if o.stopped {
        return Ok(End::Stopped);
    }
    if let Some(p) = o.problem {
        return Ok(End::Stop(p, o.summary));
    }
    Ok(End::Done(o.summary))
}

fn stage_verify(app: &Arc<Mutex<App>>, ctx: &Ctx, plan: &Plan, key: &str) -> Result<End, String> {
    if key == "verify2" && !lock(app).auto.stages.iter().any(|s| s.key == "putback" && s.state == "done") {
        return Ok(End::Skipped(vec!["Nothing was put back, so the copies are as the first check found them.".to_string()]));
    }
    let o = step(app, ctx, prepare_verify(app, &plan.work)?)?;
    if o.stopped {
        return Ok(End::Stopped);
    }
    if let Some(p) = o.problem {
        return Ok(End::Stop(p, o.summary));
    }
    Ok(End::Done(o.summary))
}

fn stage_scan(app: &Arc<Mutex<App>>, ctx: &Ctx, plan: &Plan, key: &str) -> Result<End, String> {
    if key == "scan2" && !lock(app).auto.stages.iter().any(|s| s.key == "putback" && s.state == "done") {
        return Ok(End::Skipped(vec!["Nothing was put back, so the first check of every file still stands.".to_string()]));
    }
    let join = lock(app).setting("last_map_check");
    let o = step(app, ctx, prepare_scan(app, &plan.work, true, crate::formats::DEFAULT_HOLE_KB, &join)?)?;
    if o.stopped {
        return Ok(End::Stopped);
    }
    Ok(End::Done(o.summary))
}

fn stage_extra(app: &Arc<Mutex<App>>, ctx: &Ctx, plan: &Plan) -> Result<End, String> {
    let (home, results, map_check) = {
        let a = lock(app);
        (a.home.clone(), a.results.clone(), a.setting("last_map_check"))
    };
    let started = local_now();
    let t = Instant::now();
    let jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).div_ceil(2).clamp(1, 4);
    // The map check's list of the files the damage touched: a tool's
    // findings among the files read whole are weighed against how many it
    // flags (extra.rs).
    let map_items = (!map_check.is_empty())
        .then(|| std::fs::read_to_string(Path::new(&map_check).join(crate::join::TSV_NAME)).ok())
        .flatten()
        .map(|t| crate::join::parse_tsv(&t));
    let opts = extra::Options {
        work: PathBuf::from(&plan.work),
        videos: plan.videos,
        jobs,
        done_list: Some(results.join("extra-checks-done.tsv")),
        map_items,
    };
    ctx.line("- The extra checks");
    let stop = ctx.stop.clone();
    let o = extra::run(&home, &opts, &mut |s| ctx.line(s), &mut |d, t| ctx.amount(d, t), &move || stop.load(Ordering::Relaxed))?;
    let report = fresh(&results, &format!("extra-checks-{}", stamp()), ".csv");
    let mut outputs: Vec<String> = o.lists.iter().map(|p| p.display().to_string()).collect();
    if extra::write_report(&report, &o).is_ok() {
        outputs.insert(0, report.display().to_string());
    }
    let summary = extra::summary(&o);
    let found: Vec<String> = o
        .findings
        .iter()
        .take(500)
        .map(|f| Obj::new().s("check", f.check.title()).s("path", &f.path).s("message", &f.message).end())
        .collect();
    let counts: Vec<String> = o
        .counts
        .iter()
        .map(|(c, (n, bad))| Obj::new().s("check", c.title()).n("checked", *n).n("damaged", *bad).end())
        .collect();
    let out = Outcome {
        summary: summary.clone(),
        outputs,
        data: Obj::new().raw("findings", &arr(found)).n("found", o.findings.len() as u64).raw("counts", &arr(counts)).end(),
        stopped: o.stopped,
        problem: None,
    };
    record(app, "extra", "The extra checks", &started, t.elapsed().as_secs(), &out);
    if o.stopped {
        return Ok(End::Stopped);
    }
    Ok(End::Done(summary))
}

fn damaged_to_isolate(app: &Arc<Mutex<App>>, plan: &Plan) -> bool {
    scan_csv_for(app, &plan.work).is_some()
}

fn stage_isolate(app: &Arc<Mutex<App>>, ctx: &Ctx, plan: &Plan) -> Result<End, String> {
    if !damaged_to_isolate(app, plan) {
        return Ok(End::Stop("there is no check of every copied file to say which are damaged".to_string(), Vec::new()));
    }
    let o = step(app, ctx, prepare_repair_step(app, "isolate", &plan.work, "")?)?;
    if o.stopped {
        return Ok(End::Stopped);
    }
    Ok(End::Done(o.summary))
}

fn stage_repair(app: &Arc<Mutex<App>>, ctx: &Ctx, plan: &Plan) -> Result<End, String> {
    let work = PathBuf::from(&plan.work);
    let repair = crate::repair::repair_dir(&work)?;
    if crate::repair::read_list(&repair)?.is_empty() {
        return Ok(End::Skipped(vec!["No damaged files were copied out, so there is nothing to repair.".to_string()]));
    }
    let (home, map_check) = {
        let a = lock(app);
        (a.home.clone(), a.setting("last_map_check"))
    };
    let started = local_now();
    let t = Instant::now();
    ctx.line("- Repairing the damaged files with the open-source tools");
    let opts = autorepair::Options {
        work: work.clone(),
        search: plan.search.clone(),
        map_check: (!map_check.is_empty()).then(|| PathBuf::from(&map_check)),
    };
    let stop = ctx.stop.clone();
    let o = autorepair::run(&home, &opts, &mut |s| ctx.line(s), &mut |d, t| ctx.amount(d, t), &move || stop.load(Ordering::Relaxed))?;
    let summary = autorepair::summary(&o);
    let rows: Vec<String> = autorepair::read_log(&repair)
        .iter()
        .map(|a| {
            Obj::new()
                .s("id", &a.id)
                .s("path", &a.path)
                .s("result", a.fate.word())
                .s("how", &a.how)
                .s("tool", &a.tool)
                .s("output", &a.output)
                .s("next", a.next.key())
                .s("note", &a.note)
                .end()
        })
        .collect();
    let out = Outcome {
        summary: summary.clone(),
        outputs: o.log.iter().map(|p| p.display().to_string()).chain(std::iter::once(repair.display().to_string())).collect(),
        data: Obj::new()
            .n("repaired", o.count(autorepair::Fate::Repaired) as u64)
            .n("salvaged", o.count(autorepair::Fate::Salvaged) as u64)
            .n("not_repaired", o.count(autorepair::Fate::NotRepaired) as u64)
            .n("already", o.already as u64)
            .raw("attempts", &arr(rows))
            .s("repair", &repair.display().to_string())
            .end(),
        stopped: o.stopped,
        problem: None,
    };
    record(app, "autorepair", "Repairing with the open-source tools", &started, t.elapsed().as_secs(), &out);
    if o.stopped {
        return Ok(End::Stopped);
    }
    Ok(End::Done(summary))
}

fn stage_putback(app: &Arc<Mutex<App>>, ctx: &Ctx, plan: &Plan) -> Result<End, String> {
    let work = PathBuf::from(&plan.work);
    let repair = crate::repair::repair_dir(&work)?;
    if crate::repair::read_list(&repair)?.is_empty() {
        return Ok(End::Skipped(vec!["Nothing was copied out, so nothing goes back.".to_string()]));
    }
    let o = step(app, ctx, prepare_repair_step(app, "put-back", &plan.work, "")?)?;
    if o.stopped {
        return Ok(End::Stopped);
    }
    // A repaired version that could not go back is a problem only if it is
    // one of the program's own (the person's own may be sorted later).
    Ok(End::Done(o.summary))
}

fn stage_evaluate(app: &Arc<Mutex<App>>, ctx: &Ctx, plan: &Plan) -> Result<End, String> {
    if !damaged_to_isolate(app, plan) {
        return Ok(End::Skipped(Vec::new()));
    }
    let o = step(app, ctx, prepare_repair_step(app, "evaluate", &plan.work, "")?)?;
    if o.stopped {
        return Ok(End::Stopped);
    }
    Ok(End::Done(o.summary))
}

fn stage_handoff(app: &Arc<Mutex<App>>, ctx: &Ctx, plan: &Plan) -> Result<End, String> {
    let work = PathBuf::from(&plan.work);
    let repair = crate::repair::repair_dir(&work)?;
    if crate::repair::read_list(&repair)?.is_empty() {
        return Ok(End::Skipped(vec!["Nothing is damaged, so there is nothing to hand off.".to_string()]));
    }
    let started = local_now();
    let t = Instant::now();
    ctx.line("- Getting the rest ready for the Mac or a paid tool");
    let h = handoff::prepare(&work)?;
    let summary = handoff::summary(&h);
    let out = Outcome {
        summary: summary.clone(),
        outputs: vec![h.folder.display().to_string()],
        data: Obj::new()
            .n("mac", h.mac.len() as u64)
            .n("paid", h.paid.len() as u64)
            .n("own", h.own.len() as u64)
            .n("nothing", h.nothing.len() as u64)
            .s("folder", &h.folder.display().to_string())
            .s("returned", &h.folder.join(handoff::RETURNED).display().to_string())
            .end(),
        stopped: false,
        problem: None,
    };
    record(app, "handoff", "What is left, ready for the Mac or a paid tool", &started, t.elapsed().as_secs(), &out);
    Ok(End::Done(summary))
}

// ---------------------------------------------------------------- bring back

/// Take in what came back from the Mac or a paid tool, put it back, and check
/// everything again.
pub(super) fn start_bring_back(app: &Arc<Mutex<App>>, from: &str) -> Result<u64, String> {
    let plan = plan_of(app)?;
    let work = PathBuf::from(&plan.work);
    let repair = crate::repair::repair_dir(&work)?;
    let from = if from.trim().is_empty() { repair.join(handoff::HANDOFF_DIR).join(handoff::RETURNED) } else { PathBuf::from(from.trim()) };
    if !from.is_dir() {
        return Err(format!("{} is not a folder this PC can see (is the USB stick plugged in?)", from.display()));
    }
    let app2 = app.clone();
    start_job(app, "bringback", "Bringing back the repaired files", move |ctx| {
        crate::winset::keep_awake(true);
        let r = bring_back_run(&app2, ctx, &plan, &from);
        crate::winset::keep_awake(false);
        r
    })
}

fn bring_back_run(app: &Arc<Mutex<App>>, ctx: &Ctx, plan: &Plan, from: &Path) -> Result<Outcome, String> {
    let work = PathBuf::from(&plan.work);
    ctx.line(&format!("Looking for repaired files (with their numbers) in {}...", from.display()));
    let b = handoff::bring_back(&work, from, &mut |s| ctx.line(s))?;
    let mut summary = handoff::brought_summary(&b);
    if b.repaired.is_empty() {
        return Ok(Outcome { summary, ..Default::default() });
    }
    // The same as after Start: put back, check, check every file, evaluate.
    let o = step(app, ctx, prepare_repair_step(app, "put-back", &plan.work, "")?)?;
    summary.extend(o.summary.iter().take(3).cloned());
    if o.stopped {
        return Ok(Outcome { summary, stopped: true, ..Default::default() });
    }
    let v = step(app, ctx, prepare_verify(app, &plan.work)?)?;
    if v.stopped {
        return Ok(Outcome { summary, stopped: true, ..Default::default() });
    }
    if let Some(p) = v.problem {
        summary.extend(v.summary);
        return Ok(Outcome { summary, problem: Some(p), ..Default::default() });
    }
    summary.push(v.summary.first().cloned().unwrap_or_default());
    let join = lock(app).setting("last_map_check");
    let s = step(app, ctx, prepare_scan(app, &plan.work, true, crate::formats::DEFAULT_HOLE_KB, &join)?)?;
    if s.stopped {
        return Ok(Outcome { summary, stopped: true, ..Default::default() });
    }
    let e = step(app, ctx, prepare_repair_step(app, "evaluate", &plan.work, "")?)?;
    summary.extend(e.summary.iter().take(3).cloned());
    if let Ok(h) = handoff::prepare(&work) {
        summary.extend(handoff::summary(&h));
    }
    Ok(Outcome { summary, ..Default::default() })
}

// ---------------------------------------------------------------- AIM Toolkit, tried

/// Get ready's "Try it": AIM Toolkit opens the self-test's small disk image
/// read-only, Windows gives its volume a letter and reads it, nothing can be
/// written to it, and it is closed again - before Start relies on it with
/// the real image.
pub(super) fn start_aim_test(app: &Arc<Mutex<App>>) -> Result<u64, String> {
    need_admin(app)?;
    let (home, results) = {
        let a = lock(app);
        (a.home.clone(), a.results.clone())
    };
    let exe = crate::tools::find(crate::tools::Tool::AimCli, &home).ok_or("AIM Toolkit is not installed yet (Install AIM Toolkit first)")?;
    let app2 = app.clone();
    start_job(app, "aimtest", "Trying AIM Toolkit on a test image", move |ctx| {
        let r = aim_test(ctx, &exe, &results);
        let (ok, lines) = match &r {
            Ok(lines) => (true, lines.clone()),
            Err(lines) => (false, lines.clone()),
        };
        {
            let mut a = lock(&app2);
            a.settings.insert("aim_test".to_string(), if ok { "passed" } else { "failed" }.to_string());
            a.settings.insert("aim_test_text".to_string(), lines.first().cloned().unwrap_or_default());
            a.save();
        }
        Ok(Outcome {
            summary: lines.clone(),
            problem: (!ok).then(|| lines.first().cloned().unwrap_or_default()),
            data: Obj::new().b("passed", ok).end(),
            ..Default::default()
        })
    })
}

fn aim_test(ctx: &Ctx, exe: &Path, results: &Path) -> Result<Vec<String>, Vec<String>> {
    let fail = |why: String| Err(vec![why]);
    let img = results.join("aim-test.img");
    let disk = crate::selftest::disk_image().map_err(|e| vec![e])?;
    let _ = std::fs::create_dir_all(results);
    if let Err(e) = std::fs::write(&img, &disk) {
        return fail(format!("the test image could not be written to {} ({e})", img.display()));
    }
    let serial = crate::disk::open_source(&img.display().to_string()).ok().and_then(|src| {
        let table = crate::gpt::read(&*src, &[]);
        crate::mapcheck::find_volume_quick(&*src, &table).ok().map(|(v, _)| v.serial)
    });
    ctx.line(&format!("Opening {} read-only with AIM Toolkit...", img.display()));
    let m = match crate::aim::mount_read_only(exe, &img, &|| ctx.stopping()) {
        Ok(m) => m,
        Err(e) => {
            let _ = std::fs::remove_file(&img);
            return fail(format!("AIM Toolkit could not open the test image: {e}"));
        }
    };
    ctx.line(&format!("Opened as device {}{}.", m.device, if m.read_only { ", read-only" } else { "" }));
    let mut letter = None;
    for _ in 0..30 {
        letter = serial.and_then(crate::sys::drive_for_serial);
        if letter.is_some() || !sleep_or_stop(ctx, 1) {
            break;
        }
    }
    let mut lines = Vec::new();
    let mut problem = None;
    match &letter {
        Some(l) => {
            let root = PathBuf::from(format!("{l}\\"));
            let listed = std::fs::read_dir(&root).map(|rd| rd.flatten().count()).unwrap_or(0);
            let probe = root.join("aim-test-write.tmp");
            let written = std::fs::write(&probe, b"x").is_ok();
            if written {
                let _ = std::fs::remove_file(&probe);
                problem = Some(format!("the test image opened as {l} but could be written to: AIM did not open it read-only"));
            } else if listed == 0 {
                problem = Some(format!("the test image opened as {l}, but Windows could list nothing on it"));
            } else {
                lines.push(format!(
                    "AIM Toolkit works: it opened a test image read-only as {l}, Windows listed {listed} entries on it, nothing \
                     could be written to it, and it was closed again. Start will use it the same way with the image."
                ));
            }
        }
        None => problem = Some("AIM Toolkit opened the test image, but Windows gave its volume no drive letter".to_string()),
    }
    match crate::aim::dismount(exe, &m.device) {
        Ok(()) => {}
        Err(e) => {
            problem.get_or_insert(format!("the test image could not be closed again ({e}): restart the PC to close it"));
        }
    }
    // Gone again (the letter goes in a moment).
    for _ in 0..10 {
        if letter.is_none() || serial.and_then(crate::sys::drive_for_serial).is_none() {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    let _ = std::fs::remove_file(&img);
    match problem {
        Some(p) => Err(vec![p, format!("(AIM's device number was {}.)", m.device)]),
        None => Ok(lines),
    }
}

// ---------------------------------------------------------------- the rehearsal

/// A disk's name that is most likely the SSD's own (not a USB case's or
/// adapter's), from the makers' names.
pub(super) fn looks_like_drive_name(model: &str) -> bool {
    let m = model.to_lowercase();
    const DRIVES: &[&str] = &[
        "ssd", "samsung", "crucial", "ct1000", "ct2000", "ct500", "wdc", "wd ", "wd_", "western digital", "sandisk", "kingston",
        "intel", "sk hynix", "hynix", "seagate", "toshiba", "kioxia", "micron", "adata", "pny", "corsair", "transcend", "lexar",
        "teamgroup", "silicon power", "patriot", "gigabyte", "hikvision", "netac", "apacer", "verbatim", "mushkin", "mx500",
        "870 evo", "860 evo", "850 evo", "nvme",
    ];
    !BRIDGE_NAMES.iter().any(|b| m.contains(b)) && DRIVES.iter().any(|d| m.contains(d))
}

/// Is disk `n` SPARE? A drive on it labelled SPARE; or, switched off (no
/// drive letters to show its label), the disk the rehearsal remembered. Never
/// another disk with drives of its own: the rehearsal switches SPARE off,
/// and must never switch off a disk that holds anything else.
fn is_spare(app: &Arc<Mutex<App>>, d: &crate::win::Disk) -> Result<(), String> {
    if d.system {
        return Err(format!("disk {} holds Windows itself: plug in SPARE", d.number));
    }
    if !d.drives.is_empty() {
        if d.drives.iter().any(|v| v.label.eq_ignore_ascii_case("SPARE")) {
            return Ok(());
        }
        // The disk checked as SPARE before (labelled otherwise), still empty.
        let remembered = lock(app).setting("spare_disk");
        if remembered == format!("{}|{}", d.model, d.size) && d.drives.iter().all(|v| v.total > 0 && v.total.saturating_sub(v.free) < 2 << 30) {
            return Ok(());
        }
        let shown: Vec<String> = d.drives.iter().map(|v| format!("{} {}", v.letter, v.label)).collect();
        return Err(format!(
            "disk {} holds {}, not SPARE: click the disk whose drive is labelled SPARE",
            d.number,
            shown.join(", ")
        ));
    }
    let remembered = lock(app).setting("spare_disk");
    if remembered == format!("{}|{}", d.model, d.size) || (remembered.is_empty() && d.bus.eq_ignore_ascii_case("USB")) {
        return Ok(());
    }
    Err(format!(
        "disk {} is switched off, and it is not the disk the rehearsal saw as SPARE before: plug SPARE in and check it",
        d.number
    ))
}

/// The rehearsal: SPARE plugged in the way the recovered SSD will be. What
/// Windows did with it says whether the recovered SSD will arrive switched
/// off:
///
/// - Switched off: Windows' setting for new disks holds for a disk plugged in
///   that way (or Windows remembers this disk switched off): so will the
///   recovered SSD be. SPARE is left switched off (Start switches it on when
///   the copies need it), so that nothing changes what Windows remembers.
/// - Switched on: Windows has seen SPARE before and remembers it switched on.
///   A USB case often gives Windows its own identity for whatever disk is in
///   it (the case's chip's name, such as "Realtek RTL9210 NVME", is a sign),
///   so the recovered SSD in it could be taken for SPARE and arrive switched
///   on too. So SPARE is switched off and read-only, remembered (Switch SPARE
///   off), unplugged and plugged in again: arriving switched off then proves
///   Windows keeps that - and the recovered SSD will arrive switched off,
///   whether Windows takes it for SPARE or as a disk it has never seen.
/// - Removable media, or still switched on after being switched off:
///   Windows' setting does not hold for a disk plugged in that way: stop.
pub(super) fn rehearse(app: &Arc<Mutex<App>>, n: u32) -> Result<String, String> {
    need_admin(app)?;
    let d = crate::win::disks().into_iter().find(|d| d.number == n).ok_or_else(|| format!("disk {n} is not plugged in"))?;
    is_spare(app, &d)?;
    if could_be_recovered(app, n) {
        return Err(format!(
            "disk {n} looks like a copy of the old drive (or holds an exFAT volume), not SPARE: the rehearsal is done with \
             SPARE, which is empty. If it is SPARE, stop and ask Claude: the program takes it for the recovered SSD"
        ));
    }
    let st = crate::guard::state(n)?;
    let id = format!("{}|{}", d.model, d.size);
    let (switched_off_before, unplugged) = {
        let a = lock(app);
        (a.setting("rehearsal") == "switch-off", a.setting("rehearsal_unplugged") == "1")
    };
    if switched_off_before && !unplugged && st.offline && !d.removable {
        // Still switched off from the click: that shows nothing yet. Only
        // plugged in again does SPARE show what Windows remembers.
        watch_unplug(app, n, &id);
        return Err("SPARE has not been unplugged since it was switched off, so this cannot show yet what Windows remembers. \
                    Unplug SPARE, wait five seconds, plug it back in exactly the same way (the same case, port and cable), \
                    wait a few seconds, and click This is SPARE: check it again."
            .to_string());
    }
    // (Switched on again by hand, without being unplugged: as a first check.)
    let switched_off_before = switched_off_before && unplugged;
    let named = if looks_like_drive_name(&d.model) {
        format!("Windows shows it by the SSD's own name (\"{}\")", d.model)
    } else if !d.model.is_empty() && !BRIDGE_NAMES.iter().any(|b| d.model.to_lowercase().contains(b)) {
        format!("Windows shows it as \"{}\"", d.model)
    } else {
        format!(
            "Windows shows it as \"{}\", which is the name of the chip in the USB case, not of the SSD: Windows may know \
             whatever disk is in this case by the case",
            d.model
        )
    };
    let (verdict, text) = if d.removable {
        (
            "stop",
            format!(
                "Windows treats a disk plugged in this way as removable media, and its setting does not keep removable media \
                 switched off: the recovered SSD would not be protected plugged in like this. Use another USB case or \
                 adapter (one Windows does not call removable), or a SATA port inside the PC that allows plugging in while it \
                 runs, and do the rehearsal again with it. (Windows shows the disk as \"{}\".)",
                d.model
            ),
        )
    } else if st.offline {
        (
            "passed",
            if switched_off_before {
                "SPARE arrived switched off: Windows remembers this disk switched off, plugged in this way. The recovered SSD \
                 will arrive switched off in the same case too - whether Windows takes it for SPARE or as a disk it has never \
                 seen. SPARE stays switched off until the copies need it: Start switches it on then. Now unplug SPARE."
                    .to_string()
            } else {
                "SPARE arrived switched off: Windows' setting works for a disk plugged in this way, so the recovered SSD will \
                 arrive switched off too. SPARE stays switched off until the copies need it: Start switches it on then. Now \
                 unplug SPARE."
                    .to_string()
            },
        )
    } else if switched_off_before {
        (
            "stop",
            format!(
                "SPARE arrived switched on again, although it was switched off and Windows was asked to remember that: \
                 Windows does not keep it for a disk plugged in this way, so the recovered SSD could arrive switched on. \
                 Don't plug the recovered SSD in like this: use another USB case or adapter, and do the rehearsal again \
                 with it (or ask Claude). ({named}.)"
            ),
        )
    } else {
        (
            "switch-off",
            format!(
                "SPARE arrived switched on: Windows has seen it before and remembers it switched on. {named}, so the recovered \
                 SSD in it could arrive switched on too. Click Switch SPARE off: the program switches it off and read-only, \
                 and asks Windows to remember that. Then unplug SPARE, plug it back in exactly the same way, and check it \
                 again: arriving switched off then shows Windows keeps it, and the recovered SSD will arrive switched off \
                 in this case too."
            ),
        )
    };
    {
        let mut a = lock(app);
        a.settings.insert("spare_disk".to_string(), id);
        a.settings.insert("spare_number".to_string(), n.to_string());
        if let Some(l) = d.drives.iter().find(|v| v.label.eq_ignore_ascii_case("SPARE")).or(d.drives.first()).map(|v| v.letter.clone()) {
            a.settings.insert("spare_letter".to_string(), l);
        }
        a.settings.insert("rehearsal".to_string(), verdict.to_string());
        a.settings.insert("rehearsal_text".to_string(), text.clone());
        a.settings.insert("rehearsal_when".to_string(), local_now());
        a.settings.remove("rehearsal_unplugged");
        a.save();
    }
    Ok(format!("\"verdict\":{},\"message\":{}", json_str(verdict), json_str(&text)))
}

/// Is the program watching for SPARE to be unplugged?
static WATCHING_UNPLUG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Watch, every second for up to three hours, for SPARE (disk `n`, `id`) to
/// be unplugged after Switch SPARE off: seen gone (twice running, so that a
/// moment Windows would not answer is not taken for it), the check that
/// follows shows what Windows remembers.
fn watch_unplug(app: &Arc<Mutex<App>>, n: u32, id: &str) {
    if !cfg!(windows) || WATCHING_UNPLUG.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    let id = id.to_string();
    std::thread::spawn(move || {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3 * 3600);
        let mut gone = 0;
        while std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if lock(&app).setting("rehearsal") != "switch-off" {
                break;
            }
            let here = crate::win::disk_identity(n).is_some_and(|(m, s)| format!("{m}|{s}") == id);
            gone = if here { 0 } else { gone + 1 };
            if gone >= 2 {
                let mut a = lock(&app);
                a.settings.insert("rehearsal_unplugged".to_string(), "1".to_string());
                a.save();
                break;
            }
        }
        WATCHING_UNPLUG.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

/// Switch SPARE off, remembered (the rehearsal's middle step).
pub(super) fn spare_off(app: &Arc<Mutex<App>>, n: u32) -> Result<String, String> {
    need_admin(app)?;
    let d = crate::win::disks().into_iter().find(|d| d.number == n).ok_or_else(|| format!("disk {n} is not plugged in"))?;
    is_spare(app, &d)?;
    if could_be_recovered(app, n) {
        return Err(format!("disk {n} looks like a copy of the old drive, not SPARE"));
    }
    crate::guard::switch_off_remembered(n)?;
    let text = "SPARE is switched off and read-only now, and Windows was asked to remember that. Unplug SPARE, wait five \
                seconds, plug it back in exactly the same way (the same case, port and cable), wait a few seconds, and click \
                This is SPARE: check it."
        .to_string();
    {
        let mut a = lock(app);
        a.settings.insert("rehearsal".to_string(), "switch-off".to_string());
        a.settings.insert("rehearsal_text".to_string(), text.clone());
        a.settings.insert("rehearsal_when".to_string(), local_now());
        a.settings.remove("rehearsal_unplugged");
        a.save();
    }
    watch_unplug(app, n, &format!("{}|{}", d.model, d.size));
    Ok(format!("\"message\":{}", json_str(&text)))
}

/// USB-to-SSD chips: a disk Windows shows by one of these names is shown by
/// its USB case, not by the SSD in it.
const BRIDGE_NAMES: &[&str] = &[
    "asmedia", "asm1", "asm2", "jmicron", "jms5", "jms58", "realtek", "rtl92", "sata bridge", "usb3.0", "usb 3.0", "usb3.1",
    "generic", "external", "mass storage", "storage device", "ugreen", "orico", "sabrent ec", "via labs", "vl71", "inateck",
    "startech",
];

trait SomeOrNotWindows {
    fn is_some_or_not_windows(&self) -> bool;
}

impl<T> SomeOrNotWindows for Option<T> {
    fn is_some_or_not_windows(&self) -> bool {
        self.is_some() || !cfg!(windows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_survive_being_kept() {
        let mut s = State::new();
        s.get("ready").state = "done".to_string();
        s.get("ready").summary = vec!["one".to_string(), "two, with\ttab".to_string()];
        s.get("image").state = "running".to_string();
        let back = State::from_text(&s.text());
        assert_eq!(back.stages[0].state, "done");
        assert_eq!(back.stages[0].summary, vec!["one".to_string(), "two, with\ttab".to_string()]);
        assert_eq!(back.stages[1].state, "stopped", "a stage running when the program closed did not finish");
        assert_eq!(back.resume_at(), 1);
        assert_eq!(State::new().resume_at(), 0);
    }

    #[test]
    fn drive_names_are_told_from_adapter_names() {
        assert!(looks_like_drive_name("Samsung SSD 870 EVO 1TB"));
        assert!(looks_like_drive_name("CT1000MX500SSD1"));
        assert!(!looks_like_drive_name("ASMedia ASM1153E"));
        assert!(!looks_like_drive_name("USB3.0 SATA Bridge"));
        assert!(!looks_like_drive_name("Generic External"));
        assert!(!looks_like_drive_name(""));
    }
}
