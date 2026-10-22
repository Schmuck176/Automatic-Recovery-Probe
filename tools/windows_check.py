#!/usr/bin/env python3
"""Checks the map check against Windows itself (run on GitHub's Windows
machines, as administrator).

Part 1 - test disks made by exfat_image.py, attached as virtual disks:
  * Windows must mount them and read every file back exactly as written.
  * After damage, the files Windows can still list must be exactly the ones
    the map check says it can list, and the map check, reading the attached
    disk as \\\\.\\PhysicalDriveN (the way it will read the recovered SSD),
    must give the same answers as on the image file.

  * Files the map check says Windows cannot list must not be listed, and
    Windows must open by its full path exactly the hidden files the map
    check says it still opens that way.
  * The map check's own comparison with what Windows lists (it finds the
    attached disk's drive letter by the volume's serial number) must agree,
    and must see that the volume is read-only.
  * Records left after a folder's end (tools/test_mapcheck.py,
    leftover_case) are never listed by Windows, even undamaged.

Part 2 - a disk formatted and filled by Windows itself:
  * Every 512-byte block of every file names its file and position, so after
    damaging sectors the right answer is known without reading exFAT at all.
  * The map check must name exactly the damaged files and byte ranges, and
    every file Windows can no longer list must be one the map check reports
    as hidden (or whose folder record the damage destroyed).

Part 3 - one folder of 300 files with one record damaged in each of a dozen
ways: what Windows lists and opens against what the map check predicts.
This is what the map check's model of Windows (exfat.rs, Stop) rests on.

    python windows_check.py <recovery-probe.exe> <work folder>
"""

import json
import os
import random
import re
import shutil
import string
import struct
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import exfat_image as X  # noqa: E402
import test_mapcheck as T  # noqa: E402

SECTOR = 512
LOG = []


def log(msg):
    print(msg, flush=True)
    LOG.append(msg)


def ps(cmd, check=True):
    r = subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command", cmd],
                       stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    out = r.stdout.decode("utf-8", "replace").strip()
    if check and r.returncode != 0:
        raise RuntimeError("PowerShell failed: %s\n%s" % (cmd, out))
    return out


def diskpart(lines):
    path = os.path.join(os.environ.get("RUNNER_TEMP", "."), "diskpart-script.txt")
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")
    r = subprocess.run(["diskpart", "/s", path], stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    out = r.stdout.decode("utf-8", "replace")
    if r.returncode != 0:
        raise RuntimeError("diskpart failed:\n%s" % out)
    return out


def make_container(path, size_mib):
    """A fixed-size VHD made by Windows (diskpart), so its footer is Windows' own."""
    if os.path.exists(path):
        os.remove(path)
    diskpart(['create vdisk file="%s" maximum=%d type=fixed' % (path, size_mib)])


def put_image(vhd, img):
    """Copy a raw disk image into a VHD container of the same size."""
    with open(img, "rb") as src, open(vhd, "r+b") as dst:
        while True:
            chunk = src.read(8 << 20)
            if not chunk:
                break
            dst.write(chunk)


def free_letter():
    for c in "RSTUVWXYZQPONMLKJ":
        if not os.path.exists(c + ":\\"):
            return c
    raise RuntimeError("no free drive letter")


def mount(vhd, readonly=True):
    """Attach a VHD; return (disk number, drive letter or None)."""
    ps("Mount-DiskImage -ImagePath '%s' -StorageType VHD -Access %s | Out-Null"
       % (vhd, "ReadOnly" if readonly else "ReadWrite"))
    n = int(ps("(Get-DiskImage -ImagePath '%s').Number" % vhd))
    if ps("(Get-Disk -Number %d).IsOffline" % n).lower() == "true":
        ps("Set-Disk -Number %d -IsOffline $false" % n, check=False)
    letter = None
    for _ in range(30):
        out = ps("Get-Partition -DiskNumber %d | Where-Object { $_.DriveLetter } | "
                 "ForEach-Object { $_.DriveLetter }" % n, check=False)
        m = re.search(r"^([A-Z])\s*$", out, re.M)
        if m:
            letter = m.group(1) + ":"
            break
        time.sleep(1)
    if letter is None:
        # Give the basic data partition a letter ourselves.
        c = free_letter()
        out = ps("Get-Partition -DiskNumber %d | Where-Object { $_.GptType -eq '{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}' } | "
                 "Set-Partition -NewDriveLetter %s" % (n, c), check=False)
        for _ in range(20):
            if os.path.exists(c + ":\\"):
                letter = c + ":"
                break
            time.sleep(1)
        if letter is None:
            log("     (no drive letter: %s)" % out[-300:])
    if letter:
        for _ in range(20):
            if os.path.exists(letter + "\\"):
                break
            time.sleep(1)
    return n, letter


def filesystem(n):
    """What Windows makes of the exFAT partition: 'exFAT', or 'Unknown'/'RAW' when it cannot open it."""
    out = ps("Get-Partition -DiskNumber %d | Where-Object { $_.GptType -eq '{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}' } | "
             "Get-Volume | ForEach-Object { $_.FileSystemType }" % n, check=False)
    return out.strip() or "no volume"


def unmount(vhd):
    ps("Dismount-DiskImage -ImagePath '%s' | Out-Null" % vhd, check=False)


def literal(letter, path=""):
    """A path Windows takes literally, so names ending in a space or a dot
    (which exFAT allows) are not trimmed on the way."""
    return "\\\\?\\" + letter + "\\" + path


def opens(letter, path):
    return os.path.lexists(literal(letter, path))


def listing(root, read=True):
    out = {}
    top = literal(root)
    for base, dirs, files in os.walk(top):
        rel = base[len(top):].strip("\\")
        for d in dirs:
            out[(rel + "\\" if rel else "") + d] = None
        for f in files:
            p = os.path.join(base, f)
            data = None
            if read:
                try:
                    with open(p, "rb") as fh:
                        data = fh.read()
                except OSError as e:
                    data = e
            out[(rel + "\\" if rel else "") + f] = data
    return out


def map_check(exe, mapfile, disk, out):
    r = subprocess.run([exe, "map-check", "--map", mapfile, "--disk", disk, "--out", out],
                       stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    text = r.stdout.decode("utf-8", "replace")
    if r.returncode != 0:
        raise RuntimeError("map-check failed on %s:\n%s" % (disk, text[-1500:]))
    with open(os.path.join(out, "map-check.json"), encoding="utf-8") as f:
        return json.load(f), text


IGNORE = ("System Volume Information",)


def mine(paths):
    return {p for p in paths if not p.split("\\")[0] in IGNORE}


# ---------------------------------------------------------------- part 1

def part1(exe, work, case):
    d = os.path.join(work, case.name)
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    problems = []
    truth = case.disk.truth()
    # A VHD is a whole number of MiB; a disk that ends sooner simply sits at
    # its start (like a copy on a bigger SSD).
    size_mib = -(-truth["disk_size"] // X.MiB)
    pristine = os.path.join(d, "pristine.img")
    img = os.path.join(d, "disk.img")
    case.disk.save(pristine)
    shutil.copyfile(pristine, img)
    zero = case.areas if case.zero is None else case.zero
    X.zero_areas(img, [(s, n) for s, n, *_ in zero])
    if case.garbage:
        g = random.Random(len(case.name))  # the same bytes test_mapcheck.py writes
        with open(img, "r+b") as f:
            for s, n, *_ in case.garbage:
                f.seek(s)
                f.write(g.randbytes(n))
    flipped = set()  # the names the changed records now spell
    for off in case.flips:
        slot = off - off % 32
        for x in truth["nodes"]:
            if slot in x["record_offsets"][2:]:
                k = x["record_offsets"].index(slot) - 2
                unit = 15 * k + ((off % 32) - 2) // 2
                name = x["path"].split("\\")[-1]
                name = name[:unit] + chr(ord(name[unit]) ^ 0x20) + name[unit + 1:]
                flipped.add("\\".join(x["path"].split("\\")[:-1] + [name]))
    if case.flips:
        with open(img, "r+b") as f:
            for off in case.flips:
                f.seek(off)
                b = f.read(1)
                f.seek(off)
                f.write(bytes([b[0] ^ 0x20]))
    mp = os.path.join(d, "disk.map")
    X.write_mapfile(mp, truth["gpt_disk_size"], [(s, n, st) for s, n, st in case.areas])
    oracle = T.Oracle(truth, img, pristine, [(s, s + n, st) for s, n, st in case.areas])
    try:
        exp = oracle.expected()
    finally:
        oracle.close()

    # Windows reads the undamaged disk exactly as written.
    vhd = os.path.join(d, "pristine.vhd")
    make_container(vhd, size_mib)
    put_image(vhd, pristine)
    n, letter = mount(vhd)
    try:
        if not letter:
            problems.append("Windows gave the undamaged test disk no drive letter")
        else:
            got = listing(letter)
            # (Records left after a folder's end are no files to Windows.)
            want = {x.path: x for x in case.disk.vol.nodes()
                    if x is not case.disk.vol.root and not x.deleted and not x.leftover}
            if mine(got) != set(want):
                problems.append("Windows lists different files: only Windows: %s; only the builder: %s" % (
                    sorted(mine(got) - set(want))[:4], sorted(set(want) - mine(got))[:4]))
            bad = [p for p, x in want.items() if not x.is_dir and p in got
                   and got[p] != x.data[:x.vdl] + bytes(x.data_length - x.vdl)]
            if bad:
                problems.append("Windows reads %d files differently, e.g. %s" % (len(bad), bad[:3]))
            log("     undamaged: Windows mounts it as %s and reads all %d files and folders as written"
                % (letter, len(want)) if not problems else "     undamaged: problems")
    finally:
        unmount(vhd)
        os.remove(vhd)

    # The damaged disk: what Windows still lists, and what the map check says
    # reading the attached disk the way it will read the SSD.
    vhd = os.path.join(d, "damaged.vhd")
    make_container(vhd, size_mib)
    put_image(vhd, img)
    n, letter = mount(vhd)
    try:
        listable = set(exp["found"]) - set(exp["hidden"])
        maybe = set(exp["hidden_maybe"])  # before the damaged record of a folder Windows gives up on
        got_raw, text = map_check(exe, mp, "\\\\.\\PhysicalDrive%d" % n, os.path.join(d, "out-raw"))
        diffs = T.compare(exp, got_raw)
        if diffs:
            problems.append("map check on \\\\.\\PhysicalDrive%d differs from the expected answer: %s" % (n, diffs[:4]))
        fs = filesystem(n)
        if fs.lower() != "exfat":
            problems.append("Windows cannot open the damaged test disk at all (it sees: %s)" % fs)
        elif not letter:
            problems.append("Windows gave the damaged test disk no drive letter")
        else:
            got = listing(letter, read=False)
            only_win = mine(got) - listable
            only_map = listable - mine(got)
            if maybe:
                log("     %d files sit before the damaged record of a folder Windows gives up on: Windows lists %d "
                    "of them (the map check says none)" % (len(maybe), len(maybe & mine(got))))
            if flipped:
                # A record whose checksum is wrong: does Windows still list it (under its changed name)?
                listed = flipped & only_win
                log("     a record with a wrong checksum: Windows %s (%s)" % (
                    "still lists it, under the changed name" if listed else "does not list it",
                    sorted(flipped)[0]))
                only_win -= flipped
            if only_win or only_map:
                problems.append("damaged: Windows lists %d, the map check says %d can be listed; only Windows: %s; "
                                "only the map check: %s" % (len(mine(got)), len(listable), sorted(only_win)[:4],
                                                            sorted(only_map)[:4]))
            # The map check's own comparison with what Windows lists there
            # (it found the drive by the volume's serial number).
            wc = got_raw.get("windows_compare")
            if wc is None:
                problems.append("the map check did not compare itself with what Windows lists on %s" % letter)
            else:
                ow = {p for p in wc["only_windows"] if p.split("\\")[0] not in IGNORE} - flipped
                oh = {p for p in wc["only_here"] if p.split("\\")[0] not in IGNORE}
                if ow or oh or wc["other_size_count"] or wc["failed"]:
                    problems.append("the map check's own comparison with what Windows lists differs: only Windows: %s; "
                                    "only the map check: %s; another size: %s; %s" % (
                                        sorted(ow)[:4], sorted(oh)[:4], wc["other_size"][:3], wc["failed"] or ""))
                if wc["read_only"] is not True:
                    problems.append("the map check says %s is %s, but it is mounted read-only" % (
                        letter, "writable" if wc["read_only"] is False else "of unknown kind"))
            if got_raw["volume"]["drive"] is None:
                problems.append("the map check did not find the volume's drive letter %s" % letter)
            else:
                # windows_can_see is whether Windows opens the file by its
                # name (which can work even where listing its folder fails).
                for it in got_raw["items"] + got_raw["hidden"]:
                    p = it["path"]
                    if not p or p.split("\\")[0] in IGNORE:
                        continue
                    see = opens(letter, p)
                    if it["windows_can_see"] != see:
                        problems.append("windows_can_see for %r is %s, but Windows %s open it by name" % (
                            p, it["windows_can_see"], "does" if see else "does not"))
                # The prediction: of the files Windows cannot list, it still
                # opens by full path exactly those before a damaged record.
                wrong = [h["path"] for h in got_raw["hidden"]
                         if h["path"].split("\\")[0] not in IGNORE and opens(letter, h["path"]) != h["maybe"]]
                if wrong:
                    problems.append("Windows opens %d hidden files by full path other than predicted, e.g. %s (%s)" % (
                        len(wrong), wrong[:3], "predicted to open" if [h for h in got_raw["hidden"]
                                                                       if h["path"] == wrong[0]][0]["maybe"]
                        else "predicted not to open"))
            log("     damaged: Windows lists %d files and folders; %d are hidden by the damage, as predicted"
                % (len(mine(got)), len(exp["hidden"])))
    finally:
        unmount(vhd)
        os.remove(vhd)
    for f in ("disk.img", "pristine.img"):
        os.remove(os.path.join(d, f))
    return problems


# ---------------------------------------------------------------- part 2

def part2(exe, work):
    """A disk Windows formats and fills itself; damage found by content tags."""
    d = os.path.join(work, "windows-formatted")
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    problems = []
    size_mib = 96
    vhd = os.path.join(d, "win.vhd")
    make_container(vhd, size_mib)
    letter = free_letter()
    diskpart(['select vdisk file="%s"' % vhd, "attach vdisk", "convert gpt",
              "create partition primary", 'format fs=exfat quick label="WINTEST" unit=4096',
              "assign letter=%s" % letter])
    root = letter + ":\\Test"
    files = {}  # path (from the volume root) -> (fid, size)
    fid = 1
    try:
        for _ in range(30):
            if os.path.exists(letter + ":\\"):
                break
            time.sleep(1)
        os.makedirs(os.path.join(root, "Big folder"))
        os.makedirs(os.path.join(root, "Photos", "2021"))
        os.makedirs(os.path.join(root, "Docs"))
        sizes = [5000 + 1371 * i for i in range(80)]
        for i, s in enumerate(sizes):
            p = "Test\\Big folder\\file %03d with a fairly long name.bin" % i
            files[p] = (fid, s)
            fid += 1
        for i in range(25):
            p = "Test\\Photos\\2021\\IMG_%04d.JPG" % i
            files[p] = (fid, 150_000 + 7919 * i)
            fid += 1
        for i in range(10):
            p = "Test\\Docs\\document %d.docx" % i
            files[p] = (fid, 20_000 + 333 * i)
            fid += 1
        files["Test\\Docs\\after the folders.txt"] = (fid, 3000)
        fid += 1
        for p, (f, s) in files.items():
            with open(letter + ":\\" + p, "wb") as fh:
                fh.write(X.tagged(f, s))
    finally:
        diskpart(['select vdisk file="%s"' % vhd, "detach vdisk"])

    size = size_mib * X.MiB
    # Where every tagged block landed, and where each name is recorded.
    blocks = {}  # (fid, block) -> disk offset
    names_at = {}
    by_fid = {f: p for p, (f, s) in files.items()}
    with open(vhd, "rb") as fh:
        pos = 0
        while pos < size:
            chunk = fh.read(min(8 << 20, size - pos))
            for k in range(0, len(chunk), SECTOR):
                if chunk[k:k + 6] == b"RPTAG ":
                    m = re.match(rb"RPTAG (\d{6}) (\d{8}) ", chunk[k:k + 22])
                    if m:
                        blocks[(int(m.group(1)), int(m.group(2)))] = pos + k
            for p in files:
                nm = p.split("\\")[-1][:15].encode("utf-16-le")
                i = chunk.find(nm)
                while i >= 0:
                    names_at.setdefault(p, []).append(pos + i)
                    i = chunk.find(nm, i + 1)
            pos += len(chunk)
    tagged = set(blocks.values())
    missing = [p for p, (f, s) in files.items() if (f, 0) not in blocks]
    if missing:
        problems.append("could not find the data of %d Windows-written files, e.g. %s" % (len(missing), missing[:3]))
        return problems

    # Damage: two files' data, and a folder record in the middle of "Big folder".
    a_fid, a_size = files["Test\\Photos\\2021\\IMG_0005.JPG"]
    b_fid, b_size = files["Test\\Docs\\document 3.docx"]
    # (The last block of a file may be too short to hold a whole tag; use the one before.)
    areas = [(blocks[(a_fid, 40)], 2 * SECTOR), (blocks[(b_fid, (b_size - 1) // SECTOR - 1)], SECTOR)]
    victim = "Test\\Big folder\\file 040 with a fairly long name.bin"
    rec = [o for o in names_at.get(victim, []) if o // SECTOR * SECTOR not in tagged]
    if len(rec) != 1:
        problems.append("expected one folder record naming %r, found %d" % (victim, len(rec)))
        return problems
    rec_sector = rec[0] // SECTOR * SECTOR
    areas.append((rec_sector, SECTOR))
    areas.sort()
    with open(vhd, "r+b") as fh:
        for s, n in areas:
            fh.seek(s)
            fh.write(bytes(n))
    mp = os.path.join(d, "win.map")
    X.write_mapfile(mp, size, [(s, n, "-") for s, n in areas])

    # The right answer, from the tags alone.
    expect_items = {}
    for (f, blk), off in blocks.items():
        for s, n in areas:
            if s <= off < s + n:
                p = by_fid[f]
                fsize = files[p][1]
                lo = blk * SECTOR
                hi = min(lo + SECTOR, fsize)
                expect_items.setdefault(p, []).append([lo, hi])
    for p, r in expect_items.items():
        r.sort()
        merged = []
        for x in r:
            if merged and merged[-1][1] == x[0]:
                merged[-1][1] = x[1]
            else:
                merged.append(x)
        expect_items[p] = merged

    n, letter = mount(vhd)
    try:
        got, text = map_check(exe, mp, "\\\\.\\PhysicalDrive%d" % n, os.path.join(d, "out"))
        with open(os.path.join(d, "map-check-console.txt"), "w", encoding="utf-8") as f:
            f.write(text)
        win = mine(listing(letter, read=False)) if letter else set()
        by_name = {p for p in files} | {"Test", "Test\\Big folder", "Test\\Photos", "Test\\Photos\\2021", "Test\\Docs"}
        by_name = {p for p in by_name if letter and opens(letter, p)}
    finally:
        unmount(vhd)
    got_files = {i["path"]: [[a, b] for a, b, _ in i["ranges"]] for i in got["items"] if not i["is_dir"]}
    if got_files != expect_items:
        problems.append("Windows-formatted disk: damaged files differ: got %s, expected %s" % (
            sorted(got_files.items())[:3], sorted(expect_items.items())[:3]))
    hidden = {h["path"] for h in got["hidden"]}
    all_paths = set(files) | {"Test", "Test\\Big folder", "Test\\Photos", "Test\\Photos\\2021", "Test\\Docs"}
    not_listed = all_paths - win
    if not letter:
        problems.append("Windows gave the damaged Windows-formatted disk no drive letter")
    else:
        wrongly_hidden = hidden - not_listed
        if wrongly_hidden:
            problems.append("the map check says Windows cannot list %s, but it can" % sorted(wrongly_hidden)[:4])
        unexplained = not_listed - hidden
        # Whatever else Windows cannot list must be a file whose own record the damage destroyed.
        def record_hit(p):
            # A record is: file entry, stream entry, then one name entry per 15 letters;
            # the name search finds the first name entry's letters (2 bytes into it).
            names = -(-len(p.split("\\")[-1]) // 15)
            for o in names_at.get(p, []):
                if o // SECTOR * SECTOR in tagged:
                    continue
                first = o - 2 - 64
                last = o - 2 + 32 * names
                if first < rec_sector + SECTOR and last > rec_sector:
                    return True
            return False

        destroyed = {p for p in unexplained if record_hit(p)}
        if unexplained - destroyed:
            problems.append("Windows cannot list %s, and the map check does not say why" % sorted(unexplained - destroyed)[:4])
        if not hidden:
            problems.append("the damage hid nothing on the Windows-formatted disk (the test missed its target)")
        for it in got["items"] + got["hidden"]:
            p = it["path"]
            if p in all_paths and it["windows_can_see"] != (p in by_name):
                problems.append("windows_can_see for %r is %s, but Windows %s open it by name" % (
                    p, it["windows_can_see"], "does" if p in by_name else "does not"))
        log("     Windows-formatted disk: %d damaged files found exactly; Windows cannot list %d files, "
            "%d of them reported as hidden, %d had their own record destroyed" % (
                len(got_files), len(not_listed), len(hidden & not_listed), len(destroyed)))
    os.remove(vhd)
    return problems


def experiments(work):
    """Which single pieces of damage stop Windows from opening the drive."""
    disk, exps = T.mount_experiments()
    d = os.path.join(work, "experiments")
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    size_mib = disk.disk_size // X.MiB
    pristine = os.path.join(d, "pristine.img")
    disk.save(pristine)
    for name, areas in exps:
        img = os.path.join(d, "disk.img")
        shutil.copyfile(pristine, img)
        X.zero_areas(img, areas)
        vhd = os.path.join(d, "exp.vhd")
        make_container(vhd, size_mib)
        put_image(vhd, img)
        try:
            n, letter = mount(vhd)
            fs = filesystem(n)
            count = len(listing(letter, read=False)) if letter and fs.lower() == "exfat" else 0
            if fs.lower() == "exfat":
                log("     damage to %s: Windows opens the drive (%s) and lists %d files and folders" % (name, letter, count))
            else:
                log("     damage to %s: Windows CANNOT open the drive (it sees: %s)" % (name, fs))
        except Exception as e:
            log("     damage to %s: error %s" % (name, e))
        finally:
            unmount(vhd)
            if os.path.exists(vhd):
                os.remove(vhd)
    shutil.rmtree(d, ignore_errors=True)


def listing_experiments(exe, work):
    """One folder of 300 files with one record damaged in a particular way:
    what Windows lists, and opens by full path, against what the map check
    predicts for the same disk. Returns the experiments that disagree."""
    d = os.path.join(work, "listing-experiments")
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    v = X.Volume(24 * X.MiB, cluster_size=4096, label="LISTTEST", serial=0x11223344)
    v.mkdir("Many")
    for i in range(300):
        v.add_file("Many/file %03d.txt" % i, X.tagged(i + 1, 1000))
    v.add_file("after.txt", X.tagged(999, 1000))
    disk = X.Disk(v, 32 * X.MiB, efi_start_lba=40, efi_size=2 * X.MiB, vol_start_lba=40 + 4096 + 7)
    t = disk.truth()
    rec = {n["path"]: n["record_offsets"] for n in t["nodes"]}
    everything = {n["path"] for n in t["nodes"] if n["path"]}
    pristine = os.path.join(d, "pristine.img")
    disk.save(pristine)
    victim = rec["Many\\file 150.txt"]  # [file part, stream part, name part]
    sector = victim[0] // SECTOR * SECTOR
    rnd = random.Random(7)
    exps = [
        ("one letter of a name changed (checksum wrong)", [(victim[2] + 2 + 2 * 3, b"\x5a")]),
        ("the name part of a record blanked (a blank inside a record)", [(victim[2], bytes(32))]),
        ("a record's first part marked deleted, its other parts not (stray parts)", [(victim[0], b"\x05")]),
        ("a record's first part blanked (a blank record)", [(victim[0], bytes(32))]),
        ("a record of a kind that does not exist (type 86)", [(victim[0], b"\x86")]),
        ("a record only the top folder may hold (type 83, a label)", [(victim[0], b"\x83")]),
        ("a vendor record where a file record was (type A1)", [(victim[0], b"\xa1")]),
        ("a record claiming no parts (0)", [(victim[0] + 1, b"\x00")]),
        ("a record claiming too few parts (1)", [(victim[0] + 1, b"\x01")]),
        ("a record claiming too many parts (5)", [(victim[0] + 1, b"\x05")]),
        ("a record claiming more parts than a record can have (19)", [(victim[0] + 1, b"\x13")]),
        ("a record claiming 200 parts", [(victim[0] + 1, b"\xc8")]),
        ("a record whose name length is too long for its parts", [(victim[1] + 3, b"\x1e")]),
        ("a record whose second part is marked deleted", [(victim[1], b"\x40")]),
        ("a record whose second part is a vendor part (type E0)", [(victim[1], b"\xe0")]),
        ("one sector of the folder blanked", [(sector, bytes(SECTOR))]),
        ("one sector of the folder holding random bytes", [(sector, rnd.randbytes(SECTOR))]),
        ("one sector of the folder holding other random bytes", [(sector, rnd.randbytes(SECTOR))]),
    ]
    size_mib = disk.disk_size // X.MiB
    failed = []
    for name, patches in exps:
        img = os.path.join(d, "disk.img")
        shutil.copyfile(pristine, img)
        touched = set()
        with open(img, "r+b") as f:
            for off, data in patches:
                f.seek(off)
                f.write(data)
                touched.update(range(off // SECTOR, (off + len(data) - 1) // SECTOR + 1))
        mp = os.path.join(d, "disk.map")
        bad = [(k * SECTOR, SECTOR, "-") for k in sorted(touched)]
        X.write_mapfile(mp, t["gpt_disk_size"], bad)
        # The right answer, from the oracle (which knows what was changed).
        oracle = T.Oracle(t, img, pristine, [(a, a + n, st) for a, n, st in bad])
        try:
            exp = oracle.expected()
        finally:
            oracle.close()
        listable = set(exp["found"]) - set(exp["hidden"])
        by_path = listable | set(exp["hidden_maybe"])
        vhd = os.path.join(d, "exp.vhd")
        make_container(vhd, size_mib)
        put_image(vhd, img)
        try:
            n, letter = mount(vhd)
            if not letter:
                log("FAIL %s: no drive letter" % name)
                failed.append(name)
                continue
            got, _ = map_check(exe, mp, "\\\\.\\PhysicalDrive%d" % n, os.path.join(d, "out"))
            diffs = T.compare(exp, got)
            listed = mine(listing(letter, read=False)) & everything
            expect_listed = listable & everything
            open_wrong = sorted(p for p in everything if opens(letter, p) != (p in by_path))
            many = sorted(p for p in listed if p.startswith("Many\\"))
            what = "Windows lists %d of the folder's files%s" % (
                len(many), " (%s .. %s)" % (many[0].split("\\")[1], many[-1].split("\\")[1]) if many else "")
            if diffs:
                failed.append(name)
                log("FAIL %s: the map check differs from the oracle: %s" % (name, diffs[:3]))
            elif listed == expect_listed and not open_wrong:
                wc = got.get("windows_compare") or {}
                ow = [p for p in wc.get("only_windows", []) if p.split("\\")[0] not in IGNORE]
                oh = [p for p in wc.get("only_here", []) if p.split("\\")[0] not in IGNORE]
                if not wc or ow or oh or wc.get("other_size_count") or wc.get("failed"):
                    failed.append(name)
                    log("FAIL %s: %s, as predicted, but the map check's own comparison with Windows says otherwise: "
                        "%s" % (name, what, {k: wc.get(k) for k in ("only_windows", "only_here", "other_size", "failed")}))
                else:
                    log("ok   %s: %s, and opens by full path just the ones predicted" % (name, what))
            else:
                failed.append(name)
                log("FAIL %s: %s; the map check predicts %d listed (only Windows: %s; only the map check: %s); "
                    "opened by full path other than predicted: %s" % (
                        name, what, len(expect_listed), sorted(listed - expect_listed)[:3],
                        sorted(expect_listed - listed)[:3], open_wrong[:3]))
        except Exception as e:
            failed.append(name)
            log("FAIL %s: error %s" % (name, e))
        finally:
            unmount(vhd)
            if os.path.exists(vhd):
                os.remove(vhd)
    shutil.rmtree(d, ignore_errors=True)
    return failed


def main():
    exe = os.path.abspath(sys.argv[1])
    work = os.path.abspath(sys.argv[2])
    os.makedirs(work, exist_ok=True)
    cases = [T.deliberate_cases(4096)[0], T.deliberate_cases(32768)[0], T.hidden_folder_case(),
             T.bigger_ssd_case(), T.leftover_case()]
    for c in cases:
        c.garbage = []
    # Random disks too (with garbage, like a used SSD's leftovers).
    cases += [T.random_case(seed) for seed in range(3001, 3013)]
    for c in cases:
        T.keep_mountable(c)
    log("What stops Windows from opening the drive:")
    experiments(work)
    log("How Windows reads a folder of 300 files with one record (file 150) damaged:")
    bad = listing_experiments(exe, work)
    failed = 1 if bad else 0
    total = 1
    for c in cases:
        total += 1
        log("%s:" % c.name)
        try:
            problems = part1(exe, work, c)
        except Exception as e:  # report and carry on with the next disk
            problems = ["error: %s" % e]
        if problems:
            failed += 1
            log("FAIL %s" % c.name)
            for p in problems[:8]:
                log("     " + p)
        else:
            log("ok   %s" % c.name)
    total += 1
    log("windows-formatted:")
    try:
        problems = part2(exe, work)
    except Exception as e:
        problems = ["error: %s" % e]
    if problems:
        failed += 1
        log("FAIL windows-formatted")
        for p in problems[:8]:
            log("     " + p)
    else:
        log("ok   windows-formatted")
    log("Windows checks: %d of %d agree with Windows itself (the folder experiments count as one%s)" % (
        total - failed, total, "; %d of them disagree" % len(bad) if bad else ""))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
