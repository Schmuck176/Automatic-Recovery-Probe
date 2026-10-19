//! Recovery Probe - the program you double-click.

use recovery_probe::cli::{
    run_list_disks, run_map_check, run_map_info, run_probe_jsonl, run_rescue, run_scan, ScanOpts, Status,
};
use recovery_probe::{APP, VERSION};

const USAGE: &str = "\
Recovery Probe - finds damage in files recovered with ddrescue.

Double-click the program to use it step by step in your web browser. Or:
  recovery-probe self-test
        Check the program works on this PC (on files and a disk built into it).
  recovery-probe scan <folder> [--out results.csv] [--hole-kb N] [--no-holes]
        Check every file in a folder and write the results CSV files.
  recovery-probe probe-jsonl <folder> [--hole-kb N] [--no-holes]
        One line of JSON per file (used to compare with the Python version).
  recovery-probe map-check --map <mapfile> --disk <disk> [--earlier-map <mapfile>] [--out <folder>]
        Work out which files and folders sit on the places ddrescue never read.
        <disk> is \\\\.\\PhysicalDriveN (needs administrator rights) or a disk image.
        --earlier-map: an earlier attempt's mapfile, if it copied onto the same disk.
  recovery-probe rescue --disk <disk> --out <folder> [--everything | --path <path>...] [--lost]
        Copy files straight off the disk, past Windows: by default the ones Windows
        cannot list. --lost also saves data no folder record names any more.
  recovery-probe image --disk <disk> --map <mapfile> --out <folder>
        Read the whole disk once into one image file (recovered-ssd.img) in <folder>,
        with its SHA-256, and check it by reading it back. Stopped, it carries on.
  recovery-probe verify <folder>
        Check every copy in <folder> against the SHA-256 the copying wrote into
        rescue-log.csv: changed, missing and added files.
  recovery-probe isolate <copies folder> --scan <results .csv> [--scan <results .csv>]...
        Step 1 of repairing: copy every damaged file out into REPAIR (beside the
        copies folder), sorted by what repairs it, each name tagged [R0001]...;
        REPAIR\\repair-list.csv says where each belongs. --scan: the results of
        checking the copies (scan-....csv); lists in REPAIR\\damaged-lists count too.
  recovery-probe put-back <copies folder>
        Step 3: put every repaired file in REPAIR\\repaired (found by its [R....])
        back where its original is, if that is still the damaged original; the
        original moves to REPAIR\\originals. Listed in repair-returns.csv.
  recovery-probe undo-put-back <copies folder> <R0012>
        Put the original back in place of a repaired version.
  recovery-probe evaluate <copies folder> [--scan <results .csv>]...
        Step 4: what is damaged inside every package (libraries, apps, documents
        kept as folders), what has been put back, what is still damaged.
  recovery-probe sha256 <file>
        A file's SHA-256 (the same as certutil -hashfile <file> SHA256).
  recovery-probe map-info <mapfile>
        What a ddrescue mapfile says: how much was read, how much never was.
  recovery-probe list-disks
        Show the disks this computer can see.
  recovery-probe disk-guard status | prepare | protect <disk> [--online] | restore
        Keep Windows from writing to the recovered disk: prepare (before plugging
        it in) makes Windows leave new disks switched off; protect marks the disk
        read-only (and --online switches it on); restore puts the setting back.
  recovery-probe ui [--port N] [--token KEY] [--no-browser] [--test-disk IMAGE]...
        The step-by-step page (what double-clicking starts).
  recovery-probe --version
";

fn run_ui(args: &[String]) -> i32 {
    use recovery_probe::web::{new_token, serve, Config, DEFAULT_PORT};
    let mut cfg = Config { port: DEFAULT_PORT, token: new_token(), open_browser: true, test_disks: Vec::new(), home: None };
    let mut i = 0;
    while i < args.len() {
        let val = args.get(i + 1).cloned();
        match (args[i].as_str(), val) {
            ("--port", Some(v)) => {
                cfg.port = v.parse().unwrap_or(DEFAULT_PORT);
                i += 1;
            }
            ("--token", Some(v)) => {
                cfg.token = v;
                i += 1;
            }
            ("--test-disk", Some(v)) => {
                cfg.test_disks.push(v);
                i += 1;
            }
            ("--home", Some(v)) => {
                cfg.home = Some(v.into());
                i += 1;
            }
            ("--no-browser", _) => cfg.open_browser = false,
            (other, _) => {
                eprintln!("unexpected argument {other}\n\n{USAGE}");
                return 2;
            }
        }
        i += 1;
    }
    serve(cfg)
}

fn parse_opts(args: &[String]) -> Result<(Option<String>, ScanOpts), String> {
    let mut opts = ScanOpts::default();
    let mut target = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--no-holes" => opts.do_hole = false,
            "--hole-kb" => {
                i += 1;
                let kb: u64 = args.get(i).and_then(|s| s.parse().ok()).ok_or("--hole-kb needs a whole number")?;
                opts.hole_bytes = kb.max(1) * 1024;
            }
            "--hole-bytes" => {
                i += 1;
                opts.hole_bytes = args.get(i).and_then(|s| s.parse().ok()).ok_or("--hole-bytes needs a whole number")?;
            }
            "--out" => {
                i += 1;
                opts.out = Some(args.get(i).cloned().ok_or("--out needs a file name")?);
            }
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s => {
                if target.is_some() {
                    return Err(format!("unexpected argument {s}"));
                }
                target = Some(s.to_string());
            }
        }
        i += 1;
    }
    Ok((target, opts))
}

struct MapOpts {
    map: String,
    earlier: Vec<String>,
    disk: String,
    out: String,
    status: Option<String>,
}

fn parse_map_opts(args: &[String]) -> Result<MapOpts, String> {
    let (mut map, mut disk, mut out, mut status) = (None, None, None, None);
    let mut earlier = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let val = || args.get(i + 1).cloned().ok_or_else(|| format!("{} needs a value", args[i]));
        match args[i].as_str() {
            "--map" => map = Some(val()?),
            "--earlier-map" => earlier.push(val()?),
            "--disk" => disk = Some(val()?),
            "--out" => out = Some(val()?),
            "--status" => status = Some(val()?),
            s => return Err(format!("unexpected argument {s}")),
        }
        i += 2;
    }
    Ok(MapOpts {
        map: map.ok_or("--map is needed: the ddrescue mapfile of the final attempt")?,
        earlier,
        disk: disk.ok_or("--disk is needed: the recovered disk, or a disk image")?,
        out: out.unwrap_or_else(|| "map-check".to_string()),
        status,
    })
}

type RescueArgs = (String, String, recovery_probe::rescue::Pick, bool, Option<String>, Option<u64>);

fn parse_rescue_opts(args: &[String]) -> Result<RescueArgs, String> {
    use recovery_probe::rescue::Pick;
    let (mut disk, mut out, mut status) = (None, None, None);
    let mut pick = Pick::Hidden;
    let mut paths = Vec::new();
    let mut lost = false;
    let mut stop_after = None;
    let mut i = 0;
    while i < args.len() {
        let val = || args.get(i + 1).cloned().ok_or_else(|| format!("{} needs a value", args[i]));
        match args[i].as_str() {
            "--disk" => {
                disk = Some(val()?);
                i += 1;
            }
            "--out" => {
                out = Some(val()?);
                i += 1;
            }
            "--path" => {
                paths.push(val()?);
                i += 1;
            }
            "--status" => {
                status = Some(val()?);
                i += 1;
            }
            "--stop-after" => {
                // (For the tests: stop after copying this many files.)
                stop_after = Some(val()?.parse().map_err(|_| "--stop-after needs a number".to_string())?);
                i += 1;
            }
            "--everything" => pick = Pick::Everything,
            "--lost" => lost = true,
            s => return Err(format!("unexpected argument {s}")),
        }
        i += 1;
    }
    if !paths.is_empty() {
        pick = Pick::Paths(paths);
    }
    Ok((
        disk.ok_or("--disk is needed: the recovered disk, or a disk image")?,
        out.ok_or("--out is needed: a folder on another drive")?,
        pick,
        lost,
        status,
        stop_after,
    ))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(|s| s.as_str()) {
        Some("--version") | Some("version") => {
            println!("{APP} {VERSION}");
            0
        }
        Some("scan") | Some("probe-jsonl") => match parse_opts(&args[1..]) {
            Ok((Some(target), opts)) => {
                if args[0] == "scan" {
                    run_scan(&target, &opts)
                } else {
                    run_probe_jsonl(&target, &opts)
                }
            }
            Ok((None, _)) => {
                eprintln!("Which folder? \n\n{USAGE}");
                2
            }
            Err(e) => {
                eprintln!("{e}\n\n{USAGE}");
                2
            }
        },
        Some("map-check") => match parse_map_opts(&args[1..]) {
            Ok(o) => run_map_check(&o.map, &o.earlier, &o.disk, &o.out, &Status::new(o.status)),
            Err(e) => {
                eprintln!("{e}\n\n{USAGE}");
                2
            }
        },
        Some("list-disks") => run_list_disks(),
        Some("disk-guard") => recovery_probe::cli::run_disk_guard(&args[1..]),
        Some("self-test") => recovery_probe::selftest::run_cli(),
        Some("ui") => run_ui(&args[1..]),
        // For the checks on GitHub: start a copy of this program the way a
        // double-click does (asking for administrator rights), and say
        // whether Windows did it.
        Some("check-start-elevated") => {
            let exe = std::env::current_exe().unwrap_or_default();
            let mark = std::env::temp_dir().join("recovery-probe-started.txt");
            let _ = std::fs::remove_file(&mark);
            match recovery_probe::win::start_elevated(&exe, &["write-mark".to_string(), mark.display().to_string()]) {
                Ok(()) => {
                    for _ in 0..100 {
                        if mark.exists() {
                            println!("started with administrator rights: the copy ran");
                            std::process::exit(0);
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    println!("Windows said it started the copy, but the copy did not run");
                    1
                }
                Err(e) => {
                    println!("could not start the copy: {e}");
                    1
                }
            }
        }
        Some("write-mark") => match args.get(1) {
            Some(p) => {
                let elevated = recovery_probe::win::is_elevated();
                let _ = std::fs::write(p, format!("elevated {elevated}\n"));
                0
            }
            None => 2,
        },
        None => recovery_probe::web::launch(),
        Some("rescue") => match parse_rescue_opts(&args[1..]) {
            Ok((disk, out, pick, lost, status, stop_after)) => run_rescue(&disk, &out, pick, lost, stop_after, &Status::new(status)),
            Err(e) => {
                eprintln!("{e}\n\n{USAGE}");
                2
            }
        },
        Some("image") => {
            let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
            match (get("--disk"), get("--map"), get("--out")) {
                (Some(d), Some(m), Some(o)) => recovery_probe::cli::run_image(&d, &m, &o),
                _ => {
                    eprintln!("image needs --disk, --map and --out\n\n{USAGE}");
                    2
                }
            }
        }
        Some("verify") => match args.get(1) {
            Some(f) => recovery_probe::cli::run_verify(f),
            None => {
                eprintln!("Which folder?\n\n{USAGE}");
                2
            }
        },
        Some(cmd @ ("isolate" | "evaluate")) => {
            let mut scans: Vec<String> = Vec::new();
            let mut folder: Option<String> = None;
            let mut i = 1;
            let mut bad = None;
            while i < args.len() {
                match args[i].as_str() {
                    "--scan" => match args.get(i + 1) {
                        Some(v) => {
                            scans.push(v.clone());
                            i += 1;
                        }
                        None => bad = Some("--scan needs a file".to_string()),
                    },
                    a if folder.is_none() && !a.starts_with("--") => folder = Some(a.to_string()),
                    a => bad = Some(format!("unexpected argument {a}")),
                }
                i += 1;
            }
            match (folder, bad) {
                (Some(f), None) if cmd == "isolate" && scans.is_empty() => {
                    let _ = f;
                    eprintln!("isolate needs --scan: the results of checking the copies (scan-....csv)\n\n{USAGE}");
                    2
                }
                (Some(f), None) => recovery_probe::cli::run_repair_step(cmd, &f, &scans, None),
                (_, Some(e)) => {
                    eprintln!("{e}\n\n{USAGE}");
                    2
                }
                (None, None) => {
                    eprintln!("Which folder?\n\n{USAGE}");
                    2
                }
            }
        }
        Some("put-back") => match args.get(1) {
            Some(f) => recovery_probe::cli::run_repair_step("put-back", f, &[], None),
            None => {
                eprintln!("Which folder?\n\n{USAGE}");
                2
            }
        },
        Some("undo-put-back") => match (args.get(1), args.get(2)) {
            (Some(f), Some(id)) => recovery_probe::cli::run_repair_step("undo", f, &[], Some(id)),
            _ => {
                eprintln!("undo-put-back needs the folder and the number (R0012)\n\n{USAGE}");
                2
            }
        },
        Some("sha256") => match args.get(1) {
            Some(f) => match recovery_probe::sha256::file_hex(std::path::Path::new(f), &|| false) {
                Ok(Some(h)) => {
                    println!("{h}  {f}");
                    0
                }
                Ok(None) => 1,
                Err(e) => {
                    println!("could not read {f}: {e}");
                    1
                }
            },
            None => {
                eprintln!("Which file?\n\n{USAGE}");
                2
            }
        },
        Some("map-info") => match args.get(1) {
            Some(p) => run_map_info(p),
            None => {
                eprintln!("Which mapfile?\n\n{USAGE}");
                2
            }
        },
        _ => {
            println!("{USAGE}");
            0
        }
    };
    std::process::exit(code);
}
