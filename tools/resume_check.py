"""Start, interrupted over and over: stopped with the Stop button, and the
program killed outright (as a closed window or a power cut would), at random
moments, then started again each time, until Start has run to the end. What
it leaves must be the same as one uninterrupted run (tools/auto_check.py):
every stage done, every repair the same, WORK checked against its checksums,
nothing left half-done.

    python tools/resume_check.py <automatic-recovery-probe> <work folder> [seed] [interruptions]
"""
import os
import random
import shutil
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from auto_check import FAILS, PASSES, Program, build_disk, check, read_csv  # noqa: E402


def run_clean(exe, work):
    """One uninterrupted Start, for the fates to compare with."""
    img, mp, media = build_disk(work)
    home = os.path.join(work, "home")
    os.makedirs(home)
    w = os.path.join(work, "SPARE", "WORK")
    os.makedirs(os.path.dirname(w))
    prog = Program(exe, home, img)
    try:
        prog.do(action="set", mapfile=mp, auto_image_dir=os.path.join(work, "IMAGE"), auto_work=w,
                auto_copy2=os.path.join(work, "COPY 2"), auto_videos="1")
        prog.do(action="auto-start")
        prog.wait()
    finally:
        prog.close()
    log = read_csv(os.path.join(work, "SPARE", "REPAIR", "auto-repair-log.csv"))
    return {r["path_in_work"]: r["result"] for r in log}


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    exe, work = sys.argv[1], sys.argv[2]
    seed = int(sys.argv[3]) if len(sys.argv) > 3 else 1
    times = int(sys.argv[4]) if len(sys.argv) > 4 else 12
    rng = random.Random(seed)
    shutil.rmtree(work, ignore_errors=True)
    os.makedirs(work)
    reference = run_clean(exe, os.path.join(work, "clean"))
    base = os.path.join(work, "broken-up")
    os.makedirs(base)
    img, mp, media = build_disk(base)
    home = os.path.join(base, "home")
    os.makedirs(home)
    spare = os.path.join(base, "SPARE")
    os.makedirs(spare)
    w = os.path.join(spare, "WORK")
    prog = Program(exe, home, img)
    prog.do(action="set", mapfile=mp, auto_image_dir=os.path.join(base, "IMAGE"), auto_work=w,
            auto_copy2=os.path.join(base, "COPY 2"), auto_videos="1")
    kills = stops = 0
    finished = None
    try:
        for n in range(times + 40):
            r = prog.do(action="auto-start")
            if not r.get("ok"):
                print("Start refused: %s" % r.get("error"), flush=True)
                break
            if n < times:
                # Interrupted in the middle of a stage picked at random among
                # those still to come: as soon as it is seen running, after a
                # moment more.
                s = prog.get("api/state")
                left = [x["key"] for x in s["auto"]["stages"][s["auto"]["resume_at"]:]]
                target = rng.choice(left[:2]) if left else None
                where = "-"
                deadline = time.time() + 600
                while time.time() < deadline:
                    s = prog.get("api/state")
                    if (s.get("job") or {}).get("state") != "running":
                        break
                    running = [x["key"] for x in s["auto"]["stages"] if x["state"] in ("running", "waiting")]
                    if running and (running[0] == target or target not in left):
                        where = running[0]
                        break
                    if running and target in left and left.index(running[0]) > left.index(target):
                        where = running[0]
                        break
                    time.sleep(0.02)
                time.sleep(rng.uniform(0, 0.4))
                now = prog.get("api/state")
                if (now.get("job") or {}).get("state") != "running":
                    print("(not running when due: target %s, job %s, resume at %s)" % (target, (now.get("job") or {}).get("state"),
                                                                                     now["auto"]["resume_at"]), flush=True)
                    if now["auto"]["resume_at"] >= len(now["auto"]["stages"]):
                        finished = now
                        break
                    continue
                if rng.random() < 0.5:
                    prog.do(action="stop")
                    prog.wait(600)
                    stops += 1
                    print("stopped during %s" % where, flush=True)
                else:
                    prog.p.kill()
                    prog.p.wait()
                    prog.out.close()
                    kills += 1
                    print("killed during %s" % where, flush=True)
                    prog = Program(exe, home, img)
                continue
            s = prog.wait(1800)
            if (s.get("job") or {}).get("state") == "done" or s["auto"]["resume_at"] >= len(s["auto"]["stages"]):
                finished = s
                break
        if finished is None or finished["auto"]["resume_at"] < len(finished["auto"]["stages"]):
            # Carry on to the end.
            prog.do(action="auto-start")
            finished = prog.wait(1800)
        print("%d stops, %d kills" % (stops, kills), flush=True)
        stages = {x["key"]: x for x in finished["auto"]["stages"]}
        for k, x in stages.items():
            print("  %-9s %-8s %s" % (k, x["state"], (x["summary"] or [""])[0][:140]))
        check(all(x["state"] in ("done", "skipped") for x in stages.values()), "every stage is done or skipped in the end")
        check(stages["verify2"]["state"] == "done" or stages["putback"]["state"] != "done",
              "the copies were checked again after the put-back")
        repair = os.path.join(spare, "REPAIR")
        log = read_csv(os.path.join(repair, "auto-repair-log.csv"))
        latest = {}
        for r in log:
            latest[r["path_in_work"]] = r["result"]
        for path, fate in sorted(reference.items()):
            check(latest.get(path) == fate, "%s: %s, as in one run without a break (%s)" % (path, latest.get(path), fate))
        # Nothing half-done is left.
        check(not os.path.exists(os.path.join(repair, "~auto-repair-working")), "no repair left half-done in REPAIR")
        returns = read_csv(os.path.join(w, "repair-returns.csv"))
        returned = [r["path_in_work"] for r in returns if r["action"] == "RETURNED"]
        check(len(returned) == len(set(returned)), "every repaired file was put back once (%d)" % len(returned))
        auto = os.path.join(repair, "repaired", "auto")
        names = os.listdir(auto) if os.path.isdir(auto) else []
        ids = [n.split("[")[1].split("]")[0] for n in names if "[" in n]
        check(len(ids) == len(set(ids)), "one repaired version per file in REPAIR\\repaired\\auto")
        twin = open(os.path.join(w, "DCIM", "IMG_0006.JPG"), "rb").read()
        check(twin == media["IMG_0006.JPG"], "the photo with an intact twin is exactly the original in WORK")
        # And the copies hold, checked again from scratch.
        r = prog.do(action="verify", folder=w)
        if r.get("ok"):
            s = prog.wait(600)
            j = s.get("job") or {}
            check(j.get("state") == "done" and not j.get("error"), "a fresh check of WORK passes: %s" %
                  ((j.get("summary") or [""])[0][:150]))
    finally:
        prog.close()
    print("\n%d of %d checks passed" % (len(PASSES), len(PASSES) + len(FAILS)))
    return 1 if FAILS else 0


if __name__ == "__main__":
    sys.exit(main())
