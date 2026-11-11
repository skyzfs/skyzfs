import sys
sys.path.insert(0, '.')
import daemon_check as dc, test_daemon_check as t
ALL = t.ALL if hasattr(t, 'ALL') else set('abcde')
def run(label, f, impl='reference'):
    r = t.as_expected(impl); f(r)
    _, d, e = dc.judge(r, ALL, impl=impl)
    print(f'{label}: differs {len(d)} errors {len(e)}')
base = t.as_expected('reference')
print('new lines present in as_expected:', [k for k in base if any(s in k for s in ('delay','unack','overlap','torn','waits','ublksrvd'))])
run('as expected', lambda r: None)
for k, v in [('ctl_delay_before_apply','PASS: found 3'), ('ctl_torn_overlapping_writes','PASS: none torn'),
             ('pin_c_kill_with_applied_unacknowledged','ERROR: 0'), ('pin_device_waits_for_a_server','FAIL: x'),
             ('pin_overlapping_writes_apply_whole','FAIL: 1 torn'), ('pin_no_ublksrvd_entry_from_daemon','PASS: x'),
             ('pin_c_delay_cases','FAIL: x')]:
    run(f'{k} = {v}', lambda r: r.__setitem__(k, v))
for k in ['ctl_delay_before_apply','pin_c_delay_cases','pin_device_waits_for_a_server','obs_a_unordered_overlapping_writes','obs_c_delay_kills']:
    run(f'{k} missing', lambda r: r.pop(k, None))
run('obs unordered: 0 torn', lambda r: r.__setitem__('obs_a_unordered_overlapping_writes','0 of 100000 rounds torn'))
