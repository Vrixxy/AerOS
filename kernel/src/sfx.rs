//! Short sound effects (boot chime, notification) mixed on top of whatever
//! the audio driver is already playing. The drivers call `mix` for every
//! buffer they fill.

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

const BOOT: &[u8] = include_bytes!("../../assets/sounds/bootchime.pcm");
const NOTIFICATION: &[u8] = include_bytes!("../../assets/sounds/notification.pcm");

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Boot,
    Notification,
}

/// 0 = idle, 1 = boot chime, 2 = notification.
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
/// Position in output frames (48 kHz); the effects are 24 kHz.
static POSITION: AtomicU32 = AtomicU32::new(0);

fn data(effect: usize) -> &'static [u8] {
    match effect {
        1 => BOOT,
        _ => NOTIFICATION,
    }
}

pub fn start(effect: Effect) {
    POSITION.store(0, Ordering::Release);
    ACTIVE.store(
        match effect {
            Effect::Boot => 1,
            Effect::Notification => 2,
        },
        Ordering::Release,
    );
}

pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire) != 0
}

fn sample(bytes: &[u8], index: usize) -> i32 {
    bytes
        .get(index * 2..index * 2 + 2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as i32)
        .unwrap_or(0)
}

/// Adds the current effect to interleaved stereo 48 kHz `out`.
pub fn mix(out: &mut [i16], volume: i32) {
    let effect = ACTIVE.load(Ordering::Acquire);
    if effect == 0 {
        return;
    }
    let bytes = data(effect);
    let count = bytes.len() / 2;
    let mut position = POSITION.load(Ordering::Acquire) as usize;
    let gain = volume.clamp(0, 100).max(35);
    for frame in out.chunks_exact_mut(2) {
        let index = position / 2;
        if index + 1 >= count {
            ACTIVE.store(0, Ordering::Release);
            break;
        }
        let first = sample(bytes, index);
        let second = sample(bytes, index + 1);
        let value = if position.is_multiple_of(2) {
            first
        } else {
            (first + second) / 2
        } * gain
            / 100;
        for channel in frame.iter_mut() {
            *channel = (*channel as i32 + value).clamp(-32_768, 32_767) as i16;
        }
        position += 1;
    }
    POSITION.store(position as u32, Ordering::Release);
}
