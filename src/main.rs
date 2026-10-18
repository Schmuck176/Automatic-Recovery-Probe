//! Recovery Probe - the program you double-click.

use recovery_probe::cli::{
    run_list_disks, run_map_check, run_map_info, run_probe_jsonl, run_rescue, run_scan, ScanOpts, Status,
};
use recovery_probe::{APP, VERSION};

const USAGE: &str = "\
Recovery Probe - finds damage in files recovered with ddrescue.

Usage:
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
  recovery-probe map-info <mapfile>
        What a ddrescue mapfile says: how much was read, how much never was.
  recovery-probe list-disks
        Show the disks this computer can see.
  recovery-probe --version
";

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

type RescueArgs = (String, String, recovery_probe::rescue::Pick, bool, Option<String>);

fn parse_rescue_opts(args: &[String]) -> Result<RescueArgs, String> {
    use recovery_probe::rescue::Pick;
    let (mut disk, mut out, mut status) = (None, None, None);
    let mut pick = Pick::Hidden;
    let mut paths = Vec::new();
    let mut lost = false;
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
        Some("rescue") => match parse_rescue_opts(&args[1..]) {
            Ok((disk, out, pick, lost, status)) => run_rescue(&disk, &out, pick, lost, &Status::new(status)),
            Err(e) => {
                eprintln!("{e}\n\n{USAGE}");
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
