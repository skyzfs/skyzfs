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

## Second look, head ab61bf0

Posted: can merge once CI is green. The caption alone changed, and it
now says what the column is.

## Third look, head 2be6c33: a miss of the first look

Codex found that the row parser (`load`) kept `f[2:9]`, seven values,
so `read_ms` never reached a new row. The first look ran the function
that writes the line and the division, and never the parser between
them. Fixed to `f[2:10]`.

`parser.sh` cuts `load` and the devices' differences out of the script
and runs them on made-up snapshots of eight and of seven values
(`parser.txt`): at the fix `read_ms` arrives, before it it did not.

Lesson: when a value is added to a line that one part writes and
another reads, run the path from the writer through the reader to the
output, on the real code, before saying "old and new rows read right".
