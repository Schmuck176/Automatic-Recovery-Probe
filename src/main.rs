//! Recovery Probe - the program you double-click.

use recovery_probe::cli::{run_probe_jsonl, run_scan, ScanOpts};
use recovery_probe::{APP, VERSION};

const USAGE: &str = "\
Recovery Probe - finds damage in files recovered with ddrescue.

Usage:
  recovery-probe scan <folder> [--out results.csv] [--hole-kb N] [--no-holes]
        Check every file in a folder and write the results CSV files.
  recovery-probe probe-jsonl <folder> [--hole-kb N] [--no-holes]
        One line of JSON per file (used to compare with the Python version).
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
        _ => {
            println!("{USAGE}");
            0
        }
    };
    std::process::exit(code);
}
