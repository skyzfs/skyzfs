use super::*;

const ATTACHMENT: [u8; 16] = *b"an attachment...";
const STATE: [u8; 16] = *b"a cut state.....";
const OTHER: [u8; 16] = *b"another one.....";

fn request(name: &str, after: u64, group: u64) -> Request {
    Request {
        name: name.into(),
        attachment: ATTACHMENT,
        state: STATE,
        after,
        group,
        manifest: b"the pool's manifest".to_vec(),
        expiry: EXPIRY_DEFAULT,
    }
}

fn cut(number: u64, group: u64, itself: bool) -> Known {
    Known {
        cut: CommittedCut {
            state: STATE,
            number,
            group,
        },
        itself,
        commit: [number as u8; 32],
    }
}

fn publishing() -> Publishing {
    Publishing::new(Some(ATTACHMENT), Some(STATE))
}

/// The stage of the publish of `name`, where it is pending.
fn stage(publishing: &Publishing, name: &str) -> Option<Stage> {
    publishing
        .pending
        .iter()
        .find(|p| p.request.name == name)
        .map(|p| p.stage)
}

/// The rule (design §5.2): a cut of the request's state, counted after
/// C, of group G or later, that opens as itself. Each condition alone
/// refuses a cut, the rewind case among them: a cut counted before C
/// whose group is higher than G.
#[test]
fn a_cut_carries_a_publish_only_after_c_at_or_after_g_and_opening_as_itself() {
    let asked = request("t", 5, 40);
    assert!(asked.carried_by(&cut(6, 40, true)));
    assert!(asked.carried_by(&cut(9, 99, true)));
    // Counted at or before C, of a higher group: the abandoned history
    // after `zpool import -T`.
    assert!(!asked.carried_by(&cut(5, 51, true)));
    assert!(!asked.carried_by(&cut(4, 51, true)));
    // Of a group before G.
    assert!(!asked.carried_by(&cut(6, 39, true)));
    // Not opening as itself.
    assert!(!asked.carried_by(&cut(6, 40, false)));
    // Of another cut state.
    let mut other = cut(6, 40, true);
    other.cut.state = OTHER;
    assert!(!asked.carried_by(&other));
}

/// A request is refused at once for its name, its attachment, its cut
/// state or its manifest, and nothing is pending after it.
#[test]
fn a_request_is_refused_for_its_name_attachment_state_or_manifest() {
    let mut publishing = publishing();
    let long = "x".repeat(129);
    let cases = [
        ("", ATTACHMENT, STATE, 0, Refusal::Name),
        ("a/b", ATTACHMENT, STATE, 0, Refusal::Name),
        ("t t", ATTACHMENT, STATE, 0, Refusal::Name),
        (long.as_str(), ATTACHMENT, STATE, 0, Refusal::Name),
        ("t", OTHER, STATE, 0, Refusal::Attachment),
        ("t", ATTACHMENT, OTHER, 0, Refusal::State),
        (
            "t",
            ATTACHMENT,
            STATE,
            MANIFEST_MAX + 1,
            Refusal::Manifest(MANIFEST_MAX + 1),
        ),
    ];
    let refused = cases.len() as u64;
    for (name, attachment, state, len, why) in cases {
        let asked = Request {
            name: name.to_string(),
            attachment,
            state,
            after: 0,
            group: 0,
            manifest: vec![7; len],
            expiry: EXPIRY_DEFAULT,
        };
        assert_eq!(
            publishing.ask(asked),
            Answer::Refused(why.clone()),
            "{name:?} {why:?}"
        );
    }
    assert_eq!(publishing.pending().0, 0);
    assert_eq!(publishing.figures().refused, refused);
    // At the limits: accepted.
    let mut longest = request(&"x".repeat(128), 0, 0);
    longest.manifest = vec![7; MANIFEST_MAX];
    assert_eq!(publishing.ask(longest), Answer::Accepted);
    assert_eq!(publishing.ask(request("a-Z_0.9", 0, 0)), Answer::Accepted);

    // An uploader with no attachment refuses every request, and one whose
    // store's cut state is not known refuses every request of any state:
    // one it took could wait for ever for a cut of its state (F2 of
    // #122's review).
    let mut none = Publishing::new(None, Some(STATE));
    assert_eq!(
        none.ask(request("t", 0, 0)),
        Answer::Refused(Refusal::Attachment)
    );
    let mut unknown = Publishing::new(Some(ATTACHMENT), None);
    let mut other = request("t", 0, 0);
    other.state = OTHER;
    assert_eq!(unknown.ask(other), Answer::Refused(Refusal::State));
    assert_eq!(
        unknown.ask(request("t", 0, 0)),
        Answer::Refused(Refusal::State)
    );
    assert_eq!(unknown.pending().0, 0);
    // Known from the first check of a cut: taken from then on.
    unknown.set_state(STATE);
    assert_eq!(unknown.ask(request("t", 0, 0)), Answer::Accepted);
}

/// A store's cut state and its uploader's attachment do not change while
/// it runs; a setter that changes one all the same ends what pends of the
/// other, refused, and keeps what is of its own (F2 and N4 of #122's
/// review).
#[test]
fn a_state_or_attachment_set_anew_ends_what_pends_of_another() {
    let mut publishing = publishing();
    assert_eq!(publishing.ask(request("t", 0, 0)), Answer::Accepted);
    publishing.set_state(STATE);
    publishing.set_attachment(ATTACHMENT);
    assert_eq!(
        publishing.progress("t"),
        Progress::Pending,
        "the same: kept"
    );

    publishing.set_state(OTHER);
    assert_eq!(publishing.progress("t"), Progress::Refused(Refusal::State));
    let mut of_other = request("u", 0, 0);
    of_other.state = OTHER;
    assert_eq!(publishing.ask(of_other), Answer::Accepted);

    publishing.set_attachment(OTHER);
    assert_eq!(
        publishing.progress("u"),
        Progress::Refused(Refusal::Attachment)
    );
    assert_eq!(publishing.pending().0, 0);
    assert_eq!(publishing.figures().refused, 2);
    assert_eq!(publishing.next(), None);
}

/// A stopped uploader publishes nothing more (F4 of #122's review):
/// every publish pending ends refused, at whatever stage, and every
/// request after is refused.
#[test]
fn a_stopped_uploader_ends_what_pends_and_refuses_what_comes() {
    let mut publishing = publishing();
    publishing.committed(cut(3, 42, true));
    assert_eq!(publishing.ask(request("look", 1, 40)), Answer::Accepted);
    assert_eq!(publishing.ask(request("ready", 1, 40)), Answer::Accepted);
    publishing.absent("ready");
    assert_eq!(publishing.ask(request("wait", 5, 40)), Answer::Accepted);
    publishing.absent("wait");
    assert!(!publishing.stopped());
    publishing.stop();
    assert!(publishing.stopped());
    for name in ["look", "ready", "wait"] {
        assert_eq!(
            publishing.progress(name),
            Progress::Refused(Refusal::Stopped),
            "{name}"
        );
    }
    assert_eq!(publishing.next(), None);
    assert_eq!(
        publishing.ask(request("after", 1, 40)),
        Answer::Refused(Refusal::Stopped)
    );
    assert_eq!(
        publishing.ask(request("look", 1, 40)),
        Answer::Refused(Refusal::Stopped)
    );
    assert_eq!(publishing.pending().0, 0);
    assert_eq!(publishing.figures().refused, 5);
}

/// A publish accepted is looked up first; absent, it goes on the first
/// cut known that carries it, or waits, and the first cut committed
/// after that carries it takes it, not one that does not.
#[test]
fn a_publish_goes_on_the_first_cut_that_carries_it() {
    let mut publishing = publishing();
    publishing.committed(cut(1, 30, true));
    publishing.committed(cut(2, 41, false));
    publishing.committed(cut(3, 42, true));
    publishing.committed(cut(4, 43, true));
    assert_eq!(publishing.ask(request("known", 1, 40)), Answer::Accepted);
    assert_eq!(publishing.next(), Some(("known".into(), Stage::Look)));
    publishing.absent("known");
    assert_eq!(
        stage(&publishing, "known"),
        Some(Stage::Ready(cut(3, 42, true)))
    );

    assert_eq!(publishing.ask(request("later", 4, 40)), Answer::Accepted);
    publishing.absent("later");
    assert_eq!(stage(&publishing, "later"), Some(Stage::Wait));
    publishing.committed(cut(5, 39, true));
    publishing.committed(cut(6, 44, false));
    assert_eq!(stage(&publishing, "later"), Some(Stage::Wait));
    publishing.committed(cut(7, 45, true));
    assert_eq!(
        stage(&publishing, "later"),
        Some(Stage::Ready(cut(7, 45, true)))
    );
    // A cut committed after does not take it from the first.
    publishing.committed(cut(8, 46, true));
    assert_eq!(
        stage(&publishing, "later"),
        Some(Stage::Ready(cut(7, 45, true)))
    );
    // Not yet looked up: no cut is given to it.
    assert_eq!(publishing.ask(request("unlooked", 0, 0)), Answer::Accepted);
    publishing.committed(cut(9, 47, true));
    assert_eq!(stage(&publishing, "unlooked"), Some(Stage::Look));
}

/// The cuts in view are the newest [`KNOWN_CUTS`]: a publish whose first
/// cut has left the view goes on the oldest still in it that carries it.
#[test]
fn the_cuts_in_view_are_the_newest() {
    let mut publishing = publishing();
    for number in 1..=KNOWN_CUTS as u64 + 2 {
        publishing.committed(cut(number, number, true));
    }
    assert_eq!(publishing.ask(request("t", 0, 0)), Answer::Accepted);
    publishing.absent("t");
    assert_eq!(stage(&publishing, "t"), Some(Stage::Ready(cut(3, 3, true))));
}

/// The transaction for a publish that lands ends it, done; one that
/// fails is looked up before it is made again, on the same cut, and a
/// ref found then that records it ends it done.
#[test]
fn a_failed_transaction_is_looked_up_before_it_is_made_again() {
    let mut publishing = publishing();
    publishing.committed(cut(3, 42, true));
    publishing.ask(request("t", 1, 40));
    publishing.absent("t");
    publishing.failed("t");
    assert_eq!(
        publishing.next(),
        Some(("t".into(), Stage::Recheck(cut(3, 42, true))))
    );
    // Absent: made again, on the same cut, though a later one came.
    publishing.committed(cut(4, 43, true));
    publishing.absent("t");
    assert_eq!(
        stage(&publishing, "t"),
        Some(Stage::Ready(cut(3, 42, true)))
    );
    // It landed after all: found, and done.
    publishing.failed("t");
    let (_, row, _) = publishing
        .snapshot("t", &cut(3, 42, true))
        .expect("pending");
    let found = Found {
        commit: [3; 32],
        row: Some(row.encode()),
    };
    assert_eq!(publishing.found("t", &found), Ok(row));
    assert_eq!(publishing.progress("t"), Progress::Done([3; 32]));
    assert_eq!(publishing.next(), None);
    assert_eq!(publishing.figures().found, 1);
}

/// Asking twice is asking once: the same request while it is pending
/// is accepted again and stays one publish; another with the same name
/// is refused. Once it has ended, done or refused, the name asked again
/// is looked up anew and not answered from memory (F5 of #122's review):
/// `snap/t` may have been deleted, or taken, since.
#[test]
fn asking_twice_is_asking_once() {
    let mut publishing = publishing();
    let asked = request("t", 1, 40);
    assert_eq!(publishing.ask(asked.clone()), Answer::Accepted);
    assert_eq!(publishing.ask(asked.clone()), Answer::Accepted);
    assert_eq!(publishing.pending().0, 1);
    assert_eq!(
        publishing.ask(request("t", 2, 40)),
        Answer::Refused(Refusal::Asked)
    );
    assert_eq!(publishing.figures().accepted, 1);

    publishing.committed(cut(3, 42, true));
    publishing.absent("t");
    let (_, row, _) = publishing
        .snapshot("t", &cut(3, 42, true))
        .expect("pending");
    publishing.published("t", row, [3; 32]);
    assert_eq!(publishing.progress("t"), Progress::Done([3; 32]));
    assert_eq!(publishing.pending().0, 0);
    assert_eq!(publishing.figures().published, 1);
    assert_eq!(
        publishing.figures().last,
        Some(("t".to_string(), cut(3, 42, true).cut, [3; 32]))
    );
    // Asked again once done: looked up anew, and found done by the lookup.
    assert_eq!(publishing.ask(asked.clone()), Answer::Accepted);
    assert_eq!(publishing.progress("t"), Progress::Pending);
    assert_eq!(publishing.next(), Some(("t".into(), Stage::Look)));
    let ours = Found {
        commit: [3; 32],
        row: Some(row.encode()),
    };
    assert_eq!(publishing.found("t", &ours), Ok(row));
    assert_eq!(publishing.progress("t"), Progress::Done([3; 32]));
    // Another manifest for the name: looked up too, and refused there.
    let mut other_manifest = asked.clone();
    other_manifest.manifest = b"another".to_vec();
    assert_eq!(publishing.ask(other_manifest), Answer::Accepted);
    assert_eq!(publishing.found("t", &ours), Err(Refusal::Taken));
    assert_eq!(publishing.figures().accepted, 3);
    assert_eq!(publishing.figures().found, 1);

    // Refused once looked up (another's ref), then asked again: looked
    // up anew.
    publishing.ask(request("u", 1, 40));
    let theirs = Found {
        commit: [9; 32],
        row: None,
    };
    assert_eq!(publishing.found("u", &theirs), Err(Refusal::Taken));
    assert_eq!(publishing.progress("u"), Progress::Refused(Refusal::Taken));
    assert_eq!(publishing.ask(request("u", 1, 40)), Answer::Accepted);
    assert_eq!(publishing.next(), Some(("u".into(), Stage::Look)));
    assert_eq!(publishing.progress("never"), Progress::Unknown);
}

/// A ref found at the lookup is done only where its row records the
/// request: the ref's commit (a ref made anew on another commit beside a
/// row left behind is not ours, F6 of #122's review), a cut that carries
/// it, and its manifest, by its hash as well as its length (T3). Anything
/// else is another's: refused.
#[test]
fn a_ref_found_is_done_only_where_its_row_records_the_request() {
    let row = |number: u64, group: u64, commit: u8, manifest: &[u8]| SnapRow {
        cut: cut(number, group, true).cut,
        commit: [commit; 32],
        snap: SnapId(ulid::Ulid::from(7u128)),
        manifest_len: manifest.len() as u32,
        manifest_hash: *blake3::hash(manifest).as_bytes(),
    };
    let ours = b"the pool's manifest";
    let same_length = b"the pool's manifesT";
    assert_eq!(ours.len(), same_length.len());
    let cases: [(Option<Vec<u8>>, bool); 9] = [
        (Some(row(3, 42, 3, ours).encode()), true),
        (Some(row(3, 42, 4, ours).encode()), false),
        (Some(row(1, 42, 3, ours).encode()), false),
        (Some(row(3, 39, 3, ours).encode()), false),
        (Some(row(3, 42, 3, b"another").encode()), false),
        (Some(row(3, 42, 3, same_length).encode()), false),
        (
            Some(row(3, 42, 3, ours).encode()[..SNAP_ROW_LEN - 1].to_vec()),
            false,
        ),
        (Some(vec![2, 0]), false),
        (None, false),
    ];
    for (i, (bytes, done)) in cases.into_iter().enumerate() {
        let mut publishing = publishing();
        publishing.ask(request("t", 1, 40));
        let found = Found {
            commit: [3; 32],
            row: bytes,
        };
        let outcome = publishing.found("t", &found);
        assert_eq!(outcome.is_ok(), done, "case {i}");
        let progress = if done {
            Progress::Done([3; 32])
        } else {
            Progress::Refused(Refusal::Taken)
        };
        assert_eq!(publishing.progress("t"), progress, "case {i}");
    }
}

/// At most [`PENDING_MAX`] publishes pend; one more is refused, and
/// accepted once one has ended.
#[test]
fn the_publishes_pending_are_bounded() {
    let mut publishing = publishing();
    for i in 0..PENDING_MAX {
        assert_eq!(
            publishing.ask(request(&format!("t{i}"), 0, 0)),
            Answer::Accepted
        );
    }
    assert_eq!(
        publishing.ask(request("one-more", 0, 0)),
        Answer::Refused(Refusal::Busy)
    );
    let found = Found {
        commit: [1; 32],
        row: None,
    };
    assert!(publishing.found("t0", &found).is_err());
    assert_eq!(publishing.ask(request("one-more", 0, 0)), Answer::Accepted);
    assert_eq!(publishing.pending().0, PENDING_MAX);
    assert_eq!(publishing.pending().1.map(|(name, _)| name), Some("t1"));
}

/// The outcomes in view are the newest [`OUTCOMES_KEPT`].
#[test]
fn the_outcomes_in_view_are_the_newest() {
    let mut publishing = publishing();
    let found = Found {
        commit: [1; 32],
        row: None,
    };
    for i in 0..OUTCOMES_KEPT + 1 {
        let name = format!("t{i}");
        publishing.ask(request(&name, 0, 0));
        let _ = publishing.found(&name, &found);
    }
    assert_eq!(publishing.progress("t0"), Progress::Unknown);
    assert_eq!(publishing.progress("t1"), Progress::Refused(Refusal::Taken));
}

/// A snapshot's row reads back as written, laid out as documented.
#[test]
fn a_snapshot_row_reads_back_and_is_laid_out_as_documented() {
    let mut commit = [0; 32];
    for (i, byte) in commit.iter_mut().enumerate() {
        *byte = 0x80 + i as u8;
    }
    let row = SnapRow {
        cut: CommittedCut {
            state: STATE,
            number: 0x0102_0304_0506_0708,
            group: 0x1112_1314_1516_1718,
        },
        commit,
        snap: SnapId(ulid::Ulid::from(0x2122_2324_2526_2728_292A_2B2C_2D2E_2F30)),
        manifest_len: 0x3132_3334,
        manifest_hash: [0x40; 32],
    };
    let bytes = row.encode();
    assert_eq!(bytes.len(), SNAP_ROW_LEN);
    assert_eq!(SNAP_ROW_LEN, 118);
    assert_eq!(&bytes[0..2], &[1, 0]);
    assert_eq!(&bytes[2..18], &STATE);
    assert_eq!(&bytes[18..26], &[8, 7, 6, 5, 4, 3, 2, 1]);
    assert_eq!(
        &bytes[26..34],
        &[0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11]
    );
    assert_eq!(&bytes[34..66], &commit);
    assert_eq!(bytes[66], 0x21);
    assert_eq!(bytes[81], 0x30);
    assert_eq!(&bytes[82..86], &[0x34, 0x33, 0x32, 0x31]);
    assert_eq!(&bytes[86..118], &[0x40; 32]);
    assert_eq!(SnapRow::decode(&bytes), Ok(row));
}

/// A row of another version is refused for its version, whatever its
/// length; one of version 1 with another length for its length.
#[test]
fn a_snapshot_row_of_another_version_or_length_is_unreadable() {
    let row = SnapRow {
        cut: cut(1, 2, true).cut,
        commit: [6; 32],
        snap: SnapId(ulid::Ulid::from(3u128)),
        manifest_len: 4,
        manifest_hash: [5; 32],
    };
    let mut other = row.encode();
    other[0] = 2;
    assert_eq!(SnapRow::decode(&other), Err(SnapRowError::Version(2)));
    assert_eq!(SnapRow::decode(&[2, 0]), Err(SnapRowError::Version(2)));
    let mut long = row.encode();
    long.push(0);
    assert_eq!(
        SnapRow::decode(&long),
        Err(SnapRowError::Length(SNAP_ROW_LEN + 1))
    );
    assert_eq!(
        SnapRow::decode(&row.encode()[..SNAP_ROW_LEN - 1]),
        Err(SnapRowError::Length(SNAP_ROW_LEN - 1))
    );
    assert_eq!(SnapRow::decode(&[1]), Err(SnapRowError::Length(1)));
    assert_eq!(SnapRow::decode(&[]), Err(SnapRowError::Length(0)));
}

/// The keys: the ref as SkyNBD names a snapshot's, the row among the
/// meta rows, and the ZFS manifest beside the volume's objects.
#[test]
fn the_keys_are_named_as_documented() {
    let vol = VolumeId(ulid::Ulid::from(1u128));
    let snap = SnapId(ulid::Ulid::from(2u128));
    assert_eq!(snap_ref("t"), "snap/t");
    assert_eq!(snap_row_key("t"), "skyzfs/snap/t");
    assert_eq!(
        manifest_key(vol, snap),
        format!(
            "{}skyzfs/manifest/{snap}",
            skynbd_engine::keys::volume_prefix(vol)
        )
    );
}

/// An expiry of zero, or over an hour, is refused; an hour is not.
#[test]
fn a_request_is_refused_for_its_expiry() {
    let mut publishing = publishing();
    for expiry in [Duration::ZERO, EXPIRY_MAX + Duration::from_nanos(1)] {
        let mut asked = request("t", 0, 0);
        asked.expiry = expiry;
        assert_eq!(
            publishing.ask(asked),
            Answer::Refused(Refusal::Expiry(expiry))
        );
    }
    assert_eq!(publishing.pending().0, 0);
    let mut asked = request("t", 0, 0);
    asked.expiry = EXPIRY_MAX;
    assert_eq!(publishing.ask(asked), Answer::Accepted);
    assert_eq!(EXPIRY_DEFAULT, Duration::from_secs(600));
    assert_eq!(EXPIRY_MAX, Duration::from_secs(3600));
}

/// A publish whose time is up (the lead's decision on Q1 of #122's
/// review) is dropped and counted, at any stage: to be looked up,
/// waiting for a cut, or ready on one. One whose transaction failed is
/// looked up first: absent, it ends expired and is not made again; found
/// recording it, it ends done. Before its time nothing ends, and a name
/// that expired is looked up anew when asked again.
#[test]
fn a_publish_whose_time_is_up_is_dropped_unless_its_transaction_may_have_landed() {
    let mut publishing = publishing();
    let asked_at = Instant::now();
    for name in ["look", "wait", "ready", "absent", "landed"] {
        // `wait` asks for a group that cut 3 does not reach.
        let group = if name == "wait" { 50 } else { 40 };
        let mut asked = request(name, 1, group);
        asked.expiry = Duration::from_secs(60);
        assert_eq!(publishing.ask(asked), Answer::Accepted);
    }
    publishing.absent("wait");
    for name in ["ready", "absent", "landed"] {
        publishing.absent(name);
    }
    // `committed` sweeps on the real clock: well before the deadline.
    publishing.committed(cut(3, 42, true));
    for name in ["absent", "landed"] {
        publishing.failed(name);
    }
    assert_eq!(stage(&publishing, "look"), Some(Stage::Look));
    assert_eq!(stage(&publishing, "wait"), Some(Stage::Wait));
    assert_eq!(
        stage(&publishing, "ready"),
        Some(Stage::Ready(cut(3, 42, true)))
    );

    publishing.expire(asked_at + Duration::from_secs(59));
    assert_eq!(publishing.pending().0, 5, "nothing ends before its time");

    publishing.expire(asked_at + Duration::from_secs(61));
    for name in ["look", "wait", "ready"] {
        assert_eq!(publishing.progress(name), Progress::Expired, "{name}");
    }
    assert_eq!(publishing.figures().expired, 3);
    for name in ["absent", "landed"] {
        assert_eq!(publishing.progress(name), Progress::Pending, "{name}");
    }
    assert!(matches!(publishing.next(), Some((_, Stage::Recheck(_)))));

    // Absent once looked up: expired, and not made again.
    publishing.absent("absent");
    assert_eq!(publishing.progress("absent"), Progress::Expired);
    assert_eq!(publishing.figures().expired, 4);
    // Landed after all: done.
    let (_, row, _) = publishing
        .snapshot("landed", &cut(3, 42, true))
        .expect("pending");
    let found = Found {
        commit: [3; 32],
        row: Some(row.encode()),
    };
    assert_eq!(publishing.found("landed", &found), Ok(row));
    assert_eq!(publishing.progress("landed"), Progress::Done([3; 32]));
    assert_eq!(publishing.figures().expired, 4);
    assert_eq!(publishing.next(), None);

    // Asked again: looked up anew.
    assert_eq!(publishing.ask(request("wait", 1, 40)), Answer::Accepted);
    assert_eq!(stage(&publishing, "wait"), Some(Stage::Look));
}

/// A publish withdrawn ends at once, at any stage but after a failed
/// transaction: then it is looked up first, and ends withdrawn if absent
/// and done if it landed. A name not pending is left as it stands.
#[test]
fn a_withdrawn_publish_ends_at_once_or_once_its_failed_transaction_is_looked_up() {
    let mut publishing = publishing();
    for name in ["look", "absent", "landed"] {
        assert_eq!(publishing.ask(request(name, 1, 40)), Answer::Accepted);
    }
    publishing.committed(cut(3, 42, true));
    for name in ["absent", "landed"] {
        publishing.absent(name);
        publishing.failed(name);
    }
    assert_eq!(publishing.withdraw("look"), Progress::Withdrawn);
    assert_eq!(publishing.withdraw("absent"), Progress::Pending);
    assert_eq!(publishing.withdraw("landed"), Progress::Pending);
    assert_eq!(publishing.figures().withdrawn, 1);
    publishing.absent("absent");
    assert_eq!(publishing.progress("absent"), Progress::Withdrawn);
    let (_, row, _) = publishing
        .snapshot("landed", &cut(3, 42, true))
        .expect("pending");
    let found = Found {
        commit: [3; 32],
        row: Some(row.encode()),
    };
    assert_eq!(publishing.found("landed", &found), Ok(row));
    assert_eq!(publishing.progress("landed"), Progress::Done([3; 32]));
    assert_eq!(publishing.figures().withdrawn, 2);
    assert_eq!(publishing.withdraw("landed"), Progress::Done([3; 32]));
    assert_eq!(publishing.withdraw("never"), Progress::Unknown);
    assert_eq!(publishing.figures().withdrawn, 2);
    assert_eq!(publishing.pending().0, 0);
}

/// D1 of the review of #122 (its q9): a publish withdrawn while its
/// failed transaction waits to be looked up, then asked again with the
/// same request, goes on: `Accepted` comes true, and the lookup that
/// finds the name absent makes it again on its cut. A publish whose time
/// is up is not brought back so: asked again while it pends, a publish
/// keeps its deadline, and it ends expired.
#[test]
fn a_withdrawn_publish_asked_again_with_the_same_request_goes_on() {
    let mut publishing = publishing();
    let asked_at = Instant::now();
    let mut expiring = request("expiring", 1, 40);
    expiring.expiry = Duration::from_secs(60);
    assert_eq!(publishing.ask(request("t", 1, 40)), Answer::Accepted);
    assert_eq!(publishing.ask(expiring.clone()), Answer::Accepted);
    publishing.committed(cut(3, 42, true));
    for name in ["t", "expiring"] {
        publishing.absent(name);
        publishing.failed(name);
    }
    assert_eq!(publishing.withdraw("t"), Progress::Pending);
    assert_eq!(publishing.ask(request("t", 1, 40)), Answer::Accepted);
    publishing.absent("t");
    assert_eq!(publishing.progress("t"), Progress::Pending);
    assert_eq!(
        stage(&publishing, "t"),
        Some(Stage::Ready(cut(3, 42, true)))
    );
    assert_eq!(publishing.figures().withdrawn, 0);
    // Another request of the name is still refused.
    assert_eq!(
        publishing.ask(request("t", 2, 40)),
        Answer::Refused(Refusal::Asked)
    );

    publishing.expire(asked_at + Duration::from_secs(61));
    assert_eq!(publishing.progress("expiring"), Progress::Pending);
    assert_eq!(publishing.ask(expiring), Answer::Accepted);
    publishing.absent("expiring");
    assert_eq!(publishing.progress("expiring"), Progress::Expired);
    assert_eq!(publishing.figures().expired, 1);
}

/// D2 of the review of #122: each publish has its own wait after a step
/// that failed, and while it waits the others step, oldest first. The
/// wait doubles at each failure of the same publish, a lookup that
/// succeeds does not reset it, and a publish asked after another has
/// failed starts from a second.
#[tokio::test(start_paused = true)]
async fn each_publish_waits_alone_after_its_own_failures() {
    let mut publishing = publishing();
    for name in ["a", "b"] {
        assert_eq!(publishing.ask(request(name, 1, 40)), Answer::Accepted);
    }
    let now = Instant::now();
    assert_eq!(publishing.due_at(), Some(now));
    assert_eq!(publishing.next(), Some(("a".into(), Stage::Look)));

    assert_eq!(
        publishing.wait_after("a", now),
        Some(Duration::from_secs(1))
    );
    assert_eq!(publishing.next(), Some(("b".into(), Stage::Look)));
    assert_eq!(
        publishing.wait_after("b", now),
        Some(Duration::from_secs(1))
    );
    assert_eq!(publishing.next(), None);
    assert_eq!(publishing.due_at(), Some(now + Duration::from_secs(1)));

    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(publishing.next(), Some(("a".into(), Stage::Look)));
    // Looked up, absent, and carried by cut 3: the lookup leaves the wait
    // as it was, and the next failure doubles it.
    publishing.committed(cut(3, 42, true));
    publishing.absent("a");
    let now = Instant::now();
    assert_eq!(
        publishing.wait_after("a", now),
        Some(Duration::from_secs(2))
    );
    assert_eq!(
        publishing.wait_after("a", now),
        Some(Duration::from_secs(4))
    );
    assert_eq!(publishing.next(), Some(("b".into(), Stage::Look)));

    // A publish that waits for a cut has no step, and so no due time.
    publishing.absent("b");
    let mut waiting = request("waiting", 1, 50);
    waiting.expiry = Duration::from_secs(60);
    assert_eq!(publishing.ask(waiting), Answer::Accepted);
    publishing.absent("waiting");
    assert_eq!(stage(&publishing, "waiting"), Some(Stage::Wait));
    assert_eq!(
        publishing.next(),
        Some(("b".into(), Stage::Ready(cut(3, 42, true))))
    );

    // A name that ends takes its wait with it: asked again, it starts
    // from a second.
    assert_eq!(publishing.withdraw("a"), Progress::Withdrawn);
    assert_eq!(publishing.ask(request("a", 1, 40)), Answer::Accepted);
    assert_eq!(
        publishing.wait_after("a", now),
        Some(Duration::from_secs(1))
    );
    assert_eq!(publishing.wait_after("never", now), None);
}
