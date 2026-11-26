//! The record's writes under power cuts and errors, on the model of a
//! file system ([`crate::fs::model`]; D2 and T2 of #142's review).
//!
//! A run is the store's life through two publishes: open, a first save,
//! each manifest written before a record names it, saves as the
//! publishes move, and each manifest removed once no record names it.
//! The power is cut after every call of the file system, with what was
//! not synced lost, kept or torn ([`Cut::ALL`]), and in another matrix
//! each call is made to fail in turn, and the step asked again. After
//! each cut:
//!
//! - the record loads as it was before the step or after it, never
//!   corrupt, and never absent once a save has returned;
//! - every manifest that the record loaded names (a publish at `Taken`
//!   or later) is there, whole;
//! - a manifest removed by a step that returned is not there.

use std::path::{Path, PathBuf};

use super::tests::{publish, taken};
use super::*;
use crate::fs::model::{Cut, Model};

/// The store's directory, in a base that is there.
const DIR: &str = "/var/lib/skyzfs-agent/vol";

const T1: &str = "skz-20261125T010000Z";
const T2: &str = "skz-20261125T020000Z";

/// A step of the run.
#[derive(Debug, Clone)]
enum Step {
    Save(Record),
    Write(&'static str),
    Remove(&'static str),
}

/// The manifest of `name`: of a length that a torn write cuts.
fn manifest(name: &str) -> Vec<u8> {
    format!("{{\"name\":\"{name}\",\"pad\":\"{}\"}}", "m".repeat(200)).into_bytes()
}

/// A record with `taken` (names at `Taken`) under way.
fn record(taken_now: &[&str]) -> Record {
    let mut record = Record::new([0x0f; 16]);
    record.enrolled = Some(Enrolled {
        pool: "tank".into(),
        pool_guid: 1,
        leaf_guid: 2,
    });
    for (i, name) in taken_now.iter().enumerate() {
        let group = 40 + u64::try_from(i).unwrap_or(0);
        record
            .under_way
            .push(publish(name, Phase::Taken(taken(group)), false));
    }
    record
}

/// The run's steps.
fn steps() -> Vec<Step> {
    vec![
        Step::Save(record(&[])),
        Step::Write(T1),
        Step::Save(record(&[T1])),
        Step::Write(T2),
        Step::Save(record(&[T1, T2])),
        Step::Save(record(&[T2])),
        Step::Remove(T1),
        Step::Save(record(&[])),
        Step::Remove(T2),
    ]
}

fn open(fs: &Model) -> Store<Model> {
    Store::open_on(fs.clone(), DIR).expect("opened")
}

fn take(store: &Store<Model>, step: &Step) -> Result<(), String> {
    match step {
        Step::Save(record) => store.save(record).map_err(|e| e.to_string()),
        Step::Write(name) => store
            .write_manifest(name, &manifest(name))
            .map_err(|e| e.to_string()),
        Step::Remove(name) => store.remove_manifest(name).map_err(|e| e.to_string()),
    }
}

/// What the run should leave after each step: the record (or none), and
/// the manifests removed.
#[derive(Debug, Clone, Default)]
struct After {
    record: Option<Record>,
    removed: Vec<&'static str>,
}

fn afters(steps: &[Step]) -> Vec<After> {
    let mut out = vec![After::default()];
    for step in steps {
        let mut next = out.last().cloned().unwrap_or_default();
        match step {
            Step::Save(record) => next.record = Some(record.clone()),
            Step::Write(name) => next.removed.retain(|n| n != name),
            Step::Remove(name) => next.removed.push(name),
        }
        out.push(next);
    }
    out
}

/// What a model cut during a step (between `before` and `after`) holds,
/// checked; `Err` names what is wrong.
fn check(cut: &Model, before: &After, after: &After) -> Result<(), String> {
    let store = Store::open_on(cut.clone(), DIR).map_err(|e| format!("open: {e}"))?;
    let loaded = store.load().map_err(|e| format!("load: {e}"))?;
    let record = match loaded {
        Loaded::Corrupt(why) => return Err(format!("the record is corrupt: {why}")),
        Loaded::Absent => None,
        Loaded::Present(record) => Some(record),
    };
    // The marker is written before the first record (L4 of #149's
    // review): a record with none would be refused its sweep.
    if record.is_some() && !store.marked().map_err(|e| format!("marked: {e}"))? {
        return Err("a record with no marker beside it".into());
    }
    if record != before.record && record != after.record {
        return Err(format!(
            "the record is neither before nor after the step: {record:?}"
        ));
    }
    for publish in record.iter().flat_map(|r| &r.under_way) {
        if publish.phase.taken().is_some() {
            let read = store
                .read_manifest(&publish.name)
                .map_err(|e| format!("read: {e}"))?;
            if read.as_deref() != Some(manifest(&publish.name).as_slice()) {
                return Err(format!(
                    "the manifest of {} is not whole: {:?}",
                    publish.name,
                    read.map(|b| b.len())
                ));
            }
        }
    }
    for name in &before.removed {
        if store
            .read_manifest(name)
            .map_err(|e| format!("read: {e}"))?
            .is_some()
        {
            return Err(format!("the manifest of {name} came back"));
        }
    }
    Ok(())
}

/// Run the steps on `fs` with every call's cut checked; a step that
/// fails is asked again (once: the model fails one call). The failures,
/// as text.
fn run(fs: &Model) -> Vec<String> {
    fs.base(Path::new(DIR).parent().unwrap_or(Path::new("/")));
    let steps = steps();
    let afters = afters(&steps);
    let mut wrong = Vec::new();
    fs.keep_each();
    // An open that fails is asked again, as a step is.
    let opened = Store::open_on(fs.clone(), DIR).or_else(|_| Store::open_on(fs.clone(), DIR));
    let store = match opened {
        Ok(store) => store,
        Err(e) => return vec![format!("open, asked again: {e}")],
    };
    wrong.extend(check_cuts(fs, &afters[0], &afters[0], "open"));
    for (i, step) in steps.iter().enumerate() {
        fs.keep_each();
        let first = take(&store, step);
        let mut at = format!("step {i} {step:?}");
        if let Err(e) = &first {
            at = format!("{at} (failed: {e}, then asked again)");
            if let Err(again) = take(&store, step) {
                wrong.push(format!("{at}: asked again, failed: {again}"));
            }
        }
        wrong.extend(check_cuts(fs, &afters[i], &afters[i + 1], &at));
    }
    let last = afters.last().cloned().unwrap_or_default();
    for cut in Cut::ALL {
        if let Err(why) = check(&fs.cut(cut), &last, &last) {
            wrong.push(format!("at the end ({cut:?}): {why}"));
        }
    }
    drop(store);
    wrong
}

/// The cuts after each call since the model's last `keep_each`, each
/// checked as made during a step from `before` to `after`.
fn check_cuts(fs: &Model, before: &After, after: &After, at: &str) -> Vec<String> {
    let mut wrong = Vec::new();
    for (call, cut, model) in fs.cuts() {
        if let Err(why) = check(&model, before, after) {
            wrong.push(format!("{at}, cut after `{call}` ({cut:?}): {why}"));
        }
    }
    wrong
}

fn shown(wrong: &[String]) -> String {
    let mut lines: Vec<_> = wrong.iter().take(12).cloned().collect();
    if wrong.len() > lines.len() {
        lines.push(format!("... and {} more", wrong.len() - lines.len()));
    }
    lines.join("\n")
}

/// Mutant (R4 to R8 of #142's review): the file not synced before its
/// rename, the directory not synced after it, the record written in
/// place, a new directory's parent not synced, a removed manifest's
/// directory not synced.
#[test]
fn a_power_cut_after_any_call_leaves_the_record_before_or_after_each_step() {
    let fs = Model::default();
    let wrong = run(&fs);
    assert!(wrong.is_empty(), "{}", shown(&wrong));
    assert!(fs.calls() > 30, "{:#?}", fs.log());
}

/// Mutant: a step that failed after a removal or a directory's making,
/// asked again, finds the entry gone or there and does not sync its
/// directory; or a sync that failed taken as done.
#[test]
fn a_call_that_fails_leaves_the_record_before_or_after_and_the_step_can_be_asked_again() {
    let calls = {
        let fs = Model::default();
        let _ = run(&fs);
        fs.calls()
    };
    let mut wrong = Vec::new();
    for n in 0..calls {
        let fs = Model::default();
        fs.fail_at(Some(n));
        for why in run(&fs) {
            wrong.push(format!("call {n} failed: {why}"));
        }
    }
    assert!(wrong.is_empty(), "{}", shown(&wrong));
}

/// The model itself: what a cut keeps of a file renamed before its sync,
/// and of a sync that failed (the tests above rest on it).
#[test]
fn the_model_loses_what_was_not_synced() {
    let fs = Model::default();
    fs.base(Path::new("/d"));
    let (a, b) = (PathBuf::from("/d/a"), PathBuf::from("/d/b"));
    fs.write(&a, b"one").expect("written");
    fs.rename(&a, &b).expect("renamed");
    fs.sync_dir(Path::new("/d")).expect("synced");
    let lost = fs.cut(Cut::ALL[0]);
    assert_eq!(lost.read(&b).expect("there"), b"");
    // A sync that failed marks the bytes clean: the next does not keep
    // them, until they are written again.
    fs.fail_at(Some(fs.calls()));
    assert!(fs.sync_file(&b).is_err());
    fs.sync_file(&b).expect("synced");
    assert_eq!(fs.cut(Cut::ALL[0]).read(&b).expect("there"), b"");
    fs.write(&b, b"two").expect("written");
    fs.sync_file(&b).expect("synced");
    assert_eq!(fs.cut(Cut::ALL[0]).read(&b).expect("there"), b"two");
    // An entry not synced is lost with the entries, kept with them.
    fs.write(&a, b"three").expect("written");
    fs.sync_file(&a).expect("synced");
    assert!(fs.cut(Cut::ALL[0]).read(&a).is_err());
    assert_eq!(fs.cut(Cut::ALL[1]).read(&a).expect("there"), b"three");
}

/// S6 of #142's review: one agent keeps a directory. Mutant: no lock, or
/// one not released when the store is dropped.
#[test]
fn a_second_agent_is_refused_the_directory_while_the_first_has_it() {
    let fs = Model::default();
    fs.base(Path::new("/var/lib/skyzfs-agent"));
    let first = open(&fs);
    match Store::open_on(fs.clone(), DIR) {
        Err(OpenError::Held(dir)) => assert_eq!(dir, Path::new(DIR)),
        other => panic!("{other:?}"),
    }
    drop(first);
    let _again = open(&fs);

    let root = tempfile::tempdir().expect("a directory");
    let dir = root.path().join("vol");
    let first = Store::open(&dir).expect("opened");
    let e = Store::open(&dir).expect_err("refused");
    assert!(matches!(&e, OpenError::Held(d) if *d == dir), "{e}");
    assert!(
        e.to_string().contains("another agent holds the record"),
        "{e}"
    );
    drop(first);
    Store::open(&dir).expect("opened again");
}
