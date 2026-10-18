# Where the work stands (for picking up after a pause)

Work happens on the `dev` branch. Every push builds on GitHub; the results
land on the `ci-windows` and `ci-linux` branches (read them with
`git fetch origin +refs/heads/ci-windows:refs/remotes/origin/ci-windows` and
`git show origin/ci-windows:<file>`), because build logs themselves cannot be
reached from the development workspace.

## Done

- File checks (`scan`), ported from `reference/recovery_probe.py`; identical
  results to the Python version on every test file.
- Map check (`map-check`): which files and folders sit on the places ddrescue
  never read, from the user's mapfile and the recovered SSD read directly.
- Rescue (`rescue`): copies what Windows cannot list straight off the disk.
- Windows' handling of a folder with one damaged record (it gives up on the
  whole folder) measured on GitHub and modelled (`exfat.rs`, `Stop`).
- Linux kernel exFAT driver check working on GitHub.

## The user's real data (Phase 0)

- Final mapfile: "1TB hard drive DDRescue-GUI second attempt.log" (finished
  2024-04-24). 610,816 bytes never read, in 167 places, all between byte
  781,887,033,344 and 917,830,258,688 of the 1,000,204,886,016-byte disk.
- The other mapfile ("1TB hard drive DDRescue-GUI.log") is an aborted first
  attempt that stopped at ~331 GB; it read none of those places.
- The recovered SSD: GUID partition map, exFAT, probably labelled "FINAL 1 TB".

## Next

1. Double-click front end: a local page in the browser served by the program
   itself (std only), step by step: self-test, map check (asks for
   administrator rights via a separate elevated run), rescue, scan every file
   with the map check's results joined in, results in plain English.
2. Scan joined with the map check: a file the map check says lost real data is
   reported as damaged even when its structure looks fine.
3. Built-in self-test the user can run on their PC (fixture files + a small
   test disk image made by tools/exfat_image.py).
4. Put the .exe and the final mapfile into C:\RECOVERY-TOOLS on the user's PC.
5. Update recovery-checklist.md and download-manifest.md for the Rust tool.
