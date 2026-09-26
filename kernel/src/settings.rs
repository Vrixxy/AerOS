//! Persistent system settings (time zone, automatic time). Saved to
//! `/data/SETTINGS.DAT` on the boot volume so they survive restarts.

// The Settings app (UI pending) drives the setters.
#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, Ordering};

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

/// Reads the saved settings (if any) and applies them.
pub fn load() -> bool {
    let Ok(descriptor) = vfs::open_file(PATH, false, false, false, 0, false) else {
        return false;
    };
    let mut bytes = [0u8; 8];
    let read = vfs::read(descriptor, &mut bytes);
    let _ = vfs::close(descriptor);
    if read != Ok(8) || &bytes[..4] != MAGIC {
        return false;
    }
    crate::rtc::set_timezone_index(bytes[4] as usize);
    NTP_ENABLED.store(bytes[5] != 0, Ordering::Release);
    crate::keymap::set_layout(bytes[6] as usize);
    true
}

pub fn save() -> bool {
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(MAGIC);
    bytes[4] = crate::rtc::timezone_index() as u8;
    bytes[5] = ntp_enabled() as u8;
    bytes[6] = crate::keymap::layout_index() as u8;
    let Ok(descriptor) = vfs::open_file(PATH, true, false, true, 0o644, true) else {
        return false;
    };
    let written = vfs::write(descriptor, &bytes, false) == Ok(8);
    let _ = vfs::close(descriptor);
    written
}
