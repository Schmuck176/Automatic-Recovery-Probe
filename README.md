# Automatic Recovery Probe

Recovery Probe, with the recovery automated.

[Recovery Probe](docs/recovery-probe-README.md) (version 2026-10-22 (1), the
program in `C:\RECOVERY-TOOLS`) does every part of getting files back off the
ddrescue copy safely, but one step at a time, with a long checklist around it.
Automatic Recovery Probe is built from the same code, and does the same work
from one **Start** button:

1. **Get ready**: it checks Windows is set up and the open-source tools are
   there, and makes Windows leave newly plugged-in disks switched off.
2. **Plug in**: SPARE first (the rehearsal: plugged in the way the recovered
   SSD will be), then the recovered SSD. The program recognises each one,
   protects the SSD (read-only, switched off) and switches SPARE on.
3. **Choose the folders** it works in: where the image goes, where the copies
   go, where the second copy goes.
4. **Start**, and it runs by itself: the image (read back to check it), the
   map check (checked against Windows' own reading of the image, opened
   read-only with AIM Toolkit), copying everything off, the safety checks,
   checking every file, the extra checks Krokiet, 7-Zip, MP3val and ffmpeg
   did by hand, then repairing every damaged file it can with open-source
   tools and putting each repaired file back - logged, the damaged original
   kept, undoable - and checking again.
5. **What's left** goes into a hand-off folder for paid repair tools or the Mac
   (with a script to double-click there); **Bring back** takes in whatever comes
   back, and **Finalise** makes the second copy and runs the final checks.

It stops and says why whenever a check finds something, as Recovery Probe
does. The rules stay the same: nothing is ever written to the recovered SSD or
the image; the open-source tools only read the copies in `WORK`, and only
repair the copies in `REPAIR`.

**Status:** being built (October 2026). See `NEXT-STEPS.md`.

## Building

```
cargo build --release
cargo test --release
target/release/automatic-recovery-probe self-test
```
