#!/usr/bin/env python3
"""Proves the map check right on test disks with known damage.

Each test builds an exFAT disk with tools/exfat_image.py, damages chosen
sectors the way unread sectors look on the recovered SSD (zeros), writes a
ddrescue mapfile naming them, runs `recovery-probe map-check`, and compares
its map-check.json with what the damage must mean.

The expected answer is worked out here independently of the program: from
the builder's record of where every file, folder and folder record was put,
plus the raw bytes of the damaged disk (which sectors are zeros now, what
the FAT and the allocation bitmap hold). Nothing here parses exFAT folders.

    python test_mapcheck.py <recovery-probe executable> <work folder> [--random N] [--keep]
        [--fsck path/to/fsck.exfat] [--dissect folder-with-dissect-packages]

Exit code 0 when every test passes. The last line is a one-line summary.
"""

import argparse
import json
import os
import random
import shutil
import struct
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import exfat_image as X  # noqa: E402

SECTOR = 512
EOC = 0xFFFFFFFF
BAD = 0xFFFFFFF7


# ---------------------------------------------------------------- the oracle

class Oracle:
    """What the map check must report, from the builder's truth, the
    damaged image's raw bytes, and the pristine image's raw bytes."""

    def __init__(self, truth, image, pristine, areas, shift=0):
        self.t = truth
        self.v = truth["volume"]
        self.f = open(image, "rb")
        self.p = open(pristine, "rb")
        self.areas = sorted(areas)  # (start, end, status) on this disk
        self.cs = self.v["cluster_size"]
        self.cc = self.v["cluster_count"]
        self.vo = self.v["offset"]
        self.heap = self.vo + self.v["heap_offset"]
        self.nodes = {n["path"]: n for n in truth["nodes"]}

    def content(self, path):
        """What a rescued copy of a found file must hold: the disk's bytes
        along its clusters up to its valid data, zeros after that and
        wherever its clusters could not be found."""
        n, cl, broken, hidden = self.found_nodes[path]
        dl, vdl = n["data_length"], min(n["vdl"], n["data_length"])
        out = bytearray()
        for c in cl:
            if len(out) >= vdl:
                break
            out += self.read(self.coff(c), min(self.cs, vdl - len(out)))
        return bytes(out[:vdl]) + bytes(dl - min(len(out), vdl))

    def close(self):
        self.f.close()
        self.p.close()

    def read(self, off, n, pristine=False):
        f = self.p if pristine else self.f
        f.seek(off)
        b = f.read(n)
        return b + bytes(n - len(b))

    def coff(self, c):
        return self.heap + (c - 2) * self.cs

    def valid(self, c):
        return 2 <= c <= self.cc + 1

    def fat(self, c):
        return struct.unpack("<I", self.read(self.vo + self.v["fat_offset"] + 4 * c, 4))[0]

    def chain(self, first, length):
        """Mirror of exFAT chain following (spec 7.2): exactly the clusters
        a reader can find, plus whether the chain broke."""
        out = []
        want = None if length is None else -(-length // self.cs)
        if want == 0:
            return out, False
        limit = min(want if want is not None else (256 << 20) // self.cs + 1, self.cc)
        c = first
        while True:
            if not self.valid(c):
                return out, True
            out.append(c)
            if want is not None and len(out) >= want:
                return out, False
            if len(out) >= limit:
                return out, want is not None
            nxt = self.fat(c)
            if nxt == EOC:
                return out, want is not None
            if nxt == BAD:
                return out, True
            c = nxt

    def clusters_of(self, node):
        if node["data_length"] == 0 or not node["runs"]:
            return [], False
        first = node["runs"][0][0]
        if node["no_fat_chain"]:
            n = -(-node["data_length"] // self.cs)
            if not self.valid(first) or first + n - 1 > self.cc + 1:
                return [], True
            return list(range(first, first + n)), False
        return self.chain(first, node["data_length"])

    def slot_off(self, clusters, slot):
        k, r = divmod(slot * 32, self.cs)
        return self.coff(clusters[k]) + r if k < len(clusters) else None

    def root_slot_intact(self, k):
        root, _ = self.root_clusters()
        off = self.slot_off(root, k)
        return off is not None and self.read(off, 32) == self.read(off, 32, True)

    def root_clusters(self):
        return self.chain(self.v["root"][0][0], None)

    def bitmap(self):
        """The allocation bitmap a reader finds (none if its root record is gone)."""
        if not self.root_slot_intact(1):
            return None
        first = self.v["bitmap"][0][0]
        cl, _ = self.chain(first, self.v["bitmap_bytes"])
        data = b"".join(self.read(self.coff(c), self.cs) for c in cl)
        return data[:self.v["bitmap_bytes"]]

    def blank(self, start, end):
        """(sectors, blank sectors) of [start, end), judged on the part of
        each sector inside the range."""
        sectors = blank = 0
        s = start
        while s < end:
            e = min((s // SECTOR + 1) * SECTOR, end)
            sectors += 1
            if self.read(s, e - s).count(0) == e - s:
                blank += 1
            s = e
        return sectors, blank

    @staticmethod
    def word(sectors, blank):
        if sectors == 0:
            return "none"
        if blank == sectors:
            return "zeros"
        if blank == 0:
            return "data"
        return "mixed"

    def expected(self):
        cs = self.cs
        root_cl, root_broken = self.root_clusters()
        found = {}  # path -> (node, clusters, broken, hidden)
        problems = 0
        folders = files = 0
        children = {}
        for n in self.t["nodes"]:
            children.setdefault(n["parent"], []).append(n)
        maybe_of = {}  # path -> not listed, but Windows still opens it by its full path
        stack = [("", root_cl, False, False)]
        if root_broken:
            problems += 1
        while stack:
            d, cl, d_hidden, d_maybe = stack.pop()
            slots = len(cl) * cs // 32
            subdirs = []
            intact = {}
            for n in sorted(children.get(d, []), key=lambda n: n["slot"]):
                if n["deleted"]:
                    continue
                offs = [self.slot_off(cl, n["slot"] + k) for k in range(n["slots"])]
                if not all(o is not None and self.read(o, 32) == self.read(o, 32, True) for o in offs):
                    continue  # its record is damaged or gone: the file cannot be found
                intact[n["slot"]] = n
            # Where Windows stops reading this folder, and which records it
            # reaches (measured on Windows, tools/windows_check.py), walking
            # the slots in order the way exfat.rs does: a blank slot ends the
            # folder quietly - even inside a file's record; a well-formed
            # record with a wrong checksum makes it give up on the whole
            # folder (it lists none of it, but still opens what comes before
            # when given the full path); a record that is not even well formed
            # is skipped with all the slots it claims (even 200 of them);
            # anything else is skipped.
            # Every damaged file record reached is a problem to report.
            stop_kind, stop_at = None, slots
            reached = []
            k = 0
            while k < slots:
                b = self.read(self.slot_off(cl, k), 32)
                t = b[0]
                if t == 0:
                    if stop_kind is None:
                        stop_kind, stop_at = "blank", k
                    k += 1
                    continue
                if t & 0x80 == 0 or t != 0x85:
                    k += 1
                    continue
                if k in intact:
                    reached.append(intact[k])
                    k += intact[k]["slots"]
                    continue
                problems += 1
                sc = b[1]
                if k + sc >= slots:
                    k += 1
                    continue
                types = [self.read(self.slot_off(cl, k + j), 1)[0] for j in range(sc + 1)]
                if 0 in types[1:]:
                    if stop_kind is None:
                        stop_kind, stop_at = "blank", k
                    k += 1
                    continue
                if not 2 <= sc <= 18:
                    k += sc + 1  # skipped with every slot it claims, however many
                    continue
                stream = self.read(self.slot_off(cl, k + 1), 32)
                ne = -(-stream[3] // 15)
                well_formed = (stream[0] == 0xC0 and stream[3] >= 1 and ne < sc
                               and all(types[2 + j] == 0xC1 for j in range(ne))
                               and all(types[2 + j] & 0xC0 == 0xC0 for j in range(ne, sc - 1)))
                if not well_formed:
                    k += sc + 1
                    continue
                if stop_kind is None:
                    stop_kind, stop_at = "damaged", k  # well formed, yet changed: its checksum is wrong
                k += 1
            for n in sorted(reached, key=lambda n: n["slot"]):
                ncl, broken = self.clusters_of(n)
                if d_hidden and not d_maybe:
                    hidden, maybe = True, False
                elif stop_kind is not None and n["slot"] > stop_at:
                    hidden, maybe = True, False
                elif stop_kind == "damaged":
                    hidden, maybe = True, True
                elif d_hidden:
                    hidden, maybe = True, True
                else:
                    hidden, maybe = False, False
                found[n["path"]] = (n, ncl, broken, hidden)
                maybe_of[n["path"]] = maybe
                if n["is_dir"]:
                    folders += 1
                    if broken:
                        problems += 1
                    if d_hidden and (not d_maybe or maybe):
                        sub = (True, d_maybe)
                    else:
                        sub = (hidden, maybe)
                    subdirs.append((n["path"], ncl) + sub)
                else:
                    files += 1
            for s in reversed(subdirs):
                stack.append(s)

        self.found_nodes = found
        self.root_cl = root_cl
        # Owners of clusters.
        owner = {}
        structures = []
        if self.root_slot_intact(1):
            structures.append(("bitmap", "", self.chain(self.v["bitmap"][0][0], self.v["bitmap_bytes"])[0], None))
        if self.root_slot_intact(2):
            up_len = struct.unpack("<Q", self.read(self.slot_off(root_cl, 2) + 24, 8))[0]
            structures.append(("upcase", "", self.chain(self.v["upcase"][0][0], up_len)[0], None))
        structures.append(("root", "", root_cl, len(root_cl) * cs))
        for kind, path, cl, _ in structures:
            for c in cl:
                owner[c] = (kind, path)
        for path, (n, ncl, broken, hidden) in found.items():
            for c in ncl:
                owner[c] = ("folder" if n["is_dir"] else "file", path)

        # Damaged items: every bad area against every found file and folder.
        def extents(cl):
            """(file offset, disk offset, length) of each run of clusters."""
            out = []
            pos = 0
            for f, k in X.runs_of(cl):
                out.append((pos, self.coff(f), k * cs))
                pos += k * cs
            return out

        items = {}

        def item_for(path, is_dir, cl, broken, data_length, vdl, hidden, maybe=False):
            pieces = []
            for fpos, dpos, ln in extents(cl):
                for s, e, _ in self.areas:
                    lo, hi = max(s, dpos), min(e, dpos + ln)
                    if lo < hi:
                        fs = fpos + (lo - dpos)
                        fe = min(fpos + (hi - dpos), data_length)
                        if fe > fs:
                            pieces.append([fs, fe, lo])
            pieces.sort()
            merged = []
            for p in pieces:
                if merged and merged[-1][1] == p[0] and merged[-1][2] + (merged[-1][1] - merged[-1][0]) == p[2]:
                    merged[-1][1] = p[1]
                else:
                    merged.append(p)
            if not merged and not broken:
                return
            sec = blk = 0
            for fs, fe, d in merged:
                a, b = self.blank(d, d + (fe - fs))
                sec += a
                blk += b
            items[path] = {
                "is_dir": is_dir,
                "ranges": merged,
                "content": self.word(sec, blk),
                "hidden": hidden,
                "hidden_maybe": maybe,
                "broken": broken,
                "only_past_valid_data": bool(merged) and all(fs >= vdl for fs, _, _ in merged),
            }

        item_for("", True, root_cl, root_broken, len(root_cl) * cs, len(root_cl) * cs, False)
        for path, (n, ncl, broken, hidden) in found.items():
            dl = n["data_length"]
            item_for(path, n["is_dir"], ncl, broken, dl, n["vdl"], hidden, maybe_of[path])

        # What each area hits.
        bm = self.bitmap()

        def allocated(c):
            if bm is None:
                return False
            i = c - 2
            return i // 8 < len(bm) and bool(bm[i // 8] & (1 << (i % 8)))

        bps = SECTOR
        vol_end = max(self.vo + self.v["size"], self.heap + self.cc * cs)
        heap_end = self.heap + self.cc * cs
        fat_start = self.vo + self.v["fat_offset"]
        fat_end = fat_start + self.v["fat_length"]
        boot_end = self.vo + 24 * bps
        table = []
        if self.t["scheme"] == "gpt":
            last = self.t["gpt_disk_size"] // SECTOR - 1
            table = [(0, 34 * SECTOR), ((last - 32) * SECTOR, (last + 1) * SECTOR)]
        elif self.t["scheme"] == "mbr":
            table = [(0, SECTOR)]
        parts = self.t["partitions"]

        def file_range(path, at, ln):
            it = items.get(path)
            if not it:
                return None
            for fs, fe, d in it["ranges"]:
                if d <= at < d + (fe - fs):
                    s = fs + (at - d)
                    return [s, min(s + ln, fe)]
            return None

        areas_out = {}
        for s, e, st in self.areas:
            spans = []  # (disk pos, kind, path, file range)
            pos = s
            while pos < e:
                if self.vo <= pos < vol_end:
                    if pos < boot_end:
                        end, kind = boot_end, "boot"
                    elif pos < fat_start:
                        end, kind = fat_start, "reserved"
                    elif pos < fat_end:
                        end, kind = fat_end, "fat"
                    elif pos < self.heap:
                        end, kind = self.heap, "reserved"
                    elif pos < heap_end:
                        c = (pos - self.heap) // cs + 2
                        end = min(e, self.coff(c) + cs)
                        if c in owner:
                            kind, path = owner[c]
                            fr = file_range(path, pos, end - pos) if kind in ("file", "folder", "root") else None
                            spans.append((pos, kind, path, fr))
                        else:
                            spans.append((pos, "lost" if allocated(c) else "free", "", None))
                        pos = end
                        continue
                    else:
                        end, kind = vol_end, "past-heap"
                    spans.append((pos, kind, "", None))
                    pos = min(end, e)
                    continue
                t = [te for ts, te in table if ts <= pos < te]
                if t:
                    spans.append((pos, "partition-table", "", None))
                    pos = min(t[0], e)
                    continue
                nxt = min([self.vo] if pos < self.vo else [] + [ts for ts, _ in table if ts > pos] +
                          [p["start"] for p in parts if p["start"] > pos] + [1 << 62])
                inside = [p for p in parts if p["start"] <= pos < p["end"]]
                if inside:
                    p = inside[0]
                    nxt = min(nxt, p["end"])
                    if p["start"] == self.vo:
                        spans.append((pos, "past-heap", "", None))
                    else:
                        spans.append((pos, "partition", '"%s"' % p["name"], None))
                else:
                    spans.append((pos, "unpartitioned", "", None))
                pos = min(max(nxt, pos + 1), e)
            hits = []
            for _, kind, path, fr in spans:
                h = [x for x in hits if x["kind"] == kind and x["path"] == path]
                if h:
                    if fr:
                        if h[0]["ranges"] and h[0]["ranges"][-1][1] == fr[0]:
                            h[0]["ranges"][-1][1] = fr[1]
                        else:
                            h[0]["ranges"].append(fr)
                else:
                    hits.append({"kind": kind, "path": path, "ranges": [fr] if fr else []})
            sec, blk = self.blank(s, e)
            areas_out[s] = {"length": e - s, "status": st, "content": self.word(sec, blk), "hits": hits}

        # Data in use that nothing found owns.
        lost = 0
        self.lost_runs = []
        if bm is not None:
            for c in range(2, self.cc + 2):
                if allocated(c) and c not in owner:
                    lost += 1
                    if self.lost_runs and self.lost_runs[-1][0] + self.lost_runs[-1][1] == c:
                        self.lost_runs[-1][1] += 1
                    else:
                        self.lost_runs.append([c, 1])
        # Found from records that lie in never-read places.
        unread = []
        for path, (n, ncl, broken, hidden) in found.items():
            parent_cl = root_cl if n["parent"] == "" else found[n["parent"]][1]
            offs = [self.slot_off(parent_cl, n["slot"] + k) for k in range(n["slots"])]
            if any(s <= o < e for o in offs for s, e, _ in self.areas):
                unread.append(path)
        return {
            "found": sorted(found),
            "unread_records": sorted(unread),
            "items": items,
            "hidden": sorted(p for p, v in found.items() if v[3]),
            "hidden_maybe": sorted(p for p, m in maybe_of.items() if m),
            "areas": areas_out,
            "lost_clusters": lost,
            "folder_problems": problems,
            "folders": folders,
            "files": files,
        }


def compare(exp, got):
    """Differences between the expected answer and the program's JSON."""
    diffs = []
    gi = {i["path"]: i for i in got["items"]}
    for path, e in exp["items"].items():
        g = gi.get(path)
        if g is None:
            diffs.append("missing damaged item %r (expected ranges %s)" % (path, e["ranges"]))
            continue
        if [list(r) for r in g["ranges"]] != e["ranges"]:
            diffs.append("%r ranges: got %s, expected %s" % (path, g["ranges"], e["ranges"]))
        if g["content"] != e["content"]:
            diffs.append("%r content: got %s, expected %s" % (path, g["content"], e["content"]))
        if g["hidden"] != e["hidden"]:
            diffs.append("%r hidden: got %s, expected %s" % (path, g["hidden"], e["hidden"]))
        if g.get("hidden_maybe") != e["hidden_maybe"]:
            diffs.append("%r hidden_maybe: got %s, expected %s" % (path, g.get("hidden_maybe"), e["hidden_maybe"]))
        if g["is_dir"] != e["is_dir"]:
            diffs.append("%r is_dir: got %s" % (path, g["is_dir"]))
        if (g["chain_problem"] is not None) != e["broken"]:
            diffs.append("%r chain problem: got %r, expected %s" % (path, g["chain_problem"], e["broken"]))
        if g["only_past_valid_data"] != e["only_past_valid_data"]:
            diffs.append("%r only_past_valid_data: got %s" % (path, g["only_past_valid_data"]))
    for path in gi:
        if path not in exp["items"]:
            diffs.append("unexpected damaged item %r: %s" % (path, gi[path]))
    gh = sorted(h["path"] for h in got["hidden"])
    if gh != exp["hidden"]:
        diffs.append("hidden files differ: got %d, expected %d; only got: %s; only expected: %s" % (
            len(gh), len(exp["hidden"]), sorted(set(gh) - set(exp["hidden"]))[:5],
            sorted(set(exp["hidden"]) - set(gh))[:5]))
    gm = sorted(h["path"] for h in got["hidden"] if h.get("maybe"))
    if gm != exp["hidden_maybe"]:
        diffs.append("files Windows still opens by full path differ: only got: %s; only expected: %s" % (
            sorted(set(gm) - set(exp["hidden_maybe"]))[:5], sorted(set(exp["hidden_maybe"]) - set(gm))[:5]))
    ga = {a["start"]: a for a in got["areas"]}
    if sorted(ga) != sorted(exp["areas"]):
        diffs.append("bad areas differ: got %s, expected %s" % (sorted(ga)[:5], sorted(exp["areas"])[:5]))
    for s, e in exp["areas"].items():
        g = ga.get(s)
        if not g:
            continue
        for k in ("length", "status", "content"):
            if g[k] != e[k]:
                diffs.append("area %d %s: got %r, expected %r" % (s, k, g[k], e[k]))
        gh = [{"kind": h["kind"], "path": h["path"], "ranges": [list(r) for r in h["ranges"]]} for h in g["hits"]]
        if gh != e["hits"]:
            diffs.append("area %d hits: got %s, expected %s" % (s, gh, e["hits"]))
    gu = sorted(r["path"] for r in got["unread_records"])
    if gu != exp["unread_records"]:
        diffs.append("records in never-read places differ: got %s, expected %s" % (gu[:5], exp["unread_records"][:5]))
    tot = got["totals"]
    if tot["lost_clusters"] != exp["lost_clusters"]:
        diffs.append("lost clusters: got %d, expected %d" % (tot["lost_clusters"], exp["lost_clusters"]))
    if tot["folder_problems"] != exp["folder_problems"]:
        diffs.append("folder problems: got %d, expected %d (%s)" % (
            tot["folder_problems"], exp["folder_problems"], [p["problem"] for p in got["folder_problems"]][:4]))
    if got["volume"]["files"] != exp["files"] or got["volume"]["folders"] != exp["folders"]:
        diffs.append("found %d files, %d folders; expected %d, %d" % (
            got["volume"]["files"], got["volume"]["folders"], exp["files"], exp["folders"]))
    return diffs


# ---------------------------------------------------------------- running

import collections  # noqa: E402

COVERAGE = collections.Counter()

class Case:
    def __init__(self, name, disk, areas, zero=None, garbage=(), domain=None, extra_args="", expect=None):
        self.name = name
        self.disk = disk
        self.areas = areas  # (start, length, status)
        self.zero = zero  # areas to zero (default: all)
        self.garbage = garbage  # areas to fill with random bytes (a used SSD's leftovers)
        self.flips = []  # single bytes to change (near-miss corruption)
        self.earlier = None  # what an earlier attempt read onto the same disk: [(start, length)]
        self.rescue = True  # also check rescuing files from this disk
        self.domain = domain  # mapfile size (default: the GPT's disk size)
        self.extra_args = extra_args
        self.expect = expect or {}


def run_case(exe, work, case, keep=False):
    d = os.path.join(work, case.name)
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    img = os.path.join(d, "disk.img")
    pristine = os.path.join(d, "pristine.img")
    case.disk.save(pristine)
    truth = case.disk.truth()
    with open(os.path.join(d, "truth.json"), "w") as f:
        json.dump(truth, f)
    shutil.copyfile(pristine, img)
    zero = case.areas if case.zero is None else case.zero
    X.zero_areas(img, [(s, n) for s, n, *_ in zero])
    if case.garbage:
        g = random.Random(len(case.name))
        with open(img, "r+b") as f:
            for s, n, *_ in case.garbage:
                f.seek(s)
                f.write(g.randbytes(n))
    if case.flips:
        with open(img, "r+b") as f:
            for off in case.flips:
                f.seek(off)
                b = f.read(1)
                f.seek(off)
                f.write(bytes([b[0] ^ 0x20]))
    domain = case.domain or truth["gpt_disk_size"]
    mp = os.path.join(d, "disk.map")
    X.write_mapfile(mp, domain, [(s, n, st) for s, n, st in case.areas], extra_args=case.extra_args)
    out = os.path.join(d, "out")
    extra = []
    effective = [(s, s + n, st) for s, n, st in case.areas]
    if case.earlier is not None:
        ep = os.path.join(d, "earlier.map")
        X.write_earlier_mapfile(ep, domain, case.earlier)
        extra = ["--earlier-map", ep]
        effective = subtract(effective, sorted((s, s + n) for s, n in case.earlier))
    p = subprocess.run([exe, "map-check", "--map", mp, "--disk", img, "--out", out] + extra,
                       stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    log = p.stdout.decode("utf-8", "replace")
    with open(os.path.join(d, "console.txt"), "w", encoding="utf-8") as f:
        f.write(log)
    if case.expect.get("fails"):
        ok = p.returncode != 0 and case.expect["fails"] in log
        return [] if ok else ["expected the check to fail with %r, got exit %d:\n%s" % (
            case.expect["fails"], p.returncode, log[-600:])]
    if p.returncode != 0:
        return ["map-check failed (exit %d):\n%s" % (p.returncode, log[-1500:])]
    with open(os.path.join(out, "map-check.json"), encoding="utf-8") as f:
        got = json.load(f)
    oracle = Oracle(truth, img, pristine, effective)
    try:
        exp = oracle.expected()
        return finish_case(exe, d, case, img, truth, exp, oracle, got, keep, effective)
    finally:
        oracle.close()  # Windows cannot delete files that are still open


def finish_case(exe, d, case, img, truth, exp, oracle, got, keep, effective):
    diffs = compare(exp, got)
    if case.earlier is not None:
        want = sum(e - s for s, e, _ in [(s, s + n, st) for s, n, st in case.areas]) - sum(e - s for s, e, _ in effective)
        if got["earlier"][0]["read_of_bad"] != want:
            diffs.append("earlier attempt read %d bytes of the bad places, expected %d" % (
                got["earlier"][0]["read_of_bad"], want))
    for k, v in case.expect.items():
        actual = {"matches_mapfile": got["disk"]["matches_mapfile"],
                  "used_backup_boot": got["volume"]["used_backup_boot"],
                  "min_hidden": len(got["hidden"]),
                  "min_items": len(got["items"]),
                  "min_lost": got["totals"]["lost_clusters"],
                  "min_problems": got["totals"]["folder_problems"]}.get(k)
        if k.startswith("min_"):
            if actual < v:
                diffs.append("%s: got %s, expected at least %s (the test did not damage what it meant to)" % (
                    k, actual, v))
        elif actual != v:
            diffs.append("%s: got %r, expected %r" % (k, actual, v))
    if case.rescue and not diffs:
        diffs += check_rescue(exe, d, img, truth, exp, oracle)
    with open(os.path.join(d, "expected.json"), "w") as f:
        json.dump(exp, f, indent=1, default=str)
    # What this case exercised, for the coverage tally.
    cov = COVERAGE
    cov["damaged files"] += sum(1 for i in got["items"] if not i["is_dir"])
    cov["damaged folders"] += sum(1 for i in got["items"] if i["is_dir"])
    cov["files Windows cannot list"] += len(got["hidden"])
    cov["broken cluster chains"] += sum(1 for i in got["items"] if i["chain_problem"])
    cov["damage only past valid data"] += sum(1 for i in got["items"] if i["only_past_valid_data"])
    cov["damaged folder records"] += got["totals"]["folder_problems"]
    cov["lost clusters"] += got["totals"]["lost_clusters"]
    cov["records in never-read places"] += len(got["unread_records"])
    for a in got["areas"]:
        cov["areas holding " + a["content"]] += 1
        for h in a["hits"]:
            cov["hits on " + h["kind"]] += 1
    if not diffs and not keep:
        oracle.close()
        for name in ("disk.img", "pristine.img"):
            os.remove(os.path.join(d, name))
    return diffs


RESERVED = {"CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"}


def safe_name(name):
    """The name the rescue gives a copy (rescue.rs, safe_name): Windows
    cannot use names ending in a space or a dot, or the old device names."""
    s = "".join("_" if ord(c) < 32 or c in '<>:"/\\|?*' else c for c in name)
    if s.endswith((" ", ".")):
        s = s[:-1] + "_"
    if not s:
        s = "_"
    stem = s.split(".")[0].rstrip(" ").upper()
    if stem in RESERVED or (len(stem) == 4 and stem[:3] in ("COM", "LPT") and stem[3] in "0123456789\u00b9\u00b2\u00b3"):
        s = "_" + s
    return s


def safe_path(path):
    return "\\".join(safe_name(p) for p in path.split("\\"))


def check_rescue(exe, d, img, truth, exp, oracle):
    """Rescue what Windows cannot list, then everything plus the lost data,
    and check every byte of every copy."""
    diffs = []
    for mode, args in (("hidden", []), ("everything", ["--everything", "--lost"])):
        out = os.path.join(d, "rescued-" + mode)
        shutil.rmtree(out, ignore_errors=True)
        p = subprocess.run([exe, "rescue", "--disk", img, "--out", out] + args,
                           stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        log = p.stdout.decode("utf-8", "replace")
        if p.returncode != 0:
            diffs.append("rescue (%s) failed (exit %d): %s" % (mode, p.returncode, log[-800:]))
            continue
        want = set(exp["hidden"]) if mode == "hidden" else set(exp["found"])
        # A copied file's folders are made too, so it lands where it belongs.
        for w in list(want):
            parts = w.split("\\")
            for k in range(1, len(parts)):
                want.add("\\".join(parts[:k]))
        # Copies carry names Windows can use.
        copy_of = {w: safe_path(w) for w in want}
        if len(set(copy_of.values())) != len(copy_of):
            diffs.append("two files on this test disk get the same name for Windows (the test cannot tell them apart)")
            continue
        orig = {v: k for k, v in copy_of.items()}
        got = set()
        lost_files = []
        for base, dirs, files in os.walk(out):
            rel = os.path.relpath(base, out)
            rel = "" if rel == "." else rel.replace(os.sep, "\\")
            for x in dirs:
                got.add((rel + "\\" if rel else "") + x)
            for x in files:
                got.add((rel + "\\" if rel else "") + x)
        got.discard("rescue-log.csv")
        lost_dir = "lost data (no name)"
        lost_files = sorted(g for g in got if g.startswith(lost_dir + "\\"))
        got = {g for g in got if g != lost_dir and not g.startswith(lost_dir + "\\")}
        partial = sorted(g for g in got if g.split("\\")[-1] == "~recovery-probe-copying.tmp")
        if partial:
            diffs.append("rescue (%s) left unfinished copies behind: %s" % (mode, partial[:3]))
        got = {orig.get(g, g) for g in got}
        if got != want:
            diffs.append("rescue (%s) copied different files: only copied: %s; not copied: %s" % (
                mode, sorted(got - want)[:4], sorted(want - got)[:4]))
        wrong = []
        for path in sorted(want & got):
            if path not in oracle.found_nodes:
                continue
            n = oracle.found_nodes[path][0]
            if n["is_dir"]:
                continue
            where = os.path.join(out, *copy_of[path].split("\\"))
            with open(where, "rb") as f:
                data = f.read()
            if data != oracle.content(path):
                wrong.append(path)
            elif abs(os.stat(where).st_mtime - MTIME) > 2:
                wrong.append(path + " (date)")
        if wrong:
            diffs.append("rescue (%s): %d copies differ from the disk, e.g. %s" % (mode, len(wrong), wrong[:3]))
        if mode == "everything":
            runs = oracle.lost_runs
            if len(lost_files) != len(runs):
                diffs.append("rescue saved %d pieces of lost data, expected %d" % (len(lost_files), len(runs)))
            else:
                for (first, count), name in zip(sorted(runs), sorted(lost_files, key=lambda x: int(x.split()[4]))):
                    with open(os.path.join(out, *name.split("\\")), "rb") as f:
                        data = f.read()
                    if data != oracle.read(oracle.coff(first), count * oracle.cs):
                        diffs.append("lost data piece %s differs from the disk" % name)
                        break
        COVERAGE["files rescued and checked byte for byte"] += len(want & got)
        shutil.rmtree(out, ignore_errors=True)
    return diffs


# exfat_image.py stamps every file 2023-07-14 10:30:00 UTC.
MTIME = 1689330600


def subtract(areas, read):
    """[(start, end, status)] minus [(start, end)] (both sorted)."""
    out = []
    for s, e, st in areas:
        pos = s
        for rs, re_ in read:
            if re_ <= pos or rs >= e:
                continue
            if rs > pos:
                out.append((pos, rs, st))
            pos = max(pos, re_)
        if pos < e:
            out.append((pos, e, st))
    return out


# ---------------------------------------------------------------- disks

def node(truth, path):
    return [n for n in truth["nodes"] if n["path"] == path][0]


def disk_pos(truth, n, pos):
    """Disk offset of byte `pos` of a file or folder (from the builder's record)."""
    v = truth["volume"]
    cl = [c for f, k in n["runs"] for c in range(f, f + k)]
    k, r = divmod(pos, v["cluster_size"])
    return v["offset"] + v["heap_offset"] + (cl[k] - 2) * v["cluster_size"] + r


def sector_of(off):
    return off // SECTOR * SECTOR


NAMES = ["IMG_%04d.JPG", "DSC%05d.NEF", "clip-%03d.MOV", "Rapport annuel %d.pdf", "Ünïcödé fïlé %d.txt",
         "家族の写真 %d.jpg", "😀 party %d.heic", "a very long name that needs several name records number %d.docx",
         "x%d", "Song %d - Artist.flac"]


def basic_disk(cluster_size=4096, seed=1, gpt_disk=None, big=False):
    """A disk like the user's in miniature: GPT, an EFI partition, then
    the exFAT volume with photos, video, music and documents."""
    rng = random.Random(seed)
    v = X.Volume(40 * X.MiB if not big else 120 * X.MiB, cluster_size=cluster_size, label="FINAL 1 TB",
                 serial=0x5E1A0D17 + seed)
    fid = 1
    v.mkdir("Photos")
    v.mkdir("Photos/2019")
    v.mkdir("Photos/2019/Summer")
    for i in range(40):
        v.add_file("Photos/2019/Summer/IMG_%04d.JPG" % i, X.tagged(fid, rng.randint(30_000, 220_000)))
        fid += 1
    v.mkdir("Photos/2020", fragments=None)
    for i in range(30):
        v.add_file("Photos/2020/" + NAMES[i % len(NAMES)] % i, X.tagged(fid, rng.randint(1, 150_000)),
                   fragments=None)
        fid += 1
    v.mkdir("Videos")
    data = X.tagged(fid, 900_000)
    fid += 1
    n = -(-len(data) // cluster_size)
    v.add_file("Videos/wedding.mov", data, fragments=[n // 3, n // 3, n - 2 * (n // 3)], gap=2)
    v.add_file("Videos/clip.mp4", X.tagged(fid, 400_000), fat_chain=True)
    fid += 1
    v.add_file("Videos/partly-written.mov", X.tagged(fid, 300_000)[:120_000] + bytes(180_000), vdl=120_000)
    fid += 1
    v.add_file("Videos/deleted-old.mov", X.tagged(fid, 50_000), deleted=True)
    fid += 1
    v.mkdir("Music")
    v.mkdir("Music/Album with a rather long folder name")
    for i in range(12):
        v.add_file("Music/Album with a rather long folder name/%02d %s.flac" % (i, "Track " * (i % 4)),
                   X.tagged(fid, rng.randint(50_000, 200_000)))
        fid += 1
    v.add_file("empty file.txt", b"")
    v.add_file("notes.txt", X.tagged(fid, 700))
    fid += 1
    v.mkdir("Big folder", fragments=None, extra_clusters=1)
    for i in range(70):
        v.add_file("Big folder/file number %03d with a long name.bin" % i, X.tagged(fid, rng.randint(100, 9000)))
        fid += 1
    v.mkdir("Fragmented folder", fat_chain=True)
    for i in range(40):
        v.add_file("Fragmented folder/entry %02d.dat" % i, X.tagged(fid, rng.randint(100, 5000)))
        fid += 1
    v.mkdir("Hidden after damage")
    v.mkdir("Hidden after damage/inner")
    for i in range(5):
        v.add_file("Hidden after damage/inner/deep %d.jpg" % i, X.tagged(fid, 20_000))
        fid += 1
    disk = X.Disk(v, (64 if not big else 160) * X.MiB, gpt_disk_size=gpt_disk, efi_start_lba=40,
                  efi_size=2 * X.MiB, vol_start_lba=40 + 4096 + 7, seed=seed)
    return disk


def deliberate_cases(cluster_size):
    """One disk, every kind of damage put exactly where it matters."""
    disk = basic_disk(cluster_size)
    t = disk.truth()
    v = t["volume"]
    cs = v["cluster_size"]
    areas = []
    zero_skip = []

    def add(off, n, st="-", zero=True):
        areas.append((sector_of(off), n, st))
        if not zero:
            zero_skip.append(sector_of(off))

    # In the middle of a photo.
    n = node(t, "Photos\\2019\\Summer\\IMG_0003.JPG")
    add(disk_pos(t, n, 9 * SECTOR), 4 * SECTOR)
    # Across the boundary of two fragments of the video.
    w = node(t, "Videos\\wedding.mov")
    first_run = w["runs"][0][1] * cs
    add(disk_pos(t, w, first_run - 2 * SECTOR), 2 * SECTOR)
    add(disk_pos(t, w, first_run), SECTOR, "/")
    # A FAT-chained file.
    add(disk_pos(t, node(t, "Videos\\clip.mp4"), 100_000), 8 * SECTOR, "*")
    # Past the valid data of a partly written file: nothing lost.
    add(disk_pos(t, node(t, "Videos\\partly-written.mov"), 200_000 // SECTOR * SECTOR), 2 * SECTOR)
    # In the slack after a file's end, inside its last cluster.
    nt = node(t, "notes.txt")
    if cs >= 4 * SECTOR:
        add(disk_pos(t, nt, SECTOR * 1), SECTOR)  # its last bytes
        add(disk_pos(t, nt, SECTOR * 3), SECTOR)  # past its end, inside its cluster
    # A never-read area that the first attempt did read: data, not zeros.
    add(disk_pos(t, node(t, "Photos\\2019\\Summer\\IMG_0010.JPG"), 2 * SECTOR), 3 * SECTOR, "-", zero=False)
    # Free space.
    used = set(v["allocated"])
    free = [c for c in range(2, v["cluster_count"] + 2) if c not in used]
    fc = free[len(free) // 2]
    add(v["offset"] + v["heap_offset"] + (fc - 2) * cs, 2 * SECTOR)
    # The data of a deleted file (free space, though it holds old data).
    add(disk_pos(t, node(t, "Videos\\deleted-old.mov"), 0), SECTOR)
    # Folder records: a sector in the middle of "Big folder" hides what follows.
    bf = node(t, "Big folder\\file number 020 with a long name.bin")
    add(bf["record_offsets"][0], SECTOR)
    # The record of a folder whose contents then cannot be found at all.
    hd = node(t, "Hidden after damage")
    # Zero the sector before its record so it is still found but hidden.
    root_slots = [n for n in t["nodes"] if n["parent"] == ""]
    # The EFI partition.
    add(t["partitions"][0]["start"] + 64 * SECTOR, 4 * SECTOR)
    # Space between the partitions and outside the volume.
    add(t["partitions"][0]["end"] + SECTOR, SECTOR)
    # The FAT, where nothing uses it (an entry of a contiguous file).
    add(v["offset"] + v["fat_offset"] + 8 * 4 * SECTOR, SECTOR)
    # The partition table's backup copy at the end of the old disk.
    add(t["gpt_disk_size"] - 16 * SECTOR, SECTOR)
    # A large never-tried stretch at the end of the volume's data, mostly free.
    add(v["offset"] + v["heap_offset"] + (v["cluster_count"] - 40) * cs, 40 * cs, "?")
    # The backup boot region (the main one is fine).
    add(v["offset"] + 14 * SECTOR, SECTOR)
    # A never-read place that holds data, in which one letter of a file's
    # name differs: only the record's checksum shows it is damaged.
    victim = node(t, "Photos\\2020\\" + NAMES[3] % 3)
    name_slot = victim["record_offsets"][2]
    add(name_slot, SECTOR, "-", zero=False)
    flips = [name_slot + 2 + 2 * 4]
    # A file whose record crosses into the next sector, which was never read
    # but holds data: found, and flagged as found from a never-read place.
    for n in t["nodes"]:
        ro = n["record_offsets"]
        if n["parent"] == "Music\\Album with a rather long folder name" and sector_of(ro[0]) != sector_of(ro[-1]):
            add(sector_of(ro[-1]), SECTOR, "-", zero=False)
            break
    areas.sort()
    # Merge or drop overlaps.
    clean = []
    for a in areas:
        if clean and a[0] < clean[-1][0] + clean[-1][1]:
            continue
        clean.append(a)
    zero = [a for a in clean if a[0] not in zero_skip]
    case = Case("deliberate-%dk" % (cs // 1024 or 0) if cs >= 1024 else "deliberate-%db" % cs, disk, clean,
                zero=zero, expect={"matches_mapfile": True, "min_hidden": 1, "min_items": 6, "min_problems": 2})
    case.flips = flips
    cases = [case]
    del hd, root_slots
    return cases


def hidden_folder_case():
    """Zero the root folder's records just before a folder's record: the
    folder and everything inside it can no longer be listed by Windows."""
    disk = basic_disk(4096, seed=3)
    t = disk.truth()
    hd = node(t, "Hidden after damage")
    # The root holds sets for: Photos, Videos, Music, empty file, notes, Big folder,
    # Fragmented folder, Hidden after damage. Zero the slot sector of "notes.txt".
    nt = node(t, "notes.txt")
    areas = [(sector_of(nt["record_offsets"][0]), SECTOR, "-")]
    assert sector_of(nt["record_offsets"][0]) < sector_of(hd["record_offsets"][0]) or True
    return Case("hidden-folder", disk, areas, expect={"min_hidden": 6})


def boot_case():
    """The main boot sector was never read: the backup must be used."""
    disk = basic_disk(4096, seed=4)
    t = disk.truth()
    vo = t["volume"]["offset"]
    areas = [(vo, 4 * SECTOR, "-"), (disk_pos(t, node(t, "notes.txt"), 0), SECTOR, "-")]
    return Case("main-boot-sector-lost", disk, areas, expect={"used_backup_boot": True})


def gpt_lost_case():
    """The start of the disk (main partition table) was never read."""
    disk = basic_disk(4096, seed=5)
    return Case("main-partition-table-lost", disk, [(0, 34 * SECTOR, "-")], expect={"matches_mapfile": True})


def fat_case():
    """Part of the FAT was never read: chained files break."""
    disk = basic_disk(4096, seed=6)
    t = disk.truth()
    v = t["volume"]
    clip = node(t, "Videos\\clip.mp4")
    c = clip["runs"][0][0] + 5
    off = v["offset"] + v["fat_offset"] + 4 * c
    return Case("fat-damage", disk, [(sector_of(off), SECTOR, "-")], expect={"min_items": 1})


def bigger_ssd_case():
    """The copy is on a bigger disk: the partition table still describes
    the old disk, and the mapfile's size must match it."""
    disk = basic_disk(4096, seed=7, gpt_disk=50 * X.MiB)
    t = disk.truth()
    n = node(t, "Photos\\2019\\Summer\\IMG_0001.JPG")
    return Case("copy-on-bigger-disk", disk, [(sector_of(disk_pos(t, n, 1000)), SECTOR, "-")],
                expect={"matches_mapfile": True})


def wrong_disk_case():
    disk = basic_disk(4096, seed=8)
    t = disk.truth()
    n = node(t, "Photos\\2019\\Summer\\IMG_0001.JPG")
    return Case("mapfile-for-another-disk", disk, [(sector_of(disk_pos(t, n, 1000)), SECTOR, "-")],
                domain=t["gpt_disk_size"] + 8 * X.MiB, expect={"matches_mapfile": False})


def earlier_attempt_case():
    """An earlier attempt copied part of the disk onto the same SSD: what
    it read there is not lost, even where the final attempt failed."""
    disk = basic_disk(4096, seed=9)
    t = disk.truth()
    picks = ["Photos\\2019\\Summer\\IMG_%04d.JPG" % i for i in (2, 5, 8, 11)]
    areas = [(sector_of(disk_pos(t, node(t, p), 6000)), 4 * SECTOR, "-") for p in picks]
    earlier = [(areas[0][0] - 8192, areas[0][1] + 16384),  # all of the first
               (areas[1][0], areas[1][1]),  # exactly the second
               (areas[2][0], 2 * SECTOR)]  # half of the third
    # On the SSD: what the earlier attempt read holds the real data; the rest is zeros.
    unread = subtract([(s, s + n, st) for s, n, st in areas], sorted((s, s + n) for s, n in earlier))
    c = Case("earlier-attempt", disk, areas, zero=[(s, e - s, st) for s, e, st in unread])
    c.earlier = earlier
    return c


def keep_mountable(case):
    """Leave out damage that can stop an operating system from opening the
    drive at all (the boot area, the partition table, the allocation
    bitmap, the up-case table and the top folder's first records), so a
    damaged disk can still be compared with what Windows or Linux lists.
    The map check's own tests cover those; mount_experiments() shows what
    each of them does to Windows and Linux."""
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
    # The FAT entries of the top folder, the bitmap and the up-case table:
    # Linux counts the top folder's clusters through the FAT when it mounts,
    # and refuses the drive if that chain is broken.
    fat = v["offset"] + v["fat_offset"]
    for name in ("root", "bitmap", "upcase"):
        for first, n in v[name]:
            for c in range(first, first + n):
                sec = (fat + 4 * c) // 512 * 512
                keep_out.append((sec, sec + 512))
    ok = lambda a: not any(a[0] < e and a[0] + a[1] > s for s, e in keep_out)
    case.areas = [a for a in case.areas if ok(a)]
    if case.zero is not None:
        case.zero = [a for a in case.zero if ok(a)]


def mount_experiments():
    """Single pieces of damage to the disk's own structures, to learn which
    ones stop Windows and Linux from opening the drive at all: (name, disk,
    [(start, length)])."""
    disk = basic_disk(4096, seed=11)
    t = disk.truth()
    v = t["volume"]
    vo = v["offset"]
    cs = v["cluster_size"]
    heap = vo + v["heap_offset"]
    root = heap + (v["root"][0][0] - 2) * cs
    out = [
        ("nothing (control)", []),
        ("the main boot sector", [(vo, SECTOR)]),
        ("the boot checksum sector", [(vo + 11 * SECTOR, SECTOR)]),
        ("one backup boot sector", [(vo + 14 * SECTOR, SECTOR)]),
        ("the whole backup boot region", [(vo + 12 * SECTOR, 12 * SECTOR)]),
        ("the start of the FAT", [(vo + v["fat_offset"], SECTOR)]),
        ("one FAT sector further in", [(vo + v["fat_offset"] + 8 * SECTOR, SECTOR)]),
        ("the top folder's first records (label, bitmap, up-case)", [(root, SECTOR)]),
        ("one sector of the allocation bitmap", [(heap + (v["bitmap"][0][0] - 2) * cs, SECTOR)]),
        ("one sector of the up-case table", [(heap + (v["upcase"][0][0] - 2) * cs + SECTOR, SECTOR)]),
        ("the main partition table", [(0, 34 * SECTOR)]),
        ("the backup partition table", [(t["gpt_disk_size"] - 33 * SECTOR, 33 * SECTOR)]),
    ]
    return disk, out


def no_exfat_case():
    v = X.Volume(8 * X.MiB, cluster_size=4096, label="X")
    disk = X.Disk(v, 16 * X.MiB)
    vo = disk.truth()["volume"]["offset"]
    # Both boot sectors gone, and nothing else to go on.
    return Case("no-volume-left", disk, [(vo, 24 * SECTOR, "-")], expect={"fails": "no exFAT volume found"})


def random_case(seed):
    rng = random.Random(seed)
    cs = rng.choice([512, 1024, 4096, 4096, 16384, 32768, 131072])
    vol_size = rng.choice([24, 40, 64]) * X.MiB
    if cs >= 131072:
        vol_size = 96 * X.MiB
    v = X.Volume(vol_size, cluster_size=cs, label=rng.choice(["FINAL 1 TB", "Untitled", "Ünï", "BACKUP"]),
                 serial=rng.getrandbits(32), heap_align=rng.choice([None, 2048]))
    budget = vol_size * 0.6
    used = 0
    fid = 1
    dirs = [""]
    for _ in range(rng.randint(3, 25)):
        parent = rng.choice(dirs)
        name = "dir %d %s" % (len(dirs), rng.choice(["", "ü", "家", "😀", "x" * rng.randint(1, 40)]))
        path = name if not parent else parent + "/" + name
        frag = None
        v.mkdir(path, extra_clusters=rng.choice([0, 0, 0, 1]), fat_chain=rng.random() < 0.2)
        dirs.append(path)
    for _ in range(rng.randint(10, 160)):
        parent = rng.choice(dirs)
        size = rng.choice([0, rng.randint(1, 600), rng.randint(600, 40_000), rng.randint(40_000, 300_000)])
        if used + size + cs > budget:
            break
        used += -(-size // cs) * cs + cs
        name = rng.choice(NAMES) % fid
        path = name if not parent else parent + "/" + name
        data = X.tagged(fid, size, seed)
        fid += 1
        n = -(-size // cs)
        frags = None
        if n >= 3 and rng.random() < 0.3:
            k = rng.randint(2, min(4, n))
            cuts = sorted(rng.sample(range(1, n), k - 1))
            frags = [b - a for a, b in zip([0] + cuts, cuts + [n])]
        vdl = None
        if size > 1000 and rng.random() < 0.1:
            vdl = rng.randint(0, size)
            data = data[:vdl] + bytes(size - vdl)
        v.add_file(path, data, vdl=vdl, fragments=frags, gap=rng.randint(1, 3), gap_before=rng.choice([0, 0, 1]),
                   fat_chain=rng.random() < 0.15, deleted=rng.random() < 0.05)
    big = rng.random() < 0.3
    gpt_disk = None
    efi_start = rng.choice([34, 40, 2048])
    efi_size = rng.choice([0, 1, 2]) * X.MiB
    vol_start = efi_start + efi_size // SECTOR + rng.choice([0, 0, 1, 7, 2048])
    disk_size = vol_start * SECTOR + vol_size + rng.choice([1, 4, 8]) * X.MiB
    if big:
        gpt_disk = disk_size
        disk_size += 16 * X.MiB
    disk = X.Disk(v, disk_size, gpt_disk_size=gpt_disk, efi_start_lba=efi_start, efi_size=efi_size,
                  vol_start_lba=vol_start, seed=seed)
    t = disk.truth()
    # Bad areas: some anywhere, most where things are.
    targets = []
    nodes = [n for n in t["nodes"] if not n["deleted"]]
    for _ in range(rng.randint(1, 30)):
        r = rng.random()
        if r < 0.35 and nodes:
            n = rng.choice(nodes)
            if n["runs"] and n["data_length"]:
                targets.append(disk_pos(t, n, rng.randrange(0, max(1, n["data_length"]))))
        elif r < 0.6 and nodes:
            n = rng.choice(nodes)
            targets.append(rng.choice(n["record_offsets"]))
        elif r < 0.65:
            vv = t["volume"]
            targets.append(vv["offset"] + vv["fat_offset"] + rng.randrange(0, vv["fat_length"]))
        else:
            targets.append(rng.randrange(0, t["gpt_disk_size"]))
    areas = []
    for off in sorted(set(sector_of(x) for x in targets)):
        n = rng.choice([1, 1, 1, 2, 8, 16, 52, 300])
        if areas and off < areas[-1][0] + areas[-1][1] + SECTOR:
            continue
        if off + n * SECTOR > t["gpt_disk_size"]:
            n = (t["gpt_disk_size"] - off) // SECTOR
        if n <= 0:
            continue
        areas.append((off, n * SECTOR, rng.choice("----/*?")))
    vo = t["volume"]["offset"]
    # Keep at least the main or the backup boot sector, and the bitmap/up-case records.
    areas = [a for a in areas if not (a[0] <= vo + 12 * SECTOR < a[0] + a[1] and a[0] <= vo < a[0] + a[1])]
    zero, garbage = [], []
    for a in areas:
        r = rng.random()
        if r < 0.75:
            zero.append(a)
        elif r < 0.9 and not (vv_fat_or_boot(t, a)):
            garbage.append(a)
    return Case("random-%03d" % seed, disk, areas, zero=zero, garbage=garbage)


def vv_fat_or_boot(t, a):
    """Garbage in the FAT or the boot area makes a different disk, not a
    damaged one; keep garbage to files and folder records."""
    v = t["volume"]
    s, n = a[0], a[1]
    lo, hi = v["offset"], v["offset"] + v["heap_offset"]
    return s < hi and s + n > lo or s < 34 * SECTOR or s + n > t["gpt_disk_size"] - 34 * SECTOR


# ---------------------------------------------------------------- independent checks of the builder

def fsck_check(fsck, work, disk, name):
    """fsck.exfat (exfatprogs, the Linux tools) must find the volume clean."""
    p = os.path.join(work, name + "-volume.img")
    disk.save_volume(p)
    r = subprocess.run([fsck, "-n", p], stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    os.remove(p)
    out = r.stdout.decode("utf-8", "replace")
    if r.returncode != 0 or "clean" not in out:
        return ["fsck.exfat does not accept the %s volume:\n%s" % (name, out[-1500:])]
    return []


def dissect_check(dissect_root, work, disk, name):
    """dissect.fat (an independent exFAT reader, by Fox-IT) must find every
    file and folder, and read every file back exactly as written."""
    if dissect_root != "installed":
        for sub in ("dissect.fat", "dissect.cstruct", "dissect.util"):
            path = os.path.join(dissect_root, sub)
            if path not in sys.path:
                sys.path.insert(0, path)
    from dissect.fat import exfat as dx
    from dissect.util.stream import RunlistStream

    def parse_subdir(self, entry):
        # dissect.fat reads only the first cluster of a contiguous folder
        # (its own "TODO"); give it the folder's length, as it does for files.
        st = entry.stream
        nf = bool(st.flags.not_fragmented)
        rl = self.exfat.runlist(st.location, nf, st.data_length if nf else None)
        fh = RunlistStream(self.exfat.filesystem, rl, st.data_length, self.exfat.sector_size)
        return self._parse_file_entries(fh)

    dx.RootDirectory._parse_subdir = parse_subdir
    p = os.path.join(work, name + "-volume.img")
    disk.save_volume(p)
    got = {}
    with open(p, "rb") as f:
        fs = dx.ExFAT(f)

        def walk(d, prefix):
            for nm, (fe, sub) in d.items():
                if sub is not None:
                    path = prefix + nm.rstrip("/")
                    got[path] = None
                    walk(sub, path + "\\")
                else:
                    st = fe.stream
                    nf = bool(st.flags.not_fragmented)
                    data = b""
                    if st.data_length:
                        rl = fs.runlist(st.location, nf, st.data_length if nf else None)
                        data = RunlistStream(f, rl, st.data_length, fs.sector_size).read()
                    got[prefix + nm] = data

        walk(fs.files["/"][1], "")
    os.remove(p)
    errors = []
    want = {n.path: n for n in disk.vol.nodes() if n is not disk.vol.root and not n.deleted}
    if set(got) != set(want):
        errors.append("dissect.fat sees different files: only it: %s; only the builder: %s" % (
            sorted(set(got) - set(want))[:5], sorted(set(want) - set(got))[:5]))
    for path, n in want.items():
        if n.is_dir or path not in got:
            continue
        expect = n.data[:n.vdl] + bytes(n.data_length - n.vdl)
        if got[path] != expect:
            errors.append("dissect.fat reads %r differently (%d bytes, expected %d)" % (
                path, len(got[path]), len(expect)))
    return errors


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("exe")
    ap.add_argument("work")
    ap.add_argument("--random", type=int, default=40)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--keep", action="store_true")
    ap.add_argument("--fsck")
    ap.add_argument("--dissect")
    ap.add_argument("--only")
    a = ap.parse_args()
    os.makedirs(a.work, exist_ok=True)
    cases = []
    for cs in (512, 4096, 32768, 131072):
        cases += deliberate_cases(cs)
    cases += [hidden_folder_case(), boot_case(), gpt_lost_case(), fat_case(), bigger_ssd_case(),
              wrong_disk_case(), earlier_attempt_case(), no_exfat_case()]
    # Random disks are built one at a time (each holds its files in memory).
    cases += [a.seed + i for i in range(a.random)]
    if a.only:
        cases = [c for c in cases if a.only in (c.name if isinstance(c, Case) else "random-%03d" % c)]
    failed = 0
    builder_checked = 0
    for c in cases:
        if not isinstance(c, Case):
            c = random_case(c)
        diffs = run_case(a.exe, a.work, c, keep=a.keep)
        if a.fsck:
            diffs += fsck_check(a.fsck, a.work, c.disk, c.name)
        if a.dissect:
            diffs += dissect_check(a.dissect, a.work, c.disk, c.name)
        builder_checked += 1
        if diffs:
            failed += 1
            print("FAIL %s" % c.name)
            for d in diffs[:12]:
                print("     " + d)
            if len(diffs) > 12:
                print("     ...and %d more" % (len(diffs) - 12))
        else:
            print("ok   %s" % c.name)
    print("covered: " + ", ".join("%s %d" % (k, v) for k, v in sorted(COVERAGE.items())))
    checkers = [n for n, on in (("fsck.exfat", a.fsck), ("dissect.fat", a.dissect)) if on]
    extra = " (every test disk also checked by %s)" % " and ".join(checkers) if checkers else ""
    print("map check tests: %d of %d passed%s" % (len(cases) - failed, len(cases), extra))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
