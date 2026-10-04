//! ACPI Machine Language interpreter. Definition blocks (the DSDT and any
//! SSDTs) are executed into a namespace of devices, methods, data objects and
//! operation regions held in fixed tables; methods are interpreted straight
//! from the bytecode. Hardware access goes through `Hooks`, so the interpreter
//! itself knows nothing of the kernel and can be exercised on a host.

pub const MAX_NODES: usize = 12288;
const PERSISTENT_BYTES: usize = 128 * 1024;
const TEMPORARY_BYTES: usize = 64 * 1024;
const PERSISTENT_VALUES: usize = 8192;
const TEMPORARY_VALUES: usize = 2048;
const MAX_TABLES: usize = 16;
const MAX_DEPTH: usize = 24;
const MAX_LOOPS: u32 = 4_000_000;
const MAX_SEGMENTS: usize = 8;
const ROOT: u16 = 0;
const NO_NODE: u16 = 0xffff;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    Parse,
    NotFound,
    Type,
    Unsupported,
    Memory,
    Depth,
    Fatal,
    Timeout,
    Hardware,
}

enum Flow {
    Return(Value),
    Break,
    Continue,
    Fail(Error),
}

impl From<Error> for Flow {
    fn from(error: Error) -> Self {
        Flow::Fail(error)
    }
}

type R<T> = Result<T, Flow>;

fn fail<T>(error: Error) -> R<T> {
    Err(Flow::Fail(error))
}

#[derive(Clone, Copy)]
pub struct Hooks {
    pub io_read: fn(port: u16, bytes: u8) -> u64,
    pub io_write: fn(port: u16, bytes: u8, value: u64),
    pub mem_read: fn(address: u64, bytes: u8) -> Option<u64>,
    pub mem_write: fn(address: u64, bytes: u8, value: u64) -> bool,
    pub pci_read: fn(bus: u8, slot: u8, function: u8, offset: u16, bytes: u8) -> u64,
    pub pci_write: fn(bus: u8, slot: u8, function: u8, offset: u16, bytes: u8, value: u64),
    pub now_ns: fn() -> u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    persistent: bool,
    start: u32,
    len: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RefKind {
    PackageElement,
    Byte,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reference {
    base: Span,
    kind: RefKind,
    at: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Uninit,
    Integer(u64),
    Str(Span),
    Buffer(Span),
    Package(Span),
    Node(u16),
    Ref(Reference),
    /// A name inside a package that did not exist yet when the package was
    /// read; it is looked up when the element is used.
    Unresolved {
        path: Span,
        scope: u16,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FieldKind {
    Normal,
    Index,
    Bank,
}

#[derive(Clone, Copy)]
struct FieldUnit {
    kind: FieldKind,
    /// The region (normal and bank fields) or the index field (index fields).
    first: u16,
    /// The data field (index fields) or the bank-select field (bank fields).
    second: u16,
    bank_value: u64,
    bit_offset: u32,
    bit_len: u32,
    access: u8,
    update: u8,
}

#[derive(Clone, Copy)]
enum Kind {
    Free,
    Scope,
    Device,
    Processor,
    PowerResource,
    ThermalZone,
    Name(Value),
    Method {
        offset: u32,
        end: u32,
        flags: u8,
    },
    Region {
        space: u8,
        base: u64,
        length: u64,
    },
    Field(FieldUnit),
    BufferField {
        source: Span,
        bit_offset: u32,
        bit_len: u32,
    },
    Mutex,
    Event,
    Alias(u16),
    OsInterface,
}

#[derive(Clone, Copy)]
struct Node {
    name: [u8; 4],
    parent: u16,
    table: u8,
    kind: Kind,
}

impl Node {
    const FREE: Self = Self {
        name: [0; 4],
        parent: NO_NODE,
        table: 0,
        kind: Kind::Free,
    };
}

/// What a namespace object is, for callers outside the interpreter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeKind {
    Scope,
    Device,
    Processor,
    PowerResource,
    ThermalZone,
    Data,
    Method,
    Region,
    Field,
    Mutex,
    Event,
    Alias,
}

#[derive(Clone, Copy)]
struct Path {
    root: bool,
    up: u8,
    count: u8,
    segments: [[u8; 4]; MAX_SEGMENTS],
}

enum Target {
    None,
    Local(u8),
    Arg(u8),
    Node(u16),
    Debug,
    Reference(Value),
}

struct Frame {
    code: &'static [u8],
    pos: usize,
    end: usize,
    scope: u16,
    args: [Value; 7],
    locals: [Value; 8],
}

impl Frame {
    fn new(code: &'static [u8], pos: usize, end: usize, scope: u16) -> Self {
        Self {
            code,
            pos,
            end: end.min(code.len()),
            scope,
            args: [Value::Uninit; 7],
            locals: [Value::Uninit; 8],
        }
    }

    fn peek(&self) -> R<u8> {
        if self.pos < self.end {
            Ok(self.code[self.pos])
        } else {
            fail(Error::Parse)
        }
    }

    fn peek_at(&self, ahead: usize) -> R<u8> {
        match self.code.get(self.pos + ahead) {
            Some(byte) if self.pos + ahead < self.end => Ok(*byte),
            _ => fail(Error::Parse),
        }
    }

    fn next(&mut self) -> R<u8> {
        let byte = self.peek()?;
        self.pos += 1;
        Ok(byte)
    }

    fn take(&mut self, count: usize) -> R<u64> {
        let mut value = 0u64;
        for index in 0..count {
            value |= (self.next()? as u64) << (8 * index);
        }
        Ok(value)
    }

    /// The decoded value of a PkgLength (without counting its own bytes
    /// separately from the start: callers that want an end position use `pkg_end`).
    fn pkg_value(&mut self) -> R<(usize, usize)> {
        let start = self.pos;
        let lead = self.next()?;
        let extra = (lead >> 6) as usize;
        let mut length = if extra == 0 {
            (lead & 0x3f) as usize
        } else {
            (lead & 0x0f) as usize
        };
        for index in 0..extra {
            length |= (self.next()? as usize) << (4 + 8 * index);
        }
        Ok((start, length))
    }

    /// Reads a PkgLength and returns the position where the package ends.
    fn pkg_end(&mut self) -> R<usize> {
        let (start, length) = self.pkg_value()?;
        let end = start.checked_add(length).ok_or(Flow::Fail(Error::Parse))?;
        if end > self.end || end < self.pos {
            return fail(Error::Parse);
        }
        Ok(end)
    }
}

fn is_lead(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_uppercase()
}

fn is_name_char(byte: u8) -> bool {
    is_lead(byte) || byte.is_ascii_digit()
}

fn is_name_start(byte: u8) -> bool {
    is_lead(byte) || matches!(byte, 0x5c | 0x5e | 0x2e | 0x2f)
}

fn read_segment(f: &mut Frame) -> R<[u8; 4]> {
    let mut segment = [0u8; 4];
    for (index, slot) in segment.iter_mut().enumerate() {
        let byte = f.next()?;
        if (index == 0 && !is_lead(byte)) || !is_name_char(byte) {
            return fail(Error::Parse);
        }
        *slot = byte;
    }
    Ok(segment)
}

fn parse_path(f: &mut Frame) -> R<Path> {
    let mut path = Path {
        root: false,
        up: 0,
        count: 0,
        segments: [[0; 4]; MAX_SEGMENTS],
    };
    if f.peek()? == 0x5c {
        path.root = true;
        f.pos += 1;
    } else {
        while f.peek()? == 0x5e {
            path.up = path.up.saturating_add(1);
            f.pos += 1;
        }
    }
    let count = match f.peek()? {
        0x00 => {
            f.pos += 1;
            0
        }
        0x2e => {
            f.pos += 1;
            2
        }
        0x2f => {
            f.pos += 1;
            f.next()? as usize
        }
        _ => 1,
    };
    if count > MAX_SEGMENTS {
        return fail(Error::Unsupported);
    }
    for index in 0..count {
        path.segments[index] = read_segment(f)?;
    }
    path.count = count as u8;
    Ok(path)
}

fn pad_name(name: &[u8]) -> [u8; 4] {
    let mut segment = [b'_'; 4];
    for (slot, byte) in segment.iter_mut().zip(name.iter().take(4)) {
        *slot = *byte;
    }
    segment
}

fn parse_text_path(text: &str) -> Option<Path> {
    let bytes = text.as_bytes();
    let mut path = Path {
        root: false,
        up: 0,
        count: 0,
        segments: [[0; 4]; MAX_SEGMENTS],
    };
    let mut rest = bytes;
    if rest.first() == Some(&b'\\') {
        path.root = true;
        rest = &rest[1..];
    }
    while !rest.is_empty() {
        let end = rest
            .iter()
            .position(|byte| *byte == b'.')
            .unwrap_or(rest.len());
        if end == 0 || end > 4 || path.count as usize == MAX_SEGMENTS {
            return None;
        }
        path.segments[path.count as usize] = pad_name(&rest[..end]);
        path.count += 1;
        rest = rest.get(end + 1..).unwrap_or(&[]);
    }
    Some(path)
}

pub struct Aml {
    hooks: Hooks,
    tables: [(u64, u32); MAX_TABLES],
    table_count: usize,
    nodes: [Node; MAX_NODES],
    node_count: usize,
    persistent_bytes: [u8; PERSISTENT_BYTES],
    persistent_bytes_used: usize,
    temporary_bytes: [u8; TEMPORARY_BYTES],
    temporary_bytes_used: usize,
    persistent_values: [Value; PERSISTENT_VALUES],
    persistent_values_used: usize,
    temporary_values: [Value; TEMPORARY_VALUES],
    temporary_values_used: usize,
    persist_mode: bool,
    depth: usize,
    method_mark: usize,
    int_mask: u64,
    pub notifications: u64,
    pub load_errors: u32,
}

impl Aml {
    pub const fn new(hooks: Hooks) -> Self {
        let mut aml = Self {
            hooks,
            tables: [(0, 0); MAX_TABLES],
            table_count: 0,
            nodes: [Node::FREE; MAX_NODES],
            node_count: 1,
            persistent_bytes: [0; PERSISTENT_BYTES],
            persistent_bytes_used: 0,
            temporary_bytes: [0; TEMPORARY_BYTES],
            temporary_bytes_used: 0,
            persistent_values: [Value::Uninit; PERSISTENT_VALUES],
            persistent_values_used: 0,
            temporary_values: [Value::Uninit; TEMPORARY_VALUES],
            temporary_values_used: 0,
            persist_mode: true,
            depth: 0,
            method_mark: 0,
            int_mask: u64::MAX,
            notifications: 0,
            load_errors: 0,
        };
        aml.nodes[0] = Node {
            name: *b"\\___",
            parent: NO_NODE,
            table: 0,
            kind: Kind::Scope,
        };
        aml
    }

    /// Sets up the predefined scopes and objects. Call once before loading.
    pub fn reset(&mut self) {
        self.table_count = 0;
        self.node_count = 1;
        self.nodes[0].kind = Kind::Scope;
        self.persistent_bytes_used = 0;
        self.persistent_values_used = 0;
        self.temporary_bytes_used = 0;
        self.temporary_values_used = 0;
        self.persist_mode = true;
        self.depth = 0;
        self.method_mark = 0;
        self.notifications = 0;
        self.load_errors = 0;
        for scope in [b"_GPE", b"_PR_", b"_SB_", b"_SI_", b"_TZ_"] {
            let _ = self.add(ROOT, *scope, Kind::Scope);
        }
        let _ = self.add(ROOT, *b"_GL_", Kind::Mutex);
        let _ = self.add(ROOT, *b"_REV", Kind::Name(Value::Integer(2)));
        let _ = self.add(ROOT, *b"_OSI", Kind::OsInterface);
        if let Ok(os) = self.make_string(b"Microsoft Windows NT", true) {
            let _ = self.add(ROOT, *b"_OS_", Kind::Name(os));
        }
    }

    // ----- tables -------------------------------------------------------

    fn code(&self, table: u8) -> &'static [u8] {
        let (address, length) = self.tables[table as usize];
        // SAFETY: a table registered by `load`, which the caller keeps alive
        // and readable for as long as the interpreter runs.
        unsafe { core::slice::from_raw_parts(address as usize as *const u8, length as usize) }
    }

    /// Executes the definition block at `address` into the namespace.
    pub fn load(&mut self, address: u64, length: usize) -> Result<(), Error> {
        if self.table_count == MAX_TABLES || length < 36 {
            return Err(Error::Memory);
        }
        let index = self.table_count;
        self.tables[index] = (address, length as u32);
        self.table_count += 1;
        let code = self.code(index as u8);
        if index == 0 {
            self.int_mask = if code[8] >= 2 { u64::MAX } else { 0xffff_ffff };
        }
        let mut frame = Frame::new(code, 36, length, ROOT);
        self.persist_mode = true;
        match self.exec_block(&mut frame, length) {
            Ok(()) => Ok(()),
            Err(Flow::Fail(error)) => {
                self.load_errors += 1;
                Err(error)
            }
            Err(_) => Err(Error::Parse),
        }
    }

    // ----- namespace ----------------------------------------------------

    fn child(&self, parent: u16, name: [u8; 4]) -> Option<u16> {
        (0..self.node_count)
            .find(|&index| {
                let node = &self.nodes[index];
                node.parent == parent && node.name == name && !matches!(node.kind, Kind::Free)
            })
            .map(|index| index as u16)
    }

    fn add(&mut self, parent: u16, name: [u8; 4], kind: Kind) -> R<u16> {
        if let Some(existing) = self.child(parent, name) {
            self.nodes[existing as usize].kind = kind;
            return Ok(existing);
        }
        if self.node_count == MAX_NODES {
            return fail(Error::Memory);
        }
        let index = self.node_count;
        self.nodes[index] = Node {
            name,
            parent,
            table: self.table_count.saturating_sub(1) as u8,
            kind,
        };
        self.node_count += 1;
        Ok(index as u16)
    }

    fn truncate_nodes(&mut self, mark: usize) {
        for index in mark..self.node_count {
            self.nodes[index] = Node::FREE;
        }
        self.node_count = self.node_count.min(mark).max(1);
    }

    fn deref_alias(&self, mut node: u16) -> u16 {
        for _ in 0..8 {
            match self.nodes[node as usize].kind {
                Kind::Alias(target) => node = target,
                _ => break,
            }
        }
        node
    }

    fn walk_prefix(&self, scope: u16, path: &Path) -> Option<u16> {
        let mut node = if path.root { ROOT } else { scope };
        if !path.root {
            for _ in 0..path.up {
                let parent = self.nodes[node as usize].parent;
                node = if parent == NO_NODE { ROOT } else { parent };
            }
        }
        Some(node)
    }

    /// Resolves a name the way expressions do: a bare name is looked up in the
    /// current scope and then in each enclosing one.
    fn resolve(&self, scope: u16, path: &Path) -> Option<u16> {
        let start = self.walk_prefix(scope, path)?;
        if path.count == 0 {
            return Some(start);
        }
        let first = path.segments[0];
        let mut node = if !path.root && path.up == 0 {
            let mut search = scope;
            loop {
                if let Some(found) = self.child(search, first) {
                    break found;
                }
                let parent = self.nodes[search as usize].parent;
                if parent == NO_NODE {
                    return None;
                }
                search = parent;
            }
        } else {
            self.child(start, first)?
        };
        for index in 1..path.count as usize {
            node = self.child(self.deref_alias(node), path.segments[index])?;
        }
        Some(self.deref_alias(node))
    }

    /// Resolves a name without the enclosing-scope search (scopes, definitions).
    fn resolve_exact(&self, scope: u16, path: &Path) -> Option<u16> {
        let mut node = self.walk_prefix(scope, path)?;
        for index in 0..path.count as usize {
            node = self.child(node, path.segments[index])?;
        }
        Some(node)
    }

    /// The scope a new object named by `path` belongs to, and its name.
    fn creation_point(&mut self, scope: u16, path: &Path) -> R<(u16, [u8; 4])> {
        if path.count == 0 {
            return fail(Error::Parse);
        }
        let mut node = self
            .walk_prefix(scope, path)
            .ok_or(Flow::Fail(Error::Parse))?;
        for index in 0..path.count as usize - 1 {
            node = match self.child(node, path.segments[index]) {
                Some(found) => found,
                None => self.add(node, path.segments[index], Kind::Scope)?,
            };
        }
        Ok((node, path.segments[path.count as usize - 1]))
    }

    pub fn find(&self, text: &str) -> Option<u16> {
        let path = parse_text_path(text)?;
        let mut node = ROOT;
        for index in 0..path.count as usize {
            node = self.deref_alias(self.child(node, path.segments[index])?);
        }
        Some(node)
    }

    pub fn child_node(&self, parent: u16, name: &[u8; 4]) -> Option<u16> {
        self.child(parent, *name)
    }

    pub fn node_count(&self) -> usize {
        self.node_count
    }

    #[allow(dead_code)]
    pub fn node_name(&self, node: u16) -> [u8; 4] {
        self.nodes[node as usize].name
    }

    pub fn node_kind(&self, node: u16) -> NodeKind {
        match self.nodes[node as usize].kind {
            Kind::Free | Kind::Scope => NodeKind::Scope,
            Kind::Device => NodeKind::Device,
            Kind::Processor => NodeKind::Processor,
            Kind::PowerResource => NodeKind::PowerResource,
            Kind::ThermalZone => NodeKind::ThermalZone,
            Kind::Name(_) | Kind::BufferField { .. } => NodeKind::Data,
            Kind::Method { .. } | Kind::OsInterface => NodeKind::Method,
            Kind::Region { .. } => NodeKind::Region,
            Kind::Field(_) => NodeKind::Field,
            Kind::Mutex => NodeKind::Mutex,
            Kind::Event => NodeKind::Event,
            Kind::Alias(_) => NodeKind::Alias,
        }
    }

    /// The `\_SB_.PCI0` form of a node's path, written into `out`.
    pub fn node_path(&self, node: u16, out: &mut [u8]) -> usize {
        let mut chain = [0u16; 16];
        let mut depth = 0;
        let mut at = node;
        while at != ROOT && at != NO_NODE && depth < chain.len() {
            chain[depth] = at;
            depth += 1;
            at = self.nodes[at as usize].parent;
        }
        let mut length = 0;
        let mut push = |byte: u8| {
            if length < out.len() {
                out[length] = byte;
                length += 1;
            }
        };
        push(b'\\');
        for (position, index) in (0..depth).rev().enumerate() {
            if position != 0 {
                push(b'.');
            }
            for byte in self.nodes[chain[index] as usize].name {
                push(byte);
            }
        }
        length
    }

    // ----- value storage ------------------------------------------------

    fn alloc_bytes(&mut self, persistent: bool, length: usize) -> R<Span> {
        let (used, capacity) = if persistent {
            (&mut self.persistent_bytes_used, PERSISTENT_BYTES)
        } else {
            (&mut self.temporary_bytes_used, TEMPORARY_BYTES)
        };
        if *used + length > capacity {
            return fail(Error::Memory);
        }
        let start = *used;
        *used += length;
        let span = Span {
            persistent,
            start: start as u32,
            len: length as u32,
        };
        self.bytes_mut(span).fill(0);
        Ok(span)
    }

    fn alloc_values(&mut self, persistent: bool, count: usize) -> R<Span> {
        let (used, capacity) = if persistent {
            (&mut self.persistent_values_used, PERSISTENT_VALUES)
        } else {
            (&mut self.temporary_values_used, TEMPORARY_VALUES)
        };
        if *used + count > capacity {
            return fail(Error::Memory);
        }
        let start = *used;
        *used += count;
        let span = Span {
            persistent,
            start: start as u32,
            len: count as u32,
        };
        for slot in self.values_mut(span) {
            *slot = Value::Uninit;
        }
        Ok(span)
    }

    fn bytes(&self, span: Span) -> &[u8] {
        let range = span.start as usize..(span.start + span.len) as usize;
        if span.persistent {
            &self.persistent_bytes[range]
        } else {
            &self.temporary_bytes[range]
        }
    }

    fn bytes_mut(&mut self, span: Span) -> &mut [u8] {
        let range = span.start as usize..(span.start + span.len) as usize;
        if span.persistent {
            &mut self.persistent_bytes[range]
        } else {
            &mut self.temporary_bytes[range]
        }
    }

    fn values(&self, span: Span) -> &[Value] {
        let range = span.start as usize..(span.start + span.len) as usize;
        if span.persistent {
            &self.persistent_values[range]
        } else {
            &self.temporary_values[range]
        }
    }

    fn values_mut(&mut self, span: Span) -> &mut [Value] {
        let range = span.start as usize..(span.start + span.len) as usize;
        if span.persistent {
            &mut self.persistent_values[range]
        } else {
            &mut self.temporary_values[range]
        }
    }

    fn make_string(&mut self, text: &[u8], persistent: bool) -> R<Value> {
        let span = self.alloc_bytes(persistent, text.len())?;
        self.bytes_mut(span).copy_from_slice(text);
        Ok(Value::Str(span))
    }

    fn make_buffer(&mut self, persistent: bool, data: &[u8]) -> R<Value> {
        let span = self.alloc_bytes(persistent, data.len())?;
        self.bytes_mut(span).copy_from_slice(data);
        Ok(Value::Buffer(span))
    }

    /// A copy of `value` that survives the end of the current evaluation.
    fn persist(&mut self, value: Value) -> R<Value> {
        match value {
            Value::Str(span) if !span.persistent => {
                let mut copy = [0u8; 512];
                let length = span.len as usize;
                if length > copy.len() {
                    return fail(Error::Memory);
                }
                copy[..length].copy_from_slice(self.bytes(span));
                self.make_string(&copy[..length], true)
            }
            Value::Buffer(span) if !span.persistent => {
                let target = self.alloc_bytes(true, span.len as usize)?;
                for index in 0..span.len as usize {
                    let byte = self.bytes(span)[index];
                    self.bytes_mut(target)[index] = byte;
                }
                Ok(Value::Buffer(target))
            }
            Value::Unresolved { path, scope } if !path.persistent => {
                let target = self.alloc_bytes(true, path.len as usize)?;
                for index in 0..path.len as usize {
                    let byte = self.bytes(path)[index];
                    self.bytes_mut(target)[index] = byte;
                }
                Ok(Value::Unresolved {
                    path: target,
                    scope,
                })
            }
            Value::Package(span) if !span.persistent => {
                let target = self.alloc_values(true, span.len as usize)?;
                for index in 0..span.len as usize {
                    let element = self.values(span)[index];
                    let copy = self.persist(element)?;
                    self.values_mut(target)[index] = copy;
                }
                Ok(Value::Package(target))
            }
            other => Ok(other),
        }
    }

    // ----- conversions --------------------------------------------------

    fn mask(&self, value: u64) -> u64 {
        value & self.int_mask
    }

    fn truth(&self, condition: bool) -> Value {
        Value::Integer(if condition { self.int_mask } else { 0 })
    }

    fn deref(&mut self, value: Value) -> R<Value> {
        match value {
            Value::Ref(reference) => self.read_reference(reference),
            other => Ok(other),
        }
    }

    /// Looks a package element up if it was a name that could not be found
    /// when the package was built.
    fn settle(&self, value: Value) -> Value {
        let Value::Unresolved { path, scope } = value else {
            return value;
        };
        let encoded = self.bytes(path);
        let mut parsed = Path {
            root: encoded[0] != 0,
            up: encoded[1],
            count: encoded[2],
            segments: [[0; 4]; MAX_SEGMENTS],
        };
        for index in 0..parsed.count as usize {
            parsed.segments[index].copy_from_slice(&encoded[3 + 4 * index..7 + 4 * index]);
        }
        match self.resolve(scope, &parsed) {
            Some(node) => match self.nodes[node as usize].kind {
                Kind::Name(value) => value,
                _ => Value::Node(node),
            },
            None => Value::Uninit,
        }
    }

    fn read_reference(&mut self, reference: Reference) -> R<Value> {
        match reference.kind {
            RefKind::PackageElement => self
                .values(reference.base)
                .get(reference.at as usize)
                .copied()
                .map(|value| self.settle(value))
                .ok_or(Flow::Fail(Error::Parse)),
            RefKind::Byte => self
                .bytes(reference.base)
                .get(reference.at as usize)
                .map(|byte| Value::Integer(*byte as u64))
                .ok_or(Flow::Fail(Error::Parse)),
        }
    }

    fn integer_of(&mut self, value: Value) -> R<u64> {
        match self.deref(value)? {
            Value::Integer(number) => Ok(number),
            Value::Buffer(span) => {
                let mut number = 0u64;
                for (index, byte) in self.bytes(span).iter().take(8).enumerate() {
                    number |= (*byte as u64) << (8 * index);
                }
                Ok(self.mask(number))
            }
            Value::Str(span) => Ok(self.mask(parse_integer(self.bytes(span)))),
            _ => fail(Error::Type),
        }
    }

    /// The bytes of a buffer or string operand, or of an integer in its
    /// natural width.
    fn bytes_of(&mut self, value: Value, out: &mut [u8; 8]) -> R<usize> {
        match self.deref(value)? {
            Value::Integer(number) => {
                let width = if self.int_mask == u64::MAX { 8 } else { 4 };
                out[..width].copy_from_slice(&number.to_le_bytes()[..width]);
                Ok(width)
            }
            _ => fail(Error::Type),
        }
    }

    // ----- hardware regions --------------------------------------------

    fn region_info(&self, region: u16) -> R<(u8, u64, u64)> {
        match self.nodes[region as usize].kind {
            Kind::Region {
                space,
                base,
                length,
            } => Ok((space, base, length)),
            _ => fail(Error::Type),
        }
    }

    fn name_integer(&self, parent: u16, name: &[u8; 4]) -> Option<u64> {
        let node = self.child(parent, *name)?;
        match self.nodes[node as usize].kind {
            Kind::Name(Value::Integer(number)) => Some(number),
            _ => None,
        }
    }

    /// The PCI function a `PCI_Config` region belongs to: the nearest
    /// enclosing device with an `_ADR`, directly below a root bridge.
    fn pci_function(&self, region: u16) -> R<(u8, u8, u8)> {
        let mut at = self.nodes[region as usize].parent;
        while at != NO_NODE {
            if let Some(address) = self.name_integer(at, b"_ADR") {
                let parent = self.nodes[at as usize].parent;
                let bus = if parent != NO_NODE && self.name_integer(parent, b"_ADR").is_some() {
                    return fail(Error::Unsupported);
                } else if parent != NO_NODE {
                    self.name_integer(parent, b"_BBN").unwrap_or(0) as u8
                } else {
                    0
                };
                return Ok((bus, (address >> 16) as u8 & 0x1f, address as u8 & 7));
            }
            at = self.nodes[at as usize].parent;
        }
        fail(Error::NotFound)
    }

    fn region_read(&mut self, region: u16, offset: u64, bytes: u8) -> R<u64> {
        let (space, base, length) = self.region_info(region)?;
        if offset + bytes as u64 > length {
            return fail(Error::Hardware);
        }
        match space {
            0 => (self.hooks.mem_read)(base + offset, bytes).ok_or(Flow::Fail(Error::Hardware)),
            1 => Ok((self.hooks.io_read)((base + offset) as u16, bytes)),
            2 => {
                let (bus, slot, function) = self.pci_function(region)?;
                Ok((self.hooks.pci_read)(
                    bus,
                    slot,
                    function,
                    (base + offset) as u16,
                    bytes,
                ))
            }
            _ => fail(Error::Unsupported),
        }
    }

    fn region_write(&mut self, region: u16, offset: u64, bytes: u8, value: u64) -> R<()> {
        let (space, base, length) = self.region_info(region)?;
        if offset + bytes as u64 > length {
            return fail(Error::Hardware);
        }
        match space {
            0 => {
                if (self.hooks.mem_write)(base + offset, bytes, value) {
                    Ok(())
                } else {
                    fail(Error::Hardware)
                }
            }
            1 => {
                (self.hooks.io_write)((base + offset) as u16, bytes, value);
                Ok(())
            }
            2 => {
                let (bus, slot, function) = self.pci_function(region)?;
                (self.hooks.pci_write)(bus, slot, function, (base + offset) as u16, bytes, value);
                Ok(())
            }
            _ => fail(Error::Unsupported),
        }
    }

    fn access_bytes(access: u8) -> u8 {
        match access & 0x0f {
            2 => 2,
            3 => 4,
            4 => 8,
            _ => 1,
        }
    }

    fn unit_read(&mut self, field: &FieldUnit, offset: u64, bytes: u8) -> R<u64> {
        match field.kind {
            FieldKind::Normal => self.region_read(field.first, offset, bytes),
            FieldKind::Index => {
                self.write_integer_field(field.first, offset)?;
                let data = self.read_field(field.second)?;
                self.integer_of(data)
            }
            FieldKind::Bank => {
                self.write_integer_field(field.second, field.bank_value)?;
                self.region_read(field.first, offset, bytes)
            }
        }
    }

    fn unit_write(&mut self, field: &FieldUnit, offset: u64, bytes: u8, value: u64) -> R<()> {
        match field.kind {
            FieldKind::Normal => self.region_write(field.first, offset, bytes, value),
            FieldKind::Index => {
                self.write_integer_field(field.first, offset)?;
                self.write_integer_field(field.second, value)
            }
            FieldKind::Bank => {
                self.write_integer_field(field.second, field.bank_value)?;
                self.region_write(field.first, offset, bytes, value)
            }
        }
    }

    fn write_integer_field(&mut self, node: u16, value: u64) -> R<()> {
        self.write_field(node, Value::Integer(value))
    }

    fn field_unit(&self, node: u16) -> R<FieldUnit> {
        match self.nodes[node as usize].kind {
            Kind::Field(field) => Ok(field),
            _ => fail(Error::Type),
        }
    }

    fn read_field(&mut self, node: u16) -> R<Value> {
        let field = self.field_unit(node)?;
        let unit = Self::access_bytes(field.access);
        let unit_bits = unit as u32 * 8;
        let end_bit = field.bit_offset + field.bit_len;
        let big = field.bit_len > 64;
        let buffer = if big {
            Some(self.alloc_bytes(false, field.bit_len.div_ceil(8) as usize)?)
        } else {
            None
        };
        let mut result = 0u64;
        let mut bit = field.bit_offset;
        let mut produced = 0u32;
        while bit < end_bit {
            let in_unit = bit % unit_bits;
            let take = (unit_bits - in_unit).min(end_bit - bit);
            let raw = self.unit_read(&field, (bit / unit_bits) as u64 * unit as u64, unit)?;
            let piece = shift_right(raw, in_unit) & low_bits(take);
            match buffer {
                Some(span) => put_bits(self.bytes_mut(span), produced, piece, take),
                None => result |= piece << produced,
            }
            bit += take;
            produced += take;
        }
        Ok(match buffer {
            Some(span) => Value::Buffer(span),
            None => Value::Integer(result),
        })
    }

    fn write_field(&mut self, node: u16, value: Value) -> R<()> {
        let field = self.field_unit(node)?;
        let unit = Self::access_bytes(field.access);
        let unit_bits = unit as u32 * 8;
        let end_bit = field.bit_offset + field.bit_len;
        let source = self.deref(value)?;
        let mut bit = field.bit_offset;
        let mut consumed = 0u32;
        while bit < end_bit {
            let in_unit = bit % unit_bits;
            let take = (unit_bits - in_unit).min(end_bit - bit);
            let piece = match source {
                Value::Buffer(span) => get_bits(self.bytes(span), consumed, take),
                other => {
                    let number = self.integer_of(other)?;
                    if consumed >= 64 {
                        0
                    } else {
                        shift_right(number, consumed) & low_bits(take)
                    }
                }
            };
            let offset = (bit / unit_bits) as u64 * unit as u64;
            let covers_unit = take == unit_bits;
            let merged = if covers_unit {
                piece
            } else {
                let base = match field.update {
                    1 => u64::MAX,
                    2 => 0,
                    _ => self.unit_read(&field, offset, unit)?,
                };
                let mask = low_bits(take) << in_unit;
                (base & !mask) | ((piece << in_unit) & mask)
            };
            self.unit_write(&field, offset, unit, merged & low_bits(unit_bits))?;
            bit += take;
            consumed += take;
        }
        Ok(())
    }

    fn read_buffer_field(&mut self, source: Span, bit_offset: u32, bit_len: u32) -> R<Value> {
        if (bit_offset + bit_len).div_ceil(8) > source.len {
            return fail(Error::Parse);
        }
        if bit_len > 64 {
            let span = self.alloc_bytes(false, bit_len.div_ceil(8) as usize)?;
            for index in 0..bit_len.div_ceil(8) {
                let take = (bit_len - index * 8).min(8);
                let piece = get_bits(self.bytes(source), bit_offset + index * 8, take);
                self.bytes_mut(span)[index as usize] = piece as u8;
            }
            return Ok(Value::Buffer(span));
        }
        Ok(Value::Integer(get_bits(
            self.bytes(source),
            bit_offset,
            bit_len,
        )))
    }

    fn write_buffer_field(
        &mut self,
        source: Span,
        bit_offset: u32,
        bit_len: u32,
        value: Value,
    ) -> R<()> {
        if (bit_offset + bit_len).div_ceil(8) > source.len {
            return fail(Error::Parse);
        }
        let value = self.deref(value)?;
        let mut done = 0;
        while done < bit_len {
            let take = (bit_len - done).min(64);
            let piece = match value {
                Value::Buffer(span) => get_bits(self.bytes(span), done, take),
                other => {
                    let number = self.integer_of(other)?;
                    if done >= 64 {
                        0
                    } else {
                        number & low_bits(take)
                    }
                }
            };
            put_bits(self.bytes_mut(source), bit_offset + done, piece, take);
            done += take;
        }
        Ok(())
    }

    // ----- reading and writing named objects ----------------------------

    fn read_node(&mut self, node: u16) -> R<Value> {
        let node = self.deref_alias(node);
        match self.nodes[node as usize].kind {
            Kind::Name(value) => Ok(value),
            Kind::Field(_) => self.read_field(node),
            Kind::BufferField {
                source,
                bit_offset,
                bit_len,
            } => self.read_buffer_field(source, bit_offset, bit_len),
            _ => Ok(Value::Node(node)),
        }
    }

    fn write_node(&mut self, node: u16, value: Value) -> R<()> {
        let node = self.deref_alias(node);
        match self.nodes[node as usize].kind {
            Kind::Name(old) => {
                let value = self.deref(value)?;
                let value = if (node as usize) < self.method_mark || self.depth == 0 {
                    self.persist(value)?
                } else {
                    value
                };
                let value = match (old, value) {
                    (Value::Integer(_), Value::Integer(number)) => {
                        Value::Integer(self.mask(number))
                    }
                    _ => value,
                };
                self.nodes[node as usize].kind = Kind::Name(value);
                Ok(())
            }
            Kind::Field(_) => self.write_field(node, value),
            Kind::BufferField {
                source,
                bit_offset,
                bit_len,
            } => self.write_buffer_field(source, bit_offset, bit_len, value),
            _ => fail(Error::Type),
        }
    }

    fn write_reference(&mut self, reference: Reference, value: Value) -> R<()> {
        match reference.kind {
            RefKind::PackageElement => {
                let value = self.deref(value)?;
                let value = if reference.base.persistent {
                    self.persist(value)?
                } else {
                    value
                };
                match self
                    .values_mut(reference.base)
                    .get_mut(reference.at as usize)
                {
                    Some(slot) => {
                        *slot = value;
                        Ok(())
                    }
                    None => fail(Error::Parse),
                }
            }
            RefKind::Byte => {
                let byte = self.integer_of(value)? as u8;
                match self
                    .bytes_mut(reference.base)
                    .get_mut(reference.at as usize)
                {
                    Some(slot) => {
                        *slot = byte;
                        Ok(())
                    }
                    None => fail(Error::Parse),
                }
            }
        }
    }

    fn store(&mut self, f: &mut Frame, target: Target, value: Value) -> R<()> {
        match target {
            Target::None | Target::Debug => Ok(()),
            Target::Local(index) => {
                f.locals[index as usize] = value;
                Ok(())
            }
            Target::Arg(index) => {
                if let Value::Ref(reference) = f.args[index as usize] {
                    self.write_reference(reference, value)
                } else {
                    f.args[index as usize] = value;
                    Ok(())
                }
            }
            Target::Node(node) => self.write_node(node, value),
            Target::Reference(Value::Ref(reference)) => self.write_reference(reference, value),
            Target::Reference(Value::Node(node)) => self.write_node(node, value),
            Target::Reference(_) => fail(Error::Type),
        }
    }

    fn read_target(&mut self, f: &mut Frame, target: &Target) -> R<Value> {
        match target {
            Target::None | Target::Debug => Ok(Value::Integer(0)),
            Target::Local(index) => Ok(f.locals[*index as usize]),
            Target::Arg(index) => {
                let value = f.args[*index as usize];
                self.deref(value)
            }
            Target::Node(node) => self.read_node(*node),
            Target::Reference(value) => self.deref(*value),
        }
    }

    fn parse_target(&mut self, f: &mut Frame) -> R<Target> {
        let op = f.peek()?;
        match op {
            0x00 => {
                f.pos += 1;
                Ok(Target::None)
            }
            0x60..=0x67 => {
                f.pos += 1;
                Ok(Target::Local(op - 0x60))
            }
            0x68..=0x6e => {
                f.pos += 1;
                Ok(Target::Arg(op - 0x68))
            }
            0x5b if f.peek_at(1)? == 0x31 => {
                f.pos += 2;
                Ok(Target::Debug)
            }
            0x83 => {
                f.pos += 1;
                let value = self.term_arg(f)?;
                Ok(Target::Reference(value))
            }
            0x88 => {
                let value = self.term_arg(f)?;
                Ok(Target::Reference(value))
            }
            byte if is_name_start(byte) => {
                let path = parse_path(f)?;
                let node = self
                    .resolve(f.scope, &path)
                    .ok_or(Flow::Fail(Error::NotFound))?;
                Ok(Target::Node(node))
            }
            _ => fail(Error::Parse),
        }
    }

    // ----- execution ----------------------------------------------------

    fn exec_block(&mut self, f: &mut Frame, end: usize) -> R<()> {
        while f.pos < end {
            self.exec_term(f)?;
        }
        Ok(())
    }

    /// Runs the body of a scope-like term. While loading, a failure inside
    /// only costs that body: the walk resumes after it.
    fn exec_body(&mut self, f: &mut Frame, scope: u16, end: usize) -> R<()> {
        let saved = f.scope;
        f.scope = scope;
        let result = self.exec_block(f, end);
        f.scope = saved;
        f.pos = end;
        match result {
            Err(Flow::Fail(_)) if self.depth == 0 => {
                self.load_errors += 1;
                Ok(())
            }
            other => other,
        }
    }

    fn define(&mut self, f: &mut Frame, path: &Path, kind: Kind) -> R<u16> {
        let (parent, name) = self.creation_point(f.scope, path)?;
        self.add(parent, name, kind)
    }

    fn exec_term(&mut self, f: &mut Frame) -> R<()> {
        let op = f.peek()?;
        match op {
            0x10 => {
                f.pos += 1;
                let end = f.pkg_end()?;
                let path = parse_path(f)?;
                let node = match self
                    .resolve_exact(f.scope, &path)
                    .or_else(|| self.resolve(f.scope, &path))
                {
                    Some(node) => node,
                    None => self.define(f, &path, Kind::Scope)?,
                };
                self.exec_body(f, node, end)
            }
            0x08 => {
                f.pos += 1;
                let path = parse_path(f)?;
                let value = self.term_arg(f)?;
                let value = self.persist_if_loading(value)?;
                self.define(f, &path, Kind::Name(value))?;
                Ok(())
            }
            0x06 => {
                f.pos += 1;
                let source = parse_path(f)?;
                let alias = parse_path(f)?;
                let target = self
                    .resolve(f.scope, &source)
                    .ok_or(Flow::Fail(Error::NotFound))?;
                self.define(f, &alias, Kind::Alias(target))?;
                Ok(())
            }
            0x14 => {
                f.pos += 1;
                let end = f.pkg_end()?;
                let path = parse_path(f)?;
                let flags = f.next()?;
                self.define(
                    f,
                    &path,
                    Kind::Method {
                        offset: f.pos as u32,
                        end: end as u32,
                        flags,
                    },
                )?;
                f.pos = end;
                Ok(())
            }
            0x15 => {
                f.pos += 1;
                parse_path(f)?;
                f.next()?;
                f.next()?;
                Ok(())
            }
            0x8a..=0x8d | 0x8f => self.create_field(f, op),
            0xa0 => self.exec_if(f),
            0xa2 => self.exec_while(f),
            0xa3 | 0xcc => {
                f.pos += 1;
                Ok(())
            }
            0xa4 => {
                f.pos += 1;
                let value = self.term_arg(f)?;
                Err(Flow::Return(value))
            }
            0xa5 => {
                f.pos += 1;
                Err(Flow::Break)
            }
            0x9f => {
                f.pos += 1;
                Err(Flow::Continue)
            }
            0x5b => match f.peek_at(1)? {
                0x01 | 0x02 => {
                    let kind = if f.peek_at(1)? == 0x01 {
                        Kind::Mutex
                    } else {
                        Kind::Event
                    };
                    f.pos += 2;
                    let path = parse_path(f)?;
                    if matches!(kind, Kind::Mutex) {
                        f.next()?;
                    }
                    self.define(f, &path, kind)?;
                    Ok(())
                }
                0x13 => self.create_field(f, 0x13),
                0x80 => {
                    f.pos += 2;
                    let path = parse_path(f)?;
                    let space = f.next()?;
                    let base = self.term_arg(f)?;
                    let base = self.integer_of(base)?;
                    let length = self.term_arg(f)?;
                    let length = self.integer_of(length)?;
                    self.define(
                        f,
                        &path,
                        Kind::Region {
                            space,
                            base,
                            length,
                        },
                    )?;
                    Ok(())
                }
                0x81 | 0x86 | 0x87 => self.exec_field(f),
                0x82..=0x85 => {
                    let op = f.peek_at(1)?;
                    f.pos += 2;
                    let end = f.pkg_end()?;
                    let path = parse_path(f)?;
                    let kind = match op {
                        0x82 => Kind::Device,
                        0x83 => {
                            f.take(6)?;
                            Kind::Processor
                        }
                        0x84 => {
                            f.take(3)?;
                            Kind::PowerResource
                        }
                        _ => Kind::ThermalZone,
                    };
                    let node = self.define(f, &path, kind)?;
                    self.exec_body(f, node, end)
                }
                0x88 => {
                    f.pos += 2;
                    parse_path(f)?;
                    for _ in 0..3 {
                        self.term_arg(f)?;
                    }
                    Ok(())
                }
                _ => {
                    self.term_arg(f)?;
                    Ok(())
                }
            },
            _ => {
                self.term_arg(f)?;
                Ok(())
            }
        }
    }

    fn persist_if_loading(&mut self, value: Value) -> R<Value> {
        if self.depth == 0 {
            self.persist(value)
        } else {
            Ok(value)
        }
    }

    fn exec_if(&mut self, f: &mut Frame) -> R<()> {
        f.pos += 1;
        let end = f.pkg_end()?;
        let predicate = self.term_arg(f)?;
        let taken = self.integer_of(predicate)? != 0;
        if taken {
            self.exec_block(f, end)?;
        }
        f.pos = end;
        if f.pos < f.end && f.code[f.pos] == 0xa1 {
            f.pos += 1;
            let else_end = f.pkg_end()?;
            if !taken {
                self.exec_block(f, else_end)?;
            }
            f.pos = else_end;
        }
        Ok(())
    }

    fn exec_while(&mut self, f: &mut Frame) -> R<()> {
        f.pos += 1;
        let end = f.pkg_end()?;
        let start = f.pos;
        let mut loops = 0u32;
        loop {
            f.pos = start;
            let predicate = self.term_arg(f)?;
            if self.integer_of(predicate)? == 0 {
                break;
            }
            match self.exec_block(f, end) {
                Ok(()) | Err(Flow::Continue) => {}
                Err(Flow::Break) => break,
                Err(other) => return Err(other),
            }
            loops += 1;
            if loops > MAX_LOOPS {
                return fail(Error::Timeout);
            }
        }
        f.pos = end;
        Ok(())
    }

    fn create_field(&mut self, f: &mut Frame, op: u8) -> R<()> {
        f.pos += if op == 0x13 { 2 } else { 1 };
        let source = self.term_arg(f)?;
        let index = self.term_arg(f)?;
        let index = self.integer_of(index)?;
        let (bit_offset, bit_len) = match op {
            0x8a => (index * 8, 32),
            0x8b => (index * 8, 16),
            0x8c => (index * 8, 8),
            0x8d => (index, 1),
            0x8f => (index * 8, 64),
            _ => {
                let bits = self.term_arg(f)?;
                (index, self.integer_of(bits)?)
            }
        };
        let path = parse_path(f)?;
        let Value::Buffer(span) = self.deref(source)? else {
            return fail(Error::Type);
        };
        self.define(
            f,
            &path,
            Kind::BufferField {
                source: span,
                bit_offset: bit_offset as u32,
                bit_len: bit_len as u32,
            },
        )?;
        Ok(())
    }

    fn exec_field(&mut self, f: &mut Frame) -> R<()> {
        let op = f.peek_at(1)?;
        f.pos += 2;
        let end = f.pkg_end()?;
        let first_path = parse_path(f)?;
        let first = self
            .resolve(f.scope, &first_path)
            .ok_or(Flow::Fail(Error::NotFound))?;
        let (kind, second, bank_value) = match op {
            0x81 => (FieldKind::Normal, NO_NODE, 0),
            0x86 => {
                let data_path = parse_path(f)?;
                let data = self
                    .resolve(f.scope, &data_path)
                    .ok_or(Flow::Fail(Error::NotFound))?;
                (FieldKind::Index, data, 0)
            }
            _ => {
                let bank_path = parse_path(f)?;
                let bank = self
                    .resolve(f.scope, &bank_path)
                    .ok_or(Flow::Fail(Error::NotFound))?;
                let value = self.term_arg(f)?;
                (FieldKind::Bank, bank, self.integer_of(value)?)
            }
        };
        let flags = f.next()?;
        let mut access = flags & 0x0f;
        let update = (flags >> 5) & 3;
        let mut bit_offset = 0u32;
        while f.pos < end {
            let lead = f.peek()?;
            match lead {
                0x00 => {
                    f.pos += 1;
                    let (_, bits) = f.pkg_value()?;
                    bit_offset += bits as u32;
                }
                0x01 => {
                    f.pos += 1;
                    access = f.next()? & 0x0f;
                    f.next()?;
                }
                0x02 => {
                    f.pos += 1;
                    if f.peek()? == 0x11 {
                        f.pos += 1;
                        let skip = f.pkg_end()?;
                        f.pos = skip;
                    } else {
                        parse_path(f)?;
                    }
                }
                0x03 => {
                    f.pos += 1;
                    access = f.next()? & 0x0f;
                    f.next()?;
                    f.next()?;
                }
                _ => {
                    let name = read_segment(f)?;
                    let (_, bits) = f.pkg_value()?;
                    let unit = FieldUnit {
                        kind,
                        first,
                        second,
                        bank_value,
                        bit_offset,
                        bit_len: bits as u32,
                        access,
                        update,
                    };
                    self.add(f.scope, name, Kind::Field(unit))?;
                    bit_offset += bits as u32;
                }
            }
        }
        f.pos = end;
        Ok(())
    }

    // ----- expressions --------------------------------------------------

    fn operand(&mut self, f: &mut Frame) -> R<u64> {
        let value = self.term_arg(f)?;
        self.integer_of(value)
    }

    fn finish(&mut self, f: &mut Frame, result: Value) -> R<Value> {
        let target = self.parse_target(f)?;
        self.store(f, target, result)?;
        Ok(result)
    }

    fn binary(&mut self, f: &mut Frame, op: u8) -> R<Value> {
        let left = self.operand(f)?;
        let right = self.operand(f)?;
        let width = if self.int_mask == u64::MAX { 64 } else { 32 };
        let number = match op {
            0x72 => left.wrapping_add(right),
            0x74 => left.wrapping_sub(right),
            0x77 => left.wrapping_mul(right),
            0x79 => {
                if right >= width {
                    0
                } else {
                    left << right
                }
            }
            0x7a => {
                if right >= width {
                    0
                } else {
                    left >> right
                }
            }
            0x7b => left & right,
            0x7c => !(left & right),
            0x7d => left | right,
            0x7e => !(left | right),
            0x7f => left ^ right,
            _ => {
                if right == 0 {
                    return fail(Error::Parse);
                }
                left % right
            }
        };
        let result = Value::Integer(self.mask(number));
        self.finish(f, result)
    }

    fn compare_values(&mut self, left: Value, right: Value) -> R<core::cmp::Ordering> {
        let left = self.deref(left)?;
        let right = self.deref(right)?;
        match (left, right) {
            (Value::Str(a), _) | (Value::Buffer(a), _) if !matches!(right, Value::Integer(_)) => {
                let b = match right {
                    Value::Str(b) | Value::Buffer(b) => b,
                    _ => return fail(Error::Type),
                };
                Ok(self.bytes(a).cmp(self.bytes(b)))
            }
            _ => {
                let a = self.integer_of(left)?;
                let b = self.integer_of(right)?;
                Ok(a.cmp(&b))
            }
        }
    }

    fn make_reference_to(&mut self, container: Value, index: u64) -> R<Value> {
        match self.deref(container)? {
            Value::Package(span) => {
                if index >= span.len as u64 {
                    return fail(Error::Parse);
                }
                Ok(Value::Ref(Reference {
                    base: span,
                    kind: RefKind::PackageElement,
                    at: index as u32,
                }))
            }
            Value::Buffer(span) | Value::Str(span) => {
                if index >= span.len as u64 {
                    return fail(Error::Parse);
                }
                Ok(Value::Ref(Reference {
                    base: span,
                    kind: RefKind::Byte,
                    at: index as u32,
                }))
            }
            _ => fail(Error::Type),
        }
    }

    fn size_of(&mut self, value: Value) -> R<u64> {
        match self.deref(value)? {
            Value::Str(span) | Value::Buffer(span) | Value::Package(span) => Ok(span.len as u64),
            Value::Integer(_) => Ok(if self.int_mask == u64::MAX { 8 } else { 4 }),
            _ => fail(Error::Type),
        }
    }

    fn object_type(&self, value: Value) -> u64 {
        match value {
            Value::Uninit => 0,
            Value::Integer(_) => 1,
            Value::Str(_) => 2,
            Value::Buffer(_) => 3,
            Value::Package(_) => 4,
            Value::Ref(_) | Value::Unresolved { .. } => 5,
            Value::Node(node) => match self.nodes[node as usize].kind {
                Kind::Device => 6,
                Kind::Event => 7,
                Kind::Method { .. } | Kind::OsInterface => 8,
                Kind::Mutex => 9,
                Kind::Region { .. } => 10,
                Kind::PowerResource => 11,
                Kind::Processor => 12,
                Kind::ThermalZone => 13,
                Kind::BufferField { .. } => 14,
                _ => 0,
            },
        }
    }

    fn convert_to_string(&mut self, value: Value, hex: bool, limit: usize) -> R<Value> {
        let persistent = self.persist_mode && self.depth == 0;
        match self.deref(value)? {
            Value::Integer(number) => {
                let mut digits = [0u8; 20];
                let text = format_integer(number, hex, &mut digits);
                self.make_string(text, persistent)
            }
            Value::Buffer(span) if hex => {
                let mut out = [0u8; 256];
                let mut length = 0;
                for (index, byte) in self.bytes(span).iter().take(80).enumerate() {
                    if index != 0 {
                        out[length] = b',';
                        length += 1;
                    }
                    for nibble in [byte >> 4, byte & 15] {
                        out[length] = b"0123456789ABCDEF"[nibble as usize];
                        length += 1;
                    }
                }
                self.make_string(&out[..length], persistent)
            }
            Value::Buffer(span) => {
                let mut out = [0u8; 512];
                let source = self.bytes(span);
                let mut length = 0;
                for byte in source.iter().take(limit.min(out.len())) {
                    if *byte == 0 {
                        break;
                    }
                    out[length] = *byte;
                    length += 1;
                }
                self.make_string(&out[..length], persistent)
            }
            Value::Str(span) => Ok(Value::Str(span)),
            _ => fail(Error::Type),
        }
    }

    fn term_arg(&mut self, f: &mut Frame) -> R<Value> {
        let op = f.peek()?;
        if is_name_start(op) {
            return self.name_term(f);
        }
        f.pos += 1;
        let persistent = self.persist_mode && self.depth == 0;
        match op {
            0x00 => Ok(Value::Integer(0)),
            0x01 => Ok(Value::Integer(1)),
            0xff => Ok(Value::Integer(self.int_mask)),
            0x0a => Ok(Value::Integer(f.take(1)?)),
            0x0b => Ok(Value::Integer(f.take(2)?)),
            0x0c => Ok(Value::Integer(f.take(4)?)),
            0x0e => Ok(Value::Integer(self.mask(f.take(8)?))),
            0x0d => {
                let start = f.pos;
                while f.next()? != 0 {}
                let text = &f.code[start..f.pos - 1];
                self.make_string(text, persistent)
            }
            0x11 => {
                let end = f.pkg_end()?;
                let size = self.operand(f)? as usize;
                let code = f.code;
                let initial = &code[f.pos.min(end)..end];
                let span = self.alloc_bytes(persistent, size.max(initial.len()))?;
                self.bytes_mut(span)[..initial.len()].copy_from_slice(initial);
                f.pos = end;
                Ok(Value::Buffer(span))
            }
            0x12 | 0x13 => {
                let end = f.pkg_end()?;
                let count = if op == 0x12 {
                    f.next()? as usize
                } else {
                    self.operand(f)? as usize
                };
                let span = self.alloc_values(persistent, count)?;
                let mut index = 0;
                while f.pos < end {
                    let value = self.parse_package_element(f)?;
                    if index < count {
                        self.values_mut(span)[index] = value;
                    }
                    index += 1;
                }
                f.pos = end;
                Ok(Value::Package(span))
            }
            0x60..=0x67 => Ok(f.locals[(op - 0x60) as usize]),
            0x68..=0x6e => Ok(f.args[(op - 0x68) as usize]),
            0x70 => {
                let value = self.term_arg(f)?;
                let value = self.deref(value)?;
                let target = self.parse_target(f)?;
                self.store(f, target, value)?;
                Ok(value)
            }
            0x71 => {
                let target = self.parse_target(f)?;
                match target {
                    Target::Node(node) => Ok(Value::Node(node)),
                    other => self.read_target(f, &other),
                }
            }
            0x72 | 0x74 | 0x77 | 0x79 | 0x7a | 0x7b | 0x7c | 0x7d | 0x7e | 0x7f | 0x85 => {
                self.binary(f, op)
            }
            0x73 => {
                let left = self.term_arg(f)?;
                let right = self.term_arg(f)?;
                let result = self.concatenate(left, right)?;
                self.finish(f, result)
            }
            0x75 | 0x76 => {
                let target = self.parse_target(f)?;
                let current = self.read_target(f, &target)?;
                let number = self.integer_of(current)?;
                let number = if op == 0x75 {
                    number.wrapping_add(1)
                } else {
                    number.wrapping_sub(1)
                };
                let result = Value::Integer(self.mask(number));
                self.store(f, target, result)?;
                Ok(result)
            }
            0x78 => {
                let dividend = self.operand(f)?;
                let divisor = self.operand(f)?;
                if divisor == 0 {
                    return fail(Error::Parse);
                }
                let remainder = Value::Integer(dividend % divisor);
                let quotient = Value::Integer(dividend / divisor);
                let target = self.parse_target(f)?;
                self.store(f, target, remainder)?;
                let target = self.parse_target(f)?;
                self.store(f, target, quotient)?;
                Ok(quotient)
            }
            0x80 => {
                let number = self.operand(f)?;
                let result = Value::Integer(self.mask(!number));
                self.finish(f, result)
            }
            0x81 | 0x82 => {
                let number = self.operand(f)?;
                let bits = if self.int_mask == u64::MAX { 64 } else { 32 };
                let position = if number == 0 {
                    0
                } else if op == 0x81 {
                    bits - number.leading_zeros() as u64 + (64 - bits)
                } else {
                    number.trailing_zeros() as u64 + 1
                };
                let result = Value::Integer(position);
                self.finish(f, result)
            }
            0x83 => {
                let value = self.term_arg(f)?;
                match value {
                    Value::Ref(reference) => self.read_reference(reference),
                    Value::Node(node) => self.read_node(node),
                    Value::Str(_) | Value::Buffer(_) | Value::Package(_) | Value::Integer(_) => {
                        Ok(value)
                    }
                    Value::Uninit | Value::Unresolved { .. } => fail(Error::Type),
                }
            }
            0x84 => {
                let left = self.term_arg(f)?;
                let right = self.term_arg(f)?;
                let result = self.concatenate_resources(left, right)?;
                self.finish(f, result)
            }
            0x86 => {
                self.parse_target(f)?;
                self.operand(f)?;
                self.notifications += 1;
                Ok(Value::Integer(0))
            }
            0x87 => {
                let value = self.term_arg(f)?;
                let size = self.size_of(value)?;
                Ok(Value::Integer(size))
            }
            0x88 => {
                let container = self.term_arg(f)?;
                let index = self.operand(f)?;
                let reference = self.make_reference_to(container, index)?;
                let target = self.parse_target(f)?;
                self.store(f, target, reference)?;
                Ok(reference)
            }
            0x89 => self.match_op(f),
            0x8e => {
                let target = self.parse_target_lenient(f)?;
                let kind = match target {
                    Some(Target::Node(node)) => match self.nodes[node as usize].kind {
                        Kind::Name(value) => self.object_type(value),
                        Kind::Field(_) => 5,
                        Kind::BufferField { .. } => 14,
                        _ => self.object_type(Value::Node(node)),
                    },
                    Some(other) => {
                        let value = self.read_target(f, &other)?;
                        self.object_type(value)
                    }
                    None => 0,
                };
                Ok(Value::Integer(kind))
            }
            0x90 | 0x91 => {
                let left = self.operand(f)? != 0;
                let right = self.operand(f)? != 0;
                Ok(self.truth(if op == 0x90 {
                    left && right
                } else {
                    left || right
                }))
            }
            0x92 => {
                let next = f.peek()?;
                match next {
                    0x93..=0x95 => {
                        f.pos += 1;
                        let left = self.term_arg(f)?;
                        let right = self.term_arg(f)?;
                        let order = self.compare_values(left, right)?;
                        let holds = match next {
                            0x93 => order != core::cmp::Ordering::Equal,
                            0x94 => order != core::cmp::Ordering::Greater,
                            _ => order != core::cmp::Ordering::Less,
                        };
                        Ok(self.truth(holds))
                    }
                    _ => {
                        let value = self.operand(f)?;
                        Ok(self.truth(value == 0))
                    }
                }
            }
            0x93..=0x95 => {
                let left = self.term_arg(f)?;
                let right = self.term_arg(f)?;
                let order = self.compare_values(left, right)?;
                let holds = match op {
                    0x93 => order == core::cmp::Ordering::Equal,
                    0x94 => order == core::cmp::Ordering::Greater,
                    _ => order == core::cmp::Ordering::Less,
                };
                Ok(self.truth(holds))
            }
            0x96 => {
                let value = self.term_arg(f)?;
                let result = self.buffer_of(value)?;
                self.finish(f, result)
            }
            0x97 | 0x98 => {
                let value = self.term_arg(f)?;
                let result = self.convert_to_string(value, op == 0x98, usize::MAX)?;
                self.finish(f, result)
            }
            0x99 => {
                let value = self.term_arg(f)?;
                let number = self.integer_of(value)?;
                self.finish(f, Value::Integer(number))
            }
            0x9c => {
                let value = self.term_arg(f)?;
                let length = self.operand(f)?;
                let limit = if length >= 0xffff_ffff {
                    usize::MAX
                } else {
                    length as usize
                };
                let result = self.convert_to_string(value, false, limit)?;
                self.finish(f, result)
            }
            0x9d => {
                let value = self.term_arg(f)?;
                let value = self.deref(value)?;
                let target = self.parse_target(f)?;
                self.store(f, target, value)?;
                Ok(value)
            }
            0x9e => {
                let source = self.term_arg(f)?;
                let index = self.operand(f)? as usize;
                let length = self.operand(f)? as usize;
                let result = self.mid(source, index, length)?;
                self.finish(f, result)
            }
            0x5b => self.extended(f),
            _ => fail(Error::Unsupported),
        }
    }

    fn parse_package_element(&mut self, f: &mut Frame) -> R<Value> {
        let lead = f.peek()?;
        if is_name_start(lead) {
            let path = parse_path(f)?;
            if let Some(node) = self.resolve(f.scope, &path) {
                return Ok(match self.nodes[node as usize].kind {
                    Kind::Name(value) => value,
                    _ => Value::Node(node),
                });
            }
            let persistent = self.persist_mode && self.depth == 0;
            let span = self.alloc_bytes(persistent, 3 + 4 * path.count as usize)?;
            let encoded = self.bytes_mut(span);
            encoded[0] = path.root as u8;
            encoded[1] = path.up;
            encoded[2] = path.count;
            for index in 0..path.count as usize {
                encoded[3 + 4 * index..7 + 4 * index].copy_from_slice(&path.segments[index]);
            }
            return Ok(Value::Unresolved {
                path: span,
                scope: f.scope,
            });
        }
        self.term_arg(f)
    }

    fn buffer_of(&mut self, value: Value) -> R<Value> {
        let persistent = self.persist_mode && self.depth == 0;
        match self.deref(value)? {
            Value::Buffer(span) => Ok(Value::Buffer(span)),
            Value::Str(span) => {
                let length = span.len as usize;
                let target = self.alloc_bytes(persistent, length + 1)?;
                for index in 0..length {
                    let byte = self.bytes(span)[index];
                    self.bytes_mut(target)[index] = byte;
                }
                Ok(Value::Buffer(target))
            }
            Value::Integer(number) => {
                let mut bytes = [0u8; 8];
                let width = self.bytes_of(Value::Integer(number), &mut bytes)?;
                self.make_buffer(persistent, &bytes[..width])
            }
            _ => fail(Error::Type),
        }
    }

    fn concatenate(&mut self, left: Value, right: Value) -> R<Value> {
        let persistent = self.persist_mode && self.depth == 0;
        let left = self.deref(left)?;
        let right = self.deref(right)?;
        match left {
            Value::Str(a) => {
                let second = self.convert_to_string(right, true, usize::MAX)?;
                let Value::Str(b) = second else {
                    return fail(Error::Type);
                };
                let target = self.alloc_bytes(persistent, (a.len + b.len) as usize)?;
                for index in 0..a.len as usize {
                    let byte = self.bytes(a)[index];
                    self.bytes_mut(target)[index] = byte;
                }
                for index in 0..b.len as usize {
                    let byte = self.bytes(b)[index];
                    self.bytes_mut(target)[a.len as usize + index] = byte;
                }
                Ok(Value::Str(target))
            }
            Value::Buffer(a) => {
                let second = self.buffer_of(right)?;
                let Value::Buffer(b) = second else {
                    return fail(Error::Type);
                };
                let target = self.alloc_bytes(persistent, (a.len + b.len) as usize)?;
                for index in 0..a.len as usize {
                    let byte = self.bytes(a)[index];
                    self.bytes_mut(target)[index] = byte;
                }
                for index in 0..b.len as usize {
                    let byte = self.bytes(b)[index];
                    self.bytes_mut(target)[a.len as usize + index] = byte;
                }
                Ok(Value::Buffer(target))
            }
            Value::Integer(_) => {
                let mut first = [0u8; 8];
                let mut second = [0u8; 8];
                let a = self.bytes_of(left, &mut first)?;
                let second_value = Value::Integer(self.integer_of(right)?);
                let b = self.bytes_of(second_value, &mut second)?;
                let mut joined = [0u8; 16];
                joined[..a].copy_from_slice(&first[..a]);
                joined[a..a + b].copy_from_slice(&second[..b]);
                self.make_buffer(persistent, &joined[..a + b])
            }
            _ => fail(Error::Type),
        }
    }

    fn concatenate_resources(&mut self, left: Value, right: Value) -> R<Value> {
        let persistent = self.persist_mode && self.depth == 0;
        let (Value::Buffer(a), Value::Buffer(b)) = (self.deref(left)?, self.deref(right)?) else {
            return fail(Error::Type);
        };
        let mut keep = a.len as usize;
        if keep >= 2 && self.bytes(a)[keep - 2] == 0x79 {
            keep -= 2;
        }
        let target = self.alloc_bytes(persistent, keep + b.len as usize)?;
        for index in 0..keep {
            let byte = self.bytes(a)[index];
            self.bytes_mut(target)[index] = byte;
        }
        for index in 0..b.len as usize {
            let byte = self.bytes(b)[index];
            self.bytes_mut(target)[keep + index] = byte;
        }
        Ok(Value::Buffer(target))
    }

    fn mid(&mut self, source: Value, index: usize, length: usize) -> R<Value> {
        let persistent = self.persist_mode && self.depth == 0;
        match self.deref(source)? {
            Value::Str(span) | Value::Buffer(span) => {
                let is_string = matches!(self.deref(source)?, Value::Str(_));
                let total = span.len as usize;
                let start = index.min(total);
                let take = length.min(total - start);
                let target = self.alloc_bytes(persistent, take)?;
                for offset in 0..take {
                    let byte = self.bytes(span)[start + offset];
                    self.bytes_mut(target)[offset] = byte;
                }
                Ok(if is_string {
                    Value::Str(target)
                } else {
                    Value::Buffer(target)
                })
            }
            _ => fail(Error::Type),
        }
    }

    fn match_op(&mut self, f: &mut Frame) -> R<Value> {
        let package = self.term_arg(f)?;
        let first_op = f.next()?;
        let first_value = self.term_arg(f)?;
        let second_op = f.next()?;
        let second_value = self.term_arg(f)?;
        let start = self.operand(f)? as usize;
        let Value::Package(span) = self.deref(package)? else {
            return fail(Error::Type);
        };
        let test = |this: &mut Self, op: u8, element: Value, wanted: Value| -> R<bool> {
            if op == 0 {
                return Ok(true);
            }
            let order = this.compare_values(element, wanted)?;
            Ok(match op {
                1 => order == core::cmp::Ordering::Equal,
                2 => order != core::cmp::Ordering::Greater,
                3 => order == core::cmp::Ordering::Less,
                4 => order != core::cmp::Ordering::Less,
                5 => order == core::cmp::Ordering::Greater,
                _ => order != core::cmp::Ordering::Equal,
            })
        };
        for index in start..span.len as usize {
            let element = self.values(span)[index];
            if matches!(element, Value::Uninit) {
                continue;
            }
            if test(self, first_op, element, first_value)?
                && test(self, second_op, element, second_value)?
            {
                return Ok(Value::Integer(index as u64));
            }
        }
        Ok(Value::Integer(self.int_mask))
    }

    fn extended(&mut self, f: &mut Frame) -> R<Value> {
        let op = f.next()?;
        match op {
            0x12 => {
                let source = self.parse_target_lenient(f)?;
                let destination = self.parse_target(f)?;
                match source {
                    Some(Target::Node(node)) => {
                        self.store(f, destination, Value::Node(node))?;
                        Ok(self.truth(true))
                    }
                    Some(other) => {
                        let value = self.read_target(f, &other)?;
                        self.store(f, destination, value)?;
                        Ok(self.truth(true))
                    }
                    None => Ok(self.truth(false)),
                }
            }
            0x21 | 0x22 => {
                let amount = self.operand(f)?;
                let nanoseconds = if op == 0x21 {
                    amount.saturating_mul(1000)
                } else {
                    amount.saturating_mul(1_000_000)
                };
                let start = (self.hooks.now_ns)();
                while (self.hooks.now_ns)().wrapping_sub(start) < nanoseconds {
                    core::hint::spin_loop();
                }
                Ok(Value::Integer(0))
            }
            0x23 => {
                self.parse_target(f)?;
                f.take(2)?;
                Ok(Value::Integer(0))
            }
            0x27 | 0x24 | 0x26 => {
                self.parse_target(f)?;
                Ok(Value::Integer(0))
            }
            0x25 => {
                self.parse_target(f)?;
                self.operand(f)?;
                Ok(Value::Integer(0))
            }
            0x28 => {
                let number = self.operand(f)?;
                let mut value = 0u64;
                let mut scale = 1u64;
                let mut rest = number;
                while rest != 0 {
                    value += (rest & 15) * scale;
                    scale = scale.wrapping_mul(10);
                    rest >>= 4;
                }
                self.finish(f, Value::Integer(value))
            }
            0x29 => {
                let mut rest = self.operand(f)?;
                let mut value = 0u64;
                let mut shift = 0;
                while rest != 0 && shift < 64 {
                    value |= (rest % 10) << shift;
                    rest /= 10;
                    shift += 4;
                }
                self.finish(f, Value::Integer(self.mask(value)))
            }
            0x30 => Ok(Value::Integer(2)),
            0x31 => Ok(Value::Integer(0)),
            0x32 => {
                f.take(5)?;
                self.operand(f)?;
                fail(Error::Fatal)
            }
            0x33 => Ok(Value::Integer((self.hooks.now_ns)() / 100)),
            _ => fail(Error::Unsupported),
        }
    }

    fn parse_target_lenient(&mut self, f: &mut Frame) -> R<Option<Target>> {
        let op = f.peek()?;
        if is_name_start(op) {
            let path = parse_path(f)?;
            return Ok(self.resolve(f.scope, &path).map(Target::Node));
        }
        Ok(Some(self.parse_target(f)?))
    }

    fn name_term(&mut self, f: &mut Frame) -> R<Value> {
        let path = parse_path(f)?;
        let node = self
            .resolve(f.scope, &path)
            .ok_or(Flow::Fail(Error::NotFound))?;
        match self.nodes[node as usize].kind {
            Kind::Method { flags, .. } => {
                let count = (flags & 7) as usize;
                let mut args = [Value::Uninit; 7];
                for slot in args.iter_mut().take(count) {
                    *slot = self.term_arg(f)?;
                }
                self.call_method(node, &args[..count])
            }
            Kind::OsInterface => {
                let argument = self.term_arg(f)?;
                self.os_interface(argument)
            }
            _ => self.read_node(node),
        }
    }

    fn os_interface(&mut self, argument: Value) -> R<Value> {
        let Value::Str(span) = self.deref(argument)? else {
            return Ok(Value::Integer(0));
        };
        let name = self.bytes(span);
        const SUPPORTED: [&[u8]; 14] = [
            b"Windows 2000",
            b"Windows 2001",
            b"Windows 2001 SP1",
            b"Windows 2001 SP2",
            b"Windows 2001.1",
            b"Windows 2006",
            b"Windows 2009",
            b"Windows 2012",
            b"Windows 2013",
            b"Windows 2015",
            b"Module Device",
            b"Processor Device",
            b"3.0 Thermal Model",
            b"Extended Address Space Descriptor",
        ];
        let known = SUPPORTED.contains(&name);
        Ok(self.truth(known))
    }

    fn call_method(&mut self, node: u16, args: &[Value]) -> R<Value> {
        if self.depth >= MAX_DEPTH {
            return fail(Error::Depth);
        }
        let Kind::Method { offset, end, .. } = self.nodes[node as usize].kind else {
            return fail(Error::Type);
        };
        let table = self.nodes[node as usize].table;
        let code = self.code(table);
        let mut frame = Frame::new(code, offset as usize, end as usize, node);
        frame.args[..args.len()].copy_from_slice(args);
        let saved_persist = self.persist_mode;
        let saved_mark = self.method_mark;
        let mark = self.node_count;
        self.persist_mode = false;
        self.method_mark = mark;
        self.depth += 1;
        let result = self.exec_block(&mut frame, end as usize);
        self.depth -= 1;
        self.persist_mode = saved_persist;
        self.method_mark = saved_mark;
        self.truncate_nodes(mark);
        match result {
            Ok(()) => Ok(Value::Integer(0)),
            Err(Flow::Return(value)) => Ok(value),
            Err(Flow::Break | Flow::Continue) => fail(Error::Parse),
            Err(failure) => Err(failure),
        }
    }

    // ----- public evaluation --------------------------------------------

    /// Evaluates the object at `path`, calling it with `args` if it is a
    /// method. The result's storage is valid until the next evaluation.
    pub fn evaluate(&mut self, path: &str, args: &[u64]) -> Result<Value, Error> {
        let node = self.find(path).ok_or(Error::NotFound)?;
        self.evaluate_node(node, args)
    }

    pub fn evaluate_node(&mut self, node: u16, args: &[u64]) -> Result<Value, Error> {
        self.temporary_bytes_used = 0;
        self.temporary_values_used = 0;
        let mut values = [Value::Uninit; 7];
        for (slot, argument) in values.iter_mut().zip(args) {
            *slot = Value::Integer(*argument);
        }
        let saved = self.persist_mode;
        self.persist_mode = false;
        let result = match self.nodes[node as usize].kind {
            Kind::Method { flags, .. } => self.call_method(node, &values[..(flags & 7) as usize]),
            _ => self.read_node(node),
        };
        self.persist_mode = saved;
        match result {
            Ok(value) => Ok(value),
            Err(Flow::Fail(error)) => Err(error),
            Err(_) => Err(Error::Parse),
        }
    }

    pub fn integer(&self, value: Value) -> Option<u64> {
        match value {
            Value::Integer(number) => Some(number),
            _ => None,
        }
    }

    pub fn buffer(&self, value: Value) -> Option<&[u8]> {
        match value {
            Value::Buffer(span) | Value::Str(span) => Some(self.bytes(span)),
            _ => None,
        }
    }

    pub fn package_len(&self, value: Value) -> Option<usize> {
        match value {
            Value::Package(span) => Some(span.len as usize),
            _ => None,
        }
    }

    pub fn package_element(&self, value: Value, index: usize) -> Option<Value> {
        match value {
            Value::Package(span) => self
                .values(span)
                .get(index)
                .copied()
                .map(|element| self.settle(element)),
            _ => None,
        }
    }

    pub fn value_node(&self, value: Value) -> Option<u16> {
        match value {
            Value::Node(node) => Some(node),
            _ => None,
        }
    }

    /// The `_HID`/`_CID` style identifier of a device as text (`PNP0A08`) or
    /// the string it holds.
    pub fn device_id(&mut self, node: u16, name: &[u8; 4], out: &mut [u8; 8]) -> Option<usize> {
        let child = self.child(node, *name)?;
        let value = self.read_node(child).ok()?;
        match value {
            Value::Integer(number) => {
                let bytes = (number as u32).to_le_bytes();
                let vendor = ((bytes[0] as u16) << 8) | bytes[1] as u16;
                out[0] = b'@' + ((vendor >> 10) & 0x1f) as u8;
                out[1] = b'@' + ((vendor >> 5) & 0x1f) as u8;
                out[2] = b'@' + (vendor & 0x1f) as u8;
                let hex = b"0123456789ABCDEF";
                out[3] = hex[(bytes[2] >> 4) as usize];
                out[4] = hex[(bytes[2] & 15) as usize];
                out[5] = hex[(bytes[3] >> 4) as usize];
                out[6] = hex[(bytes[3] & 15) as usize];
                Some(7)
            }
            Value::Str(span) => {
                let bytes = self.bytes(span);
                let length = bytes.len().min(8);
                out[..length].copy_from_slice(&bytes[..length]);
                Some(length)
            }
            _ => None,
        }
    }
}

fn parse_integer(text: &[u8]) -> u64 {
    let text = match text.iter().position(|byte| *byte == 0) {
        Some(end) => &text[..end],
        None => text,
    };
    let (digits, radix) = if text.len() > 2 && text[0] == b'0' && (text[1] | 0x20) == b'x' {
        (&text[2..], 16)
    } else {
        (text, 10)
    };
    let mut number = 0u64;
    for byte in digits {
        let digit = match byte {
            b'0'..=b'9' => (byte - b'0') as u64,
            b'a'..=b'f' if radix == 16 => (byte - b'a' + 10) as u64,
            b'A'..=b'F' if radix == 16 => (byte - b'A' + 10) as u64,
            _ => break,
        };
        number = number.wrapping_mul(radix).wrapping_add(digit);
    }
    number
}

fn format_integer(number: u64, hex: bool, out: &mut [u8; 20]) -> &[u8] {
    if hex {
        let mut length = 0;
        let mut started = false;
        out[0] = b'0';
        out[1] = b'x';
        length += 2;
        for shift in (0..16).rev() {
            let nibble = ((number >> (shift * 4)) & 15) as usize;
            if nibble != 0 || started || shift == 0 {
                started = true;
                out[length] = b"0123456789ABCDEF"[nibble];
                length += 1;
            }
        }
        return &out[..length];
    }
    let mut digits = [0u8; 20];
    let mut count = 0;
    let mut rest = number;
    loop {
        digits[count] = b'0' + (rest % 10) as u8;
        count += 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    for index in 0..count {
        out[index] = digits[count - 1 - index];
    }
    &out[..count]
}

fn low_bits(count: u32) -> u64 {
    if count >= 64 {
        u64::MAX
    } else {
        (1u64 << count) - 1
    }
}

fn shift_right(value: u64, shift: u32) -> u64 {
    if shift >= 64 { 0 } else { value >> shift }
}

fn get_bits(bytes: &[u8], at: u32, count: u32) -> u64 {
    let mut value = 0u64;
    for index in 0..count.min(64) {
        let bit = at + index;
        let byte = bytes.get((bit / 8) as usize).copied().unwrap_or(0);
        if byte & (1 << (bit % 8)) != 0 {
            value |= 1 << index;
        }
    }
    value
}

fn put_bits(bytes: &mut [u8], at: u32, value: u64, count: u32) {
    for index in 0..count.min(64) {
        let bit = at + index;
        if let Some(byte) = bytes.get_mut((bit / 8) as usize) {
            if value & (1 << index) != 0 {
                *byte |= 1 << (bit % 8);
            } else {
                *byte &= !(1 << (bit % 8));
            }
        }
    }
}

/// One entry of a resource template (`_CRS`, `_PRS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Irq {
        mask: u16,
    },
    Io {
        minimum: u32,
        maximum: u32,
        length: u32,
    },
    FixedIo {
        base: u16,
        length: u8,
    },
    Memory {
        minimum: u64,
        length: u64,
    },
    ExtendedIrq {
        first: u32,
        count: u8,
    },
}

/// Walks the descriptors of a resource template.
pub fn resources(template: &[u8], mut visit: impl FnMut(Resource)) {
    let mut at = 0;
    while at < template.len() {
        let tag = template[at];
        if tag & 0x80 == 0 {
            let length = (tag & 7) as usize;
            let body = template.get(at + 1..at + 1 + length).unwrap_or(&[]);
            match tag >> 3 {
                0x04 if body.len() >= 2 => visit(Resource::Irq {
                    mask: u16::from_le_bytes([body[0], body[1]]),
                }),
                0x08 if body.len() >= 7 => visit(Resource::Io {
                    minimum: u16::from_le_bytes([body[1], body[2]]) as u32,
                    maximum: u16::from_le_bytes([body[3], body[4]]) as u32,
                    length: body[6] as u32,
                }),
                0x09 if body.len() >= 3 => visit(Resource::FixedIo {
                    base: u16::from_le_bytes([body[0], body[1]]),
                    length: body[2],
                }),
                0x0f => return,
                _ => {}
            }
            at += 1 + length;
        } else {
            if at + 3 > template.len() {
                return;
            }
            let length = u16::from_le_bytes([template[at + 1], template[at + 2]]) as usize;
            let body = template.get(at + 3..at + 3 + length).unwrap_or(&[]);
            let number = |from: usize, width: usize| -> u64 {
                let mut value = 0u64;
                for (index, byte) in body.iter().skip(from).take(width).enumerate() {
                    value |= (*byte as u64) << (8 * index);
                }
                value
            };
            let address_space = |minimum: usize, size_at: usize, width: usize, needed: usize| {
                (body.len() >= needed).then(|| match body[0] {
                    0 => Some(Resource::Memory {
                        minimum: number(minimum, width),
                        length: number(size_at, width),
                    }),
                    1 => Some(Resource::Io {
                        minimum: number(minimum, width) as u32,
                        maximum: number(minimum + width, width) as u32,
                        length: number(size_at, width) as u32,
                    }),
                    _ => None,
                })
            };
            let found = match tag & 0x7f {
                0x06 if body.len() >= 9 => Some(Resource::Memory {
                    minimum: number(1, 4),
                    length: number(5, 4),
                }),
                0x07 => address_space(7, 19, 4, 23).flatten(),
                0x08 => address_space(5, 11, 2, 13).flatten(),
                0x0a => address_space(11, 35, 8, 43).flatten(),
                0x09 if body.len() >= 6 => Some(Resource::ExtendedIrq {
                    first: number(2, 4) as u32,
                    count: body[1],
                }),
                _ => None,
            };
            if let Some(resource) = found {
                visit(resource);
            }
            at += 3 + length;
        }
    }
}
