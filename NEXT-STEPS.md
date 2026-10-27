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
- The repository is public since 25 Oct 2026 (the user made it so):
  GitHub's own runners cost no Actions minutes for a public repository (the
  2,000 a month counted only while it was private). Still push with
  `[skip ci]` when there is nothing new to build, and cancel superseded runs.
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
- The program builds here for Linux (no dependencies to download). Since
  2026-10-26 (3) a plain debug build works (the exFAT checksum now wraps
  explicitly), and CI's Linux job runs the unit tests with overflow checks
  on as well, so a sum that runs past its type's range is caught.
  Chromium and Playwright are here (/opt/npm-tools/node_modules/playwright),
  so both page tests run here, and Xvfb gives a real `--app` window. For
  previews: Barlow Semi Condensed (in ~/.fonts) and Inter stand in for
  Bahnschrift and Segoe UI (fontconfig aliases); to see the page as GitHub's
  Linux machine does, a FONTCONFIG_FILE that rejects them (DejaVu Sans then,
  which is wider: what fits here may not fit there).

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
- CI run 40189925331 (058a5ce), all green on both systems (Windows: unit
  tests 109, automatic 69/69 with the PC's tool builds - the rebuilt video
  says 2023-07-16T11:00:00Z, its own date - resume 17/17, disk protection
  20/20, unplugged 35/35; Linux: automatic 68/68 with untrunc built from
  source).
- Delivered 25 Oct 2026, 12:09 GMT (the clocks went back that morning):
  version 2026-10-25 (1), C:\RECOVERY-TOOLS\automatic-recovery-probe.exe,
  5,374,464 bytes, SHA-256
  03091d75f4cd29edc03bbc1cd2f8a474471d0242396c306d7d5273d3612790fc (staged
  back and compared). main moved to it; the 2026-10-24 (1) main is kept as
  branch version-2026-10-24-1.
- [ ] On the PC (the user): the rehearsal again with the new version (This
  is SPARE: check it, Switch SPARE off, unplug, plug in the same way, check
  again, unplug SPARE), then 3. The folders (C:\IMAGE, SPARE's WORK,
  C:\COPY 2), then Start.

## 25 Oct 2026 (afternoon): the page made clearer

The user asked (12:44 GMT): wording an average person understands, a page
that looks better, icons or pictures but never without words saying what
they are for, contents that stay where they belong when the window is
resized, and whether showing it in a browser is still worth it. Keep the
page scrolling top to bottom (they read the process in order while it is
being developed). Their screen: 27-inch 4K, Windows 11 at 150% (2560 by
about 1350 CSS pixels in a maximised window).

- The browser stays: Edge draws the page sharp at 150% and keeps its
  layout; a native window would mean rewriting the page and all its tests.
  It now opens in Edge's app window (`msedge --app=`: no tabs or address bar,
  its own taskbar button), started by the launcher, which has no
  administrator rights (win::open_app_window); without Edge, or from a copy
  that has administrator rights, the usual browser as before. Edge takes
  each run for a new app (the random key is in the address) and would open
  it about 1,050 px wide, so the page fills the screen the first time it
  opens in such a window (display-mode standalone; once per window, kept in
  sessionStorage: a size chosen afterwards stays). Tried here in Chromium's
  own app window under Xvfb: 1050x900 became 2559x1439, and a smaller size
  set afterwards survived a reload. (`--start-maximized` was not used: when
  Edge is not running yet it would apply to every Edge window until Edge
  closes.)
- src/ui.html redone: three fixed places - where you are (the parts in order,
  with their states), the guide (top to bottom, as before), "Now" (the job
  running, Stop, what Start waits for you to do, what is next); below
  1680 px "Now" goes under where you are, below 1100 px where you are becomes
  a row of numbered circles on top and "Now" docks at the bottom while
  something runs. "Where your files go": the recovered SSD, the image, WORK,
  REPAIR, COPY 2, lit up as Start gets each done. Start's 15 stages grouped
  the way the files travel, in plain words. Icons: Lucide (ISC licence, its
  notice in the page), 64 of them embedded as one sprite; every one has words
  beside it (auto_ui_check.js and ui_check.js check that). Every button the
  checklist names keeps its exact name, so the checklist (and its ticks) did
  not change.
- Found on the way: the checklist's "step 7" links and "Go to step 7" went to
  the 7th numbered part on the whole page (step 1 of the steps tab, since the
  Automatic tab came first); they go to step 7 now (ui_check.js checks).
- Version 2026-10-25 (2).
- CI run 40200607237 (313f8b3), all green on both systems (Windows: unit
  tests 110 with edge_is_found, page 67/67, Automatic tab 19/19, automatic
  69/69, resume 17/17, disk protection 20/20, unplugged 35/35; Linux: page
  66/66, Automatic tab 19/19, automatic 68/68). The run before (8797723)
  failed only Linux's phone-width page check (17 px too wide): buttons were
  kept on one line, and DejaVu Sans is wider. Buttons now wrap between words;
  the tabs stay on one line in their sideways-scrolling row.
- Delivered 25 Oct 2026, 14:32 GMT: version 2026-10-25 (2),
  C:\RECOVERY-TOOLS\automatic-recovery-probe.exe, 5,435,904 bytes, SHA-256
  6280f2d1af2ecbc0ea001d67df014d37d27255dd3f033c385a104f4c67c45775 (staged
  back and compared). main moved to it; the 2026-10-25 (1) main is kept as
  branch version-2026-10-25-1.
- [ ] On the PC (the user): open the new version (it opens in Edge's own
  window, filling the screen), look the page over, then carry on as before:
  the rehearsal again (This is SPARE: check it, Switch SPARE off, unplug,
  plug in the same way, check again, unplug SPARE), 3. The folders (C:\IMAGE,
  SPARE's WORK, C:\COPY 2), then Start.

## 26 Oct 2026: active visual feedback

The user asked (25 Oct, again 26 Oct 06:26 GMT): Defender's exclusions
automated and confirmed, not an optional to-do; Defender shown as active on
the folders and drives where their icons are; SPARE shown switched off by the
icons in 3. The folders; sizes needed, estimated and actual in GB/MB for the
folders and drives, from the mapfile, the SSD, the copies, the repairs; the
time left and a small ring (the size of the text) for each running job; live
GB/MB, files and file kinds in Start; then suggestions of my own. (A first run
at it on 25 Oct, 15:00-16:00, was cut off by the usage limit before it was
committed; its work was in the working tree and was reviewed and finished.)

- Defender (web/places.rs): Start's first stage tells Defender to leave the
  image's folder and SPARE alone (the whole drive when WORK is on SPARE,
  otherwise WORK and REPAIR), by itself, then reads Get-MpPreference back to
  confirm ("defender_confirmed"); Get ready says At Start / Confirmed / Not
  taken, with Do it now. Never Windows' own drive, its system folders or
  programs, a whole profile, nor a whole drive other than SPARE (unit test).
  Folders already excluded (the user's own exclusion of SPARE) are not added
  again, so Finalise, which takes off only "defender_added", leaves them.
- api/places: the drives (letter, label, free, total), the image (made, or
  growing: .partial), WORK (from rescue-log.csv, read again only when it
  changes), REPAIR (walked, at most every 20 s), SPARE on/off/not plugged in
  (by the rehearsal's remembered identity; not looked at while a job runs),
  Defender. The page polls it every 5 s while something runs, 20 s otherwise.
- live.rs: the job running now counts files and bytes to do and done by kind
  (photos, videos, music and sound, documents, archives, other), and the file
  it is on: copying, checking the copies, checking every file, the extra
  checks, the repairs, the second copy. State JSON "live". No-op unless the
  page's job watches (the command line and unit tests never do).
- Time left: Job.pace keeps (seconds, done) once a second; "rate" is done per
  second over the last 90 s once there are 15 s to go on, none while nothing
  moves (a place the SSD struggles to read). Not from the whole run: a resumed
  copy rushes through what is already there. The page: (total - done) / rate.
- The map check's page data has "losing_bytes" (the files that lost data, in
  all): the repairs say "N damaged files so far" before REPAIR exists.
- Found on the way: the progress bar kept the last step's width when the next
  could not tell its progress yet (an inline width over the sliding stripe).
- Version 2026-10-26 (1).
- CI run 40282841889 (65a1285), all green on both systems (Windows: page
  68/68, Automatic tab 24/24, automatic 70/70 - Start read Defender's list
  and confirmed the folders, which GitHub's machine excludes already, so
  Add-MpPreference itself was not exercised there - resume 17/17, disk
  protection 20/20, unplugged 35/35; Linux: page 67/67, Automatic tab 24/24,
  automatic 68/68). Here: unit tests 116, both page tests with Windows-like
  and GitHub-like fonts, auto_check 61/61, screenshots of Start part-way on a
  60,000-file disk (copying; the extra checks with the time left).
- Delivered 26 Oct 2026, 07:20 GMT: version 2026-10-26 (1),
  C:\RECOVERY-TOOLS\automatic-recovery-probe.exe, 5,568,000 bytes, SHA-256
  500ff1ae9d0f9d79d768ddcdfcab920c451dd669860c189894af6eb83f3337bb (staged
  back and compared). main moved to it; the 2026-10-25 (2) main is kept as
  branch version-2026-10-25-2.
- [ ] On the PC (the user), as before: open it, Get ready, the rehearsal with
  SPARE, 3. The folders, Start. Defender: At Start until Start's first
  stage, then Confirmed (or Not taken, with the reason in Start's lines).

## 26 Oct 2026 (later): more live feedback, all six suggestions

The user (26 Oct 08:59 GMT) asked for all six suggestions made after the last
version: the finish time as a clock time; progress where it can be seen from
other apps (the window's title on the taskbar, Windows notifications when
Start needs you or stops); a live strip of the SSD while the image is made,
with a speed graph; the SSD's temperature, with a pause when too hot; a "what
just happened" feed; photos before and after their repair.

- The clock time: "done around 14:35" (to 5 minutes; "tomorrow" / a weekday
  further off) beside every time left (Now, Start's running stage). Not
  while reading is paused for the heat (the time left is not known then).
- The title: "37% · Making the image · 2 h 10 min left · Automatic Recovery
  Probe", "Your turn · ...", "Paused: the SSD is cooling down · ...", "Start
  is done · ...", "Needs attention · ...". Windows 11 hides taskbar labels by
  default, so taskbar.rs also paints a progress bar on the page's window's
  taskbar button (ITaskbarList3 on the Chromium window whose title ends with
  the program's name) and on the console window (and OSC 9;4 for Windows
  Terminal): green, yellow while Start waits for you or the SSD cools down,
  red after a stop on a problem; and FlashWindowEx when it starts needing you.
- notify.rs: Windows notifications through a hidden Windows PowerShell -
  first as PowerShell's own toast (stays in the notification centre), else a
  notification-area balloon. Sent when Start waits for you, stops on a
  problem, is done, says the SSD can be unplugged; when the SSD gets too hot
  (the first pause); when a step run from its own button for 2 min or more is
  done or fails. Setting "notify" (on/off) by a box beside Start; Try it.
- live.rs "what just happened": one line per event, newest first in Now (6,
  with Earlier), kept in results\what-happened.txt (read back at start).
  Stage began / done (with its first summary line) / not needed / stopped;
  the map check's findings in a line; copying folder by folder (at most every
  10 s: "Copied DCIM\2019 (412 photos, 2.1 GB)"); damage found (10 named,
  then counted every minute); every repair; the image's every 10%, slow and
  unreadable places (10 told, then every 100th); the temperature.
- The SSD's strip (live.rs Drive, State JSON "drive"): 400 cells, read / read
  slowly (a piece of at least 0.5 s at under 1/8 of the median of the last
  64) / could not be read; speed samples every 2 s (halved at 300, so the
  whole run); in Start's image stage, Now (compact) and step 4 (also after:
  "How the SSD read"). A sag (the last 5 samples under half the median) is
  warned of. The time paused is not counted as reading time.
- cooling.rs: the temperature from IOCTL_STORAGE_QUERY_PROPERTY
  (StorageDeviceTemperatureProperty, win.rs), else Get-StorageReliability-
  Counter (winset.rs), every 30 s while the SSD is read; pauses at 70 °C or
  the SSD's warning level - 2 / critical - 10 if lower (never below 50),
  carries on at 10 °C cooler (5 °C after 15 min); never left paused (three
  missed readings carry on). image.rs waits between pieces while paused.
  Many USB cases pass no temperature: the page then says it is not watched.
  Debug builds take test readings from ARP_TEST_TEMPS (driveshot.js).
- Photos before and after: api/thumb?id=R0012&which=before|after serves the
  damaged copy in REPAIR\to-repair or what the repair made, found by number
  in repair-list.csv / auto-repair-log.csv, only kinds a browser shows, never
  outside REPAIR (unit test; auto_check checks the bytes and four refusals).
  Only photos a repair made something of (repaired, or a preview saved). The
  live strip in Start's repair stage (3 newest, kept out of the list that is
  drawn every second) and a gallery in What is left (12 at a time); click for
  both big. A damaged copy the browser cannot draw shows words instead.
- Notifications show "Windows PowerShell" as where they come from (Windows'
  own name for the sender: nothing to install or register that way).
- Tests: unit tests 130 here (cooling's pause and carry-on, the strip, the
  pause not counted as reading time, damaged stretches told now and then,
  the feed, notify's quoting, taskbar's bars, thumbnails only inside REPAIR);
  auto_check 74/74 (feed in the page and in the file; api/thumb's bytes and
  refusals); auto_ui_check 32/32 and ui_check 67/67 with Windows-like and
  GitHub-like fonts; driveshot.js: Start with test temperatures pauses the
  image read when "too hot", says so in the stage, Now, the title and the
  feed, and carries on when cool; liveshot.js: screenshots of the strip,
  the speed line, the temperature, the pause, the photos and the dialog at
  2560, 1265, 900 and 390 px.
- Version 2026-10-26 (2).
- CI run 40317203888 (d279d72), all green on both systems (Windows: build
  without warnings, unit tests 131 with notify's windows_takes_it - the
  script parses as PowerShell - and taskbar's windows_takes_the_bars, page
  68/68, Automatic tab 32/32, automatic 83/83, resume 17/17, disk protection
  20/20, unplugged 35/35; Linux: page 67/67, Automatic tab 32/32, automatic
  81/81, resume 17/17).
- Delivered 26 Oct 2026, 12:08 GMT: version 2026-10-26 (2),
  C:\RECOVERY-TOOLS\automatic-recovery-probe.exe, 5,773,312 bytes, SHA-256
  088a3f50914c7aaac6b6035095665f922bd1e0e8815e0f90199f63ae9190a7e3 (staged
  back and compared). main moved to it; the 2026-10-26 (1) main is kept as
  branch version-2026-10-26-1.
- Not seen working here, only on the PC: a notification appearing (and from
  an elevated program), the taskbar bar and flash on Edge's window, the
  SSD's temperature through its USB case. If the temperature shows "does not
  tell Windows its temperature", that is the case or adapter, not a fault.
- [ ] On the PC (the user): open it; beside Start, Try it should show a
  notification (from "Windows PowerShell"); then carry on as before.

## 26 Oct 2026 (evening): Windows put back separately, "Show me in Windows", bugs

The user (26 Oct 20:08 GMT): Finalise putting back "the Windows settings the
program changed" could run with the drive still connected - make it a
separate button, greyed out until all drives are disconnected; link to
Windows' own settings that show the evidence ("Defender leaves the image
folder and SPARE alone", and whatever else is set), to verify personally;
and look for bugs.

- Nothing puts Windows' settings back by itself any more: not Start's end,
  not the copy stage (after the SSD is unplugged), not Finalise, not closing
  the window. Only **7. Put Windows back as it was** (a new part after
  Finalise; the same button in step 9 of the program's steps; the old
  guard-restore action goes the same way): web/back.rs `put_back`. It lists
  what the program changed (new disks arrive switched off; "Automatically
  restart" off, if it turned that off; Defender's exclusions it added), each
  with how it is now and how it goes back. "All drives" is taken as every
  drive of this recovery: any disk that holds a copy of the old drive (the
  recovered SSD, the 4 TB SSD - mapcheck::is_the_copy), a disk that will not
  let itself be read (it could be the SSD; not an empty card reader), and
  SPARE (its identity from the rehearsal, or the label). The page greys the
  button out and names each one still plugged in, with how to unplug it;
  the program itself refuses too (and while a job runs). The image opened by
  AIM Toolkit is not a drive to unplug. Other USB drives (a stick) do not
  hold it up.
- In the Automatic tab it says it comes last, after Finalise (the top banner
  and the part's own words); the part's state is "Last, after Finalise"
  until then.
- The PC as it stands (its results, read 26 Oct 21:10): the rehearsal passed
  on 25 Oct; Windows' setting for new disks is not changed now (the versions
  before this one put it back when the program was closed with the SSD
  unplugged); Defender: C:\IMAGE added by the program (19:58 today), E:\
  (SPARE) the person's own exclusion. So the page no longer offers the
  rehearsal's first button again once it has passed (Start sets the setting
  again by itself, before it asks for the SSD - Get ready says "At Start"),
  and Get ready's Defender line says which exclusion goes back in 7
  (C:\IMAGE) and which stays (E:\).
- If Windows is put back before the end anyway, Start sets what it needs
  again when it carries on: new disks switched off before it waits for the
  recovered SSD (stage_image), Defender's exclusions (carrying on skips the
  ready stage). Found while checking the new button: before, carrying on
  after a put-back would have let the SSD arrive switched on.
- "Show me in Windows" (showme.rs): buttons that open Windows' own page for
  each setting - Windows Security at Exclusions (windowsdefender://
  exclusions/), System Properties > Advanced (Startup and Recovery), Windows
  Update, Disk Management, the notification settings - and "Show what
  Windows itself says": a PowerShell window (read-only; a test checks it
  holds no command that changes anything, and on Windows that it parses)
  with Get-MpPreference's ExclusionPath, `diskpart san`, Get-Disk (IsOffline,
  IsReadOnly), AutoReboot, a restart waiting, powercfg /requests. In Get
  ready's rows, the rehearsal, step 3 and part 7.
- The checklist: Finalise no longer says it puts the settings back; a new box
  "7. Put Windows back as it was" after the final check; the last box says
  part 7 took Defender's exclusions off; the tab's name ("Automatic
  recovery"); the top says the image and the second copy go on `C:`. Ticks
  follow (RENAMED), and the copy delivered since 2026-10-25 (1) is known
  (KNOWN_CHECKLISTS), so it is updated in place.
- Bugs fixed:
  - PowerShell takes the curly single quotes (‘ ’ ‚ ‛) for quotes too: a
    file or folder named like "Mum’s iPhone" (as Macs and iPhones write it)
    ended the quoted words early, so a notification naming it was never
    shown, and a Defender exclusion of such a folder failed. winset::ps_quote
    now doubles every one of them (unit test; on Windows, PowerShell reads
    the words back exactly).
  - The SSD's speed graph counted a cooling pause as reading time when a
    piece was in flight as the pause began (a false dip after every pause).
  - "Nothing to put back" needed administrator rights to say so (Linux CI).
  - The put-back button could be clicked twice while it worked.
  - The exFAT entry-set checksum relied on silent wrapping (only release
    builds): now explicit, so a plain debug build runs every test with
    overflow checks on - all unit tests, auto_check, ui_check, the map-check
    tests and resume_check pass that way here, and CI's Linux job now runs
    the unit tests with overflow checks too.
  - Stale words: what Finalise does, "Finalise takes this off again", "step 9
    puts it back", "until the SSD is unplugged" (page, messages, README,
    guard.rs, the old program's README); README said the checklist has 30
    boxes (it had 27; now 28).
- Version 2026-10-26 (3).
- Tests here: unit tests 133 (with overflow checks too), auto_check 77/77,
  auto_ui_check 34/34 and ui_check 67/67 with Windows-like and GitHub-like
  fonts; with overflow checks: auto_check, ui_check, the map-check tests
  (43/43, 30 random) and resume_check (17/17). backshot.js / pcstate.js:
  part 7 with drives plugged in and not, Get ready, the rehearsal, at 2560,
  1265 and 390 px, as the PC stands now.
- CI run 40399863418 (9b54257) and 40400693281 (856e092), all green on both
  systems. Windows: unit tests 136 (with PowerShell reading the curly quotes
  back exactly, the answers window's script parsing, a notification with
  curly quotes parsing), unplugged 39/39 (Put Windows back refused with the
  copy plugged in, and with only SPARE plugged in - each named - then done
  with both unplugged, and the setting back as before), disk protection
  20/20, page 68/68, Automatic tab 34/34, automatic 86/86, resume 17/17;
  Linux: unit tests 133 twice (release, and with overflow checks), page
  67/67, Automatic tab 34/34, automatic 84/84, resume 17/17.
- Delivered 26 Oct 2026, about 21:45 GMT: version 2026-10-26 (3),
  C:\RECOVERY-TOOLS\automatic-recovery-probe.exe, 5,831,168 bytes, SHA-256
  24894749a746e90af95d3f55dd3bb331b60eed99762f9a3c06905cc5bdb223a5 (staged
  back and compared). main moved to it; the 2026-10-26 (2) main is kept as
  branch version-2026-10-26-2.
- Left for later: the program's own refusal (seen only if a drive is plugged
  in between the page's look and the click) says "Switch it off and unplug
  it" for SPARE too, where the page says Eject.

## 27 Oct 2026: bugs again, the logic, and how a person could go wrong

The user (27 Oct 12:48 GMT): check for bugs again, review how logical and
efficient the program is, and how a human could mess up using it.

Found and fixed:
- **The hand-off folders were emptied on Windows.** handoff::prepare kept a
  set of the copies it had just made (paths built with the `\\?\` long-path
  prefix) and then deleted every numbered file in the hand-off folders not in
  that set - comparing with paths read from the folder, without the prefix,
  which never match on Windows. So the copies for the Mac and the paid tools
  were deleted as soon as they were made (on Linux the paths match, and the
  tests only checked the folder was there). Now compared without the prefix
  (path_key); auto_check counts the numbered files in the hand-off against
  what it says is ready (CI on Windows checks it). supersede() had the same
  comparison (harmless there).
- **The hand-off could delete what the person saved in it**: a repaired
  version saved beside the damaged copy (a paid tool's default) instead of
  in "returned" was deleted by the next refresh. Now only the program's own
  copies (their name and size) are taken out; anything else with a number
  is moved into "returned" (" (2)" if the name is taken), and said.
- **Finalise copied WORK into COPY 2 before checking WORK**: a file changed
  in WORK (opened and saved by an app) would overwrite the good one in the
  second copy, then be found by the check - with both copies spoilt. Now
  WORK is checked first; only if it passes may a file already in COPY 2 be
  replaced (and replaced files are counted and said: COPY 2 is put right from
  a checked WORK); otherwise COPY 2 is only added to.
- **Writes could land on the wrong drive when letters moved**: the copy stage
  made WORK on whatever drive had WORK's letter (a USB stick, or the 4 TB SSD
  switched on, could take SPARE's E:). Now check_destination refuses any
  folder on a disk holding a copy of the old drive (image, copies, COPY 2,
  Bring back), and while WORK is not there yet on a USB drive it must be
  SPARE (label, or the rehearsal's disk identity); the copy stage waits and
  says why. If SPARE comes back under another letter, WORK (with its list of
  copies, on SPARE) is found there again (plan_of: work_elsewhere) - Finalise
  six months on said "Start has not copied everything yet" instead.
- **Folders inside each other / the second copy on WORK's drive** were
  accepted (COPY 2 inside WORK copies WORK into itself). check_folders refuses
  them at Start and Finalise, and the folders part says so at once (red,
  "Needs changing"). One drive is allowed in the page's tests only.
- **Step 9's "Switch it off"** acted on the disk number remembered from the
  image stage - after replugging, that number can be SPARE's. Now it finds
  the copy as plugged in now; and switching a drive off while a job runs is
  refused unless it holds a copy of the old drive.
- **Finalise's words**: "the image is as it was made" also when the image
  could not be checked (deleted to free room, say), and "both copies" with no
  second copy chosen.
- **The recovered SSD plugged in too early**: nothing protected it until
  Start's image stage. guard_watch (every 2 s while the program is open,
  elevated, on Windows): a disk holding a copy of the old drive that is
  online and writable is marked read-only and switched off at once; the page
  shows it at the top until read ("I've read this"), what-happened and a
  notification say it. Not AIM's read-only image (model Arsenal, or
  write-protected), not the system disk; disks found not to be copies are not
  read again. dropout_check E tests it on Windows.
- **The older Recovery Probe open at the same time** (port 47115, name
  "Recovery Probe"): both write the same results and could work on the same
  folders. Now launch refuses, the page shows it at the top, and no job
  starts while it answers (checked every 4 s). auto_check tests it with a
  stand-in on that port.
- The steps tab says to redo a step there only if Claude asks.
- The checklist: rule 5 (a label on each drive itself: SPARE and the
  recovered SSD take turns in one case), rule 6 (one program at a time), and
  a box after "Keep SPARE somewhere else": to use the files, copy them out of
  COPY 2\WORK; never open-and-save, rename or delete inside WORK or COPY 2.
  The 2026-10-26 (3) text is known (KNOWN_CHECKLISTS) so the person's copy is
  updated in place.
- Version 2026-10-27 (1).

The PC as it stands (results read 27 Oct): unchanged since 26 Oct 19:58; the
setting for new disks is not on now (versions before 26 Oct (3) put it back
on closing) - Start sets it again before it asks for the SSD, and the
watcher covers an early plug-in while the program is open. Not covered: the
SSD plugged in while the program is closed, before Start has set it.

## Not done / ideas for later

- Two damaged copies of the same file whose lost places differ could be
  merged into one whole file (autorepair.rs twin logic, v2).
- PNG repairs beyond re-encoding what FFmpeg decodes; GIF/TIFF/WebP.
- HEIC salvage on Windows with FFmpeg 8.1+ (as JPEG), before the Mac.
- A Windows scheduled reminder every six months (the checklist says to set a
  phone reminder instead).
