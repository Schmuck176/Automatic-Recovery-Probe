# Automatic Recovery Checklist — Recovered SSD → safe, checked, repaired copies

*Version 2026-10-25 (1) — for Automatic Recovery Probe (`automatic-recovery-probe.exe`), written with Claude Opus 5.5.
It replaces the long checklist for the steps the program now does by itself: that one stays in Obsidian as it is,
with your ticks, for reference.*

**What you have:** the recovered SSD (2 TB, the ddrescue copy of your dead 1 TB drive), the 4 TB SSD (a copy of the
recovered SSD), `SPARE` (1 TB, empty, NTFS), 2 TB free on the PC's second drive, and a Mac. The recovered SSD and the
4 TB SSD are the only copies of your files until the image is made and checked.

**What the program does by itself, after you press Start:** makes the image of the recovered SSD and reads it back
to check it; works out from the mapfile exactly which files the damage touched (checking its own reading against
Windows' own, with AIM Toolkit); copies every file into `SPARE\WORK` with a checksum each, including the ones
Windows can't see; checks every copy; checks every file's structure, and plays every photo, song and video through
(7-Zip, MP3val, qpdf, FFmpeg); copies the damaged files into `SPARE\REPAIR` and repairs every one it can with
open-source tools — an intact copy found elsewhere first, then the format's own repair — puts each repaired file
back exactly where it belongs (the damaged original kept, every repair logged and undoable), and checks everything
again. Then it makes the rest ready for the Mac or a paid repair tool.

**What you do:** plug drives in and out when it asks, and decide about the files no free tool could repair.

## The rules

1. **Never plug the recovered SSD and the 4 TB SSD in at the same time** (the program refuses to go on if it sees
   both), and never restart the PC with either plugged in.
2. **Before the recovered SSD is plugged in, the rehearsal with `SPARE`** (Phase 2). The program then keeps the SSD
   switched off and read-only by itself: nothing, not even Windows, can write to it.
3. **Nothing in `WORK` is moved, renamed or deleted by hand** — not by you, not by another program. The program's
   own put-back is the only change ever made there, and every one is listed in `WORK\repair-returns.csv`.
4. **If the program stops and says a check found something, stop too.** Read what it says; if it isn't clear, ask
   Claude before pressing Start again.

> **How the program works:** double-click `automatic-recovery-probe.exe` in `C:\RECOVERY-TOOLS` and click **Yes**
> when Windows asks (it needs that to read the recovered SSD; it never writes to it). A black window opens — leave
> it open, it *is* the program — and your browser shows its page, on the **Automatic** tab. What every step found is
> kept in `C:\RECOVERY-TOOLS\results\`, so closing and reopening loses nothing, and **Start carries on where it
> stopped**. *(The old Recovery Probe still works too, from the same folder: both read the same results.)*

---

# PHASE 1 — Get ready 🪟 (10 minutes)

- [ ] Unplug every USB drive. Double-click `automatic-recovery-probe.exe` → **Yes**. The page opens on **Automatic**.
- [ ] **1. Get ready → Check this PC.** You want a ✓ for administrator rights, the mapfile (`1TB hard drive DDRescue-GUI second attempt.log`) and *Windows is not waiting to restart*. If it says Windows is waiting to restart: restart the PC now (no SSD plugged in), and start again.
- [ ] **Run the self-test.** A few seconds: it must say *"Every check passed: the program works on this PC."*
- [ ] **AIM Toolkit:** if it shows **Install AIM Toolkit**, click it, click **Install** in the window that opens, and **Yes** if Windows asks about the Arsenal Image Mounter driver. Then **Check this PC** again, and click **Try it**: it opens a small test image read-only and closes it again — you want *"AIM Toolkit works"*. (Without it everything still works, but the map check can't check itself against Windows' own reading.)
- [ ] **The tools:** 7-Zip, FFmpeg, MP3val and untrunc should show a version (you installed them). If **qpdf** shows **Download it**, click it (it comes from qpdf's own GitHub page, checked by its SHA-256), then **Check this PC** again.

---

# PHASE 2 — The rehearsal and the folders 🪟 (10 minutes)

- [ ] **2. The rehearsal → First: make Windows leave new disks switched off.**
- [ ] Plug `SPARE` in **exactly the way the recovered SSD will be plugged in**: the same USB case or adapter (the very same one, not just the same kind), the same port and cable.
- [ ] In the list, click **This is SPARE: check it**. You want *"SPARE arrived switched off"*. If it says SPARE arrived **switched on**, click **Switch SPARE off**, unplug SPARE, plug it back in exactly the same way, and click **This is SPARE: check it** again: now you want *"SPARE arrived switched off"*. It says to **stop** (removable media, or switched on again)? Then don't plug the recovered SSD in that way: tell Claude. When it has passed, unplug SPARE (it stays switched off until Start needs it).
- [ ] **3. The folders:** check the three it suggests — the image in `IMAGE` on a drive of the PC with about 1,000 GB free (on your PC, `C:`: `D:` is too small), the copies in `WORK` on `SPARE`, the second copy in `COPY 2` beside the image. Leave **Play every video through** ticked unless you're in a hurry (it runs last, overnight, and can be stopped).

---

# PHASE 3 — Start 🪟 (a day or so, mostly waiting)

- [ ] **4. Start.** It checks everything is ready, then asks you to plug in the recovered SSD.
- [ ] When it asks: **plug in the recovered SSD** — the 2 TB one, exactly as rehearsed (in the case `SPARE` was in: take `SPARE` out first). The 4 TB SSD stays in its drawer. It arrives switched off and the program marks it read-only by itself; the image takes a few hours (it reads the SSD once, then reads the image back to check it).
- [ ] When the page says *"The image is made and checked"*: in the program's steps tab, step 9 → **Switch it off**, then unplug the recovered SSD and put it back in its drawer. It is not needed again.
- [ ] If it asks for `SPARE` (because `SPARE` was in the case the SSD used): plug `SPARE` in. It goes on by itself.
- [ ] **✅ Checkpoint** — Start checks every copy against its checksum by itself; this ticks when that check passes.
- [ ] Start says **"Start is done"**. (If it stopped instead, it says why: a check that found something — ask Claude — or something to do, then **Carry on**.) Write down **Your numbers**:

> **Your numbers**, from the map check (the program fills them in). Every check from now on should show the same
> number of files.
>
> | Files on the SSD | Size in all |
> |---|---|
> |  |  |

---

# PHASE 4 — What is left: the Mac, a paid tool, bring back (optional)

**5. What is left** says how many files were repaired and put back, how many have something salvaged, and how many
no free tool could repair, and where each of those went in `SPARE\REPAIR\hand-off`. Files that open or play as they
are, with a glitch where data was lost, are not handed off: nothing can bring that part back.

- [ ] Click **Open the hand-off folder** and read `READ ME.txt`.
- [ ] 🍎 **For the Mac** (iPhone photos, camera RAW, Pages/Numbers/Keynote), if there are any: copy the folder `1 For the Mac` onto a USB stick formatted exFAT. On the Mac, double-click `Repair on the Mac.command` on the stick (if the Mac refuses: right-click → Open → Open). It tries each file with the Mac's own decoders and saves what it repairs into `returned` on the stick. **Never** plug the recovered SSD or the 4 TB SSD into the Mac.
- [ ] **Paid repair tools**, only if you decide some files are worth it: use only the copies in `2 For a paid repair tool`; look at the tool's free preview first. Save each repaired file into `hand-off\returned`, keeping its number (`[R0012]`) in its name.
- [ ] **Office documents**, if any (`3 For their own program`): Word, Excel or PowerPoint → File → Open → arrow beside Open → **Open and Repair**; save into `hand-off\returned` with the same name.
- [ ] **Bring back:** choose the USB stick (or `hand-off\returned`) and click **Bring back**. It puts every repaired file back exactly where it belongs, and checks everything again.
- [ ] **✅ Checkpoint** — Bring back checks every copy by itself; this ticks when that check passes.
- [ ] 🍎 **A Photos library with damage inside** (What is left names it): copy the whole library from `SPARE` to the Mac, hold **⌥ Option** while opening Photos and choose the copy; only if it won't open or looks wrong, repair the copy (old checklist, Phase 7, steps 5–12). The library in `WORK` stays as it is.

---

# PHASE 5 — Keep it safe 🪟 (an hour, plus copying time)

- [ ] **6. Finalise.** It makes `RECORDS` beside `WORK` (the results, the mapfile, the checklists), copies `WORK`, `REPAIR` and `RECORDS` into `COPY 2`, checks both copies and the image, and puts Windows' settings it changed back as they were.
- [ ] **✅ The final check — both copies** — this ticks when Finalise's checks of both copies pass.
- [ ] Keep `SPARE` somewhere else (a relative's, work). Set a phone reminder every six months: plug it in, double-click the program, **Finalise** again — it checks both copies and the image, and names any file that has changed.
- [ ] The things you truly can't lose (usually the family photos) somewhere else as well, such as cloud storage.
- [ ] Undo what the old checklist's Phase −1 changed by hand: Windows Update service back to **Automatic (Delayed Start)** and **Resume updates**; the restart setting back on; Power Options back to **Balanced**; the Defender exclusion you added for `SPARE` (Windows Security → Virus & threat protection → Manage settings → Exclusions; the ones the program added, Finalise took off); AutoPlay back on if you want it. Then reconnect and let Windows catch up.

---

## What this catches, and what it can't

| | |
|---|---|
| Any part of a file the old drive never gave up — even one 512-byte sector, even in a photo that looks perfect | **Found**: the map check names it (NEVER_READ) |
| Files Windows can't even list, and folders whose own record was lost | **Found and copied** anyway |
| A file cut short, a hole of zeros, the wrong content for its name | **Found**: the check of every file |
| A photo, song or video that won't decode; an archive, Office file or PDF that won't read | **Found**: the extra checks (FFmpeg, 7-Zip, qpdf, MP3val) |
| A copy that changes later, a file deleted, moved or added by mistake | **Found**: every check of the copies, now and twice a year |
| A damaged file with an intact copy anywhere in `WORK` (or in a folder you named) | **Restored exactly** |
| A photo cut short, a video that lost its index, an MP3's broken frames, a PDF's or zip's lost index | **Repaired** with open-source tools, and put back |
| The bytes the old drive never gave up, in the middle of a picture or a song | **Can't be brought back** by any tool, free or paid: the file opens, with a glitch there |
