#!/usr/bin/env python3
"""Build test files for comparing the Rust probe with the Python reference.

    python tools/make_fixtures.py edge   <out_dir>
        Hand-made edge cases: every fixture from probe_selftest.py PART 3b,
        housekeeping files, awkward names, Photos Libraries (by name, by
        shape, nested, with a hot -wal), healthy and damaged databases.

    python tools/make_fixtures.py mutate <seed_dir> <out_dir> [--count N] [--seed S]
        Damaged copies of real files, the ways ddrescue damages them: holes
        of whole zeroed sectors, cut-off endings, zeroed endings and
        beginnings, natural zero runs that are NOT sector-shaped, garbage
        patches, and extra data after the end.

Only the Python standard library is used, so this runs on any CI machine.
Everything is deterministic for a given --seed.
"""
import os
import random
import sqlite3
import struct
import sys
import zlib

NZ = b"\xAB\xCD\xEF\x12" * 512          # 2 KiB of non-zero bytes (no FF, no 00)
HOLE = b"\x00" * 8192
TRAIL = b"\x00" * 12288


def sect(b):
    return b + b"\x5A" * (-len(b) % 512)


def write(d, name, data):
    p = os.path.join(d, name)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "wb") as fh:
        fh.write(data)
    return p


def box(typ, payload=b""):
    return struct.pack(">I", 8 + len(payload)) + typ + payload


def ebml(eid, payload):
    return eid + b"\x01" + len(payload).to_bytes(7, "big") + payload


def riff(form, body_after_header):
    data = bytearray(body_after_header)
    data[0:12] = b"RIFF" + struct.pack("<I", len(data) - 8) + form
    return bytes(data)


def flac(blocks, audio):
    out = b"fLaC"
    for i, (typ, payload) in enumerate(blocks):
        last = 0x80 if i == len(blocks) - 1 else 0
        out += bytes([last | typ]) + len(payload).to_bytes(3, "big") + payload
    return out + audio


def id3(total, pad_from=512):
    payload = b"\x03" + b"T" * (pad_from - 10 - 10 - 1)
    frames = b"TIT2" + struct.pack(">I", len(payload)) + b"\x00\x00" + payload
    n = total - 10
    body = frames + b"\x00" * (n - len(frames))
    size = bytes([(n >> 21) & 0x7F, (n >> 14) & 0x7F, (n >> 7) & 0x7F, n & 0x7F])
    return b"ID3\x03\x00\x00" + size + body


def png(with_end=True, after=b""):
    def chunk(typ, data):
        return struct.pack(">I", len(data)) + typ + data + b"\x11\x22\x33\x44"
    out = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", b"\x01" * 13) + chunk(b"IDAT", NZ)
    return out + (b"\x00\x00\x00\x00IEND\xaeB`\x82" if with_end else b"") + after


def make_db(path, rows=200, wal_copy_to=None):
    """A small Photos-shaped SQLite database. With wal_copy_to, the database
    is left in WAL mode and copied (with its -wal) while the WAL still holds
    data - a 'hot -wal', as an unclean shutdown leaves."""
    if os.path.exists(path):
        os.remove(path)
    con = sqlite3.connect(path)
    if wal_copy_to:
        con.execute("PRAGMA journal_mode=WAL")
        con.execute("PRAGMA wal_autocheckpoint=0")
    con.execute("CREATE TABLE ZASSET (Z_PK INTEGER PRIMARY KEY, ZFILENAME TEXT, ZDATE REAL)")
    con.execute("CREATE INDEX ZASSET_NAME ON ZASSET (ZFILENAME)")
    con.executemany("INSERT INTO ZASSET (ZFILENAME, ZDATE) VALUES (?, ?)",
                    [(f"IMG_{i:04d}.HEIC", 700000000.0 + i) for i in range(rows)])
    con.commit()
    if wal_copy_to:
        for suffix in ("", "-wal", "-shm"):
            if os.path.exists(path + suffix):
                with open(path + suffix, "rb") as src, open(wal_copy_to + suffix, "wb") as dst:
                    dst.write(src.read())
    con.close()


def edge(out):
    os.makedirs(out, exist_ok=True)
    # --- the probe_selftest.py PART 3b fixtures --------------------------------
    write(out, "a.gz", sect(b"\x1f\x8b" + NZ) + HOLE + NZ + TRAIL)
    write(out, "b.gz", sect(b"\x1f\x8b" + NZ) + TRAIL)
    write(out, "c.gz", sect(b"\x1f\x8b" + b"\xAB" * 3000) + b"\x00" * 6144 + b"\xAB" * 100)
    write(out, "d.webp", riff(b"WEBP", sect(b"\x00" * 12 + NZ) + HOLE + NZ))
    write(out, "e.wav", riff(b"WAVE", sect(b"\x00" * 12 + NZ) + HOLE + NZ))
    write(out, "night.gz", sect(b"\x1f\x8b" + NZ) + b"\x77" * 100 + HOLE + NZ)
    meta = [(0, b"\x12" * 34), (2, b"\x34" * 462), (1, b"\x00" * 8192)]
    write(out, "song.flac", flac(meta, b"\xff\xf8" + NZ * 4))
    write(out, "song-hole.flac", flac(meta, sect(b"\xff\xf8" + NZ) + HOLE + NZ))
    write(out, "tagged.flac", id3(1024) + flac(meta[:1], b"\xff\xf8" + NZ))
    write(out, "song.mp3", id3(10752) + b"\xff\xfb\x90\x64" + NZ * 3)
    write(out, "song-hole.mp3", id3(10752) + sect(b"\xff\xfb\x90\x64" + NZ) + HOLE + NZ)
    moov = box(b"moov", box(b"mvhd", b"\x01" * 100))
    ftyp504 = box(b"ftyp", b"isom" * 124)
    write(out, "clip.mp4", ftyp504 + box(b"free", b"\x00" * 8192) + box(b"mdat", NZ * 4) + moov)
    write(out, "clip-hole.mp4", ftyp504 + box(b"mdat", HOLE + NZ * 2) + moov)
    entry = struct.pack(">I", 36) + b"sowt" + b"\x01" * 28
    stsd = b"\x00\x00\x00\x00" + struct.pack(">I", 1) + entry
    pcm_moov = box(b"moov", box(b"trak", box(b"mdia", box(b"minf", box(b"stbl", box(b"stsd", stsd))))))
    head = box(b"ftyp", b"qt  \x00\x00\x00\x00qt  ") + pcm_moov
    fill = b"\x5A" * (-(len(head) + 8) % 512)
    write(out, "camera.mov", head + box(b"mdat", fill + b"\x00" * 16384 + NZ))
    write(out, "old.mov", moov + box(b"mdat", NZ * 2))
    write(out, "lost-index.mp4", ftyp504 + box(b"mdat", NZ * 4) + b"\x00" * 2048)
    write(out, "lost-index-long.mp4", ftyp504 + box(b"mdat", NZ * 4) + b"\x00" * 9000)
    ebml_hdr = ebml(b"\x1a\x45\xdf\xa3", b"\x42\x86\x81\x01")
    pre = ebml(b"\x15\x49\xa9\x66", b"\x5A" * (512 - 28 - 12 - 9))
    write(out, "clip.mkv", ebml_hdr + ebml(b"\x18\x53\x80\x67", pre + ebml(b"\xec", b"\x00" * 8192)
                                             + ebml(b"\x1f\x43\xb6\x75", NZ * 2)))
    whole = ebml_hdr + ebml(b"\x18\x53\x80\x67", pre + ebml(b"\x1f\x43\xb6\x75", NZ * 4))
    write(out, "cut.mkv", whole[:-3000])
    jh = (b"\xff\xd8" + b"\xff\xe0" + struct.pack(">H", 16) + b"JFIF\x00" + b"\x01" * 9
          + b"\xff\xda" + struct.pack(">H", 8) + b"\x01" * 6)
    write(out, "samsung.jpg", jh + NZ * 2 + b"\xff\xd9" + b"SEFH" + b"\x07" * 300 + b"SEFT")
    write(out, "cut.jpg", jh + NZ * 2)
    pic = jh + NZ * 2 + b"\xff\xd9"
    write(out, "motion.jpg", pic + b"\x5A" * (-len(pic) % 512) + b"\x00" * 8192 + NZ)
    write(out, "hole-trailer.jpg", jh + b"\x5A" * (512 - len(jh)) + HOLE + NZ + b"\xff\xd9" + b"SEFT")
    write(out, "restart-markers.jpg", jh + (b"\x22" * 100 + b"\xff\xd3") * 20 + b"\xff\x00" + b"\xff\xd9")
    for name, hdr in (("p.rw2", b"IIU\x00" + struct.pack("<I", 8)), ("o.orf", b"IIRO" + struct.pack("<I", 8)),
                      ("c.crw", b"II\x1a\x00\x00\x00HEAPCCDR"), ("k.raw", b"\x0070.1 .reV\x00"),
                      ("q.iiq", b"QQQQ")):
        write(out, name, hdr + NZ * 2)
    write(out, "z.nef", b"\x00" * 4096 + NZ * 2)
    write(out, "h.cr2", sect(b"II*\x00" + struct.pack("<I", 8) + NZ) + HOLE + NZ)
    # TIFF whose strip runs past the end of the file
    ifd = struct.pack("<H", 2) + struct.pack("<HHII", 273, 4, 1, 4000) + struct.pack("<HHII", 279, 4, 1, 900000)
    write(out, "short.tif", b"II*\x00" + struct.pack("<I", 8) + ifd + b"\x00\x00\x00\x00" + NZ)
    write(out, "short.dng", b"II*\x00" + struct.pack("<I", 8) + ifd + b"\x00\x00\x00\x00" + NZ)
    write(out, "badifd.tif", b"MM\x00*" + struct.pack(">I", 99999999) + NZ)
    write(out, "._IMG_0001.JPG", b"\x00\x05\x16\x07\x00\x02\x00\x00Mac OS X        " + b"\x00" * 100)
    ole = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1" + NZ
    write(out, "Thumbs.db", ole)
    write(out, "holiday.db", ole)
    write(out, "desktop.ini", b"[.ShellClassInfo]\r\n")
    write(out, ".DS_Store", b"\x00\x00\x00\x01Bud1" + NZ)
    write(out, "padded.png", png(after=b"\x00" * 600))
    write(out, "cut.png", png(with_end=False))
    write(out, "end-lost.mp3", sect(b"\xff\xfb\x90\x64" + NZ * 2) + b"\x00" * 8192)
    write(out, "end-fine.mp3", b"\xff\xfb\x90\x64" + NZ + b"\x00" * 300)
    write(out, "speed.gz", b"\x1f\x8b" + (b"\xAB" * 63 + b"\x00") * ((1 << 20) // 64))
    # --- simple formats, good and broken ---------------------------------------
    write(out, "ok.gif", b"GIF89a" + NZ + b";")
    write(out, "cut.gif", b"GIF89a" + NZ)
    write(out, "ok.pdf", b"%PDF-1.4\n" + NZ + b"\n%%EOF\n")
    write(out, "cut.pdf", b"%PDF-1.4\n" + NZ)
    write(out, "zerotail.pdf", b"%PDF-1.4\n" + NZ + b"\x00" * 2048)
    import zipfile, io
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        z.writestr("word/document.xml", "x" * 5000)
    write(out, "ok.docx", buf.getvalue())
    write(out, "cut.docx", buf.getvalue()[:-30])
    write(out, "ok.zip", buf.getvalue())
    write(out, "wrong-ext.jpg", buf.getvalue())
    write(out, "notjpeg.jpg", b"hello world, not a picture")
    write(out, "rar.rar", b"Rar!\x1a\x07\x00" + NZ)
    write(out, "seven.7z", b"7z\xbc\xaf\x27\x1c" + NZ)
    write(out, "bitmap.bmp", b"BM" + b"\x00" * 9000 + NZ)
    write(out, "short.wav", riff(b"WAVE", b"\x00" * 12 + NZ)[:-100] + b"")
    write(out, "declared-long.avi", b"RIFF" + struct.pack("<I", 999999) + b"AVI " + NZ)
    write(out, "ogg.ogg", b"OggS" + NZ * 2)
    write(out, "ogg-zero-end.ogg", sect(b"OggS" + NZ) + b"\x00" * 4608)
    write(out, "empty.jpg", b"")
    write(out, "one-byte.bin", b"\x01")
    write(out, "no extension", NZ)
    write(out, "README", b"plain text\n")
    write(out, "UPPER.JPG", b"\xff\xd8\xff" + NZ + b"\xff\xd9")
    write(out, "two.dots.tar.gz", b"\x1f\x8b" + NZ)
    write(out, "..cr2", NZ)
    write(out, ".hidden.jpg", b"\xff\xd8\xff" + NZ + b"\xff\xd9")
    # Windows does not allow " in a file name; Linux and macOS do.
    awkward = "comma, quote' and spaces.jpg" if os.name == "nt" else "comma, quote\" and spaces.jpg"
    write(out, awkward, b"\xff\xd8\xff" + NZ)
    write(out, "unicode ñ 写真 📷.jpg", b"\xff\xd8\xff" + NZ + b"\xff\xd9")
    write(out, "heic.heic", box(b"ftyp", b"heic" * 4) + box(b"meta", NZ) + box(b"mdat", NZ))
    write(out, "heic-hole.heic", box(b"ftyp", b"heic" * 4) + box(b"meta", NZ) + box(b"mdat", sect(NZ) + HOLE + NZ))
    write(out, "avif.avif", box(b"ftyp", b"avif" * 4) + box(b"meta", NZ))
    write(out, "ext64.mp4", box(b"ftyp", b"isom") + struct.pack(">I", 1) + b"mdat" + struct.pack(">Q", 16 + 4096) + NZ * 2 + moov)
    write(out, "ext64-long.mp4", box(b"ftyp", b"isom") + struct.pack(">I", 1) + b"mdat" + struct.pack(">Q", 1 << 40) + NZ)
    write(out, "box-too-small.mp4", box(b"ftyp", b"isom") + struct.pack(">I", 4) + b"free" + NZ)
    write(out, "size0-last.mp4", box(b"ftyp", b"isom") + moov + struct.pack(">I", 0) + b"mdat" + NZ)
    # --- Photos Libraries ---------------------------------------------------------
    lib = os.path.join(out, "Pictures", "Family.photoslibrary")
    os.makedirs(os.path.join(lib, "database"), exist_ok=True)
    make_db(os.path.join(lib, "database", "Photos.sqlite"))
    write(lib, "originals/0/A1.heic", box(b"ftyp", b"heic" * 4) + box(b"meta", NZ) + box(b"mdat", NZ))
    write(lib, "originals/0/B2.jpeg", b"\xff\xd8\xff" + NZ * 300 + b"\xff\xd9")
    write(lib, "originals/1/C3.jpeg", b"\xff\xd8\xff" + NZ)                       # damaged
    write(lib, "resources/derivatives/x.jpeg", b"\xff\xd8\xff" + NZ + b"\xff\xd9")
    write(lib, ".fseventsd/ignored.bin", NZ)
    # library found by shape, with a damaged database
    shaped = os.path.join(out, "Backups", "OldPhotos")
    os.makedirs(os.path.join(shaped, "database"), exist_ok=True)
    write(shaped, "database/Photos.sqlite", b"SQLite format 3\x00" + b"\xff" * 4000)
    write(shaped, "Masters/2014/IMG.JPG", b"\xff\xd8\xff" + NZ + b"\xff\xd9")
    # nested library inside a library
    nested = os.path.join(lib, "Nested.photoslibrary")
    os.makedirs(os.path.join(nested, "database"), exist_ok=True)
    make_db(os.path.join(nested, "database", "photos.db"), rows=5)
    write(nested, "originals/n.jpeg", b"\xff\xd8\xff" + NZ + b"\xff\xd9")
    # library holding small "iCloud placeholder" originals and a hot -wal
    ph = os.path.join(out, "iCloudCopy.photoslibrary")
    os.makedirs(os.path.join(ph, "database"), exist_ok=True)
    tmpdb = os.path.join(out, "_tmp_wal_source.sqlite")
    make_db(tmpdb, rows=300, wal_copy_to=os.path.join(ph, "database", "Photos.sqlite"))
    for suffix in ("", "-wal", "-shm"):
        if os.path.exists(tmpdb + suffix):
            os.remove(tmpdb + suffix)
    for i in range(7):
        write(ph, f"originals/{i}/P{i}.jpeg", b"\xff\xd8\xff" + NZ * 2 + b"\xff\xd9")
    # an empty-tables database and a plain one outside any library
    con = sqlite3.connect(os.path.join(out, "empty.sqlite"))
    con.execute("CREATE TABLE t (x)")
    con.commit()
    con.close()
    con = sqlite3.connect(os.path.join(out, "notables.sqlite"))
    con.execute("PRAGMA user_version=1")
    con.commit()
    con.close()
    write(out, "corrupt.sqlite", b"SQLite format 3\x00" + b"\x00" * 100 + b"\xff" * 4000)
    good = os.path.join(out, "good.sqlite")
    make_db(good)
    data = bytearray(open(good, "rb").read())
    data[4096 + 100:4096 + 600] = b"\x00" * 500          # damage the second page
    write(out, "damaged-page.sqlite", bytes(data))
    # system folders that must be skipped
    write(out, "$RECYCLE.BIN/deleted.jpg", b"\xff\xd8\xff" + NZ)
    write(out, "System Volume Information/x.dat", NZ)
    print(f"edge fixtures written to {out}")


# ---------------------------------------------------------------------------
# Damaged copies of real files
# ---------------------------------------------------------------------------

def mutations(data, rnd):
    n = len(data)
    out = []
    if n < 64:
        return out
    # a sector-aligned zeroed hole, as ddrescue leaves
    for sectors in (1, 2, 8, 9, 16, 40):
        if n > sectors * 512 * 3:
            off = rnd.randrange(n // 8, n - sectors * 512) // 512 * 512
            d = bytearray(data)
            d[off:off + sectors * 512] = bytes(sectors * 512)
            out.append((f"hole{sectors}s", bytes(d)))
    # the same, but not on a sector boundary (a natural zero run)
    if n > 20000:
        off = rnd.randrange(n // 8, n - 9000) // 512 * 512 + 37
        d = bytearray(data)
        d[off:off + 8192] = bytes(8192)
        out.append(("unaligned8k", bytes(d)))
    # cut short
    for frac in (0.3, 0.7, 0.97):
        out.append((f"cut{int(frac * 100)}", data[: max(1, int(n * frac))]))
    # ending never read: zeros from a sector boundary to the end
    for frac in (0.5, 0.9):
        start = int(n * frac) // 512 * 512
        if n - start >= 1024:
            out.append((f"zerotail{int(frac * 100)}", data[:start] + bytes(n - start)))
    # beginning never read
    z = min(n // 2, 4096) // 512 * 512
    if z:
        out.append(("zerohead", bytes(z) + data[z:]))
    # garbage patch (a bad sector filled with stale data, not zeros)
    if n > 4096:
        off = rnd.randrange(512, n - 2048) // 512 * 512
        d = bytearray(data)
        d[off:off + 1024] = bytes(rnd.randrange(256) for _ in range(1024))
        out.append(("garbage", bytes(d)))
    # extra data appended after the end
    out.append(("trailer", data + b"TRAILER" + bytes(rnd.randrange(256) for _ in range(300))))
    out.append(("zeropad", data + bytes(rnd.choice((100, 600, 5000)))))
    return out


def mutate(seed_dir, out, count, seed):
    rnd = random.Random(seed)
    os.makedirs(out, exist_ok=True)
    files = []
    for dirpath, _, names in os.walk(seed_dir):
        for name in sorted(names):
            p = os.path.join(dirpath, name)
            if os.path.getsize(p) <= 64 << 20:
                files.append(p)
    files.sort()
    made = 0
    for p in files:
        data = open(p, "rb").read()
        base = os.path.basename(p)
        write(out, f"orig/{base}", data)
        for tag, d in mutations(data, rnd):
            write(out, f"{tag}/{base}", d)
            made += 1
            if made >= count:
                break
        if made >= count:
            break
    print(f"{made} damaged copies of {len(files)} files written to {out}")


def main():
    if len(sys.argv) >= 3 and sys.argv[1] == "edge":
        edge(sys.argv[2])
        return 0
    if len(sys.argv) >= 4 and sys.argv[1] == "mutate":
        count, seed = 100000, 1
        args = sys.argv[4:]
        if "--count" in args:
            count = int(args[args.index("--count") + 1])
        if "--seed" in args:
            seed = int(args[args.index("--seed") + 1])
        mutate(sys.argv[2], sys.argv[3], count, seed)
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main())
