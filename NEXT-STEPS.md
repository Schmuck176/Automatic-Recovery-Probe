# Where the work stands (for picking up after a pause)

Work happens on the `dev` branch. Every push builds on GitHub; the results
land on the `ci-windows` and `ci-linux` branches (read them with
`git fetch origin +refs/heads/ci-windows:refs/remotes/origin/ci-windows` and
`git show origin/ci-windows:<file>`), because build logs themselves cannot be
reached from the development workspace. The Windows build of each commit is
`recovery-probe.exe` on the `ci-windows` branch.

## Done

- File checks (`scan`), ported from `reference/recovery_probe.py`; identical
  results to the Python version on every test file.
- Map check (`map-check`): which files and folders sit on the places ddrescue
  never read, from the user's mapfile and the recovered SSD read directly.
- Rescue (`rescue`): copies everything (or what Windows cannot list) straight
  off the disk; names Windows cannot use get an underscore; copy-then-rename;
  resumable; stop switch; space check.
- Windows' handling of damaged folder records measured on GitHub and modelled
  (`exfat.rs`, `Stop`; the oracle in `tools/test_mapcheck.py`): a blank record
  ends the folder; only a well-formed record with a wrong checksum makes
  Windows give up on the whole folder (files before it still open by full
  path); malformed records are skipped with every slot they claim; anything
  else is skipped. 18 single-record experiments and 12 random disks agree.
- Disk guard (`guard.rs`, `disk-guard`): SAN policy OfflineAll before the SSD
  is plugged in, read-only mark, policy put back. Tested on Windows with
  virtual disks byte for byte (`tools/guard_check.py`). Windows does not
  remember the read-only mark for virtual disks: use step 3 every time.
- Double-click front end (`web.rs`, `ui.html`): local page, one UAC prompt,
  steps: self-test, mapfile, connect protected, map check, copy everything,
  check every copied file (joined with the map check: NEVER_READ), finish.
  Browser-tested on Linux and Windows in CI (`tools/ui_check.js`).
- Built-in self-test (`selftest.rs`, `recovery-probe self-test`), with a
  small damaged exFAT disk made by `tools/make_selftest_disk.py`.
- `docs/recovery-checklist.md` and `docs/download-manifest.md` rewritten for
  the program (version 2026-10-18).

## The user's real data (Phase 0)

- Final mapfile: "1TB hard drive DDRescue-GUI second attempt.log" (finished
  2024-04-24). 610,816 bytes never read, in 167 places, all between byte
  781,887,033,344 and 917,830,258,688 of the 1,000,204,886,016-byte disk.
- The other mapfile ("1TB hard drive DDRescue-GUI.log") is an aborted first
  attempt that stopped at ~331 GB; it read none of those places.
- The recovered SSD: 2 TB, GUID partition map, exFAT, probably labelled
  "FINAL 1 TB". The copy sits at the start of it (partition table made for
  the 1 TB disk).
- The mapfiles are in "C:\RECOVERY-TOOLS\My DDRESCUE map files and
  screenshots.zip" on the user's PC (never in this repository).

## Delivered

- 2026-10-18 ~07:00 UTC: the Windows build of 4efff72 (every check green on
  GitHub's Windows machine), the final mapfile, and docs/recovery-checklist.md
  and docs/download-manifest.md were written into C:\RECOVERY-TOOLS on the
  user's PC ("biggamer"). The older Python files are still there (harmless).
- 2026-10-18 14:16 BST: the user ran step 1 on the PC: every check passed
  (screenshot and results\recovery-probe-state.txt agree). The user then
  reorganised C:\RECOVERY-TOOLS: recovery-probe.exe, the final mapfile and
  results\ stay at the top; the checklist and manifest now live in
  "Obsidian MD files\" (an Obsidian vault, boxes being ticked: edit that copy in
  place, keep the ticks, guard with the staged mtime); installers, Krokiet,
  MP3val and untrunc in "installed files\"; the Python files in
  "Old Python Program\". Phase -1 B (all but services.msc), C, D, E (first
  three) and Phase 1 steps 1-5 (SPARE ready) are ticked.
- The Claude app runs in an account with OneDrive and OBS folders, so the
  user set up in their usual account, not Transfer. The checklist now says to
  stay in it (OneDrive cannot reach C:\RECOVERY-TOOLS or SPARE; the ffmpeg
  Path, Obsidian and the Claude app are per account) and adds: AutoPlay off,
  "Automatically restart" off (a crash then stops at the blue screen instead
  of rebooting with the SSD attached), both undone in Phase 9.
- Not delivered yet (harmless, cosmetic): the mapfile chip on the page reads
  "bad sector: 167 places, 610,816 bytes" instead of "167 bad sector (...)".

## Next

1. The user runs Phase -1 E (double-click, Yes, step 1 self-test), then
   Phase 1 when ready to plug in the SSD. Help if anything on their PC
   differs from GitHub's Windows machine.
2. When the user plugs the SSD in: help read step 4's results (map check).
3. Phase 6 sorting helper (the checklist promises one): reads the scan CSVs,
   rescue-log.csv and sips-failures.txt, moves files from WORK into GOOD /
   BROKEN / UNSURE with a preview, never touching PHOTOSLIB.
