//! Persistent system settings (time zone, automatic time, keyboard layout,
//! sound, brightness and the quick-settings switches). Saved to
//! `/data/SETTINGS.DAT` on the boot volume so they survive restarts.

// The Settings app (UI pending) drives the setters.
#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::vfs;

const PATH: &str = "/data/SETTINGS.DAT";
const MAGIC: &[u8; 4] = b"AES1";

static NTP_ENABLED: AtomicBool = AtomicBool::new(true);

pub fn ntp_enabled() -> bool {
    NTP_ENABLED.load(Ordering::Acquire)
}

pub fn set_ntp_enabled(enabled: bool) {
    NTP_ENABLED.store(enabled, Ordering::Release);
    save();
}

pub fn set_keyboard_layout(index: usize) {
    crate::keymap::set_layout(index);
    save();
}

pub fn set_timezone(index: usize) {
    crate::rtc::set_timezone_index(index);
    save();
}

/// What the desktop keeps across restarts besides the above: sound, screen
/// brightness and the quick-settings switches.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Prefs {
    pub volume: u8,
    pub muted: bool,
    pub brightness: u8,
    pub wifi: bool,
    pub bluetooth: bool,
    pub airplane: bool,
    pub battery_saver: bool,
    pub adblock_off: bool,
}

impl Prefs {
    const PRESENT: u64 = 1 << 63;

    fn pack(self) -> u64 {
        let switches = u64::from(self.muted)
            | u64::from(self.wifi) << 1
            | u64::from(self.bluetooth) << 2
            | u64::from(self.airplane) << 3
            | u64::from(self.battery_saver) << 4
            | u64::from(self.adblock_off) << 5;
        Self::PRESENT | u64::from(self.volume) | u64::from(self.brightness) << 8 | switches << 16
    }

    fn unpack(packed: u64) -> Option<Self> {
        let switch = |bit: u32| packed >> (16 + bit) & 1 != 0;
        (packed & Self::PRESENT != 0).then(|| Self {
            volume: (packed as u8).min(100),
            brightness: ((packed >> 8) as u8).clamp(8, 100),
            muted: switch(0),
            wifi: switch(1),
            bluetooth: switch(2),
            airplane: switch(3),
            battery_saver: switch(4),
            adblock_off: switch(5),
        })
    }
}

static PREFS: AtomicU64 = AtomicU64::new(0);

/// The desktop preferences a saved file held, if it held any.
pub fn prefs() -> Option<Prefs> {
    Prefs::unpack(PREFS.load(Ordering::Acquire))
}

pub fn set_prefs(prefs: Prefs) -> bool {
    PREFS.store(prefs.pack(), Ordering::Release);
    save()
}

/// The file: magic, time zone, automatic time, keyboard layout, a spare
/// byte, then (since the desktop preferences were added) those, packed.
const BASE_BYTES: usize = 8;
const FULL_BYTES: usize = 16;

/// Reads the saved settings (if any) and applies them.
pub fn load() -> bool {
    let Ok(descriptor) = vfs::open_file(PATH, false, false, false, 0, false) else {
        return false;
    };
    let mut bytes = [0u8; FULL_BYTES + 1];
    let read = vfs::read(descriptor, &mut bytes);
    let _ = vfs::close(descriptor);
    let Ok(length) = read else {
        return false;
    };
    if (length != BASE_BYTES && length != FULL_BYTES) || &bytes[..4] != MAGIC {
        return false;
    }
    crate::rtc::set_timezone_index(bytes[4] as usize);
    NTP_ENABLED.store(bytes[5] != 0, Ordering::Release);
    crate::keymap::set_layout(bytes[6] as usize);
    if length == FULL_BYTES {
        let mut packed = [0u8; 8];
        packed.copy_from_slice(&bytes[BASE_BYTES..FULL_BYTES]);
        PREFS.store(u64::from_le_bytes(packed), Ordering::Release);
    }
    true
}

pub fn save() -> bool {
    let mut bytes = [0u8; FULL_BYTES];
    bytes[..4].copy_from_slice(MAGIC);
    bytes[4] = crate::rtc::timezone_index() as u8;
    bytes[5] = ntp_enabled() as u8;
    bytes[6] = crate::keymap::layout_index() as u8;
    let packed = PREFS.load(Ordering::Acquire);
    bytes[BASE_BYTES..].copy_from_slice(&packed.to_le_bytes());
    // Without desktop preferences the file stays in its first, short form.
    let length = if packed == 0 { BASE_BYTES } else { FULL_BYTES };
    let Ok(descriptor) = vfs::open_file(PATH, true, false, true, 0o644, true) else {
        return false;
    };
    let written = vfs::write(descriptor, &bytes[..length], false) == Ok(length);
    let _ = vfs::close(descriptor);
    written
}

#[cfg(feature = "boot-test")]
pub fn self_test() -> bool {
    let sample = Prefs {
        volume: 37,
        muted: true,
        brightness: 64,
        wifi: false,
        bluetooth: true,
        airplane: false,
        battery_saver: true,
        adblock_off: false,
    };
    let round_trip = Prefs::unpack(sample.pack()) == Some(sample);
    let absent = Prefs::unpack(0).is_none();
    // Out-of-range values in a damaged file come back inside their ranges.
    let clamped = Prefs::unpack(Prefs::PRESENT | 0xffff)
        .is_some_and(|prefs| prefs.volume == 100 && prefs.brightness == 100)
        && Prefs::unpack(Prefs::PRESENT).is_some_and(|prefs| prefs.brightness == 8);
    round_trip && absent && clamped
}
