use std::collections::VecDeque;

use aml_host::ec::{self, Bus};

#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,
    ReadAddress,
    WriteAddress,
    WriteValue(u8),
}

/// A model of the controller's host interface: the input buffer stays full for
/// a few polls after every byte, as a real one does while its firmware runs.
struct FakeEc {
    space: [u8; 256],
    state: State,
    busy: u32,
    output: Option<u8>,
    events: VecDeque<u8>,
    clock: u64,
    dead: bool,
    log: Vec<String>,
}

impl FakeEc {
    fn new() -> Self {
        Self {
            space: [0; 256],
            state: State::Idle,
            busy: 0,
            output: None,
            events: VecDeque::new(),
            clock: 0,
            dead: false,
            log: Vec::new(),
        }
    }
}

impl Bus for FakeEc {
    fn status(&mut self) -> u8 {
        self.clock += 1_000;
        if self.dead {
            return 0x02;
        }
        let mut value = 0;
        if self.busy > 0 {
            self.busy -= 1;
            value |= 0x02;
        }
        if self.output.is_some() {
            value |= 0x01;
        }
        if !self.events.is_empty() {
            value |= 0x20;
        }
        value
    }

    fn read_data(&mut self) -> u8 {
        self.output.take().expect("read without output")
    }

    fn write_command(&mut self, value: u8) {
        assert_eq!(self.busy, 0, "command written while the input buffer was full");
        self.busy = 3;
        self.log.push(format!("cmd {value:#x}"));
        self.state = match value {
            0x80 => State::ReadAddress,
            0x81 => State::WriteAddress,
            0x84 => {
                self.output = Some(self.events.pop_front().unwrap_or(0));
                State::Idle
            }
            other => panic!("unknown command {other:#x}"),
        };
    }

    fn write_data(&mut self, value: u8) {
        assert_eq!(self.busy, 0, "data written while the input buffer was full");
        self.busy = 3;
        self.log.push(format!("data {value:#x}"));
        self.state = match self.state {
            State::ReadAddress => {
                self.output = Some(self.space[value as usize]);
                State::Idle
            }
            State::WriteAddress => State::WriteValue(value),
            State::WriteValue(address) => {
                self.space[address as usize] = value;
                State::Idle
            }
            State::Idle => panic!("data byte without a command"),
        };
    }

    fn now_ns(&mut self) -> u64 {
        self.clock
    }
}

#[test]
fn reads_and_writes_bytes() {
    let mut ec = FakeEc::new();
    ec.space[0x10] = 0x5a;
    assert_eq!(ec::read(&mut ec, 0x10), Some(0x5a));
    assert!(ec::write(&mut ec, 0x20, 0xc3));
    assert_eq!(ec.space[0x20], 0xc3);
    assert_eq!(ec::read(&mut ec, 0x20), Some(0xc3));
    assert_eq!(ec.log[..4], ["cmd 0x80", "data 0x10", "cmd 0x81", "data 0x20"]);
}

#[test]
fn queries_come_out_in_order() {
    let mut ec = FakeEc::new();
    assert_eq!(ec::query(&mut ec), None);
    ec.events.extend([0x40, 0x17]);
    assert!(ec::event_pending(&mut ec));
    assert_eq!(ec::query(&mut ec), Some(0x40));
    assert_eq!(ec::query(&mut ec), Some(0x17));
    assert_eq!(ec::query(&mut ec), None);
    assert!(!ec::event_pending(&mut ec));
}

#[test]
fn a_zero_query_is_no_event() {
    let mut ec = FakeEc::new();
    ec.events.push_back(0);
    assert_eq!(ec::query(&mut ec), None);
}

#[test]
fn a_dead_controller_times_out() {
    let mut ec = FakeEc::new();
    ec.dead = true;
    assert_eq!(ec::read(&mut ec, 1), None);
    assert!(!ec::write(&mut ec, 1, 2));
    // About half a second of model time per attempt, not an endless wait.
    assert!(ec.clock < 2_000_000_000);
}
