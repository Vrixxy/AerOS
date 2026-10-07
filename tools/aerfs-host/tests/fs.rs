use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use aerfs_host::aerfs::*;

static CLOCK: AtomicU64 = AtomicU64::new(1_700_000_000);

fn clock() -> u64 {
    CLOCK.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone)]
struct Ram {
    data: Vec<u8>,
    blocks: u32,
    sector_writes: usize,
    cut_after: Option<usize>,
}

impl Ram {
    fn new(blocks: u32) -> Self {
        Self {
            data: vec![0; blocks as usize * BLOCK],
            blocks,
            sector_writes: 0,
            cut_after: None,
        }
    }

    fn alive(&self) -> bool {
        self.cut_after.is_none_or(|limit| self.sector_writes < limit)
    }

    fn put_sector(&mut self, block: u32, sector: usize, bytes: &[u8]) {
        if self.alive() {
            let at = block as usize * BLOCK + sector * SECTOR;
            self.data[at..at + SECTOR].copy_from_slice(bytes);
            self.sector_writes += 1;
        }
    }

    fn powered_off(&self) -> bool {
        !self.alive()
    }
}

impl Device for Ram {
    fn blocks(&self) -> u32 {
        self.blocks
    }

    fn read_block(&mut self, block: u32, out: &mut [u8; BLOCK]) -> bool {
        let at = block as usize * BLOCK;
        out.copy_from_slice(&self.data[at..at + BLOCK]);
        true
    }

    fn write_block(&mut self, block: u32, data: &[u8; BLOCK]) -> bool {
        for sector in 0..BLOCK / SECTOR {
            self.put_sector(block, sector, &data[sector * SECTOR..(sector + 1) * SECTOR]);
        }
        true
    }

    fn read_sector(&mut self, block: u32, out: &mut [u8; SECTOR]) -> bool {
        let at = block as usize * BLOCK;
        out.copy_from_slice(&self.data[at..at + SECTOR]);
        true
    }

    fn write_sector(&mut self, block: u32, data: &[u8; SECTOR]) -> bool {
        self.put_sector(block, 0, data);
        true
    }

    fn flush(&mut self) -> bool {
        true
    }
}

type Fs = AerFs<Ram>;

fn formatted(blocks: u32) -> Ram {
    AerFs::format(Ram::new(blocks), b"test", clock).unwrap()
}

fn mounted(ram: Ram) -> Box<Fs> {
    match AerFs::mount(ram, clock) {
        Ok(fs) => Box::new(fs),
        Err((error, _)) => panic!("mount failed: {error:?}"),
    }
}

/// The whole filesystem as path -> bytes (None for a directory).
fn state(fs: &mut Fs) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(fs: &mut Fs, dir: u32, prefix: &str, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        let mut cursor = 0;
        while let Some(entry) = fs.list_next(dir, &mut cursor).unwrap() {
            let name = String::from_utf8(entry.name().to_vec()).unwrap();
            let path = format!("{prefix}/{name}");
            if entry.node.is_directory() {
                out.insert(path.clone(), None);
                walk(fs, entry.node.ino, &path, out);
            } else {
                let mut bytes = vec![0u8; entry.node.size as usize];
                let read = fs.read_at(&entry.node, 0, &mut bytes).unwrap();
                assert_eq!(read, bytes.len());
                out.insert(path, Some(bytes));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(fs, ROOT_INO, "", &mut out);
    out
}

#[derive(Clone, Debug)]
enum Op {
    Mkdir(String),
    Write(String, u64, Vec<u8>),
    Truncate(String, u64),
    Remove(String),
    Rename(String, String),
}

type Model = BTreeMap<String, Option<Vec<u8>>>;

fn parent_of(path: &str) -> String {
    path.rsplit_once('/').map(|(parent, _)| parent.to_string()).unwrap_or_default()
}

/// Applies an operation to the model; `false` when it would be refused.
fn model_apply(model: &mut Model, op: &Op) -> bool {
    let parent_ok = |model: &Model, path: &str| {
        let parent = parent_of(path);
        parent.is_empty() || matches!(model.get(&parent), Some(None))
    };
    match op {
        Op::Mkdir(path) => {
            if model.contains_key(path) || !parent_ok(model, path) {
                return false;
            }
            model.insert(path.clone(), None);
            true
        }
        Op::Write(path, offset, data) => {
            match model.get_mut(path) {
                Some(None) => return false,
                Some(Some(bytes)) => write_into(bytes, *offset, data),
                None => {
                    if !parent_ok(model, path) {
                        return false;
                    }
                    let mut bytes = Vec::new();
                    write_into(&mut bytes, *offset, data);
                    model.insert(path.clone(), Some(bytes));
                }
            }
            true
        }
        Op::Truncate(path, length) => match model.get_mut(path) {
            Some(Some(bytes)) => {
                bytes.resize(*length as usize, 0);
                true
            }
            _ => false,
        },
        Op::Remove(path) => {
            let prefix = format!("{path}/");
            match model.get(path) {
                None => false,
                Some(None) if model.keys().any(|key| key.starts_with(&prefix)) => false,
                Some(_) => {
                    model.remove(path);
                    true
                }
            }
        }
        Op::Rename(from, to) => {
            if from == to {
                return model.contains_key(from);
            }
            let Some(source) = model.get(from).cloned() else {
                return false;
            };
            if !parent_ok(model, to) || to.starts_with(&format!("{from}/")) {
                return false;
            }
            if let Some(existing) = model.get(to) {
                let prefix = format!("{to}/");
                let kinds_match = existing.is_none() == source.is_none();
                if !kinds_match || (existing.is_none() && model.keys().any(|k| k.starts_with(&prefix)))
                {
                    return false;
                }
            }
            let prefix = format!("{from}/");
            let moved: Vec<String> = model.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
            for key in moved {
                let value = model.remove(&key).unwrap();
                model.insert(format!("{to}{}", &key[from.len()..]), value);
            }
            model.remove(from);
            model.insert(to.clone(), source);
            true
        }
    }
}

fn write_into(bytes: &mut Vec<u8>, offset: u64, data: &[u8]) {
    let end = offset as usize + data.len();
    if bytes.len() < end {
        bytes.resize(end, 0);
    }
    bytes[offset as usize..end].copy_from_slice(data);
}

/// Applies an operation to the filesystem; `Ok(false)` when it was refused
/// with an ordinary error (as the model predicts).
fn fs_apply(fs: &mut Fs, op: &Op) -> Result<bool, Error> {
    let resolve = |fs: &mut Fs, path: &str| fs.resolve(path.as_bytes());
    let refused = |error: Error| {
        matches!(
            error,
            Error::NotFound
                | Error::Exists
                | Error::NotDirectory
                | Error::IsDirectory
                | Error::NotEmpty
                | Error::InvalidArgument
        )
    };
    let result = (|| -> Result<(), Error> {
        match op {
            Op::Mkdir(path) => {
                let (dir, name) = fs.resolve_parent(path.as_bytes())?;
                fs.create_dir(dir.ino, name).map(|_| ())
            }
            Op::Write(path, offset, data) => {
                let mut node = match resolve(fs, path) {
                    Ok(node) => node,
                    Err(Error::NotFound) => {
                        let (dir, name) = fs.resolve_parent(path.as_bytes())?;
                        fs.create_file(dir.ino, name)?
                    }
                    Err(other) => return Err(other),
                };
                fs.write_at(&mut node, *offset, data).map(|_| ())
            }
            Op::Truncate(path, length) => {
                let mut node = resolve(fs, path)?;
                fs.truncate(&mut node, *length)
            }
            Op::Remove(path) => {
                let node = resolve(fs, path)?;
                fs.remove(&node)
            }
            Op::Rename(from, to) => {
                let node = resolve(fs, from)?;
                let (dir, name) = fs.resolve_parent(to.as_bytes())?;
                fs.rename(&node, dir.ino, name).map(|_| ())
            }
        }
    })();
    match result {
        Ok(()) => Ok(true),
        Err(error) if refused(error) => Ok(false),
        Err(error) => Err(error),
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, limit: u64) -> u64 {
        self.next() % limit
    }
}

fn random_path(rng: &mut Rng) -> String {
    let depth = 1 + rng.below(3);
    let mut path = String::new();
    for _ in 0..depth {
        path.push('/');
        path.push_str(["a", "b", "c", "dd", "e.txt"][rng.below(5) as usize]);
    }
    path
}

fn random_op(rng: &mut Rng) -> Op {
    match rng.below(10) {
        0 | 1 => Op::Mkdir(random_path(rng)),
        2..=5 => {
            let length = match rng.below(4) {
                0 => rng.below(40),
                1 => rng.below(5000),
                2 => 4000 + rng.below(9000),
                _ => rng.below(200),
            } as usize;
            let offset = match rng.below(5) {
                0 => rng.below(20_000),
                1 => 2 * 1024 * 1024 + rng.below(4096) * 7,
                _ => rng.below(300),
            };
            let seed = rng.next();
            let data = (0..length).map(|i| (seed.wrapping_add(i as u64 * 31) >> 3) as u8).collect();
            Op::Write(random_path(rng), offset, data)
        }
        6 => Op::Truncate(random_path(rng), rng.below(30_000)),
        7 => Op::Remove(random_path(rng)),
        _ => Op::Rename(random_path(rng), random_path(rng)),
    }
}

#[test]
fn random_operations_match_a_model_and_survive_remount() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut fs = mounted(formatted(1024));
    let mut model = Model::new();
    let mut accepted = 0;
    for step in 0..4000 {
        let op = random_op(&mut rng);
        let expected = model_apply(&mut model, &op);
        let actual = fs_apply(&mut fs, &op).unwrap_or_else(|e| panic!("step {step} {op:?}: {e:?}"));
        assert_eq!(actual, expected, "step {step}: {op:?}");
        accepted += usize::from(expected);
        if step % 100 == 99 {
            assert_eq!(state(&mut fs), model, "state differs after step {step}");
            let report = fs.fsck();
            assert!(!report.damaged(), "fsck after step {step}: {report:?}");
        }
    }
    assert!(accepted > 700, "only {accepted} operations were accepted");
    assert_eq!(state(&mut fs), model);

    // Remount: same contents, and the allocation map rebuilt from the tree
    // matches the one kept at run time (no leaked or double-used blocks).
    let running_space = fs.space();
    let ram = fs.into_device();
    let mut again = mounted(ram);
    assert_eq!(state(&mut again), model);
    assert_eq!(again.space(), running_space);
    assert!(!again.fsck().damaged());
}

#[test]
fn files_grow_through_every_tree_height_and_shrink_back() {
    let mut fs = mounted(formatted(2048));
    let baseline = fs.space().1;
    let (dir, name) = fs.resolve_parent(b"/big").unwrap();
    let mut node = fs.create_file(dir.ino, name).unwrap();
    // 100 bytes at 3 MiB forces height 2; the hole before it reads as zeros.
    let tail = b"tail of a sparse file";
    fs.write_at(&mut node, 3 * 1024 * 1024, tail).unwrap();
    assert_eq!(node.size, 3 * 1024 * 1024 + tail.len() as u64);
    let mut hole = vec![1u8; 8192];
    assert_eq!(fs.read_at(&node, 1_000_000, &mut hole).unwrap(), 8192);
    assert!(hole.iter().all(|b| *b == 0));
    let mut back = vec![0u8; tail.len()];
    fs.read_at(&node, 3 * 1024 * 1024, &mut back).unwrap();
    assert_eq!(back, tail);
    // Sparse: only a handful of blocks are really used.
    assert!(baseline - fs.space().1 < 12, "used {}", baseline - fs.space().1);

    fs.truncate(&mut node, 5000).unwrap();
    assert_eq!(node.size, 5000);
    fs.write_at(&mut node, 5000, &[9u8; 100]).unwrap();
    let mut mid = [0xffu8; 200];
    fs.read_at(&node, 4900, &mut mid).unwrap();
    assert!(mid[..100].iter().all(|b| *b == 0), "tail of block 1 must be zero after the cut");
    assert!(mid[100..].iter().all(|b| *b == 9));

    fs.truncate(&mut node, 0).unwrap();
    fs.remove(&node).unwrap();
    assert_eq!(fs.space().1, baseline, "blocks leaked");
    assert!(!fs.fsck().damaged());
}

#[test]
fn a_flipped_bit_is_reported_not_returned() {
    let mut fs = mounted(formatted(256));
    let (dir, name) = fs.resolve_parent(b"/data").unwrap();
    let mut node = fs.create_file(dir.ino, name).unwrap();
    let payload: Vec<u8> = (0..9000u32).map(|i| (i * 7) as u8).collect();
    fs.write_at(&mut node, 0, &payload).unwrap();
    assert!(!fs.fsck().damaged());

    let ram = fs.into_device();
    // Find the file's data blocks by flipping each used block in turn: every
    // flip inside live metadata or data must show up in fsck or the read.
    let mut detected = 0;
    let mut live = 0;
    for block in 2..ram.blocks {
        let mut damaged = ram.clone();
        let at = block as usize * BLOCK + 100;
        let before = damaged.data[at];
        damaged.data[at] ^= 0x10;
        match AerFs::mount(damaged, clock) {
            Err((error, _)) => {
                assert!(matches!(error, Error::Corrupt | Error::Io), "{error:?}");
                detected += 1;
                live += 1;
            }
            Ok(mut fs) => {
                let report = fs.fsck();
                let node = fs.resolve(b"/data");
                let mut out = vec![0u8; payload.len()];
                let read = node.and_then(|n| fs.read_at(&n, 0, &mut out));
                if report.damaged() || read.is_err() {
                    detected += 1;
                    live += 1;
                    if let Ok(count) = read {
                        // A read that "succeeded" must not have changed a byte.
                        assert_eq!(&out[..count], &payload[..count]);
                    }
                } else if report.data_blocks > 0 && (block as usize) < 400 {
                    // Unused block: the flip changes nothing anyone can see.
                    assert_eq!(out, payload);
                }
            }
        }
        let _ = before;
    }
    assert!(live >= 3 && detected == live, "live {live}, detected {detected}");
}

#[test]
fn running_out_of_space_leaves_everything_intact() {
    let mut fs = mounted(formatted(96));
    let mut files = 0;
    let mut last_error = None;
    for index in 0.. {
        let name = format!("f{index}");
        let (dir, _) = fs.resolve_parent(b"/x").unwrap();
        let node = match fs.create_file(dir.ino, name.as_bytes()) {
            Ok(node) => node,
            Err(error) => {
                last_error = Some(error);
                break;
            }
        };
        let mut node = node;
        match fs.write_at(&mut node, 0, &vec![index as u8; 12_000]) {
            Ok(_) => files += 1,
            Err(error) => {
                last_error = Some(error);
                // The failed write must not have changed the file or leaked.
                let fresh = fs.resolve(name.as_bytes()).unwrap();
                assert_eq!(fresh.size, 0);
                break;
            }
        }
    }
    assert_eq!(last_error, Some(Error::NoSpace));
    assert!(files >= 5);
    let report = fs.fsck();
    assert!(!report.damaged(), "{report:?}");
    // Everything that was written is still right.
    for index in 0..files {
        let node = fs.resolve(format!("/f{index}").as_bytes()).unwrap();
        let mut out = vec![0u8; 12_000];
        assert_eq!(fs.read_at(&node, 0, &mut out).unwrap(), 12_000);
        assert!(out.iter().all(|b| *b == index as u8));
    }
    // Deleting frees space and writing works again.
    let node = fs.resolve(b"/f0").unwrap();
    fs.remove(&node).unwrap();
    let (dir, _) = fs.resolve_parent(b"/x").unwrap();
    let mut again = fs.create_file(dir.ino, b"again").unwrap();
    fs.write_at(&mut again, 0, &[5u8; 9000]).unwrap();
    assert!(!fs.fsck().damaged());
}

#[test]
fn rename_and_name_rules() {
    let mut fs = mounted(formatted(256));
    let root = fs.root().ino;
    let a = fs.create_dir(root, b"a").unwrap();
    let b = fs.create_dir(root, b"b").unwrap();
    let mut file = fs.create_file(a.ino, b"one").unwrap();
    fs.write_at(&mut file, 0, b"first").unwrap();
    let mut other = fs.create_file(b.ino, b"two").unwrap();
    fs.write_at(&mut other, 0, b"second").unwrap();

    // Replacing an existing file is one atomic step.
    let moved = fs.rename(&file, b.ino, b"two").unwrap();
    assert_eq!(fs.resolve(b"/a/one"), Err(Error::NotFound));
    let mut text = [0u8; 16];
    let n = fs.read_at(&moved, 0, &mut text).unwrap();
    assert_eq!(&text[..n], b"first");
    assert_eq!(fs.resolve(b"/b/two").unwrap().ino, file.ino);

    // A directory cannot go into itself or replace a file.
    assert_eq!(fs.rename(&a, a.ino, b"x"), Err(Error::InvalidArgument));
    let inner = fs.create_dir(a.ino, b"inner").unwrap();
    assert_eq!(fs.rename(&a, inner.ino, b"x"), Err(Error::InvalidArgument));
    assert!(fs.rename(&a, b.ino, b"two").is_err());
    // Non-empty directories cannot be removed or replaced.
    assert_eq!(fs.remove(&a), Err(Error::NotEmpty));
    assert_eq!(fs.rename(&b, root, b"a"), Err(Error::NotEmpty));

    let long = [b'x'; NAME_MAX];
    assert!(fs.create_file(root, &long).is_ok());
    assert_eq!(fs.create_file(root, &[b'y'; NAME_MAX + 1]), Err(Error::InvalidName));
    for bad in [&b""[..], b".", b"..", b"a/b", b"nul\0"] {
        assert_eq!(fs.create_file(root, bad), Err(Error::InvalidName));
    }
    assert_eq!(fs.create_file(root, b"a"), Err(Error::Exists));
    assert!(!fs.fsck().damaged());
}

/// A scripted sequence that touches multi-block files, the tree-height
/// change, overwrites, truncation, rename-over, and removal.
fn script() -> Vec<Op> {
    let blob = |seed: u8, length: usize| -> Vec<u8> {
        (0..length).map(|i| seed.wrapping_add((i * 13) as u8)).collect()
    };
    vec![
        Op::Mkdir("/docs".into()),
        Op::Write("/docs/a.txt".into(), 0, blob(1, 10_000)),
        Op::Write("/docs/b.txt".into(), 0, blob(2, 50)),
        Op::Mkdir("/docs/old".into()),
        Op::Write("/docs/a.txt".into(), 4090, blob(3, 300)),
        Op::Write("/big".into(), 2 * 1024 * 1024 + 100, blob(4, 700)),
        Op::Truncate("/docs/a.txt".into(), 5000),
        Op::Rename("/docs/b.txt".into(), "/docs/old/b.txt".into()),
        Op::Write("/docs/c.txt".into(), 0, blob(5, 20_000)),
        Op::Rename("/docs/c.txt".into(), "/docs/a.txt".into()),
        Op::Remove("/docs/old/b.txt".into()),
        Op::Remove("/docs/old".into()),
        Op::Truncate("/big".into(), 100),
        Op::Write("/docs/a.txt".into(), 19_990, blob(6, 40)),
        Op::Mkdir("/z".into()),
        Op::Rename("/z".into(), "/docs/z".into()),
        Op::Remove("/big".into()),
    ]
}

#[test]
fn power_cut_at_every_sector_write_leaves_a_whole_filesystem() {
    let base = formatted(1024);
    let ops = script();

    // Uncut run: the state after each operation and the writes it cost.
    let mut fs = mounted(base.clone());
    let mut model = Model::new();
    let mut states = vec![model.clone()];
    let mut starts = Vec::new();
    for op in &ops {
        starts.push(fs.device().sector_writes);
        assert!(model_apply(&mut model, op), "script op refused by model: {op:?}");
        assert_eq!(fs_apply(&mut fs, op), Ok(true), "script op failed: {op:?}");
        states.push(model.clone());
    }
    let total = fs.device().sector_writes;
    let first = starts[0];
    assert!(total - first > 150, "only {} writes", total - first);
    assert_eq!(state(&mut fs), model);

    let mut checked = 0;
    for cut in first..total {
        let mut ram = base.clone();
        ram.sector_writes = 0;
        ram.cut_after = Some(cut);
        let mut fs = mounted(ram);
        // Replay until the lights go out.
        let mut in_flight = ops.len();
        for (index, op) in ops.iter().enumerate() {
            if fs.device().powered_off() {
                in_flight = index.saturating_sub(1);
                break;
            }
            let _ = fs_apply(&mut fs, op);
            if fs.device().powered_off() {
                in_flight = index;
                break;
            }
        }
        let mut ram = fs.into_device();
        ram.cut_after = None; // power is back
        let mut after = mounted(ram);
        let report = after.fsck();
        assert!(!report.damaged(), "cut {cut}: fsck {report:?}");
        let found = state(&mut after);
        let before = &states[in_flight];
        let done = &states[(in_flight + 1).min(ops.len())];
        // Creating a missing file and writing it are two commits, so a cut
        // between them leaves the new file empty.
        let created_empty = match ops.get(in_flight) {
            Some(Op::Write(path, _, _)) if !before.contains_key(path) => {
                let mut half = before.clone();
                half.insert(path.clone(), Some(Vec::new()));
                Some(half)
            }
            _ => None,
        };
        let described = ops.get(in_flight).map(|op| match op {
            Op::Mkdir(p) => format!("mkdir {p}"),
            Op::Write(p, o, d) => format!("write {p} at {o} ({} bytes)", d.len()),
            Op::Truncate(p, l) => format!("truncate {p} to {l}"),
            Op::Remove(p) => format!("remove {p}"),
            Op::Rename(a, b) => format!("rename {a} to {b}"),
        });
        assert!(
            found == *before || found == *done || created_empty.as_ref() == Some(&found),
            "cut {cut} during op {in_flight} ({described:?}): recovered state is not the one before, between or after"
        );
        // And the recovered filesystem is usable.
        let mut node = after.resolve(b"/").unwrap();
        let _ = &mut node;
        let probe = after.create_file(after.root().ino, b"probe");
        assert!(probe.is_ok() || probe == Err(Error::Exists), "cut {cut}: {probe:?}");
        checked += 1;
    }
    assert!(checked > 150);
}
