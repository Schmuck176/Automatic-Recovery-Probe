# The tools Automatic Recovery Probe uses — where they came from, what they're for

*Version 2026-10-23 (1) — goes with `automatic-recovery-checklist.md` and `automatic-recovery-probe.exe`.*

**Get ready → Check this PC** shows each one, found or not, with its version. Every tool is optional: without one,
what it does is left out (and said so), and everything else still works. None of them ever touches the recovered
SSD or the image: the checks run them only with options that read, on the copies in `WORK`; the repairs run them
only on copies of the damaged files, in `REPAIR`.

**Software cost: $0.** Only the paid repair tools you may decide on in Phase 4 cost money.

> If you ever download one again, **type the address below by hand** — never through a search result or a
> "download site" (Softonic, Uptodown, CNET, FileHippo, Softpedia wrap programs in junk). If a download worries you,
> upload it to **virustotal.com** first.

## Where the program looks for them

In this order: its own `tools` folder (`C:\RECOVERY-TOOLS\tools`, where it downloads what it can), where each
program normally installs itself (7-Zip in `C:\Program Files\7-Zip`, FFmpeg in `C:\ffmpeg`), the PATH, and then
anywhere a few folders down in `C:\RECOVERY-TOOLS` (your `installed files` folder).

## The tools

| Tool | What the program uses it for | Where it comes from | You have |
|---|---|---|---|
| **7-Zip** | Tests every archive, and every Word, Excel, PowerPoint and ePub file (they are zips inside); rebuilds a damaged one from the parts that read back right; takes out what can be read from damaged archives | **https://www.7-zip.org** (and its GitHub, github.com/ip7z/7zip) | `7z2603-x64.exe`, installed ✔ |
| **FFmpeg** | Plays every photo, song and video through to find damage the structure check can't see (8.1 and later also iPhone HEIC photos); rebuilds a damaged video's or song's container; re-encodes a damaged FLAC losslessly | **https://ffmpeg.org/download.html** → Windows → the gyan.dev or BtbN builds | `ffmpeg-9.0.1-essentials_build`, in `C:\ffmpeg` on the PATH ✔ |
| **MP3val** | Reads every MP3 frame by frame (a cut-short MP3 still claims its full length); rebuilds broken frames and headers | **https://mp3val.sourceforge.net** | `mp3val-0.1.8` in `installed files` ✔ |
| **untrunc** | Rebuilds a video (MP4, MOV) that lost its index or stops part-way, using a good video from the same camera as the pattern — the program picks it (same folder, same camera name, same kind, made around the same time) | **https://github.com/anthwlock/untrunc/releases** | `untrunc_x64` in `installed files` ✔ |
| **qpdf** | Checks every PDF and every stream in it; rebuilds a damaged PDF's table of where everything is | **https://github.com/qpdf/qpdf/releases** — the program can download version 12.3.2 itself (`qpdf-12.3.2-msvc64.zip`, 24,555,583 bytes, SHA-256 `8941870a604e7c87ed24566b038d46c24ce76616254d2383c578f60c0677f202`, as GitHub publishes it; anything else is thrown away) | new: **Download it** in Get ready |
| **AIM Toolkit** | Opens the image read-only as a drive, so the map check can check its own reading against Windows'; closed again straight after | **https://sourceforge.net/projects/aim-toolkit/files/** → `AIMtk.zip` (by the author of ImDisk Toolkit; Arsenal Image Mounter's driver, github.com/ArsenalRecon/Arsenal-Image-Mounter, open source) | `AIMtk.zip`, unzipped in `installed files\AIMtk` — Get ready's **Install AIM Toolkit** runs its installer ✔ |

**Krokiet, MultiPar, TeraCopy and Obsidian** aren't needed by the program any more: the extra checks Krokiet made
are made by FFmpeg, 7-Zip and MP3val directly, and the copying and the second copy are the program's own, checked
against the checksums. Keep using any of them for anything else if you like them. Obsidian still shows this
checklist, kept the same as the program's.

## On the Mac (nothing to install)

`Repair on the Mac.command` (in `hand-off\1 For the Mac`, made by the program) uses only what every Mac has: `sips`
(the same decoder as Photos, for iPhone photos and camera RAW) and `avconvert` (the Mac's own video framework).
It never changes the files it tries, and saves what it repairs into `returned` beside itself.

For Photos libraries, the old checklist's Phase 7 still holds: **osxphotos** (https://pypi.org/project/osxphotos/)
to copy everything out of a library first, and Photos' own Repair only on a copy.

## Paid repair tools (only if you decide to)

**Stellar Repair for Photo / Video** (https://www.stellarinfo.com) and **Wondershare Repairit**
(https://repairit.wondershare.com): roughly $40–70, usually a yearly subscription. Both show a free preview of the
repaired file before you pay: **look at the preview first** — if it doesn't show the file repaired, paying won't
repair it either. Use them only on the copies in `hand-off\2 For a paid repair tool`, and save what they repair into
`hand-off\returned` keeping the number in its name.
