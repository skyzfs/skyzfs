#!/usr/bin/env python3
"""#121 at b189873: plants against test_cut_plan.py's closure() and
imports(), and the recorder's plants the answer says are now killed.

    plants5.py CHECKOUT OUT [PREFIX ...]

The harness is plants.py's, under ../m2.t3-3. A plant here is a gap put
into the plan's list (P), the workflow's paths (Y), a manifest (M: a new
dependency on a workspace crate that is in neither list), a module of
the rig (I: a new import of tests/kernel/control_check.py, which is in
neither list) or the recorder (S). KILLED means test_cut_plan.py failed,
which is what a gap should do. Run it twice, with cargo on PATH and
without, to see what the manifests' test holds on its own.
"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'm2.t3-3'))
import plants   # noqa: E402
import plants2  # noqa: E402,F401

R, P = plants.R, plants2.P
Y = '.github/workflows/skyzfs-cut-daemon.yml'
UP = 'crates/skyzfs-upload/Cargo.toml'
TREE = 'crates/skynbd-tree/Cargo.toml'
NB = 'bins/skyzfs-nodeb/Cargo.toml'
K = 'tests/kernel/'
PLAN_TEST = [('py', 'tests/kernel/test_cut_plan.py')]
for f in (Y, UP, TREE, NB, K + 'cut_ref.py', K + 'cut_daemon_check.py', K + 'cut_watch.py', K + 'cut_summary.py',
          K + 'logwrites.py'):
    plants.TESTS[f] = PLAN_TEST
# The recorder's plants here are about what the upload job runs: the
# plan's test is what should notice, and the recorder's own must pass.
CONTROL = "        \"$2\" \"$st\" \"$n\" 0 || rc=$?\n"

CRATES = ('crates/skyzfs-upload', 'bins/skyzfsd', 'crates/skyzfs-local', 'crates/skyzfs-ublk', 'crates/skyzfs-ublk-sys',
          'crates/skyzfs-blk', 'crates/skyzfs-mapped', 'crates/skyzfs-uberblock', 'bins/skyzfs-nodeb',
          'crates/skynbd-engine', 'crates/skynbd-refs', 'crates/skynbd-store', 'crates/skynbd-format',
          'crates/skynbd-tree', 'crates/skynbd-types')
MODULES = ('cut_watch.py', 'cut_daemon_check.py', 'cut_plan.py', 'cut_summary.py', 'cut_check.py', 'cut_ref.py',
           'replay_check.py', 'logwrites.py')
OTHER = ('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'tests/lib/', 'tests/kernel/cut-record.sh',
         '.github/workflows/skyzfs-cut-daemon.yml')
DEPS = "[dependencies]\n"
DEV = "[dev-dependencies]\n"

NEW = []
for k, c in enumerate(CRATES):
    NEW.append((f'P{100 + k} the plan without {c}/', P, f"    '{c}/',\n", ''))
    NEW.append((f'Y{100 + k} the workflow without {c}/**', Y, f"      - {c}/**\n", ''))
for k, m in enumerate(MODULES):
    NEW.append((f'P{120 + k} the plan without tests/kernel/{m}', P, f"    'tests/kernel/{m}',\n", ''))
for k, o in enumerate(OTHER):
    NEW.append((f'P{130 + k} the plan without {o}', P, f"    '{o}',\n", ''))
NEW += [
    ('Y120 the workflow without tests/kernel/cut*', Y, "      - tests/kernel/cut*\n", ''),
    ('Y121 the workflow without tests/kernel/replay_check.py', Y, "      - tests/kernel/replay_check.py\n", ''),
    ('Y122 the workflow without tests/kernel/logwrites.py', Y, "      - tests/kernel/logwrites.py\n", ''),
    ('Y130 the workflow without Cargo.lock', Y, "      - Cargo.lock\n", ''),
    ('Y131 the workflow without Cargo.toml', Y, "      - Cargo.toml\n", ''),
    ('Y132 the workflow without rust-toolchain.toml', Y, "      - rust-toolchain.toml\n", ''),
    ('Y133 the workflow without tests/lib/**', Y, "      - tests/lib/**\n", ''),
    ('Y134 the workflow no longer starts on itself', Y,
     "      - rust-toolchain.toml\n      - .github/workflows/skyzfs-cut-daemon.yml\n", "      - rust-toolchain.toml\n"),
    ('Y140 the workflow leaves out a crate with a pattern that begins with !', Y,
     "      - crates/skynbd-types/**\n", "      - crates/skynbd-types/**\n      - '!crates/skynbd-tree/**'\n"),
    ('Y141 the workflow has paths-ignore for a crate', Y, "  push:\n    branches: ['skyzfs-testing*']\n",
     "    paths-ignore:\n      - crates/skynbd-tree/**\n  push:\n    branches: ['skyzfs-testing*']\n"),
    # ---- a new dependency on a workspace crate in neither list
    ('M01 the uploader depends on skynbd-nbd (workspace = true)', UP, DEPS, DEPS + 'skynbd-nbd = { workspace = true }\n'),
    ('M02 the uploader depends on skynbd-nbd (the dotted form)', UP, DEPS, DEPS + 'skynbd-nbd.workspace = true\n'),
    ('M03 the uploader depends on skynbd-stream by path', UP, DEPS, DEPS + 'skynbd-stream = { path = "../skynbd-stream" }\n'),
    ('M04 the uploader depends on skynbd-stream by path under another name', UP, DEPS,
     DEPS + 'other = { path = "../skynbd-stream", package = "skynbd-stream" }\n'),
    ('M05 the uploader has skynbd-nbd as a build dependency', UP, DEV,
     '[build-dependencies]\nskynbd-nbd = { workspace = true }\n\n' + DEV),
    ('M06 the uploader has skynbd-nbd as a dependency on one target', UP, DEV,
     "[target.'cfg(unix)'.dependencies]\nskynbd-nbd = { workspace = true }\n\n" + DEV),
    ('M07 the uploader has skynbd-nbd as an optional dependency', UP, DEPS,
     DEPS + 'skynbd-nbd = { workspace = true, optional = true }\n'),
    ('M08 the uploader has skynbd-nbd as a table of its own', UP, DEV,
     '[dependencies.skynbd-nbd]\nworkspace = true\n\n' + DEV),
    ('M09 node B depends on skynbd-nbd', NB, DEPS, DEPS + 'skynbd-nbd = { workspace = true }\n'),
    ('M10 a crate deep under the uploader (skynbd-tree) depends on skynbd-stream by path', TREE, DEV,
     '[dependencies]\nskynbd-stream = { path = "../skynbd-stream" }\n\n' + DEV),
    ('M20 (no gap) the uploader has skynbd-nbd as a dev-dependency', UP, DEV, DEV + 'skynbd-nbd = { workspace = true }\n'),
    # ---- a new import of a module in neither list
    ('I01 cut_ref.py: import control_check', K + 'cut_ref.py', "import struct\n", "import struct\nimport control_check\n"),
    ('I02 cut_ref.py: from control_check import EXPECTED', K + 'cut_ref.py', "import struct\n",
     "import struct\nfrom control_check import EXPECTED\n"),
    ('I03 cut_ref.py: import control_check as cc', K + 'cut_ref.py', "import struct\n",
     "import struct\nimport control_check as cc\n"),
    ('I04 cut_ref.py: import os, control_check on one line', K + 'cut_ref.py', "import struct\n",
     "import struct, control_check\n"),
    ('I05 cut_ref.py: the import under try', K + 'cut_ref.py', "import struct\n",
     "import struct\ntry:\n    import control_check\nexcept ImportError:\n    control_check = None\n"),
    ('I06 logwrites.py (reached through two modules): the import inside a function', K + 'logwrites.py',
     "def main():\n", "def main():\n    import control_check\n"),
    ('I07 cut_watch.py: the import inside a function', K + 'cut_watch.py', "def main():\n",
     "def main():\n    from control_check import EXPECTED\n"),
    ('I08 cut_summary.py: the import inside a function', K + 'cut_summary.py', "def main():\n",
     "def main():\n    import control_check\n"),
    ('I10 cut_ref.py: importlib.import_module', K + 'cut_ref.py', "import struct\n",
     "import struct\nimport importlib\ncontrol_check = importlib.import_module('control_check')\n"),
    ('I11 cut_ref.py: __import__', K + 'cut_ref.py', "import struct\n",
     "import struct\ncontrol_check = __import__('control_check')\n"),
    ('I12 cut_watch.py: the script run as a command', K + 'cut_watch.py', "def main():\n",
     "def main():\n    subprocess.run([sys.executable, os.path.join(os.path.dirname(__file__), 'control_check.py')])\n"),
    ('I13 the recorder runs a script of tests/kernel that is in neither list', R, CONTROL,
     "        python3 \"$HERE/control_check.py\" > /dev/null 2>&1 || true\n" + CONTROL),
    ('I14 the recorder sources a script of tests/kernel that is in neither list', R, "\n. \"$HERE/../lib/gnu-dd.sh\"\n",
     "\n. \"$HERE/../lib/gnu-dd.sh\"\n. \"$HERE/daemon-lib.sh\"\n"),
]
plants.TESTS[R] = PLAN_TEST
PLAN = [p for p in NEW]

# ---- the recorder's plants of the review, against its own test
REC = [
    ('R64 other_act: the control is not written to guard-controls.txt', R,
     "        printf '%s\\t%s\\n' \"$1\" \"$st:$n:0\" >> \"$RDIR/guard-controls.txt\"\n", ''),
    ('R65 other_act: one control, before the first act alone', R, CONTROL,
     "        [ \"$_i\" = 1 ] && { \"$2\" \"$st\" \"$n\" 0 || rc=$?; } || rc=6\n"),
    ('R66 other_act: the control is on the number one past and the row\'s group, as before', R, CONTROL,
     "        \"$2\" \"$st\" $((n + 1)) \"$g\" || rc=$?\n"),
    ('R67 other_act: the control is on group 1', R, CONTROL, "        \"$2\" \"$st\" \"$n\" 1 || rc=$?\n"),
    ('R68 other_act: the control is on group 0 before the first act and on the row\'s group after', R, CONTROL,
     "        if [ \"$_i\" = 1 ]; then \"$2\" \"$st\" \"$n\" 0 || rc=$?; else \"$2\" \"$st\" $((n + 1)) \"$g\" || rc=$?; fi\n"),
    ('R60 other_act: no control', R, CONTROL, "        rc=6\n"),
    ('R62 other_act: the control acts on the row as read', R, CONTROL, "        \"$2\" \"$st\" \"$n\" \"$g\" || rc=$?\n"),
]

if __name__ == '__main__':
    if os.environ.get('PLANTS5') == 'recorder':
        plants.TESTS[R] = [('sh', 'tests/kernel/test_cut_record.sh')]
        plants.PLANTS = REC
    else:
        plants.PLANTS = PLAN
    plants.main()
