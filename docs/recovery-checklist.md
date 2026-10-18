# Recovery Checklist — Recovered Files → Clean SSD

*Version 2026-10-18 — written with Claude Opus 5.5 for the Recovery Probe program
(`recovery-probe.exe`). Replaces every earlier copy, including the 2026-10-12 one
that used the Python probe.*

**What you have:** a 2 TB SSD holding the ddrescue copy of your dead 1 TB hard drive
(a GUID partition map with one exFAT volume, as the Mac made it), an empty 2 TB SSD,
a Windows PC and a Mac. The old hard drive is gone, so every file on that first SSD
is the only copy in existence.

**What you're doing:** sorting about a million recovered files into three folders —
`GOOD` (proven to open), `BROKEN` (proven damaged) and `UNSURE` (nothing could
test it) — then storing the good ones so you'll know if they ever rot.

**The one rule that matters most:** an Apple Photos Library is a folder that holds
every photo *and* a small copy of every photo. A duplicate finder sees those as
duplicates and offers to delete them. **Never point a duplicate finder at a Photos
Library.** Phase 1 finds every library and moves them out of the way first.

**The second rule:** the recovered SSD is the only copy. It is never formatted, never
"repaired", never written to, and never plugged in while the PC restarts. **Every
time** it is plugged in, Recovery Probe's step 3 comes first: it makes Windows leave
the SSD switched off and read-only, so nothing — not even Windows itself — can
change it.

> **The files you need**, together in `C:\RECOVERY-TOOLS\`:
>
> | File | What it is |
> |---|---|
> | `recovery-checklist.md` | This document. Tick the boxes as you go — Obsidian saves them. |
> | `download-manifest.md` | Where each tool came from, how to install it, what it's for. |
> | `recovery-probe.exe` | **The program. Double-click it.** It checks itself, works out from the mapfile exactly which files the damage touched, copies everything off the recovered SSD without letting Windows write to it (including files Windows can't even see), and checks every copied file. |
> | `1TB hard drive DDRescue-GUI second attempt.log` | The mapfile from the final ddrescue run: the list of every place on the old drive that could never be read. **Keep it forever.** |
>
> The program's page says **Version 2026-10-18** (or later) at the top. The older
> files `recovery_probe.py`, `probe_selftest.py`, `RUN_PROBE.bat` and
> `RUN_SELFTEST.bat` aren't needed any more — the program does all of that and more.
> You can delete them, or leave them; they do no harm.

> **How the program works:** double-click `recovery-probe.exe`. Windows asks
> *"Do you want to allow this app to make changes to your device?"* — click **Yes**
> (it needs that to read the recovered SSD directly; it never writes to it). A black
> window opens — **leave it open**, it *is* the program — and your web browser shows
> the program's page: seven numbered steps, each with one button. Nothing on the page
> goes to the internet; it works offline. Close the black window when you're done.
> What each step found is kept in `C:\RECOVERY-TOOLS\results\`, so closing and
> reopening loses nothing.
>
> - Browser didn't open? The black window shows an address — copy it into the browser.
> - The page says **"The program is not running any more"**? Its black window was
>   closed. Double-click the program again.
> - *"Windows protected your PC"*? Click **More info → Run anyway** — only for this
>   file from the chat.

**Steps are numbered within each phase**, so "Phase 4, step 2" always means one thing.
🪟 = on the Windows PC. 🍎 = on the Mac. 💬 = in the Claude chat.

---

## How the machines are used

Windows does almost everything. The Mac does one session in Phase 5 (HEIC photos,
camera RAW and the Photos Libraries — nothing on Windows can check those properly).

The **original recovery SSD never goes near the Mac.** Only the working copy does,
and that's safe because macOS can only *read* a Windows-formatted (NTFS) drive, never
write to it.

## The tools

**Recovery Probe** (`recovery-probe.exe`) copies the files off the recovered SSD
and does the checks only it can do. **Krokiet** does most of the rest. Around them:
**7-Zip**, **MP3val**, **MultiPar**, **ffmpeg**, and on the Mac **`sips`**, which
is built in.

> ⚠️ **The most important Krokiet setting:** out of the box, Krokiet **ignores every
> file smaller than 16 KB** in every scan except Empty Files. Small files simply
> never get checked, and nothing tells you. Phase 2 shows the one box to change.
>
> Its Broken Files checks for audio, images, PDFs and archives are **on** by
> default; its two **video** checks are **off** until you tick them (Phase 4).

**What nothing on Windows can fully check, and how each gap is covered:**

| Gap | Covered by |
|---|---|
| HEIC and camera RAW can't be decoded on Windows | The map check names every one the drive lost data from (Phase 1); the probe checks their structure (Phase 3); the Mac decodes them fully (Phase 5) |
| A cut-short MP3 can look complete | MP3val (Phase 4) |
| Word, Excel, PowerPoint and ePub files are zips inside — Krokiet doesn't open those | 7-Zip's test (Phase 4) |

> ⚠️ Unofficial Krokiet builds with "extra codecs" exist. Don't use them — they aren't
> from the Krokiet developers, and an unofficial program is not something to run over
> a million irreplaceable files. Use the official build plus the Mac.

---

# PHASE −1 — Prepare the Windows PC 🪟

Do all of this **while still online.** Going offline is the last step.

## A. The separate Windows account — `Transfer`

You've already made this account. It's called **`Transfer`**. For the record, this is
how it was set up (check the last two lines are true):

- [ ] Settings → **Accounts** → **Other users** → **Add account** → "I don't have this person's sign-in information" → "Add a user without a Microsoft account" → named `Transfer`.
- [ ] Click the account → **Change account type** → **Administrator**.
- [ ] **Sign out of your normal account fully** (not "switch user" — that leaves it running).
- [ ] Sign in as **`Transfer`**. Everything from here happens in this account.

Why: a fresh account has no OneDrive. OneDrive seeing a million new files would try
to sync them all and can quietly swap real files for cloud placeholders.

> **Set everything up in your usual account instead of `Transfer`? Then stay in it.**
> OneDrive only syncs folders inside your user folder (Desktop, Documents, Pictures and
> the like), and nothing in this job goes there: the program and its results are in
> `C:\RECOVERY-TOOLS`, the copies go on `SPARE`. Switching now would mean installing
> Obsidian again, redoing the ffmpeg **Path** (section D, step 4 — it belongs to one
> account) and linking the Claude app again. The power, Windows Update and Defender
> settings count for every account either way.

- [ ] In the account you use: Settings → **Bluetooth & devices** → **AutoPlay** → turn **off** *Use AutoPlay for all media and devices*, so nothing offers to import photos when a drive appears.

> **Can't find your downloads?** If you downloaded the tools while signed in to your
> *normal* account, they're in *that* account's Downloads folder, not Transfer's.
> Open File Explorer, go to `C:\Users\`, open your normal account's folder →
> **Downloads** → `installed files`. Windows may say you don't have permission —
> click **Continue**. Copy the whole `installed files` folder to
> `C:\RECOVERY-TOOLS\installers\`.

## B. Stop Windows rebooting on you

An update that's already downloaded will still reboot you, even offline. So:

- [ ] Settings → **Windows Update** → **Check for updates**. Install everything.
- [ ] **Reboot.** Check again. Repeat until nothing is pending (two or three rounds is normal).
- [ ] Only now: Windows Update → **Pause updates** → longest option offered.
- [ ] Advanced options → turn **off** "Restart this device as soon as possible when a restart is required".
- [ ] `Win+R` → type `services.msc` → find **Windows Update** → Startup type **Disabled** → Stop. Try the same for **Update Orchestrator Service** — it usually refuses, which is fine.
- [ ] `Win+R` → type `sysdm.cpl` → **Advanced** → under *Startup and Recovery* click **Settings** → untick **Automatically restart** → OK. If Windows ever crashes, it then stays on the blue screen instead of restarting by itself with the recovered SSD plugged in: unplug the SSD, then restart.

## C. Stop the PC sleeping

A PC that sleeps mid-scan can corrupt a drive that's being written to.

- [ ] Settings → System → **Power** → sleep **Never** (when plugged in).
- [ ] Control Panel → Power Options → Change plan settings → **Change advanced power settings** (some of these may not exist on your PC — skip any that don't):
  - [ ] Hard disk → Turn off hard disk after → **Never**
  - [ ] USB settings → USB selective suspend → **Disabled**
  - [ ] Sleep → Allow hybrid sleep → **Off**
- [ ] Control Panel → Security and Maintenance → Automatic Maintenance → untick "Allow scheduled maintenance to wake up my computer".

## D. Install the tools

Make a folder `C:\RECOVERY-TOOLS\`. `download-manifest.md` has the links if you ever
need to download one again.

> Windows may show **"Windows protected your PC"** for `recovery-probe.exe`. That's
> because it's a new program Windows hasn't seen before, not because anything is
> wrong — click **More info → Run anyway**. Only do that for the file from this chat.

- [ ] **Python** — not needed any more: the probe is now part of `recovery-probe.exe`. Already installed? Leave it; it does no harm.
- [ ] **Visual C++ Redistributable** — run **both** `VC_redist.x64.exe` and `VC_redist.x86.exe`.
- [ ] **7-Zip** (`7z2603-x64.exe`) — run it.
- [ ] **TeraCopy** (`teracopy.exe`) — optional now: Recovery Probe does the copying (TeraCopy would miss the files Windows can't list). Install it if you like it for other copying.
- [ ] **Obsidian** (`Obsidian-1.13.7.exe`) — run it. Open this checklist in it so you can tick boxes.
- [ ] **MultiPar** (`MultiPar1336_setup.exe`) — run it.
- [ ] **Krokiet** (the folder `windows_krokiet_on_windows_all_backends`) — nothing to install. Move the whole folder into `C:\RECOVERY-TOOLS\`. You run the `.exe` inside it. *(If a window doesn't appear, try one of the `.bat` files in the same folder — each starts it with a different way of drawing the window.)*
- [ ] **MP3val** (the folder `mp3val-0.1.8_with_frontend-0.1.1-bin-win32`) — nothing to install. Move it into `C:\RECOVERY-TOOLS\`. Run the *frontend* `.exe` inside it.
- [ ] **untrunc** (the folder `untrunc_x64`) — nothing to install. Move it into `C:\RECOVERY-TOOLS\`. Not needed until Phase 7.
- [ ] **ffmpeg** (`ffmpeg-9.0.1-essentials_build.zip`) — Krokiet's video checks need it. **This one strands people, do it carefully:**
  1. Right-click the zip → **Extract All** → extract to `C:\ffmpeg`.
  2. Open `C:\ffmpeg` and keep opening folders until you see a folder called **`bin`** that contains `ffmpeg.exe` and `ffprobe.exe`. (It's usually one level down, e.g. `C:\ffmpeg\ffmpeg-9.0.1-essentials_build\bin`.)
  3. Click in that `bin` folder's address bar and copy the full path.
  4. `Win+R` → type `sysdm.cpl` → **Advanced** → **Environment Variables** → under *User variables* select **Path** → **Edit** → **New** → paste the path → OK, OK, OK.
  5. **Open a new Command Prompt** (an old one won't see the change) and type `ffmpeg -version`, then `ffprobe -version`.
  6. A block of version text for both means it worked. *"Not recognized"* means Krokiet will list **every** video as broken with "Failed to run ffmpeg" — go back to step 3 and check the path.
- [ ] Check the files from the box at the top of this checklist are in `C:\RECOVERY-TOOLS\`.

### On the Mac, while still online

- [ ] Nothing to install — `sips` is part of macOS.
- [ ] **Check the Mac's free space** (Apple menu → About This Mac → Storage, or System Settings → General → Storage). Phase 5 may need to copy a Photos Library onto it.

### Paid repair tools

Stellar and Repairit need internet to activate, even for a trial. Don't get them now —
Phase 7 is when you reconnect and decide whether you need them at all.

## E. Test everything before you disconnect

Make a test folder (for example `C:\RECOVERY-TOOLS\test-files`) with a few photos, a
video, a zip, a PDF and an MP3 (copies of ordinary files of yours — not anything from
the recovered SSD).

- [ ] **Double-click `recovery-probe.exe`** in `C:\RECOVERY-TOOLS\` → **Yes**. The page opens in your browser and the badge at the top says **Running with administrator rights**.
- [ ] **Step 1 → Run the self-test.** A few seconds. It must say **"Every check passed: the program works on this PC."** It checks itself on sample photos, music, videos, documents and a Photos database, intact and damaged, and on a small damaged test disk it carries, copying every file off it and comparing them byte for byte. Anything else → take a screenshot (`Win+Shift+S`) and paste it into the Claude chat.
- [ ] **Step 2** should already show the mapfile, with a red-and-green map of the old drive at the top of the page.
- [ ] **Step 6** on the test folder: click **Choose…** beside the folder box, open the test folder, click **Choose this folder**, then **Check every copied file**. (*Use what step 4 found* stays greyed out until step 4 has run — that's right.) Your good test files should come back **clean** (photos, PDFs, zips, videos and MP3s) or **not testable** (iPhone HEIC photos and camera RAW — always, on purpose).
- [ ] Close the black window. The page then says the program isn't running — that's right.
- [ ] **Krokiet:** open it, find **Diagnostic Info** (in its About / build information section) and check that it found **FFmpeg** and **FFprobe**. Then run **Broken Files** on the test folder with **Video (ffmpeg)** ticked. Your good video should *not* be listed. If every video is listed with "Failed to run ffmpeg", ffmpeg isn't set up (section D).
- [ ] Now disconnect from the internet.

> Honestly, section B already stops the reboots, and being offline removes your
> safety net (the Claude chat). It's your call.

---

# PHASE 0 — The mapfile 💬 — ✅ done (17 October 2026)

The mapfile is the small file ddrescue wrote next to its copy. It records exactly
which parts of the old drive could never be read. Claude read yours:

- The final run is `1TB hard drive DDRescue-GUI second attempt.log` (finished
  24 April 2024). It's now in `C:\RECOVERY-TOOLS\` — keep it there, and keep it forever.
- **610,816 bytes (about 600 KB) were never read, in 167 places.** That's the
  "relax" end of the scale: on a 1 TB drive, a few hundred files at most.
- 40 of the places are single 512-byte sectors; the typical place is 4 KB; the
  largest is 26 KB. All of them lie between 728 GB and 855 GB from the start of the
  drive (as Windows counts sizes).
- The other mapfile, `1TB hard drive DDRescue-GUI.log`, is the first attempt. It
  stopped at about a third of the drive and read none of those 167 places, so it
  changes nothing.

Because many places are smaller than 4 KB, the probe's hole check (Phase 3) can't
see all of them on its own. The map check in Phase 1 can: it works out from the
mapfile and the recovered SSD exactly which files and folders sit on those places,
whatever size they are.

---

# PHASE 1 — Copy everything off, then find every Photos Library 🪟 (about 2–5 hrs)

## Prepare the empty SSD first — on its own

> ⚠️ **Format nothing while the recovered SSD is plugged in.** Both drives are 2 TB
> SSDs, and formatting the wrong one would erase the only copy. So the empty one is
> prepared **alone**.

- [ ] **1.** Make sure the recovered SSD is **not** plugged in. Plug in **only the empty SSD**.
- [ ] **2.** Format it as **NTFS**, with the name **`SPARE`**: File Explorer → right-click the drive → **Format** → File system **NTFS**, Volume label **`SPARE`**, Quick Format ticked → Start. *(Not in File Explorer? It may be brand new. Right-click Start → **Disk Management** → if it asks to initialise a disk, choose **GPT** → right-click the empty space on that disk → **New Simple Volume** → NTFS, label `SPARE`.)* This is now the **working drive**.
- [ ] **3.** Turn off indexing on it: right-click `SPARE` → Properties → untick "Allow files on this drive to have contents indexed" → apply to all files and folders.
- [ ] **4.** Note `SPARE`'s drive letter (e.g. `E:`).
- [ ] **5. Defender exclusion for `SPARE`:** Windows Security → Virus & threat protection → Manage settings → Exclusions → **Add an exclusion → Folder** → `SPARE` (e.g. `E:\`). Otherwise Defender scans every file the program writes and checks (2–3× slower), and if it dislikes a recovered file it *quarantines* it. (The recovered SSD needs no exclusion: it stays switched off, so Defender never sees it.)

## Now bring in the recovered SSD — protected

> ⚠️ **Never restart the PC with the recovered SSD plugged in.**
>
> ⚠️ If Windows ever says **"There's a problem with this drive. Scan and fix?"**,
> click **Cancel**. Never let Windows repair the recovery drive — "fixing" can delete
> damaged files to tidy up. (With step 3 done first, Windows never gets the chance.)

- [ ] **6.** `SPARE` is plugged in; the recovered SSD is **not**. Double-click `recovery-probe.exe` → **Yes**.
- [ ] **7. Step 3 → Get Windows ready.** It answers that new disks now stay switched off. *(Plug in anything else you need, like `SPARE`, before this — it would arrive switched off too. If it does: click it in the step 3 list, then **Switch disk N on, writable**.)*
- [ ] **8. Plug in the recovered SSD.** Within a few seconds it appears in the step 3 list marked **the copy ddrescue made** and **switched off**, already chosen. Nothing appears in File Explorer — that's the point.
- [ ] **9.** Click **Protect disk N**. The page answers that the disk is read-only and Windows' setting is back to normal. Step 3 gets a green tick.
  - *Plugged it in before step 7?* Click it and **Protect** anyway: the program switches it off and on again, read-only. Then carry on.
  - *Not marked "the copy ddrescue made"?* Stop and tell Claude before going further.
  - *The page shows an error instead?* Unplug the recovered SSD, and tell Claude.
- [ ] **10. Step 4 → Find what the damage touched.** A minute or two. It reads the SSD directly and shows how many **files lost data**, how many **files Windows cannot list** (their folder record sat on a place that was never read, so File Explorer and TeraCopy would never see them), and how many **folders** are damaged. The red marks on the map at the top now show what each place held. Write the three numbers down. Everything is also saved in `C:\RECOVERY-TOOLS\results\map-check-…\`.
- [ ] **11. Step 5 → copy everything.** Click **Choose…**, click `SPARE`, then **Choose this folder**: the program puts the copies in a `WORK` folder on it (e.g. `E:\WORK`). Keep **"Also save the data no folder names any more"** ticked. Click **Start copying**.
  - This copies **every** file straight off the SSD, including the ones Windows can't list, into `SPARE\WORK`, with the same folders. It takes hours (a full 1 TB over USB: roughly 2–4). The bar at the bottom shows how far it is and about how long is left.
  - You can click **Stop** and start it again later (same folder): what's already copied is kept and skipped.
  - While steps 4 and 5 run, Windows is set to leave newly plugged-in disks switched off again (a banner at the top says so), so if the SSD's cable is knocked out and goes back in, the SSD arrives switched off rather than open for Windows to write to. The setting is put back when the step ends.
  - **If the page says the SSD stopped answering:** nothing is lost. Check its cable is in firmly. If it came out, plug it back in (it arrives switched off), click **Protect disk N** in step 3, then **Start copying** again: it carries on where it stopped. The same goes for step 4: run it again.
  - A few names change slightly, because Windows can't use them (a name ending in a space gets `_` instead). `rescue-log.csv` in `SPARE\WORK` lists every one, and every file.
- [ ] **12.** When it says **Done**, read the summary. Files that *"could only be copied in part"* are damaged — they go to `BROKEN` in Phase 6 (`rescue-log.csv` lists them as "partly copied"). Files that *"could not be copied"*: take a screenshot and tell Claude.
  - *The PC lost power or crashed during the copy?* Unplug the recovered SSD **before** you switch the PC back on. Then do steps 6–9 again and **Start copying** again into the same folder: it checks what it copied before and copies again anything the crash spoilt.
- [ ] **13. Step 7:** click **Switch it off** (if shown), then unplug the recovered SSD and put it in a drawer. It's your untouched master.

## Find every Photos Library — before anything else touches the files

- [ ] **14. Quick census.** Step 6 → the folder is already `E:\WORK` → **untick "Look for holes"** (the quick pass) → **Check every copied file**.
- [ ] **15.** When it finishes, the page lists every Photos Library it found, under **Photos Libraries found**. The results are saved in `C:\RECOVERY-TOOLS\results\`: `scan-DATE.csv` (the damaged files), `scan-DATE-photo-libraries.csv` — one row per library with everything the probe learned about it — and `scan-DATE-not-testable.csv`.
- [ ] **16. Move every library it listed into `SPARE\PHOTOSLIB\`**, each in its own numbered folder (`PHOTOSLIB\1`, `PHOTOSLIB\2`…). Just move the folders — you don't need to look inside. Moving within the same drive is instant.
- [ ] **17. Run the quick census on `SPARE\WORK` again. It must now find zero libraries.** If it still finds one, move that one and run it again. Repeat until the count is zero. This is the check that keeps Phase 2 safe.
- [ ] **18.** Open the newest `scan-DATE-photo-libraries.csv` from step 15 (Excel or any spreadsheet). A library with **`likely_icloud_placeholders` = yes** probably holds small stand-in files, not real photos — that's what "Optimize Mac Storage" leaves behind, and the real photos are still in iCloud. **Check icloud.com before spending effort on a library flagged that way.** These are listed last on purpose.
- [ ] **19.** Same file: **`damaged_inside`** is how many files in that library the probe found damaged; **`unverified_inside`** is how many are HEIC/RAW waiting for the Mac. These numbers are provisional — Phase 3 step 3 rescans the libraries with every check on. A library with a lot of damage is still your photos; it isn't a reason to give up on it.
- [ ] **20.** Moving a whole library folder keeps `Photos.sqlite-wal` and `Photos.sqlite-shm` next to `Photos.sqlite` (inside `database/`), which is what you want — they hold the newest part of the library's database. Never move files out of a library individually.
- [ ] **21.** Add up the library sizes and compare with the Mac's free space (Phase −1). You'll do them one at a time in Phase 5.

> ⚠️ A library marked **`DB_DAMAGED`** is still worth keeping whole. The photos inside
> are separate files and are usually fine; Phase 5 has a repair step. Don't delete anything.

---

# PHASE 2 — Free eliminations 🪟 Krokiet (about 1 hr)

**First, three settings in Krokiet** (they're in the same settings area as
*Included paths*):

- [ ] **Included paths:** `SPARE\WORK` only.
- [ ] **Excluded item:** add `*/PHOTOSLIB*`, `*.photoslibrary*` and `*/._*` (separated by commas). The first two are belt and braces on top of Phase 1. The third skips Mac `._` files — tiny metadata files a Mac leaves beside every file it saves to a non-Mac drive. They aren't photos, and every one would otherwise show up as a "broken" image or video.
- [ ] **File Size (Kilobytes) → Min:** change **16** to **0**. Otherwise every file under 16 KB is silently skipped by every scan below.

> **Always use the `Trash` button, never `Delete`.** Trash can be undone.

- [ ] **1. Empty Files.** Under *Additional file types to find*, **tick both**: *Files filled with null bytes* and *Files filled with non-printable characters*. Scan → **Select** → *Select all* → **Trash**.
- [ ] **2. Temporary Files.** Defaults are fine. Scan → Select all → **Trash**.
- [ ] **3. Duplicate Files**, Check method **Hash**, hash type **Blake3**. Scan → **Select** → *Select all except oldest* → **Trash**. (Every exact duplicate removed now is one you never have to test in Phase 4.)
- [ ] **4. Bad Extensions.** **Don't rename, don't delete.** Click **Save** and save the list as `C:\RECOVERY-TOOLS\bad-extensions.txt`. Used in Phase 6.

⚠️ **Don't run Similar Images yet.** That's Phase 8.

---

# PHASE 3 — The full check 🪟 (1–3 hrs, runs by itself)

- [ ] **1.** Double-click `recovery-probe.exe` → **Yes**. **Step 6**: folder `E:\WORK`, **"Look for holes" ticked**, **"Use what step 4 found" ticked** → **Check every copied file**. (The recovered SSD isn't needed: step 4's results were saved.)
- [ ] **2.** When it finishes, the page shows how many files are damaged, clean and not testable, and lists the damaged ones. The results are in `C:\RECOVERY-TOOLS\results\`, named `scan-DATE…` with the date and time of this run: the results (`scan-DATE.csv`), `scan-DATE-not-testable.csv`, and — if some damaged files have no copy in `WORK` — `scan-DATE-map-check-not-here.csv` (usually because they're inside a library you moved to `PHOTOSLIB`). **Rename `scan-DATE.csv` to `probe-work.csv`** (and its companions the same way) so you know later which run was which.
- [ ] **3. Run it again on `E:\PHOTOSLIB`** (same settings) and rename the results to `probe-libraries.csv`. The `-photo-libraries.csv` beside it is the full per-library report Phase 5 works from.

**What the check does:** it reads every file's structure without opening it as a
picture or video, which finds a kind of damage that looks fine on screen — and, with
step 4's results, it marks every file the old drive lost data from, whatever its
type. It only ever reads; it changes nothing. It covers JPEG, PNG, GIF, WebP, TIFF, HEIC, camera RAW,
PDF, ZIP (including Word/Excel/PowerPoint files), MP3, FLAC, OGG, WAV, MP4/MOV,
MKV/WebM, AVI and the Photos Library database.

**What the verdicts mean:**

| Verdict | Meaning |
|---|---|
| **NEVER_READ** | ddrescue never read part of this file (step 4 knows exactly which bytes). Those bytes are zeros or leftovers on the copy. **It may open and look perfect — it's still damaged.** This is the only verdict that catches damage in HEIC and RAW files on Windows, and damage smaller than 4 KB. |
| **TRUNCATED** | The file ends before its own format says it should — or its last disk sectors were never read and are just zeros. For videos this includes "the index is missing", which makes a video unplayable. |
| **HOLE** | A block of zeros in the middle of the file, shaped exactly like whole disk sectors. ddrescue writes zeros where it couldn't read, so this is the fingerprint of a bad sector — and these files usually *open fine* and look normal. This is the probe's most valuable catch. |
| **MISMATCH** | The file's contents don't match its name (a `.jpg` that isn't a JPEG), or the start of the file was never read. |
| **DB_DAMAGED** | A Photos Library database failed its integrity check. The photos are separate files and may be fine; the albums, dates and faces are at risk. |
| **UNREADABLE** | The probe couldn't read it (Windows refused, or it was locked). Not proof of damage. |
| **LIBRARY** | A Photos Library (information, not a fault). |
| *not testable* | The probe has no test for that type. **Every HEIC and camera RAW lands here on purpose** — nothing on Windows can decode them, so the probe refuses to call them fine. They're checked on the Mac in Phase 5. Mac `._` files, `Thumbs.db` and similar housekeeping files also land here, labelled as such. Expect this to be a big pile; it's listed in the `-not-testable.csv`. |

> If you see **`UNREADABLE`** rows mentioning very long paths, tell Claude — the
> program handles long paths itself, so that shouldn't happen.

---

# PHASE 4 — Test by opening 🪟 Krokiet

> **After every scan, click Save** and save the results into `C:\RECOVERY-TOOLS\`
> with an obvious name. Krokiet forgets results when the next scan starts.
>
> Check the Phase 2 settings are still in place (Min file size **0**, the three
> Excluded items).

## Pass 1 — everything except video (a few hours)

Krokiet → **Broken Files** → *Type of files to check*:

| Check | Set to |
|---|---|
| Audio | **on** (on by default) |
| Image | **on** (on by default) |
| Pdf | **on** (on by default) |
| Archive | **on** (on by default — covers .zip, .7z, .gz, .tar and similar, *not* Word/Excel/PowerPoint) |
| Font | on |
| Markup (JSON/XML/TOML) | on |
| Video (ffprobe) | off |
| Video (ffmpeg) | off *for now* |

- [ ] Scan, then **Save the results**.

Krokiet's checks sometimes flag files that are fine. Treat this as a list of suspects,
not a verdict — nothing gets deleted because of it. This pass can't check HEIC or RAW;
that's Phase 5.

## Pass 2 — archives and Office files, properly (about 1 hr)

Krokiet doesn't open Word, Excel, PowerPoint or ePub files at all, so this pass is the
only real test for them.

- [ ] In File Explorer, open `SPARE\WORK` and type this in the search box (top right):
  `ext:.zip OR ext:.docx OR ext:.xlsx OR ext:.pptx OR ext:.epub OR ext:.7z OR ext:.rar`
  Wait for the search to finish (it's slow on a big drive), press **Ctrl+A**, right-click → **7-Zip → Test archive**. *(On Windows 11 the 7-Zip entry may be under **Show more options** first.)*
- [ ] When it finishes, take a screenshot or note of any files it reports errors in.

## Pass 3 — MP3s (about 20 min)

- [ ] **MP3val** (the frontend) on your music folders. **Only scan — don't click its
  fix/repair button.** Repairs belong in Phase 7, on copies.

A cut-short MP3 still claims its full length, so ordinary players and Krokiet can't
tell. MP3val counts the actual audio. If Krokiet flagged lots of MP3s that play fine,
trust MP3val instead.

## Pass 4 — video, full decode (hours to days)

- [ ] Krokiet → **Broken Files** again, this time **only "Video (ffmpeg)"** ticked, everything else off.
- [ ] **Save the results.**

This is the slowest step of the whole project — start it before bed. Krokiet itself
warns this check is strict and may flag files that play fine; treat it as a suspect
list.

---

# PHASE 5 — The Mac 🍎 (one session)

Take **only the working drive** (`SPARE`) to the Mac. The master stays in its drawer.
The Mac can only read the drive, so nothing here can change it.

- [ ] **Before unplugging the drive from the PC**, copy `C:\RECOVERY-TOOLS\results\probe-libraries-photo-libraries.csv` (from Phase 3 step 3) onto it (anywhere — the top of the drive is fine). Job 2 needs it and the Mac can read it from there.

## Job 1 — decode every HEIC and RAW

- [ ] **1.** Plug the drive into the Mac. Open **Terminal** (Applications → Utilities → Terminal). Type this to see the drive's name:

```
ls /Volumes
```

   If macOS asks whether **Terminal may access files on a removable volume**, click
   **Allow** — otherwise every check below fails.

- [ ] **2. First, prove the check works.** In Finder, copy any one `.heic` from the drive to your Desktop and rename it `good.heic`. Then paste these three lines into Terminal (they make a deliberately broken copy and test both):

```
cd ~/Desktop
head -c 20000 good.heic > broken.heic
for f in good.heic broken.heic; do : > t.jpg; err=$(sips -s format jpeg "$f" --out t.jpg 2>&1 >/dev/null); if [ $? -ne 0 ] || [ ! -s t.jpg ] || printf "%s" "$err" | grep -qi error; then echo "$f: BROKEN"; else echo "$f: ok"; fi; done
```

   You want **`good.heic: ok`** and **`broken.heic: BROKEN`**. If the broken one
   says `ok`, **stop and tell Claude** — the next step relies on it, and it's the
   one thing here that couldn't be tested on a real Mac in advance. (The check looks
   at three things — sips's exit code, whether it wrote a picture, and whether it
   printed an error — so it doesn't depend on any one of them.)

- [ ] **3. Run the full check.** Replace `DRIVENAME` with the name from step 1 (keep the quotes). This decodes every HEIC and RAW on the drive and writes the name of each one that fails to a list on your Desktop. It keeps the Mac awake by itself; leave the lid open. It takes **hours** and prints each file as it goes, so you can see it's working:

```
mkdir -p ~/Desktop/sips-check
: > ~/Desktop/sips-failures.txt
cd "/Volumes/DRIVENAME"
caffeinate -i find -E . \( -name '$RECYCLE.BIN' -o -name 'System Volume Information' -o -name '._*' \) -prune -o -type f -iregex '.*\.(heic|heif|avif|hif|cr2|cr3|crw|nef|nrw|arw|srf|sr2|dng|raf|orf|rw2|raw|pef|ptx|srw|3fr|erf|mef|mos|mrw|x3f|iiq|k25|kdc|dcr|rwl|fff)$' -print -exec sh -c 'T="$HOME/Desktop/sips-check/test.jpg"; : > "$T"; err=$(sips -s format jpeg "$1" --out "$T" 2>&1 >/dev/null); if [ $? -ne 0 ] || [ ! -s "$T" ] || printf "%s" "$err" | grep -qi error; then printf "%s\n" "$1" >> "$HOME/Desktop/sips-failures.txt"; fi' _ {} \;
```

- [ ] **4.** When it finishes, `sips-failures.txt` on your Desktop is your broken HEIC/RAW list (one file per line). If it's empty, nothing failed.

## Job 2 — the Photos Libraries

> **A library can't be opened while it's on the working drive** — Photos needs a
> Mac-formatted disk. Storing it there is fine; opening it there isn't.

Open `probe-libraries-photo-libraries.csv` (the one you copied onto the drive; from
Phase 3 step 3) in Numbers or any spreadsheet, and work down it:

- [ ] **1. Skip any with `database_verdict` = `DB_DAMAGED` for now.** A healthy one, if you have one, answers the question faster.
- [ ] **2. Start with row 1** (the probe's best guess). Exception: if a smaller library has a much newer `newest_file` date, it may be the one you were actually using and the big one an old archive — try that instead.
- [ ] **3. Copy just that one library to the Mac.** It needs about 20% more free space than its size.
- [ ] **4. Hold ⌥ Option + ⌘ Command while opening Photos**, and choose **Repair**. On a big library this takes hours.
- [ ] **5. Check what survived, in this order** — each can fail on its own:
  - Do the photos appear at all?
  - Are the **dates** right?
  - Are the **albums** there?
  - Are **faces and keywords** there?
- [ ] **6. If photos are missing or blank, check that library's `damaged_inside` number** before re-running Repair. If 4,000 files inside it were damaged, no amount of repairing brings them back — don't spend six hours hoping.
- [ ] **7. Now compare against the other libraries.** With one open, spot-check whether photos from the smaller ones are already in it. Only then do you know whether the others are redundant.
- [ ] **8.** If Photos won't open it at all, **write down the exact error message.**

> **Don't merge libraries by hand and don't dedupe across them.** If two overlap,
> Photos' own Import handles it. Deleting files inside a library breaks it.

> **A library made on a newer macOS won't open on an older one.** If the Mac is behind,
> update it before deciding a library is dead.

### If no library fits on the Mac

You can still know whether it's intact: `database_verdict` in the CSV tells you if
the database survived, and Job 1 tested the photos inside it. To actually *use* the
library you'll need a drive with room, formatted for Mac.

## Job 3 — bring the results back

- [ ] Copy `sips-failures.txt` from the Mac's Desktop back to Windows (email it to yourself or use a USB stick — the Mac can't write to the drive), plus any notes from Job 2.
- [ ] Unplug the drive and take it back to the PC.

---

# PHASE 6 — Sort into three folders 🪟 (1–2 hrs)

On `SPARE`, make `GOOD`, `BROKEN` and `UNSURE`.

> ⚠️ **Never move anything out of `PHOTOSLIB`** — not even files the probe or the Mac
> called damaged. A library is kept whole; taking files out of it breaks it. Everything
> below is about `WORK` only.

> 💬 **Before you start this phase, ask Claude for the sorting helper.** The
> program's CSV files and `sips-failures.txt` list thousands of files; moving those
> by hand isn't realistic. The helper reads those lists and moves each file from `WORK` into
> the right folder, keeping the folder structure, never touching `PHOTOSLIB`, and
> showing you a preview before it moves anything. It isn't built yet because the
> exact shape of your result files decides how it should work.

**For Krokiet's own results**, the easiest way to move them is to re-run the scan that
found them, **Select → Select all**, then **Move** — and in the Move window tick
**Preserve folder structure**, or every file lands in one heap and you lose which
folder it came from.

The rules the sorting follows:

- [ ] **1. To `BROKEN`:** everything Krokiet's Broken Files flagged, everything 7-Zip failed, everything MP3val flagged, everything in `WORK` the check called NEVER_READ, TRUNCATED, HOLE, MISMATCH or DB_DAMAGED, everything in `WORK` listed in `sips-failures.txt`, and the files `rescue-log.csv` calls "partly copied" (Phase 1 step 12).
- [ ] **2. To `UNSURE`:** everything on the Bad Extensions list from Phase 2, everything the probe called UNREADABLE, and everything in the probe's `-not-testable.csv` that no other test passed (HEIC and RAW that the Mac decoded without complaint count as passed). The housekeeping files (Mac `._` files, `Thumbs.db`) can simply stay in `WORK`.
- [ ] **3. Everything left is `GOOD`** — it passed at least one real test.
- [ ] **4. Look through `UNSURE` by file type.** Sort by extension; usually 5–10 types are nearly all of it. Open a handful of each type in its normal program. If they open, move that type to `GOOD` — and **write a note that you spot-checked those rather than proved them.**

---

# PHASE 7 — Repair 🪟 (reconnect to the internet for this)

Set expectations: **a repair tool produces a file that opens, not the file you had.**
The damaged part is papered over with plausible-looking filler.

- [ ] **1. First, open a sample of `BROKEN` by hand.** Krokiet sometimes flags files that are fine. If they open normally, you may need to buy nothing.
- [ ] **2.** Try the free previews: Stellar and Repairit both show the repaired result before you pay. **untrunc** (already in `C:\RECOVERY-TOOLS\`) is free and fixes MP4/MOV videos that stop partway — but it needs a **good video from the same camera or phone** as a pattern to work from. Keep one handy for each device.
- [ ] **3. Repaired files go in a new folder and back through Phases 3 and 4.** A repair that hasn't been re-tested is a guess. Passes → `GOOD`.
- [ ] **4. Whatever is still broken stays in `BROKEN`. Keep it. Don't delete it.** It's the only copy that exists, and repair tools get better every year.

---

# PHASE 8 — Rescue photos from their near-twins 🪟 Krokiet

- [ ] **1.** Run **Similar Images** with Included paths set to `GOOD`, `BROKEN` and `UNSURE`.
- [ ] **2.** ⚠️ **Check the `PHOTOSLIB` exclusions are still in Excluded item.** This is the scan that would do the most damage to a library.
- [ ] **3.** Set **Geometric invariance** to **Mirror + Flip + Rotate 90** (finds rotated and mirrored copies).
- [ ] **4.** Use **Compare** to see each pair side by side.
- [ ] **5.** Where a broken photo has an intact twin in `GOOD`, that picture is recovered. Leave the broken one where it is.

This rescues more photos than the repair tools do.

---

# PHASE 9 — Tidy up and protect it 🪟 (about 1 hr)

- [ ] **1.** Move `GOOD` to the top of the drive. Keep `BROKEN`, `UNSURE` and `PHOTOSLIB`, clearly named. Delete what's left of `WORK` once you're certain.
- [ ] **2.** Put all your saved result files (the whole `C:\RECOVERY-TOOLS\results` folder, and `rescue-log.csv`) and the mapfile in a `RECORDS` folder on the drive. In three years you'll want to know what was checked.
- [ ] **3. MultiPar:** create PAR2 files at **10%**, one set per top-level folder. This both detects and *repairs* files that quietly change over time.
  - Only do this for a Photos Library you're keeping as a frozen archive. If you'll keep using it on the Mac, Photos changes it constantly and the PAR2 files go stale — use Time Machine for a live library instead.
- [ ] **4. Set a phone reminder every three months** to run MultiPar's **Verify**.
- [ ] **5. Undo Phase −1:**
  - `services.msc` → Windows Update → **Automatic (Delayed Start)** → Start
  - Settings → Windows Update → **Resume updates**
  - Advanced options → turn the restart setting back on
  - Power Options → back to **Balanced**
  - Remove the Defender exclusion for `SPARE`
  - `sysdm.cpl` → Advanced → Startup and Recovery → Settings → tick **Automatically restart** again
  - AutoPlay back on, if you want it
  - In Recovery Probe, step 7 should say *"Windows' setting for new disks is as it was."* If it doesn't, click **Put Windows' setting back**
  - Reconnect and let Windows catch up

---

## What this catches, and what it can't

| Kind of damage | Caught? |
|---|---|
| **Any part of a file the old drive never gave up** — even one 512-byte sector, even in a photo that looks perfect, even HEIC and RAW | **Yes** — the map check (Phase 1), marked NEVER_READ (Phase 3) |
| Files Windows can't even list, because their folder record was never read | **Yes** — found by the map check and copied anyway (Phase 1) |
| File cut short (video stops halfway, JPEG without its end, PDF/zip missing their ending) | **Yes** — Phase 3 |
| The last part of a file never read (zeros at the end) | **Yes** for photos, PDFs, zips, MP3/FLAC/OGG and MP4/MOV/HEIC — Phase 3. RAW and MKV: Phases 4 and 5 |
| A video whose index was lost (won't play at all) | **Yes** — Phase 3 |
| MP3 cut short but claiming full length | **Yes** — MP3val, Phase 4 |
| Won't decode (video glitches partway, HEIC that won't open) | **Yes** — Phases 4 and 5 |
| Photos Library database damaged | **Yes** — Phase 3 |
| Bad sectors filled with zeros in the middle (opens fine, looks normal) | **Yes** — the map check, any size; the probe's hole check too, for patches of 4 KB or more |
| Bad sector filled with leftover data (grey smear across a photo) | **Yes** — the map check knows where ddrescue failed, whatever the SSD holds there |
| Looks perfect but the content is subtly wrong, where the drive *did* read | **No** — the old drive would have had to hand over wrong data without an error, which is rare |

The last row is the loss nothing can find. The mapfile total from Phase 0 —
about 600 KB — is how much was lost to places the drive could not read. Phases 3, 4 and 5 catch *different* things, which is
why all three exist — none of them replaces another.

Realistic outcome: every file either proven to open, or sitting in a folder you can
see. Nothing silently assumed good. That's a much higher bar than "I opened a few
and they looked OK."

## Afterwards

Two SSDs in a drawer is not a backup: same kind of device, same place, and unpowered
SSDs slowly lose their data. **Plug both in twice a year.** And put the things you
truly can't lose — usually the family photos — somewhere else as well, such as cloud
storage.
