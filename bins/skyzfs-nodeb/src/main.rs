//! Node B for the rigs (M2.T2, design §9): a place that knows only the
//! object store, and a stand-in for the uploader that puts a volume
//! image there through the engine.
//!
//! ```text
//! skyzfs-nodeb put  --endpoint URL --bucket B --volume NAME --image FILE
//!                   --snapshot T [--hole OFF:LEN]...
//! skyzfs-nodeb refs --endpoint URL --bucket B --volume NAME
//! skyzfs-nodeb get  --endpoint URL --bucket B --volume NAME --ref REF --out FILE
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
//!
//! Exit 0: done. Exit 3: `get` found no such ref. Exit 2: anything else
//! (a usage error, the store, the engine), the rig's own error.

use std::io::{Read, Seek, SeekFrom, Write};
use std::process::ExitCode;
use std::sync::Arc;

use skynbd_engine::{Engine, EngineConfig};
use skynbd_format::CommitKind;
use skynbd_refs::{ObjectRefStore, RefStore};
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
        "refs" => &[],
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

/// Node B's read, on any store: the ref's content into `out` -> its
/// commit and the volume's size, or None when there is no such ref.
async fn get_ref<S: ObjectStore + Clone + 'static>(
    store: S,
    refs: Arc<ObjectRefStore<S>>,
    vol: VolumeId,
    reference: &str,
    out_path: &str,
) -> Result<Option<([u8; 32], u64)>, String> {
    let Some(rv) = refs
        .get(vol, reference)
        .await
        .map_err(|e| format!("refs: {e}"))?
    else {
        return Ok(None);
    };
    let eng = Engine::open_ref(store, refs, vol, reference, EngineConfig::default())
        .await
        .map_err(|e| format!("open {reference}: {e}"))?;
    let size = eng.size();
    let mut out = std::fs::File::create(out_path).map_err(|e| format!("{out_path}: {e}"))?;
    out.set_len(size).map_err(|e| format!("{out_path}: {e}"))?;
    let mut at = 0u64;
    while at < size {
        let n = (size - at).min(READ_CHUNK);
        let b = eng
            .read(at, n)
            .await
            .map_err(|e| format!("read at {at}: {e}"))?;
        if b.len() as u64 != n {
            return Err(format!("read at {at}: {} bytes, not {n}", b.len()));
        }
        if b.iter().any(|&x| x != 0) {
            out.seek(SeekFrom::Start(at))
                .map_err(|e| format!("{out_path}: {e}"))?;
            out.write_all(&b).map_err(|e| format!("{out_path}: {e}"))?;
        }
        at += n;
    }
    out.sync_all().map_err(|e| format!("{out_path}: {e}"))?;
    Ok(Some((rv.commit, size)))
}

/// -> Ok(false) when the ref does not exist.
async fn get(a: &Args) -> Result<bool, String> {
    let (store, refs) = open(a).await?;
    match get_ref(store, refs, volume_id(&a.volume), &a.reference, &a.out).await? {
        None => Ok(false),
        Some((c, size)) => {
            println!("read\t{}\t{}\t{size}", a.reference, hex(&c));
            Ok(true)
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let a = match parse(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("skyzfs-nodeb: {e}\nusage: put|refs|get (see the source's header)");
            return ExitCode::from(2);
        }
    };
    let r = match a.cmd.as_str() {
        "put" => put(&a).await.map(|()| true),
        "refs" => list(&a).await.map(|()| true),
        _ => get(&a).await,
    };
    match r {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => {
            eprintln!(
                "skyzfs-nodeb: volume {} has no ref {}",
                a.volume, a.reference
            );
            ExitCode::from(3)
        }
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
        assert_eq!(got, Some((p.snapshot, size as u64)));
        assert!(
            std::fs::read(&back).expect("read back") == want,
            "the content read back differs"
        );
        let none = get_ref(store, node_b, vol, "snap/u", back.to_str().expect("path"))
            .await
            .expect("get");
        assert_eq!(none, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_id_is_the_provisioners() {
        // skynbd-api's volume_id, the same bytes.
        let h = blake3::hash(b"vol1");
        let want = u128::from_be_bytes(h.as_bytes()[..16].try_into().unwrap_or([0; 16]));
        assert_eq!(volume_id("vol1").0, ulid::Ulid::from(want));
    }
}
