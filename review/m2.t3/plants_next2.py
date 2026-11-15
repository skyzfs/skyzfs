#!/usr/bin/env python3
"""#99 at 6e0ecd8: plants in the code the answer changed (the status, the unplaced requests,
the new controls and the table), with the four test files.

    plants_next2.py CHECKOUT
"""
import plants

CDC = plants.CDC
plants.TESTS.append('tests/kernel/test_cut_daemon_main.py')
plants.PLANTS[:] = [
    ('N1 a failed cuts line does not fail the check', CDC,
     "        if not ok and not cannot:\n            rc = 1", "        if False:\n            rc = 1"),
    ('N2 a cuts line that is the rig\'s error ends with 0', CDC,
     "    if errored and rc == 0:\n        rc = 2", "    if False:\n        rc = 2"),
    ('N3 a cuts line that passes is an error too where the log holds what cannot be placed', CDC,
     "        cannot = [] if ok else unplaced(records, dev)", "        cannot = unplaced(records, dev)"),
    ('N4 the control last leaves nothing out', CDC,
     "        'last': dcuts[:-1],", "        'last': list(dcuts),"),
    ('N5 the control extra adds nothing', CDC,
     "        'extra': dcuts + [dict(dcuts[-1], prefix=after(dcuts[-1]['prefix']) or dcuts[-1]['prefix'] + 1)],",
     "        'extra': list(dcuts),"),
    ('N6 the control late moves nothing', CDC,
     "[dict(c, prefix=nxt) if k == mid else c for k, c in enumerate(dcuts)]", "list(dcuts)"),
    ('N7 a rule that gives the design\'s cuts is compared all the same', CDC,
     "            if rule != 'early' and compare(rref, ref)[0]:", "            if False:"),
    ('N8 a control that differs from the table does not fail the check', CDC,
     "            if not good:\n                rc = 1", "            pass"),
    ('N9 where the table allows two verdicts it allows any', CDC,
     "            good = verdict in want if isinstance(want, tuple) else verdict == want",
     "            good = True if isinstance(want, tuple) else verdict == want"),
    ('N10 a recording with no row of the table is taken', CDC,
     "        if name not in RECS:", "        if False:"),
    ('N11 the status\'s time without the clock\'s tick', CDC,
     "    return after - int(age) * 1_000_000 + CLOCK_NS", "    return after - int(age) * 1_000_000"),
    ('N12 a write-zeroes anywhere counts as one the check cannot place', CDC,
     "    n = sum(1 for r in records if r.get('op') == 'write_zeroes'\n"
     "            and cut_ref.touches_label(r['offset'], r['bytes'], labels))",
     "    n = sum(1 for r in records if r.get('op') == 'write_zeroes')"),
    ('N13 the control early is the reference by the design\'s rule', CDC,
     "            rcs, _ = cut_ref.cuts(records, ubs, rule)",
     "            rcs, _ = cut_ref.cuts(records, ubs, rule) if rule != 'early' else cut_ref.cuts(records, ubs)"),
    ('N14 a flush after a write with FUA is not counted as unplaced', CDC,
     "    n = sum(1 for r in records if r.get('op') == 'fua_flush')", "    n = 0"),
    ('N15 a status that differs passes if it was written after the last cut', CDC,
     "    if not bad:\n        return True,", "    if not bad or (latest is not None and times and latest >= times[-1]):\n        return True,"),
    ('N16 the table expects txg-alone to be the same on rewind', CDC,
     "    'txg-alone': dict({r: 'same' for r in RECS}, rewind='caught', rewindf='caught',",
     "    'txg-alone': dict({r: 'same' for r in RECS}, rewind='same', rewindf='caught',"),
    ('N17 the table expects count-repeats to be the same on multihost', CDC,
     "    'count-repeats': dict({r: 'same' for r in RECS}, multihost='caught', stray=('same', 'caught')),",
     "    'count-repeats': dict({r: 'same' for r in RECS}, multihost='same', stray=('same', 'caught')),"),
]
plants.main()
