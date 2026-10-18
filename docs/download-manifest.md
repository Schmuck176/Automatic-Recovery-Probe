# Download Manifest — the tools, where they came from, what they're for

*Version 2026-10-18 — written with Claude Opus 5.5. Goes with
`recovery-checklist.md` and `recovery-probe.exe`. Any copy with an older date is an
old draft — delete it.*

You've already downloaded everything below. This page is here so you know what each
one is for, and where to get it again if you ever need to. **Installing is covered in
the checklist, Phase −1 section D.**

**Total cost: $0.** Only the optional repair software in Section 4 costs money, and
you decide about that in Phase 7.

---

## ⛔ Fake download sites

Searching for these tools turns up impostor sites and "download managers" that wrap
the real program in junk. If you ever need to re-download anything, **type the
address below by hand** — don't search for it.

| Fake | Real |
|---|---|
| `czkawka.net` | github.com/qarmin/czkawka (the developer says there is **no** official website) |
| `teracopy.net` | codesector.com |
| Softonic, Uptodown, CNET, FileHippo, Softpedia | always the maker's own site |

If a download ever worries you, upload it to **virustotal.com** first (free, no
account — it runs ~70 virus scanners on it).

---

## 1. Recovery Probe — the program at the centre of it

### `recovery-probe.exe`
- Made for this recovery, by Claude, and put into `C:\RECOVERY-TOOLS\` by Claude. Nothing to install: double-click it.
- Its source is in your GitHub repository **Schmuck176/Recovery-Probe** (private). GitHub's own Windows machines build it from that source and test it on every change: against the old Python probe file by file, on hundreds of damaged test disks, against what Windows itself makes of them, and in a real web browser.
- It uses only Rust's own standard parts and Windows' own parts — no code from anyone else runs over your files. It reads the Photos Library databases with SQLite, which is part of Windows (`winsqlite3.dll`).
- It never writes to the recovered SSD. It writes only into the folder you choose for the copies and into `C:\RECOVERY-TOOLS\results\`.
- What it does: checks itself (step 1); reads the mapfile (step 2); keeps Windows from writing to the recovered SSD (step 3); works out which files and folders sit on the places the old drive never gave up, and which files Windows can't even list (step 4, "the map check"); copies everything off, including those (step 5); checks every copied file's structure, together with what step 4 found (step 6 — this is the old "probe", plus NEVER_READ).
- **Python** is no longer needed (the old probe was a Python program). Already installed? It does no harm.

## 2. Programs that other programs need

### Visual C++ Redistributable — needed by Krokiet and MultiPar
- **https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist** — you have both `VC_redist.x64.exe` and `VC_redist.x86.exe` ✔. Install both.

### ffmpeg — the engine behind Krokiet's video checks
- **https://ffmpeg.org/download.html** → Windows → the **gyan.dev** or **BtbN** builds (the ones ffmpeg.org itself links to). You have `ffmpeg-9.0.1-essentials_build.zip` ✔
- Krokiet uses two programs from it, `ffmpeg.exe` and `ffprobe.exe`, for its **video** checks only. (Its audio check is built in and doesn't need ffmpeg.)
- **Not automatic.** It has to be unzipped and added to your PATH. If it isn't, Krokiet lists **every video** as broken with "Failed to run ffmpeg". The checklist walks through it step by step (Phase −1, section D) and section E tests it — Krokiet's **Diagnostic Info** also shows whether it found FFmpeg and FFprobe.

---

## 3. The other tools

### Krokiet — does about five jobs
- **https://github.com/qarmin/czkawka/releases** — you have the folder `windows_krokiet_on_windows_all_backends` (version 12.0.1, released 29 July 2026) ✔
- That's an **official** build: `windows_krokiet_on_windows_all_backends.zip` is one of the files on the 12.0.1 release page. ("all_backends" means it bundles several ways of drawing its window — if the `.exe` doesn't open, try the `.bat` files next to it.)
- Nothing to install: it runs from its folder.
- Used for: junk and empty files, exact duplicates, broken-file checks (images, PDFs, archives, audio, video), bad extensions, similar images. Its **Save**, **Trash** and **Move** buttons are what make Phases 2 and 6 manageable.

> ⚠️ **Krokiet ignores files smaller than 16 KB** in every scan except Empty Files,
> until you change *File Size (Kilobytes) → Min* to **0**. Phase 2 does this first.
>
> ⚠️ **Broken Files:** audio, image, PDF, archive, font and markup checks are **on**
> by default; the two **video** checks are **off** until you tick them (Phase 4).
>
> ⚠️ Its **archive** check covers `.zip`, `.jar`, `.7z`, `.gz`, `.tar`, `.zst`,
> `.bz2` and `.xz` — **not** Word, Excel, PowerPoint, ePub or RAR files. 7-Zip
> (below) is what tests those.
>
> ⚠️ **On Windows it cannot decode HEIC photos or camera RAW.** (The official Mac and
> Linux builds have HEIC versions; the Windows ones don't.) Section 3 covers how those
> get checked.
>
> ⚠️ Unofficial "extra codecs" rebuilds of Krokiet exist. They aren't from the Krokiet
> developers. Don't run an unofficial program over a million irreplaceable files — use
> this build plus the Mac.
>
> ⚠️ **Never point Krokiet at a Photos Library.** See the checklist's first page.

### 7-Zip — proper archive testing
- **https://www.7-zip.org** — you have `7z2603-x64.exe` ✔
- Checks the *contents* of zip files — including Word, Excel and PowerPoint files and ePubs, which are zips inside and which Krokiet doesn't open at all — plus 7z and RAR.

### MP3val — the one thing nothing else catches
- **https://mp3val.sourceforge.net** — you have `mp3val-0.1.8_with_frontend-0.1.1-bin-win32` (the version with the graphical frontend) ✔
- Nothing to install: run the frontend `.exe` from its folder.
- An MP3 that's been cut short still claims its full length, so players and Krokiet can't tell. MP3val counts the actual audio. Twenty minutes in Phase 4.
- It also has a repair button. Don't use it in Phase 4 — scanning only. Repairs belong in Phase 7, on copies.

### TeraCopy — copying that checks itself (optional now)
- **https://codesector.com/teracopy** — you have `teracopy.exe` ✔
- Recovery Probe now does the copying off the recovered SSD: TeraCopy, like File Explorer, can only copy what Windows can list, and some of your files sit in folders whose records the old drive never gave up. Keep TeraCopy for ordinary copying if you like it.

### MultiPar — protection against slow rot, for the end
- **https://github.com/Yutaka-Sawada/MultiPar/releases** — you have `MultiPar1336_setup.exe` ✔
- Makes "PAR2" files that can detect *and repair* files that quietly change over the years. Used in Phase 9.

### Obsidian — for reading this checklist
- **https://obsidian.md** — you have `Obsidian-1.13.7.exe` ✔
- Free. Shows these documents properly and lets you tick the checkboxes as you go (it saves them into the file). Ignore its Sync and Publish offers — you don't need them.

### untrunc — repairs cut-off videos (Phase 7)
- **https://github.com/anthwlock/untrunc/releases** — you have the folder `untrunc_x64` ✔
- Free, nothing to install, no licence needed. Fixes MP4/MOV files that stop partway.
- **It needs a working video from the same camera or phone** to use as a pattern; without one, the developer says the chances are slim.

---

## 4. HEIC photos and camera RAW — nothing to download, but not optional

Krokiet can't decode either on Windows, so your iPhone photos and RAW files would go
untested there. Three things close that gap, and they catch different problems:

**The map check (Recovery Probe, Phases 1 and 3).** It knows from the mapfile exactly
which bytes the old drive never gave up, so any HEIC or RAW file that lost data is
marked **NEVER_READ** — whatever its format, and however small the loss.

**The probe (Recovery Probe, Phase 3).** It checks the *structure* of every HEIC and RAW
file — whether the file is cut short, or has a bad-sector hole punched through it.
But it will **never report a HEIC or RAW as fine**, because it can't actually decode
them. They come back as *not testable*, are listed in the probe's `-not-testable.csv`,
and go to the `UNSURE` pile unless the Mac passes them. Expect that pile to be most of
your photos. That's the probe being honest, not failing.

**`sips` (Phase 5, on the Mac).** Already part of macOS. It uses the same decoder as
Apple's Photos app — the most authoritative one there is for iPhone HEIC and camera
RAW. Phase 5 has the exact commands. It only reads the drive.

*If you flatly refuse to involve the Mac:* ImageMagick for Windows
(**https://imagemagick.org/script/download.php#windows**, free) is the fallback, but
it's a weaker check on RAW — it can pass a file whose preview is fine while the real
image data is damaged. Say so and we'll weigh it properly.

**The Photos Library database** is checked by the probe with no extra download
(Windows includes the SQLite it needs).

### If a library's database is beyond repair
Only relevant if Phase 5 finds that Photos won't open a library and Repair can't fix it:

- **osxphotos** — reads a Photos database directly and exports the photos *with* their albums, keywords and dates. Free; a one-person project, but well regarded and the only tool that does this. Its author's recommended install, on the Mac in Terminal (needs internet):
  ```
  curl -LsSf https://astral.sh/uv/install.sh | sh
  uv tool install --python 3.13 osxphotos
  ```
  Ask Claude to walk you through it if you get there.
- It needs a *partly* readable database. If the database is completely gone, you keep the pictures (the files in `originals/`) and lose the albums.

---

## 5. Repair software — the only things that cost money

Decide **after** Phase 6, when you know what's actually broken. Both need internet to
activate, even for a trial — that's why Phase 7 is when you reconnect.

### Stellar Repair for Photo / Video
- **https://www.stellarinfo.com**
- Roughly **$40–$70**, usually sold as a **yearly subscription**. Legitimate, long-established company. (Prices change — check on the day.)
- The free trial shows you the repaired file before you pay. **Use that.**

### Wondershare Repairit
- **https://repairit.wondershare.com**
- Same kind of product, similar price and model.

**Buying advice:** never search for these — type the address. Check the renewal
terms before paying (auto-renew is normal). And if the trial preview doesn't show a
fixed file, paying won't produce one either.

---

## What each tool covers, at a glance

| | Checked by |
|---|---|
| **Any file the old drive lost data from**, whatever its type | **Recovery Probe's map check** (Phase 1), marked NEVER_READ (Phase 3) |
| Files Windows can't even list | Found by the map check and copied by Recovery Probe (Phase 1) |
| Ordinary photos (JPEG, PNG, GIF, WebP, TIFF) | Recovery Probe (Phase 3) + Krokiet image check (Phase 4) |
| iPhone photos (HEIC) and camera RAW | Recovery Probe structure (Phase 3) + `sips` full decode (Phase 5) |
| Video (MP4, MOV, MKV, WebM, AVI…) | Recovery Probe (Phase 3) + Krokiet video decode (Phase 4) + untrunc for repair (Phase 7) |
| MP3 | Recovery Probe (Phase 3) + Krokiet audio (Phase 4) + **MP3val** for cut-short files (Phase 4) |
| Other audio (FLAC, OGG, WAV, M4A) | Recovery Probe (Phase 3) + Krokiet audio (Phase 4) |
| PDF | Recovery Probe (Phase 3) + Krokiet PDF check (Phase 4) |
| Zip and 7z archives | Recovery Probe (Phase 3) + Krokiet archive check + **7-Zip** test (Phase 4) |
| Word, Excel, PowerPoint, ePub, RAR | Recovery Probe (Phase 3) + **7-Zip** test (Phase 4) — Krokiet doesn't open these |
| Apple Photos Libraries | Recovery Probe finds and checks them (Phases 1 and 3) + Photos' own Repair on the Mac (Phase 5) |
| Everything else | `UNSURE` pile, spot-checked by hand (Phase 6) |
