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
it) and opens a page in the PC's own browser, on the **Automatic** tab:

1. **Get ready** - checks the PC (administrator rights, the mapfile, the
   self-test, a restart Windows is waiting to do, "Automatically restart"
   after a crash) and the open-source tools, with an Install button for AIM
   Toolkit and a Download button for qpdf.
2. **The rehearsal with SPARE** - makes Windows leave new disks switched off,
   then looks at what Windows does with SPARE plugged in exactly the way the
   recovered SSD will be (the same case or adapter, port and cable): arrived
   switched off, or remembered by its own name - or removable media, or the
   adapter's name: stop.
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
   kept), the final check of both copies and of the image, and Windows'
   settings the program changed put back. Run again twice a year.

The checklist (now 30 boxes, `docs/automatic-checklist.md`) and the program's
steps one at a time are the other two tabs. The page shows each stage's results
in the steps tab as if its button had been clicked, and both programs read the
same results folder, so Recovery Probe can carry on from anything this one did.

## What never changes

- Nothing is ever written to the recovered SSD or the image. The SSD is read
  once, read-only, switched off for Windows.
- The open-source tools never see the SSD or the image: the checks run them
  only with options that read, on the copies in WORK; the repairs run them on
  copies of copies, in a folder of their own.
- Nothing in WORK is moved, renamed or deleted, except by put-back, which
  replaces a damaged copy with its repaired version only if the copy is still
  exactly the damaged original, keeps the original in `REPAIR\originals`, and
  lists both checksums in `WORK\repair-returns.csv`.
- Any check that finds something stops Start, and says why.

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
python tools/auto_check.py target/release/automatic-recovery-probe work
```
