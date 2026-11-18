//! Node B for the rigs (M2.T2, design §9): a place that knows only the
//! object store, and a stand-in for the uploader that puts a volume
//! image there through the engine.
//!
//! ```text
//! skyzfs-nodeb put  --endpoint URL --bucket B --volume NAME --image FILE
//!                   --snapshot T [--hole OFF:LEN]...
//! skyzfs-nodeb refs --endpoint URL --bucket B --volume NAME
//! skyzfs-nodeb get  --endpoint URL --bucket B --volume NAME --ref REF --out FILE
//! skyzfs-nodeb row  --endpoint URL --bucket B --volume NAME
//! skyzfs-nodeb bucket --endpoint URL --bucket B --volume NAME
//! skyzfs-nodeb lease --endpoint URL --bucket B --volume NAME --holder H --ttl-s N
//! skyzfs-nodeb release --endpoint URL --bucket B --volume NAME --holder H
//! skyzfs-nodeb move-head --endpoint URL --bucket B --volume NAME --at OFFSET --group G|none [--expect-row ROW]
//! skyzfs-nodeb set-row --endpoint URL --bucket B --volume NAME --row STATE:NUMBER:GROUP|none|hex:BYTES [--expect-row ROW]
//! ```
//!
//! The keys are `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`. The
//! refs are the object store's own (`ObjectRefStore`, one object per
//! volume): neither command keeps anything on the local disk, and
//! neither takes the volume's lease.
//!
//! - `put`, the stand-in: creates the volume, writes every 4 KiB block
//!   of the image that is not all zeros (a new volume reads zeros
//!   elsewhere), then each `--hole` as zeroes written explicitly (a
//!   discard), commits, and takes the engine's snapshot `T`, which is
//!   the ref `snap/T`. The volume must not exist yet.
//! - `refs`: lists the volume's refs, one per line: name, commit, generation.
//! - `get`: reads the ref's content into FILE, sparse where it is zeros.
//! - `row` (M2.T3, the daemon's uploads): head's commit and generation,
//!   `head HEX GEN` or `head none`, then the cut's row that the daemon's
//!   uploader writes with each head swap (the meta row `skyzfs/cut`,
//!   `skyzfs-upload`'s row.rs): `row STATE NUMBER GROUP` for a row of
//!   version 1, `row none`, or `row unreadable HEX`. The two are read
//!   one after the other, not together: a reader that wants them to
//!   agree reads them again around what it does. Then the volume's
//!   lease: `lease HOLDER EPOCH EXPIRES_UNIX_MS` or `lease none`.
//!
//! `bucket` makes the bucket where it is not there (`put` does it too),
//! for a daemon's `create` that puts the volume there.
//!
//! Another writer, for the rig's courses on the daemon's uploads (each
//! acts on the volume as the daemon's uploader would not, with the
//! refs object unfenced, as `put` does):
//! - `lease`: takes the volume's lease as HOLDER for N seconds, as any
//!   holder takes it (`RefStore::lease_acquire`): `lease acquired
//!   HOLDER EPOCH EXPIRES` (exit 0), or `lease held HOLDER EPOCH
//!   EXPIRES` when another's has not run out (exit 5);
//! - `release`: lets HOLDER's lease go;
//! - `move-head`: opens the volume at head, writes one 4 KiB block of
//!   0x5a bytes at OFFSET (a multiple of 4096), sets the commit's group
//!   to G or none, and commits: head moves, the row does not;
//!   `head HEX`;
//! - `set-row`: puts the cut's row (version 1, from a state of 32 hex
//!   digits, a number and a group), deletes it (`none`), or puts BYTES
//!   as they are (`hex:...`, a row the uploader cannot read); head
//!   does not move.
//!
//! With `--expect-row ROW` (as `--row` writes it, `none` for no row),
//! `move-head` and `set-row` act only while the row is ROW, in the same
//! transaction as their write: a daemon that committed a cut since the
//! caller read the row moved the row with its head, and the command
//! changes nothing, prints `row moved`, and ends with 6. The caller reads
//! again and acts on what it finds (the review of #119, G2).
//!
//! Exit 0: done. Exit 6: `--expect-row` did not hold. Exit 3: `get` found no such ref. Exit 4: `get` found
//! the ref and the bucket does not hold its content: an object it needs
//! is not there, does not decode, or is corrupt. For the rig that is a
//! verdict against what put the volume there, not its own error. Exit
//! 2: anything else (a usage error, a store that does not answer, the
//! local file), the rig's own error.

use std::io::{Read, Seek, SeekFrom, Write};
use std::process::ExitCode;
use std::sync::Arc;

use skynbd_engine::{Engine, EngineConfig, EngineError};
use skynbd_format::CommitKind;
use skynbd_refs::{LeaseOutcome, ObjectRefStore, RefOp, RefStore, TxnOutcome};
use skynbd_store::{ObjectStore, S3Store};
use skynbd_types::VolumeId;

const BLOCK: usize = 4096;
/// Commit after this much written, so that the engine's memory stays bounded.
const COMMIT_EVERY: u64 = 128 << 20;
/// How much `get` reads at a time.
const READ_CHUNK: u64 = 4 << 20;

#[derive(Default)]
struct Args {
    cmd: String,
    endpoint: String,
    bucket: String,
    volume: String,
    image: String,
    out: String,
    reference: String,
    snapshot: String,
    holes: Vec<(u64, u64)>,
    holder: String,
    ttl_s: String,
    at: String,
    group: String,
    row: String,
    expect_row: Option<String>,
}

fn parse(argv: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = argv.iter();
    a.cmd = it.next().cloned().ok_or("no command")?;
    while let Some(k) = it.next() {
        let v = it
            .next()
            .ok_or_else(|| format!("{k} needs a value"))?
            .clone();
        match k.as_str() {
            "--endpoint" => a.endpoint = v,
            "--bucket" => a.bucket = v,
            "--volume" => a.volume = v,
            "--image" => a.image = v,
            "--out" => a.out = v,
            "--ref" => a.reference = v,
            "--snapshot" => a.snapshot = v,
            "--hole" => a.holes.push(parse_hole(&v)?),
            "--holder" => a.holder = v,
            "--ttl-s" => a.ttl_s = v,
            "--at" => a.at = v,
            "--group" => a.group = v,
            "--row" => a.row = v,
            "--expect-row" => a.expect_row = Some(v),
            _ => return Err(format!("unknown option {k}")),
        }
    }
    for (name, v) in [
        ("--endpoint", &a.endpoint),
        ("--bucket", &a.bucket),
        ("--volume", &a.volume),
    ] {
        if v.is_empty() {
            return Err(format!("{name} is required"));
        }
    }
    let need: &[(&str, &String)] = match a.cmd.as_str() {
        "put" => &[("--image", &a.image), ("--snapshot", &a.snapshot)],
        "get" => &[("--ref", &a.reference), ("--out", &a.out)],
        "refs" | "row" | "bucket" => &[],
        "lease" => &[("--holder", &a.holder), ("--ttl-s", &a.ttl_s)],
        "release" => &[("--holder", &a.holder)],
        "move-head" => &[("--at", &a.at), ("--group", &a.group)],
        "set-row" => &[("--row", &a.row)],
        c => return Err(format!("unknown command {c}")),
    };
    for (name, v) in need {
        if v.is_empty() {
            return Err(format!("{} needs {name}", a.cmd));
        }
    }
    Ok(a)
}

/// `OFF:LEN` in bytes, both multiples of 512.
fn parse_hole(s: &str) -> Result<(u64, u64), String> {
    let (o, l) = s
        .split_once(':')
        .ok_or_else(|| format!("--hole {s}: expected OFF:LEN"))?;
    let o: u64 = o.parse().map_err(|_| format!("--hole {s}: bad offset"))?;
    let l: u64 = l.parse().map_err(|_| format!("--hole {s}: bad length"))?;
    if !o.is_multiple_of(512) || !l.is_multiple_of(512) || l == 0 {
        return Err(format!(
            "--hole {s}: offset and length must be multiples of 512, the length above 0"
        ));
    }
    Ok((o, l))
}

/// The provisioner's derivation (skynbd-api's `volume_id`), so that a
/// volume put here has the id a daemon would give its name.
fn volume_id(name: &str) -> VolumeId {
    let h = blake3::hash(name.as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&h.as_bytes()[..16]);
    VolumeId(ulid::Ulid::from(u128::from_be_bytes(b)))
}

fn hex(h: &[u8]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// The non-zero runs of `buf` at block granularity -> (offset in buf, length).
fn runs(buf: &[u8]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (k, b) in buf.chunks(BLOCK).enumerate() {
        if b.iter().all(|&x| x == 0) {
            continue;
        }
        let at = k * BLOCK;
        match out.last_mut() {
            Some((o, l)) if *o + *l == at => *l += b.len(),
            _ => out.push((at, b.len())),
        }
    }
    out
}

type S3 = Arc<S3Store>;

async fn open(a: &Args) -> Result<(S3, Arc<ObjectRefStore<S3>>), String> {
    let key = std::env::var("AWS_ACCESS_KEY_ID").map_err(|_| "AWS_ACCESS_KEY_ID is not set")?;
    let secret =
        std::env::var("AWS_SECRET_ACCESS_KEY").map_err(|_| "AWS_SECRET_ACCESS_KEY is not set")?;
    let store =
        Arc::new(S3Store::new(&a.endpoint, &a.bucket, &key, &secret, "us-east-1", true).await);
    // No owner: no lease is taken (node B only reads; the stand-in is
    // the only writer of its volume).
    let refs = Arc::new(ObjectRefStore::new(Arc::clone(&store)));
    Ok((store, refs))
}

struct Put {
    size: u64,
    written: u64,
    head: [u8; 32],
    snapshot: [u8; 32],
}

/// The stand-in's work, on any store: the volume `vol` from the image,
/// the holes, a commit and the snapshot.
async fn put_image<S: ObjectStore + Clone + 'static>(
    store: S,
    refs: Arc<ObjectRefStore<S>>,
    vol: VolumeId,
    image: &str,
    holes: &[(u64, u64)],
    snapshot: &str,
) -> Result<Put, String> {
    let mut f = std::fs::File::open(image).map_err(|e| format!("{image}: {e}"))?;
    let size = f.metadata().map_err(|e| format!("{image}: {e}"))?.len();
    let mut eng = Engine::create_volume(store, refs, vol, size, EngineConfig::default())
        .await
        .map_err(|e| format!("create the volume: {e}"))?;
    let mut buf = vec![0u8; 1 << 20];
    let (mut at, mut written, mut since) = (0u64, 0u64, 0u64);
    while at < size {
        let n = (size - at).min(buf.len() as u64) as usize;
        f.read_exact(&mut buf[..n])
            .map_err(|e| format!("{image}: {e}"))?;
        for (o, l) in runs(&buf[..n]) {
            eng.write(at + o as u64, &buf[o..o + l])
                .map_err(|e| format!("write at {}: {e}", at + o as u64))?;
            written += l as u64;
            since += l as u64;
        }
        if since >= COMMIT_EVERY {
            eng.commit(CommitKind::Import)
                .await
                .map_err(|e| format!("commit: {e}"))?;
            since = 0;
        }
        at += n as u64;
    }
    for &(o, l) in holes {
        eng.write_zeroes(o, l)
            .map_err(|e| format!("hole {o}:{l}: {e}"))?;
    }
    let head = eng
        .commit(CommitKind::Import)
        .await
        .map_err(|e| format!("commit: {e}"))?;
    let (_, snapshot) = eng
        .snapshot_create(snapshot)
        .await
        .map_err(|e| format!("snapshot {snapshot}: {e}"))?;
    Ok(Put {
        size,
        written,
        head,
        snapshot,
    })
}

async fn put(a: &Args) -> Result<(), String> {
    let (store, refs) = open(a).await?;
    store
        .ensure_bucket()
        .await
        .map_err(|e| format!("bucket: {e}"))?;
    let vol = volume_id(&a.volume);
    let p = put_image(store, refs, vol, &a.image, &a.holes, &a.snapshot).await?;
    println!("volume\t{}\t{vol}\t{}", a.volume, p.size);
    println!("written\t{}", p.written);
    println!("holes\t{}", a.holes.len());
    println!("head\t{}", hex(&p.head));
    println!("snapshot\tsnap/{}\t{}", a.snapshot, hex(&p.snapshot));
    Ok(())
}

async fn list(a: &Args) -> Result<(), String> {
    let (_, refs) = open(a).await?;
    let all = refs
        .list(volume_id(&a.volume))
        .await
        .map_err(|e| format!("refs: {e}"))?;
    for (name, v) in all {
        println!("{name}\t{}\t{}", hex(&v.commit), v.generation);
    }
    Ok(())
}

/// The cut's row among the refs object's meta rows, and the length of
/// its version 1 (`skyzfs-upload`'s `ROW_KEY` and `ROW_LEN`: version u16,
/// the cut state's name 16 bytes, the cut's number and group u64, all
/// little endian).
const ROW_KEY: &str = "skyzfs/cut";
const ROW_LEN: usize = 34;

fn row_line(bytes: Option<&[u8]>) -> String {
    let Some(b) = bytes else {
        return "row\tnone".to_string();
    };
    if b.len() != ROW_LEN || b[..2] != 1u16.to_le_bytes() {
        return format!("row\tunreadable\t{}", hex(b));
    }
    let mut n = [0u8; 8];
    let mut g = [0u8; 8];
    n.copy_from_slice(&b[18..26]);
    g.copy_from_slice(&b[26..34]);
    format!(
        "row\t{}\t{}\t{}",
        hex(&b[2..18]),
        u64::from_le_bytes(n),
        u64::from_le_bytes(g)
    )
}

async fn row(a: &Args) -> Result<(), String> {
    let (_, refs) = open(a).await?;
    let vol = volume_id(&a.volume);
    match refs
        .get(vol, "head")
        .await
        .map_err(|e| format!("refs: {e}"))?
    {
        Some(v) => println!("head\t{}\t{}", hex(&v.commit), v.generation),
        None => println!("head\tnone"),
    }
    let b = refs
        .meta_get(vol, ROW_KEY)
        .await
        .map_err(|e| format!("meta {ROW_KEY}: {e}"))?;
    println!("{}", row_line(b.as_deref()));
    match refs
        .lease_get(vol)
        .await
        .map_err(|e| format!("lease: {e}"))?
    {
        Some(l) => println!("lease\t{}\t{}\t{}", l.holder, l.epoch, l.expires_at_ms),
        None => println!("lease\tnone"),
    }
    Ok(())
}

fn number(name: &str, v: &str) -> Result<u64, String> {
    v.parse().map_err(|_| format!("{name} {v}: not a number"))
}

/// `--row`'s value -> the bytes to put, or None to delete the row.
fn row_bytes(v: &str) -> Result<Option<Vec<u8>>, String> {
    if v == "none" {
        return Ok(None);
    }
    if let Some(h) = v.strip_prefix("hex:") {
        return unhex(h).map(Some);
    }
    let f: Vec<&str> = v.split(':').collect();
    let [state, n, g] = f[..] else {
        return Err(format!(
            "--row {v}: expected STATE:NUMBER:GROUP, none or hex:BYTES"
        ));
    };
    let state = unhex(state)?;
    if state.len() != 16 {
        return Err(format!("--row {v}: the state is 16 bytes"));
    }
    let mut b = 1u16.to_le_bytes().to_vec();
    b.extend_from_slice(&state);
    b.extend_from_slice(&number("--row's number", n)?.to_le_bytes());
    b.extend_from_slice(&number("--row's group", g)?.to_le_bytes());
    Ok(Some(b))
}

fn unhex(h: &str) -> Result<Vec<u8>, String> {
    if !h.len().is_multiple_of(2) || !h.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{h}: not hex"));
    }
    (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).map_err(|e| format!("{h}: {e}")))
        .collect()
}

async fn bucket(a: &Args) -> Result<(), String> {
    let (store, _) = open(a).await?;
    store
        .ensure_bucket()
        .await
        .map_err(|e| format!("bucket: {e}"))?;
    println!("bucket\t{}", a.bucket);
    Ok(())
}

async fn lease(a: &Args) -> Result<u8, String> {
    let (_, refs) = open(a).await?;
    let ttl = number("--ttl-s", &a.ttl_s)?;
    let got = refs
        .lease_acquire(volume_id(&a.volume), &a.holder, ttl.saturating_mul(1000))
        .await
        .map_err(|e| format!("lease: {e}"))?;
    let (word, l, code) = match got {
        LeaseOutcome::Acquired(l) => ("acquired", l, 0),
        LeaseOutcome::Held(l) => ("held", l, 5),
    };
    println!(
        "lease\t{word}\t{}\t{}\t{}",
        l.holder, l.epoch, l.expires_at_ms
    );
    Ok(code)
}

async fn release(a: &Args) -> Result<(), String> {
    let (_, refs) = open(a).await?;
    refs.lease_release(volume_id(&a.volume), &a.holder)
        .await
        .map_err(|e| format!("release: {e}"))?;
    println!("released\t{}", a.holder);
    Ok(())
}

async fn move_head(a: &Args) -> Result<u8, String> {
    let at = number("--at", &a.at)?;
    if !at.is_multiple_of(BLOCK as u64) {
        return Err(format!("--at {at}: not a multiple of {BLOCK}"));
    }
    let group = match a.group.as_str() {
        "none" => None,
        g => Some(number("--group", g)?),
    };
    let expect = a.expect_row.as_deref().map(row_bytes).transpose()?;
    let (store, refs) = open(a).await?;
    let moved = move_head_on(store, refs, volume_id(&a.volume), at, group, expect).await?;
    Ok(report(moved, |head| println!("head\t{}", hex(&head))))
}

/// `Some(x)` -> print it and 0; `None` (the expected row did not hold)
/// -> `row moved` and 6.
fn report<T>(got: Option<T>, print: impl FnOnce(T)) -> u8 {
    match got {
        Some(x) => {
            print(x);
            0
        }
        None => {
            println!("row\tmoved");
            6
        }
    }
}

/// The meta op that holds the row at EXPECT and leaves it as it is.
fn row_held(expect: Option<Vec<u8>>) -> Result<RefOp, String> {
    let value = expect
        .clone()
        .ok_or("--expect-row none: move-head keeps a row it can name")?;
    Ok(RefOp::MetaCas {
        key: ROW_KEY.to_string(),
        expected: expect,
        value,
    })
}

/// move-head on a volume's store and refs -> head's new commit, or None
/// where `expect` (the row's bytes, `Some(None)` for no row) did not hold.
async fn move_head_on<S: ObjectStore + Clone + 'static>(
    store: S,
    refs: Arc<ObjectRefStore<S>>,
    vol: VolumeId,
    at: u64,
    group: Option<u64>,
    expect: Option<Option<Vec<u8>>>,
) -> Result<Option<[u8; 32]>, String> {
    let ops = expect.map(row_held).transpose()?;
    let mut eng = Engine::open(store, refs, vol, EngineConfig::default())
        .await
        .map_err(|e| format!("open the volume: {e}"))?;
    eng.write(at, &[0x5a; BLOCK])
        .map_err(|e| format!("write at {at}: {e}"))?;
    eng.set_group(group);
    let guarded = ops.is_some();
    let done = match ops {
        Some(op) => eng.commit_with(CommitKind::Flush, vec![op]).await,
        None => eng.commit(CommitKind::Flush).await,
    };
    match done {
        Ok(head) => Ok(Some(head)),
        // The row, or head with it (a daemon's commit moves both): what
        // the caller read is gone.
        Err(EngineError::OpConflict { .. }) => Ok(None),
        Err(EngineError::Conflict) if guarded => Ok(None),
        Err(e) => Err(format!("commit: {e}")),
    }
}

async fn set_row(a: &Args) -> Result<u8, String> {
    let bytes = row_bytes(&a.row)?;
    let expect = a.expect_row.as_deref().map(row_bytes).transpose()?;
    let (_, refs) = open(a).await?;
    let set = set_row_on(&refs, volume_id(&a.volume), bytes, expect).await?;
    Ok(report(set.then_some(()), |()| {
        println!("row\tset\t{}", a.row)
    }))
}

/// set-row on a volume's refs -> whether it was set (false where
/// `expect`, the row's bytes or `Some(None)` for no row, did not hold).
async fn set_row_on<S: ObjectStore + Clone + 'static>(
    refs: &ObjectRefStore<S>,
    vol: VolumeId,
    bytes: Option<Vec<u8>>,
    expect: Option<Option<Vec<u8>>>,
) -> Result<bool, String> {
    let key = ROW_KEY.to_string();
    let ops = match (bytes, expect) {
        (Some(value), None) => vec![RefOp::MetaPut { key, value }],
        (None, None) => vec![RefOp::MetaDelete { key }],
        (Some(value), Some(expected)) => vec![RefOp::MetaCas {
            key,
            expected,
            value,
        }],
        // The row held, then deleted, in one transaction.
        (None, Some(expected)) => vec![
            row_held(expected)?,
            RefOp::MetaDelete {
                key: ROW_KEY.to_string(),
            },
        ],
    };
    match refs
        .transact(vol, ops)
        .await
        .map_err(|e| format!("set the row: {e}"))?
    {
        TxnOutcome::Committed => Ok(true),
        TxnOutcome::Conflict { .. } => Ok(false),
    }
}

/// What `get` found.
#[derive(Debug, PartialEq, Eq)]
enum Got {
    /// No such ref.
    Absent,
    /// The ref's commit and the volume's size; the content is in the file.
    Read([u8; 32], u64),
    /// The ref is there and its content could not be read.
    Unreadable(String),
}

/// Is the engine's error about what the bucket holds (an object not
/// there, that does not decode, or is corrupt), and not about a store
/// that does not answer?
fn about_content(e: &EngineError) -> bool {
    matches!(
        e,
        EngineError::Store(
            skynbd_store::StoreError::NotFound(_) | skynbd_store::StoreError::BadRange { .. }
        ) | EngineError::Format(_)
            | EngineError::Corrupt(_)
    )
}

/// The exit code for what `get` found.
fn exit_code(g: &Got) -> u8 {
    match g {
        Got::Read(..) => 0,
        Got::Absent => 3,
        Got::Unreadable(_) => 4,
    }
}

/// Node B's read, on any store: the ref's content into `out`. Err is
/// the rig's own error: a store that does not answer, or the local file.
async fn get_ref<S: ObjectStore + Clone + 'static>(
    store: S,
    refs: Arc<ObjectRefStore<S>>,
    vol: VolumeId,
    reference: &str,
    out_path: &str,
) -> Result<Got, String> {
    let Some(rv) = refs
        .get(vol, reference)
        .await
        .map_err(|e| format!("refs: {e}"))?
    else {
        return Ok(Got::Absent);
    };
    let eng = match Engine::open_ref(store, refs, vol, reference, EngineConfig::default()).await {
        Ok(e) => e,
        Err(e) if about_content(&e) => {
            return Ok(Got::Unreadable(format!("open {reference}: {e}")))
        }
        Err(e) => return Err(format!("open {reference}: {e}")),
    };
    let size = eng.size();
    let mut out = std::fs::File::create(out_path).map_err(|e| format!("{out_path}: {e}"))?;
    out.set_len(size).map_err(|e| format!("{out_path}: {e}"))?;
    let mut at = 0u64;
    while at < size {
        let n = (size - at).min(READ_CHUNK);
        let b = match eng.read(at, n).await {
            Ok(b) => b,
            Err(e) if about_content(&e) => {
                return Ok(Got::Unreadable(format!("read at {at}: {e}")))
            }
            Err(e) => return Err(format!("read at {at}: {e}")),
        };
        if b.len() as u64 != n {
            return Ok(Got::Unreadable(format!(
                "read at {at}: {} bytes, not {n}",
                b.len()
            )));
        }
        if b.iter().any(|&x| x != 0) {
            out.seek(SeekFrom::Start(at))
                .map_err(|e| format!("{out_path}: {e}"))?;
            out.write_all(&b).map_err(|e| format!("{out_path}: {e}"))?;
        }
        at += n;
    }
    out.sync_all().map_err(|e| format!("{out_path}: {e}"))?;
    Ok(Got::Read(rv.commit, size))
}

/// -> the exit code.
async fn get(a: &Args) -> Result<u8, String> {
    let (store, refs) = open(a).await?;
    let g = get_ref(store, refs, volume_id(&a.volume), &a.reference, &a.out).await?;
    match &g {
        Got::Absent => eprintln!(
            "skyzfs-nodeb: volume {} has no ref {}",
            a.volume, a.reference
        ),
        Got::Unreadable(e) => {
            eprintln!("skyzfs-nodeb: {} of volume {}: {e}", a.reference, a.volume)
        }
        Got::Read(c, size) => println!("read\t{}\t{}\t{size}", a.reference, hex(c)),
    }
    Ok(exit_code(&g))
}

#[tokio::main]
async fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let a = match parse(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("skyzfs-nodeb: {e}\nusage: put|refs|get|row|bucket|lease|release|move-head|set-row (see the source's header)");
            return ExitCode::from(2);
        }
    };
    let r = match a.cmd.as_str() {
        "put" => put(&a).await.map(|()| 0),
        "refs" => list(&a).await.map(|()| 0),
        "row" => row(&a).await.map(|()| 0),
        "bucket" => bucket(&a).await.map(|()| 0),
        "lease" => lease(&a).await,
        "release" => release(&a).await.map(|()| 0),
        "move-head" => move_head(&a).await,
        "set-row" => set_row(&a).await,
        _ => get(&a).await,
    };
    match r {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("skyzfs-nodeb: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn runs_join_adjacent_blocks_and_skip_zeros() {
        let mut b = vec![0u8; 5 * BLOCK + 512];
        b[0] = 1;
        b[BLOCK + 7] = 1;
        b[3 * BLOCK] = 1;
        b[5 * BLOCK + 100] = 1;
        assert_eq!(
            runs(&b),
            vec![(0, 2 * BLOCK), (3 * BLOCK, BLOCK), (5 * BLOCK, 512)]
        );
        assert!(runs(&vec![0u8; 3 * BLOCK]).is_empty());
    }

    #[test]
    fn the_cut_row_is_read_as_the_uploader_writes_it() {
        assert_eq!(row_line(None), "row\tnone");
        let mut b = 1u16.to_le_bytes().to_vec();
        b.extend_from_slice(&[0xab; 16]);
        b.extend_from_slice(&7u64.to_le_bytes());
        b.extend_from_slice(&4242u64.to_le_bytes());
        assert_eq!(
            row_line(Some(&b)),
            format!("row\t{}\t7\t4242", "ab".repeat(16))
        );
        let mut v2 = b.clone();
        v2[0] = 2;
        assert!(row_line(Some(&v2)).starts_with("row\tunreadable\t02"));
        assert!(row_line(Some(&b[..33])).starts_with("row\tunreadable\t"));
    }

    #[test]
    fn a_row_is_put_as_the_uploader_reads_it() {
        let b = row_bytes(&format!("{}:7:4242", "ab".repeat(16)))
            .unwrap()
            .unwrap();
        assert_eq!(
            row_line(Some(&b)),
            format!("row\t{}\t7\t4242", "ab".repeat(16))
        );
        assert_eq!(row_bytes("none"), Ok(None));
        assert_eq!(row_bytes("hex:0200ff"), Ok(Some(vec![2, 0, 0xff])));
        assert!(row_bytes("ab:7:1").is_err());
        assert!(row_bytes(&format!("{}:x:1", "ab".repeat(16))).is_err());
        assert!(row_bytes("hex:0g").is_err());
    }

    /// The other writer acts only on the row it read (the review of #119,
    /// G2): a row that moved since leaves the volume as it is.
    #[tokio::test]
    async fn the_other_writer_acts_on_the_row_it_read() {
        let (store, refs, vol, _) = put_small("expect-row").await;
        let row = |n: u64, g: u64| row_bytes(&format!("{}:{n}:{g}", "ab".repeat(16))).unwrap();
        let read = || async { refs.meta_get(vol, ROW_KEY).await.expect("meta_get") };
        // No row yet: expecting none sets it; expecting a row does not.
        assert_eq!(
            set_row_on(&refs, vol, row(1, 4), Some(row(9, 9))).await,
            Ok(false)
        );
        assert_eq!(read().await, None);
        assert_eq!(
            set_row_on(&refs, vol, row(1, 4), Some(None)).await,
            Ok(true)
        );
        assert_eq!(read().await, row(1, 4));
        // The row as read: set; a stale one: nothing changes.
        assert_eq!(
            set_row_on(&refs, vol, row(2, 5), row(1, 4).map(Some)).await,
            Ok(true)
        );
        assert_eq!(
            set_row_on(&refs, vol, row(3, 6), row(1, 4).map(Some)).await,
            Ok(false)
        );
        assert_eq!(read().await, row(2, 5));
        // Unguarded, as before.
        assert_eq!(set_row_on(&refs, vol, row(4, 7), None).await, Ok(true));
        assert_eq!(read().await, row(4, 7));
        // move-head: head moves with the row as read, and the row stays.
        let head = refs
            .get(vol, "head")
            .await
            .expect("get")
            .expect("head")
            .commit;
        let moved = move_head_on(
            store.clone(),
            Arc::clone(&refs),
            vol,
            0,
            Some(1000),
            row(4, 7).map(Some),
        )
        .await
        .expect("move-head");
        let now = refs
            .get(vol, "head")
            .await
            .expect("get")
            .expect("head")
            .commit;
        assert!(moved.is_some_and(|h| h == now) && now != head);
        assert_eq!(read().await, row(4, 7));
        // A stale row: head stays.
        let stale = move_head_on(
            store.clone(),
            Arc::clone(&refs),
            vol,
            0,
            Some(1001),
            row(3, 6).map(Some),
        )
        .await
        .expect("move-head");
        assert_eq!(stale, None);
        assert_eq!(
            refs.get(vol, "head")
                .await
                .expect("get")
                .expect("head")
                .commit,
            now
        );
        // Deleting under a guard: the row held, then gone.
        assert_eq!(
            set_row_on(&refs, vol, None, row(3, 6).map(Some)).await,
            Ok(false)
        );
        assert_eq!(
            set_row_on(&refs, vol, None, row(4, 7).map(Some)).await,
            Ok(true)
        );
        assert_eq!(read().await, None);
        assert!(move_head_on(store, refs, vol, 0, None, Some(None))
            .await
            .is_err());
    }

    #[test]
    fn holes_must_be_sector_aligned() {
        assert_eq!(parse_hole("4096:8192"), Ok((4096, 8192)));
        assert!(parse_hole("100:512").is_err());
        assert!(parse_hole("512:0").is_err());
        assert!(parse_hole("512").is_err());
    }

    #[test]
    fn each_command_names_what_it_needs() {
        let base = "--endpoint http://x --bucket b --volume v";
        assert!(parse(&args(&format!(
            "put {base} --image i --snapshot t --hole 0:512"
        )))
        .is_ok());
        assert!(parse(&args(&format!("put {base} --image i"))).is_err());
        assert!(parse(&args(&format!("get {base} --ref snap/t --out o"))).is_ok());
        assert!(parse(&args(&format!("get {base} --out o"))).is_err());
        assert!(parse(&args(&format!("refs {base}"))).is_ok());
        assert!(parse(&args(&format!("row {base}"))).is_ok());
        assert!(parse(&args(&format!("lease {base} --holder h --ttl-s 5"))).is_ok());
        assert!(parse(&args(&format!("lease {base} --holder h"))).is_err());
        assert!(parse(&args(&format!("release {base}"))).is_err());
        assert!(parse(&args(&format!("move-head {base} --at 0 --group none"))).is_ok());
        assert!(parse(&args(&format!("move-head {base} --at 0"))).is_err());
        assert!(parse(&args(&format!("set-row {base} --row none"))).is_ok());
        let a = parse(&args(&format!(
            "set-row {base} --row none --expect-row none"
        )))
        .expect("parse");
        assert_eq!(a.expect_row.as_deref(), Some("none"));
        assert_eq!(
            parse(&args(&format!("set-row {base} --row none")))
                .expect("parse")
                .expect_row,
            None
        );
        assert!(parse(&args("refs --endpoint http://x --bucket b")).is_err());
        assert!(parse(&args(&format!("copy {base}"))).is_err());
        assert!(parse(&args(&format!("refs {base} --nope 1"))).is_err());
    }

    /// The stand-in puts an image with a hole into a store; a second
    /// ref store over the same objects (node B, which has nothing else)
    /// finds `snap/t` and reads back the image with the hole in it.
    #[tokio::test]
    async fn a_put_image_reads_back_through_a_second_ref_store() {
        let dir = std::env::temp_dir().join(format!("skyzfs-nodeb-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir)
            .map_err(|e| e.to_string())
            .expect("tmp dir");
        let image = dir.join("in.img");
        let back = dir.join("out.img");
        let size = 3 << 20;
        let mut want = vec![0u8; size];
        for (k, b) in want.iter_mut().enumerate() {
            // Zero runs and data runs, at block and sub-block offsets.
            if (k / 3000) % 3 != 0 {
                *b = (k % 251) as u8 + 1;
            }
        }
        std::fs::write(&image, &want).expect("write image");
        let store = skynbd_store::MemoryStore::new();
        let refs = Arc::new(ObjectRefStore::new(store.clone()));
        let vol = volume_id("vol-test");
        let hole = (2 * BLOCK as u64, 2 * BLOCK as u64);
        let p = put_image(
            store.clone(),
            refs,
            vol,
            image.to_str().expect("path"),
            &[hole],
            "t",
        )
        .await
        .expect("put");
        assert_eq!(p.size, size as u64);
        for b in &mut want[hole.0 as usize..(hole.0 + hole.1) as usize] {
            *b = 0;
        }
        let node_b = Arc::new(ObjectRefStore::new(store.clone()));
        let names: Vec<String> = node_b
            .list(vol)
            .await
            .expect("list")
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert!(
            names.contains(&"snap/t".to_string()) && names.contains(&"head".to_string()),
            "{names:?}"
        );
        let got = get_ref(
            store.clone(),
            Arc::clone(&node_b),
            vol,
            "snap/t",
            back.to_str().expect("path"),
        )
        .await
        .expect("get");
        assert_eq!(got, Got::Read(p.snapshot, size as u64));
        assert!(
            std::fs::read(&back).expect("read back") == want,
            "the content read back differs"
        );
        let none = get_ref(
            store.clone(),
            Arc::clone(&node_b),
            vol,
            "snap/u",
            back.to_str().expect("path"),
        )
        .await
        .expect("get");
        assert_eq!(none, Got::Absent);

        // The head moves on after the snapshot (review of #97, S6): get
        // of snap/t still reads the snapshot, and get of head the new state.
        let mut eng = Engine::open(
            store.clone(),
            Arc::clone(&node_b),
            vol,
            EngineConfig::default(),
        )
        .await
        .expect("open");
        eng.write(0, &[0xEEu8; BLOCK]).expect("write");
        let head = eng.commit(CommitKind::Flush).await.expect("commit");
        let got = get_ref(
            store.clone(),
            Arc::clone(&node_b),
            vol,
            "snap/t",
            back.to_str().expect("path"),
        )
        .await
        .expect("get");
        assert_eq!(got, Got::Read(p.snapshot, size as u64));
        assert!(
            std::fs::read(&back).expect("read back") == want,
            "snap/t read the head"
        );
        let got = get_ref(store, node_b, vol, "head", back.to_str().expect("path"))
            .await
            .expect("get");
        assert_eq!(got, Got::Read(head, size as u64));
        let mut moved = want.clone();
        moved[..BLOCK].fill(0xEE);
        assert!(
            std::fs::read(&back).expect("read back") == moved,
            "head is not the new state"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The volume put with an image of 1 MiB, its keys, and the ref
    /// store: for the tests that take objects away.
    async fn put_small(
        name: &str,
    ) -> (
        skynbd_store::MemoryStore,
        Arc<ObjectRefStore<skynbd_store::MemoryStore>>,
        VolumeId,
        Vec<String>,
    ) {
        let dir = std::env::temp_dir().join(format!("skyzfs-nodeb-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tmp dir");
        let image = dir.join("in.img");
        std::fs::write(&image, vec![7u8; 1 << 20]).expect("write image");
        let store = skynbd_store::MemoryStore::new();
        let refs = Arc::new(ObjectRefStore::new(store.clone()));
        let vol = volume_id(name);
        put_image(
            store.clone(),
            Arc::clone(&refs),
            vol,
            image.to_str().expect("path"),
            &[],
            "t",
        )
        .await
        .expect("put");
        let _ = std::fs::remove_dir_all(&dir);
        let keys = store.list("").await.expect("list");
        (store, refs, vol, keys)
    }

    async fn get_after_removing(name: &str, gone: impl Fn(&str) -> bool) -> Got {
        let (store, refs, vol, keys) = put_small(name).await;
        let mut n = 0;
        for k in keys.iter().filter(|k| gone(k)) {
            store.delete(k).await.expect("delete");
            n += 1;
        }
        assert!(n > 0, "nothing removed among {keys:?}");
        let out =
            std::env::temp_dir().join(format!("skyzfs-nodeb-{name}-{}.out", std::process::id()));
        let got = get_ref(store, refs, vol, "snap/t", out.to_str().expect("path"))
            .await
            .expect("get");
        let _ = std::fs::remove_file(&out);
        got
    }

    /// A listed ref whose commit is gone: the open fails (exit 4).
    #[tokio::test]
    async fn a_ref_whose_objects_are_gone_is_unreadable() {
        let got =
            get_after_removing("gone", |k| !k.ends_with("/refs") && !k.contains("/sb/")).await;
        assert!(
            matches!(got, Got::Unreadable(ref e) if e.starts_with("open ")),
            "{got:?}"
        );
        assert_eq!(exit_code(&got), 4);
    }

    /// A listed ref whose commit opens and whose data is not there, as
    /// a ref swapped before its segments were put would be (review of
    /// #97, N4): the read fails (exit 4), not the rig.
    #[tokio::test]
    async fn a_ref_whose_segments_are_gone_is_unreadable() {
        let got = get_after_removing("noseg", |k| k.contains("/seg/")).await;
        assert!(
            matches!(got, Got::Unreadable(ref e) if e.starts_with("read at ")),
            "{got:?}"
        );
        assert_eq!(exit_code(&got), 4);
    }

    #[test]
    fn exit_codes() {
        assert_eq!(exit_code(&Got::Read([0; 32], 512)), 0);
        assert_eq!(exit_code(&Got::Absent), 3);
        assert_eq!(exit_code(&Got::Unreadable(String::new())), 4);
    }

    #[test]
    fn a_store_that_does_not_answer_is_not_about_content() {
        use skynbd_store::StoreError;
        assert!(about_content(&EngineError::Store(StoreError::NotFound(
            "k".into()
        ))));
        assert!(about_content(&EngineError::Corrupt("x".into())));
        assert!(!about_content(&EngineError::Store(StoreError::Backend(
            "timeout".into()
        ))));
        assert!(!about_content(&EngineError::Conflict));
    }

    #[test]
    fn the_id_is_the_provisioners() {
        // skynbd-api's volume_id, the same bytes.
        let h = blake3::hash(b"vol1");
        let want = u128::from_be_bytes(h.as_bytes()[..16].try_into().unwrap_or([0; 16]));
        assert_eq!(volume_id("vol1").0, ulid::Ulid::from(want));
    }
}
