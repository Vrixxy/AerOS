//! Intel AC'97 audio (the ICH codec QEMU emulates): one PCM-out stream fed
//! from a ring of buffers by a small tone synthesiser. The desktop calls
//! `pump()` frequently; a song keeps playing as long as it is pumped.

use crate::arch;
use crate::memory::FrameAllocator;
use crate::pci::PciInventory;
use crate::sync::TicketLock;

const PAGE_SIZE: u64 = 4096;
const BDL_ENTRIES: usize = 32;
const SAMPLES_PER_BUFFER: usize = 4096;
const BUFFER_BYTES: usize = SAMPLES_PER_BUFFER * 2;
const RATE: u32 = 48_000;

const NAM_RESET: u16 = 0x00;
const NAM_MASTER_VOLUME: u16 = 0x02;
const NAM_PCM_VOLUME: u16 = 0x18;
const NABM_PO_BDBAR: u16 = 0x10;
const NABM_PO_CIV: u16 = 0x14;
const NABM_PO_LVI: u16 = 0x15;
const NABM_PO_SR: u16 = 0x16;
const NABM_PO_CR: u16 = 0x1b;
const NABM_GLOBAL_CONTROL: u16 = 0x2c;
const NABM_GLOBAL_STATUS: u16 = 0x30;

const SR_HALTED: u16 = 1;
const CR_RUN: u8 = 1;
const CR_RESET: u8 = 2;

#[derive(Clone, Copy)]
pub struct Ac97Report {
    pub present: bool,
    pub nam: u16,
    pub nabm: u16,
    pub codec_ready: bool,
    pub buffers_played: u32,
    pub verified: bool,
}

impl Ac97Report {
    pub const EMPTY: Self = Self {
        present: false,
        nam: 0,
        nabm: 0,
        codec_ready: false,
        buffers_played: 0,
        verified: false,
    };
}

/// (semitones above C4, length in eighth notes); 255 = rest.
type Note = (u8, u8);

struct Song {
    melody: &'static [Note],
    bass: &'static [Note],
    /// Frames per eighth note.
    eighth: u32,
}

const SONGS: [Song; 4] = [
    Song {
        melody: &[
            (9, 1),
            (12, 1),
            (16, 1),
            (12, 1),
            (9, 1),
            (12, 1),
            (16, 1),
            (19, 1),
            (5, 1),
            (9, 1),
            (12, 1),
            (9, 1),
            (5, 1),
            (9, 1),
            (12, 1),
            (17, 1),
            (0, 1),
            (4, 1),
            (7, 1),
            (4, 1),
            (0, 1),
            (4, 1),
            (7, 1),
            (12, 1),
            (7, 1),
            (11, 1),
            (14, 1),
            (11, 1),
            (7, 1),
            (11, 1),
            (14, 1),
            (19, 1),
        ],
        bass: &[(255, 0), (9, 8), (5, 8), (0, 8), (7, 8)],
        eighth: 14_400,
    },
    Song {
        melody: &[
            (7, 2),
            (12, 2),
            (16, 4),
            (14, 2),
            (12, 2),
            (9, 4),
            (7, 2),
            (9, 2),
            (12, 4),
            (16, 2),
            (19, 2),
            (24, 4),
            (19, 4),
        ],
        bass: &[(255, 0), (0, 8), (5, 8), (7, 8), (0, 8)],
        eighth: 18_000,
    },
    Song {
        melody: &[
            (4, 1),
            (7, 1),
            (11, 1),
            (16, 1),
            (4, 1),
            (7, 1),
            (11, 1),
            (16, 1),
            (2, 1),
            (5, 1),
            (9, 1),
            (14, 1),
            (2, 1),
            (5, 1),
            (9, 1),
            (14, 1),
            (0, 1),
            (4, 1),
            (7, 1),
            (12, 1),
            (0, 1),
            (4, 1),
            (7, 1),
            (12, 1),
        ],
        bass: &[(255, 0), (4, 8), (2, 8), (0, 8)],
        eighth: 12_000,
    },
    Song {
        melody: &[
            (12, 3),
            (11, 1),
            (9, 2),
            (7, 2),
            (9, 3),
            (7, 1),
            (4, 2),
            (2, 2),
            (4, 4),
            (7, 4),
            (9, 2),
            (12, 2),
            (16, 4),
        ],
        bass: &[(255, 0), (9, 8), (4, 8), (5, 8), (7, 8)],
        eighth: 21_600,
    },
];

/// Frequency of `semitones` above C4 as a phase increment (2^32 = one cycle
/// per sample).
fn phase_step(semitones: u8, octave_shift: i32) -> u32 {
    const C4_TO_B4: [u32; 12] = [262, 277, 294, 311, 330, 349, 370, 392, 415, 440, 466, 494];
    let octave = (semitones / 12) as i32 + octave_shift;
    let mut hertz = C4_TO_B4[(semitones % 12) as usize] as u64;
    if octave >= 0 {
        hertz <<= octave;
    } else {
        hertz >>= -octave;
    }
    ((hertz << 32) / RATE as u64) as u32
}

/// Sine from a 32-bit phase, as a signed 16-bit value (parabola with a
/// correction term, accurate to a couple of percent - plenty for a tone).
fn sine(phase: u32) -> i32 {
    let x = (phase >> 16) as i32 - 32_768; // -32768..32767 over one cycle
    let folded = if x < 0 { -x } else { x }; // 0..32768 (triangle)
    let centered = folded - 16_384; // -16384..16384
    let square = (centered as i64 * centered as i64 / 16_384) as i32; // 0..16384
    let parabola = 16_384 - square; // 0..16384 hump
    let signed = if x < 0 { -parabola } else { parabola };
    signed * 2
}

pub struct Player {
    song: usize,
    melody_index: usize,
    melody_left: u32,
    melody_phase: u32,
    bass_index: usize,
    bass_left: u32,
    bass_phase: u32,
    melody_age: u32,
    melody_step: u32,
    bass_step: u32,
}

impl Player {
    pub const fn new(song: usize) -> Self {
        Self {
            song,
            melody_index: 0,
            melody_left: 0,
            melody_phase: 0,
            bass_index: 0,
            bass_left: 0,
            bass_phase: 0,
            melody_age: 0,
            melody_step: 0,
            bass_step: 0,
        }
    }

    pub fn render(&mut self, out: &mut [i16], volume: i32) {
        let song = &SONGS[self.song % SONGS.len()];
        for frame in out.chunks_exact_mut(2) {
            if self.melody_left == 0 {
                let (note, eighths) = song.melody[self.melody_index % song.melody.len()];
                self.melody_index = (self.melody_index + 1) % song.melody.len();
                self.melody_left = eighths.max(1) as u32 * song.eighth;
                self.melody_age = 0;
                self.melody_step = if note == 255 { 0 } else { phase_step(note, 1) };
            }
            if self.bass_left == 0 {
                let (note, eighths) = song.bass[self.bass_index % song.bass.len()];
                self.bass_index = (self.bass_index + 1) % song.bass.len();
                self.bass_left = eighths.max(1) as u32 * song.eighth;
                self.bass_step = if note == 255 { 0 } else { phase_step(note, -1) };
            }
            self.melody_phase = self.melody_phase.wrapping_add(self.melody_step);
            self.bass_phase = self.bass_phase.wrapping_add(self.bass_step);
            self.melody_left -= 1;
            self.bass_left -= 1;
            self.melody_age += 1;
            // Melody: a plucked decay; bass: steady and quiet.
            let decay = 1024 - (self.melody_age.min(24_000) as i32 * 900 / 24_000);
            let attack = (self.melody_age.min(200) as i32) * 1024 / 200;
            let envelope = decay.min(attack + 24);
            let lead = sine(self.melody_phase) * envelope / 1024;
            let low = sine(self.bass_phase) / 2;
            let mixed = (lead * 5 / 8 + low * 3 / 8) * volume / 100;
            let value = mixed.clamp(-32_000, 32_000) as i16;
            frame[0] = value;
            frame[1] = value;
        }
    }
}

struct Ac97State {
    nam: u16,
    nabm: u16,
    bdl: u64,
    buffers: u64,
    next_fill: usize,
    ready: bool,
    playing: bool,
    stream_on: bool,
    quiet: u32,
    volume: i32,
    player: Player,
}

impl Ac97State {
    const EMPTY: Self = Self {
        nam: 0,
        nabm: 0,
        bdl: 0,
        buffers: 0,
        next_fill: 0,
        ready: false,
        playing: false,
        stream_on: false,
        quiet: 0,
        volume: 60,
        player: Player::new(0),
    };
}

static AC97: TicketLock<Ac97State> = TicketLock::new(Ac97State::EMPTY);

unsafe fn inw(port: u16) -> u16 {
    let value: u16;
    unsafe {
        core::arch::asm!("in ax, dx", in("dx") port, out("ax") value, options(nomem, nostack, preserves_flags));
    }
    value
}

impl Ac97State {
    fn buffer(&self, index: usize) -> *mut i16 {
        (self.buffers as usize + index * BUFFER_BYTES) as *mut i16
    }

    fn fill(&mut self, index: usize) {
        let samples =
            unsafe { core::slice::from_raw_parts_mut(self.buffer(index), SAMPLES_PER_BUFFER) };
        if self.playing {
            let volume = self.volume;
            self.player.render(samples, volume);
        } else {
            samples.fill(0);
        }
        if self.playing || crate::sfx::active() {
            self.quiet = 0;
        } else {
            self.quiet = self.quiet.saturating_add(1);
        }
        crate::sfx::mix(samples, self.volume);
    }

    fn stop_stream(&mut self) {
        unsafe { arch::outb(self.nabm + NABM_PO_CR, 0) };
        self.stream_on = false;
    }

    fn start_stream(&mut self) {
        unsafe {
            arch::outb(self.nabm + NABM_PO_CR, CR_RESET);
            let mut waited = 0;
            while arch::inb(self.nabm + NABM_PO_CR) & CR_RESET != 0 && waited < 1_000_000 {
                waited += 1;
            }
            arch::outl(self.nabm + NABM_PO_BDBAR, self.bdl as u32);
        }
        self.next_fill = 0;
        for index in 0..8 {
            self.fill(index);
        }
        self.next_fill = 8;
        unsafe {
            arch::outb(self.nabm + NABM_PO_LVI, 7);
            arch::outb(self.nabm + NABM_PO_CR, CR_RUN);
        }
    }

    /// Keeps the ring topped up; restarts the stream after an underrun.
    fn pump(&mut self) {
        if !self.ready || !self.stream_on {
            return;
        }
        if !self.playing && self.quiet as usize > BDL_ENTRIES + 4 {
            self.stop_stream();
            return;
        }
        let (civ, status) = unsafe {
            (
                arch::inb(self.nabm + NABM_PO_CIV) as usize % BDL_ENTRIES,
                inw(self.nabm + NABM_PO_SR),
            )
        };
        if status & SR_HALTED != 0 {
            // Underrun: the controller stopped at the last valid entry.
            unsafe { arch::outw(self.nabm + NABM_PO_SR, 0x1c) };
            self.next_fill = (civ + 1) % BDL_ENTRIES;
            for _ in 0..8 {
                let index = self.next_fill;
                self.fill(index);
                self.next_fill = (index + 1) % BDL_ENTRIES;
            }
            unsafe {
                arch::outb(
                    self.nabm + NABM_PO_LVI,
                    ((self.next_fill + BDL_ENTRIES - 1) % BDL_ENTRIES) as u8,
                );
                arch::outb(self.nabm + NABM_PO_CR, CR_RUN);
            }
            return;
        }
        let mut pending = (self.next_fill + BDL_ENTRIES - civ) % BDL_ENTRIES;
        while pending < BDL_ENTRIES - 4 {
            let index = self.next_fill;
            self.fill(index);
            self.next_fill = (index + 1) % BDL_ENTRIES;
            pending += 1;
        }
        unsafe {
            arch::outb(
                self.nabm + NABM_PO_LVI,
                ((self.next_fill + BDL_ENTRIES - 1) % BDL_ENTRIES) as u8,
            );
        }
    }
}

pub fn pump() {
    AC97.lock().pump();
}

/// Starts (or restarts) a song from its beginning.
pub fn play_song(song: usize) {
    let mut state = AC97.lock();
    if !state.ready {
        return;
    }
    state.player = Player::new(song);
    state.playing = true;
    state.stream_on = true;
    state.start_stream();
}

/// Plays a short effect over whatever is playing (starting the stream if
/// nothing is).
pub fn play_effect(effect: crate::sfx::Effect) {
    let mut state = AC97.lock();
    if !state.ready {
        return;
    }
    crate::sfx::start(effect);
    if !state.stream_on {
        state.stream_on = true;
        state.quiet = 0;
        state.start_stream();
    }
}

/// Stops the stream (the ring is left silent).
pub fn stop() {
    let mut state = AC97.lock();
    if !state.ready {
        return;
    }
    state.playing = false;
    if !crate::sfx::active() {
        state.stop_stream();
    }
}

/// Resumes a stopped song where it left off.
pub fn resume() {
    let mut state = AC97.lock();
    if !state.ready || state.playing {
        return;
    }
    state.playing = true;
    state.stream_on = true;
    state.start_stream();
}

/// 0..=100 software volume (applied when samples are rendered).
pub fn set_volume(percent: u8) {
    AC97.lock().volume = percent.min(100) as i32;
}

pub fn initialize(pci: &PciInventory, frames: &mut FrameAllocator) -> Ac97Report {
    let Some(device) = pci.find_class(0x04, 0x01, 0x00) else {
        return Ac97Report::EMPTY;
    };
    if device.bars[0] & 1 == 0 || device.bars[1] & 1 == 0 || !pci.enable_io_bus_master(device) {
        return Ac97Report::EMPTY;
    }
    let nam = (device.bars[0] & 0xfffc) as u16;
    let nabm = (device.bars[1] & 0xfffc) as u16;
    let mut report = Ac97Report {
        present: true,
        nam,
        nabm,
        ..Ac97Report::EMPTY
    };
    // Cold reset, then wait for the primary codec.
    unsafe {
        arch::outl(nabm + NABM_GLOBAL_CONTROL, 0x02);
        let mut waited = 0;
        while arch::inl(nabm + NABM_GLOBAL_STATUS) & (1 << 8) == 0 && waited < 5_000_000 {
            waited += 1;
        }
        report.codec_ready = arch::inl(nabm + NABM_GLOBAL_STATUS) & (1 << 8) != 0;
        arch::outw(nam + NAM_RESET, 1);
        arch::outw(nam + NAM_MASTER_VOLUME, 0x0000);
        arch::outw(nam + NAM_PCM_VOLUME, 0x0000);
    }
    if !report.codec_ready {
        return report;
    }
    let pages = 1 + (BDL_ENTRIES * BUFFER_BYTES) as u64 / PAGE_SIZE;
    let Some(block) = frames.allocate_contiguous(pages, 1) else {
        return report;
    };
    let base = block.address();
    if base + pages * PAGE_SIZE > u32::MAX as u64 {
        return report;
    }
    unsafe { core::ptr::write_bytes(base as usize as *mut u8, 0, (pages * PAGE_SIZE) as usize) };
    let mut state = Ac97State::EMPTY;
    state.nam = nam;
    state.nabm = nabm;
    state.bdl = base;
    state.buffers = base + PAGE_SIZE;
    for index in 0..BDL_ENTRIES {
        let entry = (base as usize + index * 8) as *mut u32;
        unsafe {
            core::ptr::write_volatile(
                entry,
                (state.buffers + (index * BUFFER_BYTES) as u64) as u32,
            );
            // Length in samples; bit 31 = interrupt on completion (unused).
            core::ptr::write_volatile(entry.add(1), SAMPLES_PER_BUFFER as u32);
        }
    }
    state.ready = true;
    *AC97.lock() = state;

    // Self-test: play a short tune and confirm the controller consumed
    // several buffers (its current-index register advanced).
    play_song(0);
    let start = crate::time::monotonic_nanoseconds();
    let mut civ_max = 0u32;
    while crate::time::monotonic_nanoseconds().saturating_sub(start) < 400_000_000 {
        pump();
        let nabm = AC97.lock().nabm;
        civ_max = civ_max.max(unsafe { arch::inb(nabm + NABM_PO_CIV) } as u32);
    }
    stop();
    report.buffers_played = civ_max;
    report.verified = report.codec_ready && civ_max >= 3;
    report
}
