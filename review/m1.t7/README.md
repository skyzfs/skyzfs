# M1.T7 (pull request 20), first look at e260e7b

Base c9c87df. Read: `control_check.py`, `control-rig.sh`, the diff of
`daemon-rig.sh` and `daemon_check.py`, the workflows, `net-try.sh`.
Ran: `test_daemon_check.py` (34), `test_control_check.py` (17),
`test_net_try.sh`: all pass. No kernel here; no run's log read.

## One experiment with the control rig's judge

Results as the table has them, except `pin_kernel_tests_end` = FAIL
(the binary given up on after 600 s) and two `control_*` lines missing
(the tests that never ran):

    differs: pin_kernel_tests_end: expected PASS, got FAIL
    errors:  control_stop_under_writers_leaves_later_memory_alone: no result
             control_test_program_steps: no result
    exit 2, headline "The rig itself failed; this is not a verdict on
    the binding"

So a kernel test of the binding that hangs is reported as the rig's
own error.

## Paths that start no rig

`skyzfs-daemon-rig.yml`, pull_request paths: no `Cargo.lock`, no root
`Cargo.toml`, no `tests/lib/**` (the rig sources `lim.sh`, `net-try.sh`,
`gnu-dd.sh` from there). The job `plan` counts only `tests/kernel/` and
the workflow file as "the rig".
`skyzfs-control-rig.yml`: the same three, and no `crates/skyzfs-blk/**`,
`tests/kernel/build-ublksrv.sh`, `tests/kernel/ublksrvd-look.sh`.
