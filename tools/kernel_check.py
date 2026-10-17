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
import stat
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import exfat_image as X  # noqa: E402
import test_mapcheck as T  # noqa: E402


def sh(*cmd):
    return subprocess.run(cmd, check=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT).stdout.decode().strip()


def dmesg_tail():
    r = subprocess.run(["dmesg"], stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    lines = [x for x in r.stdout.decode("utf-8", "replace").splitlines() if "exfat" in x.lower() or "loop" in x]
    return " / ".join(lines[-3:])


def try_mount(dev, mnt):
    """Mount read-only; return None, or why it failed."""
    r = subprocess.run(["mount", "-t", "exfat", "-o", "ro", dev, mnt], stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if r.returncode == 0:
        return None
    return "%s (%s)" % (r.stdout.decode("utf-8", "replace").strip(), dmesg_tail())


def wait_for(dev):
    """The partition's device file appears a moment after the kernel reads
    the partition table; make it ourselves if udev is slow."""
    subprocess.run(["udevadm", "settle"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(50):
        if os.path.exists(dev):
            return
        time.sleep(0.1)
    name = os.path.basename(dev)
    base = name.split("p")[0]
    with open("/sys/block/%s/%s/dev" % (base, name)) as f:
        major, minor = (int(x) for x in f.read().split(":"))
    os.mknod(dev, stat.S_IFBLK | 0o600, os.makedev(major, minor))


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
    if dev:
        wait_for(dev[0])
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
        why = try_mount(dev, mnt) if dev else None
        if dev and why:
            problems.append("Linux will not mount the undamaged disk: %s" % why)
        elif dev:
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
    try:
        exp = oracle.expected()
    finally:
        oracle.close()
    listable = set(exp["found"]) - set(exp["hidden"])
    maybe = set(exp["hidden_maybe"])
    loop, dev, _ = attach(img, truth)
    try:
        if dev:
            why = try_mount(dev, mnt)
            if why:
                problems.append("Linux would not mount the damaged disk: %s" % why)
                return problems, None
            try:
                got = listing(mnt)
            finally:
                sh("umount", mnt)
            if not (listable <= set(got) <= listable | maybe):
                problems.append("on the damaged disk Linux lists %d files and folders, the map check says %d can be "
                                "listed; only Linux: %s; only the map check: %s" % (
                                    len(got), len(listable), sorted(set(got) - listable - maybe)[:4],
                                    sorted(listable - set(got))[:4]))
            hidden = len(exp["hidden"])
    finally:
        sh("losetup", "-d", loop)
    shutil.rmtree(d, ignore_errors=True)
    return problems, hidden


def experiments(work):
    """Which single pieces of damage stop Linux from mounting the drive."""
    disk, exps = T.mount_experiments()
    truth = disk.truth()
    d = os.path.join(work, "experiments")
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    pristine = os.path.join(d, "pristine.img")
    disk.save(pristine)
    mnt = os.path.join(d, "mnt")
    os.makedirs(mnt)
    for name, areas in exps:
        img = os.path.join(d, "disk.img")
        shutil.copyfile(pristine, img)
        X.zero_areas(img, areas)
        try:
            loop, dev, _ = attach(img, truth)
        except subprocess.CalledProcessError as e:
            print("     damage to %s: the disk cannot be attached (%s)" % (name, e.stdout.decode()[-200:]))
            continue
        try:
            if not dev:
                print("     damage to %s: Linux finds no partitions" % name)
                continue
            why = try_mount(dev, mnt)
            if why:
                print("     damage to %s: Linux will NOT mount the drive: %s" % (name, why))
            else:
                n = len(listing(mnt))
                sh("umount", mnt)
                print("     damage to %s: Linux mounts the drive and lists %d files and folders" % (name, n))
        finally:
            sh("losetup", "-d", loop)
    shutil.rmtree(d, ignore_errors=True)


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
        T.keep_mountable(c)
    print("What stops Linux from opening the drive:")
    experiments(work)
    failed = 0
    total_hidden = 0
    for c in cases:
        try:
            problems, hidden = check_case(work, c)
        except Exception as e:  # report it and carry on with the next disk
            problems, hidden = ["error: %r" % e], 0
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
