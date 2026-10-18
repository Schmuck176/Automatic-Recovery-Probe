//! The double-click front end: a page in the PC's own web browser, served by
//! this program to this PC only (127.0.0.1, behind a random key in the web
//! address), that walks through the whole job one step at a time.
//!
//! Double-clicking the program starts a copy of it with administrator
//! rights (Windows asks first: reading the recovered SSD directly needs
//! them) in a window of its own, then opens the page. Each step that takes a
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

const PAGE: &str = include_str!("ui.html");
/// The address stays the same from one run to the next where it can.
pub const DEFAULT_PORT: u16 = 47115;

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
    stop: Arc<AtomicBool>,
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
            j.done = done;
            j.total = total;
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
        let mut t = String::from("# What Recovery Probe's steps found last time (read back when it starts).\r\n");
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
        let _ = std::fs::write(self.state_file(), t);
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
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&ctx)));
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

/// Run the page until the program is closed.
/// When the program's window is closed (or the PC shuts down) while Windows
/// is still set to leave new disks switched off, put that setting back:
/// otherwise every drive plugged in later would arrive switched off too.
/// Windows allows a few seconds for this, and diskpart needs about one.
#[cfg(windows)]
fn restore_setting_on_close() {
    extern "system" fn handler(ctrl: u32) -> i32 {
        // Ctrl+C, Ctrl+Break, the window closed, signing out, shutting down.
        if matches!(ctrl, 0 | 1 | 2 | 5 | 6) && crate::cli::policy_to_restore().is_some() {
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
    };
    app.load();
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
    println!("If the page did not open, copy this address into the browser:");
    println!("  {url}");
    println!();
    if !elevated {
        println!("(Without administrator rights it cannot read the recovered SSD itself; it can still check copied files.)");
    }
    println!("Keep this window open while you use it. Close it when you are done.");
    if cfg.open_browser {
        crate::win::open_browser(&url);
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
            "<p>This address is not complete. Use the address shown in Recovery Probe's window.</p>".as_bytes(),
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
        .raw("last", &last.end())
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
    let matches = match (original, map_size) {
        (Some(o), Some(m)) => Some(o == m),
        _ => None,
    };
    let volume = match crate::mapcheck::find_volume(&*src, &table) {
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
                let Ok(text) = std::fs::read(&p) else { continue };
                if !text.starts_with(b"# Mapfile") && !text.starts_with(b"# Rescue Logfile") {
                    continue;
                }
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
    arr(found
        .iter()
        .map(|(p, size, fin, bad)| Obj::new().s("path", p).n("disk_size", *size).b("finished", *fin).n("bad_bytes", *bad).end())
        .collect())
}

// ---------------------------------------------------------------- actions

fn need_admin(app: &Arc<Mutex<App>>) -> Result<(), String> {
    if lock(app).elevated {
        Ok(())
    } else {
        Err("This step needs administrator rights. Close Recovery Probe's window, double-click the program again and \
             click Yes when Windows asks."
            .to_string())
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
        "guard-restore" => need_admin(app).and_then(|_| crate::cli::guard_restore()).map(|m| format!("\"message\":{}", json_str(&m))),
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
        "guard-online" => need_admin(app)
            .and_then(|_| disk_number("disk"))
            .and_then(|n| crate::guard::set_online(n, on("on")).map(|_| String::new())),
        "selftest" => start_job(app, "selftest", "Checking the program works on this PC", |ctx| {
            crate::selftest::run(&mut |s: &str| ctx.line(s), &ctx.stop)
        })
        .map(|id| format!("\"job\":{id}")),
        "mapcheck" => start_map_check(app, &g("map"), &g("earlier"), &g("disk")).map(|id| format!("\"job\":{id}")),
        "rescue" => start_rescue(app, &g("disk"), &g("out"), on("everything"), on("lost")).map(|id| format!("\"job\":{id}")),
        "scan" => {
            let kb: u64 = g("hole_kb").parse().unwrap_or(crate::formats::DEFAULT_HOLE_KB);
            start_scan(app, &g("folder"), on("holes"), kb, &g("join")).map(|id| format!("\"job\":{id}"))
        }
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

fn check_disk(app: &Arc<Mutex<App>>, spec: &str) -> Result<(), String> {
    if spec.is_empty() {
        return Err("Choose the recovered SSD first (step 3).".to_string());
    }
    if !is_test_disk(app, spec) {
        need_admin(app)?;
        if !spec.starts_with(r"\\.\PhysicalDrive") {
            return Err(format!("{spec} is not a disk"));
        }
    }
    // Disk numbers change when drives are plugged in and out: make sure this
    // one is still the copy the mapfile describes.
    let map = lock(app).setting("mapfile");
    let Some((s, e)) = mapfile_domain(&map) else { return Ok(()) };
    let src = crate::disk::open_source(spec).map_err(|err| format!("could not open {spec}: {err}"))?;
    let table = crate::gpt::read(&*src, &[]);
    match table.original_disk_size() {
        Some(made_for) if made_for != e - s => Err(format!(
            "This disk is not the copy ddrescue made: its partition table was made for a {}-byte drive, but the \
             mapfile describes a {}-byte drive. Disk numbers change when drives are plugged in and out: choose the \
             recovered SSD again in step 3.",
            commas(made_for),
            commas(e - s)
        )),
        _ => Ok(()),
    }
}

fn start_map_check(app: &Arc<Mutex<App>>, map: &str, earlier: &str, disk: &str) -> Result<u64, String> {
    check_disk(app, disk)?;
    if map.is_empty() {
        return Err("Choose the mapfile first (step 2).".to_string());
    }
    let out = lock(app).results.join(format!("map-check-{}", stamp()));
    let opts = crate::mapcheck::Options {
        map_path: map.to_string(),
        earlier_maps: if earlier.is_empty() { Vec::new() } else { vec![earlier.to_string()] },
        disk: disk.to_string(),
        out_dir: out.display().to_string(),
    };
    let app2 = app.clone();
    start_job(app, "mapcheck", "Finding what the damage touched", move |ctx| {
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
    })
}

fn start_rescue(app: &Arc<Mutex<App>>, disk: &str, out: &str, everything: bool, lost: bool) -> Result<u64, String> {
    check_disk(app, disk)?;
    if out.trim().is_empty() {
        return Err("Choose where the copies go first.".to_string());
    }
    let opts_disk = disk.to_string();
    let opts_out = out.to_string();
    start_job(app, "rescue", "Copying everything off the recovered SSD", move |ctx| {
        let opts = crate::rescue::Options {
            disk: opts_disk,
            out_dir: opts_out.clone(),
            pick: if everything { crate::rescue::Pick::Everything } else { crate::rescue::Pick::Hidden },
            lost,
            stop: Some(ctx.stop.clone()),
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
        if o.lost_pieces > 0 {
            summary.push(format!(
                "Saved {} pieces of data that no folder record names any more ({}) in \"lost data (no name)\".",
                commas(o.lost_pieces),
                size(o.lost_bytes)
            ));
        }
        if o.renamed > 0 {
            summary.push(format!(
                "{} copies have slightly different names, because Windows cannot use the original ones (the log lists them).",
                commas(o.renamed)
            ));
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
        if o.stopped {
            summary.push("Stopped before the end. Start it again to copy the rest: what is already copied is kept.".to_string());
        }
        summary.push(format!("The full list is in {}", o.log));
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
            .b("stopped", o.stopped)
            .s("out", &opts_out)
            .end();
        Ok(Outcome {
            summary,
            outputs: vec![opts_out.clone(), o.log.clone()],
            data,
            stopped: o.stopped,
            problem: (!o.failed.is_empty()).then(|| format!("{} files could not be copied", o.failed.len())),
        })
    })
}

fn start_scan(app: &Arc<Mutex<App>>, folder: &str, holes: bool, hole_kb: u64, join_dir: &str) -> Result<u64, String> {
    if folder.trim().is_empty() || !Path::new(folder).is_dir() {
        return Err(format!("Choose a folder to check first ({folder:?} is not one)."));
    }
    let out = lock(app).results.join(format!("scan-{}.csv", stamp())).display().to_string();
    let folder = folder.to_string();
    let join_dir = join_dir.to_string();
    start_job(app, "scan", "Checking every copied file", move |ctx| {
        scan_job(ctx, &folder, holes, hole_kb.max(1) * 1024, &join_dir, &out)
    })
}

fn count_files(root: &str, stop: &AtomicBool) -> u64 {
    let mut n = 0u64;
    crate::walk::walk(root, |_, dirs, files| {
        dirs.retain(|d| !crate::formats::SYSTEM_DIRS.contains(&d.as_str()));
        n += files.len() as u64;
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
    let mut counts: HashMap<&'static str, u64> = VERDICTS.iter().map(|v| (*v, 0)).collect();
    let mut rows: Vec<Row> = Vec::new();
    let mut skipped: Vec<Row> = Vec::new();
    let mut libraries: Vec<LibInfo> = Vec::new();
    let mut untestable = Tally::default();
    let mut scanned = 0u64;
    let mut bytes = 0u64;
    let mut last = Instant::now();
    walk_and_probe(folder, hole_bytes, holes, Some(&ctx.stop), |ev| match ev {
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

/// What double-clicking the program does.
pub fn launch() -> i32 {
    let exe = std::env::current_exe().unwrap_or_default();
    // Already open? Then just show it again.
    if let Ok(mut s) = TcpStream::connect_timeout(&(std::net::Ipv4Addr::LOCALHOST, DEFAULT_PORT).into(), Duration::from_millis(500)) {
        let _ = s.write_all(format!("GET /ping HTTP/1.1\r\nHost: 127.0.0.1:{DEFAULT_PORT}\r\n\r\n").as_bytes());
        let mut buf = Vec::new();
        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = s.read_to_end(&mut buf);
        if String::from_utf8_lossy(&buf).contains(crate::APP) {
            println!("{} is already running in another window.", crate::APP);
            println!("Its window shows the web address to open. To start again, close that window first.");
            pause(8);
            return 0;
        }
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
                    crate::win::open_browser(&url);
                    println!();
                    println!("It is open in your web browser now. This window can be closed.");
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
    fn keys_are_random() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
