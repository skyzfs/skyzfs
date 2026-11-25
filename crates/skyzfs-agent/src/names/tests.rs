use super::*;

/// Mutant: a field of the date or time off by one, the month and day
/// swapped, a leap day missed, or the `-N` written for none.
#[test]
fn a_name_is_the_utc_second_in_iso_8601s_basic_form() {
    for (unix_s, taken, wanted) in [
        (0, 0, "skz-19700101T000000Z"),
        (59, 0, "skz-19700101T000059Z"),
        (86_399, 0, "skz-19700101T235959Z"),
        (951_782_400, 0, "skz-20000229T000000Z"),
        (951_868_800, 0, "skz-20000301T000000Z"),
        (1_709_164_800, 0, "skz-20240229T000000Z"),
        (1_764_032_706, 0, "skz-20251125T010506Z"),
        (4_102_444_799, 0, "skz-20991231T235959Z"),
        (4_107_456_000, 0, "skz-21000228T000000Z"),
        (4_107_542_400, 0, "skz-21000301T000000Z"),
        (1_764_032_706, 1, "skz-20251125T010506Z-1"),
        (1_764_032_706, 12, "skz-20251125T010506Z-12"),
    ] {
        assert_eq!(name(unix_s, taken), wanted, "{unix_s}");
    }
}

/// Mutant: a character the protocol or ZFS refuses.
#[test]
fn every_name_is_one_the_protocol_takes() {
    for unix_s in [0, 951_782_400, 4_102_444_799, 253_402_300_799] {
        for taken in [0, 1, u32::MAX] {
            let name = name(unix_s, taken);
            assert!(skyzfs_control::valid_name(&name), "{name}");
            assert!(!name.contains(':') && !name.contains(' '), "{name}");
        }
    }
}

/// Names of one length sort as their times, a day at a time across
/// four centuries (every leap rule).
#[test]
fn names_sort_as_their_times() {
    let mut last = name(0, 0);
    for day in 1..146_097 + 365 {
        let next = name(day * 86_400 + day % 86_400, 0);
        assert!(next > last, "{last} then {next}");
        last = next;
    }
}
