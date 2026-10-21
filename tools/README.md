# Test tools

None of these is part of the program, and none is needed to run it: the
program is `recovery-probe.exe` alone. They are how every change is shown to
be safe before it reaches the person's PC, and GitHub runs them on every push
to `dev` (`.github/workflows/build.yml`).

Most are Python on purpose. They work out the right answers a second,
separate way - from what a test disk was built to hold, or from what Windows
and Linux themselves see - so a mistake in the program can't hide behind the
same mistake in its test.

| Tool | What it checks |
|---|---|
| `exfat_image.py` | Builds exFAT test disks the way a Mac does, with known damage (folder records, files, the FAT, the bitmap, lost folders), and writes down exactly what is where. |
| `test_mapcheck.py` | The independent oracle: from what each test disk holds, works out what the map check must find and what the copying must produce, runs the program and compares - names, bytes, dates, folders rebuilt from their contents, pieces of lost data, stopping and carrying on, the safety check at all three stages, and the check of the copies with the map check (exactly the copies of files that lost data are marked NEVER_READ). Also asks `fsck.exfat` and `dissect.fat` about every disk (Linux). |
| `kernel_check.py` | Mounts the test disks with the Linux kernel's exFAT driver and compares what it lists with what the map check says Windows can't list. |
| `windows_check.py` | Attaches test disks to Windows as virtual disks and compares what Windows lists and opens with what the map check predicts, record by record. |
| `guard_check.py` | The disk protection on Windows: a protected disk is checked byte for byte before and after every step, to prove nothing wrote to it. |
| `dropout_check.py` | Unplugs a virtual disk in the middle of copying, on Windows: the copying must stop cleanly and carry on to the same result. |
| `make_big_disk.py` | A test disk with very many files, for timing and for `dropout_check.py`. |
| `make_selftest_disk.py` | Makes the small damaged disk the program's own self-test carries (`src/selftest/`); GitHub checks the committed one is exactly what it makes. |
| `ui_check.js` | The page in a real browser (Chromium, through Playwright), the way a person uses it: every step, the safety checks and the checklist, at desktop and phone size. |

The sample files the self-test checks are in `tests/seeds/`.
