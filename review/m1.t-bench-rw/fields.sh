#!/bin/bash
# The benchmark's diskstats_line at 3568356, on a made-up line whose field N holds N*100
# (fields 1 to 3 are major, minor and name), and on a line of a kernel before 5.5.
line() { awk -v d="$1" '$3==d { f = (NF >= 19) ? $19 : 0; fm = (NF >= 20) ? $20 : 0; print $4, $6, $8, $10, f, $11, fm, $7 }' "$2"; }
printf '8 0 sda 400 500 600 700 800 900 1000 1100 1200 1300 1400 1500 1600 1700 1800 1900 2000\n8 16 old 400 500 600 700 800 900 1000 1100 1200 1300 1400\n' > stats
echo "new kernel: $(line sda stats)   (reads rsect writes wsect flushes write_ms flush_ms read_ms)"
echo "old kernel: $(line old stats)"
