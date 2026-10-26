//! Progress on the taskbar, seen from other apps: the program's buttons on
//! the taskbar (its page's window, and its own console window) fill with
//! green as the job running gets on, turn yellow while Start waits for you or
//! the SSD cools down, and red when it stops on a problem - the way Windows
//! shows a file copy's progress. When it starts waiting for you, or stops on
//! a problem, the page's button flashes until you look at it.
//!
//! (Windows 11 shows only icons on the taskbar unless told otherwise, so the
//! page's title, which says the same in words, shows only when the mouse is
//! over its button.) Through the taskbar's own interface (ITaskbarList3), and
//! for Windows Terminal, which holds the console window when it is the
//! terminal Windows uses, its progress escape sequence (OSC 9;4).

/// What the taskbar shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bar {
    /// Nothing running: no bar.
    Off,
    /// Running, how far not known yet: a moving bar.
    Busy,
    /// Running: done of total.
    Part(u64, u64),
    /// Waiting for you, or paused while the SSD cools down: yellow.
    Held(u64, u64),
    /// Stopped on a problem: red.
    Problem,
}

impl Bar {
    /// Windows Terminal's progress sequence: ESC ] 9 ; 4 ; state ; percent BEL.
    pub fn osc(self) -> String {
        let pct = |d: u64, t: u64| if t == 0 { 0 } else { (d.min(t) * 100 / t).min(100) };
        let (state, p) = match self {
            Bar::Off => (0, 0),
            Bar::Busy => (3, 0),
            Bar::Part(d, t) => (1, pct(d, t)),
            Bar::Held(d, t) => (4, pct(d, t)),
            Bar::Problem => (2, 100),
        };
        format!("\x1b]9;4;{state};{p}\x07")
    }

    /// Worth flashing the page's button for, coming from `before`: it now
    /// needs you.
    pub fn calls(self, before: Bar) -> bool {
        matches!((self, before), (Bar::Held(..), b) if !matches!(b, Bar::Held(..)))
            || matches!((self, before), (Bar::Problem, b) if b != Bar::Problem)
    }
}

/// The page's window: a browser window showing the page (its title ends with
/// the program's name; in a browser's own window, the name then " - ").
pub fn is_page_title(title: &str) -> bool {
    title.ends_with(crate::APP) || title.contains(&format!("{} - ", crate::APP))
}

#[cfg(windows)]
mod imp {
    use super::Bar;
    use std::ffi::c_void;
    use std::ptr::null_mut;

    type Hwnd = isize;

    #[repr(C)]
    struct Guid {
        d1: u32,
        d2: u16,
        d3: u16,
        d4: [u8; 8],
    }

    const CLSID_TASKBAR_LIST: Guid = Guid { d1: 0x56FD_F344, d2: 0xFD6D, d3: 0x11D0, d4: [0x95, 0x8A, 0x00, 0x60, 0x97, 0xC9, 0xA0, 0x90] };
    const IID_ITASKBAR_LIST3: Guid = Guid { d1: 0xEA1A_FB91, d2: 0x9E28, d3: 0x4B86, d4: [0x90, 0xE9, 0x9E, 0x9F, 0x8A, 0x5E, 0xEF, 0xAF] };

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> i32;
        fn CoCreateInstance(clsid: *const Guid, outer: *mut c_void, ctx: u32, iid: *const Guid, out: *mut *mut c_void) -> i32;
    }

    #[repr(C)]
    struct FlashInfo {
        cb_size: u32,
        hwnd: Hwnd,
        flags: u32,
        count: u32,
        timeout: u32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(cb: extern "system" fn(Hwnd, isize) -> i32, lparam: isize) -> i32;
        fn GetWindowTextW(hwnd: Hwnd, buf: *mut u16, max: i32) -> i32;
        fn GetClassNameW(hwnd: Hwnd, buf: *mut u16, max: i32) -> i32;
        fn IsWindowVisible(hwnd: Hwnd) -> i32;
        fn FlashWindowEx(info: *const FlashInfo) -> i32;
        fn GetForegroundWindow() -> Hwnd;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleWindow() -> Hwnd;
        fn GetStdHandle(which: u32) -> isize;
        fn GetConsoleMode(handle: isize, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: isize, mode: u32) -> i32;
    }

    /// ITaskbarList3, as far as it is used (its functions in their order).
    #[repr(C)]
    struct Vtbl {
        query_interface: usize,
        add_ref: usize,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        hr_init: unsafe extern "system" fn(*mut c_void) -> i32,
        add_tab: usize,
        delete_tab: usize,
        activate_tab: usize,
        set_active_alt: usize,
        mark_fullscreen_window: usize,
        set_progress_value: unsafe extern "system" fn(*mut c_void, Hwnd, u64, u64) -> i32,
        set_progress_state: unsafe extern "system" fn(*mut c_void, Hwnd, u32) -> i32,
    }

    #[repr(C)]
    struct Object {
        vtbl: *const Vtbl,
    }

    pub struct Taskbar(*mut Object);

    impl Taskbar {
        /// The taskbar's interface (on this thread, which it must stay on).
        pub fn new() -> Option<Taskbar> {
            const COINIT_APARTMENTTHREADED: u32 = 0x2;
            const CLSCTX_INPROC_SERVER: u32 = 0x1;
            unsafe {
                CoInitializeEx(null_mut(), COINIT_APARTMENTTHREADED);
                let mut p: *mut c_void = null_mut();
                let hr = CoCreateInstance(&CLSID_TASKBAR_LIST, null_mut(), CLSCTX_INPROC_SERVER, &IID_ITASKBAR_LIST3, &mut p);
                if hr < 0 || p.is_null() {
                    return None;
                }
                let t = Taskbar(p as *mut Object);
                if ((*(*t.0).vtbl).hr_init)(p) < 0 {
                    return None;
                }
                Some(t)
            }
        }

        pub fn show(&self, hwnd: Hwnd, bar: Bar) {
            const NONE: u32 = 0;
            const BUSY: u32 = 0x1;
            const NORMAL: u32 = 0x2;
            const ERROR: u32 = 0x4;
            const PAUSED: u32 = 0x8;
            let (state, value) = match bar {
                Bar::Off => (NONE, None),
                Bar::Busy => (BUSY, None),
                Bar::Part(d, t) => (NORMAL, Some((d.min(t), t.max(1)))),
                Bar::Held(d, t) => (PAUSED, Some((d.min(t), t.max(1)))),
                Bar::Problem => (ERROR, Some((1, 1))),
            };
            unsafe {
                let v = &*(*self.0).vtbl;
                let me = self.0 as *mut c_void;
                (v.set_progress_state)(me, hwnd, state);
                if let Some((d, t)) = value {
                    // (A bar held at nothing would not show its colour.)
                    let d = if state == PAUSED { d.max(t / 100) } else { d };
                    (v.set_progress_value)(me, hwnd, d, t);
                }
            }
        }
    }

    impl Drop for Taskbar {
        fn drop(&mut self) {
            unsafe {
                ((*(*self.0).vtbl).release)(self.0 as *mut c_void);
            }
        }
    }

    fn text_of(hwnd: Hwnd, f: unsafe extern "system" fn(Hwnd, *mut u16, i32) -> i32) -> String {
        let mut buf = [0u16; 512];
        let n = unsafe { f(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    extern "system" fn each(hwnd: Hwnd, found: isize) -> i32 {
        let found = unsafe { &mut *(found as *mut Vec<Hwnd>) };
        if unsafe { IsWindowVisible(hwnd) } != 0
            && text_of(hwnd, GetClassNameW) == "Chrome_WidgetWin_1"
            && super::is_page_title(&text_of(hwnd, GetWindowTextW))
        {
            found.push(hwnd);
        }
        1
    }

    /// The browser windows showing the page.
    pub fn page_windows() -> Vec<Hwnd> {
        let mut found: Vec<Hwnd> = Vec::new();
        unsafe {
            EnumWindows(each, &mut found as *mut Vec<Hwnd> as isize);
        }
        found
    }

    /// The program's own console window (0: none, or one Windows Terminal holds).
    pub fn console_window() -> Hwnd {
        unsafe { GetConsoleWindow() }
    }

    /// Can the console take escape sequences (Windows Terminal shows the
    /// progress one; the classic console ignores it once they are on)?
    pub fn console_takes_sequences() -> bool {
        const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
        const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
        unsafe {
            let h = GetStdHandle(STD_OUTPUT_HANDLE);
            let mut mode = 0u32;
            if h == 0 || h == -1 || GetConsoleMode(h, &mut mode) == 0 {
                return false;
            }
            mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0 || SetConsoleMode(h, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
        }
    }

    /// Flash a window's taskbar button until it is looked at.
    pub fn flash(hwnd: Hwnd) {
        const FLASHW_TRAY: u32 = 0x2;
        const FLASHW_TIMERNOFG: u32 = 0xC;
        unsafe {
            if GetForegroundWindow() == hwnd {
                return;
            }
            let info = FlashInfo {
                cb_size: std::mem::size_of::<FlashInfo>() as u32,
                hwnd,
                flags: FLASHW_TRAY | FLASHW_TIMERNOFG,
                count: 0,
                timeout: 0,
            };
            FlashWindowEx(&info);
        }
    }
}

/// Keep the taskbar showing `now()` (asked every second), from a thread of
/// its own. Only on Windows.
pub fn watch(now: impl Fn() -> Bar + Send + 'static) {
    #[cfg(windows)]
    std::thread::spawn(move || {
        use std::collections::HashMap;
        use std::io::Write;
        use std::time::{Duration, Instant};
        let mut tb = imp::Taskbar::new();
        let mut tried = Instant::now();
        let sequences = imp::console_takes_sequences();
        let mut shown: HashMap<isize, Bar> = HashMap::new();
        let mut last = Bar::Off;
        let mut fresh = Instant::now();
        loop {
            let bar = now();
            // (The taskbar forgets when Explorer starts again: now and then,
            // everything is shown afresh; and its interface is asked for
            // again if it was not there.)
            if fresh.elapsed() >= Duration::from_secs(30) {
                shown.clear();
                fresh = Instant::now();
            }
            if tb.is_none() && tried.elapsed() >= Duration::from_secs(30) {
                tb = imp::Taskbar::new();
                tried = Instant::now();
            }
            let pages = imp::page_windows();
            let mut windows = pages.clone();
            let console = imp::console_window();
            if console != 0 {
                windows.push(console);
            }
            if let Some(t) = &tb {
                for &w in &windows {
                    if shown.get(&w) != Some(&bar) {
                        t.show(w, bar);
                        shown.insert(w, bar);
                    }
                }
                shown.retain(|w, _| windows.contains(w));
            }
            if bar != last {
                if bar.calls(last) {
                    for &w in &pages {
                        imp::flash(w);
                    }
                }
                if sequences {
                    let mut out = std::io::stdout();
                    let _ = out.write_all(bar.osc().as_bytes());
                    let _ = out.flush();
                }
                last = bar;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
    #[cfg(not(windows))]
    let _ = now;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_in_words_for_windows_terminal() {
        assert_eq!(Bar::Part(37, 100).osc(), "\x1b]9;4;1;37\x07");
        assert_eq!(Bar::Part(5, 0).osc(), "\x1b]9;4;1;0\x07");
        assert_eq!(Bar::Held(1, 2).osc(), "\x1b]9;4;4;50\x07");
        assert_eq!(Bar::Busy.osc(), "\x1b]9;4;3;0\x07");
        assert_eq!(Bar::Problem.osc(), "\x1b]9;4;2;100\x07");
        assert_eq!(Bar::Off.osc(), "\x1b]9;4;0;0\x07");
    }

    #[test]
    fn it_calls_you_once_when_it_needs_you() {
        assert!(Bar::Held(1, 2).calls(Bar::Part(1, 2)));
        assert!(!Bar::Held(3, 4).calls(Bar::Held(1, 2)), "still waiting: once is enough");
        assert!(Bar::Problem.calls(Bar::Busy));
        assert!(!Bar::Problem.calls(Bar::Problem));
        assert!(!Bar::Part(1, 2).calls(Bar::Held(1, 2)));
        assert!(!Bar::Off.calls(Bar::Part(1, 2)));
    }

    #[test]
    fn the_page_is_told_by_its_title() {
        assert!(is_page_title("Automatic Recovery Probe"));
        assert!(is_page_title("37% · Making the image · 2 h left · Automatic Recovery Probe"));
        assert!(is_page_title("Your turn · Automatic Recovery Probe - Personal - Microsoft Edge"));
        assert!(!is_page_title("Automatic-Recovery-Probe: GitHub - Microsoft Edge"));
        assert!(!is_page_title("Automatic Recovery Probe review - Google Search - Google Chrome"));
    }

    /// On Windows: the taskbar's interface, if there is a taskbar here, takes
    /// every kind of bar for the console window (if there is one).
    #[cfg(windows)]
    #[test]
    fn windows_takes_the_bars() {
        let _ = imp::page_windows();
        if let Some(t) = imp::Taskbar::new() {
            let w = imp::console_window();
            if w != 0 {
                for b in [Bar::Busy, Bar::Part(1, 3), Bar::Held(2, 3), Bar::Problem, Bar::Off] {
                    t.show(w, b);
                }
            }
        }
    }
}
