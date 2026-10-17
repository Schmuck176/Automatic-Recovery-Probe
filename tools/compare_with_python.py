#!/usr/bin/env python3
"""Run the Rust probe and the Python reference over the same folders and
compare them file by file: verdict, detail text and size. Then compare the
three CSV files a full scan writes.

Usage:  python tools/compare_with_python.py <recovery-probe executable> <folder> [<folder> ...]

Exit code 0 only if every file got the same answer from both.

The one allowed difference is the wording of an UNREADABLE detail: Python
and Rust phrase operating-system errors differently ("[Errno 2] ..." vs
"(os error 2)"). The verdict itself must still match.
"""
import csv
import importlib.util
import json
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REFERENCE = os.environ.get("PROBE_REFERENCE") or os.path.join(HERE, "..", "reference", "recovery_probe.py")


def load_reference():
    spec = importlib.util.spec_from_file_location("reference_probe", REFERENCE)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def python_rows(ref, folder, hole_bytes, do_hole):
    out = {}
    for kind, payload in ref.walk_and_probe(folder, hole_bytes, do_hole):
        if kind == "row":
            verdict, detail, size, path = payload
            out[path] = (verdict, detail, size)
    return out


def rust_rows(exe, folder, hole_kb, do_hole):
    cmd = [exe, "probe-jsonl", folder, "--hole-kb", str(hole_kb)]
    if not do_hole:
        cmd.append("--no-holes")
    res = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8")
    if res.returncode != 0:
        raise SystemExit(f"rust probe failed: {res.stderr}")
    out = {}
    for line in res.stdout.splitlines():
        if line.strip():
            d = json.loads(line)
            out[d["path"]] = (d["verdict"], d["detail"], d["size"])
    return out


def read_csv(path):
    if not os.path.exists(path):
        return None
    with open(path, encoding="utf-8-sig", newline="") as fh:
        return list(csv.reader(fh))


def compare_scans(ref, exe, folder):
    """Both programs' full console scan; compare the three CSV files."""
    problems = []
    with tempfile.TemporaryDirectory(prefix="probe-compare-") as td:
        py_out = os.path.join(td, "py.csv")
        rs_out = os.path.join(td, "rs.csv")
        import io, contextlib
        with contextlib.redirect_stdout(io.StringIO()):
            ref.run_cli(folder, out=py_out)
        res = subprocess.run([exe, "scan", folder, "--out", rs_out], capture_output=True, text=True, encoding="utf-8")
        if res.returncode != 0:
            return [f"rust scan failed: {res.stdout[-500:]} {res.stderr[-500:]}"]
        for suffix in ("", "-not-testable", "-photo-libraries"):
            a = read_csv(py_out.replace(".csv", f"{suffix}.csv"))
            b = read_csv(rs_out.replace(".csv", f"{suffix}.csv"))
            name = f"results{suffix}.csv"
            if a is None and b is None:
                continue
            if (a is None) != (b is None):
                problems.append(f"{name}: written by only one of them (python={a is not None}, rust={b is not None})")
                continue
            if a[0] != b[0]:
                problems.append(f"{name}: different header rows")
                continue
            key = (lambda r: r) if suffix != "-photo-libraries" else (lambda r: r)
            body_a, body_b = a[1:], b[1:]
            if suffix != "-photo-libraries":
                body_a, body_b = sorted(body_a), sorted(body_b)
            # UNREADABLE details may be worded differently (see top of file).
            def norm(rows):
                return [[r[0], "" if r[0] == "UNREADABLE" else r[1]] + r[2:] for r in rows]
            if norm(body_a) != norm(body_b):
                problems.append(f"{name}: rows differ ({len(body_a)} python vs {len(body_b)} rust)")
                for ra, rb in zip(norm(body_a), norm(body_b)):
                    if ra != rb:
                        problems.append(f"    python: {ra}\n    rust:   {rb}")
                        break
    return problems


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    exe = os.path.abspath(sys.argv[1])
    ref = load_reference()
    total = mismatches = 0
    for folder in sys.argv[2:]:
        folder = os.path.abspath(folder)
        for hole_kb, do_hole in ((4, True), (1, True), (4, False)):
            py = python_rows(ref, folder, hole_kb * 1024, do_hole)
            rs = rust_rows(exe, folder, hole_kb, do_hole)
            for path in sorted(set(py) | set(rs)):
                total += 1
                a, b = py.get(path), rs.get(path)
                if a is None or b is None:
                    mismatches += 1
                    print(f"ONLY IN {'RUST' if a is None else 'PYTHON'}: {path}")
                    continue
                same = a[0] == b[0] and a[2] == b[2] and (a[1] == b[1] or a[0] == "UNREADABLE")
                if not same:
                    mismatches += 1
                    if mismatches <= 30:
                        print(f"DIFF ({hole_kb} KB, holes={'on' if do_hole else 'off'}) {path}\n"
                              f"    python: {a}\n    rust:   {b}")
        for p in compare_scans(ref, exe, folder):
            mismatches += 1
            print(f"SCAN CSV {folder}: {p}")
    print(f"\ncompared {total:,} file results: {mismatches} difference(s)")
    return 0 if mismatches == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
