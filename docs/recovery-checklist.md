# Recovery Checklist — Recovered SSD → safe, checked copies

*Version 2026-10-19 — written with Claude Opus 5.5 for the Recovery Probe program
(`recovery-probe.exe`), after an independent review of the program and of the
2026-10-18 plan. Replaces every earlier copy; where a step is the same as in the
2026-10-18 copy, the box you ticked there stays ticked here.*

**What you have:** a 2 TB SSD holding the ddrescue copy of your dead 1 TB hard drive
(a GUID partition map with one exFAT volume, as the Mac made it), an empty 2 TB SSD
(`SPARE`), a Windows PC and a Mac. The old hard drive is gone, so every file on that
first SSD is the only copy in existence.

**What you need as well:** **one more drive, for the image** — at least 1 TB free
(the image is 1,000 GB; Windows shows it as 932 GB). An ordinary external hard disk is
fine. A 4 TB one can later hold a second copy of your files as well (Phase 7).

**What you're doing, in short:**

1. **Make an image of the recovered SSD** onto that drive: a second complete copy,
   checked by reading it back. The SSD then goes back in its drawer for good.
2. **From the image:** find what the damage touched, and copy every file into
   `SPARE\WORK`, with a checksum for each file.
3. **Check every copy once**, for a report. Nothing in `WORK` is ever moved,
   renamed or deleted.
4. **Work only on copies of the few damaged files**, in a `REPAIR` folder.
5. **Keep two copies of your files**, on two drives, one of them away from home, and
   check them against the checksums once or twice a year.

**Why this way:** the damage is tiny and exactly known. ddrescue failed to read
610,816 bytes in 167 places, about 0.00006% of the drive. That can damage the
contents of at most a few hundred files, and the program names them. Everything else
was read without error. So the risk is no longer the damage: it is the only copy, and
anything that deletes or moves files by the thousand.

## The rules

1. **The recovered SSD is the only copy until the image is checked.** It is never
   formatted, never "repaired", never written to, and never plugged in while the PC
   restarts. **Every time** it is plugged in, Recovery Probe's step 3 comes first: it
   makes Windows leave the SSD switched off and read-only, so nothing — not even
   Windows itself — can change it.
2. **Never point a duplicate finder at an Apple Photos Library, and never take files
   out of a library** or out of any other Mac package (iMovie and Final Cut
   libraries, Logic and GarageBand projects, Keynote/Pages/Numbers files, `.rtfd`,
   iPhoto and Aperture libraries). A library is a folder that holds every photo
   *and* a small copy of every photo; a duplicate finder sees those as duplicates.
3. **Nothing in `WORK` is ever moved, renamed or deleted** — not by you, not by
   Krokiet, not by a repair tool. What the checks find goes into reports. Damaged
   files are *copied* into `REPAIR`.

> **The files you need**, together in `C:\RECOVERY-TOOLS\`:
>
> | File | What it is |
> |---|---|
> | `recovery-checklist.md` | This document. Tick the boxes as you go — Obsidian saves them. |
> | `download-manifest.md` | Where each tool came from, how to install it, what it's for. |
> | `recovery-probe.exe` | **The program. Double-click it.** It checks itself, makes the image, works out from the mapfile exactly which files the damage touched, copies everything off without letting Windows write to the SSD (including files Windows can't even see), and checks every copied file. |
> | `1TB hard drive DDRescue-GUI second attempt.log` | The mapfile from the final ddrescue run: the list of every place on the old drive that could never be read. **Keep it forever.** |
>
> The program's page says **Version 2026-10-19** (or later) at the top. The older
> files `recovery_probe.py`, `probe_selftest.py`, `RUN_PROBE.bat` and
> `RUN_SELFTEST.bat` aren't needed any more. You can delete them, or leave them;
> they do no harm.

> **How the program works:** double-click `recovery-probe.exe`. Windows asks
> *"Do you want to allow this app to make changes to your device?"* — click **Yes**
> (it needs that to read the recovered SSD directly; it never writes to it). A black
> window opens — **leave it open**, it *is* the program — and your web browser shows
> the program's page: eight numbered steps, each with one button. Nothing on the page
> goes to the internet; it works offline. What each step found is kept in
> `C:\RECOVERY-TOOLS\results\`, so closing and reopening loses nothing.
>
> | Step | What it does |
> |---|---|
> | 1 | Checks the program works on this PC |
> | 2 | Your ddrescue mapfile |
> | 3 | Connects the recovered SSD, protected |
> | 4 | Makes an image of the SSD: a second copy |
> | 5 | Finds what the damage touched (the "map check") |
> | 6 | Copies everything off, with a checksum per file |
> | 7 | Checks every copied file; checks the copies are unchanged |
> | 8 | Finish: take the SSD out, put Windows' setting back |
>
> - Browser didn't open? The black window shows an address — copy it into the browser.
> - The page says **"The program is not running any more"**? Its black window was
>   closed. Double-click the program again.
> - *"Windows protected your PC"*? Click **More info → Run anyway** — only for this
>   file from the chat.

**Steps are numbered within each phase**, so "Phase 2, step 3" always means one thing.
🪟 = on the Windows PC. 🍎 = on the Mac. 💬 = in the Claude chat.

---

## How the machines are used

Windows does almost everything. The Mac is optional (Phase 5): it is the only thing
that can fully decode HEIC photos and camera RAW, and open a Photos Library.

The **original recovery SSD never goes near the Mac.** Only `SPARE` does, and that's
safe because macOS can only *read* a Windows-formatted (NTFS) drive — unless an NTFS
write add-on is installed on the Mac (Phase 5 checks).

## The tools

**Recovery Probe** (`recovery-probe.exe`) does the essential work: the image, the map
check, the copying and the checks. Everything else is optional: **Krokiet**, **7-Zip**
and **MP3val** for extra checks that only read (Phase 4), **ffmpeg** for Krokiet's
video check, **`sips`** on the Mac (built in, only reads), and **untrunc** and the
paid repair tools, used only on the copies in `REPAIR` (Phase 6).

> ⚠️ **Krokiet has Trash, Delete and Move buttons. In this plan they are never used.**
> Its scans only make lists. (Windows' Recycle Bin has a size limit — about 5% of the
> drive — and when it's full it quietly deletes the oldest items for good, so "Trash
> can be undone" isn't true for big batches.)

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
> `C:\RECOVERY-TOOLS`, the image goes on `IMAGE`, the copies on `SPARE`. Switching now
> would mean installing Obsidian again, redoing the ffmpeg **Path** (section D, step 4
> — it belongs to one account) and linking the Claude app again. The power, Windows
> Update and Defender settings count for every account either way.

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

A PC that sleeps mid-copy can corrupt a drive that's being written to.

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
- [ ] **TeraCopy** (`teracopy.exe`) — optional: Recovery Probe does the copying (TeraCopy would miss the files Windows can't list). Install it if you like it for other copying.
- [ ] **Obsidian** (`Obsidian-1.13.7.exe`) — run it. Open this checklist in it so you can tick boxes.
- [ ] **MultiPar** (`MultiPar1336_setup.exe`) — optional now (Phase 7 explains why). Install it or not, as you like.
- [ ] **Krokiet** (the folder `windows_krokiet_on_windows_all_backends`) — nothing to install. Move the whole folder into `C:\RECOVERY-TOOLS\`. You run the `.exe` inside it. *(If a window doesn't appear, try one of the `.bat` files in the same folder — each starts it with a different way of drawing the window.)*
- [ ] **MP3val** (the folder `mp3val-0.1.8_with_frontend-0.1.1-bin-win32`) — nothing to install. Move it into `C:\RECOVERY-TOOLS\`. Run the *frontend* `.exe` inside it.
- [ ] **untrunc** (the folder `untrunc_x64`) — nothing to install. Move it into `C:\RECOVERY-TOOLS\`. Not needed until Phase 6.
- [ ] **ffmpeg** (`ffmpeg-9.0.1-essentials_build.zip`) — Krokiet's video check needs it. **This one strands people, do it carefully:**
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
Phase 6 is when you reconnect and decide whether you need them at all.

## E. Test everything before you disconnect

*(Do this section again whenever `recovery-probe.exe` is replaced by a newer version.)*

Make a test folder (for example `C:\RECOVERY-TOOLS\test-files`) with a few photos, a
video, a zip, a PDF and an MP3 (copies of ordinary files of yours — not anything from
the recovered SSD).

- [ ] **Double-click `recovery-probe.exe`** in `C:\RECOVERY-TOOLS\` → **Yes**. The page opens in your browser and the badge at the top says **Running with administrator rights**.
- [ ] **Step 1 → Run the self-test.** A few seconds. It must say **"Every check passed: the program works on this PC."** It checks itself on sample photos, music, videos, documents and a Photos database, intact and damaged, and on a small damaged test disk it carries: it copies every file off it and compares them byte for byte, including a folder whose own record the damage destroyed. Anything else → take a screenshot (`Win+Shift+S`) and paste it into the Claude chat.
- [ ] **Step 2** should already show the mapfile, with a red-and-green map of the old drive at the top of the page.
- [ ] **Step 7** on the test folder: click **Choose…** beside the folder box, open the test folder, click **Choose this folder**, then **Check every copied file**. (*Use what step 5 found* stays greyed out until step 5 has run — that's right.) Your good test files should come back **clean** (photos, PDFs, zips, videos and MP3s) or **not testable** (iPhone HEIC photos and camera RAW — always, on purpose).
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

Because many places are smaller than 4 KB, the hole check (Phase 3) can't see all of
them on its own. The map check (Phase 2) can: it works out from the mapfile exactly
which files and folders sit on those places, whatever size they are.

---

# PHASE 1 — Make the image 🪟 (about 2–5 hrs, mostly waiting)

## Prepare the two empty drives first — each on its own

> ⚠️ **Format nothing while the recovered SSD is plugged in.** Both SSDs are 2 TB,
> and formatting the wrong one would erase the only copy. So the empty drives are
> prepared **alone**.

- [ ] **1.** Make sure the recovered SSD is **not** plugged in. Plug in **only the empty SSD**.
- [ ] **2.** Format it as **NTFS**, with the name **`SPARE`**: File Explorer → right-click the drive → **Format** → File system **NTFS**, Volume label **`SPARE`**, Quick Format ticked → Start. *(Not in File Explorer? It may be brand new. Right-click Start → **Disk Management** → if it asks to initialise a disk, choose **GPT** → right-click the empty space on that disk → **New Simple Volume** → NTFS, label `SPARE`.)* This is where the copies go.
- [ ] **3.** Turn off indexing on it: right-click `SPARE` → Properties → untick "Allow files on this drive to have contents indexed" → apply to all files and folders.
- [ ] **4.** Note `SPARE`'s drive letter (e.g. `E:`).
- [ ] **5. Defender exclusion for `SPARE`:** Windows Security → Virus & threat protection → Manage settings → Exclusions → **Add an exclusion → Folder** → `SPARE` (e.g. `E:\`). Otherwise Defender scans every file the program writes and checks (2–3× slower), and if it dislikes a recovered file it *quarantines* it. (The recovered SSD needs no exclusion: it stays switched off, so Defender never sees it.)
- [ ] **6.** Unplug `SPARE`. Plug in **only the image drive**. Format it the same way: **NTFS**, label **`IMAGE`**, Quick Format. *(Already holds files you want to keep? Then don't format it: it only needs to be NTFS with at least 1,000 GB free.)*
- [ ] **7.** Turn off indexing on `IMAGE` (as step 3) and add the same Defender exclusion for it (as step 5). Note its drive letter (e.g. `F:`).

## A dress rehearsal with `SPARE` — before the recovered SSD

Windows' setting that keeps a new disk switched off does **not** work for a disk
Windows calls "removable media", and some USB cases and adapters say that about the
SSD inside them. So first try the way the recovered SSD will be plugged in, with
`SPARE`, which is empty and has nothing to lose.

- [ ] **8.** Unplug everything except `IMAGE`. Double-click `recovery-probe.exe` → **Yes**.
- [ ] **9. Step 3 → Get Windows ready.** It answers that new disks now stay switched off. (`IMAGE`, already plugged in, stays as it is.)
- [ ] **10.** Plug `SPARE` in **the way the recovered SSD will be plugged in**: the same USB port and cable, and if the recovered SSD sits in a USB case or adapter, the same kind of case or adapter.
- [ ] **11.** In the step 3 list, click `SPARE`'s disk. You want: **switched off**, no **removable media** tag, the page saying *"It arrived switched off, so Windows' setting works for a disk plugged in this way"*, and no new drive in File Explorer.
  - *It says **removable media**, or `SPARE` got a drive letter?* **Stop.** That way of plugging in isn't safe for the recovered SSD. Tell Claude: a different case or adapter, a SATA port inside the PC, or a hardware write blocker are the ways round it.
- [ ] **12.** Click **Switch disk N on, writable** for `SPARE`. It gets its drive letter back. (Windows stays set to leave new disks switched off until the recovered SSD has been unplugged; step 8 of the program puts the setting back.)

## Bring in the recovered SSD — protected

> ⚠️ **Never restart the PC with the recovered SSD plugged in.**
>
> ⚠️ If Windows ever says **"There's a problem with this drive. Scan and fix?"**,
> click **Cancel**. Never let Windows repair the recovery drive — "fixing" can delete
> damaged files to tidy up. (With step 3 done first, Windows never gets the chance.)

- [ ] **13. Plug in the recovered SSD**, the way you rehearsed. Within a few seconds it appears in the step 3 list marked **the copy ddrescue made** and **switched off**, already chosen. Nothing appears in File Explorer — that's the point.
- [ ] **14.** Click **Protect disk N**. The page answers that the disk is read-only. Step 3 gets a green tick.
  - *Not marked "the copy ddrescue made"?* Stop and tell Claude before going further.
  - *Marked **removable media**?* The page says so in red: make the image (next step) straight away, then unplug it, and tell Claude before ever plugging it in again.
  - *The page shows an error instead?* Unplug the recovered SSD, and tell Claude.
- [ ] **15. Step 4 → Make the image.** In the box, type `F:\IMAGE` (with `IMAGE`'s drive letter: the program makes the folder), then click **Make the image**. The program reads the whole SSD once, front to back, into one file (`recovered-ssd.img`), then reads that file back to check it. A 1 TB image over USB takes roughly 1–3 hours to make and as long again to check. The bar at the bottom shows how far it is.
  - You can click **Stop** and start it again later: it carries on where it stopped.
  - **If the page says the SSD stopped answering:** nothing is lost. Check its cable is in firmly. If it came out, plug it back in (it arrives switched off), choose it in step 3, click **Protect disk N**, then **Make the image** again: it carries on.
- [ ] **16.** When it says **Done**, read the result. You want **"…and it reads back exactly as it was written"** and **"The SSD gave every byte."**
  - *It lists places the SSD could not read?* They are zeros in the image, and the map check counts them as never read, so nothing is hidden. But tell Claude: the SSD itself is then failing, which matters for keeping it.
  - *It says the image does NOT read back the same?* The `IMAGE` drive has a problem. Don't use that image; tell Claude.
- [ ] **17. Step 8:** click **Switch it off** (if shown), then unplug the recovered SSD and put it in a drawer. It's your untouched master, and it is not needed again.
- [ ] **18.** Still in step 8, click **Put Windows' setting back**. It answers *"Windows' setting for new disks is as it was."* (It refuses while the recovered SSD is still plugged in.)

Next to the image, in `IMAGE\`, there are now `recovered-ssd.img.sha256` (its
checksum) and `recovered-ssd.img.txt` (what it is, where it came from). Keep all three
together.

---

# PHASE 2 — Find what the damage touched, and copy everything 🪟 (about 3–6 hrs)

Everything in this phase reads the image. `SPARE` and `IMAGE` are plugged in; the
recovered SSD stays in its drawer. (Step 5 and 6 say **"Reads from the image"** under
their text; if they don't, step 4's image isn't there: check `IMAGE` is plugged in.)

- [ ] **1. Step 5 → Find what the damage touched.** A few minutes. It shows how many **files lost data**, how many **files Windows cannot list** (their folder record sat on a place that was never read, so File Explorer and TeraCopy would never see them), how many **folders** are damaged, how much **data no folder names any more**, and how many files are on the SSD. The red marks on the map at the top now show what each place held. Write the numbers down. Everything is also saved in `C:\RECOVERY-TOOLS\results\map-check-…\`.
- [ ] **2. Read the date line under the numbers.** It should say the newest date on the SSD is *before* ddrescue finished (24 April 2024). **If it shows a yellow warning instead, stop and tell Claude**: something may have changed the SSD after the copy (a computer it was plugged into, say), or a file simply has a wrong date.
- [ ] **3.** If the page says some **folders were found by their contents**: those are folders whose own record (with the folder's name) sat on a never-read place. The files inside keep their names, dates and folders; only the folder's own name is lost. Step 6 puts each one in `found folders (name lost)\folder at cluster N`.
- [ ] **4.** If **data no folder names any more** is more than a few MB, tell Claude before going further. It is usually files whose folder record was lost; a second recovery program can be tried on the image to put names to some of it (safely: it's the image).
- [ ] **5. Step 6 → Copy everything.** Click **Choose…**, click `SPARE`, then **Choose this folder**: the program puts the copies in a `WORK` folder on it (e.g. `E:\WORK`). Keep **"Also save the data no folder names any more"** ticked. Click **Start copying**.
  - It copies **every** file, including the ones Windows can't list, into `SPARE\WORK`, with the same folders and dates, and writes each file's checksum (SHA-256) into the list `WORK\rescue-log.csv`. It takes hours. The bar at the bottom shows how far it is and about how long is left.
  - You can click **Stop** and start it again later (same folder): what's already copied is checked against its checksum and kept.
  - A few names change slightly, because Windows can't use them (a name ending in a space gets `_` instead). `rescue-log.csv` lists every one, and every file.
  - *The PC lost power or crashed?* Start the program again and **Start copying** again into the same folder: each earlier copy is read back whole and copied again if the crash spoilt it.
- [ ] **6.** When it says **Done**, read the summary. Files that *"could only be copied in part"* are damaged (`rescue-log.csv` lists them as "partly copied"); they're on the damaged list in Phase 6 anyway. Files that *"could not be copied"*: take a screenshot and tell Claude.
- [ ] **7. Step 7 → Check the copies are unchanged** (the button under *"Are the copies still exactly what came off the SSD?"*). It reads every copy back and compares it with the checksum written while copying. You want **"All N copies are exactly what was copied off the disk."** Anything else: tell Claude.

> From now on `SPARE\WORK` is your working copy, and it is **never changed**. The
> image and the SSD in the drawer are the copies it came from.

---

# PHASE 3 — The full check 🪟 (1–3 hrs, runs by itself)

- [ ] **1. Step 7 → Check every copied file**: folder `E:\WORK` (it's filled in already), **"Look for holes" ticked**, **"Use what step 5 found" ticked**.
- [ ] **2.** When it finishes, the page shows how many files are damaged, clean and not testable, and lists the damaged ones. The results are in `C:\RECOVERY-TOOLS\results\`, named `scan-DATE…`: the results (`scan-DATE.csv`), `scan-DATE-not-testable.csv`, and `scan-DATE-photo-libraries.csv` (one row per Photos Library, with everything learnt about it). **Rename `scan-DATE.csv` to `check-work.csv`** (and its companions the same way) so you know later which run was which.
- [ ] **3.** The page also lists every **Photos Library** it found. That's information only: the libraries stay exactly where they are in `WORK`.

**What the check does:** it reads every file's structure without opening it as a
picture or video, which finds a kind of damage that looks fine on screen — and, with
step 5's results, it marks every file the old drive lost data from, whatever its
type. It only ever reads; it changes nothing. It covers JPEG, PNG, GIF, WebP, TIFF,
HEIC, camera RAW, PDF, ZIP (including Word/Excel/PowerPoint files), MP3, FLAC, OGG,
WAV, MP4/MOV, MKV/WebM, AVI and the Photos Library database.

**What the verdicts mean:**

| Verdict | Meaning |
|---|---|
| **NEVER_READ** | ddrescue never read part of this file (step 5 knows exactly which bytes). Those bytes are zeros or leftovers on the copy. **It may open and look perfect — it's still damaged.** This is the only verdict that catches damage in HEIC and RAW files on Windows, and damage smaller than 4 KB. |
| **TRUNCATED** | The file ends before its own format says it should — or its last disk sectors were never read and are just zeros. For videos this includes "the index is missing", which makes a video unplayable. |
| **HOLE** | A block of zeros in the middle of the file, shaped exactly like whole disk sectors. ddrescue writes zeros where it couldn't read, so this is the fingerprint of a bad sector — and these files usually *open fine* and look normal. |
| **MISMATCH** | The file's contents don't match its name (a `.jpg` that isn't a JPEG), or the start of the file was never read. |
| **DB_DAMAGED** | A Photos Library database failed its integrity check. The photos are separate files and may be fine; the albums, dates and faces are at risk. |
| **UNREADABLE** | The check couldn't read it (Windows refused, or it was locked). Not proof of damage. |
| **LIBRARY** | A Photos Library (information, not a fault). |
| *not testable* | No test for that type. **Every HEIC and camera RAW lands here on purpose** — nothing on Windows can decode them, so the program refuses to call them fine. The Mac can check them (Phase 5). Mac `._` files, `Thumbs.db` and similar housekeeping files also land here, labelled as such. Expect this to be a big list; it's in the `-not-testable.csv`. |

> **Keep the Mac `._` files and `Thumbs.db`** — nothing in `WORK` is deleted anyway.
> A `._` file holds the Mac's extra information about the file beside it (tags,
> comments, where it came from). For some very old Mac files it holds the *whole*
> file, and the file beside it is 0 bytes: the check may call that 0-byte file
> TRUNCATED. Keep both. A `Thumbs.db` can hold the only intact small picture of a
> photo that is now damaged.

> If you see **`UNREADABLE`** rows mentioning very long paths, tell Claude — the
> program handles long paths itself, so that shouldn't happen.

---

# PHASE 4 — Extra checks, read only 🪟 (optional)

These look for problems the drive failure didn't cause — files that were already
broken before it died. They take days of computer time, and they're optional. Every
one of them **only makes a list**: never click Trash, Delete, Move, Rename, Fix or
Repair in any of them. Save each list into `C:\RECOVERY-TOOLS\results\` with an
obvious name; the lists go into the damaged list in Phase 6.

**First, three settings in Krokiet** (they're in the same settings area as
*Included paths*):

- [ ] **Included paths:** `SPARE\WORK` only.
- [ ] **Excluded item:** add `*.photoslibrary*` and `*/._*` (separated by a comma). The first keeps Krokiet out of the Photos Libraries (thousands of small previews in there would only bury the results). The second skips Mac `._` files, which would otherwise show up as "broken" images or videos.
- [ ] **File Size (Kilobytes) → Min:** change **16** to **0**. Out of the box Krokiet **ignores every file smaller than 16 KB** in every scan except Empty Files, and nothing tells you.

> **After every scan, click Save** and save the results. Krokiet forgets results when
> the next scan starts. Never click **Trash**, **Delete** or **Move**.

## Pass 1 — Broken Files, everything except video (a few hours)

Krokiet → **Broken Files** → *Type of files to check*: Audio, Image, Pdf, Archive, Font
and Markup **on** (the first four are on by default); both Video checks **off**.

- [ ] Scan, then **Save the results**.

Krokiet's checks sometimes flag files that are fine. Treat this as a list of suspects,
not a verdict. It can't check HEIC or RAW on Windows.

## Pass 2 — archives and Office files, properly (about 1 hr)

Krokiet doesn't open Word, Excel, PowerPoint or ePub files at all, so this pass is the
only real test for them.

- [ ] In File Explorer, open `SPARE\WORK` and type this in the search box (top right):
  `ext:.zip OR ext:.docx OR ext:.xlsx OR ext:.pptx OR ext:.epub OR ext:.7z OR ext:.rar`
  Wait for the search to finish (it's slow on a big drive), press **Ctrl+A**, right-click → **7-Zip → Test archive**. *(On Windows 11 the 7-Zip entry may be under **Show more options** first.)* Testing only reads.
- [ ] When it finishes, take a screenshot or note of any files it reports errors in.

## Pass 3 — MP3s (about 20 min)

- [ ] **MP3val** (the frontend) on your music folders. **Only scan — never click its
  fix/repair button.**

A cut-short MP3 still claims its full length, so ordinary players and Krokiet can't
tell. MP3val counts the actual audio.

## Pass 4 — video, full decode (hours to days)

- [ ] Krokiet → **Broken Files** again, this time **only "Video (ffmpeg)"** ticked, everything else off. Start it before bed.
- [ ] **Save the results.** Krokiet itself warns this check is strict and may flag files that play fine; it's a suspect list.

## Pass 5 — Bad Extensions (a few minutes)

- [ ] Krokiet → **Bad Extensions** → Scan → **Save** the list. **Don't rename anything.** (A file whose name says one type but holds another.)

> Some copies may have paths longer than Windows' usual 260 characters (folders
> inside folders inside a library). File Explorer and older tools can fail on those
> or skip them. That's one more reason nothing here moves files: the copies stay
> exactly where the program put them.

---

# PHASE 5 — The Mac 🍎 (optional, one session)

Take **only `SPARE`** to the Mac. The SSD stays in its drawer; the image stays on
`IMAGE`.

- [ ] **Before unplugging `SPARE` from the PC**, copy `C:\RECOVERY-TOOLS\results\check-work-photo-libraries.csv` (from Phase 3) onto it (the top of the drive is fine). Job 2 needs it.
- [ ] **Check the Mac can only read `SPARE`.** Plug it in, select it in Finder → **File → Get Info** → *Sharing & Permissions* should say **"You can only read"**. If it doesn't (an NTFS add-on such as Paragon NTFS, Tuxera NTFS, iBoysoft NTFS or Mounty is installed), **unplug it and tell Claude** — don't create, rename or delete anything on it.

## Job 1 — decode every HEIC and RAW

- [ ] **1.** Open **Terminal** (Applications → Utilities → Terminal). Type this to see the drive's name:

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
   says `ok`, **stop and tell Claude** — the next step relies on it.

- [ ] **3. Run the full check.** Replace `DRIVENAME` with the name from step 1 (keep the quotes). This decodes every HEIC and RAW on the drive and writes the name of each one that fails to a list on your Desktop. It only reads the drive. It keeps the Mac awake by itself; leave the lid open. It takes **hours** and prints each file as it goes:

```
mkdir -p ~/Desktop/sips-check
: > ~/Desktop/sips-failures.txt
cd "/Volumes/DRIVENAME"
caffeinate -i find -E . \( -name '$RECYCLE.BIN' -o -name 'System Volume Information' -o -name '._*' \) -prune -o -type f -iregex '.*\.(heic|heif|avif|hif|cr2|cr3|crw|nef|nrw|arw|srf|sr2|dng|raf|orf|rw2|raw|pef|ptx|srw|3fr|erf|mef|mos|mrw|x3f|iiq|k25|kdc|dcr|rwl|fff)$' -print -exec sh -c 'T="$HOME/Desktop/sips-check/test.jpg"; : > "$T"; err=$(sips -s format jpeg "$1" --out "$T" 2>&1 >/dev/null); if [ $? -ne 0 ] || [ ! -s "$T" ] || printf "%s" "$err" | grep -qi error; then printf "%s\n" "$1" >> "$HOME/Desktop/sips-failures.txt"; fi' _ {} \;
```

- [ ] **4.** When it finishes, `sips-failures.txt` on your Desktop is your broken HEIC/RAW list (one file per line). If it's empty, nothing failed.

## Job 2 — the Photos Libraries

> **A library can't be opened while it's on `SPARE`** — Photos needs a Mac-formatted
> disk. So a library is *copied* to the Mac, whole, and opened there. The one in
> `WORK` is never changed.

Open `check-work-photo-libraries.csv` (the one you copied onto the drive) in Numbers
or any spreadsheet, and work down it:

- [ ] **1. Skip any with `database_verdict` = `DB_DAMAGED` for now.** A healthy one, if you have one, answers the question faster.
- [ ] **2. Start with row 1** (the best guess). Exception: if a smaller library has a much newer `newest_file` date, it may be the one you were actually using and the big one an old archive — try that instead.
- [ ] **3. A library with `likely_icloud_placeholders` = yes** may hold small stand-ins, not the real photos ("Optimize Mac Storage" leaves those; the real photos are in iCloud). It's only a guess from the typical file size: a library of genuinely small photos (an early digital camera, scans, screenshots) looks the same. Look at a few photos, and at icloud.com, before trusting it either way.
- [ ] **4. Copy that one library to the Mac** — drag the whole library from `SPARE` to the Mac. It needs about 20% more free space than its size.
- [ ] **5. Hold ⌥ Option + ⌘ Command while opening Photos**, choose the copy, and choose **Repair**. On a big library this takes hours.
  - If Photos offers to make it the **System Photo Library**, say no.
  - Don't turn on **iCloud Photos** for it (Photos → Settings → iCloud stays off): it would start syncing and merging with your iCloud.
- [ ] **6. Check what survived, in this order** — each can fail on its own: do the photos appear at all? Are the **dates** right? Are the **albums** there? Are **faces and keywords** there?
- [ ] **7. If photos are missing or blank, check that library's `damaged_inside` number** before re-running Repair. If thousands of files inside it were damaged, no amount of repairing brings them back.
- [ ] **8.** Compare against the other libraries: with one open, spot-check whether photos from the smaller ones are already in it.
- [ ] **9.** If Photos won't open it at all, **write down the exact error message.**

> **Don't merge libraries by hand and don't dedupe across them.** If two overlap,
> Photos' own Import handles it.

> **A library made on a newer macOS won't open on an older one.** If the Mac is behind,
> update it before deciding a library is dead.

## Job 3 — bring the results back

- [ ] Copy `sips-failures.txt` from the Mac's Desktop back to Windows (email it to yourself or use a USB stick — the Mac can't write to `SPARE`), plus any notes from Job 2.
- [ ] Unplug `SPARE` and take it back to the PC.

---

# PHASE 6 — The damaged files 🪟 (reconnect to the internet for the paid tools)

The damaged files are a short list: everything step 5 named, everything Phase 3
called NEVER_READ, TRUNCATED, HOLE, MISMATCH or DB_DAMAGED, the files
`rescue-log.csv` calls "partly copied", and — if you did Phases 4 and 5 — what
Krokiet, 7-Zip, MP3val and `sips` flagged.

- [ ] **1. 💬 Ask Claude for the copying helper.** It reads the program's lists and *copies* each damaged file from `WORK` into `SPARE\REPAIR`, in the same folders, with a list of what it copied and why each one is there. It never changes `WORK` and never takes anything out of a Photos Library or other package: a damaged file inside a library goes on the list, and the library is dealt with whole (Phase 5).
- [ ] **2. Open a sample of `REPAIR` by hand.** The extra checks sometimes flag files that are fine. If they open normally, you may need to buy nothing.
- [ ] **3. Near-twins first** — this rescues more photos than repair tools do. Krokiet → **Similar Images**, Included paths `SPARE\REPAIR` *and* `SPARE\WORK`, the `.photoslibrary` exclusion still in place, **Geometric invariance** → **Mirror + Flip + Rotate 90**. Use **Compare** to see pairs side by side. Where a damaged photo has an intact twin (an edited copy, an email attachment, an export), that picture is recovered: note which file it is. **Don't delete or move anything from the results.** Thumbs.db files and a library's previews can hold intact small versions too.
- [ ] **4. Repair tools — on the copies in `REPAIR` only.** Set expectations: **a repair tool produces a file that opens, not the file you had**; the damaged part is papered over with plausible filler. Try the free previews: Stellar and Repairit both show the repaired result before you pay. **untrunc** (already in `C:\RECOVERY-TOOLS\`) is free and fixes MP4/MOV videos that stop partway — but it needs a **good video from the same camera or phone** as a pattern.
- [ ] **5. Put each repaired file beside its damaged copy in `REPAIR`**, with a name that says so (`IMG_0123 (repaired).JPG`), and check it with step 7 (and Krokiet, if you like). A repair that hasn't been re-tested is a guess.
- [ ] **6. Keep every damaged original.** It's the only copy that exists, and repair tools get better every year.

---

# PHASE 7 — Keep it safe 🪟 (about 1 hr, plus copying time)

- [ ] **1. A `RECORDS` folder on `SPARE`:** copy into it the whole `C:\RECOVERY-TOOLS\results` folder, the mapfile, this checklist and your notes. (`WORK\rescue-log.csv` stays where it is: step 7 needs it there.) In three years you'll want to know what was checked.
- [ ] **2. A second copy, on another drive, kept away from home:** copy `WORK` (with its `rescue-log.csv`), `REPAIR` and `RECORDS` onto it. A 4 TB `IMAGE` drive has room beside the image. On a 2 TB `IMAGE` there's room for one or the other: the image has done its job once `WORK` is checked, and the recovered SSD in the drawer is the master, so replacing the image with the second copy is reasonable — but it's a deletion, so decide it yourself, not because this list says so.
- [ ] **3. Check the second copy:** in step 7, click **Choose…** beside the folder box, choose the second copy's `WORK` folder, then click **Check the copies are unchanged**. You want every copy exactly as copied.
- [ ] **4. Set a phone reminder every six months** to plug both drives in and run **Check the copies are unchanged** on each `WORK` (the same way). A file that has quietly changed shows up by name; take a good copy of it from the other drive. (Unpowered SSDs slowly lose their data: plugging them in twice a year also keeps them healthy.)
- [ ] **5.** The things you truly can't lose — usually the family photos — somewhere else as well, such as cloud storage.

> **MultiPar / PAR2 is optional now.** A PAR2 set covers at most 32,768 files, so "one
> set per top-level folder" fails for big folders, and a Photos Library you keep using
> changes all the time. Two copies plus the checksum list protect better and are
> simpler. PAR2 still suits a small folder you'll never change.

- [ ] **6. Undo Phase −1:**
  - `services.msc` → Windows Update → **Automatic (Delayed Start)** → Start
  - Settings → Windows Update → **Resume updates**
  - Advanced options → turn the restart setting back on
  - Power Options → back to **Balanced**
  - Remove the Defender exclusions for `SPARE` and `IMAGE`
  - `sysdm.cpl` → Advanced → Startup and Recovery → Settings → tick **Automatically restart** again
  - AutoPlay back on, if you want it
  - In Recovery Probe, step 8 should say *"Windows' setting for new disks is as it was."* If it doesn't (and the recovered SSD is unplugged), click **Put Windows' setting back**
  - Reconnect and let Windows catch up

---

## What this catches, and what it can't

| Kind of damage | Caught? |
|---|---|
| **Any part of a file the old drive never gave up** — even one 512-byte sector, even in a photo that looks perfect, even HEIC and RAW | **Yes** — the map check (Phase 2), marked NEVER_READ (Phase 3) |
| Files Windows can't even list, because their folder record was never read | **Yes** — found by the map check and copied anyway (Phase 2) |
| A folder whose own record was never read | **Yes** — found by its contents; its files keep their names (Phase 2). Only the folder's own name is lost |
| A copy that changes later — on the drive, in a later copy, over the years | **Yes** — the checksum list and *Check the copies are unchanged* (Phases 2 and 7) |
| File cut short (video stops halfway, JPEG without its end, PDF/zip missing their ending) | **Yes** — Phase 3 |
| The last part of a file never read (zeros at the end) | **Yes** for photos, PDFs, zips, MP3/FLAC/OGG and MP4/MOV/HEIC — Phase 3. RAW and MKV: Phases 4 and 5 |
| A video whose index was lost (won't play at all) | **Yes** — Phase 3 |
| MP3 cut short but claiming full length | **Yes** — MP3val, Phase 4 |
| Won't decode (video glitches partway, HEIC that won't open) | **Yes** — Phases 4 and 5 |
| Photos Library database damaged | **Yes** — Phase 3 |
| Bad sectors filled with zeros in the middle (opens fine, looks normal) | **Yes** — the map check, any size; the hole check too, for patches of 4 KB or more |
| Bad sector filled with leftover data (grey smear across a photo) | **Yes** — the map check knows where ddrescue failed, whatever the SSD holds there |
| Looks perfect but the content is subtly wrong, where the drive *did* read | **No** — the old drive would have had to hand over wrong data without an error, which is rare |

The last row is the loss nothing can find. The mapfile total from Phase 0 — about
600 KB — is how much was lost to places the drive could not read.

Realistic outcome: every file copied exactly, with a checksum; every damaged file
named; nothing deleted, nothing moved, nothing silently assumed good.
