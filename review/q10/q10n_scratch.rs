//! Review scratch (Q10, PR #148, next look), not product code. The
//! adapter of #147 at 0f709ae (`ZfsCli` on the fake host) fed the
//! records of the recorder's runs after the review at 73dce38, as data.
//! Each record is given to the fake under the words in its own `argv`
//! file (NUL-separated), and the adapter is called with its own
//! constants (`HOLD_TAG`, `ASSIST_PROPERTY`): where the adapter asks for
//! other words than the recorder ran, the fake answers "no output given
//! for ...", which the reading shows. Prints what each reading gives.
//!
//! Run from a scratch checkout of 0f709ae with this file at
//! crates/skyzfs-agent/src/zfs/cli/q10n_scratch.rs, `#[cfg(test)] mod
//! q10n_scratch;` added to cli.rs, and
//! `Q10_REC=<dir holding zfs-cli/> cargo test -p skyzfs-agent --lib
//! q10n_scratch -- --nocapture`.

use std::path::{Path, PathBuf};

use super::*;
use crate::host::fake::Fake;
use crate::host::{Node, Output};
use crate::zfs::HOLD_TAG;

/// A record: the words of its argv, and what the command gave.
fn rec(dir: &Path, name: &str) -> Option<(String, Output)> {
    let d = dir.join("zfs-cli").join(name);
    if !d.exists() {
        return None;
    }
    let read = |f: &str| std::fs::read(d.join(f)).unwrap_or_default();
    let argv = read("argv");
    let words: Vec<String> = argv
        .split(|b| *b == 0)
        .filter(|w| !w.is_empty())
        .map(|w| String::from_utf8_lossy(w).into_owned())
        .collect();
    let rc = String::from_utf8_lossy(&read("rc"))
        .trim()
        .parse::<i32>()
        .expect("rc");
    Some((
        words.join(" "),
        Output {
            code: Some(rc),
            stdout: read("stdout"),
            stderr: read("stderr"),
        },
    ))
}

/// The adapter on a fake host that gives `names`' records, each under
/// its own words; `None` where a record is missing.
fn cli(dir: &Path, names: &[&str]) -> Option<ZfsCli<Fake>> {
    let mut fake = Fake::default();
    for name in names {
        let (words, output) = rec(dir, name)?;
        fake.give(&words, Ok(output));
    }
    for p in ["/dev/ublkb0", "/dev/ublkb1"] {
        fake.nodes.insert(PathBuf::from(p), Node::Block(0x1000));
    }
    for n in 0..8 {
        fake.nodes
            .insert(PathBuf::from(format!("/dev/loop{n}")), Node::Block(0x700 + n));
    }
    for f in ["m0", "m1", "m2", "spare", "s0", "s1", "d0", "d1", "r0", "r1"] {
        fake.nodes.insert(
            PathBuf::from(format!("/mnt/skz-cli-record/{f}.img")),
            Node::File,
        );
    }
    if let Some((_, nocache)) = rec(dir, "nocacheflush") {
        fake.files.insert(PathBuf::from(NOCACHEFLUSH), nocache.stdout);
    }
    Some(ZfsCli::new(fake, Limits::default()))
}

/// What a reading gave, with the words the adapter ran.
fn show<T: std::fmt::Debug>(what: &str, z: Option<ZfsCli<Fake>>, f: impl FnOnce(&mut ZfsCli<Fake>) -> T) {
    match z {
        None => println!("{what}: (not recorded)"),
        Some(mut z) => {
            let got = f(&mut z);
            let ran = z.host().words().join(" | ");
            println!("{what}: {got:?}\n    ran: {ran}");
        }
    }
}

#[test]
fn q10n_readings_on_the_records() {
    let Ok(root) = std::env::var("Q10_REC") else {
        return;
    };
    let dir = Path::new(&root);
    let tag = HOLD_TAG;

    for (x, pool) in [("rig", "skzq"), ("mirror", "skzqm")] {
        for a in [
            "status",
            "scrub-status",
            "scrub-paused",
            "scrub-canceled",
            "resilver-status",
            "resilvered",
            "rebuild-status",
            "rebuilt",
        ] {
            let (a, b) = (format!("{x}-{a}"), format!("{x}-{a}-g"));
            show(&format!("layout {a} + {b}"), cli(dir, &[&a, &b]), |z| z.layout(pool));
        }
        for g in ["get", "get-checkpoint"] {
            let n = format!("{x}-{g}");
            show(&format!("props {n}"), cli(dir, &[&n]), |z| z.props(pool));
        }
        let (s, y) = (format!("{x}-assist-set"), format!("{x}-assist-sync"));
        show(&format!("assist {s} + {y}"), cli(dir, &[&s, &y]), |z| {
            z.assist(pool, 1_700_000_000_001)
        });
        for (n, t) in [
            ("snapshot-1", "t"),
            ("snapshot-2", "t"),
            ("snapshot-odd", "o0"),
            ("snapshot-exists-child", "p"),
        ] {
            let n = format!("{x}-{n}");
            show(&format!("snapshot {n} at @{t}"), cli(dir, &[&n]), |z| z.snapshot(pool, t));
        }
        let n = format!("{x}-members");
        show(&format!("members {n}"), cli(dir, &[&n]), |z| z.members(pool, "t"));
        let n = format!("{x}-members-absent");
        show(&format!("members {n}"), cli(dir, &[&n]), |z| z.members(pool, "absent"));
        for t in ["tagged", "tagged-held"] {
            let n = format!("{x}-{t}");
            show(&format!("tagged {n} (HOLD_TAG)"), cli(dir, &[&n]), |z| z.tagged(pool, tag));
        }
        let n = format!("{x}-holds");
        show(&format!("held {n} (HOLD_TAG)"), cli(dir, &[&n]), |z| z.held(pool, "t", tag));
        let n = format!("{x}-holds-absent");
        show(&format!("held {n} at @absent"), cli(dir, &[&n]), |z| z.held(pool, "absent", tag));
        let n = format!("{x}-hold-1");
        show(&format!("hold {n}"), cli(dir, &[&n]), |z| z.hold(pool, "t", tag));
        let (h, m, r) = (format!("{x}-hold-2"), format!("{x}-members"), format!("{x}-holds-r"));
        show(&format!("hold {h} + {m} + {r} (all held)"), cli(dir, &[&h, &m, &r]), |z| {
            z.hold(pool, "t", tag)
        });
        let (h, m, r) = (
            format!("{x}-hold-r-partial"),
            format!("{x}-members"),
            format!("{x}-holds-r-partial"),
        );
        show(&format!("hold {h} + {m} + {r} (root only)"), cli(dir, &[&h, &m, &r]), |z| {
            z.hold(pool, "t", tag)
        });
        let n = format!("{x}-hold-absent");
        show(&format!("hold {n} at @absent"), cli(dir, &[&n]), |z| z.hold(pool, "absent", tag));
        let n = format!("{x}-release-1");
        show(&format!("release {n}"), cli(dir, &[&n]), |z| z.release(pool, "t", tag));
        let (l, r) = (format!("{x}-release-2"), format!("{x}-holds-r-released"));
        show(&format!("release {l} + {r}"), cli(dir, &[&l, &r]), |z| z.release(pool, "t", tag));
        let n = format!("{x}-release-r-partial");
        show(&format!("release {n} (child only had it)"), cli(dir, &[&n]), |z| {
            z.release(pool, "t", tag)
        });
        let n = format!("{x}-release-absent");
        show(&format!("release {n} at @absent"), cli(dir, &[&n]), |z| {
            z.release(pool, "absent", tag)
        });
    }

    for a in ["remove-status", "remove-in-progress", "remove-done"] {
        let b = format!("{a}-g");
        show(&format!("layout {a} + {b}"), cli(dir, &[a, &b]), |z| z.layout("skzqr"));
    }

    let pool = "skznone";
    show("layout none", cli(dir, &["none-status", "none-status-g"]), |z| z.layout(pool));
    show("props none (+ none-status)", cli(dir, &["none-get", "none-status"]), |z| z.props(pool));
    show("assist none", cli(dir, &["none-assist-set", "none-assist-sync"]), |z| {
        z.assist(pool, 1_700_000_000_001)
    });
    show("snapshot none", cli(dir, &["none-snapshot-1"]), |z| z.snapshot(pool, "t"));
    show("members none", cli(dir, &["none-members"]), |z| z.members(pool, "t"));
    show("tagged none", cli(dir, &["none-tagged"]), |z| z.tagged(pool, tag));
    show("held none", cli(dir, &["none-holds"]), |z| z.held(pool, "t", tag));
    show("hold none", cli(dir, &["none-hold"]), |z| z.hold(pool, "t", tag));
    show("release none", cli(dir, &["none-release"]), |z| z.release(pool, "t", tag));
    show("nocacheflush", cli(dir, &[]), |z| z.nocacheflush());
}
