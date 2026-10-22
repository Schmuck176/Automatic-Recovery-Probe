#!/usr/bin/env python3
"""Builds exFAT disk images for testing the map check.

Written from scratch from Microsoft's exFAT specification and the UEFI GPT
layout (no third-party code), so test disks can be made anywhere Python
runs. Every image records exactly where each file, folder and folder
record was put (the "truth"), so the map check's answers can be checked
against something that does not depend on the program's own exFAT reader.

The images themselves are checked independently: by fsck.exfat
(exfatprogs), by mounting them with the Linux kernel's exFAT driver, and
by attaching them to Windows as virtual disks (see .github/workflows).

    v = Volume(48 * MiB, cluster_size=4096, label="FINAL 1 TB")
    v.mkdir("Photos")
    v.add_file("Photos/IMG_0001.JPG", tagged(1, 300_000))
    disk = Disk(v, disk_size=64 * MiB)
    disk.save("test.img")
    truth = disk.truth()
"""

import json
import random
import struct
import sys
import uuid
import zlib

SECTOR = 512
KiB = 1 << 10
MiB = 1 << 20
EOC = 0xFFFFFFFF
LEFTOVER_GAP = 4  # blank records between a folder's end and the records left after it

EFI_TYPE = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B"
BASIC_DATA = "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7"
APPLE_HFS = "48465300-0000-11AA-AA11-00306543ECAC"

ATTR_READ_ONLY = 0x01
ATTR_HIDDEN = 0x02
ATTR_SYSTEM = 0x04
ATTR_DIRECTORY = 0x10
ATTR_ARCHIVE = 0x20


# ---------------------------------------------------------------- checksums

def entry_set_checksum(data):
    """exFAT spec 6.3.3: every byte except bytes 2 and 3 of the set."""
    c = 0
    for i, b in enumerate(data):
        if i in (2, 3):
            continue
        c = ((((c & 1) << 15) | (c >> 1)) + b) & 0xFFFF
    return c


def checksum32(data, boot=False):
    """exFAT spec 3.4 (boot region, skipping bytes 106, 107, 112) and 7.2.2
    (up-case table, every byte)."""
    c = 0
    for i, b in enumerate(data):
        if boot and i in (106, 107, 112):
            continue
        c = ((((c & 1) << 31) | (c >> 1)) + b) & 0xFFFFFFFF
    return c


def make_upcase():
    """An up-case table built from Python's own upper-casing of the Basic
    Multilingual Plane, compressed the way the spec describes (a run of
    unchanged characters is 0xFFFF followed by its length). Returns
    (mapping list, table bytes)."""
    m = []
    for c in range(0x10000):
        u = c
        # Only blocks whose letters have long been in Unicode: newer Pythons
        # know newer Unicode versions (3.14 added capitals 3.13 lacks), and
        # the tables must come out the same on every Python version.
        settled = lambda x: x < 0x1C80 or 0x1D00 <= x < 0x2C00 or 0xFF00 <= x < 0xFFF0
        if settled(c):
            up = chr(c).upper()
            if len(up) == 1 and settled(ord(up)):
                u = ord(up)
        m.append(u)
    out = []
    i = 0
    while i < 0x10000:
        if m[i] == i:
            j = i
            while j < 0x10000 and m[j] == j:
                j += 1
            run = j - i
            if run >= 3:
                while run:
                    n = min(run, 0xFFFE)
                    out += [0xFFFF, n]
                    run -= n
                i = j
                continue
        out.append(m[i])
        i += 1
    return m, struct.pack("<%dH" % len(out), *out)


UPCASE_MAP, UPCASE_TABLE = make_upcase()


def utf16_units(name):
    b = name.encode("utf-16-le")
    return list(struct.unpack("<%dH" % (len(b) // 2), b))


def name_hash(units):
    """exFAT spec 7.6.4, over the up-cased name."""
    h = 0
    for u in units:
        u = UPCASE_MAP[u]
        for byte in (u & 0xFF, u >> 8):
            h = ((((h & 1) << 15) | (h >> 1)) + byte) & 0xFFFF
    return h


def exfat_timestamp(y, mo, d, h, mi, s):
    return ((y - 1980) << 25) | (mo << 21) | (d << 16) | (h << 11) | (mi << 5) | (s // 2)


def tagged(fid, size, seed=0):
    """File contents in which every 512-byte block names its file and
    position ("RPTAG <file> <block>") and no block is all zeros, so any
    sector of a disk can be traced back to the file it belongs to."""
    rng = random.Random(fid * 1000003 + seed)
    out = bytearray()
    block = 0
    while len(out) < size:
        head = b"RPTAG %06d %08d " % (fid, block)
        out += head + rng.randbytes(SECTOR - len(head))
        block += 1
    return bytes(out[:size])


def runs_of(clusters):
    runs = []
    for c in clusters:
        if runs and runs[-1][0] + runs[-1][1] == c:
            runs[-1][1] += 1
        else:
            runs.append([c, 1])
    return runs


# ---------------------------------------------------------------- the volume

class Node:
    def __init__(self, vol, name, parent, is_dir):
        self.vol = vol
        self.name = name
        self.parent = parent
        self.is_dir = is_dir
        self.children = []
        self.data = b""
        self.data_length = 0
        self.vdl = 0
        self.attrs = ATTR_DIRECTORY if is_dir else ATTR_ARCHIVE
        self.clusters = []  # in file order
        self.no_fat_chain = True
        self.force_fat_chain = False
        self.fragments = None  # list of cluster counts
        self.gap = 1  # free clusters between fragments
        self.gap_before = 0
        self.extra_clusters = 0  # folders: spare clusters
        self.deleted = False
        self.slot = None  # first slot in the parent folder
        self.mtime = exfat_timestamp(2023, 7, 14, 10, 30, 0)
        self.record_offsets = []  # disk... volume offsets of each slot of the set
        self.leftover = False  # a record left after its folder's end (Volume.add_leftover)
        self.points_at = None  # ...claiming this file's clusters

    @property
    def path(self):
        if self.parent is None:
            return ""
        p = self.parent.path
        return self.name if not p else p + "\\" + self.name

    def slots(self):
        """Folder slots this entry set takes in its parent."""
        n = len(utf16_units(self.name))
        return 2 + (n + 14) // 15


class Volume:
    def __init__(self, size, cluster_size=4096, label="TEST VOLUME", serial=0x1234ABCD, fat_offset=128,
                 heap_align=None, sector=SECTOR):
        assert sector == SECTOR, "only 512-byte sectors are built"
        assert cluster_size % sector == 0 and cluster_size & (cluster_size - 1) == 0
        self.size = size // sector * sector
        self.cs = cluster_size
        self.spc = cluster_size // sector
        self.label = label
        self.serial = serial
        self.fat_offset = fat_offset  # sectors
        align = heap_align or max(self.spc, 8)
        sectors = self.size // sector
        cc = (sectors - fat_offset) // self.spc
        while True:
            fat_len = -(-((cc + 2) * 4) // sector)
            heap = -(-(fat_offset + fat_len) // align) * align
            new_cc = (sectors - heap) // self.spc
            if new_cc >= cc:
                break
            cc = new_cc
        self.fat_length = fat_len  # sectors
        self.heap_offset = heap  # sectors
        self.cluster_count = cc
        self.root = Node(self, "", None, True)
        self.events = []
        self.cursor = 2
        self.allocated = set()
        self.fat = {}
        self.laid_out = False
        self.bitmap_clusters = []
        self.upcase_clusters = []

    # -- building the tree ---------------------------------------------------
    def _parent(self, path):
        parts = path.replace("/", "\\").split("\\")
        node = self.root
        for p in parts[:-1]:
            found = [c for c in node.children if c.name == p and c.is_dir and not c.deleted]
            if not found:
                raise ValueError("no folder %r for %r" % (p, path))
            node = found[0]
        return node, parts[-1]

    def mkdir(self, path, extra_clusters=0, fragments=None, gap=1, fat_chain=False, hidden=False):
        parent, name = self._parent(path)
        n = Node(self, name, parent, True)
        n.extra_clusters = extra_clusters
        n.fragments = fragments
        n.gap = gap
        n.force_fat_chain = fat_chain
        if hidden:
            n.attrs |= ATTR_HIDDEN
        parent.children.append(n)
        self.events.append(n)
        return n

    def add_file(self, path, data, vdl=None, fragments=None, gap=1, gap_before=0, fat_chain=False,
                 deleted=False, attrs=None, data_length=None):
        parent, name = self._parent(path)
        n = Node(self, name, parent, False)
        n.data = data
        n.data_length = len(data) if data_length is None else data_length
        n.vdl = n.data_length if vdl is None else vdl
        n.fragments = fragments
        n.gap = gap
        n.gap_before = gap_before
        n.force_fat_chain = fat_chain
        n.deleted = deleted
        if attrs is not None:
            n.attrs = attrs
        parent.children.append(n)
        self.events.append(n)
        return n

    def add_leftover(self, path, data=None, points_at=None, size=None):
        """A file record left behind after its folder's end: a blank record
        (the end of the folder), and after it records that were never
        cleared, as a folder's clusters hold when they were used before. The
        exFAT rules say nothing but blank records may follow the end;
        Windows stops reading there and never lists them; fsck.exfat calls
        them an error; dissect.fat lists them. `points_at`: the path of a
        file whose clusters it claims, the first `size` bytes' worth (as if
        they were used again since); otherwise it claims free clusters
        holding `data` (as a deleted file's data stays behind)."""
        parent, name = self._parent(path)
        n = Node(self, name, parent, False)
        n.leftover = True
        if points_at is not None:
            n.points_at = points_at.replace("/", "\\")
            n.data_length = n.vdl = size
        else:
            n.data = data
            n.data_length = n.vdl = len(data)
        parent.children.append(n)
        self.events.append(n)
        return n

    def _by_path(self, path):
        for n in self.events:
            if n.path == path and not n.deleted and not n.leftover:
                return n
        raise ValueError("no file %r" % path)

    # -- allocation ------------------------------------------------------------
    def _take(self, count):
        out = []
        for _ in range(count):
            if self.cursor > self.cluster_count + 1:
                raise ValueError("the test volume is full")
            out.append(self.cursor)
            self.cursor += 1
        return out

    def _alloc(self, node, count):
        if count == 0:
            return []
        self.cursor += node.gap_before
        if node.fragments:
            assert sum(node.fragments) == count, (node.path, node.fragments, count)
            out = []
            for i, n in enumerate(node.fragments):
                if i:
                    self.cursor += node.gap
                out += self._take(n)
            return out
        return self._take(count)

    def _dir_slots(self, d):
        n = sum(c.slots() for c in d.children if not c.leftover)
        if d is self.root:
            n += 3  # label, allocation bitmap, up-case table
        left = [c for c in d.children if c.leftover]
        if left:
            n += LEFTOVER_GAP + sum(c.slots() for c in left)
        return n

    def layout(self):
        if self.laid_out:
            return
        bitmap_bytes = -(-self.cluster_count // 8)
        self.bitmap_clusters = self._take(-(-bitmap_bytes // self.cs))
        self.upcase_clusters = self._take(-(-len(UPCASE_TABLE) // self.cs))
        root_n = max(1, -(-self._dir_slots(self.root) * 32 // self.cs))
        self.root.clusters = self._take(root_n)
        self.root.no_fat_chain = False
        for c in self.bitmap_clusters + self.upcase_clusters + self.root.clusters:
            self.allocated.add(c)
        for chain in (self.bitmap_clusters, self.upcase_clusters, self.root.clusters):
            self._chain(chain)
        for n in self.events:
            if n.leftover:
                continue  # its clusters are someone else's, or free (below)
            if n.is_dir:
                count = max(1, -(-self._dir_slots(n) * 32 // self.cs)) + n.extra_clusters
                n.data_length = n.vdl = count * self.cs
            else:
                count = -(-n.data_length // self.cs)
            n.clusters = self._alloc(n, count)
            contiguous = all(b == a + 1 for a, b in zip(n.clusters, n.clusters[1:]))
            n.no_fat_chain = contiguous and not n.force_fat_chain
            if n.deleted:
                continue
            for c in n.clusters:
                assert c not in self.allocated
                self.allocated.add(c)
            if not n.no_fat_chain:
                self._chain(n.clusters)
        free_end = self.cluster_count + 1
        for n in self.events:
            if not n.leftover:
                continue
            count = -(-n.data_length // self.cs)
            if n.points_at is not None:
                n.clusters = self._by_path(n.points_at).clusters[:count]
            else:
                n.clusters = []
                while len(n.clusters) < count:
                    if free_end in self.allocated or free_end < self.cursor:
                        raise ValueError("no free clusters left for a leftover record")
                    n.clusters.insert(0, free_end)
                    free_end -= 1
            # (A chain a leftover follows is the FAT's: its file's, if any.)
            n.no_fat_chain = all(b == a + 1 for a, b in zip(n.clusters, n.clusters[1:]))
        self.laid_out = True

    def _chain(self, clusters):
        for a, b in zip(clusters, clusters[1:]):
            self.fat[a] = b
        if clusters:
            self.fat[clusters[-1]] = EOC

    # -- writing ---------------------------------------------------------------
    def cluster_offset(self, c):
        """Offset of a cluster from the volume start."""
        return self.heap_offset * SECTOR + (c - 2) * self.cs

    def node_offset(self, node, pos):
        """Volume offset of byte `pos` of a file or folder."""
        k, r = divmod(pos, self.cs)
        return self.cluster_offset(node.clusters[k]) + r

    def _entry_set(self, n):
        units = utf16_units(n.name)
        names = (len(units) + 14) // 15
        sc = 1 + names
        e = bytearray(32 * (sc + 1))
        e[0] = 0x85
        e[1] = sc
        struct.pack_into("<H", e, 4, n.attrs)
        struct.pack_into("<III", e, 8, n.mtime, n.mtime, n.mtime)
        e[20] = 0
        e[21] = 0
        e[22] = e[23] = e[24] = 0x80 | 0  # UTC offsets valid, +00:00
        s = 32
        e[s] = 0xC0
        flags = 0
        if n.clusters:
            flags |= 1  # AllocationPossible
        if n.no_fat_chain and n.clusters:
            flags |= 2
        e[s + 1] = flags
        e[s + 3] = len(units)
        struct.pack_into("<H", e, s + 4, name_hash(units))
        struct.pack_into("<Q", e, s + 8, n.vdl)
        struct.pack_into("<I", e, s + 20, n.clusters[0] if n.clusters else 0)
        struct.pack_into("<Q", e, s + 24, n.data_length)
        for k in range(names):
            o = 64 + 32 * k
            e[o] = 0xC1
            part = units[15 * k:15 * k + 15]
            struct.pack_into("<%dH" % len(part), e, o + 2, *part)
        struct.pack_into("<H", e, 2, entry_set_checksum(e))
        if n.deleted:
            # Deleting clears the "in use" bit of each part and nothing else:
            # the checksum stays the one made while the file was in use.
            for k in range(sc + 1):
                e[32 * k] &= 0x7F
        return bytes(e)

    def _dir_bytes(self, d):
        out = bytearray()
        if d is self.root:
            lab = utf16_units(self.label)[:11]
            e = bytearray(32)
            e[0] = 0x83
            e[1] = len(lab)
            struct.pack_into("<%dH" % len(lab), e, 2, *lab)
            out += e
            e = bytearray(32)
            e[0] = 0x81
            struct.pack_into("<IQ", e, 20, self.bitmap_clusters[0], -(-self.cluster_count // 8))
            out += e
            e = bytearray(32)
            e[0] = 0x82
            struct.pack_into("<I", e, 4, checksum32(UPCASE_TABLE))
            struct.pack_into("<IQ", e, 20, self.upcase_clusters[0], len(UPCASE_TABLE))
            out += e
        for c in d.children:
            if c.leftover:
                continue
            c.slot = len(out) // 32
            out += self._entry_set(c)
        left = [c for c in d.children if c.leftover]
        if left:
            out += bytes(32 * LEFTOVER_GAP)  # the end of the folder
            for c in left:
                c.slot = len(out) // 32
                out += self._entry_set(c)
        size = len(d.clusters) * self.cs
        assert len(out) <= size
        out += bytes(size - len(out))
        return out

    def build(self):
        """The volume as bytes (built once)."""
        if getattr(self, "_img", None) is not None:
            return self._img
        self.layout()
        img = bytearray(self.size)
        cs = self.cs
        # Up-case table, directories and files.
        def put(clusters, data):
            for k in range(0, len(data), cs):
                o = self.cluster_offset(clusters[k // cs])
                chunk = data[k:k + cs]
                img[o:o + len(chunk)] = chunk

        put(self.upcase_clusters, UPCASE_TABLE)
        dirs = [self.root] + [n for n in self.events if n.is_dir and not n.deleted]
        for d in dirs:
            put(d.clusters, self._dir_bytes(d))
        for n in self.events:
            if not n.is_dir and not n.deleted and n.clusters:
                data = n.data[:n.vdl]
                put(n.clusters, data)
            if n.deleted and n.clusters and not n.is_dir:
                put(n.clusters, n.data[:n.vdl])  # deleted files leave their data behind
        # Where each entry set's slots are (volume offsets).
        for d in dirs:
            for c in d.children:
                c.record_offsets = [self.node_offset(d, (c.slot + k) * 32) for k in range(c.slots())]
        # Allocation bitmap.
        bm = bytearray(-(-self.cluster_count // 8))
        for c in self.allocated:
            i = c - 2
            bm[i // 8] |= 1 << (i % 8)
        put(self.bitmap_clusters, bytes(bm))
        # FAT.
        fat_off = self.fat_offset * SECTOR
        struct.pack_into("<II", img, fat_off, 0xFFFFFFF8, 0xFFFFFFFF)
        for c, nxt in self.fat.items():
            struct.pack_into("<I", img, fat_off + 4 * c, nxt)
        # Boot region (main, then backup).
        boot = bytearray(SECTOR)
        boot[0:3] = b"\xEB\x76\x90"
        boot[3:11] = b"EXFAT   "
        struct.pack_into("<QQIIIIIIHHBBBBB", boot, 64,
                         0,  # PartitionOffset: filled in by Disk
                         self.size // SECTOR,
                         self.fat_offset, self.fat_length, self.heap_offset, self.cluster_count,
                         self.root.clusters[0], self.serial,
                         0x0100,  # revision 1.00
                         0,  # volume flags
                         9, (self.spc).bit_length() - 1, 1, 0x80,
                         min(100, len(self.allocated) * 100 // self.cluster_count))
        boot[510] = 0x55
        boot[511] = 0xAA
        self._boot = boot
        self._img = img
        self.write_boot(0)
        return img

    def write_boot(self, partition_offset_sectors):
        boot = bytearray(self._boot)
        struct.pack_into("<Q", boot, 64, partition_offset_sectors)
        region = bytearray(boot)
        for _ in range(8):  # extended boot sectors
            s = bytearray(SECTOR)
            s[508:512] = b"\x00\x00\x55\xAA"
            region += s
        region += bytes(SECTOR * 2)  # OEM parameters, reserved
        csum = checksum32(region, boot=True)
        region += struct.pack("<I", csum) * (SECTOR // 4)
        self._img[0:12 * SECTOR] = region
        self._img[12 * SECTOR:24 * SECTOR] = region

    def nodes(self):
        return [self.root] + self.events


# ---------------------------------------------------------------- the disk

def _guid(rng):
    # Version and variant bits set by hand: uuid.UUID(version=4) does not
    # give the same answer on every Python version.
    b = bytearray(rng.randbytes(16))
    b[6] = (b[6] & 0x0F) | 0x40
    b[8] = (b[8] & 0x3F) | 0x80
    return str(uuid.UUID(bytes=bytes(b))).upper()


class Disk:
    """A disk with a GPT: an EFI system partition, then the exFAT volume.

    gpt_disk_size: the size the partition table was written for. A ddrescue
    copy of a smaller disk onto a bigger SSD keeps the old disk's table, so
    the table (its backup header) still points at the old disk's end.
    """

    def __init__(self, vol, disk_size, gpt_disk_size=None, efi_start_lba=40, efi_size=1 * MiB,
                 vol_start_lba=None, seed=1, scheme="gpt"):
        self.vol = vol
        self.disk_size = disk_size // SECTOR * SECTOR
        self.gpt_disk_size = (gpt_disk_size or disk_size) // SECTOR * SECTOR
        self.scheme = scheme
        rng = random.Random(seed)
        self.disk_guid = _guid(rng)
        self.efi_start = efi_start_lba
        self.efi_sectors = efi_size // SECTOR
        if vol_start_lba is None:
            vol_start_lba = efi_start_lba + self.efi_sectors
        self.vol_start = vol_start_lba
        self.parts = []
        if scheme == "gpt":
            if efi_size:
                self.parts.append(("EFI System Partition", EFI_TYPE, _guid(rng), self.efi_start,
                                   self.efi_start + self.efi_sectors - 1))
            self.parts.append((vol.label, BASIC_DATA, _guid(rng), self.vol_start,
                               self.vol_start + vol.size // SECTOR - 1))
            last = self.gpt_disk_size // SECTOR - 1
            assert self.parts[-1][4] <= last - 33, "the volume does not fit in the partition table's disk"
        assert self.vol_start * SECTOR + vol.size <= self.disk_size

    @property
    def vol_offset(self):
        return self.vol_start * SECTOR

    def _gpt(self):
        last = self.gpt_disk_size // SECTOR - 1
        entries = bytearray(128 * 128)
        for i, (name, typ, guid, first, end) in enumerate(self.parts):
            e = bytearray(128)
            e[0:16] = uuid.UUID(typ).bytes_le
            e[16:32] = uuid.UUID(guid).bytes_le
            struct.pack_into("<QQQ", e, 32, first, end, 0)
            n = name.encode("utf-16-le")[:72]
            e[56:56 + len(n)] = n
            entries[i * 128:(i + 1) * 128] = e
        ecrc = zlib.crc32(entries) & 0xFFFFFFFF

        def header(my, alt, entries_lba):
            h = bytearray(SECTOR)
            h[0:8] = b"EFI PART"
            struct.pack_into("<IIIIQQQQ", h, 8, 0x00010000, 92, 0, 0, my, alt, 34, last - 33)
            h[56:72] = uuid.UUID(self.disk_guid).bytes_le
            struct.pack_into("<QIII", h, 72, entries_lba, 128, 128, ecrc)
            struct.pack_into("<I", h, 16, zlib.crc32(h[:92]) & 0xFFFFFFFF)
            return bytes(h)

        mbr = bytearray(SECTOR)
        mbr[446:462] = struct.pack("<B3sB3sII", 0, b"\x00\x02\x00", 0xEE, b"\xff\xff\xff", 1,
                                   min(last, 0xFFFFFFFF))
        mbr[510:512] = b"\x55\xAA"
        return [
            (0, bytes(mbr)),
            (SECTOR, header(1, last, 2)),
            (2 * SECTOR, bytes(entries)),
            ((last - 32) * SECTOR, bytes(entries)),
            (last * SECTOR, header(last, 1, last - 32)),
        ]

    def _mbr(self):
        mbr = bytearray(SECTOR)
        mbr[446:462] = struct.pack("<B3sB3sII", 0, b"\xfe\xff\xff", 0x07, b"\xfe\xff\xff", self.vol_start,
                                   self.vol.size // SECTOR)
        mbr[510:512] = b"\x55\xAA"
        return [(0, bytes(mbr))]

    def pieces(self):
        """(disk offset, bytes) for everything that is not zeros."""
        vol_img = self.vol.build()
        self.vol.write_boot(self.vol_start)
        out = self._gpt() if self.scheme == "gpt" else (self._mbr() if self.scheme == "mbr" else [])
        out.append((self.vol_offset, bytes(vol_img)))
        return out

    def save(self, path, vhd=False):
        with open(path, "wb") as f:
            f.truncate(self.disk_size)
            for off, data in self.pieces():
                # Write only the parts that are not zeros, so the file stays sparse.
                step = 64 * KiB
                for k in range(0, len(data), step):
                    chunk = data[k:k + step]
                    if chunk.count(0) != len(chunk):
                        f.seek(off + k)
                        f.write(chunk)
            if vhd:
                f.seek(self.disk_size)
                f.write(vhd_footer(self.disk_size))
        return path

    def save_volume(self, path):
        """Only the exFAT volume (for fsck.exfat)."""
        img = self.vol.build()
        self.vol.write_boot(self.vol_start)
        with open(path, "wb") as f:
            f.write(img)

    def truth(self):
        v = self.vol
        v.build()
        v.write_boot(self.vol_start)
        vo = self.vol_offset
        nodes = []
        for n in v.nodes():
            if n is v.root:
                continue
            nodes.append({
                "path": n.path,
                "is_dir": n.is_dir,
                "deleted": n.deleted,
                "data_length": n.data_length,
                "vdl": n.vdl,
                "runs": runs_of(n.clusters),
                "no_fat_chain": n.no_fat_chain,
                "parent": n.parent.path,
                "slot": n.slot,
                "slots": n.slots(),
                "record_offsets": [vo + o for o in n.record_offsets],
                "leftover": n.leftover,
            })
        return {
            "disk_size": self.disk_size,
            "gpt_disk_size": self.gpt_disk_size,
            "scheme": self.scheme,
            "partitions": [{"name": p[0], "type": p[1], "start": p[3] * SECTOR, "end": (p[4] + 1) * SECTOR}
                           for p in self.parts],
            "volume": {
                "offset": vo,
                "size": v.size,
                "cluster_size": v.cs,
                "fat_offset": v.fat_offset * SECTOR,
                "fat_length": v.fat_length * SECTOR,
                "heap_offset": v.heap_offset * SECTOR,
                "cluster_count": v.cluster_count,
                "serial": v.serial,
                "label": v.label,
                "bitmap": runs_of(v.bitmap_clusters),
                "bitmap_bytes": -(-v.cluster_count // 8),
                "upcase": runs_of(v.upcase_clusters),
                "root": runs_of(v.root.clusters),
                "allocated": sorted(v.allocated),
            },
            "nodes": nodes,
        }


def vhd_footer(size):
    """A fixed-size VHD footer, so Windows can attach the image as a disk."""
    total = size // SECTOR
    if total > 65535 * 16 * 255:
        total = 65535 * 16 * 255
    if total >= 65535 * 16 * 63:
        spt, heads = 255, 16
        cth = total // spt
    else:
        spt = 17
        cth = total // spt
        heads = max(4, (cth + 1023) // 1024)
        if cth >= heads * 1024 or heads > 16:
            spt, heads = 31, 16
            cth = total // spt
        if cth >= heads * 1024:
            spt, heads = 63, 16
            cth = total // spt
    cyl = cth // heads
    f = bytearray(512)
    f[0:8] = b"conectix"
    struct.pack_into(">IIQI", f, 8, 2, 0x00010000, 0xFFFFFFFFFFFFFFFF, 0)
    f[28:32] = b"rprb"
    struct.pack_into(">I", f, 32, 0x00010000)
    f[36:40] = b"Wi2k"
    struct.pack_into(">QQHBBI", f, 40, size, size, cyl, heads, spt, 2)
    f[68:84] = uuid.UUID(int=size ^ 0x5250).bytes
    struct.pack_into(">I", f, 64, (~sum(f)) & 0xFFFFFFFF)
    return bytes(f)


# ---------------------------------------------------------------- mapfiles

def write_mapfile(path, disk_size, bad, version="1.25", finished=True, start=0, extra_args=""):
    """A GNU ddrescue mapfile covering [start, disk_size) where the areas in
    `bad` (a list of (start, length, status)) were never read."""
    bad = sorted(bad)
    lines = [
        "# Mapfile. Created by GNU ddrescue version %s" % version,
        "# Command line: ddrescue -v -f %s-s %d -r 2 -c 32 -b 512 /dev/rdisk5 /dev/disk4 map.log"
        % (extra_args, disk_size - start),
        "# Start time:   2024-04-21 02:14:00",
        "# Current time: 2024-04-24 21:38:08",
    ]
    if finished:
        lines.append("# Finished")
    lines += ["# current_pos  current_status  current_pass", "0x%08X     +               2" % start,
              "#      pos        size  status"]
    pos = start
    for s, n, st in bad:
        assert s >= pos, "overlapping bad areas"
        if s > pos:
            lines.append("0x%08X  0x%08X  +" % (pos, s - pos))
        lines.append("0x%08X  0x%08X  %s" % (s, n, st))
        pos = s + n
    if pos < disk_size:
        lines.append("0x%08X  0x%08X  +" % (pos, disk_size - pos))
    with open(path, "w", newline="\n") as f:
        f.write("\n".join(lines) + "\n")


def write_earlier_mapfile(path, disk_size, read):
    """An unfinished earlier attempt's mapfile that read only `read`
    (a list of (start, length)); everything else was never tried."""
    lines = [
        "# Mapfile. Created by GNU ddrescue version 1.25",
        "# Command line: ddrescue -v -f -s %d -r 2 -c 128 -b 512 /dev/rdisk5 /dev/disk4 first.log" % disk_size,
        "# Start time:   2024-04-17 11:33:47",
        "# Current time: 2024-04-17 12:35:17",
        "# current_pos  current_status  current_pass",
        "0x%08X     ?               1" % 0,
        "#      pos        size  status",
    ]
    pos = 0
    for s, n in sorted(read):
        if s > pos:
            lines.append("0x%08X  0x%08X  ?" % (pos, s - pos))
        lines.append("0x%08X  0x%08X  +" % (s, n))
        pos = s + n
    if pos < disk_size:
        lines.append("0x%08X  0x%08X  ?" % (pos, disk_size - pos))
    with open(path, "w", newline="\n") as f:
        f.write("\n".join(lines) + "\n")


def zero_areas(path, areas):
    """Zero the given (offset, length) areas of an image file in place."""
    with open(path, "r+b") as f:
        for s, n in areas:
            f.seek(s)
            f.write(bytes(n))


if __name__ == "__main__":
    # A small demonstration disk: python exfat_image.py demo.img [--vhd]
    out = sys.argv[1] if len(sys.argv) > 1 else "demo.img"
    v = Volume(24 * MiB, cluster_size=4096, label="DEMO")
    v.mkdir("Photos")
    for i in range(20):
        v.add_file("Photos/IMG_%04d.JPG" % i, tagged(i + 1, 20000 + 997 * i))
    d = Disk(v, 32 * MiB)
    d.save(out, vhd="--vhd" in sys.argv)
    with open(out + ".truth.json", "w") as f:
        json.dump(d.truth(), f, indent=1)
    print("wrote", out)
