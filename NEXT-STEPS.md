# Where the work stands (for picking up after a pause)

## What the user asked for (23 Oct 2026, 14:51 BST)

A new program, Automatic-Recovery-Probe, built from Recovery-Probe, that
automates "messing about with open source software to fix things": get ready,
plug in the SSDs, point at the folders on the PC it works in, press Start, and
every damaged file is found and fixed as far as open-source tools can; what
can't be fixed the user may send to paid software and/or the Mac, then bring
the repaired files back for finalisation. Automate the remaining checklist
steps "wherever it is REASONABLY feasible". Use the tools' open-source GitHub
versions rather than the GUI programs. The user has Claude Pro: schedule
automatic resumes for usage pauses (send_later into this session; delete them
when done).

The user's answers (23 Oct, 15:05):
- "Backup locations" = the folders on the PC the program works in (image,
  copies, second copy). An optional box for folders holding older copies to
  search for intact originals is fine, not required.
- Repairs: **put everything back automatically** (still logged, original kept,
  undoable).
- **Wait for the new program** before making the image (the rehearsal and the
  image become part of Start). Nothing on the real drives has been touched:
  Phase 1 of the old checklist was not started.

## Conventions (as in Recovery-Probe)

- Work on `dev`; every push to dev builds on GitHub (Windows and Linux) and
  publishes to `ci-windows` / `ci-linux` (read with `git fetch origin
  +refs/heads/ci-windows:refs/remotes/origin/ci-windows`). `main` = the
  version on the PC: moved only once a version is installed in
  C:\RECOVERY-TOOLS and checked byte for byte there; the version it replaces
  is kept in a branch `version-YYYY-MM-DD-N`. No tags (the session's GitHub
  access cut tag pushes off).
- Private repo: Actions minutes are limited, Windows counts double. Batch
  changes, push with `[skip ci]` when there is nothing new to build, cancel
  superseded runs.
- Deliver to the PC under a fresh staged name, then stage back and compare
  the SHA-256 (a reused /mnt/user-data/outputs name once delivered an old file).
  The exe is locked while it runs: delivery fails while the program is open.
- Commits end with the Co-Authored-By and Claude-Session lines.

## The workspace (what works here)

- No Windows target: static.rust-lang.org and crates.io are blocked, so
  Windows-only code is checked by GitHub's Windows run only. Keep it small and
  in few places (win.rs, guard.rs, tools.rs's Windows parts).
- GitHub release pages/assets cannot be fetched from the shell (403); WebFetch
  can read pages. Git clones of public repos work.
- Here: ffmpeg, qpdf, ImageMagick 6 (`convert`), zip/unzip. Not here: 7z,
  mp3val, untrunc (the Linux CI run installs them with apt / builds untrunc).

## The PC ("biggamer", Windows, user's own account)

- C:\RECOVERY-TOOLS: recovery-probe.exe (2026-10-22 (1)), the final mapfile
  "1TB hard drive DDRescue-GUI second attempt.log", results\ (state, ticks:
  31 ticked), "Obsidian MD files\" (the old checklist and manifest, synced by
  Recovery-Probe), "installed files\" (7z2603-x64.exe installed, ffmpeg
  9.0.1 essentials zip -> C:\ffmpeg (on PATH), mp3val-0.1.8 (mp3val.exe),
  untrunc_x64 (untrunc.exe, untrunc-gui.exe), Krokiet 12.0.1, MultiPar,
  AIMtk\AIMtk20260420 (install.bat -> config.exe from files.cab: MountImg.exe,
  aim_cli.exe, AIMtk-Dlg.exe, phdskmnt.sys); whether AIM is installed is not
  known).
- Drives: the recovered SSD (2 TB, GPT, exFAT "FINAL 1 TB", copy of the 1 TB
  drive at its start), a 4 TB SSD with a ddrescue copy of the 2 TB SSD (never
  plugged in with it), SPARE (1 TB NTFS, empty, for WORK and REPAIR), 2 TB
  free on the PC's internal drive (IMAGE, later COPY 2).

## The design

- Same crate, renamed `automatic-recovery-probe` (APP "Automatic Recovery
  Probe", port 47116 so Recovery Probe (47115) can run too). It shares
  Recovery-Probe's results folder and state file (recovery-probe-state.txt),
  so either program carries on where the other stopped; its own checklist is
  a separate file (automatic-recovery-checklist.md, ticks in
  automatic-checklist-ticks.txt, settings auto_checklist_copy_*), and it
  never touches the old checklist or manifest in Obsidian.
- New: tools.rs (find/run/download the open-source tools), auto.rs (the
  Start pipeline, resumable, waits for drives), extra.rs (read-only checks
  with 7z/mp3val/ffmpeg/qpdf -> REPAIR\damaged-lists), autorepair.rs
  (twin restore, per-format repairs, judged by re-checking), handoff.rs
  (paid tools / Mac script / Bring back), finalise.rs (RECORDS, verified
  COPY 2, final checks), an "Automatic" tab on the page.

## Plan and progress

- [x] Repo set up from Recovery-Probe's whole history (main = the foundation,
      Recovery-Probe 2026-10-22 (1) plus this README); renamed; tests pass.
- [x] tools.rs (find/run, qpdf download with pinned SHA-256 - not yet tried on
      Windows: CI and the PC will tell).
- [x] web/auto.rs: Start's stages, resumable, waits for the SSD / SPARE,
      protects the SSD, AIM mount (aim.rs; syntax from Arsenal's
      ConsoleAppImplementation.cs: --mount --readonly --online --filename=
      --background; --dismount=NNNNNN --force), stops on the map check's red
      flags. The Automatic tab in ui.html.
- [x] extra.rs, autorepair.rs, handoff.rs (Mac script, Bring back),
      web/finalise.rs (RECORDS, COPY 2, both checks, image check, settings).
- [x] New checklist docs/automatic-checklist.md (30 boxes) and
      docs/automatic-manifest.md, built in under their own names
      (automatic-recovery-checklist.md, automatic-download-manifest.md,
      results\automatic-checklist-ticks.txt, settings auto_checklist_copy_*);
      Recovery Probe's own checklist copy in Obsidian is never touched.
- [x] Tests here (Linux): unit 99, self-test, tools/auto_check.py 48/48 (FFmpeg
      and qpdf here), tools/auto_ui_check.js 14/14, tools/ui_check.js 64/64.
- [x] CI run 39978253835 (750bb9f): Windows compiled the Windows-only code
      and passed unit tests, self-test, map check tests, Windows checks,
      protection, unplugged; Linux with every tool (7-Zip 23.01, FFmpeg 6.1.1,
      MP3val 0.1.8, qpdf 11.9.0, untrunc v1-9d86ec9 built from source) passed
      auto_check 50/51: untrunc rebuilt the MOV with IMG_0001.MOV as pattern,
      MP3val the MP3, qpdf the PDF (new trailer); the docx miss was the test's
      own damage cutting into the only member (now a many-part docx whose
      index fills whole sectors).
- [x] After that run: tools get a short-named working copy (input.<ext>);
      extra checks remember what they checked (results\extra-checks-done.tsv)
      so a stopped run carries on; a check flagging >30% (and >=30) of one
      kind is set aside as the tool's limit, not damage; the self-test checks
      the repairs needing no tool; Defender exclusion button; the first copy
      of the checklist goes beside Recovery Probe's in Obsidian; the copy
      stage says where SPARE is if its letter changed.
- [x] Run 39978253835 also showed on Windows (48/49) and Linux that 7-Zip
      always reports "Headers Error" (exit code 2) for a zip whose index is
      lost, so the rebuild was always called a salvage. Now (138b5e5) a
      rebuild counts as repaired when 7-Zip names no part as failed (its
      "ERROR: CRC Failed : <part>" lines; its errors about the zip as a whole
      don't count) and the zip's own local headers, walked from the start
      (autorepair.rs zip_walk), lead to the index with none missing.
      Read from 7-Zip 26.05's source (ZipIn.cpp LOCALS-CD-MODE,
      ExtractCallbackConsole.cpp): building it here was not allowed.
- [x] Windows CI now also uses untrunc's own Windows build (anthwlock
      release "latest", the PC has untrunc_x64 with FFmpeg 3.x DLLs) and
      MP3val 0.1.8 for Windows, and tools/download_check.py downloads qpdf
      for real (0326a6e).
- [ ] Deliver to C:\RECOVERY-TOOLS (automatic-recovery-probe.exe beside
      recovery-probe.exe; qpdf from the Download button or delivered into
      C:\RECOVERY-TOOLS\tools\qpdf-12.3.2), check byte for byte, the
      user's self-test and Get ready on the PC.

## Not done / ideas for later

- Two damaged copies of the same file whose lost places differ could be
  merged into one whole file (autorepair.rs twin logic, v2).
- PNG repairs beyond re-encoding what FFmpeg decodes; GIF/TIFF/WebP.
- HEIC salvage on Windows with FFmpeg 8.1+ (as JPEG), before the Mac.
- A Windows scheduled reminder every six months (the checklist says to set a
  phone reminder instead).
