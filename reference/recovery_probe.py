#!/usr/bin/python3
"""
Recovery Probe - structural damage scanner for ddrescue output.

Version: 2026-10-12 (Opus 5.5 review - see CHANGES at the end of this note.
Supersedes every earlier copy, including recovery_probe_claude_1_and_2.py.)

Standard library only. No internet. No installation.
Double-click RUN_PROBE.bat (or this file), or: python recovery_probe.py

WHAT IT DOES
    Finds damage that opens cleanly and looks fine, which is the category
    decoders miss. Four independent checks per file:

      1. TRUNCATED   The file's own format says it should continue, and it
                     doesn't. Checked against real end-of-file markers and
                     internal size fields, not guesswork.

      2. HOLE        A long run of zero bytes buried inside a compressed
                     file. ddrescue writes zeros where it could not read,
                     so this is the fingerprint of an unreadable sector.

      3. MISMATCH    The file's magic bytes disagree with its extension, or
                     a known extension has no readable header at all.

      4. DB_DAMAGED  SQLite databases failing their own integrity check. For
                     an Apple Photos Library, Photos.sqlite is what turns
                     UUID-named files back into albums and dates - the
                     highest-stakes single file in the whole project.

    Camera RAW and HEIC get checks 1-3 but never a clean bill of health. RAW is
    checked by following the strip offsets in its first directory (which
    catches some cut-short files, not all) and by scanning for zero runs. It
    is then reported SKIPPED rather than OK, because nothing here can confirm the
    sensor data actually decodes - that happens on the Mac.

    Apple Photos Libraries are scanned, not skipped: this tool only ever reads,
    and Photos.sqlite is the single most valuable thing in one. Never point a
    DUPLICATE FINDER at a library - that is what destroys them.

WHAT IT DOES NOT DO
    It does not decode anything. A photo with a bad sector full of random
    garbage rather than zeros will pass here and look fine until you notice
    the smear. That is what the decoder tools are for. Run both.

CHANGES 2026-10-12 (each one found by running the probe on real files)
    - The hole scan was quadratic: for every zero byte it met it copied the
      rest of the 1 MiB block, twice. Measured: 3.1 s for one 6 MB photo,
      which is days for a full drive. Rewritten to find long zero runs
      directly - about 1,000x faster, and checked byte-for-byte against a
      slow reference on thousands of random files (see probe_selftest.py).
    - Zeros that are part of the format are no longer called holes: FLAC
      padding blocks (8 KiB by default, so nearly every FLAC was a HOLE), ID3
      tag padding, MP4/MOV free/skip/wide boxes, Matroska Void elements, and
      MOV files with uncompressed (PCM) audio, whose silence is zeros.
    - A complete JPEG with data after its end marker (phone motion photos,
      Samsung trailers, Photo Mechanic, FotoStation...) is no longer
      TRUNCATED: the picture is walked to its real end.
    - Camera RAW headers it has no signature for (Panasonic RW2, Olympus ORF,
      Canon CRW, Minolta MRW and others) go to not-testable, not MISMATCH.
    - QuickTime files that start with moov/mdat/wide/free are recognised.
    - Mac '._' files, Thumbs.db, .DS_Store and desktop.ini are labelled as
      system files instead of being called damaged.
    - A database that cannot be OPENED (locked, path too long) is
      UNREADABLE, not DB_DAMAGED.
    - Export also writes <name>-not-testable.csv, the list of files no test
      could verify, which the sorting step needs.
"""

import os
import re
import sys
import csv
import queue
import shutil
import struct
import pathlib
import sqlite3
import tempfile
import threading
import traceback
from collections import Counter
from datetime import datetime

try:
    import tkinter as tk
    from tkinter import ttk, filedialog, messagebox
    HAVE_TK = True
except ImportError:          # falls back to console mode - see run_cli()
    HAVE_TK = False

APP = "Recovery Probe"
VERSION = "2026-10-12"   # shown in the window title, so you can see which copy is running
CHUNK = 1 << 20          # 1 MiB read size
DEFAULT_HOLE_KB = 4      # zero-run threshold in KiB

# ----------------------------------------------------------------------------
# Format knowledge
# ----------------------------------------------------------------------------

# (offset, magic bytes) -> format key
MAGIC = [
    (0, b"\xff\xd8\xff",                 "jpeg"),
    (0, b"\x89PNG\r\n\x1a\n",            "png"),
    (0, b"GIF87a",                       "gif"),
    (0, b"GIF89a",                       "gif"),
    (0, b"BM",                           "bmp"),
    (0, b"II*\x00",                      "tiff"),
    (0, b"MM\x00*",                      "tiff"),
    # TIFF-shaped camera RAW with a magic number of its own: Panasonic and
    # Leica (RW2, RWL, RAW) and Olympus (ORF). Everything after the first four
    # bytes is laid out like TIFF. Without these, every such RAW was MISMATCH.
    (0, b"IIU\x00",                      "tiff"),
    (0, b"IIRO",                         "tiff"),
    (0, b"IIRS",                         "tiff"),
    (0, b"MMOR",                         "tiff"),
    (6, b"HEAPCCDR",                     "crw"),       # Canon CRW, before 2004
    (0, b"\x00MRM",                      "mrw"),       # Minolta / Konica Minolta RAW
    (0, b"%PDF-",                        "pdf"),
    (0, b"PK\x03\x04",                   "zip"),
    (0, b"PK\x05\x06",                   "zip"),
    (0, b"Rar!\x1a\x07",                 "rar"),
    (0, b"7z\xbc\xaf\x27\x1c",           "7z"),
    (0, b"\x1f\x8b",                     "gzip"),
    (0, b"fLaC",                         "flac"),      # (an ID3 tag in front: see sniff)
    (0, b"OggS",                         "ogg"),
    (0, b"RIFF",                         "riff"),
    (0, b"\x1a\x45\xdf\xa3",             "matroska"),
    (4, b"ftyp",                         "isobmff"),   # mp4 / mov / m4a / heic / cr3
    # Older QuickTime files have no ftyp box and begin directly with one of
    # these. They are perfectly valid, and every one was reported MISMATCH.
    (4, b"moov",                         "isobmff"),
    (4, b"mdat",                         "isobmff"),
    (4, b"wide",                         "isobmff"),
    (4, b"free",                         "isobmff"),
    (4, b"skip",                         "isobmff"),
    (4, b"pnot",                         "isobmff"),
    (0, b"SQLite format 3\x00",          "sqlite"),
    (0, b"FUJIFILMCCD-RAW",              "raf"),       # Fujifilm RAW
    (0, b"FOVb",                         "x3f"),       # Sigma RAW
    # The '._' files a Mac writes beside every file it saves to a non-Mac
    # disk (AppleDouble / AppleSingle). Finder metadata, not a picture.
    (0, b"\x00\x05\x16\x07",             "appledouble"),
    (0, b"\x00\x05\x16\x00",             "appledouble"),
]

# extension -> expected format key(s)
EXT_MAP = {
    ".jpg": {"jpeg"}, ".jpeg": {"jpeg"}, ".jpe": {"jpeg"},
    ".png": {"png"}, ".gif": {"gif"}, ".bmp": {"bmp"},
    ".tif": {"tiff"}, ".tiff": {"tiff"},
    # camera RAW - almost all are TIFF containers underneath
    ".cr2": {"tiff"}, ".nef": {"tiff"}, ".arw": {"tiff"}, ".dng": {"tiff"},
    ".orf": {"tiff"}, ".rw2": {"tiff"}, ".pef": {"tiff"}, ".srw": {"tiff"},
    ".raf": {"raf"}, ".cr3": {"isobmff"}, ".x3f": {"x3f"},
    ".webp": {"riff"}, ".avif": {"isobmff"},
    # Not ".db": most .db files are not SQLite (Windows' Thumbs.db is an OLE
    # file), so expecting SQLite turned every one of them into MISMATCH. A .db
    # that IS SQLite is still recognised by its content and checked.
    ".sqlite": {"sqlite"}, ".sqlite3": {"sqlite"},
    ".pdf": {"pdf"},
    ".zip": {"zip"}, ".docx": {"zip"}, ".xlsx": {"zip"}, ".pptx": {"zip"},
    ".epub": {"zip"}, ".odt": {"zip"}, ".ods": {"zip"}, ".jar": {"zip"},
    ".rar": {"rar"}, ".7z": {"7z"}, ".gz": {"gzip"},
    ".mp3": {"mp3", "riff"}, ".flac": {"flac"}, ".ogg": {"ogg"},
    ".wav": {"riff"}, ".avi": {"riff"},
    ".mp4": {"isobmff"}, ".m4v": {"isobmff"}, ".mov": {"isobmff"},
    ".m4a": {"isobmff"}, ".heic": {"isobmff"}, ".heif": {"isobmff"},
    ".mkv": {"matroska"}, ".webm": {"matroska"},
}

# Formats where a long zero run is genuinely abnormal (entropy-coded or
# compressed throughout). Uncompressed formats are excluded: silence in a WAV
# and black pixels in a BMP are legitimately thousands of zero bytes.
COMPRESSED = {"jpeg", "png", "gif", "pdf", "zip", "rar", "7z", "gzip",
              "mp3", "flac", "ogg", "isobmff", "matroska", "raf"}

# Camera RAW. These are TIFF or ISOBMFF containers, so a sniffer calls them
# "tiff"/"isobmff" and - unless listed here - skips the zero-run scan, because
# neither key is in COMPRESSED. A RAW file with a 100 KB hole punched through it
# was reported clean until this set existed. RAW sensor data is entropy-coded, so
# a long zero run is genuinely abnormal.
RAW_EXT = {".cr2", ".cr3", ".crw", ".nef", ".nrw", ".arw", ".srf", ".sr2",
           ".dng", ".raf", ".orf", ".rw2", ".raw", ".pef", ".ptx", ".srw",
           ".3fr", ".erf", ".mef", ".mos", ".x3f", ".iiq", ".k25", ".kdc",
           ".dcr", ".rwl", ".fff", ".mrw"}

# Formats whose structure we can check but whose pixel data we cannot. Reported
# SKIPPED with a reason, never OK - "not testable" is honest, "clean" is not.
STRUCTURE_ONLY = {"tiff", "raf", "x3f"}

# Same problem as RAW, and it was missed for a whole round: nothing on Windows
# decodes HEIC either, so a structurally intact .heic is NOT verified. Reporting
# it OK is the identical lie of omission the RAW fix was for. These go to UNSURE
# and are resolved by the Mac pass.
UNDECODABLE_EXT = {".heic", ".heif", ".avif", ".hif"}

# Apple Photos / iPhoto / Aperture library bundles - directories, not files.
LIBRARY_BUNDLES = (".photoslibrary", ".photolibrary", ".migratedphotolibrary",
                   ".aplibrary", ".pplibrary", ".aplibrary2", ".photolibrary2")

SYSTEM_DIRS = (".Spotlight-V100", ".fseventsd", "$RECYCLE.BIN",
               "System Volume Information", ".Trashes", "found.000")

VERDICTS = ("DB_DAMAGED", "TRUNCATED", "HOLE", "MISMATCH", "UNREADABLE",
            "LIBRARY", "OK", "SKIPPED")

# The verdicts that mean "this file is damaged", as opposed to unverified,
# fine, or a library header. Used to attribute damage to the library holding it.
DAMAGE_VERDICTS = frozenset(("DB_DAMAGED", "TRUNCATED", "HOLE", "MISMATCH",
                             "UNREADABLE"))

# Housekeeping files Windows and macOS scatter through folders. None of them
# is the user's data, and calling them damaged only buries the real damage.
# ("Icon\r" is the Mac's custom-folder-icon file; on a Windows disk the \r is
# stored as U+F00D.)
SYSTEM_FILES = frozenset(("thumbs.db", "ehthumbs.db", "desktop.ini", ".ds_store",
                          ".localized", "icon\r", "icon\uf00d"))
SYSTEM_KEY = "(system files)"
SYSTEM_NOTE = ("Mac or Windows housekeeping file (folder settings, thumbnails or a "
               "Mac '._' metadata file), not your data - nothing to test")

# Where a JPEG has no end marker as its last two bytes, the whole picture is
# walked to find its real end (see _jpeg_image_end). Beyond this size that is
# not attempted and the file is judged by its last bytes as before.
JPEG_PARSE_LIMIT = 256 << 20
# Biggest MP4/MOV 'moov' box read when looking for padding and PCM audio.
MOOV_LIMIT = 64 << 20

_ZEROS = re.compile(rb"\x00+")


def _id3_end(head):
    """Where an ID3v2 tag at the very start of the file ends; 0 if there is none."""
    if head[:3] != b"ID3" or len(head) < 10:
        return 0
    s = head[6:10]
    size = ((s[0] & 0x7F) << 21) | ((s[1] & 0x7F) << 14) | ((s[2] & 0x7F) << 7) | (s[3] & 0x7F)
    return 10 + size + (10 if head[5] & 0x10 else 0)


def sniff(head: bytes) -> str:
    id3 = _id3_end(head)
    if id3:
        # An ID3 tag usually fronts an MP3, but some taggers put one in front
        # of FLAC too, which then read as "content is mp3" - MISMATCH. Look
        # past the tag before deciding.
        if head[id3:id3 + 4] == b"fLaC":
            return "flac"
        return "mp3"
    for off, magic, key in MAGIC:
        if head[off:off + len(magic)] == magic:
            return key
    # bare MP3 frame sync (no ID3 tag)
    if len(head) > 1 and head[0] == 0xFF and (head[1] & 0xE0) == 0xE0:
        return "mp3"
    return ""


def _zero_tail_note(tail):
    """Explain a missing end marker when the tail is zeros, not just absent.

    ddrescue fills what it could not read with zeros, so a file whose last
    bytes are all zeros usually ends in a dead region rather than being cleanly
    cut short. Both are TRUNCATED, but "no end marker" is the wrong sentence to
    put in front of someone: the marker may well be there, sitting under a zero
    run longer than the window we look at. Say which it is.
    """
    n = len(tail) - len(tail.rstrip(b"\x00"))
    if n >= 512:
        most = "at least " if n == len(tail) else ""
        return (f" - the last {most}{n:,} bytes are zeros, so the end of this file "
                f"was never read off the disk")
    return ""


def check_trailer(fmt, size, head, tail):
    """Return (verdict, detail). verdict is '' when nothing is wrong."""
    if fmt == "jpeg":
        end = tail.rstrip(b"\x00")
        if not end.endswith(b"\xff\xd9"):
            return "TRUNCATED", "no JPEG end-of-image marker (FFD9)" + _zero_tail_note(tail)

    elif fmt == "png":
        # The IEND chunk with its fixed checksum, searched for rather than
        # required in the last 16 bytes: zero padding or extra data after it
        # does not make a picture incomplete. It cannot occur by chance inside
        # the compressed data, so a cut-short PNG still fails.
        if b"IEND\xaeB`\x82" not in tail:
            return "TRUNCATED", "no PNG IEND chunk" + _zero_tail_note(tail)

    elif fmt == "gif":
        if not tail.rstrip(b"\x00").endswith(b"\x3b"):
            return "TRUNCATED", "no GIF trailer byte (0x3B)" + _zero_tail_note(tail)

    elif fmt == "pdf":
        if b"%%EOF" not in tail:
            return "TRUNCATED", "no %%EOF in final bytes" + _zero_tail_note(tail)

    elif fmt == "zip":
        # End of Central Directory; comment may be up to 65535 bytes
        if b"PK\x05\x06" not in tail:
            return "TRUNCATED", "no ZIP end-of-central-directory record" + _zero_tail_note(tail)

    elif fmt == "riff":
        if len(head) >= 8:
            declared = int.from_bytes(head[4:8], "little") + 8
            if declared > size:
                short = declared - size
                return "TRUNCATED", f"RIFF header declares {declared:,} bytes, file is {size:,} (short {short:,})"

    elif fmt == "isobmff":
        v, d = _check_isobmff(head, size)
        if v:
            return v, d

    elif fmt == "tiff":
        v, d = _check_tiff(head, size)
        if v:
            return v, d

    return "", ""


def _check_isobmff(head, size):
    """Walk top-level MP4/MOV boxes using only the head we already read."""
    pos = 0
    limit = len(head)
    while pos + 8 <= limit:
        box = int.from_bytes(head[pos:pos + 4], "big")
        name = head[pos + 4:pos + 8]
        if box == 1:                      # 64-bit extended size
            if pos + 16 > limit:
                break
            box = int.from_bytes(head[pos + 8:pos + 16], "big")
        elif box == 0:                    # extends to end of file
            return "", ""
        if box < 8:
            return "TRUNCATED", f"malformed box '{name.decode('ascii', 'replace')}' (size {box})"
        if pos + box > size:
            over = pos + box - size
            return "TRUNCATED", (f"box '{name.decode('ascii', 'replace')}' runs "
                                 f"{over:,} bytes past end of file")
        pos += box
    return "", ""


def _check_isobmff_index(path, size):
    """Walk every top-level box of an MP4/MOV/HEIC file, not just the first MiB.

    A video's index - the 'moov' box - very often sits at the END of the file,
    after the picture data. When the end of the file was unreadable, ddrescue
    leaves zeros there: the file keeps its full size, every box size still
    adds up, and the old head-only check called it fine. But without its
    index the video does not play at all. So: step box by box through the
    whole file (a few small reads), and require that an index box exists
    ('moov' for video and audio, 'meta' for HEIC/AVIF photos). Zeros where a
    box header should be are reported once no index has turned up.
    """
    seen = set()
    pos = 0
    try:
        with open(long_path(path), "rb") as fh:
            for _ in range(1000000):
                if pos + 8 > size:
                    break
                fh.seek(pos)
                h = fh.read(16)
                box, typ, hdr = int.from_bytes(h[:4], "big"), h[4:8], 8
                if box == 1 and len(h) >= 16:
                    box, hdr = int.from_bytes(h[8:16], "big"), 16
                elif box == 0:
                    box = size - pos
                if typ == b"\x00\x00\x00\x00" or box < hdr:
                    if seen & {b"moov", b"meta", b"moof"}:
                        break               # zeros after a complete file: harmless
                    return "TRUNCATED", (f"zeros where a box header should be, at offset "
                                         f"{pos:,} of {size:,} - this part of the file was "
                                         f"never read, and the index box is missing")
                if pos + box > size:
                    return "TRUNCATED", (f"box '{typ.decode('ascii', 'replace')}' runs "
                                         f"{pos + box - size:,} bytes past end of file")
                seen.add(typ)
                pos += box
    except OSError:
        return "", ""
    if not seen & {b"moov", b"meta", b"moof"}:
        return "TRUNCATED", ("no index box (moov/meta) anywhere in the file - without it the "
                             "video or photo cannot be opened")
    return "", ""


def _check_matroska_size(head, size):
    """A Matroska/WebM Segment that declares more bytes than the file holds."""
    try:
        _, n = _ebml_vint(head, 0, True)            # EBML header
        hlen, m = _ebml_vint(head, n, False)
        pos = n + m + hlen
        seg, n = _ebml_vint(head, pos, True)
        if seg != 0x18538067:
            return "", ""
        length, m = _ebml_vint(head, pos + n, False)
    except (ValueError, IndexError):
        return "", ""
    if length == (1 << (7 * m)) - 1:
        return "", ""                               # "unknown size": live recordings
    end = pos + n + m + length
    if end > size:
        return "TRUNCATED", (f"the file says it is {end:,} bytes long but stops at "
                             f"{size:,} (short {end - size:,})")
    return "", ""


def _trailing_zeros(path, size, tail):
    """How many zero bytes the file ends with (reading further back if needed)."""
    t = len(tail) - len(tail.rstrip(b"\x00"))
    if t < len(tail):
        return t
    pos = size - len(tail)
    try:
        with open(long_path(path), "rb") as fh:
            while pos > 0:
                step = min(CHUNK, pos)
                fh.seek(pos - step)
                kept = len(fh.read(step).rstrip(b"\x00"))
                if kept:
                    return size - (pos - step + kept)
                pos -= step
    except OSError:
        return t
    return size


# Formats whose last bytes are always real data, never padding: MP3, FLAC and
# Ogg end in an audio frame, and MP4/MOV/HEIC in box data. If one of these
# ends in a sector-aligned run of zeros, the end of the file was unreadable -
# and nothing else on the Windows side notices, because the file keeps its
# full size and every header still adds up. (Camera RAW is left out: a dark
# picture's last rows can legitimately be zeros.)
ZERO_TAIL_MEANS_DAMAGE = frozenset(("mp3", "flac", "ogg", "isobmff"))


_TIFF_TYPESIZE = {1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 6: 1, 7: 1, 8: 2, 9: 4, 10: 8, 11: 4, 12: 8}


def _check_tiff(head, size):
    """Validate a TIFF/camera-RAW container by following its own pointers.

    Camera RAW (CR2, NEF, ARW, DNG, ORF, RW2 ...) is TIFF underneath, and the
    sensor payload sits at the end of the file. A truncated RAW therefore still
    has a valid header and readable EXIF - but its StripOffsets/TileOffsets
    point past the end of the file, which is what this catches. Nothing else in
    the Windows toolchain tests these formats at all.
    """
    try:
        if len(head) < 8:
            return "TRUNCATED", "TIFF header incomplete"
        bo = "<" if head[0:2] == b"II" else ">"
        ifd = struct.unpack(bo + "I", head[4:8])[0]
        if ifd == 0 or ifd >= size:
            return "TRUNCATED", f"IFD0 offset {ifd:,} lies outside a {size:,} byte file"
        if head[2:4] not in (b"*\x00", b"\x00*"):
            # Panasonic and Olympus RAW: TIFF-shaped, but their strip tags do
            # not mean what TIFF's do. Panasonic stores 0xFFFFFFFF as its
            # StripOffsets and keeps the real offset elsewhere, which read as
            # "image data ends 4 GB past the end of the file" - TRUNCATED for
            # every RW2. Only the header and IFD0 position are checked for them.
            return "", ""
        if ifd + 2 > len(head):
            return "", ""                     # IFD beyond our read window
        count = struct.unpack(bo + "H", head[ifd:ifd + 2])[0]
        if ifd + 2 + count * 12 > len(head):
            return "", ""
        offsets, lengths = [], []
        for i in range(count):
            e = ifd + 2 + i * 12
            tag, typ, cnt = struct.unpack(bo + "HHI", head[e:e + 8])
            ts = _TIFF_TYPESIZE.get(typ)
            if not ts:
                continue
            total = ts * cnt
            if total > 4:
                voff = struct.unpack(bo + "I", head[e + 8:e + 12])[0]
                if voff + total > size:
                    return "TRUNCATED", (f"tag {tag} value block runs "
                                         f"{voff + total - size:,} bytes past end of file")
                vals = _tiff_values(head, bo, voff, typ, cnt, ts)
            else:
                vals = _tiff_values(head, bo, e + 8, typ, cnt, ts)
            if tag in (273, 324):             # StripOffsets / TileOffsets
                offsets = vals
            elif tag in (279, 325):           # StripByteCounts / TileByteCounts
                lengths = vals
        if (offsets and lengths and len(offsets) == len(lengths)
                and 0xFFFFFFFF not in offsets and 0xFFFFFFFF not in lengths):
            end = max(o + n for o, n in zip(offsets, lengths))
            if end > size:
                return "TRUNCATED", (f"image data ends at {end:,} but the file stops at "
                                     f"{size:,} (short {end - size:,} bytes)")
    except (struct.error, ValueError, IndexError):
        return "", ""                          # never let an odd RAW crash the scan
    return "", ""


def _tiff_values(buf, bo, off, typ, cnt, ts):
    if off + ts * cnt > len(buf):
        return []
    fmt = {3: "H", 4: "I"}.get(typ)
    if not fmt:
        return []
    return list(struct.unpack(bo + fmt * cnt, buf[off:off + ts * cnt]))


def _jpeg_image_end(path, size):
    """Offset just past the main picture's end-of-image marker, or None.

    Most JPEGs end in FFD9 and never come here. Plenty of complete ones do
    not: phone "motion photos" carry a short video after the picture, Samsung
    phones append an edit trailer, and Photo Mechanic, FotoStation and others
    append metadata. Judged on the last two bytes, every one of those was
    TRUNCATED. So walk the JPEG's own structure - each segment's length, then
    the compressed data up to the next real marker - and find where the
    picture actually ends. If the walk never reaches an end-of-image marker,
    the picture really is cut short and None keeps it TRUNCATED.
    """
    if size > JPEG_PARSE_LIMIT:
        return None
    try:
        with open(long_path(path), "rb") as fh:
            d = fh.read()
    except OSError:
        return None
    n = len(d)
    if d[:2] != b"\xff\xd8":
        return None
    pos = 2
    for _ in range(100000):             # real JPEGs have dozens of segments
        if pos >= n or d[pos] != 0xFF:
            return None                 # no marker where one must be
        while pos < n and d[pos] == 0xFF:
            pos += 1                    # fill bytes before a marker
        if pos >= n:
            return None
        marker = d[pos]
        pos += 1
        if marker == 0xD9:
            return pos
        if 0xD0 <= marker <= 0xD8 or marker == 0x01:
            continue                    # markers with no length field
        if pos + 2 > n:
            return None
        seg = (d[pos] << 8) | d[pos + 1]
        if seg < 2:
            return None
        pos += seg
        if marker == 0xDA:
            # Start of scan: compressed data runs to the next real marker.
            # Inside it a data byte FF is always followed by 00, and FF D0-D7
            # are restart markers, so neither ends the data.
            while True:
                k = d.find(b"\xff", pos)
                if k < 0 or k + 1 >= n:
                    return None
                nxt = d[k + 1]
                if nxt == 0x00 or 0xD0 <= nxt <= 0xD7:
                    pos = k + 2
                elif nxt == 0xFF:
                    pos = k + 1
                else:
                    pos = k
                    break
    return None


def _padding_ranges(fmt, path, head, size):
    """Where zeros are part of the format rather than damage.

    Returns (ranges, reason). `ranges` are (start, end) byte offsets the hole
    scan must not count. `reason` is set when zeros are normal content right
    through the file, so a zero run says nothing about damage at all.

    Each of these was reported as a HOLE before, on files that were fine:
      FLAC       PADDING blocks - 8 KiB of zeros by default from both the
                 reference encoder and ffmpeg, so very nearly every FLAC
      MP3, FLAC  the padding at the end of an ID3 tag, left for later edits
      MP4, MOV   free / skip / wide boxes, at the top level or inside moov
      MOV        uncompressed (PCM) audio, which many cameras record -
                 silence is stored as zeros
      MKV, WebM  Void elements: reserved space, filled with zeros
    A structure that cannot be followed simply excuses nothing.
    """
    ranges = []
    try:
        with open(long_path(path), "rb") as fh:
            id3 = _id3_end(head)
            if id3:
                ranges.append(_id3_padding(fh, id3))
            if fmt == "flac":
                _flac_padding(fh, id3, size, ranges)
            elif fmt == "isobmff":
                if _mp4_padding(fh, size, ranges):
                    return [], ("uncompressed (PCM) audio - its silence is stored as zeros, "
                                "so the hole check cannot be used here; Phase 4's video "
                                "decode covers this file")
            elif fmt == "matroska":
                _mkv_padding(fh, size, ranges)
    except (OSError, ValueError, IndexError, struct.error):
        pass
    return [r for r in ranges if r[1] > r[0]], ""


def _id3_padding(fh, end):
    """The zeros at the end of an ID3v2 tag that ends at `end`, as (start, end)."""
    start = max(10, end - (1 << 20))
    fh.seek(start)
    body = fh.read(end - start)
    return (start + len(body.rstrip(b"\x00")), start + len(body))


def _flac_padding(fh, base, size, out):
    """PADDING metadata blocks of a FLAC stream whose 'fLaC' is at `base`."""
    pos = base + 4
    for _ in range(1000):
        fh.seek(pos)
        h = fh.read(4)
        if len(h) < 4:
            return
        length = int.from_bytes(h[1:4], "big")
        if h[0] & 0x7F == 1:
            out.append((pos + 4, min(pos + 4 + length, size)))
        pos += 4 + length
        if h[0] & 0x80 or pos >= size:
            return


_MP4_PAD = frozenset((b"free", b"skip", b"wide"))
_MP4_CONTAINERS = frozenset((b"moov", b"trak", b"mdia", b"minf", b"stbl", b"udta",
                             b"edts", b"dinf", b"mvex", b"moof", b"traf", b"meta"))
# Sample formats whose silence (or black) is literally zero bytes.
_MP4_UNCOMPRESSED = frozenset((b"sowt", b"twos", b"lpcm", b"in24", b"in32",
                               b"fl32", b"fl64", b"NONE", b"raw "))


def _mp4_padding(fh, size, out):
    """Collect padding boxes into `out`; True if any track is uncompressed."""
    pcm = False
    pos = 0
    for _ in range(100000):
        if pos + 8 > size:
            break
        fh.seek(pos)
        h = fh.read(16)
        box, typ, hdr = int.from_bytes(h[:4], "big"), h[4:8], 8
        if box == 1 and len(h) >= 16:
            box, hdr = int.from_bytes(h[8:16], "big"), 16
        elif box == 0:
            box = size - pos
        if box < hdr:
            break
        if typ in _MP4_PAD:
            out.append((pos + hdr, min(pos + box, size)))
        elif typ == b"moov" and box <= MOOV_LIMIT:
            fh.seek(pos + hdr)
            pcm = _mp4_walk(fh.read(box - hdr), pos + hdr, out, 0) or pcm
        pos += box
    return pcm


def _mp4_walk(buf, base, out, depth):
    """Walk the boxes in buf (file offset `base`); True if PCM audio is seen."""
    pcm = False
    pos, n = 0, len(buf)
    while pos + 8 <= n and depth < 12:
        box, typ, hdr = int.from_bytes(buf[pos:pos + 4], "big"), buf[pos + 4:pos + 8], 8
        if box == 1 and pos + 16 <= n:
            box, hdr = int.from_bytes(buf[pos + 8:pos + 16], "big"), 16
        elif box == 0:
            box = n - pos
        if box < hdr or pos + box > n:
            break
        start, end = pos + hdr, pos + box
        if typ in _MP4_PAD:
            out.append((base + start, base + end))
        elif typ in _MP4_CONTAINERS:
            if typ == b"meta" and buf[start:start + 4] == b"\x00\x00\x00\x00":
                start += 4              # ISO 'meta' is a full box; QuickTime's is not
            pcm = _mp4_walk(buf[start:end], base + start, out, depth + 1) or pcm
        elif typ == b"stsd":
            p = start + 8               # version/flags, entry count
            while p + 8 <= end:
                if buf[p + 4:p + 8] in _MP4_UNCOMPRESSED:
                    pcm = True
                esize = int.from_bytes(buf[p:p + 4], "big")
                if esize < 8:
                    break
                p += esize
        pos += box
    return pcm


def _ebml_vint(buf, pos, keep_marker):
    """(value, length) of the EBML variable-length integer at buf[pos]."""
    first = buf[pos]
    if not first:
        raise ValueError("not an EBML number")
    length = 9 - first.bit_length()     # 1xxxxxxx is 1 byte ... 00000001 is 8
    if pos + length > len(buf):
        raise IndexError("EBML number runs off the buffer")
    val = first if keep_marker else first & ((1 << (8 - length)) - 1)
    for b in buf[pos + 1:pos + length]:
        val = (val << 8) | b
    return val, length


def _mkv_padding(fh, size, out):
    """Void elements in a Matroska/WebM segment's first 4 MiB."""
    fh.seek(0)
    buf = fh.read(min(size, 4 << 20))
    _, n = _ebml_vint(buf, 0, True)                 # the EBML header
    hlen, m = _ebml_vint(buf, n, False)
    pos = n + m + hlen
    seg, n = _ebml_vint(buf, pos, True)
    if seg != 0x18538067:                           # Segment
        return
    _, m = _ebml_vint(buf, pos + n, False)
    pos += n + m
    while pos + 2 <= len(buf):
        eid, n = _ebml_vint(buf, pos, True)
        length, m = _ebml_vint(buf, pos + n, False)
        if length == (1 << (7 * m)) - 1:
            return                                  # unknown size: cannot step over it
        if eid == 0xEC:                             # Void
            out.append((pos + n + m, min(pos + n + m + length, size)))
        pos += n + m + length


def display_path(p):
    r"""Undo long_path() for anything a human will read.

    long_path() has to prefix the walk root, because os.scandir cannot descend
    past 260 characters on Windows without it - which means every path that
    comes back out of os.walk carries \\?\ too. Printed, that turns the
    library census into a list of paths the user cannot paste into Explorer's
    address bar, and the census exists precisely so they can go and move those
    folders. Strip it at the presentation boundary; no-op off Windows.
    """
    if os.name != "nt":
        return p
    if p.startswith("\\\\?\\UNC\\"):
        return "\\" + p[7:]
    if p.startswith("\\\\?\\"):
        return p[4:]
    return p


def walk_clean(root):
    """os.walk that descends deep Windows paths but yields displayable ones.

    The two halves have to travel together. Prefixing the walk root and then
    cleaning only the paths you print leaves os.path.relpath comparing a
    prefixed directory against an unprefixed base - which silently returns a
    path full of '..' and, in survey_library, would quietly zero the originals
    count on Windows and nowhere else. Doing both in one place makes that
    mismatch impossible to reintroduce.
    """
    for dirpath, dirnames, filenames in os.walk(long_path(root)):
        yield display_path(dirpath), dirnames, filenames


def check_sqlite(path):
    """Integrity-check a SQLite database without writing beside the original.

    The -wal trap, measured. A database recovered from an unclean shutdown very
    often has a hot `-wal` next to it, and its recent contents - sometimes ALL
    of its contents - live in that WAL rather than the main file:

      ?mode=ro&immutable=1  cannot see the WAL. On a healthy database whose data
                            is all in the WAL it reports integrity "ok" for a
                            file it effectively read as empty. A false pass on
                            the highest-stakes file in the project.
      ?mode=ro              reads the WAL correctly, but creates -shm (and can
                            recreate -wal) beside your recovered data, and may
                            replay the WAL into the database you are assessing.

    So when a hot WAL is present, the database and its sidecars are copied to a
    scratch folder and the COPY is checked with mode=ro: correct answer, and
    nothing is written next to the recovery output. With no WAL, immutable=1 is
    both safe and correct, so it is used directly.
    """
    wal = path + "-wal"
    try:
        hot_wal = os.path.exists(long_path(wal)) and os.path.getsize(long_path(wal)) > 0
    except OSError:
        hot_wal = False

    if not hot_wal:
        return _sqlite_verdict(path, "?mode=ro&immutable=1", had_wal=False)

    try:
        with tempfile.TemporaryDirectory(prefix="probe-db-") as td:
            copy = os.path.join(td, "db.sqlite")
            for suffix in ("", "-wal", "-shm"):
                src = path + suffix
                if os.path.exists(long_path(src)):
                    shutil.copy2(long_path(src), copy + suffix)
            return _sqlite_verdict(copy, "?mode=ro", had_wal=True,
                                   note=" (checked with its -wal, on a scratch copy)")
    except OSError as e:
        # Out of scratch space, most likely. Say what happened rather than
        # silently falling back to a check that cannot see half the database.
        return "UNREADABLE", (f"has a hot -wal and could not be copied for checking "
                              f"({type(e).__name__}: {e}) - check it by hand")


def _sqlite_verdict(path, suffix, had_wal=False, note=""):
    uri = pathlib.Path(path).absolute().as_uri() + suffix
    try:
        con = sqlite3.connect(uri, uri=True, timeout=5)
        try:
            # Both pragmas, cheapest first. Measured elsewhere on an indexed
            # Photos-shaped database: quick_check ~0.08s, integrity_check ~1.87s
            # at 87 MB. They detect zeroed bad sectors identically; the extra
            # thing integrity_check does is verify indexes still agree with
            # tables, and albums, faces and keywords are index-backed lookups.
            # Two minutes once, on the most valuable file in the recovery.
            rows = con.execute("PRAGMA quick_check(20)").fetchall()
            if rows and str(rows[0][0]).lower() == "ok":
                rows = con.execute("PRAGMA integrity_check").fetchall()
            names = [r[0] for r in con.execute(
                "SELECT name FROM sqlite_master WHERE type='table' "
                "AND name NOT LIKE 'sqlite_%' LIMIT 40")]
            tables = len(names)
            # A database whose tables all came back empty is the signature of a
            # WAL that SQLite discarded: a damaged -wal is silently dropped and
            # the main file - which never received the data - is used instead.
            # No error is raised for that, so this is the only visible symptom.
            populated = 0
            for t in names:
                try:
                    if con.execute(f'SELECT EXISTS(SELECT 1 FROM "{t}" LIMIT 1)').fetchone()[0]:
                        populated += 1
                        break
                except sqlite3.DatabaseError:
                    pass
        finally:
            con.close()
    except sqlite3.DatabaseError as e:
        # sqlite3's "cannot open", "locked" and "disk I/O" errors are
        # subclasses of DatabaseError, so a separate except below them never
        # fired: a database Windows merely could not open was DB_DAMAGED.
        # Locked or permission-denied is a fact about access, not damage.
        if _sqlite_access_error(e):
            return "UNREADABLE", f"{type(e).__name__}: {e}"
        return "DB_DAMAGED", f"SQLite cannot read it: {' '.join(str(e).split())}{note}"
    except (sqlite3.Error, OSError) as e:
        return "UNREADABLE", f"{type(e).__name__}: {e}"

    msgs = [str(r[0]) for r in rows if r and r[0]]
    if msgs and not (len(msgs) == 1 and msgs[0].lower() == "ok"):
        more = f" (+{len(msgs) - 3} more)" if len(msgs) > 3 else ""
        flat = " ".join(" ".join(msgs[:3]).split())
        return "DB_DAMAGED", f"integrity check failed: {flat[:100]}{more}{note}"
    if tables == 0:
        # "ok" on a database with no tables means nothing was actually read.
        return "DB_DAMAGED", ("integrity check passed but the database contains no tables "
                              "- its contents were not readable" + note)
    # Only meaningful when a WAL was present: SQLite silently discards a damaged
    # -wal and falls back to the main file, which then reads as structurally fine
    # but empty. Without a WAL, an empty database is simply an empty database.
    if had_wal and populated == 0:
        return "DB_DAMAGED", ("integrity check passed but every table is empty - the -wal "
                              "beside it was unreadable and SQLite discarded it silently"
                              + note)
    return "", ""


# SQLite result codes that describe access, not content: PERM, BUSY, LOCKED,
# READONLY, IOERR, CANTOPEN, AUTH. (CORRUPT and NOTADB are the damage ones.)
_SQLITE_ACCESS_CODES = frozenset((3, 5, 6, 8, 10, 14, 23))


def _sqlite_access_error(e):
    """True if this sqlite3 error means "could not get at it", not "it is broken"."""
    code = getattr(e, "sqlite_errorcode", None)     # Python 3.11 and later
    if code is not None:
        return (code & 0xFF) in _SQLITE_ACCESS_CODES
    msg = str(e).lower()
    return any(s in msg for s in ("unable to open", "locked", "readonly",
                                  "disk i/o", "permission"))


def long_path(p):
    r"""Make a path usable on Windows past the 260-character MAX_PATH limit.

    Recovered trees are deep - ddrescue output inside a working folder, inside a
    Photos library, inside its hex subfolders with long UUID filenames. Without
    the \\?\\ prefix, Python on Windows raises FileNotFoundError on those paths
    unless LongPathsEnabled is set, and the probe would report UNREADABLE for
    files that are perfectly fine - a false alarm nobody would spot among a
    million rows.

    No-op on macOS and Linux, and on paths already prefixed. UNC-aware.
    """
    if os.name != "nt":
        return p
    p = os.path.abspath(p)
    if p.startswith("\\\\?\\"):
        return p
    if p.startswith("\\\\"):
        return "\\\\?\\UNC" + p[1:]
    return "\\\\?\\" + p


def _outside(start, end, skip):
    """The longest piece of [start, end) left after removing the skip ranges.

    Returns (length, start of that piece). Used so a zero run lying inside
    known padding is not counted, while a run that spills out of padding into
    real data still counts for the part that spilled.
    """
    pieces = [(start, end)]
    for a, b in skip:
        kept = []
        for x, y in pieces:
            if b <= x or a >= y:
                kept.append((x, y))
                continue
            if a > x:
                kept.append((x, a))
            if b < y:
                kept.append((b, y))
        pieces = kept
    if not pieces:
        return 0, start
    x, y = max(pieces, key=lambda p: (p[1] - p[0], -p[0]))
    return y - x, x


# A hole ddrescue leaves is whole disk sectors of zeros, and a file's data
# always starts on a sector boundary (every filesystem allocates in whole
# clusters). So a real hole starts and ends on a multiple of 512 in the file,
# give or take a few zero bytes of the file's own on either side. Natural
# zero runs land anywhere. Measured on good files - night-sky and flat-colour
# JPEGs (optimised Huffman tables encode flat areas as zero bytes), real
# Canon CR2 and Nikon NEF files, camera video - 190 of 191 natural runs of
# 4 KiB or more failed this test, and the one that passed was PCM silence,
# which is excused separately. Without it each of those files was a HOLE.
SECTOR = 512
SECTOR_SLOP = 8


def _sector_core(start, length):
    """Bytes of the run lying on whole sectors, or 0 if it is not hole-shaped."""
    end = start + length
    a = -(-start // SECTOR) * SECTOR
    b = end // SECTOR * SECTOR
    if b <= a or a - start > SECTOR_SLOP or end - b > SECTOR_SLOP:
        return 0
    return b - a


def scan_zero_run(path, threshold, size, skip=()):
    """Longest hole-shaped run of zeros with real data after it: (run, offset).

    A run counts only if, once any known padding (`skip`) is taken out, at
    least `threshold` bytes of it sit on whole disk sectors (see SECTOR above).
    Exact for every such run. Runs shorter than `threshold` cannot qualify, so
    inside a read chunk they are not looked at, and that is where the speed
    comes from.

    A run of zeros that reaches the end of the file is padding or a truncation,
    not a hole punched through the middle, so it is never counted. (An older
    version kept only the single longest run and dropped it if it touched EOF,
    which hid a real interior hole whenever a longer zero run trailed it.) A
    run becomes a candidate only once real data is seen after it, so the
    trailing run is never closed and never counted, however many read chunks
    it spans. Runs inside the `skip` ranges - known padding - do not count.

    Speed. The previous version stepped through every zero byte and, at each
    one, copied the rest of the 1 MiB chunk twice. JPEG and video data hold a
    zero byte every hundred or so, so that was ~0.5 s per MiB: 3.1 seconds for
    a single 6 MB photo, days for a whole drive. Now each chunk is searched
    for `threshold` consecutive zeros in one C-speed find(), and only the
    chunk's edges - where a run can cross into the next read - are measured
    byte-exactly. probe_selftest.py checks this against a slow byte-by-byte
    reference on random files with tiny chunks.
    """
    best = best_off = 0
    run = run_start = 0          # zeros at the end of what has been read so far
    off = 0
    needle = b"\x00" * max(1, int(threshold))

    def consider(length, start):
        nonlocal best, best_off
        if skip:
            length, start = _outside(start, start + length, skip)
        if _sector_core(start, length) >= len(needle) and length > best:
            best, best_off = length, start

    with open(long_path(path), "rb") as fh:
        while True:
            buf = fh.read(CHUNK)
            if not buf:
                break
            n = len(buf)
            m = _ZEROS.match(buf)
            lead = m.end() if m else 0
            if lead == n:
                # All zeros: whatever run is open simply grows.
                if not run:
                    run_start = off
                run += n
                off += n
                continue
            # Real data starts at `lead`, so the run open from the last chunk,
            # plus this chunk's leading zeros, has data after it: a candidate.
            if run or lead:
                if not run:
                    run_start = off
                run += lead
                consider(run, run_start)
            run = 0
            trail = n - len(buf.rstrip(b"\x00"))
            i, j = lead, n - trail
            while True:
                k = buf.find(needle, i, j)
                if k < 0:
                    break
                # buf[k-1] is data (else the find would have stopped there),
                # and the run ends before j, which is data too: a closed run.
                e = _ZEROS.match(buf, k, j).end()
                consider(e - k, off + k)
                i = e
            if trail:
                run, run_start = trail, off + n - trail
            off += n
    # Anything still open here ran to the end of the file: not a hole.
    return best, best_off


def probe_file(path, hole_bytes, do_hole):
    try:
        size = os.path.getsize(long_path(path))
    except OSError as e:
        return "UNREADABLE", f"{type(e).__name__}: {e}", 0

    name = os.path.basename(path)
    if name.lower() in SYSTEM_FILES:
        return "SKIPPED", SYSTEM_NOTE, size

    if size == 0:
        return "TRUNCATED", "zero bytes", 0

    try:
        with open(long_path(path), "rb") as fh:
            head = fh.read(min(CHUNK, size))
            tail_len = min(65600, size)
            fh.seek(size - tail_len)
            tail = fh.read(tail_len)
    except OSError as e:
        return "UNREADABLE", f"{type(e).__name__}: {e}", size

    fmt = sniff(head)
    ext = os.path.splitext(path)[1].lower()
    is_raw = ext in RAW_EXT

    if fmt == "appledouble" or (not fmt and name.startswith("._")):
        return "SKIPPED", SYSTEM_NOTE, size

    if not fmt:
        lead = len(head) - len(head.lstrip(b"\x00"))
        if lead >= 512 and (ext in EXT_MAP or is_raw):
            return "MISMATCH", (f"the first {lead:,} bytes are zeros - the start of this "
                                f"file was never read off the disk"), size
        if is_raw:
            # A camera RAW type this probe has no signature for (there are
            # dozens). Not knowing a format is not evidence of damage, so it
            # is still scanned for holes and then goes to not-testable.
            fmt = "raw"
        elif ext in EXT_MAP:
            # A known extension with an unrecognisable header is damage, not
            # an untestable file. Only genuinely unknown types are skipped.
            return "MISMATCH", (f"extension says {ext} but the header is not recognisable "
                                f"- destroyed header or misnamed file"), size
        else:
            return "SKIPPED", f"unrecognised format ({ext or 'no extension'})", size

    if fmt == "sqlite":
        verdict, detail = check_sqlite(path)
        return (verdict or "OK"), detail, size

    verdict, detail = check_trailer(fmt, size, head, tail)
    picture_end = size
    if verdict == "TRUNCATED" and fmt == "jpeg":
        end = _jpeg_image_end(path, size)
        if end is not None:
            # A complete picture with something stored after it. That extra
            # data is not part of the photo, so it is not scanned for holes.
            verdict, detail, picture_end = "", "", end
    if not verdict and fmt == "isobmff":
        verdict, detail = _check_isobmff_index(path, size)
    elif not verdict and fmt == "matroska":
        verdict, detail = _check_matroska_size(head, size)
    if verdict:
        return verdict, detail, size

    # Extension-vs-content. RAW is exempt: a .cr2 legitimately sniffs as tiff and
    # a .cr3 as isobmff, and neither is a mismatch.
    expected = EXT_MAP.get(ext)
    if expected and fmt not in expected and not is_raw:
        return "MISMATCH", f"content is {fmt}, extension says {ext}", size

    # RAW sensor payloads are entropy-coded, so a long zero run is abnormal there
    # even though it is normal inside an uncompressed TIFF scan.
    # RIFF is a container, kept out of COMPRESSED because silence in a WAV is
    # legitimately zeros. WebP is RIFF too, but VP8-compressed, so it gets the
    # scan; the form type at bytes 8-12 is what tells them apart. (AVI is also
    # RIFF and is left out on purpose: its padding chunks and any PCM audio
    # track are real zeros, and Phase 4's full ffmpeg decode covers it.)
    if fmt in ZERO_TAIL_MEANS_DAMAGE:
        zt = _trailing_zeros(path, size, tail)
        if zt >= hole_bytes:
            # Padding is excused here too, and so is a file with PCM audio,
            # whose last chunk of sound can be genuine silence.
            pad, pcm = _padding_ranges(fmt, path, head, size)
            run, start = _outside(size - zt, size, pad)
            aligned = -(-start // SECTOR) * SECTOR
            if (not pcm and start + run == size and aligned - start <= SECTOR_SLOP
                    and size - aligned >= hole_bytes):
                return "TRUNCATED", (f"the last {run:,} bytes are zeros, starting on a disk-sector "
                                     f"boundary - the end of this file was never read off the disk"), size

    is_webp = fmt == "riff" and head[8:12] == b"WEBP"
    if do_hole and (fmt in COMPRESSED or is_raw or is_webp):
        skip, no_scan = _padding_ranges(fmt, path, head, size)
        if picture_end < size:
            skip.append((picture_end, size))
        if not no_scan:
            try:
                run, at = scan_zero_run(path, hole_bytes, size, skip)
            except OSError as e:
                return "UNREADABLE", f"{type(e).__name__}: {e}", size
            if run >= hole_bytes:
                pct = 100.0 * at / size
                return "HOLE", f"{run:,} zero bytes at offset {at:,} ({pct:.0f}% in)", size

    # Nothing found - but for some formats "nothing found" is not "clean", and
    # saying OK would be a lie of omission.
    if is_raw:
        return "SKIPPED", ("camera RAW: header, strip offsets and zero-runs all pass, but "
                           "the sensor data cannot be decoded here - confirm on the Mac"), size
    if ext in UNDECODABLE_EXT:
        return "SKIPPED", ("HEIC/AVIF: structure and zero-runs pass, but nothing on Windows "
                           "decodes it - confirm on the Mac"), size
    if fmt in STRUCTURE_ONLY:
        return "SKIPPED", (f"{fmt}: structure checked, pixel data cannot be verified here "
                           f"- open a sample to confirm"), size

    return "OK", "", size


def looks_like_library(dirpath, dirnames):
    """Is this directory an Apple Photos / iPhoto / Aperture library bundle?

    Two independent tests, because you cannot rely on the name alone. A library
    whose folder was renamed - or whose extension was lost somewhere between a
    failing disk and a recovery tool - is still a library, and a duplicate
    finder pointed at it will still gut it. So we also detect the *shape*: a
    bundle carries a database folder beside its originals.
    """
    base = os.path.basename(dirpath).lower()
    if base.endswith(LIBRARY_BUNDLES):
        return "name"
    low = {d.lower() for d in dirnames}
    if "database" in low and ({"originals", "masters"} & low):
        return "structure (database/ beside originals/)"
    return ""


LIBRARY_DBS = ("photos.sqlite", "photos.db", "library.apdb", "librarydata.apdb")


def _size(n):
    if n >= (1 << 30):
        return f"{n / (1 << 30):,.1f} GB"
    if n >= (1 << 20):
        return f"{n / (1 << 20):,.0f} MB"
    return f"{n / (1 << 10):,.0f} KB"


def survey_library(libpath, how):
    """Inventory one Photos library in a single cheap stat pass.

    Measured against accumulating this inline during the main walk: the separate
    pass is ~33% faster on a 14,000-file fixture. Inline looks cheaper because it
    avoids "walking twice", but it pays a startswith test against every open
    library for every file on the drive plus a second stat for mtime, where this
    gets size and mtime from one os.stat and leaves the main walk clean. I had it
    this way, switched to inline on that reasoning, and was wrong.

    Originals are the count that matters. `resources/` holds thumbnails and
    edited derivatives, so counting every file flatters a heavily-edited library
    over one that simply holds more photos.
    """
    info = {"path": libpath, "how": how, "originals": 0, "orig_bytes": 0,
            "orig_sizes": [], "total_bytes": 0, "files": 0, "db": "missing",
            "db_bytes": 0, "hot_wal": False, "newest": 0,
            "db_verdict": "", "db_detail": "", "median_orig": 0,
            # Filled in during the main walk, not here: this pass only stats.
            "damaged": 0, "unverified": 0, "damage": Counter()}
    for dirpath, dirnames, filenames in walk_clean(libpath):
        dirnames[:] = [d for d in dirnames if d not in SYSTEM_DIRS]
        # A library nested inside another gets its own census entry from the
        # main walk, so counting its files here too would inflate the parent -
        # and the parent is usually the one being ranked first. Same predicate
        # the main walk uses, so a nested bundle detected by shape is excluded
        # as reliably as one detected by name.
        if dirpath != libpath and looks_like_library(dirpath, dirnames):
            dirnames[:] = []
            continue
        rel = os.path.relpath(dirpath, libpath).replace("\\", "/").lower()
        in_orig = rel.split("/")[0] in ("originals", "masters")
        for name in filenames:
            try:
                st = os.stat(long_path(os.path.join(dirpath, name)))
            except OSError:
                continue
            info["files"] += 1
            info["total_bytes"] += st.st_size
            if st.st_mtime > info["newest"]:
                info["newest"] = st.st_mtime
            if in_orig:
                info["originals"] += 1
                info["orig_bytes"] += st.st_size
                # Capped sample: the median is for spotting iCloud placeholder
                # stubs, and a few thousand files settle that as well as a few
                # hundred thousand.
                if len(info["orig_sizes"]) < 20000:
                    info["orig_sizes"].append(st.st_size)
            if name.lower() in LIBRARY_DBS:
                info["db"] = "present"
                info["db_bytes"] = st.st_size
                try:
                    w = os.path.join(dirpath, name) + "-wal"
                    info["hot_wal"] = (os.path.exists(long_path(w))
                                       and os.path.getsize(long_path(w)) > 0)
                except OSError:
                    pass
    sizes = sorted(info["orig_sizes"])
    if sizes:
        info["median_orig"] = sizes[len(sizes) // 2]
    return info


# An iPhone HEIC is normally 1.5-3 MB. iCloud's "Optimize Mac Storage" replaces
# originals with small proxies, so a library whose typical original is a few
# hundred KB is probably holding placeholders while the real photos sit in
# iCloud - which would make a large part of this recovery unnecessary.
PLACEHOLDER_MEDIAN = 500 * 1024


def _is_placeholder(info):
    """True if this library's typical original looks like an iCloud proxy."""
    m = info.get("median_orig") or 0
    return bool(m) and m < PLACEHOLDER_MEDIAN


def _rank_key(info):
    """Placeholder libraries last, then most originals, then biggest bundle."""
    return (_is_placeholder(info), -(info.get("originals") or 0),
            -(info.get("total_bytes") or 0))


def format_library_survey(infos):
    """Ranked, human-readable comparison of every library found."""
    if not infos:
        return []
    out = ["", "=" * 76, f"APPLE PHOTOS LIBRARIES FOUND: {len(infos)}", "=" * 76, "",
           "Ranked by number of originals. MOVE EVERY ONE out of the scanning area",
           "before you run any duplicate finder.", ""]
    for n, i in enumerate(sorted(infos, key=_rank_key), 1):
        newest = (datetime.fromtimestamp(i.get("newest", 0)).strftime("%Y-%m-%d")
                  if i.get("newest", 0) else "unknown")
        out.append(f"{n}. {i.get('path', '?')}")
        out.append(f"     found by     {i.get('how', '?')}")
        out.append(f"     originals    {i.get('originals', 0):>9,} files   {_size(i.get('orig_bytes', 0))}")
        out.append(f"     whole bundle {i.get('files', 0):>9,} files   {_size(i.get('total_bytes', 0))}")
        out.append(f"     database     {i.get('db', 'missing')}"
                   + (f", {_size(i.get('db_bytes', 0))}" if i.get("db_bytes", 0) else "")
                   + ("   [hot -wal present]" if i.get("hot_wal") else ""))
        out.append(f"     newest file  {newest}")
        dmg = i.get("damaged", 0)
        if dmg:
            parts = ", ".join(f"{n:,} {v.lower()}" for v, n in
                              sorted(i.get("damage", {}).items(),
                                     key=lambda kv: -kv[1]))
            pct = 100.0 * dmg / i.get("files", 0) if i.get("files") else 0.0
            out.append(f"     damaged      {dmg:>9,} files   ({pct:.1f}% of the bundle)")
            out.append(f"                  {parts}")
        elif i.get("files"):
            out.append(f"     damaged      {0:>9,} files   nothing flagged inside it")
        if i.get("unverified", 0):
            out.append(f"     unverified   {i.get('unverified', 0):>9,} files   "
                       f"HEIC/RAW - this library's share of the Mac pass")
        if i.get("originals", 0):
            out.append(f"     typical photo{_size(i.get('median_orig', 0)):>14}   (median of originals/)")
            if i.get("median_orig", 0) and i.get("median_orig", 0) < PLACEHOLDER_MEDIAN:
                out.append("     >> LIKELY iCLOUD PLACEHOLDERS. A typical original this small")
                out.append("        means 'Optimize Mac Storage' was on and the real photos are")
                out.append("        still in iCloud. Check icloud.com before spending effort on")
                out.append("        this one - you may not need to recover it at all.")
        if i.get("db_verdict") == "DB_DAMAGED":
            out.append("     >> DATABASE DAMAGED - the photos in originals/ may be perfectly")
            out.append("        fine, but the albums, dates and faces are at risk. Try Photos'")
            out.append("        own Repair first (hold Option+Command while launching it).")
            if i.get("db_detail"):
                out.append(f"        {i.get('db_detail', '')[:68]}")
        elif i.get("db", "missing") == "missing":
            out.append("     >> no database found - the photos may survive but the albums,")
            out.append("        dates and faces do not. Treat as a folder of loose files.")
        elif i.get("db_verdict") == "OK":
            out.append("     >> database passed its integrity check")
        out.append("")
    if len(infos) > 1:
        out += ["Comparing them:",
                "  - Most originals is usually the one to restore first - but check the dates.",
                "    A smaller library with a NEWER newest-file is often the one you were",
                "    actually still using, and the big one an old archive.",
                "  - Libraries overlap heavily. Do not assume the small ones are redundant",
                "    until you have opened the big one and found the photos in it.",
                "  - A damaged database does not mean lost photos. originals/ is separate.",
                "  - 'damaged' counts flagged files INSIDE that bundle, so you can see which",
                "    library actually took the hit rather than reading it off a million-row",
                "    CSV. It is reported, not ranked on: a library with the most photos is",
                "    still usually the one to restore, damage and all.",
                "  - 'unverified' is that library's share of the Phase 5 Mac pass.",
                "  - Libraries flagged as iCloud placeholders are listed last: they hold",
                "    proxies, not your photos, so they are rarely the one to restore.",
                ""]
    return out


def walk_and_probe(root, hole_bytes, do_hole, should_stop=None):
    """Yield ('lib', info), ('row', tuple) and ('libdone', info) events.

    One walker, shared by the window and the console fallback.

    Photos libraries are inventoried AND walked, in the same pass. Surveying
    alone ranks them without ever checking a photo - and originals/ is where the
    actual pictures live, so a truncated JPEG or a zero-holed HEIC inside a
    bundle has to be found like any other file. This tool only ever reads; what
    destroys a library is a duplicate finder, which is a different program.
    """
    libs = []
    for dirpath, dirnames, filenames in walk_clean(root):
        dirnames[:] = [d for d in dirnames if d not in SYSTEM_DIRS]
        how = looks_like_library(dirpath, dirnames)
        if how:
            info = survey_library(dirpath, how)
            libs.append(info)
            yield ("lib", info)
        # Which library, if any, contains this directory - resolved ONCE per
        # folder rather than once per file. On a drive with a million files and
        # a dozen libraries that is a few thousand string tests instead of
        # twelve million. The innermost match wins, so a library nested inside
        # another counts its own damage rather than inflating its parent's.
        here = None
        for info in libs:
            base = info["path"]
            if dirpath == base or dirpath.startswith(base + os.sep):
                if here is None or len(base) > len(here["path"]):
                    here = info
        for name in filenames:
            if should_stop is not None and should_stop():
                return
            p = os.path.join(dirpath, name)
            try:
                verdict, detail, size = probe_file(p, hole_bytes, do_hole)
            except Exception as e:                      # never die on one file
                verdict, detail, size = "UNREADABLE", f"{type(e).__name__}: {e}", 0
            if here is not None:
                if verdict in DAMAGE_VERDICTS:
                    here["damaged"] += 1
                    here["damage"][verdict] += 1
                elif verdict == "SKIPPED":
                    here["unverified"] += 1
                if name.lower() in LIBRARY_DBS:
                    here["db_verdict"] = verdict
                    here["db_detail"] = detail
            yield ("row", (verdict, detail, size, p))
    for info in libs:
        yield ("libdone", info)


def format_untestable(counter, total):
    """Break the 'not testable' pile down by extension.

    Since HEIC and RAW are never certified on Windows, this pile is usually
    most of a photo collection - and its size is what decides how long the Mac
    pass in Phase 5 takes. A single number does not tell you that; a breakdown
    by extension does.
    """
    if not counter:
        return []
    out = ["", f"NOT TESTABLE: {total:,} files, by extension", "-" * 46]
    for ext, n in counter.most_common(12):
        note = ""
        if ext in UNDECODABLE_EXT:
            note = "  <- needs the Mac (sips)"
        elif ext in RAW_EXT:
            note = "  <- camera RAW, needs the Mac"
        elif ext == SYSTEM_KEY:
            note = "  <- Mac/Windows housekeeping, not your files"
        out.append(f"  {ext or '(no extension)':<16}{n:>10,}{note}")
    if len(counter) > 12:
        out.append(f"  ...and {len(counter) - 12} other extensions")
    out += ["", "These are not damaged - they are unverified. Everything above marked",
            "'needs the Mac' is what Phase 5 Job 1 has to decode.", ""]
    return out


def untestable_key(path):
    """How a not-testable file is grouped in the summary.

    By extension - except that housekeeping files get one bucket of their own,
    so that 5,000 '._IMG_1234.JPG' sidecars are not read as 5,000 untested
    JPEGs.
    """
    name = os.path.basename(path)
    if name.startswith("._") or name.lower() in SYSTEM_FILES:
        return SYSTEM_KEY
    return os.path.splitext(name)[1].lower()


RESULT_HEADER = ["verdict", "detail", "size_bytes", "path"]


def write_results_csv(path, rows):
    """The results file and the not-testable list share one layout."""
    with open(path, "w", newline="", encoding="utf-8-sig") as fh:
        w = csv.writer(fh)
        w.writerow(RESULT_HEADER)
        w.writerows(rows)


def untestable_csv_path(results_csv):
    """Where the not-testable list goes: beside the results file, same stem.

    Every file the probe could not verify, one row each. Without it there is
    no way to tell, later, which files were tested and passed from which were
    never tested at all - and the sorting step depends on that difference.
    """
    return os.path.splitext(results_csv)[0] + "-not-testable.csv"


def library_row(info):
    """Turn a library survey entry into a normal result row."""
    db = (info.get("db_verdict") or
          ("no database" if info.get("db") == "missing" else "database ok"))
    tag = "  [likely iCloud placeholders]" if _is_placeholder(info) else ""
    dmg = info.get("damaged", 0)
    hit = f", {dmg:,} damaged inside" if dmg else ""
    return ("LIBRARY",
            f"{info.get('originals', 0):,} originals "
            f"({_size(info.get('orig_bytes', 0))}), "
            f"bundle {_size(info.get('total_bytes', 0))}{hit} - "
            f"found by {info.get('how', '?')} - {db}{tag}",
            info.get("total_bytes", 0), info.get("path", "?"))


def library_csv_path(results_csv):
    """Where the library inventory goes: beside the results file, same stem."""
    return os.path.splitext(results_csv)[0] + "-photo-libraries.csv"


def write_library_csv(libcsv, libraries):
    """One row per Photos library, ranked. Shared by the window and the console.

    This used to live inside run_cli only, so the window's Export button never
    produced it - while the checklist told the user a second CSV would land
    beside the results file and Phase 5 sent them back to it to choose which
    library to restore. Someone following the double-click instructions would
    have reached Phase 5 looking for a file that did not exist (claude_3).

    Column order and row order both match the console listing. The checklist
    sends the user to this file to choose which library to restore first, so a
    CSV sorted differently from the ranking they were just shown would quietly
    contradict it.
    """
    with open(libcsv, "w", newline="", encoding="utf-8-sig") as fh:
        w = csv.writer(fh)
        w.writerow(["rank", "path", "found_by", "originals", "originals_bytes",
                    "total_files", "total_bytes", "damaged_inside",
                    "damage_breakdown", "unverified_inside",
                    "database", "database_bytes",
                    "median_original_bytes", "likely_icloud_placeholders",
                    "hot_wal", "database_verdict", "database_detail", "newest_file"])
        for n, i in enumerate(sorted(libraries, key=_rank_key), 1):
            w.writerow([n, i.get("path", "?"), i.get("how", "?"),
                        i.get("originals", 0), i.get("orig_bytes", 0),
                        i.get("files", 0), i.get("total_bytes", 0),
                        i.get("damaged", 0),
                        " ".join(f"{v}={c}" for v, c in
                                 sorted(i.get("damage", {}).items())),
                        i.get("unverified", 0),
                        i.get("db", "missing"), i.get("db_bytes", 0),
                        i.get("median_orig", 0),
                        "yes" if _is_placeholder(i) else "no",
                        "yes" if i.get("hot_wal") else "no",
                        i.get("db_verdict", ""), i.get("db_detail", ""),
                        datetime.fromtimestamp(i.get("newest", 0)).isoformat(" ", "seconds")
                        if i.get("newest", 0) else ""])


# ----------------------------------------------------------------------------
# GUI
# ----------------------------------------------------------------------------

class ProbeApp:
    def __init__(self, root):
        self.root = root
        root.title(f"{APP} - version {VERSION}")
        root.geometry("1080x660")
        root.minsize(820, 480)

        self.q = queue.Queue()
        self.worker = None
        self.stop_flag = threading.Event()
        self.rows = []                      # flagged rows only
        self.skipped = []                   # not-testable rows, for their own CSV
        self.libraries = []                 # Photos library census
        self.untestable = Counter()         # SKIPPED files by extension
        self.counts = {v: 0 for v in VERDICTS}
        self.scanned = 0
        self.bytes_read = 0

        self._build()
        self.root.after(150, self._drain)

    # -- layout ------------------------------------------------------------
    def _build(self):
        pad = dict(padx=8, pady=6)

        top = ttk.Frame(self.root)
        top.pack(fill="x", **pad)

        ttk.Label(top, text="Folder").pack(side="left")
        self.folder = tk.StringVar()
        ttk.Entry(top, textvariable=self.folder).pack(side="left", fill="x", expand=True, padx=8)
        ttk.Button(top, text="Browse…", command=self.pick).pack(side="left")

        opts = ttk.Frame(self.root)
        opts.pack(fill="x", padx=8)

        self.do_hole = tk.BooleanVar(value=True)
        ttk.Checkbutton(opts, text="Scan for zero-byte holes (reads every byte — slow but this is the point)",
                        variable=self.do_hole).pack(side="left")

        ttk.Label(opts, text="   Hole size at least").pack(side="left")
        self.hole_kb = tk.StringVar(value=str(DEFAULT_HOLE_KB))
        ttk.Spinbox(opts, from_=1, to=1024, width=6, textvariable=self.hole_kb).pack(side="left", padx=4)
        ttk.Label(opts, text="KB").pack(side="left")

        btns = ttk.Frame(self.root)
        btns.pack(fill="x", **pad)
        self.start_btn = ttk.Button(btns, text="Start scan", command=self.start)
        self.start_btn.pack(side="left")
        self.stop_btn = ttk.Button(btns, text="Stop", command=self.stop, state="disabled")
        self.stop_btn.pack(side="left", padx=6)
        self.export_btn = ttk.Button(btns, text="Export CSV…", command=self.export, state="disabled")
        self.export_btn.pack(side="left")

        ttk.Label(btns, text="   Show").pack(side="left")
        self.filt = tk.StringVar(value="All problems")
        cb = ttk.Combobox(btns, textvariable=self.filt, width=16, state="readonly",
                          values=["All problems", "LIBRARY", "DB_DAMAGED", "TRUNCATED",
                                  "HOLE", "MISMATCH", "UNREADABLE"])
        cb.pack(side="left", padx=4)
        cb.bind("<<ComboboxSelected>>", lambda e: self.refresh())

        self.bar = ttk.Progressbar(self.root, mode="determinate")
        self.bar.pack(fill="x", padx=8)

        self.status = tk.StringVar(value="Pick the folder you copied everything into, then Start scan.")
        ttk.Label(self.root, textvariable=self.status).pack(fill="x", padx=8, pady=(4, 0))

        cols = ("verdict", "detail", "size", "path")
        self.tree = ttk.Treeview(self.root, columns=cols, show="headings", selectmode="extended")
        for c, w, t in (("verdict", 110, "Verdict"), ("detail", 330, "What's wrong"),
                        ("size", 100, "Size"), ("path", 520, "File")):
            self.tree.heading(c, text=t)
            self.tree.column(c, width=w, anchor="w")
        vs = ttk.Scrollbar(self.root, orient="vertical", command=self.tree.yview)
        self.tree.configure(yscrollcommand=vs.set)
        self.tree.pack(side="left", fill="both", expand=True, padx=(8, 0), pady=8)
        vs.pack(side="right", fill="y", padx=(0, 8), pady=8)

        self.tree.tag_configure("LIBRARY", foreground="#0B6E4F")
        self.tree.tag_configure("DB_DAMAGED", foreground="#7B1FA2")
        self.tree.tag_configure("TRUNCATED", foreground="#B3261E")
        self.tree.tag_configure("HOLE", foreground="#8A5A12")
        self.tree.tag_configure("MISMATCH", foreground="#1F4E79")
        self.tree.tag_configure("UNREADABLE", foreground="#6E6E6E")

    # -- actions -----------------------------------------------------------
    def pick(self):
        d = filedialog.askdirectory(title="Choose the folder to scan")
        if d:
            self.folder.set(d)

    def start(self):
        root = self.folder.get().strip()
        if not root or not os.path.isdir(root):
            messagebox.showerror(APP, "Pick a folder that exists.")
            return
        try:
            hole_bytes = max(1, int(self.hole_kb.get())) * 1024
        except ValueError:
            messagebox.showerror(APP, "Hole size must be a whole number of KB.")
            return

        self.tree.delete(*self.tree.get_children())
        self.rows.clear()
        self.skipped.clear()
        self.libraries.clear()
        self.untestable.clear()
        self.counts = {v: 0 for v in VERDICTS}
        self.scanned = 0
        self.bytes_read = 0
        self.stop_flag.clear()
        self.bar.configure(mode="indeterminate")
        self.bar.start(12)
        self.start_btn.configure(state="disabled")
        self.stop_btn.configure(state="normal")
        self.export_btn.configure(state="disabled")

        self.worker = threading.Thread(
            target=self._scan, args=(root, hole_bytes, self.do_hole.get()), daemon=True)
        self.worker.start()

    def stop(self):
        self.stop_flag.set()
        self.status.set("Stopping…")

    def _scan(self, root, hole_bytes, do_hole):
        try:
            for kind, payload in walk_and_probe(root, hole_bytes, do_hole,
                                                should_stop=self.stop_flag.is_set):
                self.q.put((kind, payload))
            self.q.put(("done", "stopped" if self.stop_flag.is_set() else "finished"))
        except Exception:
            self.q.put(("fatal", traceback.format_exc()))

    def _drain(self):
        added = 0
        try:
            while True:
                kind, payload = self.q.get_nowait()
                if kind == "row":
                    verdict, detail, size, path = payload
                    self.scanned += 1
                    self.bytes_read += size
                    self.counts[verdict] = self.counts.get(verdict, 0) + 1
                    if verdict == "SKIPPED":
                        self.untestable[untestable_key(path)] += 1
                        self.skipped.append((verdict, detail, size, path))
                    if verdict not in ("OK", "SKIPPED"):
                        self.rows.append((verdict, detail, size, path))
                        if self._visible(verdict):
                            self._insert((verdict, detail, size, path))
                            added += 1
                elif kind == "lib":
                    self.libraries.append(payload)
                elif kind == "libdone":
                    row = library_row(payload)
                    self.rows.append(row)
                    if self._visible("LIBRARY"):
                        self._insert(row)
                        added += 1
                elif kind == "done":
                    self._finish(payload)
                elif kind == "fatal":
                    self._finish("error")
                    messagebox.showerror(APP, payload)
                if added > 400:
                    break
        except queue.Empty:
            pass
        self._status()
        self.root.after(150, self._drain)

    def _visible(self, verdict):
        f = self.filt.get()
        return f == "All problems" or f == verdict

    def _insert(self, row):
        verdict, detail, size, path = row
        self.tree.insert("", "end", tags=(verdict,),
                         values=(verdict, detail, f"{size:,}", path))

    def refresh(self):
        self.tree.delete(*self.tree.get_children())
        for r in self.rows:
            if self._visible(r[0]):
                self._insert(r)

    def _status(self):
        c = self.counts
        gb = self.bytes_read / (1 << 30)
        self.status.set(
            f"Scanned {self.scanned:,} files ({gb:,.1f} GB)   |   "
            f"photo libraries {len(self.libraries):,}   "
            f"db damaged {c.get('DB_DAMAGED',0):,}   "
            f"truncated {c.get('TRUNCATED',0):,}   holes {c.get('HOLE',0):,}   "
            f"mismatched {c.get('MISMATCH',0):,}   unreadable {c.get('UNREADABLE',0):,}   "
            f"|   clean {c.get('OK',0):,}   not testable {c.get('SKIPPED',0):,}"
        )

    def _finish(self, why):
        self.bar.stop()
        self.bar.configure(mode="determinate", value=0)
        self.start_btn.configure(state="normal")
        self.stop_btn.configure(state="disabled")
        self.export_btn.configure(state="normal" if (self.rows or self.skipped) else "disabled")
        if why != "finished":
            return
        msg = (f"Scan complete.\n\n{self.scanned:,} files examined.\n"
               f"{len(self.rows):,} rows to review.\n\n"
               "Click Export CSV. In the results file:\n"
               "  TRUNCATED, HOLE, MISMATCH - damaged files\n"
               "  DB_DAMAGED - a damaged database (inside a Photos Library: leave it there)\n"
               "  UNREADABLE - could not be read at all\n"
               "  LIBRARY - a Photos Library (information, not a fault)\n\n"
               f"{len(self.skipped):,} files could not be tested here. They go in a second "
               "file, the -not-testable list: unverified, which is not the same as fine. "
               "Camera RAW and HEIC always land there, for the Mac to check.")
        if self.untestable:
            top = ", ".join(f"{e or '(none)'} {n:,}"
                            for e, n in self.untestable.most_common(6))
            msg += f"\n\nNot testable, by extension: {top}"
        if self.libraries:
            n = len(self.libraries)
            ranked = sorted(self.libraries, key=_rank_key)
            listing = "\n".join(
                "  {}\n      {:,} originals, bundle {}{} - {}{}".format(
                    i.get("path", "?"), i.get("originals", 0),
                    _size(i.get("total_bytes", 0)),
                    ", {:,} damaged inside".format(i["damaged"]) if i.get("damaged") else "",
                    i.get("db_verdict") or ("no database"
                                            if i.get("db") == "missing" else "database ok"),
                    "  [likely iCloud placeholders]" if _is_placeholder(i) else "")
                for i in ranked[:8])
            more = f"\n  ...and {n - 8} more (see the LIBRARY rows)" if n > 8 else ""
            msg += ("\n\n" + "=" * 52 +
                    f"\n{n} Apple Photos librar{'y' if n == 1 else 'ies'} found, "
                    "ranked by originals.\nMOVE EVERY ONE out of the scanning area "
                    "before you run any duplicate\nfinder - that is what destroys a "
                    "library.\n\n" + listing + more)
        messagebox.showinfo(APP, msg)

    def export(self):
        if not (self.rows or self.skipped):
            return
        default = f"probe-results-{datetime.now():%Y%m%d-%H%M}.csv"
        path = filedialog.asksaveasfilename(
            title="Save results", defaultextension=".csv",
            initialfile=default, filetypes=[("CSV", "*.csv")])
        if not path:
            return
        try:
            write_results_csv(path, self.rows)
        except OSError as e:
            messagebox.showerror(APP, f"Could not write the file:\n{e}")
            return
        msg = f"Saved {len(self.rows):,} rows to\n{path}"
        if self.skipped:
            ntcsv = untestable_csv_path(path)
            try:
                write_results_csv(ntcsv, self.skipped)
                msg += (f"\n\nNot-testable list ({len(self.skipped):,} files) saved to\n"
                        f"{ntcsv}")
            except OSError as e:
                msg += f"\n\n(could not write the not-testable list: {e})"
        if self.libraries:
            # The same inventory the console writes: one ranked row per library.
            # Phase 5 of the checklist sends the user back to this file.
            libcsv = library_csv_path(path)
            try:
                write_library_csv(libcsv, self.libraries)
                msg += (f"\n\nPhotos library inventory ({len(self.libraries):,} "
                        f"librar{'y' if len(self.libraries) == 1 else 'ies'}, ranked) saved to\n{libcsv}")
            except OSError as e:
                msg += f"\n\n(could not write the library inventory: {e})"
        messagebox.showinfo(APP, msg)


# ----------------------------------------------------------------------------
# Console fallback - used automatically if Tkinter is unavailable, or with --cli
# ----------------------------------------------------------------------------

def run_cli(target, hole_bytes=DEFAULT_HOLE_KB * 1024, do_hole=True, out=None):
    if not os.path.isdir(target):
        print(f"Not a folder: {target}")
        return 2
    out = out or os.path.join(
        os.getcwd(), f"probe-results-{datetime.now():%Y%m%d-%H%M}.csv")
    counts = {v: 0 for v in VERDICTS}
    rows = []
    skipped = []
    libraries = []
    untestable = Counter()
    scanned = 0
    print(f"{APP} - version {VERSION}\nScanning {target}\nWriting to {out}\nCtrl+C to stop.\n")
    try:
        for kind, payload in walk_and_probe(target, hole_bytes, do_hole):
            if kind == "lib":
                libraries.append(payload)
                print(f"  PHOTOS LIBRARY  {payload['path']}\n"
                      f"                  (detected by {payload['how']})")
            elif kind == "libdone":
                row = library_row(payload)
                rows.append(row)
                counts["LIBRARY"] = counts.get("LIBRARY", 0) + 1
            elif kind == "row":
                verdict, detail, size, p_ = payload
                scanned += 1
                counts[verdict] = counts.get(verdict, 0) + 1
                if verdict == "SKIPPED":
                    untestable[untestable_key(p_)] += 1
                    skipped.append(payload)
                if verdict not in ("OK", "SKIPPED"):
                    rows.append(payload)
                    print(f"  {verdict:<11} {p_}\n              {detail}")
                if scanned % 2000 == 0:
                    print(f"  ... {scanned:,} files, {len(rows):,} flagged")
    except KeyboardInterrupt:
        print("\nStopped early.")
    write_results_csv(out, rows)
    print(f"\n{scanned:,} files examined, {len(rows):,} flagged.")
    print("  db damaged {DB_DAMAGED:,}   truncated {TRUNCATED:,}   holes {HOLE:,}   "
          "mismatched {MISMATCH:,}   unreadable {UNREADABLE:,}"
          .format(**{k: counts.get(k, 0) for k in VERDICTS}))
    print(f"  clean {counts.get('OK', 0):,}   not testable {counts.get('SKIPPED', 0):,}"
          "   <- camera RAW and HEIC always land here")
    for line in format_untestable(untestable, counts.get('SKIPPED', 0)):
        print(line)
    for line in format_library_survey(libraries):
        print(line)
    if libraries:
        libcsv = library_csv_path(out)
        try:
            write_library_csv(libcsv, libraries)
            print(f"  Library inventory: {libcsv}")
        except OSError as e:
            print(f"  (could not write the library inventory: {e})")
    if skipped:
        ntcsv = untestable_csv_path(out)
        try:
            write_results_csv(ntcsv, skipped)
            print(f"  Not-testable list: {ntcsv}")
        except OSError as e:
            print(f"  (could not write the not-testable list: {e})")
    print(f"\nResults: {out}")
    return 0


def main():
    args = [a for a in sys.argv[1:]]
    want_cli = "--cli" in args
    if want_cli:
        args.remove("--cli")

    if not HAVE_TK and not want_cli:
        print("Tkinter is not available, so the window cannot open.\n"
              "On Windows, reinstall Python from python.org and keep\n"
              "'tcl/tk and IDLE' ticked. Falling back to console mode.\n")
        want_cli = True

    if want_cli:
        target = args[0] if args else input("Folder to scan: ").strip().strip('"')
        return run_cli(target)

    root = tk.Tk()
    for theme in ("vista", "winnative", "clam"):
        try:
            ttk.Style().theme_use(theme)
            break
        except tk.TclError:
            continue
    ProbeApp(root)
    root.mainloop()
    return 0


if __name__ == "__main__":
    sys.exit(main())
