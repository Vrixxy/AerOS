//! xHCI (USB 3) host controller driver, polled. Enumerates the root ports and
//! everything behind USB hubs, and drives HID keyboards, HID pointers (boot
//! mice and report-descriptor devices such as USB tablets) and bulk-only
//! mass-storage disks. Keyboard reports become PS/2 scancodes, pointer
//! reports become pointer motion, and the disk is exposed through
//! `storage_read`/`storage_write`.

#![allow(clippy::collapsible_if)]

use core::hint::spin_loop;

use crate::memory::FrameAllocator;
use crate::pci::PciInventory;
use crate::sync::TicketLock;

const PAGE_SIZE: u64 = 4096;
const MAX_DEVICES: usize = 10;
const FUNCS: usize = 3;
/// DMA pages per device: output context, control ring, three endpoint rings,
/// a scratch/report page and a 4 KiB data page.
const PAGES_PER_DEVICE: u64 = 7;
const RING_TRBS: usize = 64;
const EVENT_TRBS: usize = 256;
const TIMEOUT: usize = 30_000_000;
/// Commands and transfers get this long (real time) before they count as lost.
const TRANSFER_TIMEOUT_NS: u64 = 3_000_000_000;
const MAX_INTERFACES: usize = 6;

// Offsets inside a device's scratch page.
const REPORT_AREA: u64 = 0; // FUNCS * 64 bytes of HID reports
const CBW_AREA: u64 = 256;
const CSW_AREA: u64 = 320;
const SMALL_AREA: u64 = 384; // 64 bytes of control scratch
const DESCRIPTOR_AREA: u64 = 1024; // 3 KiB for descriptors
const DESCRIPTOR_MAX: u16 = 1024;

// TRB types.
const TRB_NORMAL: u32 = 1;
const TRB_SETUP: u32 = 2;
const TRB_DATA: u32 = 3;
const TRB_STATUS: u32 = 4;
const TRB_LINK: u32 = 6;
const TRB_ENABLE_SLOT: u32 = 9;
const TRB_DISABLE_SLOT: u32 = 10;
const TRB_ADDRESS_DEVICE: u32 = 11;
const TRB_CONFIGURE_ENDPOINT: u32 = 12;
const TRB_EVALUATE_CONTEXT: u32 = 13;
const TRB_RESET_ENDPOINT: u32 = 14;
const TRB_SET_DEQUEUE: u32 = 16;
const TRB_TRANSFER_EVENT: u32 = 32;
const TRB_COMMAND_COMPLETION: u32 = 33;
const TRB_PORT_STATUS_CHANGE: u32 = 34;

const CC_SUCCESS: u32 = 1;
const CC_SHORT_PACKET: u32 = 13;

const SPEED_FULL: u32 = 1;
const SPEED_LOW: u32 = 2;
const SPEED_HIGH: u32 = 3;
const SPEED_SUPER: u32 = 4;

#[derive(Clone, Copy)]
pub struct XhciReport {
    pub present: bool,
    pub base: u64,
    pub version: u16,
    pub ports: u32,
    pub slots: u32,
    pub devices: u32,
    pub keyboards: u32,
    pub mice: u32,
    pub tablets: u32,
    pub hubs: u32,
    pub disks: u32,
    pub disk_sectors: u64,
    pub disk_read: bool,
    pub disk_write: bool,
    pub verified: bool,
}

impl XhciReport {
    pub const EMPTY: Self = Self {
        present: false,
        base: 0,
        version: 0,
        ports: 0,
        slots: 0,
        devices: 0,
        keyboards: 0,
        mice: 0,
        tablets: 0,
        hubs: 0,
        disks: 0,
        disk_sectors: 0,
        disk_read: false,
        disk_write: false,
        verified: false,
    };
}

#[derive(Clone, Copy)]
struct Ring {
    base: u64,
    enqueue: usize,
    cycle: bool,
}

impl Ring {
    const EMPTY: Self = Self {
        base: 0,
        enqueue: 0,
        cycle: true,
    };

    fn at(base: u64) -> Self {
        Self {
            base,
            enqueue: 0,
            cycle: true,
        }
    }

    /// Queues one TRB (cycle bit filled in) and returns its address.
    fn push(&mut self, mut trb: [u32; 4]) -> u64 {
        trb[3] = (trb[3] & !1) | self.cycle as u32;
        let address = self.base + (self.enqueue * 16) as u64;
        write_trb(address, trb);
        self.enqueue += 1;
        if self.enqueue == RING_TRBS - 1 {
            // Link TRB back to the start, toggling the cycle bit.
            let link = [
                self.base as u32,
                (self.base >> 32) as u32,
                0,
                TRB_LINK << 10 | 1 << 1 | self.cycle as u32,
            ];
            write_trb(self.base + ((RING_TRBS - 1) * 16) as u64, link);
            self.enqueue = 0;
            self.cycle = !self.cycle;
        }
        address
    }

    /// Where the controller should resume after a halted transfer: the next
    /// TRB to be written, with the current cycle state.
    fn dequeue_pointer(&self) -> u64 {
        (self.base + (self.enqueue * 16) as u64) | self.cycle as u64
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    None,
    Keyboard,
    /// Boot-protocol mouse.
    Mouse,
    /// Pointer described by a HID report descriptor (tablets, other mice).
    Pointer,
    /// A hub's status-change endpoint (its report says which ports changed).
    Hub,
}

/// One bit field of a HID input report.
#[derive(Clone, Copy)]
struct Field {
    present: bool,
    bit: u16,
    size: u8,
    signed: bool,
    relative: bool,
    min: i32,
    max: i32,
}

impl Field {
    const EMPTY: Self = Self {
        present: false,
        bit: 0,
        size: 0,
        signed: false,
        relative: false,
        min: 0,
        max: 0,
    };

    fn read(&self, report: &[u8]) -> i32 {
        let mut value: u32 = 0;
        for index in 0..self.size as usize {
            let bit = self.bit as usize + index;
            if report
                .get(bit / 8)
                .is_some_and(|byte| byte >> (bit % 8) & 1 != 0)
            {
                value |= 1 << index;
            }
        }
        if self.signed && self.size > 0 && self.size < 32 && value & (1 << (self.size - 1)) != 0 {
            value as i32 - (1i32 << self.size)
        } else {
            value as i32
        }
    }
}

/// Where the interesting fields sit in a pointer's input report.
#[derive(Clone, Copy)]
struct Pointer {
    report_id: u8,
    buttons_bit: u16,
    buttons: u8,
    x: Field,
    y: Field,
    wheel: Field,
}

impl Pointer {
    const EMPTY: Self = Self {
        report_id: 0,
        buttons_bit: 0,
        buttons: 0,
        x: Field::EMPTY,
        y: Field::EMPTY,
        wheel: Field::EMPTY,
    };
}

#[derive(Clone, Copy)]
struct Func {
    kind: Kind,
    dci: u8,
    ring: usize,
    report: u64,
    report_bytes: u32,
    previous: [u8; 8],
    pointer: Pointer,
}

impl Func {
    const EMPTY: Self = Self {
        kind: Kind::None,
        dci: 0,
        ring: 0,
        report: 0,
        report_bytes: 8,
        previous: [0; 8],
        pointer: Pointer::EMPTY,
    };
}

#[derive(Clone, Copy)]
struct Storage {
    interface: u8,
    in_dci: u8,
    out_dci: u8,
    tag: u32,
    sectors: u64,
    block_bytes: u32,
}

impl Storage {
    const EMPTY: Self = Self {
        interface: 0,
        in_dci: 0,
        out_dci: 0,
        tag: 0,
        sectors: 0,
        block_bytes: 0,
    };
}

#[derive(Clone, Copy)]
struct Device {
    used: bool,
    slot: u8,
    speed: u8,
    root_port: u8,
    route: u32,
    /// Hubs above this device (a root-attached hub is depth 0).
    depth: u8,
    tt_slot: u8,
    tt_port: u8,
    /// The hub slot and port this device hangs off (0 for a root port).
    parent_slot: u8,
    parent_port: u8,
    control: Ring,
    rings: [Ring; FUNCS],
    scratch: u64,
    data: u64,
    funcs: [Func; FUNCS],
    hub_ports: u8,
    hub_mtt: bool,
    hub_think: u8,
    storage: Storage,
}

impl Device {
    const EMPTY: Self = Self {
        used: false,
        slot: 0,
        speed: 0,
        root_port: 0,
        route: 0,
        depth: 0,
        tt_slot: 0,
        tt_port: 0,
        parent_slot: 0,
        parent_port: 0,
        control: Ring::EMPTY,
        rings: [Ring::EMPTY; FUNCS],
        scratch: 0,
        data: 0,
        funcs: [Func::EMPTY; FUNCS],
        hub_ports: 0,
        hub_mtt: false,
        hub_think: 0,
        storage: Storage::EMPTY,
    };
}

struct XhciState {
    operational: u64,
    runtime: u64,
    doorbells: u64,
    context_bytes: usize,
    command: Ring,
    event_base: u64,
    event_dequeue: usize,
    event_cycle: bool,
    dcbaa: u64,
    input_context: u64,
    device_memory: u64,
    devices: [Device; MAX_DEVICES],
    ports: u32,
    /// Hub status-change reports waiting for `poll` to act on: (hub slot, port bitmap).
    hub_changes: [(u8, u16); 4],
    ready: bool,
}

impl XhciState {
    const EMPTY: Self = Self {
        operational: 0,
        runtime: 0,
        doorbells: 0,
        context_bytes: 32,
        command: Ring::EMPTY,
        event_base: 0,
        event_dequeue: 0,
        event_cycle: true,
        dcbaa: 0,
        input_context: 0,
        device_memory: 0,
        devices: [Device::EMPTY; MAX_DEVICES],
        ports: 0,
        hub_changes: [(0, 0); 4],
        ready: false,
    };
}

static XHCI: TicketLock<XhciState> = TicketLock::new(XhciState::EMPTY);

fn write_trb(address: u64, trb: [u32; 4]) {
    for (index, word) in trb.iter().enumerate() {
        unsafe { core::ptr::write_volatile((address as usize + index * 4) as *mut u32, *word) };
    }
}

fn read_trb(address: u64) -> [u32; 4] {
    let mut trb = [0u32; 4];
    for (index, word) in trb.iter_mut().enumerate() {
        *word = unsafe { core::ptr::read_volatile((address as usize + index * 4) as *const u32) };
    }
    trb
}

unsafe fn read32(base: u64, offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u32) }
}

unsafe fn write32(base: u64, offset: usize, value: u32) {
    unsafe { core::ptr::write_volatile((base as usize + offset) as *mut u32, value) }
}

unsafe fn write64(base: u64, offset: usize, value: u64) {
    unsafe {
        write32(base, offset, value as u32);
        write32(base, offset + 4, (value >> 32) as u32);
    }
}

fn read_byte(address: u64) -> u8 {
    unsafe { core::ptr::read_volatile(address as usize as *const u8) }
}

fn read_u16(address: u64) -> u16 {
    read_byte(address) as u16 | (read_byte(address + 1) as u16) << 8
}

fn read_u32(address: u64) -> u32 {
    read_u16(address) as u32 | (read_u16(address + 2) as u32) << 16
}

fn write_byte(address: u64, value: u8) {
    unsafe { core::ptr::write_volatile(address as usize as *mut u8, value) };
}

fn write_u32(address: u64, value: u32) {
    for index in 0..4 {
        write_byte(address + index, (value >> (index * 8)) as u8);
    }
}

fn wait_for(mut condition: impl FnMut() -> bool) -> bool {
    for _ in 0..TIMEOUT {
        if condition() {
            return true;
        }
        spin_loop();
    }
    false
}

fn delay_ms(milliseconds: u64) {
    let start = crate::time::monotonic_nanoseconds();
    while crate::time::monotonic_nanoseconds().saturating_sub(start) < milliseconds * 1_000_000 {
        spin_loop();
    }
}

fn zero(address: u64, bytes: usize) {
    unsafe { core::ptr::write_bytes(address as usize as *mut u8, 0, bytes) };
}

/// Standard GET_DESCRIPTOR setup packet (device to host).
fn get_descriptor(kind: u8, index: u8, length: u16) -> [u8; 8] {
    [
        0x80,
        0x06,
        index,
        kind,
        0,
        0,
        length as u8,
        (length >> 8) as u8,
    ]
}

impl XhciState {
    fn ring_doorbell(&self, slot: usize, target: u32) {
        unsafe { write32(self.doorbells, slot * 4, target) };
    }

    /// Pops one event from the event ring, if any.
    fn next_event(&mut self) -> Option<[u32; 4]> {
        let address = self.event_base + (self.event_dequeue * 16) as u64;
        let trb = read_trb(address);
        if (trb[3] & 1 != 0) != self.event_cycle {
            return None;
        }
        self.event_dequeue += 1;
        if self.event_dequeue == EVENT_TRBS {
            self.event_dequeue = 0;
            self.event_cycle = !self.event_cycle;
        }
        let pointer = self.event_base + (self.event_dequeue * 16) as u64;
        // Interrupter 0 event ring dequeue pointer; bit 3 clears "busy".
        unsafe { write64(self.runtime, 0x20 + 0x18, pointer | 8) };
        Some(trb)
    }

    /// Runs a command and returns (completion code, slot id).
    fn command(&mut self, trb: [u32; 4]) -> Option<(u32, u32)> {
        let address = self.command.push(trb);
        self.ring_doorbell(0, 0);
        let start = crate::time::monotonic_nanoseconds();
        while crate::time::monotonic_nanoseconds().saturating_sub(start) < TRANSFER_TIMEOUT_NS {
            if let Some(event) = self.next_event() {
                if event[3] >> 10 & 0x3f == TRB_COMMAND_COMPLETION
                    && (event[0] as u64 | (event[1] as u64) << 32) == address
                {
                    return Some((event[2] >> 24, event[3] >> 24));
                }
                self.dispatch_transfer(event);
            }
            spin_loop();
        }
        None
    }

    /// Waits for the transfer event of `slot`/`dci` (any TRB of the transfer
    /// that completed or failed); events for other endpoints keep flowing to
    /// their drivers meanwhile. Returns (completion code, residual bytes).
    fn wait_transfer(&mut self, slot: u8, dci: u8) -> Option<(u32, u32)> {
        let start = crate::time::monotonic_nanoseconds();
        while crate::time::monotonic_nanoseconds().saturating_sub(start) < TRANSFER_TIMEOUT_NS {
            if let Some(event) = self.next_event() {
                if event[3] >> 10 & 0x3f == TRB_TRANSFER_EVENT
                    && (event[3] >> 24) as u8 == slot
                    && ((event[3] >> 16) & 0x1f) as u8 == dci
                {
                    return Some((event[2] >> 24, event[2] & 0xff_ffff));
                }
                self.dispatch_transfer(event);
            }
            spin_loop();
        }
        None
    }

    /// Releases a controller slot (device removed or enumeration failed).
    fn disable_slot(&mut self, slot: u8) {
        let _ = self.command([0, 0, 0, TRB_DISABLE_SLOT << 10 | (slot as u32) << 24]);
        unsafe {
            core::ptr::write_volatile((self.dcbaa as usize + slot as usize * 8) as *mut u64, 0);
        }
    }

    /// Recovers a halted endpoint: reset it and move its dequeue pointer past
    /// the failed transfer.
    fn recover_endpoint(&mut self, slot: u8, dci: u8, dequeue: u64) {
        let slot32 = slot as u32;
        let _ = self.command([
            0,
            0,
            0,
            TRB_RESET_ENDPOINT << 10 | (dci as u32) << 16 | slot32 << 24,
        ]);
        let _ = self.command([
            dequeue as u32,
            (dequeue >> 32) as u32,
            0,
            TRB_SET_DEQUEUE << 10 | (dci as u32) << 16 | slot32 << 24,
        ]);
    }

    /// One control transfer on a device's default endpoint. `data` is a DMA
    /// address (0 for none), `length` its size, `input` the data direction.
    fn control(
        &mut self,
        device: usize,
        setup: [u8; 8],
        data: u64,
        length: u16,
        input: bool,
    ) -> bool {
        let slot = self.devices[device].slot;
        let ring = &mut self.devices[device].control;
        let transfer_type = match (length, input) {
            (0, _) => 0,
            (_, true) => 3,
            (_, false) => 2,
        };
        ring.push([
            u32::from_le_bytes([setup[0], setup[1], setup[2], setup[3]]),
            u32::from_le_bytes([setup[4], setup[5], setup[6], setup[7]]),
            8,
            TRB_SETUP << 10 | 1 << 6 | transfer_type << 16,
        ]);
        if length != 0 {
            ring.push([
                data as u32,
                (data >> 32) as u32,
                length as u32,
                TRB_DATA << 10 | (input as u32) << 16,
            ]);
        }
        let status_in = length == 0 || !input;
        ring.push([
            0,
            0,
            0,
            TRB_STATUS << 10 | 1 << 5 | (status_in as u32) << 16,
        ]);
        self.ring_doorbell(slot as usize, 1);
        match self.wait_transfer(slot, 1) {
            Some((code, _)) if code == CC_SUCCESS || code == CC_SHORT_PACKET => true,
            Some(_) => {
                let dequeue = self.devices[device].control.dequeue_pointer();
                self.recover_endpoint(slot, 1, dequeue);
                false
            }
            None => false,
        }
    }

    /// One bulk transfer of up to a page on the device's data page (or any
    /// DMA address). Returns the residual byte count on success.
    fn bulk(
        &mut self,
        device: usize,
        ring: usize,
        dci: u8,
        buffer: u64,
        length: u32,
    ) -> Option<u32> {
        let slot = self.devices[device].slot;
        let input = dci & 1 != 0;
        self.devices[device].rings[ring].push([
            buffer as u32,
            (buffer >> 32) as u32,
            length,
            TRB_NORMAL << 10 | 1 << 5 | (input as u32) << 2,
        ]);
        self.ring_doorbell(slot as usize, dci as u32);
        match self.wait_transfer(slot, dci) {
            Some((code, residual)) if code == CC_SUCCESS || code == CC_SHORT_PACKET => {
                Some(residual)
            }
            Some(_) => {
                let dequeue = self.devices[device].rings[ring].dequeue_pointer();
                self.recover_endpoint(slot, dci, dequeue);
                // Clear the device side of the halt as well.
                let endpoint = (dci >> 1) | if input { 0x80 } else { 0 };
                let _ = self.control(device, [0x02, 0x01, 0, 0, endpoint, 0, 0, 0], 0, 0, false);
                None
            }
            None => None,
        }
    }

    /// Routes one transfer event (from the poll loop or from inside another
    /// wait) to the HID function that owns the endpoint.
    fn dispatch_transfer(&mut self, event: [u32; 4]) {
        if event[3] >> 10 & 0x3f != TRB_TRANSFER_EVENT {
            return;
        }
        let slot = (event[3] >> 24) as u8;
        let endpoint = ((event[3] >> 16) & 0x1f) as u8;
        let code = event[2] >> 24;
        let mut found = None;
        for (device_index, device) in self.devices.iter().enumerate() {
            if !device.used || device.slot != slot {
                continue;
            }
            for (func_index, func) in device.funcs.iter().enumerate() {
                if func.dci == endpoint
                    && matches!(
                        func.kind,
                        Kind::Keyboard | Kind::Mouse | Kind::Pointer | Kind::Hub
                    )
                {
                    found = Some((device_index, func_index));
                }
            }
        }
        let Some((device_index, func_index)) = found else {
            return;
        };
        if code == CC_SUCCESS || code == CC_SHORT_PACKET {
            let func = self.devices[device_index].funcs[func_index];
            let mut data = [0u8; 64];
            for (offset, byte) in data.iter_mut().enumerate() {
                *byte = read_byte(func.report + offset as u64);
            }
            match func.kind {
                Kind::Keyboard => {
                    let mut report = [0u8; 8];
                    report.copy_from_slice(&data[..8]);
                    handle_keyboard(report, func.previous);
                    self.devices[device_index].funcs[func_index].previous = report;
                }
                Kind::Mouse => {
                    crate::mouse::inject_motion(
                        data[0] & 7,
                        data[1] as i8 as i32,
                        data[2] as i8 as i32,
                        data[3] as i8 as i32,
                    );
                }
                Kind::Pointer => handle_pointer(&func.pointer, &data),
                Kind::Hub => {
                    let bitmap = data[0] as u16 | (data[1] as u16) << 8;
                    self.note_hub_change(slot, bitmap);
                }
                _ => {}
            }
        } else {
            // A stalled/failed interrupt endpoint: reset it and carry on.
            let ring = self.devices[device_index].funcs[func_index].ring;
            let dequeue = self.devices[device_index].rings[ring].dequeue_pointer();
            self.recover_endpoint(slot, endpoint, dequeue);
        }
        self.queue_report(device_index, func_index);
        self.ring_doorbell(slot as usize, endpoint as u32);
    }

    /// Remembers which ports of a hub reported a change (merged per hub).
    fn note_hub_change(&mut self, slot: u8, bitmap: u16) {
        if bitmap == 0 {
            return;
        }
        for entry in self.hub_changes.iter_mut() {
            if entry.0 == slot {
                entry.1 |= bitmap;
                return;
            }
        }
        if let Some(entry) = self.hub_changes.iter_mut().find(|entry| entry.1 == 0) {
            *entry = (slot, bitmap);
        }
    }

    /// Queues a HID report read on a function's interrupt endpoint.
    fn queue_report(&mut self, device: usize, func: usize) {
        let function = self.devices[device].funcs[func];
        zero(function.report, 64);
        self.devices[device].rings[function.ring].push([
            function.report as u32,
            (function.report >> 32) as u32,
            function.report_bytes,
            TRB_NORMAL << 10 | 1 << 5 | 1 << 2,
        ]);
    }
}

/// Where a device's contexts live, for the input-context builders.
struct Contexts {
    input: u64,
    bytes: usize,
}

impl Contexts {
    fn dword(&self, offset: usize, value: u32) {
        write_u32(self.input + offset as u64, value);
    }

    /// Fills the slot context from what is known about the device.
    fn slot(&self, device: &Device, entries: u32) {
        let mut first = device.route & 0xf_ffff | (device.speed as u32) << 20 | entries << 27;
        if device.hub_ports != 0 {
            first |= 1 << 26;
            if device.hub_mtt {
                first |= 1 << 25;
            }
        }
        self.dword(self.bytes, first);
        self.dword(
            self.bytes + 4,
            (device.root_port as u32) << 16 | (device.hub_ports as u32) << 24,
        );
        self.dword(
            self.bytes + 8,
            device.tt_slot as u32 | (device.tt_port as u32) << 8 | (device.hub_think as u32) << 16,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn endpoint(
        &self,
        dci: u32,
        kind: u32,
        max_packet: u32,
        interval: u32,
        ring: u64,
        average: u32,
    ) {
        let at = (dci as usize + 1) * self.bytes;
        self.dword(at, interval << 16);
        self.dword(at + 4, 3 << 1 | kind << 3 | max_packet << 16);
        self.dword(at + 8, ring as u32 | 1);
        self.dword(at + 12, (ring >> 32) as u32);
        // Bulk endpoints carry no periodic payload (max ESIT stays 0).
        let esit = if kind == 2 || kind == 6 {
            0
        } else {
            max_packet
        };
        self.dword(at + 16, average | esit << 16);
    }
}

pub fn initialize(pci: &PciInventory, frames: &mut FrameAllocator) -> XhciReport {
    let Some(device) = pci.find_class(0x0c, 0x03, 0x30) else {
        return XhciReport::EMPTY;
    };
    if device.bars[0] & 1 != 0 || !pci.enable_memory_bus_master(device) {
        return XhciReport::EMPTY;
    }
    let mut base = (device.bars[0] & 0xffff_fff0) as u64;
    if device.bars[0] & 0x6 == 0x4 {
        base |= (device.bars[1] as u64) << 32;
    }
    if base == 0 {
        return XhciReport::EMPTY;
    }
    let mut report = XhciReport {
        present: true,
        base,
        ..XhciReport::EMPTY
    };
    let cap_length = unsafe { read32(base, 0) } & 0xff;
    report.version = (unsafe { read32(base, 0) } >> 16) as u16;
    let params1 = unsafe { read32(base, 0x04) };
    let params2 = unsafe { read32(base, 0x08) };
    let cap_params = unsafe { read32(base, 0x10) };
    let doorbell_offset = (unsafe { read32(base, 0x14) } & !3) as u64;
    let runtime_offset = (unsafe { read32(base, 0x18) } & !0x1f) as u64;
    let max_slots = params1 & 0xff;
    report.ports = params1 >> 24;
    report.slots = max_slots;
    let operational = base + cap_length as u64;
    let mut state = XhciState::EMPTY;
    state.operational = operational;
    state.runtime = base + runtime_offset;
    state.doorbells = base + doorbell_offset;
    state.context_bytes = if cap_params & (1 << 2) != 0 { 64 } else { 32 };

    // Halt and reset the controller.
    unsafe {
        write32(operational, 0, read32(operational, 0) & !1);
    }
    if !wait_for(|| unsafe { read32(operational, 4) } & 1 != 0) {
        return report;
    }
    unsafe { write32(operational, 0, 2) };
    if !wait_for(|| {
        let command = unsafe { read32(operational, 0) };
        let status = unsafe { read32(operational, 4) };
        command & 2 == 0 && status & (1 << 11) == 0
    }) {
        return report;
    }

    // One contiguous DMA area: DCBAA, scratchpad list, command ring, event
    // ring + ERST, input context, per-device contexts/rings/buffers.
    let scratch_pages = ((params2 >> 21) & 0x1f) | (((params2 >> 27) & 0x1f) << 5);
    let total_pages = 6 + PAGES_PER_DEVICE * MAX_DEVICES as u64 + scratch_pages as u64;
    let Some(dma) = frames.allocate_contiguous(total_pages, 1) else {
        return report;
    };
    let memory = dma.address();
    zero(memory, (total_pages * PAGE_SIZE) as usize);
    let mut cursor = memory;
    let mut take = |pages: u64| {
        let start = cursor;
        cursor += pages * PAGE_SIZE;
        start
    };
    let dcbaa = take(1);
    let scratch_list = take(1);
    let command_ring = take(1);
    let event_ring = take(EVENT_TRBS as u64 * 16 / PAGE_SIZE);
    let erst = take(1);
    let input_context = take(1);
    let device_memory = take(PAGES_PER_DEVICE * MAX_DEVICES as u64);
    let scratch_area = take(scratch_pages as u64);
    if scratch_pages != 0 {
        for index in 0..scratch_pages as u64 {
            unsafe {
                core::ptr::write_volatile(
                    (scratch_list as usize + index as usize * 8) as *mut u64,
                    scratch_area + index * PAGE_SIZE,
                );
            }
        }
        unsafe { core::ptr::write_volatile(dcbaa as usize as *mut u64, scratch_list) };
    }
    state.command = Ring::at(command_ring);
    state.event_base = event_ring;
    state.dcbaa = dcbaa;
    state.input_context = input_context;
    state.device_memory = device_memory;
    unsafe {
        write32(operational, 0x38, max_slots.min(MAX_DEVICES as u32 + 6));
        write64(operational, 0x30, dcbaa);
        write64(operational, 0x18, command_ring | 1);
        // Event ring segment table for interrupter 0.
        core::ptr::write_volatile(erst as usize as *mut u64, event_ring);
        core::ptr::write_volatile((erst as usize + 8) as *mut u32, EVENT_TRBS as u32);
        write32(state.runtime, 0x20 + 0x08, 1);
        write64(state.runtime, 0x20 + 0x18, event_ring);
        write64(state.runtime, 0x20 + 0x10, erst);
        // Run (no interrupts: the driver polls).
        write32(operational, 0, 1);
    }
    if !wait_for(|| unsafe { read32(operational, 4) } & 1 == 0) {
        return report;
    }

    // Bring up each connected root port.
    state.ports = report.ports;
    scan_root_ports(&mut state);
    report.devices = state.devices.iter().filter(|device| device.used).count() as u32;
    for device in state.devices.iter().filter(|device| device.used) {
        for func in &device.funcs {
            match func.kind {
                Kind::Keyboard => report.keyboards += 1,
                Kind::Mouse => report.mice += 1,
                Kind::Pointer => report.tablets += 1,
                _ => {}
            }
        }
        if device.hub_ports != 0 {
            report.hubs += 1;
        }
        if device.storage.sectors != 0 {
            report.disks += 1;
            report.disk_sectors = device.storage.sectors;
        }
    }
    state.ready = true;
    *XHCI.lock() = state;

    // Disk self-test: sector 0 carries the test signature, then an 8-sector
    // write/read-back round trip in a scratch area of the test image.
    if report.disks > 0 {
        let mut sector = [0u8; 4096];
        report.disk_read =
            storage_read(0, 1, &mut sector[..512]) && sector[..14] == *b"AEROS-USB-TEST";
        let mut pattern = [0u8; 4096];
        for (index, byte) in pattern.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(37).wrapping_add(11);
        }
        let mut readback = [0u8; 4096];
        report.disk_write = report.disk_sectors > 16
            && storage_write(8, 8, &pattern)
            && storage_read(8, 8, &mut readback)
            && readback == pattern;
    }
    report.verified = report.devices > 0
        && report.keyboards + report.mice + report.tablets + report.disks > 0
        && (report.disks == 0 || (report.disk_read && report.disk_write));
    report
}

/// PORTSC bits that are plain state, safe to write back (the RW1C change bits
/// must never be written back by accident).
const PORTSC_PRESERVE: u32 = 0x0e00_c200;

/// Handles root ports whose state changed: a removed device is torn down,
/// a newly connected one is reset and enumerated. Also used at start-up,
/// when every connected port is new.
fn scan_root_ports(state: &mut XhciState) {
    for port in 0..state.ports as usize {
        let register = state.operational + 0x400 + (port * 0x10) as u64;
        let status = unsafe { read32(register, 0) };
        let root_port = port as u8 + 1;
        let known = state
            .devices
            .iter()
            .any(|device| device.used && device.root_port == root_port);
        // Acknowledge connect/enable/link change bits.
        let changes = status & (0x7f << 17);
        if changes != 0 {
            unsafe { write32(register, 0, (status & PORTSC_PRESERVE) | changes) };
        }
        if status & 1 == 0 {
            if known {
                remove_port(state, root_port);
            }
            continue;
        }
        if known && changes & (1 << 17) == 0 {
            continue;
        }
        if known {
            // Reconnected without us noticing the gap: start over.
            remove_port(state, root_port);
        }
        // Reset the port (keep only the power bit; never write change bits).
        let status = unsafe { read32(register, 0) };
        unsafe { write32(register, 0, (status & PORTSC_PRESERVE) | (1 << 4)) };
        if !wait_for(|| unsafe { read32(register, 0) } & (1 << 21) != 0) {
            continue;
        }
        let status = unsafe { read32(register, 0) };
        unsafe { write32(register, 0, (status & PORTSC_PRESERVE) | (1 << 21)) };
        let status = unsafe { read32(register, 0) };
        if status & 2 == 0 {
            continue;
        }
        let attach = Attach {
            parent_slot: 0,
            parent_port: 0,
            root_port: root_port as u32,
            route: 0,
            depth: 0,
            speed: (status >> 10) & 0xf,
            tt_slot: 0,
            tt_port: 0,
        };
        let added = enumerate(state, attach);
        crate::serial::format(format_args!(
            "AEROS_USB_PORT port={} speed={} added={}
",
            root_port, attach.speed, added
        ));
    }
}

/// Tears down every device (a hub and everything behind it included) that
/// hangs off a root port.
fn remove_port(state: &mut XhciState, root_port: u8) {
    for index in 0..MAX_DEVICES {
        let device = state.devices[index];
        if device.used && device.root_port == root_port {
            if device.storage.sectors != 0 {
                crate::datafs::MEDIA_DIRTY.store(true, core::sync::atomic::Ordering::Release);
            }
            state.devices[index] = Device::EMPTY;
            state.disable_slot(device.slot);
            crate::serial::format(format_args!(
                "AEROS_USB_PORT port={} removed
",
                root_port
            ));
        }
    }
}

/// How a new device is attached to the tree.
#[derive(Clone, Copy)]
struct Attach {
    parent_slot: u8,
    parent_port: u8,
    root_port: u32,
    route: u32,
    depth: u8,
    speed: u32,
    tt_slot: u8,
    tt_port: u8,
}

/// One interface parsed from a configuration descriptor.
#[derive(Clone, Copy)]
struct Interface {
    number: u8,
    class: u8,
    subclass: u8,
    protocol: u8,
    report_length: u16,
    /// (address, attributes, max packet, interval) per endpoint.
    endpoints: [(u8, u8, u16, u8); 4],
    endpoint_count: usize,
}

impl Interface {
    const EMPTY: Self = Self {
        number: 0,
        class: 0,
        subclass: 0,
        protocol: 0,
        report_length: 0,
        endpoints: [(0, 0, 0, 0); 4],
        endpoint_count: 0,
    };

    fn endpoint(&self, transfer: u8, input: bool) -> Option<(u8, u16, u8)> {
        self.endpoints[..self.endpoint_count]
            .iter()
            .find(|(address, attributes, _, _)| {
                attributes & 3 == transfer && (address & 0x80 != 0) == input
            })
            .map(|&(address, _, packet, interval)| (address, packet, interval))
    }
}

fn parse_configuration(buffer: u64, total: u16) -> ([Interface; MAX_INTERFACES], usize) {
    let mut interfaces = [Interface::EMPTY; MAX_INTERFACES];
    let mut count = 0usize;
    let mut offset = 0u64;
    let mut current: Option<usize> = None;
    while offset + 2 <= total as u64 {
        let length = read_byte(buffer + offset) as u64;
        let kind = read_byte(buffer + offset + 1);
        if length < 2 {
            break;
        }
        let byte = |at: u64| read_byte(buffer + offset + at);
        match kind {
            4 if length >= 9 => {
                current = None;
                // Only alternate setting 0 is used.
                if byte(3) == 0 && count < MAX_INTERFACES {
                    interfaces[count] = Interface {
                        number: byte(2),
                        class: byte(5),
                        subclass: byte(6),
                        protocol: byte(7),
                        ..Interface::EMPTY
                    };
                    current = Some(count);
                    count += 1;
                }
            }
            0x21 if length >= 9 => {
                if let Some(index) = current {
                    interfaces[index].report_length = read_u16(buffer + offset + 7);
                }
            }
            5 if length >= 7 => {
                if let Some(index) = current {
                    let interface = &mut interfaces[index];
                    if interface.endpoint_count < 4 {
                        interface.endpoints[interface.endpoint_count] = (
                            byte(2),
                            byte(3),
                            read_u16(buffer + offset + 4) & 0x7ff,
                            byte(6),
                        );
                        interface.endpoint_count += 1;
                    }
                }
            }
            _ => {}
        }
        offset += length;
    }
    (interfaces, count)
}

/// Reads a HID report descriptor for the first pointer-like input report
/// (X/Y axes plus buttons and, when present, a wheel).
fn parse_pointer(descriptor: u64, length: u16) -> Option<Pointer> {
    let mut usage_page = 0u32;
    let mut logical_min = 0i32;
    let mut logical_max = 0i32;
    let mut report_size = 0u32;
    let mut report_count = 0u32;
    let mut report_id = 0u8;
    let mut usages = [0u32; 16];
    let mut usage_count = 0usize;
    let mut usage_min = 0u32;
    let mut usage_max = 0u32;
    let mut bit = 0u32;
    let mut pointer = Pointer::EMPTY;
    let mut button_report = 0u8;
    let mut offset = 0u64;
    while offset < length as u64 {
        let prefix = read_byte(descriptor + offset);
        if prefix == 0xfe {
            let size = read_byte(descriptor + offset + 1) as u64;
            offset += 3 + size;
            continue;
        }
        let size = match prefix & 3 {
            3 => 4,
            other => other as u64,
        };
        let kind = (prefix >> 2) & 3;
        let tag = prefix >> 4;
        let mut raw = 0u32;
        for index in 0..size {
            raw |= (read_byte(descriptor + offset + 1 + index) as u32) << (index * 8);
        }
        let signed = match size {
            1 => raw as u8 as i8 as i32,
            2 => raw as u16 as i16 as i32,
            _ => raw as i32,
        };
        offset += 1 + size;
        match (kind, tag) {
            (1, 0) => usage_page = raw,
            (1, 1) => logical_min = signed,
            (1, 2) => logical_max = signed,
            (1, 7) => report_size = raw,
            (1, 8) => {
                report_id = raw as u8;
                bit = 0;
            }
            (1, 9) => report_count = raw,
            (2, 0) if usage_count < usages.len() => {
                usages[usage_count] = raw;
                usage_count += 1;
            }
            (2, 1) => usage_min = raw,
            (2, 2) => usage_max = raw,
            (0, 0xa) => {
                usage_count = 0;
                usage_min = 0;
                usage_max = 0;
            }
            (0, 8) => {
                let id_bits = if report_id != 0 { 8 } else { 0 };
                for index in 0..report_count {
                    let field_bit = bit + id_bits;
                    bit += report_size;
                    if raw & 1 != 0 {
                        continue;
                    }
                    let usage = if usage_count > 0 {
                        usages[(index as usize).min(usage_count - 1)]
                    } else if usage_max >= usage_min && usage_max != 0 {
                        usage_min + index
                    } else {
                        0
                    };
                    let field = Field {
                        present: true,
                        bit: field_bit as u16,
                        size: report_size as u8,
                        signed: logical_min < 0,
                        relative: raw & 4 != 0,
                        min: logical_min,
                        max: logical_max,
                    };
                    match (usage_page, usage) {
                        (9, _) => {
                            if pointer.buttons == 0 {
                                pointer.buttons_bit = field_bit as u16;
                                button_report = report_id;
                            }
                            if button_report == report_id {
                                pointer.buttons = pointer.buttons.saturating_add(1);
                            }
                        }
                        (1, 0x30) => {
                            pointer.x = field;
                            pointer.report_id = report_id;
                        }
                        (1, 0x31) => pointer.y = field,
                        (1, 0x38) => pointer.wheel = field,
                        _ => {}
                    }
                }
                usage_count = 0;
                usage_min = 0;
                usage_max = 0;
            }
            (0, 9) | (0, 0xb) => {
                usage_count = 0;
                usage_min = 0;
                usage_max = 0;
            }
            _ => {}
        }
    }
    (pointer.x.present && pointer.y.present).then_some(pointer)
}

fn handle_pointer(pointer: &Pointer, report: &[u8; 64]) {
    if pointer.report_id != 0 && report[0] != pointer.report_id {
        return;
    }
    let data: &[u8] = report;
    let mut buttons = 0u8;
    for index in 0..pointer.buttons.min(3) {
        let bit = pointer.buttons_bit as usize + index as usize;
        if data
            .get(bit / 8)
            .is_some_and(|byte| byte >> (bit % 8) & 1 != 0)
        {
            buttons |= 1 << index;
        }
    }
    let wheel = if pointer.wheel.present {
        pointer.wheel.read(data)
    } else {
        0
    };
    let x = pointer.x.read(data);
    let y = pointer.y.read(data);
    if pointer.x.relative {
        crate::mouse::inject_motion(buttons, x, y, wheel);
    } else {
        let scale = |value: i32, field: &Field| -> u32 {
            let span = (field.max as i64 - field.min as i64).max(1);
            ((value as i64 - field.min as i64).clamp(0, span) * 0xffff / span) as u32
        };
        crate::mouse::inject_absolute(buttons, scale(x, &pointer.x), scale(y, &pointer.y), wheel);
    }
}

/// Addresses a new device, reads its descriptors and brings up whatever
/// functions this driver knows (HID keyboard/pointer, hub, mass storage).
/// Hubs are scanned recursively. Returns whether a device was added.
fn enumerate(state: &mut XhciState, attach: Attach) -> bool {
    let Some(index) = state.devices.iter().position(|device| !device.used) else {
        return false;
    };
    if enumerate_at(state, index, attach) {
        return true;
    }
    // Give a half-set-up device's controller slot back.
    let slot = state.devices[index].slot;
    state.devices[index] = Device::EMPTY;
    if slot != 0 {
        state.disable_slot(slot);
    }
    false
}

fn enumerate_at(state: &mut XhciState, index: usize, attach: Attach) -> bool {
    let Some((code, slot)) = state.command([0, 0, 0, TRB_ENABLE_SLOT << 10]) else {
        return false;
    };
    if code != CC_SUCCESS || slot == 0 || slot as usize >= 256 {
        return false;
    }
    let area = state.device_memory + index as u64 * PAGES_PER_DEVICE * PAGE_SIZE;
    zero(area, (PAGES_PER_DEVICE * PAGE_SIZE) as usize);
    let output = area;
    let scratch = area + 5 * PAGE_SIZE;
    let mut device = Device {
        used: true,
        slot: slot as u8,
        speed: attach.speed as u8,
        root_port: attach.root_port as u8,
        route: attach.route,
        depth: attach.depth,
        tt_slot: attach.tt_slot,
        tt_port: attach.tt_port,
        parent_slot: attach.parent_slot,
        parent_port: attach.parent_port,
        control: Ring::at(area + PAGE_SIZE),
        scratch,
        data: area + 6 * PAGE_SIZE,
        ..Device::EMPTY
    };
    for ring in 0..FUNCS {
        device.rings[ring] = Ring::at(area + (2 + ring as u64) * PAGE_SIZE);
        device.funcs[ring].report = scratch + REPORT_AREA + ring as u64 * 64;
    }
    unsafe {
        core::ptr::write_volatile(
            (state.dcbaa as usize + slot as usize * 8) as *mut u64,
            output,
        );
    }
    let contexts = Contexts {
        input: state.input_context,
        bytes: state.context_bytes,
    };
    let max_packet = match attach.speed {
        SPEED_LOW | SPEED_FULL => 8u32,
        SPEED_HIGH => 64,
        _ => 512,
    };
    // Input context: add the slot and default-endpoint contexts.
    zero(state.input_context, PAGE_SIZE as usize);
    contexts.dword(4, 0b11);
    contexts.slot(&device, 1);
    let ep0 = 2 * contexts.bytes;
    contexts.dword(ep0 + 4, 3 << 1 | 4 << 3 | max_packet << 16);
    contexts.dword(ep0 + 8, device.control.base as u32 | 1);
    contexts.dword(ep0 + 12, (device.control.base >> 32) as u32);
    contexts.dword(ep0 + 16, 8);
    state.devices[index] = device;
    match state.command([
        state.input_context as u32,
        (state.input_context >> 32) as u32,
        0,
        TRB_ADDRESS_DEVICE << 10 | slot << 24,
    ]) {
        Some((CC_SUCCESS, _)) => {}
        _ => return false,
    }

    // The first 8 bytes of the device descriptor reveal the real EP0 packet
    // size; low/full speed devices may need the context corrected.
    let small = scratch + SMALL_AREA;
    let big = scratch + DESCRIPTOR_AREA;
    zero(big, 64);
    if !state.control(index, get_descriptor(1, 0, 8), big, 8, true) {
        return false;
    }
    let reported = read_byte(big + 7) as u32;
    let real_packet = if attach.speed >= SPEED_SUPER {
        1u32 << reported.min(15)
    } else {
        reported
    };
    if real_packet != 0 && real_packet != max_packet && attach.speed < SPEED_HIGH {
        zero(state.input_context, PAGE_SIZE as usize);
        contexts.dword(4, 0b10);
        contexts.dword(ep0 + 4, 3 << 1 | 4 << 3 | real_packet << 16);
        let _ = state.command([
            state.input_context as u32,
            (state.input_context >> 32) as u32,
            0,
            TRB_EVALUATE_CONTEXT << 10 | slot << 24,
        ]);
    }
    if !state.control(index, get_descriptor(1, 0, 18), big, 18, true) {
        return false;
    }
    let device_class = read_byte(big + 4);
    let device_protocol = read_byte(big + 6);
    if !state.control(index, get_descriptor(2, 0, 9), big, 9, true) {
        return false;
    }
    let total = read_u16(big + 2).min(DESCRIPTOR_MAX);
    let configuration = read_byte(big + 5);
    if !state.control(index, get_descriptor(2, 0, total), big, total, true) {
        return false;
    }
    let (interfaces, interface_count) = parse_configuration(big, total);
    // Everything from the configuration is copied out before the scratch
    // area is reused for report descriptors.
    let interfaces = &interfaces[..interface_count];

    if !state.control(
        index,
        [0x00, 0x09, configuration, 0, 0, 0, 0, 0],
        0,
        0,
        false,
    ) {
        return false;
    }

    let mut func_count = 0usize;
    let mut add_flags = 1u32;
    let mut highest = 1u32;
    zero(state.input_context, PAGE_SIZE as usize);
    let mut hub_interface: Option<Interface> = None;
    let mut storage_interface: Option<Interface> = None;

    for interface in interfaces {
        match interface.class {
            3 if func_count < FUNCS => {
                let Some((address, packet, interval)) = interface.endpoint(3, true) else {
                    continue;
                };
                let mut kind = match (interface.subclass, interface.protocol) {
                    (1, 1) => Kind::Keyboard,
                    (1, 2) => Kind::Mouse,
                    _ => Kind::None,
                };
                let mut pointer = Pointer::EMPTY;
                if kind == Kind::None && interface.report_length != 0 {
                    let wanted = interface.report_length.min(DESCRIPTOR_MAX);
                    zero(big, wanted as usize);
                    // GET_DESCRIPTOR(HID report) addressed to the interface.
                    let setup = [
                        0x81,
                        0x06,
                        0,
                        0x22,
                        interface.number,
                        0,
                        wanted as u8,
                        (wanted >> 8) as u8,
                    ];
                    if state.control(index, setup, big, wanted, true) {
                        if let Some(found) = parse_pointer(big, wanted) {
                            kind = Kind::Pointer;
                            pointer = found;
                        }
                    }
                }
                if kind == Kind::None {
                    continue;
                }
                if matches!(kind, Kind::Keyboard | Kind::Mouse) {
                    // Boot protocol, no idle repeat.
                    let _ = state.control(
                        index,
                        [0x21, 0x0b, 0, 0, interface.number, 0, 0, 0],
                        0,
                        0,
                        false,
                    );
                    let _ = state.control(
                        index,
                        [0x21, 0x0a, 0, 0, interface.number, 0, 0, 0],
                        0,
                        0,
                        false,
                    );
                }
                let dci = (address & 0x0f) as u32 * 2 + 1;
                let ring = func_count;
                contexts.endpoint(
                    dci,
                    7,
                    packet.min(64) as u32,
                    interrupt_interval(attach.speed, interval),
                    state.devices[index].rings[ring].base,
                    8,
                );
                add_flags |= 1 << dci;
                highest = highest.max(dci);
                let report_bytes = (packet.min(64) as u32).max(1);
                let function = &mut state.devices[index].funcs[func_count];
                function.kind = kind;
                function.dci = dci as u8;
                function.ring = ring;
                function.report_bytes = report_bytes;
                function.pointer = pointer;
                func_count += 1;
            }
            9 if hub_interface.is_none() => hub_interface = Some(*interface),
            8 if interface.subclass == 6
                && interface.protocol == 0x50
                && storage_interface.is_none() =>
            {
                storage_interface = Some(*interface)
            }
            _ => {}
        }
    }
    if device_class == 9 && hub_interface.is_none() {
        hub_interface = interfaces.first().copied();
    }

    // A hub: read its descriptor for the port count first, then configure it
    // (slot context gets the hub flag) together with its status endpoint.
    let mut hub_status: Option<(u32, u32, u32)> = None;
    if let Some(interface) = hub_interface {
        let hub_type = if attach.speed >= SPEED_SUPER {
            0x2a
        } else {
            0x29
        };
        zero(small, 16);
        let setup = [0xa0, 0x06, 0, hub_type, 0, 0, 9, 0];
        if state.control(index, setup, small, 9, true) {
            let ports = read_byte(small + 2);
            let characteristics = read_u16(small + 3);
            let think = ((characteristics >> 5) & 3) as u8;
            if ports > 0 {
                let device = &mut state.devices[index];
                device.hub_ports = ports.min(15);
                device.hub_mtt = device_protocol == 2;
                device.hub_think = think;
                if let Some((address, packet, interval)) = interface.endpoint(3, true) {
                    let dci = (address & 0x0f) as u32 * 2 + 1;
                    hub_status = Some((dci, packet as u32, interval as u32));
                }
            }
        }
    }
    if let Some((dci, packet, interval)) = hub_status {
        let function = &mut state.devices[index].funcs[FUNCS - 1];
        function.kind = Kind::Hub;
        function.dci = dci as u8;
        function.ring = FUNCS - 1;
        function.report_bytes = 2;
        contexts.endpoint(
            dci,
            7,
            packet.min(64),
            interrupt_interval(attach.speed, interval as u8),
            state.devices[index].rings[FUNCS - 1].base,
            2,
        );
        add_flags |= 1 << dci;
        highest = highest.max(dci);
    }

    // Mass storage: bulk in and bulk out endpoints.
    let mut storage_ready = false;
    if let Some(interface) = storage_interface {
        if let (Some((in_address, in_packet, _)), Some((out_address, out_packet, _))) =
            (interface.endpoint(2, true), interface.endpoint(2, false))
        {
            let in_dci = (in_address & 0x0f) as u32 * 2 + 1;
            let out_dci = (out_address & 0x0f) as u32 * 2;
            contexts.endpoint(
                in_dci,
                6,
                in_packet as u32,
                0,
                state.devices[index].rings[0].base,
                in_packet as u32,
            );
            contexts.endpoint(
                out_dci,
                2,
                out_packet as u32,
                0,
                state.devices[index].rings[1].base,
                out_packet as u32,
            );
            add_flags |= 1 << in_dci | 1 << out_dci;
            highest = highest.max(in_dci).max(out_dci);
            let storage = &mut state.devices[index].storage;
            storage.interface = interface.number;
            storage.in_dci = in_dci as u8;
            storage.out_dci = out_dci as u8;
            storage_ready = true;
        }
    }

    let hub_or_functions = func_count > 0 || hub_status.is_some() || storage_ready;
    if hub_or_functions {
        contexts.dword(4, add_flags);
        contexts.slot(&state.devices[index], highest);
        match state.command([
            state.input_context as u32,
            (state.input_context >> 32) as u32,
            0,
            TRB_CONFIGURE_ENDPOINT << 10 | slot << 24,
        ]) {
            Some((CC_SUCCESS, _)) => {}
            _ => {
                return false;
            }
        }
    }
    for func in 0..func_count {
        state.queue_report(index, func);
        let dci = state.devices[index].funcs[func].dci as u32;
        state.ring_doorbell(slot as usize, dci);
    }
    if hub_status.is_some() {
        let dci = state.devices[index].funcs[FUNCS - 1].dci as u32;
        state.queue_report(index, FUNCS - 1);
        state.ring_doorbell(slot as usize, dci);
    }
    if storage_ready {
        setup_storage(state, index);
        crate::datafs::MEDIA_DIRTY.store(true, core::sync::atomic::Ordering::Release);
    }
    if state.devices[index].hub_ports != 0 {
        scan_hub(state, index);
    }
    true
}

/// xHCI endpoint interval exponent for an interrupt endpoint.
fn interrupt_interval(speed: u32, interval: u8) -> u32 {
    if speed >= SPEED_HIGH {
        interval.saturating_sub(1) as u32
    } else {
        (32 - ((interval.max(1) as u32) * 8).leading_zeros())
            .saturating_sub(1)
            .clamp(3, 10)
    }
}

/// Powers, resets and enumerates every port of a hub.
fn scan_hub(state: &mut XhciState, hub: usize) {
    let ports = state.devices[hub].hub_ports as u16;
    for port in 1..=ports {
        let _ = state.control(hub, hub_request(0x23, 0x03, 8, port), 0, 0, false);
    }
    // Power-on-to-power-good plus connect debounce.
    delay_ms(150);
    for port in 1..=ports {
        hub_port_connect(state, hub, port);
    }
}

/// Class request to a hub port (`type_`: 0x23 host-to-device, 0xa3 device-to-host).
fn hub_request(type_: u8, request: u8, feature: u8, port: u16) -> [u8; 8] {
    let length = if type_ == 0xa3 { 4 } else { 0 };
    [
        type_,
        request,
        feature,
        0,
        port as u8,
        (port >> 8) as u8,
        length,
        0,
    ]
}

/// Reads a hub port's (status, change) word.
fn hub_port_status(state: &mut XhciState, hub: usize, port: u16) -> Option<u32> {
    let small = state.devices[hub].scratch + SMALL_AREA;
    zero(small, 8);
    state
        .control(hub, hub_request(0xa3, 0x00, 0, port), small, 4, true)
        .then(|| read_u32(small))
}

/// If something is plugged into `port` of `hub`, resets and enumerates it.
fn hub_port_connect(state: &mut XhciState, hub: usize, port: u16) {
    if state.devices.iter().all(|device| device.used) {
        return;
    }
    let Some(status) = hub_port_status(state, hub, port) else {
        return;
    };
    if status & 1 == 0 {
        return;
    }
    let _ = state.control(hub, hub_request(0x23, 0x01, 16, port), 0, 0, false);
    if !state.control(hub, hub_request(0x23, 0x03, 4, port), 0, 0, false) {
        return;
    }
    let mut done = false;
    for _ in 0..50 {
        delay_ms(20);
        let Some(status) = hub_port_status(state, hub, port) else {
            return;
        };
        if status & (1 << 20) != 0 {
            done = true;
            break;
        }
    }
    if !done {
        return;
    }
    let _ = state.control(hub, hub_request(0x23, 0x01, 20, port), 0, 0, false);
    delay_ms(20);
    let Some(status) = hub_port_status(state, hub, port) else {
        return;
    };
    if status & 2 == 0 {
        return;
    }
    let parent = state.devices[hub];
    let speed = if parent.speed as u32 >= SPEED_SUPER {
        SPEED_SUPER
    } else if status & (1 << 9) != 0 {
        SPEED_LOW
    } else if status & (1 << 10) != 0 {
        SPEED_HIGH
    } else {
        SPEED_FULL
    };
    // Low/full-speed devices behind a high-speed hub go through its
    // transaction translator; deeper down they inherit the hub's.
    let (tt_slot, tt_port) = if speed == SPEED_LOW || speed == SPEED_FULL {
        if parent.speed as u32 == SPEED_HIGH {
            (parent.slot, port as u8)
        } else {
            (parent.tt_slot, parent.tt_port)
        }
    } else {
        (0, 0)
    };
    let attach = Attach {
        parent_slot: parent.slot,
        parent_port: port as u8,
        root_port: parent.root_port as u32,
        route: parent.route | (port as u32) << (4 * parent.depth as u32),
        depth: parent.depth + 1,
        speed,
        tt_slot,
        tt_port,
    };
    let added = enumerate(state, attach);
    crate::serial::format(format_args!(
        "AEROS_USB_HUB_PORT hub_slot={} port={} speed={} added={}\n",
        parent.slot, port, speed, added
    ));
}

/// Removes the device on a hub port, and (for a hub) everything behind it.
fn remove_hub_child(state: &mut XhciState, hub_slot: u8, port: u8) {
    for index in 0..MAX_DEVICES {
        let device = state.devices[index];
        if device.used && device.parent_slot == hub_slot && device.parent_port == port {
            remove_device(state, index);
            crate::serial::format(format_args!(
                "AEROS_USB_HUB_PORT hub_slot={} port={} removed\n",
                hub_slot, port
            ));
        }
    }
}

/// Frees a device and its descendants.
fn remove_device(state: &mut XhciState, index: usize) {
    let device = state.devices[index];
    if device.storage.sectors != 0 {
        crate::datafs::MEDIA_DIRTY.store(true, core::sync::atomic::Ordering::Release);
    }
    state.devices[index] = Device::EMPTY;
    state.disable_slot(device.slot);
    for child in 0..MAX_DEVICES {
        if state.devices[child].used && state.devices[child].parent_slot == device.slot {
            remove_device(state, child);
        }
    }
}

/// Acts on a hub's status-change report: clears the change bits, tears down
/// unplugged devices and enumerates newly plugged ones.
fn handle_hub_change(state: &mut XhciState, hub_slot: u8, bitmap: u16) {
    let Some(hub) = state
        .devices
        .iter()
        .position(|device| device.used && device.slot == hub_slot && device.hub_ports != 0)
    else {
        return;
    };
    for port in 1..=state.devices[hub].hub_ports as u16 {
        if bitmap & (1 << port) == 0 {
            continue;
        }
        let Some(status) = hub_port_status(state, hub, port) else {
            continue;
        };
        let changes = status >> 16;
        for bit in 0..5u8 {
            if changes & (1 << bit) != 0 {
                let _ = state.control(hub, hub_request(0x23, 0x01, 16 + bit, port), 0, 0, false);
            }
        }
        if changes & 1 == 0 {
            continue;
        }
        remove_hub_child(state, hub_slot, port as u8);
        if status & 1 != 0 {
            hub_port_connect(state, hub, port);
        }
    }
}

/// SCSI over bulk-only transport: sends `cdb`, moves `length` bytes through
/// the device's data page and checks the status wrapper.
fn scsi(state: &mut XhciState, device: usize, cdb: &[u8], length: u32, input: bool) -> bool {
    let (in_dci, out_dci, scratch, data) = {
        let device = &state.devices[device];
        (
            device.storage.in_dci,
            device.storage.out_dci,
            device.scratch,
            device.data,
        )
    };
    let tag = state.devices[device].storage.tag.wrapping_add(1);
    state.devices[device].storage.tag = tag;
    let cbw = scratch + CBW_AREA;
    zero(cbw, 32);
    write_u32(cbw, 0x4342_5355);
    write_u32(cbw + 4, tag);
    write_u32(cbw + 8, length);
    write_byte(cbw + 12, if input { 0x80 } else { 0 });
    write_byte(cbw + 14, cdb.len() as u8);
    for (index, byte) in cdb.iter().enumerate() {
        write_byte(cbw + 15 + index as u64, *byte);
    }
    if state.bulk(device, 1, out_dci, cbw, 31).is_none() {
        return false;
    }
    if length != 0 {
        let (ring, dci) = if input { (0, in_dci) } else { (1, out_dci) };
        if state.bulk(device, ring, dci, data, length).is_none() {
            // A stalled data stage is followed by the status wrapper anyway.
            let csw = scratch + CSW_AREA;
            zero(csw, 16);
            let _ = state.bulk(device, 0, in_dci, csw, 13);
            return false;
        }
    }
    let csw = scratch + CSW_AREA;
    zero(csw, 16);
    if state.bulk(device, 0, in_dci, csw, 13).is_none() {
        return false;
    }
    read_u32(csw) == 0x5342_5355 && read_u32(csw + 4) == tag && read_byte(csw + 12) == 0
}

/// Brings a mass-storage device to "ready" and reads its capacity.
fn setup_storage(state: &mut XhciState, device: usize) {
    let mut ready = false;
    for _ in 0..8 {
        if scsi(state, device, &[0x00, 0, 0, 0, 0, 0], 0, false) {
            ready = true;
            break;
        }
        // Clear any pending UNIT ATTENTION.
        let _ = scsi(state, device, &[0x03, 0, 0, 0, 18, 0], 18, true);
        delay_ms(20);
    }
    if !ready {
        return;
    }
    if !scsi(state, device, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], 8, true) {
        return;
    }
    let data = state.devices[device].data;
    let last = u32::from_be_bytes([
        read_byte(data),
        read_byte(data + 1),
        read_byte(data + 2),
        read_byte(data + 3),
    ]);
    let block = u32::from_be_bytes([
        read_byte(data + 4),
        read_byte(data + 5),
        read_byte(data + 6),
        read_byte(data + 7),
    ]);
    let storage = &mut state.devices[device].storage;
    storage.sectors = last as u64 + 1;
    storage.block_bytes = block;
}

/// Sectors on the first USB mass-storage disk (0 = none). Only 512-byte
/// blocks are served.
pub fn storage_sectors() -> u64 {
    let state = XHCI.lock();
    state
        .devices
        .iter()
        .find(|device| device.storage.sectors != 0 && device.storage.block_bytes == 512)
        .map_or(0, |device| device.storage.sectors)
}

/// Bulk-only "reset recovery": reset the transport, then un-halt both bulk
/// endpoints, so the next command starts from a known state.
fn bot_recover(state: &mut XhciState, device: usize) {
    let storage = state.devices[device].storage;
    let _ = state.control(
        device,
        [0x21, 0xff, 0, 0, storage.interface, 0, 0, 0],
        0,
        0,
        false,
    );
    for dci in [storage.in_dci, storage.out_dci] {
        let address = (dci >> 1) | if dci & 1 != 0 { 0x80 } else { 0 };
        let _ = state.control(device, [0x02, 0x01, 0, 0, address, 0, 0, 0], 0, 0, false);
    }
}

fn storage_transfer(lba: u64, sectors: usize, buffer: *mut u8, write: bool) -> bool {
    if sectors == 0 || sectors > 8 {
        return false;
    }
    let mut state = XHCI.lock();
    let Some(index) = state
        .devices
        .iter()
        .position(|device| device.storage.sectors != 0 && device.storage.block_bytes == 512)
    else {
        return false;
    };
    if lba + sectors as u64 > state.devices[index].storage.sectors || lba > u32::MAX as u64 {
        return false;
    }
    let data = state.devices[index].data;
    let bytes = sectors * 512;
    let mut cdb = [0u8; 10];
    cdb[0] = if write { 0x2a } else { 0x28 };
    cdb[2..6].copy_from_slice(&(lba as u32).to_be_bytes());
    cdb[7..9].copy_from_slice(&(sectors as u16).to_be_bytes());
    if write {
        unsafe { core::ptr::copy_nonoverlapping(buffer, data as usize as *mut u8, bytes) };
    }
    let mut done = false;
    for _attempt in 0..3 {
        if scsi(&mut state, index, &cdb, bytes as u32, !write) {
            done = true;
            break;
        }
        bot_recover(&mut state, index);
    }
    if !done {
        return false;
    }
    if !write {
        unsafe { core::ptr::copy_nonoverlapping(data as usize as *const u8, buffer, bytes) };
    }
    true
}

/// Reads `sectors` (1..=8) 512-byte sectors from the USB disk.
pub fn storage_read(lba: u64, sectors: usize, destination: &mut [u8]) -> bool {
    destination.len() >= sectors * 512
        && storage_transfer(lba, sectors, destination.as_mut_ptr(), false)
}

/// Writes `sectors` (1..=8) 512-byte sectors to the USB disk.
pub fn storage_write(lba: u64, sectors: usize, source: &[u8]) -> bool {
    source.len() >= sectors * 512
        && storage_transfer(lba, sectors, source.as_ptr() as *mut u8, true)
}

/// HID keyboard usage -> (PS/2 set-1 make code, extended prefix).
fn scancode_for(usage: u8) -> Option<(u8, bool)> {
    const LETTERS: [u8; 26] = [
        0x1e, 0x30, 0x2e, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18,
        0x19, 0x10, 0x13, 0x1f, 0x14, 0x16, 0x2f, 0x11, 0x2d, 0x15, 0x2c,
    ];
    match usage {
        0x04..=0x1d => Some((LETTERS[(usage - 0x04) as usize], false)),
        0x1e..=0x26 => Some((0x02 + (usage - 0x1e), false)),
        0x27 => Some((0x0b, false)),
        0x28 => Some((0x1c, false)),
        0x29 => Some((0x01, false)),
        0x2a => Some((0x0e, false)),
        0x2b => Some((0x0f, false)),
        0x2c => Some((0x39, false)),
        0x2d => Some((0x0c, false)),
        0x2e => Some((0x0d, false)),
        0x2f => Some((0x1a, false)),
        0x30 => Some((0x1b, false)),
        0x31 => Some((0x2b, false)),
        0x33 => Some((0x27, false)),
        0x34 => Some((0x28, false)),
        0x35 => Some((0x29, false)),
        0x36 => Some((0x33, false)),
        0x37 => Some((0x34, false)),
        0x38 => Some((0x35, false)),
        0x39 => Some((0x3a, false)),
        0x3a..=0x43 => Some((0x3b + (usage - 0x3a), false)),
        0x44 => Some((0x57, false)),
        0x45 => Some((0x58, false)),
        0x49 => Some((0x52, true)),
        0x4a => Some((0x47, true)),
        0x4b => Some((0x49, true)),
        0x4c => Some((0x53, true)),
        0x4d => Some((0x4f, true)),
        0x4e => Some((0x51, true)),
        0x4f => Some((0x4d, true)),
        0x50 => Some((0x4b, true)),
        0x51 => Some((0x50, true)),
        0x52 => Some((0x48, true)),
        _ => None,
    }
}

fn push_key(scancode: u8, extended: bool, released: bool) {
    if extended {
        crate::keyboard::push_scancode(0xe0);
    }
    crate::keyboard::push_scancode(scancode | if released { 0x80 } else { 0 });
}

fn handle_keyboard(report: [u8; 8], previous: [u8; 8]) {
    // Modifier byte: LCtrl LShift LAlt LGui RCtrl RShift RAlt RGui.
    const MODIFIERS: [(u8, bool); 8] = [
        (0x1d, false),
        (0x2a, false),
        (0x38, false),
        (0x5b, true),
        (0x1d, true),
        (0x36, false),
        (0x38, true),
        (0x5c, true),
    ];
    let changed = report[0] ^ previous[0];
    for (bit, (code, extended)) in MODIFIERS.iter().enumerate() {
        if changed & (1 << bit) != 0 {
            push_key(*code, *extended, report[0] & (1 << bit) == 0);
        }
    }
    for usage in &previous[2..8] {
        if *usage > 3 && !report[2..8].contains(usage) {
            if let Some((code, extended)) = scancode_for(*usage) {
                push_key(code, extended, true);
            }
        }
    }
    for usage in &report[2..8] {
        if *usage > 3 && !previous[2..8].contains(usage) {
            if let Some((code, extended)) = scancode_for(*usage) {
                push_key(code, extended, false);
            }
        }
    }
}

/// Drains the event ring; call this often (it never blocks).
pub fn poll() {
    let mut state = XHCI.lock();
    if !state.ready {
        return;
    }
    let mut rescan = false;
    for _ in 0..64 {
        let Some(event) = state.next_event() else {
            break;
        };
        if event[3] >> 10 & 0x3f == TRB_PORT_STATUS_CHANGE {
            rescan = true;
        }
        state.dispatch_transfer(event);
    }
    if rescan {
        scan_root_ports(&mut state);
    }
    for slot in 0..state.hub_changes.len() {
        let (hub_slot, bitmap) = state.hub_changes[slot];
        if bitmap != 0 {
            state.hub_changes[slot] = (0, 0);
            handle_hub_change(&mut state, hub_slot, bitmap);
        }
    }
}
