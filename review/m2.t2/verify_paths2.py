#!/usr/bin/env python3
"""#97 at 0f2f8c7: what snapshot_files makes of a command of its own that times out, with no kernel.

    verify_paths2.py CHECKOUT

Every command is replaced by a stand-in that says what a runner could say; the code under test
is NodeB.verify, NodeB.snapshot_files and control_verdict as they are at the head.

1. `zfs list` times out on the early control's pool (Codex on 0f2f8c7): the line reads caught at
   the snapshot, as expected, though the listing never came to an end.
2. `mount` times out on a control whose snapshot is listed: the line reads caught at the mount,
   which EXPECTED allows for lacks, later and hole.
"""
import os
import sys

ROOT = sys.argv[1]
sys.path.insert(0, os.path.join(ROOT, 'tests/kernel'))
import nodeb_check  # noqa: E402
from nodeb_check import POOL  # noqa: E402


def commands(timed_out):
    def run(cmd, timeout=None):
        if cmd[:2] == ['zpool', 'list']:
            return 1, f"cannot open '{POOL}': no such pool", 0
        if cmd[0] == 'losetup' and '--show' in cmd:
            return 0, '/dev/loop9\n', 0
        if cmd[0] == timed_out:
            return 124, f'\nTIMEOUT after {timeout}s', 0
        if cmd[:2] == ['zfs', 'list']:
            return 0, f'{POOL}@s6\n{POOL}@s24\n', 0
        return 0, '', 0
    return run


def node_b():
    nb = nodeb_check.NodeB.__new__(nodeb_check.NodeB)
    nb.img, nb.image, nb.logdev = '/nonexistent', '/nonexistent/nodeb.img', None
    nb.mnt, nb.alt = '/nonexistent/mnt', '/nonexistent/alt'
    nb.tool_run = lambda args, timeout=600: (0, 'snap/t\tc0\t1\n' if args[0] == 'refs' else 'read\tsnap/t\tc0\t1\n')
    return nb


def main():
    saved = nodeb_check.run, nodeb_check.replay_check.every_copy_verdict
    nodeb_check.replay_check.every_copy_verdict = lambda rc, text, image: (None, '', {'blocks': 1, 'copies': 2})
    try:
        for n, (cmd, ctls) in enumerate((('zfs', ('early',)), ('mount', ('lacks', 'later', 'hole'))), 1):
            nodeb_check.run = commands(cmd)
            seen = node_b().verify('v', 'snap/t', 's24', {}, damaged=True)
            print(f'{n}. {cmd} timed out:', seen.text())
            for ctl in ctls:
                print(f'   {ctl}:', nodeb_check.control_verdict(seen, nodeb_check.EXPECTED[ctl],
                                                                nodeb_check.CLEAN.get(ctl, ())))
    finally:
        nodeb_check.run, nodeb_check.replay_check.every_copy_verdict = saved


if __name__ == '__main__':
    main()
