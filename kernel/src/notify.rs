//! Desktop notifications: anything in the system can post one; the desktop
//! shows the newest as a toast and keeps the last few for the notification
//! stack.

use crate::sync::TicketLock;

pub const CAPACITY: usize = 8;
pub const TITLE_MAX: usize = 28;
pub const BODY_MAX: usize = 64;

#[derive(Clone, Copy)]
pub struct Notification {
    pub app: u8,
    pub title: [u8; TITLE_MAX],
    pub title_len: u8,
    pub body: [u8; BODY_MAX],
    pub body_len: u8,
    pub hour: u8,
    pub minute: u8,
}

impl Notification {
    pub const EMPTY: Self = Self {
        app: 0,
        title: [0; TITLE_MAX],
        title_len: 0,
        body: [0; BODY_MAX],
        body_len: 0,
        hour: 0,
        minute: 0,
    };

    pub fn title(&self) -> &str {
        core::str::from_utf8(&self.title[..self.title_len as usize]).unwrap_or("")
    }

    pub fn body(&self) -> &str {
        core::str::from_utf8(&self.body[..self.body_len as usize]).unwrap_or("")
    }
}

struct Store {
    /// Newest first.
    items: [Notification; CAPACITY],
    count: usize,
    generation: u32,
}

static STORE: TicketLock<Store> = TicketLock::new(Store {
    items: [Notification::EMPTY; CAPACITY],
    count: 0,
    generation: 0,
});

fn copy_text(target: &mut [u8], source: &str) -> u8 {
    let mut used = 0;
    for byte in source.bytes() {
        if used == target.len() {
            break;
        }
        if byte.is_ascii_graphic() || byte == b' ' {
            target[used] = byte;
            used += 1;
        }
    }
    used as u8
}

pub fn push(app: u8, title: &str, body: &str) {
    let now = crate::rtc::local_date_time();
    let mut item = Notification::EMPTY;
    item.app = app;
    item.title_len = copy_text(&mut item.title, title);
    item.body_len = copy_text(&mut item.body, body);
    item.hour = now.hour;
    item.minute = now.minute;
    let mut store = STORE.lock();
    store.items.copy_within(0..CAPACITY - 1, 1);
    store.items[0] = item;
    store.count = (store.count + 1).min(CAPACITY);
    store.generation = store.generation.wrapping_add(1);
}

/// Bumps every time something is posted (so the desktop can tell).
pub fn generation() -> u32 {
    STORE.lock().generation
}

/// Newest first; returns how many are held.
pub fn snapshot(out: &mut [Notification; CAPACITY]) -> usize {
    let store = STORE.lock();
    *out = store.items;
    store.count
}

pub fn clear() {
    let mut store = STORE.lock();
    store.count = 0;
    store.generation = store.generation.wrapping_add(1);
}
