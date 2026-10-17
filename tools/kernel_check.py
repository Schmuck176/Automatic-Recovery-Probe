#!/usr/bin/env python3
"""Checks the test disks against the Linux kernel's own exFAT driver.

Runs on GitHub's Ubuntu machines (needs sudo, losetup and the exfat module):

1. Every undamaged test disk is attached as a loop device: the kernel must
   find the same partitions in its partition table, mount the exFAT volume,
   and read every file back exactly as written. This proves the test disks
   are real exFAT disks, not something only this project's code understands.

2. On the damaged disks, the folders Linux lists must hold exactly the files
   the map check says can still be listed: Linux, like Windows, stops
   reading a folder at its first blank record and skips damaged records.

    sudo python3 kernel_check.py <work folder>
"""

import os
import shutil
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import exfat_image as X  # noqa: E402
import test_mapcheck as T  # noqa: E402


def sh(*cmd):
    return subprocess.run(cmd, check=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT).stdout.decode().strip()


def listing(mnt):
    """Every file and folder Linux lists, as exFAT paths, with file contents."""
    out = {}
    for root, dirs, files in os.walk(mnt):
        rel = os.path.relpath(root, mnt)
        rel = "" if rel == "." else rel.replace("/", "\\")
        for d in dirs:
            out[(rel + "\\" if rel else "") + d] = None
        for f in files:
            p = os.path.join(root, f)
            try:
                with open(p, "rb") as fh:
                    data = fh.read()
            except OSError as e:
                data = e
            out[(rel + "\\" if rel else "") + f] = data
    return out


def attach(img, truth):
    """Attach with the kernel reading the partition table; return (loop, partition device)."""
    loop = sh("losetup", "--find", "--show", "--read-only", "--partscan", img)
    name = os.path.basename(loop)
    want = truth["partitions"]
    found = []
    for k in range(1, 9):
        part = "/sys/block/%s/%sp%d" % (name, name, k)
        if os.path.exists(part):
            with open(part + "/start") as f:
                start = int(f.read()) * 512
            with open(part + "/size") as f:
                size = int(f.read()) * 512
            found.append((start, start + size, "%sp%d" % (loop, k)))
    problems = []
    if [(s, e) for s, e, _ in found] != [(p["start"], p["end"]) for p in want]:
        problems.append("the kernel reads the partition table as %s, expected %s" % (
            [(s, e) for s, e, _ in found], [(p["start"], p["end"]) for p in want]))
    dev = [d for s, e, d in found if s == truth["volume"]["offset"]]
    return loop, (dev[0] if dev else None), problems


def check_case(work, case):
    d = os.path.join(work, case.name)
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    pristine = os.path.join(d, "pristine.img")
    img = os.path.join(d, "disk.img")
    case.disk.save(pristine)
    truth = case.disk.truth()
    shutil.copyfile(pristine, img)
    zero = case.areas if case.zero is None else case.zero
    X.zero_areas(img, [(s, n) for s, n, *_ in zero])
    mnt = os.path.join(d, "mnt")
    os.makedirs(mnt)
    problems = []

    # 1. The undamaged disk, as written.
    loop, dev, p = attach(pristine, truth)
    problems += p
    try:
        if dev:
            sh("mount", "-t", "exfat", "-o", "ro", dev, mnt)
            try:
                got = listing(mnt)
            finally:
                sh("umount", mnt)
            want = {n.path: n for n in case.disk.vol.nodes() if n is not case.disk.vol.root and not n.deleted}
            if set(got) != set(want):
                problems.append("Linux lists different files: only Linux: %s; only the builder: %s" % (
                    sorted(set(got) - set(want))[:4], sorted(set(want) - set(got))[:4]))
            for path, n in want.items():
                if not n.is_dir and path in got:
                    expect = n.data[:n.vdl] + bytes(n.data_length - n.vdl)
                    if got[path] != expect:
                        problems.append("Linux reads %r differently" % path)
        else:
            problems.append("the kernel found no partition at the exFAT volume's offset")
    finally:
        sh("losetup", "-d", loop)

    # 2. The damaged disk: what can Linux still list?
    oracle = T.Oracle(truth, img, pristine, [(s, s + n, st) for s, n, st in case.areas])
    exp = oracle.expected()
    listable = set(exp["found"]) - set(exp["hidden"])
    loop, dev, _ = attach(img, truth)
    try:
        if dev:
            try:
                sh("mount", "-t", "exfat", "-o", "ro", dev, mnt)
            except subprocess.CalledProcessError as e:
                problems.append("Linux would not mount the damaged disk: %s" % e.stdout.decode()[-300:])
                return problems, None
            try:
                got = listing(mnt)
            finally:
                sh("umount", mnt)
            if set(got) != listable:
                problems.append("on the damaged disk Linux lists %d files and folders, the map check says %d can be "
                                "listed; only Linux: %s; only the map check: %s" % (
                                    len(got), len(listable), sorted(set(got) - listable)[:4],
                                    sorted(listable - set(got))[:4]))
            hidden = len(exp["hidden"])
    finally:
        sh("losetup", "-d", loop)
    shutil.rmtree(d, ignore_errors=True)
    return problems, hidden


def keep_mountable(case):
    """Linux reads only the main boot sector and needs the partition table,
    the allocation bitmap, the up-case table and the top folder's first
    records to mount at all; leave those out of the damage (the map check's
    own tests cover them)."""
    t = case.disk.truth()
    v = t["volume"]
    cs = v["cluster_size"]
    heap = v["offset"] + v["heap_offset"]
    keep_out = [(0, 34 * 512), (v["offset"], v["offset"] + 24 * 512)]
    for name in ("bitmap", "upcase"):
        for first, n in v[name]:
            keep_out.append((heap + (first - 2) * cs, heap + (first - 2 + n) * cs))
    root_first = v["root"][0][0]
    keep_out.append((heap + (root_first - 2) * cs, heap + (root_first - 2) * cs + 512))
    ok = lambda a: not any(a[0] < e and a[0] + a[1] > s for s, e in keep_out)
    case.areas = [a for a in case.areas if ok(a)]
    if case.zero is not None:
        case.zero = [a for a in case.zero if ok(a)]


def main():
    work = sys.argv[1]
    os.makedirs(work, exist_ok=True)
    cases = []
    for cs in (512, 4096, 32768, 131072):
        for c in T.deliberate_cases(cs):
            c.flips = []  # Linux does not check record checksums; Windows is tested for that separately
            cases.append(c)
    cases += [T.hidden_folder_case(), T.fat_case(), T.bigger_ssd_case()]
    cases += [T.random_case(1000 + i) for i in range(int(sys.argv[2]) if len(sys.argv) > 2 else 12)]
    for c in cases:
        c.garbage = []  # zeros only: what unread sectors look like on a new SSD
        keep_mountable(c)
    failed = 0
    total_hidden = 0
    for c in cases:
        problems, hidden = check_case(work, c)
        total_hidden += hidden or 0
        if problems:
            failed += 1
            print("FAIL %s" % c.name)
            for p in problems[:8]:
                print("     " + p)
        else:
            print("ok   %s (%d files and folders hidden by the damage)" % (c.name, hidden or 0))
    print("Linux kernel exFAT checks: %d of %d disks agree (%d hidden files and folders confirmed)" % (
        len(cases) - failed, len(cases), total_hidden))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
