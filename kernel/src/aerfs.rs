//! AerFS: a copy-on-write filesystem with checksummed block pointers.
//!
//! Nothing is ever overwritten in place. Every change writes new blocks (data,
//! the radix-tree nodes above them, the inode table blocks) into free space
//! and then commits by writing one 512-byte superblock sector, alternating
//! between two slots. A crash before that sector lands leaves the previous
//! tree exactly as it was; a crash after leaves the new one. Mount picks the
//! valid superblock with the highest generation, so there is no journal to
//! replay and no repair step. Every pointer carries the CRC-32 of the block it
//! points to, so a flipped bit is reported as an error rather than returned as
//! data.
//!
//! Layout (4 KiB blocks): blocks 0 and 1 are the two superblock slots. A file
//! (and the inode table, and every directory) is a radix tree of 512-way
//! pointer blocks over data blocks, with sparse holes. Inodes are 128 bytes in
//! the inode table; directories are files of 64-byte entries (58-byte names).
//! Free space is not stored: the allocation map is rebuilt at mount from the
//! metadata, so it can never disagree with the tree.
//!
//! Every public mutating call is one transaction, committed before it
//! returns, so `rename` (even over an existing file) is atomic.

// Some items exist for the host test harness (`tools/aerfs-host`) only.
#![allow(dead_code)]

use core::cmp::{max, min};

pub const BLOCK: usize = 4096;
pub const SECTOR: usize = 512;
pub const NAME_MAX: usize = 58;
pub const ROOT_INO: u32 = 1;
/// Largest volume: 65,536 blocks (256 MiB).
pub const MAX_BLOCKS: usize = 65_536;
const BITMAP_BYTES: usize = MAX_BLOCKS / 8;
const FANOUT: u64 = 512;
const MAX_HEIGHT: u8 = 3;
const INODE_SIZE: usize = 128;
const DIR_ENTRY: usize = 64;
const MAX_INODES: u32 = 65_535;
const MAGIC: [u8; 8] = *b"AERFS\x00\x00\x01";

const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 != 0 {
                (value >> 1) ^ 0xedb8_8320
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
};

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in data {
        crc = CRC_TABLE[((crc ^ u32::from(*byte)) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    NotFound,
    Exists,
    NotDirectory,
    IsDirectory,
    NotEmpty,
    NoSpace,
    InvalidName,
    InvalidArgument,
    TooBig,
    /// A block failed its checksum.
    Corrupt,
    Io,
    /// Not an AerFS volume.
    BadSuperblock,
}

pub trait Device {
    fn blocks(&self) -> u32;
    fn read_block(&mut self, block: u32, out: &mut [u8; BLOCK]) -> bool;
    /// May be torn at a sector boundary by a power cut.
    fn write_block(&mut self, block: u32, data: &[u8; BLOCK]) -> bool;
    /// The first sector of a block; a sector write is atomic.
    fn read_sector(&mut self, block: u32, out: &mut [u8; SECTOR]) -> bool;
    fn write_sector(&mut self, block: u32, data: &[u8; SECTOR]) -> bool;
    /// Everything written so far is on the medium.
    fn flush(&mut self) -> bool;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Ptr {
    block: u32,
    crc: u32,
}

impl Ptr {
    const NULL: Ptr = Ptr { block: 0, crc: 0 };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Tree {
    size: u64,
    height: u8,
    root: Ptr,
}

impl Tree {
    const EMPTY: Tree = Tree {
        size: 0,
        height: 0,
        root: Ptr::NULL,
    };
}

#[derive(Clone, Copy)]
struct Super {
    generation: u64,
    blocks: u32,
    inodes: Tree,
    label: [u8; 16],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    File,
    Directory,
}

const KIND_FREE: u8 = 0;
const KIND_FILE: u8 = 1;
const KIND_DIRECTORY: u8 = 2;

#[derive(Clone, Copy)]
struct Inode {
    kind: u8,
    read_only: bool,
    mode: u16,
    parent: u32,
    mtime: u64,
    tree: Tree,
}

impl Inode {
    const FREE: Inode = Inode {
        kind: KIND_FREE,
        read_only: false,
        mode: 0,
        parent: 0,
        mtime: 0,
        tree: Tree::EMPTY,
    };

    fn encode(&self) -> [u8; INODE_SIZE] {
        let mut out = [0u8; INODE_SIZE];
        out[0] = self.kind;
        out[1] = u8::from(self.read_only);
        out[2..4].copy_from_slice(&self.mode.to_le_bytes());
        out[4..8].copy_from_slice(&self.parent.to_le_bytes());
        out[8..16].copy_from_slice(&self.tree.size.to_le_bytes());
        out[16..24].copy_from_slice(&self.mtime.to_le_bytes());
        out[24] = self.tree.height;
        out[28..32].copy_from_slice(&self.tree.root.block.to_le_bytes());
        out[32..36].copy_from_slice(&self.tree.root.crc.to_le_bytes());
        out
    }

    fn decode(bytes: &[u8]) -> Self {
        let word = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        let mut size = [0u8; 8];
        size.copy_from_slice(&bytes[8..16]);
        let mut mtime = [0u8; 8];
        mtime.copy_from_slice(&bytes[16..24]);
        Self {
            kind: bytes[0],
            read_only: bytes[1] != 0,
            mode: u16::from_le_bytes([bytes[2], bytes[3]]),
            parent: word(4),
            mtime: u64::from_le_bytes(mtime),
            tree: Tree {
                size: u64::from_le_bytes(size),
                height: bytes[24],
                root: Ptr {
                    block: word(28),
                    crc: word(32),
                },
            },
        }
    }
}

/// What the filesystem knows about one file or directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    pub ino: u32,
    pub parent: u32,
    pub kind: Kind,
    pub mode: u16,
    pub size: u64,
    pub mtime: u64,
    pub read_only: bool,
}

impl Node {
    pub const EMPTY: Node = Node {
        ino: 0,
        parent: 0,
        kind: Kind::File,
        mode: 0,
        size: 0,
        mtime: 0,
        read_only: false,
    };

    pub fn is_directory(&self) -> bool {
        self.kind == Kind::Directory
    }
}

#[derive(Clone, Copy)]
pub struct Listed {
    pub node: Node,
    pub name: [u8; NAME_MAX],
    pub name_len: usize,
}

impl Listed {
    pub fn name(&self) -> &[u8] {
        &self.name[..self.name_len]
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct FsckReport {
    pub files: u32,
    pub directories: u32,
    pub data_blocks: u32,
    pub tree_blocks: u32,
    pub bad_checksums: u32,
    pub bad_entries: u32,
    pub double_references: u32,
    pub map_mismatches: u32,
    pub orphans: u32,
}

impl FsckReport {
    pub fn damaged(&self) -> bool {
        self.bad_checksums
            + self.bad_entries
            + self.double_references
            + self.map_mismatches
            + self.orphans
            != 0
    }
}

fn bit(map: &[u8], block: u32) -> bool {
    map[(block / 8) as usize] & (1 << (block % 8)) != 0
}

fn set_bit(map: &mut [u8], block: u32, value: bool) {
    let byte = &mut map[(block / 8) as usize];
    if value {
        *byte |= 1 << (block % 8);
    } else {
        *byte &= !(1 << (block % 8));
    }
}

fn get_ptr(block: &[u8; BLOCK], index: u64) -> Ptr {
    let at = index as usize * 8;
    Ptr {
        block: u32::from_le_bytes([block[at], block[at + 1], block[at + 2], block[at + 3]]),
        crc: u32::from_le_bytes([block[at + 4], block[at + 5], block[at + 6], block[at + 7]]),
    }
}

fn put_ptr(block: &mut [u8; BLOCK], index: u64, ptr: Ptr) {
    let at = index as usize * 8;
    block[at..at + 4].copy_from_slice(&ptr.block.to_le_bytes());
    block[at + 4..at + 8].copy_from_slice(&ptr.crc.to_le_bytes());
}

const fn capacity(height: u8) -> u64 {
    let mut blocks = 1u64;
    let mut level = 0;
    while level < height {
        blocks *= FANOUT;
        level += 1;
    }
    blocks
}

pub struct AerFs<D: Device> {
    device: D,
    committed: Super,
    working: Super,
    /// Blocks reachable from the committed tree (and the superblocks).
    used: [u8; BITMAP_BYTES],
    /// Blocks taken in the running transaction.
    fresh: [u8; BITMAP_BYTES],
    /// Committed blocks the running transaction has replaced.
    freed: [u8; BITMAP_BYTES],
    /// One buffer per tree level (0 is the data level).
    scratch: [[u8; BLOCK]; 4],
    hint: u32,
    clock: fn() -> u64,
}

impl<D: Device> AerFs<D> {
    pub const fn new_empty(device: D, clock: fn() -> u64) -> Self {
        Self {
            device,
            committed: Super {
                generation: 0,
                blocks: 0,
                inodes: Tree::EMPTY,
                label: [0; 16],
            },
            working: Super {
                generation: 0,
                blocks: 0,
                inodes: Tree::EMPTY,
                label: [0; 16],
            },
            used: [0; BITMAP_BYTES],
            fresh: [0; BITMAP_BYTES],
            freed: [0; BITMAP_BYTES],
            scratch: [[0; BLOCK]; 4],
            hint: 2,
            clock,
        }
    }

    pub fn into_device(self) -> D {
        self.device
    }

    pub fn device(&mut self) -> &mut D {
        &mut self.device
    }

    // ------------------------------------------------------- allocation

    fn alloc(&mut self) -> Result<u32, Error> {
        let total = self.working.blocks;
        if total <= 2 {
            return Err(Error::NoSpace);
        }
        let span = total - 2;
        for step in 0..span {
            let candidate = 2 + (self.hint.max(2) - 2 + step) % span;
            if !bit(&self.used, candidate) && !bit(&self.fresh, candidate) {
                set_bit(&mut self.fresh, candidate, true);
                self.hint = candidate + 1;
                return Ok(candidate);
            }
        }
        Err(Error::NoSpace)
    }

    fn release(&mut self, block: u32) {
        if block == 0 {
            return;
        }
        if bit(&self.fresh, block) {
            set_bit(&mut self.fresh, block, false);
        } else if bit(&self.used, block) {
            set_bit(&mut self.freed, block, true);
        }
    }

    fn rollback(&mut self) {
        self.working = self.committed;
        self.fresh.fill(0);
        self.freed.fill(0);
    }

    fn encode_super(sb: &Super) -> [u8; SECTOR] {
        let mut out = [0u8; SECTOR];
        out[..8].copy_from_slice(&MAGIC);
        out[8..12].copy_from_slice(&1u32.to_le_bytes());
        out[12..20].copy_from_slice(&sb.generation.to_le_bytes());
        out[20..24].copy_from_slice(&sb.blocks.to_le_bytes());
        out[32..40].copy_from_slice(&sb.inodes.size.to_le_bytes());
        out[40] = sb.inodes.height;
        out[44..48].copy_from_slice(&sb.inodes.root.block.to_le_bytes());
        out[48..52].copy_from_slice(&sb.inodes.root.crc.to_le_bytes());
        out[52..68].copy_from_slice(&sb.label);
        let crc = crc32(&out[..508]);
        out[508..512].copy_from_slice(&crc.to_le_bytes());
        out
    }

    fn decode_super(sector: &[u8; SECTOR]) -> Option<Super> {
        if sector[..8] != MAGIC {
            return None;
        }
        let crc = u32::from_le_bytes([sector[508], sector[509], sector[510], sector[511]]);
        if crc != crc32(&sector[..508]) {
            return None;
        }
        let word = |at: usize| {
            u32::from_le_bytes([sector[at], sector[at + 1], sector[at + 2], sector[at + 3]])
        };
        let mut generation = [0u8; 8];
        generation.copy_from_slice(&sector[12..20]);
        let mut size = [0u8; 8];
        size.copy_from_slice(&sector[32..40]);
        let mut label = [0u8; 16];
        label.copy_from_slice(&sector[52..68]);
        Some(Super {
            generation: u64::from_le_bytes(generation),
            blocks: word(20),
            inodes: Tree {
                size: u64::from_le_bytes(size),
                height: sector[40],
                root: Ptr {
                    block: word(44),
                    crc: word(48),
                },
            },
            label,
        })
    }

    /// Ends the running transaction: all new blocks are flushed, then one
    /// superblock sector makes them the filesystem.
    fn commit(&mut self) -> Result<(), Error> {
        let mut next = self.working;
        next.generation = self.committed.generation + 1;
        if !self.device.flush() {
            self.rollback();
            return Err(Error::Io);
        }
        let sector = Self::encode_super(&next);
        let slot = (next.generation & 1) as u32;
        if !self.device.write_sector(slot, &sector) || !self.device.flush() {
            self.rollback();
            return Err(Error::Io);
        }
        for index in 0..BITMAP_BYTES {
            self.used[index] = (self.used[index] | self.fresh[index]) & !self.freed[index];
            self.fresh[index] = 0;
            self.freed[index] = 0;
        }
        self.committed = next;
        self.working = next;
        Ok(())
    }

    /// Runs `body` as one transaction.
    fn transaction<T>(
        &mut self,
        body: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        match body(self).and_then(|value| self.commit().map(|()| value)) {
            Ok(value) => Ok(value),
            Err(error) => {
                self.rollback();
                Err(error)
            }
        }
    }

    // ------------------------------------------------------- block I/O

    fn read_checked(&mut self, ptr: Ptr, level: usize) -> Result<(), Error> {
        if ptr.block < 2 || ptr.block >= self.working.blocks {
            return Err(Error::Corrupt);
        }
        if !self.device.read_block(ptr.block, &mut self.scratch[level]) {
            return Err(Error::Io);
        }
        if crc32(&self.scratch[level]) != ptr.crc {
            return Err(Error::Corrupt);
        }
        Ok(())
    }

    fn write_new(&mut self, level: usize) -> Result<Ptr, Error> {
        let block = self.alloc()?;
        if !self.device.write_block(block, &self.scratch[level]) {
            return Err(Error::Io);
        }
        Ok(Ptr {
            block,
            crc: crc32(&self.scratch[level]),
        })
    }

    // ------------------------------------------------------------ trees

    fn leaf_pointer(&mut self, tree: &Tree, index: u64) -> Result<Ptr, Error> {
        if index >= capacity(tree.height) {
            return Ok(Ptr::NULL);
        }
        let mut ptr = tree.root;
        let mut level = tree.height;
        while level > 0 {
            if ptr.block == 0 {
                return Ok(Ptr::NULL);
            }
            self.read_checked(ptr, level as usize)?;
            let digit = (index >> (9 * u32::from(level - 1))) & (FANOUT - 1);
            ptr = get_ptr(&self.scratch[level as usize], digit);
            level -= 1;
        }
        Ok(ptr)
    }

    fn tree_read(&mut self, tree: &Tree, offset: u64, out: &mut [u8]) -> Result<usize, Error> {
        if offset >= tree.size {
            return Ok(0);
        }
        let count = min(out.len() as u64, tree.size - offset) as usize;
        let mut done = 0;
        while done < count {
            let position = offset + done as u64;
            let index = position / BLOCK as u64;
            let within = (position % BLOCK as u64) as usize;
            let take = min(BLOCK - within, count - done);
            let ptr = self.leaf_pointer(tree, index)?;
            if ptr.block == 0 {
                out[done..done + take].fill(0);
            } else {
                self.read_checked(ptr, 0)?;
                out[done..done + take].copy_from_slice(&self.scratch[0][within..within + take]);
            }
            done += take;
        }
        Ok(count)
    }

    fn grow(&mut self, tree: &mut Tree, blocks: u64) -> Result<(), Error> {
        while capacity(tree.height) < blocks {
            if tree.height == MAX_HEIGHT {
                return Err(Error::TooBig);
            }
            if tree.root.block != 0 {
                let level = tree.height as usize + 1;
                self.scratch[level].fill(0);
                put_ptr(&mut self.scratch[level], 0, tree.root);
                tree.root = self.write_new(level)?;
            }
            tree.height += 1;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn write_node(
        &mut self,
        ptr: Ptr,
        level: u8,
        first: u64,
        lo: u64,
        hi: u64,
        data: &[u8],
        data_start: u64,
    ) -> Result<Ptr, Error> {
        if level == 0 {
            let block_start = first * BLOCK as u64;
            let from = max(data_start, block_start);
            let to = min(data_start + data.len() as u64, block_start + BLOCK as u64);
            let covers = from == block_start && to == block_start + BLOCK as u64;
            if !covers {
                if ptr.block != 0 {
                    self.read_checked(ptr, 0)?;
                } else {
                    self.scratch[0].fill(0);
                }
            }
            let source = &data[(from - data_start) as usize..(to - data_start) as usize];
            self.scratch[0][(from - block_start) as usize..(to - block_start) as usize]
                .copy_from_slice(source);
            let fresh = self.write_new(0)?;
            self.release(ptr.block);
            return Ok(fresh);
        }
        if ptr.block != 0 {
            self.read_checked(ptr, level as usize)?;
        } else {
            self.scratch[level as usize].fill(0);
        }
        let span = capacity(level - 1);
        for child in 0..FANOUT {
            let child_first = first + child * span;
            if child_first + span <= lo || child_first >= hi {
                continue;
            }
            let old = get_ptr(&self.scratch[level as usize], child);
            let new = self.write_node(old, level - 1, child_first, lo, hi, data, data_start)?;
            put_ptr(&mut self.scratch[level as usize], child, new);
        }
        let fresh = self.write_new(level as usize)?;
        self.release(ptr.block);
        Ok(fresh)
    }

    fn tree_write(&mut self, tree: &mut Tree, offset: u64, data: &[u8]) -> Result<(), Error> {
        if data.is_empty() {
            return Ok(());
        }
        let end = offset + data.len() as u64;
        let last = (end - 1) / BLOCK as u64;
        self.grow(tree, last + 1)?;
        let first = offset / BLOCK as u64;
        tree.root = self.write_node(tree.root, tree.height, 0, first, last + 1, data, offset)?;
        tree.size = max(tree.size, end);
        Ok(())
    }

    fn free_subtree(&mut self, ptr: Ptr, level: u8) -> Result<(), Error> {
        if ptr.block == 0 {
            return Ok(());
        }
        if level > 0 {
            self.read_checked(ptr, level as usize)?;
            for child in 0..FANOUT {
                let pointer = get_ptr(&self.scratch[level as usize], child);
                if pointer.block != 0 {
                    self.free_subtree(pointer, level - 1)?;
                }
            }
        }
        self.release(ptr.block);
        Ok(())
    }

    fn truncate_node(
        &mut self,
        ptr: Ptr,
        level: u8,
        first: u64,
        keep: u64,
        new_size: u64,
    ) -> Result<Ptr, Error> {
        if ptr.block == 0 {
            return Ok(Ptr::NULL);
        }
        if first >= keep {
            self.free_subtree(ptr, level)?;
            return Ok(Ptr::NULL);
        }
        if level == 0 {
            let block_start = first * BLOCK as u64;
            if new_size < block_start + BLOCK as u64 {
                self.read_checked(ptr, 0)?;
                let from = (new_size - block_start) as usize;
                self.scratch[0][from..].fill(0);
                let fresh = self.write_new(0)?;
                self.release(ptr.block);
                return Ok(fresh);
            }
            return Ok(ptr);
        }
        let span = capacity(level);
        let tail_inside = !new_size.is_multiple_of(BLOCK as u64) && keep - 1 < first + span;
        if first + span <= keep && !tail_inside {
            return Ok(ptr);
        }
        self.read_checked(ptr, level as usize)?;
        let child_span = capacity(level - 1);
        let mut changed = false;
        let mut remaining = false;
        for child in 0..FANOUT {
            let old = get_ptr(&self.scratch[level as usize], child);
            if old.block == 0 {
                continue;
            }
            let new =
                self.truncate_node(old, level - 1, first + child * child_span, keep, new_size)?;
            if new != old {
                put_ptr(&mut self.scratch[level as usize], child, new);
                changed = true;
            }
            remaining |= new.block != 0;
        }
        if !remaining {
            self.release(ptr.block);
            return Ok(Ptr::NULL);
        }
        if changed {
            let fresh = self.write_new(level as usize)?;
            self.release(ptr.block);
            return Ok(fresh);
        }
        Ok(ptr)
    }

    fn tree_truncate(&mut self, tree: &mut Tree, new_size: u64) -> Result<(), Error> {
        if new_size >= tree.size {
            tree.size = new_size;
            return Ok(());
        }
        let keep = new_size.div_ceil(BLOCK as u64);
        tree.root = self.truncate_node(tree.root, tree.height, 0, keep, new_size)?;
        tree.size = new_size;
        while tree.height > 0 && keep <= capacity(tree.height - 1) {
            if tree.root.block == 0 {
                tree.height = 0;
                break;
            }
            self.read_checked(tree.root, tree.height as usize)?;
            let child = get_ptr(&self.scratch[tree.height as usize], 0);
            self.release(tree.root.block);
            tree.root = child;
            tree.height -= 1;
        }
        if tree.root.block == 0 {
            tree.height = 0;
        }
        Ok(())
    }

    // ----------------------------------------------------------- inodes

    fn get_inode(&mut self, ino: u32) -> Result<Inode, Error> {
        if ino == 0 || ino > MAX_INODES {
            return Err(Error::NotFound);
        }
        let mut raw = [0u8; INODE_SIZE];
        let tree = self.working.inodes;
        let read = self.tree_read(&tree, u64::from(ino) * INODE_SIZE as u64, &mut raw)?;
        if read < INODE_SIZE {
            return Ok(Inode::FREE);
        }
        Ok(Inode::decode(&raw))
    }

    fn put_inode(&mut self, ino: u32, inode: &Inode) -> Result<(), Error> {
        let mut tree = self.working.inodes;
        self.tree_write(
            &mut tree,
            u64::from(ino) * INODE_SIZE as u64,
            &inode.encode(),
        )?;
        self.working.inodes = tree;
        Ok(())
    }

    fn live_inode(&mut self, ino: u32) -> Result<Inode, Error> {
        let inode = self.get_inode(ino)?;
        if inode.kind == KIND_FREE {
            Err(Error::NotFound)
        } else {
            Ok(inode)
        }
    }

    fn node_of(ino: u32, inode: &Inode) -> Node {
        Node {
            ino,
            parent: inode.parent,
            kind: if inode.kind == KIND_DIRECTORY {
                Kind::Directory
            } else {
                Kind::File
            },
            mode: inode.mode,
            size: inode.tree.size,
            mtime: inode.mtime,
            read_only: inode.read_only,
        }
    }

    fn free_inode_number(&mut self) -> Result<u32, Error> {
        let slots = self.working.inodes.size / INODE_SIZE as u64;
        for ino in 2..slots.min(u64::from(MAX_INODES) + 1) as u32 {
            if self.get_inode(ino)?.kind == KIND_FREE {
                return Ok(ino);
            }
        }
        let next = max(slots, 2) as u32;
        if next > MAX_INODES {
            return Err(Error::NoSpace);
        }
        Ok(next)
    }

    // ------------------------------------------------------ directories

    /// Finds an entry by name: `(ino, kind, byte offset in the directory)`.
    fn dir_find(&mut self, dir: &Inode, name: &[u8]) -> Result<Option<(u32, u8, u64)>, Error> {
        let tree = dir.tree;
        let mut offset = 0;
        let mut entry = [0u8; DIR_ENTRY];
        while offset < tree.size {
            self.tree_read(&tree, offset, &mut entry)?;
            let ino = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
            let length = usize::from(entry[4]);
            if ino != 0 && length == name.len() && entry[6..6 + length] == *name {
                return Ok(Some((ino, entry[5], offset)));
            }
            offset += DIR_ENTRY as u64;
        }
        Ok(None)
    }

    fn dir_find_ino(&mut self, dir: &Inode, ino: u32) -> Result<Option<u64>, Error> {
        let tree = dir.tree;
        let mut offset = 0;
        let mut entry = [0u8; DIR_ENTRY];
        while offset < tree.size {
            self.tree_read(&tree, offset, &mut entry)?;
            if u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]) == ino {
                return Ok(Some(offset));
            }
            offset += DIR_ENTRY as u64;
        }
        Ok(None)
    }

    fn dir_is_empty_inode(&mut self, dir: &Inode) -> Result<bool, Error> {
        let tree = dir.tree;
        let mut offset = 0;
        let mut entry = [0u8; DIR_ENTRY];
        while offset < tree.size {
            self.tree_read(&tree, offset, &mut entry)?;
            if u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]) != 0 {
                return Ok(false);
            }
            offset += DIR_ENTRY as u64;
        }
        Ok(true)
    }

    /// Adds an entry to `dir_ino` (reusing a free slot when there is one).
    fn dir_insert(&mut self, dir_ino: u32, name: &[u8], ino: u32, kind: u8) -> Result<(), Error> {
        let mut dir = self.live_inode(dir_ino)?;
        let mut tree = dir.tree;
        let mut slot = tree.size;
        let mut offset = 0;
        let mut existing = [0u8; DIR_ENTRY];
        while offset < tree.size {
            self.tree_read(&tree, offset, &mut existing)?;
            if u32::from_le_bytes([existing[0], existing[1], existing[2], existing[3]]) == 0 {
                slot = offset;
                break;
            }
            offset += DIR_ENTRY as u64;
        }
        let mut entry = [0u8; DIR_ENTRY];
        entry[..4].copy_from_slice(&ino.to_le_bytes());
        entry[4] = name.len() as u8;
        entry[5] = kind;
        entry[6..6 + name.len()].copy_from_slice(name);
        self.tree_write(&mut tree, slot, &entry)?;
        dir.tree = tree;
        dir.mtime = (self.clock)();
        self.put_inode(dir_ino, &dir)
    }

    /// Clears an entry and gives back any run of free slots at the end of
    /// the directory, so a directory that had many entries shrinks again.
    fn dir_clear(&mut self, dir_ino: u32, offset: u64) -> Result<(), Error> {
        let mut dir = self.live_inode(dir_ino)?;
        let mut tree = dir.tree;
        self.tree_write(&mut tree, offset, &[0u8; DIR_ENTRY])?;
        let mut size = tree.size;
        let mut entry = [0u8; DIR_ENTRY];
        while size >= DIR_ENTRY as u64 {
            self.tree_read(&tree, size - DIR_ENTRY as u64, &mut entry)?;
            if u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]) != 0 {
                break;
            }
            size -= DIR_ENTRY as u64;
        }
        if size != tree.size {
            self.tree_truncate(&mut tree, size)?;
        }
        dir.tree = tree;
        dir.mtime = (self.clock)();
        self.put_inode(dir_ino, &dir)
    }

    fn valid_name(name: &[u8]) -> bool {
        !name.is_empty()
            && name.len() <= NAME_MAX
            && name != b"."
            && name != b".."
            && !name.iter().any(|byte| *byte == b'/' || *byte == 0)
    }

    // ---------------------------------------------------- public queries

    pub fn root(&self) -> Node {
        Node {
            ino: ROOT_INO,
            parent: ROOT_INO,
            kind: Kind::Directory,
            mode: 0o755,
            size: 0,
            mtime: 0,
            read_only: false,
        }
    }

    pub fn refresh(&mut self, node: &Node) -> Result<Node, Error> {
        let inode = self.live_inode(node.ino)?;
        Ok(Self::node_of(node.ino, &inode))
    }

    pub fn find(&mut self, dir: u32, name: &[u8]) -> Result<Node, Error> {
        let directory = self.live_inode(dir)?;
        if directory.kind != KIND_DIRECTORY {
            return Err(Error::NotDirectory);
        }
        match self.dir_find(&directory, name)? {
            Some((ino, _, _)) => {
                let inode = self.live_inode(ino)?;
                Ok(Self::node_of(ino, &inode))
            }
            None => Err(Error::NotFound),
        }
    }

    pub fn resolve(&mut self, path: &[u8]) -> Result<Node, Error> {
        let mut node = {
            let root = self.live_inode(ROOT_INO)?;
            Self::node_of(ROOT_INO, &root)
        };
        for part in path
            .split(|byte| *byte == b'/')
            .filter(|part| !part.is_empty())
        {
            if node.kind != Kind::Directory {
                return Err(Error::NotDirectory);
            }
            node = self.find(node.ino, part)?;
        }
        Ok(node)
    }

    pub fn resolve_parent<'a>(&mut self, path: &'a [u8]) -> Result<(Node, &'a [u8]), Error> {
        let trimmed = path.strip_suffix(b"/").unwrap_or(path);
        let split = trimmed.iter().rposition(|byte| *byte == b'/');
        let (parent, name) = match split {
            Some(at) => (&trimmed[..at], &trimmed[at + 1..]),
            None => (&b""[..], trimmed),
        };
        let directory = self.resolve(parent)?;
        if directory.kind != Kind::Directory {
            return Err(Error::NotDirectory);
        }
        Ok((directory, name))
    }

    pub fn list_next(&mut self, dir: u32, cursor: &mut u32) -> Result<Option<Listed>, Error> {
        let directory = self.live_inode(dir)?;
        if directory.kind != KIND_DIRECTORY {
            return Err(Error::NotDirectory);
        }
        let tree = directory.tree;
        let mut entry = [0u8; DIR_ENTRY];
        while u64::from(*cursor) * (DIR_ENTRY as u64) < tree.size {
            self.tree_read(&tree, u64::from(*cursor) * DIR_ENTRY as u64, &mut entry)?;
            *cursor += 1;
            let ino = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
            if ino == 0 {
                continue;
            }
            let inode = self.live_inode(ino)?;
            let length = usize::from(entry[4]).min(NAME_MAX);
            let mut name = [0u8; NAME_MAX];
            name[..length].copy_from_slice(&entry[6..6 + length]);
            return Ok(Some(Listed {
                node: Self::node_of(ino, &inode),
                name,
                name_len: length,
            }));
        }
        Ok(None)
    }

    pub fn read_at(&mut self, node: &Node, offset: u64, out: &mut [u8]) -> Result<usize, Error> {
        let inode = self.live_inode(node.ino)?;
        if inode.kind == KIND_DIRECTORY {
            return Err(Error::IsDirectory);
        }
        self.tree_read(&inode.tree, offset, out)
    }

    pub fn label(&self) -> &[u8; 16] {
        &self.working.label
    }

    /// `(total blocks, free blocks)`.
    pub fn space(&self) -> (u32, u32) {
        let total = self.working.blocks;
        let mut free = 0;
        for block in 2..total {
            if !bit(&self.used, block) && !bit(&self.fresh, block) {
                free += 1;
            }
        }
        (total, free)
    }

    // -------------------------------------------------- public mutations

    fn create(&mut self, dir: u32, name: &[u8], kind: u8) -> Result<Node, Error> {
        if !Self::valid_name(name) {
            return Err(Error::InvalidName);
        }
        self.transaction(|fs| {
            let directory = fs.live_inode(dir)?;
            if directory.kind != KIND_DIRECTORY {
                return Err(Error::NotDirectory);
            }
            if fs.dir_find(&directory, name)?.is_some() {
                return Err(Error::Exists);
            }
            let ino = fs.free_inode_number()?;
            let inode = Inode {
                kind,
                read_only: false,
                mode: if kind == KIND_DIRECTORY { 0o755 } else { 0o644 },
                parent: dir,
                mtime: (fs.clock)(),
                tree: Tree::EMPTY,
            };
            fs.put_inode(ino, &inode)?;
            fs.dir_insert(dir, name, ino, kind)?;
            Ok(Self::node_of(ino, &inode))
        })
    }

    pub fn create_file(&mut self, dir: u32, name: &[u8]) -> Result<Node, Error> {
        self.create(dir, name, KIND_FILE)
    }

    pub fn create_dir(&mut self, dir: u32, name: &[u8]) -> Result<Node, Error> {
        self.create(dir, name, KIND_DIRECTORY)
    }

    pub fn write_at(&mut self, node: &mut Node, offset: u64, data: &[u8]) -> Result<usize, Error> {
        let ino = node.ino;
        let updated = self.transaction(|fs| {
            let mut inode = fs.live_inode(ino)?;
            if inode.kind == KIND_DIRECTORY {
                return Err(Error::IsDirectory);
            }
            if inode.read_only {
                return Err(Error::InvalidArgument);
            }
            let mut tree = inode.tree;
            fs.tree_write(&mut tree, offset, data)?;
            inode.tree = tree;
            inode.mtime = (fs.clock)();
            fs.put_inode(ino, &inode)?;
            Ok(Self::node_of(ino, &inode))
        })?;
        *node = updated;
        Ok(data.len())
    }

    pub fn truncate(&mut self, node: &mut Node, length: u64) -> Result<(), Error> {
        let ino = node.ino;
        let updated = self.transaction(|fs| {
            let mut inode = fs.live_inode(ino)?;
            if inode.kind == KIND_DIRECTORY {
                return Err(Error::IsDirectory);
            }
            if inode.read_only {
                return Err(Error::InvalidArgument);
            }
            let mut tree = inode.tree;
            fs.tree_truncate(&mut tree, length)?;
            inode.tree = tree;
            inode.mtime = (fs.clock)();
            fs.put_inode(ino, &inode)?;
            Ok(Self::node_of(ino, &inode))
        })?;
        *node = updated;
        Ok(())
    }

    pub fn set_read_only(&mut self, node: &mut Node, read_only: bool) -> Result<(), Error> {
        let ino = node.ino;
        let updated = self.transaction(|fs| {
            let mut inode = fs.live_inode(ino)?;
            inode.read_only = read_only;
            fs.put_inode(ino, &inode)?;
            Ok(Self::node_of(ino, &inode))
        })?;
        *node = updated;
        Ok(())
    }

    pub fn set_mode(&mut self, node: &mut Node, mode: u16) -> Result<(), Error> {
        let ino = node.ino;
        let updated = self.transaction(|fs| {
            let mut inode = fs.live_inode(ino)?;
            inode.mode = mode & 0o7777;
            fs.put_inode(ino, &inode)?;
            Ok(Self::node_of(ino, &inode))
        })?;
        *node = updated;
        Ok(())
    }

    pub fn dir_is_empty(&mut self, dir: u32) -> Result<bool, Error> {
        let inode = self.live_inode(dir)?;
        if inode.kind != KIND_DIRECTORY {
            return Err(Error::NotDirectory);
        }
        self.dir_is_empty_inode(&inode)
    }

    fn drop_inode(&mut self, ino: u32, inode: &Inode) -> Result<(), Error> {
        self.free_subtree(inode.tree.root, inode.tree.height)?;
        self.put_inode(ino, &Inode::FREE)
    }

    pub fn remove(&mut self, node: &Node) -> Result<(), Error> {
        let ino = node.ino;
        if ino == ROOT_INO {
            return Err(Error::InvalidArgument);
        }
        self.transaction(|fs| {
            let inode = fs.live_inode(ino)?;
            if inode.read_only {
                return Err(Error::InvalidArgument);
            }
            if inode.kind == KIND_DIRECTORY && !fs.dir_is_empty_inode(&inode)? {
                return Err(Error::NotEmpty);
            }
            let parent = fs.live_inode(inode.parent)?;
            let offset = fs.dir_find_ino(&parent, ino)?.ok_or(Error::Corrupt)?;
            fs.dir_clear(inode.parent, offset)?;
            fs.drop_inode(ino, &inode)
        })
    }

    fn is_inside(&mut self, mut candidate: u32, ancestor: u32) -> Result<bool, Error> {
        for _ in 0..4096 {
            if candidate == ancestor {
                return Ok(true);
            }
            if candidate == ROOT_INO {
                return Ok(false);
            }
            candidate = self.live_inode(candidate)?.parent;
        }
        Err(Error::Corrupt)
    }

    /// Moves `node` to `new_dir`/`new_name`, replacing an existing file (or
    /// an empty directory with a directory) atomically.
    pub fn rename(&mut self, node: &Node, new_dir: u32, new_name: &[u8]) -> Result<Node, Error> {
        if !Self::valid_name(new_name) {
            return Err(Error::InvalidName);
        }
        let ino = node.ino;
        if ino == ROOT_INO {
            return Err(Error::InvalidArgument);
        }
        self.transaction(|fs| {
            let mut inode = fs.live_inode(ino)?;
            let target_dir = fs.live_inode(new_dir)?;
            if target_dir.kind != KIND_DIRECTORY {
                return Err(Error::NotDirectory);
            }
            if inode.kind == KIND_DIRECTORY && fs.is_inside(new_dir, ino)? {
                return Err(Error::InvalidArgument);
            }
            if let Some((existing, existing_kind, _)) = fs.dir_find(&target_dir, new_name)? {
                if existing == ino {
                    return Ok(Self::node_of(ino, &inode));
                }
                if existing_kind != inode.kind {
                    return Err(if existing_kind == KIND_DIRECTORY {
                        Error::IsDirectory
                    } else {
                        Error::NotDirectory
                    });
                }
                let victim = fs.live_inode(existing)?;
                if victim.read_only {
                    return Err(Error::InvalidArgument);
                }
                if victim.kind == KIND_DIRECTORY && !fs.dir_is_empty_inode(&victim)? {
                    return Err(Error::NotEmpty);
                }
                let current_dir = fs.live_inode(new_dir)?;
                let at = fs
                    .dir_find_ino(&current_dir, existing)?
                    .ok_or(Error::Corrupt)?;
                fs.dir_clear(new_dir, at)?;
                fs.drop_inode(existing, &victim)?;
            }
            let old_parent = fs.live_inode(inode.parent)?;
            let offset = fs.dir_find_ino(&old_parent, ino)?.ok_or(Error::Corrupt)?;
            fs.dir_clear(inode.parent, offset)?;
            fs.dir_insert(new_dir, new_name, ino, inode.kind)?;
            inode = fs.live_inode(ino)?;
            inode.parent = new_dir;
            fs.put_inode(ino, &inode)?;
            Ok(Self::node_of(ino, &inode))
        })
    }

    // -------------------------------------------------- format and mount

    /// Formats the device this object holds and leaves it mounted and empty.
    pub fn format_here(&mut self, label: &[u8]) -> Result<(), Error> {
        let blocks = self.device.blocks();
        if blocks < 16 || blocks as usize > MAX_BLOCKS {
            return Err(Error::InvalidArgument);
        }
        let zero = [0u8; SECTOR];
        if !self.device.write_sector(0, &zero) || !self.device.write_sector(1, &zero) {
            return Err(Error::Io);
        }
        self.used.fill(0);
        self.fresh.fill(0);
        self.freed.fill(0);
        self.hint = 2;
        self.working = Super {
            generation: 0,
            blocks,
            inodes: Tree::EMPTY,
            label: [0; 16],
        };
        let count = min(label.len(), 16);
        self.working.label[..count].copy_from_slice(&label[..count]);
        self.committed = self.working;
        set_bit(&mut self.used, 0, true);
        set_bit(&mut self.used, 1, true);
        self.transaction(|fs| {
            let root = Inode {
                kind: KIND_DIRECTORY,
                read_only: false,
                mode: 0o755,
                parent: ROOT_INO,
                mtime: (fs.clock)(),
                tree: Tree::EMPTY,
            };
            fs.put_inode(ROOT_INO, &root)
        })
    }

    pub fn format(device: D, label: &[u8], clock: fn() -> u64) -> Result<D, Error> {
        let mut fs = Self::new_empty(device, clock);
        fs.format_here(label)?;
        Ok(fs.into_device())
    }

    /// Swaps the device (used by storage that lives in a static slot).
    pub fn replace_device(&mut self, device: D) -> D {
        core::mem::replace(&mut self.device, device)
    }

    /// Reads the newest valid superblock and rebuilds the allocation map.
    pub fn mount(device: D, clock: fn() -> u64) -> Result<Self, (Error, D)> {
        let mut fs = Self::new_empty(device, clock);
        match fs.load() {
            Ok(()) => Ok(fs),
            Err(error) => Err((error, fs.device)),
        }
    }

    /// Mounts the device this object holds.
    pub fn load(&mut self) -> Result<(), Error> {
        let mut best: Option<Super> = None;
        for slot in 0..2 {
            let mut sector = [0u8; SECTOR];
            if self.device.read_sector(slot, &mut sector)
                && let Some(candidate) = Self::decode_super(&sector)
                && candidate.blocks <= self.device.blocks()
                && candidate.blocks as usize <= MAX_BLOCKS
                && candidate.blocks >= 16
                && best.is_none_or(|current| candidate.generation > current.generation)
            {
                best = Some(candidate);
            }
        }
        let Some(sb) = best else {
            return Err(Error::BadSuperblock);
        };
        self.committed = sb;
        self.working = sb;
        self.used.fill(0);
        self.fresh.fill(0);
        self.freed.fill(0);
        self.hint = 2;
        set_bit(&mut self.used, 0, true);
        set_bit(&mut self.used, 1, true);
        self.scan()
    }

    /// True if `sector` is the first sector of a valid AerFS superblock
    /// (used to recognise a volume before mounting it).
    pub fn is_superblock(sector: &[u8; SECTOR]) -> bool {
        Self::decode_super(sector).is_some()
    }

    fn mark_node(&mut self, ptr: Ptr, level: u8, verify_data: bool) -> Result<(), Error> {
        if ptr.block == 0 {
            return Ok(());
        }
        if ptr.block < 2 || ptr.block >= self.working.blocks {
            return Err(Error::Corrupt);
        }
        set_bit(&mut self.used, ptr.block, true);
        if level > 0 {
            self.read_checked(ptr, level as usize)?;
            for child in 0..FANOUT {
                let pointer = get_ptr(&self.scratch[level as usize], child);
                self.mark_node(pointer, level - 1, verify_data)?;
            }
        } else if verify_data {
            self.read_checked(ptr, 0)?;
        }
        Ok(())
    }

    /// Marks every block of the committed tree (reading the metadata, not the
    /// file data).
    fn scan(&mut self) -> Result<(), Error> {
        let table = self.working.inodes;
        self.mark_node(table.root, table.height, false)?;
        let slots = table.size / INODE_SIZE as u64;
        for ino in 1..slots.min(u64::from(MAX_INODES) + 1) as u32 {
            let inode = self.get_inode(ino)?;
            if inode.kind != KIND_FREE {
                self.mark_node(inode.tree.root, inode.tree.height, false)?;
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------- fsck

    fn check_node(
        &mut self,
        ptr: Ptr,
        level: u8,
        seen: &mut [u8; BITMAP_BYTES],
        report: &mut FsckReport,
    ) {
        if ptr.block == 0 {
            return;
        }
        if ptr.block < 2 || ptr.block >= self.working.blocks {
            report.bad_checksums += 1;
            return;
        }
        if bit(seen, ptr.block) {
            report.double_references += 1;
            return;
        }
        set_bit(seen, ptr.block, true);
        if self.read_checked(ptr, level as usize).is_err() {
            report.bad_checksums += 1;
            return;
        }
        if level == 0 {
            report.data_blocks += 1;
            return;
        }
        report.tree_blocks += 1;
        for child in 0..FANOUT {
            let pointer = get_ptr(&self.scratch[level as usize], child);
            self.check_node(pointer, level - 1, seen, report);
        }
    }

    /// Reads every block and cross-checks the tree: checksums, blocks
    /// referenced twice, directory entries, unreachable inodes, and the
    /// allocation map against what is reachable.
    pub fn fsck(&mut self) -> FsckReport {
        let mut report = FsckReport::default();
        let mut seen = [0u8; BITMAP_BYTES];
        set_bit(&mut seen, 0, true);
        set_bit(&mut seen, 1, true);
        let table = self.working.inodes;
        self.check_node(table.root, table.height, &mut seen, &mut report);
        let slots = (table.size / INODE_SIZE as u64).min(u64::from(MAX_INODES) + 1) as u32;
        let mut reachable = [0u8; BITMAP_BYTES];
        set_bit(&mut reachable, ROOT_INO, true);
        for ino in 1..slots {
            let Ok(inode) = self.get_inode(ino) else {
                report.bad_checksums += 1;
                continue;
            };
            if inode.kind == KIND_FREE {
                continue;
            }
            if inode.kind == KIND_DIRECTORY {
                report.directories += 1;
            } else {
                report.files += 1;
            }
            self.check_node(inode.tree.root, inode.tree.height, &mut seen, &mut report);
        }
        for ino in 1..slots {
            let Ok(inode) = self.get_inode(ino) else {
                continue;
            };
            if inode.kind != KIND_DIRECTORY {
                continue;
            }
            let mut offset = 0;
            let mut entry = [0u8; DIR_ENTRY];
            while offset < inode.tree.size {
                if self.tree_read(&inode.tree, offset, &mut entry).is_err() {
                    report.bad_entries += 1;
                    break;
                }
                offset += DIR_ENTRY as u64;
                let target = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
                if target == 0 {
                    continue;
                }
                match self.get_inode(target) {
                    Ok(found) if found.kind == entry[5] && found.parent == ino => {
                        set_bit(&mut reachable, target, true);
                    }
                    _ => report.bad_entries += 1,
                }
            }
        }
        for ino in 2..slots {
            if let Ok(inode) = self.get_inode(ino)
                && inode.kind != KIND_FREE
                && !bit(&reachable, ino)
            {
                report.orphans += 1;
            }
        }
        for block in 0..self.working.blocks {
            if bit(&seen, block) != bit(&self.used, block) {
                report.map_mismatches += 1;
            }
        }
        report
    }
}
