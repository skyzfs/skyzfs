#!/usr/bin/env python3
"""The control rig's table and judge (tests/kernel/control-rig.sh).

    control_check.py OUT_DIR > summary.md

The rig asks the daemon's binding to the ublk driver, through the test
program `skyzfs-ublk-control` and the crate's own kernel tests, what it
says and does about devices in each state the driver knows, and about
devices of another server ("strangers", made with the reference
server). `OUT_DIR/results.tsv` has one line per result, `key<tab>value`:

    control_<property>   PASS | FAIL: why      from the crate's kernel tests
    pin_<name>           PASS: ... | FAIL: ... | ERROR: ...
    ctl_<name>           the same; a control is expected to FAIL
    obs_<name>           an observation, judged by nothing

Exit 0: every line is as the table EXPECTED has it. Exit 1: a line
differs. Exit 2: the rig itself failed (a line missing, a line the
table does not know, an ERROR); not a verdict on the binding.

Each entry names the row of the design's table of device states
(docs/skyzfs-design.md §4.2, "The states of a device") or the sentence
of §4.2 it puts a run behind.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from daemon_check import NOT_RUN  # noqa: E402

JUDGED = ('pin_', 'ctl_', 'control_')


def read_results(path):
    """-> (key -> value; rig errors). A judged line written twice with
    two results is a rig error: the last would decide which counts."""
    out, twice = {}, []
    with open(path, encoding='utf-8', errors='replace') as f:
        for line in f:
            key, _, value = line.rstrip('\n').partition('\t')
            if not key:
                continue
            if key.startswith(JUDGED) and key in out and out[key].partition(':')[0] != value.partition(':')[0]:
                twice.append(f"{key}: written twice, as {out[key].partition(':')[0]} and as {value.partition(':')[0]}")
            out[key] = value
    return out, twice

CRATE = "the crate's kernel test"
RIG = 'the rig'
STATES = ('live', 'waiting', 'never_started', 'deleted_held')
STATE_WORDS = {
    'live': 'live, served by the reference server',
    'waiting': 'waiting, its reference server killed',
    'never_started': 'added by another process and never started (the reference tool has no add without a start: '
                     'the test program adds it, with another depth than the add under test asks for)',
    'deleted_held': 'deleted while a process held it open',
}

# name -> (expected, who produces it, the design's row or sentence, the claim)
EXPECTED = {
    # The crate's own tests: no stranger needed.
    'control_can_reissue': ('PASS', CRATE, 'Recovery', 'the driver offers recovery with reissue'),
    'control_lookup_free_is_none': ('PASS', CRATE, 'row: not there', 'a lookup of a free number says nothing is there'),
    'control_add_taken_delete': (
        'PASS', CRATE, 'rows: an add under a free number; an add under its number; a delete, unheld',
        'an added device is dead with what was asked (flags under the mask); a second add is "taken" and '
        'changes nothing; a delete frees the number'),
    'control_start_no_flag_and_keep': (
        'PASS', CRATE, 'row: dead, never started, is started; Library',
        "the library's control object made with no flag starts a device the binding added; an orderly end "
        'leaves it waiting; dropping the library\'s objects deletes nothing'),
    'control_take_over_beside_stale_file': (
        'PASS', CRATE, 'Library', 'a take-over works beside a stale file of another server under /run/ublksrvd '
        'and leaves it as it was'),
    'control_buffers_4k_1m_across_take_over': (
        'PASS', CRATE, 'Library: the buffers',
        'writes of 4 KiB to 1 MiB through the binding\'s queue read back the same, in a first life of the '
        'device and after a take-over'),
    'control_test_program_steps': ('PASS', CRATE, 'none: the rig\'s own means',
                                   'the test program answers each step in one line'),

    # The binding's own device, step by step.
    'pin_orderly_stop_leaves_waiting': (
        'PASS', RIG, 'row: live, its server ends (an orderly stop)',
        'a server ended by SIGTERM leaves its device waiting, with the block device there'),
    'pin_killed_server_device_waits': (
        'PASS', RIG, 'row: live, its server ends (a kill)',
        'a server killed with SIGKILL leaves its device waiting within 5 s, the daemon\'s limit'),
    'pin_take_over_hands_nothing_before_the_end': (
        'PASS', RIG, 'row: waiting, a server begins a recovery, fetches, and ends it',
        'with 2 s between the queue being ready and the end of the recovery, and a reader blocked on the '
        'device, the new server is handed no request before the end'),
    'pin_take_over_reissues_a_request_in_flight': (
        'PASS', RIG, 'row: waiting, a server begins a recovery, fetches, and ends it',
        'a read held by the server that was killed returns its data after the take-over, with no error'),
    'pin_abandoned_recovery_leaves_waiting': (
        'PASS', RIG, 'row: waiting, a server begins a recovery and ends before ending it',
        'after a recovery that was begun and not ended the device still waits, and the next take-over works'),
    'pin_begin_recovery_on_live_says_busy': (
        'PASS', RIG, 'row: live, another process begins a recovery',
        'the binding passes the driver\'s answer on: busy; the server and its I/O are as before'),
    'pin_begin_recovery_on_never_started_says_busy': (
        'PASS', RIG, 'row: dead, never started',
        'a recovery begun on a device that was added and never started answers busy, and the device '
        'is as before. Busy is so not only "a server is still being torn down": seen on both images '
        'in the binding\'s own runs before this pin was written'),
    'pin_begin_recovery_on_free_says_nothing': (
        'PASS', RIG, 'row: not there', 'a recovery begun on a free number answers that nothing is there'),
    'pin_delete_returns_while_held': (
        'PASS', RIG, 'row: in any state, held open, a delete',
        'the binding\'s delete of a device a process holds open returns within 2 s'),
    'pin_deleted_held_is_dead_and_keeps_number': (
        'PASS', RIG, 'row: in any state, held open, a delete',
        'the deleted device reads as dead, has no nodes, and an add under its number is "taken"'),
    'pin_delete_fails_requests': (
        'PASS', RIG, 'row: in any state, held open, a delete',
        'a request in flight at the delete fails for the holder, and so does a later one'),
    'pin_number_free_when_holder_lets_go': (
        'PASS', RIG, 'row: in any state, held open, a delete',
        'when the holder closes the device, a lookup of the number says nothing is there'),
    'pin_dead_after_delete_is_not_taken_over': (
        'PASS', RIG, 'row: dead after a delete',
        'a recovery begun on a deleted and held device is not begun, and the device is as before'),
    'pin_delete_unheld_waiting_frees_number': (
        'PASS', RIG, 'row: in any state, with nothing holding the block device open, a delete',
        'a waiting device nobody holds is gone after the binding\'s delete, its number free'),
    'pin_client_holds_exclusive_holder': (
        'PASS', RIG, 'Library', 'a process holding the device open exclusively is detected: held'),
    'pin_client_holds_plain_reader_unknown': (
        'PASS', RIG, 'Library', 'a process holding the device open without O_EXCL is not detected: unknown, never "not held"'),
    'pin_no_file_under_run_ublksrvd': (
        'PASS', RIG, 'Library',
        'after an add, a serve, a take-over, a stop and a delete by the binding, /run/ublksrvd has no entry '
        'under the device\'s number newer than the first of them'),
    'pin_added_device_is_dead_when_its_adder_ends': (
        'PASS', RIG, 'row: dead, never started, the process that added it ends',
        'the test program adds a device and ends without starting it; the binding then reports the '
        'device as dead, under the same number'),
    'pin_set_params_refused_once_started': (
        'PASS', RIG, 'sentence: a device\'s parameters cannot be changed once it has been started',
        'on a waiting device that was started, the set-parameters command with another size is not '
        'answered "set", and the size the binding reports is the old one'),
    'pin_info_server_pid_is_the_servers': (
        'PASS', RIG, 'The states of a device: live',
        'the server_pid the binding reports of a live device (the driver\'s value, unchanged) is the '
        'pid of the process serving it, for the binding\'s own server and for the reference server'),
    'ctl_add_on_free_number_is_taken': (
        'FAIL', RIG, 'control', 'control: the check that an add answers "taken", made on a free number'),
    'ctl_stranger_deleted_between_looks': (
        'FAIL', RIG, 'control', 'control: the check that a stranger is untouched, with the rig deleting the '
        'stranger between its two looks'),
}
# The strangers: a device of the reference server in each state.
for _state in STATES:
    _what = STATE_WORDS[_state]
    _row = ('rows: an add under its number; dead after a delete' if _state == 'deleted_held'
            else 'row: in any state, an add under its number')
    EXPECTED[f'pin_add_on_{_state}_stranger_is_taken'] = (
        'PASS', RIG, _row, f'an add under the number of a stranger that is {_what} answers "taken"')
    EXPECTED[f'pin_{_state}_stranger_untouched'] = (
        'PASS', RIG, 'Library: the daemon must never delete a stranger\'s device',
        f'after the refused add, a stranger that is {_what} has the same state, server and parameters'
        + (', and a write and a read through it succeed' if _state == 'live' else ''))
    EXPECTED[f'pin_info_of_{_state}_stranger'] = (
        'PASS', RIG, 'The states of a device: the three states',
        f'the binding reports a stranger that is {_what} as '
        + {'live': 'live', 'waiting': 'waiting'}.get(_state, 'dead'))

OBSERVED = {
    'obs_killed_server_live_ms': 'how long a killed server\'s device still reads live, by the binding\'s lookup',
    'obs_flags_beyond_the_mask': 'the flag bits the driver reports beyond those asked for',
    'obs_serve_of_a_device_whose_adder_ended': 'whether another process can still start a device that was '
                                               'added and never started, its adder gone (the design does '
                                               'not say; the daemon deletes such a device)',
    # Lines the crate's tests print beside their verdicts.
    'control_started_detail': 'the crate\'s test of a start: requests answered and first-write copies',
    'control_buffers_detail': 'the crate\'s test of the buffers: first-write copies and the longest write',
    'control_set_params_on_a_started_device': 'the crate\'s own word on set-parameters after a start',
    'obs_delete_ms': 'how long the binding\'s delete took, held and unheld',
}


def judge(results):
    """-> (rows, differs, rig_errors). A row is (name, expected, got, detail, who, row, claim)."""
    rows, differs, errors = [], [], []
    for name, (want, who, row, claim) in EXPECTED.items():
        value = results.get(name)
        if value is None:
            errors.append(f'{name}: no result')
            rows.append((name, want, 'missing', '', who, row, claim))
            continue
        got, _, detail = value.partition(': ')
        if got not in ('PASS', 'FAIL'):
            errors.append(f'{name}: {value}')
        elif got != want:
            differs.append(f'{name}: expected {want}, got {got}: {detail}')
        rows.append((name, want, got, detail, who, row, claim))
    for name in OBSERVED:
        if name not in results:
            errors.append(f'{name}: no observation')
        elif NOT_RUN.search(results[name]):
            errors.append(f'{name}: {results[name]}')
    for name in results:
        if name.startswith(JUDGED) and name not in EXPECTED and name not in OBSERVED:
            errors.append(f'{name}: not in the table EXPECTED: {results[name]}')
    if results.get('setup') != 'ok':
        errors.append(f"setup: {results.get('setup', 'no line')}")
    return rows, differs, errors


def summary(results, rows, differs, errors):
    out = ['## Control rig: the binding\'s control commands', '',
           f"kernel {results.get('kernel', '?')}, binding at {results.get('commit', '?')}", '']
    if errors:
        out += ['**The rig itself failed; this is not a verdict on the binding:**', ''] + [f'- {e}' for e in errors] + ['']
    if differs:
        out += ['**Differs from the table:**', ''] + [f'- {d}' for d in differs] + ['']
    if not errors and not differs:
        out += ['Every line is as the table has it.', '']
    out += ['| Line | Expected | Got | From | Design | Claim | Detail |', '|---|---|---|---|---|---|---|']
    for name, want, got, detail, who, row, claim in rows:
        out.append(f'| `{name}` | {want} | {got} | {who} | {row} | {claim} | {detail.replace("|", "/")} |')
    out += ['', '| Observation | What it is | Value |', '|---|---|---|']
    for name, what in OBSERVED.items():
        out.append(f'| `{name}` | {what} | {results.get(name, "missing").replace("|", "/")} |')
    return '\n'.join(out) + '\n'


def main(argv):
    if len(argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    results, twice = read_results(os.path.join(argv[1], 'results.tsv'))
    rows, differs, errors = judge(results)
    errors = twice + errors
    sys.stdout.write(summary(results, rows, differs, errors))
    return 2 if errors else 1 if differs else 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
