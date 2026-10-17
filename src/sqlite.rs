//! SQLite integrity check, for Apple Photos Library databases.
//!
//! Uses the SQLite that ships with the operating system (winsqlite3.dll on
//! Windows 10 and 11, libsqlite3 on Linux), loaded at run time, so nothing
//! has to be bundled. The logic is a port of the Python version's
//! check_sqlite / _sqlite_verdict, including its handling of a hot -wal.

use crate::checks::{ok, Finding};
use crate::sys::Library;
use std::ffi::{c_char, c_int, c_void, CStr};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

type Db = c_void;
type Stmt = c_void;

struct Api {
    _lib: Library,
    open_v2: unsafe extern "C" fn(*const c_char, *mut *mut Db, c_int, *const c_char) -> c_int,
    close_v2: unsafe extern "C" fn(*mut Db) -> c_int,
    busy_timeout: unsafe extern "C" fn(*mut Db, c_int) -> c_int,
    prepare_v2: unsafe extern "C" fn(*mut Db, *const c_char, c_int, *mut *mut Stmt, *mut *const c_char) -> c_int,
    step: unsafe extern "C" fn(*mut Stmt) -> c_int,
    column_count: unsafe extern "C" fn(*mut Stmt) -> c_int,
    column_type: unsafe extern "C" fn(*mut Stmt, c_int) -> c_int,
    column_text: unsafe extern "C" fn(*mut Stmt, c_int) -> *const u8,
    column_int64: unsafe extern "C" fn(*mut Stmt, c_int) -> i64,
    finalize: unsafe extern "C" fn(*mut Stmt) -> c_int,
    errmsg: unsafe extern "C" fn(*mut Db) -> *const c_char,
    extended_errcode: unsafe extern "C" fn(*mut Db) -> c_int,
    errstr: unsafe extern "C" fn(c_int) -> *const c_char,
}

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_NULL: c_int = 5;
const SQLITE_INTEGER: c_int = 1;
const OPEN_READWRITE: c_int = 0x02;
const OPEN_CREATE: c_int = 0x04;
const OPEN_URI: c_int = 0x40;

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        let names: &[&str] = if cfg!(windows) {
            &["winsqlite3.dll"]
        } else {
            &["libsqlite3.so.0", "libsqlite3.so", "libsqlite3.dylib"]
        };
        let lib = Library::open(names)?;
        macro_rules! sym {
            ($name:expr) => {{
                let p = lib.symbol($name);
                if p.is_null() {
                    return None;
                }
                std::mem::transmute(p)
            }};
        }
        Some(Api {
            open_v2: sym!("sqlite3_open_v2"),
            close_v2: sym!("sqlite3_close_v2"),
            busy_timeout: sym!("sqlite3_busy_timeout"),
            prepare_v2: sym!("sqlite3_prepare_v2"),
            step: sym!("sqlite3_step"),
            column_count: sym!("sqlite3_column_count"),
            column_type: sym!("sqlite3_column_type"),
            column_text: sym!("sqlite3_column_text"),
            column_int64: sym!("sqlite3_column_int64"),
            finalize: sym!("sqlite3_finalize"),
            errmsg: sym!("sqlite3_errmsg"),
            extended_errcode: sym!("sqlite3_extended_errcode"),
            errstr: sym!("sqlite3_errstr"),
            _lib: lib,
        })
    })
    .as_ref()
}

/// Is a working SQLite available on this machine?
pub fn available() -> bool {
    api().is_some()
}

/// An SQLite error: the extended result code and the message.
struct SqlErr {
    code: c_int,
    msg: String,
}

#[derive(Clone)]
enum Val {
    Null,
    Int(i64),
    Text(String),
}

impl Val {
    fn truthy(&self) -> bool {
        match self {
            Val::Null => false,
            Val::Int(i) => *i != 0,
            Val::Text(s) => !s.is_empty(),
        }
    }
    fn text(&self) -> String {
        match self {
            Val::Null => "None".to_string(),
            Val::Int(i) => i.to_string(),
            Val::Text(s) => s.clone(),
        }
    }
}

struct Conn {
    api: &'static Api,
    db: *mut Db,
}

impl Drop for Conn {
    fn drop(&mut self) {
        unsafe {
            (self.api.close_v2)(self.db);
        }
    }
}

unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

impl Conn {
    fn open(api: &'static Api, uri: &str) -> Result<Conn, SqlErr> {
        let c = format!("{uri}\0");
        let mut db: *mut Db = std::ptr::null_mut();
        let rc = unsafe { (api.open_v2)(c.as_ptr() as *const c_char, &mut db, OPEN_READWRITE | OPEN_CREATE | OPEN_URI, std::ptr::null()) };
        if db.is_null() {
            return Err(SqlErr { code: rc, msg: unsafe { cstr((api.errstr)(rc)) } });
        }
        let conn = Conn { api, db };
        if rc != SQLITE_OK {
            return Err(conn.err());
        }
        unsafe {
            (api.busy_timeout)(db, 5000);
        }
        Ok(conn)
    }

    fn err(&self) -> SqlErr {
        unsafe { SqlErr { code: (self.api.extended_errcode)(self.db), msg: cstr((self.api.errmsg)(self.db)) } }
    }

    /// Run one statement and fetch every row, like Python's execute().fetchall().
    fn query(&self, sql: &str) -> Result<Vec<Vec<Val>>, SqlErr> {
        let c = format!("{sql}\0");
        let mut stmt: *mut Stmt = std::ptr::null_mut();
        let rc = unsafe {
            (self.api.prepare_v2)(self.db, c.as_ptr() as *const c_char, -1, &mut stmt, std::ptr::null_mut())
        };
        if rc != SQLITE_OK {
            if !stmt.is_null() {
                unsafe {
                    (self.api.finalize)(stmt);
                }
            }
            return Err(self.err());
        }
        let mut rows = Vec::new();
        loop {
            let rc = unsafe { (self.api.step)(stmt) };
            if rc == SQLITE_ROW {
                let n = unsafe { (self.api.column_count)(stmt) };
                let mut row = Vec::with_capacity(n as usize);
                for i in 0..n {
                    let t = unsafe { (self.api.column_type)(stmt, i) };
                    row.push(match t {
                        SQLITE_NULL => Val::Null,
                        SQLITE_INTEGER => Val::Int(unsafe { (self.api.column_int64)(stmt, i) }),
                        _ => {
                            let p = unsafe { (self.api.column_text)(stmt, i) };
                            if p.is_null() {
                                Val::Null
                            } else {
                                Val::Text(unsafe { CStr::from_ptr(p as *const c_char).to_string_lossy().into_owned() })
                            }
                        }
                    });
                }
                rows.push(row);
            } else if rc == SQLITE_DONE {
                break;
            } else {
                let e = self.err();
                unsafe {
                    (self.api.finalize)(stmt);
                }
                return Err(e);
            }
        }
        unsafe {
            (self.api.finalize)(stmt);
        }
        Ok(rows)
    }
}

/// The Python sqlite3 module's exception class for a result code.
fn py_exception_name(code: c_int) -> &'static str {
    match code & 0xFF {
        2 | 12 => "InternalError",
        1 | 3 | 4 | 5 | 6 | 8 | 9 | 10 | 13 | 14 | 15 | 16 | 17 => "OperationalError",
        11 => "DatabaseError",
        18 => "DataError",
        19 | 20 => "IntegrityError",
        21 | 25 => "InterfaceError",
        _ => "DatabaseError",
    }
}

/// SQLite result codes that describe access, not content: PERM, BUSY,
/// LOCKED, READONLY, IOERR, CANTOPEN, AUTH.
fn is_access_error(code: c_int) -> bool {
    matches!(code & 0xFF, 3 | 5 | 6 | 8 | 10 | 14 | 23)
}

/// `pathlib.Path(path).absolute().as_uri()` - a file: URI SQLite accepts.
fn file_uri(path: &Path) -> String {
    let abs: PathBuf = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let s = abs.to_string_lossy().replace('\\', "/");
    let (prefix, rest) = if cfg!(windows) {
        if s.len() >= 2 && s.as_bytes()[1] == b':' {
            (format!("file:///{}", &s[..2]), s[2..].to_string())
        } else if let Some(unc) = s.strip_prefix("//") {
            ("file://".to_string(), unc.to_string())
        } else {
            ("file://".to_string(), s.clone())
        }
    } else {
        ("file://".to_string(), s.clone())
    };
    let mut out = prefix;
    for b in rest.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Integrity-check a SQLite database without writing beside the original.
///
/// With no hot `-wal` beside it, the file is opened immutable, which is safe
/// and correct. With one, the database and its sidecars are copied to a
/// scratch folder and the copy is checked, so the WAL is read but nothing is
/// ever written next to the recovered data.
pub fn check_sqlite(path: &Path) -> Finding {
    let api = match api() {
        Some(a) => a,
        None => {
            return (
                "UNREADABLE",
                "SQLite is not available on this PC, so this database could not be checked - check it by hand".to_string(),
            )
        }
    };
    let wal = with_suffix(path, "-wal");
    let hot_wal = std::fs::metadata(&wal).map(|m| m.len() > 0).unwrap_or(false);
    if !hot_wal {
        return sqlite_verdict(api, path, "?mode=ro&immutable=1", false, "");
    }
    let scratch = match scratch_dir() {
        Ok(d) => d,
        Err(e) => return copy_failed(&e),
    };
    let result = (|| -> std::io::Result<Finding> {
        let copy = scratch.join("db.sqlite");
        for suffix in ["", "-wal", "-shm"] {
            let src = with_suffix(path, suffix);
            if src.exists() {
                std::fs::copy(&src, with_suffix(&copy, suffix))?;
            }
        }
        Ok(sqlite_verdict(api, &copy, "?mode=ro", true, " (checked with its -wal, on a scratch copy)"))
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    match result {
        Ok(f) => f,
        Err(e) => copy_failed(&e),
    }
}

fn copy_failed(e: &std::io::Error) -> Finding {
    (
        "UNREADABLE",
        format!("has a hot -wal and could not be copied for checking ({}: {e}) - check it by hand", crate::probe::py_error_name(e)),
    )
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

fn scratch_dir() -> std::io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let base = std::env::temp_dir();
    for _ in 0..100 {
        let d = base.join(format!("probe-db-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        match std::fs::create_dir(&d) {
            Ok(()) => return Ok(d),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("could not create a scratch folder"))
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sqlite_verdict(api: &'static Api, path: &Path, suffix: &str, had_wal: bool, note: &str) -> Finding {
    let uri = format!("{}{}", file_uri(path), suffix);
    let outcome = (|| -> Result<(Vec<Vec<Val>>, usize, usize), SqlErr> {
        let con = Conn::open(api, &uri)?;
        // Both pragmas, cheapest first.
        let mut rows = con.query("PRAGMA quick_check(20)")?;
        if let Some(first) = rows.first().and_then(|r| r.first()) {
            if first.text().to_lowercase() == "ok" {
                rows = con.query("PRAGMA integrity_check")?;
            }
        }
        let names: Vec<String> = con
            .query("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' LIMIT 40")?
            .into_iter()
            .map(|r| r.first().map(|v| v.text()).unwrap_or_default())
            .collect();
        let tables = names.len();
        // A database whose tables all came back empty is the signature of a
        // WAL that SQLite discarded.
        let mut populated = 0;
        for t in &names {
            if let Ok(r) = con.query(&format!("SELECT EXISTS(SELECT 1 FROM \"{t}\" LIMIT 1)")) {
                if r.first().and_then(|row| row.first()).map(|v| v.truthy()).unwrap_or(false) {
                    populated += 1;
                    break;
                }
            }
        }
        Ok((rows, tables, populated))
    })();
    let (rows, tables, populated) = match outcome {
        Ok(v) => v,
        Err(e) => {
            let name = py_exception_name(e.code);
            if name == "InterfaceError" || is_access_error(e.code) {
                // Locked or permission-denied is a fact about access, not damage.
                return ("UNREADABLE", format!("{name}: {}", e.msg));
            }
            return ("DB_DAMAGED", format!("SQLite cannot read it: {}{note}", collapse_ws(&e.msg)));
        }
    };
    let msgs: Vec<String> = rows
        .iter()
        .filter_map(|r| r.first())
        .filter(|v| v.truthy())
        .map(|v| v.text())
        .collect();
    if !msgs.is_empty() && !(msgs.len() == 1 && msgs[0].to_lowercase() == "ok") {
        let more = if msgs.len() > 3 { format!(" (+{} more)", msgs.len() - 3) } else { String::new() };
        let flat = collapse_ws(&msgs.iter().take(3).cloned().collect::<Vec<_>>().join(" "));
        let flat: String = flat.chars().take(100).collect();
        return ("DB_DAMAGED", format!("integrity check failed: {flat}{more}{note}"));
    }
    if tables == 0 {
        return (
            "DB_DAMAGED",
            format!("integrity check passed but the database contains no tables - its contents were not readable{note}"),
        );
    }
    if had_wal && populated == 0 {
        return (
            "DB_DAMAGED",
            format!(
                "integrity check passed but every table is empty - the -wal beside it was unreadable and SQLite discarded it silently{note}"
            ),
        );
    }
    ok()
}
