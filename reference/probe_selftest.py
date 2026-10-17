r"""Self-test for recovery_probe.py — structure, paths and GUI.

claude_1's tool, adopted whole and extended by claude_2. Their AST half is the
right idea and it found three real faults in my v7 on its first run. Four
changes, each because the tool missed something real:

  1. A function passed as a VALUE is not dead code. `sorted(x, key=_rank_key)`
     never syntactically calls _rank_key, so the dead-code check reported a
     function that is load-bearing in three places. A checker that tells you to
     delete live code gets ignored, which costs you the true positives too.
  2. A duplicate LOCAL assignment is now caught. The repeated-statement check
     only looked at Expr nodes and the attribute check only at self.X, so
     `libraries = []` twice in one function fell between them - which is why
     claude_1's own v8 passes 17/17 while carrying exactly that in run_cli.
  3. One duplicated if/elif test was reported twice, because ast.walk descends
     into the orelse chain it has already covered.
  4. The census-record checks build a MINIMAL record again. claude_1's library()
     helper carries every key either version has ever used, which is precisely
     the case that never breaks. The record shape has changed four times in this
     collaboration and every change broke a display path; the test has to be the
     record as it was two versions ago, not as it is today.

Plus a new section: the Windows path helpers, exercised as pure string
functions under a simulated nt. That settles the string logic. It does not
settle \\?\ semantics on real Windows - only Windows does.

Why this exists. Two assistants have been patching this file across many rounds
without being able to run it interactively, and that produces a specific class of
fault: a diff appends a second definition of something that already exists further
up the file, the later one silently wins, and nothing complains. It happened to
both of us with the same two constants.

In claude_1's v7 the two LIBRARY_BUNDLES definitions were not even identical:

    line 147:  7 suffixes, including .aplibrary2 and .photolibrary2
    line 538:  5 suffixes                      <- this one won

So two Aperture/iPhoto bundle types were never detected, and editing the visible
definition at the top of the file would have changed nothing. For a user with
several libraries of unknown vintage, a missed library is the failure that loses
one.

Neither half of this replaces opening the window once on the target machine.

Version: 2026-10-12 (Opus 5.5 review - supersedes every earlier copy,
including probe_selftest_claude_1_and_2.py)

What the 2026-10-12 review found. Every earlier round tested the code's
SHAPE - duplicate definitions, dead functions, the mocked window - and all 32
checks passed. Then the probe was run on real files: real camera RAWs, real
QuickTime movies, ffmpeg-made music and video, ExifTool's sample collection,
and copies of all of them damaged the way ddrescue damages files. It took 3
seconds per photo (a week for the drive) and called dozens of good files
damaged. None of that was visible to a structural test. So PART 3b now pins
down what the probe SAYS about 30-odd kinds of file, good and damaged, and
--verify proves each of those checks can fail. Run against the 2026-09-13
probe, this self-test reports 20 failures.

Usage:  double-click RUN_SELFTEST.bat   (keeps the window open, saves a copy)
   or:  python probe_selftest.py recovery_probe.py
        python probe_selftest.py recovery_probe.py --verify   (deeper; ~1 min)
"""
import sys, os, ast, types, importlib.util, traceback, collections

FAILURES = []
CHECKS = 0


def check(name, fn):
    global CHECKS
    CHECKS += 1
    try:
        fn()
        print(f"  pass   {name}")
    except Exception as e:
        FAILURES.append((name, traceback.format_exc()))
        print(f"  FAIL   {name}  ->  {type(e).__name__}: {e}")


# ===========================================================================
# PART 1 — structure. Pure AST, no import, so it works on a file that crashes.
# ===========================================================================

def structural_report(path):
    src = open(path, encoding="utf-8").read()
    tree = ast.parse(src)
    lines = src.split("\n")
    out = {"shadowed": [], "dup_funcs": [], "dead": [], "dup_branches": [],
           "dup_attrs": [], "unreachable": [], "dup_stmts": [], "dup_assigns": []}

    # --- module-level names assigned more than once
    seen = collections.defaultdict(list)
    for n in tree.body:
        if isinstance(n, ast.Assign):
            for t in n.targets:
                if isinstance(t, ast.Name):
                    seen[t.id].append(n)
    for name, nodes in seen.items():
        if len(nodes) < 2:
            continue
        vals = ["\n".join(lines[n.lineno - 1:n.end_lineno]) for n in nodes]
        literals = []
        for n in nodes:
            try:
                literals.append(ast.literal_eval(n.value))
            except Exception:
                literals.append(None)
        differ = len({repr(v) for v in literals}) > 1
        lost = ""
        if differ and all(isinstance(v, (tuple, list, set, frozenset)) for v in literals):
            winner = set(literals[-1])
            for v in literals[:-1]:
                missing = set(v) - winner
                if missing:
                    lost = f"  SILENTLY DROPPED: {sorted(missing)}"
        out["shadowed"].append(
            (name, [n.lineno for n in nodes], "VALUES DIFFER" if differ else "identical", lost))

    # --- duplicate function definitions at module level
    fseen = collections.defaultdict(list)
    for n in tree.body:
        if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)):
            fseen[n.name].append(n.lineno)
    out["dup_funcs"] = [(k, v) for k, v in fseen.items() if len(v) > 1]

    # --- module-level functions nothing calls
    # Every name that is READ anywhere, not just every name that is called.
    # `sorted(infos, key=_rank_key)` loads _rank_key without calling it, and a
    # decorator or a dispatch table does the same. Counting only ast.Call makes
    # the checker demand you delete working code.
    called = set()
    for n in ast.walk(tree):
        if isinstance(n, ast.Name) and isinstance(n.ctx, ast.Load):
            called.add(n.id)
        elif isinstance(n, ast.Attribute):
            called.add(n.attr)
    for name, linenos in fseen.items():
        if name in ("main", "run_cli") or name.startswith("__"):
            continue
        if name not in called:
            out["dead"].append((name, linenos[0]))

    # --- repeated comparisons in one if/elif chain (second is unreachable)
    for fn in ast.walk(tree):
        if not isinstance(fn, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        # Only start at the head of a chain. ast.walk also yields each elif as
        # an If of its own, so walking every one reports a single duplicated
        # test once per level below it.
        inner = set()
        for node in ast.walk(fn):
            if isinstance(node, ast.If):
                cur = node
                while (len(cur.orelse) == 1 and isinstance(cur.orelse[0], ast.If)):
                    cur = cur.orelse[0]
                    inner.add(id(cur))
        for node in ast.walk(fn):
            if not isinstance(node, ast.If) or id(node) in inner:
                continue
            tests, cur = [], node
            while isinstance(cur, ast.If):
                try:
                    tests.append(ast.dump(cur.test))
                except Exception:
                    pass
                cur = cur.orelse[0] if len(cur.orelse) == 1 and isinstance(cur.orelse[0], ast.If) else None
            dup = [t for t, c in collections.Counter(tests).items() if c > 1]
            if dup:
                out["dup_branches"].append((fn.name, node.lineno, len(dup)))

    # --- same self.X = ... assigned twice in one BLOCK.
    #     Scoped to siblings in one block for the same reason dup_assigns below
    #     is: the whole-function version fired on `self.x = default` followed by
    #     `if cond: self.x = other`, which is healthy, idiomatic code (claude_3
    #     demonstrated it: a benign default-then-override made the self-test
    #     report 1 of 27 FAILED on a probe that ran fine). The fault this check
    #     exists for - a patch re-adding a line that is already there - puts both
    #     assignments in the same block, so nothing real is lost by narrowing it.
    for fn in ast.walk(tree):
        if not isinstance(fn, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        for node in ast.walk(fn):
            body = getattr(node, "body", None)
            if not isinstance(body, list) or len(body) < 2:
                continue
            attrs = collections.Counter()
            for st in body:
                if isinstance(st, ast.Assign):
                    for t in st.targets:
                        if (isinstance(t, ast.Attribute) and isinstance(t.value, ast.Name)
                                and t.value.id == "self"):
                            attrs[t.attr] += 1
            for a, c in attrs.items():
                if c > 1:
                    out["dup_attrs"].append((fn.name, a, c))

    # --- identical consecutive statements in one block (e.g. a repeated
    #     self.x.clear() left behind by a patch). Only flags *adjacent-ish*
    #     repeats within the same block, since a legitimately repeated call
    #     almost never sits twice in the same straight-line run.
    for fn in ast.walk(tree):
        if not isinstance(fn, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        for node in ast.walk(fn):
            body = getattr(node, "body", None)
            if not isinstance(body, list) or len(body) < 2:
                continue
            dumps = []
            for st in body:
                try:
                    dumps.append(ast.dump(st))
                except Exception:
                    dumps.append(None)
            counts = collections.Counter(d for d in dumps if d)
            for d, c in counts.items():
                if c > 1 and d.startswith("Expr("):
                    idx = [i for i, x in enumerate(dumps) if x == d]
                    out["dup_stmts"].append((fn.name, body[idx[1]].lineno))

    # --- the same local name assigned an identical value twice in one block.
    #     `libraries = []` at the top of a function and again eight lines down
    #     is the signature of a patch that re-added a line instead of moving
    #     it. Harmless in itself; it is the same residue that produced the
    #     dangerous constant shadowing, and it sits in the gap between the
    #     Expr-only repeat check and the self.X-only attribute check.
    #     Scoped to SIBLINGS in one block, which is the whole difficulty. The
    #     obvious version - any two identical assignments anywhere in a
    #     function - fires on `run = 0` reset at two points inside a loop and on
    #     `w = csv.writer(fh)` in two separate with-blocks. Both are correct
    #     code. A check that cries wolf on correct code gets ignored, and then
    #     it costs you the true positive as well.
    for fn in ast.walk(tree):
        if not isinstance(fn, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        for node in ast.walk(fn):
            body = getattr(node, "body", None)
            if not isinstance(body, list) or len(body) < 2:
                continue
            seen = {}
            for st in body:
                if not (isinstance(st, ast.Assign) and len(st.targets) == 1
                        and isinstance(st.targets[0], ast.Name)):
                    continue
                try:
                    key = (st.targets[0].id, ast.dump(st.value))
                except Exception:
                    continue
                if key in seen:
                    out["dup_assigns"].append((fn.name, key[0], [seen[key], st.lineno]))
                seen[key] = st.lineno

    # --- statements after an unconditional return in the same block
    for fn in ast.walk(tree):
        if not isinstance(fn, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        for node in ast.walk(fn):
            body = getattr(node, "body", None)
            if not isinstance(body, list):
                continue
            for i, st in enumerate(body[:-1]):
                if isinstance(st, ast.Return):
                    out["unreachable"].append((fn.name, body[i + 1].lineno))
                    break
    return out


def run_structure(path):
    print("STRUCTURE")
    r = structural_report(path)

    def shadowed():
        if r["shadowed"]:
            detail = "; ".join(f"{n} at lines {ls} ({why}){lost}"
                               for n, ls, why, lost in r["shadowed"])
            raise AssertionError(detail)
    check("no module constant defined twice", shadowed)

    def dupfn():
        if r["dup_funcs"]:
            raise AssertionError("; ".join(f"{k} at {v}" for k, v in r["dup_funcs"]))
    check("no function defined twice", dupfn)

    def dead():
        if r["dead"]:
            raise AssertionError("; ".join(f"{k} (line {v})" for k, v in r["dead"]))
    check("no module-level function is never called", dead)

    def branches():
        if r["dup_branches"]:
            raise AssertionError("; ".join(f"{f} line {l}: {n} repeated test(s)"
                                           for f, l, n in r["dup_branches"]))
    check("no repeated test in an if/elif chain", branches)

    def attrs():
        if r["dup_attrs"]:
            raise AssertionError("; ".join(f"{f}: self.{a} assigned {c}x"
                                           for f, a, c in r["dup_attrs"]))
    check("no attribute assigned twice in one function", attrs)

    def dupstmt():
        if r["dup_stmts"]:
            raise AssertionError("; ".join(f"{f} line {l}" for f, l in r["dup_stmts"]))
    check("no identical statement repeated in one block", dupstmt)

    def dupassign():
        if r["dup_assigns"]:
            raise AssertionError("; ".join(f"{f}: {n} = same value at lines {ls}"
                                           for f, n, ls in r["dup_assigns"]))
    check("no local assigned the same value twice in one function", dupassign)

    def unreachable():
        if r["unreachable"]:
            raise AssertionError("; ".join(f"{f} line {l}" for f, l in r["unreachable"]))
    check("no statement after an unconditional return", unreachable)


# ===========================================================================
# PART 2 — GUI under a strict mock Tk
# ===========================================================================

class TclError(Exception):
    pass


class Var:
    def __init__(self, master=None, value=None, **kw):
        self._v = value if value is not None else ""

    def set(self, v):
        self._v = v

    def get(self):
        return self._v


class BooleanVar(Var):
    def __init__(self, master=None, value=False, **kw):
        self._v = bool(value)


class Widget:
    _ALLOWED = {
        "pack", "grid", "place", "configure", "config", "bind", "focus",
        "insert", "delete", "see", "yview", "xview", "set", "get", "state",
        "heading", "column", "selection", "identify_row", "item",
        "tag_configure", "current", "start", "stop", "step", "theme_use",
        "theme_names", "pack_propagate", "grid_columnconfigure",
        "grid_rowconfigure", "columnconfigure", "rowconfigure", "title",
        "geometry", "minsize", "protocol", "after", "mainloop", "destroy",
        "update_idletasks", "winfo_children", "selection_set", "focus_set",
    }

    def __init__(self, *a, **kw):
        self._items = []

    def __getattr__(self, name):
        if name in self._ALLOWED:
            return self._call(name)
        raise AttributeError(f"mock Tk has no attribute {name!r} "
                             f"(add it to Widget._ALLOWED if legitimate)")

    def _call(self, name):
        def f(*a, **kw):
            if name == "insert":
                self._items.append((a, kw))
                return f"I{len(self._items):04d}"
            if name in ("get_children", "winfo_children"):
                return []
            if name == "theme_names":
                return ("clam", "default")
            if name in ("identify_row",):
                return ""
            if name == "selection":
                return ()
            return None
        return f

    def get_children(self, *a):
        return [f"I{i+1:04d}" for i in range(len(self._items))]


def build_mock():
    tk = types.ModuleType("tkinter")
    tk.Tk = lambda *a, **k: Widget()
    tk.StringVar, tk.BooleanVar, tk.TclError = Var, BooleanVar, TclError
    for k, v in dict(END="end", W="w", E="e", N="n", S="s", BOTH="both",
                     X="x", Y="y", LEFT="left", RIGHT="right",
                     TOP="top", BOTTOM="bottom").items():
        setattr(tk, k, v)
    ttk = types.ModuleType("tkinter.ttk")
    for w in ("Button", "Checkbutton", "Combobox", "Entry", "Frame", "Label",
              "Progressbar", "Scrollbar", "Spinbox", "Style", "Treeview",
              "Notebook", "PanedWindow", "Separator", "LabelFrame"):
        setattr(ttk, w, lambda *a, **k: Widget())
    mb = types.ModuleType("tkinter.messagebox")
    mb.showinfo = lambda t, m, **k: None
    mb.showerror = lambda t, m, **k: None
    mb.showwarning = lambda t, m, **k: None
    mb.askyesno = lambda t, m, **k: True
    fd = types.ModuleType("tkinter.filedialog")
    fd.askdirectory = lambda **k: "/tmp/scan-root"
    fd.asksaveasfilename = lambda **k: "/tmp/probe-selftest.csv"
    tk.ttk, tk.messagebox, tk.filedialog = ttk, mb, fd
    for n, m in (("tkinter", tk), ("tkinter.ttk", ttk),
                 ("tkinter.messagebox", mb), ("tkinter.filedialog", fd)):
        sys.modules[n] = m
    return tk


def library(path, originals, files, total_bytes, db_state, how="name",
            hot_wal=False, newest=1700000000, median=2 << 20):
    """A census record with every key either version has used."""
    return {
        "path": path, "how": how, "originals": originals,
        "orig_bytes": int(total_bytes * 0.9), "total_bytes": total_bytes,
        "bytes": total_bytes, "files": files, "median_orig": median,
        "db": {"ok": "database passes its integrity check",
               "damaged": "DB_DAMAGED - database disk image is malformed",
               "missing": "missing"}[db_state],
        "db_bytes": 0 if db_state == "missing" else 5 << 20,
        "db_ok": {"ok": True, "damaged": False, "missing": None}[db_state],
        "db_verdict": {"ok": "OK", "damaged": "DB_DAMAGED", "missing": ""}[db_state],
        "db_detail": "", "hot_wal": hot_wal, "newest": newest,
    }


def minimal_library(path="/W/Old.photoslibrary"):
    """A census record carrying only the keys EVERY version has had.

    This is the case that keeps breaking. library() above builds a maximal
    record - every key either of us has ever added - and a maximal record is
    exactly the one that never fails. The record shape has changed four times
    across this collaboration and each change broke a display path that
    hard-subscripted the new key: claude_1's `orig_bytes`, claude_2's
    `median_orig`, and two before those. Reading each other's diffs caught none
    of them.
    """
    return {"path": path, "how": "name", "originals": 1200, "files": 2400,
            "total_bytes": 3 << 30, "db": "present"}


def run_gui(path):
    print("\nGUI (mock Tk)")
    build_mock()
    spec = importlib.util.spec_from_file_location("probe_under_test", path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules["probe_under_test"] = mod
    spec.loader.exec_module(mod)

    if not hasattr(mod, "ProbeApp"):
        print("  (no ProbeApp class - skipping)")
        return mod

    box = {}
    check("construct ProbeApp", lambda: box.__setitem__("app", mod.ProbeApp(Widget())))
    app = box.get("app")
    if app is None:
        return mod

    libs = [
        library("/W/Pictures/Family.photoslibrary", 41000, 90210, 88 << 30, "ok"),
        library("/W/Backups/OldPhotos", 5000, 12000, 9 << 30, "damaged",
                how="structure (database/ beside originals/)", hot_wal=True),
        library("/W/old/iPhoto.migratedphotolibrary", 800, 1600, 2 << 30, "missing"),
        library("/W/Backups/iCloudCopy.photoslibrary", 90000, 91000, 8 << 30, "ok",
                median=170 * 1024),          # placeholder library
    ]

    def drain():
        for ev in ([("lib", l) for l in libs] +
                   [("row", ("HOLE", "60,000 zero bytes at offset 1,024", 4 << 20, "/W/a.heic")),
                    ("row", ("TRUNCATED", "ends early", 1 << 20, "/W/b.jpg")),
                    ("row", ("DB_DAMAGED", "malformed\nsecond line\nthird", 5 << 20, "/W/db.sqlite")),
                    ("row", ("SKIPPED", "camera RAW", 20 << 20, "/W/c.cr2")),
                    ("row", ("SKIPPED", "HEIC needs the Mac", 3 << 20, "/W/d.heic")),
                    ("row", ("OK", "", 1 << 20, "/W/e.png")),
                    ("row", ("MISMATCH", "content is png", 1 << 10, "/W/f.jpg")),
                    ("row", ("UNREADABLE", "PermissionError", 0, "/W/g.bin")),
                    ("row", ("LIBRARY", "found by name", 1 << 30, "/W/Family.photoslibrary"))] +
                   [("libdone", l) for l in libs]):
            app.q.put(ev)
        app.q.put(("done", "finished"))
        app._drain()
    check("drain: every event type and verdict", drain)

    check("_finish with 4 libraries", lambda: app._finish("finished"))

    def none():
        app.libraries = []
        app._finish("finished")
    check("_finish with no libraries", none)

    def many():
        app.libraries = [library(f"/W/l{i}.photoslibrary", 100 * i, 200 * i,
                                 (i + 1) << 30, "ok") for i in range(12)]
        app._finish("finished")
    check("_finish with 12 libraries", many)

    def minimal():
        app.libraries = [minimal_library()]
        app._finish("finished")
    check("_finish with a minimal census record", minimal)

    def minimal_row():
        row = mod.library_row(minimal_library())
        if not isinstance(row, tuple) or len(row) != 4:
            raise AssertionError(f"library_row returned {row!r}")
    check("library_row with a minimal census record", minimal_row)

    def minimal_survey():
        if hasattr(mod, "format_library_survey"):
            mod.format_library_survey([minimal_library(), minimal_library("/W/B")])
    check("format_library_survey with minimal records", minimal_survey)

    check("_finish when stopped early", lambda: app._finish("stopped"))
    check("_status", lambda: app._status())

    def filters():
        # History worth keeping. This check originally probed for filter_var /
        # _refilter / apply_filter. Neither probe has any of them - both use
        # `filt` and `refresh` - so it executed nothing and printed "pass".
        # claude_2 caught that by reading and fixed it in the round-7 harness;
        # the fix then failed to carry into the round-8 self-test and the
        # vacuous version came back. --verify caught the regression, which is
        # the argument for --verify: a check that cannot fail reads as coverage,
        # and re-reading it is how it survived several rounds.
        var = next((getattr(app, n) for n in ("filt", "filter_var", "filter")
                    if hasattr(app, n)), None)
        redraw = next((getattr(app, n) for n in
                       ("refresh", "_refresh", "_refilter", "apply_filter")
                       if callable(getattr(app, n, None))), None)
        if var is None or redraw is None:
            raise AssertionError(
                "could not identify the filter variable and/or its redraw method. "
                "Add this probe's names to the candidate lists rather than letting "
                "this check pass vacuously.")
        for v in ("All problems", "LIBRARY", "DB_DAMAGED", "TRUNCATED", "HOLE",
                  "MISMATCH", "UNREADABLE", "SKIPPED"):
            var.set(v)
            redraw()
        var.set("All problems")
        redraw()
    check("filter across every verdict", filters)

    def export():
        import tempfile
        out = os.path.join(tempfile.mkdtemp(prefix="probe-selftest-"), "probe-selftest.csv")
        mod.filedialog.asksaveasfilename = lambda **k: out
        app.export()
        if not os.path.exists(out):
            raise AssertionError("export produced no file")
        # The window must write the same ranked library inventory the console
        # does, beside the results file. It did not, for several rounds, while
        # the checklist promised it would and Phase 5 depended on it (claude_3).
        # app.libraries is non-empty here (set by the _finish checks above).
        if app.libraries:
            libcsv = os.path.splitext(out)[0] + "-photo-libraries.csv"
            if not os.path.exists(libcsv):
                raise AssertionError("export wrote the results CSV but no "
                                     "-photo-libraries.csv beside it")
            with open(libcsv, encoding="utf-8-sig") as fh:
                header = fh.readline().strip().split(",")
            for col in ("rank", "median_original_bytes", "unverified_inside",
                        "likely_icloud_placeholders", "database_verdict"):
                if col not in header:
                    raise AssertionError(f"library CSV is missing the {col!r} column")
        # ...and the list of files no test could verify, which the sorting
        # step needs to tell "tested and fine" from "never tested". The drain
        # above fed two SKIPPED rows in, so the list must exist and hold them.
        if hasattr(app, "skipped"):
            ntcsv = os.path.splitext(out)[0] + "-not-testable.csv"
            if not os.path.exists(ntcsv):
                raise AssertionError("export wrote no -not-testable.csv beside the results")
            with open(ntcsv, encoding="utf-8-sig") as fh:
                rows = fh.read().strip().split("\n")
            if len(rows) != 1 + len(app.skipped) or not app.skipped:
                raise AssertionError(f"not-testable list has {len(rows) - 1} rows, "
                                     f"expected {len(app.skipped)}")
    check("export to CSV (results, library inventory and not-testable list)", export)

    def fatal():
        app.q.put(("fatal", "Traceback: synthetic"))
        app._drain()
    check("fatal event handled", fatal)
    return mod


def run_paths(mod):
    r"""The Windows long-path helpers, as pure string functions under a fake nt.

    What this settles: that long_path() prefixes, is idempotent, handles UNC,
    and that display_path() undoes it exactly - including the composition
    survey_library depends on, where a prefixed directory measured against an
    unprefixed base returns a relpath full of '..' and silently zeroes the
    originals count on Windows and nowhere else.

    What it does NOT settle: whether \\?\ behaves on real Windows. Nothing
    runnable here can. If it misbehaves the symptom is UNREADABLE rows with
    FileNotFoundError on deep paths.
    """
    print("\nWINDOWS PATHS (simulated nt)")
    if not hasattr(mod, "long_path"):
        print("  (no long_path - skipping)")
        return
    import ntpath
    have_display = hasattr(mod, "display_path")
    real_name, real_abspath = os.name, os.path.abspath
    deep = "D:\\WORK\\" + "\\".join("d" * 40 for _ in range(9)) + "\\IMG_0001.HEIC"
    try:
        os.name = "nt"
        os.path.abspath = ntpath.abspath

        def prefixes():
            if len(deep) <= 260:
                raise AssertionError("test path is not actually long")
            if not mod.long_path(deep).startswith("\\\\?\\"):
                raise AssertionError("long_path did not prefix a 390-char path")
        check("long_path prefixes a path past MAX_PATH", prefixes)

        def idem():
            once = mod.long_path(deep)
            if mod.long_path(once) != once:
                raise AssertionError("long_path is not idempotent")
        check("long_path is idempotent", idem)

        def unc():
            got = mod.long_path("\\\\NAS\\share\\Lib.photoslibrary")
            if not got.startswith("\\\\?\\UNC\\"):
                raise AssertionError(f"UNC path became {got!r}")
        check("long_path handles a UNC path", unc)

        if have_display:
            def undo():
                base = "D:\\WORK\\Pictures\\Family.photoslibrary"
                if mod.display_path(mod.long_path(base)) != base:
                    raise AssertionError("display_path did not undo long_path")
                if mod.display_path(mod.long_path(deep)) != deep:
                    raise AssertionError("display_path failed on a long path")
                already = "\\\\?\\D:\\already\\prefixed"
                if mod.display_path(already) != "D:\\already\\prefixed":
                    raise AssertionError("display_path failed on a prefixed input")
            check("display_path exactly undoes long_path", undo)

            def relpath_composition():
                base = mod.display_path(mod.long_path("D:\\WORK\\Lib.photoslibrary"))
                rel = ntpath.relpath(base + "\\originals\\0", base)
                if rel.startswith(".."):
                    raise AssertionError(f"relpath under its own base gave {rel!r} "
                                         "- originals would count zero on Windows")
            check("relpath under a cleaned base stays inside it", relpath_composition)
    finally:
        os.name, os.path.abspath = real_name, real_abspath

    def posix_noop():
        r"""The `if os.name != "nt": return p` branch, forced open.

        This check used to run with os.name as the machine found it, which
        made it a check that could only ever pass OFF Windows - and the one
        place it matters that these helpers stay honest is a run ON Windows.
        The block above restores os.name to the real value, so on the target
        machine this asked "is long_path a no-op?" of a machine where
        long_path is supposed to prefix, and it duly reported the correct
        behaviour as a failure: 1 of 32 FAILED on a probe with nothing wrong
        with it. A red check nobody can act on is worse than no check - it
        buries the real ones.

        Simulating posix here tests the branch instead of the host.
        """
        real = os.name
        try:
            os.name = "posix"
            if mod.long_path("/tmp/x") != "/tmp/x":
                raise AssertionError("long_path is not a no-op off Windows")
            if have_display and mod.display_path("/tmp/x") != "/tmp/x":
                raise AssertionError("display_path is not a no-op off Windows")
        finally:
            os.name = real
    check("both helpers are no-ops off Windows", posix_noop)


# ===========================================================================
# PART 3b - verdicts on built fixtures
#
# Everything above tests structure, paths and the window. This part tests
# what the probe actually SAYS about a file, which is where every serious
# fault has been. claude_3 found a real 8 KiB hole reported OK whenever a
# longer zero run trailed it. The 2026-10-12 review ran the probe on real
# files and found the rest: perfectly good FLACs, Panasonic RAWs, old
# QuickTime movies, camera videos with PCM sound, night-sky JPEGs and real
# Canon/Nikon RAWs all reported damaged - and a hole scan slow enough to take
# days. Each fixture below pins one of those down.
#
# Holes are built the way ddrescue leaves them: whole 512-byte sectors of
# zeros, starting on a sector boundary of the file (see SECTOR in the probe).
# ===========================================================================

def run_verdicts(mod):
    print("\nVERDICTS (built fixtures)")
    if not hasattr(mod, "probe_file"):
        print("  (no probe_file - skipping)")
        return
    import tempfile, struct, shutil, sqlite3, time, random
    d = tempfile.mkdtemp(prefix="probe-selftest-")
    NZ = b"\xAB\xCD\xEF\x12" * 512          # 2 KiB of non-zero bytes (and no FF)
    HOLE = b"\x00" * 8192                   # 8 KiB: twice the default threshold
    TRAIL = b"\x00" * 12288                 # a LONGER zero run, reaching EOF

    def sect(b):
        """Pad with non-zero bytes to a whole number of 512-byte sectors, so
        whatever comes next starts where a real bad sector would."""
        return b + b"\x5A" * (-len(b) % 512)

    def write(name, data):
        p = os.path.join(d, name)
        with open(p, "wb") as fh:
            fh.write(data)
        return p

    def probe(name, data):
        return mod.probe_file(write(name, data), 4096, True)[0]

    def expect(name, data, want):
        got = probe(name, data)
        if got != want:
            raise AssertionError(f"{name} gave {got!r}, expected {want!r}")

    def box(typ, payload=b""):
        return struct.pack(">I", 8 + len(payload)) + typ + payload

    def ebml(eid, payload):
        return eid + b"\x01" + len(payload).to_bytes(7, "big") + payload

    # --- the original hole fixtures, now with sector-shaped holes -------------

    def interior_hole_survives_trailing():
        got = probe("a.gz", sect(b"\x1f\x8b" + NZ) + HOLE + NZ + TRAIL)
        if got != "HOLE":
            raise AssertionError(f"8 KiB interior hole + longer trailing zeros gave {got!r}, "
                                 "not HOLE - the false OK is back")
    check("an interior hole is reported even when a longer zero run trails it",
          interior_hole_survives_trailing)

    def trailing_alone_not_hole():
        got = probe("b.gz", sect(b"\x1f\x8b" + NZ) + TRAIL)
        if got != "OK":
            raise AssertionError(f"trailing zero padding alone gave {got!r}, not OK")
    check("trailing zero padding alone is not reported as a hole", trailing_alone_not_hole)

    def hole_across_chunk_boundary():
        real = mod.CHUNK
        mod.CHUNK = 4096                    # so a small file spans several reads
        try:
            got = probe("c.gz", sect(b"\x1f\x8b" + b"\xAB" * 3000) + b"\x00" * 6144 + b"\xAB" * 100)
        finally:
            mod.CHUNK = real
        if got != "HOLE":
            raise AssertionError(f"hole across a read-chunk boundary gave {got!r}, not HOLE")
    check("a hole spanning a read-chunk boundary is reported", hole_across_chunk_boundary)

    def riff(form, body_after_header):
        data = bytearray(body_after_header)
        data[0:12] = b"RIFF" + struct.pack("<I", len(data) - 8) + form
        return bytes(data)

    def webp_hole():
        got = probe("d.webp", riff(b"WEBP", sect(b"\x00" * 12 + NZ) + HOLE + NZ))
        if got != "HOLE":
            raise AssertionError(f"zero-holed WebP gave {got!r}, not HOLE")
    check("a zero hole inside a WebP is reported", webp_hole)

    def wav_zeros_ok():
        got = probe("e.wav", riff(b"WAVE", sect(b"\x00" * 12 + NZ) + HOLE + NZ))
        if got != "OK":
            raise AssertionError(f"WAV with 8 KiB of silence gave {got!r}, not OK")
    check("zeros inside a WAV are not reported as a hole", wav_zeros_ok)

    # --- natural zero runs are not holes -------------------------------------

    def unaligned_zeros_ok():
        # Dark or flat JPEGs, and real Canon CR2 / Nikon NEF files, hold long
        # natural zero runs. They start anywhere; a bad sector starts on one.
        expect("night.gz", sect(b"\x1f\x8b" + NZ) + b"\x77" * 100 + HOLE + NZ, "OK")
    check("a long zero run that does not sit on disk sectors is not a hole", unaligned_zeros_ok)

    # --- known padding is not a hole -----------------------------------------
    # Each padding block is placed exactly on sector boundaries, so that only
    # the padding logic - not the sector rule - stands between it and HOLE.

    def flac(blocks, audio):
        out = b"fLaC"
        for i, (typ, payload) in enumerate(blocks):
            last = 0x80 if i == len(blocks) - 1 else 0
            out += bytes([last | typ]) + len(payload).to_bytes(3, "big") + payload
        return out + audio

    # STREAMINFO, then an APPLICATION block sized so the PADDING block's
    # zeros run from byte 512 to byte 8704 - both sector boundaries.
    FLAC_META = [(0, b"\x12" * 34), (2, b"\x34" * 462), (1, b"\x00" * 8192)]

    def flac_padding_ok():
        expect("song.flac", flac(FLAC_META, b"\xff\xf8" + NZ * 4), "OK")
    check("FLAC padding blocks are not reported as holes", flac_padding_ok)

    def flac_hole():
        expect("song-hole.flac", flac(FLAC_META, sect(b"\xff\xf8" + NZ) + HOLE + NZ), "HOLE")
    check("a real hole in FLAC audio is still reported", flac_hole)

    def id3(total, pad_from=512):
        """An ID3v2.3 tag `total` bytes long: one frame, then zero padding
        starting exactly at `pad_from`."""
        payload = b"\x03" + b"T" * (pad_from - 10 - 10 - 1)
        frames = b"TIT2" + struct.pack(">I", len(payload)) + b"\x00\x00" + payload
        n = total - 10
        body = frames + b"\x00" * (n - len(frames))
        size = bytes([(n >> 21) & 0x7F, (n >> 14) & 0x7F, (n >> 7) & 0x7F, n & 0x7F])
        return b"ID3\x03\x00\x00" + size + body

    def flac_behind_id3():
        expect("tagged.flac", id3(1024) + flac(FLAC_META[:1], b"\xff\xf8" + NZ), "OK")
    check("a FLAC with an ID3 tag in front is not a mismatch", flac_behind_id3)

    def mp3_tag_padding_ok():
        expect("song.mp3", id3(10752) + b"\xff\xfb\x90\x64" + NZ * 3, "OK")
    check("ID3 tag padding in an MP3 is not reported as a hole", mp3_tag_padding_ok)

    def mp3_hole():
        expect("song-hole.mp3", id3(10752) + sect(b"\xff\xfb\x90\x64" + NZ) + HOLE + NZ, "HOLE")
    check("a real hole in MP3 audio is still reported", mp3_hole)

    MOOV = box(b"moov", box(b"mvhd", b"\x01" * 100))
    FTYP504 = box(b"ftyp", b"isom" * 124)   # 504 bytes: the next box's data starts at 512

    def mp4_free_ok():
        expect("clip.mp4", FTYP504 + box(b"free", b"\x00" * 8192) + box(b"mdat", NZ * 4) + MOOV, "OK")
    check("MP4 free/skip padding boxes are not reported as holes", mp4_free_ok)

    def mp4_hole():
        expect("clip-hole.mp4", FTYP504 + box(b"mdat", HOLE + NZ * 2) + MOOV, "HOLE")
    check("a real hole in MP4 picture data is still reported", mp4_hole)

    def mov_pcm_ok():
        entry = struct.pack(">I", 36) + b"sowt" + b"\x01" * 28
        stsd = b"\x00\x00\x00\x00" + struct.pack(">I", 1) + entry
        moov = box(b"moov", box(b"trak", box(b"mdia", box(b"minf", box(b"stbl", box(b"stsd", stsd))))))
        head = box(b"ftyp", b"qt  \x00\x00\x00\x00qt  ") + moov
        fill = b"\x5A" * (-(len(head) + 8) % 512)
        expect("camera.mov", head + box(b"mdat", fill + b"\x00" * 16384 + NZ), "OK")
    check("silence in a MOV with uncompressed (PCM) audio is not a hole", mov_pcm_ok)

    def mov_moov_first():
        expect("old.mov", MOOV + box(b"mdat", NZ * 2), "OK")
    check("a QuickTime movie that starts with moov is not a mismatch", mov_moov_first)

    def mp4_index_lost():
        # The index (moov) normally sits at the end. Here the end was never
        # read: zeros where moov should be. Every size still adds up. (Kept
        # under the 4 KiB hole size on purpose, so that it is this check and
        # not the zero-ending rule that has to catch it.)
        expect("lost-index.mp4", FTYP504 + box(b"mdat", NZ * 4) + b"\x00" * 2048, "TRUNCATED")
    check("an MP4 whose index box was zero-filled is TRUNCATED", mp4_index_lost)

    EBML_HDR = ebml(b"\x1a\x45\xdf\xa3", b"\x42\x86\x81\x01")              # 16 bytes
    SEG_START = 16 + 12                                                    # children of Segment

    def mkv(children):
        return EBML_HDR + ebml(b"\x18\x53\x80\x67", children)

    # an Info element sized so the Void's zeros start at byte 512
    MKV_PRE = ebml(b"\x15\x49\xa9\x66", b"\x5A" * (512 - SEG_START - 12 - 9))

    def mkv_void_ok():
        expect("clip.mkv", mkv(MKV_PRE + ebml(b"\xec", b"\x00" * 8192)
                               + ebml(b"\x1f\x43\xb6\x75", NZ * 2)), "OK")
    check("Matroska Void padding is not reported as a hole", mkv_void_ok)

    def mkv_cut():
        whole = mkv(MKV_PRE + ebml(b"\x1f\x43\xb6\x75", NZ * 4))
        expect("cut.mkv", whole[:-3000], "TRUNCATED")
    check("a Matroska file shorter than its Segment says is TRUNCATED", mkv_cut)

    # --- JPEG end detection ---------------------------------------------------

    JH = (b"\xff\xd8" + b"\xff\xe0" + struct.pack(">H", 16) + b"JFIF\x00" + b"\x01" * 9
          + b"\xff\xda" + struct.pack(">H", 8) + b"\x01" * 6)

    def jpeg_trailer_ok():
        expect("samsung.jpg", JH + NZ * 2 + b"\xff\xd9" + b"SEFH" + b"\x07" * 300 + b"SEFT", "OK")
    check("a complete JPEG with a trailer after its end marker is not TRUNCATED", jpeg_trailer_ok)

    def jpeg_cut():
        expect("cut.jpg", JH + NZ * 2, "TRUNCATED")
    check("a JPEG cut off in its picture data is TRUNCATED", jpeg_cut)

    def jpeg_motion_photo_ok():
        pic = JH + NZ * 2 + b"\xff\xd9"
        video = b"\x5A" * (-len(pic) % 512) + b"\x00" * 8192 + NZ
        expect("motion.jpg", pic + video, "OK")
    check("zeros in a motion photo's attached video are not counted against the photo",
          jpeg_motion_photo_ok)

    def jpeg_hole_with_trailer():
        pic = JH + b"\x5A" * (512 - len(JH)) + HOLE + NZ + b"\xff\xd9"
        expect("hole-trailer.jpg", pic + b"SEFT", "HOLE")
    check("a hole inside the picture of a JPEG with a trailer is still reported",
          jpeg_hole_with_trailer)

    # --- camera RAW -------------------------------------------------------------

    def raw_unknown_headers():
        for name, head in (("p.rw2", b"IIU\x00" + struct.pack("<I", 8)),
                           ("o.orf", b"IIRO" + struct.pack("<I", 8)),
                           ("c.crw", b"II\x1a\x00\x00\x00HEAPCCDR"),
                           ("k.raw", b"\x0070.1 .reV\x00"),
                           ("q.iiq", b"QQQQ")):
            expect(name, head + NZ * 2, "SKIPPED")
    check("RAW types without a known signature are not-testable, not MISMATCH",
          raw_unknown_headers)

    def raw_zero_start():
        expect("z.nef", b"\x00" * 4096 + NZ * 2, "MISMATCH")
    check("a RAW whose first sectors are zeros is still reported", raw_zero_start)

    def raw_hole():
        expect("h.cr2", sect(b"II*\x00" + struct.pack("<I", 8) + NZ) + HOLE + NZ, "HOLE")
    check("a real hole in a RAW file is still reported", raw_hole)

    # --- housekeeping files ---------------------------------------------------

    def system_files():
        ole = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1" + NZ
        expect("._IMG_0001.JPG", b"\x00\x05\x16\x07\x00\x02\x00\x00Mac OS X        " + b"\x00" * 100,
               "SKIPPED")
        expect("Thumbs.db", ole, "SKIPPED")
        expect("holiday.db", ole, "SKIPPED")
    check("Mac '._' files, Thumbs.db and non-SQLite .db files are not damage", system_files)

    # --- databases ------------------------------------------------------------

    def sqlite_verdicts():
        good = os.path.join(d, "good.sqlite")
        con = sqlite3.connect(good)
        con.execute("CREATE TABLE photos (id INTEGER PRIMARY KEY, name TEXT)")
        con.executemany("INSERT INTO photos (name) VALUES (?)", [(f"IMG_{i}",) for i in range(500)])
        con.commit()
        con.close()
        got = mod.probe_file(good, 4096, True)[0]
        if got != "OK":
            raise AssertionError(f"a healthy database gave {got!r}")
        expect("broken.sqlite", b"SQLite format 3\x00" + b"\xff" * 4000, "DB_DAMAGED")
    check("SQLite: a healthy database is OK and a corrupt one DB_DAMAGED", sqlite_verdicts)

    def sqlite_cannot_open():
        got = mod.check_sqlite(os.path.join(d, "no-such-folder", "Photos.sqlite"))[0]
        if got != "UNREADABLE":
            raise AssertionError(f"a database that could not be opened gave {got!r}, "
                                 "not UNREADABLE")
    check("a database that cannot be opened is UNREADABLE, not DB_DAMAGED", sqlite_cannot_open)

    # --- PNG, and audio whose end was never read ------------------------------

    def png(with_end=True, after=b""):
        def chunk(typ, data):
            return struct.pack(">I", len(data)) + typ + data + b"\x11\x22\x33\x44"
        out = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", b"\x01" * 13) + chunk(b"IDAT", NZ)
        return out + (b"\x00\x00\x00\x00IEND\xaeB`\x82" if with_end else b"") + after

    def png_endings():
        expect("padded.png", png(after=b"\x00" * 600), "OK")
        expect("cut.png", png(with_end=False), "TRUNCATED")
    check("PNG: zeros after IEND are fine, a missing IEND is TRUNCATED", png_endings)

    def audio_zero_tail():
        expect("end-lost.mp3", sect(b"\xff\xfb\x90\x64" + NZ * 2) + b"\x00" * 8192, "TRUNCATED")
        expect("end-fine.mp3", b"\xff\xfb\x90\x64" + NZ + b"\x00" * 300, "OK")
    check("an MP3 whose last sectors are zeros is TRUNCATED", audio_zero_tail)

    # --- speed ------------------------------------------------------------------

    def speed():
        # A zero byte every 64 bytes, as in busy JPEG data. The old scan
        # copied the rest of its 1 MiB block at every one of them and took
        # tens of seconds here; the fixed one takes milliseconds.
        p = write("speed.gz", b"\x1f\x8b" + (b"\xAB" * 63 + b"\x00") * ((8 << 20) // 64))
        t = time.perf_counter()
        mod.probe_file(p, 4096, True)
        took = time.perf_counter() - t
        if took > 3.0:
            raise AssertionError(f"scanning 8 MiB took {took:.1f} s - the hole scan is slow "
                                 "again (should be well under a second)")
    check("the hole scan reads 8 MiB of photo-like data in under 3 seconds", speed)

    # --- the hole scan against a slow, obviously-correct reference ------------

    def reference(data, thr, skip):
        best = best_off = 0
        i, n = 0, len(data)
        while i < n:
            if data[i]:
                i += 1
                continue
            j = i
            while j < n and data[j] == 0:
                j += 1
            if j < n:                           # real data after it
                length, start = j - i, i
                if skip:
                    length, start = mod._outside(start, start + length, skip)
                if mod._sector_core(start, length) >= thr and length > best:
                    best, best_off = length, start
            i = j
        return best, best_off

    def differential():
        if not all(hasattr(mod, n) for n in ("_outside", "_sector_core", "SECTOR")):
            raise AssertionError("probe has no _outside/_sector_core/SECTOR to compare against")
        rnd = random.Random(2026)
        real = (mod.CHUNK, mod.SECTOR, mod.SECTOR_SLOP)
        p = os.path.join(d, "random.bin")
        try:
            for trial in range(300):
                mod.SECTOR, mod.SECTOR_SLOP = rnd.choice([(8, 2), (16, 3), (4, 0), (32, 5)])
                mod.CHUNK = rnd.choice([1, 2, 3, 5, 7, 16, 64, 100, 4096])
                parts = []
                for _ in range(rnd.randint(0, 40)):
                    r = rnd.random()
                    if r < 0.4:
                        parts.append(b"\x00" * rnd.randint(1, 80))
                    elif r < 0.5:
                        cur = sum(map(len, parts))
                        parts.append(bytes(rnd.randint(1, 255) for _ in range((-cur) % mod.SECTOR)))
                        parts.append(b"\x00" * (mod.SECTOR * rnd.randint(1, 6) + rnd.randint(0, 3)))
                    else:
                        parts.append(bytes(rnd.randint(1, 255) for _ in range(rnd.randint(1, 30))))
                data = b"".join(parts)
                thr = rnd.choice([1, 2, 4, 8, 16, 24, 40])
                skip = []
                if data and rnd.random() < 0.3:
                    for _ in range(rnd.randint(1, 3)):
                        a = rnd.randint(0, len(data))
                        skip.append((a, a + rnd.randint(1, 60)))
                with open(p, "wb") as fh:
                    fh.write(data)
                got = mod.scan_zero_run(p, thr, len(data), skip)
                want = reference(data, thr, skip)
                if got != want:
                    raise AssertionError(f"random file {trial}: scan gave {got}, the slow "
                                         f"reference {want} (chunk {mod.CHUNK}, threshold {thr})")
        finally:
            mod.CHUNK, mod.SECTOR, mod.SECTOR_SLOP = real
    check("the hole scan agrees with a slow reference on 300 random files", differential)

    shutil.rmtree(d, ignore_errors=True)


# ===========================================================================
# PART 4 - verify the checks themselves (--verify)
#
# claude_2 found that the filter check probed for attribute names neither
# probe has, so it ran nothing and printed "pass" for years of rounds. That is
# worse than no check: it reads as coverage.
#
# The general fix is not to fix that one check but to prove every check can
# fail. Each mutation below injects one targeted fault and asserts that the
# named check catches it. A mutation that slips through means the check is
# vacuous, whatever it prints on a healthy file.
# ===========================================================================

MUTATIONS = [
    ("shadow a module constant with a smaller value",
     "no module constant defined twice",
     lambda s: s + '\n\nSYSTEM_DIRS = (".fseventsd",)\n'),

    ("add a function nothing calls",
     "no module-level function is never called",
     lambda s: s + '\n\ndef orphaned_helper(x):\n    return x\n'),

    ("define an existing function a second time",
     "no function defined twice",
     lambda s: s + '\n\ndef check_sqlite(path):\n    return "", ""\n'),

    ("assign the same attribute twice in __init__",
     "no attribute assigned twice in one function",
     lambda s: s.replace("        self.rows = []", "        self.rows = []\n        self.rows = []", 1)),

    ("repeat an identical statement in one block",
     "no identical statement repeated in one block",
     lambda s: s.replace("        self.libraries.clear()",
                         "        self.libraries.clear()\n        self.libraries.clear()", 1)),

    ("leave a statement after an unconditional return",
     "no statement after an unconditional return",
     lambda s: s.replace("def display_path(p):\n", "def display_path(p):\n    return p\n    _dead = 1\n", 1)),

    ("hard-subscript a census key in the display path",
     "minimal",
     lambda s: s.replace('i.get("originals", 0)', 'i["a_key_no_record_has"]', 1)
               if 'i.get("originals", 0)' in s
               else s.replace("i.get('originals', 0)", "i['a_key_no_record_has']", 1)),

    ("assign the same local twice in one function",
     "no local assigned the same value twice",
     lambda s: s.replace("    libraries = []", "    libraries = []\n    libraries = []", 1)),

    ("stop display_path undoing long_path",
     "display_path exactly undoes long_path",
     lambda s: s.replace("        return p[4:]", "        return p", 1)),

    ("break the filter redraw so the combo does nothing",
     "filter across every verdict",
     lambda s: s.replace("    def refresh(self", "    def refresh_DISABLED(self", 1)),

    ("break the completion dialog on library records",
     "_finish with 4 libraries",
     lambda s: s.replace("    def _finish(self, why):",
                         "    def _finish(self, why):\n        _ = \"x\".join(self.libraries)", 1)),

    # --- the hole scan. Each mutation puts back one specific fault.
    ("reintroduce the single-longest-run bug (interior hole lost to a trailing run)",
     "interior hole is reported even when a longer zero run trails it",
     lambda s: s.replace("    # Anything still open here ran to the end of the file: not a hole.\n"
                         "    return best, best_off",
                         "    if run > best:\n        best, best_off = run, run_start\n"
                         "    if best_off + best >= size:\n        return 0, 0\n"
                         "    return best, best_off", 1)),

    ("count a zero run that reaches EOF as a hole",
     "trailing zero padding alone is not reported as a hole",
     lambda s: s.replace("    # Anything still open here ran to the end of the file: not a hole.\n"
                         "    return best, best_off",
                         "    if run > best:\n        best, best_off = run, run_start\n"
                         "    return best, best_off", 1)),

    ("stop a zero run continuing across a read-chunk boundary",
     "hole spanning a read-chunk boundary",
     lambda s: s.replace("            m = _ZEROS.match(buf)\n",
                         "            run = 0\n            m = _ZEROS.match(buf)\n", 1)),

    ("an off-by-one where a zero run crosses into the next read",
     "agrees with a slow reference",
     lambda s: s.replace("                run += lead\n", "                run += lead + 1\n", 1)),

    ("drop the disk-sector rule, so natural zero runs count as holes again",
     "does not sit on disk sectors",
     lambda s: s.replace("        if _sector_core(start, length) >= len(needle) and length > best:",
                         "        if length >= len(needle) and length > best:", 1)),

    ("make the hole scan copy the rest of the block at every zero again",
     "reads 8 MiB of photo-like data in under 3 seconds",
     lambda s: s.replace('    needle = b"\\x00" * max(1, int(threshold))',
                         '    needle = b"\\x00"', 1)
                .replace("                consider(e - k, off + k)\n",
                         "                consider(e - k, off + k)\n"
                         "                buf[e:].lstrip(b\"\\x00\")\n", 1)),

    ("stop recognising WebP as compressed RIFF",
     "zero hole inside a WebP",
     lambda s: s.replace('head[8:12] == b"WEBP"', 'head[8:12] == b"NOPE"', 1)),

    ("hole-scan every RIFF file, WAV included",
     "zeros inside a WAV",
     lambda s: s.replace('is_webp = fmt == "riff" and head[8:12] == b"WEBP"',
                         'is_webp = fmt == "riff"', 1)),

    # --- padding that is part of a format, not damage
    ("stop excusing FLAC padding blocks",
     "FLAC padding blocks are not reported",
     lambda s: s.replace("        if h[0] & 0x7F == 1:\n", "        if False:\n", 1)),

    ("stop excusing ID3 tag padding",
     "ID3 tag padding in an MP3",
     lambda s: s.replace("                ranges.append(_id3_padding(fh, id3))",
                         "                pass", 1)),

    ("stop looking past an ID3 tag for FLAC",
     "FLAC with an ID3 tag in front",
     lambda s: s.replace('        if head[id3:id3 + 4] == b"fLaC":', "        if False:", 1)),

    ("stop excusing MP4 free/skip boxes",
     "MP4 free/skip padding boxes",
     lambda s: s.replace('_MP4_PAD = frozenset((b"free", b"skip", b"wide"))',
                         "_MP4_PAD = frozenset()", 1)),

    ("stop recognising uncompressed (PCM) audio in MOV",
     "silence in a MOV with uncompressed (PCM) audio",
     lambda s: s.replace('_MP4_UNCOMPRESSED = frozenset((b"sowt", ',
                         '_MP4_UNCOMPRESSED = frozenset((b"xxxx", ', 1)),

    ("stop recognising QuickTime movies that start with moov",
     "starts with moov is not a mismatch",
     lambda s: s.replace('    (4, b"moov",                         "isobmff"),\n', "", 1)),

    ("stop checking that an MP4 still has its index",
     "index box was zero-filled",
     lambda s: s.replace("        verdict, detail = _check_isobmff_index(path, size)",
                         '        verdict, detail = "", ""', 1)),

    ("stop excusing Matroska Void elements",
     "Matroska Void padding",
     lambda s: s.replace("        if eid == 0xEC:                             # Void",
                         "        if False:", 1)),

    ("stop checking the Matroska Segment size",
     "Matroska file shorter than its Segment",
     lambda s: s.replace("        verdict, detail = _check_matroska_size(head, size)",
                         '        verdict, detail = "", ""', 1)),

    # --- JPEG endings
    ("judge a JPEG by its last two bytes again",
     "complete JPEG with a trailer",
     lambda s: s.replace("        end = _jpeg_image_end(path, size)", "        end = None", 1)),

    ("accept any JPEG as complete once its picture data starts",
     "JPEG cut off in its picture data",
     lambda s: s.replace("        if marker == 0xD9:\n            return pos",
                         "        if marker in (0xD9, 0xDA):\n            return pos", 1)),

    ("scan a motion photo's attached video as if it were the photo",
     "motion photo's attached video",
     lambda s: s.replace("        if picture_end < size:\n            skip.append((picture_end, size))",
                         "        if False:\n            skip.append((picture_end, size))", 1)),

    # --- RAW, housekeeping files, databases, endings
    ("call unknown RAW headers MISMATCH again",
     "RAW types without a known signature",
     lambda s: s.replace('            fmt = "raw"', '            return "MISMATCH", "unknown RAW", size', 1)),

    ("stop flagging files whose first sectors are zeros",
     "first sectors are zeros",
     lambda s: s.replace("        if lead >= 512 and (ext in EXT_MAP or is_raw):",
                         "        if False:", 1)),

    ("call Mac '._' files damaged again",
     "Mac '._' files",
     lambda s: s.replace('    if fmt == "appledouble" or (not fmt and name.startswith("._")):',
                         "    if False:", 1)),

    ("expect SQLite inside every .db file again",
     "non-SQLite .db files",
     lambda s: s.replace('    ".sqlite": {"sqlite"}, ".sqlite3": {"sqlite"},',
                         '    ".sqlite": {"sqlite"}, ".sqlite3": {"sqlite"}, ".db": {"sqlite"},', 1)),

    ("call a database that cannot be opened DB_DAMAGED again",
     "cannot be opened is UNREADABLE",
     lambda s: s.replace("        if _sqlite_access_error(e):", "        if False:", 1)),

    ("judge a PNG by its last 16 bytes again",
     "zeros after IEND are fine",
     lambda s: s.replace(r'if b"IEND\xaeB`\x82" not in tail:', 'if b"IEND" not in tail[-16:]:', 1)),

    ("stop flagging files whose last sectors are zeros",
     "MP3 whose last sectors are zeros",
     lambda s: s.replace("    if fmt in ZERO_TAIL_MEANS_DAMAGE:", "    if False:", 1)),

    # --- exports
    ("stop the window's Export writing the library inventory",
     "library inventory",
     lambda s: s.replace("                write_library_csv(libcsv, self.libraries)",
                         "                pass", 1)),

    ("stop the window's Export writing the not-testable list",
     "not-testable list",
     lambda s: s.replace("                write_results_csv(ntcsv, self.skipped)",
                         "                pass", 1)),

    # The guard is what makes long_path safe to call unconditionally from
    # every open(), stat() and walk in the probe. Removing it is not a
    # theoretical fault: it turns every path on macOS into \\?\/Users/... and
    # the whole scan into UNREADABLE. The check that catches it only started
    # being able to fail once posix_noop stopped asking the host what os.name
    # was - so this mutation is here to prove it is no longer vacuous.
    ("make long_path prefix on every platform, not just Windows",
     "both helpers are no-ops off Windows",
     lambda s: s.replace('    if os.name != "nt":\n        return p\n'
                         '    p = os.path.abspath(p)',
                         '    p = os.path.abspath(p)', 1)),
]


def run_verify(path):
    import subprocess, tempfile
    src = open(path, encoding="utf-8").read()
    print("\nVERIFY - does each check actually catch its fault?")
    print("  (a mutation that slips through means the check is vacuous)\n")
    vacuous, skipped = [], []
    for desc, expect, mutate in MUTATIONS:
        try:
            mutated = mutate(src)
        except Exception as e:
            print(f"  SKIP   {desc}: could not apply ({e})")
            skipped.append(desc)
            continue
        if mutated == src:
            print(f"  SKIP   {desc}: mutation did not apply to this file")
            skipped.append(desc)
            continue
        with tempfile.NamedTemporaryFile("w", suffix=".py", delete=False,
                                         encoding="utf-8") as fh:
            fh.write(mutated)
            tmp = fh.name
        try:
            # PROBE_SELFTEST_CHILD stops the child pausing for Enter at exit
            # (see PART 5). capture_output already makes its stdout a pipe
            # rather than a console, which is the primary guard; this is the
            # belt to that pair of braces, because a child that blocks on
            # input() here would hang the whole verify pass until it timed out
            # and then report every check as vacuous.
            child_env = dict(os.environ, PROBE_SELFTEST_CHILD="1")
            try:
                r = subprocess.run([sys.executable, os.path.abspath(__file__), tmp],
                                   capture_output=True, text=True, timeout=300,
                                   env=child_env)
            except subprocess.TimeoutExpired:
                # Only the slow-scan mutation is expected to take long, and a
                # run that never finishes is that fault showing itself.
                print(f"  caught {desc}  (the test run did not finish in 5 minutes)")
                continue
            out = r.stdout
            caught = any(line.startswith("  FAIL") and expect in line
                         for line in out.split("\n"))
            # a mutation that breaks import entirely also counts as caught
            crashed = "Traceback" in out and "FAIL" in out
            if caught or crashed:
                print(f"  caught {desc}")
            else:
                print(f"  MISSED {desc}")
                print(f"         -> '{expect}' did not fail; that check is vacuous")
                vacuous.append((desc, expect))
        finally:
            os.unlink(tmp)
    print()
    if vacuous:
        print(f"{len(vacuous)} mutation(s) slipped through - those checks prove nothing.")
        return 1
    if skipped:
        # A mutation that cannot be applied tests nothing. On the copy of the
        # probe this self-test ships with, every one applies; a skip means the
        # probe was edited and this list was not.
        print(f"{len(skipped)} mutation(s) could not be applied to this copy of the probe, "
              "so those checks were not verified.")
        return 1
    print(f"all {len(MUTATIONS)} mutations were caught; no check is vacuous")
    return 0


HERE = os.path.dirname(os.path.abspath(__file__))


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    # Resolved against THIS file's folder, not the working directory. A
    # double-click, a "Run with Python" from a context menu and a shortcut all
    # set the working directory differently, and a bare "recovery_probe.py"
    # then fails to open for reasons that have nothing to do with the probe.
    path = args[0] if args else os.path.join(HERE, "recovery_probe.py")
    if not os.path.exists(path):
        print(f"\nCannot find the file to test:\n    {path}\n")
        print("Put recovery_probe.py in the same folder as this self-test, or")
        print("drag recovery_probe.py onto this file, or run:")
        print(f'    python "{os.path.abspath(__file__)}" "C:\\path\\to\\recovery_probe.py"')
        return 2
    if "--verify" in sys.argv:
        return run_verify(path)
    print(f"\nSelf-test: {path}\n" + "=" * 64)
    run_structure(path)
    try:
        mod = run_gui(path)
        if mod is not None:
            run_paths(mod)
            run_verdicts(mod)
    except Exception:
        FAILURES.append(("gui setup", traceback.format_exc()))
        print("  FAIL   gui setup")
    print("=" * 64)
    if FAILURES:
        print(f"{len(FAILURES)} of {CHECKS} checks FAILED\n")
        for name, tb in FAILURES:
            print(f"--- {name} ---\n{tb}")
        return 1
    print(f"all {CHECKS} checks passed")
    print("\nThe GUI half is a mock: it catches type and attribute errors on the")
    print("real code paths but cannot confirm a window draws. That is still")
    print("Phase -1 step F on the target machine.")
    return 0


# ===========================================================================
# PART 5 - keep the window open, and keep a copy of what it said
#
# Double-clicking a .py on Windows opens a console window that closes the
# instant the script returns. The whole report - every check, and any
# traceback that matters - scrolls past in about a second and is then
# unrecoverable. The script was never failing; it was finishing.
#
# The first attempt at this gated both the pause and the log on
# sys.stdout.isatty(), which was wrong twice over. isatty() is False under
# IDLE, under any launcher that wraps the process, and anywhere stdout is
# redirected - so on those the window still vanished and no log appeared, with
# nothing said about why. And when the log file could not be opened at all
# (a read-only folder, or Controlled Folder Access refusing an unknown program
# write access to Desktop/Documents) the OSError was swallowed and the run
# looked identical to one that never tried.
#
# So: the pause and the log are now unconditional. The ONE case that must not
# pause is a --verify child, and run_verify marks those explicitly with
# PROBE_SELFTEST_CHILD - a positive marker, not a guess about the terminal.
# A log that cannot be written says so, loudly, and falls back to TEMP. And
# the banner prints where the log is going BEFORE the work starts, so even a
# run that dies half way leaves a readable trail.
#
# RUN_SELFTEST.bat sidesteps all of this by redirecting and pausing in the
# shell, where nothing depends on Python detecting anything. Prefer it.
# ===========================================================================

BUILD = "build 2026-10-12 (real-file fixtures, sector-aware holes)"
LOG_NAME = "probe_selftest-output.txt"


class _Tee:
    """Write to the console and the log file at once, flushing as it goes.

    Flushing every write costs nothing at this volume and means the log holds
    the full report even if the process is killed half way through.
    """

    def __init__(self, *streams):
        self._streams = [s for s in streams if s is not None]

    def write(self, s):
        for st in self._streams:
            try:
                st.write(s)
                st.flush()
            except Exception:
                pass
        return len(s)

    def flush(self):
        for st in self._streams:
            try:
                st.flush()
            except Exception:
                pass

    def isatty(self):
        return False


def _is_verify_child():
    """True only inside a subprocess run_verify started.

    The only situation in which this script must not wait for Enter. Marked
    positively by the parent rather than inferred from the terminal, because
    every inference available here has been wrong at least once.
    """
    return bool(os.environ.get("PROBE_SELFTEST_CHILD"))


def _open_log():
    """(file, path, note). Beside this script, else TEMP, else console only."""
    first = os.path.join(HERE, LOG_NAME)
    try:
        return open(first, "w", encoding="utf-8"), first, ""
    except OSError as e:
        why = f"{type(e).__name__}: {e}"
    import tempfile
    fallback = os.path.join(tempfile.gettempdir(), LOG_NAME)
    try:
        return (open(fallback, "w", encoding="utf-8"), fallback,
                f"could not write beside this script ({why}) - using TEMP instead")
    except OSError as e:
        return None, None, f"no log file could be opened ({why}; then {e})"


if __name__ == "__main__":
    child = _is_verify_child()
    # --no-log: something else is already capturing this run. RUN_SELFTEST.bat
    # redirects the whole thing to probe_selftest-output.txt itself, and cmd
    # holds that file open for the life of the command - so Python opening the
    # SAME path a moment later fails with PermissionError on Windows and
    # falls back to TEMP, which is exactly what happened on the first real run
    # of this build. Two writers, one filename, and the error message blamed
    # the folder. One writer per file: the launcher passes --no-log.
    own_log = not child and "--no-log" not in sys.argv
    log = logpath = None
    note = ""
    if own_log:
        log, logpath, note = _open_log()
        if log is not None:
            sys.stdout = sys.stderr = _Tee(sys.__stdout__, log)
    if not child:
        print(f"\nRecovery Probe self-test - {BUILD}")
        print(f"  this file  {os.path.abspath(__file__)}")
        print(f"  python     {sys.version.split()[0]} on {sys.platform}")
        if own_log:
            print(f"  log file   {logpath or '(none)'}")
            if note:
                print(f"             ^^ {note}")
        else:
            print("  log file   written by the launcher, not by this script")
    code = 1
    try:
        code = main()
    except SystemExit as e:
        code = e.code if isinstance(e.code, int) else 1
    except BaseException:
        # Including the import-time explosion this tool exists to catch.
        traceback.print_exc()
        print("\nThe self-test itself crashed. That traceback IS the result -")
        print("it is what the closing window has been hiding.")
        code = 1
    finally:
        # Nothing in this block may change the exit code. Writing to a console
        # that has already gone away raises - EOFError with no keyboard,
        # BrokenPipeError when the reader has closed the pipe - and an
        # exception raised in a finally REPLACES the result of the run. The
        # first version of this caught EOFError only and turned a clean
        # `... | head` into exit 1 plus a traceback, which is precisely the
        # kind of false alarm this whole file exists to prevent.
        if not child:
            try:
                if own_log and logpath:
                    print(f"\nThis whole report was also saved to:\n    {logpath}")
            except BaseException:
                pass
            if log:
                try:
                    log.close()
                except BaseException:
                    pass
            sys.stdout, sys.stderr = sys.__stdout__, sys.__stderr__
            if "--no-pause" not in sys.argv:
                try:
                    input("\nPress Enter to close this window. ")
                except BaseException:
                    pass          # no keyboard, or the console has gone away
    sys.exit(code)
