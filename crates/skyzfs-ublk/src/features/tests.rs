use super::*;

/// Every flag the driver of Linux 6.5 and later reports.
const ALL: u64 = (1 << 3) | (1 << 4) | (1 << 6) | 0x1 | 0x2 | 0x20 | 0x80;

/// Releases below, at and above 6.9, as `uname -r` gives them.
const BELOW: [&str; 4] = ["6.8.0-45-generic", "6.5.0", "5.14.0-570.el9.x86_64", "6.08"];
const AT_OR_ABOVE: [&str; 6] = [
    "6.9.0",
    "6.9",
    "6.11.0-29-generic",
    "7.0.0",
    "6.10-rc1",
    "10.0.0",
];
/// Releases that do not begin with a major and a minor number.
const UNREADABLE: [&str; 9] = [
    "",
    "6",
    "6.",
    ".9",
    "v6.9",
    "six.nine",
    "6-9",
    " 6.9",
    "99999999999.1",
];

/// A driver with every flag, on a kernel of 6.9 or later: the start
/// goes on.
#[test]
fn a_driver_with_every_flag_on_a_new_enough_kernel_serves() {
    for release in AT_OR_ABOVE {
        assert_eq!(check(Some(ALL), release, false), Ok(()), "{release}");
        assert_eq!(check(Some(u64::MAX), release, false), Ok(()), "{release}");
        // What a device of the spike's kernels reported.
        assert_eq!(check(Some(0x2605a), release, false), Ok(()), "{release}");
        // The override changes nothing here.
        assert_eq!(check(Some(ALL), release, true), Ok(()), "{release}");
    }
}

/// The release is read strictly: digits, a dot, digits, then the end or
/// anything but a digit.
#[test]
fn the_release_is_read_from_its_start() {
    assert_eq!(release_number("6.8.0-45-generic"), Some((6, 8)));
    assert_eq!(release_number("7.0.0"), Some((7, 0)));
    assert_eq!(release_number("5.14.0-570.el9.x86_64"), Some((5, 14)));
    assert_eq!(release_number("6.10-rc1"), Some((6, 10)));
    assert_eq!(release_number("6.9"), Some((6, 9)));
    // "6.08" is 6.8, not 6.80.
    assert_eq!(release_number("6.08"), Some((6, 8)));
    for release in UNREADABLE {
        assert_eq!(release_number(release), None, "{release:?}");
    }
}

/// A kernel below 6.9 is refused for the asynchronous delete, naming
/// the command, its kernel, why, and the override; with the override it
/// serves.
#[test]
fn a_release_below_the_asynchronous_delete_is_refused_unless_asserted() {
    for release in BELOW {
        let refused = check(Some(ALL), release, false);
        assert_eq!(
            refused,
            Err(Unsupported(vec![Lack::Release(release.to_string())])),
            "{release}"
        );
        let said = refused.unwrap_err().to_string();
        assert_delete_said(&said);
        assert!(said.contains(release), "{said}");
        assert_eq!(check(Some(ALL), release, true), Ok(()), "{release}");
    }
}

/// A release that does not parse is a refusal that says so, with the
/// same override; not a pass.
#[test]
fn an_unreadable_release_is_refused_unless_asserted() {
    for release in UNREADABLE {
        let refused = check(Some(ALL), release, false);
        assert_eq!(
            refused,
            Err(Unsupported(vec![Lack::UnreadableRelease(
                release.to_string()
            )])),
            "{release:?}"
        );
        let said = refused.unwrap_err().to_string();
        assert_delete_said(&said);
        assert!(
            said.contains("does not begin with a major and a minor"),
            "{said}"
        );
        assert_eq!(check(Some(ALL), release, true), Ok(()), "{release:?}");
    }
}

/// What a refusal for the delete says: the command, Linux 6.9, why the
/// daemon will not run without it, and the operator's override.
fn assert_delete_said(said: &str) {
    assert!(said.contains("UBLK_U_CMD_DEL_DEV_ASYNC"), "{said}");
    assert!(said.contains("Linux 6.9"), "{said}");
    assert!(
        said.contains("cannot give up a failed volume's device"),
        "{said}"
    );
    assert!(said.contains(ASYNC_DELETE_OPTION), "{said}");
    assert!(
        said.contains(&format!("Linux {OLDEST_KERNEL} or later")),
        "{said}"
    );
    assert!(!said.contains("not supported"), "{said}");
}

/// A driver without one flag is refused, naming that flag and the
/// first kernel that has it, and nothing else; on every release and
/// with the override, which does not cover flags. Below 6.9 the release
/// is named as well.
#[test]
fn a_missing_flag_is_named_with_its_kernel() {
    let releases = AT_OR_ABOVE.iter().chain(&BELOW).chain(&UNREADABLE);
    for (release, asserted) in releases.flat_map(|r| [(*r, false), (*r, true)]) {
        let old = !asserted && !AT_OR_ABOVE.contains(&release);
        for flag in NEEDED {
            let refused = check(Some(ALL & !flag.bit), release, asserted);
            let lacks = refused.clone().unwrap_err().0;
            assert_eq!(lacks[0], Lack::Flag(flag), "{release:?} {asserted}");
            assert_eq!(lacks.len(), 1 + usize::from(old), "{release:?} {asserted}");
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
            assert_eq!(said.contains("UBLK_U_CMD_DEL_DEV_ASYNC"), old, "{said}");
            assert!(!said.contains("not supported"), "{said}");
        }
    }
}

/// A driver without any of the flags names all three, in order, and
/// then the release when it is below 6.9.
#[test]
fn every_missing_flag_is_named() {
    let refused = check(Some(0), "7.0.0", false).unwrap_err();
    let flags: Vec<Lack> = NEEDED.iter().map(|f| Lack::Flag(*f)).collect();
    assert_eq!(refused, Unsupported(flags.clone()));
    let said = refused.to_string();
    let at: Vec<usize> = NEEDED.iter().map(|f| said.find(f.name).unwrap()).collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{said}");

    let refused = check(Some(0), "6.8.0-45-generic", false).unwrap_err();
    let mut lacks = flags;
    lacks.push(Lack::Release("6.8.0-45-generic".to_string()));
    assert_eq!(refused, Unsupported(lacks));
}

/// A driver without the features command is refused, naming the
/// command and its kernel, whatever the release and with the override;
/// below 6.9 or unreadable, the release is named as well.
#[test]
fn a_driver_without_the_features_command_is_refused() {
    for release in AT_OR_ABOVE {
        assert_eq!(
            check(None, release, false),
            Err(Unsupported(vec![Lack::NoFeatures]))
        );
    }
    for release in BELOW.iter().chain(&UNREADABLE) {
        assert_eq!(
            check(None, release, true),
            Err(Unsupported(vec![Lack::NoFeatures]))
        );
        let lacks = check(None, release, false).unwrap_err().0;
        assert_eq!(lacks.len(), 2, "{release:?}");
        assert_eq!(lacks[0], Lack::NoFeatures);
    }
    let said = check(None, "7.0.0", false).unwrap_err().to_string();
    assert!(said.contains("UBLK_U_CMD_GET_FEATURES"), "{said}");
    assert!(said.contains(&format!("Linux {FEATURES_KERNEL}")), "{said}");
    assert!(
        said.contains(&format!("Linux {OLDEST_KERNEL} or later")),
        "{said}"
    );
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
        .chain([FEATURES_KERNEL])
        .map(version)
        .chain([DELETE_KERNEL])
        .max()
        .unwrap();
    assert_eq!(version(OLDEST_KERNEL), newest);
}
