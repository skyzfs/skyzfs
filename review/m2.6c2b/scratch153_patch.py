#!/usr/bin/env python3
"""Review of #153 (M2.6c-2b) at 65d72ee: patch a scratch copy of the head
for the review's search (scratch153.rs). Never committed to the product.

- engine/fake.rs: knobs on the fake world (a person's action as a late
  take lands, a take or a reading that fails once, a destroy refused or
  gone through with an error, datasets `zfs snapshot -r` skips as
  inconsistent) and four more of a person's actions (a dataset created,
  one destroyed with its snapshots, one renamed, a person's own hold on
  `pool@t`);
- engine/tests.rs: `#[path = "review_scratch.rs"] mod review_scratch;`
  appended, and scratch153.rs copied beside it.

usage: python3 -I scratch153_patch.py WORKTREE
"""
import pathlib
import shutil
import sys

ROOT = pathlib.Path(sys.argv[1])
HERE = pathlib.Path(__file__).resolve().parent
ENG = ROOT / "crates/skyzfs-agent/src/engine"
FAKE = ENG / "fake.rs"


def sub(text, old, new, what):
    if text.count(old) != 1:
        raise SystemExit(f"{what}: site found {text.count(old)} times")
    return text.replace(old, new)


t = FAKE.read_text()

t = sub(t, """    /// `zfs set user:note=N pool/db`, a new N each time: a property of a
    /// member changed after the take, or before it.
    SetProperty,
}""", """    /// `zfs set user:note=N pool/db`, a new N each time: a property of a
    /// member changed after the take, or before it.
    SetProperty,
    /// Review scratch: `zfs create pool/newN`.
    NewDs,
    /// Review scratch: `zfs destroy -r pool/vol`; refused while held.
    RmDs,
    /// Review scratch: `zfs rename pool/db pool/db2`.
    RenameDs,
    /// Review scratch: `zfs hold theirs pool@t`, never released.
    HoldTheirs,
}""", "User")

t = sub(t, """    /// How many times a person changed [`NOTE`].
    pub notes: u64,""", """    /// How many times a person changed [`NOTE`].
    pub notes: u64,
    /// Review scratch: a person's action as soon as a late take lands.
    pub on_land: Option<User>,
    /// Review scratch: the agent's next take fails before its ioctl.
    pub snap_fail_once: bool,
    /// Review scratch: the agent's `datasets` read of this number fails.
    pub datasets_fail_at: Option<usize>,
    /// Review scratch: the agent's `datasets` reads so far.
    pub datasets_calls: usize,
    /// Review scratch: 0 a destroy as ZFS does it; 1 refused, nothing
    /// destroyed; 2 destroyed, and an error returned (killed at its limit
    /// after its ioctl).
    pub destroy_mode: u8,
    /// Review scratch: datasets `zfs snapshot -r` skips (inconsistent,
    /// libzfs `zfs_snapshot_cb`), which the members program still lists.
    pub inconsistent: BTreeSet<String>,
    /// Review scratch: datasets a person created.
    pub created: u64,""", "World fields")

t = sub(t, """            notes: 0,
            log: Vec::new(),""", """            notes: 0,
            on_land: None,
            snap_fail_once: false,
            datasets_fail_at: None,
            datasets_calls: 0,
            destroy_mode: 0,
            inconsistent: BTreeSet::new(),
            created: 0,
            log: Vec::new(),""", "World::new")

t = sub(t, """            User::Attach | User::Add => {
                let Some(pool) = &mut self.pool else {
                    return;
                };""", """            User::NewDs => {
                self.created += 1;
                let guid = self.guid();
                let n = self.created;
                let Some(pool) = &mut self.pool else {
                    return;
                };
                let createtxg = pool.next_txg.unwrap_or(pool.txg + 1);
                pool.datasets.push(Ds {
                    name: format!("{POOL}/new{n}"),
                    kind: Kind::Filesystem,
                    guid,
                    createtxg,
                    local: BTreeMap::new(),
                });
                pool.datasets
                    .sort_by(|a, b| (a.name != POOL, &a.name).cmp(&(b.name != POOL, &b.name)));
                self.sync("create");
            }
            User::RmDs => {
                let Some(pool) = &mut self.pool else {
                    return;
                };
                let prefix = format!("{POOL}/vol@");
                if pool
                    .snaps
                    .iter()
                    .any(|(k, s)| k.starts_with(&prefix) && !s.holds.is_empty())
                {
                    return;
                }
                if !pool.datasets.iter().any(|d| d.name == format!("{POOL}/vol")) {
                    return;
                }
                pool.snaps.retain(|k, _| !k.starts_with(&prefix));
                pool.datasets.retain(|d| d.name != format!("{POOL}/vol"));
                self.sync("destroy vol");
            }
            User::RenameDs => {
                let Some(pool) = &mut self.pool else {
                    return;
                };
                let (from, to) = (format!("{POOL}/db"), format!("{POOL}/db2"));
                let Some(d) = pool.datasets.iter_mut().find(|d| d.name == from) else {
                    return;
                };
                d.name = to.clone();
                let moved: Vec<String> = pool
                    .snaps
                    .keys()
                    .filter(|k| k.starts_with(&format!("{from}@")))
                    .cloned()
                    .collect();
                for k in moved {
                    let s = pool.snaps.remove(&k).expect("listed");
                    pool.snaps.insert(k.replacen(&from, &to, 1), s);
                }
                pool.datasets
                    .sort_by(|a, b| (a.name != POOL, &a.name).cmp(&(b.name != POOL, &b.name)));
                self.sync("rename");
            }
            User::HoldTheirs => {
                let Some(pool) = &mut self.pool else {
                    return;
                };
                if let Some(s) = pool.snaps.get_mut(&format!("{POOL}@{t}")) {
                    s.holds.insert("theirs".into());
                    self.sync("hold theirs");
                }
            }
            User::Attach | User::Add => {
                let Some(pool) = &mut self.pool else {
                    return;
                };""", "user()")

# zfs snapshot -r skips inconsistent datasets.
t = sub(t, """        let mut members = Vec::new();
        for d in datasets {
            let snapshot_guid = self.guid();""", """        let mut members = Vec::new();
        for d in datasets {
            if self.inconsistent.contains(&d.name) {
                continue;
            }
            let snapshot_guid = self.guid();""", "take skips inconsistent")

# A person's action as a late take lands.
t = sub(t, """    fn land(&mut self) {
        if let Some((t, _)) = self.pending_take.take() {
            let landed = self.agent_take(&t);
            self.log
                .push(format!("late take of {t} landed: {landed:?}"));
        }
    }""", """    pub fn land(&mut self) {
        if let Some((t, _)) = self.pending_take.take() {
            let landed = self.agent_take(&t);
            self.log
                .push(format!("late take of {t} landed: {landed:?}"));
            if landed.is_ok() {
                if let Some(user) = self.on_land.take() {
                    self.user(user, &t);
                }
            }
        }
    }""", "land")

t = sub(t, """            Some(0) => {
                let landed = world.agent_take(t);
                world
                    .log
                    .push(format!("late take of {t} landed: {landed:?}"));
            }""", """            Some(0) => {
                let landed = world.agent_take(t);
                world
                    .log
                    .push(format!("late take of {t} landed: {landed:?}"));
                if landed.is_ok() {
                    if let Some(user) = world.on_land.take() {
                        world.user(user, t);
                    }
                }
            }""", "snapshot Some(0)")

t = sub(t, """        // A sync task waiting lands before this one.
        world.land();
        match world.late_take.take() {""", """        // A sync task waiting lands before this one.
        world.land();
        if world.snap_fail_once {
            world.snap_fail_once = false;
            world.log.push(format!("scratch: take of {t} failed, nothing taken"));
            return Err(ZfsError::Other("zfs snapshot -r: out of space (scratch)".into()));
        }
        match world.late_take.take() {""", "snapshot fail once")

t = sub(t, """        self.0.borrow_mut().zfs_call();
        self.0.borrow_mut().calls.push("datasets");""", """        self.0.borrow_mut().zfs_call();
        self.0.borrow_mut().calls.push("datasets");
        {
            let mut world = self.0.borrow_mut();
            let k = world.datasets_calls;
            world.datasets_calls += 1;
            if world.datasets_fail_at == Some(k) {
                world.log.push(format!("scratch: datasets read {k} failed"));
                return Err(ZfsError::Other("zfs program: timed out (scratch)".into()));
            }
        }""", "datasets fail")

t = sub(t, """        let done = match world.destroy(t) {
            Err(ZfsError::NoSnapshot) => Ok(()),
            done => done,
        };""", """        let done = match world.destroy_mode {
            1 => Err(ZfsError::Other("dataset is busy (scratch)".into())),
            2 => {
                let went = world.destroy(t);
                world.log.push(format!("scratch: destroy -r {t} went through: {went:?}"));
                Err(ZfsError::Other("killed at its limit (scratch)".into()))
            }
            _ => match world.destroy(t) {
                Err(ZfsError::NoSnapshot) => Ok(()),
                done => done,
            },
        };""", "destroy mode")

FAKE.write_text(t)

tests = ENG / "tests.rs"
tt = tests.read_text()
line = '\n#[path = "review_scratch.rs"]\nmod review_scratch;\n'
if line not in tt:
    tests.write_text(tt + line)
shutil.copy(HERE / "scratch153.rs", ENG / "review_scratch.rs")
print("patched")
