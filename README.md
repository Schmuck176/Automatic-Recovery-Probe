# Automatic Recovery Probe

Recovery Probe, with the recovery automated.

[Recovery Probe](docs/recovery-probe-README.md) (version 2026-10-22 (1)) does
every part of getting files back off a ddrescue copy safely, and of finding
every bit of damage in them - but one step at a time, with a 130-box checklist
around it. Automatic Recovery Probe is built from the same code (the whole of
it, with its history and its tests) and does the same work from one **Start**
button, then repairs what the damage touched with open-source tools.

**Double-click `automatic-recovery-probe.exe`.** It asks Windows for
administrator rights (to read the recovered SSD directly; it never writes to
it) and opens its page in a window of its own (Microsoft Edge's app window: no
tabs or address bar; the PC's usual browser if Edge is not there), on the
**Automatic recovery** tab. The page keeps three things in fixed places: on
the left, where you are (the parts in order, each with its state); in the
middle, the guide, read from top to bottom; on the right, what is happening
now (the job running, with Stop, and what Start is waiting for you to do).
A narrower window keeps the same order in fewer columns. Every icon has words
beside it. Ctrl + and Ctrl − make everything bigger or smaller (Edge remembers).

While something runs, it shows how far it has got - a small ring and the
percentage - the time left at its speed of the last minute and a half, and when
it will be done by the clock ("done around 14:35"); Start shows the GB and files
done, kind by kind (photos, videos, music, documents, archives), and the file it
is on. The same is seen from other apps: the window's title says the percentage
and what is running (or "Your turn"), and its button on the taskbar fills with
green as it gets on - yellow while Start waits for you or the SSD cools down,
red when it stops on a problem - and flashes when it needs you. Windows
notifications (on by default: a box beside Start turns them off, and Try it
sends one) say when Start needs you, stops on a problem or is done.
**What just happened**, under "Now", tells each event in one line with its time
("07:12 Copied DCIM\2019 (412 photos, 2.1 GB)", "07:15 Found damage: ...",
"07:40 Repaired IMG_0005.JPG: ..."); `results\what-happened.txt` keeps them all.
While the image is made, a strip of the whole SSD fills in as it is read (green
read, amber read slowly, red could not be read), with how fast it reads over
time (a sagging speed is said to be an early sign it struggles) and how warm
it is, read from Windows every half minute: at 70 °C (lower if the SSD's own
warning level is lower) reading pauses until it has cooled by 10 °C. While the
repairs run, and afterwards in What is left, the photos are shown before and
after their repair (click one to see it bigger). Beside each folder: the drive it is on and its
free space, what it needs and what it holds (from the mapfile, then the map
check, then the copies), whether SPARE is switched off, and whether Defender
leaves it alone. Start tells Defender itself to leave the image's folder and
SPARE alone, and confirms it in Defender's own list; 7. Put Windows back as it
was takes that off again. Never Windows' own drive or folders, nor a whole drive
other than SPARE. Every setting the program changes has a **Show me** button
beside it that opens Windows' own page for it (Windows Security at Exclusions,
the "Automatically restart" setting, Windows Update, Disk Management, the
notification settings), and **Show what Windows itself says** opens a window
with Windows' own answers, read-only: Defender's exclusions, the policy for new
disks (`diskpart san`), the disks and whether each is switched off, the crash
setting, a restart waiting, what keeps the PC awake.

The parts of the Automatic tab:

1. **Get ready** - checks the PC (administrator rights, the mapfile, the
   self-test, a restart Windows is waiting to do, "Automatically restart"
   after a crash, Defender) and the open-source tools, with an Install button
   for AIM Toolkit and a Download button for qpdf.
2. **The rehearsal with SPARE** - makes Windows leave new disks switched off,
   then looks at what Windows does with SPARE plugged in exactly the way the
   recovered SSD will be (the same case or adapter, port and cable). Arrived
   switched off: passed. Arrived switched on (Windows remembers it - often by
   the USB case's identity, which the recovered SSD in the same case would
   share): **Switch SPARE off** switches it off and read-only and asks Windows
   to remember that; unplugged (the program watches for it) and plugged in
   again, it must arrive switched off - then so will the recovered SSD, taken
   for SPARE or as a disk never seen. Removable media, or switched on again:
   stop.
3. **The folders** - suggested: the image in `IMAGE` on the PC's second drive,
   the copies in `WORK` on SPARE, the second copy in `COPY 2`; and,
   optionally, folders holding older copies of the files.
4. **Start** - runs by itself, stage by stage, and carries on where it stopped
   after Stop, a closed window or a power cut:

   | Stage | What it does |
   |---|---|
   | ready | the checks again; Windows set to leave new disks switched off |
   | image | waits for the recovered SSD, marks it read-only by itself, reads it once into one file, reads the file back to check it (Recovery Probe's step 4) |
   | open | opens the image read-only with AIM Toolkit's command line, so the map check can compare itself with Windows' own reading |
   | mapcheck | which files the never-read places touched; stops on a date after the copy, a disagreement with Windows, records claiming space not theirs, or files that won't fit (step 5) |
   | copy | waits for SPARE (switches it on if it arrived switched off), copies everything off the image with a checksum per file (step 6) |
   | verify | every copy against its checksum, and the safety check (step 7) |
   | scan | every file's structure, joined with the map check (step 7) |
   | extra | 7-Zip tests every archive and Office file, MP3val every MP3, qpdf every PDF, FFmpeg plays every photo, song and video through (HEIC too with FFmpeg 8.1+) |
   | isolate | the damaged files copied into `REPAIR` (step 8) |
   | repair | an intact twin first (identical in every byte the drive gave up: restored exactly), then the format's own repair: JPEG end marker, untrunc with a pattern video it picks, FFmpeg remux, MP3val, FLAC re-encode, WAV header, qpdf (with a new trailer if the old one was lost), 7-Zip rebuild of zips and Office files; previews out of RAW and JPEG, what can be read out of archives |
   | putback | every repaired file back exactly where its original is; the original kept; logged; undoable (step 8) |
   | verify2, scan2, evaluate | the checks again, and what is still damaged inside every library and package |
   | handoff | what is left, ready in `REPAIR\hand-off`: for the Mac (with a script to double-click there), for a paid repair tool, for the file's own program |

5. **What is left** - every damaged file and what was done with it, and
   **Bring back**: takes in what comes back from the Mac or a paid tool (by
   the `[R0012]` number in its name), puts it back, checks everything again.
6. **Finalise** - `RECORDS` beside WORK, the second copy (resumable, dates
   kept), the final check of both copies and of the image. Run again twice a
   year. It leaves Windows' settings as they are.
7. **Put Windows back as it was** - what the program changed in Windows (new
   disks arriving switched off, "Automatically restart" after a crash if it
   turned that off, Defender's exclusions it added), each with its Show me
   button, and one button that puts them all back. It is greyed out, and the
   program itself refuses, while a job runs or anything is plugged in that
   holds a copy of the old drive (the recovered SSD, the 4 TB SSD), will not
   let itself be read (it could be the SSD), or is SPARE: with a drive still
   plugged in, the recovered SSD could otherwise one day arrive switched on.
   Nothing puts these back by itself - not Start, not Finalise, not closing
   the window - and it says to do it last, after Finalise (put back earlier,
   Start sets what it needs again when it carries on). Step 9 of the
   program's steps has the same button, by the same rule.

The checklist (now 29 boxes, `docs/automatic-checklist.md`) and the program's
steps one at a time are the other two tabs. The page shows each stage's results
in the steps tab as if its button had been clicked, and both programs read the
same results folder, so Recovery Probe can carry on from anything this one did.

## What never changes

- Nothing is ever written to the recovered SSD or the image. The SSD is read
  once, read-only, switched off for Windows. While the program is open, a
  copy of the old drive (the recovered SSD, the 4 TB SSD) that turns up
  switched on and writable - plugged in too early, or in another case - is
  marked read-only and switched off within seconds, and the page says so.
- Nothing is written to a drive that holds a copy of the old drive, whatever
  letter it has: the image, WORK, the second copy and Bring back all check
  the drive under their folder first (letters move as drives come and go).
  WORK is made only on SPARE (by its label or the disk the rehearsal saw)
  when it is on a USB drive; if SPARE comes back under another letter, WORK
  is found there again.
- The folders are checked against each other: none inside another, and the
  second copy on another drive than WORK.
- The older Recovery Probe open at the same time: the page says so, and
  nothing starts until it is closed (they share the results and folders).
- The open-source tools never see the SSD or the image: the checks run them
  only with options that read, on the copies in WORK; the repairs run them on
  copies of copies, in a folder of their own.
- Nothing in WORK is moved, renamed or deleted, except by put-back, which
  replaces a damaged copy with its repaired version only if the copy is still
  exactly the damaged original, keeps the original in `REPAIR\originals`, and
  lists both checksums in `WORK\repair-returns.csv`.
- Any check that finds something stops Start, and says why.
- Finalise checks WORK before it copies anything from it: a file changed in
  WORK since it was copied never spreads over the second copy (only what the
  second copy lacks is added, and the problem is named). With WORK as copied,
  anything changed in the second copy is put back from WORK.
- The hand-off folders never lose a file the person saved in them: a file
  with a number there that the program did not put there (a repaired version
  saved beside the damaged copy, say) is moved into `returned` for Bring back.
- Every file keeps its dates (created and modified), for sorting by date:
  the copies get the old drive's; a repaired file put back gets its
  original's, whatever tool made it (here, on the Mac or a paid tool), and
  its folder keeps its own; what is salvaged gets its original's; the second
  copy keeps them all. Inside the files: a video rebuilt by untrunc (which
  takes the index of a good video as its pattern) gets its own made date
  back, not the pattern's; FFmpeg keeps every tag of an MP4 or MOV it
  rebuilds (an iPhone's creation date and place too); a PDF given a new
  trailer keeps its information dictionary (made and changed dates, title);
  a PNG saved again keeps its tIME, text, EXIF and colour profile.

## The tools

7-Zip, FFmpeg, MP3val, untrunc, qpdf and AIM Toolkit (`docs/automatic-manifest.md`
says where each comes from). Each is optional: without one, what it does is left
out and the page says so. The program finds them in its own `tools` folder, in
their usual install folders, on the PATH, or a few folders down in its own
folder; it can download qpdf 12.3.2 from qpdf's GitHub release, checked by size
and SHA-256.

## Versions and branches

As in Recovery Probe: `dev` is where changes are made, and every push builds
and tests on GitHub's Windows and Linux machines, publishing to `ci-windows`
(with the program) and `ci-linux`; `main` is the version installed on the PC.

## How it is tested

Everything Recovery Probe's tests checked, still (unit tests, the self-test,
hundreds of damaged test disks against an independent oracle, Windows' and
Linux's own exFAT drivers, the disk protection, the page), plus:

- `tools/auto_check.py`: a damaged test disk of real media made with FFmpeg
  (videos from one camera with one's index lost, a photo cut short, one with an
  intact twin, one with a hole, a RAW with a preview, MP3, FLAC, a PDF with its
  end lost, a docx with its zip index lost) put through Start, Bring back and
  Finalise via the page's own API, every result checked against what must
  happen given the tools present.
- `tools/auto_ui_check.js`: the Automatic tab in a real browser.
- `tools/download_check.py` (on Windows): Get ready's download of qpdf, for
  real, from qpdf's GitHub release, checked by size and SHA-256.
- On Windows the tests run with the tools' own Windows builds, as on the
  PC (7-Zip, FFmpeg, qpdf, untrunc, MP3val); on Linux with the same tools
  built for Linux (untrunc from its own source).
- Unit tests of every repair, the twin rule, the tools' outputs, AIM's answer,
  the hand-off and bring back.

```
cargo build --release
cargo test --release
target/release/automatic-recovery-probe self-test
python tools/auto_check.py target/release/automatic-recovery-probe target/auto-check
```
