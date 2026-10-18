#!/usr/bin/env python3
"""A big exFAT test disk, for timing the program on a volume with as many
files as a real one: FOLDERS folders of FILES small files each (every file
one 4 KB cluster: small JPEGs, PNGs and text files), written straight into
a sparse image file without holding the image in memory.

    python make_big_disk.py big.img big.map 2000 500

The mapfile marks places that were "never read" (and the image holds zeros
there, as on a ddrescue copy): inside some files, inside one folder's
records (which hides the files after them from Windows and loses a few
records), and in free space. big.img.json says which.
"""

import io
import json
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import exfat_image as X  # noqa: E402

SECTOR = 512
CS = 4096
SPC = CS // SECTOR
FAT_OFFSET = 128  # sectors
MTIME = X.exfat_timestamp(2023, 7, 14, 10, 30, 0)


def png(w, h, rgb):
    """A small valid PNG, made without any imaging library."""
    import zlib

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    raw = b"".join(b"\x00" + bytes(rgb) * w for _ in range(h))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def contents():
    out = []
    for i, (w, h) in enumerate([(16, 16), (24, 12), (8, 30)]):
        rgb = (i * 60, 120, 200 - i * 50)
        out.append(("png", png(w, h, rgb)))
        try:
            from PIL import Image
            img = Image.new("RGB", (w, h), rgb)
            b = io.BytesIO()
            img.save(b, "JPEG", quality=80)
            out.append(("jpg", b.getvalue()))
        except ImportError:
            pass
    out.append(("txt", b"just some words\r\n" * 20))
    assert all(len(c) <= CS for _, c in out)
    return out


def entry_set(name, attrs, first, length):
    units = X.utf16_units(name)
    names = (len(units) + 14) // 15
    sc = 1 + names
    e = bytearray(32 * (sc + 1))
    e[0] = 0x85
    e[1] = sc
    struct.pack_into("<H", e, 4, attrs)
    struct.pack_into("<III", e, 8, MTIME, MTIME, MTIME)
    e[22] = e[23] = e[24] = 0x80
    e[32] = 0xC0
    e[33] = 3 if length else 0  # AllocationPossible, NoFatChain
    e[35] = len(units)
    struct.pack_into("<H", e, 36, X.name_hash(units))
    struct.pack_into("<Q", e, 40, length)
    struct.pack_into("<I", e, 52, first if length else 0)
    struct.pack_into("<Q", e, 56, length)
    for k in range(names):
        o = 64 + 32 * k
        e[o] = 0xC1
        part = units[15 * k:15 * k + 15]
        struct.pack_into("<%dH" % len(part), e, o + 2, *part)
    struct.pack_into("<H", e, 2, X.entry_set_checksum(e))
    return bytes(e)


class FakeVol:
    def __init__(self, size, label):
        self.size = size
        self.label = label


def main():
    img_path, map_path = sys.argv[1], sys.argv[2]
    folders, per = int(sys.argv[3]), int(sys.argv[4])
    kinds = contents()
    file_slots = 3  # names of up to 15 characters
    dir_clusters = -(-(per * file_slots * 32) // CS)
    root_bytes = (3 + folders * 3) * 32
    root_clusters = -(-root_bytes // CS)
    up_clusters = -(-len(X.UPCASE_TABLE) // CS)
    spare = 4096  # free clusters at the end
    used_data = root_clusters + up_clusters + folders * (dir_clusters + per)
    cc = used_data + spare
    for _ in range(5):
        bm_clusters = -(-(-(-cc // 8)) // CS)
        cc = used_data + bm_clusters + spare
    fat_len = -(-((cc + 2) * 4) // SECTOR)
    heap = -(-(FAT_OFFSET + fat_len) // SPC) * SPC
    vol_sectors = heap + cc * SPC
    vol_size = vol_sectors * SECTOR
    disk_size = -(-(2088 * SECTOR + vol_size + 2 * X.MiB) // X.MiB) * X.MiB
    disk = X.Disk(FakeVol(vol_size, "FINAL 1 TB"), disk_size)
    vol_off = disk.vol_offset

    def coff(c):
        return vol_off + heap * SECTOR + (c - 2) * CS

    nxt = 2
    bitmap = list(range(nxt, nxt + bm_clusters))
    nxt += bm_clusters
    upcase = list(range(nxt, nxt + up_clusters))
    nxt += up_clusters
    root = list(range(nxt, nxt + root_clusters))
    nxt += root_clusters
    layout = []  # (folder name, first dir cluster, first file cluster)
    for i in range(folders):
        layout.append(("Folder %04d" % i, nxt, nxt + dir_clusters))
        nxt += dir_clusters + per
    last_used = nxt - 1

    f = open(img_path, "wb")
    f.truncate(disk_size)
    for off, data in disk._gpt():
        f.seek(off)
        f.write(data)
    # Boot region, twice.
    boot = bytearray(SECTOR)
    boot[0:3] = b"\xEB\x76\x90"
    boot[3:11] = b"EXFAT   "
    struct.pack_into("<QQIIIIIIHHBBBBB", boot, 64, disk.vol_start, vol_sectors, FAT_OFFSET, fat_len, heap, cc,
                     root[0], 0x5EED1234, 0x0100, 0, 9, SPC.bit_length() - 1, 1, 0x80,
                     min(100, (last_used - 1) * 100 // cc))
    boot[510:512] = b"\x55\xAA"
    region = bytearray(boot)
    for _ in range(8):
        s = bytearray(SECTOR)
        s[508:512] = b"\x00\x00\x55\xAA"
        region += s
    region += bytes(SECTOR * 2)
    region += struct.pack("<I", X.checksum32(region, boot=True)) * (SECTOR // 4)
    f.seek(vol_off)
    f.write(region)
    f.write(region)
    # FAT: chains only for the bitmap, the up-case table and the top folder.
    fat = bytearray(4 * (cc + 2))
    struct.pack_into("<II", fat, 0, 0xFFFFFFF8, 0xFFFFFFFF)
    for chain in (bitmap, upcase, root):
        for a, b in zip(chain, chain[1:]):
            struct.pack_into("<I", fat, 4 * a, b)
        struct.pack_into("<I", fat, 4 * chain[-1], X.EOC)
    f.seek(vol_off + FAT_OFFSET * SECTOR)
    f.write(fat)
    # Allocation bitmap: clusters 2 .. last_used.
    bm = bytearray(-(-cc // 8))
    n_used = last_used - 1
    bm[:n_used // 8] = b"\xff" * (n_used // 8)
    for k in range(n_used // 8 * 8, n_used):
        bm[k // 8] |= 1 << (k % 8)
    f.seek(coff(bitmap[0]))
    f.write(bm)
    f.seek(coff(upcase[0]))
    f.write(X.UPCASE_TABLE)
    # The top folder.
    out = bytearray()
    lab = X.utf16_units("FINAL 1 TB")
    e = bytearray(32)
    e[0] = 0x83
    e[1] = len(lab)
    struct.pack_into("<%dH" % len(lab), e, 2, *lab)
    out += e
    e = bytearray(32)
    e[0] = 0x81
    struct.pack_into("<IQ", e, 20, bitmap[0], len(bm))
    out += e
    e = bytearray(32)
    e[0] = 0x82
    struct.pack_into("<I", e, 4, X.checksum32(X.UPCASE_TABLE))
    struct.pack_into("<IQ", e, 20, upcase[0], len(X.UPCASE_TABLE))
    out += e
    for name, d0, _ in layout:
        out += entry_set(name, X.ATTR_DIRECTORY, d0, dir_clusters * CS)
    f.seek(coff(root[0]))
    f.write(out)
    # Every folder: its records, then its files.
    truth = {"files": folders * per, "folders": folders, "damaged_files": [], "hidden_after": None,
             "disk_size": disk_size, "volume_offset": vol_off}
    for i, (name, d0, f0) in enumerate(layout):
        region = bytearray((dir_clusters + per) * CS)
        recs = bytearray()
        for k in range(per):
            ext, data = kinds[(i * per + k) % len(kinds)]
            fname = "IMG_%06d.%s" % (i * per + k, ext)
            recs += entry_set(fname, X.ATTR_ARCHIVE, f0 + k, len(data))
            o = (dir_clusters + k) * CS
            region[o:o + len(data)] = data
        region[0:len(recs)] = recs
        f.seek(coff(d0))
        f.write(region)
        if i % 200 == 0:
            print("  folder %d of %d" % (i, folders), flush=True)
    # Places never read.
    bad = []
    step = max(1, folders // 25)
    for i in range(3, folders, step):  # the first sector of one file in each of 25 folders
        name, d0, f0 = layout[i]
        k = (i * 7) % per
        bad.append((coff(f0 + k), SECTOR))
        truth["damaged_files"].append("%s\\IMG_%06d" % (name, i * per + k))
    name, d0, f0 = layout[folders // 2]  # one sector of one folder's records
    bad.append((coff(d0) + 4 * SECTOR, SECTOR))
    truth["hidden_after"] = name
    free0 = last_used + 100
    bad.append((coff(free0), 8 * SECTOR))
    bad.sort()
    for off, n in bad:
        f.seek(off)
        f.write(bytes(n))
    f.close()
    X.write_mapfile(map_path, disk_size, [(s, n, "-") for s, n in bad])
    with open(img_path + ".json", "w") as j:
        json.dump(truth, j, indent=1)
    print("%s: %d folders, %d files, %.1f GB" % (img_path, folders, folders * per, disk_size / 1e9))


if __name__ == "__main__":
    main()
