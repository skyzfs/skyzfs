//! Review scratch (Q10, PR #148), not product code. The adapter of #147
//! at 366b217 (`ZfsCli` on the fake host) fed the records of the first
//! recording run, as data: each call gets the record's exit code, stdout
//! and stderr under the words the adapter runs. Prints what each reading
//! gives. Run from a scratch checkout of 366b217 with this file at
//! crates/skyzfs-agent/src/zfs/cli/q10_scratch.rs, `#[cfg(test)] mod
//! q10_scratch;` added to cli.rs, and
//! `Q10_REC=<dir holding zfs-cli/> cargo test -p skyzfs-agent --lib
//! q10_scratch -- --nocapture`.

use std::path::{Path, PathBuf};

use super::*;
use crate::host::fake::Fake;
use crate::host::{Node, Output};
use crate::zfs::HOLD_TAG;

fn rec(dir: &Path, name: &str) -> Output {
    let d = dir.join("zfs-cli").join(name);
    let read = |f: &str| std::fs::read(d.join(f)).unwrap_or_default();
    let rc = String::from_utf8_lossy(&read("rc"))
        .trim()
        .parse::<i32>()
        .expect("rc");
    Output {
        code: Some(rc),
        stdout: read("stdout"),
        stderr: read("stderr"),
    }
}

fn cli(dir: &Path, given: &[(&str, &str)]) -> ZfsCli<Fake> {
    let mut fake = Fake::default();
    for (words, name) in given {
        fake.give(words, Ok(rec(dir, name)));
    }
    // Every leaf path the records name: block devices and the mirror's files.
    for p in ["/dev/ublkb0", "/dev/ublkb1"] {
        fake.nodes.insert(PathBuf::from(p), Node::Block(0x1000));
    }
    for n in 0..8 {
        fake.nodes
            .insert(PathBuf::from(format!("/dev/loop{n}")), Node::Block(0x700 + n));
    }
    for p in ["/mnt/skz-cli-record/m0.img", "/mnt/skz-cli-record/m1.img"] {
        fake.nodes.insert(PathBuf::from(p), Node::File);
    }
    ZfsCli::new(fake, Limits::default())
}

fn status(dir: &Path, pool: &str, named: &str, guided: &str) -> String {
    let mut z = cli(
        dir,
        &[
            (&format!("zpool status -P {pool}"), named),
            (&format!("zpool status -P -g {pool}"), guided),
        ],
    );
    format!("{:?}", z.layout(pool))
}

#[test]
fn q10_readings_on_the_records() {
    let Ok(root) = std::env::var("Q10_REC") else {
        return;
    };
    let dir = Path::new(&root);
    let p = |what: &str, v: String| println!("{what}: {v}");

    for (x, pool) in [("rig", "skzq"), ("mirror", "skzqm")] {
        for (a, b) in [
            ("status", "status-g"),
            ("scrub-status", "scrub-status-g"),
            ("resilver-status", "resilver-status-g"),
        ] {
            let (a, b) = (format!("{x}-{a}"), format!("{x}-{b}"));
            if dir.join("zfs-cli").join(&a).exists() {
                p(&format!("layout {a}/{b}"), status(dir, pool, &a, &b));
            }
        }
        // The paused and canceled scrubs, and the end of the resilver, are
        // recorded with -P only: their -g is the idle one's shape.
        for a in ["scrub-paused", "scrub-canceled"] {
            let a = format!("{x}-{a}");
            p(&format!("layout {a}/{x}-status-g"), status(dir, pool, &a, &format!("{x}-status-g")));
        }
        for g in ["get", "get-checkpoint"] {
            let mut z = cli(
                dir,
                &[(
                    &format!(
                        "zpool get -H -p -o property,value guid,load_guid,multihost,readonly,checkpoint {pool}"
                    ),
                    &format!("{x}-{g}"),
                )],
            );
            p(&format!("props {x}-{g}"), format!("{:?}", z.props(pool)));
        }
        let prog = |read_only: bool, args: &str| {
            let mut w = String::from("zfs program");
            if read_only {
                w.push_str(" -n");
            }
            w.push_str(&format!(" -j -t 100000000 -m 104857600 {pool} - {args}"));
            w
        };
        let mut z = cli(dir, &[(&prog(false, &format!("{pool} t skyzfs:assist")), &format!("{x}-snapshot-1"))]);
        p(&format!("snapshot {x}-snapshot-1"), format!("{:?}", z.snapshot(pool, "t")));
        let mut z = cli(dir, &[(&prog(true, &format!("{pool} t")), &format!("{x}-members"))]);
        p(&format!("members {x}-members"), format!("{:?}", z.members(pool, "t")));
        let mut z = cli(dir, &[(&prog(true, &format!("{pool} absent")), &format!("{x}-members-absent"))]);
        p(&format!("members {x}-members-absent"), format!("{:?}", z.members(pool, "absent")));
        // tagged: the recorder ran it with the tag `skyzfs`; the words given
        // here are the recorder's, the tag passed is the same.
        for t in ["tagged", "tagged-held"] {
            let mut z = cli(dir, &[(&prog(true, &format!("{pool} skyzfs")), &format!("{x}-{t}"))]);
            p(&format!("tagged {x}-{t} (tag skyzfs)"), format!("{:?}", z.tagged(pool, "skyzfs")));
        }
        // held on the record of `zfs holds -H`, with the recorder's tag and
        // with the adapter's own (HOLD_TAG).
        for tag in ["skyzfs", HOLD_TAG] {
            let mut z = cli(dir, &[(&format!("zfs holds -H {pool}@t"), &format!("{x}-holds"))]);
            p(&format!("held {x}-holds tag {tag}"), format!("{:?}", z.held(pool, "t", tag)));
        }
        // hold a second time: "tag already exists".
        let mut z = cli(dir, &[(&format!("zfs hold -r skyzfs {pool}@t"), &format!("{x}-hold-2"))]);
        p(&format!("hold {x}-hold-2"), format!("{:?}", z.hold(pool, "t", "skyzfs")));
        // release a second time: "no such tag", then `zfs holds -H -r`. The
        // only record of holds -r is the one taken while held (none after
        // the release), so the look finds the tag kept.
        let mut z = cli(
            dir,
            &[
                (&format!("zfs release -r skyzfs {pool}@t"), &format!("{x}-release-2")),
                (&format!("zfs holds -H -r {pool}@t"), &format!("{x}-holds-r")),
            ],
        );
        p(
            &format!("release {x}-release-2 then {x}-holds-r (held)"),
            format!("{:?}", z.release(pool, "t", "skyzfs")),
        );
    }

    // The pool that does not exist.
    let pool = "skznone";
    let mut z = cli(dir, &[("zpool status -P skznone", "none-status"), ("zpool status -P -g skznone", "none-status-g")]);
    p("layout none", format!("{:?}", z.layout(pool)));
    let mut z = cli(
        dir,
        &[(
            "zpool get -H -p -o property,value guid,load_guid,multihost,readonly,checkpoint skznone",
            "none-get",
        )],
    );
    p("props none", format!("{:?}", z.props(pool)));
    let mut z = cli(dir, &[("zfs program -j -t 100000000 -m 104857600 skznone - skznone t skyzfs:assist", "none-snapshot-1")]);
    p("snapshot none", format!("{:?}", z.snapshot(pool, "t")));
    let mut z = cli(dir, &[("zfs program -n -j -t 100000000 -m 104857600 skznone - skznone t", "none-members")]);
    p("members none", format!("{:?}", z.members(pool, "t")));
    let mut z = cli(dir, &[("zfs program -n -j -t 100000000 -m 104857600 skznone - skznone skyzfs", "none-tagged")]);
    p("tagged none", format!("{:?}", z.tagged(pool, "skyzfs")));
    let mut z = cli(dir, &[("zfs holds -H skznone@t", "none-holds")]);
    p("held none", format!("{:?}", z.held(pool, "t", "skyzfs")));
    let mut z = cli(dir, &[("zfs hold -r skyzfs skznone@t", "none-hold")]);
    p("hold none", format!("{:?}", z.hold(pool, "t", "skyzfs")));
    let mut z = cli(dir, &[("zfs release -r skyzfs skznone@t", "none-release")]);
    p("release none", format!("{:?}", z.release(pool, "t", "skyzfs")));

    let mut z = cli(dir, &[]);
    z.host().files.insert(
        PathBuf::from(NOCACHEFLUSH),
        rec(dir, "nocacheflush").stdout,
    );
    p("nocacheflush", format!("{:?}", z.nocacheflush()));
}
