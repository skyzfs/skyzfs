#!/usr/bin/env python3
"""#97 at 75bf72b: two paths of NodeB.verify, shown with no kernel.

    verify_paths.py CHECKOUT

The tool, the import and zdb are replaced by stand-ins that say what a runner could say; the
code under test is NodeB.verify, Seen and control_verdict as they are at the head.

1. A control whose import and files pass and whose zdb times out reads "caught at copies",
   which EXPECTED allows for lacks, later and hole. The replay rig counts only
   'rig: zdb died with signal 6' as a catch (replay_check.py:619).
2. The published cut whose import fails and on whose image zdb then aborts ends as the rig's
   error (RigError, exit 2) and not as the cut's FAIL: the stage that failed is lost.
"""
import os
import sys
from unittest import mock

ROOT = sys.argv[1]
sys.path.insert(0, os.path.join(ROOT, 'tests/kernel'))
import nodeb_check  # noqa: E402
from cut_check import RigError  # noqa: E402


def node_b():
    nb = nodeb_check.NodeB.__new__(nodeb_check.NodeB)
    nb.img, nb.image, nb.logdev = '/nonexistent', '/nonexistent/nodeb.img', None
    nb.tool_run = lambda args, timeout=600: (0, 'snap/t\tc0\t1\n' if args[0] == 'refs' else 'read\tsnap/t\tc0\t1\n')
    return nb


def stages(import_ok):
    def snapshot_files(snap, want, seen):
        seen.put('import', import_ok, 'read-only' if import_ok else "cannot import 'skzcut': I/O error")
        if import_ok:
            seen.put('snapshot', True, 'skzcut@s24')
            seen.put('files', True, '24 files, each as the source held it')
    return snapshot_files


def main():
    # 1. zdb times out on a control (run() returns 124).
    nb = node_b()
    nb.snapshot_files = stages(True)
    with mock.patch.object(nodeb_check, 'run', return_value=(124, '', '')):
        seen = nb.verify('life-lacks', 'snap/t', 's24', {}, damaged=True)
    print('1. a control, zdb timed out:', seen.text())
    for ctl in ('lacks', 'later', 'hole'):
        print(f'   {ctl}:', nodeb_check.control_verdict(seen, nodeb_check.EXPECTED[ctl]))

    # 2. The published cut: the import fails, then zdb aborts (run() returns -6).
    nb = node_b()
    nb.snapshot_files = stages(False)
    with mock.patch.object(nodeb_check, 'run', return_value=(-6, 'PANIC: Pool has encountered an uncorrectable I/O failure', '')):
        try:
            seen = nb.verify('life-cut', 'snap/t', 's24', {}, damaged=False)
            print('2. the published cut:', seen.text())
        except RigError as e:
            print('2. the published cut, import failed, zdb aborted: RigError:', e)


if __name__ == '__main__':
    main()
