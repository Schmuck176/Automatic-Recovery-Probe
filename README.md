# Recovery Probe

A Windows program for getting files back off a ddrescue copy safely, and
finding every bit of damage in them — including the kind that opens fine and
looks normal.

**Double-click `recovery-probe.exe`.** It asks Windows for administrator
rights (to read the recovered disk directly; it never writes to it), and
opens a page in the PC's own web browser with two tabs:

- **The checklist** — the whole recovery, phase by phase
  (`docs/recovery-checklist.md`, built into the program), with tick boxes
  kept in `results\checklist-ticks.txt`. A bar at the top says what's next,
  when a checkpoint is due, or when to stop. Checkpoint boxes tick themselves
  when a safety check made after the boxes before them passes. The person's
  own copy of the checklist (in Obsidian) is kept the same, both ways, and
  never written over if they have added notes of their own.
- **The program's steps**, one at a time:

1. **Check the program works on this PC** — a built-in self-test on sample
   files and a small damaged test disk, where every right answer is known.
2. **The ddrescue mapfile** — found in the program's folder, and drawn as a
   map of the old drive with every place that was never read.
3. **Connect the recovered SSD, protected** — Windows is told to leave new
   disks switched off (until the SSD is unplugged again), and the SSD is
   marked read-only. Nothing, not even Windows itself, can write to it.
   Disks Windows calls "removable media" are shown as such: Windows' setting
   does not cover them.
4. **Make an image of the SSD** — the whole SSD read once into one file on
   another drive, with its SHA-256, read back to check it; places the SSD
   could not read are recorded the way ddrescue does. From then on the next
   steps read the image, and the SSD can go back in its drawer. The image
   can be checked again later against its SHA-256, without the SSD.
5. **See what the damage touched** — the *map check*: from the mapfile and
   the volume's own exFAT structures, exactly which files lost which bytes,
   which folders are damaged, which files Windows cannot even list, and the
   newest date on the volume (a date after the copy means something changed
   it). Folders whose own record was lost are found by their contents.
6. **Copy everything off** — straight from the image (or the disk), past
   Windows: also the files Windows cannot list, the folders found by their
   contents (in `found folders (name lost)`), and the data no folder names
   any more. Every copy's SHA-256 goes into `rescue-log.csv`; created,
   modified and accessed dates are kept. A *safety check* then compares the
   list of copies with what step 5 found on the SSD: the number of files and
   their total size.
7. **Check every copied file** — every file's structure (the *probe*),
   together with what step 5 found: a file the drive lost data from is
   marked **NEVER_READ** even when it looks perfect. And: are the copies
   still exactly what was copied (against `rescue-log.csv`)? Its safety check
   adds it up stage by stage — the files on the SSD, in the list of copies,
   and here now — so a file missing, changed or added is named at the stage
   where it went wrong. Every check goes into `results\safety-checks.csv`;
   run after every phase of the checklist, they show when anything changed.
8. **Repair the damaged files: isolate, repair, put back, evaluate** — the
   damaged copies are copied (never moved) into `REPAIR` beside the copies,
   sorted by what can repair them, each name tagged `[R0012]`;
   `repair-list.csv` records where each belongs and its SHA-256. Repaired
   versions saved in `REPAIR\repaired` go back to exactly that place, only if
   the file there is still the damaged original, which moves to
   `REPAIR\originals`; `repair-returns.csv` logs both checksums (the copies
   check and the scan know about it), and a put-back can be undone. What
   can't take the original's place (a TIFF made from a damaged RAW photo) is
   kept in `REPAIR\salvaged`, and the original stays as it is. Packages
   are never split: damaged media inside a library go back inside it, a
   document kept as a folder goes out and back whole, and a library's own
   database or an app is left to its own app (evaluated, not copied out).
   Put back, the one step that changes the copies, waits while the last
   safety check of them found something.
9. **Finish** — take the SSD out, and put Windows' setting back once it is
   unplugged.

Results are kept in a `results` folder next to the program. `docs/` has the
checklist the whole recovery follows, and the tools it uses.

## Versions and branches

- **`main`** is the version in `C:\RECOVERY-TOOLS` on the PC. It moves
  forward only when a new version has passed every test on GitHub, has been
  installed on the PC and checked byte for byte there, and its self-test has
  passed on the PC.
- **`dev`** is where changes are made; every change to it is built and
  tested on GitHub's Windows and Linux machines.
- **`ci-windows`** and **`ci-linux`** hold the latest test results and the
  tested program (only the latest: they are replaced at every build).
- When a newer version replaces it on the PC, the version it replaces is
  kept in a branch named after its version, such as `version-2026-10-20-1`
  (the program's page shows "Version 2026-10-20 (1)"). Every earlier state
  stays in `main`'s history too.

## What the checks say

| Verdict | Meaning |
|---|---|
| NEVER_READ | ddrescue never read part of the file (from the map check): those bytes are zeros or leftovers on the copy. |
| TRUNCATED | The file ends before its own format says it should, or its last disk sectors were never read. |
| HOLE | A block of zeros in the middle, shaped exactly like unreadable disk sectors. |
| MISMATCH | The content doesn't match the name, or the start of the file was never read. |
| DB_DAMAGED | A Photos Library database failed its integrity check. |
| UNREADABLE | The file couldn't be read at all. |
| LIBRARY | An Apple Photos Library (information, not a fault). |
| not testable | Nothing here can verify this type (HEIC and camera RAW always land here). |

The checks only ever read. They never change, move or delete a file. The only change the program ever makes to the copies is step 8's put-back, file by file, logged and undoable.

## Also from the command line

```
recovery-probe self-test
recovery-probe image --disk \\.\PhysicalDriveN --map <mapfile> --out <folder>
recovery-probe map-check --map <mapfile> --disk <image or \\.\PhysicalDriveN> --out <folder>
recovery-probe rescue --disk <image or \\.\PhysicalDriveN> --out <folder> --everything --lost
recovery-probe verify <folder the copies went into> [--map-check <map check results folder>]
recovery-probe isolate <folder the copies went into> --scan <scan results .csv>
recovery-probe put-back <folder the copies went into>
recovery-probe undo-put-back <folder the copies went into> R0012
recovery-probe evaluate <folder the copies went into> --scan <scan results .csv>
recovery-probe scan <folder>
recovery-probe sha256 <file>
recovery-probe disk-guard status | prepare | protect <disk> [--online] | restore
recovery-probe map-info <mapfile>
recovery-probe list-disks
```

## No third-party code

The program uses nothing but Rust's own standard library and Windows' own
parts: no code from anyone else runs over the recovered files. For Photos
Library databases it uses the SQLite that comes with Windows
(`winsqlite3.dll`).

## How it is tested

Every push is built and tested on GitHub's Windows and Linux machines
(`.github/workflows/build.yml`); the results land on the `ci-windows` and
`ci-linux` branches, with the Windows program itself.

- The file checks (the probe) were ported from a Python program and gave
  the same answer for every one of 1,425 test files, detail text included,
  on every run until 19 October 2026; that comparison and the Python
  original were then retired (they are in the repository's history). The
  built-in self-test checks them on intact and damaged samples of every
  kind (`tests/seeds`).
- The map check and the copying are tested on hundreds of damaged test disks
  made by `tools/exfat_image.py`, against an independent oracle
  (`tools/test_mapcheck.py`), against `fsck.exfat` and `dissect.fat`, and
  against the Linux kernel's exFAT driver.
- Against Windows itself: test disks attached as virtual disks, with what
  Windows lists and opens compared with what the map check predicts, record
  by record (`tools/windows_check.py`); and the disk protection, byte for
  byte (`tools/guard_check.py`).
- The page in a real browser, step by step, the checklist included
  (`tools/ui_check.js`).

The test tools are in `tools/` (see `tools/README.md`). They are Python on
purpose: a second, separate way of working out the right answers, so a
mistake in the program can't hide behind the same mistake in its test. None
of them is part of the program, and none is needed to run it.

## Building

```
cargo build --release
cargo test --release
target/release/recovery-probe self-test
python tools/test_mapcheck.py target/release/recovery-probe work --random 40
```
