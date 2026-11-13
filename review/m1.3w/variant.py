#!/usr/bin/env python3
"""variant.py FILE MODE: rewrite the two timing tests' look and own time.
MODE: old | old5 | new5 | slack-old | slack-new. Each spot must match once."""
import sys
path, mode = sys.argv[1], sys.argv[2]
src = open(path).read()
new1 = ("    let (timing, state) = (served.timing(), served.log_state());\n"
        "    let total = began.elapsed();\n")
new2 = ("    let timing = served.timing();\n"
        "    let total = began.elapsed().as_nanos() as u64;\n")
look1 = "    let (timing, state) = (served.timing(), served.log_state());\n"
tot1 = "    let total = began.elapsed();\n"
look2 = "    let timing = served.timing();\n"
tot2 = "    let total = began.elapsed().as_nanos() as u64;\n"
nap = "    std::thread::sleep(Duration::from_millis(5));\n"
s1 = '    eprintln!("SLACK1 {}", total.as_nanos() as i64 - timing.worker.iter().sum::<u64>() as i64);\n'
s2 = '    eprintln!("SLACK2 {}", total as i64 - timing.reader.iter().sum::<u64>() as i64);\n'
out = {
    "old": (tot1 + look1, tot2 + look2),
    "old5": (tot1 + nap + look1, tot2 + nap + look2),
    "new5": (nap + look1 + nap + tot1, nap + look2 + nap + tot2),
    "slack-old": (tot1 + look1 + s1, tot2 + look2 + s2),
    "slack-new": (look1 + tot1 + s1, look2 + tot2 + s2),
}[mode]
for old, new in ((new1, out[0]), (new2, out[1])):
    assert src.count(old) == 1, (src.count(old), old)
    src = src.replace(old, new)
open(path, "w").write(src)
