//! The double-click front end: a page in the PC's own web browser, served by
//! this program to this PC only (127.0.0.1, behind a random key in the web
//! address), that walks through the whole job one step at a time.
//!
//! Double-clicking the program starts a copy of it with administrator
//! rights (Windows asks first: reading the recovered SSD directly needs
//! them) in a window of its own, then opens the page in Microsoft Edge's
//! app window (a window of its own, no tabs; the PC's usual browser if Edge
//! is not there). Each step that takes a
//! while runs as a "job" in the background; the page asks how it is going
//! once a second. What each step found is kept in the results folder, so
//! closing the program and opening it again loses nothing.

use crate::fmt::{commas, json_str, size};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

mod auto;
mod finalise;
mod places;

const PAGE: &str = include_str!("ui.html");
/// The address stays the same from one run to the next where it can.
/// (Recovery Probe itself uses 47115, so both can run.)
pub const DEFAULT_PORT: u16 = 47116;

pub struct Config {
    pub port: u16,
    pub token: String,
    pub open_browser: bool,
    /// Disk images offered as if they were disks (for testing the page).
    pub test_disks: Vec<String>,
    /// Where results and notes go (default: the program's folder).
    pub home: Option<PathBuf>,
}

/// A random key for the web address.
pub fn new_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

// ---------------------------------------------------------------- jobs

#[derive(Clone, Default)]
struct Job {
    id: u64,
    kind: String,
    title: String,
    lines: VecDeque<String>,
    summary: Vec<String>,
    outputs: Vec<String>,
    /// JSON for the page (what the step found, in detail).
    data: String,
    done: u64,
    total: u64,
    /// "running", "done", "failed" or "stopped".
    state: String,
    error: String,
    started_at: String,
    seconds: u64,
    started: Option<Instant>,
    /// When the present stretch of counting (done of total) began.
    phase: Option<Instant>,
    /// How fast this stretch is getting on, lately: the time left is worked
    /// out from it.
    pace: Pace,
    stop: Arc<AtomicBool>,
}

/// How fast a job is getting through its work, over the last minute and a
/// half: the time left is worked out from that, not from the whole run. (A
/// step that carries on after a stop rushes through what was done before;
/// copying slows down on big videos and speeds up on small photos; and a
/// place the SSD struggles to read stalls it for a while.)
#[derive(Clone, Debug, Default)]
struct Pace {
    /// (seconds into the stretch, done then): one a second at most.
    samples: VecDeque<(f64, u64)>,
}

impl Pace {
    /// The speed is taken over this many seconds back,
    const WINDOW: f64 = 90.0;
    /// and only once it has this many to go on.
    const LEAST: f64 = 15.0;

    fn note(&mut self, at: f64, done: u64) {
        if self.samples.back().is_some_and(|&(t, _)| at - t < 1.0) {
            return;
        }
        self.samples.push_back((at, done));
        while self.samples.len() > 2 && self.samples.front().is_some_and(|&(t, _)| at - t > Self::WINDOW * 2.0) {
            self.samples.pop_front();
        }
    }

    /// Done per second lately (None: too soon to tell, or nothing moved).
    fn rate(&self, now: f64, done: u64) -> Option<f64> {
        let &(t0, d0) = self.samples.iter().find(|&&(t, _)| now - t <= Self::WINDOW)?;
        let span = now - t0;
        (span >= Self::LEAST && done > d0).then(|| (done - d0) as f64 / span)
    }
}

impl Job {
    fn json(&self, lines: usize) -> String {
        let secs = match (self.state.as_str(), self.started) {
            ("running", Some(t)) => t.elapsed().as_secs(),
            _ => self.seconds,
        };
        let skip = self.lines.len().saturating_sub(lines);
        Obj::new()
            .n("id", self.id)
            .s("kind", &self.kind)
            .s("title", &self.title)
            .s("state", &self.state)
            .s("error", &self.error)
            .s("started_at", &self.started_at)
            .n("seconds", secs)
            .n("done", self.done)
            .n("total", self.total)
            .n("phase_seconds", match (self.state.as_str(), self.phase) {
                ("running", Some(t)) => t.elapsed().as_secs(),
                _ => 0,
            })
            .raw(
                "rate",
                &match (self.state.as_str(), self.phase) {
                    ("running", Some(t)) => self.pace.rate(t.elapsed().as_secs_f64(), self.done),
                    _ => None,
                }
                .map(|r| format!("{r:.3}"))
                .unwrap_or_else(|| "null".to_string()),
            )
            .raw("lines", &strings(self.lines.iter().skip(skip)))
            .raw("summary", &strings(self.summary.iter()))
            .raw("outputs", &strings(self.outputs.iter()))
            .raw("data", if self.data.is_empty() { "null" } else { &self.data })
            .end()
    }
}

/// What a job's work hands back.
#[derive(Default)]
pub struct Outcome {
    pub summary: Vec<String>,
    pub outputs: Vec<String>,
    pub data: String,
    pub stopped: bool,
    /// Finished, but something needs the person's attention.
    pub problem: Option<String>,
}

/// Handed to a job's work: report progress, see whether to stop.
#[derive(Clone)]
pub struct Ctx {
    app: Arc<Mutex<App>>,
    id: u64,
    pub stop: Arc<AtomicBool>,
}

impl Ctx {
    fn with(&self, f: impl FnOnce(&mut Job)) {
        let mut a = lock(&self.app);
        if let Some(j) = a.job.as_mut().filter(|j| j.id == self.id) {
            f(j);
        }
    }
    pub fn line(&self, s: &str) {
        self.with(|j| {
            for l in s.lines() {
                j.lines.push_back(l.to_string());
            }
            while j.lines.len() > 400 {
                j.lines.pop_front();
            }
        });
    }
    pub fn amount(&self, done: u64, total: u64) {
        self.with(|j| {
            // A new stretch of counting: going back, or a new total from naught.
            if j.phase.is_none() || done < j.done || (done == 0 && total != j.total) {
                j.phase = Some(Instant::now());
                j.pace = Pace::default();
            }
            j.done = done;
            j.total = total;
            if let Some(t) = j.phase {
                j.pace.note(t.elapsed().as_secs_f64(), done);
            }
        });
    }
    pub fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------- the app

struct App {
    elevated: bool,
    home: PathBuf,
    results: PathBuf,
    settings: HashMap<String, String>,
    job: Option<Job>,
    last: HashMap<String, Job>,
    next_id: u64,
    test_disks: Vec<String>,
    /// Start's stages (auto.rs).
    auto: auto::State,
}

fn lock(app: &Arc<Mutex<App>>) -> MutexGuard<'_, App> {
    // A job that panicked while holding the lock must not stop the page.
    app.lock().unwrap_or_else(|e| e.into_inner())
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n").replace('\r', "\\r")
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl App {
    fn state_file(&self) -> PathBuf {
        self.results.join("recovery-probe-state.txt")
    }

    fn save(&self) {
        let mut t = String::from("# What Recovery Probe's steps (and Automatic Recovery Probe's) found last time (read back when it starts).\r\n");
        let mut keys: Vec<&String> = self.settings.keys().collect();
        keys.sort();
        for k in keys {
            t.push_str(&format!("S\t{}\t{}\r\n", escape(k), escape(&self.settings[k])));
        }
        let mut kinds: Vec<&String> = self.last.keys().collect();
        kinds.sort();
        for k in kinds {
            let j = &self.last[k];
            let f = |name: &str, v: &str| format!("J\t{}\t{name}\t{}\r\n", escape(k), escape(v));
            t.push_str(&f("title", &j.title));
            t.push_str(&f("state", &j.state));
            t.push_str(&f("error", &j.error));
            t.push_str(&f("started_at", &j.started_at));
            t.push_str(&f("seconds", &j.seconds.to_string()));
            t.push_str(&f("done", &j.done.to_string()));
            t.push_str(&f("total", &j.total.to_string()));
            for s in &j.summary {
                t.push_str(&f("summary", s));
            }
            for s in &j.outputs {
                t.push_str(&f("output", s));
            }
            for s in j.lines.iter().skip(j.lines.len().saturating_sub(30)) {
                t.push_str(&f("line", s));
            }
            t.push_str(&f("data", &j.data));
        }
        let _ = std::fs::create_dir_all(&self.results);
        // Written so that a power cut leaves the old state or the new one, whole.
        let _ = crate::sys::write_safely(&self.state_file(), t.as_bytes());
    }

    fn load(&mut self) {
        let Ok(text) = std::fs::read_to_string(self.state_file()) else { return };
        for line in text.lines() {
            let f: Vec<&str> = line.splitn(4, '\t').collect();
            match f.as_slice() {
                ["S", k, v] => {
                    self.settings.insert(unescape(k), unescape(v));
                }
                ["J", kind, name, v] => {
                    let j = self.last.entry(unescape(kind)).or_insert_with(|| Job { kind: unescape(kind), ..Default::default() });
                    let v = unescape(v);
                    match *name {
                        "title" => j.title = v,
                        "state" => j.state = v,
                        "error" => j.error = v,
                        "started_at" => j.started_at = v,
                        "seconds" => j.seconds = v.parse().unwrap_or(0),
                        "done" => j.done = v.parse().unwrap_or(0),
                        "total" => j.total = v.parse().unwrap_or(0),
                        "summary" => j.summary.push(v),
                        "output" => j.outputs.push(v),
                        "line" => j.lines.push_back(v),
                        "data" => j.data = v,
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    fn setting(&self, k: &str) -> String {
        self.settings.get(k).cloned().unwrap_or_default()
    }
}

/// A step's work, checked and ready to run (as a job of its own, or as one
/// stage of Start: auto.rs).
type Work = Box<dyn FnOnce(&Ctx) -> Result<Outcome, String> + Send + 'static>;

/// A step ready to run: what kind of step, its title, and its work.
struct Prepared {
    pub kind: &'static str,
    pub title: &'static str,
    pub work: Work,
}

fn prepared(kind: &'static str, title: &'static str, work: impl FnOnce(&Ctx) -> Result<Outcome, String> + Send + 'static) -> Prepared {
    Prepared { kind, title, work: Box::new(work) }
}

fn start_prepared(app: &Arc<Mutex<App>>, p: Result<Prepared, String>) -> Result<u64, String> {
    let p = p?;
    start_job(app, p.kind, p.title, p.work)
}

/// Start a job unless one is running.
fn start_job(
    app: &Arc<Mutex<App>>,
    kind: &str,
    title: &str,
    work: impl FnOnce(&Ctx) -> Result<Outcome, String> + Send + 'static,
) -> Result<u64, String> {
    let ctx = {
        let mut a = lock(app);
        if let Some(j) = &a.job {
            if j.state == "running" {
                return Err(format!("\"{}\" is still running. Wait for it to finish, or stop it first.", j.title));
            }
        }
        a.next_id += 1;
        let id = a.next_id;
        let stop = Arc::new(AtomicBool::new(false));
        a.job = Some(Job {
            id,
            kind: kind.to_string(),
            title: title.to_string(),
            state: "running".to_string(),
            started_at: local_now(),
            started: Some(Instant::now()),
            stop: stop.clone(),
            ..Default::default()
        });
        Ctx { app: app.clone(), id, stop }
    };
    let id = ctx.id;
    crate::live::watch(true);
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&ctx)));
        crate::live::watch(false);
        let mut a = lock(&ctx.app);
        let Some(mut j) = a.job.take() else { return };
        if j.id != ctx.id {
            a.job = Some(j);
            return;
        }
        j.seconds = j.started.map(|t| t.elapsed().as_secs()).unwrap_or(0);
        match result {
            Ok(Ok(o)) => {
                j.state = if o.stopped {
                    "stopped"
                } else if o.problem.is_some() {
                    "failed"
                } else {
                    "done"
                }
                .to_string();
                j.error = o.problem.unwrap_or_default();
                j.summary = o.summary;
                j.outputs = o.outputs;
                j.data = o.data;
            }
            Ok(Err(e)) => {
                j.state = "failed".to_string();
                j.error = e;
            }
            Err(p) => {
                j.state = "failed".to_string();
                let why = p
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                j.error = format!("the program hit an internal error ({why}) - please report it");
            }
        }
        a.last.insert(j.kind.clone(), j.clone());
        a.job = Some(j);
        a.save();
    });
    Ok(id)
}

fn local_now() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as f64).unwrap_or(0.0);
    crate::fmt::local_datetime(now)
}

fn stamp() -> String {
    crate::fmt::now_stamp()
}

/// `dir/name.ext`, or `dir/name-2.ext`... when a run in the same minute
/// already used that name.
fn fresh(dir: &Path, name: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{name}{ext}"));
    if !first.exists() {
        return first;
    }
    (2..).map(|k| dir.join(format!("{name}-{k}{ext}"))).find(|p| !p.exists()).expect("a free name")
}

// ---------------------------------------------------------------- JSON

struct Obj(String);

impl Obj {
    fn new() -> Obj {
        Obj(String::from("{"))
    }
    fn raw(mut self, k: &str, v: &str) -> Obj {
        if self.0.len() > 1 {
            self.0.push(',');
        }
        self.0.push_str(&json_str(k));
        self.0.push(':');
        self.0.push_str(v);
        self
    }
    fn s(self, k: &str, v: &str) -> Obj {
        let v = json_str(v);
        self.raw(k, &v)
    }
    fn n(self, k: &str, v: u64) -> Obj {
        self.raw(k, &v.to_string())
    }
    fn b(self, k: &str, v: bool) -> Obj {
        self.raw(k, if v { "true" } else { "false" })
    }
    fn end(mut self) -> String {
        self.0.push('}');
        self.0
    }
}

fn strings<'a>(it: impl Iterator<Item = &'a String>) -> String {
    format!("[{}]", it.map(|s| json_str(s)).collect::<Vec<_>>().join(","))
}

fn arr(items: Vec<String>) -> String {
    format!("[{}]", items.join(","))
}

fn opt_bool(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "true",
        Some(false) => "false",
        None => "null",
    }
}

// ---------------------------------------------------------------- HTTP

struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'+' {
            out.push(b' ');
        } else if b[i] == b'%' && i + 3 <= b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() {
            let hex = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
            out.push(hex(b[i + 1]) * 16 + hex(b[i + 2]));
            i += 2;
        } else {
            out.push(b[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_form(s: &str) -> HashMap<String, String> {
    s.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (url_decode(k), url_decode(v)),
            None => (url_decode(p), String::new()),
        })
        .collect()
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Request> {
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end;
    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "closed"));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            head_end = p;
            break;
        }
        if buf.len() > 64 * 1024 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "request too large"));
        }
    }
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split(' ');
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let mut headers = HashMap::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if len > 1 << 20 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "body too large"));
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(len);
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), parse_form(q)),
        None => (target, HashMap::new()),
    };
    Ok(Request { method, path: url_decode(&path), query, headers, body })
}

fn respond(stream: &mut TcpStream, status: u16, ctype: &str, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn json_ok(extra: &str) -> String {
    if extra.is_empty() {
        "{\"ok\":true}".to_string()
    } else {
        format!("{{\"ok\":true,{extra}}}")
    }
}

fn json_err(e: &str) -> String {
    format!("{{\"ok\":false,\"error\":{}}}", json_str(e))
}

// ---------------------------------------------------------------- serving

/// The size of the disk the chosen mapfile describes (0: no mapfile), kept
/// where the window-closing handler below can see it.
static MAP_SIZE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn map_size_now() -> Option<u64> {
    Some(MAP_SIZE.load(Ordering::Relaxed)).filter(|&s| s > 0)
}

/// The number of a plugged-in disk that is the recovered SSD: the copy the
/// mapfile describes (mapcheck::is_the_copy - not just any drive of the old
/// drive's size, such as SPARE), or, with no mapfile chosen yet, a disk that
/// holds an exFAT volume. While one is attached, Windows stays set to leave
/// new disks switched off: if it dropped off and came back, it must arrive
/// switched off again.
pub fn recovered_disk_attached(map_size: Option<u64>) -> Option<u32> {
    for d in crate::win::disks() {
        if d.system {
            continue;
        }
        let Ok(src) = crate::disk::open_source(&format!(r"\\.\PhysicalDrive{}", d.number)) else { continue };
        let table = crate::gpt::read(&*src, &[]);
        let it = match map_size {
            Some(m) => crate::mapcheck::is_the_copy(&*src, &table, m),
            None => crate::mapcheck::find_volume_quick(&*src, &table).is_ok(),
        };
        if it {
            return Some(d.number);
        }
    }
    None
}

/// Run the page until the program is closed.
/// When the program's window is closed (or the PC shuts down) while Windows
/// is still set to leave new disks switched off, put that setting back -
/// but only once the recovered SSD is no longer plugged in. (Otherwise every
/// drive plugged in later would arrive switched off too; with the SSD still
/// attached, the setting stays and the page offers to put it back when the
/// program is started again.) Windows allows a few seconds for this, and
/// diskpart needs about one.
#[cfg(windows)]
fn restore_setting_on_close() {
    extern "system" fn handler(ctrl: u32) -> i32 {
        // Ctrl+C, Ctrl+Break, the window closed, signing out, shutting down.
        if matches!(ctrl, 0 | 1 | 2 | 5 | 6)
            && crate::cli::policy_to_restore().is_some()
            && recovered_disk_attached(map_size_now()).is_none()
        {
            let _ = crate::cli::guard_restore();
        }
        0 // then end the program as usual
    }
    extern "system" {
        fn SetConsoleCtrlHandler(handler: Option<extern "system" fn(u32) -> i32>, add: i32) -> i32;
    }
    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }
}

#[cfg(not(windows))]
fn restore_setting_on_close() {}

pub fn serve(cfg: Config) -> i32 {
    let listener = match TcpListener::bind(("127.0.0.1", cfg.port)) {
        Ok(l) => l,
        Err(e) => {
            println!("Could not start the page on port {}: {e}", cfg.port);
            return 1;
        }
    };
    restore_setting_on_close();
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(cfg.port);
    if port != DEFAULT_PORT {
        let _ = std::fs::write(port_note(), port.to_string());
    }
    let home = cfg.home.clone().unwrap_or_else(crate::cli::home_dir);
    let results = home.join("results");
    let mut app = App {
        elevated: crate::win::is_elevated(),
        home: home.clone(),
        results: results.clone(),
        settings: HashMap::new(),
        job: None,
        last: HashMap::new(),
        next_id: 0,
        test_disks: cfg.test_disks.clone(),
        auto: auto::State::new(),
    };
    app.load();
    auto::load(&mut app);
    MAP_SIZE.store(mapfile_domain(&app.setting("mapfile")).map(|(s, e)| e.saturating_sub(s)).unwrap_or(0), Ordering::Relaxed);
    // A job that was running when the program was closed did not finish.
    for j in app.last.values_mut() {
        if j.state == "running" {
            j.state = "stopped".to_string();
        }
    }
    let elevated = app.elevated;
    let app = Arc::new(Mutex::new(app));
    let url = format!("http://127.0.0.1:{port}/{}/", cfg.token);
    println!("{} {} is running in your web browser.", crate::APP, crate::VERSION);
    println!();
    println!("If the page did not open, or its window was closed, copy this address into a browser:");
    println!("  {url}");
    println!();
    if !elevated {
        println!("(Without administrator rights it cannot read the recovered SSD itself; it can still check copied files.)");
    }
    println!("Keep this window open while you use it. Close it when you are done.");
    if cfg.open_browser {
        // (Edge's own window only without administrator rights: with them,
        // the usual browser, through Explorer.)
        crate::win::open_app_window(&url);
    }
    let token = Arc::new(cfg.token.clone());
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let app = app.clone();
        let token = token.clone();
        std::thread::spawn(move || {
            if let Ok(req) = read_request(&mut stream) {
                handle(&app, &token, port, req, &mut stream);
            }
        });
    }
    0
}

fn handle(app: &Arc<Mutex<App>>, token: &str, port: u16, req: Request, stream: &mut TcpStream) {
    // Only pages from this program, on this PC, may talk to it.
    let host = req.headers.get("host").map(|h| h.to_lowercase()).unwrap_or_default();
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return respond(stream, 403, "text/plain", b"wrong host");
    }
    if req.path == "/ping" {
        return respond(stream, 200, "text/plain", crate::APP.as_bytes());
    }
    let prefix = format!("/{token}/");
    let Some(rest) = req.path.strip_prefix(&prefix).map(|s| s.to_string()) else {
        return respond(
            stream,
            404,
            "text/html; charset=utf-8",
            format!("<p>This address is not complete. Use the address shown in {}'s window.</p>", crate::APP).as_bytes(),
        );
    };
    if req.method == "POST" {
        // A page elsewhere cannot set this header without asking first,
        // and this program never says yes.
        if req.headers.get("x-key").map(|k| k.as_str()) != Some(token) {
            return respond(stream, 403, "text/plain", b"missing key");
        }
    }
    let q = |k: &str| req.query.get(k).cloned().unwrap_or_default();
    let (status, ctype, body): (u16, &str, String) = match (req.method.as_str(), rest.as_str()) {
        ("GET", "") => (200, "text/html; charset=utf-8", PAGE.replace("__KEY__", token)),
        ("GET", "api/state") => (200, "application/json", state_json(app)),
        ("GET", "api/disks") => (200, "application/json", disks_json(app)),
        ("GET", "api/drives") => (200, "application/json", drives_json()),
        ("GET", "api/list") => (200, "application/json", list_json(&q("path"), &q("show"))),
        ("GET", "api/mapinfo") => (200, "application/json", mapinfo_json(&q("path"))),
        ("GET", "api/find-mapfiles") => (200, "application/json", find_mapfiles_json(app)),
        ("GET", "api/checklist") => (200, "application/json", checklist_page().to_string()),
        ("GET", "api/checklist-state") => (200, "application/json", checklist_sync(app)),
        ("GET", "api/places") => {
            let map = lock(app).setting("mapfile");
            let (image, work, _) = suggested_folders(app, &map);
            (200, "application/json", places::places_json(app, &image, &work))
        }
        ("GET", "api/tools") => (200, "application/json", crate::tools::status_json(&lock(app).home.clone())),
        ("GET", "api/ready") => {
            if q("fresh") == "1" {
                crate::tools::forget();
            }
            (200, "application/json", ready_json(app, q("fresh") == "1"))
        }
        ("POST", "api/do") => {
            let form = parse_form(&String::from_utf8_lossy(&req.body));
            (200, "application/json", act(app, &form))
        }
        _ => (404, "text/plain", "not found".to_string()),
    };
    respond(stream, status, ctype, body.as_bytes());
}

// ---------------------------------------------------------------- answers

fn state_json(app: &Arc<Mutex<App>>) -> String {
    let a = lock(app);
    let mut settings: Vec<(&String, &String)> = a.settings.iter().collect();
    settings.sort();
    let mut s = Obj::new();
    for (k, v) in settings {
        s = s.s(k, v);
    }
    let mut last = Obj::new();
    let mut kinds: Vec<&String> = a.last.keys().collect();
    kinds.sort();
    for k in kinds {
        last = last.raw(k, &a.last[k].json(30));
    }
    let restore = crate::cli::policy_to_restore().map(|p| json_str(p.keyword())).unwrap_or_else(|| "null".to_string());
    Obj::new()
        .s("app", crate::APP)
        .s("version", crate::VERSION)
        .b("elevated", a.elevated)
        .b("windows", cfg!(windows))
        .s("home", &a.home.display().to_string())
        .s("results", &a.results.display().to_string())
        .raw("settings", &s.end())
        .raw("policy_to_restore", &restore)
        .raw("job", &a.job.as_ref().map(|j| j.json(60)).unwrap_or_else(|| "null".to_string()))
        .raw("live", &crate::live::snapshot().map(|b| b.json()).unwrap_or_else(|| "null".to_string()))
        .raw("last", &last.end())
        .raw("auto", &auto::state_json(&a))
        .end()
}

/// The mapfile's disk size, for telling the recovered SSD from other disks.
fn mapfile_domain(path: &str) -> Option<(u64, u64)> {
    let text = std::fs::read(path).ok()?;
    let m = crate::mapfile::parse(&String::from_utf8_lossy(&text)).ok()?;
    Some((m.domain_start, m.domain_end))
}

fn describe_disk(spec: &str, map_size: Option<u64>) -> String {
    let src = match crate::disk::open_source(spec) {
        Ok(s) => s,
        Err(e) => {
            let why = if e.kind() == std::io::ErrorKind::PermissionDenied {
                "reading it needs administrator rights".to_string()
            } else {
                e.to_string()
            };
            return Obj::new().s("error", &why).end();
        }
    };
    let table = crate::gpt::read(&*src, &[]);
    let parts: Vec<String> = table
        .partitions
        .iter()
        .map(|p| Obj::new().s("name", &p.name).s("kind", p.kind()).n("start", p.start).n("end", p.end).end())
        .collect();
    let original = table.original_disk_size();
    let found = crate::mapcheck::find_volume_quick(&*src, &table);
    // The copy ddrescue made: mapcheck::is_the_copy (with the volume, which
    // is looked for here anyway).
    let matches = map_size.map(|m| crate::mapcheck::copy_rule(original, src.size(), || found.is_ok(), m));
    let volume = match found {
        Ok((v, _)) => Obj::new()
            .s("label", &v.label)
            .n("offset", v.offset)
            .n("size", v.cluster_count as u64 * v.cluster_size)
            .n("serial", v.serial as u64)
            .end(),
        Err(_) => "null".to_string(),
    };
    Obj::new()
        .s("scheme", table.scheme)
        .raw("table_disk_size", &original.map(|o| o.to_string()).unwrap_or_else(|| "null".to_string()))
        .raw("partitions", &arr(parts))
        .raw("volume", &volume)
        .raw("matches_mapfile", opt_bool(matches))
        .end()
}

fn disks_json(app: &Arc<Mutex<App>>) -> String {
    let (map, tests, elevated) = {
        let a = lock(app);
        (a.setting("mapfile"), a.test_disks.clone(), a.elevated)
    };
    let map_size = mapfile_domain(&map).map(|(s, e)| e.saturating_sub(s));
    let mut out = Vec::new();
    for d in crate::win::disks() {
        let spec = format!(r"\\.\PhysicalDrive{}", d.number);
        let st = crate::guard::state(d.number).ok();
        let drives: Vec<String> = d
            .drives
            .iter()
            .map(|v| Obj::new().s("letter", &v.letter).s("label", &v.label).s("fs", &v.fs).n("total", v.total).n("free", v.free).end())
            .collect();
        let detail = if d.system || !elevated { "null".to_string() } else { describe_disk(&spec, map_size) };
        out.push(
            Obj::new()
                .n("number", d.number as u64)
                .s("spec", &spec)
                .n("size", d.size)
                .s("model", &d.model)
                .s("bus", &d.bus)
                .b("system", d.system)
                .b("removable", d.removable)
                .raw("offline", opt_bool(st.as_ref().map(|s| s.offline)))
                .raw("read_only", opt_bool(st.as_ref().map(|s| s.read_only)))
                .raw("writable", opt_bool(st.as_ref().and_then(|s| s.writable)))
                .raw("drives", &arr(drives))
                .raw("detail", &detail)
                .end(),
        );
    }
    for (i, t) in tests.iter().enumerate() {
        let size = std::fs::metadata(t).map(|m| m.len()).unwrap_or(0);
        out.push(
            Obj::new()
                .n("number", 90 + i as u64)
                .s("spec", t)
                .n("size", size)
                .s("model", "test disk image")
                .s("bus", "file")
                .b("system", false)
                .b("offline", false)
                .b("read_only", true)
                .b("writable", false)
                .raw("drives", "[]")
                .raw("detail", &describe_disk(t, map_size))
                .b("test", true)
                .end(),
        );
    }
    arr(out)
}

fn drives_json() -> String {
    arr(crate::win::drives()
        .iter()
        .map(|d| {
            Obj::new()
                .s("letter", &d.letter)
                .s("label", &d.label)
                .s("fs", &d.fs)
                .n("total", d.total)
                .n("free", d.free)
                .s("kind", d.kind)
                .raw("disk", &d.disk.map(|n| n.to_string()).unwrap_or_else(|| "null".to_string()))
                .end()
        })
        .collect())
}

/// A folder's contents for the picker; "" lists the drives.
fn list_json(path: &str, show: &str) -> String {
    let files_too = show == "files";
    if path.is_empty() {
        let entries: Vec<String> = crate::win::drives()
            .iter()
            .filter(|d| d.kind != "cd" || d.total > 0)
            .map(|d| {
                let root = if cfg!(windows) { format!("{}\\", d.letter) } else { d.letter.clone() };
                let label = if d.label.is_empty() { String::new() } else { format!(" {}", d.label) };
                Obj::new()
                    .s("name", &format!("{}{label}", d.letter))
                    .s("path", &root)
                    .b("dir", true)
                    .n("free", d.free)
                    .n("total", d.total)
                    .end()
            })
            .collect();
        return Obj::new().s("path", "").s("parent", "").raw("entries", &arr(entries)).end();
    }
    let p = Path::new(path);
    let rd = match std::fs::read_dir(p) {
        Ok(r) => r,
        Err(e) => return json_err(&format!("could not open {path}: {e}")),
    };
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for e in rd.flatten().take(20_000) {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('$') || name == "System Volume Information" {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            dirs.push(name);
        } else if files_too {
            files.push((name, e.metadata().map(|m| m.len()).unwrap_or(0)));
        }
    }
    dirs.sort_by_key(|a| a.to_lowercase());
    files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    let mut entries: Vec<String> = dirs
        .iter()
        .map(|d| Obj::new().s("name", d).s("path", &p.join(d).display().to_string()).b("dir", true).end())
        .collect();
    entries.extend(files.iter().map(|(f, n)| {
        Obj::new().s("name", f).s("path", &p.join(f).display().to_string()).b("dir", false).n("size", *n).end()
    }));
    let parent = p.parent().map(|x| x.display().to_string()).unwrap_or_default();
    Obj::new()
        .s("path", &p.display().to_string())
        .s("parent", &parent)
        .raw("free", &crate::win::free_space(p).map(|f| f.to_string()).unwrap_or_else(|| "null".to_string()))
        .raw("entries", &arr(entries))
        .end()
}

fn mapinfo_json(path: &str) -> String {
    let text = match std::fs::read(path) {
        Ok(t) => t,
        Err(e) => return json_err(&format!("could not read {path}: {e}")),
    };
    let m = match crate::mapfile::parse(&String::from_utf8_lossy(&text)) {
        Ok(m) => m,
        Err(e) => return json_err(&format!("this is not a ddrescue mapfile the program can read: {e}")),
    };
    let bad = m.bad_areas();
    let bad_bytes: u64 = bad.iter().map(|a| a.len()).sum();
    let mut by: Vec<(char, u64, u64)> = Vec::new();
    for a in &bad {
        match by.iter_mut().find(|s| s.0 == a.status) {
            Some(s) => {
                s.1 += 1;
                s.2 += a.len();
            }
            None => by.push((a.status, 1, a.len())),
        }
    }
    let by: Vec<String> = by
        .iter()
        .map(|(st, n, b)| Obj::new().s("status", &st.to_string()).s("name", crate::mapfile::status_name(*st)).n("count", *n).n("bytes", *b).end())
        .collect();
    let areas: Vec<String> = bad.iter().take(20_000).map(|a| format!("[{},{},{}]", a.start, a.end, json_str(&a.status.to_string()))).collect();
    let sizes: Vec<u64> = bad.iter().map(|a| a.len()).collect();
    let mut sorted = sizes.clone();
    sorted.sort_unstable();
    Obj::new()
        .b("ok", true)
        .s("path", path)
        .s("version", &m.version)
        .s("start_time", &m.start_time)
        .s("current_time", &m.current_time)
        .b("finished", m.finished)
        .n("domain_start", m.domain_start)
        .n("domain_end", m.domain_end)
        .n("read_bytes", m.rescued_bytes())
        .n("bad_bytes", bad_bytes)
        .n("bad_count", bad.len() as u64)
        .n("largest", sorted.last().copied().unwrap_or(0))
        .n("median", sorted.get(sorted.len() / 2).copied().unwrap_or(0))
        .raw("by_status", &arr(by))
        .raw("areas", &arr(areas))
        .raw("warnings", &strings(m.warnings.iter()))
        .end()
}

/// ddrescue mapfiles in the program's folder (and one level down).
fn find_mapfiles_json(app: &Arc<Mutex<App>>) -> String {
    let home = lock(app).home.clone();
    arr(find_mapfiles(&home)
        .iter()
        .map(|(p, size, fin, bad)| Obj::new().s("path", p).n("disk_size", *size).b("finished", *fin).n("bad_bytes", *bad).end())
        .collect())
}

/// The mapfile to use when none is chosen: of the finished ones, the one
/// with the fewest bytes never read (the last, most complete attempt).
fn best_mapfile(home: &Path) -> Option<String> {
    let all = find_mapfiles(home);
    let mut fin: Vec<&(String, u64, bool, u64)> = all.iter().filter(|m| m.2).collect();
    if fin.is_empty() {
        fin = all.iter().collect();
    }
    fin.sort_by_key(|m| m.3);
    fin.first().map(|m| m.0.clone())
}

/// (path, disk size, finished, bytes never read) of every mapfile found.
fn find_mapfiles(home: &Path) -> Vec<(String, u64, bool, u64)> {
    let home = home.to_path_buf();
    let mut found = Vec::new();
    let look = |dir: &Path, found: &mut Vec<(String, u64, bool, u64)>| {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if !p.is_file() {
                    continue;
                }
                let len = e.metadata().map(|m| m.len()).unwrap_or(0);
                if len == 0 || len > 64 << 20 {
                    continue;
                }
                // Look at the start first: most files here are not mapfiles.
                let mut head = [0u8; 16];
                let n = std::fs::File::open(&p).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
                if !head[..n].starts_with(b"# Mapfile") && !head[..n].starts_with(b"# Rescue Logf") {
                    continue;
                }
                let Ok(text) = std::fs::read(&p) else { continue };
                if let Ok(m) = crate::mapfile::parse(&String::from_utf8_lossy(&text)) {
                    let bad: u64 = m.bad_areas().iter().map(|a| a.len()).sum();
                    found.push((p.display().to_string(), m.domain_end.saturating_sub(m.domain_start), m.finished, bad));
                }
            }
        }
    };
    look(&home, &mut found);
    if let Ok(rd) = std::fs::read_dir(&home) {
        for e in rd.flatten() {
            if e.path().is_dir() && e.file_name() != "results" {
                look(&e.path(), &mut found);
            }
        }
    }
    found.sort();
    found
}

// ---------------------------------------------------------------- actions

fn need_admin(app: &Arc<Mutex<App>>) -> Result<(), String> {
    if lock(app).elevated {
        Ok(())
    } else {
        Err(format!(
            "This step needs administrator rights. Close {}'s window, double-click the program again and click Yes \
             when Windows asks.",
            crate::APP
        ))
    }
}

fn act(app: &Arc<Mutex<App>>, f: &HashMap<String, String>) -> String {
    let g = |k: &str| f.get(k).cloned().unwrap_or_default();
    let on = |k: &str| matches!(g(k).as_str(), "1" | "true" | "on" | "yes");
    let disk_number = |k: &str| -> Result<u32, String> {
        g(k).trim_start_matches(r"\\.\PhysicalDrive").parse().map_err(|_| "which disk?".to_string())
    };
    let r: Result<String, String> = match g("action").as_str() {
        "set" => {
            let mut a = lock(app);
            for (k, v) in f {
                if k != "action" {
                    a.settings.insert(k.clone(), v.clone());
                }
            }
            a.save();
            if let Some(m) = f.get("mapfile") {
                MAP_SIZE.store(mapfile_domain(m).map(|(s, e)| e.saturating_sub(s)).unwrap_or(0), Ordering::Relaxed);
            }
            Ok(String::new())
        }
        "stop" => {
            let a = lock(app);
            if let Some(j) = &a.job {
                j.stop.store(true, Ordering::Relaxed);
            }
            Ok(String::new())
        }
        "open" => {
            let p = g("path");
            if crate::win::open_folder(Path::new(&p)) {
                Ok(String::new())
            } else {
                Err(format!("could not open {p}"))
            }
        }
        "guard-prepare" => need_admin(app).and_then(|_| crate::cli::guard_prepare()).map(|m| format!("\"message\":{}", json_str(&m))),
        "guard-restore" => need_admin(app)
            .and_then(|_| match recovered_disk_attached(map_size_now()) {
                Some(n) => Err(format!(
                    "The recovered SSD (disk {n}) is still plugged in, so Windows stays set to leave new disks switched \
                     off: if it dropped off and came back, it must arrive switched off again. Switch it off and unplug it \
                     first, then click this again."
                )),
                None => crate::cli::guard_restore(),
            })
            .map(|m| format!("\"message\":{}", json_str(&m))),
        "guard-protect" => need_admin(app).and_then(|_| disk_number("disk")).and_then(|n| {
            crate::guard::protect(n, on("online")).map(|s| {
                format!(
                    "\"message\":{}",
                    json_str(&format!(
                        "Disk {n} is read-only{}.",
                        if s.offline { " and switched off" } else { " and switched on" }
                    ))
                )
            })
        }),
        "guard-online" => need_admin(app).and_then(|_| disk_number("disk")).and_then(|n| {
            if on("on") && !crate::guard::state(n)?.read_only && could_be_recovered(app, n) {
                return Err(format!(
                    "Disk {n} could be the recovered SSD and is not marked read-only, so it stays switched off: \
                     protect it first."
                ));
            }
            crate::guard::set_online(n, on("on")).map(|_| String::new())
        }),
        "guard-release" => need_admin(app).and_then(|_| disk_number("disk")).and_then(|n| {
            if could_be_recovered(app, n) {
                return Err(format!("Disk {n} could be the recovered SSD, so it stays as it is."));
            }
            crate::guard::release(n).map(|_| {
                format!("\"message\":{}", json_str(&format!("Disk {n} is switched on, and can be written to.")))
            })
        }),
        "selftest" => start_job(app, "selftest", "Checking the program works on this PC", |ctx| {
            crate::selftest::run(&mut |s: &str| ctx.line(s), &ctx.stop)
        })
        .map(|id| format!("\"job\":{id}")),
        "mapcheck" => start_map_check(app, &g("map"), &g("earlier"), &g("disk")).map(|id| format!("\"job\":{id}")),
        "rescue" => start_rescue(app, &g("disk"), &g("out"), on("everything"), on("lost")).map(|id| format!("\"job\":{id}")),
        "image" => start_image(app, &g("disk"), &g("out")).map(|id| format!("\"job\":{id}")),
        "verify" => start_verify(app, &g("folder")).map(|id| format!("\"job\":{id}")),
        "check-image" => start_check_image(app).map(|id| format!("\"job\":{id}")),
        "tick" => tick(app, &g("i"), on("on")).map(|st| format!("\"state\":{st}")),
        "isolate" => start_repair_step(app, "isolate", &g("folder"), "").map(|id| format!("\"job\":{id}")),
        "put-back" => start_repair_step(app, "put-back", &g("folder"), "").map(|id| format!("\"job\":{id}")),
        "undo-put-back" => start_repair_step(app, "undo", &g("folder"), &g("id")).map(|id| format!("\"job\":{id}")),
        "evaluate" => start_repair_step(app, "evaluate", &g("folder"), "").map(|id| format!("\"job\":{id}")),
        "scan" => {
            let kb: u64 = g("hole_kb").parse().unwrap_or(crate::formats::DEFAULT_HOLE_KB);
            start_scan(app, &g("folder"), on("holes"), kb, &g("join")).map(|id| format!("\"job\":{id}"))
        }
        "auto-start" => auto::start(app).map(|id| format!("\"job\":{id}")),
        "auto-start-over" => auto::start_over(app).map(|_| String::new()),
        "auto-rehearse" => disk_number("disk").and_then(|n| auto::rehearse(app, n)),
        "auto-spare-off" => disk_number("disk").and_then(|n| auto::spare_off(app, n)),
        "bring-back" => auto::start_bring_back(app, &g("from")).map(|id| format!("\"job\":{id}")),
        "tool-download" => start_tool_download(app, &g("tool")).map(|id| format!("\"job\":{id}")),
        "finalise" => finalise::start(app).map(|id| format!("\"job\":{id}")),
        "crash-restart-off" => need_admin(app)
            .and_then(|_| {
                if crate::winset::restarts_after_crash() == Some(true) {
                    crate::winset::set_restarts_after_crash(false)?;
                    let mut a = lock(app);
                    a.settings.insert("crash_restart_changed".to_string(), "1".to_string());
                    a.save();
                }
                Ok(())
            })
            .map(|_| format!("\"message\":{}", json_str("Windows no longer restarts by itself after a crash: it stops at the blue screen instead (Finalise puts this back)."))),
        "defender-ensure" => need_admin(app).map(|_| {
            let map = lock(app).setting("mapfile");
            let (si, sw, _) = suggested_folders(app, &map);
            let (i, w) = {
                let a = lock(app);
                (a.setting("auto_image_dir"), a.setting("auto_work"))
            };
            let lines = places::defender_ensure(
                app,
                if i.trim().is_empty() { &si } else { &i },
                if w.trim().is_empty() { &sw } else { &w },
            );
            format!("\"message\":{}", json_str(&lines.join(" ")))
        }),
        "aim-install" => need_admin(app).and_then(|_| aim_install(app)).map(|m| format!("\"message\":{}", json_str(&m))),
        "aim-test" => auto::start_aim_test(app).map(|id| format!("\"job\":{id}")),
        other => Err(format!("unknown action {other:?}")),
    };
    match r {
        Ok(extra) => json_ok(&extra),
        Err(e) => json_err(&e),
    }
}

fn is_test_disk(app: &Arc<Mutex<App>>, spec: &str) -> bool {
    lock(app).test_disks.iter().any(|t| t == spec)
}

/// Could disk `n` be the recovered SSD? (It is the copy the mapfile
/// describes, or it holds an exFAT volume, or it cannot be read.) Such a
/// disk is never made writable, nor switched on unless read-only. A drive
/// that is merely as big as the old one - SPARE, if it is a 1 TB drive -
/// is not: see mapcheck::is_the_copy.
fn could_be_recovered(app: &Arc<Mutex<App>>, n: u32) -> bool {
    let map = lock(app).setting("mapfile");
    let Ok(src) = crate::disk::open_source(&format!(r"\\.\PhysicalDrive{n}")) else { return true };
    let table = crate::gpt::read(&*src, &[]);
    if let Some((s, e)) = mapfile_domain(&map) {
        if crate::mapcheck::is_the_copy(&*src, &table, e - s) {
            return true;
        }
    }
    crate::mapcheck::find_volume_quick(&*src, &table).is_ok()
}

/// While anything reads the recovered SSD, Windows is set to leave new disks
/// switched off (normally it already is, from "Get Windows ready", and it
/// stays so until the SSD is unplugged): if the SSD comes unplugged and is
/// plugged in again (a cable knocked, a USB hiccup), it arrives switched
/// off, not open for Windows to write to. The setting is put back only by
/// "Put Windows' setting back", once the SSD is unplugged.
fn hold_new_disks_off(ctx: &Ctx, guard: bool) {
    if !guard || crate::cli::policy_to_restore().is_some() {
        return;
    }
    match crate::cli::guard_prepare() {
        Ok(_) => ctx.line(
            "Windows now leaves newly plugged-in disks switched off, so the recovered SSD stays protected even if it \
             is unplugged and plugged in again. This stays so until the SSD is unplugged.",
        ),
        Err(e) => ctx.line(&format!("(Could not set Windows to leave new disks switched off: {e})")),
    }
}

/// `\\.\PhysicalDriveN` -> N.
fn physical_number(spec: &str) -> Option<u32> {
    spec.strip_prefix(r"\\.\PhysicalDrive").and_then(|n| n.parse().ok())
}

/// A disk image file (made by step 4, or a test disk), not a disk.
fn is_image_file(spec: &str) -> bool {
    physical_number(spec).is_none() && Path::new(spec).is_file()
}

/// Does Windows treat this disk as removable media?
fn is_removable(spec: &str) -> bool {
    physical_number(spec).is_some_and(|n| crate::win::disks().iter().any(|d| d.number == n && d.removable))
}

const REMOVABLE_NOTE: &str = "Windows says this disk is removable media (like a USB stick). For removable disks its \
setting to leave new disks switched off does not work, so Windows could write to it whenever it is plugged in. Make \
the image (step 4) now, while it is read-only, and then unplug it; ask for help before plugging it in again.";

fn check_disk(app: &Arc<Mutex<App>>, spec: &str) -> Result<(), String> {
    if spec.is_empty() {
        return Err("Choose the recovered SSD first (step 3).".to_string());
    }
    if !is_test_disk(app, spec) && !is_image_file(spec) {
        need_admin(app)?;
        let Some(n) = physical_number(spec) else { return Err(format!("{spec} is not a disk")) };
        // Only ever read a disk Windows cannot write to: protected in step 3
        // (or still switched off by Windows' setting, which is read-only too).
        match crate::guard::state(n) {
            Ok(s) if s.read_only => {}
            Ok(_) => {
                return Err(format!(
                    "Disk {n} can still be written to by Windows. Protect it first in step 3 (that marks it read-only)."
                ))
            }
            Err(e) => return Err(format!("Windows would not say whether disk {n} is protected ({e}). Choose it again in step 3.")),
        }
    }
    // Disk numbers change when drives are plugged in and out: make sure this
    // one is still the copy the mapfile describes - not just another drive
    // of the same size (mapcheck::is_the_copy).
    let map = lock(app).setting("mapfile");
    let Some((s, e)) = mapfile_domain(&map) else { return Ok(()) };
    let src = crate::disk::open_source(spec).map_err(|err| format!("could not open {spec}: {err}"))?;
    let table = crate::gpt::read(&*src, &[]);
    match crate::mapcheck::not_the_copy(&*src, &table, e - s) {
        None => Ok(()),
        Some(why) => Err(format!(
            "This disk is not the copy ddrescue made: {why}. Disk numbers change when drives are plugged in and \
             out: choose the recovered SSD (marked \"the copy ddrescue made\") again in step 3."
        )),
    }
}

fn start_map_check(app: &Arc<Mutex<App>>, map: &str, earlier: &str, disk: &str) -> Result<u64, String> {
    start_prepared(app, prepare_map_check(app, map, earlier, disk))
}

fn prepare_map_check(app: &Arc<Mutex<App>>, map: &str, earlier: &str, disk: &str) -> Result<Prepared, String> {
    check_disk(app, disk)?;
    if map.is_empty() {
        return Err("Choose the mapfile first (step 2).".to_string());
    }
    let out = fresh(&lock(app).results, &format!("map-check-{}", stamp()), "");
    let opts = crate::mapcheck::Options {
        map_path: map.to_string(),
        earlier_maps: if earlier.is_empty() { Vec::new() } else { vec![earlier.to_string()] },
        disk: disk.to_string(),
        out_dir: out.display().to_string(),
        compare_with: None,
    };
    let app2 = app.clone();
    let guard = cfg!(windows) && physical_number(disk).is_some() && !is_test_disk(app, disk);
    let removable = is_removable(disk);
    Ok(prepared("mapcheck", "Finding what the damage touched", move |ctx| {
        hold_new_disks_off(ctx, guard);
        if removable {
            ctx.line(REMOVABLE_NOTE);
        }
        let report = crate::mapcheck::run(&opts, &mut |s: &str| ctx.line(s))?;
        {
            let mut a = lock(&app2);
            a.settings.insert("last_map_check".to_string(), opts.out_dir.clone());
            a.save();
        }
        let mut outputs = vec![opts.out_dir.clone()];
        outputs.extend(report.written.iter().cloned());
        Ok(Outcome {
            summary: crate::mapcheck::summary(&report),
            outputs,
            data: crate::mapcheck::page_json(&report),
            ..Default::default()
        })
    }))
}

fn start_rescue(app: &Arc<Mutex<App>>, disk: &str, out: &str, everything: bool, lost: bool) -> Result<u64, String> {
    start_prepared(app, prepare_rescue(app, disk, out, everything, lost))
}

fn prepare_rescue(app: &Arc<Mutex<App>>, disk: &str, out: &str, everything: bool, lost: bool) -> Result<Prepared, String> {
    check_disk(app, disk)?;
    if out.trim().is_empty() {
        return Err("Choose where the copies go first.".to_string());
    }
    let opts_disk = disk.to_string();
    let opts_out = out.to_string();
    let guard = cfg!(windows) && physical_number(disk).is_some() && !is_test_disk(app, disk);
    let removable = is_removable(disk);
    let map_check = lock(app).setting("last_map_check");
    let results = lock(app).results.clone();
    Ok(prepared("rescue", "Copying everything off the recovered SSD", move |ctx| {
        hold_new_disks_off(ctx, guard);
        if removable {
            ctx.line(REMOVABLE_NOTE);
        }
        let opts = crate::rescue::Options {
            disk: opts_disk,
            out_dir: opts_out.clone(),
            pick: if everything { crate::rescue::Pick::Everything } else { crate::rescue::Pick::Hidden },
            lost,
            stop: Some(ctx.stop.clone()),
            stop_after_files: None,
        };
        let o = crate::rescue::run(&opts, &mut |s: &str| ctx.line(s), &mut |d, t| ctx.amount(d, t))?;
        let mut summary = vec![format!(
            "Copied {} files ({}) and {} folders into {}.",
            commas(o.files),
            size(o.bytes),
            commas(o.folders),
            opts_out
        )];
        if o.already_there > 0 {
            summary.push(format!("{} files were already there from an earlier run and were left as they were.", commas(o.already_there)));
        }
        if o.recopied > 0 {
            summary.push(format!(
                "{} files an earlier run had left did not hold what the disk holds (the PC may have lost power), so \
                 they were copied again.",
                commas(o.recopied)
            ));
        }
        if o.lost_pieces > 0 {
            summary.push(format!(
                "Saved {} of data that no folder record names any more ({}) in \"lost data (no name)\".",
                crate::rescue::pieces(o.lost_pieces),
                size(o.lost_bytes)
            ));
        }
        if o.renamed > 0 {
            summary.push(if o.renamed == 1 {
                "1 copy has a slightly different name, because Windows cannot use the original one (the log lists it)."
                    .to_string()
            } else {
                format!(
                    "{} copies have slightly different names, because Windows cannot use the original ones (the log \
                     lists them).",
                    commas(o.renamed)
                )
            });
        }
        if !o.partial.is_empty() {
            summary.push(format!("{} files could only be copied in part: the missing parts are zeros.", commas(o.partial.len() as u64)));
        }
        if !o.failed.is_empty() {
            summary.push(format!("{} could not be copied:", commas(o.failed.len() as u64)));
            for (p, n) in o.failed.iter().take(10) {
                summary.push(format!("  {p}: {n}"));
            }
        }
        if let Some(e) = &o.disk_lost {
            summary.insert(0, crate::rescue::disk_lost_text(e));
        } else if let Some(e) = &o.dest_lost {
            summary.insert(0, crate::rescue::dest_lost_text(e));
        } else if let Some(e) = &o.keeps_failing {
            summary.insert(0, crate::rescue::keeps_failing_text(e));
        } else if o.stopped {
            summary.push("Stopped before the end. Start it again to copy the rest: what is already copied is kept.".to_string());
        }
        summary.push(format!("The full list is in {}", o.log));
        // The safety check: the whole list of copies (this run's and any
        // earlier run's) against what step 5 found on the SSD.
        let safety = crate::verify::ListTotals::read(Path::new(&opts_out))
            .ok()
            .map(|l| crate::verify::copy_safety(&l, ssd_totals(&map_check).as_ref()));
        if let Some(s) = &safety {
            if let Err(e) = s.record(&results, &crate::fmt::when_now(), crate::verify::STEP6, &opts_out, None) {
                summary.push(format!("(The safety check could not be added to the list of every check: {e})"));
            }
        }
        let data = Obj::new()
            .n("files", o.files)
            .n("folders", o.folders)
            .n("bytes", o.bytes)
            .n("already_there", o.already_there)
            .n("partial", o.partial.len() as u64)
            .n("failed", o.failed.len() as u64)
            .n("lost_pieces", o.lost_pieces)
            .n("lost_bytes", o.lost_bytes)
            .n("renamed", o.renamed)
            .n("recopied", o.recopied)
            .s(
                "why_stopped",
                if o.disk_lost.is_some() {
                    "disk"
                } else if o.dest_lost.is_some() {
                    "dest"
                } else if o.keeps_failing.is_some() {
                    "failing"
                } else {
                    ""
                },
            )
            .b("stopped", o.stopped)
            .s("out", &opts_out)
            .raw("safety", &safety.as_ref().map(|s| s.json()).unwrap_or_else(|| "null".to_string()))
            .raw("history", &crate::verify::history_json(&results, HISTORY_SHOWN))
            .end();
        let problem = if !o.failed.is_empty() {
            Some(format!("{} files could not be copied", o.failed.len()))
        } else if !o.stopped && safety.as_ref().is_some_and(|s| !s.fine) {
            Some("The safety check below found something: read it before going on.".to_string())
        } else {
            None
        };
        Ok(Outcome { summary, outputs: vec![opts_out.clone(), o.log.clone()], data, stopped: o.stopped, problem })
    }))
}

// ---------------------------------------------------------------- getting ready

/// What Get ready shows: the checks of this PC, the tools, the folders it suggests.
fn ready_json(app: &Arc<Mutex<App>>, fresh: bool) -> String {
    {
        // No mapfile chosen yet: the best one in the program's folder.
        let home = lock(app).home.clone();
        if lock(app).setting("mapfile").is_empty() {
            if let Some(m) = best_mapfile(&home) {
                let mut a = lock(app);
                a.settings.insert("mapfile".to_string(), m.clone());
                a.save();
                MAP_SIZE.store(mapfile_domain(&m).map(|(s, e)| e.saturating_sub(s)).unwrap_or(0), Ordering::Relaxed);
            }
        }
    }
    let (home, elevated, map, image_dir, work, copy2) = {
        let a = lock(app);
        (
            a.home.clone(),
            a.elevated,
            a.setting("mapfile"),
            a.setting("auto_image_dir"),
            a.setting("auto_work"),
            a.setting("auto_copy2"),
        )
    };
    let restart = crate::winset::restart_pending();
    let crash = crate::winset::restarts_after_crash();
    let aim_driver = crate::winset::aim_driver_installed();
    let aim_setup = find_aim_setup(&home);
    let (suggest_image, suggest_work, suggest_copy2) = suggested_folders(app, &map);
    Obj::new()
        .b("elevated", elevated)
        .b("windows", cfg!(windows))
        .s("mapfile", &map)
        .raw("restart_pending", opt_bool(restart))
        .raw("restarts_after_crash", opt_bool(crash))
        .raw("aim_driver", opt_bool(aim_driver))
        .raw(
            "defender",
            &places::defender_json(
                app,
                if image_dir.trim().is_empty() { &suggest_image } else { &image_dir },
                if work.trim().is_empty() { &suggest_work } else { &work },
                fresh,
            ),
        )
        .s("aim_setup", &aim_setup.map(|p| p.display().to_string()).unwrap_or_default())
        .raw("policy_to_restore", if crate::cli::policy_to_restore().is_some() { "true" } else { "false" })
        .raw("tools", &crate::tools::status_json(&home))
        .s("image_dir", &image_dir)
        .s("work", &work)
        .s("copy2", &copy2)
        .s("suggest_image_dir", &suggest_image)
        .s("suggest_work", &suggest_work)
        .s("suggest_copy2", &suggest_copy2)
        .end()
}

/// The folders to suggest, before any is chosen: (image, WORK, second copy).
fn suggested_folders(app: &Arc<Mutex<App>>, map: &str) -> (String, String, String) {
    // Folders to suggest. The image and the second copy: a drive of the PC
    // (NTFS, not SPARE) with room for the image - another drive than
    // Windows' own if one has room, Windows' own if only it has (as on a PC
    // whose second drive is small). The copies: the drive labelled SPARE (or
    // the letter it had at the rehearsal, if it is switched off now).
    let drives = crate::win::drives();
    let windows_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string()).to_uppercase();
    let need = mapfile_domain(&map).map(|(s, e)| e.saturating_sub(s) + (1 << 30)).unwrap_or(0);
    let fixed: Vec<&crate::win::Drive> = drives
        .iter()
        .filter(|d| d.kind == "fixed" && d.fs.eq_ignore_ascii_case("NTFS") && !d.label.eq_ignore_ascii_case("SPARE"))
        .collect();
    let is_windows = |d: &&&crate::win::Drive| d.letter.eq_ignore_ascii_case(&windows_drive);
    let image_drive = fixed
        .iter()
        .filter(|d| !is_windows(d) && d.free >= need)
        .max_by_key(|d| d.free)
        .or_else(|| fixed.iter().filter(|d| d.free >= need).max_by_key(|d| d.free))
        .or_else(|| fixed.iter().filter(|d| !is_windows(d)).max_by_key(|d| d.free))
        .or_else(|| fixed.iter().max_by_key(|d| d.free))
        .map(|d| d.letter.clone());
    let spare = drives
        .iter()
        .find(|d| d.label.eq_ignore_ascii_case("SPARE"))
        .map(|d| d.letter.clone())
        .or_else(|| Some(lock(app).setting("spare_letter")).filter(|l| !l.is_empty()));
    let sug = |l: &Option<String>, f: &str| l.as_ref().map(|l| format!("{l}\\{f}")).unwrap_or_default();
    (sug(&image_drive, "IMAGE"), sug(&spare, "WORK"), sug(&image_drive, "COPY 2"))
}

/// AIM Toolkit's installer (install.bat), where the person keeps it.
fn find_aim_setup(home: &Path) -> Option<PathBuf> {
    fn look(dir: &Path, depth: u32) -> Option<PathBuf> {
        let rd = std::fs::read_dir(dir).ok()?;
        let mut sub = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_lowercase();
            if p.is_file() && name == "install.bat" && p.parent().is_some_and(|d| d.join("files.cab").is_file()) {
                return Some(p);
            }
            if p.is_dir() && depth > 0 && name != "results" {
                sub.push(p);
            }
        }
        sub.into_iter().find_map(|d| look(&d, depth - 1))
    }
    look(home, 4)
}

/// Run AIM Toolkit's installer (it shows its own window; Windows asks about
/// the driver).
fn aim_install(app: &Arc<Mutex<App>>) -> Result<String, String> {
    let home = lock(app).home.clone();
    let bat = find_aim_setup(&home).ok_or_else(|| {
        "AIM Toolkit's installer (install.bat, beside files.cab) was not found in the program's folder. Download AIMtk.zip from \
         sourceforge.net/projects/aim-toolkit and unzip it into C:\\RECOVERY-TOOLS\\installed files."
            .to_string()
    })?;
    let dir = bat.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    std::process::Command::new(crate::win::system_program("cmd.exe"))
        .arg("/c")
        .arg(&bat)
        .current_dir(&dir)
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", bat.display()))?;
    crate::tools::forget();
    Ok("AIM Toolkit's installer is starting in its own window: click Install there, and Yes if Windows asks about the \
        Arsenal Image Mounter driver. Then click Check again."
        .to_string())
}

fn start_tool_download(app: &Arc<Mutex<App>>, key: &str) -> Result<u64, String> {
    let tool = crate::tools::Tool::from_key(key).ok_or_else(|| format!("which tool? ({key})"))?;
    let home = lock(app).home.clone();
    start_job(app, "download", "Downloading a tool", move |ctx| {
        let exe = crate::tools::download(tool, &home, &mut |s| ctx.line(s), &|| ctx.stopping())?;
        Ok(Outcome { summary: vec![format!("{} is ready: {}", tool.name(), exe.display())], ..Default::default() })
    })
}

// ---------------------------------------------------------------- the checklist

/// The checklist's HTML and boxes: the same for as long as the program runs.
fn checklist_page() -> &'static str {
    static PAGE_JSON: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PAGE_JSON.get_or_init(|| crate::checklist::page_json(crate::checklist::TEXT))
}

/// A file's time in seconds since 1970.
fn secs_of(t: std::time::SystemTime) -> i64 {
    t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn mtime_ms(p: &Path) -> Option<u64> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(m.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64)
}

/// Where the checklist stands. On the way it takes in the ticks of the
/// person's own copy (if it changed since the program last wrote it), and
/// writes that copy back with every box as it stands now - never over
/// anything the person wrote into it themselves.
fn checklist_sync(app: &Arc<Mutex<App>>) -> String {
    use crate::checklist as cl;
    let (home, results, map_check, copy_mtime, copy_hash) = {
        let a = lock(app);
        (
            a.home.clone(),
            a.results.clone(),
            a.setting("last_map_check"),
            a.setting("auto_checklist_copy_mtime"),
            a.setting("auto_checklist_copy_hash"),
        )
    };
    let (phases, items) = cl::parse(cl::TEXT);
    let ticks_path = results.join(cl::TICKS_NAME);
    let mut ticks = cl::Ticks::load(&ticks_path);
    let mut notes: Vec<String> = Vec::new();
    let found = cl::find_copy(&home);
    let target = found.clone().unwrap_or_else(|| cl::first_copy_place(&home));
    if let Some(p) = &found {
        let changed = mtime_ms(p).map(|m| m.to_string()) != Some(copy_mtime.clone());
        if changed {
            if let (Ok(text), Ok(modified)) = (std::fs::read_to_string(p), std::fs::metadata(p).and_then(|m| m.modified())) {
                // The newer change wins, compared as moments (local times
                // repeat an hour when the clocks go back).
                let secs = secs_of(modified);
                let when = crate::fmt::when_of(secs);
                for (key, on) in cl::ticks_in(&text) {
                    let known = items.iter().any(|i| i.key == key && !i.checkpoint);
                    if known && ticks.on(&key) != on && secs > crate::fmt::instant(ticks.when(&key)).unwrap_or(i64::MIN) {
                        if let Err(e) = ticks.set(&ticks_path, &key, on, &when) {
                            notes.push(e);
                            break;
                        }
                    }
                }
            }
        }
    }
    let history = crate::verify::history(&results);
    let st = cl::statuses(&items, &ticks, &history);
    let done = cl::ticked(&items, &ticks, &st);
    let numbers = ssd_totals(&map_check).and_then(|s| s.bytes.map(|b| (s.files, b)));
    let md = cl::markdown(cl::TEXT, &items, &done, numbers);
    match std::fs::read_to_string(&target) {
        Ok(cur) if cur == md => {}
        Ok(cur) if !cl::is_unedited(&cur, copy_hash.parse().ok()) => notes.push(format!(
            "Your copy {} has something of your own written in it, so the program takes the ticks from it but leaves \
             it as it is.",
            target.display()
        )),
        _ => match cl::write_whole(&target, &md) {
            Ok(()) => {
                let mut a = lock(app);
                a.settings.insert("auto_checklist_copy_mtime".to_string(), mtime_ms(&target).unwrap_or(0).to_string());
                a.settings.insert("auto_checklist_copy_hash".to_string(), cl::written_hash(&md).to_string());
                a.save();
            }
            Err(e) => notes.push(e),
        },
    }
    // The download manifest beside it, when it is one the program knows.
    if let Some(dir) = target.parent() {
        let m = dir.join(cl::MANIFEST_NAME);
        match std::fs::read_to_string(&m) {
            Ok(cur) if cur == cl::MANIFEST || !cl::manifest_unedited(&cur) => {}
            _ => {
                if let Err(e) = cl::write_whole(&m, cl::MANIFEST) {
                    notes.push(e);
                }
            }
        }
    }
    cl::state_json(&phases, &items, &done, &st, numbers, &target.display().to_string(), &notes.join(" "))
}

/// A box ticked or unticked on the page.
fn tick(app: &Arc<Mutex<App>>, i: &str, on: bool) -> Result<String, String> {
    use crate::checklist as cl;
    let (_, items) = cl::parse(cl::TEXT);
    let i: usize = i.parse().map_err(|_| "which box?".to_string())?;
    let results = lock(app).results.clone();
    let path = results.join(cl::TICKS_NAME);
    let mut ticks = cl::Ticks::load(&path);
    cl::may_set(&items, &ticks, i, on)?;
    ticks.set(&path, &items[i].key, on, &crate::fmt::when_now())?;
    Ok(checklist_sync(app))
}

/// The newest safety check (step 7) of the copies in `folder`.
fn last_check_of(results: &Path, folder: &Path) -> Option<crate::verify::Checked> {
    let norm = |p: &str| p.trim().trim_end_matches(['\\', '/']).to_lowercase();
    let want = norm(&folder.display().to_string());
    crate::verify::history(results).into_iter().rev().find(|c| c.step == crate::verify::STEP7 && norm(&c.folder) == want)
}

/// How many of the latest safety checks the page lists under steps 6 and 7.
const HISTORY_SHOWN: usize = 12;

/// What the latest map check (step 5) found on the SSD, for the safety checks.
fn ssd_totals(map_check: &str) -> Option<crate::verify::OnSsd> {
    if map_check.is_empty() {
        None
    } else {
        crate::verify::map_check_totals(Path::new(map_check))
    }
}

/// The drive (letter) a folder is on.
fn drive_of(path: &str) -> Option<crate::win::Drive> {
    let p = path.trim().trim_start_matches(r"\\?\");
    let letter = p.get(..2)?.to_uppercase();
    if !letter.ends_with(':') {
        return None;
    }
    crate::win::drives().into_iter().find(|d| d.letter.eq_ignore_ascii_case(&letter))
}

/// Reads the image back against the checksum saved beside it (no SSD
/// needed), and adds the result to the list of every check.
fn start_check_image(app: &Arc<Mutex<App>>) -> Result<u64, String> {
    start_prepared(app, prepare_check_image(app))
}

fn prepare_check_image(app: &Arc<Mutex<App>>) -> Result<Prepared, String> {
    let image = lock(app).setting("image");
    if image.is_empty() {
        return Err("There is no image to check yet: step 4 makes it.".to_string());
    }
    if !Path::new(&image).is_file() {
        return Err(format!("{image} is not there: is the drive it is on plugged in?"));
    }
    let results = lock(app).results.clone();
    Ok(prepared("imagecheck", "Checking the image is unchanged", move |ctx| {
        let o = crate::image::check_existing(Path::new(&image), &mut |s| ctx.line(s), &mut |d, t| ctx.amount(d, t), &|| ctx.stopping())?;
        let mut summary = Vec::new();
        if o.stopped {
            summary.push("Stopped before the end: nothing was changed. Check it again when you have the time.".to_string());
        } else if o.verified {
            summary.push(format!(
                "The image {image} ({}) is exactly as it was made: its SHA-256 matches the one saved when it was made.",
                size(o.size)
            ));
        } else {
            summary.push(format!(
                "The image {image} is NOT as it was made: its SHA-256 does not match the one saved beside it. Don't use it \
                 or delete it: ask for help. Your copies and the SSDs in the drawer are not affected."
            ));
        }
        if !o.stopped {
            let result = if o.verified { "OK".to_string() } else { "NOT OK: the image does not match its checksum".to_string() };
            if let Err(e) = crate::verify::add_to_history(
                &results,
                &crate::fmt::when_now(),
                crate::verify::STEP4,
                &image,
                [None, None, None, None, None, Some(o.size)],
                [None, None, None],
                &result,
            ) {
                summary.push(format!("(The check could not be added to the list of every check: {e})"));
            }
        }
        let data = Obj::new()
            .b("verified", o.verified)
            .b("stopped", o.stopped)
            .n("size", o.size)
            .raw("history", &crate::verify::history_json(&results, HISTORY_SHOWN))
            .end();
        Ok(Outcome {
            summary,
            outputs: Vec::new(),
            data,
            stopped: o.stopped,
            problem: (!o.stopped && !o.verified).then(|| "the image does not match its checksum".to_string()),
        })
    }))
}

fn start_image(app: &Arc<Mutex<App>>, disk: &str, out: &str) -> Result<u64, String> {
    start_prepared(app, prepare_image(app, disk, out))
}

fn prepare_image(app: &Arc<Mutex<App>>, disk: &str, out: &str) -> Result<Prepared, String> {
    check_disk(app, disk)?;
    if out.trim().is_empty() {
        return Err("Choose where the image goes first.".to_string());
    }
    let map = lock(app).setting("mapfile");
    let Some((s, e)) = mapfile_domain(&map) else {
        return Err("Choose the mapfile first (step 2): it says how big the old drive was.".to_string());
    };
    let size = e.saturating_sub(s);
    if let Some(d) = drive_of(out) {
        if physical_number(disk).is_some_and(|n| d.disk == Some(n)) {
            return Err(format!("{} is on the recovered SSD itself. Choose a folder on another drive.", d.letter));
        }
        if d.fs.eq_ignore_ascii_case("FAT32") || d.fs.eq_ignore_ascii_case("FAT") {
            return Err(format!(
                "{} is formatted {}, which cannot hold a file bigger than 4 GB. Use a drive formatted NTFS.",
                d.letter, d.fs
            ));
        }
    }
    let guard = cfg!(windows) && physical_number(disk).is_some() && !is_test_disk(app, disk);
    let removable = is_removable(disk);
    let app2 = app.clone();
    let disk = disk.to_string();
    let out = out.to_string();
    Ok(prepared("image", "Making an image of the recovered SSD", move |ctx| {
        hold_new_disks_off(ctx, guard);
        if removable {
            ctx.line(REMOVABLE_NOTE);
        }
        let src = crate::disk::open_source(&disk).map_err(|e| format!("could not open {disk}: {e}"))?;
        let opts = crate::image::Options { disk: disk.clone(), out_dir: out.clone(), size, stop: Some(ctx.stop.clone()) };
        let o = crate::image::make(&*src, &opts, &mut |s| ctx.line(s), &mut |d, t| ctx.amount(d, t))?;
        if o.verified {
            let mut a = lock(&app2);
            a.settings.insert("image".to_string(), o.path.clone());
            a.settings.insert("image_sha".to_string(), o.sha256.clone());
            a.save();
        }
        let data = Obj::new()
            .s("path", &o.path)
            .n("size", o.size)
            .s("sha256", &o.sha256)
            .b("verified", o.verified)
            .b("reused", o.reused)
            .n("unreadable", o.unreadable.len() as u64)
            .n("unreadable_bytes", o.unreadable.iter().map(|r| r.1).sum())
            .n("stepped_over", o.stepped_over.len() as u64)
            .n("stepped_over_bytes", o.stepped_over.iter().map(|r| r.1).sum())
            .b("stopped", o.stopped)
            .b("disk_lost", o.disk_lost.is_some());
        let data = match o.lost_at {
            Some(p) => data.n("lost_at", p),
            None => data,
        }
        .end();
        Ok(Outcome {
            summary: crate::image::summary(&o),
            outputs: vec![o.path.clone()],
            data,
            stopped: o.stopped,
            problem: (!o.stopped && !o.verified).then(|| "the image does not read back the same as it was written".to_string()),
        })
    }))
}

/// The latest check of the copies (step 7) of this folder: its results CSV.
fn scan_csv_for(app: &Arc<Mutex<App>>, folder: &str) -> Option<PathBuf> {
    let a = lock(app);
    let csv = a.setting("last_scan_csv");
    let same = |x: &str, y: &str| {
        let n = |p: &str| p.trim().replace('/', "\\").trim_end_matches('\\').to_lowercase();
        !x.trim().is_empty() && n(x) == n(y)
    };
    (same(&a.setting("last_scan_folder"), folder) && Path::new(&csv).is_file()).then(|| PathBuf::from(csv))
}

/// Step 8: copy the damaged files out, put the repaired ones back (or undo
/// one), evaluate.
fn start_repair_step(app: &Arc<Mutex<App>>, step: &str, folder: &str, id: &str) -> Result<u64, String> {
    start_prepared(app, prepare_repair_step(app, step, folder, id))
}

fn prepare_repair_step(app: &Arc<Mutex<App>>, step: &str, folder: &str, id: &str) -> Result<Prepared, String> {
    let work = PathBuf::from(folder.trim());
    if folder.trim().is_empty() || !work.is_dir() {
        return Err("Choose the folder the copies went into first (step 7's folder).".to_string());
    }
    if !work.join(crate::rescue::LOG_NAME).is_file() {
        return Err(format!(
            "{} has no {}: choose the folder step 6 copied into.",
            folder.trim(),
            crate::rescue::LOG_NAME
        ));
    }
    let repair = crate::repair::repair_dir(&work)?;
    let scans: Vec<PathBuf> = scan_csv_for(app, folder).into_iter().collect();
    if matches!(step, "isolate" | "evaluate") && scans.is_empty() {
        return Err(
            "Check every copied file in step 7 first, with \"Use what step 5 found\" ticked: that is what says which \
             copies are damaged."
                .to_string(),
        );
    }
    if step == "put-back" {
        // Put back is the one step that changes WORK: not while the last safety
        // check of it found something nobody has sorted out.
        let results = lock(app).results.clone();
        if let Some(c) = last_check_of(&results, &work).filter(|c| !c.ok) {
            return Err(format!(
                "The last safety check of this folder ({}) found something: {} Sort that out first (ask Claude), then \
                 check again (step 7 \u{2192} Check the copies are unchanged). Put back waits for a check that passes.",
                crate::fmt::when_shown(&c.when),
                c.result.trim_start_matches("NOT OK: ")
            ));
        }
    }
    let id = id.trim().to_string();
    if step == "undo" && id.is_empty() {
        return Err("Type the number of the file to undo, such as R0012.".to_string());
    }
    let (kind, title) = match step {
        "isolate" => ("isolate", "Copying the damaged files out"),
        "put-back" => ("put-back", "Putting the repaired files back"),
        "undo" => ("put-back", "Undoing a put-back"),
        _ => ("evaluate", "Evaluating what is damaged"),
    };
    let step = step.to_string();
    Ok(prepared(kind, title, move |ctx| {
        let stop = || ctx.stopping();
        let repair_s = repair.display().to_string();
        match step.as_str() {
            "isolate" => {
                let o = crate::repair::isolate(&work, &scans, &mut |s| ctx.line(s), &stop)?;
                let mut groups: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
                for i in &o.copied {
                    *groups.entry(i.group.clone()).or_default() += 1;
                }
                let g: Vec<String> = groups.iter().map(|(k, v)| Obj::new().s("group", k).n("files", *v).end()).collect();
                let data = Obj::new()
                    .n("copied", o.copied.len() as u64)
                    .n("already", o.already as u64)
                    .n("left", o.left.len() as u64)
                    .n("problems", o.problems.len() as u64)
                    .raw("groups", &arr(g))
                    .s("repair", &repair_s)
                    .end();
                Ok(Outcome {
                    summary: crate::repair::isolate_summary(&o),
                    outputs: vec![repair_s.clone(), repair.join(crate::repair::LIST_NAME).display().to_string()],
                    data,
                    stopped: o.stopped,
                    problem: (!o.problems.is_empty()).then(|| "some damaged files could not be copied out".to_string()),
                })
            }
            "put-back" => {
                let o = crate::repair::put_back(&work, &mut |s| ctx.line(s), &stop)?;
                let back: Vec<String> = o
                    .returned
                    .iter()
                    .map(|(id, path, from)| Obj::new().s("id", id).s("path", path).s("from", from).end())
                    .collect();
                let data = Obj::new()
                    .n("returned", o.returned.len() as u64)
                    .n("already", o.already as u64)
                    .n("skipped", o.skipped.len() as u64)
                    .n("waiting", o.waiting as u64)
                    .raw("back", &arr(back))
                    .s("repair", &repair_s)
                    .end();
                Ok(Outcome {
                    summary: crate::repair::put_back_summary(&o, &repair),
                    outputs: vec![repair_s.clone()],
                    data,
                    stopped: o.stopped,
                    problem: (!o.skipped.is_empty()).then(|| "some repaired files were not put back".to_string()),
                })
            }
            "undo" => {
                let m = crate::repair::undo(&work, &id)?;
                Ok(Outcome {
                    summary: vec![m],
                    outputs: vec![repair_s.clone()],
                    data: Obj::new().s("repair", &repair_s).b("undone", true).end(),
                    stopped: false,
                    problem: None,
                })
            }
            _ => {
                let ev = crate::repair::evaluate(&work, &scans, &mut |s| ctx.line(s), &stop)?;
                let pk: Vec<String> = ev
                    .packages
                    .iter()
                    .filter(|p| p.damaged > 0)
                    .take(300)
                    .map(|p| {
                        Obj::new()
                            .s("path", &p.path)
                            .s("what", p.what)
                            .n("files", p.files)
                            .n("bytes", p.bytes)
                            .n("damaged", p.damaged)
                            .n("put_back", p.put_back)
                            .s("verdict", &p.verdict)
                            .end()
                    })
                    .collect();
                let data = Obj::new()
                    .n("put_back", ev.put_back)
                    .n("waiting", ev.waiting)
                    .n("left", ev.left)
                    .n("packages", ev.packages.len() as u64)
                    .n("intact_packages", ev.intact_packages)
                    .raw("damaged_packages", &arr(pk))
                    .s("repair", &repair_s)
                    .end();
                Ok(Outcome {
                    summary: crate::repair::evaluation_summary(&ev),
                    outputs: ev.written.iter().map(|w| w.display().to_string()).chain(std::iter::once(repair_s.clone())).collect(),
                    data,
                    stopped: ev.stopped,
                    problem: None,
                })
            }
        }
    }))
}

fn start_verify(app: &Arc<Mutex<App>>, folder: &str) -> Result<u64, String> {
    start_prepared(app, prepare_verify(app, folder))
}

fn prepare_verify(app: &Arc<Mutex<App>>, folder: &str) -> Result<Prepared, String> {
    let dir = Path::new(folder.trim());
    if folder.trim().is_empty() || !dir.is_dir() {
        return Err(format!("Choose the folder the copies went into first ({folder:?} is not one)."));
    }
    if !dir.join(crate::rescue::LOG_NAME).is_file() {
        return Err(format!(
            "{} has no {}: choose the folder step 6 copied into (that list is in it).",
            folder.trim(),
            crate::rescue::LOG_NAME
        ));
    }
    let out = fresh(&lock(app).results, &format!("verify-{}", stamp()), ".csv");
    let dir = dir.to_path_buf();
    let map_check = lock(app).setting("last_map_check");
    let results = lock(app).results.clone();
    Ok(prepared("verify", "Checking the copies are unchanged", move |ctx| {
        // The check counts from when it started: a box ticked while it ran
        // may stand for something it did not see (the checkpoints, checklist.rs).
        let started = crate::fmt::when_now();
        let mc = (!map_check.is_empty()).then(|| PathBuf::from(&map_check));
        let r = crate::verify::run_checked(
            &dir,
            mc.as_deref(),
            Some(&out),
            &mut |s| ctx.line(s),
            &mut |d, t| ctx.amount(d, t),
            &|| ctx.stopping(),
        )?;
        // The safety check: the SSD, the list of copies, the copies here now.
        let safety = (!r.stopped).then(|| crate::verify::safety(&r));
        let mut summary = crate::verify::summary(&r);
        if let Some(s) = &safety {
            if let Err(e) = s.record(&results, &started, crate::verify::STEP7, &dir.display().to_string(), Some(&r)) {
                summary.push(format!("(The safety check could not be added to the list of every check: {e})"));
            }
        }
        let data = Obj::new()
            .n("listed", r.listed)
            .n("ok", r.ok)
            .n("changed", r.changed.len() as u64)
            .n("missing", r.missing.len() as u64)
            .n("extra", r.extra.len() as u64)
            .b("stopped", r.stopped)
            .raw("safety", &safety.as_ref().map(|s| s.json()).unwrap_or_else(|| "null".to_string()))
            .raw("history", &crate::verify::history_json(&results, HISTORY_SHOWN))
            .end();
        let bad = !r.changed.is_empty() || !r.missing.is_empty();
        let problem = if bad {
            Some("some copies are not what was copied".to_string())
        } else if safety.as_ref().is_some_and(|s| !s.fine) {
            Some("The safety check found something: read it before going on.".to_string())
        } else {
            None
        };
        Ok(Outcome {
            summary,
            outputs: r.written.clone().into_iter().collect(),
            data,
            stopped: r.stopped,
            problem,
        })
    }))
}

fn start_scan(app: &Arc<Mutex<App>>, folder: &str, holes: bool, hole_kb: u64, join_dir: &str) -> Result<u64, String> {
    start_prepared(app, prepare_scan(app, folder, holes, hole_kb, join_dir))
}

fn prepare_scan(app: &Arc<Mutex<App>>, folder: &str, holes: bool, hole_kb: u64, join_dir: &str) -> Result<Prepared, String> {
    if folder.trim().is_empty() || !Path::new(folder).is_dir() {
        return Err(format!("Choose a folder to check first ({folder:?} is not one)."));
    }
    let out = fresh(&lock(app).results, &format!("scan-{}", stamp()), ".csv").display().to_string();
    let folder = folder.to_string();
    let join_dir = join_dir.to_string();
    Ok(prepared("scan", "Checking every copied file", move |ctx| {
        scan_job(ctx, &folder, holes, hole_kb.max(1) * 1024, &join_dir, &out)
    }))
}

/// How many files there are to check (each also counted for the page, by
/// kind and size).
fn count_files(root: &str, stop: &AtomicBool) -> u64 {
    let mut n = 0u64;
    crate::walk::walk_sized(root, |_, dirs, files| {
        dirs.retain(|d| !crate::formats::SYSTEM_DIRS.contains(&d.as_str()));
        n += files.len() as u64;
        for (name, size) in files {
            crate::live::to_do(name, *size);
        }
        !stop.load(Ordering::Relaxed)
    });
    n
}

fn scan_job(ctx: &Ctx, folder: &str, holes: bool, hole_bytes: u64, join_dir: &str, out: &str) -> Result<Outcome, String> {
    use crate::formats::VERDICTS;
    use crate::report::{
        format_library_survey, format_untestable, library_csv_path, library_row, untestable_csv_path, untestable_key,
        write_library_csv, write_results_csv, Tally,
    };
    use crate::walk::{walk_and_probe, Event, LibInfo, Row};
    ctx.line(&format!("Counting the files in {folder}..."));
    crate::live::begin("checked");
    let total = count_files(folder, &ctx.stop);
    ctx.line(&format!("{} files to check", commas(total)));
    ctx.amount(0, total);
    let items = if join_dir.is_empty() {
        Vec::new()
    } else {
        let p = Path::new(join_dir).join(crate::join::TSV_NAME);
        match std::fs::read_to_string(&p) {
            Ok(t) => crate::join::parse_tsv(&t),
            Err(e) => {
                ctx.line(&format!("(could not read the map check's list {}: {e} - checking without it)", p.display()));
                Vec::new()
            }
        }
    };
    let mut collector = crate::join::Collector::new(&items);
    collector.note_returns(folder);
    let mut counts: HashMap<&'static str, u64> = VERDICTS.iter().map(|v| (*v, 0)).collect();
    let mut rows: Vec<Row> = Vec::new();
    let mut skipped: Vec<Row> = Vec::new();
    let mut libraries: Vec<LibInfo> = Vec::new();
    let mut untestable = Tally::default();
    let mut scanned = 0u64;
    let mut bytes = 0u64;
    let mut last = Instant::now();
    let still_there = walk_and_probe(folder, hole_bytes, holes, Some(&ctx.stop), |ev| match ev {
        Event::Lib(_, info) => {
            ctx.line(&format!("Photos Library: {}", info.path));
            libraries.push(info);
        }
        Event::LibDone(i, info) => {
            rows.push(library_row(&info));
            *counts.entry("LIBRARY").or_insert(0) += 1;
            if i < libraries.len() {
                libraries[i] = info;
            }
        }
        Event::Row(r) => {
            scanned += 1;
            bytes += r.size;
            crate::live::done(&r.path, r.size);
            crate::live::now(&r.path);
            collector.saw(&r);
            *counts.entry(r.verdict).or_insert(0) += 1;
            if r.verdict == "SKIPPED" {
                untestable.add(&untestable_key(&r.path));
                skipped.push(r.clone());
            }
            if r.verdict != "OK" && r.verdict != "SKIPPED" {
                ctx.line(&format!("{}  {}", r.verdict, r.path));
                rows.push(r);
            }
            if last.elapsed().as_millis() > 300 {
                ctx.amount(scanned, total.max(scanned));
                last = Instant::now();
            }
        }
    });
    ctx.amount(scanned, total.max(scanned));
    if !still_there {
        return Err(crate::walk::GONE.to_string());
    }
    let stopped = ctx.stopping();
    let mut summary = Vec::new();
    let mut outputs = vec![out.to_string()];
    let mut join_lines = Vec::new();
    if !items.is_empty() {
        let o = crate::join::find(&items, &collector);
        join_lines = crate::join::apply(&o, &mut rows, &mut skipped);
        if !o.missing.is_empty() || !o.hidden_missing.is_empty() {
            let p = out.trim_end_matches(".csv").to_string() + "-map-check-not-here.csv";
            if std::fs::write(&p, format!("\u{FEFF}{}", crate::join::missing_csv(&o))).is_ok() {
                outputs.push(p);
            }
        }
        counts.insert(crate::join::VERDICT, rows.iter().filter(|r| r.verdict == crate::join::VERDICT).count() as u64);
    }
    write_results_csv(out, &rows).map_err(|e| format!("could not write {out}: {e}"))?;
    {
        let mut a = lock(&ctx.app);
        a.settings.insert("last_scan_csv".to_string(), out.to_string());
        a.settings.insert("last_scan_folder".to_string(), folder.to_string());
        a.save();
    }
    let c = |k: &str| counts.get(k).copied().unwrap_or(0);
    summary.push(format!(
        "{} files checked ({}){}.",
        commas(scanned),
        size(bytes),
        if stopped { " before it was stopped" } else { "" }
    ));
    let damaged = ["NEVER_READ", "DB_DAMAGED", "TRUNCATED", "HOLE", "MISMATCH", "UNREADABLE"];
    let named: Vec<String> = damaged.iter().filter(|k| c(k) > 0).map(|k| format!("{} {}", commas(c(k)), k)).collect();
    summary.push(if named.is_empty() { "No damaged files found.".to_string() } else { format!("Damaged: {}.", named.join(", ")) });
    summary.push(format!(
        "Clean: {}. Not testable: {} (camera RAW and HEIC always land here: nothing on Windows can check them fully).",
        commas(c("OK")),
        commas(c("SKIPPED"))
    ));
    summary.extend(join_lines);
    summary.extend(format_untestable(&untestable, c("SKIPPED")));
    summary.extend(format_library_survey(&libraries));
    if !libraries.is_empty() {
        let p = library_csv_path(out);
        if write_library_csv(&p, &libraries).is_ok() {
            outputs.push(p);
        }
    }
    if !skipped.is_empty() {
        let p = untestable_csv_path(out);
        if write_results_csv(&p, &skipped).is_ok() {
            outputs.push(p);
        }
    }
    let flagged: Vec<String> = rows
        .iter()
        .filter(|r| r.verdict != "LIBRARY")
        .take(1000)
        .map(|r| Obj::new().s("verdict", r.verdict).s("path", &r.path).s("detail", &r.detail).n("size", r.size).end())
        .collect();
    let libs: Vec<String> = libraries
        .iter()
        .map(|l| {
            Obj::new()
                .s("path", &l.path)
                .n("files", l.files)
                .n("originals", l.originals)
                .n("bytes", l.total_bytes)
                .n("damaged", l.damaged)
                .n("unverified", l.unverified)
                .s("database", &l.db_verdict)
                .b("placeholders", crate::report::is_placeholder(l))
                .end()
        })
        .collect();
    let mut cnt = Obj::new();
    for k in VERDICTS.iter().chain(std::iter::once(&crate::join::VERDICT)) {
        cnt = cnt.n(k, c(k));
    }
    let data = Obj::new()
        .n("scanned", scanned)
        .n("bytes", bytes)
        .raw("counts", &cnt.end())
        .raw("flagged", &arr(flagged))
        .n("flagged_total", rows.iter().filter(|r| r.verdict != "LIBRARY").count() as u64)
        .raw("libraries", &arr(libs))
        .b("joined", !items.is_empty())
        .s("folder", folder)
        .end();
    Ok(Outcome { summary, outputs, data, stopped, problem: None })
}

// ---------------------------------------------------------------- starting

/// Where the page's port is noted when it is not the usual one (Windows may
/// keep the usual one for itself after a restart: Hyper-V and WSL set ranges
/// of ports aside, a different range each time).
fn port_note() -> PathBuf {
    std::env::temp_dir().join("automatic-recovery-probe-port.txt")
}

fn remembered_port() -> Option<u16> {
    std::fs::read_to_string(port_note()).ok()?.trim().parse().ok()
}

/// Does this program answer on `port`?
fn already_running_on(port: u16) -> bool {
    let Ok(mut s) = TcpStream::connect_timeout(&(std::net::Ipv4Addr::LOCALHOST, port).into(), Duration::from_millis(500)) else {
        return false;
    };
    let _ = s.write_all(format!("GET /ping HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes());
    let mut buf = Vec::new();
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).contains(crate::APP)
}

/// What double-clicking the program does.
pub fn launch() -> i32 {
    let exe = std::env::current_exe().unwrap_or_default();
    // Already open (on its usual port, or on the one it last had to use
    // instead)? Then say so: two copies must not work on the same folders.
    let mut ports = vec![DEFAULT_PORT];
    ports.extend(remembered_port().filter(|&p| p != DEFAULT_PORT));
    if ports.into_iter().any(already_running_on) {
        println!("{} is already running in another window.", crate::APP);
        println!("Its window shows the web address to open. To start again, close that window first.");
        pause(8);
        return 0;
    }
    let port = TcpListener::bind(("127.0.0.1", DEFAULT_PORT))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
        .unwrap_or(DEFAULT_PORT);
    let token = new_token();
    let cfg = Config { port, token: token.clone(), open_browser: true, test_disks: Vec::new(), home: None };
    if crate::win::is_elevated() || !cfg!(windows) {
        return serve(cfg);
    }
    println!("{} {}", crate::APP, crate::VERSION);
    println!();
    println!("Windows will now ask whether to let this program make changes.");
    println!("Click Yes: it needs that to read the recovered SSD directly (it never writes to it).");
    // This copy opens the browser itself (so the browser does not get
    // administrator rights), so the other copy must not open it too.
    let args = vec![
        "ui".to_string(),
        "--port".to_string(),
        port.to_string(),
        "--token".to_string(),
        token.clone(),
        "--no-browser".to_string(),
    ];
    match crate::win::start_elevated(&exe, &args) {
        Ok(()) => {
            let url = format!("http://127.0.0.1:{port}/{token}/");
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(60) {
                if TcpStream::connect_timeout(&(std::net::Ipv4Addr::LOCALHOST, port).into(), Duration::from_millis(300)).is_ok() {
                    // (This copy has no administrator rights: it may start
                    // Edge itself, in a window of its own.)
                    let own = crate::win::open_app_window(&url);
                    println!();
                    if own {
                        println!("It is open in a window of its own now (Microsoft Edge). This window can be closed.");
                    } else {
                        println!("It is open in your web browser now. This window can be closed.");
                    }
                    pause(5);
                    return 0;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
            println!("The program did not start. Try double-clicking it again.");
            pause(20);
            1
        }
        Err(e) if e == "cancelled" => {
            println!();
            println!("Without administrator rights the program cannot read the recovered SSD itself.");
            println!("It still runs, and can check copied files.");
            println!();
            serve(cfg)
        }
        Err(e) => {
            println!("{e}");
            println!("Starting without administrator rights instead.");
            serve(cfg)
        }
    }
}

fn pause(secs: u64) {
    std::thread::sleep(Duration::from_secs(secs));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_decode() {
        let f = parse_form("action=scan&folder=E%3A%5CWORK%5CPhotos+2019&holes=1&x");
        assert_eq!(f["action"], "scan");
        assert_eq!(f["folder"], r"E:\WORK\Photos 2019");
        assert_eq!(f["holes"], "1");
        assert_eq!(f["x"], "");
        assert_eq!(url_decode("%E5%AE%B6%20%F0%9F%98%80"), "家 😀");
        assert_eq!(url_decode("100%"), "100%");
        assert_eq!(url_decode("%zz"), "%zz");
    }

    #[test]
    fn state_lines_round_trip() {
        let s = "tab\there\nnew\\line\r";
        assert_eq!(unescape(&escape(s)), s);
    }

    #[test]
    fn the_pace_is_the_recent_one() {
        let mut p = Pace::default();
        // Carrying on after a stop: half of it rushed through in 5 s (files
        // already there), then 2 units a second.
        for s in 0..=5 {
            p.note(s as f64, s * 100);
        }
        assert_eq!(p.rate(5.0, 500), None, "too soon to tell");
        let mut done = 500;
        for s in 6..=200u64 {
            done += 2;
            p.note(s as f64, done);
        }
        let r = p.rate(200.0, done).unwrap();
        assert!((r - 2.0).abs() < 0.05, "the rush at the start is forgotten: {r}");
        // Stalled for a while (a place the SSD struggles to read): no guess.
        for s in 201..=300u64 {
            p.note(s as f64, done);
        }
        assert_eq!(p.rate(300.0, done), None);
        // Several reports in one second count once.
        let mut q = Pace::default();
        for i in 0..50 {
            q.note(i as f64 * 0.01, i);
        }
        assert_eq!(q.samples.len(), 1);
    }

    #[test]
    fn keys_are_random() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
