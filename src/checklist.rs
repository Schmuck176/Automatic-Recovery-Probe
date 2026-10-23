//! The recovery checklist, built into the program.
//!
//! `docs/automatic-checklist.md` is part of the program (Recovery Probe's own,
//! longer one, `docs/recovery-checklist.md`, is not: the person's copy of it in
//! Obsidian is left exactly as it is). The page shows it with
//! its tick boxes, and the ticks are kept in `results\automatic-checklist-ticks.txt`,
//! each with when it was ticked (a box ticked by mistake is unticked the same
//! way; the file keeps both).
//!
//! The person's own copy of the checklist (the one in Obsidian) is kept the
//! same: ticks made there are taken in, and the program writes the copy back
//! with every change - unless the person has written something of their own
//! into it, which the program never overwrites.
//!
//! A checkpoint box (one with ✅) is not ticked by hand: it shows as ticked
//! when a safety check (step 7, "Check the copies are unchanged") made after
//! the boxes before it passed. The boxes before it are the ones of its own
//! phase (and of any phase it says it covers), so a checkpoint says whether
//! what was done in that phase left every copy as it was.

use crate::fmt::{commas, json_str, size};
use crate::verify::Checked;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The checklist and the download manifest, as built into the program.
pub const TEXT: &str = include_str!("../docs/automatic-checklist.md");
pub const MANIFEST: &str = include_str!("../docs/automatic-manifest.md");
/// (Not Recovery Probe's names: its checklist, manifest and ticks are its own.)
pub const COPY_NAME: &str = "automatic-recovery-checklist.md";
pub const MANIFEST_NAME: &str = "automatic-download-manifest.md";
pub const TICKS_NAME: &str = "automatic-checklist-ticks.txt";

/// Lines whose wording changed since a checklist the person has ticked:
/// (old, new), so a tick follows its line. (None yet.)
const RENAMED: &[(&str, &str)] = &[];

/// Checklists and manifests delivered before (their text with ticks and Your
/// numbers left out, hashed): a copy that is one of these, or the program's
/// own, holds nothing the person wrote. (None yet: version 2026-10-23 (1) is
/// the first.)
const KNOWN_CHECKLISTS: &[u64] = &[];
const KNOWN_MANIFESTS: &[u64] = &[];

/// One tick box.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Its line in the text (from 0).
    pub line: usize,
    /// Its text, with " #2", " #3"... when the same text came before.
    pub key: String,
    /// Its phase (0: before the first phase).
    pub phase: usize,
    pub checkpoint: bool,
    /// The final check: two copies, both checked.
    pub both_copies: bool,
    /// Other phases a checkpoint covers ("covers Phases 3 and 4").
    pub covers: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Phase {
    /// The heading, without the "# ".
    pub title: String,
    /// "−1", "0", "1"... ("" for the part before the first phase).
    pub number: String,
}

/// The tick boxes and phases of a checklist text.
pub fn parse(text: &str) -> (Vec<Phase>, Vec<Item>) {
    let mut phases = vec![Phase { title: String::new(), number: String::new() }];
    let mut raw: Vec<(usize, String, usize)> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut fence = false;
    for (n, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        if let Some(t) = line.strip_prefix("# PHASE ") {
            let number: String = t.chars().take_while(|c| !c.is_whitespace()).collect();
            phases.push(Phase { title: line[2..].trim().to_string(), number });
            continue;
        }
        if let Some((_, _, text)) = box_parts(line) {
            let k = seen.entry(text.to_string()).or_insert(0);
            *k += 1;
            let key = if *k == 1 { text.to_string() } else { format!("{text} #{k}") };
            raw.push((n, key, phases.len() - 1));
        }
    }
    let items = raw
        .into_iter()
        .map(|(line, key, phase)| {
            let checkpoint = key.contains('✅');
            let covers = covered_numbers(&key)
                .iter()
                .filter_map(|num| phases.iter().position(|p| &p.number == num))
                .filter(|&p| p != phase)
                .collect();
            Item { line, both_copies: checkpoint && key.contains("both copies"), key, phase, checkpoint, covers }
        })
        .collect();
    (phases, items)
}

/// (indent, ticked, text) of a tick box line.
fn box_parts(line: &str) -> Option<(usize, bool, &str)> {
    let t = line.trim_start();
    let indent = line.len() - t.len();
    let rest = t.strip_prefix("- [")?;
    let mut c = rest.chars();
    let mark = c.next()?;
    let text = c.as_str().strip_prefix("] ")?;
    match mark {
        ' ' => Some((indent, false, text.trim_end())),
        'x' | 'X' => Some((indent, true, text.trim_end())),
        _ => None,
    }
}

/// "covers Phases 3 and 4" -> ["3", "4"].
fn covered_numbers(text: &str) -> Vec<String> {
    let Some(at) = text.find("covers Phases ") else { return Vec::new() };
    let rest = &text[at + "covers Phases ".len()..];
    let end = rest.find(')').unwrap_or(rest.len());
    rest[..end].split(|c: char| c == ',' || c.is_whitespace()).filter(|w| !w.is_empty() && *w != "and").map(String::from).collect()
}

fn renamed(key: &str) -> &str {
    RENAMED.iter().find(|(old, _)| *old == key).map(|(_, new)| *new).unwrap_or(key)
}

// ---------------------------------------------------------------- ticks

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tick {
    pub on: bool,
    /// Local date and time with its offset from UTC (fmt::when_of),
    /// "2026-10-25 01:30:00 +01:00"; ticks made before 22 October 2026 have
    /// no offset. "" when not known.
    pub when: String,
}

/// Every box's latest state: ticked or not, and since when.
#[derive(Debug, Clone, Default)]
pub struct Ticks {
    pub map: HashMap<String, Tick>,
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

impl Ticks {
    /// Reads the ticks file: one line per tick or untick, the last one of a
    /// box counts.
    pub fn load(path: &Path) -> Ticks {
        let mut t = Ticks::default();
        let Ok(text) = std::fs::read_to_string(path) else { return t };
        for line in text.lines() {
            let f: Vec<&str> = line.splitn(3, '\t').collect();
            if let [mark @ ("T" | "U"), when, key] = f.as_slice() {
                let key = unescape(key);
                t.map.insert(renamed(&key).to_string(), Tick { on: *mark == "T", when: unescape(when) });
            }
        }
        t
    }

    pub fn on(&self, key: &str) -> bool {
        self.map.get(key).is_some_and(|t| t.on)
    }

    pub fn when(&self, key: &str) -> &str {
        self.map.get(key).map(|t| t.when.as_str()).unwrap_or("")
    }

    /// Ticks or unticks a box, and adds it to the ticks file.
    pub fn set(&mut self, path: &Path, key: &str, on: bool, when: &str) -> Result<(), String> {
        use std::io::Write;
        let mut line = String::new();
        if !path.exists() {
            line.push_str("# Automatic Recovery Probe: the checklist's tick boxes, one line each time one is ticked (T) or unticked (U).\r\n");
        }
        line.push_str(&format!("{}\t{}\t{}\r\n", if on { "T" } else { "U" }, escape(when), escape(key)));
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Made safe on the drive at once: a tick is the person's record of
        // what they have done.
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| {
                f.write_all(line.as_bytes())?;
                f.sync_all()
            })
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
        self.map.insert(key.to_string(), Tick { on, when: when.to_string() });
        Ok(())
    }
}

// ---------------------------------------------------------------- checkpoints

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum State {
    /// Nothing before it in its phase is ticked yet.
    Waiting,
    /// Some of the boxes before it are still to do.
    After,
    /// Everything before it is ticked, and no check since.
    Due,
    /// The final check: one copy checked, the other not yet.
    Partly,
    /// A check after the boxes before it passed.
    Passed,
    /// The newest check after them found something.
    Failed,
    /// Ticked by hand when nothing in its phase was done (the phase skipped).
    Skipped,
}

impl State {
    fn word(self) -> &'static str {
        match self {
            State::Waiting => "waiting",
            State::After => "after",
            State::Due => "due",
            State::Partly => "partly",
            State::Passed => "passed",
            State::Failed => "failed",
            State::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub item: usize,
    pub state: State,
    /// In plain words, for beside the box.
    pub text: String,
}

/// The boxes a checkpoint covers: the ones before it in its own phase, and
/// all of any other phase it names.
fn covered(items: &[Item], c: usize) -> impl Iterator<Item = usize> + '_ {
    let cp = &items[c];
    (0..c).filter(move |&j| !items[j].checkpoint && (items[j].phase == cp.phase || cp.covers.contains(&items[j].phase)))
}

fn check_words(c: &Checked) -> String {
    let mut w = Vec::new();
    if let Some(f) = c.files {
        w.push(format!("{} files", commas(f)));
    }
    if let Some(b) = c.bytes {
        w.push(size(b));
    }
    w.join(", ")
}

/// Where every checkpoint stands, from the ticks and the list of every check.
pub fn statuses(items: &[Item], ticks: &Ticks, history: &[Checked]) -> Vec<Status> {
    let mut out = Vec::new();
    for (c, cp) in items.iter().enumerate().filter(|(_, it)| it.checkpoint) {
        let cov: Vec<usize> = covered(items, c).collect();
        let done: Vec<usize> = cov.iter().copied().filter(|&j| ticks.on(&items[j].key)).collect();
        let st = |state: State, text: String| Status { item: c, state, text };
        if done.is_empty() {
            out.push(if ticks.on(&cp.key) {
                st(State::Skipped, "Skipped: nothing before it in this phase was ticked.".to_string())
            } else {
                st(State::Waiting, String::new())
            });
            continue;
        }
        // Compared as moments, not as text: local times repeat an hour when
        // the clocks go back (fmt::instant). A time not known counts as the
        // earliest.
        let at = |w: &str| crate::fmt::instant(w).unwrap_or(i64::MIN);
        let since = done.iter().map(|&j| at(ticks.when(&items[j].key))).max().unwrap_or(i64::MIN);
        let checks: Vec<&Checked> =
            history.iter().filter(|h| h.step == crate::verify::STEP7 && at(&h.when) >= since).collect();
        let all_done = done.len() == cov.len();
        let waiting = || {
            if all_done {
                st(
                    State::Due,
                    "Due now: Start, Bring back and Finalise check the copies by themselves (or the program's steps: step 7 → \
                     Check the copies are unchanged)."
                        .to_string(),
                )
            } else {
                st(
                    State::After,
                    "When the boxes above are done: Start, Bring back and Finalise check the copies by themselves.".to_string(),
                )
            }
        };
        let failed = |h: &Checked| {
            st(
                State::Failed,
                format!(
                    "✗ The check of {} found something: {} Stop, and tell Claude.",
                    crate::fmt::when_shown(&h.when),
                    h.result.trim_start_matches("NOT OK: ")
                ),
            )
        };
        let passed = |h: &Checked, both: bool| {
            st(
                State::Passed,
                format!(
                    "✓ {} {}: {} — nothing missing, nothing changed.",
                    if both { "Both copies checked, the last on" } else { "Checked" },
                    crate::fmt::when_shown(&h.when),
                    check_words(h)
                ),
            )
        };
        if cp.both_copies {
            // The newest check of each folder: two folders, both passed.
            let mut newest: Vec<&Checked> = Vec::new();
            for h in checks.iter().rev() {
                if !newest.iter().any(|n| same_folder(&n.folder, &h.folder)) {
                    newest.push(h);
                }
            }
            if let Some(bad) = newest.iter().find(|h| !h.ok) {
                out.push(failed(bad));
            } else if newest.len() >= 2 {
                out.push(passed(newest[0], true));
            } else if let Some(one) = newest.first() {
                out.push(st(
                    State::Partly,
                    format!(
                        "One copy checked ({}, {}): now check the other one.",
                        crate::fmt::when_shown(&one.when),
                        one.folder
                    ),
                ));
            } else {
                out.push(waiting());
            }
            continue;
        }
        out.push(match checks.last() {
            Some(h) if h.ok => passed(h, false),
            Some(h) => failed(h),
            None => waiting(),
        });
    }
    out
}

fn same_folder(a: &str, b: &str) -> bool {
    let n = |p: &str| p.trim().trim_end_matches(['\\', '/']).to_lowercase();
    n(a) == n(b)
}

/// Whether each box shows as ticked: by hand, or (for a checkpoint) by the
/// check that passed.
pub fn ticked(items: &[Item], ticks: &Ticks, st: &[Status]) -> Vec<bool> {
    items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            if it.checkpoint {
                st.iter().any(|s| s.item == i && matches!(s.state, State::Passed | State::Skipped))
            } else {
                ticks.on(&it.key)
            }
        })
        .collect()
}

/// The box to do next: the first not ticked after the last one ticked.
pub fn next(done: &[bool]) -> Option<usize> {
    let start = done.iter().rposition(|&d| d).map(|i| i + 1).unwrap_or(0);
    (start..done.len()).find(|&i| !done[i]).or_else(|| done.iter().position(|&d| !d))
}

/// Can this box be ticked (or unticked) by hand? A checkpoint only as
/// "skipped", when nothing in its phase was done.
pub fn may_set(items: &[Item], ticks: &Ticks, i: usize, on: bool) -> Result<(), String> {
    let it = items.get(i).ok_or("no such box")?;
    if !it.checkpoint || !on {
        return Ok(());
    }
    if covered(items, i).any(|j| ticks.on(&items[j].key)) {
        return Err(
            "This box ticks itself when the safety check passes: step 7 → Check the copies are unchanged. (By hand it can \
             only be ticked as skipped, when nothing in its phase was done.)"
                .to_string(),
        );
    }
    Ok(())
}

// ---------------------------------------------------------------- the copy in Obsidian

fn fnv64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

const NUMBERS_HEADER: &str = "| Files on the SSD | Size in all |";

/// A text as it is apart from ticks, Your numbers and line endings.
fn normal(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut numbers_row = 0;
    for line in text.replace("\r\n", "\n").lines() {
        let mut l = line.trim_end().to_string();
        if let Some((indent, true, rest)) = box_parts(&l) {
            l = format!("{}- [ ] {rest}", " ".repeat(indent));
        }
        if numbers_row == 2 && l.starts_with("> |") {
            l = "> |  |  |".to_string();
        }
        numbers_row = if l.contains(NUMBERS_HEADER) { 1 } else if numbers_row == 1 { 2 } else { 0 };
        out.push(l);
    }
    out.join("\n").trim_end().to_string()
}

/// The checklist with these ticks, and Your numbers filled in when known.
pub fn markdown(text: &str, items: &[Item], done: &[bool], numbers: Option<(u64, u64)>) -> String {
    let by_line: HashMap<usize, usize> = items.iter().enumerate().map(|(i, it)| (it.line, i)).collect();
    let mut out = String::with_capacity(text.len() + 256);
    let mut numbers_row = 0;
    for (n, line) in text.lines().enumerate() {
        let mut l = line.to_string();
        if let (Some(&i), Some((indent, _, rest))) = (by_line.get(&n), box_parts(line)) {
            l = format!("{}- [{}] {rest}", " ".repeat(indent), if done[i] { "x" } else { " " });
        }
        if numbers_row == 2 && l.starts_with("> |") {
            if let Some((f, b)) = numbers {
                l = format!("> | {} | {} |", commas(f), size(b));
            }
        }
        numbers_row = if l.contains(NUMBERS_HEADER) { 1 } else if numbers_row == 1 { 2 } else { 0 };
        out.push_str(&l);
        out.push('\n');
    }
    out
}

/// The boxes a copy has ticked or not, by key.
pub fn ticks_in(copy: &str) -> HashMap<String, bool> {
    let (_, items) = parse(copy);
    let lines: Vec<&str> = copy.lines().collect();
    items
        .iter()
        .filter_map(|it| box_parts(lines[it.line]).map(|(_, on, _)| (renamed(&it.key).to_string(), on)))
        .collect()
}

/// Is this copy a checklist the program (or Claude) wrote, apart from ticks
/// and Your numbers? Then nothing the person wrote is lost by writing over it.
pub fn is_unedited(copy: &str, written_hash: Option<u64>) -> bool {
    let h = fnv64(&normal(copy));
    h == fnv64(&normal(TEXT)) || KNOWN_CHECKLISTS.contains(&h) || written_hash == Some(h)
}

/// The same for a copy of the download manifest.
pub fn manifest_unedited(copy: &str) -> bool {
    let h = fnv64(&normal(copy));
    h == fnv64(&normal(MANIFEST)) || KNOWN_MANIFESTS.contains(&h)
}

/// The hash `is_unedited` compares, for a text the program writes.
pub fn written_hash(text: &str) -> u64 {
    fnv64(&normal(text))
}

/// The person's copy of the checklist: the newest `recovery-checklist.md` in
/// the program's folder or a folder in it (two deep), not in results.
pub fn find_copy(home: &Path) -> Option<PathBuf> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let mut dirs = vec![(home.to_path_buf(), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                if depth < 2 && !name.starts_with('.') && !name.eq_ignore_ascii_case("results") {
                    dirs.push((e.path(), depth + 1));
                }
            } else if name.eq_ignore_ascii_case(COPY_NAME) {
                if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                    found.push((m, e.path()));
                }
            }
        }
    }
    found.into_iter().max_by_key(|(m, _)| *m).map(|(_, p)| p)
}

/// Where a first copy of this checklist goes: beside Recovery Probe's own
/// checklist copy (the person's Obsidian folder), if there is one; else in
/// the program's folder.
pub fn first_copy_place(home: &Path) -> PathBuf {
    let mut dirs = vec![(home.to_path_buf(), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                if depth < 2 && !name.starts_with('.') && !name.eq_ignore_ascii_case("results") {
                    dirs.push((e.path(), depth + 1));
                }
            } else if name.eq_ignore_ascii_case("recovery-checklist.md") {
                return dir.join(COPY_NAME);
            }
        }
    }
    home.join(COPY_NAME)
}

/// Writes `text` to `path` by way of a file beside it, made safe on the
/// drive first, so that a cut-off write or a power cut never leaves half a
/// checklist - or a whole one full of zeros (sys::write_safely).
pub fn write_whole(path: &Path, text: &str) -> Result<(), String> {
    crate::sys::write_safely(path, text.as_bytes()).map_err(|e| format!("could not write {}: {e}", path.display()))
}

// ---------------------------------------------------------------- the page

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Inline markdown: `code`, **bold**, *italic*, and "Step N →" as a link to
/// the program's step.
pub fn inline(s: &str) -> String {
    // Code spans first, kept aside.
    let parts: Vec<&str> = s.split('`').collect();
    let mut codes: Vec<String> = Vec::new();
    let mut t = String::new();
    for (k, p) in parts.iter().enumerate() {
        if k % 2 == 1 && k + 1 < parts.len() {
            t.push_str(&format!("\u{E000}{}\u{E001}", codes.len()));
            codes.push(format!("<code>{}</code>", esc(p)));
        } else {
            if k % 2 == 1 {
                t.push('`');
            }
            t.push_str(&esc(p));
        }
    }
    for n in 1..=9 {
        for w in ["Step", "step"] {
            t = t.replace(
                &format!("{w} {n} →"),
                &format!("<a href=\"#\" class=\"steplink\" data-step=\"{n}\">{w} {n}</a> →"),
            );
        }
    }
    let t = pairs(&t, "**", "b");
    let mut t = pairs(&t, "*", "i");
    for (n, c) in codes.iter().enumerate() {
        t = t.replace(&format!("\u{E000}{n}\u{E001}"), c);
    }
    t
}

/// `**a**` -> `<b>a</b>`: pairs of `mark` become a tag; one left over stays.
fn pairs(s: &str, mark: &str, tag: &str) -> String {
    let parts: Vec<&str> = s.split(mark).collect();
    if parts.len() < 3 {
        return s.to_string();
    }
    let mut out = String::new();
    let paired = if parts.len() % 2 == 1 { parts.len() } else { parts.len() - 1 };
    for (k, p) in parts.iter().enumerate() {
        if k > 0 {
            if k < paired {
                let t = if k % 2 == 1 { format!("<{tag}>") } else { format!("</{tag}>") };
                out.push_str(&t);
            } else {
                out.push_str(mark);
            }
        }
        out.push_str(p);
    }
    out
}

enum Kind {
    Box,
    Bullet,
    Number(u32),
}

fn list_item(line: &str) -> Option<(usize, Kind, &str)> {
    if let Some((indent, _, text)) = box_parts(line) {
        return Some((indent, Kind::Box, text));
    }
    let t = line.trim_start();
    let indent = line.len() - t.len();
    if let Some(rest) = t.strip_prefix("- ") {
        return Some((indent, Kind::Bullet, rest));
    }
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        if let Some(rest) = t[digits.len()..].strip_prefix(". ") {
            return Some((indent, Kind::Number(digits.parse().unwrap_or(1)), rest));
        }
    }
    None
}

fn block_start(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || line.starts_with('#') || line.starts_with('>') || line.starts_with('|') || line.trim() == "---"
}

/// The checklist as HTML for the page: one section per phase, every tick box
/// an `input.tick` with `data-i` (its number), every checkpoint a `div.cp`
/// for its state, and Your numbers' cells `data-num`.
pub fn html(text: &str, items: &[Item]) -> String {
    let by_line: HashMap<usize, usize> = items.iter().enumerate().map(|(i, it)| (it.line, i)).collect();
    let lines: Vec<(usize, String)> = text.lines().map(String::from).enumerate().collect();
    let mut out = String::from("<section class=\"ph\" id=\"ph-0\">");
    let mut phase = 0;
    blocks(&lines, &by_line, items, &mut out, Some(&mut phase));
    out.push_str("</section>");
    out
}

fn blocks(
    lines: &[(usize, String)],
    by_line: &HashMap<usize, usize>,
    items: &[Item],
    out: &mut String,
    mut phase: Option<&mut usize>,
) {
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i].1.trim_end();
        if raw.trim().is_empty() {
            i += 1;
            continue;
        }
        let t = raw.trim_start();
        if t.starts_with("```") {
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].1.trim_start().starts_with("```") {
                code.push(esc(&lines[i].1));
                i += 1;
            }
            i += 1;
            out.push_str(&format!("<pre class=\"code\"><code>{}</code></pre>", code.join("\n")));
            continue;
        }
        if raw.starts_with('#') {
            let level = raw.chars().take_while(|&c| c == '#').count();
            let title = raw[level..].trim();
            match phase.as_deref_mut() {
                Some(p) if level == 1 && title.starts_with("PHASE ") => {
                    *p += 1;
                    out.push_str(&format!(
                        "</section><section class=\"ph\" id=\"ph-{p}\"><h2 class=\"phase-title\">{} <span class=\"phcount\" \
                         data-ph=\"{p}\"></span></h2>",
                        inline(title)
                    ));
                }
                _ => {
                    let h = (level + 1).min(5);
                    out.push_str(&format!("<h{h}>{}</h{h}>", inline(title)));
                }
            }
            i += 1;
            continue;
        }
        if raw.trim() == "---" {
            out.push_str("<hr>");
            i += 1;
            continue;
        }
        if raw.starts_with('>') {
            let mut inner = Vec::new();
            while i < lines.len() && lines[i].1.starts_with('>') {
                let l = &lines[i].1[1..];
                inner.push((lines[i].0, l.strip_prefix(' ').unwrap_or(l).to_string()));
                i += 1;
            }
            out.push_str("<blockquote>");
            blocks(&inner, by_line, items, out, None);
            out.push_str("</blockquote>");
            continue;
        }
        if raw.starts_with('|') && lines.get(i + 1).is_some_and(|l| l.1.starts_with('|') && l.1.contains("---")) {
            let cells = |l: &str| -> Vec<String> {
                let l = l.trim().trim_start_matches('|').trim_end_matches('|');
                l.split('|').map(|c| c.trim().to_string()).collect()
            };
            let head = cells(raw);
            let numbers = raw.contains(NUMBERS_HEADER);
            out.push_str("<div class=\"tablewrap\"><table><tr>");
            for c in &head {
                out.push_str(&format!("<th>{}</th>", inline(c)));
            }
            out.push_str("</tr>");
            i += 2;
            while i < lines.len() && lines[i].1.starts_with('|') {
                out.push_str("<tr>");
                for (k, c) in cells(&lines[i].1).iter().enumerate() {
                    if numbers {
                        let which = if k == 0 { "files" } else { "bytes" };
                        out.push_str(&format!("<td class=\"num\" data-num=\"{which}\">{}</td>", inline(c)));
                    } else {
                        out.push_str(&format!("<td>{}</td>", inline(c)));
                    }
                }
                out.push_str("</tr>");
                i += 1;
            }
            out.push_str("</table></div>");
            continue;
        }
        if list_item(raw).is_some() {
            // The list: its items, and the lines that carry on an item.
            let mut entries: Vec<(usize, usize, Kind, String)> = Vec::new();
            while i < lines.len() {
                let l = lines[i].1.trim_end();
                if l.trim().is_empty() || (block_start(l) && list_item(l).is_none()) {
                    break;
                }
                match list_item(l) {
                    Some((indent, kind, text)) => entries.push((lines[i].0, indent, kind, text.to_string())),
                    None => {
                        if let Some(last) = entries.last_mut() {
                            last.3.push(' ');
                            last.3.push_str(l.trim());
                        }
                    }
                }
                i += 1;
            }
            list(&entries, by_line, items, out);
            continue;
        }
        // A paragraph.
        let mut para = vec![t.to_string()];
        i += 1;
        while i < lines.len() {
            let l = lines[i].1.trim_end();
            if l.trim().is_empty() || block_start(l) || list_item(l).is_some() {
                break;
            }
            para.push(l.trim().to_string());
            i += 1;
        }
        out.push_str(&format!("<p>{}</p>", inline(&para.join(" "))));
    }
}

fn list(entries: &[(usize, usize, Kind, String)], by_line: &HashMap<usize, usize>, items: &[Item], out: &mut String) {
    let mut open: Vec<(usize, &'static str)> = Vec::new();
    for (line, indent, kind, text) in entries {
        let tag = if matches!(kind, Kind::Number(_)) { "ol" } else { "ul" };
        while open.last().is_some_and(|(ind, _)| ind > indent) {
            let (_, t) = open.pop().unwrap();
            out.push_str(&format!("</li></{t}>"));
        }
        match open.last() {
            Some((ind, t)) if ind == indent && *t == tag => out.push_str("</li>"),
            Some((ind, t)) if ind == indent => {
                out.push_str(&format!("</li></{t}>"));
                open.pop();
                open_list(out, tag, kind);
                open.push((*indent, tag));
            }
            _ => {
                open_list(out, tag, kind);
                open.push((*indent, tag));
            }
        }
        match (kind, by_line.get(line)) {
            (Kind::Box, Some(&i)) => {
                let cp = items[i].checkpoint;
                out.push_str(&format!(
                    "<li class=\"box{}\" id=\"it-{i}\"><input type=\"checkbox\" class=\"tick\" data-i=\"{i}\" \
                     aria-labelledby=\"bt-{i}\"><div class=\"box-text\" id=\"bt-{i}\">{}</div>{}",
                    if cp { " checkpoint" } else { "" },
                    inline(text),
                    if cp { format!("<div class=\"cp\" data-cp=\"{i}\"></div>") } else { String::new() }
                ));
            }
            _ => out.push_str(&format!("<li>{}", inline(text))),
        }
    }
    while let Some((_, t)) = open.pop() {
        out.push_str(&format!("</li></{t}>"));
    }
}

fn open_list(out: &mut String, tag: &str, kind: &Kind) {
    match kind {
        Kind::Number(n) if *n != 1 => out.push_str(&format!("<ol start=\"{n}\">")),
        Kind::Box => out.push_str("<ul class=\"boxes\">"),
        _ => out.push_str(&format!("<{tag}>")),
    }
}

/// What the page needs once: the HTML, and every box's phase.
pub fn page_json(text: &str) -> String {
    let (phases, items) = parse(text);
    let ph: Vec<String> = phases
        .iter()
        .map(|p| format!("{{\"title\":{},\"number\":{}}}", json_str(&p.title), json_str(&p.number)))
        .collect();
    let it: Vec<String> = items
        .iter()
        .map(|i| format!("{{\"phase\":{},\"checkpoint\":{},\"text\":{}}}", i.phase, i.checkpoint, json_str(&i.key)))
        .collect();
    format!(
        "{{\"html\":{},\"phases\":[{}],\"items\":[{}]}}",
        json_str(&html(text, &items)),
        ph.join(","),
        it.join(",")
    )
}

/// Where everything stands, for the page.
pub fn state_json(
    phases: &[Phase],
    items: &[Item],
    done: &[bool],
    st: &[Status],
    numbers: Option<(u64, u64)>,
    copy_path: &str,
    copy_note: &str,
) -> String {
    let mut per: Vec<(u64, u64)> = vec![(0, 0); phases.len()];
    for (i, it) in items.iter().enumerate() {
        per[it.phase].1 += 1;
        if done[i] {
            per[it.phase].0 += 1;
        }
    }
    let nx = next(done);
    let num = |v: Option<u64>| v.map(|x| x.to_string()).unwrap_or_else(|| "null".to_string());
    format!(
        "{{\"ticked\":[{}],\"status\":[{}],\"phases\":[{}],\"done\":{},\"total\":{},\"next\":{},\"numbers\":{},\
         \"numbers_text\":{},\"copy\":{},\"copy_note\":{}}}",
        done.iter().map(|&d| if d { "1" } else { "0" }).collect::<Vec<_>>().join(","),
        st.iter()
            .map(|s| format!("{{\"i\":{},\"state\":\"{}\",\"text\":{}}}", s.item, s.state.word(), json_str(&s.text)))
            .collect::<Vec<_>>()
            .join(","),
        per.iter().map(|(d, t)| format!("{{\"done\":{d},\"total\":{t}}}")).collect::<Vec<_>>().join(","),
        done.iter().filter(|&&d| d).count(),
        done.len(),
        num(nx.map(|n| n as u64)),
        numbers.map(|(f, b)| format!("{{\"files\":{f},\"bytes\":{b}}}")).unwrap_or_else(|| "null".to_string()),
        numbers
            .map(|(f, b)| format!("{{\"files\":{},\"bytes\":{}}}", json_str(&commas(f)), json_str(&size(b))))
            .unwrap_or_else(|| "null".to_string()),
        json_str(copy_path),
        json_str(copy_note)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# Title\n\nIntro with `code` and **bold**.\n\n# PHASE −1 — Prepare\n\n- [ ] one\n- [x] two\n  - a sub-point\n- [ ] one\n\n\
                       # PHASE 2 — Copy\n\n- [ ] **1. Step 5 → Find.**\n\n> **Your numbers**\n>\n> | Files on the SSD | Size in all |\n> |---|---|\n> |  |  |\n\n\
                       - [ ] **2.** copy\n- [ ] **3. ✅ Checkpoint — Step 7 → Check.**\n\n# PHASE 3 — Scan\n\n- [ ] scan\n\n# PHASE 4 — Extra\n\n- [ ] extra\n\
                       - [ ] **✅ Checkpoint** (covers Phases 3 and 4): check.\n\n# PHASE 8 — Safe\n\n- [ ] second copy\n\
                       - [ ] **3. ✅ The final check — both copies.** Both.\n\n```\n- [ ] not a box\n```\n";

    fn check(when: &str, folder: &str, ok: bool) -> Checked {
        Checked {
            when: when.to_string(),
            step: crate::verify::STEP7.to_string(),
            folder: folder.to_string(),
            files: Some(96),
            bytes: Some(349_184),
            ok,
            result: if ok { "OK".to_string() } else { "NOT OK: 1 of the copies is not what was copied".to_string() },
        }
    }

    #[test]
    fn boxes_phases_and_checkpoints_are_found() {
        let (phases, items) = parse(DOC);
        assert_eq!(phases.iter().map(|p| p.number.as_str()).collect::<Vec<_>>(), vec!["", "−1", "2", "3", "4", "8"]);
        let keys: Vec<&str> = items.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys[..3], ["one", "two", "one #2"], "a repeated line gets its own key");
        assert_eq!(items.len(), 11, "{keys:?}");
        let cps: Vec<&Item> = items.iter().filter(|i| i.checkpoint).collect();
        assert_eq!(cps.len(), 3);
        assert_eq!(cps[1].covers, vec![3], "the phase 4 checkpoint covers phase 3 too");
        assert!(cps[2].both_copies && !cps[0].both_copies);
    }

    #[test]
    fn the_real_checklist_parses() {
        let (phases, items) = parse(TEXT);
        assert_eq!(phases.iter().map(|p| p.number.as_str()).collect::<Vec<_>>(), vec!["", "1", "2", "3", "4", "5"]);
        // Short: what the program does by itself is not a box.
        assert!(items.len() >= 20 && items.len() <= 40, "{}", items.len());
        assert_eq!(items.iter().filter(|i| i.checkpoint).count(), 3);
        assert_eq!(items.iter().filter(|i| i.both_copies).count(), 1);
        // Every checkpoint has boxes before it in its phase, ticked before its check runs.
        for (c, it) in items.iter().enumerate().filter(|(_, i)| i.checkpoint) {
            assert!(covered(&items, c).count() >= 1, "{}", it.key);
        }
        // Every box in the HTML, once.
        let h = html(TEXT, &items);
        for i in 0..items.len() {
            assert_eq!(h.matches(&format!("data-i=\"{i}\"")).count(), 1, "box {i}");
        }
        assert!(h.contains("data-num=\"files\""));
        assert_eq!(h.matches("<section").count(), h.matches("</section>").count());
        assert_eq!(h.matches("<li").count(), h.matches("</li>").count());
        assert_eq!(h.matches("<ul").count() + h.matches("<ol").count(), h.matches("</ul>").count() + h.matches("</ol>").count());
        assert!(!h.contains("**"), "every bold pair is turned into a tag");
        // A tick on a reworded line follows it: the new wording is a box here.
        for (_, new) in RENAMED {
            assert!(items.iter().any(|i| i.key == *new), "not a box in the checklist: {new}");
        }
    }

    #[test]
    fn inline_markdown() {
        assert_eq!(inline("a **b** *c* `d*e` x"), "a <b>b</b> <i>c</i> <code>d*e</code> x");
        assert_eq!(inline("*It says **this**, or `E:` too?* **Stop.**"), "<i>It says <b>this</b>, or <code>E:</code> too?</i> <b>Stop.</b>");
        assert_eq!(inline("<b> & \"q\""), "&lt;b&gt; &amp; &quot;q&quot;");
        assert_eq!(inline("one * alone"), "one * alone");
        assert!(inline("**Step 5 → Find**").starts_with("<b><a href=\"#\" class=\"steplink\" data-step=\"5\">Step 5</a> →"));
        assert_eq!(inline("as in step 5."), "as in step 5.", "only \"step N →\" is a link");
    }

    #[test]
    fn html_blocks() {
        let (_, items) = parse(DOC);
        let h = html(DOC, &items);
        assert!(h.contains("<ul class=\"boxes\"><li class=\"box\" id=\"it-0\"><input type=\"checkbox\" class=\"tick\" data-i=\"0\""), "{h}");
        assert!(h.contains("<ul><li>a sub-point</li></ul></li>"), "a sub-point sits inside its box: {h}");
        assert!(h.contains("<pre class=\"code\"><code>- [ ] not a box</code></pre>"));
        assert!(h.contains("<blockquote><p><b>Your numbers</b></p>"));
        assert!(h.contains("<td class=\"num\" data-num=\"files\"></td>"));
        assert!(h.contains("<section class=\"ph\" id=\"ph-2\"><h2 class=\"phase-title\">PHASE 2 — Copy <span class=\"phcount\" data-ph=\"2\">"));
        assert!(h.contains("<div class=\"cp\" data-cp=\"5\"></div>"));
    }

    #[test]
    fn ticks_are_kept_and_follow_renamed_lines() {
        let dir = std::env::temp_dir().join(format!("rp-checklist-ticks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(TICKS_NAME);
        let mut t = Ticks::load(&path);
        t.set(&path, "one", true, "2026-10-20 10:00:00").unwrap();
        t.set(&path, "two", true, "2026-10-20 10:01:00").unwrap();
        t.set(&path, "two", false, "2026-10-20 10:02:00").unwrap();
        let back = Ticks::load(&path);
        assert!(back.on("one") && !back.on("two"));
        for (old, new) in RENAMED {
            t.set(&path, old, true, "2026-10-20 10:03:00").unwrap();
            assert!(Ticks::load(&path).on(new));
        }
        assert_eq!(back.when("two"), "2026-10-20 10:02:00");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn checkpoints_follow_the_checks() {
        let (_, items) = parse(DOC);
        let cp = items.iter().position(|i| i.key.starts_with("**3. ✅")).unwrap();
        let key = |k: &str| items.iter().find(|i| i.key.starts_with(k)).unwrap().key.clone();
        let mut t = Ticks::default();
        let state = |t: &Ticks, h: &[Checked]| statuses(&items, t, h).into_iter().find(|s| s.item == cp).unwrap().state;
        assert_eq!(state(&t, &[]), State::Waiting);
        t.map.insert(key("**1."), Tick { on: true, when: "2026-10-20 10:00:00".into() });
        assert_eq!(state(&t, &[]), State::After);
        t.map.insert(key("**2."), Tick { on: true, when: "2026-10-20 11:00:00".into() });
        // A check from before the last box does not count.
        let early = check("2026-10-20 10:30:00", "E:\\WORK", true);
        assert_eq!(state(&t, &[early.clone()]), State::Due);
        let good = check("2026-10-20 12:00:00", "E:\\WORK", true);
        assert_eq!(state(&t, &[early.clone(), good.clone()]), State::Passed);
        let bad = check("2026-10-20 13:00:00", "E:\\WORK", false);
        let s = statuses(&items, &t, &[good.clone(), bad.clone()]);
        assert_eq!(s.iter().find(|s| s.item == cp).unwrap().state, State::Failed);
        assert!(s.iter().find(|s| s.item == cp).unwrap().text.contains("1 of the copies is not what was copied"));
        // Shown as ticked only when passed; not by hand while due.
        let done = ticked(&items, &t, &statuses(&items, &t, &[good.clone()]));
        assert!(done[cp]);
        assert!(may_set(&items, &t, cp, true).is_err());
        assert!(may_set(&items, &t, cp, false).is_ok());
        // The phase 4 checkpoint covers phase 3: a box there makes it wait for a check.
        let cp4 = items.iter().position(|i| i.covers == vec![3]).unwrap();
        t.map.insert(key("scan"), Tick { on: true, when: "2026-10-20 14:00:00".into() });
        assert_eq!(statuses(&items, &t, &[good.clone()]).iter().find(|s| s.item == cp4).unwrap().state, State::After);
        // Skipped by hand when nothing was done in it.
        let cp8 = items.iter().position(|i| i.both_copies).unwrap();
        assert!(may_set(&items, &t, cp8, true).is_ok());
    }

    #[test]
    fn checkpoints_hold_when_the_clocks_go_back() {
        let (_, items) = parse(DOC);
        let cp = items.iter().position(|i| i.key.starts_with("**3. ✅")).unwrap();
        let key = |k: &str| items.iter().find(|i| i.key.starts_with(k)).unwrap().key.clone();
        let state = |t: &Ticks, h: &[Checked]| statuses(&items, t, h).into_iter().find(|s| s.item == cp).unwrap();
        let mut t = Ticks::default();
        t.map.insert(key("**1."), Tick { on: true, when: "2026-10-25 00:30:00 +01:00".into() });
        // The last box ticked at 01:10 after the clocks went back (GMT)...
        t.map.insert(key("**2."), Tick { on: true, when: "2026-10-25 01:10:00 +00:00".into() });
        // ...and a check at 01:50 before they did (BST): 20 minutes before
        // that box was ticked, so it says nothing about it.
        let before = check("2026-10-25 01:50:00 +01:00", "E:\\WORK", true);
        assert_eq!(state(&t, std::slice::from_ref(&before)).state, State::Due);
        // A check at 01:20 GMT, after it, counts - and is shown in local time.
        let after = check("2026-10-25 01:20:00 +00:00", "E:\\WORK", true);
        let s = state(&t, &[before, after]);
        assert_eq!(s.state, State::Passed);
        assert!(s.text.contains("Checked 2026-10-25 01:20:00:"), "{}", s.text);
    }

    #[test]
    fn the_final_check_needs_both_copies() {
        let (_, items) = parse(DOC);
        let cp8 = items.iter().position(|i| i.both_copies).unwrap();
        let mut t = Ticks::default();
        t.map.insert("second copy".into(), Tick { on: true, when: "2026-11-01 09:00:00".into() });
        let st = |h: &[Checked]| statuses(&items, &t, h).into_iter().find(|s| s.item == cp8).unwrap().state;
        let one = check("2026-11-01 10:00:00", "E:\\WORK", true);
        let same = check("2026-11-01 11:00:00", "e:\\work\\", true);
        let other = check("2026-11-01 12:00:00", "D:\\COPY 2\\WORK", true);
        assert_eq!(st(&[one.clone()]), State::Partly);
        assert_eq!(st(&[one.clone(), same.clone()]), State::Partly, "the same folder twice is one copy");
        assert_eq!(st(&[one.clone(), other.clone()]), State::Passed);
        let bad = check("2026-11-01 13:00:00", "E:\\WORK", false);
        assert_eq!(st(&[one, other, bad]), State::Failed);
    }

    #[test]
    fn next_is_after_the_last_ticked() {
        assert_eq!(next(&[false, false]), Some(0));
        assert_eq!(next(&[true, false, true, false]), Some(3), "a box skipped earlier does not hold it back");
        assert_eq!(next(&[true, false, true, true]), Some(1));
        assert_eq!(next(&[true, true]), None);
    }

    #[test]
    fn the_copy_round_trip() {
        let (_, items) = parse(DOC);
        let mut done = vec![false; items.len()];
        done[0] = true;
        done[2] = true;
        let md = markdown(DOC, &items, &done, Some((96, 349_184)));
        assert!(md.contains("- [x] one\n- [ ] two\n  - a sub-point\n- [x] one\n"), "{md}");
        assert!(md.contains("> | 96 | 341 KB |"), "{md}");
        assert!(md.contains("```\n- [ ] not a box\n```"), "a box in a code block is not touched");
        let back = ticks_in(&md);
        assert_eq!(back.get("one"), Some(&true));
        assert_eq!(back.get("one #2"), Some(&true));
        assert_eq!(back.get("two"), Some(&false));
        // Ticks, numbers and line endings aside, it is the same text.
        assert_eq!(normal(&md), normal(DOC));
        assert_eq!(normal(&md.replace('\n', "\r\n")), normal(DOC));
        assert_ne!(normal(&md.replace("copy\n", "copy - my note: E:\n")), normal(DOC));
    }

    #[test]
    fn copies_we_wrote_are_known_and_edited_ones_are_left_alone() {
        let (_, items) = parse(TEXT);
        let md = markdown(TEXT, &items, &vec![true; items.len()], Some((1, 2)));
        assert!(is_unedited(&md, None));
        let mine = md.replacen("**Run the self-test.**", "**Run the self-test.** (mine passed on Monday)", 1);
        assert!(!is_unedited(&mine, None), "a note of the person's own");
        assert!(is_unedited(&mine, Some(written_hash(&mine))), "unless the program wrote exactly that");
        assert!(manifest_unedited(MANIFEST));
        assert!(!manifest_unedited(&format!("{MANIFEST}\nmy note\n")));
    }

    #[test]
    fn a_first_copy_goes_beside_recovery_probes_own() {
        let home = std::env::temp_dir().join(format!("arp-checklist-first-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("Obsidian MD files")).unwrap();
        assert_eq!(first_copy_place(&home), home.join(COPY_NAME));
        std::fs::write(home.join("Obsidian MD files").join("recovery-checklist.md"), "x").unwrap();
        assert_eq!(first_copy_place(&home), home.join("Obsidian MD files").join(COPY_NAME));
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn the_copy_is_found_in_a_folder_inside() {
        let home = std::env::temp_dir().join(format!("rp-checklist-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("Obsidian MD files")).unwrap();
        std::fs::create_dir_all(home.join("results")).unwrap();
        assert_eq!(find_copy(&home), None);
        std::fs::write(home.join("results").join(COPY_NAME), "x").unwrap();
        assert_eq!(find_copy(&home), None, "not in results");
        std::fs::write(home.join("Obsidian MD files").join(COPY_NAME), "x").unwrap();
        assert_eq!(find_copy(&home), Some(home.join("Obsidian MD files").join(COPY_NAME)));
        write_whole(&home.join("Obsidian MD files").join(COPY_NAME), "new").unwrap();
        assert_eq!(std::fs::read_to_string(home.join("Obsidian MD files").join(COPY_NAME)).unwrap(), "new");
        assert!(!home.join("Obsidian MD files").join("recovery-checklist.md.new").exists());
        std::fs::remove_dir_all(&home).ok();
    }
}
