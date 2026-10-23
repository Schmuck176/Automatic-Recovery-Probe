"""On Windows: Get ready's "Download it" for qpdf, for real. The program
downloads qpdf 12.3.2 from qpdf's own GitHub release with Windows' own
curl.exe, checks it by its size and SHA-256, unpacks it with Windows' own
tar.exe into its own tools folder, and finds it there.

    python tools/download_check.py automatic-recovery-probe.exe workdir
"""
import os
import shutil
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from auto_check import FAILS, PASSES, Program, check  # noqa: E402


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    exe, work = sys.argv[1], sys.argv[2]
    shutil.rmtree(work, ignore_errors=True)
    home = os.path.join(work, "home")
    os.makedirs(home)
    own = os.path.join(home, "tools")
    prog = Program(exe, home, None)
    try:
        r = prog.do(action="tool-download", tool="qpdf")
        check("job" in r, "the download starts (%s)" % r)
        s = prog.wait(900)
        j = s.get("job") or {}
        print("\n".join(j.get("lines") or []))
        check(j.get("state") == "done", "the download finishes (%s %s)" % (j.get("state"), j.get("error") or ""))
        q = next((t for t in prog.get("api/tools") if t["key"] == "qpdf"), None) or {}
        path = q.get("path") or ""
        check(os.path.normcase(path).startswith(os.path.normcase(own)), "qpdf is found in the program's own tools folder (%s)" % path)
        check("12.3.2" in (q.get("version") or ""), "it is version 12.3.2 (%s)" % q.get("version"))
        names = os.listdir(own) if os.path.isdir(own) else []
        check("qpdf-12.3.2.zip" in names, "the checked download is kept beside it")
        check(not [n for n in names if n.endswith((".partial", ".unpacking"))], "nothing half-done is left (%s)" % names)
    finally:
        prog.close()
    print("\n%d of %d checks passed" % (len(PASSES), len(PASSES) + len(FAILS)))
    return 1 if FAILS else 0


if __name__ == "__main__":
    sys.exit(main())
