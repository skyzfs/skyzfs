use super::*;

const DEVICE: u64 = 0x10_0003;

fn leaf(kind: VdevKind, guid: u64, rdev: Option<u64>) -> Vdev {
    Vdev {
        kind,
        guid,
        rdev,
        children: Vec::new(),
    }
}

fn layout() -> Layout {
    Layout {
        data: vec![leaf(VdevKind::Disk, 77, Some(DEVICE))],
        special: 0,
        dedup: 0,
        other: 0,
        busy: None,
        removed: None,
    }
}

fn props() -> Props {
    Props {
        guid: 900,
        load_guid: 1,
        multihost: false,
        checkpoint: false,
        readonly: false,
    }
}

fn enrolled() -> Enrolled {
    Enrolled {
        pool: "tank".into(),
        pool_guid: 900,
        leaf_guid: 77,
    }
}

/// Mutant: any one rule of the premise left out.
#[test]
fn a_pool_is_one_leaf_disk_on_the_volume_and_nothing_else_it_must_not_be() {
    let check =
        |layout: &Layout, props: &Props, nocacheflush: bool, enrolled: Option<&Enrolled>| {
            premise(layout, props, nocacheflush, DEVICE, "tank", enrolled)
        };
    assert_eq!(check(&layout(), &props(), false, None), Ok(77));
    assert_eq!(check(&layout(), &props(), false, Some(&enrolled())), Ok(77));
    // Logs, caches and spares not in use are allowed (the lead's Q3).
    let with_log = Layout {
        other: 3,
        ..layout()
    };
    assert_eq!(check(&with_log, &props(), false, None), Ok(77));

    let with = |change: &dyn Fn(&mut Layout)| {
        let mut layout = layout();
        change(&mut layout);
        layout
    };
    let mirror = Vdev {
        kind: VdevKind::Mirror,
        guid: 5,
        rdev: None,
        children: vec![
            leaf(VdevKind::Disk, 77, Some(DEVICE)),
            leaf(VdevKind::Disk, 78, Some(DEVICE + 1)),
        ],
    };
    type Case = (&'static str, Layout, fn(&Refusal) -> bool);
    let layouts: Vec<Case> = vec![
        ("no data vdev", with(&|l| l.data.clear()), |r| {
            matches!(r, Refusal::Layout(_))
        }),
        (
            "a second data vdev",
            with(&|l| l.data.push(leaf(VdevKind::Disk, 78, Some(DEVICE + 1)))),
            |r| matches!(r, Refusal::Layout(_)),
        ),
        ("a mirror", with(&|l| l.data[0] = mirror.clone()), |r| {
            matches!(r, Refusal::Layout(_))
        }),
        (
            "a spare in use",
            with(&|l| {
                l.data[0] = Vdev {
                    kind: VdevKind::Spare,
                    ..mirror.clone()
                }
            }),
            |r| matches!(r, Refusal::Layout(_)),
        ),
        (
            "a leaf being replaced",
            with(&|l| {
                l.data[0] = Vdev {
                    kind: VdevKind::Replacing,
                    ..mirror.clone()
                }
            }),
            |r| matches!(r, Refusal::Layout(_)),
        ),
        (
            "a disk with children",
            with(&|l| l.data[0].children = mirror.children.clone()),
            |r| matches!(r, Refusal::Layout(_)),
        ),
        ("a file", with(&|l| l.data[0].kind = VdevKind::File), |r| {
            matches!(r, Refusal::Layout(_))
        }),
        (
            "another device",
            with(&|l| l.data[0].rdev = Some(DEVICE + 1)),
            |r| matches!(r, Refusal::Device { .. }),
        ),
        ("a device not read", with(&|l| l.data[0].rdev = None), |r| {
            matches!(r, Refusal::Device { leaf: None, .. })
        }),
        ("a special vdev", with(&|l| l.special = 1), |r| {
            *r == Refusal::Class
        }),
        ("a dedup vdev", with(&|l| l.dedup = 1), |r| {
            *r == Refusal::Class
        }),
        (
            "a resilver",
            with(&|l| l.busy = Some("resilver in progress".into())),
            |r| matches!(r, Refusal::Busy(_)),
        ),
        (
            "a vdev removed",
            with(&|l| {
                l.removed = Some(
                    "remove: Removal of vdev 1 copied 1.20G in 0h0m, completed on Wed Nov 25 2026"
                        .into(),
                )
            }),
            |r| matches!(r, Refusal::Removed(_)),
        ),
    ];
    for (what, layout, refusal) in layouts {
        let got = check(&layout, &props(), false, None);
        assert!(got.as_ref().is_err_and(refusal), "{what}: {got:?}");
    }
    let refused: Vec<(&str, Props, bool, Enrolled, Refusal)> = vec![
        (
            "multihost",
            Props {
                multihost: true,
                ..props()
            },
            false,
            enrolled(),
            Refusal::Multihost,
        ),
        (
            "a checkpoint",
            Props {
                checkpoint: true,
                ..props()
            },
            false,
            enrolled(),
            Refusal::Checkpoint,
        ),
        (
            "read-only",
            Props {
                readonly: true,
                ..props()
            },
            false,
            enrolled(),
            Refusal::Readonly,
        ),
        (
            "nocacheflush",
            props(),
            true,
            enrolled(),
            Refusal::Nocacheflush,
        ),
        (
            "another pool's GUID",
            Props {
                guid: 901,
                ..props()
            },
            false,
            enrolled(),
            Refusal::Enrolled,
        ),
        (
            "another leaf",
            props(),
            false,
            Enrolled {
                leaf_guid: 78,
                ..enrolled()
            },
            Refusal::Enrolled,
        ),
        (
            "another pool's name",
            props(),
            false,
            Enrolled {
                pool: "tank2".into(),
                ..enrolled()
            },
            Refusal::Enrolled,
        ),
    ];
    for (what, props, nocacheflush, enrolled, refusal) in refused {
        assert_eq!(
            check(&layout(), &props, nocacheflush, Some(&enrolled)),
            Err(refusal),
            "{what}"
        );
    }
}
