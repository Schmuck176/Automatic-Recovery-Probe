//! The person's own names for the drives they plug in.
//!
//! The program has its own words for the three drives - the recovered SSD,
//! the 4 TB SSD, SPARE - and the checklist asks for a label on each drive
//! itself (rule 5). What the label says is the person's to choose: 1. Get
//! ready has a section, Your drives, that first says what the program
//! expects each drive to be, and then takes a name for each.
//!
//! A name is only what the person calls the drive: nothing on any drive is
//! changed for it. Once the names are given, the page shows each one beside
//! the program's own word for the drive (ui.html does that itself, from the
//! settings), the notifications say it (`beside`), the checklist's copy
//! lists it (checklist.rs), and a drive called that in Windows is known as
//! SPARE just as one called SPARE is (`is_spare_label`), in case the person
//! renames it there too.

use std::collections::HashMap;
use std::sync::Mutex;

/// One of the drives the person plugs in.
pub struct Drive {
    /// Its name in the page's and the actions' fields.
    pub key: &'static str,
    /// The setting its name is kept in.
    pub setting: &'static str,
    /// What the program calls it, in a sentence.
    pub word: &'static str,
    /// The words to look for where the program names it.
    pub find: &'static str,
    /// The name the checklist suggests for its label.
    pub suggest: &'static str,
}

pub const DRIVES: [Drive; 3] = [
    Drive { key: "recovered", setting: "name_recovered", word: "the recovered SSD", find: "recovered SSD", suggest: "RECOVERED 2 TB" },
    Drive { key: "copy", setting: "name_copy", word: "the 4 TB SSD", find: "4 TB SSD", suggest: "4 TB COPY" },
    Drive { key: "spare", setting: "name_spare", word: "SPARE", find: "SPARE", suggest: "SPARE 1 TB" },
];

/// The setting that says when the names were given (empty: not yet).
pub const DECIDED: &str = "names_at";
/// The longest name taken: what fits on a label, and in a line of the page.
pub const LONGEST: usize = 40;

/// A name as it is kept: one line, spaces single, none at the ends.
fn tidy(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The names as they are kept, from the three as typed (in DRIVES' order):
/// one left empty takes the checklist's suggestion. Refused, with why: a
/// name too long or with something in it that is not a letter, digit, sign
/// or space; two drives with the same name; a name that is the program's own
/// word (or suggestion) for another drive - any of those would mix the
/// drives up, which is what the labels are there to prevent.
pub fn check(typed: &[String; 3]) -> Result<[String; 3], String> {
    let mut names: [String; 3] = Default::default();
    for (i, d) in DRIVES.iter().enumerate() {
        let t = tidy(&typed[i]);
        if typed[i].chars().any(|c| (c.is_control() && !c.is_whitespace()) || unseen(c)) {
            return Err(format!("The name for {} has something in it that cannot be shown: type it again with letters, digits and spaces.", d.word));
        }
        if t.chars().count() > LONGEST {
            return Err(format!(
                "The name for {} is {} characters long: keep it to {LONGEST} or fewer, so that it fits on a label.",
                d.word,
                t.chars().count()
            ));
        }
        names[i] = if t.is_empty() { d.suggest.to_string() } else { t };
    }
    let same = |a: &str, b: &str| a.to_lowercase() == b.to_lowercase();
    for (i, d) in DRIVES.iter().enumerate() {
        for o in DRIVES.iter().filter(|o| o.key != d.key) {
            if [o.word, o.find, o.suggest].iter().any(|w| same(&names[i], w)) {
                return Err(format!(
                    "\"{}\" is what the program calls {}: choose another name for {}, or the page would point at the wrong drive.",
                    names[i], o.word, d.word
                ));
            }
        }
    }
    for (i, d) in DRIVES.iter().enumerate() {
        for (j, o) in DRIVES.iter().enumerate().skip(i + 1) {
            if same(&names[i], &names[j]) {
                return Err(format!(
                    "{} and {} have the same name (\"{}\"): give each drive a name of its own, or they are easy to mix up.",
                    capital(d.word),
                    o.word,
                    names[i]
                ));
            }
        }
    }
    Ok(names)
}

/// Characters that take no room on the page, or turn the writing round:
/// in a name they would only hide what it says.
fn unseen(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}')
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The names given, from the settings (in DRIVES' order): None until the
/// person has given them (or taken the suggestions).
pub fn given(settings: &HashMap<String, String>) -> Option<[String; 3]> {
    if settings.get(DECIDED).map(|w| w.trim().is_empty()).unwrap_or(true) {
        return None;
    }
    let mut names: [String; 3] = Default::default();
    for (i, d) in DRIVES.iter().enumerate() {
        let n = settings.get(d.setting).map(|n| tidy(n)).unwrap_or_default();
        names[i] = if n.is_empty() { d.suggest.to_string() } else { n };
    }
    Some(names)
}

/// The names in use now: what the notifications say, and what SPARE may be
/// called in Windows. (Kept here, as whether notifications are on is kept in
/// notify.rs: both are asked for from places that have no settings at hand.)
static NOW: Mutex<Option<[String; 3]>> = Mutex::new(None);

/// The names from the settings, in use from now on (None: none given).
pub fn use_from(settings: &HashMap<String, String>) {
    *NOW.lock().unwrap_or_else(|e| e.into_inner()) = given(settings);
}

/// The names in use now.
pub fn now() -> Option<[String; 3]> {
    NOW.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Is this what SPARE is called in Windows? SPARE itself - or the name the
/// person gave it, should they have renamed it in File Explorer to match its
/// label.
pub fn is_spare_label(label: &str) -> bool {
    is_spare_label_with(label, now().as_ref())
}

fn is_spare_label_with(label: &str, names: Option<&[String; 3]>) -> bool {
    let l = tidy(label).to_lowercase();
    l == "spare" || names.is_some_and(|n| !l.is_empty() && l == n[2].to_lowercase())
}

/// Where `find` is named in `text` as a drive: not as part of a longer word
/// or of a path (SPARE\WORK is a folder), nor - for SPARE - in the suggested
/// label itself (SPARE 1 TB). The place just after it, a possessive ('s)
/// included.
fn named_at(text: &str, find: &str) -> Option<usize> {
    let part_of_more = |c: char| c.is_alphanumeric() || c == '\\' || c == '/' || c == '_';
    let mut from = 0;
    while let Some(k) = text[from..].find(find) {
        let (at, end) = (from + k, from + k + find.len());
        let before_ok = !text[..at].chars().next_back().is_some_and(part_of_more);
        let after = &text[end..];
        let after_ok = !after.chars().next().is_some_and(part_of_more) && !(find == "SPARE" && after.starts_with(" 1 TB"));
        if before_ok && after_ok {
            let s = ["'s", "\u{2019}s"].iter().find(|s| after.starts_with(**s)).map(|s| s.len()).unwrap_or(0);
            return Some(end + s);
        }
        from = end;
    }
    None
}

/// `text` with each drive's name beside the first place the text names that
/// drive: "Plug in the recovered SSD ("Blue one") now." (The page does the
/// same with a small label; this is for words that leave the page - the
/// notifications.)
pub fn beside_with(text: &str, names: &[String; 3]) -> String {
    // (Each place found in the text as it came: a name put in is not looked
    // through for another drive's word.)
    let mut places: Vec<(usize, &String)> = DRIVES
        .iter()
        .zip(names)
        // (A name that is the program's own word says nothing new.)
        .filter(|(d, name)| !name.eq_ignore_ascii_case(d.find) && !name.eq_ignore_ascii_case(d.word))
        .filter_map(|(d, name)| named_at(text, d.find).map(|at| (at, name)))
        .collect();
    places.sort_by_key(|p| std::cmp::Reverse(p.0));
    let mut out = text.to_string();
    for (at, name) in places {
        out.insert_str(at, &format!(" (\u{201c}{name}\u{201d})"));
    }
    out
}

/// The same, with the names in use now (none given: the text as it is).
pub fn beside(text: &str) -> String {
    match now() {
        Some(names) => beside_with(text, &names),
        None => text.to_string(),
    }
}

/// A name as a cell of a Markdown table (the checklist's copy): a | in it
/// would end the cell.
pub fn md_cell(name: &str) -> String {
    name.replace('\\', "\\\\").replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three(a: &str, b: &str, c: &str) -> [String; 3] {
        [a.to_string(), b.to_string(), c.to_string()]
    }

    #[test]
    fn names_are_tidied_and_empty_ones_take_the_suggestion() {
        assert_eq!(check(&three("  Blue   one ", "", "Dad's drive")).unwrap(), three("Blue one", "4 TB COPY", "Dad's drive"));
        assert_eq!(check(&three("", "", "")).unwrap(), three("RECOVERED 2 TB", "4 TB COPY", "SPARE 1 TB"));
        // Anything that can be written on a label: other alphabets, signs.
        assert_eq!(check(&three("Żółty #1 (2 TB)", "<b>&\"|\\", "备用")).unwrap(), three("Żółty #1 (2 TB)", "<b>&\"|\\", "备用"));
        // SPARE may keep its own name, and so may the others.
        assert!(check(&three("recovered ssd", "The 4 TB SSD", "spare")).is_ok());
    }

    #[test]
    fn names_that_would_mix_the_drives_up_are_refused() {
        let e = check(&three("Blue", "blue", "Red")).unwrap_err();
        assert!(e.contains("same name") && e.contains("The recovered SSD and the 4 TB SSD"), "{e}");
        // (One left empty takes its suggestion: that counts too.)
        assert!(check(&three("4 tb copy", "", "Red")).unwrap_err().contains("what the program calls the 4 TB SSD"));
        let e = check(&three("SPARE", "Green", "Red")).unwrap_err();
        assert!(e.contains("what the program calls SPARE") && e.contains("for the recovered SSD"), "{e}");
        assert!(check(&three("Blue", "Green", "the recovered ssd")).unwrap_err().contains("what the program calls the recovered SSD"));
        assert!(check(&three("Blue", "Spare 1 TB", "Red")).unwrap_err().contains("what the program calls SPARE"));
        let long = "x".repeat(LONGEST + 1);
        assert!(check(&three(&long, "", "")).unwrap_err().contains("41 characters long"));
        assert!(check(&three(&"é".repeat(LONGEST), "", "")).is_ok(), "counted in characters, not bytes");
        assert!(check(&three("Blue\u{0}one", "", "")).unwrap_err().contains("cannot be shown"));
        assert!(check(&three("Blue\u{1b}[31m", "", "")).unwrap_err().contains("cannot be shown"));
        assert!(check(&three("Blue\u{202e}eno", "", "")).unwrap_err().contains("cannot be shown"), "writing turned round");
        // (A line break or a tab typed in is only space.)
        assert_eq!(check(&three("Blue\r\n\tone", "", "")).unwrap()[0], "Blue one");
    }

    #[test]
    fn the_names_given_are_read_from_the_settings() {
        let mut s: HashMap<String, String> = HashMap::new();
        assert_eq!(given(&s), None);
        s.insert("name_spare".to_string(), "Red".to_string());
        assert_eq!(given(&s), None, "not until they are given");
        s.insert(DECIDED.to_string(), "2026-11-10 17:00:00".to_string());
        assert_eq!(given(&s), Some(three("RECOVERED 2 TB", "4 TB COPY", "Red")));
        s.insert(DECIDED.to_string(), " ".to_string());
        assert_eq!(given(&s), None);
    }

    #[test]
    fn spare_is_known_in_windows_by_its_own_name_or_the_one_given() {
        let n = three("Blue", "Green", "Dad's Drive");
        assert!(is_spare_label_with("SPARE", None) && is_spare_label_with("spare", Some(&n)));
        assert!(is_spare_label_with("DAD'S  DRIVE", Some(&n)));
        assert!(!is_spare_label_with("Dad's Drive", None), "only once it is given");
        assert!(!is_spare_label_with("Blue", Some(&n)) && !is_spare_label_with("Green", Some(&n)), "the other drives' names are not SPARE");
        assert!(!is_spare_label_with("", Some(&n)) && !is_spare_label_with("", None));
        assert!(!is_spare_label_with("SPARE 2", Some(&n)));
    }

    #[test]
    fn a_name_goes_beside_the_first_place_its_drive_is_named() {
        let n = three("Blue one", "Green", "Red");
        assert_eq!(
            beside_with("Plug in the recovered SSD now - the 2 TB one. The recovered SSD arrives switched off.", &n),
            "Plug in the recovered SSD (\u{201c}Blue one\u{201d}) now - the 2 TB one. The recovered SSD arrives switched off."
        );
        assert_eq!(
            beside_with("A copy of the old drive - the recovered SSD or the 4 TB SSD - arrived. Plug SPARE in.", &n),
            "A copy of the old drive - the recovered SSD (\u{201c}Blue one\u{201d}) or the 4 TB SSD (\u{201c}Green\u{201d}) - arrived. Plug SPARE (\u{201c}Red\u{201d}) in."
        );
        // Not in a path, a longer word or the suggested label; after a possessive.
        assert_eq!(beside_with(r"The copies go into E:\SPARE\WORK and SPARE/REPAIR.", &n), r"The copies go into E:\SPARE\WORK and SPARE/REPAIR.");
        assert_eq!(beside_with("SPARED, SPARE_2 and SPARE 1 TB are not it; SPARE is.", &n), "SPARED, SPARE_2 and SPARE 1 TB are not it; SPARE (\u{201c}Red\u{201d}) is.");
        assert_eq!(beside_with("If E: was SPARE's letter before", &n), "If E: was SPARE's (\u{201c}Red\u{201d}) letter before");
        assert_eq!(beside_with("Nothing about drives.", &n), "Nothing about drives.");
        // A name with another drive's word in it is left whole.
        let odd = three("Not SPARE", "Green", "Red");
        assert_eq!(beside_with("The recovered SSD, then SPARE.", &odd), "The recovered SSD (\u{201c}Not SPARE\u{201d}), then SPARE (\u{201c}Red\u{201d}).");
        // A name that is the program's own word is not said twice.
        let own = three("Recovered SSD", "the 4 TB SSD", "spare");
        assert_eq!(beside_with("Plug in the recovered SSD, never the 4 TB SSD; then SPARE.", &own), "Plug in the recovered SSD, never the 4 TB SSD; then SPARE.");
    }

    #[test]
    fn a_name_fits_in_a_table_of_the_checklists_copy() {
        assert_eq!(md_cell("Blue | one"), r"Blue \| one");
        assert_eq!(md_cell(r"C:\ drive"), r"C:\\ drive");
        assert_eq!(md_cell("Red"), "Red");
    }
}
