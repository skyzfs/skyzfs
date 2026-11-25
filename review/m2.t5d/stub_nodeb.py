#!/usr/bin/python3 -I
"""A node B for cut_watch.py: `row` names a cut at head; `get` ends 4
(STUB_GET=4: the bucket does not hold what the commit needs) or with
STUB_GET's status; STUB_ROW=3 makes `row` end 3 (a node B that fails)."""
import os
import sys
cmd = sys.argv[1]
if cmd == 'row':
    rc = int(os.environ.get('STUB_ROW', '0'))
    if rc:
        sys.stderr.write('row failed\n')
        sys.exit(rc)
    print('head\tabc123 7\nrow\tstate 5 20\nlease\tnode-a 1 held\nrow_commit\tabc123')
    sys.exit(0)
if cmd == 'get':
    rc = int(os.environ.get('STUB_GET', '4'))
    sys.stderr.write('object blocks/00ff not found\n')
    sys.exit(rc)
sys.exit(9)
