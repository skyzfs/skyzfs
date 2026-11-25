#!/usr/bin/env python3
"""Review of #141 at e0c51f8: the courses' faults put through
publish_check.main with the PR's own stand-in node B
(test_publish_check_main.run_course), with the request log as the
faulty daemon would write it, not only the answers.

Usage: python3 -I scenarios.py TESTS_KERNEL_DIR
"""
import sys

sys.path.insert(0, sys.argv[1])
import test_publish_check_main as m  # noqa: E402

PID = m.PID


def log(lines):
    return m.write('requests.tsv', m.log_text([(PID, lines)]))


def both(*changes):
    def change(d):
        for c in changes:
            c(d)
    return change


CASES = [
    ('prewindf as a right daemon leaves it', 'prewindf', None, None),
    # A daemon without rule (R) on -F's first cut: f1 is never ended, the
    # pool goes on cutting while the rig nudges; no `ended` line.
    ('prewindf, daemon does not end f1 (realistic log)', 'prewindf',
     both(m.answer('f1-end', 'pending after 300 s'),
          log([(8, 52, True), (9, 55, True), (10, 55, False), (11, 56, True), (12, 57, True)])), None),
    # The PR's own test of the same fault keeps the right daemon's `ended` line.
    ('prewindf, the PR test (ended line kept)', 'prewindf', m.answer('f1-end', 'pending after 300 s'), None),
    # A daemon whose mark only sees groups strictly below the top (the
    # equal group of -F's first cut not marked): f1 waits, no `ended` line.
    ('prewindf, mark strictly below the top (realistic log)', 'prewindf',
     both(m.answer('f1-end', 'pending after 300 s'),
          log([(8, 52, True), (9, 55, True), (10, 55, True), (11, 56, True)])), None),
    # A daemon that ends f1 `expired` (its expiry) and writes that line.
    ('prewindf, f1 ended expired', 'prewindf',
     both(m.answer('f1-end', 'expired'),
          log([(8, 52, True), (9, 55, True), (10, 55, False), ('publish', 'ended', 'f1 expired'), (11, 56, True)])),
     None),
    ('prewind as a right daemon leaves it', 'prewind', None, None),
    # M2.5 (no rule R): r1 accepted at the ask and left waiting with the
    # store down; the request log's line is `r1 accepted`.
    ('prewind, M2.5 at the ask (realistic log)', 'prewind',
     both(m.answer('r1', 'pending after 300 s'),
          log([(6, 52, True), (7, 45, False), (8, 50, False), (9, 51, False),
               ('publish', 'asked', 'r1 accepted'), (10, 52, True), (11, 53, True)])), None),
    ('prewind, M2.5 at the ask, the PR test (refusal line kept)', 'prewind',
     m.answer('r1', 'done commit=' + m.commit_of('r1')), None),
    # A daemon that refuses r1 with another code (`refused asked`).
    ('prewind, r1 refused with another code (realistic log)', 'prewind',
     both(m.answer('r1', 'refused asked other parameters'),
          log([(6, 52, True), (7, 45, False), (8, 50, False), (9, 51, False),
               ('publish', 'asked', 'r1 refused asked'), (10, 52, True), (11, 53, True)])), None),
    # #139's D1 (fixed): r1 made by a rewind-only cut before node B's row's.
    ('prewind, r1 on the earlier rewind-only cut 8 (D1)', 'prewind', None, {'r1': (8, 50)}),
    ('prewind, r1 on rewind-only cut 9 (the row named)', 'prewind', None, {'r1': (9, 51)}),
    # r1 made at the top's group (not above) by a cut that opens as itself.
    ('prewind, r1 at the top group 52', 'prewind', None, {'r1': (10, 52)}),
]

for title, course, change, rows in CASES:
    rc, lines = m.run_course(course, change, rows=rows)
    bad = {k: v for k, v in lines.items() if v[0] not in ('ok', 'INFO', 'CAUGHT', 'CAUGHT_AT_FILES')}
    print(f'{title}: exit {rc}')
    for k, (v, detail) in bad.items():
        print(f'    {k} {v}: {detail[:150]}')
