//! The kernel key retention service (`add_key`, `request_key`, `keyctl`) for
//! the "user" and "logon" key types: named secrets a program stores and
//! looks up again, held in memory. Every AerOS process runs as one user, so
//! the thread, process, session and user keyrings are one shared keyring;
//! keys can still be read only by the process group that made them (a
//! "logon" key never).

use super::*;
use core::fmt::Write;

const EINVAL: u64 = 22;
const EFAULT: u64 = 14;
const EACCES: u64 = 13;
const ENOKEY: u64 = 126;
const EKEYREVOKED: u64 = 128;
const EDQUOT: u64 = 122;
const EOPNOTSUPP: u64 = 95;
const KEYS: usize = 32;
const PAYLOAD: usize = 2048;
const NAME: usize = 64;
/// The serial of the one keyring the special keyring ids stand for.
const KEYRING: i32 = 0x2a00_0001;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    User,
    Logon,
}

#[derive(Clone, Copy)]
struct Key {
    serial: i32,
    kind: Kind,
    group: u64,
    revoked: bool,
    permissions: u32,
    description: [u8; NAME],
    description_len: usize,
    payload: [u8; PAYLOAD],
    payload_len: usize,
}

const EMPTY: Key = Key {
    serial: 0,
    kind: Kind::User,
    group: 0,
    revoked: false,
    permissions: 0x3f01_0000,
    description: [0; NAME],
    description_len: 0,
    payload: [0; PAYLOAD],
    payload_len: 0,
};

static TABLE: TicketLock<[Key; KEYS]> = TicketLock::new([EMPTY; KEYS]);
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(0x1000_0001);

fn kind_of(name: &[u8]) -> Option<Kind> {
    match name {
        b"user" => Some(Kind::User),
        b"logon" => Some(Kind::Logon),
        _ => None,
    }
}

fn read_string(address: u64, out: &mut [u8; NAME]) -> Result<usize, u64> {
    let mut buffer = [0u8; NAME + 1];
    let length = user::copy_string(address, &mut buffer).ok_or(EFAULT)?;
    if length == 0 || length > NAME {
        return Err(EINVAL);
    }
    out[..length].copy_from_slice(&buffer[..length]);
    Ok(length)
}

/// A keyring argument: one of the special ids or the keyring's serial.
fn keyring_valid(id: u64) -> bool {
    let id = id as i32;
    (-5..=-1).contains(&id) || id == KEYRING
}

fn find_key(table: &mut [Key; KEYS], serial: i32) -> Option<&mut Key> {
    table.iter_mut().find(|key| key.serial != 0 && key.serial == serial)
}

fn mine(key: &Key) -> bool {
    key.group == crate::scheduler::current_group()
}

/// `add_key(type, description, payload, plen, keyring)`.
pub(super) fn add_key(kind: u64, description: u64, payload: u64, length: u64, keyring: u64) -> u64 {
    let mut kind_name = [0u8; NAME];
    let kind = match read_string(kind, &mut kind_name) {
        Ok(len) => kind_of(&kind_name[..len]),
        Err(errno) => return error(errno),
    };
    let Some(kind) = kind else {
        return error(if kind_name.starts_with(b"keyring") { EOPNOTSUPP } else { 19 });
    };
    let mut name = [0u8; NAME];
    let name_len = match read_string(description, &mut name) {
        Ok(len) => len,
        Err(errno) => return error(errno),
    };
    if !keyring_valid(keyring) {
        return error(EINVAL);
    }
    if length > 32_767 {
        return error(EINVAL);
    }
    if length as usize > PAYLOAD {
        return error(EDQUOT);
    }
    let mut data = [0u8; PAYLOAD];
    if length != 0 && !user::copy_from_user(payload, &mut data[..length as usize]) {
        return error(EFAULT);
    }
    let group = crate::scheduler::current_group();
    let mut table = TABLE.lock();
    let existing = table.iter().position(|key| {
        key.serial != 0
            && key.group == group
            && key.kind == kind
            && key.description[..key.description_len] == name[..name_len]
    });
    let index = match existing.or_else(|| table.iter().position(|key| key.serial == 0)) {
        Some(index) => index,
        None => return error(EDQUOT),
    };
    let serial = if existing.is_some() {
        table[index].serial
    } else {
        (NEXT_SERIAL.fetch_add(1, Ordering::Relaxed) & 0x3fff_ffff) as i32
    };
    let key = &mut table[index];
    *key = Key {
        serial,
        kind,
        group,
        description: name,
        description_len: name_len,
        payload_len: length as usize,
        ..EMPTY
    };
    key.payload[..length as usize].copy_from_slice(&data[..length as usize]);
    serial as u64
}

fn search(kind: Kind, name: &[u8]) -> u64 {
    let table = TABLE.lock();
    match table.iter().find(|key| {
        key.serial != 0
            && mine(key)
            && !key.revoked
            && key.kind == kind
            && key.description[..key.description_len] == *name
    }) {
        Some(key) => key.serial as u64,
        None => error(ENOKEY),
    }
}

/// `request_key(type, description, callout, dest_keyring)`: only keys that
/// already exist are found (there is no user-space helper to make one).
pub(super) fn request_key(kind: u64, description: u64) -> u64 {
    let mut kind_name = [0u8; NAME];
    let kind = match read_string(kind, &mut kind_name) {
        Ok(len) => kind_of(&kind_name[..len]),
        Err(errno) => return error(errno),
    };
    let Some(kind) = kind else {
        return error(ENOKEY);
    };
    let mut name = [0u8; NAME];
    match read_string(description, &mut name) {
        Ok(len) => search(kind, &name[..len]),
        Err(errno) => error(errno),
    }
}

fn copy_out(address: u64, capacity: u64, data: &[u8]) -> u64 {
    let take = data.len().min(capacity as usize);
    if take != 0 && address != 0 && !user::copy_to_user(address, &data[..take]) {
        return error(EFAULT);
    }
    data.len() as u64
}

/// `keyctl(operation, ...)`.
pub(super) fn keyctl(operation: u64, a: [u64; 6]) -> u64 {
    match operation {
        // GET_KEYRING_ID, JOIN_SESSION_KEYRING
        0 if keyring_valid(a[1]) => KEYRING as u64,
        0 => error(ENOKEY),
        1 => KEYRING as u64,
        // UPDATE
        2 => {
            if a[3] as usize > PAYLOAD {
                return error(EDQUOT);
            }
            let mut data = [0u8; PAYLOAD];
            if a[3] != 0 && !user::copy_from_user(a[2], &mut data[..a[3] as usize]) {
                return error(EFAULT);
            }
            let mut table = TABLE.lock();
            let Some(key) = find_key(&mut table, a[1] as i32) else {
                return error(ENOKEY);
            };
            if !mine(key) {
                return error(EACCES);
            }
            if key.revoked {
                return error(EKEYREVOKED);
            }
            key.payload[..a[3] as usize].copy_from_slice(&data[..a[3] as usize]);
            key.payload_len = a[3] as usize;
            0
        }
        // REVOKE, UNLINK, INVALIDATE
        3 | 9 | 21 => {
            let mut table = TABLE.lock();
            let Some(key) = find_key(&mut table, a[1] as i32) else {
                return error(ENOKEY);
            };
            if !mine(key) {
                return error(EACCES);
            }
            if operation == 3 {
                key.revoked = true;
            } else {
                *key = EMPTY;
            }
            0
        }
        // CHOWN: there is one user.
        4 => 0,
        // SETPERM
        5 => {
            let mut table = TABLE.lock();
            match find_key(&mut table, a[1] as i32) {
                Some(key) if mine(key) => {
                    key.permissions = a[2] as u32;
                    0
                }
                Some(_) => error(EACCES),
                None => error(ENOKEY),
            }
        }
        // DESCRIBE: "type;uid;gid;perm;description".
        6 => {
            let mut text = [0u8; NAME + 48];
            let length = {
                let mut table = TABLE.lock();
                if a[1] as i32 == KEYRING || (-5..=-1).contains(&(a[1] as i32)) {
                    let mut writer = Cursor::new(&mut text);
                    let _ = write!(writer, "keyring;0;0;3f3f0000;_ses");
                    writer.len()
                } else {
                    let Some(key) = find_key(&mut table, a[1] as i32) else {
                        return error(ENOKEY);
                    };
                    if !mine(key) {
                        return error(EACCES);
                    }
                    let kind = if key.kind == Kind::User { "user" } else { "logon" };
                    let mut writer = Cursor::new(&mut text);
                    let _ = write!(writer, "{kind};0;0;{:08x};", key.permissions);
                    let description =
                        core::str::from_utf8(&key.description[..key.description_len]).unwrap_or("");
                    let _ = writer.write_str(description);
                    writer.len()
                }
            };
            // The terminating zero is part of the answer.
            copy_out(a[2], a[3], &text[..(length + 1).min(text.len())])
        }
        // CLEAR the keyring: this group's keys.
        7 => {
            if !keyring_valid(a[1]) {
                return error(ENOKEY);
            }
            let mut table = TABLE.lock();
            for key in table.iter_mut().filter(|key| key.serial != 0 && mine(key)) {
                *key = EMPTY;
            }
            0
        }
        // LINK: keys already live in the one keyring.
        8 if keyring_valid(a[2]) => 0,
        8 => error(EINVAL),
        // SEARCH(keyring, type, description, dest)
        10 => {
            if !keyring_valid(a[1]) {
                return error(ENOKEY);
            }
            let mut kind_name = [0u8; NAME];
            let kind = match read_string(a[2], &mut kind_name) {
                Ok(len) => kind_of(&kind_name[..len]),
                Err(errno) => return error(errno),
            };
            let Some(kind) = kind else {
                return error(ENOKEY);
            };
            let mut name = [0u8; NAME];
            match read_string(a[3], &mut name) {
                Ok(len) => search(kind, &name[..len]),
                Err(errno) => error(errno),
            }
        }
        // READ: a key's payload, or the keyring's list of serials.
        11 => {
            let mut table = TABLE.lock();
            if keyring_valid(a[1]) {
                let mut list = [0u8; KEYS * 4];
                let mut length = 0;
                for key in table.iter().filter(|key| key.serial != 0 && mine(key)) {
                    list[length..length + 4].copy_from_slice(&key.serial.to_le_bytes());
                    length += 4;
                }
                return copy_out(a[2], a[3], &list[..length]);
            }
            let Some(key) = find_key(&mut table, a[1] as i32) else {
                return error(ENOKEY);
            };
            if !mine(key) || key.kind == Kind::Logon {
                return error(EACCES);
            }
            if key.revoked {
                return error(EKEYREVOKED);
            }
            let payload = key.payload;
            let length = key.payload_len;
            drop(table);
            copy_out(a[2], a[3], &payload[..length])
        }
        // SET_REQKEY_KEYRING: the previous default (the default one).
        14 => 0,
        // SET_TIMEOUT: keys do not expire, which a timeout of 0 means.
        15 => {
            let mut table = TABLE.lock();
            match find_key(&mut table, a[1] as i32) {
                Some(key) if mine(key) => 0,
                Some(_) => error(EACCES),
                None => error(ENOKEY),
            }
        }
        // GET_SECURITY: no security module labels.
        17 => copy_out(a[2], a[3], b"\0"),
        _ => error(EOPNOTSUPP),
    }
}

struct Cursor<'a> {
    buffer: &'a mut [u8],
    used: usize,
}

impl<'a> Cursor<'a> {
    fn new(buffer: &'a mut [u8]) -> Self {
        Self { buffer, used: 0 }
    }

    fn len(&self) -> usize {
        self.used
    }
}

impl core::fmt::Write for Cursor<'_> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let room = self.buffer.len().saturating_sub(self.used + 1);
        let take = text.len().min(room);
        self.buffer[self.used..self.used + take].copy_from_slice(&text.as_bytes()[..take]);
        self.used += take;
        Ok(())
    }
}
