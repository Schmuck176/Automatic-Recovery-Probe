# Test tools

None of these is part of the program, and none is needed to run it: the
program is `automatic-recovery-probe.exe` alone. They are how every change is
shown to be safe before it reaches the person's PC, and GitHub runs them on
every push to `dev` (`.github/workflows/build.yml`).

Most are Python on purpose. They work out the right answers a second,
separate way - from what a test disk was built to hold, or from what Windows
and Linux themselves see - so a mistake in the program can't hide behind the
same mistake in its test.

| Tool | What it checks |
|---|---|
| `exfat_image.py` | Builds exFAT test disks the way a Mac does, with known damage (folder records, files, the FAT, the bitmap, lost folders) and, on purpose, records left after a folder's end, and writes down exactly what is where. |
| `test_mapcheck.py` | The independent oracle: from what each test disk holds, works out what the map check must find and what the copying must produce, runs the program and compares - names, bytes, dates, folders rebuilt from their contents, pieces of lost data, stopping and carrying on, the safety check at all three stages, and the check of the copies with the map check (exactly the copies of files that lost data are marked NEVER_READ); leftover records, and clusters claimed twice or claimed though free; and the map check's comparison with what Windows lists, against a folder made to hold just that (Linux). Also asks `fsck.exfat` and `dissect.fat` about every disk (Linux). |
| `kernel_check.py` | Mounts the test disks with the Linux kernel's exFAT driver and compares what it lists with what the map check says Windows can't list. |
| `windows_check.py` | Attaches test disks to Windows as virtual disks and compares what Windows lists and opens with what the map check predicts, record by record - and checks the map check's own comparison with Windows agrees. |
| `guard_check.py` | The disk protection on Windows: a protected disk is checked byte for byte before and after every step, to prove nothing wrote to it; and Windows' own disk is refused. |
| `dropout_check.py` | Unplugs a virtual disk in the middle of copying, on Windows: the copying must stop cleanly and carry on to the same result. |
| `make_big_disk.py` | A test disk with very many files, for timing and for `dropout_check.py`. |
| `make_selftest_disk.py` | Makes the small damaged disk the program's own self-test carries (`src/selftest/`); GitHub checks the committed one is exactly what it makes. |
| `ui_check.js` | The page in a real browser (Chromium, through Playwright), the way a person uses it: every step, the safety checks and the checklist, at desktop and phone size; every icon on the steps tab has words beside it; the checklist's "step 7" goes to step 7. |
| `auto_check.py` | Start, Bring back and Finalise, through the page's own API, on a damaged test disk of real media made with FFmpeg (videos from one camera with one's index lost, photos cut short, with a hole, with an intact twin, a RAW, MP3, FLAC, a PDF and a Word file that lost their ends, an MP3 broken before the drive failed): every stage, every repair and every file in WORK checked against what must happen with the tools present. |
| `resume_check.py` | The same Start, stopped and killed outright at random moments in every stage and started again each time: it must end exactly as one run without a break. |
| `auto_ui_check.js` | The Automatic tab in a real browser: Get ready, the folders, Start, What is left, Finalise; every icon has words beside it; where you are, the guide and "Now" stay in their places at 2560 px (the PC's 27-inch 4K screen at 150%), half that, 900 px and phone size, with nothing sticking out sideways. |
| `download_check.py` | Get ready's qpdf download, for real (Windows): from qpdf's GitHub release, checked by size and SHA-256, unpacked into the program's own tools folder. |

The sample files the self-test checks are in `tests/seeds/`.
