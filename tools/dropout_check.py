#!/usr/bin/env python3
"""The recovered SSD coming unplugged in the middle of being copied (run on
GitHub's Windows machines, as administrator), through the step-by-step page
the way a person uses it:

  A. A test disk with many files arrives switched off ("Get Windows ready"),
     is protected, and copying starts. Windows stays set to leave new disks
     switched off the whole time the disk is plugged in, and the page refuses
     to put the setting back until it is unplugged.
  B. Part-way, the disk is unplugged (the virtual disk is detached): the copy
     must stop, saying the SSD stopped answering, with nothing listed as
     failed - and Windows must still be set to leave new disks switched off.
  C. Plugged in again, the disk arrives switched off. Protected again, the
     copy starts again and must finish with exactly the copies and the list
     one straight run gives; Windows' setting stays until it is unplugged,
     and is put back after that.
  D. A drive that is not the recovered SSD (NTFS, like SPARE), plugged in
     while new disks are left switched off: the page switches it on,
     writable. It refuses to do that to the recovered SSD, and refuses to
     switch that on unless it is read-only.

    python dropout_check.py <recovery-probe.exe> <work folder>
"""

import hashlib
import json
import os
import random
import shutil
import subprocess
import sys
import time
import urllib.parse
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import exfat_image as X  # noqa: E402
import windows_check as W  # noqa: E402

RESULTS = []


def log(msg):
    print(msg, flush=True)


def check(ok, what):
    RESULTS.append((bool(ok), what))
    log("%s %s" % ("ok  " if ok else "FAIL", what))


class Page:
    def __init__(self, exe, home, port):
        self.key = "dropoutcheck%08d" % random.randrange(10 ** 8)
        self.url = "http://127.0.0.1:%d/%s/" % (port, self.key)
        self.out = open(os.path.join(home, "server-output.txt"), "w")
        self.proc = subprocess.Popen([exe, "ui", "--port", str(port), "--token", self.key, "--no-browser",
                                      "--home", home], stdout=self.out, stderr=subprocess.STDOUT)
        for _ in range(100):
            try:
                self.get("api/state")
                return
            except OSError:
                time.sleep(0.1)
        raise RuntimeError("the page did not start")

    def get(self, path):
        with urllib.request.urlopen(self.url + path, timeout=30) as r:
            return json.loads(r.read().decode("utf-8"))

    def act(self, **fields):
        body = urllib.parse.urlencode(fields).encode()
        req = urllib.request.Request(self.url + "api/do", data=body, method="POST",
                                     headers={"X-Key": self.key, "Content-Type": "application/x-www-form-urlencoded"})
        with urllib.request.urlopen(req, timeout=120) as r:
            return json.loads(r.read().decode("utf-8"))

    def wait_job(self, timeout=900):
        t0 = time.time()
        while time.time() - t0 < timeout:
            s = self.get("api/state")
            j = s.get("job")
            if j and j.get("state") != "running":
                return j
            time.sleep(0.5)
        raise RuntimeError("the job did not end")

    def close(self):
        self.proc.kill()
        self.proc.wait()
        self.out.close()


def policy(exe):
    r = subprocess.run([exe, "disk-guard", "status"], stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    for line in r.stdout.decode("utf-8", "replace").splitlines():
        if line.startswith("SAN policy:"):
            return line.split(":", 1)[1].split("(")[0].strip()
    return "?"


def attach(vhd):
    W.ps("Mount-DiskImage -ImagePath '%s' -StorageType VHD -Access ReadWrite | Out-Null" % vhd)
    for _ in range(50):
        out = W.ps("(Get-DiskImage -ImagePath '%s').Number" % vhd, check=False).strip()
        if out.isdigit():
            return int(out)
        time.sleep(0.2)
    raise RuntimeError("the virtual disk has no number")


def detach(vhd):
    return W.ps("Dismount-DiskImage -ImagePath '%s' | Out-Null" % vhd, check=False)


def disk_info(n):
    out = W.ps("Get-Disk -Number %d | ForEach-Object { \"$($_.IsOffline) $($_.IsReadOnly) $($_.OfflineReason)\" }" % n,
               check=False)
    p = out.split()
    if len(p) < 2:
        return {"raw": out}
    return {"offline": p[0].lower() == "true", "read_only": p[1].lower() == "true", "reason": " ".join(p[2:])}


def tree(top):
    """{relative path: (size, sha1)} for every file, plus the rescue list's rows."""
    files = {}
    for base, _, names in os.walk(top):
        for x in names:
            p = os.path.join(base, x)
            rel = os.path.relpath(p, top)
            if rel.startswith("rescue-log"):
                continue
            with open(p, "rb") as f:
                files[rel] = (os.path.getsize(p), hashlib.sha1(f.read()).hexdigest())
    import csv
    with open(os.path.join(top, "rescue-log.csv"), encoding="utf-8-sig", newline="") as f:
        rows = sorted(tuple(r) for r in csv.reader(f))
    return files, rows


def make_ntfs_vhd(path):
    if os.path.exists(path):
        os.remove(path)
    W.diskpart(['create vdisk file="%s" maximum=64 type=fixed' % path, 'select vdisk file="%s"' % path,
                "attach vdisk", "convert gpt", "create partition primary", 'format fs=ntfs quick label="SPARE"',
                "detach vdisk"])


def main():
    exe = os.path.abspath(sys.argv[1])
    work = os.path.abspath(sys.argv[2])
    shutil.rmtree(work, ignore_errors=True)
    os.makedirs(work)
    here = os.path.dirname(os.path.abspath(__file__))
    log("Policy at the start: %s" % policy(exe))
    start_policy = policy(exe)

    # The disk: 300 folders of 300 small files.
    img = os.path.join(work, "big.img")
    mp = os.path.join(work, "big.map")
    subprocess.run([sys.executable, os.path.join(here, "make_big_disk.py"), img, mp, "300", "300"], check=True)
    size = os.path.getsize(img)
    vhd = os.path.join(work, "big.vhd")
    W.make_container(vhd, size // X.MiB)
    W.put_image(vhd, img)

    # What one straight run copies, from the image file itself.
    ref = os.path.join(work, "reference")
    r = subprocess.run([exe, "rescue", "--disk", img, "--out", ref, "--everything", "--lost"],
                       stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    check(r.returncode == 0, "one straight copy of the image file")
    want = tree(ref)
    log("     %d files in the straight copy" % len(want[0]))

    # SPARE, made now: while Windows leaves new disks switched off, a new
    # disk also arrives read-only, so it could not be formatted then.
    spare = os.path.join(work, "spare.vhd")
    make_ntfs_vhd(spare)

    home = os.path.join(work, "home")
    os.makedirs(home)
    shutil.copyfile(mp, os.path.join(home, "big.map"))
    page = Page(exe, home, 47000 + random.randrange(900))
    vhd_in = False
    try:
        page.act(action="set", mapfile=os.path.join(home, "big.map"))

        # A.
        log("A. Plugged in after 'Get Windows ready', protected, copying:")
        j = page.act(action="guard-prepare")
        check(j.get("ok"), "Get Windows ready: %s" % j)
        n = attach(vhd)
        vhd_in = True
        time.sleep(3)
        info = disk_info(n)
        check(info.get("offline") is True, "the disk arrives switched off (%s)" % info)
        spec = "\\\\.\\PhysicalDrive%d" % n
        disks = page.get("api/disks")
        me = [d for d in disks if d.get("spec") == spec]
        check(me and (me[0].get("detail") or {}).get("matches_mapfile") is True,
              "the page recognises it as the copy ddrescue made")
        j = page.act(action="guard-protect", disk=spec, online="0")
        check(j.get("ok"), "Protect: %s" % j)
        check(policy(exe) == "OfflineAll", "Windows stays set to leave new disks switched off after Protect")
        j = page.act(action="guard-restore")
        check(not j.get("ok"), "putting the setting back is refused while the SSD is plugged in: %s" % j.get("error"))
        check(policy(exe) == "OfflineAll", "the setting is still to leave new disks switched off")
        copies = os.path.join(work, "copies")
        j = page.act(action="rescue", disk=spec, out=copies, everything="1", lost="1")
        check(j.get("ok"), "copying starts: %s" % j)
        seen_policy = None
        t0 = time.time()
        while time.time() - t0 < 600:
            s = page.get("api/state")
            job = s.get("job") or {}
            if job.get("state") != "running":
                break
            if job.get("total") and job.get("done", 0) >= job["total"] // 5:
                seen_policy = policy(exe)
                break
            time.sleep(0.05)
        check(seen_policy == "OfflineAll", "while it copies, Windows leaves new disks switched off (%s)" % seen_policy)

        # B.
        log("B. Unplugged part-way:")
        out = detach(vhd)
        vhd_in = False
        log("     detached (%s)" % (out or "no message"))
        job = page.wait_job()
        summary = " ".join(job.get("summary") or [])
        data = job.get("data") or {}
        log("     the page says: %s" % summary[:400])
        check(job.get("state") == "stopped", "the copy stopped (%s)" % job.get("state"))
        check("stopped answering" in summary, "it says the SSD stopped answering")
        check(data.get("failed") == 0, "nothing is listed as failed (%s)" % data.get("failed"))
        check(policy(exe) == "OfflineAll", "Windows still leaves new disks switched off")
        check(os.path.exists(os.path.join(copies, "rescue-log.csv")), "the list so far is there")

        # C.
        log("C. Plugged in again, protected, copying again:")
        n = attach(vhd)
        vhd_in = True
        time.sleep(3)
        info = disk_info(n)
        check(info.get("offline") is True, "it arrives switched off (%s)" % info)
        spec = "\\\\.\\PhysicalDrive%d" % n
        j = page.act(action="guard-protect", disk=spec, online="0")
        check(j.get("ok"), "Protect: %s" % j)
        j = page.act(action="rescue", disk=spec, out=copies, everything="1", lost="1")
        check(j.get("ok"), "copying starts again: %s" % j)
        job = page.wait_job()
        data = job.get("data") or {}
        log("     the page says: %s" % " ".join(job.get("summary") or [])[:300])
        check(job.get("state") == "done", "the copy finished (%s)" % job.get("state"))
        check(data.get("failed") == 0 and data.get("already_there", 0) > 0,
              "nothing failed; what was copied before was kept (%s)" % data)
        check(policy(exe) == "OfflineAll", "with the SSD still plugged in, Windows still leaves new disks switched off")
        got = tree(copies)
        check(got[0] == want[0], "the copies are exactly those of one straight run (%d / %d files)" % (
            len(got[0]), len(want[0])))
        check(got[1] == want[1], "the list is exactly that of one straight run (%d / %d lines)" % (
            len(got[1]), len(want[1])))

        # D.
        log("D. A drive that is not the recovered SSD, plugged in while new disks are left switched off:")
        j = page.act(action="guard-prepare")
        check(j.get("ok"), "Get Windows ready again")
        m = attach(spare)
        time.sleep(3)
        info = disk_info(m)
        log("     SPARE arrived: %s" % info)
        j = page.act(action="guard-release", disk="\\\\.\\PhysicalDrive%d" % m)
        check(j.get("ok"), "the page switches it on, writable: %s" % j)
        info = disk_info(m)
        check(info.get("offline") is False and info.get("read_only") is False, "Windows agrees (%s)" % info)
        letter = None
        for _ in range(30):
            o = W.ps("Get-Partition -DiskNumber %d | Where-Object { $_.DriveLetter } | ForEach-Object { $_.DriveLetter }"
                     % m, check=False).strip()
            if len(o) == 1:
                letter = o + ":"
                break
            if _ == 5:
                W.ps("Get-Partition -DiskNumber %d | Where-Object { $_.Type -eq 'Basic' } | Set-Partition -NewDriveLetter %s"
                     % (m, W.free_letter()), check=False)
            time.sleep(1)
        if letter:
            try:
                with open(letter + "\\written.txt", "w") as f:
                    f.write("ok")
                check(True, "a file can be written to it (%s)" % letter)
            except OSError as e:
                check(False, "a file can be written to it: %s" % e)
        else:
            check(False, "it gets a drive letter")
        W.ps("Dismount-DiskImage -ImagePath '%s' | Out-Null" % spare, check=False)
        j = page.act(action="guard-release", disk=spec)
        check(not j.get("ok"), "it refuses to do that to the recovered SSD: %s" % j.get("error"))
        info = disk_info(n)
        check(info.get("read_only") is True, "the recovered SSD is still read-only (%s)" % info)
        detach(vhd)
        vhd_in = False
        time.sleep(2)
        j = page.act(action="guard-restore")
        check(j.get("ok"), "once the SSD is unplugged, the setting is put back: %s" % j)
        check(policy(exe) == start_policy, "Windows' setting is back (%s)" % policy(exe))
    finally:
        if vhd_in:
            detach(vhd)
        page.close()
        if policy(exe) != start_policy:
            subprocess.run([exe, "disk-guard", "restore"])
    bad = [w for ok, w in RESULTS if not ok]
    log("Unplugged mid-copy checks: %d of %d passed" % (len(RESULTS) - len(bad), len(RESULTS)))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
