use crate::datafs;
use crate::fat;
use crate::sync::TicketLock;

const MAX_NODES: usize = 96;
const MAX_HANDLES: usize = 24;
pub const MAX_NAME: usize = 255;
const MAX_DEPTH: usize = 16;
const ROOT_NODE: u16 = 0;
const READ_BITS: u16 = 0o444;
// Was 8; the quarantine store's at-rest encryption key (antivirus.rs) added
// one permanent consumer of this pool (previously only ever /tmp scratch
// files and quarantine entries, all transient), and self-tests that
// exercise encryption need their own scratch file on top of that. /proc and /sys keep
// about twenty-five generated files here, hence 56.
const MAX_MUTABLE_FILES: usize = 56;
const MUTABLE_FILE_BYTES: usize = 4096;
const STATIC_STORAGE: u8 = u8::MAX;
/// How many symlink hops `resolve_following` will chase before giving up -
/// generous for any real use, tight enough that a cycle (`ln -s a b; ln -s
/// b a`) fails fast instead of spinning.
const MAX_SYMLINK_HOPS: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    Empty,
    Directory,
    File,
    /// A symbolic link. Its target string lives in the same `MUTABLE_FILES`
    /// pool a regular mutable file's content does (see `create_symlink`),
    /// capped at `MAX_NAME` bytes rather than the pool's full per-slot
    /// capacity - real symlink targets are always short paths. Only ever
    /// created in the in-memory tmpfs subtree (`/tmp`); `/data`, `/home` and
    /// `/media` refuse `symlink()` outright since FAT has no on-disk
    /// representation for one.
    Symlink,
}

#[derive(Clone, Copy)]
struct Node {
    kind: NodeKind,
    parent: u16,
    name: [u8; MAX_NAME],
    name_len: u8,
    mode: u16,
    data: &'static [u8],
    storage: u8,
}

impl Node {
    const EMPTY: Self = Self {
        kind: NodeKind::Empty,
        parent: ROOT_NODE,
        name: [0; MAX_NAME],
        name_len: 0,
        mode: 0,
        data: &[],
        storage: STATIC_STORAGE,
    };

    const ROOT: Self = Self {
        kind: NodeKind::Directory,
        parent: ROOT_NODE,
        name: [0; MAX_NAME],
        name_len: 0,
        mode: 0o755,
        data: &[],
        storage: STATIC_STORAGE,
    };

    fn named(kind: NodeKind, parent: u16, name: &str, mode: u16, data: &'static [u8]) -> Self {
        let mut stored = [0; MAX_NAME];
        stored[..name.len()].copy_from_slice(name.as_bytes());
        Self {
            kind,
            parent,
            name: stored,
            name_len: name.len() as u8,
            mode,
            data,
            storage: STATIC_STORAGE,
        }
    }

    fn name_matches(&self, name: &str) -> bool {
        self.name_len as usize == name.len()
            && self.name[..self.name_len as usize] == *name.as_bytes()
    }

    fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }
}

fn short_name_char_ok(byte: u8) -> bool {
    byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
}

/// Maps a VFS file name to a FAT 8.3 short name. Deliberately an identity
/// mapping with no case-folding or truncation: since VFS names within one
/// directory are already unique, requiring the name to already be
/// short-name-shaped (and uppercase) guarantees two different VFS names can
/// never collide into the same on-disk short name.
fn vfs_name_to_short(name: &str) -> Option<[u8; 11]> {
    let bytes = name.as_bytes();
    let dot = bytes.iter().position(|byte| *byte == b'.')?;
    let (base, rest) = bytes.split_at(dot);
    let ext = &rest[1..];
    if base.is_empty() || base.len() > 8 || ext.is_empty() || ext.len() > 3 {
        return None;
    }
    if !base.iter().all(|b| short_name_char_ok(*b)) || !ext.iter().all(|b| short_name_char_ok(*b)) {
        return None;
    }
    let mut short = [b' '; 11];
    short[..base.len()].copy_from_slice(base);
    short[8..8 + ext.len()].copy_from_slice(ext);
    Some(short)
}

/// Inverse of [`vfs_name_to_short`]: reconstructs `BASE.EXT` from a raw
/// FAT directory-entry short name, rejecting anything that wouldn't have
/// been produced by our own mapping (so files created by something other
/// than the VFS's own persistent-storage mount are just left alone).
fn short_name_to_vfs(short: &[u8; 11]) -> Option<([u8; 12], usize)> {
    let base_len = short[..8]
        .iter()
        .rposition(|b| *b != b' ')
        .map_or(0, |index| index + 1);
    let ext_len = short[8..11]
        .iter()
        .rposition(|b| *b != b' ')
        .map_or(0, |index| index + 1);
    if base_len == 0 || ext_len == 0 {
        return None;
    }
    if !short[..base_len].iter().all(|b| short_name_char_ok(*b))
        || !short[8..8 + ext_len].iter().all(|b| short_name_char_ok(*b))
    {
        return None;
    }
    let mut out = [0u8; 12];
    out[..base_len].copy_from_slice(&short[..base_len]);
    out[base_len] = b'.';
    out[base_len + 1..base_len + 1 + ext_len].copy_from_slice(&short[8..8 + ext_len]);
    Some((out, base_len + 1 + ext_len))
}

/// Directory counterpart to `vfs_name_to_short`: directories conventionally
/// have no extension, so this is deliberately a *different, non-
/// overlapping* naming convention (no dot allowed at all) rather than
/// reusing the file mapping with an empty extension - keeping the two
/// separate means a name can never be ambiguous between "persisted file"
/// and "persisted directory" enumeration.
fn vfs_dir_name_to_short(name: &str) -> Option<[u8; 11]> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 8 || bytes.contains(&b'.') {
        return None;
    }
    if !bytes.iter().all(|b| short_name_char_ok(*b)) {
        return None;
    }
    let mut short = [b' '; 11];
    short[..bytes.len()].copy_from_slice(bytes);
    Some(short)
}

/// Inverse of `vfs_dir_name_to_short`. Only accepts a short name with a
/// fully blank extension field, so it can never collide with a name
/// `short_name_to_vfs` (the file mapping) would also accept.
fn short_dir_name_to_vfs(short: &[u8; 11]) -> Option<([u8; 8], usize)> {
    if short[8..11] != *b"   " {
        return None;
    }
    let base_len = short[..8]
        .iter()
        .rposition(|b| *b != b' ')
        .map_or(0, |index| index + 1);
    if base_len == 0 || !short[..base_len].iter().all(|b| short_name_char_ok(*b)) {
        return None;
    }
    let mut out = [0u8; 8];
    out[..base_len].copy_from_slice(&short[..base_len]);
    Some((out, base_len))
}

/// True for the top-level `/tmp` and `/data` mount points themselves (not
/// their contents) - both are writable subtrees, but the mounts can't be
/// removed, renamed or chmod'd out from under the rest of the kernel.
fn is_mutable_mount(node: &Node) -> bool {
    node.parent == ROOT_NODE && (node.name_matches("tmp") || node.name_matches("data"))
}

#[derive(Clone, Copy)]
struct MutableFile {
    used: bool,
    length: usize,
    data: [u8; MUTABLE_FILE_BYTES],
}

impl MutableFile {
    const EMPTY: Self = Self {
        used: false,
        length: 0,
        data: [0; MUTABLE_FILE_BYTES],
    };
}

#[derive(Clone, Copy)]
struct OpenHandle {
    node: u16,
    generation: u16,
    cursor: usize,
    open: bool,
    /// Written through this handle since it was opened (scanned on close).
    dirty: bool,
}

impl OpenHandle {
    const EMPTY: Self = Self {
        node: 0,
        generation: 1,
        cursor: 0,
        open: false,
        dirty: false,
    };
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VfsError {
    InvalidPath,
    Traversal,
    NameTooLong,
    DepthExceeded,
    NotFound,
    NotDirectory,
    IsDirectory,
    Exists,
    PermissionDenied,
    NodeLimit,
    HandleLimit,
    BadDescriptor,
    OffsetOverflow,
    FileTooLarge,
    NotEmpty,
    Busy,
    /// A name under `/data` couldn't be mapped to a FAT 8.3 short name for
    /// persistence - distinct from `InvalidPath` so the caller can tell a
    /// user "name must be short and uppercase" instead of a generic
    /// "invalid path" that gives no clue what was actually wrong with an
    /// otherwise perfectly normal-looking name like `report.txt`.
    PersistNameUnsupported,
    /// A symlink chain exceeded `MAX_SYMLINK_HOPS` while being followed -
    /// either a genuine cycle or just a chain deeper than any legitimate
    /// use needs; either way, resolution stops instead of looping forever.
    TooManyLinks,
}

#[derive(Clone, Copy)]
pub struct InitramfsEntry {
    pub path: &'static str,
    pub data: &'static [u8],
    pub mode: u16,
}

#[derive(Clone, Copy)]
pub struct FileView {
    pub data: &'static [u8],
    pub mode: u16,
}

#[derive(Clone, Copy)]
pub struct Metadata {
    pub inode: u64,
    pub mode: u32,
    pub size: u64,
    /// Last modification, Unix seconds (0 when the filesystem doesn't record it).
    pub modified: u64,
}

#[derive(Clone, Copy)]
pub struct DirectoryEntry {
    pub inode: u64,
    pub kind: u8,
    pub name: [u8; MAX_NAME],
    pub name_len: u8,
}

#[derive(Clone, Copy)]
pub struct VfsStats {
    pub nodes: usize,
    pub directories: usize,
    pub files: usize,
    pub bytes: usize,
    pub open_handles: usize,
    pub mutable_files: usize,
    pub mutable_bytes: usize,
    pub verified: bool,
}

struct ParsedPath<'a> {
    components: [&'a str; MAX_DEPTH],
    count: usize,
}

impl ParsedPath<'_> {
    fn parse(path: &str) -> Result<ParsedPath<'_>, VfsError> {
        if !path.starts_with('/') || path.as_bytes().contains(&0) {
            return Err(VfsError::InvalidPath);
        }
        let mut parsed = ParsedPath {
            components: [""; MAX_DEPTH],
            count: 0,
        };
        for component in path.split('/') {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                if parsed.count == 0 {
                    return Err(VfsError::Traversal);
                }
                parsed.count -= 1;
                continue;
            }
            if component.len() > MAX_NAME {
                return Err(VfsError::NameTooLong);
            }
            if component
                .as_bytes()
                .iter()
                .any(|byte| *byte < 0x20 || *byte == 0x7f)
            {
                return Err(VfsError::InvalidPath);
            }
            if parsed.count == MAX_DEPTH {
                return Err(VfsError::DepthExceeded);
            }
            parsed.components[parsed.count] = component;
            parsed.count += 1;
        }
        Ok(parsed)
    }
}

pub struct Vfs {
    nodes: [Node; MAX_NODES],
    node_count: usize,
    handles: [OpenHandle; MAX_HANDLES],
}

impl Vfs {
    const fn new() -> Self {
        let mut nodes = [Node::EMPTY; MAX_NODES];
        nodes[0] = Node::ROOT;
        Self {
            nodes,
            node_count: 1,
            handles: [OpenHandle::EMPTY; MAX_HANDLES],
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn mount(&mut self, entries: &[InitramfsEntry]) -> Result<(), VfsError> {
        for entry in entries {
            self.mount_file(entry.path, entry.data, entry.mode)?;
        }
        self.mount_directory("/tmp", 0o777)?;
        self.mount_directory("/data", 0o777)?;
        self.mount_directory("/home", 0o755)?;
        self.mount_directory("/media", 0o755)?;
        for path in ["/var", "/opt", "/srv", "/root"] {
            self.mount_directory(path, 0o755)?;
        }
        self.load_persisted_files();
        Ok(())
    }

    /// Surfaces files a previous boot wrote under `/data` (via
    /// [`fat::write_root_file`]) as VFS nodes again, loading their content
    /// into a mutable-file slot so the existing read/write/seek code paths
    /// work unmodified. Runs once at mount time; if node or mutable-file
    /// slots run out partway through, the remaining on-disk files are
    /// simply left unlisted rather than failing the whole mount.
    fn load_persisted_files(&mut self) {
        let Some(data_node) = self.find_child(ROOT_NODE, "data") else {
            return;
        };
        let mut entries = [fat::FatFileEntry::EMPTY; MAX_MUTABLE_FILES];
        let count = fat::list_root_files(&mut entries);
        for entry in &entries[..count] {
            let Some((name_buf, name_len)) = short_name_to_vfs(&entry.name) else {
                continue;
            };
            let Ok(name) = core::str::from_utf8(&name_buf[..name_len]) else {
                continue;
            };
            if self.find_child(data_node, name).is_some() {
                continue;
            }
            let slot = {
                let mut files = MUTABLE_FILES.lock();
                match files.iter().position(|file| !file.used) {
                    Some(slot) => {
                        files[slot] = MutableFile {
                            used: true,
                            ..MutableFile::EMPTY
                        };
                        slot
                    }
                    None => return,
                }
            };
            let Ok(node) = self.add_node(NodeKind::File, data_node, name, 0o644, &[]) else {
                MUTABLE_FILES.lock()[slot] = MutableFile::EMPTY;
                continue;
            };
            self.nodes[node as usize].storage = slot as u8;
            let want = (entry.bytes as usize).min(MUTABLE_FILE_BYTES);
            let mut files = MUTABLE_FILES.lock();
            let length =
                fat::read_root_file(&entry.name, &mut files[slot].data[..want]).unwrap_or(0);
            files[slot].length = length;
        }
        // Subdirectories persist as empty shells only - a prior session's
        // files placed *inside* one aren't tracked by this design (only
        // things directly under /data are), so there's nothing further to
        // repopulate here even though the directory itself comes back.
        let mut directories = [fat::FatFileEntry::EMPTY; 4];
        let directory_count = fat::list_root_directories(&mut directories);
        for entry in &directories[..directory_count] {
            let Some((name_buf, name_len)) = short_dir_name_to_vfs(&entry.name) else {
                continue;
            };
            let Ok(name) = core::str::from_utf8(&name_buf[..name_len]) else {
                continue;
            };
            if self.find_child(data_node, name).is_some() {
                continue;
            }
            let _ = self.add_node(NodeKind::Directory, data_node, name, 0o755, &[]);
        }
    }

    /// The FAT short name a write to this node should be mirrored to, or
    /// `None` if it isn't a `/data`-backed file (the common case).
    fn persisted_short_name(&self, node: &Node) -> Option<[u8; 11]> {
        let data_node = self.find_child(ROOT_NODE, "data")?;
        if node.parent != data_node {
            return None;
        }
        vfs_name_to_short(node.name_str())
    }

    /// Directory counterpart to `persisted_short_name`.
    fn persisted_dir_short_name(&self, node: &Node) -> Option<[u8; 11]> {
        let data_node = self.find_child(ROOT_NODE, "data")?;
        if node.parent != data_node {
            return None;
        }
        vfs_dir_name_to_short(node.name_str())
    }

    fn mount_directory(&mut self, path: &str, mode: u16) -> Result<(), VfsError> {
        let parsed = ParsedPath::parse(path)?;
        if parsed.count == 0 {
            return Err(VfsError::Exists);
        }
        let mut parent = ROOT_NODE;
        for component in &parsed.components[..parsed.count - 1] {
            parent = self
                .find_child(parent, component)
                .ok_or(VfsError::NotFound)?;
            if self.nodes[parent as usize].kind != NodeKind::Directory {
                return Err(VfsError::NotDirectory);
            }
        }
        let name = parsed.components[parsed.count - 1];
        if self.find_child(parent, name).is_some() {
            return Err(VfsError::Exists);
        }
        self.add_node(NodeKind::Directory, parent, name, mode & 0o777, &[])?;
        Ok(())
    }

    fn mount_file(&mut self, path: &str, data: &'static [u8], mode: u16) -> Result<(), VfsError> {
        let parsed = ParsedPath::parse(path)?;
        if parsed.count == 0 {
            return Err(VfsError::IsDirectory);
        }
        let mut parent = ROOT_NODE;
        for component in &parsed.components[..parsed.count - 1] {
            parent = match self.find_child(parent, component) {
                Some(index) if self.nodes[index as usize].kind == NodeKind::Directory => index,
                Some(_) => return Err(VfsError::NotDirectory),
                None => self.add_node(NodeKind::Directory, parent, component, 0o755, &[])?,
            };
        }
        let name = parsed.components[parsed.count - 1];
        if self.find_child(parent, name).is_some() {
            return Err(VfsError::Exists);
        }
        self.add_node(NodeKind::File, parent, name, mode & 0o777, data)?;
        Ok(())
    }

    fn add_node(
        &mut self,
        kind: NodeKind,
        parent: u16,
        name: &str,
        mode: u16,
        data: &'static [u8],
    ) -> Result<u16, VfsError> {
        let index = if let Some(index) =
            (1..self.node_count).find(|index| self.nodes[*index].kind == NodeKind::Empty)
        {
            index
        } else {
            if self.node_count == MAX_NODES {
                return Err(VfsError::NodeLimit);
            }
            let index = self.node_count;
            self.node_count += 1;
            index
        };
        self.nodes[index] = Node::named(kind, parent, name, mode, data);
        Ok(index as u16)
    }

    fn create_directory(&mut self, path: &str, mode: u16) -> Result<(), VfsError> {
        let parsed = ParsedPath::parse(path)?;
        if parsed.count == 0 {
            return Err(VfsError::Exists);
        }
        let (parent, name) = self.resolve_parent(&parsed)?;
        if !self.mutable_subtree(parent) || self.nodes[parent as usize].mode & 0o222 == 0 {
            return Err(VfsError::PermissionDenied);
        }
        if self.find_child(parent, name).is_some() {
            return Err(VfsError::Exists);
        }
        let persist = self.find_child(ROOT_NODE, "data") == Some(parent);
        if persist {
            let short = vfs_dir_name_to_short(name).ok_or(VfsError::PersistNameUnsupported)?;
            if !fat::create_root_directory(&short) {
                return Err(VfsError::PermissionDenied);
            }
        }
        match self.add_node(NodeKind::Directory, parent, name, mode & 0o777, &[]) {
            Ok(_) => Ok(()),
            Err(failure) => {
                if persist && let Some(short) = vfs_dir_name_to_short(name) {
                    fat::delete_root_directory(&short);
                }
                Err(failure)
            }
        }
    }

    fn remove(&mut self, path: &str, directory: bool) -> Result<(), VfsError> {
        let parsed = ParsedPath::parse(path)?;
        if parsed.count == 0 {
            return Err(VfsError::Busy);
        }
        let node = self.resolve(path)?;
        let stored = self.nodes[node as usize];
        if !self.mutable_subtree(node) || is_mutable_mount(&stored) {
            return Err(VfsError::PermissionDenied);
        }
        if directory && stored.kind != NodeKind::Directory {
            return Err(VfsError::NotDirectory);
        }
        if !directory && stored.kind != NodeKind::File && stored.kind != NodeKind::Symlink {
            return Err(VfsError::IsDirectory);
        }
        if stored.kind == NodeKind::Directory
            && self.nodes[..self.node_count]
                .iter()
                .any(|entry| entry.kind != NodeKind::Empty && entry.parent == node)
        {
            return Err(VfsError::NotEmpty);
        }
        if self
            .handles
            .iter()
            .any(|handle| handle.open && handle.node == node)
        {
            return Err(VfsError::Busy);
        }
        if stored.kind == NodeKind::Directory {
            if let Some(short) = self.persisted_dir_short_name(&stored) {
                fat::delete_root_directory(&short);
            }
        } else if let Some(short) = self.persisted_short_name(&stored) {
            fat::delete_root_file(&short);
        }
        self.release_node(node);
        Ok(())
    }

    fn rename(&mut self, source: &str, destination: &str) -> Result<(), VfsError> {
        self.rename_with(source, destination, false)
    }

    fn rename_noreplace(&mut self, source: &str, destination: &str) -> Result<(), VfsError> {
        self.rename_with(source, destination, true)
    }

    fn rename_with(
        &mut self,
        source: &str,
        destination: &str,
        noreplace: bool,
    ) -> Result<(), VfsError> {
        let source_parsed = ParsedPath::parse(source)?;
        let destination_parsed = ParsedPath::parse(destination)?;
        if source_parsed.count == 0 || destination_parsed.count == 0 {
            return Err(VfsError::Busy);
        }
        let source_node = self.resolve(source)?;
        let source_stored = self.nodes[source_node as usize];
        if !self.mutable_subtree(source_node) || is_mutable_mount(&source_stored) {
            return Err(VfsError::PermissionDenied);
        }
        let source_parent = source_stored.parent;
        if self.nodes[source_parent as usize].mode & 0o222 == 0 {
            return Err(VfsError::PermissionDenied);
        }
        let (destination_parent, destination_name) = self.resolve_parent(&destination_parsed)?;
        if !self.mutable_subtree(destination_parent)
            || self.nodes[destination_parent as usize].mode & 0o222 == 0
        {
            return Err(VfsError::PermissionDenied);
        }
        // Renaming a disk-persisted file would need its own on-disk rename
        // (delete old short name, write new one) kept in lockstep with the
        // VFS node - not yet implemented, so /data is excluded from rename
        // for now rather than risk the two staying out of sync.
        if self
            .find_child(ROOT_NODE, "data")
            .is_some_and(|data_node| source_parent == data_node || destination_parent == data_node)
        {
            return Err(VfsError::PermissionDenied);
        }
        let destination_node = self.find_child(destination_parent, destination_name);
        if destination_node == Some(source_node) {
            return Ok(());
        }
        if destination_node.is_some() && noreplace {
            return Err(VfsError::Exists);
        }
        if let Some(destination_node) = destination_node {
            let destination_stored = self.nodes[destination_node as usize];
            if source_stored.kind == NodeKind::Directory
                && destination_stored.kind != NodeKind::Directory
            {
                return Err(VfsError::NotDirectory);
            }
            if source_stored.kind != NodeKind::Directory
                && destination_stored.kind == NodeKind::Directory
            {
                return Err(VfsError::IsDirectory);
            }
            if destination_stored.kind == NodeKind::Directory
                && self.nodes[..self.node_count]
                    .iter()
                    .any(|entry| entry.kind != NodeKind::Empty && entry.parent == destination_node)
            {
                return Err(VfsError::NotEmpty);
            }
            if self
                .handles
                .iter()
                .any(|handle| handle.open && handle.node == destination_node)
            {
                return Err(VfsError::Busy);
            }
        }
        if source_stored.kind == NodeKind::Directory {
            let mut cursor = destination_parent;
            loop {
                if cursor == source_node {
                    return Err(VfsError::Traversal);
                }
                if cursor == ROOT_NODE {
                    break;
                }
                cursor = self.nodes[cursor as usize].parent;
            }
        }
        if let Some(destination_node) = destination_node {
            self.release_node(destination_node);
        }
        let mut name = [0; MAX_NAME];
        name[..destination_name.len()].copy_from_slice(destination_name.as_bytes());
        self.nodes[source_node as usize].parent = destination_parent;
        self.nodes[source_node as usize].name = name;
        self.nodes[source_node as usize].name_len = destination_name.len() as u8;
        Ok(())
    }

    fn release_node(&mut self, node: u16) {
        let stored = self.nodes[node as usize];
        if stored.storage != STATIC_STORAGE {
            MUTABLE_FILES.lock()[stored.storage as usize] = MutableFile::EMPTY;
        }
        self.nodes[node as usize] = Node::EMPTY;
    }

    fn chmod(&mut self, path: &str, mode: u16) -> Result<(), VfsError> {
        let node = self.resolve(path)?;
        if !self.mutable_subtree(node) || is_mutable_mount(&self.nodes[node as usize]) {
            return Err(VfsError::PermissionDenied);
        }
        self.nodes[node as usize].mode = mode & 0o777;
        Ok(())
    }

    fn fchmod(&mut self, descriptor: u32, mode: u16) -> Result<(), VfsError> {
        let node = self.handle_mut(descriptor)?.node;
        if !self.mutable_subtree(node) {
            return Err(VfsError::PermissionDenied);
        }
        self.nodes[node as usize].mode = mode & 0o777;
        Ok(())
    }

    fn truncate(&mut self, descriptor: u32, length: usize) -> Result<(), VfsError> {
        if length > MUTABLE_FILE_BYTES {
            return Err(VfsError::FileTooLarge);
        }
        let node = self.handle_mut(descriptor)?.node;
        let stored = self.nodes[node as usize];
        if stored.kind != NodeKind::File {
            return Err(VfsError::IsDirectory);
        }
        if stored.storage == STATIC_STORAGE {
            return Err(VfsError::PermissionDenied);
        }
        let persist_name = self.persisted_short_name(&stored);
        {
            let mut files = MUTABLE_FILES.lock();
            let file = &mut files[stored.storage as usize];
            if !file.used {
                return Err(VfsError::BadDescriptor);
            }
            if length > file.length {
                file.data[file.length..length].fill(0);
            } else {
                file.data[length..file.length].fill(0);
            }
            file.length = length;
        }
        if let Some(short) = persist_name {
            let files = MUTABLE_FILES.lock();
            fat::write_root_file(&short, &files[stored.storage as usize].data[..length]);
        }
        Ok(())
    }

    fn resolve_parent<'a>(&self, parsed: &ParsedPath<'a>) -> Result<(u16, &'a str), VfsError> {
        let mut parent = ROOT_NODE;
        for component in &parsed.components[..parsed.count - 1] {
            parent = self
                .find_child(parent, component)
                .ok_or(VfsError::NotFound)?;
            if self.nodes[parent as usize].kind != NodeKind::Directory {
                return Err(VfsError::NotDirectory);
            }
        }
        Ok((parent, parsed.components[parsed.count - 1]))
    }

    fn mutable_subtree(&self, node: u16) -> bool {
        let mut cursor = node;
        loop {
            let stored = self.nodes[cursor as usize];
            if stored.parent == ROOT_NODE {
                return stored.kind == NodeKind::Directory && is_mutable_mount(&stored);
            }
            if cursor == ROOT_NODE || stored.kind == NodeKind::Empty {
                return false;
            }
            cursor = stored.parent;
        }
    }

    fn find_child(&self, parent: u16, name: &str) -> Option<u16> {
        self.nodes[..self.node_count]
            .iter()
            .enumerate()
            .find(|(_, node)| {
                node.kind != NodeKind::Empty && node.parent == parent && node.name_matches(name)
            })
            .map(|(index, _)| index as u16)
    }

    fn resolve(&self, path: &str) -> Result<u16, VfsError> {
        let parsed = ParsedPath::parse(path)?;
        let mut current = ROOT_NODE;
        for component in &parsed.components[..parsed.count] {
            if self.nodes[current as usize].kind != NodeKind::Directory {
                return Err(VfsError::NotDirectory);
            }
            current = self
                .find_child(current, component)
                .ok_or(VfsError::NotFound)?;
        }
        Ok(current)
    }

    fn file(&self, path: &str) -> Result<FileView, VfsError> {
        let node = self.nodes[self.resolve(path)? as usize];
        if node.kind != NodeKind::File {
            return Err(VfsError::IsDirectory);
        }
        Ok(FileView {
            data: node.data,
            mode: node.mode,
        })
    }

    fn metadata(&self, path: &str) -> Result<Metadata, VfsError> {
        let node = self.resolve_following(path)?;
        Ok(self.node_metadata(node))
    }

    fn symlink_metadata(&self, path: &str) -> Result<Metadata, VfsError> {
        let node = self.resolve(path)?;
        Ok(self.node_metadata(node))
    }

    fn descriptor_metadata(&mut self, descriptor: u32) -> Result<Metadata, VfsError> {
        let node = self.handle_mut(descriptor)?.node;
        Ok(self.node_metadata(node))
    }

    fn node_metadata(&self, index: u16) -> Metadata {
        let node = self.nodes[index as usize];
        let kind = match node.kind {
            NodeKind::Directory => 0o040000,
            NodeKind::File => 0o100000,
            NodeKind::Symlink => 0o120000,
            NodeKind::Empty => 0,
        };
        Metadata {
            inode: index as u64 + 1,
            modified: 0,
            mode: kind | node.mode as u32,
            size: if node.storage == STATIC_STORAGE {
                node.data.len() as u64
            } else {
                MUTABLE_FILES.lock()[node.storage as usize].length as u64
            },
        }
    }

    fn open(&mut self, path: &str) -> Result<u32, VfsError> {
        self.open_kind(path, false)
    }

    fn open_file(
        &mut self,
        path: &str,
        create: bool,
        exclusive: bool,
        truncate: bool,
        mode: u16,
        write: bool,
    ) -> Result<u32, VfsError> {
        let node = match self.resolve_following(path) {
            Ok(_) if create && exclusive => return Err(VfsError::Exists),
            Ok(node) => node,
            Err(VfsError::NotFound) if create => self.create_file(path, mode)?,
            Err(failure) => return Err(failure),
        };
        let stored = self.nodes[node as usize];
        if stored.kind != NodeKind::File {
            return Err(VfsError::IsDirectory);
        }
        if write && stored.storage == STATIC_STORAGE {
            return Err(VfsError::PermissionDenied);
        }
        if !write && stored.mode & READ_BITS == 0 {
            return Err(VfsError::PermissionDenied);
        }
        if truncate {
            if stored.storage == STATIC_STORAGE {
                return Err(VfsError::PermissionDenied);
            }
            let persist_name = self.persisted_short_name(&stored);
            {
                let mut files = MUTABLE_FILES.lock();
                let file = &mut files[stored.storage as usize];
                file.data.fill(0);
                file.length = 0;
            }
            if let Some(short) = persist_name {
                fat::write_root_file(&short, &[]);
            }
        }
        self.open_node(node)
    }

    fn create_file(&mut self, path: &str, mode: u16) -> Result<u16, VfsError> {
        let parsed = ParsedPath::parse(path)?;
        if parsed.count == 0 {
            return Err(VfsError::IsDirectory);
        }
        let mut parent = ROOT_NODE;
        for component in &parsed.components[..parsed.count - 1] {
            parent = self
                .find_child(parent, component)
                .ok_or(VfsError::NotFound)?;
            if self.nodes[parent as usize].kind != NodeKind::Directory {
                return Err(VfsError::NotDirectory);
            }
        }
        if self.nodes[parent as usize].mode & 0o222 == 0 {
            return Err(VfsError::PermissionDenied);
        }
        let name = parsed.components[parsed.count - 1];
        if self.find_child(parent, name).is_some() {
            return Err(VfsError::Exists);
        }
        let persist = self.find_child(ROOT_NODE, "data") == Some(parent);
        let short_name = if persist {
            Some(vfs_name_to_short(name).ok_or(VfsError::PersistNameUnsupported)?)
        } else {
            None
        };
        let slot = {
            let mut files = MUTABLE_FILES.lock();
            let Some(slot) = files.iter().position(|file| !file.used) else {
                return Err(VfsError::NodeLimit);
            };
            files[slot] = MutableFile {
                used: true,
                ..MutableFile::EMPTY
            };
            slot
        };
        if let Some(short) = short_name
            && !fat::write_root_file(&short, &[])
        {
            MUTABLE_FILES.lock()[slot] = MutableFile::EMPTY;
            return Err(VfsError::PermissionDenied);
        }
        match self.add_node(NodeKind::File, parent, name, mode & 0o666, &[]) {
            Ok(node) => {
                self.nodes[node as usize].storage = slot as u8;
                Ok(node)
            }
            Err(failure) => {
                MUTABLE_FILES.lock()[slot] = MutableFile::EMPTY;
                if let Some(short) = short_name {
                    fat::delete_root_file(&short);
                }
                Err(failure)
            }
        }
    }

    /// Creates a symlink at `path` whose target is the literal string
    /// `target`, unresolved and unvalidated (matching real `symlink()`: the
    /// target need not exist, and can be relative or absolute). Refused
    /// under `/data` since a symlink has no FAT on-disk form to persist it
    /// as - unlike a mutable file, there is no degraded fallback here, only
    /// silently losing the link across the next remount, so it is rejected
    /// up front instead.
    fn create_symlink(&mut self, path: &str, target: &str) -> Result<u16, VfsError> {
        if target.is_empty() || target.len() > MAX_NAME {
            return Err(VfsError::NameTooLong);
        }
        let parsed = ParsedPath::parse(path)?;
        if parsed.count == 0 {
            return Err(VfsError::IsDirectory);
        }
        let (parent, name) = self.resolve_parent(&parsed)?;
        if !self.mutable_subtree(parent) || self.nodes[parent as usize].mode & 0o222 == 0 {
            return Err(VfsError::PermissionDenied);
        }
        if self.find_child(ROOT_NODE, "data") == Some(parent) {
            return Err(VfsError::PermissionDenied);
        }
        if self.find_child(parent, name).is_some() {
            return Err(VfsError::Exists);
        }
        let slot = {
            let mut files = MUTABLE_FILES.lock();
            let Some(slot) = files.iter().position(|file| !file.used) else {
                return Err(VfsError::NodeLimit);
            };
            let mut file = MutableFile {
                used: true,
                ..MutableFile::EMPTY
            };
            file.data[..target.len()].copy_from_slice(target.as_bytes());
            file.length = target.len();
            files[slot] = file;
            slot
        };
        match self.add_node(NodeKind::Symlink, parent, name, 0o777, &[]) {
            Ok(node) => {
                self.nodes[node as usize].storage = slot as u8;
                Ok(node)
            }
            Err(failure) => {
                MUTABLE_FILES.lock()[slot] = MutableFile::EMPTY;
                Err(failure)
            }
        }
    }

    /// The raw target string a symlink was created with - `path` itself
    /// must be the symlink (unlike `open_file`/`metadata`, this never
    /// follows), matching real `readlink()`.
    fn readlink(&self, path: &str, buffer: &mut [u8]) -> Result<usize, VfsError> {
        let node = self.resolve(path)?;
        let stored = self.nodes[node as usize];
        if stored.kind != NodeKind::Symlink {
            return Err(VfsError::InvalidPath);
        }
        let files = MUTABLE_FILES.lock();
        let file = &files[stored.storage as usize];
        let length = file.length.min(buffer.len());
        buffer[..length].copy_from_slice(&file.data[..length]);
        Ok(length)
    }

    /// Like `resolve`, but if the final component is a symlink, follows it
    /// (relative targets are joined against the symlink's own parent
    /// directory via `node_path`) up to `MAX_SYMLINK_HOPS` times. Symlinks
    /// named by a non-final path component are deliberately not followed,
    /// since `resolve`'s per-component loop already requires every component
    /// but the last to be a real `Directory`, so a symlink there surfaces as
    /// the existing `NotDirectory` error rather than silently working
    /// sometimes.
    fn resolve_following(&self, path: &str) -> Result<u16, VfsError> {
        let mut current = self.resolve(path)?;
        for _ in 0..MAX_SYMLINK_HOPS {
            let stored = self.nodes[current as usize];
            if stored.kind != NodeKind::Symlink {
                return Ok(current);
            }
            let (target_buf, target_len) = {
                let files = MUTABLE_FILES.lock();
                let file = &files[stored.storage as usize];
                let mut buf = [0u8; MAX_NAME];
                buf[..file.length].copy_from_slice(&file.data[..file.length]);
                (buf, file.length)
            };
            let target = core::str::from_utf8(&target_buf[..target_len])
                .map_err(|_| VfsError::InvalidPath)?;
            let mut joined = [0u8; 128];
            let resolved = if target.starts_with('/') {
                target
            } else {
                // A symlink only ever exists under a writable mount (never
                // at the root itself), so `parent_len` is always a real,
                // non-empty path like "/tmp" here, never the 0 `node_path`
                // uses to mean "root" or "didn't fit".
                let parent_len = self.node_path(stored.parent, &mut joined);
                if parent_len == 0 {
                    return Err(VfsError::NameTooLong);
                }
                let bytes = target.as_bytes();
                if parent_len + 1 + bytes.len() > joined.len() {
                    return Err(VfsError::NameTooLong);
                }
                joined[parent_len] = b'/';
                joined[parent_len + 1..parent_len + 1 + bytes.len()].copy_from_slice(bytes);
                let len = parent_len + 1 + bytes.len();
                core::str::from_utf8(&joined[..len]).map_err(|_| VfsError::InvalidPath)?
            };
            current = self.resolve(resolved)?;
        }
        Err(VfsError::TooManyLinks)
    }

    fn open_directory(&mut self, path: &str) -> Result<u32, VfsError> {
        self.open_kind(path, true)
    }

    fn open_kind(&mut self, path: &str, directory: bool) -> Result<u32, VfsError> {
        let node_index = self.resolve_following(path)?;
        let node = self.nodes[node_index as usize];
        if directory && node.kind != NodeKind::Directory {
            return Err(VfsError::NotDirectory);
        }
        if !directory && node.kind != NodeKind::File {
            return Err(VfsError::IsDirectory);
        }
        if node.mode & READ_BITS == 0 {
            return Err(VfsError::PermissionDenied);
        }
        self.open_node(node_index)
    }

    fn open_node(&mut self, node: u16) -> Result<u32, VfsError> {
        let slot = self
            .handles
            .iter()
            .position(|handle| !handle.open)
            .ok_or(VfsError::HandleLimit)?;
        let generation = self.handles[slot].generation.max(1);
        self.handles[slot] = OpenHandle {
            node,
            generation,
            cursor: 0,
            open: true,
            dirty: false,
        };
        Ok(((generation as u32) << 16) | slot as u32)
    }

    fn next_directory_entry(
        &mut self,
        descriptor: u32,
    ) -> Result<Option<DirectoryEntry>, VfsError> {
        let (directory, cursor) = {
            let handle = self.handle_mut(descriptor)?;
            (handle.node, handle.cursor)
        };
        if self.nodes[directory as usize].kind != NodeKind::Directory {
            return Err(VfsError::NotDirectory);
        }
        let Some(index) = (cursor.max(1)..self.node_count).find(|index| {
            self.nodes[*index].kind != NodeKind::Empty && self.nodes[*index].parent == directory
        }) else {
            self.handle_mut(descriptor)?.cursor = self.node_count;
            return Ok(None);
        };
        self.handle_mut(descriptor)?.cursor = index + 1;
        let node = self.nodes[index];
        Ok(Some(DirectoryEntry {
            inode: index as u64 + 1,
            kind: match node.kind {
                NodeKind::Directory => 4,
                NodeKind::File => 8,
                NodeKind::Symlink => 10,
                NodeKind::Empty => 0,
            },
            name: node.name,
            name_len: node.name_len,
        }))
    }

    fn handle_mut(&mut self, descriptor: u32) -> Result<&mut OpenHandle, VfsError> {
        let slot = descriptor as usize & 0xffff;
        let generation = (descriptor >> 16) as u16;
        let Some(handle) = self.handles.get_mut(slot) else {
            return Err(VfsError::BadDescriptor);
        };
        if !handle.open || handle.generation != generation {
            return Err(VfsError::BadDescriptor);
        }
        Ok(handle)
    }

    fn read(&mut self, descriptor: u32, destination: &mut [u8]) -> Result<usize, VfsError> {
        let (node_index, cursor) = {
            let handle = self.handle_mut(descriptor)?;
            (handle.node, handle.cursor)
        };
        let node = self.nodes[node_index as usize];
        let count = if node.storage == STATIC_STORAGE {
            if cursor >= node.data.len() {
                return Ok(0);
            }
            let count = node
                .data
                .len()
                .saturating_sub(cursor)
                .min(destination.len());
            destination[..count].copy_from_slice(&node.data[cursor..cursor + count]);
            count
        } else {
            let files = MUTABLE_FILES.lock();
            let file = files[node.storage as usize];
            if cursor >= file.length {
                return Ok(0);
            }
            let count = file.length.saturating_sub(cursor).min(destination.len());
            destination[..count].copy_from_slice(&file.data[cursor..cursor + count]);
            count
        };
        self.handle_mut(descriptor)?.cursor =
            cursor.checked_add(count).ok_or(VfsError::OffsetOverflow)?;
        Ok(count)
    }

    fn read_at(
        &mut self,
        descriptor: u32,
        offset: usize,
        destination: &mut [u8],
    ) -> Result<usize, VfsError> {
        let node = self.handle_mut(descriptor)?.node;
        let node = self.nodes[node as usize];
        if node.storage == STATIC_STORAGE {
            if offset >= node.data.len() {
                return Ok(0);
            }
            let count = node
                .data
                .len()
                .saturating_sub(offset)
                .min(destination.len());
            destination[..count].copy_from_slice(&node.data[offset..offset + count]);
            Ok(count)
        } else {
            let files = MUTABLE_FILES.lock();
            let file = files[node.storage as usize];
            if offset >= file.length {
                return Ok(0);
            }
            let count = file.length.saturating_sub(offset).min(destination.len());
            destination[..count].copy_from_slice(&file.data[offset..offset + count]);
            Ok(count)
        }
    }

    fn write(&mut self, descriptor: u32, source: &[u8], append: bool) -> Result<usize, VfsError> {
        let (node, cursor) = {
            let handle = self.handle_mut(descriptor)?;
            (handle.node, handle.cursor)
        };
        let stored = self.nodes[node as usize];
        if stored.storage == STATIC_STORAGE {
            return Err(VfsError::PermissionDenied);
        }
        let persist_name = self.persisted_short_name(&stored);
        let end;
        let persisted_length;
        {
            let mut files = MUTABLE_FILES.lock();
            let file = &mut files[stored.storage as usize];
            if !file.used {
                return Err(VfsError::BadDescriptor);
            }
            let start = if append { file.length } else { cursor };
            end = start
                .checked_add(source.len())
                .ok_or(VfsError::OffsetOverflow)?;
            if end > MUTABLE_FILE_BYTES {
                return Err(VfsError::FileTooLarge);
            }
            file.data[start..end].copy_from_slice(source);
            file.length = file.length.max(end);
            persisted_length = file.length;
        }
        if let Some(short) = persist_name {
            let files = MUTABLE_FILES.lock();
            fat::write_root_file(
                &short,
                &files[stored.storage as usize].data[..persisted_length],
            );
        }
        self.handle_mut(descriptor)?.cursor = end;
        Ok(source.len())
    }

    /// `pwrite`-style write at an explicit offset that leaves the handle's
    /// own cursor untouched, unlike [`Vfs::write`].
    fn write_at(
        &mut self,
        descriptor: u32,
        offset: usize,
        source: &[u8],
    ) -> Result<usize, VfsError> {
        let node = self.handle_mut(descriptor)?.node;
        let stored = self.nodes[node as usize];
        if stored.storage == STATIC_STORAGE {
            return Err(VfsError::PermissionDenied);
        }
        let persist_name = self.persisted_short_name(&stored);
        let persisted_length;
        {
            let mut files = MUTABLE_FILES.lock();
            let file = &mut files[stored.storage as usize];
            if !file.used {
                return Err(VfsError::BadDescriptor);
            }
            let end = offset
                .checked_add(source.len())
                .ok_or(VfsError::OffsetOverflow)?;
            if end > MUTABLE_FILE_BYTES {
                return Err(VfsError::FileTooLarge);
            }
            if offset > file.length {
                file.data[file.length..offset].fill(0);
            }
            file.data[offset..end].copy_from_slice(source);
            file.length = file.length.max(end);
            persisted_length = file.length;
        }
        if let Some(short) = persist_name {
            let files = MUTABLE_FILES.lock();
            fat::write_root_file(
                &short,
                &files[stored.storage as usize].data[..persisted_length],
            );
        }
        Ok(source.len())
    }

    fn seek(&mut self, descriptor: u32, offset: i64, whence: u64) -> Result<usize, VfsError> {
        let (node, cursor) = {
            let handle = self.handle_mut(descriptor)?;
            (handle.node, handle.cursor)
        };
        let base = match whence {
            0 => 0i128,
            1 => cursor as i128,
            2 => self.node_metadata(node).size as i128,
            _ => return Err(VfsError::InvalidPath),
        };
        let target = base + offset as i128;
        if target < 0 || target > usize::MAX as i128 {
            return Err(VfsError::OffsetOverflow);
        }
        let target = target as usize;
        self.handle_mut(descriptor)?.cursor = target;
        Ok(target)
    }

    fn mark_dirty(&mut self, descriptor: u32) {
        if let Ok(handle) = self.handle_mut(descriptor) {
            handle.dirty = true;
        }
    }

    /// Whether the handle's file lives in the read-only boot image.
    fn handle_is_static(&mut self, descriptor: u32) -> bool {
        match self.handle_mut(descriptor) {
            Ok(handle) => {
                let node = handle.node;
                self.nodes[node as usize].storage == STATIC_STORAGE
            }
            Err(_) => true,
        }
    }

    /// The absolute path of a node, written into `out`; returns its length
    /// (0 when it does not fit).
    fn node_path(&self, node: u16, out: &mut [u8; 128]) -> usize {
        let mut position = out.len();
        let mut current = node;
        while current != ROOT_NODE {
            let entry = &self.nodes[current as usize];
            let name = &entry.name[..entry.name_len as usize];
            if position < name.len() + 1 {
                return 0;
            }
            position -= name.len();
            out[position..position + name.len()].copy_from_slice(name);
            position -= 1;
            out[position] = b'/';
            current = entry.parent;
        }
        let length = out.len() - position;
        out.copy_within(position.., 0);
        length
    }

    fn handle_path(&mut self, descriptor: u32) -> Option<([u8; 128], usize)> {
        let node = self.handle_mut(descriptor).ok()?.node;
        let mut buffer = [0u8; 128];
        let length = self.node_path(node, &mut buffer);
        Some(if node == ROOT_NODE {
            buffer[0] = b'/';
            (buffer, 1)
        } else {
            (buffer, length)
        })
    }

    /// If the handle was written to, clears that and returns the file's path.
    fn take_dirty_path(&mut self, descriptor: u32) -> Option<([u8; 128], usize)> {
        let handle = self.handle_mut(descriptor).ok()?;
        if !handle.dirty {
            return None;
        }
        handle.dirty = false;
        let node = handle.node;
        let mut buffer = [0u8; 128];
        let length = self.node_path(node, &mut buffer);
        (length > 0).then_some((buffer, length))
    }

    fn close(&mut self, descriptor: u32) -> Result<(), VfsError> {
        let handle = self.handle_mut(descriptor)?;
        handle.open = false;
        handle.cursor = 0;
        // Bit 15 stays clear so a descriptor never has its top bit set (that
        // marks the persistent mount's descriptors).
        handle.generation = (handle.generation.wrapping_add(1) & 0x7fff).max(1);
        Ok(())
    }

    fn stats(&self, verified: bool) -> VfsStats {
        let mut directories = 0;
        let mut files = 0;
        let mut bytes = 0;
        for node in &self.nodes[..self.node_count] {
            match node.kind {
                NodeKind::Directory => directories += 1,
                NodeKind::File => {
                    files += 1;
                    bytes += if node.storage == STATIC_STORAGE {
                        node.data.len()
                    } else {
                        MUTABLE_FILES.lock()[node.storage as usize].length
                    };
                }
                NodeKind::Symlink | NodeKind::Empty => {}
            }
        }
        let mutable = MUTABLE_FILES.lock();
        VfsStats {
            nodes: self.nodes[..self.node_count]
                .iter()
                .filter(|node| node.kind != NodeKind::Empty)
                .count(),
            directories,
            files,
            bytes,
            open_handles: self.handles.iter().filter(|handle| handle.open).count(),
            mutable_files: mutable.iter().filter(|file| file.used).count(),
            mutable_bytes: mutable.iter().map(|file| file.length).sum(),
            verified,
        }
    }

    fn self_test(&mut self) -> bool {
        let Ok(file) = self.file("//bin/./init") else {
            return false;
        };
        if file.data.get(..4) != Some(b"\x7fELF") || file.mode & READ_BITS == 0 {
            return false;
        }
        if self.resolve("/../../bin/init") != Err(VfsError::Traversal) {
            return false;
        }
        let Ok(descriptor) = self.open("/bin/../bin/init") else {
            return false;
        };
        let mut header = [0; 16];
        let read_valid = self.read(descriptor, &mut header) == Ok(header.len())
            && header[..4] == *b"\x7fELF"
            && self.seek(descriptor, 0, 0) == Ok(0)
            && self.read(descriptor, &mut header[..4]) == Ok(4);
        let close_valid = self.close(descriptor).is_ok()
            && self.read(descriptor, &mut header) == Err(VfsError::BadDescriptor);
        let baseline = self.stats(true);
        let mutation_valid = self.create_directory("/tmp/rename-test", 0o755).is_ok()
            && self
                .open_file("/tmp/rename-test/source", true, true, false, 0o644, true)
                .and_then(|handle| {
                    let written = self.write(handle, b"replacement", false)?;
                    self.close(handle)?;
                    Ok(written)
                })
                == Ok(11)
            && self
                .open_file("/tmp/rename-test/target", true, true, false, 0o644, true)
                .and_then(|handle| {
                    let written = self.write(handle, b"old", false)?;
                    self.close(handle)?;
                    Ok(written)
                })
                == Ok(3)
            && self.rename_noreplace("/tmp/rename-test/source", "/tmp/rename-test/target")
                == Err(VfsError::Exists)
            && self
                .rename("/tmp/rename-test/source", "/tmp/rename-test/target")
                .is_ok()
            && self
                .rename("/tmp/rename-test/target", "/tmp/rename-test/target")
                .is_ok();
        let replacement_valid = self.open("/tmp/rename-test/target").and_then(|handle| {
            let mut contents = [0u8; 11];
            let read = self.read(handle, &mut contents)?;
            self.close(handle)?;
            Ok(read == contents.len() && contents == *b"replacement")
        }) == Ok(true);
        let cleanup_valid = self.remove("/tmp/rename-test/target", false).is_ok()
            && self.remove("/tmp/rename-test", true).is_ok();
        let after = self.stats(true);
        read_valid
            && close_valid
            && mutation_valid
            && replacement_valid
            && cleanup_valid
            && after.nodes == baseline.nodes
            && after.directories == baseline.directories
            && after.files == baseline.files
            && after.bytes == baseline.bytes
            && after.open_handles == baseline.open_handles
            && after.mutable_files == baseline.mutable_files
            && after.mutable_bytes == baseline.mutable_bytes
    }
}

static FILESYSTEM: TicketLock<Vfs> = TicketLock::new(Vfs::new());
static MUTABLE_FILES: TicketLock<[MutableFile; MAX_MUTABLE_FILES]> =
    TicketLock::new([MutableFile::EMPTY; MAX_MUTABLE_FILES]);

pub fn initialize(entries: &[InitramfsEntry]) -> VfsStats {
    *MUTABLE_FILES.lock() = [MutableFile::EMPTY; MAX_MUTABLE_FILES];
    let mut filesystem = FILESYSTEM.lock();
    filesystem.reset();
    let mounted = filesystem.mount(entries).is_ok();
    let verified = mounted && filesystem.self_test();
    filesystem.stats(verified)
}

/// The mount (and the path inside it) a path lives on, when it is under a
/// live persistent mount (`/home`, `/media/<label>`).
fn home(path: &str) -> Option<(usize, &str)> {
    datafs::route(path)
}

fn is_mounted_descriptor(descriptor: u32) -> bool {
    descriptor & datafs::HANDLE_FLAG != 0
}

pub fn file(path: &str) -> Result<FileView, VfsError> {
    if home(path).is_some() {
        // Views of persistent files would have to be 'static; use open+read.
        return Err(VfsError::NotFound);
    }
    FILESYSTEM.lock().file(path)
}

pub fn metadata(path: &str) -> Result<Metadata, VfsError> {
    if let Some(mapped) = crate::procfs::resolve(path) {
        return metadata(mapped.as_str());
    }
    if let Some(mapped) = crate::mounts::resolve(path) {
        return metadata(mapped.as_str());
    }
    if let Some((mount, rest)) = home(path) {
        return datafs::metadata(mount, rest);
    }
    FILESYSTEM.lock().metadata(path)
}

/// Like `metadata`, but a final-component symlink is described, not followed.
pub fn symlink_metadata(path: &str) -> Result<Metadata, VfsError> {
    if let Some(mapped) = crate::procfs::resolve(path) {
        return symlink_metadata(mapped.as_str());
    }
    if let Some(mapped) = crate::mounts::resolve(path) {
        return symlink_metadata(mapped.as_str());
    }
    if let Some((mount, rest)) = home(path) {
        return datafs::metadata(mount, rest);
    }
    FILESYSTEM.lock().symlink_metadata(path)
}

pub fn descriptor_metadata(descriptor: u32) -> Result<Metadata, VfsError> {
    if is_mounted_descriptor(descriptor) {
        return datafs::descriptor_metadata(descriptor);
    }
    FILESYSTEM.lock().descriptor_metadata(descriptor)
}

/// The path an open descriptor was opened with, for `fstatfs`.
pub fn descriptor_path(descriptor: u32) -> Option<([u8; 128], usize)> {
    if is_mounted_descriptor(descriptor) {
        return datafs::descriptor_path(descriptor);
    }
    FILESYSTEM.lock().handle_path(descriptor)
}

pub fn open_file(
    path: &str,
    create: bool,
    exclusive: bool,
    truncate: bool,
    mode: u16,
    write: bool,
) -> Result<u32, VfsError> {
    if crate::procfs::is_virtual_path(path) {
        if create || truncate || write {
            return Err(VfsError::PermissionDenied);
        }
        let mapped = crate::procfs::resolve(path).ok_or(VfsError::NotFound)?;
        return open_file(mapped.as_str(), false, false, false, 0, false);
    }
    if let Some(mapped) = crate::mounts::resolve(path) {
        return open_file(mapped.as_str(), create, exclusive, truncate, mode, write);
    }
    if let Some((mount, rest)) = home(path) {
        let descriptor = datafs::open_file(mount, rest, create, exclusive, truncate, write, path)?;
        if write || truncate {
            datafs::mark_dirty(descriptor);
        }
        // Realtime protection: a file is scanned before anyone reads it.
        if !create && !write && !truncate && !crate::antivirus::on_open(path) {
            let _ = datafs::close(descriptor);
            return Err(VfsError::PermissionDenied);
        }
        return Ok(descriptor);
    }
    let descriptor = FILESYSTEM
        .lock()
        .open_file(path, create, exclusive, truncate, mode, write)?;
    if truncate || write {
        FILESYSTEM.lock().mark_dirty(descriptor);
    }
    // Realtime protection: a writable file is scanned before anyone reads
    // it. (The read-only boot image is trusted and scanned at boot.)
    if !create && !write && !truncate && !FILESYSTEM.lock().handle_is_static(descriptor) {
        let readable = crate::antivirus::on_open(path);
        if !readable {
            let _ = FILESYSTEM.lock().close(descriptor);
            return Err(VfsError::PermissionDenied);
        }
    }
    Ok(descriptor)
}

/// Bounded scratch space for reading a whole persistent-volume file into
/// memory at once - used by `with_home_file` below. `/home`/`/media` have no
/// `'static` in-memory copy the way the initramfs and tmpfs do (their bytes
/// live in this process's own storage arrays already), so there is nothing
/// for `file()` to hand out a zero-copy view into; the bytes have to be read
/// off the FAT volume into *something* first.
const HOME_FILE_MAX: usize = 4 * 1024 * 1024;

struct HomeFileBuffer(core::cell::UnsafeCell<[u8; HOME_FILE_MAX]>);
unsafe impl Sync for HomeFileBuffer {}

static HOME_FILE_BUFFER: HomeFileBuffer =
    HomeFileBuffer(core::cell::UnsafeCell::new([0; HOME_FILE_MAX]));
/// Serializes every use of `HOME_FILE_BUFFER`: held for the whole call below,
/// not just the read, so two callers (e.g. two CPUs both running programs
/// out of `/home`) can never see or overwrite each other's bytes.
static HOME_FILE_LOCK: TicketLock<()> = TicketLock::new(());

/// Reads the whole contents of a `/home` or `/media` file into a shared
/// scratch buffer and calls `action` with them plus the file's mode -
/// the persistent-volume equivalent of `file()`, for callers (running a
/// program, `execve`) that just need to look at the bytes once and are
/// happy with an ordinary borrowed lifetime rather than `file()`'s `'static`
/// one. Goes through the same realtime-protection open-time scan as any
/// other read of a `/home` path. `action` must not itself open another
/// `/home`/`/media` file (that would deadlock on `HOME_FILE_LOCK`).
pub fn with_home_file<R>(path: &str, action: impl FnOnce(&[u8], u16) -> R) -> Result<R, VfsError> {
    if let Some(mapped) = crate::mounts::resolve(path) {
        return with_home_file(mapped.as_str(), action);
    }
    if home(path).is_none() {
        return Err(VfsError::NotFound);
    }
    let _guard = HOME_FILE_LOCK.lock();
    let descriptor = open_file(path, false, false, false, 0, false)?;
    let mode = descriptor_metadata(descriptor)
        .map(|metadata| metadata.mode as u16)
        .unwrap_or(0o755);
    // SAFETY: `_guard` gives this call exclusive use of the buffer for as
    // long as it is held (through the end of `action`), and every other
    // accessor holds the same lock for as long as it touches the buffer.
    let buffer = unsafe { &mut *HOME_FILE_BUFFER.0.get() };
    let mut total = 0usize;
    let outcome = loop {
        match read(descriptor, &mut buffer[total..]) {
            Ok(0) => break Ok(()),
            Ok(count) => {
                total += count;
                if total >= buffer.len() {
                    break Err(VfsError::FileTooLarge);
                }
            }
            Err(failure) => break Err(failure),
        }
    };
    let _ = close(descriptor);
    outcome?;
    Ok(action(&buffer[..total], mode))
}

/// Opens a file for reading without realtime scanning: for the scanner's own
/// use, which must not recurse into itself.
pub fn open_file_raw(path: &str) -> Result<u32, VfsError> {
    if let Some(mapped) = crate::mounts::resolve(path) {
        return open_file_raw(mapped.as_str());
    }
    if let Some((mount, rest)) = home(path) {
        return datafs::open_file(mount, rest, false, false, false, false, path);
    }
    FILESYSTEM
        .lock()
        .open_file(path, false, false, false, 0, false)
}

pub fn open_directory(path: &str) -> Result<u32, VfsError> {
    if let Some(mapped) = crate::procfs::resolve(path) {
        return open_directory(mapped.as_str());
    }
    if let Some(mapped) = crate::mounts::resolve(path) {
        return open_directory(mapped.as_str());
    }
    if let Some((mount, rest)) = home(path) {
        return datafs::open_directory(mount, rest);
    }
    FILESYSTEM.lock().open_directory(path)
}

pub fn next_directory_entry(descriptor: u32) -> Result<Option<DirectoryEntry>, VfsError> {
    if is_mounted_descriptor(descriptor) {
        return datafs::next_directory_entry(descriptor);
    }
    FILESYSTEM.lock().next_directory_entry(descriptor)
}

pub fn read(descriptor: u32, destination: &mut [u8]) -> Result<usize, VfsError> {
    if is_mounted_descriptor(descriptor) {
        return datafs::read(descriptor, destination);
    }
    FILESYSTEM.lock().read(descriptor, destination)
}

pub fn read_at(descriptor: u32, offset: usize, destination: &mut [u8]) -> Result<usize, VfsError> {
    if is_mounted_descriptor(descriptor) {
        return datafs::read_at(descriptor, offset, destination);
    }
    FILESYSTEM.lock().read_at(descriptor, offset, destination)
}

pub fn write_at(descriptor: u32, offset: usize, source: &[u8]) -> Result<usize, VfsError> {
    if is_mounted_descriptor(descriptor) {
        return datafs::write_at(descriptor, offset, source);
    }
    let result = FILESYSTEM.lock().write_at(descriptor, offset, source);
    if result.is_ok() {
        FILESYSTEM.lock().mark_dirty(descriptor);
    }
    result
}

pub fn write(descriptor: u32, source: &[u8], append: bool) -> Result<usize, VfsError> {
    if is_mounted_descriptor(descriptor) {
        return datafs::write(descriptor, source, append);
    }
    let result = FILESYSTEM.lock().write(descriptor, source, append);
    if result.is_ok() {
        FILESYSTEM.lock().mark_dirty(descriptor);
    }
    result
}

pub fn seek(descriptor: u32, offset: i64, whence: u64) -> Result<usize, VfsError> {
    if is_mounted_descriptor(descriptor) {
        return datafs::seek(descriptor, offset, whence);
    }
    FILESYSTEM.lock().seek(descriptor, offset, whence)
}

pub fn close(descriptor: u32) -> Result<(), VfsError> {
    if is_mounted_descriptor(descriptor) {
        return datafs::close(descriptor);
    }
    let written = FILESYSTEM.lock().take_dirty_path(descriptor);
    let result = FILESYSTEM.lock().close(descriptor);
    // Realtime protection: scan a file when its writer is done with it.
    if let Some((buffer, length)) = written
        && let Ok(path) = core::str::from_utf8(&buffer[..length])
    {
        crate::antivirus::on_modified(path);
    }
    result
}

pub fn create_directory(path: &str, mode: u16) -> Result<(), VfsError> {
    if crate::mounts::is_mount_point(path) {
        return Err(VfsError::Exists);
    }
    if let Some(mapped) = crate::mounts::resolve(path) {
        return create_directory(mapped.as_str(), mode);
    }
    if let Some((mount, rest)) = home(path) {
        return datafs::create_directory(mount, rest);
    }
    FILESYSTEM.lock().create_directory(path, mode)
}

/// Creates a symlink at `path` pointing at `target`. `/home` and `/media`
/// are FAT-backed exactly like `/data` and refused for the same reason (see
/// `Filesystem::create_symlink`): only the in-memory tmpfs subtree (`/tmp`)
/// can hold one.
pub fn symlink(path: &str, target: &str) -> Result<(), VfsError> {
    if home(path).is_some() || crate::mounts::resolve(path).is_some() {
        return Err(VfsError::PermissionDenied);
    }
    FILESYSTEM.lock().create_symlink(path, target).map(|_| ())
}

/// The literal target string a symlink at `path` was created with - `path`
/// itself must name the symlink, since this never follows it.
pub fn readlink(path: &str, buffer: &mut [u8]) -> Result<usize, VfsError> {
    if home(path).is_some() || crate::mounts::resolve(path).is_some() {
        return Err(VfsError::NotFound);
    }
    FILESYSTEM.lock().readlink(path, buffer)
}

pub fn remove(path: &str, directory: bool) -> Result<(), VfsError> {
    if crate::mounts::is_mount_point(path) {
        return Err(VfsError::Busy);
    }
    if let Some(mapped) = crate::mounts::resolve(path) {
        return remove(mapped.as_str(), directory);
    }
    if let Some((mount, rest)) = home(path) {
        return datafs::remove(mount, rest, directory);
    }
    FILESYSTEM.lock().remove(path, directory)
}

/// Copies a file between the in-memory tree and a mount, then deletes the
/// source (a rename that crosses mounts).
fn move_across(source: &str, destination: &str, replace: bool) -> Result<(), VfsError> {
    let metadata = metadata(source)?;
    if metadata.mode & 0o170000 == 0o040000 {
        return Err(VfsError::Busy);
    }
    if !replace && self::metadata(destination).is_ok() {
        return Err(VfsError::Exists);
    }
    let input = open_file_raw(source)?;
    let output = match open_file(destination, true, false, true, 0o644, true) {
        Ok(descriptor) => descriptor,
        Err(error) => {
            let _ = close(input);
            return Err(error);
        }
    };
    let mut buffer = [0u8; 4096];
    let mut failure = None;
    loop {
        match read(input, &mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                if let Err(error) = write(output, &buffer[..count], false) {
                    failure = Some(error);
                    break;
                }
            }
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }
    let _ = close(input);
    let _ = close(output);
    if let Some(error) = failure {
        let _ = remove(destination, false);
        return Err(error);
    }
    remove(source, false)
}

/// Rewrites a rename's paths through the bind table; `None` when neither is
/// under a bind. Errors when either is itself a mount point.
fn bound_rename_paths(
    source: &str,
    destination: &str,
) -> Result<Option<(crate::mounts::Mapped, crate::mounts::Mapped)>, VfsError> {
    if crate::mounts::is_mount_point(source) || crate::mounts::is_mount_point(destination) {
        return Err(VfsError::Busy);
    }
    let from = crate::mounts::resolve(source);
    let to = crate::mounts::resolve(destination);
    if from.is_none() && to.is_none() {
        return Ok(None);
    }
    let copy = |mapped: Option<crate::mounts::Mapped>, original: &str| {
        mapped.unwrap_or_else(|| crate::mounts::Mapped::of(original))
    };
    Ok(Some((copy(from, source), copy(to, destination))))
}

pub fn rename(source: &str, destination: &str) -> Result<(), VfsError> {
    if let Some((from, to)) = bound_rename_paths(source, destination)? {
        return rename(from.as_str(), to.as_str());
    }
    match (home(source), home(destination)) {
        (Some((from_mount, from)), Some((to_mount, to))) if from_mount == to_mount => {
            datafs::rename(from_mount, from, to, true)
        }
        (None, None) => FILESYSTEM.lock().rename(source, destination),
        _ => move_across(source, destination, true),
    }
}

pub fn rename_noreplace(source: &str, destination: &str) -> Result<(), VfsError> {
    if let Some((from, to)) = bound_rename_paths(source, destination)? {
        return rename_noreplace(from.as_str(), to.as_str());
    }
    match (home(source), home(destination)) {
        (Some((from_mount, from)), Some((to_mount, to))) if from_mount == to_mount => {
            datafs::rename(from_mount, from, to, false)
        }
        (None, None) => FILESYSTEM.lock().rename_noreplace(source, destination),
        _ => move_across(source, destination, false),
    }
}

pub fn chmod(path: &str, mode: u16) -> Result<(), VfsError> {
    if let Some(mapped) = crate::mounts::resolve(path) {
        return chmod(mapped.as_str(), mode);
    }
    if let Some((mount, rest)) = home(path) {
        return datafs::chmod(mount, rest, mode);
    }
    FILESYSTEM.lock().chmod(path, mode)
}

pub fn fchmod(descriptor: u32, mode: u16) -> Result<(), VfsError> {
    if is_mounted_descriptor(descriptor) {
        return datafs::fchmod(descriptor, mode);
    }
    FILESYSTEM.lock().fchmod(descriptor, mode)
}

pub fn truncate(descriptor: u32, length: usize) -> Result<(), VfsError> {
    if is_mounted_descriptor(descriptor) {
        let result = datafs::truncate(descriptor, length);
        if result.is_ok() {
            datafs::mark_dirty(descriptor);
        }
        return result;
    }
    let result = FILESYSTEM.lock().truncate(descriptor, length);
    if result.is_ok() {
        FILESYSTEM.lock().mark_dirty(descriptor);
    }
    result
}

pub fn stats() -> VfsStats {
    FILESYSTEM.lock().stats(true)
}
