# Where the work stands (for picking up after a pause)

Work happens on the `dev` branch. Every push builds on GitHub; the results
land on the `ci-windows` and `ci-linux` branches (read them with
`git fetch origin +refs/heads/ci-windows:refs/remotes/origin/ci-windows` and
`git show origin/ci-windows:<file>`), because build logs themselves cannot be
reached from the development workspace. The Windows build of each commit is
`recovery-probe.exe` on the `ci-windows` branch.

`main` is the version on the user's PC (the user's rule, 21 Oct 2026). Move
it forward once: CI is green on dev; it is installed in C:\RECOVERY-TOOLS
and its sha256 checked on the PC; the delivery is recorded here in a
`[skip ci]` commit. ("GitHub's tests are proof enough", the user, 21 Oct
21:51: the self-test on the PC is still asked for after each install, but
main does not wait for it.) Then, first, keep the version main is leaving in a
branch named after it (the user's choice): `git push origin
origin/main:refs/heads/version-YYYY-MM-DD-N` (2026-10-20 (1) becomes
`version-2026-10-20-1`); then fast-forward main to that commit
(`git push origin dev:main`). (A tag was tried on 21 Oct: this session's
GitHub access cut the push of a tag off, so no tags.) Pushes to main never
start a build (the workflow builds dev only).

## Done

- File checks (`scan`), ported from the original Python program; identical
  results on every test file until the comparison and the Python original
  were retired on 19 Oct 2026 (in git history: `reference/`,
  `tools/compare_with_python.py`, `tools/make_fixtures.py`).
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
  screenshots.zip" on the user's PC (never in this repository). The zip holds
  only the 1 TB drive's two logs and screenshots (DDRescue-GUI, on a Mac).
- The user's drives (told 19 Oct): the recovered SSD (2 TB); a 4 TB SSD with a
  ddrescue copy of the whole 2 TB SSD (2 TB left unpartitioned; no log of that
  copy seen, so it is the spare, not the source); SPARE, 1 TB, NTFS (about
  930 GB as Windows counts); 2 TB free on the PC (internal; the image goes in
  an IMAGE folder there, the second copy in Phase 8 beside it). Nothing to
  buy unless step 5 says the files take more than about 900 GB.

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
  of rebooting with the SSD attached), both undone in Phase 8 (step 8).

## Pre-flight audit (done 2026-10-18 16:50 BST; delivered)

The user asked, before plugging in the recovered SSD: is the program right,
and will it cause no problems, since the whole process should be done once.
They are waiting for Claude's answer before plugging in. "Take all the time
you need; pause and resume automatically if usage runs out."

Checks (the fallback scheduled continuations were deleted when done):
- [x] 1. Nothing can write to the SSD (disk handles, IOCTLs, every page step):
      confirmed by the independent review (docs/review/REVIEW-FINDINGS.md,
      "What was checked").
- [x] 2. Copying right for a Mac-made exFAT (runs, NoFatChain, valid data
      length, >4 GB, timestamps, NFD names, forbidden characters, long paths):
      covered by the test disks (no-FAT-chain files, valid data length,
      non-English and long names) and unit tests (names Windows can't use);
      never tried on the real Mac-made volume, which only Phase 2 will show.
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
- [x] Scale after the changes (Linux, 1M files): first copy 11 MB peak;
      stopped at 500k then resumed: identical copies and list; re-checking a
      complete copy (start and end of every file against the disk) 75 s,
      350 MB peak.
- [x] Windows CI f1a1285: dropout_check 30/30 (copy stopped with "os error
      433", nothing failed, policy kept OfflineAll, re-plugged disk arrived
      offline, copy finished identical to a straight run; SPARE released).
- Note: the repository is private, so GitHub Actions minutes are limited
  (Windows counts double). Batch changes; cancel superseded runs.
- [x] Extra: copies an earlier run left are checked (start and end) against
      the disk before being kept (a power cut can leave zeros); the scan stops
      when the checked folder goes away (walk.rs GONE); the list is kept on
      disk, not in memory (1M files: ~100 MB on a first run).
- [x] 7. CI green for 05717a4 (Windows: self-test, map check tests 42/42,
      Windows checks 18/18, disk protection 16/16, unplugged mid-copy 30/30,
      page 24/24; Linux success). Delivered to C:\RECOVERY-TOOLS\recovery-probe.exe
      (sha256 eb2af91b..., version "2026-10-18 (2)"), checked byte for byte.
      LESSON: device_commit_files from a reused /mnt/user-data/outputs name
      delivered the OLD file once; always stage under a fresh name and check
      the size/hash on the PC afterwards.
- [x] 8. Checklist Phase 1 steps 7-12 updated in the user's copy (35 ticks
      kept, verified). Fallback scheduled tasks deleted.

## Independent review and its fixes (18-19 October 2026)

A fresh session reviewed the program and the plan from REVIEW-BRIEF.md; the
findings are in REVIEW-FINDINGS.md (both now in docs/review/). The `review`
branch is out of date and can be deleted (its last commit is a2dc9ed1e4029bd04582f53e8b743abdb34be161,
if it is ever wanted back); this session's GitHub access could not delete
branches, so the user does it on GitHub. The user then asked for the fixes.
Done on `dev`:

- CI publishes to ci-windows/ci-linux only from `dev` pushes (a `review`
  push had overwritten them; restored from ci-backup/ on `review`).
- Windows' disk setting stays OfflineAll from "Get Windows ready" until no
  disk that could be the recovered SSD is attached (no restore after
  Protect, after a step, or on closing the window while it is attached).
- Disks Windows calls removable media are shown, and the page says to image
  a removable recovered SSD at once and ask before plugging it in again.
- Created and accessed dates are copied (files and folders); the map check
  shows the newest date on the volume and warns if it is after the mapfile.
- `image` (step 4): the whole disk into recovered-ssd.img with its SHA-256,
  read back to check; unreadable 4 KB units become zeros and are recorded
  as a ddrescue-style mapfile next to it, which the map check folds in.
  Steps 5 and 6 read the image once it is checked.
- SHA-256 per copied file in rescue-log.csv (software, or SHA-NI); a resumed
  copy keeps an earlier copy only if the whole file matches; `verify`
  (step 7, "Check the copies are unchanged").
- Lost-data pieces on never-read places are named in the map check the way
  the copying names them, so the check of the copies marks them NEVER_READ.
- Folders whose own record was lost are rebuilt from their contents
  (exfat.rs find_orphan_folders), walked as "found folders (name lost)\
  folder at cluster N"; the oracle in test_mapcheck.py mirrors it (500
  random disks agree); the builder writes deleted records as real drives
  leave them.
- Notes in rescue-log.csv carry the disk offset and appear once each.
- docs/recovery-checklist.md rewritten along the review's simpler plan
  (image first; nothing moved or deleted in WORK; damaged files copied to
  REPAIR; two copies checked against the checksums); download-manifest.md
  to match.

## Isolate, repair, put back, evaluate (19 October 2026, the user's request)

src/repair.rs, step 8 on the page, CLI isolate / put-back / undo-put-back /
evaluate. Damaged copies (from step 7's results CSV, rescue-log "partly
copied", and any list in REPAIR\damaged-lists) are copied into
REPAIR\to-repair\<group by repair tool>\<name [Rnnnn].ext>; repair-list.csv
holds id, path in WORK, sha256, why, package. Put back finds REPAIR\repaired
files by their tag, checks WORK's file is still the original (or the last
version put back), moves it to REPAIR\originals (rename: same drive), copies
the repaired one in with the original's dates, logs WORK\repair-returns.csv;
verify and the scan's join read that log. Packages: libraries' media members
go out and back one by one; their core and previews are not copied out (role
by folder names and extension); document packages go out and back whole (tree
hash); programs and disk images are left. Evaluate walks WORK for package
roots (names, or Photos library structure) and writes REPAIR\evaluation.csv.
Unit tests in repair.rs (round trip, undo, interrupted put-back), join.rs;
tools/ui_check.js drives it in the browser (36 checks).

CI for fb074a0 was green (Windows unit tests 41, self-test, map check 42/42,
Windows checks 18/18, protection 16/16, unplugged 31/31, page 37/37; Linux
92/92, kernel 23/23, page 36/36). Delivered 19 Oct 13:01 BST with the user's
yes: recovery-probe.exe (sha256 dcd1d4bc...), the checklist (31 ticks) and
the manifest, checked byte for byte on the PC.

## The safety check (19 October 2026, the user's request)

"A safety check after all phases that the total size matches, so files were
not deleted by accident; reassuring checks along the way to find where I go
wrong." The mapfile can't give a file total (it covers the whole disk), so:
step 5 adds up every file's size (map-check.json volume.bytes); step 6 ends
with a two-stage safety check (step 5's files vs rescue-log.csv: number and
size, failed files, a copying that never finished); step 7's verify adds the
third stage, the copies here now against their checksums (verify.rs safety,
copy_safety, ListTotals). Every check is appended to
results\safety-checks.csv and step 7 lists the latest (history_json). The
checklist has a "Your numbers" box (Phase 2) and a checkpoint at the end of
every phase; Phase 8 ends with the final check on both copies. Also: the
page only redraws a section when it changed (a folded-out report no longer
folds up every few seconds), and Phase 7's Photos steps were made safe
(open the copy with Option first; Option+Command repairs the current
library, so only after checking Settings > General; osxphotos export first
as a safety net; switch back afterwards). Version "2026-10-19 (2)".

CI for b9867a5 was green: Windows unit tests 44, self-test (with the safety
check line), Python comparison 0 differences, map check tests 42/42 (the
safety check agreed on 41), Windows checks 18/18, protection 16/16,
unplugged 31/31, page 46/46; Linux map check tests 92/92 with fsck.exfat and
dissect.fat (safety agreed on 91), kernel 23/23, page 45/45. Locally 312 of
312 random and edge disks. Delivered 19 Oct 17:29 BST into C:\RECOVERY-TOOLS:
recovery-probe.exe (b9867a5, sha256 22db873e..., 3,866,112 bytes), the
checklist of 3f05f2a with the user's 31 ticks (sha256 ba015b35...) and the
manifest (7d127308...), each checked byte for byte on the PC.

## The checklist in the program, and a tidy-up (19 October 2026, evening)

The user asked: build the checklist into the program, check the safety checks
are enough and come at sensible times, drop Python that isn't needed, check
the Mac tools are the best ones, and resume by itself if usage runs out.

- checklist.rs: docs/recovery-checklist.md built in; the page opens on "The
  checklist" (the steps are the other tab). Ticks in
  results\checklist-ticks.txt (T/U lines with local time). Checkpoint boxes
  are derived, not stored: passed when the newest step-7 check made after
  the last tick among the boxes they cover (own phase before them, plus
  "covers Phases N and M") passed; the final one needs two folders. The
  person's copy (newest recovery-checklist.md under the program's folder,
  two deep: "Obsidian MD files") is synced: ticks taken in when its mtime
  changed (newest change wins), written back whole unless it holds the
  person's own text (normalised hash vs the delivered versions 05717a4,
  4c10e41, 3f05f2a and the last one written). The manifest beside it too.
- Step 4 "Check the image is unchanged" (image::check_existing); the result
  joins safety-checks.csv as "Image checked (step 4)".
- Put back refuses while the newest step-7 check of that WORK failed.
- Removed: reference/ (the Python original), tools/compare_with_python.py,
  tools/make_fixtures.py and their CI steps; REVIEW-* moved to docs/review/;
  tools/README.md says what each remaining test tool is for.
- Checklist: "When the safety checks happen, and why then"; image check in
  Phase 8 and twice a year; RAW second opinion (Adobe DNG Converter);
  PowerPhotos and Photos' own Import for combining libraries; other Mac
  libraries (iMovie, Final Cut Pro, Logic, GarageBand) opened as copies.
- The three automatic resumes (in case usage ran out) were deleted when the
  work was done.

CI for f1d1f3e was green: Windows unit tests 55, self-test, map check tests
42/42 (the safety check agreed on 41), Windows checks 18/18, protection
16/16, unplugged 31/31, page 60/60; Linux map check tests 92/92 with
fsck.exfat and dissect.fat (safety agreed on 91), kernel 23/23, page 59/59.
(7d47903's Windows page test had timed out taking a whole-page picture of
the long checklist; it now takes a window-sized one.) Delivered 19 Oct
23:32 BST into C:\RECOVERY-TOOLS: recovery-probe.exe (f1d1f3e, sha256
5bcf5012..., 4,118,016 bytes), the checklist of 7d47903 with the user's 31
ticks (written the way the program writes it; sha256 80b872cf...) and the
manifest (910d31f2...), each checked byte for byte on the PC. From now on
the program itself keeps the Obsidian copy and the manifest current.

## Final polish (20 October 2026)

The user ran the self-test with f1d1f3e (it passed), and asked for a final
polish, feedback on the project, and whether the whole operation is logical.
Version "2026-10-20 (1)":

- Put back ignores what a Mac writes beside every file on a USB stick
  ("._IMG_0003 [R0012].HEIC" carries the same number, so every repair done
  on the Mac would have been refused as "more than one repaired version"),
  .DS_Store and the like, and folders from the top of a stick (.fseventsd,
  .Spotlight-V100...). The lists in damaged-lists skip "._" files too
  (a "._sips-failures.txt" would have shown as lines naming nothing).
  Unit test what_a_mac_adds_on_a_usb_stick_is_left_alone (fails without the
  fix).
- REPAIR\salvaged: what can't take the original's place (a TIFF made from a
  damaged RAW photo, since nothing can save a camera RAW again; what 7-Zip
  takes out of a damaged archive). The checklist's Phase 6 said to export a
  RAW "in the same format", which can't be done, and to put what 7-Zip took
  out "beside the archive", which the safety check would call added files.
  Put back's message for a file of another kind says where it can go.
- Code on the page wraps as whole words where it can (it broke mid-word).
- Checklist: the program's page no longer said to have "one button" per
  step; Phase 6 folders 2 and 7 and Phase 8 step 2 (RENAMED keeps ticks on
  those three lines). Manifest: the retired Python comparison is no longer
  claimed; MultiPar points at Phase 8. KNOWN_* hold 7d47903's hashes, so the
  copies delivered on 19 October are upgraded in place, ticks kept
  (simulated on the delivered copy: 31 ticks kept; with 3 more ticked in
  Obsidian first: 34 kept).
- CI for a728da4 was green (run 39340397360): Windows unit tests 56,
  self-test, map check tests 42/42, Windows checks 18/18, protection 16/16,
  unplugged 31/31, page 60/60; Linux map check tests 92/92 with fsck.exfat
  and dissect.fat, kernel 23/23, page 59/59. The Windows build is
  4,123,136 bytes, sha256 9dbaaa60da6777b3... (on ci-windows, 12998bd).
  Delivery: refused at 00:41, 01:02 and 05:01 BST because the program was
  open on the PC (the exe is locked while it runs); delivered by the 10:00
  BST automatic resume into C:\RECOVERY-TOOLS\recovery-probe.exe, staged
  back and checked: 4,123,136 bytes, sha256 9dbaaa60..., version
  "2026-10-20 (1)". Only the exe went in: the program upgrades the Obsidian
  copy and the manifest itself the first time it opens (both were still
  the 19 October ones at 10:00). The automatic resumes are gone (two fired,
  the last one deleted; none left scheduled).
- Feedback given (in the chat): the order is logical (the one risky read
  first and rehearsed; every phase checked against Your numbers; repairs on
  copies; two copies plus the image at the end); essential phases 1, 2, 3,
  6 and 8, with 4, 5 and 7 optional; Windows Update's block only matters in
  Phase 1; the unknowns only the real hardware can settle (USB case: the
  rehearsal; the real volume: the bitmap backstop, "data no folder names
  any more"; SPARE's room; the Mac reading SPARE only).
- For the next change to the checklist (not worth a build on its own): in
  Phase 8, say that WORK and COPY 2 are the archive from then on, and that
  files used or reorganised day to day are copied somewhere else first
  (anything changed inside shows at the six-monthly check). Also: names in a
  Mac list (sips-failures.txt) are matched exactly; if a list's non-English
  names show as "name no file in WORK", look at Unicode normalisation (NFD
  from the Mac vs the names in WORK).

## main brought up to date (21 October 2026)

The user ran the self-test with 2026-10-20 (1) on the PC (passed at
05:06:20, results\recovery-probe-state.txt agrees), and opening it upgraded
the Obsidian checklist (31 ticks kept) and the manifest, both checked. The
user deleted the `review` branch. GitHub's front page showed `main`, still
at 2fe0e9b (17 Oct, 52 changes behind, with the old Python program in
reference/), hence "Python 54.7%". The user asked: bring main up to date;
from now on main = what is in C:\RECOVERY-TOOLS, moved only once tested and
proven to work; old versions kept under their version numbers; the old main
needs no branch of its own. Done: main fast-forwarded to dev (2fe0e9b stays
in the history, as an ancestor), the workflow builds dev only, and the
README says what each branch is. 2026-10-20 (1) (built from a728da4) gets
its own branch only when a newer version replaces it on main. On dev the
languages are about Rust 72%, Python 18% (the test tools), HTML 8%,
JavaScript 2%.

## Testing everything and looking for bugs (21 October 2026)

The user asked: test everything and look for bugs. Done here (Linux):
unit tests, self-test, the oracle's standard 92, 400 + 4,000 more random
damaged disks, 1,500 more with the new scan check, 4,000 corrupted disks
(map check, copying and the safety check on each: no crash, hang or
disagreement), 66,000 damaged sample files through the file checks (no
crash, hang or internal error), the page test; and a read of the riskiest
code. Version "2026-10-21 (1)". Found and fixed:

- image.rs (serious, after a crash only): carrying on with a stopped image
  kept the whole .partial file; after a crash or power cut its end can be
  zeros (Windows notes the size before the data reaches the drive), which
  would have gone into the image and its SHA-256 unnoticed. Now the image
  and the list of unreadable places are made safe on the drive every 2 GiB
  and when stopped, the point is noted (recovered-ssd.img.partial-safe.txt,
  written safely), and a run carries on only from there. Test
  after_a_crash_it_carries_on_only_from_the_safe_point fails without it.
- image.rs: the read-back after making the image, and "Check the image is
  unchanged", read from the drive itself (unbuffered on Windows), not from
  what Windows still holds in memory of a file it just wrote.
- rescue-log.csv (both places it is rewritten) and the program's state file
  are written safely (sys::write_safely: a file beside, synced, renamed).
- tools/test_mapcheck.py: an operator-precedence slip in the oracle expected
  an unreadable area running from unpartitioned space into the EFI
  partition to hit only "unpartitioned" (2 of 4,000 random disks; the
  program was right).
- The command-line scan takes --map-check (as the page does), and the
  oracle now checks on every random disk that exactly the copies of the
  files that lost data are marked NEVER_READ.
- tools/ui_check.js waited a fixed 1.5 s for the restarted program; under
  load the old page's poll failed in between. It now waits until it answers.

- A check of the copies is recorded at its start (81e3460): a box ticked
  while it ran no longer counts as checked by it.

CI for 81e3460 was green (run 39549031906; 9e9339a's run was cancelled for
it): Windows unit tests 59 (the crash test, the safe write and the
unbuffered read on NTFS among them), self-test, map check tests 42/42 (103
copies marked NEVER_READ by the scan with the map check), Windows checks
18/18, protection 16/16, unplugged 31/31, page 60/60; Linux map check tests
92/92 with fsck.exfat and dissect.fat, kernel 23/23, page 59/59. The Windows
build: 4,134,912 bytes, sha256 10614db6268e5439... (ci-windows bd80456).
Install attempts at 06:55, 07:08, 07:39, 10:19 and 15:20 BST were refused: the program was
open on the PC all day. The user closed it at 15:21; installed 15:21 BST into
C:\RECOVERY-TOOLS\recovery-probe.exe and staged back: 4,134,912 bytes, sha256 10614db6...,
version "2026-10-21 (1)" (mtime 1792592502300). The user's self-test passed at 21:50:29
(results\recovery-probe-state.txt agrees). main moved to this version at 21:53 BST; 2026-10-20 (1)
(053e082) kept in branch version-2026-10-20-1.

(Both left open here were fixed in 2026-10-21 (2), below: the hour the clocks
go back, and a put-back interrupted between its rename and its log line.)

## Second review and its fixes (21 October 2026, evening)

A fresh session reviewed the project; the user then asked "Fix all the
problems". Version "2026-10-21 (2)". Found and fixed:

- **SPARE taken for the recovered SSD (serious; before the rehearsal).** The
  copy was recognised by its partition table being made for the mapfile's
  drive, 1,000,204,886,016 bytes - the size of every "1 TB" drive, so a 1 TB
  SPARE that Windows set up as GPT was "the copy ddrescue made" too: chosen
  in step 3, "Switch disk N on, writable" refused for it, "Put Windows'
  setting back" refused while it was plugged in, and steps 4-6 would have
  read it (step 4: an image of SPARE, then "the SSD is not needed any
  more"). Now `mapcheck::is_the_copy`: made for that size AND bigger than it
  (the copy at the start of the 2 TB SSD) or holding the exFAT volume (the
  image). Used by the disk list, `could_be_recovered`,
  `recovered_disk_attached` and `check_disk`; `image::make` refuses any
  other disk, and an existing or unfinished image that does not start like
  the disk (first 64 KiB). The page chooses a copy that arrives after
  another disk was clicked (`autoChoice`), keeps looking for disks for 15
  minutes after any step 3 action, and shows a drive's partition-table size
  only when it was made for another drive. Tests: sparse 1 TB / 2 TB
  stand-ins (mapcheck.rs), image.rs, ui_check.js (a SPARE stand-in: the
  test disk's partition table only; those checks fail against 2026-10-21
  (1)), dropout_check.py (SPARE exactly as big as the copy: not the copy,
  released, setting put back with it still plugged in).
- image.rs: the .sha256 and the list of places the SSD could not read are
  written safely before the image gets its name (a crash in between left an
  image that could be neither checked nor carried on - or, after SSD read
  errors, no list of them for the map check); a stale list is removed; the
  line "it is not needed any more" came before the read-back check (now:
  keep the SSD in until Done).
- guard.rs: diskpart gets its commands on its input (it was a predictable
  script file in %TEMP%, run with administrator rights); diskpart,
  explorer and rundll32 are started by full path (`win::system_program`,
  `windows_program`), not looked up in the program's own folder first.
- Safe writes: the Obsidian checklist copy, repair-list.csv and the note of
  Windows' setting (`sys::write_safely`); checklist-ticks.txt and
  safety-checks.csv synced. Put back makes the repaired version safe on the
  drive and reads it back from the drive itself before it replaces
  anything; a put-back the PC stopped between putting the file in place
  and listing it is listed by the next put-back (if the original is in
  REPAIR\originals); a file put in place by hand, or a "repaired" file
  identical to the damaged original, is refused.
- The clocks going back: ticks and checks are recorded with their offset
  from UTC ("2026-10-25 01:30:00 +01:00", `fmt::when_of`) and compared as
  moments (`fmt::instant`); the older ones, without an offset, are read as
  local time; times are shown without the offset.
- Checklist Phase 1 steps 11 (stop if SPARE is marked the copy), 13 (chosen
  by itself; else click it, or Look again) and 15 (keep the SSD in until
  Done); none of them was ticked. `KNOWN_CHECKLISTS` has a728da4's hash, so
  the copy in Obsidian is upgraded in place: simulated on a copy of the
  PC's files, 31 ticks kept, the ticks file untouched.

Here (Linux): unit tests 69, self-test, map check tests 72/72 (seed 7),
page 63/63, in Europe/London, Los Angeles, Kolkata and Lord Howe time for
the time tests. Not checked here: the Windows-only code (no Windows target
in this workspace); GitHub's Windows run is the check.

## Next

1. Phase 1 (rehearsal with SPARE, then the image onto the PC's 2 TB drive).
   Help read step 5's results; check the files fit on SPARE.
2. Phase 6 is the program's step 8 (isolate, repair, put back, evaluate);
   help with the repairs themselves when the user gets there.
