//! The AerOS clipboard: the current text plus a short history (Super+V).
//! Text copied inside a Linux guest arrives here through the agent's
//! mailbox, and anything copied on the AerOS side is pushed into the guest
//! (see the desktop loop), so both sides paste each other's copies.
//!
//! The storage is one static (the desktop loop is the only user), not a field
//! of the desktop state, which is copied around freely.

use core::cell::UnsafeCell;

/// Longest text kept per entry (longer copies are cut at a character edge).
pub const CLIP_ENTRY: usize = 16 * 1024;
pub const CLIP_HISTORY: usize = 8;

pub struct Clipboard {
    /// Entry `i` lives at `i * CLIP_ENTRY`; index 0 is the newest.
    data: [u8; CLIP_ENTRY * CLIP_HISTORY],
    lens: [usize; CLIP_HISTORY],
    count: usize,
    /// Bumped on every change of the current entry; the desktop compares it
    /// with the value last pushed to the guest.
    pub generation: u32,
}

struct Global(UnsafeCell<Clipboard>);

// SAFETY: only the desktop loop (one CPU) touches the clipboard.
unsafe impl Sync for Global {}

static CLIPBOARD: Global = Global(UnsafeCell::new(Clipboard::new()));
static SCRATCH: ScratchCell = ScratchCell(UnsafeCell::new([0; CLIP_ENTRY]));

struct ScratchCell(UnsafeCell<[u8; CLIP_ENTRY]>);

// SAFETY: as above.
unsafe impl Sync for ScratchCell {}

/// The system clipboard.
pub fn get() -> &'static mut Clipboard {
    // SAFETY: single user (the desktop loop); never held across calls that
    // could re-enter it.
    unsafe { &mut *CLIPBOARD.0.get() }
}

/// A scratch buffer the size of one entry, for copying text in and out.
pub fn scratch() -> &'static mut [u8; CLIP_ENTRY] {
    // SAFETY: as for `get`.
    unsafe { &mut *SCRATCH.0.get() }
}

impl Clipboard {
    pub const fn new() -> Self {
        Self {
            data: [0; CLIP_ENTRY * CLIP_HISTORY],
            lens: [0; CLIP_HISTORY],
            count: 0,
            generation: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn entry(&self, index: usize) -> &[u8] {
        if index < self.count {
            let start = index * CLIP_ENTRY;
            &self.data[start..start + self.lens[index]]
        } else {
            &[]
        }
    }

    /// The most recent copy, if any.
    pub fn current(&self) -> Option<&[u8]> {
        (self.count > 0).then(|| self.entry(0))
    }

    /// Records a new copy as the current entry. An identical earlier entry is
    /// moved to the front instead of duplicated. Returns whether the current
    /// entry changed.
    pub fn push(&mut self, bytes: &[u8]) -> bool {
        let bytes = trim_to_char_edge(bytes, CLIP_ENTRY);
        if bytes.is_empty() {
            return false;
        }
        if self.current() == Some(bytes) {
            return false;
        }
        let existing = (0..self.count).find(|&index| self.entry(index) == bytes);
        let last = match existing {
            Some(index) => index,
            None => self.count.min(CLIP_HISTORY - 1),
        };
        // Shift entries 0..last up by one (dropping `last`'s old text), then
        // put the new text first.
        self.data.copy_within(0..last * CLIP_ENTRY, CLIP_ENTRY);
        self.lens.copy_within(0..last, 1);
        self.data[..bytes.len()].copy_from_slice(bytes);
        self.lens[0] = bytes.len();
        if existing.is_none() && self.count < CLIP_HISTORY {
            self.count += 1;
        }
        self.generation = self.generation.wrapping_add(1);
        true
    }

    /// Makes history entry `index` the current one.
    pub fn select(&mut self, index: usize) {
        if index == 0 || index >= self.count {
            return;
        }
        // Rotate entries 0..=index so `index` comes first.
        self.data[..(index + 1) * CLIP_ENTRY].rotate_right(CLIP_ENTRY);
        // rotate_right by one entry moved entry `index` to the front only if
        // it was the last of the range, which it is.
        self.lens[..=index].rotate_right(1);
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn clear(&mut self) {
        self.count = 0;
        self.lens = [0; CLIP_HISTORY];
    }
}

fn trim_to_char_edge(bytes: &[u8], max: usize) -> &[u8] {
    if bytes.len() <= max {
        return bytes;
    }
    let mut end = max;
    // Back off over UTF-8 continuation bytes so a multi-byte character is
    // never cut in half.
    while end > 0 && bytes[end] & 0xc0 == 0x80 {
        end -= 1;
    }
    &bytes[..end]
}
