//! A mounted volume: a FAT filesystem or an AerFS one behind the same set of
//! operations, so `datafs` (and through it the VFS) does not care which. An
//! AerFS file is presented as a `fatfs::Node` whose `first_cluster` is its
//! inode number and whose `dir` is its parent's.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::aerfs::{self, AerFs, Device};
use crate::fatfs::{self, Box64, Disk, FsError, FsInfo, FsckReport, Listed, Node};

pub struct DiskDevice {
    disk: Disk,
    start: u64,
    blocks: u32,
}

impl DiskDevice {
    const NONE: Self = Self {
        disk: Disk::Usb,
        start: 0,
        blocks: 0,
    };

    fn lba(&self, block: u32) -> u64 {
        self.start + u64::from(block) * 8
    }
}

impl Device for DiskDevice {
    fn blocks(&self) -> u32 {
        self.blocks
    }

    fn read_block(&mut self, block: u32, out: &mut [u8; aerfs::BLOCK]) -> bool {
        self.disk.read_run(self.lba(block), 8, out)
    }

    fn write_block(&mut self, block: u32, data: &[u8; aerfs::BLOCK]) -> bool {
        self.disk.write_run(self.lba(block), 8, data)
    }

    fn read_sector(&mut self, block: u32, out: &mut [u8; aerfs::SECTOR]) -> bool {
        self.disk.read_run(self.lba(block), 1, out)
    }

    fn write_sector(&mut self, block: u32, data: &[u8; aerfs::SECTOR]) -> bool {
        self.disk.write_run(self.lba(block), 1, data)
    }

    fn flush(&mut self) -> bool {
        true
    }
}

fn clock() -> u64 {
    crate::rtc::unix_seconds()
}

type Aer = AerFs<DiskDevice>;

const SLOTS: usize = 2;
static mut AER_SLOTS: [Aer; SLOTS] = [
    Aer::new_empty(DiskDevice::NONE, clock),
    Aer::new_empty(DiskDevice::NONE, clock),
];
static AER_USED: [AtomicBool; SLOTS] = [const { AtomicBool::new(false) }; SLOTS];

/// A mounted AerFS volume in static storage (the object is far too big for a
/// stack).
pub struct AerBox {
    index: usize,
}

impl AerBox {
    fn claim() -> Option<Self> {
        for (index, used) in AER_USED.iter().enumerate() {
            if !used.swap(true, Ordering::AcqRel) {
                return Some(Self { index });
            }
        }
        None
    }

    fn get(&mut self) -> &mut Aer {
        // SAFETY: the slot was claimed, so this is the only reference.
        unsafe { &mut *core::ptr::addr_of_mut!(AER_SLOTS[self.index]) }
    }

    fn view(&self) -> &Aer {
        // SAFETY: as above; shared access only.
        unsafe { &*core::ptr::addr_of!(AER_SLOTS[self.index]) }
    }
}

impl Drop for AerBox {
    fn drop(&mut self) {
        self.get().replace_device(DiskDevice::NONE);
        AER_USED[self.index].store(false, Ordering::Release);
    }
}

pub enum Volume {
    Fat(Box64),
    Aer(AerBox),
}

fn map(error: aerfs::Error) -> FsError {
    use aerfs::Error as A;
    match error {
        A::NotFound => FsError::NotFound,
        A::Exists => FsError::Exists,
        A::NotDirectory => FsError::NotDirectory,
        A::IsDirectory => FsError::IsDirectory,
        A::NotEmpty => FsError::NotEmpty,
        A::NoSpace => FsError::NoSpace,
        A::InvalidName => FsError::InvalidName,
        A::InvalidArgument => FsError::ReadOnly,
        A::TooBig => FsError::TooLarge,
        A::Corrupt | A::BadSuperblock => FsError::Corrupt,
        A::Io => FsError::Io,
    }
}

fn to_node(node: &aerfs::Node) -> Node {
    let (date, time) = fatfs::unix_to_fat(node.mtime);
    Node::from_parts(
        node.parent,
        node.ino,
        node.size.min(u64::from(u32::MAX)) as u32,
        node.is_directory(),
        node.read_only,
        date,
        time,
    )
}

fn from_node(node: &Node) -> aerfs::Node {
    aerfs::Node {
        ino: node.first_cluster,
        ..aerfs::Node::EMPTY
    }
}

impl Volume {
    pub fn type_name(&self) -> &'static str {
        match self {
            Volume::Fat(_) => "vfat",
            Volume::Aer(_) => "aerfs",
        }
    }

    fn device(disk: Disk, start: u64, sectors: u64) -> DiskDevice {
        DiskDevice {
            disk,
            start,
            blocks: (sectors / 8).min(aerfs::MAX_BLOCKS as u64) as u32,
        }
    }

    /// Mounts an existing AerFS volume.
    pub fn mount_aerfs(disk: Disk, start: u64, sectors: u64) -> Result<Volume, FsError> {
        let mut slot = AerBox::claim().ok_or(FsError::NoSpace)?;
        slot.get()
            .replace_device(Self::device(disk, start, sectors));
        slot.get().load().map_err(map)?;
        Ok(Volume::Aer(slot))
    }

    /// Writes a new, empty AerFS and returns it mounted.
    pub fn format_aerfs(
        disk: Disk,
        start: u64,
        sectors: u64,
        label: &[u8],
    ) -> Result<Volume, FsError> {
        let mut slot = AerBox::claim().ok_or(FsError::NoSpace)?;
        slot.get()
            .replace_device(Self::device(disk, start, sectors));
        slot.get().format_here(label).map_err(map)?;
        Ok(Volume::Aer(slot))
    }

    /// True when `sector` (the first of a disk or partition) starts an AerFS
    /// volume.
    pub fn sniff_aerfs(sector: &[u8; 512]) -> bool {
        Aer::is_superblock(sector)
    }

    pub fn resolve(&mut self, path: &[u8]) -> Result<Node, FsError> {
        match self {
            Volume::Fat(fs) => fs.resolve(path),
            Volume::Aer(fs) => fs
                .get()
                .resolve(path)
                .map(|node| to_node(&node))
                .map_err(map),
        }
    }

    pub fn resolve_parent<'a>(&mut self, path: &'a [u8]) -> Result<(Node, &'a [u8]), FsError> {
        match self {
            Volume::Fat(fs) => fs.resolve_parent(path),
            Volume::Aer(fs) => fs
                .get()
                .resolve_parent(path)
                .map(|(node, name)| (to_node(&node), name))
                .map_err(map),
        }
    }

    pub fn list_next(&mut self, dir: u32, cursor: &mut u32) -> Result<Option<Listed>, FsError> {
        match self {
            Volume::Fat(fs) => fs.list_next(dir, cursor),
            Volume::Aer(fs) => fs
                .get()
                .list_next(dir, cursor)
                .map(|entry| {
                    entry.map(|entry| {
                        let mut name = [0u8; fatfs::NAME_MAX];
                        name[..entry.name_len].copy_from_slice(entry.name());
                        Listed {
                            node: to_node(&entry.node),
                            name,
                            name_len: entry.name_len,
                        }
                    })
                })
                .map_err(map),
        }
    }

    pub fn create_file(&mut self, dir: u32, name: &[u8]) -> Result<Node, FsError> {
        match self {
            Volume::Fat(fs) => fs.create_file(dir, name),
            Volume::Aer(fs) => fs
                .get()
                .create_file(dir, name)
                .map(|n| to_node(&n))
                .map_err(map),
        }
    }

    pub fn create_dir(&mut self, dir: u32, name: &[u8]) -> Result<Node, FsError> {
        match self {
            Volume::Fat(fs) => fs.create_dir(dir, name),
            Volume::Aer(fs) => fs
                .get()
                .create_dir(dir, name)
                .map(|n| to_node(&n))
                .map_err(map),
        }
    }

    pub fn read_at(
        &mut self,
        node: &mut Node,
        offset: u64,
        out: &mut [u8],
    ) -> Result<usize, FsError> {
        match self {
            Volume::Fat(fs) => fs.read_at(node, offset, out),
            Volume::Aer(fs) => fs.get().read_at(&from_node(node), offset, out).map_err(map),
        }
    }

    pub fn write_at(
        &mut self,
        node: &mut Node,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, FsError> {
        match self {
            Volume::Fat(fs) => fs.write_at(node, offset, data),
            Volume::Aer(fs) => {
                let mut inner = from_node(node);
                let written = fs.get().write_at(&mut inner, offset, data).map_err(map)?;
                *node = to_node(&inner);
                Ok(written)
            }
        }
    }

    pub fn truncate(&mut self, node: &mut Node, length: u64) -> Result<(), FsError> {
        match self {
            Volume::Fat(fs) => fs.truncate(node, length),
            Volume::Aer(fs) => {
                let mut inner = from_node(node);
                fs.get().truncate(&mut inner, length).map_err(map)?;
                *node = to_node(&inner);
                Ok(())
            }
        }
    }

    pub fn set_read_only(&mut self, node: &mut Node, read_only: bool) -> Result<(), FsError> {
        match self {
            Volume::Fat(fs) => fs.set_read_only(node, read_only),
            Volume::Aer(fs) => {
                let mut inner = from_node(node);
                fs.get().set_read_only(&mut inner, read_only).map_err(map)?;
                *node = to_node(&inner);
                Ok(())
            }
        }
    }

    pub fn remove(&mut self, node: &Node) -> Result<(), FsError> {
        match self {
            Volume::Fat(fs) => fs.remove(node),
            Volume::Aer(fs) => fs.get().remove(&from_node(node)).map_err(map),
        }
    }

    pub fn rename(
        &mut self,
        node: &Node,
        new_dir: u32,
        name: &[u8],
        replace: bool,
    ) -> Result<Node, FsError> {
        match self {
            Volume::Fat(fs) => fs.rename(node, new_dir, name, replace),
            Volume::Aer(fs) => {
                let fs = fs.get();
                if !replace
                    && let Ok(existing) = fs.find(new_dir, name)
                    && existing.ino != node.first_cluster
                {
                    return Err(FsError::Exists);
                }
                fs.rename(&from_node(node), new_dir, name)
                    .map(|n| to_node(&n))
                    .map_err(map)
            }
        }
    }

    /// AerFS has nothing to repair (every change is atomic); `repair` only
    /// matters for FAT.
    pub fn fsck(&mut self, repair: bool) -> Result<FsckReport, FsError> {
        match self {
            Volume::Fat(fs) => fs.fsck(repair),
            Volume::Aer(fs) => {
                let found = fs.get().fsck();
                Ok(FsckReport {
                    files: found.files,
                    directories: found.directories,
                    broken_chains: found.bad_checksums + found.bad_entries,
                    cross_linked: found.double_references,
                    duplicates: found.orphans,
                    size_mismatches: found.map_mismatches,
                    orphan_clusters: 0,
                    chains_cut: 0,
                })
            }
        }
    }

    pub fn label(&self) -> &[u8; 11] {
        match self {
            Volume::Fat(fs) => fs.label(),
            Volume::Aer(fs) => {
                let label = fs.view().label();
                <&[u8; 11]>::try_from(&label[..11]).unwrap_or(&[0; 11])
            }
        }
    }

    pub fn info(&mut self) -> Result<FsInfo, FsError> {
        match self {
            Volume::Fat(fs) => fs.info(),
            Volume::Aer(fs) => {
                let (total, free) = fs.get().space();
                Ok(FsInfo {
                    bytes_per_cluster: aerfs::BLOCK as u32,
                    clusters: total,
                    free_clusters: free,
                    fat32: false,
                })
            }
        }
    }
}
