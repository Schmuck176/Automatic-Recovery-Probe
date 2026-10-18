# Independent review brief

You are asked to review this project with fresh eyes. It was written by
another Claude session over several days, for a person who is not technical.
They have one chance at this: the files involved are the only copy of years
of their photos and documents. Look for anything that could harm that copy,
lose or mislabel files, or make them redo the long process. Be blunt.

## The situation

- A 1 TB hard drive died. GNU ddrescue (on a Mac) copied it sector for
  sector onto a 2 TB SSD (the "recovered SSD"). 610,816 bytes in 167 places
  could never be read; all lie between byte 781,887,033,344 and
  917,830,258,688 of the 1,000,204,886,016-byte disk. The old drive is gone,
  so **the recovered SSD is the only copy**. It holds the old drive's GPT
  partition table and one exFAT volume made by macOS (probably labelled
  "FINAL 1 TB"), so the GPT's backup header points at the old 1 TB disk's end.
- The person has a Windows 11 PC ("biggamer"), an empty 2 TB SSD formatted
  NTFS and named SPARE, and a Mac for one later step.
- The plan is `docs/recovery-checklist.md` (the person's own copy, with ticked
  boxes, lives on their PC in `C:\RECOVERY-TOOLS\Obsidian MD files\`).
  Phase 1 copies everything off the recovered SSD onto SPARE with this
  program; later phases check every copy with this program, Krokiet, 7-Zip,
  MP3val and the Mac's `sips`, then sort and archive.
- The person's mapfile is not in this repository (personal data); the summary
  above is everything the program needs to know about it.

## The rules (must never be broken)

1. Nothing ever writes to the recovered SSD. It is never formatted,
   "repaired", or plugged in while the PC restarts. Windows itself must not
   be given the chance to write to it either (it creates "System Volume
   Information" and marks volumes dirty on its own).
2. Never point a duplicate finder at an Apple Photos Library; never take
   files out of a library (or, the person has just pointed out, out of any
   macOS package: .app, .fcpbundle, .logicx, .pages, .numbers, .key, and many
   more - see "Open questions").
3. Nothing personal goes into this repository.
4. Deleting anything on the person's computer needs their say-so.

## What the program is

`recovery-probe.exe`, Rust, standard library only (no crates), ~12,600 lines
in `src/`, plus ~4,700 lines of test tools in `tools/`. Double-clicking it
asks Windows for administrator rights and opens a local web page
(`src/web.rs`, `src/ui.html`, 127.0.0.1 with a random key) with seven steps:

1. Self-test (`src/selftest.rs`): sample files and a small damaged exFAT disk
   built into the program (`src/selftest/`, made by `tools/make_selftest_disk.py`).
2. The mapfile (`src/mapfile.rs`).
3. Connect the SSD protected (`src/guard.rs`): `diskpart san policy=OfflineAll`
   before it is plugged in, so it arrives offline; then
   `IOCTL_DISK_SET_DISK_ATTRIBUTES` read-only (persisted) and the policy put
   back. While steps 4 and 5 run the policy is OfflineAll again
   (`web.rs keep_new_disks_off`), so a SSD that drops off and comes back
   arrives offline. Raw reads of `\\.\PhysicalDriveN` work while offline.
4. Map check (`src/mapcheck.rs`, `src/exfat.rs`, `src/gpt.rs`): which files and
   folders sit on never-read places, and which files Windows cannot list
   (Windows' folder-listing behaviour on damaged records was measured on
   GitHub's Windows machines and modelled: see the `Stop` comment in exfat.rs).
5. Copy everything (`src/rescue.rs`): straight off the raw disk, past Windows,
   into SPARE\WORK: also files Windows cannot list, and allocated clusters no
   file claims ("lost data (no name)"). Copy-then-rename, resumable, stops
   cleanly if the SSD or SPARE goes away, copies left by an earlier run are
   checked (start and end) against the disk before being kept, one list
   (`rescue-log.csv`) across runs.
6. Check every copied file (`src/probe.rs`, `src/checks.rs`, `src/holes.rs`,
   `src/walk.rs`, ported from `reference/recovery_probe.py` and compared with
   it file by file), joined with step 4 (`src/join.rs`): a file the drive lost
   data from is NEVER_READ even if it looks fine.
7. Finish.

The only code that opens a disk for writing is `guard.rs` `open(n, true)`,
used for the disk-attribute IOCTLs. Everything else opens disks read-only
(`disk.rs`). Please check that claim.

## How it is tested

Every push builds and tests on GitHub (`.github/workflows/build.yml`). Build
logs cannot be read from the development workspace, so results are published
to branches: `git fetch origin +refs/heads/ci-windows:refs/remotes/origin/ci-windows`
then `git show origin/ci-windows:<file>` (files `25-self-test.txt`,
`40-map-check-tests.txt`, `50-windows-disks.txt`, `60-disk-protection.txt`,
`65-unplugged.txt`, `70-page.txt`, `BUILD-INFO.txt`, and the program itself).

- `cargo test --release` (unit tests, including a disk that stops answering
  mid-copy, stopping and starting again, a power cut leaving zeroed copies).
- `tools/test_mapcheck.py`: random damaged exFAT disks from
  `tools/exfat_image.py` (written from the spec), checked against an
  independent oracle, `fsck.exfat`, `dissect.fat`; every disk is copied whole
  and in several stopped runs and must come out identical.
- `tools/windows_check.py`: virtual disks attached to Windows; what Windows
  lists and opens compared with the model, record by record.
- `tools/guard_check.py`: the disk protection on virtual disks, byte for byte.
- `tools/dropout_check.py`: the disk unplugged mid-copy, through the page.
- `tools/ui_check.js`: the page in a real browser (Playwright).
- `tools/kernel_check.py` (Linux): against the kernel's exFAT driver.
- `tools/make_big_disk.py`: a million-file disk for timing.

Latest results (commit 05717a4, delivered to the PC as version
"2026-10-18 (2)"): every check green; see NEXT-STEPS.md.

## What to review, most important first

1. **Can anything write to the recovered SSD?** `guard.rs`, the `guard-*`
   actions in `web.rs` `act()`, `disk.rs`, the step 3 code in `ui.html`.
   Think about real USB hardware, not just virtual disks: enclosures that
   report removable media (SAN policy does not apply), 4 KB-sector bridges,
   the SSD dropping off and coming back, a crash or power cut with it
   plugged in, Windows remembering (or not) the read-only mark.
2. **Are the copies exactly right?** `rescue.rs`, `exfat.rs`: exFAT details a
   Mac-made volume may have that the synthetic test disks do not (NoFatChain
   folders, vendor entries, valid data length, names in decomposed Unicode,
   very large folders, files over 4 GB).
3. **Does step 4 name every damaged file and nothing else?** `mapcheck.rs`,
   `join.rs` (the join finds copies by name, size and shared path tail).
4. **Is the plan sound?** `docs/recovery-checklist.md`, especially the
   person's new point below.
5. Anything a better tool does better (see below).

## Open questions (the person raised these on 18 October)

- **macOS packages.** Phase 2 trashes files with Krokiet (empty files,
  temporary files, duplicates) and Phase 6 moves files into GOOD / BROKEN /
  UNSURE folders. Both can split packages (`.app`, `.fcpbundle`, `.logicx`,
  `.pages`, `.numbers`, `.key`, `.sparsebundle`, `.imovielibrary`, `.band`,
  `.rtfd`, `.bundle`, `.framework`, ...) and only Photos Libraries are
  protected today. The person proposes: (1) isolate damaged files, (2) repair
  them, (3) return them exactly where they came from, (4) evaluate packages
  left with unrepairable files. The current lean: never move or delete
  anything in WORK until the very end; isolate by *copying* into a REPAIR
  folder with a manifest; return verified against the manifest; treat a
  package as one unit; sort by report instead of by folders. Please weigh in.
- **Housekeeping files** (`.DS_Store`, `._*`, `Thumbs.db`, `desktop.ini`):
  `._*` files can hold the only copy of a classic Mac file's resource fork,
  and `Thumbs.db` can hold previews of photos that are now damaged.
- **Better tools.** UFS Explorer Professional (commercial) can load ddrescue
  maps and flag files with unread blocks; `ddru_findbad` (ddrutility, Linux)
  maps ddrescue bad sectors to files; a hardware write blocker would make
  step 3 unnecessary. Is there anything the person should use instead of, or
  alongside, this program?

## How to report

Do not change anything on the person's computer, and do not ask them to plug
in the recovered SSD. Work on a branch of your own (for example `review`),
write your findings to `REVIEW-FINDINGS.md` there (most serious first, each
with where it is and what would happen), and summarise them for the person in
plain words: they are not technical. Fix things only if they agree. Note that
the repository is private, so GitHub Actions minutes are limited (Windows
minutes count double): batch any test runs.
