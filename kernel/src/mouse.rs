use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, AtomicU64, Ordering};

use crate::{acpi::AcpiInfo, arch, ioapic};

const DATA: u16 = 0x60;
const STATUS: u16 = 0x64;
const COMMAND: u16 = 0x64;
const VECTOR: u8 = 53;

const VMMOUSE_MAGIC: u32 = 0x564d_5868;
const VMMOUSE_PORT: u16 = 0x5658;
const VMMOUSE_GETVERSION: u32 = 10;
const VMMOUSE_DATA: u32 = 39;
const VMMOUSE_STATUS: u32 = 40;
const VMMOUSE_COMMAND: u32 = 41;
const VMMOUSE_CMD_ENABLE: u32 = 0x4541_4552;
const VMMOUSE_CMD_REQUEST_ABSOLUTE: u32 = 0x5342_4152;
const VMMOUSE_VERSION: u32 = 0x3442_554a;
const VMMOUSE_ERROR: u32 = 0xffff_0000;
const VMMOUSE_RELATIVE: u32 = 0x0001_0000;
const VMMOUSE_LEFT: u32 = 0x20;
const VMMOUSE_RIGHT: u32 = 0x10;
const VMMOUSE_MIDDLE: u32 = 0x08;

static ABSOLUTE: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);
static POS_X: AtomicI32 = AtomicI32::new(900);
static POS_Y: AtomicI32 = AtomicI32::new(250);
static BUTTONS: AtomicU8 = AtomicU8::new(0);
static BOUNDS: AtomicU32 = AtomicU32::new((1280 << 16) | 800);
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// Where each recent left-button press happened, so a tap that begins and
/// ends between two frames is still seen (a fingertip is often that short).
const PRESS_SLOTS: usize = 8;
static PRESS_X: [AtomicI32; PRESS_SLOTS] = [const { AtomicI32::new(0) }; PRESS_SLOTS];
static PRESS_Y: [AtomicI32; PRESS_SLOTS] = [const { AtomicI32::new(0) }; PRESS_SLOTS];
static PRESS_WRITE: AtomicU32 = AtomicU32::new(0);
static PRESS_READ: AtomicU32 = AtomicU32::new(0);
static PACKETS: AtomicU64 = AtomicU64::new(0);
/// Wheel movement not yet consumed (positive = scroll down), accumulated
/// from the absolute (vmmouse) packets' z field.
static WHEEL: AtomicI32 = AtomicI32::new(0);

static mut PACKET: [u8; 4] = [0; 4];
static PACKET_INDEX: AtomicU8 = AtomicU8::new(0);

#[derive(Clone, Copy)]
pub struct MouseReport {
    pub present: bool,
    pub routed: bool,
    pub reporting: bool,
    pub absolute: bool,
    pub verified: bool,
}

#[derive(Clone, Copy)]
pub struct MouseState {
    pub x: i32,
    pub y: i32,
    pub left: bool,
    pub right: bool,
    pub middle: bool,
    pub generation: u64,
}

pub fn initialize(acpi: &AcpiInfo, destination: u32) -> MouseReport {
    READY.store(false, Ordering::Release);
    PACKET_INDEX.store(0, Ordering::Release);
    let present = configure_controller();
    let routed = ioapic::route_legacy_irq(acpi, 12, destination, VECTOR);
    let reporting = present && enable_reporting();
    let absolute = reporting && enable_vmmouse();
    ABSOLUTE.store(absolute, Ordering::Release);
    let verified = present && routed && reporting;
    READY.store(verified, Ordering::Release);
    MouseReport {
        present,
        routed,
        reporting,
        absolute,
        verified,
    }
}

pub fn set_bounds(width: usize, height: usize) {
    if width == 0 || height == 0 {
        return;
    }
    let packed = ((width as u32) << 16) | (height as u32 & 0xffff);
    BOUNDS.store(packed, Ordering::Release);
    let (max_x, max_y) = (width as i32 - 1, height as i32 - 1);
    POS_X.store(
        POS_X.load(Ordering::Relaxed).clamp(0, max_x),
        Ordering::Relaxed,
    );
    POS_Y.store(
        POS_Y.load(Ordering::Relaxed).clamp(0, max_y),
        Ordering::Relaxed,
    );
}

pub fn state() -> MouseState {
    let buttons = BUTTONS.load(Ordering::Acquire);
    MouseState {
        x: POS_X.load(Ordering::Acquire),
        y: POS_Y.load(Ordering::Acquire),
        left: buttons & 1 != 0,
        right: buttons & 2 != 0,
        middle: buttons & 4 != 0,
        generation: GENERATION.load(Ordering::Acquire),
    }
}

/// Returns (and clears) the wheel movement accumulated since the last call.
pub fn take_wheel() -> i32 {
    WHEEL.swap(0, Ordering::AcqRel)
}

fn record_press(x: i32, y: i32) {
    let write = PRESS_WRITE.load(Ordering::Relaxed);
    let slot = write as usize % PRESS_SLOTS;
    PRESS_X[slot].store(x, Ordering::Relaxed);
    PRESS_Y[slot].store(y, Ordering::Relaxed);
    PRESS_WRITE.store(write.wrapping_add(1), Ordering::Release);
}

/// The next left-button press since the last call (oldest first), as the
/// pointer position where it happened.
pub fn take_press() -> Option<(i32, i32)> {
    let write = PRESS_WRITE.load(Ordering::Acquire);
    let mut read = PRESS_READ.load(Ordering::Relaxed);
    if write.wrapping_sub(read) as usize > PRESS_SLOTS {
        read = write.wrapping_sub(PRESS_SLOTS as u32);
    }
    if read == write {
        return None;
    }
    let slot = read as usize % PRESS_SLOTS;
    let point = (
        PRESS_X[slot].load(Ordering::Relaxed),
        PRESS_Y[slot].load(Ordering::Relaxed),
    );
    PRESS_READ.store(read.wrapping_add(1), Ordering::Relaxed);
    Some(point)
}

pub fn ready() -> bool {
    READY.load(Ordering::Acquire)
}

pub fn handle_interrupt() {
    for _ in 0..64 {
        let status = unsafe { arch::inb(STATUS) };
        if status & 1 == 0 {
            return;
        }
        let byte = unsafe { arch::inb(DATA) };
        if status & 0x20 != 0 {
            feed_byte(byte);
        } else {
            crate::keyboard::push_scancode(byte);
        }
    }
}

pub fn feed_byte(byte: u8) {
    if ABSOLUTE.load(Ordering::Acquire) {
        drain_vmmouse();
        return;
    }
    let index = PACKET_INDEX.load(Ordering::Relaxed) as usize;
    if index == 0 && byte & 0x08 == 0 {
        return;
    }
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(PACKET[index]), byte);
    }
    if index < 2 {
        PACKET_INDEX.store((index + 1) as u8, Ordering::Release);
        return;
    }
    PACKET_INDEX.store(0, Ordering::Release);
    let flags = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(PACKET[0])) };
    let raw_x = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(PACKET[1])) };
    let raw_y = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(PACKET[2])) };
    if flags & 0xc0 != 0 {
        return;
    }
    let delta_x = sign_extend(raw_x, flags & 0x10 != 0);
    let delta_y = sign_extend(raw_y, flags & 0x20 != 0);
    // PS/2 reports "up" as positive; the pointer math below wants "down".
    inject_motion(flags & 0x07, delta_x, -delta_y, 0);
}

/// One relative pointer report (PS/2 or USB HID): `buttons` bit 0 = left,
/// 1 = right, 2 = middle; `dy_down` positive moves the pointer down; a
/// positive `wheel` is a scroll away from the user (up).
pub fn inject_motion(buttons: u8, delta_x: i32, dy_down: i32, wheel: i32) {
    let pressed = buttons & 0x01 != 0 && BUTTONS.load(Ordering::Relaxed) & 1 == 0;
    BUTTONS.store(buttons & 0x07, Ordering::Release);
    PACKETS.fetch_add(1, Ordering::Relaxed);
    if wheel != 0 {
        WHEEL.fetch_add(-wheel, Ordering::AcqRel);
    }
    if delta_x == 0 && dy_down == 0 {
        if pressed {
            record_press(POS_X.load(Ordering::Relaxed), POS_Y.load(Ordering::Relaxed));
        }
        return;
    }
    let packed = BOUNDS.load(Ordering::Acquire);
    let max_x = ((packed >> 16) as i32 - 1).max(0);
    let max_y = ((packed & 0xffff) as i32 - 1).max(0);
    let next_x = (POS_X.load(Ordering::Relaxed) + delta_x).clamp(0, max_x);
    let next_y = (POS_Y.load(Ordering::Relaxed) + dy_down).clamp(0, max_y);
    POS_X.store(next_x, Ordering::Release);
    POS_Y.store(next_y, Ordering::Release);
    if pressed {
        record_press(next_x, next_y);
    }
    GENERATION.fetch_add(1, Ordering::Release);
}

/// One absolute pointer report (USB tablet): `x`/`y` are 0..=0xffff across the
/// screen; buttons and wheel as in `inject_motion`.
pub fn inject_absolute(buttons: u8, x: u32, y: u32, wheel: i32) {
    let pressed = buttons & 0x01 != 0 && BUTTONS.load(Ordering::Relaxed) & 1 == 0;
    BUTTONS.store(buttons & 0x07, Ordering::Release);
    PACKETS.fetch_add(1, Ordering::Relaxed);
    if wheel != 0 {
        WHEEL.fetch_add(-wheel, Ordering::AcqRel);
    }
    let packed = BOUNDS.load(Ordering::Acquire);
    let max_x = ((packed >> 16) as i32 - 1).max(0);
    let max_y = ((packed & 0xffff) as i32 - 1).max(0);
    let next_x = (x.min(0xffff) as u64 * max_x as u64 / 0xffff) as i32;
    let next_y = (y.min(0xffff) as u64 * max_y as u64 / 0xffff) as i32;
    POS_X.store(next_x, Ordering::Release);
    POS_Y.store(next_y, Ordering::Release);
    if pressed {
        record_press(next_x, next_y);
    }
    GENERATION.fetch_add(1, Ordering::Release);
}

fn sign_extend(value: u8, negative: bool) -> i32 {
    if negative {
        value as i32 - 256
    } else {
        value as i32
    }
}

fn vmmouse_backdoor(command: u32, argument: u32) -> (u32, u32, u32, u32) {
    let mut eax: u32 = VMMOUSE_MAGIC;
    let mut ebx: u64 = argument as u64;
    let mut ecx: u32 = command;
    let mut edx: u32 = VMMOUSE_PORT as u32;
    unsafe {
        core::arch::asm!(
            "xchg rbx, {bx}",
            "in eax, dx",
            "xchg rbx, {bx}",
            bx = inout(reg) ebx,
            inout("eax") eax,
            inout("ecx") ecx,
            inout("edx") edx,
            options(nostack),
        );
    }
    (eax, ebx as u32, ecx, edx)
}

fn enable_vmmouse() -> bool {
    let (version, magic, _, _) = vmmouse_backdoor(VMMOUSE_GETVERSION, 0);
    if magic != VMMOUSE_MAGIC || version == 0xffff_ffff {
        return false;
    }
    vmmouse_backdoor(VMMOUSE_COMMAND, VMMOUSE_CMD_ENABLE);
    let (status, _, _, _) = vmmouse_backdoor(VMMOUSE_STATUS, 0);
    if status & 0xffff == 0 || status & VMMOUSE_ERROR == VMMOUSE_ERROR {
        return false;
    }
    let (data_version, _, _, _) = vmmouse_backdoor(VMMOUSE_DATA, 1);
    if data_version != VMMOUSE_VERSION {
        return false;
    }
    vmmouse_backdoor(VMMOUSE_COMMAND, VMMOUSE_CMD_REQUEST_ABSOLUTE);
    true
}

fn drain_vmmouse() {
    for _ in 0..64 {
        let (status, _, _, _) = vmmouse_backdoor(VMMOUSE_STATUS, 0);
        if status & VMMOUSE_ERROR == VMMOUSE_ERROR {
            ABSOLUTE.store(enable_vmmouse(), Ordering::Release);
            return;
        }
        if status & 0xffff < 4 {
            return;
        }
        let (flags, x, y, z) = vmmouse_backdoor(VMMOUSE_DATA, 4);
        if z != 0 {
            WHEEL.fetch_add((z as i32).clamp(-8, 8), Ordering::AcqRel);
        }
        apply_vmmouse_packet(flags, x, y);
    }
}

fn apply_vmmouse_packet(flags: u32, x: u32, y: u32) {
    let mut buttons = 0u8;
    if flags & VMMOUSE_LEFT != 0 {
        buttons |= 1;
    }
    if flags & VMMOUSE_RIGHT != 0 {
        buttons |= 2;
    }
    if flags & VMMOUSE_MIDDLE != 0 {
        buttons |= 4;
    }
    let pressed = buttons & 1 != 0 && BUTTONS.load(Ordering::Relaxed) & 1 == 0;
    BUTTONS.store(buttons, Ordering::Release);

    let packed = BOUNDS.load(Ordering::Acquire);
    let max_x = ((packed >> 16) as i32 - 1).max(0);
    let max_y = ((packed & 0xffff) as i32 - 1).max(0);
    let (next_x, next_y) = if flags & VMMOUSE_RELATIVE != 0 {
        (
            POS_X.load(Ordering::Relaxed) + x as i32,
            POS_Y.load(Ordering::Relaxed) - y as i32,
        )
    } else {
        (
            (x as u64 * max_x as u64 / 0xffff) as i32,
            (y as u64 * max_y as u64 / 0xffff) as i32,
        )
    };
    POS_X.store(next_x.clamp(0, max_x), Ordering::Release);
    POS_Y.store(next_y.clamp(0, max_y), Ordering::Release);
    if pressed {
        record_press(next_x.clamp(0, max_x), next_y.clamp(0, max_y));
    }
    PACKETS.fetch_add(1, Ordering::Relaxed);
    GENERATION.fetch_add(1, Ordering::Release);
}

fn configure_controller() -> bool {
    if !wait_write() {
        return false;
    }
    unsafe {
        arch::outb(COMMAND, 0xa8);
    }
    if !wait_write() {
        return false;
    }
    unsafe {
        arch::outb(COMMAND, 0x20);
    }
    if !wait_read() {
        return false;
    }
    let configuration = (unsafe { arch::inb(DATA) } | 0x02) & !0x20;
    if !wait_write() {
        return false;
    }
    unsafe {
        arch::outb(COMMAND, 0x60);
    }
    if !wait_write() {
        return false;
    }
    unsafe {
        arch::outb(DATA, configuration);
    }
    true
}

fn enable_reporting() -> bool {
    aux_command(0xf6) && aux_command(0xf4)
}

fn aux_command(command: u8) -> bool {
    for _ in 0..3 {
        if !wait_write() {
            return false;
        }
        unsafe {
            arch::outb(COMMAND, 0xd4);
        }
        if !wait_write() {
            return false;
        }
        unsafe {
            arch::outb(DATA, command);
        }
        if !wait_read() {
            return false;
        }
        match unsafe { arch::inb(DATA) } {
            0xfa => return true,
            0xfe => continue,
            _ => return false,
        }
    }
    false
}

fn wait_write() -> bool {
    for _ in 0..200_000 {
        if unsafe { arch::inb(STATUS) } & 2 == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

fn wait_read() -> bool {
    for _ in 0..2_000_000 {
        if unsafe { arch::inb(STATUS) } & 1 != 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}
