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
- GitHub release assets could be fetched from the shell on 23 Oct (qpdf's
  Linux builds, to test qpdf 12 here); PyPI could not. Git clones of public
  repos work; building code cloned from elsewhere was not allowed.
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

## Done (23 Oct 2026)

Built and tested (unit tests, self-test, tools/auto_check.py on a damaged disk
of real media, the page tests, on GitHub's Windows machine with the PC's own
tool builds and on Linux), then delivered: version 2026-10-23 (1), CI run
40016273279, C:\RECOVERY-TOOLS\automatic-recovery-probe.exe, SHA-256
1d7dda80efeb5e09078b682f14af730ec327c91ff1fe5483129e783993ecdd04; main moved
to it (the foundation is kept as branch version-2026-10-22-1). Things learnt
on the way, kept in the code's comments: 7-Zip always reports "Headers Error"
for a zip whose index is lost (autorepair.rs zip_walk judges the rebuild);
qpdf 12 rebuilds a PDF whose end was lost without a trailer but leaves the
trailer without /Size (fix_pdf also tries a new trailer); Windows keeps port
ranges for itself (tests take a system-given port; the program notes a
fallback port in %TEMP%).

## 24 Oct 2026: the user's PC, and a review

On the PC (11:07-11:08 BST): Check this PC, the self-test (passed, with the
repairs needing no tool), Install AIM Toolkit (C:\Program Files\AIM Toolkit),
Download qpdf (12.3.2, checked) - all done by the user. Then asked for a bug
hunt and a clean-up of the repository. Found and fixed:

- The evaluation counted only what the latest check flagged: after the
  repairs it said "7 damaged, 5 put back" where the repairs said 9 and 7.
  Every file copied out is now counted.
- AIM's mount was left open if the program closed between "open" and the
  map check (the device number is now kept in the settings and closed next
  time), a Stop while waiting for the drive letter marked "open" skipped for
  good, and a map check carried on after a stop ran without the comparison
  with Windows (now "open" runs again first).
- The repairs' log was written only at the end of the stage: a power cut lost
  it, and a salvage made before the cut then failed as "already there". Each
  attempt is logged at once; a salvage already there is kept.
- Files the old drive gave up whole (no never-read place, per the map check)
  and found only by a checking tool: the tool's complaint was there before
  the failure (or is the tool's limit). They are still tried, but a tool's
  version is kept in REPAIR\salvaged (not put in their place), they are never
  handed off, and the reports say so. The doubt rule weighs only those files
  (at least 10 and over 3% of a kind: the tool's limit). FFmpeg's complaints
  about a photo's EXIF, ICC or APP data are not damage. 7-Zip's exit code 0
  is trusted over words in the archive's path ("Error reports\x.zip").
- The extra checks reused an old verdict for a file put back with its
  original's size and dates (an intact twin): those are checked again.
- The second copy skipped a put-back file with its original's size and dates:
  put-back files are now compared by contents.
- MP3val and qpdf (older Windows programs) can't open paths with letters
  outside ASCII, or long ones: the extra checks give them a short-named copy
  (auto_check.py has an intact MP3 named in three alphabets and a PDF at the
  end of a 300-character path).
- Get ready: "Try it" for AIM Toolkit (opens the self-test image read-only,
  checks Windows reads it and it can't be written, closes it; aim_cli's
  options and messages checked against C:\Program Files\AIM Toolkit\aim_cli.exe
  on the PC: Arsenal's .NET aim_cli, --background via --detach and a READY
  token, device numbers in hex); Defender exclusions already there are shown
  (the user excluded SPARE by hand in the old Phase -1, on 19 Oct, with the
  rest of Phase -1: updates paused, Automatically restart off, power and
  sleep settings, a separate "Transfer" account).
- tools/resume_check.py: Start stopped and killed at random in every stage,
  carried on each time, must end as one unbroken run (CI, both systems).
- Clean-up: unused test seeds and Recovery Probe's own docs removed (its
  README kept for the steps tab, with a note); .gitignore and tools/README
  brought up to date; builds skipped for notes-only changes.
- CI run 40088888927 (9fc5ba9), all green on both systems (Windows: unit
  tests 104, automatic 54/54 with the PC's tool builds, resume 17/17 with
  stops and kills in ready, mapcheck, extra, isolate and repair).
- Delivered 24 Oct 2026, 12:38 BST: version 2026-10-24 (1),
  C:\RECOVERY-TOOLS\automatic-recovery-probe.exe, 5,320,192 bytes, SHA-256
  2184ecf0a7e591b032d9871e24546d99a9bc57f4a0c49f5a20ea49d034c5fcb5 (staged
  back and compared). main moved to it; the 2026-10-23 (1) main is kept as
  branch version-2026-10-23-1.
- [ ] On the PC (the user): open the new version, Check this PC, "Try it"
  for AIM Toolkit, then Phase 2 (the rehearsal with SPARE, the folders).

## 25 Oct 2026: the rehearsal on the PC, and file dates

- The rehearsal on the PC (10:46 BST): SPARE arrived switched on, shown as
  "Realtek RTL9210 NVME" - the USB case's chip (version 2026-10-24 (1) asked
  the user whether that was SPARE's own name). Windows often knows a disk in
  a USB case by the case's identity, so the recovered SSD in it could have
  arrived switched on. Now: Switch SPARE off (offline and read-only, both
  remembered: IOCTL_DISK_SET_DISK_ATTRIBUTES with Persist), unplug (watched
  for every second: the check refuses until SPARE has been seen gone), plug
  in again, check: switched off = passed (the recovered SSD in that case
  arrives switched off either as SPARE's remembered identity or as a new
  disk under Windows' setting); switched on again = stop. SPARE stays
  switched off after the rehearsal (Start's copy stage switches it on,
  remembered, once the image is made and Windows' setting put back). Only a
  disk labelled SPARE, or the switched-off disk the rehearsal saw, can be
  switched off. The old "ask" verdict asks for the check again.
- The folders: the image drive suggested must have room (mapfile size + 1
  GiB): C: on the PC, D: is too small. SPARE's letter is remembered from the
  rehearsal (SPARE is switched off at the folders step).
- The user asked whether anything changes files' created or modified dates
  (to sort the files by date afterwards). The copies get the old drive's
  (rescue, from the exFAT records); put-back gives a repaired file its
  original's (as before) and now its folder its own back, however it ends
  (KeepFolderDates; undo too); a package's files get theirs; what the
  repairs make and what Bring back takes in get the original's dates in
  REPAIR; COPY 2 keeps all. Inside files: untrunc copies the pattern's moov
  (mvhd/tkhd/mdhd dates, udta/meta text dates - seen in untrunc's source),
  so mp4time.rs moves them to the video's own (its file date, turned into
  the index's moment the way the pattern turns its own, to the quarter
  hour); FFmpeg's remux of MP4/MOV keeps every tag (-movflags
  use_metadata_tags); a PDF given a new trailer points at its /Info; a PNG
  saved again by FFmpeg gets back its tIME/tEXt/zTXt/iTXt/eXIf (and colour
  chunks and pHYs if FFmpeg's has none).
- Found on the way: pdf_with_trailer sliced a lossy-UTF-8 copy of the PDF
  at byte offsets, which panics when a slice ends inside a multi-byte
  character (binary streams): it works on bytes now.
- auto_check.py: the videos' made dates are an hour before their dates on
  the card; it checks the dates of put-back files, their folders, salvaged
  files, COPY 2, and the rebuilt video's made date (with untrunc: CI).

## Not done / ideas for later

- Two damaged copies of the same file whose lost places differ could be
  merged into one whole file (autorepair.rs twin logic, v2).
- PNG repairs beyond re-encoding what FFmpeg decodes; GIF/TIFF/WebP.
- HEIC salvage on Windows with FFmpeg 8.1+ (as JPEG), before the Mac.
- A Windows scheduled reminder every six months (the checklist says to set a
  phone reminder instead).
