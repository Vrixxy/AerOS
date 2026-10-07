//! The ACPI embedded controller's byte-wise command protocol (specification
//! chapter 12). Generic over the port access, so it is tested on the host
//! against a model of the controller. Most laptops keep the battery, the
//! mains adapter and the lid behind it.

const OUTPUT_FULL: u8 = 0x01;
const INPUT_FULL: u8 = 0x02;
const SCI_EVENT: u8 = 0x20;
const COMMAND_READ: u8 = 0x80;
const COMMAND_WRITE: u8 = 0x81;
const COMMAND_QUERY: u8 = 0x84;
const TIMEOUT_NS: u64 = 500_000_000;

pub trait Bus {
    fn status(&mut self) -> u8;
    fn read_data(&mut self) -> u8;
    fn write_command(&mut self, value: u8);
    fn write_data(&mut self, value: u8);
    fn now_ns(&mut self) -> u64;
}

fn wait(bus: &mut impl Bus, mask: u8, value: u8) -> bool {
    let start = bus.now_ns();
    loop {
        if bus.status() & mask == value {
            return true;
        }
        if bus.now_ns().saturating_sub(start) > TIMEOUT_NS {
            return false;
        }
        core::hint::spin_loop();
    }
}

fn send_command(bus: &mut impl Bus, command: u8) -> bool {
    if !wait(bus, INPUT_FULL, 0) {
        return false;
    }
    bus.write_command(command);
    true
}

fn send_data(bus: &mut impl Bus, value: u8) -> bool {
    if !wait(bus, INPUT_FULL, 0) {
        return false;
    }
    bus.write_data(value);
    true
}

fn receive(bus: &mut impl Bus) -> Option<u8> {
    wait(bus, OUTPUT_FULL, OUTPUT_FULL).then(|| bus.read_data())
}

pub fn read(bus: &mut impl Bus, address: u8) -> Option<u8> {
    if !send_command(bus, COMMAND_READ) || !send_data(bus, address) {
        return None;
    }
    receive(bus)
}

pub fn write(bus: &mut impl Bus, address: u8, value: u8) -> bool {
    send_command(bus, COMMAND_WRITE) && send_data(bus, address) && send_data(bus, value)
}

/// Whether the controller has an event waiting for the OS.
pub fn event_pending(bus: &mut impl Bus) -> bool {
    bus.status() & SCI_EVENT != 0
}

/// Takes the next event's query value (the `_Qxx` to run). `None` when no
/// event is pending or the controller does not answer.
pub fn query(bus: &mut impl Bus) -> Option<u8> {
    if !event_pending(bus) || !send_command(bus, COMMAND_QUERY) {
        return None;
    }
    receive(bus).filter(|code| *code != 0)
}
