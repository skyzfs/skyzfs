use super::*;

/// Every flag the driver of Linux 6.5 and later reports.
const ALL: u64 = (1 << 3) | (1 << 4) | (1 << 6) | 0x1 | 0x2 | 0x20 | 0x80;

/// A driver with every flag: the start goes on.
#[test]
fn a_driver_with_every_flag_serves() {
    assert_eq!(check(Some(ALL)), Ok(()));
    assert_eq!(check(Some(u64::MAX)), Ok(()));
    // What a device of the spike's kernels reported, and the flags the
    // daemon asks for alone.
    assert_eq!(check(Some(0x2605a)), Ok(()));
}

/// A driver without one flag is refused, naming that flag and the
/// first kernel that has it, and nothing else.
#[test]
fn a_missing_flag_is_named_with_its_kernel() {
    for flag in NEEDED {
        let refused = check(Some(ALL & !flag.bit));
        assert_eq!(refused, Err(Unsupported::Missing(vec![flag])));
        let said = refused.unwrap_err().to_string();
        assert!(said.contains(flag.name), "{said}");
        assert!(said.contains(&format!("Linux {}", flag.kernel)), "{said}");
        assert!(
            said.contains(&format!("Linux {OLDEST_KERNEL} or later")),
            "{said}"
        );
        for other in NEEDED.iter().filter(|o| o.name != flag.name) {
            // Names are prefixes of each other: matched with what
            // follows a name in the message.
            assert!(!said.contains(&format!("{} (", other.name)), "{said}");
        }
        assert!(!said.contains("not supported"), "{said}");
    }
}

/// A driver without any of the flags names all three, in order.
#[test]
fn every_missing_flag_is_named() {
    let refused = check(Some(0)).unwrap_err();
    assert_eq!(refused, Unsupported::Missing(NEEDED.to_vec()));
    let said = refused.to_string();
    let at: Vec<usize> = NEEDED.iter().map(|f| said.find(f.name).unwrap()).collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{said}");
}

/// A driver without the features command is refused, naming the
/// command and its kernel.
#[test]
fn a_driver_without_the_features_command_is_refused() {
    assert_eq!(check(None), Err(Unsupported::NoFeatures));
    let said = Unsupported::NoFeatures.to_string();
    assert!(said.contains("UBLK_U_CMD_GET_FEATURES"), "{said}");
    assert!(said.contains(&format!("Linux {FEATURES_KERNEL}")), "{said}");
}

/// The driver's answer: the features; `ENODEV` from a driver before
/// 6.5, which has no such command; any other error is an error.
#[test]
fn the_answer_to_the_features_command_is_read() {
    assert_eq!(answered(Ok(ALL)), Ok(Some(ALL)));
    assert_eq!(answered(Err(19)), Ok(None));
    assert_eq!(answered(Err(1)), Err(1));
    assert_eq!(answered(Err(95)), Err(95));
}

/// The kernels are in order: the oldest the daemon runs on is the
/// newest of the kernels of what it needs.
#[test]
fn the_oldest_kernel_is_the_newest_of_the_needed() {
    let version = |v: &str| -> (u32, u32) {
        let (major, minor) = v.split_once('.').unwrap();
        (major.parse().unwrap(), minor.parse().unwrap())
    };
    let newest = NEEDED
        .iter()
        .map(|f| f.kernel)
        .chain([FEATURES_KERNEL, DELETE_KERNEL])
        .map(version)
        .max()
        .unwrap();
    assert_eq!(version(OLDEST_KERNEL), newest);
}
