//! The open-source tools the automatic checks and repairs use: 7-Zip,
//! FFmpeg, MP3val, untrunc, qpdf, and AIM Toolkit's command-line tool.
//!
//! None of them is part of this program, and none of them ever touches the
//! recovered SSD or the image: the checks run them on the copies in WORK,
//! only ever with options that read; the repairs run them on the copies in
//! REPAIR (or on copies of those, in a folder of their own). Each runs as a
//! separate program, with a time limit, and can be stopped.
//!
//! Where a tool is found: in the program's own `tools` folder first (what the
//! program downloaded or was delivered with, at a version and SHA-256 it
//! knows), then where Windows programs usually install it, then on the PATH,
//! then anywhere a few folders down in the program's own folder (where the
//! person keeps the ones they downloaded: "installed files").

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tool {
    SevenZip,
    Ffmpeg,
    Ffprobe,
    Mp3val,
    Untrunc,
    Qpdf,
    AimCli,
}

impl Tool {
    pub const ALL: [Tool; 7] =
        [Tool::SevenZip, Tool::Ffmpeg, Tool::Ffprobe, Tool::Mp3val, Tool::Untrunc, Tool::Qpdf, Tool::AimCli];

    pub fn key(self) -> &'static str {
        match self {
            Tool::SevenZip => "7zip",
            Tool::Ffmpeg => "ffmpeg",
            Tool::Ffprobe => "ffprobe",
            Tool::Mp3val => "mp3val",
            Tool::Untrunc => "untrunc",
            Tool::Qpdf => "qpdf",
            Tool::AimCli => "aim",
        }
    }

    pub fn from_key(k: &str) -> Option<Tool> {
        Tool::ALL.iter().copied().find(|t| t.key() == k)
    }

    pub fn name(self) -> &'static str {
        match self {
            Tool::SevenZip => "7-Zip",
            Tool::Ffmpeg => "FFmpeg",
            Tool::Ffprobe => "FFprobe (comes with FFmpeg)",
            Tool::Mp3val => "MP3val",
            Tool::Untrunc => "untrunc",
            Tool::Qpdf => "qpdf",
            Tool::AimCli => "AIM Toolkit (Arsenal Image Mounter's command line)",
        }
    }

    /// What the program uses it for, in plain words.
    pub fn used_for(self) -> &'static str {
        match self {
            Tool::SevenZip => "tests every archive and Office file; takes out what can be read from damaged ones",
            Tool::Ffmpeg => "plays every video, song and photo through to find damage; repairs damaged containers",
            Tool::Ffprobe => "reads what is inside a video, to pick a good video from the same camera for untrunc",
            Tool::Mp3val => "finds MP3s cut short or with broken frames, and repairs them",
            Tool::Untrunc => "repairs videos (MP4, MOV) that stop part-way or lost their index",
            Tool::Qpdf => "checks every PDF, and rebuilds damaged ones",
            Tool::AimCli => "opens the image read-only as a drive, so the map check can check itself against Windows",
        }
    }

    /// The program can do without it, at the cost of what `used_for` says.
    pub fn optional(self) -> bool {
        true
    }

    fn exe_names(self) -> &'static [&'static str] {
        if cfg!(windows) {
            match self {
                Tool::SevenZip => &["7z.exe", "7za.exe"],
                Tool::Ffmpeg => &["ffmpeg.exe"],
                Tool::Ffprobe => &["ffprobe.exe"],
                Tool::Mp3val => &["mp3val.exe"],
                Tool::Untrunc => &["untrunc.exe"],
                Tool::Qpdf => &["qpdf.exe"],
                Tool::AimCli => &["aim_cli.exe"],
            }
        } else {
            match self {
                Tool::SevenZip => &["7z", "7zz", "7za"],
                Tool::Ffmpeg => &["ffmpeg"],
                Tool::Ffprobe => &["ffprobe"],
                Tool::Mp3val => &["mp3val"],
                Tool::Untrunc => &["untrunc"],
                Tool::Qpdf => &["qpdf"],
                Tool::AimCli => &["aim_cli"],
            }
        }
    }

    fn version_args(self) -> &'static [&'static str] {
        match self {
            Tool::SevenZip => &[],
            Tool::Ffmpeg | Tool::Ffprobe => &["-hide_banner", "-version"],
            Tool::Mp3val => &[],
            Tool::Untrunc => &["-V"],
            Tool::Qpdf => &["--version"],
            Tool::AimCli => &["--version"],
        }
    }
}

// ---------------------------------------------------------------- finding

/// The folder the program keeps its own copies of tools in.
pub fn own_dir(home: &Path) -> PathBuf {
    home.join("tools")
}

/// Look for any of `names` in `dir`, `depth` folders down at most; the
/// last in name order wins (so "qpdf-12.3.2" beats "qpdf-11.9.1").
fn search(dir: &Path, names: &[&str], depth: u32) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut entries: Vec<(String, PathBuf, bool)> = rd
        .flatten()
        .map(|e| {
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            (e.file_name().to_string_lossy().into_owned(), e.path(), is_dir)
        })
        .collect();
    entries.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    for (name, path, is_dir) in &entries {
        if !is_dir && names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            return Some(path.clone());
        }
    }
    if depth == 0 {
        return None;
    }
    let mut best = None;
    for (name, path, is_dir) in entries {
        let low = name.to_lowercase();
        // Not worth looking inside these (and the results can be big).
        if !is_dir || low == "results" || low.starts_with('$') || low == "system volume information" || low.starts_with('.') {
            continue;
        }
        if let Some(p) = search(&path, names, depth - 1) {
            best = Some(p);
        }
    }
    best
}

fn on_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for n in names {
            let p = dir.join(n);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(windows)]
fn usual_places(tool: Tool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let pf: Vec<PathBuf> = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
        .iter()
        .filter_map(|v| std::env::var_os(v).map(PathBuf::from))
        .chain([PathBuf::from(r"C:\Program Files"), PathBuf::from(r"C:\Program Files (x86)")])
        .collect();
    match tool {
        Tool::SevenZip => {
            for p in &pf {
                out.push(p.join("7-Zip").join("7z.exe"));
            }
        }
        Tool::Ffmpeg | Tool::Ffprobe => {
            let exe = if tool == Tool::Ffmpeg { "ffmpeg.exe" } else { "ffprobe.exe" };
            if let Some(p) = search(Path::new(r"C:\ffmpeg"), &[exe], 3) {
                out.push(p);
            }
            for p in &pf {
                if let Some(f) = search(&p.join("ffmpeg"), &[exe], 3) {
                    out.push(f);
                }
            }
        }
        Tool::Qpdf => {
            for p in &pf {
                if let Ok(rd) = std::fs::read_dir(p) {
                    let mut dirs: Vec<PathBuf> = rd
                        .flatten()
                        .filter(|e| e.file_name().to_string_lossy().to_lowercase().starts_with("qpdf"))
                        .map(|e| e.path().join("bin").join("qpdf.exe"))
                        .collect();
                    dirs.sort();
                    out.extend(dirs.into_iter().rev());
                }
            }
        }
        Tool::AimCli => {
            for p in &pf {
                for sub in ["AIM Toolkit", "AIMtk", "Arsenal Image Mounter"] {
                    out.push(p.join(sub).join("aim_cli.exe"));
                }
                if let Some(f) = search(p, &["aim_cli.exe"], 2) {
                    out.push(f);
                }
            }
        }
        Tool::Mp3val | Tool::Untrunc => {}
    }
    out
}

#[cfg(not(windows))]
fn usual_places(_tool: Tool) -> Vec<PathBuf> {
    Vec::new()
}

/// Where a tool is, looked for afresh.
pub fn locate(tool: Tool, home: &Path) -> Option<PathBuf> {
    let names = tool.exe_names();
    if tool == Tool::Ffprobe {
        // The FFprobe that came with the FFmpeg in use.
        if let Some(f) = find(Tool::Ffmpeg, home) {
            if let Some(dir) = f.parent() {
                for n in names {
                    if dir.join(n).is_file() {
                        return Some(dir.join(n));
                    }
                }
            }
        }
    }
    if let Some(p) = search(&own_dir(home), names, 4) {
        return Some(p);
    }
    if let Some(p) = usual_places(tool).into_iter().find(|p| p.is_file()) {
        return Some(p);
    }
    if let Some(p) = on_path(names) {
        return Some(p);
    }
    if cfg!(windows) {
        if let Some(p) = search(home, names, 4) {
            return Some(p);
        }
    }
    None
}

static FOUND: Mutex<Vec<(Tool, Option<PathBuf>)>> = Mutex::new(Vec::new());

/// Where a tool is (remembered once found; `forget` looks again).
pub fn find(tool: Tool, home: &Path) -> Option<PathBuf> {
    if let Some(hit) = FOUND.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(t, _)| *t == tool) {
        return hit.1.clone();
    }
    let p = locate(tool, home);
    FOUND.lock().unwrap_or_else(|e| e.into_inner()).push((tool, p.clone()));
    p
}

/// Look for every tool again (after one was installed or downloaded).
pub fn forget() {
    FOUND.lock().unwrap_or_else(|e| e.into_inner()).clear();
    VERSIONS.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

static VERSIONS: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());

/// The first line that says which version a tool is (remembered).
pub fn version(tool: Tool, exe: &Path) -> String {
    if let Some(v) = VERSIONS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(p, _)| p == exe) {
        return v.1.clone();
    }
    let args: Vec<OsString> = tool.version_args().iter().map(OsString::from).collect();
    let v = match run(exe, &args, None, Duration::from_secs(20), &|| false) {
        Ok(r) => version_line(tool, &format!("{}\n{}", r.out, r.err)),
        Err(e) => format!("(could not start it: {e})"),
    };
    VERSIONS.lock().unwrap_or_else(|e| e.into_inner()).push((exe.to_path_buf(), v.clone()));
    v
}

fn version_line(tool: Tool, text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    let pick = match tool {
        Tool::SevenZip => lines.iter().find(|l| l.starts_with("7-Zip") || l.starts_with("p7zip")),
        Tool::Ffmpeg => lines.iter().find(|l| l.starts_with("ffmpeg version")),
        Tool::Ffprobe => lines.iter().find(|l| l.starts_with("ffprobe version")),
        Tool::Mp3val => lines.iter().find(|l| l.to_lowercase().contains("version")),
        Tool::Qpdf => lines.iter().find(|l| l.starts_with("qpdf version")),
        Tool::Untrunc | Tool::AimCli => lines.first(),
    };
    pick.or(lines.first()).map(|l| l.chars().take(120).collect()).unwrap_or_default()
}

/// FFmpeg's version as (major, minor), from "ffmpeg version 9.0.1-essentials..."
/// or "ffmpeg version n8.1"; None for a build from the development tree
/// ("N-123456-g...", taken as the newest).
pub fn ffmpeg_number(version_line: &str) -> Option<(u32, u32)> {
    let rest = version_line.strip_prefix("ffmpeg version ")?.trim_start_matches(['n', 'v']);
    let mut it = rest.split(|c: char| !c.is_ascii_digit());
    let major: u32 = it.next()?.parse().ok()?;
    let minor: u32 = it.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

/// Can this FFmpeg decode iPhone photos (tiled HEIF), from its own command
/// line? FFmpeg 7.0 learnt to read them, and its command line 8.1 to put the
/// tiles together.
pub fn ffmpeg_decodes_heif(version_line: &str) -> bool {
    match ffmpeg_number(version_line) {
        Some(v) => v >= (8, 1),
        None => version_line.contains("version N-"),
    }
}

// ---------------------------------------------------------------- running

/// What a tool did.
#[derive(Clone, Debug, Default)]
pub struct Ran {
    pub code: Option<i32>,
    pub out: String,
    pub err: String,
    pub timed_out: bool,
    pub stopped: bool,
    pub secs: f64,
}

impl Ran {
    pub fn finished_ok(&self) -> bool {
        self.code == Some(0) && !self.timed_out && !self.stopped
    }
    /// Both outputs, the error output first (where tools say what is wrong).
    pub fn text(&self) -> String {
        if self.out.trim().is_empty() {
            self.err.clone()
        } else if self.err.trim().is_empty() {
            self.out.clone()
        } else {
            format!("{}\n{}", self.err, self.out)
        }
    }
}

const KEEP_OUTPUT: u64 = 256 * 1024;

fn temp_name(what: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!("arp-{}-{n}-{t}-{what}.txt", std::process::id()))
}

fn read_kept(p: &Path) -> String {
    use std::io::Read;
    let mut s = Vec::new();
    if let Ok(f) = std::fs::File::open(p) {
        let _ = f.take(KEEP_OUTPUT).read_to_end(&mut s);
    }
    let _ = std::fs::remove_file(p);
    String::from_utf8_lossy(&s).into_owned()
}

/// Run a tool and wait for it, at most `limit`, stopping it if `stop` says
/// so. Its output goes to files (not pipes, which a chatty tool can fill and
/// stall on) and the first 256 KB of each is kept.
pub fn run(exe: &Path, args: &[OsString], dir: Option<&Path>, limit: Duration, stop: &dyn Fn() -> bool) -> std::io::Result<Ran> {
    run_with(exe, args, dir, limit, stop, false)
}

/// The same, at a lower priority than everything else on the PC (for the
/// long checks, so the PC stays usable while they run).
pub fn run_low(exe: &Path, args: &[OsString], dir: Option<&Path>, limit: Duration, stop: &dyn Fn() -> bool) -> std::io::Result<Ran> {
    run_with(exe, args, dir, limit, stop, true)
}

fn run_with(
    exe: &Path,
    args: &[OsString],
    dir: Option<&Path>,
    limit: Duration,
    stop: &dyn Fn() -> bool,
    low: bool,
) -> std::io::Result<Ran> {
    let out_p = temp_name("out");
    let err_p = temp_name("err");
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::fs::File::create(&out_p)?)
        .stderr(std::fs::File::create(&err_p)?);
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        cmd.creation_flags(CREATE_NO_WINDOW | if low { BELOW_NORMAL_PRIORITY_CLASS } else { 0 });
    }
    #[cfg(not(windows))]
    let _ = low;
    let started = Instant::now();
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&out_p);
            let _ = std::fs::remove_file(&err_p);
            return Err(e);
        }
    };
    let mut ran = Ran::default();
    loop {
        match child.try_wait()? {
            Some(status) => {
                ran.code = status.code();
                break;
            }
            None => {
                if stop() {
                    ran.stopped = true;
                } else if started.elapsed() > limit {
                    ran.timed_out = true;
                }
                if ran.stopped || ran.timed_out {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                std::thread::sleep(Duration::from_millis(if started.elapsed() < Duration::from_secs(2) { 20 } else { 100 }));
            }
        }
    }
    ran.secs = started.elapsed().as_secs_f64();
    ran.out = read_kept(&out_p);
    ran.err = read_kept(&err_p);
    Ok(ran)
}

pub fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

// ---------------------------------------------------------------- what is there

/// One tool, as found now.
#[derive(Clone, Debug)]
pub struct Status {
    pub tool: Tool,
    pub path: Option<PathBuf>,
    pub version: String,
    /// The program's own copy (in its tools folder).
    pub own: bool,
}

pub fn status(home: &Path) -> Vec<Status> {
    Tool::ALL
        .iter()
        .filter(|t| cfg!(windows) || **t != Tool::AimCli)
        .map(|&t| {
            let path = find(t, home);
            let version = path.as_ref().map(|p| version(t, p)).unwrap_or_default();
            let own = path.as_ref().is_some_and(|p| p.starts_with(own_dir(home)));
            Status { tool: t, path, version, own }
        })
        .collect()
}

pub fn status_json(home: &Path) -> String {
    use crate::fmt::json_str;
    let items: Vec<String> = status(home)
        .iter()
        .map(|s| {
            let dl = DOWNLOADS.iter().find(|d| d.tool == s.tool);
            format!(
                "{{\"key\":{},\"name\":{},\"used_for\":{},\"path\":{},\"version\":{},\"own\":{},\"download\":{}}}",
                json_str(s.tool.key()),
                json_str(s.tool.name()),
                json_str(s.tool.used_for()),
                s.path.as_ref().map(|p| json_str(&p.display().to_string())).unwrap_or_else(|| "null".to_string()),
                json_str(&s.version),
                s.own,
                dl.map(|d| format!("{{\"version\":{},\"size\":{},\"from\":{}}}", json_str(d.version), d.size, json_str(d.url)))
                    .unwrap_or_else(|| "null".to_string())
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

// ---------------------------------------------------------------- downloading

/// A tool the program can download itself, from the project's own release
/// page on GitHub, at exactly this version: the file must have exactly this
/// size and SHA-256 (as GitHub publishes it), or it is not used.
pub struct Download {
    pub tool: Tool,
    pub version: &'static str,
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    /// The folder it is unpacked into, in the program's tools folder.
    pub folder: &'static str,
}

pub const DOWNLOADS: &[Download] = &[Download {
    tool: Tool::Qpdf,
    version: "12.3.2",
    url: "https://github.com/qpdf/qpdf/releases/download/v12.3.2/qpdf-12.3.2-msvc64.zip",
    size: 24_555_583,
    sha256: "8941870a604e7c87ed24566b038d46c24ce76616254d2383c578f60c0677f202",
    folder: "qpdf-12.3.2",
}];

/// Download a tool into the program's tools folder, check it, unpack it.
pub fn download(tool: Tool, home: &Path, say: &mut dyn FnMut(&str), stop: &dyn Fn() -> bool) -> Result<PathBuf, String> {
    let d = DOWNLOADS
        .iter()
        .find(|d| d.tool == tool)
        .ok_or_else(|| format!("the program does not download {} itself", tool.name()))?;
    if !cfg!(windows) {
        return Err("downloading tools is only done on Windows".to_string());
    }
    let dir = own_dir(home);
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    let zip = dir.join(format!("{}.zip.partial", d.folder));
    let _ = std::fs::remove_file(&zip);
    say(&format!("Downloading {} {} from {}...", tool.name(), d.version, d.url));
    let curl = crate::win::system_program("curl.exe");
    let r = run(
        &curl,
        &[
            OsString::from("--location"),
            OsString::from("--fail"),
            OsString::from("--silent"),
            OsString::from("--show-error"),
            OsString::from("--output"),
            zip.clone().into_os_string(),
            OsString::from(d.url),
        ],
        None,
        Duration::from_secs(1800),
        stop,
    )
    .map_err(|e| format!("could not start Windows' curl.exe to download it: {e}"))?;
    if r.stopped {
        let _ = std::fs::remove_file(&zip);
        return Err("stopped".to_string());
    }
    if !r.finished_ok() {
        let _ = std::fs::remove_file(&zip);
        return Err(format!(
            "the download did not work (is the PC online?): {}",
            r.text().lines().last().unwrap_or("no reason given")
        ));
    }
    let got = std::fs::metadata(&zip).map(|m| m.len()).unwrap_or(0);
    let sha = crate::sha256::file_hex(&zip, stop).map_err(|e| format!("could not read the download: {e}"))?;
    let Some(sha) = sha else {
        let _ = std::fs::remove_file(&zip);
        return Err("stopped".to_string());
    };
    if got != d.size || !sha.eq_ignore_ascii_case(d.sha256) {
        let _ = std::fs::remove_file(&zip);
        return Err(format!(
            "the downloaded file is not the one expected ({got} bytes, SHA-256 {sha}; expected {} bytes, SHA-256 {}), so it \
             was thrown away and not used",
            d.size, d.sha256
        ));
    }
    say(&format!("Downloaded and checked: {} bytes, SHA-256 {sha}. Unpacking...", commas_of(got)));
    let dest = dir.join(d.folder);
    let tmp = dir.join(format!("{}.unpacking", d.folder));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("could not make {}: {e}", tmp.display()))?;
    let tar = crate::win::system_program("tar.exe");
    let r = run(&tar, &[OsString::from("-xf"), zip.clone().into_os_string(), OsString::from("-C"), tmp.clone().into_os_string()], None, Duration::from_secs(600), stop)
        .map_err(|e| format!("could not start Windows' tar.exe to unpack it: {e}"))?;
    if !r.finished_ok() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("it could not be unpacked: {}", r.text().trim()));
    }
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&tmp, &dest).map_err(|e| format!("could not put it in place: {e}"))?;
    let _ = std::fs::rename(&zip, dir.join(format!("{}.zip", d.folder)));
    forget();
    let exe = search(&dest, tool.exe_names(), 4).ok_or_else(|| format!("{} was not in what was downloaded", tool.exe_names()[0]))?;
    say(&format!("{} is ready: {}", tool.name(), exe.display()));
    Ok(exe)
}

fn commas_of(n: u64) -> String {
    crate::fmt::commas(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_versions_are_read() {
        assert_eq!(ffmpeg_number("ffmpeg version 9.0.1-essentials_build-www.gyan.dev Copyright (c)"), Some((9, 0)));
        assert_eq!(ffmpeg_number("ffmpeg version n8.1-2-g1234 Copyright"), Some((8, 1)));
        assert_eq!(ffmpeg_number("ffmpeg version 6.1.1-3ubuntu5 Copyright"), Some((6, 1)));
        assert_eq!(ffmpeg_number("ffmpeg version N-117123-gabcdef"), None);
        assert!(ffmpeg_decodes_heif("ffmpeg version 9.0.1-essentials_build-www.gyan.dev"));
        assert!(ffmpeg_decodes_heif("ffmpeg version 8.1 Copyright"));
        assert!(!ffmpeg_decodes_heif("ffmpeg version 8.0.1 Copyright"));
        assert!(!ffmpeg_decodes_heif("ffmpeg version 6.1.1-3ubuntu5"));
        assert!(ffmpeg_decodes_heif("ffmpeg version N-117123-gabcdef"));
    }

    #[test]
    fn version_lines_are_picked() {
        assert_eq!(version_line(Tool::Qpdf, "qpdf version 12.3.2\nRun qpdf --copyright"), "qpdf version 12.3.2");
        assert_eq!(
            version_line(Tool::SevenZip, "\n7-Zip 26.03 (x64) : Copyright (c) 1999-2026 Igor Pavlov\n\nUsage: 7z"),
            "7-Zip 26.03 (x64) : Copyright (c) 1999-2026 Igor Pavlov"
        );
        assert_eq!(version_line(Tool::Mp3val, "MP3val - a program for MPEG audio stream validation\nVersion 0.1.8\n"), "Version 0.1.8");
    }

    #[test]
    fn a_tool_is_run_with_a_time_limit_and_can_be_stopped() {
        if cfg!(windows) {
            return;
        }
        let sh = PathBuf::from("/bin/sh");
        let r = run(&sh, &os(&["-c", "echo out; echo err >&2; exit 3"]), None, Duration::from_secs(10), &|| false).unwrap();
        assert_eq!(r.code, Some(3));
        assert_eq!(r.out.trim(), "out");
        assert_eq!(r.err.trim(), "err");
        let t = Instant::now();
        let r = run(&sh, &os(&["-c", "sleep 30"]), None, Duration::from_millis(300), &|| false).unwrap();
        assert!(r.timed_out && t.elapsed() < Duration::from_secs(5));
        let t = Instant::now();
        let r = run(&sh, &os(&["-c", "sleep 30"]), None, Duration::from_secs(60), &|| t.elapsed() > Duration::from_millis(200)).unwrap();
        assert!(r.stopped && t.elapsed() < Duration::from_secs(5));
        // A tool that writes a lot does not stall.
        let r = run(&sh, &os(&["-c", "head -c 3000000 /dev/zero | tr '\\0' x; echo"]), None, Duration::from_secs(30), &|| false).unwrap();
        assert!(r.finished_ok());
        assert_eq!(r.out.len() as u64, KEEP_OUTPUT);
    }

    #[test]
    fn tools_are_found_in_the_programs_own_folder_first() {
        let home = std::env::temp_dir().join(format!("arp-tools-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let name = Tool::Qpdf.exe_names()[0];
        for v in ["qpdf-11.9.1", "qpdf-12.3.2"] {
            let d = own_dir(&home).join(v).join("bin");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(name), b"x").unwrap();
        }
        assert_eq!(locate(Tool::Qpdf, &home), Some(own_dir(&home).join("qpdf-12.3.2").join("bin").join(name)));
        let _ = std::fs::remove_dir_all(&home);
    }
}
