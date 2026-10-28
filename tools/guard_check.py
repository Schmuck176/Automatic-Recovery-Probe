#!/usr/bin/env python3
"""Checks that the program keeps Windows from writing to the recovered disk
(run on GitHub's Windows machines, as administrator). Test disks are
attached as virtual disks that Windows may write to; each one's bytes are
compared with the original afterwards.

  A. Unprotected: what Windows writes to an exFAT disk just by being given
     it (so the protection is known to be needed, and what it stops).
  B. Protected the way the front end does it: `disk-guard prepare` before
     the disk arrives, so it arrives switched off; the map check reads it
     like that; `disk-guard protect --online` marks it read-only and
     switches it on; nothing can be written; the disk comes back unchanged;
     `disk-guard restore` puts Windows' setting back.
  C. The same disk attached again later: does Windows remember it is
     read-only? (Logged: for virtual disks it does not.)
  D. A disk that was plugged in before Windows was ready, protected after.
  E. Windows' own disk: the program refuses to switch it off or mark it
     read-only, from the command line too.

    python guard_check.py <recovery-probe.exe> <work folder>
"""

import os
import shutil
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import exfat_image as X  # noqa: E402
import test_mapcheck as T  # noqa: E402
import windows_check as W  # noqa: E402

SECTOR = 512
RESULTS = []  # (ok, what)


def log(msg):
    print(msg, flush=True)


def check(ok, what):
    RESULTS.append((bool(ok), what))
    log("%s %s" % ("ok  " if ok else "FAIL", what))


def rp(exe, *args):
    r = subprocess.run([exe] + list(args), stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    return r.returncode, r.stdout.decode("utf-8", "replace").strip()


def attach(vhd):
    W.ps("Mount-DiskImage -ImagePath '%s' -StorageType VHD -Access ReadWrite | Out-Null" % vhd)
    return int(W.ps("(Get-DiskImage -ImagePath '%s').Number" % vhd))


def detach(vhd):
    W.ps("Dismount-DiskImage -ImagePath '%s' | Out-Null" % vhd, check=False)


def disk_info(n):
    out = W.ps("Get-Disk -Number %d | ForEach-Object { \"$($_.IsOffline) $($_.IsReadOnly) $($_.OfflineReason)\" }" % n,
               check=False)
    parts = out.split()
    if len(parts) < 2:
        return {"raw": out}
    return {"offline": parts[0].lower() == "true", "read_only": parts[1].lower() == "true",
            "reason": " ".join(parts[2:])}


def letter_of(n, wait=30):
    for _ in range(wait):
        out = W.ps("Get-Partition -DiskNumber %d -ErrorAction SilentlyContinue | Where-Object { $_.DriveLetter } | "
                   "ForEach-Object { $_.DriveLetter }" % n, check=False)
        for line in out.splitlines():
            line = line.strip()
            if len(line) == 1 and line.isalpha() and os.path.exists(line + ":\\"):
                return line + ":"
        time.sleep(1)
    return None


def give_letter(n):
    c = W.free_letter()
    W.ps("Get-Partition -DiskNumber %d | Where-Object { $_.GptType -eq '{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}' } | "
         "Set-Partition -NewDriveLetter %s" % (n, c), check=False)
    return letter_of(n, 20)


def changed(a, b, size):
    """Where two VHDs' disks differ (the 512-byte VHD footer left out): [(start, end)]."""
    out = []
    with open(a, "rb") as fa, open(b, "rb") as fb:
        pos = 0
        while pos < size:
            n = min(1 << 20, size - pos)
            x, y = fa.read(n), fb.read(n)
            if x != y:
                for k in range(0, n, SECTOR):
                    if x[k:k + SECTOR] != y[k:k + SECTOR]:
                        s = pos + k
                        if out and out[-1][1] == s:
                            out[-1][1] = s + SECTOR
                        else:
                            out.append([s, s + SECTOR])
            pos += n
    return out


def what_is(off, t):
    v = t["volume"]
    vo, cs = v["offset"], v["cluster_size"]
    heap = vo + v["heap_offset"]
    if off < 34 * SECTOR:
        return "the partition table"
    if off >= t["gpt_disk_size"] - 33 * SECTOR:
        return "the backup partition table"
    if vo <= off < vo + 12 * SECTOR:
        return "the boot sector (volume flags)" if off == vo else "the boot region"
    if vo + 12 * SECTOR <= off < vo + 24 * SECTOR:
        return "the backup boot region"
    if vo + v["fat_offset"] <= off < vo + v["fat_offset"] + v["fat_length"]:
        return "the FAT"
    if off >= heap:
        c = (off - heap) // cs + 2
        for name in ("bitmap", "upcase", "root"):
            for first, k in v[name]:
                if first <= c < first + k:
                    return {"bitmap": "the allocation bitmap", "upcase": "the up-case table",
                            "root": "the top folder's records"}[name]
        for n in t["nodes"]:
            for first, k in n.get("runs") or []:
                if first <= c < first + k:
                    return "%s %s" % ("the records of folder" if n["is_dir"] else "the data of", n["path"])
        return "free space (cluster %d)" % c
    return "unused space"


def try_write(letter):
    try:
        with open(letter + "\\written-by-the-test.txt", "w") as f:
            f.write("this must not be possible")
        return None
    except OSError as e:
        return e


def make_vhd(work, name, seed):
    disk = T.basic_disk(4096, seed=seed)
    t = disk.truth()
    size_mib = -(-t["disk_size"] // X.MiB)
    img = os.path.join(work, name + ".img")
    disk.save(img)
    vhd = os.path.join(work, name + ".vhd")
    W.make_container(vhd, size_mib)
    W.put_image(vhd, img)
    os.remove(img)
    keep = os.path.join(work, name + "-original.vhd")
    shutil.copyfile(vhd, keep)
    return disk, t, vhd, keep, size_mib * X.MiB


def report_changes(label, diffs, t):
    if not diffs:
        log("     %s: not one byte changed" % label)
        return
    places = {}
    for s, e in diffs:
        places.setdefault(what_is(s, t), 0)
        places[what_is(s, t)] += e - s
    log("     %s: %d places changed: %s" % (label, len(diffs), "; ".join(
        "%s (%d bytes)" % (k, n) for k, n in sorted(places.items()))))


def main():
    exe = os.path.abspath(sys.argv[1])
    work = os.path.abspath(sys.argv[2])
    shutil.rmtree(work, ignore_errors=True)
    os.makedirs(work)
    code, text = rp(exe, "disk-guard", "status")
    log("Before:\n" + "\n".join("     " + x for x in text.splitlines()))
    start_policy = [x for x in text.splitlines() if x.startswith("SAN policy:")]

    # A. Unprotected.
    log("A. A disk Windows is simply given:")
    disk, t, vhd, keep, size = make_vhd(work, "a", 31)
    n = attach(vhd)
    try:
        letter = letter_of(n) or give_letter(n)
        log("     attached as disk %d, drive %s; %s" % (n, letter, disk_info(n)))
        time.sleep(20)
        if letter:
            log("     its top folder now holds: %s" % sorted(os.listdir(letter + "\\")))
    finally:
        detach(vhd)
    report_changes("after 20 seconds attached", changed(vhd, keep, size), t)

    # B. Protected.
    log("B. The same kind of disk, protected the way the front end does it:")
    disk, t, vhd, keep, size = make_vhd(work, "b", 32)
    code, text = rp(exe, "disk-guard", "prepare")
    check(code == 0, "prepare: %s" % text.replace("\n", " "))
    n = attach(vhd)
    try:
        time.sleep(5)
        info = disk_info(n)
        check(info.get("offline") is True, "the disk arrives switched off (Windows says %s)" % info)
        check(letter_of(n, 5) is None, "it has no drive letter")
        code, text = rp(exe, "disk-guard", "status")
        mine = [x for x in text.splitlines() if x.startswith("disk %d:" % n)]
        log("     the program sees: %s" % mine)
        # (What the program reads to know the setting is on now - not its own
        # note, which can outlive the setting: cli::new_disks_held_off.)
        check("SAN policy in the registry: Some(3)" in text and "Held off now: true" in text,
              "Windows' own record of the setting says new disks stay switched off (%s)" %
              [x for x in text.splitlines() if "registry" in x or "Held off" in x])
        mp = os.path.join(work, "b.map")
        X.write_mapfile(mp, t["gpt_disk_size"], [])
        code, text = rp(exe, "map-check", "--map", mp, "--disk", "\\\\.\\PhysicalDrive%d" % n,
                        "--out", os.path.join(work, "b-map-check"))
        check(code == 0 and "folders and" in text,
              "the map check reads the disk while it is switched off%s" % ("" if code == 0 else ": " + text[-300:]))
        code, text = rp(exe, "disk-guard", "protect", str(n), "--online")
        check(code == 0 and "read-only true" in text, "protect --online: %s" % text)
        info = disk_info(n)
        check(info.get("read_only") is True and info.get("offline") is False,
              "Windows now says the disk is read-only and switched on (%s)" % info)
        letter = letter_of(n) or give_letter(n)
        check(letter is not None, "Windows mounts it (drive %s)" % letter)
        if letter:
            err = try_write(letter)
            check(err is not None, "writing a file to it fails (%s)" % err)
            names = sorted(os.listdir(letter + "\\"))
            check("Photos" in names, "its files can be read: %s" % names[:6])
            want = [x for x in disk.vol.nodes() if x.path == "notes.txt"][0]
            with open(letter + "\\notes.txt", "rb") as f:
                check(f.read() == want.data, "a file reads back exactly")
            time.sleep(10)
    finally:
        detach(vhd)
    diffs = changed(vhd, keep, size)
    report_changes("after all that", diffs, t)
    check(not diffs, "the protected disk came back unchanged")
    code, text = rp(exe, "disk-guard", "restore")
    check(code == 0, "restore: %s" % text)
    code, text = rp(exe, "disk-guard", "status")
    check([x for x in text.splitlines() if x.startswith("SAN policy:")] == start_policy,
          "Windows' setting is back as it was (%s)" % start_policy)

    # C. Attached again later. Whether Windows remembers the read-only mark
    #    is logged, not judged: for virtual disks it does not, so the person
    #    is told to use "Get Windows ready" every time the SSD is plugged in.
    log("C. The protected disk attached again, with Windows' setting back to normal:")
    n = attach(vhd)
    try:
        time.sleep(5)
        info = disk_info(n)
        log("     Windows %s it read-only (%s)" % ("remembers" if info.get("read_only") else "does not remember", info))
    finally:
        detach(vhd)

    # D. Plugged in before Windows was ready.
    log("D. A disk plugged in before Windows was ready, protected after:")
    disk, t, vhd, keep, size = make_vhd(work, "d", 34)
    n = attach(vhd)
    try:
        letter = letter_of(n) or give_letter(n)
        log("     attached as disk %d, drive %s; %s" % (n, letter, disk_info(n)))
        code, text = rp(exe, "disk-guard", "protect", str(n))
        check(code == 0 and "read-only true" in text, "protect: %s" % text)
        # Switch it off and on so Windows mounts it again, read-only this time.
        c1, t1 = rp(exe, "disk-guard", "offline", str(n))
        c2, t2 = rp(exe, "disk-guard", "online", str(n))
        check(c1 == 0 and c2 == 0, "switched off and on again: %s / %s" % (t1, t2))
        letter = letter_of(n) or give_letter(n)
        if letter:
            err = try_write(letter)
            check(err is not None, "writing a file to it fails (%s)" % err)
        time.sleep(10)
    finally:
        detach(vhd)
    report_changes("in all", changed(vhd, keep, size), t)

    # E. Windows' own disk.
    log("E. Windows' own disk is never touched:")
    sysdisk = int(W.ps("(Get-Partition -DriveLetter $env:SystemDrive.Substring(0,1)).DiskNumber"))
    before = disk_info(sysdisk)
    for args in (["protect", str(sysdisk)], ["offline", str(sysdisk)], ["protect", str(sysdisk), "--online"]):
        code, text = rp(exe, "disk-guard", *args)
        check(code != 0 and "Windows itself" in text, "disk-guard %s on Windows' own disk is refused: %s" % (
            " ".join(args), text))
    check(disk_info(sysdisk) == before, "Windows' own disk is as it was (%s)" % disk_info(sysdisk))

    code, text = rp(exe, "disk-guard", "status")
    log("After:\n" + "\n".join("     " + x for x in text.splitlines()))
    bad = [w for ok, w in RESULTS if not ok]
    log("Disk protection checks: %d of %d passed" % (len(RESULTS) - len(bad), len(RESULTS)))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
