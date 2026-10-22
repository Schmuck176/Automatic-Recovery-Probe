# Independent review: findings

Three reviews so far, each by a fresh Claude session: the first (18-19
October 2026) is this page down to "What was fixed"; then "Second review (21
October 2026)" and "Third review (22 October 2026)", at the end.

Reviewer: a fresh Claude session, 18-19 October 2026, working from
REVIEW-BRIEF.md on `dev` at 2fe1eec (the code built and delivered as 05717a4,
version "2026-10-18 (2)"). Nothing on the person's PC was changed;
C:\RECOVERY-TOOLS was only read.

## In plain words

The program does what it promises where it matters most: **nothing in it
writes to the recovered SSD**, and it copies files Windows cannot see. The
real risks are elsewhere:

1. there is still only one copy of everything, and the plan keeps using it;
2. later in the plan, deleting duplicates and moving files into GOOD / BROKEN
   / UNSURE can break or lose files;
3. two ways Windows could still write to the SSD: a USB case that calls
   itself "removable", and the gaps when the program has already put
   Windows' disk setting back to normal.

None of this needs the SSD. All of it can be fixed before it is plugged in.

## What was checked

- **The program on the PC is the tested build.** Its sha256 (eb2af91b...)
  matches the CI build of 05717a4. The checklist in "Obsidian MD files"
  matches docs/recovery-checklist.md, apart from the ticks.
- **The claim "only guard.rs `open(n, true)` opens a disk for writing" is
  true.** Every other way the program opens a disk is read-only (disk.rs
  `PhysicalDrive::open`, `list`) or asks for no access at all (win.rs
  `open_query`). Every file the program creates goes into its own folder
  (results and its notes), the temp folder or the chosen copies folder. `rescue.rs same_volume`
  refuses a copies folder on the recovered volume. Opening a disk for
  writing, as `guard.rs` does to change its read-only and offline switches,
  writes nothing to the disk itself.
- Built and unit-tested here (Linux). The self-test passes. The real
  mapfile, read on the PC, parses as the brief says: 167 places, 610,816
  bytes, `-s` given, no `-o`, so mapfile positions are SSD positions.
- Read in full: guard.rs, disk.rs, win.rs, web.rs, ui.html, cli.rs, main.rs,
  rescue.rs, exfat.rs, gpt.rs, mapfile.rs, mapcheck.rs, join.rs, walk.rs,
  probe.rs, holes.rs, formats.rs, report.rs and the CI workflow. Read in part:
  checks.rs, sqlite.rs, selftest.rs and the test tools.
- One experiment on a test disk, made with tools/exfat_image.py (finding 5).
- **Not checked, and it cannot be checked from here:** how the person's real
  USB SSD and its case behave on Windows (every CI test used virtual disks),
  and the real Mac-made volume.

**What is good and should stay:** reading past blank folder records, so
nothing is left behind; copy-then-rename; refusing a disk whose partition
table doesn't match the mapfile; stopping cleanly when a drive goes away;
refusing to call HEIC/RAW "fine"; the map check, which pinpoints the files
ddrescue lost data from at any size, and which few tools offer; and the
byte-for-byte protection tests.

Ordered by how much harm each could do, most serious first.

---

## 1. There is still only one copy, and the plan keeps using it

**Where:** docs/recovery-checklist.md, Phase 1 steps 6-13.

**What would happen:** until Phase 1 step 12 finishes, the recovered SSD is
the only complete copy. It is read for hours (step 4, step 5, and again after
any stop). It is an SSD that has probably sat unpowered for about two and a
half years. If
it develops read errors, or a USB problem or a mistake happens while it is
plugged in, there is nothing to fall back on. The copies in SPARE\WORK don't
stay a faithful second copy either: Phases 2 and 6 trash and move them.

**What to do instead** (standard practice for one-chance recoveries): make a
sector-by-sector image of the SSD's first 1,000,204,886,016 bytes first.
Check it, put the SSD back in the drawer, and do every later step from the
image. The SSD is then read once, front to back, and never again. Every step
can be re-run, and other tools can be tried safely.

- The program already reads image files (`disk.rs ImageFile`; `open_source`
  takes a path). What's missing is a "make an image" step, with a checksum
  and with any read errors recorded the way ddrescue does it, plus a way for
  the page to choose the image instead of a disk.
- The image is 1,000 GB (932 GB as Windows counts). On SPARE, the image and
  a full set of copies would together need nearly all of its 2 TB if the old
  volume was full, leaving no room to work. One more 2 TB drive is the
  single most valuable thing to buy for this job. An ordinary external hard
  disk is fine for holding an image.

---

## 2. Phases 2 and 6 can lose files or break them, on SPARE

**Where:** checklist Phase 2 (Krokiet: Empty Files, Temporary Files,
Duplicate Files -> Trash) and Phase 6 (moving files into GOOD / BROKEN /
UNSURE).

**What would happen:**

- *"Trash can be undone" is not guaranteed.* Windows gives each drive's
  Recycle Bin a maximum size, by default about 5% of the drive. When the bin
  is full, Windows permanently deletes the oldest items to make room. Trashing
  a large batch of duplicates can silently destroy what was trashed earlier.
- *Packages get split.* Only Photos Libraries are protected. Inside an
  .fcpbundle, .imovielibrary, .logicx, .band, .key/.pages/.numbers, .rtfd,
  an iPhoto or Aperture library and many more, duplicates and "temporary"
  files get trashed one by one, and Phase 6 moves single files out.
- *Files get separated from their companions.* Sorting by verdict moves:
  - a RAW into GOOD and its .xmp edits into UNSURE;
  - an iPhone photo away from its .AAE edit file;
  - a video away from its .THM;
  - a .cue away from its .bin;
  - a web page away from its _files folder;
  - photos away from the Lightroom catalogue that finds them by path.

  It also leaves the `._` files behind in WORK (point 7). Without a record,
  none of this can be put back together later.
- Phase 6 comes after days of other work, so a mistake there means redoing
  much of the process.

**What to do instead:** the brief's "current lean" is right.

- Never delete or move anything in WORK.
- Record verdicts in a report (CSV/HTML), not in folder names.
- Copy (not move) damaged files into a REPAIR folder with a manifest. Put
  repaired versions beside the originals, or in a REPAIRED tree with the
  same layout.
- Treat a package as one unit. Recognise packages by their extensions and
  by their structure (for example `Contents/Info.plist`).
- If you want to remove duplicates at all, do it at the very end, outside
  packages, from a list you've reviewed first. Move duplicates to a
  DUPLICATES folder on the same drive rather than the Recycle Bin.

---

## 3. A USB case that reports "removable media" gets no protection until after Windows has mounted the volume

**Where:** src/guard.rs (relies on the SAN policy); the `identity` functions
in src/disk.rs (line 372) and src/win.rs (line 307), which read the storage
descriptor but ignore its RemovableMedia byte; checklist Phase 1 steps 7-9.

**What would happen:** as the brief says, the SAN policy doesn't apply to
disks Windows treats as removable.

- Such an SSD arrives switched on and writable, whatever step 3 did, and
  Windows mounts its volume at once.
- On removable drives, the Storage Service and Windows Search are known to
  create System Volume Information\WPSettings.dat and IndexerVolumeGuid.
- Protect may then mark the disk read-only, but it can't switch it off.
- The checklist's "if the page shows an error, unplug" comes after the
  writes.

**What to do:**

- **Dress rehearsal with SPARE first, through the same case, cable and
  port.** It is still empty, so nothing is at risk. Click "Get Windows
  ready", plug SPARE in, and check it arrives switched off with no drive
  letter. Then click "Switch disk N on, writable". If SPARE gets a drive
  letter, stop: that USB path reports removable media. (This only tells you
  about the recovered SSD if it goes through the same kind of case.
  Otherwise ask how its model is reported.)
- **Show "removable" in the step 3 disk list.** It is byte 10 of
  STORAGE_DEVICE_DESCRIPTOR, which `identity` already fetches. Refuse to go
  on with a removable recovered SSD.
- A hardware USB write blocker removes this whole class of risk, and point 4
  too. The Tableau T8u lists at about $600 new; second-hand ones are much
  cheaper. If one is bought, the image in point 1 is the thing to make
  through it.

---

## 4. Windows' "leave new disks switched off" setting is put back while the SSD is still plugged in

**Where:**
- src/ui.html `protect()` (calls `guard-restore` straight after Protect,
  line 776);
- src/web.rs `start_map_check` / `start_rescue` (put the setting back when
  the step ends, lines 1137 and 1182);
- src/web.rs `restore_setting_on_close` (puts it back when the black window
  is closed, line 533).

**What would happen:** the SAN policy is OfflineAll only while steps 4 and 5
run. In these gaps it is back to OnlineAll with the SSD still attached:

- between Protect and step 4;
- between step 4 and step 5, while the person reads the results and writes
  down the numbers;
- after step 5, until the SSD is unplugged;
- whenever the program's window is closed.

If the SSD re-appears to Windows in one of those gaps (a USB hiccup, a
knocked cable, a hub reset), it arrives switched on. Unless Windows
remembered the read-only mark, it is also writable; the tests showed Windows
does not remember the mark for virtual disks. Windows then mounts the exFAT
volume and may write to it: System Volume Information, the volume's dirty
flag, and the "Scan and fix?" offer. The same applies after a power cut, if
the PC is set to switch itself back on when power returns and the SSD is
still plugged in.

**Fix (small):** keep OfflineAll from "Get Windows ready" until the SSD has
been unplugged.
- Don't call `guard-restore` in `protect()`.
- In step 7, offer "Put Windows' setting back" only once no disk that
  `could_be_recovered` is attached.
- When the window is closed, put the setting back only if no such disk is
  attached (or not at all: the banner and the note file already remind).

SPARE and other drives plugged in meanwhile can still be switched on with
the existing "Switch disk N on, writable" button.

---

## 5. A folder whose own record was never read comes out as one nameless blob, even though its files' names survive on the disk

**Where:** src/exfat.rs `walk_until` (line 438: a folder can only be reached
through its parent's record), src/rescue.rs "lost data (no name)" (line 894).

**What would happen (tested):** on a test disk, one 512-byte never-read
sector held the record of the sub-folder "Photos\Holiday 2019" (30 photos).

- The map check reported "1 folders affected" and "2 MB of data ... belongs
  to no file".
- The copy saved the folder's own records, all 30 photos and five other
  files as **one 1.7 MB file**: `lost data (no name)\cluster 26 (424
  clusters).bin`.
- Yet the sub-folder's own cluster was intact, with every name, size, date
  and cluster number. A recovery tool that looks for orphaned folders would
  rebuild "Holiday 2019" completely.

**How likely on the real disk:** low, because a never-read place has to hit
a sub-folder's record. Step 4 will show it as a large "data no folder names
any more" figure.

**What to do:** if that figure is more than a few MB, stop before going on
and rebuild those folders. Either in the program (scan the allocated
clusters no file claims for valid file-record sets, and walk them as
orphaned folders), or with a mature recovery tool working on the image. The
same tools can split the remaining nameless pieces into files; PhotoRec is
free.

---

## 6. No checksum is kept of what was copied

**Where:** src/rescue.rs `copy_file` (line 216), `copy_matches` (line 331).

**What would happen:**

- An earlier run's copy is kept if its size, and its first and last 4 KB,
  match the disk. After a crash or power cut, a copy whose middle never
  reached SPARE still passes that test when the file's real last 4 KB are
  zeros. Disk images and some video and audio files end in padding like
  that.
- Once the files are on SPARE, nothing can later prove that a file in the
  final archive is byte for byte what came off the SSD, after Krokiet, moves,
  the Mac trip and repairs.

**Fix:** compute a hash of each file while copying it, from the bytes read
off the disk, and write it into rescue-log.csv. Then:
- a resumed run can compare whole files cheaply;
- step 6 can confirm every copy;
- at the end, the archive can be checked against the list, on any machine,
  at any time, without the SSD.

---

## 7. Smaller points

- **`._` files and Thumbs.db.** Keep them, next to their partner files.
  - A `._` file holds the Mac's extra information about a file: tags,
    comments, "where from". For classic Mac files it also holds the
    resource fork; for an old font suitcase or a classic document that is
    the whole file, and its data file is 0 bytes. Krokiet's Empty Files and
    the probe's "TRUNCATED: zero bytes" (probe.rs line 63) would both treat
    that data file as junk.
  - Thumbs.db can hold the only intact (small) picture of a photo that is
    now damaged. So can a Photos Library's previews and thumbnails.
- **Long paths.** The copies keep the original paths, and some may be
  longer than Windows' usual 260-character limit. File Explorer, and older
  tools such as MP3val, can fail on them or skip them, which is one more
  reason not to sort by moving.
- **"Lost data (no name)" pieces aren't joined with the map check.**
  map-check-files.tsv lists only named files (mapcheck.rs line 1155). So a
  lost piece that contains a never-read place is caught only if its zeros
  happen to be 4 KB or more.
- **Created dates aren't copied.** rescue.rs line 296 sets only the modified
  time; `std::fs::FileTimes` can also set created and accessed times on
  Windows. Finder and Photos use created dates for files without EXIF.
- **Deleted entries are ignored** (exfat.rs line 485). That's by design, but
  if the SSD was ever mounted and "repaired" after the copy (next point),
  files the repair deleted would be among them.
- **The SSD's history.** Was it ever plugged into the Mac, or any computer,
  after ddrescue finished on 24 April 2024? Computers write index folders to
  drives when they're plugged in, and macOS can also check and repair exFAT
  drives. Step 4 could show the newest date found on the volume. Anything
  after 2024-04-24 means the SSD changed after the copy, and the mapfile no
  longer describes it exactly.
- **.Trashes** on the volume holds whatever was in the Mac's Trash for that
  drive. It is copied, but the probe skips the folder (formats.rs line 137),
  so nothing in it is checked.
- **"Probably iCloud stand-ins"** (report.rs line 12) is a guess based on the
  typical file size being under 500 KB. A library of genuinely small photos
  (early digital cameras, scans, screenshots) is flagged the same way. Check
  on the Mac before trusting it.
- **PAR2 can't cover more than 32,768 files per set** (par2 manual). So
  Phase 9 step 3, "one set per top-level folder", fails for big folders. Two
  copies on different kinds of drive, one of them away from home, plus the
  checksum list from point 6, protect better and are simpler.
- **Phase 5.**
  - Check that the Mac has no NTFS write driver (Paragon, Tuxera, Mounty);
    with one, it *can* write to SPARE.
  - When opening an old Photos Library, don't make it the System Photo
    Library, and don't turn on iCloud Photos for it.

---

## Better tools, and what to use alongside

| Tool | Reads the ddrescue mapfile and names affected files | Rebuilds damaged / orphaned exFAT folders | Notes |
|---|---|---|---|
| Recovery Probe | Yes (step 4) | No (finding 5) | Windows, step by step, free. New code, tested only on synthetic disks and virtual drives. |
| UFS Explorer Professional Recovery / Technician | Yes: imports GNU ddrescue maps and marks files with unimaged blocks | Yes | Windows. From $699.95 / $469.95 (time-limited). No ready-made list of affected files: they are marked in the browser. |
| ddrutility `ddru_findbad` | Yes | No | Free, Linux, command line. Not confirmed here which file systems it handles; test it on the image first. |
| R-Studio | No: it shows unread sectors for its own imaging maps, but users reported in 2019 that it can't use ddrescue maps to pick out files | Yes | Commercial. |
| DMDE, TestDisk, PhotoRec | No | DMDE and TestDisk read exFAT; PhotoRec carves files out of nameless data | DMDE is cheap; TestDisk and PhotoRec are free. |
| Hardware write blocker | - | - | Stops all writes in hardware (findings 3 and 4). |

**Recommendation:** don't replace the program, and don't rely on it alone.

- Image first (finding 1).
- Run the program's steps 4 and 5 from the image.
- Then cross-check once with one mature tool on the same image: compare the
  folder and file counts, check a sample of files, and let it rebuild any
  orphaned folders (finding 5).
- Run PhotoRec over anything left in "lost data (no name)".

## Is the plan sound? Is it too much?

What the person wants is reasonable, and the program can deliver the core of
it. Most of the length of the plan isn't needed for this failure:

- **The damage is tiny and exactly known.** ddrescue failed to read 610,816
  bytes in 167 places, about 0.00006% of the drive. That can damage the
  contents of at most a few hundred files, and step 4 names them. If some of
  those places fall on folder records, more files can lose their names
  (finding 5), and step 4 shows that too. Everything else was read without
  error and is as it was on the old drive.
- **The rest of the plan mostly checks for problems this failure didn't
  cause:** the Krokiet, 7-Zip and MP3val passes (Phases 2 and 4), decoding
  every HEIC and RAW on the Mac (Phase 5), the three-folder sort (Phase 6)
  and PAR2 (Phase 9). It takes days, and it carries most of the risk found
  in this review (findings 2 and 7). The step 6 scan is still worth running
  once: it is automatic, changes nothing, and finds files that were already
  broken before the drive died.

**A simpler plan that keeps all the safety:**

1. Image the SSD onto a new drive. The SSD goes back in the drawer.
2. Map check from the image (step 4).
3. Copy everything from the image into SPARE\WORK (step 5), with checksums.
4. Cross-check once with an independent tool. Rebuild orphaned folders if
   step 4 shows any.
5. Scan once (step 6) for a report. Don't move or delete anything in WORK.
6. For the short list of damaged files only: copy them into REPAIR, try
   near-twins (Phase 8) and repair tools (Phase 7), and keep the originals.
7. Two copies of WORK, one of them away from home. Check them against the
   checksum list once or twice a year.

## Fixes the program would need (only if the person agrees)

Small:
- Keep OfflineAll while the SSD is attached (finding 4).
- Show "removable" and refuse a removable recovered SSD (finding 3).
- Copy created and accessed dates.
- Join lost pieces with the map check.
- Show the newest date on the volume.

Medium:
- A "make an image" step, and steps 4-5 working from an image (finding 1).
- A hash per file in rescue-log.csv, checked when resuming and in step 6
  (finding 6).
- The checklist rewritten along the simpler plan above (findings 2 and 7).

Larger:
- Rebuild orphaned folders from unclaimed clusters (finding 5). Or leave it
  to a mature tool if step 4 shows a large "data no folder names" figure.

Each change needs a Windows CI run. Batch them, because the repository's
Actions minutes are limited.

## Note: a mistake made during this review

The first push of this `review` branch started the CI workflow. The run was
cancelled after about 30 seconds, but its publish step (`if: always()`)
still force-pushed near-empty "cancelled" results over `ci-windows` and
`ci-linux`. Their exact earlier contents, including the tested exe, are in
`ci-backup/` on this branch, with the two commands that put them back. Every
later commit on this branch carries `[skip ci]`. The workflow might be
better off publishing only from `dev`.

## What was fixed (19 October 2026, on `dev`)

The person agreed to the fixes. All of the list above is done, and checked on
test disks (the Windows-only parts on GitHub's Windows machine):

- **Finding 1:** a new step 4 makes an image of the SSD (one read, front to
  back, with its SHA-256, read back to check it; places the SSD can't read
  are recorded the way ddrescue does and counted by the map check). Steps 5
  and 6 then read the image. The checklist adds the IMAGE drive.
- **Finding 2:** the checklist no longer deletes or moves anything: verdicts
  go into reports, damaged files are copied into REPAIR, packages and Photos
  Libraries are kept whole, and Krokiet's Trash/Delete/Move are never used.
- **Finding 3:** the step 3 list marks disks Windows calls removable media;
  for a removable recovered SSD the page says to make the image at once and
  ask before plugging it in again. The checklist adds the dress rehearsal
  with SPARE.
- **Finding 4:** Windows stays set to leave new disks switched off until no
  disk that could be the recovered SSD is attached; "Put Windows' setting
  back" refuses until then, and closing the window doesn't put it back
  either while the SSD is attached.
- **Finding 5:** folders whose own record was lost are found by their
  contents and copied as `found folders (name lost)\folder at cluster N`,
  with every file's name, dates and sub-folders. In the table above,
  Recovery Probe's "Rebuilds orphaned exFAT folders" is now "Yes, when the
  folder's own clusters are intact". (On 500 random test disks, 431 of 444
  rebuilt folders came out with exactly their own clusters; the rest lost
  part of a folder to the damage, or took in the next folder's records when
  one folder's records exactly filled its last cluster — never a file's
  contents.)
- **Finding 6:** every copy's SHA-256 is in rescue-log.csv; a resumed copy
  keeps an earlier copy only if the whole file matches; step 7 has "Check
  the copies are unchanged".
- **Point 7:** created and accessed dates are copied; the map check shows
  the newest date on the volume and warns if it is after the copy; lost
  pieces are joined with the map check; the checklist covers `._` files,
  Thumbs.db, long paths, the iCloud guess, PAR2's limit and the Mac's NTFS
  add-ons and iCloud Photos.

---

# Second review (21 October 2026)

A fresh session reviewed version 2026-10-21 (1) (81e3460, the build on the
PC, checked: sha256 10614db6...). Nothing in the program writes to the
recovered SSD, and every test passed (here: unit tests, self-test, 352
random damaged disks). Found, and fixed in 2026-10-21 (2) at the user's
request:

1. **A 1 TB SPARE would have been taken for the recovered SSD.** The copy
   was recognised only by its partition table being made for the old
   drive's size, which every "1 TB" drive shares. In the rehearsal SPARE
   could not be switched on writable; Windows' setting could not be put
   back while it was plugged in; and if left chosen, step 4 would have
   imaged SPARE and called the SSD not needed. Now the copy must also be
   bigger than the old drive or hold the exFAT volume, everywhere.
2. Step 4 said the SSD was not needed before the image's read-back check.
3. The image's checksum (and list of unreadable places) was written after
   the image got its name, unsynced: a crash in between left an image that
   could be neither checked nor carried on.
4. diskpart ran (as administrator) from a script file with a predictable
   name in the temporary folder.
5. The Obsidian checklist, repair-list.csv and the repaired file being put
   back were not made safe on the drive before replacing what was there.
6. Checkpoints compared local times as text: wrong in the hour the clocks
   go back (25 October 2026).

See NEXT-STEPS.md, "Second review and its fixes", for how.

---

# Third review (22 October 2026)

A fresh session reviewed version 2026-10-21 (2) (8d0eb83 on `main`, the build
on the PC). What follows is the review as given to the user, then what was
done about it. The user then asked: "make fixes 2 and 3 plus the checklist
wording in one new build, and add this review to the project's review notes
so later sessions can find it. Do whatever fixes you think are necessary."
That became version 2026-10-22 (1).

## The review, as given

**Verdict:** the project is in good shape; nothing stops Phase 1. Before the
recovered SSD is plugged in, two things about how the plugging-in and the
image are done should change.

**What was checked**

- The program on the PC is the tested one: `C:\RECOVERY-TOOLS\recovery-probe.exe`
  is byte for byte the build GitHub tested (4,154,880 bytes, sha256
  4f1145962c3e085e...). `main` differed from it only in the notes file.
- Nothing in the program can write to the SSD: only the protection code
  (`guard.rs`) opens a disk for writing, and it only changes Windows' on/off
  and read-only switches.
- Every test passed here too: 69 unit tests, the self-test, and 52 of 52
  test disks for the map check (40 with random damage).
- The earlier reviews' fixes are all in: image first, nothing deleted, a
  checksum for every file, the safety checks, put-back that is safe.

**1. Windows remembers USB adapters, so the rehearsal needs the very same
adapter and port.** Microsoft documents that the "leave new disks switched
off" decision (the SAN policy) is made when a disk is first identified, and
then remembered per device (the Partmgr `Attributes` value). GitHub's tests
cannot show this: virtual disks are new to Windows every time. So:

- Phase 1 step 10 said "the same *kind* of case or adapter"; it must be the
  very same case or adapter, in the same USB port.
- SPARE has been plugged into this PC before, so in the rehearsal it may
  come up with a drive letter without the adapter being unsafe: Windows may
  just remember SPARE. The checklist's "stop" is still right; the model name
  step 3 shows for SPARE tells which: SPARE's own drive name means the SSD,
  which this PC has never seen, should still arrive switched off; the
  adapter's own name means the SSD needs an adapter this PC has never seen.
- The 4 TB SSD needs the same care, if it is ever needed.
- After the rehearsal SPARE may keep arriving switched off on this PC; then,
  at Phase 2 step 6, step 3's "Switch disk N on, writable".

**2. The image step can get stuck on a weak spot.** The SSD sat unpowered for
about 2.5 years. Tested with a test disk with a weak spot: a stretch that
fails is re-read in 4 KB pieces - up to 2,048 failed reads per 8 MB - with
the bar not moving, and Stop took effect only after all 2,049 failed reads
(on a real drive each can take seconds: hours of strain on a weak SSD). And
if the weak spot makes the USB adapter reset, every "carry on" stopped at
exactly the same place (four times out of four in the test). Interim advice:
if the bar does not move for about 10 minutes, or the SSD stops answering
twice at the same point, close the program and ask for help.

**3. Nothing compared the program's reading of the real disk with anything
else.** All the numbers come from the program's own exFAT reader, tested
only on made-up disks. A file it misses shows up as "data no folder names
any more", but a record it misreads would pass every check. Shown with a
test disk with no damage and one leftover record after a folder's end (a
"ghost"): the program reported a file "Windows cannot list", copied the
leftover under its name with a different file's contents, and every check
passed. Unlikely on a Mac-made disk, but cheap safeguards exist:

- trust "hidden behind a blank record" only when that blank is on a place
  ddrescue never read;
- use the volume's own allocation bitmap to flag clusters claimed twice, or
  claimed though marked free;
- once, after the image is made: open it read-only (OSFMount) and compare
  Windows' own listing with step 5's; "Check the image is unchanged" then
  shows the image was not touched.

**Smaller points**

- The program wrote and deleted a test file in `C:\RECOVERY-TOOLS` every few
  seconds (the page's polling); a failed write (antivirus) could make it
  lose track of the Windows setting it changed.
- The program itself did not refuse to switch off or protect Windows' own
  disk; only the page stopped it being chosen.
- With clusters of 32 KB or less, steps 5 and 6 could be slow (the FAT
  cache was emptied whole whenever it held 64 MiB). A Mac normally uses
  128 KB clusters on a drive this size, which is fine.
- An image stopped and carried on was checked against the file, not the
  SSD, for the part made before the stop (the checksum was worked out again
  from the file).
- `rescue-log.csv` may be looked at in Excel but never saved from it: the
  safety checks would then fail.
- The checklist's "a SATA port inside the PC" clashes with "never restart
  with the SSD plugged in", unless that port allows plugging in while the
  PC is on.

Sources given: Microsoft, "How to set the Partmgr Attributes registry value"
(support.microsoft.com/en-us/topic/8b948867-15b2-0612-a409-0ef6e100025e: the
setting "is initially set when a disk is identified based on the SAN Policy
value in place at the time", stored for each device); Microsoft,
VDS_SAN_POLICY (learn.microsoft.com/en-us/openspecs/windows_protocols/ms-vds/81e7ff81-71d7-408f-a800-38a0dd2828de:
"All newly discovered disks remain offline and READONLY").

## What was done (version 2026-10-22 (1))

1. **The checklist** (Phase 1 steps 10, 11, 13, 15, 16, a new step 19 and its
   section; Phase 2 steps 1, 2, 6; the drives table; the checks table; "What
   this catches"; Phase −1 section D gets OSFMount; the manifest gets
   OSFMount): the very same case or adapter, port and cable; what SPARE
   arriving with a drive letter can mean, and what to note; the SATA port
   only if it allows hot-plugging; the 4 TB SSD the same way; SPARE perhaps
   switched off in Phase 2; what the image does now at a weak spot, and
   when to stop; open the image read-only with OSFMount and read step 5's
   comparison with Windows; Excel: look, never save. Ticked lines are
   unchanged (all 31 ticks carry over).
2. **The image at a weak spot** (`image.rs`): a piece that will not read is
   read 4 KB at a time, but only so far - after 16 failed 4 KB reads in a
   row, 64 in the piece, or 5 minutes on it, the rest of the 8 MB piece is
   stepped over unread ('?' in the list beside the image; '-' tried and not
   read; both zeros, both counted as never read by the map check). Stop
   works at any moment (the main loop no longer waits on the reader), and
   the page hears how the careful reading is going every 5 s. A place where
   the SSD stops answering is noted beside the image: the next run reads
   that piece 4 KB at a time; if it stops answering there again, the run
   after steps over the rest of the piece from that place - so the image
   always gets past it, in at most three runs. The checksum's state is
   saved with each safe point, so the SHA-256 always comes from the SSD's
   own bytes, also across stops (the read-back then catches a part changed
   on the PC's drive in between). The checked image is marked read-only.
3. **The map check checks itself** (`exfat.rs`, `mapcheck.rs`):
   - every entry hidden by a blank record carries where that record is
     (`Entry::hidden_by_blank`, inherited by a folder's contents); one whose
     blank record was read is a *leftover*, reported apart ("most likely a
     leftover of something deleted or replaced long ago, not something the
     damage hid"), not counted among the files Windows cannot list, and
     left out of the join's HIDDEN lines; the wording no longer calls the
     blank record "never-read";
   - clusters claimed twice (with who first and who again) and clusters
     claimed that the allocation bitmap marks as free - where that part of
     the bitmap was read - are reported (summary, `map-check-consistency.csv`,
     JSON, page);
   - when the volume has a drive letter here (found by its serial number:
     the image open in OSFMount, or the SSD switched on read-only), every
     file and folder Windows lists is compared with what the check says
     Windows can list, sizes included, and whether that volume is read-only
     (`map-check-windows-compare.csv`, JSON, page); `--compare-with` names
     another drive or folder.
4. **Smaller points:** the program's folder is found once (no test file every
   poll); `guard.rs` refuses to change Windows' own disk, from anywhere; the
   FAT cache keeps 128 MB and lets the oldest 1 MB block go, one at a time;
   the image's checksum carries across stops (above); the Excel and SATA
   notes are in the checklist.

**Found on the way:** `fsck.exfat` (exfatprogs) calls any record after a
folder's end an error ("other entry follows unused entry"), while
`dissect.fat` lists such records as files (it reads on past the end).
Windows stops at the end (measured: `tools/windows_check.py`, which now
also attaches a disk with leftovers). The program follows Windows, and now
says when such a record is a leftover.

See NEXT-STEPS.md, "Third review and its fixes", for how, and for the tests.
