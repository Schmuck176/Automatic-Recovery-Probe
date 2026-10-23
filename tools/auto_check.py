#!/usr/bin/env python3
"""End-to-end check of Start, the whole automatic recovery, through the
program's own page (its web API, as the page uses it).

Builds a damaged test disk like the user's (GPT, exFAT "FINAL 1 TB") holding
real media files made with FFmpeg, damages some of them the way a ddrescue
copy is damaged (zeros where the old drive was never read, named in the
mapfile), then has the program do everything: the image, the map check, the
copying, the checks, the extra checks, the repairs, the put back, the checks
again, the hand-off; then Bring back and Finalise. Every result is compared
with what must happen, given the tools on this machine:

    python tools/auto_check.py <automatic-recovery-probe> <work folder>

Needs FFmpeg (to make the media files). 7-Zip, MP3val, untrunc and qpdf are
used when found, and what they repair is expected only then.
"""

import json
import os
import random
import shutil
import socket
import struct
import subprocess
import sys
import time
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import exfat_image as X  # noqa: E402
import test_mapcheck as T  # noqa: E402

SECTOR = 512
CLUSTER = 4096
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FAILS = []
PASSES = []


def check(ok, what):
    (PASSES if ok else FAILS).append(what)
    print(("ok   " if ok else "FAIL ") + what, flush=True)
    return ok


def have(tool):
    names = {"7z": ["7z", "7zz", "7za"]}.get(tool, [tool])
    return any(shutil.which(n) for n in names)


def ff(*args):
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", *args], check=True)


def pdf_bytes(pages=6):
    """A small but real PDF, with its cross-reference table at the end."""
    objs = []
    kids = " ".join("%d 0 R" % (3 + 2 * i) for i in range(pages))
    objs.append(b"<< /Type /Catalog /Pages 2 0 R >>")
    objs.append(("<< /Type /Pages /Kids [%s] /Count %d >>" % (kids, pages)).encode())
    for i in range(pages):
        text = ("BT /F1 18 Tf 50 750 Td (Page %d of the annual report) Tj ET\n" % (i + 1)) + "% filler\n" * 120
        objs.append(("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents %d 0 R "
                     "/Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> >>"
                     % (4 + 2 * i)).encode())
        stream = text.encode()
        objs.append(b"<< /Length %d >>\nstream\n" % len(stream) + stream + b"\nendstream")
    out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for n, o in enumerate(objs, 1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % n + o + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
    for o in offsets:
        out += b"%010d 00000 n \n" % o
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref)
    return bytes(out)


def fake_raw(preview):
    """A camera RAW in miniature: a TIFF header, sensor data, and the
    camera's JPEG preview inside."""
    head = b"II*\x00\x08\x00\x00\x00" + b"CR\x02\x00" + bytes(500)
    rng = random.Random(9)
    sensor = rng.randbytes(40_000)
    return head + sensor + preview + rng.randbytes(9_000)


def make_media(d):
    os.makedirs(d, exist_ok=True)
    m = {}
    # Three videos from the same "camera" (moov at the end, as phones write it).
    for i in (1, 2, 3):
        p = os.path.join(d, "IMG_000%d.MOV" % i)
        ff("-f", "lavfi", "-i", "testsrc=duration=%d:size=320x240:rate=25" % (3 + i), "-f", "lavfi", "-i",
           "sine=f=440:d=%d" % (3 + i), "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "aac",
           "-shortest", p)
        m["IMG_000%d.MOV" % i] = open(p, "rb").read()
    for i in (5, 6, 7):
        # (Each a different picture: the test pattern's counter differs.)
        p = os.path.join(d, "IMG_000%d.JPG" % i)
        ff("-f", "lavfi", "-i", "testsrc=size=1280x960:rate=1:duration=10", "-vf", "select=eq(n\\,%d)" % i,
           "-frames:v", "1", "-q:v", "2", p)
        m["IMG_000%d.JPG" % i] = open(p, "rb").read()
    p = os.path.join(d, "preview.jpg")
    ff("-f", "lavfi", "-i", "testsrc2=size=1600x1200", "-frames:v", "1", "-q:v", "2", p)
    m["IMG_0100.CR2"] = fake_raw(open(p, "rb").read())
    p = os.path.join(d, "song.mp3")
    ff("-f", "lavfi", "-i", "sine=f=440:d=12", "-c:a", "libmp3lame", "-b:a", "128k", p)
    m["song.mp3"] = open(p, "rb").read()
    p = os.path.join(d, "track.flac")
    ff("-f", "lavfi", "-i", "sine=f=330:d=8", "-c:a", "flac", p)
    m["track.flac"] = open(p, "rb").read()
    m["report.pdf"] = pdf_bytes()
    m["report.docx"] = open(os.path.join(ROOT, "tests", "seeds", "report.docx"), "rb").read()
    m["notes.txt"] = b"Shopping list\r\n" * 300
    return m


def build_disk(work):
    media = make_media(os.path.join(work, "media"))
    v = X.Volume(40 * X.MiB, cluster_size=CLUSTER, label="FINAL 1 TB", serial=0x5E1F7E58)
    for folder in ["DCIM", "Backup", "Backup/Phone", "Music", "Documents", "RAW"]:
        v.mkdir(folder)
    for i in (1, 2, 3):
        v.add_file("DCIM/IMG_000%d.MOV" % i, media["IMG_000%d.MOV" % i])
    for i in (5, 6, 7):
        v.add_file("DCIM/IMG_000%d.JPG" % i, media["IMG_000%d.JPG" % i])
    v.add_file("Backup/Phone/IMG_0006.JPG", media["IMG_0006.JPG"])  # an intact twin of a damaged photo
    v.add_file("RAW/IMG_0100.CR2", media["IMG_0100.CR2"])
    v.add_file("Music/song.mp3", media["song.mp3"])
    v.add_file("Music/track.flac", media["track.flac"])
    v.add_file("Documents/report.pdf", media["report.pdf"])
    v.add_file("Documents/report.docx", media["report.docx"])
    v.add_file("Documents/notes.txt", media["notes.txt"])
    disk = X.Disk(v, 48 * X.MiB, efi_start_lba=40, efi_size=1 * X.MiB, vol_start_lba=40 + 2048 + 8, seed=7)
    t = disk.truth()
    node = {n["path"]: n for n in t["nodes"]}
    areas = []

    def lose(path, file_off, length):
        n = node[path]
        start = T.disk_pos(t, n, file_off)
        assert start % SECTOR == 0 and length % SECTOR == 0
        # (Within one cluster, so it is one place on the disk.)
        assert file_off // CLUSTER == (file_off + length - 1) // CLUSTER
        areas.append((start, length, "-"))

    def last_cluster(data):
        return (len(data) - 1) // CLUSTER * CLUSTER

    # The video's index (moov, at its end) lost: untrunc's job.
    mov = media["IMG_0002.MOV"]
    moov = mov.rfind(b"moov") - 4
    for c in range(moov // CLUSTER * CLUSTER, len(mov), CLUSTER):
        lose("DCIM\\IMG_0002.MOV", c, min(CLUSTER, (len(mov) - c + SECTOR - 1) // SECTOR * SECTOR))
    # A photo whose end was never read: its end marker goes back.
    j5 = media["IMG_0005.JPG"]
    lose("DCIM\\IMG_0005.JPG", last_cluster(j5), min(CLUSTER, (len(j5) - last_cluster(j5) + SECTOR - 1) // SECTOR * SECTOR))
    # A photo with an intact twin elsewhere: restored exactly.
    lose("DCIM\\IMG_0006.JPG", 8 * CLUSTER, CLUSTER)
    # A hole in the middle of a photo with no twin: it opens with part grey.
    lose("DCIM\\IMG_0007.JPG", 6 * CLUSTER, 2 * SECTOR)
    # The RAW's header: its preview is salvaged.
    lose("RAW\\IMG_0100.CR2", 0, SECTOR)
    # Holes in the sound: MP3val, FLAC re-encoded.
    lose("Music\\song.mp3", 20 * CLUSTER, CLUSTER)
    lose("Music\\track.flac", 10 * CLUSTER, CLUSTER)
    # The PDF's cross-reference table at its end: qpdf rebuilds it.
    pdf = media["report.pdf"]
    lose("Documents\\report.pdf", last_cluster(pdf), (len(pdf) - last_cluster(pdf) + SECTOR - 1) // SECTOR * SECTOR)
    # The docx's own index (zip central directory) at its end: 7-Zip rebuilds it.
    dx = media["report.docx"]
    cd = dx.rfind(b"PK\x01\x02")
    first_cd = dx.find(b"PK\x01\x02")
    s = first_cd // SECTOR * SECTOR
    if s // CLUSTER != (len(dx) - 1) // CLUSTER:
        s = last_cluster(dx)
    lose("Documents\\report.docx", s, (len(dx) - s + SECTOR - 1) // SECTOR * SECTOR)
    del cd
    areas.sort()
    img = os.path.join(work, "disk.img")
    disk.save(img)
    with open(img, "r+b") as f:
        for st, n, _ in areas:
            f.seek(st)
            f.write(bytes(n))
    mp = os.path.join(work, "1TB test disk.log")
    X.write_mapfile(mp, t["gpt_disk_size"], areas)
    return img, mp, media


# ---------------------------------------------------------------- talking to the program

class Program:
    def __init__(self, exe, home, disk):
        s = socket.socket()
        s.bind(("127.0.0.1", 0))
        self.port = s.getsockname()[1]
        s.close()
        self.key = "k" * 32
        self.out = open(os.path.join(home, "server-output.txt"), "w")
        self.p = subprocess.Popen([exe, "ui", "--port", str(self.port), "--token", self.key, "--no-browser", "--home", home,
                                   "--test-disk", disk], stdout=self.out, stderr=subprocess.STDOUT)
        self.base = "http://127.0.0.1:%d/%s/" % (self.port, self.key)
        for _ in range(100):
            try:
                self.get("api/state")
                return
            except Exception:
                time.sleep(0.1)
        raise SystemExit("the program did not start")

    def get(self, path):
        with urllib.request.urlopen(self.base + path, timeout=30) as r:
            return json.loads(r.read().decode())

    def do(self, **fields):
        data = urllib.parse.urlencode(fields).encode()
        req = urllib.request.Request(self.base + "api/do", data=data, headers={"X-Key": self.key})
        with urllib.request.urlopen(req, timeout=60) as r:
            return json.loads(r.read().decode())

    def wait(self, limit=1800):
        t = time.time()
        last = ""
        while time.time() - t < limit:
            s = self.get("api/state")
            j = s.get("job") or {}
            line = (j.get("lines") or [""])[-1] if j.get("lines") else ""
            if line != last:
                last = line
            if j.get("state") != "running":
                return s
            time.sleep(0.5)
        raise SystemExit("the job did not finish in time")

    def close(self):
        self.p.terminate()
        try:
            self.p.wait(10)
        except subprocess.TimeoutExpired:
            self.p.kill()
        self.out.close()


def read_csv(path):
    import csv
    with open(path, encoding="utf-8-sig", newline="") as f:
        return list(csv.DictReader(f))


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    exe = os.path.abspath(sys.argv[1])
    work = os.path.abspath(sys.argv[2])
    if not have("ffmpeg"):
        print("FFmpeg is needed to make the test media")
        return 2
    shutil.rmtree(work, ignore_errors=True)
    os.makedirs(work)
    img, mp, media = build_disk(work)
    home = os.path.join(work, "home")
    os.makedirs(home)
    spare = os.path.join(work, "SPARE")
    os.makedirs(spare)
    w = os.path.join(spare, "WORK")
    tools = {t: have(t) for t in ["7z", "mp3val", "untrunc", "qpdf"]}
    print("tools here:", ", ".join("%s %s" % (k, "yes" if v else "no") for k, v in tools.items()), flush=True)
    prog = Program(exe, home, img)
    try:
        prog.do(action="set", mapfile=mp, auto_image_dir=os.path.join(work, "IMAGE"), auto_work=w,
                auto_copy2=os.path.join(work, "COPY 2"), auto_videos="1")
        r = prog.do(action="auto-start")
        check(r.get("ok"), "Start begins: %s" % r.get("error", ""))
        s = prog.wait()
        j = s["job"]
        print("\n".join(j.get("summary") or []))
        check(j["state"] == "done", "Start ran to the end (%s: %s)" % (j["state"], j.get("error", "")))
        stages = {x["key"]: x for x in s["auto"]["stages"]}
        for k, x in stages.items():
            print("  %-9s %-8s %s" % (k, x["state"], (x["summary"] or [""])[0][:150]))
        for k in ["ready", "image", "mapcheck", "copy", "verify", "scan", "extra", "isolate", "repair", "putback", "verify2",
                  "scan2", "evaluate", "handoff"]:
            check(stages[k]["state"] == "done", "stage %s done" % k)
        check(stages["open"]["state"] in ("skipped", "done"), "stage open skipped off Windows")
        # The steps' own results, as the step buttons would have left them.
        for kind in ["image", "mapcheck", "rescue", "verify", "scan", "isolate", "put-back", "evaluate", "autorepair",
                     "extra", "handoff"]:
            check(kind in s["last"] and s["last"][kind]["state"] in ("done", "failed"),
                  "the steps tab shows %s (%s)" % (kind, s["last"].get(kind, {}).get("state")))
        repair = os.path.join(spare, "REPAIR")
        log = {r["path_in_work"].replace("\\", "/"): r for r in read_csv(os.path.join(repair, "auto-repair-log.csv"))}
        for k, r in log.items():
            print("  %-26s %-13s %s" % (k, r["result"], r["how"][:110]))

        def fate(path):
            return log.get(path, {}).get("result", "(not tried)")

        check(fate("DCIM/IMG_0006.JPG") == "REPAIRED" and log["DCIM/IMG_0006.JPG"]["how"].startswith("an intact copy"),
              "the photo with an intact twin is restored from it")
        check(fate("DCIM/IMG_0005.JPG") == "REPAIRED", "the photo cut short gets its end back")
        check(fate("DCIM/IMG_0007.JPG") in ("SALVAGED", "NOT REPAIRED"), "the photo with a hole in the middle is not "
              "called repaired (%s)" % fate("DCIM/IMG_0007.JPG"))
        check(fate("RAW/IMG_0100.CR2") == "SALVAGED", "the RAW's preview is salvaged")
        check(fate("Music/track.flac") in ("REPAIRED", "NOT REPAIRED"), "the FLAC is tried (%s)" % fate("Music/track.flac"))
        check(fate("Documents/report.pdf") == ("REPAIRED" if tools["qpdf"] else "NOT REPAIRED"),
              "the PDF %s" % ("is rebuilt by qpdf" if tools["qpdf"] else "waits for qpdf"))
        if tools["7z"]:
            check(fate("Documents/report.docx") == "REPAIRED", "the docx is rebuilt by 7-Zip")
        if tools["untrunc"]:
            check(fate("DCIM/IMG_0002.MOV") == "REPAIRED", "the video is rebuilt by untrunc")
        if tools["mp3val"]:
            check(fate("Music/song.mp3") in ("REPAIRED", "NOT REPAIRED"), "the MP3 is tried by MP3val")
        # Put back: every REPAIRED file is in WORK now, its original kept.
        rets = read_csv(os.path.join(w, "repair-returns.csv"))
        back = {r["path_in_work"].replace("\\", "/") for r in rets if r["action"] == "RETURNED"}
        repaired = {k for k, r in log.items() if r["result"] == "REPAIRED"}
        check(repaired and repaired <= back, "every repaired file was put back (%d)" % len(repaired))
        twin = open(os.path.join(w, "DCIM", "IMG_0006.JPG"), "rb").read()
        check(twin == media["IMG_0006.JPG"], "the restored photo in WORK is exactly the original")
        check(os.path.isfile(os.path.join(repair, "originals", "DCIM", "IMG_0006.JPG")), "its damaged original is kept")
        hand = os.path.join(repair, "hand-off")
        check(os.path.isdir(hand), "the hand-off folder is there")
        if os.path.isdir(os.path.join(hand, "1 For the Mac")):
            check(os.path.isfile(os.path.join(hand, "1 For the Mac", "Repair on the Mac.command")), "with the Mac script")
        # Bring back: the Mac (say) repaired the photo with a hole.
        hole = [r for r in read_csv(os.path.join(repair, "repair-list.csv")) if r["path_in_work"].endswith("IMG_0007.JPG")][0]
        name = hole["copied_out_as"].split("\\")[-1]
        ret = os.path.join(hand, "returned", "repaired")
        os.makedirs(ret, exist_ok=True)
        with open(os.path.join(ret, name), "wb") as f:
            f.write(media["IMG_0007.JPG"])
        with open(os.path.join(ret, "._" + name), "wb") as f:
            f.write(b"mac metadata")
        r = prog.do(action="bring-back", **{"from": os.path.join(hand, "returned")})
        check(r.get("ok"), "Bring back begins: %s" % r.get("error", ""))
        s = prog.wait()
        print("\n".join(s["job"].get("summary") or []))
        check(s["job"]["state"] == "done", "Bring back finishes (%s: %s)" % (s["job"]["state"], s["job"].get("error", "")))
        check(open(os.path.join(w, "DCIM", "IMG_0007.JPG"), "rb").read() == media["IMG_0007.JPG"],
              "the brought-back photo is in WORK")
        # Finalise: RECORDS, the second copy, the final checks.
        r = prog.do(action="finalise")
        check(r.get("ok"), "Finalise begins: %s" % r.get("error", ""))
        s = prog.wait()
        print("\n".join(s["job"].get("summary") or []))
        check(s["job"]["state"] == "done", "Finalise finishes (%s: %s)" % (s["job"]["state"], s["job"].get("error", "")))
        c2 = os.path.join(work, "COPY 2", "WORK", "DCIM", "IMG_0007.JPG")
        check(os.path.isfile(c2) and open(c2, "rb").read() == media["IMG_0007.JPG"], "the second copy holds the files")
        check(os.path.isdir(os.path.join(spare, "RECORDS", "results")), "RECORDS is beside WORK")
        hist = read_csv(os.path.join(home, "results", "safety-checks.csv"))
        check(len(hist) >= 5, "every check is in safety-checks.csv (%d)" % len(hist))
        # Start again: nothing left to do.
        r = prog.do(action="auto-start")
        s = prog.wait()
        check(s["job"]["state"] == "done" and "already" in " ".join(s["job"]["summary"]),
              "Start again says it is all done")
    finally:
        prog.close()
    print("\n%d of %d checks passed" % (len(PASSES), len(PASSES) + len(FAILS)))
    return 1 if FAILS else 0


if __name__ == "__main__":
    import urllib.parse  # noqa: F401
    sys.exit(main())
