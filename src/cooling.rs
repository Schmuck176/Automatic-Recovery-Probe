//! The SSD's temperature while the image is made - hours of reading, often
//! in a USB case with little air around it: read from Windows every half
//! minute, shown on the page, and reading paused when it gets too hot,
//! until it has cooled down (image.rs waits while `pause` is set).
//!
//! Windows is asked directly first (IOCTL_STORAGE_QUERY_PROPERTY, win.rs)
//! and, if the SSD's driver does not answer that, through its storage
//! reliability counter (winset.rs). Many USB cases answer neither: the page
//! then says so, and the image is made without watching.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Reading pauses at this temperature at the latest (°C): below where SSDs
/// slow themselves down to cool (usually 70-85 °C), as a failing SSD is
/// better kept cool.
pub const MOST: i16 = 70;
/// ...and never below this, whatever the SSD says of itself (a low warning
/// level would otherwise keep it paused in a warm room).
pub const LEAST: i16 = 50;
/// Reading carries on when it has cooled this much below the limit.
pub const COOLER_BY: i16 = 10;
/// How often it is read.
const EVERY: Duration = Duration::from_secs(30);
/// Paused this long, it carries on a little sooner (at 5 °C below the limit).
const LONG_PAUSE: Duration = Duration::from_secs(15 * 60);
/// Paused this long without the reading going any lower, the reading is
/// taken as stuck (a sensor or USB case that keeps saying the same), or the
/// SSD as cool as it gets where it is: an SSD at rest cools by a degree
/// within minutes, until it is as cool as the air around it lets it be.
const STUCK_AFTER: Duration = Duration::from_secs(45 * 60);
/// A reading taken as stuck pauses reading again only when it rises this
/// much above where it stuck.
const STUCK_RISE: i16 = 5;

/// Why reading last carried on after a pause: cooled (0), not read for a
/// while (1), not cooled in STUCK_AFTER (2).
static WHY: AtomicU8 = AtomicU8::new(0);

/// What the image's line says when reading carries on after a pause.
pub fn carried_on_because() -> &'static str {
    match WHY.load(Ordering::SeqCst) {
        1 => "The SSD's temperature could not be read for a while: reading carries on.",
        2 => "The SSD has not cooled in 45 minutes - its temperature reading may be stuck: reading carries on.",
        _ => "The SSD has cooled down: reading carries on.",
    }
}

/// Where reading pauses for this SSD, from the levels it reports itself.
pub fn limit_for(warning: Option<i16>, critical: Option<i16>) -> i16 {
    let mut l = MOST;
    if let Some(w) = warning {
        l = l.min(w - 2);
    }
    if let Some(c) = critical {
        l = l.min(c - 10);
    }
    l.clamp(LEAST, MOST)
}

/// What a reading changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// The first reading.
    First(i16),
    /// Close to the limit (told once).
    Warm(i16),
    Pause(i16),
    Resume(i16),
    /// Paused, and it could not be read for a while: reading carries on.
    ResumeUnknown,
    /// Paused for long, the reading not gone any lower (the lowest it has
    /// been, here): taken as stuck, reading carries on.
    ResumeStuck(i16),
}

/// Pause or carry on, reading by reading.
#[derive(Clone, Debug)]
pub struct Cooling {
    pub limit: i16,
    pub paused_since: Option<Instant>,
    pub pauses: u32,
    first: bool,
    warned: bool,
    misses: u32,
    /// The lowest reading since the pause began, and when it was first that
    /// low.
    lowest: i16,
    lowest_since: Option<Instant>,
    /// The reading taken as stuck (see STUCK_AFTER), until the SSD has
    /// cooled to where reading carries on.
    pub stuck_at: Option<i16>,
}

impl Cooling {
    pub fn new(limit: i16) -> Cooling {
        Cooling {
            limit,
            paused_since: None,
            pauses: 0,
            first: true,
            warned: false,
            misses: 0,
            lowest: i16::MAX,
            lowest_since: None,
            stuck_at: None,
        }
    }

    /// Where reading pauses: the limit - or, a reading taken as stuck, only a
    /// real rise above it.
    pub fn pause_at(&self) -> i16 {
        match self.stuck_at {
            Some(s) => self.limit.max(s.saturating_add(STUCK_RISE)),
            None => self.limit,
        }
    }

    pub fn resume_at(&self) -> i16 {
        self.limit - COOLER_BY
    }

    /// Where reading carries on now (paused long, a little sooner).
    pub fn carries_on_at(&self, now: Instant) -> i16 {
        match self.paused_since {
            Some(since) if now.duration_since(since) >= LONG_PAUSE => self.limit - 5,
            _ => self.resume_at(),
        }
    }

    /// A reading (None: it could not be read this time).
    pub fn reading(&mut self, t: Option<i16>, now: Instant) -> Vec<Change> {
        let mut out = Vec::new();
        let Some(t) = t else {
            self.misses += 1;
            if self.paused_since.is_some() && self.misses >= 3 {
                self.paused_since = None;
                out.push(Change::ResumeUnknown);
            }
            return out;
        };
        self.misses = 0;
        if self.first {
            self.first = false;
            out.push(Change::First(t));
        }
        // Cooled to where reading carries on: whatever was taken as stuck
        // was not.
        if t <= self.resume_at() {
            self.stuck_at = None;
        }
        match self.paused_since {
            None if t >= self.pause_at() => {
                self.paused_since = Some(now);
                self.pauses += 1;
                self.warned = true;
                self.lowest = t;
                self.lowest_since = Some(now);
                out.push(Change::Pause(t));
            }
            None => {
                if t >= self.limit - 5 && !self.warned {
                    self.warned = true;
                    out.push(Change::Warm(t));
                }
            }
            Some(since) => {
                if t < self.lowest {
                    self.lowest = t;
                    self.lowest_since = Some(now);
                }
                let long = now.duration_since(since) >= LONG_PAUSE;
                let low_for = now.duration_since(self.lowest_since.unwrap_or(since));
                if t <= self.resume_at() || (long && t <= self.limit - 5) {
                    self.paused_since = None;
                    out.push(Change::Resume(t));
                } else if low_for >= STUCK_AFTER {
                    // Not a degree lower in all that time at rest: the
                    // reading is stuck (a sensor or USB case that keeps
                    // saying the same), or the SSD as cool as it gets where
                    // it is - waiting on it would wait for good.
                    self.paused_since = None;
                    self.stuck_at = Some(t);
                    out.push(Change::ResumeStuck(self.lowest));
                }
            }
        }
        out
    }
}

/// Where the readings come from.
enum Way {
    /// Windows asked directly (with the levels the SSD reports).
    Direct(u32),
    /// The storage reliability counter (PowerShell).
    Counter(u32),
    /// Readings given by a test (debug builds: ARP_TEST_TEMPS).
    Test(Vec<i16>, usize),
}

impl Way {
    fn read(&mut self) -> Option<i16> {
        match self {
            Way::Direct(n) => crate::win::disk_temperature(*n).map(|t| t.now),
            Way::Counter(n) => crate::winset::reliability_temperature(*n),
            Way::Test(ts, i) => {
                let t = ts.get(*i).or(ts.last()).copied();
                *i += 1;
                t
            }
        }
    }
}

/// Readings a test gives, and how often (debug builds only).
fn test_readings() -> Option<(Vec<i16>, Duration)> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let ts: Vec<i16> = std::env::var("ARP_TEST_TEMPS").ok()?.split(',').filter_map(|t| t.trim().parse().ok()).collect();
    let ms = std::env::var("ARP_TEST_TEMP_MS").ok().and_then(|m| m.trim().parse().ok()).unwrap_or(1000);
    (!ts.is_empty()).then(|| (ts, Duration::from_millis(ms)))
}

/// Watch disk `n` (None: not a disk Windows numbers, such as a test disk)
/// while the image reads it, until `done`. Reading begins with the strip of
/// the drive (live::drive_begin) and ends when it has all been read.
pub fn watch(n: Option<u32>, pause: Arc<AtomicBool>, done: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let wait = |d: Duration| {
            let until = Instant::now() + d;
            while Instant::now() < until && !done.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        // Until the SSD itself is being read (an image made before is only
        // checked, without reading the SSD).
        while !crate::live::drive_reading() {
            if done.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let (mut way, limit, every) = if let Some((ts, every)) = test_readings() {
            (Way::Test(ts, 0), MOST, every)
        } else if let Some(n) = n {
            if let Some(t) = crate::win::disk_temperature(n) {
                (Way::Direct(n), limit_for(t.warning, t.critical), EVERY)
            } else if crate::winset::reliability_temperature(n).is_some() {
                (Way::Counter(n), MOST, EVERY)
            } else {
                crate::live::drive_temp(
                    None,
                    None,
                    None,
                    "This SSD (or its USB case) does not tell Windows its temperature, so it cannot be watched. Keep \
                     it where air can get to it.",
                );
                crate::live::event("info", "The SSD does not tell Windows its temperature: the image is made without watching it");
                return;
            }
        } else {
            return;
        };
        let mut c = Cooling::new(limit);
        while !done.load(Ordering::Relaxed) && crate::live::drive_reading() {
            let t = way.read();
            if done.load(Ordering::Relaxed) {
                break;
            }
            let now = Instant::now();
            let changes = c.reading(t, now);
            if t.is_some() {
                crate::live::drive_temp(t, Some(c.pause_at()), Some(c.carries_on_at(now)), "");
            }
            for ch in changes {
                match ch {
                    Change::First(t) => crate::live::event(
                        "info",
                        &format!("The SSD is at {t} °C (reading pauses if it gets to {} °C, and carries on at {} °C)", c.limit, c.resume_at()),
                    ),
                    Change::Warm(t) => {
                        crate::live::event("warn", &format!("The SSD is getting warm: {t} °C (reading pauses at {} °C)", c.limit))
                    }
                    Change::Pause(t) => {
                        pause.store(true, Ordering::Relaxed);
                        crate::live::drive_paused(true);
                        crate::live::event("warn", &format!("The SSD is at {t} °C: reading paused until it has cooled to {} °C", c.resume_at()));
                        if c.pauses == 1 {
                            crate::notify::toast(
                                "The SSD is too hot: reading paused",
                                &format!(
                                    "It is at {t} °C. Reading carries on by itself when it has cooled to {} °C. Give it air: \
                                     take it out of a closed space, or point a fan at it.",
                                    c.resume_at()
                                ),
                                true,
                            );
                        }
                    }
                    Change::Resume(t) => {
                        WHY.store(0, Ordering::SeqCst);
                        pause.store(false, Ordering::SeqCst);
                        crate::live::drive_paused(false);
                        crate::live::event("good", &format!("The SSD has cooled to {t} °C: reading carries on"));
                    }
                    Change::ResumeUnknown => {
                        WHY.store(1, Ordering::SeqCst);
                        pause.store(false, Ordering::SeqCst);
                        crate::live::drive_paused(false);
                        crate::live::event("warn", "The SSD's temperature could not be read for a while: reading carries on");
                    }
                    Change::ResumeStuck(low) => {
                        WHY.store(2, Ordering::SeqCst);
                        pause.store(false, Ordering::SeqCst);
                        crate::live::drive_paused(false);
                        crate::live::event(
                            "warn",
                            &format!(
                                "The SSD has not cooled below {low} °C in 45 minutes at rest - its temperature reading may \
                                 be stuck, or it is as cool as it can get where it is. Reading carries on, and pauses again \
                                 if it gets to {} °C. If the SSD really feels hot, click Stop and give it air.",
                                c.pause_at()
                            ),
                        );
                        crate::notify::toast(
                            "The SSD has not cooled in 45 minutes: reading carries on",
                            &format!(
                                "It has stayed at {low} °C or more at rest - its temperature reading may be stuck. If the \
                                 SSD really feels hot, click Stop and give it air."
                            ),
                            true,
                        );
                    }
                }
            }
            wait(if c.paused_since.is_some() { every / 2 } else { every });
        }
        // Never left paused.
        if pause.swap(false, Ordering::Relaxed) {
            crate::live::drive_paused(false);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn where_it_pauses() {
        assert_eq!(limit_for(None, None), 70);
        // NVMe SSDs often warn at 80-85 °C: the program pauses earlier.
        assert_eq!(limit_for(Some(84), Some(88)), 70);
        assert_eq!(limit_for(Some(65), None), 63);
        assert_eq!(limit_for(None, Some(75)), 65);
        // Never so low it would not read in a warm room.
        assert_eq!(limit_for(Some(30), Some(40)), 50);
    }

    #[test]
    fn pauses_when_hot_and_carries_on_when_cool() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut c = Cooling::new(70);
        assert_eq!(c.reading(Some(41), at(0)), vec![Change::First(41)]);
        assert_eq!(c.reading(Some(55), at(30)), vec![]);
        assert_eq!(c.reading(Some(66), at(60)), vec![Change::Warm(66)]);
        assert_eq!(c.reading(Some(67), at(90)), vec![], "warned once");
        assert_eq!(c.reading(Some(71), at(120)), vec![Change::Pause(71)]);
        assert!(c.paused_since.is_some());
        assert_eq!(c.reading(Some(68), at(150)), vec![]);
        assert_eq!(c.reading(Some(61), at(180)), vec![], "not yet cool enough");
        assert_eq!(c.reading(Some(60), at(210)), vec![Change::Resume(60)]);
        assert!(c.paused_since.is_none());
        // Hot again: paused again (not warned again).
        assert_eq!(c.reading(Some(67), at(240)), vec![]);
        assert_eq!(c.reading(Some(70), at(270)), vec![Change::Pause(70)]);
        assert_eq!(c.pauses, 2);
        // A long pause carries on a little sooner.
        assert_eq!(c.reading(Some(65), at(270 + 14 * 60)), vec![]);
        assert_eq!(c.reading(Some(65), at(270 + 15 * 60)), vec![Change::Resume(65)]);
    }

    /// A reading that stays at one value while reading is paused (a sensor or
    /// USB case that keeps saying the same): after 45 minutes at rest with not
    /// a degree lower it is taken as stuck, and reading carries on - pausing
    /// again only on a real rise. (Before, the pause lasted for good.)
    #[test]
    fn a_reading_stuck_hot_does_not_pause_for_good() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut c = Cooling::new(70);
        assert_eq!(c.reading(Some(85), at(0)), vec![Change::First(85), Change::Pause(85)]);
        for m in 1..45 {
            assert_eq!(c.reading(Some(85), at(m * 60)), vec![], "minute {m}");
            assert!(c.paused_since.is_some());
        }
        assert_eq!(c.reading(Some(85), at(45 * 60)), vec![Change::ResumeStuck(85)]);
        assert!(c.paused_since.is_none());
        assert_eq!(c.pause_at(), 90);
        // The same reading again: reading goes on.
        assert_eq!(c.reading(Some(85), at(46 * 60)), vec![]);
        assert_eq!(c.reading(Some(89), at(47 * 60)), vec![]);
        // A real rise: paused again.
        assert_eq!(c.reading(Some(90), at(48 * 60)), vec![Change::Pause(90)]);
        // Cooled to where reading carries on: not stuck after all, and it
        // pauses at the limit again.
        assert_eq!(c.reading(Some(60), at(49 * 60)), vec![Change::Resume(60)]);
        assert_eq!((c.stuck_at, c.pause_at()), (None, 70));
        assert_eq!(c.reading(Some(70), at(50 * 60)), vec![Change::Pause(70)]);

        // A sensor that wavers by a degree, stuck all the same: taken as
        // stuck 45 minutes after its lowest.
        let mut w = Cooling::new(70);
        w.reading(Some(85), at(0));
        for m in 1..46 {
            assert_eq!(w.reading(Some(if m % 2 == 1 { 84 } else { 85 }), at(m * 60)), vec![], "minute {m}");
        }
        assert_eq!(w.reading(Some(85), at(46 * 60)), vec![Change::ResumeStuck(84)]);
    }

    /// An SSD that cools, however slowly, is waited for: a degree every three
    /// minutes from 90 °C is still 75 °C after 45 minutes, and reading
    /// carries on only once it has cooled (before this was right, it was taken
    /// as stuck there). One that stops cooling above where reading carries on
    /// - as cool as it gets where it is - is not waited for for good.
    #[test]
    fn an_ssd_cooling_slowly_is_waited_for() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut c = Cooling::new(70);
        assert_eq!(c.reading(Some(90), at(0)), vec![Change::First(90), Change::Pause(90)]);
        let mut resumed = None;
        for half in 1..=200u64 {
            let t = 90 - (half / 6) as i16;
            let ch = c.reading(Some(t), at(half * 30));
            assert!(!ch.iter().any(|c| matches!(c, Change::ResumeStuck(_))), "at {} s ({t} °C): {ch:?}", half * 30);
            if ch == vec![Change::Resume(t)] {
                resumed = Some((half * 30, t));
                break;
            }
        }
        // Paused 15 minutes and more, it carries on at 5 °C below the limit.
        assert_eq!(resumed, Some((75 * 60, 65)));

        // Cooled to 72 °C in ten minutes, and no lower: 45 minutes after that,
        // reading carries on; it pauses again on a rise of 5 °C.
        let mut p = Cooling::new(70);
        p.reading(Some(80), at(0));
        for m in 1..=10u64 {
            assert_eq!(p.reading(Some(80 - (m as i16 * 8) / 10), at(m * 60)), vec![], "minute {m}");
        }
        for m in 11..55 {
            assert_eq!(p.reading(Some(72), at(m * 60)), vec![], "minute {m}");
        }
        assert_eq!(p.reading(Some(72), at(55 * 60)), vec![Change::ResumeStuck(72)]);
        assert_eq!(p.reading(Some(76), at(56 * 60)), vec![]);
        assert_eq!(p.reading(Some(77), at(57 * 60)), vec![Change::Pause(77)]);
    }

    #[test]
    fn never_stuck_paused() {
        let t0 = Instant::now();
        let mut c = Cooling::new(70);
        c.reading(Some(75), t0);
        assert!(c.paused_since.is_some());
        assert_eq!(c.reading(None, t0), vec![]);
        assert_eq!(c.reading(None, t0), vec![]);
        assert_eq!(c.reading(None, t0), vec![Change::ResumeUnknown]);
        assert!(c.paused_since.is_none());
        // Not read while reading goes on: nothing changes.
        for _ in 0..5 {
            assert_eq!(c.reading(None, t0), vec![]);
        }
    }
}
