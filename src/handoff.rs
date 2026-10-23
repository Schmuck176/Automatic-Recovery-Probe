//! What is left after the automatic repairs, made ready for whoever can do
//! better - the Mac, a paid repair tool, the file's own program - and
//! bringing back what comes back.
//!
//! The hand-off goes into REPAIR\hand-off, as copies (the files in
//! REPAIR\to-repair stay as they are):
//!
//! - "1 For the Mac": iPhone photos, camera RAW, Pages/Numbers/Keynote, with
//!   "Repair on the Mac.command": double-clicked on the Mac, it tries each
//!   file with the Mac's own decoders (sips, avconvert), never changes them,
//!   and saves what it repairs into "returned" beside itself, numbers kept.
//! - "2 For a paid repair tool": photos, videos, PDFs and the rest that no
//!   open-source tool here could repair. Look at the tool's free preview
//!   first: a paid tool only does better than what was tried here if its
//!   preview shows it.
//! - "3 For their own program": Office documents (Open and Repair).
//!
//! Files that open or play as they are, with a glitch where data was lost,
//! are not handed off: nothing can bring the lost part back.
//!
//! Bring back takes in every file with a number ([R0012]) from a folder (the
//! USB stick, the paid tool's output, REPAIR\hand-off\returned): the same
//! kind of file as its original goes into REPAIR\repaired\returned (an
//! earlier version for that number moves to REPAIR\replaced, so put back
//! finds exactly one), another kind into REPAIR\salvaged\returned. Then put
//! back, the checks and evaluate run as after Start.

use crate::autorepair::{self, Fate, Next};
use crate::fmt::commas;
use crate::repair;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const HANDOFF_DIR: &str = "hand-off";
pub const RETURNED: &str = "returned";
pub const FOR_MAC: &str = "1 For the Mac";
pub const FOR_PAID: &str = "2 For a paid repair tool";
pub const FOR_OWN: &str = "3 For their own program";
pub const MAC_SCRIPT: &str = "Repair on the Mac.command";
const README: &str = "READ ME.txt";

/// A camera RAW, by its extension (without the dot).
fn is_raw(ext: &str) -> bool {
    crate::formats::is_raw_ext(&format!(".{}", ext.to_lowercase()))
}

fn ext_lower(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => e.to_lowercase(),
        _ => String::new(),
    }
}

fn under(base: &Path, rel: &str) -> PathBuf {
    let mut p = crate::rescue::verbatim(base);
    for c in rel.split('\\').filter(|c| !c.is_empty()) {
        p.push(c);
    }
    p
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<u64> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        let mut n = 0;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            n += copy_tree(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(n)
    } else {
        if let Some(p) = to.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::copy(from, to)
    }
}

/// Where a file that was not repaired goes, when there is no attempt to say.
fn next_by_kind(name: &str) -> Next {
    let e = ext_lower(name);
    let e = e.as_str();
    if is_raw(e) || ["heic", "heif", "hif", "avif", "pages", "numbers", "key", "rtfd", "webarchive"].contains(&e) {
        Next::Mac
    } else if ["doc", "docx", "docm", "xls", "xlsx", "xlsm", "ppt", "pptx", "pptm", "odt", "ods", "odp", "rtf"].contains(&e) {
        Next::OwnApp
    } else {
        Next::Paid
    }
}

#[derive(Debug, Default)]
pub struct HandedOff {
    pub folder: PathBuf,
    pub mac: Vec<String>,
    pub paid: Vec<String>,
    pub own: Vec<String>,
    /// Open or play as they are; nothing to do.
    pub nothing: Vec<(String, String)>,
}

/// Copy everything still damaged into REPAIR\hand-off, sorted by who can
/// do better, with the Mac script and the READ MEs. Run again, it adds what
/// is new and takes out what has been repaired since.
pub fn prepare(work: &Path) -> Result<HandedOff, String> {
    let repair = repair::repair_dir(work)?;
    let items = repair::read_list(&repair)?;
    let attempts: HashMap<String, autorepair::Attempt> = autorepair::read_log(&repair).into_iter().map(|a| (a.id.clone(), a)).collect();
    let back = repair::returned_now(work);
    let root = repair.join(HANDOFF_DIR);
    let mut o = HandedOff { folder: root.clone(), ..Default::default() };
    let mut keep: HashSet<PathBuf> = HashSet::new();
    for item in &items {
        if back.get(&item.path.to_lowercase()).is_some_and(|n| n.repaired) {
            continue;
        }
        let name = item.copied_as.rsplit('\\').next().unwrap_or(&item.copied_as).to_string();
        let a = attempts.get(&item.id);
        if a.is_some_and(|a| a.fate == Fate::Repaired) {
            continue;
        }
        let next = a.map(|a| a.next).unwrap_or_else(|| next_by_kind(&name));
        let folder = match next {
            Next::Mac => FOR_MAC,
            Next::Paid => FOR_PAID,
            Next::OwnApp => FOR_OWN,
            Next::Nothing => {
                o.nothing.push((item.path.clone(), a.map(|a| a.how.clone()).unwrap_or_default()));
                continue;
            }
            Next::Done => continue,
        };
        // In the paid folder, sorted the way to-repair is (what repairs it).
        let group = item.group.clone();
        let rel = if folder == FOR_PAID { format!("{folder}\\{group}\\{name}") } else { format!("{folder}\\{name}") };
        let dest = under(&root, &rel);
        let src = under(&repair, &item.copied_as);
        if !src.exists() {
            continue;
        }
        if !dest.exists() {
            copy_tree(&src, &dest).map_err(|e| format!("could not copy {} into the hand-off: {e}", item.copied_as))?;
        }
        keep.insert(dest);
        match next {
            Next::Mac => o.mac.push(item.path.clone()),
            Next::OwnApp => o.own.push(item.path.clone()),
            _ => o.paid.push(item.path.clone()),
        }
    }
    // What was repaired since an earlier hand-off comes out of it.
    for f in [FOR_MAC, FOR_PAID, FOR_OWN] {
        remove_unkept(&root.join(f), &keep);
    }
    std::fs::create_dir_all(root.join(RETURNED)).map_err(|e| format!("could not make {}: {e}", root.join(RETURNED).display()))?;
    write_readmes(&root, &o)?;
    Ok(o)
}

fn remove_unkept(dir: &Path, keep: &HashSet<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name == README || name == MAC_SCRIPT || name == RETURNED || name.starts_with("._") {
            continue;
        }
        if repair::tag_in(&name).is_some() {
            if !keep.contains(&p) {
                if p.is_dir() {
                    let _ = std::fs::remove_dir_all(&p);
                } else {
                    let _ = std::fs::remove_file(&p);
                }
            }
        } else if p.is_dir() {
            remove_unkept(&p, keep);
            let _ = std::fs::remove_dir(&p); // only if empty
        }
    }
}

const MAC_README: &str = "These files are copies of damaged files the PC could not repair. The Mac's own decoders know \
iPhone photos (HEIC) and camera RAW better than anything on Windows.

1. Copy this whole folder (\"1 For the Mac\") onto a USB stick formatted exFAT, so both computers can write to it.
   (Never plug the recovered SSD or the 4 TB SSD into the Mac.)
2. On the Mac, open the folder on the stick and double-click \"Repair on the Mac.command\".
   If the Mac says it can't be opened because it is from an unidentified developer: right-click it, choose Open,
   then Open. If it asks whether Terminal may use files on a removable volume, click Allow.
3. It tries every file here, never changes them, and saves what it can repair into the folder \"returned\" beside
   it: \"returned/repaired\" (the same kind of file: put back into WORK) and \"returned/salvaged\" (a TIFF made from
   a RAW, say: kept, the original stays). It keeps each file's number ([R0012]). It writes what it did into
   \"returned/mac-repair-log.txt\".
4. Documents kept as folders (Pages, Numbers, Keynote) can't be repaired by a script: open each in its own app,
   and if it opens, File > Save a copy into \"returned/repaired\" with the same name and number.
5. Bring the stick back to the PC, and in Automatic Recovery Probe click \"Bring back\" and choose the stick.
";

const PAID_README: &str = "These files are copies of damaged files no open-source tool could repair, sorted into folders \
by what kind of tool can try (photo, video, PDF...). Nothing here is needed: decide for each whether it is worth paying.

1. A paid repair tool (Stellar, Repairit...) shows a free preview of what it can do before you pay. Look at the preview
   first: if it does not show the file repaired, paying will not repair it either.
2. Save every repaired file into the folder \"returned\" (in REPAIR\\hand-off), in the same format as the original,
   keeping its number in its name: \"IMG_0003 [R0012].JPG\" or \"IMG_0003 [R0012]_repaired.JPG\" are both fine.
   If a tool drops the number, add it back by hand.
3. In Automatic Recovery Probe, click \"Bring back\": it puts every repaired file back exactly where its original is in
   WORK, keeping the original, and checks everything again.

Never point a repair tool at WORK itself: only at these copies.
";

const OWN_README: &str = "These are Office documents (Word, Excel, PowerPoint) that could not be rebuilt automatically. \
Their own program has a repair built in:

1. In Word, Excel or PowerPoint: File > Open, choose the file here, then click the arrow beside Open > Open and Repair.
2. If it opens, save it under the same name and number into the folder \"returned\" (in REPAIR\\hand-off).
3. In Automatic Recovery Probe, click \"Bring back\".
";

fn write_readmes(root: &Path, o: &HandedOff) -> Result<(), String> {
    let w = |p: PathBuf, t: &str| -> Result<(), String> {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        std::fs::write(&p, t.replace('\n', "\r\n")).map_err(|e| format!("could not write {}: {e}", p.display()))
    };
    if !o.mac.is_empty() {
        w(root.join(FOR_MAC).join(README), MAC_README)?;
        let p = root.join(FOR_MAC).join(MAC_SCRIPT);
        std::fs::write(&p, MAC_SCRIPT_TEXT).map_err(|e| format!("could not write {}: {e}", p.display()))?;
    }
    if !o.paid.is_empty() {
        w(root.join(FOR_PAID).join(README), PAID_README)?;
    }
    if !o.own.is_empty() {
        w(root.join(FOR_OWN).join(README), OWN_README)?;
    }
    let mut t = String::from(
        "What was left after the automatic repairs, and where it went.\n\n\
         Repaired files come back into the folder \"returned\" (or anywhere: \"Bring back\" asks where), keeping their \
         number ([R0012]).\n\n",
    );
    t.push_str(&format!("{FOR_MAC}: {} files\n{FOR_PAID}: {} files\n{FOR_OWN}: {} files\n", o.mac.len(), o.paid.len(), o.own.len()));
    if !o.nothing.is_empty() {
        t.push_str(&format!(
            "\nNot handed off ({}): they open or play as they are, with a glitch where data was lost. Nothing can bring \
             that part back, so there is nothing to do:\n",
            o.nothing.len()
        ));
        for (p, why) in &o.nothing {
            t.push_str(&format!("  {p}: {why}\n"));
        }
    }
    w(root.join(README), &t)
}

/// The Mac's part, as a shell script (zsh, which every Mac since 2019 has;
/// it only uses what macOS comes with).
pub const MAC_SCRIPT_TEXT: &str = r#"#!/bin/zsh
# Automatic Recovery Probe - repairs on the Mac.
# Tries every file in this folder with the Mac's own decoders. It never changes
# them: what it repairs goes into "returned" beside this script, each file's
# [R....] number kept, ready for "Bring back" on the PC.
setopt null_glob extended_glob
here="${0:A:h}"
cd "$here" || exit 1
out="$here/returned"
mkdir -p "$out/repaired" "$out/salvaged"
log="$out/mac-repair-log.txt"
print "Repairs on the Mac, $(date)" >> "$log"
caffeinate -i -w $$ &

decodes() {  # $1 file: does sips decode it?
  local t; t="$(mktemp -t arp).png"
  if sips -s format png "$1" --out "$t" >/dev/null 2>"$t.err" && [[ -s "$t" ]] && ! grep -qi error "$t.err"; then
    rm -f "$t" "$t.err"; return 0
  fi
  rm -f "$t" "$t.err"; return 1
}

repaired=0; salvaged=0; failed=0; byhand=0
for f in **/*(.D); do
  name="${f:t}"
  case "$name" in
    ._*|.DS_Store|"READ ME.txt"|*.command) continue ;;
  esac
  [[ "$f" == returned/* ]] && continue
  ext="${name:e:l}"; stem="${name:r}"
  print -n "$name ... "
  ok=""
  case "$ext" in
    heic|heif|hif|avif)
      if decodes "$f" && sips -s format heic "$f" --out "$out/repaired/$name" >/dev/null 2>&1 && [[ -s "$out/repaired/$name" ]] && ! cmp -s "$f" "$out/repaired/$name"; then
        ok="repaired: saved again by the Mac's own decoder"
        repaired=$((repaired+1))
      elif sips -s format jpeg "$f" --out "$out/salvaged/$stem (as JPEG).jpg" >/dev/null 2>&1 && [[ -s "$out/salvaged/$stem (as JPEG).jpg" ]]; then
        rm -f "$out/repaired/$name"
        ok="salvaged: what decodes saved as a JPEG"
        salvaged=$((salvaged+1))
      else
        rm -f "$out/repaired/$name" "$out/salvaged/$stem (as JPEG).jpg"
      fi ;;
    dng|cr2|cr3|crw|nef|nrw|arw|srf|sr2|raf|orf|rw2|raw|pef|ptx|srw|3fr|erf|mef|mos|mrw|x3f|iiq|k25|kdc|dcr|rwl|fff)
      if sips -s format tiff "$f" --out "$out/salvaged/$stem (as TIFF).tif" >/dev/null 2>&1 && [[ -s "$out/salvaged/$stem (as TIFF).tif" ]]; then
        ok="salvaged: what the Mac decodes saved as a TIFF (a RAW can't be saved as RAW again)"
        salvaged=$((salvaged+1))
      else
        rm -f "$out/salvaged/$stem (as TIFF).tif"
      fi ;;
    jpg|jpeg|jpe|png|tif|tiff|gif|bmp)
      fmt="$ext"; [[ "$ext" == jpg || "$ext" == jpe ]] && fmt=jpeg; [[ "$ext" == tif ]] && fmt=tiff
      if decodes "$f" && sips -s format "$fmt" "$f" --out "$out/repaired/$name" >/dev/null 2>&1 && [[ -s "$out/repaired/$name" ]] && ! cmp -s "$f" "$out/repaired/$name"; then
        ok="repaired: saved again by the Mac's own decoder"
        repaired=$((repaired+1))
      else
        rm -f "$out/repaired/$name"
      fi ;;
    mov|mp4|m4v|m4a)
      if command -v avconvert >/dev/null && avconvert --source "$f" --output "$out/repaired/$name" --preset PresetPassthrough >/dev/null 2>&1 && [[ -s "$out/repaired/$name" ]]; then
        ok="repaired: copied into a new file by the Mac's own video framework"
        repaired=$((repaired+1))
      else
        rm -f "$out/repaired/$name"
      fi ;;
    *)
      ok="by hand: open it in its own app, and if it opens save a copy into returned/repaired with the same name"
      byhand=$((byhand+1)) ;;
  esac
  if [[ -z "$ok" ]]; then ok="not repaired: the Mac could not decode it either"; failed=$((failed+1)); fi
  print "$ok"
  print "$name: $ok" >> "$log"
done
for d in **/*.(pages|numbers|key|rtfd)(/N); do
  print "${d:t}: by hand - open it in its own app; if it opens, File > Save a copy into returned/repaired with the same name"
  print "${d:t}: by hand" >> "$log"
  byhand=$((byhand+1))
done
print ""
print "Done: $repaired repaired, $salvaged salvaged, $failed not repaired, $byhand to do by hand."
print "Everything is in the folder \"returned\". Take this USB stick back to the PC and click \"Bring back\"."
print "Done: $repaired repaired, $salvaged salvaged, $failed not repaired, $byhand by hand" >> "$log"
"#;

// ---------------------------------------------------------------- bring back

#[derive(Debug, Default)]
pub struct Brought {
    /// (id, from, to in REPAIR)
    pub repaired: Vec<(String, String, String)>,
    pub salvaged: Vec<(String, String, String)>,
    /// Exactly the damaged original, or the same as what is there already.
    pub unchanged: usize,
    /// Files with no number, or a number not in repair-list.csv.
    pub unknown: Vec<String>,
    /// Earlier versions moved to REPAIR\replaced to make way.
    pub superseded: usize,
}

fn is_mac_housekeeping(name: &str) -> bool {
    name.starts_with("._") || name.eq_ignore_ascii_case(".ds_store") || crate::formats::is_system_file(&name.to_lowercase())
}

fn drive_housekeeping(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        ".spotlight-v100" | ".fseventsd" | ".trashes" | ".temporaryitems" | "$recycle.bin" | "system volume information"
    )
}

/// Take in every repaired file (by its number) from `from`.
pub fn bring_back(work: &Path, from: &Path, say: &mut dyn FnMut(&str)) -> Result<Brought, String> {
    let repair = repair::repair_dir(work)?;
    let items = repair::read_list(&repair)?;
    let by_id: HashMap<&str, &repair::Item> = items.iter().map(|i| (i.id.as_str(), i)).collect();
    if !from.is_dir() {
        return Err(format!("{} is not a folder", from.display()));
    }
    let mut o = Brought::default();
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    let mut stack = vec![from.to_path_buf()];
    let repair_low = repair.display().to_string().to_lowercase();
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let p = e.path();
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            if is_dir {
                // Not REPAIR's own folders (when "from" is REPAIR or above it).
                let low = p.display().to_string().to_lowercase();
                if drive_housekeeping(&name) || (low.starts_with(&repair_low) && !low.starts_with(&format!("{repair_low}\\{HANDOFF_DIR}").to_lowercase()) && !low.starts_with(&format!("{repair_low}/{HANDOFF_DIR}").to_lowercase())) {
                    continue;
                }
                if repair::tag_in(&name).is_some() && repair::package_of_name(&name).is_some() {
                    files.push((p, name)); // a document kept as a folder
                } else {
                    stack.push(p);
                }
            } else if !is_mac_housekeeping(&name) && name != README && name != MAC_SCRIPT && !name.ends_with("-log.txt") {
                files.push((p, name));
            }
        }
    }
    files.sort();
    for (p, name) in files {
        let Some(id) = repair::tag_in(&name) else {
            o.unknown.push(p.display().to_string());
            continue;
        };
        let Some(item) = by_id.get(id.as_str()) else {
            o.unknown.push(p.display().to_string());
            continue;
        };
        let sha = if p.is_dir() {
            repair::hash_of(&p, &|| false).ok().flatten().map(|h| h.1)
        } else {
            crate::sha256::file_hex(&p, &|| false).ok().flatten()
        };
        let Some(sha) = sha else { continue };
        if sha == item.sha256 {
            o.unchanged += 1; // the damaged original itself (the hand-off's own copy)
            continue;
        }
        let orig_name = item.path.rsplit('\\').next().unwrap_or(&item.path);
        let same_kind = ext_lower(orig_name) == ext_lower(&name) && (item.package == p.is_dir());
        let dest_rel = if same_kind {
            format!("{}\\{RETURNED}\\{name}", repair::REPAIRED)
        } else {
            format!("{}\\{RETURNED}\\{name}", repair::SALVAGED)
        };
        let dest = under(&repair, &dest_rel);
        // Already brought back (the same file)?
        if dest.exists() {
            let there = if dest.is_dir() {
                repair::hash_of(&dest, &|| false).ok().flatten().map(|h| h.1)
            } else {
                crate::sha256::file_hex(&dest, &|| false).ok().flatten()
            };
            if there.as_deref() == Some(sha.as_str()) {
                o.unchanged += 1;
                continue;
            }
        }
        if same_kind {
            // Exactly one version per number in "repaired": earlier ones
            // (the automatic repair's, an earlier bring-back's) make way.
            o.superseded += supersede(&repair, &id, &dest)?;
        }
        if dest.exists() {
            let _ = if dest.is_dir() { std::fs::remove_dir_all(&dest) } else { std::fs::remove_file(&dest) };
        }
        copy_tree(&p, &dest).map_err(|e| format!("could not copy {} into REPAIR: {e}", p.display()))?;
        say(&format!("{id}  {}  <- {}", if same_kind { "repaired" } else { "salvaged" }, p.display()));
        let row = (id.clone(), p.display().to_string(), dest_rel);
        if same_kind {
            o.repaired.push(row);
        } else {
            o.salvaged.push(row);
        }
    }
    Ok(o)
}

/// Move every version of `id` in REPAIR\repaired (but `keep`) to REPAIR\replaced.
fn supersede(repair: &Path, id: &str, keep: &Path) -> Result<usize, String> {
    let mut moved = 0;
    let mut stack = vec![repair.join(repair::REPAIRED)];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("._") {
                continue;
            }
            let tagged = repair::tag_in(&name);
            if tagged.as_deref() == Some(id) && p != keep {
                let to = repair.join(repair::REPLACED).join(format!("{id} {} {name}", crate::fmt::now_stamp()));
                let _ = std::fs::create_dir_all(repair.join(repair::REPLACED));
                std::fs::rename(&p, &to).map_err(|e| format!("could not move {} aside: {e}", p.display()))?;
                moved += 1;
            } else if tagged.is_none() && e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(p);
            }
        }
    }
    Ok(moved)
}

pub fn summary(o: &HandedOff) -> Vec<String> {
    let mut t = Vec::new();
    let n = o.mac.len() + o.paid.len() + o.own.len();
    if n == 0 {
        t.push("Nothing is left to hand off.".to_string());
    } else {
        t.push(format!("{} files are ready in {} for whoever can do better:", commas(n as u64), o.folder.display()));
        if !o.mac.is_empty() {
            t.push(format!("  {FOR_MAC}: {} (with \"{MAC_SCRIPT}\" to double-click on the Mac)", commas(o.mac.len() as u64)));
        }
        if !o.paid.is_empty() {
            t.push(format!("  {FOR_PAID}: {} (look at a tool's free preview before paying)", commas(o.paid.len() as u64)));
        }
        if !o.own.is_empty() {
            t.push(format!("  {FOR_OWN}: {} (Word, Excel, PowerPoint: Open and Repair)", commas(o.own.len() as u64)));
        }
    }
    if !o.nothing.is_empty() {
        t.push(format!(
            "{} more open or play as they are, with a glitch where data was lost: nothing can bring that part back, so they \
             are not handed off.",
            commas(o.nothing.len() as u64)
        ));
    }
    t
}

pub fn brought_summary(o: &Brought) -> Vec<String> {
    let mut t = Vec::new();
    if o.repaired.is_empty() && o.salvaged.is_empty() {
        t.push("Nothing new to bring back.".to_string());
    } else {
        t.push(format!(
            "Brought back {} repaired files (they go back into WORK next) and {} salvaged ones (kept in REPAIR\\salvaged).",
            commas(o.repaired.len() as u64),
            commas(o.salvaged.len() as u64)
        ));
    }
    if o.superseded > 0 {
        t.push(format!("{} earlier versions made way for them (kept in REPAIR\\replaced).", commas(o.superseded as u64)));
    }
    if o.unchanged > 0 {
        t.push(format!("{} files were the damaged originals or already brought back, so they were left.", commas(o.unchanged as u64)));
    }
    if !o.unknown.is_empty() {
        t.push(format!(
            "{} files have no number ([R0012]) the program knows, so where they belong is not known, for example {}",
            commas(o.unknown.len() as u64),
            o.unknown.first().cloned().unwrap_or_default()
        ));
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn where_unrepaired_files_go() {
        assert_eq!(next_by_kind("a [R0001].HEIC"), Next::Mac);
        assert_eq!(next_by_kind("a [R0001].cr2"), Next::Mac);
        assert_eq!(next_by_kind("a [R0001].docx"), Next::OwnApp);
        assert_eq!(next_by_kind("a [R0001].jpg"), Next::Paid);
    }

    #[test]
    fn the_mac_script_only_uses_what_macos_has() {
        for tool in ["sips", "avconvert", "caffeinate", "cmp", "mktemp", "grep"] {
            assert!(MAC_SCRIPT_TEXT.contains(tool));
        }
        assert!(MAC_SCRIPT_TEXT.starts_with("#!/bin/zsh\n"));
        assert!(!MAC_SCRIPT_TEXT.contains('\r'), "a Mac script must have Unix line ends");
        // It never writes over the files it tries.
        assert!(!MAC_SCRIPT_TEXT.contains("--out \"$f\""));
    }

    #[test]
    fn bring_back_takes_repaired_files_by_number() {
        let base = std::env::temp_dir().join(format!("arp-bring-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let work = base.join("WORK");
        std::fs::create_dir_all(work.join("Pics")).unwrap();
        std::fs::write(work.join("Pics").join("a.jpg"), b"damaged").unwrap();
        std::fs::write(work.join("Pics").join("b.cr2"), b"damaged raw").unwrap();
        let repair = base.join("REPAIR");
        std::fs::create_dir_all(repair.join("repaired").join("auto")).unwrap();
        let sha = |b: &[u8]| {
            let mut h = crate::sha256::Sha256::new();
            h.update(b);
            h.finish_hex()
        };
        let list = format!(
            "\u{FEFF}id,path_in_work,kind,repair_with,size_bytes,sha256,why,inside_package,copied_out_as\r\n\
             R0001,Pics\\a.jpg,file,1 Photos,7,{},HOLE,,to-repair\\1 Photos\\a [R0001].jpg\r\n\
             R0002,Pics\\b.cr2,file,2 RAW,11,{},HOLE,,to-repair\\2 RAW\\b [R0002].cr2\r\n",
            sha(b"damaged"),
            sha(b"damaged raw")
        );
        std::fs::write(repair.join(repair::LIST_NAME), list).unwrap();
        std::fs::write(repair.join("repaired").join("auto").join("a [R0001].jpg"), b"auto version").unwrap();
        let stick = base.join("stick");
        std::fs::create_dir_all(stick.join("returned").join("repaired")).unwrap();
        std::fs::create_dir_all(stick.join("returned").join("salvaged")).unwrap();
        std::fs::write(stick.join("a [R0001].jpg"), b"damaged").unwrap(); // the hand-off's copy of the original
        std::fs::write(stick.join("returned").join("repaired").join("a [R0001].jpg"), b"mac version").unwrap();
        std::fs::write(stick.join("returned").join("repaired").join("._a [R0001].jpg"), b"mac junk").unwrap();
        std::fs::write(stick.join("returned").join("salvaged").join("b [R0002] (as TIFF).tif"), b"tiff").unwrap();
        std::fs::write(stick.join("returned").join("notes.txt"), b"no number").unwrap();
        let o = bring_back(&work, &stick, &mut |_| {}).unwrap();
        assert_eq!(o.repaired.len(), 1, "{o:?}");
        assert_eq!(o.salvaged.len(), 1, "{o:?}");
        assert_eq!(o.unchanged, 1);
        assert_eq!(o.superseded, 1);
        assert_eq!(o.unknown.len(), 1);
        assert_eq!(std::fs::read(repair.join("repaired").join(RETURNED).join("a [R0001].jpg")).unwrap(), b"mac version");
        assert!(!repair.join("repaired").join("auto").join("a [R0001].jpg").exists());
        assert!(repair.join("salvaged").join(RETURNED).join("b [R0002] (as TIFF).tif").exists());
        // Again: nothing new.
        let o = bring_back(&work, &stick, &mut |_| {}).unwrap();
        assert!(o.repaired.is_empty() && o.salvaged.is_empty(), "{o:?}");
        let _ = std::fs::remove_dir_all(&base);
    }
}
