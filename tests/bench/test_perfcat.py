"""Tests of perfcat.py: python3 tests/bench/test_perfcat.py"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import perfcat as pc  # noqa: E402

# As `perf script -F comm,tid,ip,sym` prints a recording with call graphs.
SCRIPT = """\
    skyzfs-pump  4101
\tffffffff81a1b2c3 copy_user_enhanced_fast_string
\tffffffff81a00001 _copy_from_iter
\tffffffffc0a00002 ublk_copy_user_pages
\tffffffffc0a00003 ublk_ch_uring_cmd
\tffffffff81b00004 io_uring_cmd
\tffffffff81b00005 __x64_sys_io_uring_enter
\t    7f00deadbeef io_uring_enter

    skyzfs-pump  4101
\tffffffff81100010 _raw_spin_unlock_irqrestore
\tffffffff81100011 try_to_wake_up
\tffffffff81100012 futex_wake
\tffffffff81100013 __x64_sys_futex
\t    7f00deadbe00 syscall

 skyzfs-reader-0  4102
\tffffffff81a1b2c3 copy_user_enhanced_fast_string
\tffffffff81a00020 copy_page_to_iter
\tffffffff81a00021 filemap_read
\tffffffff81a00022 xfs_file_read_iter
\tffffffff81a00023 __x64_sys_pread64

 skyzfs-reader-1  4103
\t    55d0aabbccdd skyzfsd::store::read
\t    55d0aabbcc00 main

 skyzfs-reader-1  4103
\tffffffff81b00030 io_submit_sqes
\tffffffff81b00031 __x64_sys_io_uring_enter

   iou-wrk-4100  4200
\tffffffff81300040 native_queued_spin_lock_slowpath
\tffffffff81300041 some_lock
"""


class Parse(unittest.TestCase):
    def test_samples(self):
        s = pc.samples(SCRIPT)
        self.assertEqual([c for c, _ in s], ['skyzfs-pump', 'skyzfs-pump', 'skyzfs-reader-0', 'skyzfs-reader-1',
                                             'skyzfs-reader-1', 'iou-wrk-4100'])
        self.assertEqual(s[0][1][0], (0xffffffff81a1b2c3, 'copy_user_enhanced_fast_string'))
        self.assertEqual(len(s[0][1]), 7)

    def test_categories(self):
        self.assertEqual(pc.category(['copy_user_x', '_copy_from_iter', 'ublk_copy_user_pages']), 'copy, ublk')
        self.assertEqual(pc.category(['copy_user_x', 'copy_page_to_iter', 'filemap_read']), 'copy, page cache')
        self.assertEqual(pc.category(['memcpy_orig', 'something']), 'copy, other')
        self.assertEqual(pc.category(['_raw_spin_unlock', 'try_to_wake_up', 'futex_wake']), 'wake and switch')
        self.assertEqual(pc.category(['xas_load', 'filemap_get_pages']), 'page cache')
        self.assertEqual(pc.category(['blk_mq_end_request', 'ublk_commit_completion']), 'ublk driver')
        self.assertEqual(pc.category(['io_submit_sqes', '__x64_sys_io_uring_enter']), 'io_uring')
        self.assertEqual(pc.category(['native_queued_spin_lock_slowpath']), 'other')
        self.assertEqual(pc.category(['_raw_spin_unlock_irqrestore', 'taskq_dispatch_ent', 'zio_interrupt',
                                      'blk_mq_end_request', 'try_to_wake_up']), 'ZFS completion')

    def test_reclaim_memory_group_and_write_back_from_a_write(self):
        # A buffered write that charges a page over the group's limit and
        # reclaims on its own thread (the leaf is no copy).
        self.assertEqual(pc.category(['shrink_folio_list', 'shrink_lruvec', 'shrink_node', 'do_try_to_free_pages',
                                      'try_to_free_mem_cgroup_pages', 'try_charge_memcg', '__mem_cgroup_charge',
                                      '__filemap_add_folio', 'generic_perform_write', '__x64_sys_pwrite64']),
                         'page reclaim')
        # The group's reclaim after a charge went over memory.high.
        self.assertEqual(pc.category(['_raw_spin_unlock_irqrestore', 'mem_cgroup_handle_over_high',
                                      'exit_to_user_mode_prepare']), 'page reclaim')
        # The charge itself, no reclaim.
        self.assertEqual(pc.category(['page_counter_try_charge', 'try_charge_memcg', '__mem_cgroup_charge',
                                      '__filemap_add_folio', 'filemap_add_folio']), 'memory group')
        # The dirty throttle, and write-back under a write call.
        self.assertEqual(pc.category(['schedule', 'io_schedule_timeout', 'balance_dirty_pages',
                                      'balance_dirty_pages_ratelimited_flags', 'generic_perform_write']),
                         'write-back from a write')
        self.assertEqual(pc.category(['ext4_writepages', 'do_writepages', 'filemap_fdatawrite_wbc',
                                      'vfs_write', '__arm64_sys_pwrite64']), 'write-back from a write')
        # An fdatasync's write-back stays the page cache's.
        self.assertEqual(pc.category(['ext4_writepages', 'do_writepages', 'file_write_and_wait_range',
                                      'vfs_fsync_range', '__x64_sys_fdatasync']), 'page cache')
        # A copy is still a copy, whatever is above it.
        self.assertEqual(pc.category(['copy_user_x', 'copy_page_from_iter_atomic', 'generic_perform_write',
                                      'balance_dirty_pages_ratelimited_flags']), 'copy, page cache')

    def test_summarize(self):
        t = pc.summarize(SCRIPT)
        self.assertEqual(sorted(t), ['iou-wrk', 'skyzfs-pump', 'skyzfs-reader'])
        self.assertEqual(t['skyzfs-pump']['samples'], 2)
        self.assertEqual(t['skyzfs-pump']['kernel'], 2)
        self.assertEqual(t['skyzfs-pump']['categories'], {'copy, ublk': 1, 'wake and switch': 1})
        r = t['skyzfs-reader']
        self.assertEqual((r['samples'], r['kernel']), (3, 2))  # one sample in user space
        self.assertEqual(r['categories'], {'copy, page cache': 1, 'io_uring': 1})
        self.assertEqual(r['symbols'], {'copy_user_enhanced_fast_string': 1, 'io_submit_sqes': 1})
        self.assertEqual(t['iou-wrk']['categories'], {'other': 1})


class RunnerKernel(unittest.TestCase):
    """Samples cut from a real recording (run 43285245364, ubuntu-24.04): the symbols this kernel prints."""

    def setUp(self):
        with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'fixtures', 'perfscript-24.04.txt')) as f:
            self.samples = pc.samples(f.read())

    def kinds(self):
        return [(comm, pc.category([s for _, s in frames])) for comm, frames in self.samples]

    def test_each_sample_is_put_in_its_kind(self):
        self.assertEqual(self.kinds(), [
            ('skyzfsd-queue', 'wake and switch'),         # futex_wake -> try_to_wake_up
            ('skyzfsd-queue', 'ZFS completion'),          # zio_interrupt -> taskq_dispatch_ent
            ('skyzfsd-queue', 'copy, ublk'),              # check_heap_object under ublk_copy_user_pages
            ('skyzfs-store', 'copy, page cache'),         # copy_folio_from_iter_atomic, a buffered write
            ('skyzfs-reader-1', 'wake and switch'),       # eventfd_write
            ('skyzfs-reader-3', 'wake and switch'),       # schedule under futex_wait
            ('skyzfs-store', 'copy, page cache'),         # _copy_to_iter under filemap_read
            ('skyzfsd-queue', 'wake and switch'),         # schedule under io_cqring_wait
        ])

    def test_a_copy_is_judged_by_its_leaf_outside_the_ublk_copy(self):
        # A wait that the page cache's read path makes is no copy.
        self.assertEqual(pc.category(['finish_task_switch', 'schedule', 'io_schedule', 'folio_wait_bit_common',
                                      'filemap_read', '_copy_to_iter']), 'wake and switch')


if __name__ == '__main__':
    unittest.main()
