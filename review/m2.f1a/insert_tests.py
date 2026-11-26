"""Put the reviewer's scratch tests (review_tests.rs) inside `mod socket`
of bins/skyzfsd/src/control/tests.rs in WORKTREE, or take them out
(--remove). The file is otherwise left as the head has it."""
import os, sys

W = sys.argv[1]
HERE = os.path.dirname(os.path.abspath(__file__))
T = os.path.join(W, "bins/skyzfsd/src/control/tests.rs")
s = open(T).read()
mark = "}\n\nmod uploader {\n"
begin = "\n    /// The reviewer's tests of #166"
if "--remove" in sys.argv:
    i = s.find(begin)
    if i >= 0:
        j = s.index(mark, i)
        s = s[:i] + s[j:]
else:
    assert begin not in s, "already inserted"
    assert s.count(mark) == 1
    s = s.replace(mark, open(os.path.join(HERE, "review_tests.rs")).read() + mark, 1)
open(T, "w").write(s)
