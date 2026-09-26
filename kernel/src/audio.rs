//! One audio output for the desktop: Intel HD Audio when present, otherwise
//! AC'97. The music player only talks to this module.

use crate::{ac97, hda};

pub fn play_song(song: usize) {
    if hda::ready() {
        hda::play_song(song);
    } else {
        ac97::play_song(song);
    }
}

pub fn play_effect(effect: crate::sfx::Effect) {
    if hda::ready() {
        hda::play_effect(effect);
    } else {
        ac97::play_effect(effect);
    }
}

pub fn stop() {
    hda::stop();
    ac97::stop();
}

pub fn resume() {
    if hda::ready() {
        hda::resume();
    } else {
        ac97::resume();
    }
}

pub fn set_volume(percent: u8) {
    hda::set_volume(percent);
    ac97::set_volume(percent);
}

pub fn pump() {
    hda::pump();
    ac97::pump();
}
