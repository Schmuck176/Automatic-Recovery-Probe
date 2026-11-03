"""The hand-off's Mac script ("Repair on the Mac.command"), run by zsh for
real, with stand-ins for the Mac's own tools (sips, avconvert, caffeinate,
and mktemp as the Mac's takes -t): every kind of file it tries, what it says,
and that run again it keeps what is in "returned" - what an earlier run made,
and what the person saved there by hand.

    python3 tools/mac_script_check.py workdir

Needs zsh (every Mac has it; on Linux: apt-get install zsh). The script is
read from src/handoff.rs, exactly as the program writes it.
"""
import hashlib
import os
import shutil
import stat
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
FAILS = []
PASSES = []


def check(ok, what):
    (PASSES if ok else FAILS).append(what)
    print(("ok   " if ok else "FAIL ") + what, flush=True)
    return ok


def the_script():
    s = open(os.path.join(HERE, "..", "src", "handoff.rs"), encoding="utf-8").read()
    start = 'pub const MAC_SCRIPT_TEXT: &str = r#"'
    i = s.index(start) + len(start)
    return s[i:s.index('"#;', i)]


SIPS = r'''#!/usr/bin/env python3
# sips -s format FMT IN --out OUT: a file with UNDECODABLE in it does not decode.
import sys
a = sys.argv[1:]
fmt, src, out = a[a.index("-s") + 2], a[a.index("-s") + 3], a[a.index("--out") + 1]
data = open(src, "rb").read()
if b"UNDECODABLE" in data:
    print("Error: cannot decode " + src, file=sys.stderr)
    sys.exit(1)
open(out, "wb").write(b"sips " + fmt.encode() + b" of " + data)
'''

AVCONVERT = r'''#!/usr/bin/env python3
# As the Mac's: writes over no file unless told to (--replace); an older one
# (STUB_OLD_AVCONVERT) does not know --disableMetadataFilter.
import os, sys
a = sys.argv[1:]
src, out = a[a.index("--source") + 1], a[a.index("--output") + 1]
if "--disableMetadataFilter" in a and os.environ.get("STUB_OLD_AVCONVERT"):
    print("avconvert: unrecognized option --disableMetadataFilter", file=sys.stderr)
    sys.exit(1)
if os.path.exists(out) and "--replace" not in a:
    print("Error: the output file exists", file=sys.stderr)
    sys.exit(1)
data = open(src, "rb").read()
if b"UNDECODABLE" in data:
    sys.exit(1)
open(out, "wb").write(b"avconvert " + " ".join(a[2:]).encode() + b"\n" + data)
'''

MKTEMP = r'''#!/usr/bin/env python3
# The Mac's mktemp: -t PREFIX makes PREFIX.XXXXXXXX in $TMPDIR (-d: a folder).
import os, sys, tempfile
a = sys.argv[1:]
prefix = a[a.index("-t") + 1] + "."
base = os.environ.get("TMPDIR", "/tmp")
if "-d" in a:
    print(tempfile.mkdtemp(prefix=prefix, dir=base))
else:
    fd, p = tempfile.mkstemp(prefix=prefix, dir=base)
    os.close(fd)
    print(p)
'''


def stub(d, name, text):
    p = os.path.join(d, name)
    with open(p, "w") as f:
        f.write(text)
    os.chmod(p, os.stat(p).st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def hashes(root, skip=None):
    out = {}
    for d, dirs, files in os.walk(root):
        rel_d = os.path.relpath(d, root)
        if skip and (rel_d == skip or rel_d.startswith(skip + os.sep)):
            continue
        for f in files:
            p = os.path.join(d, f)
            out[os.path.relpath(p, root)] = hashlib.sha256(open(p, "rb").read()).hexdigest()
    return out


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    work = os.path.abspath(sys.argv[1])
    shutil.rmtree(work, ignore_errors=True)
    zsh = shutil.which("zsh")
    if not check(zsh is not None, "zsh is there to run the script"):
        print("\n%d of %d checks passed" % (len(PASSES), len(PASSES) + len(FAILS)))
        return 1
    stubs = os.path.join(work, "stubs")
    tmp = os.path.join(work, "tmp")
    mac = os.path.join(work, "stick", "1 For the Mac")
    for d in (stubs, tmp, os.path.join(mac, "Photos"), os.path.join(mac, "Report [R0007].pages", "Data")):
        os.makedirs(d)
    stub(stubs, "sips", SIPS)
    stub(stubs, "avconvert", AVCONVERT)
    stub(stubs, "mktemp", MKTEMP)
    stub(stubs, "caffeinate", "#!/bin/sh\nexit 0\n")
    script = os.path.join(mac, "Repair on the Mac.command")
    with open(script, "w", newline="\n") as f:
        f.write(the_script())
    files = {
        "a [R0001].heic": b"an iPhone photo",
        "Photos/b [R0002].jpg": b"a photo in a folder",
        "c [R0003].jpg": b"UNDECODABLE photo",
        "d [R0004].mov": b"a video",
        "e [R0005].cr2": b"a camera RAW",
        "g [R0008].docx": b"a Word document",
        "Report [R0007].pages/Index.zip": b"a Pages document",
        "Report [R0007].pages/Data/pic.jpg": b"a picture inside it",
        "READ ME.txt": b"read me",
        "._a [R0001].heic": b"Mac housekeeping",
    }
    for rel, data in files.items():
        with open(os.path.join(mac, *rel.split("/")), "wb") as f:
            f.write(data)
    # A version the person saved by hand, of the photo the Mac cannot decode.
    by_hand = os.path.join(mac, "returned", "repaired", "c [R0003].jpg")
    os.makedirs(os.path.dirname(by_hand))
    with open(by_hand, "wb") as f:
        f.write(b"saved by hand in Preview")
    originals = hashes(mac, skip="returned")
    env = dict(os.environ, PATH=stubs + os.pathsep + os.environ.get("PATH", ""), TMPDIR=tmp, HOME=work)
    env.pop("STUB_OLD_AVCONVERT", None)
    returned = os.path.join(mac, "returned")

    def run(what, **extra):
        r = subprocess.run([zsh, script], cwd=work, env=dict(env, **extra), capture_output=True, text=True, timeout=300)
        print("--- %s:\n%s%s" % (what, r.stdout, r.stderr))
        return r

    def there(rel):
        return os.path.join(returned, *rel.split("/"))

    def read(rel):
        try:
            return open(there(rel), "rb").read()
        except OSError:
            return None

    made = ["repaired/a [R0001].heic", "repaired/b [R0002].jpg", "repaired/d [R0004].mov", "salvaged/e [R0005] (as TIFF).tif"]
    done = "Done: 3 repaired, 1 salvaged, 1 not repaired, 2 to do by hand."
    for n, (what, extra) in enumerate([("first run", {}), ("run again", {}), ("run again, with an older avconvert", {"STUB_OLD_AVCONVERT": "1"})]):
        r = run(what, **extra)
        check(r.returncode == 0, "%s: the script runs to its end (code %d)" % (what, r.returncode))
        check(done in r.stdout, "%s: it says %r" % (what, done))
        for rel in made:
            check(bool(read(rel)), "%s: %s is in returned" % (what, rel))
        check(read("repaired/c [R0003].jpg") == b"saved by hand in Preview", "%s: the version saved there by hand is kept as it was" % what)
        check(b"sips heic of an iPhone photo" == read("repaired/a [R0001].heic"), "%s: the HEIC is saved again by the Mac's decoder" % what)
        video = read("repaired/d [R0004].mov") or b""
        if n < 2:
            check(b"--disableMetadataFilter" in video, "%s: the video keeps when and where it was recorded" % what)
        else:
            check(video.startswith(b"avconvert ") and b"--disableMetadataFilter" not in video and video.endswith(b"a video"),
                  "%s: the video is made all the same, without the option it does not know" % what)
        check(not os.path.exists(there("~recovery-probe-copying.tmp")), "%s: nothing half-made is left in returned" % what)
        check(not os.path.exists(there("repaired/pic.jpg")), "%s: a file inside the Pages document is not tried by itself" % what)
        check(not [f for f in os.listdir(os.path.join(returned, "repaired")) + os.listdir(os.path.join(returned, "salvaged"))
                   if "READ ME" in f or f.startswith("._")], "%s: READ ME and the Mac's housekeeping are not tried" % what)
        check(hashes(mac, skip="returned") == originals, "%s: the files it tried are not changed" % what)
        log = open(there("mac-repair-log.txt"), encoding="utf-8").read()
        check(log.count("Done: 3 repaired, 1 salvaged, 1 not repaired, 2 by hand") == n + 1, "%s: the log says what it did" % what)
        check("Report [R0007].pages: by hand" in log and "c [R0003].jpg: not repaired" in log, "%s: the log names each file" % what)
    print("\n%d of %d checks passed" % (len(PASSES), len(PASSES) + len(FAILS)))
    return 1 if FAILS else 0


if __name__ == "__main__":
    sys.exit(main())
