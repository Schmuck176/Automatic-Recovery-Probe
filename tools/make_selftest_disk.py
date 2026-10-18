#!/usr/bin/env python3
"""Makes the small damaged test disk built into the program for its
self-test (src/selftest/): a GPT disk with an exFAT volume like the user's,
damaged the way a ddrescue copy is, with the right answers worked out by
test_mapcheck.py's oracle.

    python tools/make_selftest_disk.py      (writes src/selftest/*)

Files written:
  disk.bin       the damaged disk, sparse: only the 512-byte sectors that
                 are not all zeros ("RPSPARSE1", disk size, then runs of
                 <offset u64><sector count u32><bytes>)
  disk.map       the mapfile naming the damaged places
  expected.tsv   what the map check and the rescue must find
"""

import os
import random
import struct
import sys
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import exfat_image as X  # noqa: E402
import test_mapcheck as T  # noqa: E402

SECTOR = 512
OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src", "selftest")


def build():
    rng = random.Random(2026)
    v = X.Volume(12 * X.MiB, cluster_size=4096, label="FINAL 1 TB", serial=0x5E1F7E57)
    fid = 1

    def add(path, size, **kw):
        nonlocal fid
        v.add_file(path, X.tagged(fid, size, 77), **kw)
        fid += 1

    v.mkdir("Photos")
    for i in range(12):
        add("Photos/IMG_%04d.JPG" % i, rng.randint(9_000, 30_000))
    v.mkdir("Photos/Summer 2019 ")  # ends in a space: Windows cannot use that name
    for i in range(4):
        add("Photos/Summer 2019 /beach %d.jpg" % i, rng.randint(3_000, 9_000))
    v.mkdir("家族の写真")
    for i in range(3):
        add("家族の写真/写真 %d.jpg" % i, rng.randint(2_000, 6_000))
    v.mkdir("Documents")
    for i in range(20):
        add("Documents/letter %02d.docx" % i, rng.randint(1_000, 5_000))
    v.mkdir("Big folder", extra_clusters=1)
    for i in range(60):
        add("Big folder/file number %02d with a longer name.bin" % i, rng.randint(100, 2_000))
    v.mkdir("Big folder/zz a folder after the damage")
    for i in range(2):
        add("Big folder/zz a folder after the damage/inside %d.txt" % i, rng.randint(100, 900))
    v.mkdir("Archive")
    add("Archive/before.txt", 700)
    v.mkdir("Archive/inner")
    for i in range(3):
        add("Archive/inner/old %d.pdf" % i, rng.randint(1_000, 4_000))
    add("Archive/after.txt", 800)
    v.mkdir("Music")
    add("Music/song.flac", 40_000, fragments=[4, 3, 3], gap=2)
    add("Music/clip.mov", 20_000, fat_chain=True)
    add("notes.txt", 600)
    disk = X.Disk(v, 16 * X.MiB, efi_start_lba=40, efi_size=1 * X.MiB, vol_start_lba=40 + 2048 + 7, seed=2026)
    return disk


def main():
    disk = build()
    t = disk.truth()
    node = {n["path"]: n for n in t["nodes"]}
    areas = []  # (start, length, status)
    zero = []
    garbage = []
    flips = []
    # 1. Two sectors inside a photo's data.
    p = node["Photos\\IMG_0003.JPG"]
    s = T.disk_pos(t, p, 8192)
    areas.append((s, 2 * SECTOR, "-"))
    zero.append((s, 2 * SECTOR))
    # 2. A blank sector in the middle of a big folder's records.
    r = node["Big folder\\file number 30 with a longer name.bin"]["record_offsets"][0] // SECTOR * SECTOR
    areas.append((r, SECTOR, "-"))
    zero.append((r, SECTOR))
    # 3. One letter of a name in Documents changed, so that record's checksum
    #    is wrong: Windows then reports the whole folder as corrupted.
    r = node["Documents\\letter 08.docx"]["record_offsets"][2] + 2 + 2 * 3
    areas.append((r // SECTOR * SECTOR, SECTOR, "-"))
    flips.append(r)
    # 3b. Leftover data (random bytes) over some of Photos' records: Windows
    #     skips the junk, so only the files whose records it covers are gone.
    r = node["Photos\\IMG_0007.JPG"]["record_offsets"][0] // SECTOR * SECTOR
    areas.append((r, SECTOR, "-"))
    garbage.append((r, SECTOR))
    # 4. The folder record of Archive\inner: the folder and what is in it.
    r = node["Archive\\inner"]["record_offsets"][0] // SECTOR * SECTOR
    areas.append((r, SECTOR, "/"))
    zero.append((r, SECTOR))
    # 5. Free space near the end of the volume.
    vv = t["volume"]
    free = vv["offset"] + vv["heap_offset"] + (vv["cluster_count"] - 3) * vv["cluster_size"]
    areas.append((free, 4 * SECTOR, "-"))
    zero.append((free, 4 * SECTOR))
    areas.sort()

    os.makedirs(OUT, exist_ok=True)
    work = os.path.join(OUT, "..", "..", "target", "selftest-build")
    os.makedirs(work, exist_ok=True)
    pristine = os.path.join(work, "pristine.img")
    img = os.path.join(work, "disk.img")
    disk.save(pristine)
    with open(pristine, "rb") as f:
        data = bytearray(f.read())
    for s, n in zero:
        data[s:s + n] = bytes(n)
    g = random.Random(5)
    for s, n in garbage:
        data[s:s + n] = g.randbytes(n)
    for off in flips:
        data[off] ^= 0x20
    with open(img, "wb") as f:
        f.write(data)
    mp = os.path.join(OUT, "disk.map")
    X.write_mapfile(mp, t["gpt_disk_size"], areas)

    oracle = T.Oracle(t, img, pristine, [(s, s + n, st) for s, n, st in areas])
    try:
        exp = oracle.expected()
        lines = ["DISK\t%d" % len(data)]
        for path, it in sorted(exp["items"].items()):
            if not path:
                continue
            dmg = sum(e - s for s, e, _ in it["ranges"])
            lines.append("ITEM\t%s\t%d\t%d\t%s" % (path, 1 if it["is_dir"] else 0, dmg,
                                                  ";".join("%d-%d" % (s, e) for s, e, _ in it["ranges"])))
        maybe = set(exp["hidden_maybe"])
        for path in exp["hidden"]:
            lines.append("HIDDEN\t%s\t%d" % (path, 1 if path in maybe else 0))
        for path in exp["found"]:
            n = oracle.found_nodes[path][0]
            if n["is_dir"]:
                lines.append("FOUND\t%s\t1\t0\t0" % path)
            else:
                c = oracle.content(path)
                lines.append("FOUND\t%s\t0\t%d\t%d" % (path, len(c), zlib.crc32(c)))
        lines.append("LOST\t%d" % len(oracle.lost_runs))
        lines.append("PROBLEMS\t%d" % exp["folder_problems"])
    finally:
        oracle.close()
    with open(os.path.join(OUT, "expected.tsv"), "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")

    # The disk, sparse.
    runs = []
    k = 0
    n_sec = len(data) // SECTOR
    while k < n_sec:
        if data[k * SECTOR:(k + 1) * SECTOR] == bytes(SECTOR):
            k += 1
            continue
        j = k
        while j < n_sec and data[j * SECTOR:(j + 1) * SECTOR] != bytes(SECTOR):
            j += 1
        runs.append((k, j - k))
        k = j
    with open(os.path.join(OUT, "disk.bin"), "wb") as f:
        f.write(b"RPSPARSE1" + struct.pack("<QI", len(data), len(runs)))
        for k, c in runs:
            f.write(struct.pack("<QI", k * SECTOR, c))
            f.write(bytes(data[k * SECTOR:(k + c) * SECTOR]))
    print("wrote %s: %d bytes of disk in %d runs (%d bytes); %d expected lines" % (
        OUT, len(data), len(runs), os.path.getsize(os.path.join(OUT, "disk.bin")), len(lines)))
    print("hidden: %d (by path %d), damaged items: %d, found: %d, lost runs: %d, folder problems: %d" % (
        len(exp["hidden"]), len(maybe), len([p for p in exp["items"] if p]), len(exp["found"]),
        len(oracle.lost_runs), exp["folder_problems"]))


if __name__ == "__main__":
    main()
