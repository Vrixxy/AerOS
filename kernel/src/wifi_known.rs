//! The Wi-Fi networks this machine has joined, with their passwords, kept in
//! `/data/WIFI.DAT` (a file only the system reads) so the next start can
//! join one again without asking. Most recently joined first.

use crate::sync::TicketLock;
use crate::vfs;

const PATH: &str = "/data/WIFI.DAT";
const MAGIC: &[u8; 4] = b"AEW1";
pub const MAX_KNOWN: usize = 4;
const SSID_MAX: usize = 32;
const PASSPHRASE_MAX: usize = 64;
const ENTRY_BYTES: usize = 2 + SSID_MAX + PASSPHRASE_MAX;
const FILE_BYTES: usize = 5 + MAX_KNOWN * ENTRY_BYTES;

#[derive(Clone, Copy)]
pub struct Known {
    ssid: [u8; SSID_MAX],
    ssid_len: usize,
    passphrase: [u8; PASSPHRASE_MAX],
    passphrase_len: usize,
}

impl Known {
    const EMPTY: Self = Self {
        ssid: [0; SSID_MAX],
        ssid_len: 0,
        passphrase: [0; PASSPHRASE_MAX],
        passphrase_len: 0,
    };

    pub fn ssid(&self) -> &[u8] {
        &self.ssid[..self.ssid_len]
    }

    pub fn passphrase(&self) -> &[u8] {
        &self.passphrase[..self.passphrase_len]
    }
}

struct Store {
    loaded: bool,
    count: usize,
    entries: [Known; MAX_KNOWN],
}

impl Store {
    const EMPTY: Self = Self {
        loaded: false,
        count: 0,
        entries: [Known::EMPTY; MAX_KNOWN],
    };

    fn lookup(&self, ssid: &[u8]) -> Option<Known> {
        self.entries[..self.count]
            .iter()
            .copied()
            .find(|known| known.ssid() == ssid)
    }

    /// Puts the network first, replacing what was known under its name; the
    /// oldest one goes when there is no room. False for a name or password
    /// that cannot be stored.
    fn remember(&mut self, ssid: &[u8], passphrase: &[u8]) -> bool {
        if ssid.is_empty() || ssid.len() > SSID_MAX || passphrase.len() > PASSPHRASE_MAX {
            return false;
        }
        let mut kept = [Known::EMPTY; MAX_KNOWN];
        let mut count = 1;
        kept[0].ssid[..ssid.len()].copy_from_slice(ssid);
        kept[0].ssid_len = ssid.len();
        kept[0].passphrase[..passphrase.len()].copy_from_slice(passphrase);
        kept[0].passphrase_len = passphrase.len();
        for known in &self.entries[..self.count] {
            if known.ssid() != ssid && count < MAX_KNOWN {
                kept[count] = *known;
                count += 1;
            }
        }
        self.entries = kept;
        self.count = count;
        true
    }

    fn forget(&mut self, ssid: &[u8]) -> bool {
        let Some(index) = self.entries[..self.count]
            .iter()
            .position(|known| known.ssid() == ssid)
        else {
            return false;
        };
        self.entries.copy_within(index + 1..self.count, index);
        self.count -= 1;
        self.entries[self.count] = Known::EMPTY;
        true
    }

    fn encode(&self) -> [u8; FILE_BYTES] {
        let mut bytes = [0u8; FILE_BYTES];
        bytes[..4].copy_from_slice(MAGIC);
        bytes[4] = self.count as u8;
        for (index, known) in self.entries[..self.count].iter().enumerate() {
            let entry = &mut bytes[5 + index * ENTRY_BYTES..5 + (index + 1) * ENTRY_BYTES];
            entry[0] = known.ssid_len as u8;
            entry[1] = known.passphrase_len as u8;
            entry[2..2 + SSID_MAX].copy_from_slice(&known.ssid);
            entry[2 + SSID_MAX..].copy_from_slice(&known.passphrase);
        }
        bytes
    }

    /// The networks in a saved file; nothing when any part of it is off.
    fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != FILE_BYTES || &bytes[..4] != MAGIC {
            return None;
        }
        let count = usize::from(bytes[4]);
        if count > MAX_KNOWN {
            return None;
        }
        let mut store = Self::EMPTY;
        for index in 0..count {
            let entry = &bytes[5 + index * ENTRY_BYTES..5 + (index + 1) * ENTRY_BYTES];
            let (ssid_len, passphrase_len) = (usize::from(entry[0]), usize::from(entry[1]));
            if ssid_len == 0 || ssid_len > SSID_MAX || passphrase_len > PASSPHRASE_MAX {
                return None;
            }
            let known = &mut store.entries[index];
            known.ssid_len = ssid_len;
            known.passphrase_len = passphrase_len;
            known.ssid[..ssid_len].copy_from_slice(&entry[2..2 + ssid_len]);
            known.passphrase[..passphrase_len]
                .copy_from_slice(&entry[2 + SSID_MAX..2 + SSID_MAX + passphrase_len]);
        }
        store.count = count;
        Some(store)
    }
}

static STORE: TicketLock<Store> = TicketLock::new(Store::EMPTY);

fn with_store<R>(body: impl FnOnce(&mut Store) -> R) -> R {
    let mut store = STORE.lock();
    if !store.loaded {
        let mut bytes = [0u8; FILE_BYTES + 1];
        let read = vfs::as_system(|| {
            let descriptor = vfs::open_file(PATH, false, false, false, 0, false).ok()?;
            let read = vfs::read(descriptor, &mut bytes).ok();
            let _ = vfs::close(descriptor);
            read
        });
        if let Some(saved) = read.and_then(|length| Store::decode(&bytes[..length])) {
            *store = saved;
        }
        crate::auth::wipe(&mut bytes);
        store.loaded = true;
    }
    body(&mut store)
}

/// Whether any network has been joined before.
pub fn any() -> bool {
    with_store(|store| store.count != 0)
}

/// The saved network of that name.
pub fn lookup(ssid: &[u8]) -> Option<Known> {
    with_store(|store| store.lookup(ssid))
}

fn save(store: &Store) -> bool {
    let mut bytes = store.encode();
    let written = vfs::as_system(|| {
        let Ok(descriptor) = vfs::open_file(PATH, true, false, true, 0o600, true) else {
            return false;
        };
        let written = vfs::write(descriptor, &bytes, false) == Ok(bytes.len());
        let _ = vfs::close(descriptor);
        written
    });
    crate::auth::wipe(&mut bytes);
    written
}

/// Keeps a network that was just joined (an open one has no password).
pub fn remember(ssid: &[u8], passphrase: &[u8]) -> bool {
    with_store(|store| store.remember(ssid, passphrase) && save(store))
}

/// Drops a saved network and its password.
pub fn forget(ssid: &[u8]) -> bool {
    with_store(|store| store.forget(ssid) && save(store))
}

#[cfg(feature = "boot-test")]
pub fn self_test() -> bool {
    let mut store = Store::EMPTY;
    let filled = store.remember(b"home", b"correct horse")
        && store.remember(b"cafe", b"")
        && store.remember(b"work", b"battery staple");
    let order = store.count == 3
        && store.entries[0].ssid() == b"work"
        && store.entries[2].ssid() == b"home"
        && store
            .lookup(b"cafe")
            .is_some_and(|known| known.passphrase().is_empty())
        && store.lookup(b"nowhere").is_none();
    // Joining a known network again moves it to the front with its new password.
    let renewed = store.remember(b"home", b"new password")
        && store.count == 3
        && store.entries[0].ssid() == b"home"
        && store.entries[0].passphrase() == b"new password";
    let full = store.remember(b"four", b"44444444")
        && store.remember(b"five", b"55555555")
        && store.count == MAX_KNOWN
        && store.entries[0].ssid() == b"five"
        && store.lookup(b"cafe").is_none();
    let forgotten = store.forget(b"work")
        && store.count == MAX_KNOWN - 1
        && store.lookup(b"work").is_none()
        && store.entries[0].ssid() == b"five"
        && store.entries[store.count].ssid_len == 0
        && !store.forget(b"work")
        && store.remember(b"work", b"battery staple");
    let refused = !store.remember(b"", b"password")
        && !store.remember(&[b'x'; SSID_MAX + 1], b"password")
        && !store.remember(b"name", &[b'p'; PASSPHRASE_MAX + 1]);
    let bytes = store.encode();
    let round_trip = Store::decode(&bytes).is_some_and(|back| {
        back.count == store.count
            && back.entries[..back.count]
                .iter()
                .zip(&store.entries[..store.count])
                .all(|(a, b)| a.ssid() == b.ssid() && a.passphrase() == b.passphrase())
    });
    let mut bad_count = bytes;
    bad_count[4] = MAX_KNOWN as u8 + 1;
    let mut bad_length = bytes;
    bad_length[5] = SSID_MAX as u8 + 1;
    let mut bad_magic = bytes;
    bad_magic[0] ^= 1;
    let rejected = Store::decode(&bad_count).is_none()
        && Store::decode(&bad_length).is_none()
        && Store::decode(&bad_magic).is_none()
        && Store::decode(&bytes[..FILE_BYTES - 1]).is_none();
    filled && order && renewed && full && forgotten && refused && round_trip && rejected
}
