# The benchmark's work disk ms per read and per write (pull request 68), head 3568356

Review posted: nothing blocks.

- Field 7 of `/proc/diskstats` (ms reading) is appended as the eighth
  value; the seven before keep their places. `fields.sh` runs the
  script's awk on a made-up line whose field N holds N*100
  (`fields.txt`), and on a line of a kernel before 5.5.
- The per-request figures are the window's differences divided, as
  iostat's r_await and w_await. A row with seven values shows n/a for
  the read half.
- Wording: the time includes waiting in the queue, and a mean shows a
  stall only as far as a mean can.

CI and the rendering on run 43498281407's rows were not read.
