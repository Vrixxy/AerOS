//! Intel High Definition Audio: immediate-command verb transport, a small codec walk
//! that finds a DAC wired to an output pin, and one output stream fed from a
//! ring of buffers by the shared tone synthesiser (see `ac97::Player`).

use crate::ac97::Player;
use crate::memory::FrameAllocator;
use crate::pci::PciInventory;
use crate::sync::TicketLock;

const PAGE_SIZE: u64 = 4096;
const BDL_ENTRIES: usize = 8;
const SAMPLES_PER_BUFFER: usize = 8192;
const BUFFER_BYTES: usize = SAMPLES_PER_BUFFER * 2;
const TIMEOUT: usize = 4_000_000;

const GCAP: usize = 0x00;
const GCTL: usize = 0x08;
const STATESTS: usize = 0x0e;
const STREAM_BASE: usize = 0x80;
const STREAM_STRIDE: usize = 0x20;

#[derive(Clone, Copy)]
pub struct HdaReport {
    pub present: bool,
    pub base: u64,
    pub codecs: u32,
    pub dac: u32,
    pub pin: u32,
    pub path_found: bool,
    pub bytes_played: u32,
    pub verified: bool,
}

impl HdaReport {
    pub const EMPTY: Self = Self {
        present: false,
        base: 0,
        codecs: 0,
        dac: 0,
        pin: 0,
        path_found: false,
        bytes_played: 0,
        verified: false,
    };
}

struct HdaState {
    base: u64,
    codec: u32,
    stream: usize,
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

impl HdaState {
    const EMPTY: Self = Self {
        base: 0,
        codec: 0,
        stream: 0,
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

static HDA: TicketLock<HdaState> = TicketLock::new(HdaState::EMPTY);

unsafe fn r8(base: u64, offset: usize) -> u8 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u8) }
}
unsafe fn r16(base: u64, offset: usize) -> u16 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u16) }
}
unsafe fn r32(base: u64, offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u32) }
}
unsafe fn w8(base: u64, offset: usize, value: u8) {
    unsafe { core::ptr::write_volatile((base as usize + offset) as *mut u8, value) }
}
unsafe fn w16(base: u64, offset: usize, value: u16) {
    unsafe { core::ptr::write_volatile((base as usize + offset) as *mut u16, value) }
}
unsafe fn w32(base: u64, offset: usize, value: u32) {
    unsafe { core::ptr::write_volatile((base as usize + offset) as *mut u32, value) }
}

fn wait_for(mut condition: impl FnMut() -> bool) -> bool {
    for _ in 0..TIMEOUT {
        if condition() {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

impl HdaState {
    /// Sends one verb and returns the codec's 32-bit response.
    fn verb(&mut self, node: u32, verb: u32, payload: u32) -> Option<u32> {
        let command = self.codec << 28 | node << 20 | verb << 8 | payload;
        // 4-bit verbs (set format, set amp gain) carry a 16-bit payload.
        self.send(command)
    }

    fn verb16(&mut self, node: u32, verb: u32, payload: u32) -> Option<u32> {
        self.send(self.codec << 28 | node << 20 | verb << 16 | payload)
    }

    /// Sends one command through the immediate-command registers (no CORB /
    /// RIRB DMA needed) and returns the codec's response.
    fn send(&mut self, command: u32) -> Option<u32> {
        let base = self.base;
        const ICO: usize = 0x60;
        const ICI: usize = 0x64;
        const IRS: usize = 0x68;
        if !wait_for(|| unsafe { r16(base, IRS) } & 1 == 0) {
            return None;
        }
        unsafe {
            w16(base, IRS, 2); // clear "response valid"
            w32(base, ICO, command);
            w16(base, IRS, 1); // go
        }
        if !wait_for(|| unsafe { r16(base, IRS) } & 3 == 2) {
            return None;
        }
        Some(unsafe { r32(base, ICI) })
    }

    fn parameter(&mut self, node: u32, parameter: u32) -> u32 {
        self.verb(node, 0xf00, parameter).unwrap_or(0)
    }

    fn connections(&mut self, node: u32, out: &mut [u32; 8]) -> usize {
        let length_info = self.verb(node, 0xf00, 0x0e).unwrap_or(0);
        let long_form = length_info & 0x80 != 0;
        let count = (length_info & 0x7f).min(8) as usize;
        let step = if long_form { 2 } else { 4 };
        let mut found = 0;
        let mut index = 0;
        while found < count {
            let response = self.verb(node, 0xf02, index).unwrap_or(0);
            for slot in 0..step {
                if found >= count {
                    break;
                }
                let value = if long_form {
                    (response >> (slot * 16)) & 0xffff
                } else {
                    (response >> (slot * 8)) & 0xff
                };
                out[found] = value;
                found += 1;
            }
            index += step as u32;
        }
        count
    }

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
        let (base, sd) = (self.base, self.stream_register());
        unsafe { w8(base, sd, 0) };
        self.stream_on = false;
    }

    fn stream_register(&self) -> usize {
        STREAM_BASE + self.stream * STREAM_STRIDE
    }

    fn start_stream(&mut self) {
        let base = self.base;
        let sd = self.stream_register();
        unsafe {
            // Reset the stream descriptor, then program it.
            w8(base, sd, 1);
            wait_for(|| r8(base, sd) & 1 != 0);
            w8(base, sd, 0);
            wait_for(|| r8(base, sd) & 1 == 0);
        }
        for index in 0..BDL_ENTRIES {
            self.fill(index);
        }
        self.next_fill = 0;
        unsafe {
            w32(base, sd + 0x18, self.bdl as u32);
            w32(base, sd + 0x1c, (self.bdl >> 32) as u32);
            w32(base, sd + 0x08, (BDL_ENTRIES * BUFFER_BYTES) as u32);
            w16(base, sd + 0x0c, BDL_ENTRIES as u16 - 1);
            // 48 kHz, 16-bit, stereo.
            w16(base, sd + 0x12, 0x0011);
            // Stream tag 1 (bits 23:20 of the control word), then run.
            w8(base, sd + 0x02, 1 << 4);
            w8(base, sd, 0x02);
        }
    }

    /// Position (in ring buffers) the controller is currently playing.
    fn playing_buffer(&self) -> usize {
        let position = unsafe { r32(self.base, self.stream_register() + 0x04) } as usize;
        (position / BUFFER_BYTES) % BDL_ENTRIES
    }

    fn pump(&mut self) {
        if !self.ready || !self.stream_on {
            return;
        }
        if !self.playing && self.quiet as usize > BDL_ENTRIES + 4 {
            self.stop_stream();
            return;
        }
        // Refill every buffer the controller has finished with.
        let current = self.playing_buffer();
        while self.next_fill != current {
            let index = self.next_fill;
            self.fill(index);
            self.next_fill = (index + 1) % BDL_ENTRIES;
        }
    }
}

pub fn pump() {
    HDA.lock().pump();
}

pub fn play_song(song: usize) {
    let mut state = HDA.lock();
    if !state.ready {
        return;
    }
    state.player = Player::new(song);
    state.playing = true;
    state.stream_on = true;
    state.start_stream();
}

pub fn play_effect(effect: crate::sfx::Effect) {
    let mut state = HDA.lock();
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

pub fn stop() {
    let mut state = HDA.lock();
    if !state.ready {
        return;
    }
    state.playing = false;
    if !crate::sfx::active() {
        state.stop_stream();
    }
}

pub fn resume() {
    let mut state = HDA.lock();
    if !state.ready || state.playing {
        return;
    }
    state.playing = true;
    state.stream_on = true;
    state.start_stream();
}

pub fn set_volume(percent: u8) {
    HDA.lock().volume = percent.min(100) as i32;
}

pub fn ready() -> bool {
    HDA.lock().ready
}

pub fn initialize(pci: &PciInventory, frames: &mut FrameAllocator) -> HdaReport {
    let Some(device) = pci.find_class(0x04, 0x03, 0x00) else {
        return HdaReport::EMPTY;
    };
    if device.bars[0] & 1 != 0 || !pci.enable_memory_bus_master(device) {
        return HdaReport::EMPTY;
    }
    let mut base = (device.bars[0] & 0xffff_fff0) as u64;
    if device.bars[0] & 0x6 == 0x4 {
        base |= (device.bars[1] as u64) << 32;
    }
    if base == 0 {
        return HdaReport::EMPTY;
    }
    let mut report = HdaReport {
        present: true,
        base,
        ..HdaReport::EMPTY
    };
    let mut state = HdaState::EMPTY;
    state.base = base;

    // Controller reset.
    unsafe {
        w32(base, GCTL, 0);
        wait_for(|| r32(base, GCTL) & 1 == 0);
        w32(base, GCTL, 1);
    }
    if !wait_for(|| unsafe { r32(base, GCTL) } & 1 == 1) {
        return report;
    }
    // Codecs announce themselves shortly after reset.
    let start = crate::time::monotonic_nanoseconds();
    while crate::time::monotonic_nanoseconds().saturating_sub(start) < 100_000_000 {
        core::hint::spin_loop();
    }
    let present = unsafe { r16(base, STATESTS) } & 0x7fff;
    report.codecs = present.count_ones();
    if present == 0 {
        return report;
    }
    state.codec = present.trailing_zeros();

    // One DMA block: the buffer descriptor list, then the audio buffers.
    let buffer_pages = (BDL_ENTRIES * BUFFER_BYTES) as u64 / PAGE_SIZE;
    let Some(dma) = frames.allocate_contiguous(1 + buffer_pages, 1) else {
        return report;
    };
    let memory = dma.address();
    unsafe {
        core::ptr::write_bytes(
            memory as usize as *mut u8,
            0,
            ((1 + buffer_pages) * PAGE_SIZE) as usize,
        )
    };
    state.bdl = memory;
    state.buffers = memory + PAGE_SIZE;
    for index in 0..BDL_ENTRIES {
        let entry = (state.bdl as usize + index * 16) as *mut u64;
        unsafe {
            core::ptr::write_volatile(entry, state.buffers + (index * BUFFER_BYTES) as u64);
            core::ptr::write_volatile(entry.add(1), BUFFER_BYTES as u64);
        }
    }
    // Walk the codec: find an audio-output converter and the output pin
    // that connects to it (directly or through one mixer/selector).
    let root_children = state.parameter(0, 0x04);
    let (function_start, function_count) = ((root_children >> 16) & 0xff, root_children & 0xff);
    let (mut dac, mut pin) = (None, None);
    'search: for function in function_start..function_start + function_count {
        let widgets = state.parameter(function, 0x04);
        let (first, count) = ((widgets >> 16) & 0xff, widgets & 0xff);
        let mut pins = [0u32; 16];
        let mut pin_count = 0;
        for node in first..first + count {
            let caps = state.parameter(node, 0x09);
            let kind = (caps >> 20) & 0xf;
            if kind == 0 && dac.is_none() {
                dac = Some(node);
            }
            if kind == 4 && pin_count < pins.len() {
                let pin_caps = state.parameter(node, 0x0c);
                let config = state.verb(node, 0xf1c, 0).unwrap_or(0);
                let connectivity = (config >> 30) & 3;
                // Output-capable pin that is physically connected.
                if pin_caps & (1 << 4) != 0 && connectivity != 1 {
                    pins[pin_count] = node;
                    pin_count += 1;
                }
            }
        }
        let Some(converter) = dac else {
            continue;
        };
        for &candidate in &pins[..pin_count] {
            let mut list = [0u32; 8];
            let count = state.connections(candidate, &mut list);
            for &source in &list[..count] {
                if source == converter {
                    pin = Some((candidate, converter, None));
                    break 'search;
                }
                let mut second = [0u32; 8];
                let mid_count = state.connections(source, &mut second);
                if second[..mid_count].contains(&converter) {
                    pin = Some((candidate, converter, Some(source)));
                    break 'search;
                }
            }
        }
    }
    let Some((pin_node, dac_node, middle)) = pin else {
        return report;
    };
    report.dac = dac_node;
    report.pin = pin_node;
    report.path_found = true;

    // Power up and unmute the path, route the stream to the converter.
    for node in [Some(pin_node), middle, Some(dac_node)]
        .into_iter()
        .flatten()
    {
        let _ = state.verb(node, 0x705, 0);
        // Output amp, both channels, unmuted, gain 0x2f.
        let _ = state.verb16(node, 0x3, 0xb02f);
        // Input amp too (mixers/selectors), index 0 and 1.
        let _ = state.verb16(node, 0x3, 0x7020);
        let _ = state.verb16(node, 0x3, 0x7121);
    }
    if let Some(mid) = middle {
        // Select the converter as the mixer's input when it is a selector.
        let mut list = [0u32; 8];
        let count = state.connections(mid, &mut list);
        if let Some(index) = list[..count].iter().position(|node| *node == dac_node) {
            let _ = state.verb(mid, 0x701, index as u32);
        }
    }
    let mut list = [0u32; 8];
    let count = state.connections(pin_node, &mut list);
    let source = middle.unwrap_or(dac_node);
    if let Some(index) = list[..count].iter().position(|node| *node == source) {
        let _ = state.verb(pin_node, 0x701, index as u32);
    }
    let _ = state.verb(pin_node, 0x707, 0x40); // pin output enable
    let _ = state.verb(pin_node, 0x70c, 2); // EAPD on
    let _ = state.verb(dac_node, 0x706, 1 << 4); // stream tag 1, channel 0
    let _ = state.verb16(dac_node, 0x2, 0x0011); // 48 kHz, 16-bit, stereo

    // First output stream descriptor follows the input ones.
    let capabilities = unsafe { r16(base, GCAP) } as usize;
    let inputs = (capabilities >> 8) & 0xf;
    let outputs = (capabilities >> 12) & 0xf;
    if outputs == 0 {
        return report;
    }
    state.stream = inputs;
    state.ready = true;
    *HDA.lock() = state;

    // Self-test: play a tune and confirm the stream position advanced.
    play_song(0);
    let start = crate::time::monotonic_nanoseconds();
    let mut position_max = 0u32;
    while crate::time::monotonic_nanoseconds().saturating_sub(start) < 400_000_000 {
        pump();
        let state = HDA.lock();
        position_max = position_max.max(unsafe { r32(state.base, state.stream_register() + 0x04) });
    }
    stop();
    report.bytes_played = position_max;
    report.verified = report.path_found && position_max > BUFFER_BYTES as u32;
    report
}
