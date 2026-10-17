# Recovery Probe

Finds damage in files recovered with ddrescue: the kind that opens fine and
looks normal. Every file is sorted into **damaged**, **not testable** or
**clean**, and every Apple Photos Library on the way is found and
inventoried, so it can be moved out of harm's way before any duplicate
finder runs.

This is the Windows program. It is a Rust port of `reference/recovery_probe.py`
and gives the same answer as it for every file, detail text included: each
change is checked against the Python version file by file, on thousands of
test files, on real Windows (see `.github/workflows/build.yml`).

## What it checks

| Verdict | Meaning |
|---|---|
| TRUNCATED | The file ends before its own format says it should, or its last disk sectors were never read. |
| HOLE | A block of zeros in the middle, shaped exactly like unreadable disk sectors. |
| MISMATCH | The content doesn't match the name, or the start of the file was never read. |
| DB_DAMAGED | A Photos Library database failed its integrity check. |
| UNREADABLE | The file couldn't be read at all. |
| LIBRARY | An Apple Photos Library (information, not a fault). |
| not testable | Nothing here can verify this type (HEIC and camera RAW always land here). |

It only ever reads. It never changes, moves or deletes a file.

## No third-party code

The program uses nothing but Rust's own standard library. For Photos Library
databases it uses the SQLite that comes with Windows (`winsqlite3.dll`).

## Building

```
cargo build --release
cargo test --release
```

Comparing with the Python version:

```
python tools/make_fixtures.py edge  fixtures/edge
python tools/make_fixtures.py mutate tests/seeds fixtures/mut
python tools/compare_with_python.py target/release/recovery-probe fixtures/edge fixtures/mut
```
