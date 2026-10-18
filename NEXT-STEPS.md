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

## Pre-flight audit (in progress, started 2026-10-18 15:30 BST)

The user asked, before plugging in the recovered SSD: is the program right,
and will it cause no problems, since the whole process should be done once.
They are waiting for Claude's answer before plugging in. "Take all the time
you need; pause and resume automatically if usage runs out."

Checks (tick here as they are done, push after each). Fallback scheduled
continuations: trig_01H6LBYNesZY4pPuLF4zde73 (19:32 UTC), trig_01KDwd27r7k2sPmmoDbfoNzm
(00:32 UTC): delete both when done.
- [ ] 1. Nothing can write to the SSD (disk handles, IOCTLs, every page step).
- [ ] 2. Copying right for a Mac-made exFAT (runs, NoFatChain, valid data
      length, >4 GB, timestamps, NFD names, forbidden characters, long paths).
- [x] 3. Long copy: stops cleanly when the SSD stops answering (disk.rs
      Watched: a failed read re-reads the boot sector; gone -> stop, nothing
      listed as failed; also between folders), when the copies' folder goes
      away, or after 25 failures in a row. rescue-log.csv is merged across
      runs (Earlier/RunLog in rescue.rs; rescue-log-unfinished.csv written as
      it goes, folded in by the next run after a crash). Tested: unit tests
      (unplugged mid-copy, stop/again, crash), and test_mapcheck.py
      check_stop_and_again (51 disks: same copies and list as one run).
- [x] 5. Join: damaged files found under safe names and " (2)" names
      (join.rs copy_name/without_number); PHOTOSLIB\N works by the shared
      tail of the path (unit test copies_whose_names_were_changed_are_found_too).
- [x] 4. Scale (before the changes): 1M files on Linux: map check 0.6 s /
      97 MB, rescue 33 s / 94 MB, scan 39 s / 59 MB (tools/make_big_disk.py).
- [x] 6. Disk protection with a real USB disk: while steps 4/5 run the policy
      is OfflineAll again (web.rs keep_new_disks_off; kept after the SSD
      stopped answering), because Windows forgets a re-plugged virtual
      disk's read-only mark (guard_check.py C). guard-online refuses to
      switch on a could-be-SSD disk unless read-only; guard-release switches
      SPARE on (refused for could-be-SSD disks). Page chooses the copy by
      itself. tools/dropout_check.py tests unplugging mid-copy on Windows CI.
      4 KB bridges: GPT is looked for at 512 and 4096, reads are 4 KB aligned.
      Removable-media disks cannot be switched off: Protect would show an
      error; the checklist says to unplug and ask.
- [x] Extra: copies an earlier run left are checked (start and end) against
      the disk before being kept (a power cut can leave zeros); the scan stops
      when the checked folder goes away (walk.rs GONE); the list is kept on
      disk, not in memory (1M files: ~100 MB on a first run).
- [ ] 7. CI green, deliver the exe (only while the program is closed: ask).
- [ ] 8. Walk Phase 1 against the program; update checklist; answer the user.

## Next

1. The user runs Phase -1 E (double-click, Yes, step 1 self-test), then
   Phase 1 when ready to plug in the SSD. Help if anything on their PC
   differs from GitHub's Windows machine.
2. When the user plugs the SSD in: help read step 4's results (map check).
3. Phase 6 sorting helper (the checklist promises one): reads the scan CSVs,
   rescue-log.csv and sips-failures.txt, moves files from WORK into GOOD /
   BROKEN / UNSURE with a preview, never touching PHOTOSLIB.
