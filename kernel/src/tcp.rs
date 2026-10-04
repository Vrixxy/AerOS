//! TCP state machine, independent of any network card: segments go in through
//! `Tcp::input`, come out through a `Sink`, and time only advances through the
//! `now` passed to each call. It has retransmission with a measured timeout
//! and backoff, slow start, congestion avoidance, fast retransmit and fast
//! recovery, a receive window, zero-window probing, passive and active open,
//! and the full close sequence. There is no SACK, window scaling, delayed ACK
//! or out-of-order buffering: segments that arrive ahead of a gap are
//! discarded and a duplicate ACK asks for the missing one again.

#![allow(dead_code)]

use crate::ip::{self, Address};
use crate::sockopt::Options;

pub const MSS: usize = 1460;
/// Largest segment payload that fits an Ethernet frame behind an IPv6 header.
const MSS_V6: usize = 1440;
pub const BUFFER: usize = 8192;
pub const SOCKETS: usize = 8;
const BACKLOG: usize = 4;

pub const FIN: u8 = 0x01;
pub const SYN: u8 = 0x02;
pub const RST: u8 = 0x04;
pub const PSH: u8 = 0x08;
pub const ACK: u8 = 0x10;

const INITIAL_RTO_NS: u64 = 1_000_000_000;
const MIN_RTO_NS: u64 = 200_000_000;
const MAX_RTO_NS: u64 = 8_000_000_000;
const TIME_WAIT_NS: u64 = 2_000_000_000;
const MAX_RETRIES: u32 = 8;
const MAX_SYN_RETRIES: u32 = 5;
const MAX_CWND: u32 = 65_535;
const INITIAL_CWND: u32 = 2 * MSS as u32;

pub const ECONNRESET: u64 = 104;
pub const ETIMEDOUT: u64 = 110;
pub const ECONNREFUSED: u64 = 111;
pub const ENOTCONN: u64 = 107;
pub const EADDRINUSE: u64 = 98;
pub const EAGAIN: u64 = 11;
pub const EPIPE: u64 = 32;
pub const EISCONN: u64 = 106;
pub const EINVAL: u64 = 22;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

pub const MAX_SACK: usize = 4;
const SCOREBOARD: usize = 8;

/// The selective-acknowledgement part of a segment's options: the permission
/// on a SYN, up to four received ranges (start, end exclusive) on an ACK.
#[derive(Clone, Copy, Default)]
pub struct Sack {
    pub permitted: bool,
    pub count: usize,
    pub blocks: [(u32, u32); MAX_SACK],
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Closed => "CLOSED",
            State::Listen => "LISTEN",
            State::SynSent => "SYN_SENT",
            State::SynReceived => "SYN_RECV",
            State::Established => "ESTABLISHED",
            State::FinWait1 => "FIN_WAIT1",
            State::FinWait2 => "FIN_WAIT2",
            State::CloseWait => "CLOSE_WAIT",
            State::Closing => "CLOSING",
            State::LastAck => "LAST_ACK",
            State::TimeWait => "TIME_WAIT",
        }
    }
}

pub struct Incoming<'a> {
    pub remote: Address,
    pub remote_port: u16,
    pub local_port: u16,
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    pub window: u16,
    /// The peer's MSS option on a SYN, 0 when there is none.
    pub mss: u16,
    pub sack: Sack,
    pub payload: &'a [u8],
}

pub struct Outgoing<'a> {
    pub remote: Address,
    pub local_port: u16,
    pub remote_port: u16,
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    pub window: u16,
    /// MSS option to put on a SYN, 0 for none.
    pub mss: u16,
    pub sack: Sack,
    pub payload: &'a [u8],
}

pub trait Sink {
    fn transmit(&mut self, segment: &Outgoing) -> bool;
}

/// The segment size to use given the peer's MSS option (RFC 9293: assume 536
/// when there is none).
fn negotiated_mss(peer: u16) -> usize {
    if peer == 0 {
        536
    } else {
        (peer as usize).clamp(128, MSS)
    }
}

/// The largest segment payload the path to `remote` carries.
fn path_limit(remote: &Address) -> usize {
    if ip::is_v4(remote) { MSS } else { MSS_V6 }
}

fn seq_lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

fn seq_le(a: u32, b: u32) -> bool {
    a == b || seq_lt(a, b)
}

#[derive(Clone, Copy)]
struct Ring {
    data: [u8; BUFFER],
    head: usize,
    len: usize,
}

impl Ring {
    const EMPTY: Self = Self {
        data: [0; BUFFER],
        head: 0,
        len: 0,
    };

    fn free(&self) -> usize {
        BUFFER - self.len
    }

    fn push(&mut self, bytes: &[u8]) -> usize {
        let count = bytes.len().min(self.free());
        let tail = (self.head + self.len) % BUFFER;
        let first = (BUFFER - tail).min(count);
        self.data[tail..tail + first].copy_from_slice(&bytes[..first]);
        self.data[..count - first].copy_from_slice(&bytes[first..count]);
        self.len += count;
        count
    }

    /// Stores `bytes` `offset` bytes past the end of the data without
    /// counting them (out-of-order data waiting for the gap to fill).
    fn poke(&mut self, offset: usize, bytes: &[u8]) -> bool {
        if offset + bytes.len() > self.free() {
            return false;
        }
        let start = (self.head + self.len + offset) % BUFFER;
        let first = (BUFFER - start).min(bytes.len());
        self.data[start..start + first].copy_from_slice(&bytes[..first]);
        self.data[..bytes.len() - first].copy_from_slice(&bytes[first..]);
        true
    }

    fn peek(&self, offset: usize, out: &mut [u8]) -> usize {
        if offset >= self.len {
            return 0;
        }
        let count = out.len().min(self.len - offset);
        let start = (self.head + offset) % BUFFER;
        let first = (BUFFER - start).min(count);
        out[..first].copy_from_slice(&self.data[start..start + first]);
        out[first..count].copy_from_slice(&self.data[..count - first]);
        count
    }

    fn consume(&mut self, count: usize) {
        let count = count.min(self.len);
        self.head = (self.head + count) % BUFFER;
        self.len -= count;
    }
}

#[derive(Clone, Copy)]
struct Tcb {
    used: bool,
    state: State,
    parent: Option<usize>,
    accepted: bool,
    orphan: bool,
    local_port: u16,
    remote: Address,
    remote_port: u16,
    snd_una: u32,
    snd_nxt: u32,
    snd_max: u32,
    snd_wnd: u32,
    rcv_nxt: u32,
    send: Ring,
    recv: Ring,
    cwnd: u32,
    ssthresh: u32,
    dup_acks: u32,
    in_recovery: bool,
    recover: u32,
    rto: u64,
    srtt: u64,
    rttvar: u64,
    rtt_seq: u32,
    rtt_start: u64,
    timer: u64,
    retries: u32,
    time_wait_until: u64,
    fin_queued: bool,
    fin_sent: bool,
    fin_seq: Option<u32>,
    peer_closed: bool,
    error: u64,
    advertised: usize,
    /// Largest segment payload the peer accepts.
    mss: usize,
    /// MSS option sent on our SYN or SYN-ACK.
    own_mss: u16,
    retransmits: u32,
    fast_retransmits: u32,
    timeouts: u32,
    /// A listener that also takes IPv6 peers (an `AF_INET6` socket).
    dual: bool,
    options: Options,
    /// Whether we offer SACK on a SYN.
    own_sack: bool,
    /// Both ends agreed on SACK.
    sack_ok: bool,
    /// Received ranges beyond `rcv_nxt`, the most recent first.
    ooo: [(u32, u32); MAX_SACK],
    ooo_count: usize,
    /// Ranges the peer reports having, kept sorted and merged.
    scoreboard: [(u32, u32); SCOREBOARD],
    score_count: usize,
    /// Next sequence to consider when retransmitting holes.
    rexmit_next: u32,
    sack_retransmits: u32,
}

impl Tcb {
    const EMPTY: Self = Self {
        used: false,
        state: State::Closed,
        parent: None,
        accepted: false,
        orphan: false,
        local_port: 0,
        remote: [0; 16],
        remote_port: 0,
        snd_una: 0,
        snd_nxt: 0,
        snd_max: 0,
        snd_wnd: 0,
        rcv_nxt: 0,
        send: Ring::EMPTY,
        recv: Ring::EMPTY,
        cwnd: INITIAL_CWND,
        ssthresh: MAX_CWND,
        dup_acks: 0,
        in_recovery: false,
        recover: 0,
        rto: INITIAL_RTO_NS,
        srtt: 0,
        rttvar: 0,
        rtt_seq: 0,
        rtt_start: 0,
        timer: 0,
        retries: 0,
        time_wait_until: 0,
        fin_queued: false,
        fin_sent: false,
        fin_seq: None,
        peer_closed: false,
        error: 0,
        advertised: 0,
        mss: 536,
        own_mss: MSS as u16,
        retransmits: 0,
        fast_retransmits: 0,
        timeouts: 0,
        dual: false,
        options: Options::DEFAULT,
        own_sack: false,
        sack_ok: false,
        ooo: [(0, 0); MAX_SACK],
        ooo_count: 0,
        scoreboard: [(0, 0); SCOREBOARD],
        score_count: 0,
        rexmit_next: 0,
        sack_retransmits: 0,
    };

    fn emit(&mut self, sink: &mut dyn Sink, seq: u32, flags: u8, payload: &[u8]) -> bool {
        let window = self.recv.free().min(65_535);
        self.advertised = window;
        let mut sack = Sack::default();
        if flags & SYN != 0 {
            sack.permitted = self.own_sack && (self.state == State::SynSent || self.sack_ok);
        } else if self.sack_ok && self.ooo_count > 0 && flags & ACK != 0 {
            sack.count = self.ooo_count;
            sack.blocks = self.ooo;
        }
        sink.transmit(&Outgoing {
            remote: self.remote,
            local_port: self.local_port,
            remote_port: self.remote_port,
            seq,
            ack: self.rcv_nxt,
            flags,
            window: window as u16,
            mss: if flags & SYN != 0 { self.own_mss } else { 0 },
            sack,
            payload,
        })
    }

    /// Keeps a segment that starts beyond `rcv_nxt` and remembers its range.
    fn note_out_of_order(&mut self, seq: u32, payload: &[u8]) {
        let offset = seq.wrapping_sub(self.rcv_nxt) as usize;
        if offset >= self.recv.free() {
            return;
        }
        let fit = payload.len().min(self.recv.free() - offset);
        if fit == 0 || !self.recv.poke(offset, &payload[..fit]) {
            return;
        }
        let (mut start, mut end) = (seq, seq.wrapping_add(fit as u32));
        let mut kept = [(0u32, 0u32); MAX_SACK];
        let mut kept_count = 0;
        for &(first, last) in &self.ooo[..self.ooo_count] {
            if seq_le(first, end) && seq_le(start, last) {
                if seq_lt(first, start) {
                    start = first;
                }
                if seq_lt(end, last) {
                    end = last;
                }
            } else {
                kept[kept_count] = (first, last);
                kept_count += 1;
            }
        }
        self.ooo[0] = (start, end);
        let mut count = 1;
        for &range in &kept[..kept_count] {
            if count < MAX_SACK {
                self.ooo[count] = range;
                count += 1;
            }
        }
        self.ooo_count = count;
    }

    /// Delivers out-of-order data that is now contiguous with `rcv_nxt`.
    fn merge_out_of_order(&mut self) {
        let mut index = 0;
        while index < self.ooo_count {
            let (start, end) = self.ooo[index];
            let stale = seq_le(end, self.rcv_nxt);
            if stale || seq_le(start, self.rcv_nxt) {
                if !stale {
                    let advance = (end.wrapping_sub(self.rcv_nxt) as usize).min(self.recv.free());
                    self.recv.len += advance;
                    if self.orphan {
                        self.recv.consume(advance);
                    }
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(advance as u32);
                }
                self.ooo.copy_within(index + 1..self.ooo_count, index);
                self.ooo_count -= 1;
                index = 0;
            } else {
                index += 1;
            }
        }
    }

    fn add_sacked(&mut self, start: u32, end: u32) {
        let (mut start, mut end) = (start, end);
        let mut kept = [(0u32, 0u32); SCOREBOARD];
        let mut kept_count = 0;
        for &(first, last) in &self.scoreboard[..self.score_count] {
            if seq_le(first, end) && seq_le(start, last) {
                if seq_lt(first, start) {
                    start = first;
                }
                if seq_lt(end, last) {
                    end = last;
                }
            } else {
                kept[kept_count] = (first, last);
                kept_count += 1;
            }
        }
        if kept_count == SCOREBOARD {
            kept_count -= 1;
        }
        kept[kept_count] = (start, end);
        kept_count += 1;
        kept[..kept_count].sort_unstable_by_key(|&(first, _)| first.wrapping_sub(self.snd_una));
        self.scoreboard = kept;
        self.score_count = kept_count;
    }

    fn read_sack(&mut self, sack: &Sack) {
        for &(start, end) in &sack.blocks[..sack.count.min(MAX_SACK)] {
            if seq_lt(start, end)
                && seq_lt(self.snd_una, end)
                && seq_le(end, self.snd_max)
                && seq_le(self.snd_una, start)
            {
                self.add_sacked(start, end);
            }
        }
    }

    fn trim_scoreboard(&mut self) {
        let mut kept = 0;
        for index in 0..self.score_count {
            let (start, end) = self.scoreboard[index];
            if seq_le(end, self.snd_una) {
                continue;
            }
            self.scoreboard[kept] = if seq_lt(start, self.snd_una) {
                (self.snd_una, end)
            } else {
                (start, end)
            };
            kept += 1;
        }
        self.score_count = kept;
    }

    fn sacked_bytes(&self) -> u32 {
        self.scoreboard[..self.score_count]
            .iter()
            .map(|&(start, end)| end.wrapping_sub(start))
            .sum()
    }

    /// Resends up to `limit` segments from the holes below the highest
    /// range the peer has reported.
    fn retransmit_holes(&mut self, sink: &mut dyn Sink, limit: usize) {
        let Some(&(_, high)) = self.scoreboard[..self.score_count].last() else {
            return;
        };
        let mut sent = 0;
        while sent < limit {
            let mut hole = if seq_lt(self.rexmit_next, self.snd_una) {
                self.snd_una
            } else {
                self.rexmit_next
            };
            for &(start, end) in &self.scoreboard[..self.score_count] {
                if seq_le(start, hole) && seq_lt(hole, end) {
                    hole = end;
                }
            }
            if !seq_lt(hole, high) {
                return;
            }
            let hole_end = self.scoreboard[..self.score_count]
                .iter()
                .map(|&(start, _)| start)
                .find(|&start| seq_lt(hole, start))
                .unwrap_or(high);
            let offset = hole.wrapping_sub(self.snd_una) as usize;
            if offset >= self.send.len {
                return;
            }
            let length = (hole_end.wrapping_sub(hole) as usize)
                .min(self.mss)
                .min(self.send.len - offset);
            let mut chunk = [0u8; MSS];
            let copied = self.send.peek(offset, &mut chunk[..length]);
            if copied == 0 || !self.emit(sink, hole, ACK, &chunk[..copied]) {
                return;
            }
            self.rexmit_next = hole.wrapping_add(copied as u32);
            self.retransmits += 1;
            self.sack_retransmits += 1;
            sent += 1;
        }
    }

    fn send_ack(&mut self, sink: &mut dyn Sink) {
        let seq = self.snd_nxt;
        self.emit(sink, seq, ACK, &[]);
    }

    fn flight(&self) -> u32 {
        self.snd_nxt.wrapping_sub(self.snd_una)
    }

    fn data_in_flight(&self) -> usize {
        (self.flight() as usize)
            .saturating_sub(self.fin_sent as usize)
            .min(self.send.len)
    }

    fn arm(&mut self, now: u64) {
        self.timer = now.saturating_add(self.rto);
    }

    fn close_connection(&mut self, error: u64) {
        if error != 0 {
            self.error = error;
        }
        self.state = State::Closed;
        self.timer = 0;
        if self.orphan || self.parent.is_some() && !self.accepted {
            *self = Self::EMPTY;
        }
    }

    fn sample_rtt(&mut self, ack: u32, now: u64) {
        if self.rtt_start == 0 || !seq_lt(self.rtt_seq, ack) {
            return;
        }
        let sample = now.saturating_sub(self.rtt_start).max(1);
        self.rtt_start = 0;
        if self.srtt == 0 {
            self.srtt = sample;
            self.rttvar = sample / 2;
        } else {
            let delta = self.srtt.abs_diff(sample);
            self.rttvar = (3 * self.rttvar + delta) / 4;
            self.srtt = (7 * self.srtt + sample) / 8;
        }
        self.rto = (self.srtt + (4 * self.rttvar).max(1_000_000)).clamp(MIN_RTO_NS, MAX_RTO_NS);
    }

    /// Sends whatever the congestion and receive windows allow, then the FIN
    /// once every byte has gone out.
    fn output(&mut self, now: u64, sink: &mut dyn Sink) {
        if !matches!(
            self.state,
            State::Established
                | State::CloseWait
                | State::FinWait1
                | State::LastAck
                | State::Closing
        ) {
            return;
        }
        let mut chunk = [0u8; MSS];
        loop {
            let flight = self.flight() as usize;
            let sent = self.data_in_flight();
            let queued = self.send.len - sent;
            if queued > 0 && !self.fin_sent {
                let window = (self.cwnd.min(self.snd_wnd)) as usize;
                let pipe = if self.in_recovery && self.sack_ok {
                    flight.saturating_sub(self.sacked_bytes() as usize)
                } else {
                    flight
                };
                let mut allowed = window.saturating_sub(pipe);
                if self.snd_wnd == 0 && flight == 0 {
                    allowed = 1;
                }
                let count = queued.min(self.mss).min(allowed);
                if count == 0 {
                    break;
                }
                let copied = self.send.peek(sent, &mut chunk[..count]);
                let flags = if copied == queued { ACK | PSH } else { ACK };
                let seq = self.snd_nxt;
                if !self.emit(sink, seq, flags, &chunk[..copied]) {
                    break;
                }
                if self.rtt_start == 0 {
                    self.rtt_start = now;
                    self.rtt_seq = seq;
                }
                self.snd_nxt = self.snd_nxt.wrapping_add(copied as u32);
                if seq_lt(self.snd_max, self.snd_nxt) {
                    self.snd_max = self.snd_nxt;
                }
                if self.timer == 0 {
                    self.arm(now);
                }
                continue;
            }
            if queued == 0 && self.fin_queued && !self.fin_sent {
                let seq = self.snd_nxt;
                if !self.emit(sink, seq, FIN | ACK, &[]) {
                    break;
                }
                self.fin_seq = Some(seq);
                self.fin_sent = true;
                self.snd_nxt = self.snd_nxt.wrapping_add(1);
                if seq_lt(self.snd_max, self.snd_nxt) {
                    self.snd_max = self.snd_nxt;
                }
                match self.state {
                    State::Established => self.state = State::FinWait1,
                    State::CloseWait => self.state = State::LastAck,
                    _ => {}
                }
                if self.timer == 0 {
                    self.arm(now);
                }
            }
            break;
        }
    }

    fn retransmit_first(&mut self, sink: &mut dyn Sink) {
        let sent = self.data_in_flight();
        if sent > 0 {
            let mut chunk = [0u8; MSS];
            let count = sent.min(self.mss);
            let copied = self.send.peek(0, &mut chunk[..count]);
            let seq = self.snd_una;
            self.emit(sink, seq, ACK, &chunk[..copied]);
        } else if let Some(seq) = self.fin_seq {
            self.emit(sink, seq, FIN | ACK, &[]);
        }
        self.rtt_start = 0;
        self.retransmits += 1;
    }

    fn handle_timeout(&mut self, now: u64, sink: &mut dyn Sink) {
        if self.timer == 0 || now < self.timer {
            return;
        }
        match self.state {
            State::SynSent => {
                self.retries += 1;
                if self.retries > MAX_SYN_RETRIES {
                    self.close_connection(ETIMEDOUT);
                    return;
                }
                self.rto = (self.rto * 2).min(MAX_RTO_NS);
                let seq = self.snd_una;
                self.emit(sink, seq, SYN, &[]);
                self.retransmits += 1;
                self.arm(now);
            }
            State::SynReceived => {
                self.retries += 1;
                if self.retries > MAX_SYN_RETRIES {
                    self.close_connection(ETIMEDOUT);
                    return;
                }
                self.rto = (self.rto * 2).min(MAX_RTO_NS);
                let seq = self.snd_una;
                self.emit(sink, seq, SYN | ACK, &[]);
                self.retransmits += 1;
                self.arm(now);
            }
            State::Established
            | State::CloseWait
            | State::FinWait1
            | State::Closing
            | State::LastAck => {
                if self.flight() == 0 {
                    self.timer = 0;
                    return;
                }
                if self.snd_wnd != 0 {
                    self.retries += 1;
                    if self.retries > MAX_RETRIES {
                        let seq = self.snd_nxt;
                        self.emit(sink, seq, RST | ACK, &[]);
                        self.close_connection(ETIMEDOUT);
                        return;
                    }
                    let flight = self.flight().max(2 * MSS as u32);
                    self.ssthresh = (flight / 2).max(2 * MSS as u32);
                    self.cwnd = MSS as u32;
                    self.rto = (self.rto * 2).min(MAX_RTO_NS);
                }
                self.timeouts += 1;
                self.dup_acks = 0;
                self.in_recovery = false;
                self.score_count = 0;
                self.snd_nxt = self.snd_una;
                self.fin_sent = false;
                self.rtt_start = 0;
                self.retransmits += 1;
                self.arm(now);
                self.output(now, sink);
            }
            State::TimeWait | State::FinWait2 => {}
            State::Closed | State::Listen => self.timer = 0,
        }
    }

    fn process_ack(&mut self, segment: &Incoming, now: u64, sink: &mut dyn Sink) -> bool {
        let ack = segment.ack;
        let mut fin_acked_now = false;
        if self.sack_ok {
            self.read_sack(&segment.sack);
        }
        if seq_lt(self.snd_una, ack) && seq_le(ack, self.snd_max) {
            let advanced = ack.wrapping_sub(self.snd_una);
            let fin_acked = self.fin_seq.is_some_and(|fin| ack == fin.wrapping_add(1));
            let data_acked = advanced - fin_acked as u32;
            self.send.consume(data_acked as usize);
            self.snd_una = ack;
            if seq_lt(self.snd_nxt, ack) {
                self.snd_nxt = ack;
                if fin_acked {
                    self.fin_sent = true;
                }
            }
            self.dup_acks = 0;
            self.retries = 0;
            self.sample_rtt(ack, now);
            self.trim_scoreboard();
            if self.in_recovery {
                if seq_le(self.recover, ack) {
                    self.in_recovery = false;
                    self.score_count = 0;
                    self.cwnd = self.ssthresh;
                } else {
                    self.cwnd = self
                        .cwnd
                        .saturating_sub(data_acked)
                        .saturating_add(MSS as u32);
                    if self.sack_ok {
                        self.retransmit_holes(sink, 2);
                    }
                }
            } else if self.cwnd < self.ssthresh {
                self.cwnd = (self.cwnd + data_acked.min(MSS as u32)).min(MAX_CWND);
            } else {
                let increase = ((MSS * MSS) as u32 / self.cwnd.max(1)).max(1);
                self.cwnd = (self.cwnd + increase).min(MAX_CWND);
            }
            if self.snd_una == self.snd_nxt {
                self.timer = 0;
            } else {
                self.arm(now);
            }
            fin_acked_now = fin_acked;
        } else if ack == self.snd_una
            && segment.payload.is_empty()
            && segment.flags & (SYN | FIN) == 0
            && self.flight() > 0
            && self.snd_wnd != 0
            && segment.window as u32 == self.snd_wnd
        {
            self.dup_acks += 1;
            if self.dup_acks == 3 && !self.in_recovery {
                let flight = self.flight().max(2 * MSS as u32);
                self.ssthresh = (flight / 2).max(2 * MSS as u32);
                self.cwnd = self.ssthresh + 3 * MSS as u32;
                self.in_recovery = true;
                self.recover = self.snd_max;
                self.fast_retransmits += 1;
                self.rtt_start = 0;
                self.arm(now);
                if self.sack_ok && self.score_count > 0 {
                    self.rexmit_next = self.snd_una;
                    self.retransmit_holes(sink, 2);
                    if self.rexmit_next == self.snd_una {
                        self.retransmit_first(sink);
                    }
                } else {
                    self.retransmits += 1;
                    self.snd_nxt = self.snd_una;
                    self.fin_sent = false;
                }
            } else if self.in_recovery {
                self.cwnd = (self.cwnd + MSS as u32).min(MAX_CWND);
                if self.sack_ok {
                    self.retransmit_holes(sink, 2);
                }
            }
        }
        if !seq_lt(ack, self.snd_una) {
            self.snd_wnd = segment.window as u32;
        }
        fin_acked_now
    }

    fn input(&mut self, segment: &Incoming, now: u64, sink: &mut dyn Sink) {
        match self.state {
            State::SynSent => {
                let expected = self.snd_una.wrapping_add(1);
                if segment.flags & RST != 0 {
                    if segment.flags & ACK != 0 && segment.ack == expected {
                        self.close_connection(ECONNREFUSED);
                    }
                    return;
                }
                if segment.flags & (SYN | ACK) == SYN | ACK && segment.ack == expected {
                    self.rcv_nxt = segment.seq.wrapping_add(1);
                    self.mss = negotiated_mss(segment.mss).min(path_limit(&self.remote));
                    self.sack_ok = self.own_sack && segment.sack.permitted;
                    self.snd_una = expected;
                    self.snd_nxt = expected;
                    self.snd_max = expected;
                    self.snd_wnd = segment.window as u32;
                    self.state = State::Established;
                    self.retries = 0;
                    self.timer = 0;
                    self.sample_rtt_handshake(now);
                    self.send_ack(sink);
                    self.output(now, sink);
                }
                return;
            }
            State::SynReceived => {
                if segment.flags & RST != 0 {
                    self.close_connection(ECONNRESET);
                    return;
                }
                if segment.flags & SYN != 0 && segment.flags & ACK == 0 {
                    let seq = self.snd_una;
                    self.emit(sink, seq, SYN | ACK, &[]);
                    return;
                }
                if segment.flags & ACK != 0 && segment.ack == self.snd_una.wrapping_add(1) {
                    self.snd_una = segment.ack;
                    self.snd_wnd = segment.window as u32;
                    self.state = State::Established;
                    self.retries = 0;
                    self.timer = 0;
                } else {
                    return;
                }
            }
            State::Closed | State::Listen => return,
            _ => {}
        }

        if segment.flags & RST != 0 {
            let in_window = seq_le(self.rcv_nxt, segment.seq)
                && seq_lt(
                    segment.seq,
                    self.rcv_nxt.wrapping_add(self.recv.free().max(1) as u32),
                );
            if in_window {
                self.close_connection(ECONNRESET);
            }
            return;
        }
        if segment.flags & SYN != 0 {
            self.send_ack(sink);
            return;
        }
        if segment.flags & ACK == 0 {
            return;
        }

        let fin_acked = self.process_ack(segment, now, sink);
        if fin_acked {
            match self.state {
                State::FinWait1 => self.state = State::FinWait2,
                State::Closing => self.enter_time_wait(now),
                State::LastAck => {
                    self.close_connection(0);
                    return;
                }
                _ => {}
            }
        }

        let mut needs_ack = false;
        let mut payload = segment.payload;
        let mut seq = segment.seq;
        let mut truncated = false;
        let can_receive = matches!(
            self.state,
            State::Established | State::FinWait1 | State::FinWait2
        ) && !self.peer_closed;
        if !payload.is_empty() || segment.flags & FIN != 0 {
            needs_ack = true;
            if can_receive {
                if seq_lt(seq, self.rcv_nxt) {
                    let skip = self.rcv_nxt.wrapping_sub(seq) as usize;
                    if skip >= payload.len() {
                        payload = &[];
                    } else {
                        payload = &payload[skip..];
                    }
                    seq = self.rcv_nxt;
                }
                if seq_lt(self.rcv_nxt, seq) && !payload.is_empty() {
                    self.note_out_of_order(seq, payload);
                }
                if seq == self.rcv_nxt {
                    if !payload.is_empty() {
                        let taken = self.recv.push(payload);
                        self.rcv_nxt = self.rcv_nxt.wrapping_add(taken as u32);
                        if self.orphan {
                            self.recv.consume(taken);
                        }
                        truncated = taken < payload.len();
                    }
                    if !truncated
                        && segment.flags & FIN != 0
                        && seq.wrapping_add(payload.len() as u32) == self.rcv_nxt
                    {
                        self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                        self.peer_closed = true;
                        match self.state {
                            State::Established => self.state = State::CloseWait,
                            State::FinWait1 => self.state = State::Closing,
                            State::FinWait2 => self.enter_time_wait(now),
                            _ => {}
                        }
                    }
                    self.merge_out_of_order();
                }
            }
        }
        if needs_ack {
            self.send_ack(sink);
        }
        if self.state != State::Closed {
            self.output(now, sink);
        }
    }

    fn sample_rtt_handshake(&mut self, now: u64) {
        if self.rtt_start != 0 && self.retries == 0 {
            let sample = now.saturating_sub(self.rtt_start).max(1);
            self.srtt = sample;
            self.rttvar = sample / 2;
            self.rto = (self.srtt + (4 * self.rttvar).max(1_000_000)).clamp(MIN_RTO_NS, MAX_RTO_NS);
        }
        self.rtt_start = 0;
    }

    fn enter_time_wait(&mut self, now: u64) {
        self.state = State::TimeWait;
        self.timer = 0;
        self.time_wait_until = now.saturating_add(TIME_WAIT_NS);
    }
}

pub struct Tcp {
    tcbs: [Tcb; SOCKETS],
    next_port: u16,
    local_mss: u16,
    sack: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Counters {
    pub retransmits: u32,
    pub fast_retransmits: u32,
    pub timeouts: u32,
}

impl Tcp {
    pub const fn new() -> Self {
        Self {
            tcbs: [Tcb::EMPTY; SOCKETS],
            next_port: 49_152,
            local_mss: MSS as u16,
            sack: true,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// The MSS advertised on connections made from now on.
    pub fn set_local_mss(&mut self, mss: u16) {
        self.local_mss = mss;
    }

    /// Whether connections made from now on offer selective acknowledgements.
    pub fn set_sack(&mut self, enabled: bool) {
        self.sack = enabled;
    }

    pub fn options(&self, index: usize) -> Options {
        self.tcbs
            .get(index)
            .filter(|tcb| tcb.used)
            .map_or(Options::DEFAULT, |tcb| tcb.options)
    }

    pub fn set_options(&mut self, index: usize, options: Options) {
        if let Some(tcb) = self.tcbs.get_mut(index).filter(|tcb| tcb.used) {
            tcb.options = options;
        }
    }

    /// Bytes waiting to be read.
    pub fn available(&self, index: usize) -> usize {
        self.tcbs
            .get(index)
            .filter(|tcb| tcb.used)
            .map_or(0, |tcb| tcb.recv.len)
    }

    /// Lets a socket created for `AF_INET6` accept IPv6 peers.
    pub fn set_dual(&mut self, index: usize, dual: bool) {
        if let Some(tcb) = self.tcbs.get_mut(index) {
            tcb.dual = dual;
        }
    }

    /// Segments resent by the SACK scoreboard on a connection.
    pub fn sack_retransmits(&self, index: usize) -> u32 {
        self.tcbs.get(index).map_or(0, |tcb| tcb.sack_retransmits)
    }

    pub fn sack_negotiated(&self, index: usize) -> bool {
        self.tcbs.get(index).is_some_and(|tcb| tcb.sack_ok)
    }

    /// A free slot, or, when the table is full, one still waiting out
    /// `TIME_WAIT` (the connection is already finished; only the late-segment
    /// guard is lost).
    pub fn socket(&mut self) -> Option<usize> {
        let index = self.tcbs.iter().position(|tcb| !tcb.used).or_else(|| {
            self.tcbs
                .iter()
                .position(|tcb| tcb.state == State::TimeWait && tcb.orphan)
        })?;
        self.tcbs[index] = Tcb {
            used: true,
            own_mss: self.local_mss,
            own_sack: self.sack,
            ..Tcb::EMPTY
        };
        Some(index)
    }

    fn port_in_use(&self, port: u16) -> bool {
        self.tcbs
            .iter()
            .any(|tcb| tcb.used && tcb.local_port == port && tcb.state != State::Closed)
    }

    pub fn bind(&mut self, index: usize, port: u16) -> Result<(), u64> {
        let tcb = self.tcbs.get(index).filter(|tcb| tcb.used).ok_or(EINVAL)?;
        if tcb.state != State::Closed || tcb.local_port != 0 {
            return Err(EINVAL);
        }
        let reuse = tcb.options.reuse_address;
        if port != 0
            && self.tcbs.iter().enumerate().any(|(other, tcb)| {
                other != index
                    && tcb.used
                    && tcb.local_port == port
                    && (!reuse || tcb.state == State::Listen)
            })
        {
            return Err(EADDRINUSE);
        }
        self.tcbs[index].local_port = port;
        Ok(())
    }

    pub fn local_port(&self, index: usize) -> Option<u16> {
        self.tcbs
            .get(index)
            .filter(|tcb| tcb.used)
            .map(|tcb| tcb.local_port)
    }

    pub fn peer(&self, index: usize) -> Option<(Address, u16)> {
        self.tcbs
            .get(index)
            .filter(|tcb| tcb.used && tcb.remote_port != 0)
            .map(|tcb| (tcb.remote, tcb.remote_port))
    }

    /// State, local port, peer and peer port of a socket, for `netstat`.
    pub fn summary(&self, index: usize) -> Option<(State, u16, Address, u16)> {
        let tcb = self.tcbs.get(index).filter(|tcb| tcb.used)?;
        Some((tcb.state, tcb.local_port, tcb.remote, tcb.remote_port))
    }

    pub fn state(&self, index: usize) -> Option<State> {
        self.tcbs
            .get(index)
            .filter(|tcb| tcb.used)
            .map(|tcb| tcb.state)
    }

    pub fn error(&self, index: usize) -> u64 {
        self.tcbs.get(index).map_or(0, |tcb| tcb.error)
    }

    pub fn counters(&self, index: usize) -> Option<Counters> {
        self.tcbs
            .get(index)
            .filter(|tcb| tcb.used)
            .map(|tcb| Counters {
                retransmits: tcb.retransmits,
                fast_retransmits: tcb.fast_retransmits,
                timeouts: tcb.timeouts,
            })
    }

    pub fn congestion(&self, index: usize) -> Option<(u32, u32)> {
        self.tcbs
            .get(index)
            .filter(|tcb| tcb.used)
            .map(|tcb| (tcb.cwnd, tcb.ssthresh))
    }

    pub fn take_error(&mut self, index: usize) -> u64 {
        self.tcbs
            .get_mut(index)
            .map_or(0, |tcb| core::mem::take(&mut tcb.error))
    }

    pub fn listen(&mut self, index: usize) -> Result<(), u64> {
        if self
            .tcbs
            .get(index)
            .is_some_and(|tcb| tcb.used && tcb.state == State::Closed && tcb.local_port == 0)
        {
            let port = self.allocate_port();
            self.tcbs[index].local_port = port;
        }
        let port = self
            .tcbs
            .get(index)
            .filter(|tcb| tcb.used && tcb.state == State::Closed && tcb.local_port != 0)
            .map(|tcb| tcb.local_port)
            .ok_or(EINVAL)?;
        if self
            .tcbs
            .iter()
            .any(|tcb| tcb.used && tcb.state == State::Listen && tcb.local_port == port)
        {
            return Err(EADDRINUSE);
        }
        self.tcbs[index].state = State::Listen;
        Ok(())
    }

    pub fn accept(&mut self, listener: usize) -> Option<usize> {
        if self.tcbs.get(listener)?.state != State::Listen {
            return None;
        }
        let child = self.tcbs.iter().position(|tcb| {
            tcb.used
                && tcb.parent == Some(listener)
                && !tcb.accepted
                && tcb.state != State::SynReceived
        })?;
        self.tcbs[child].accepted = true;
        Some(child)
    }

    pub fn connect(
        &mut self,
        index: usize,
        remote: Address,
        remote_port: u16,
        now: u64,
        sink: &mut dyn Sink,
    ) -> Result<(), u64> {
        if remote_port == 0 {
            return Err(EINVAL);
        }
        let tcb = self.tcbs.get(index).filter(|tcb| tcb.used).ok_or(EINVAL)?;
        match tcb.state {
            State::Closed => {}
            State::SynSent => return Err(EAGAIN),
            _ => return Err(EISCONN),
        }
        if tcb.error != 0 {
            let error = tcb.error;
            self.tcbs[index].error = 0;
            return Err(error);
        }
        if self.tcbs[index].local_port == 0 {
            let port = self.allocate_port();
            self.tcbs[index].local_port = port;
        }
        let iss = initial_sequence(now, self.tcbs[index].local_port, remote_port);
        let tcb = &mut self.tcbs[index];
        tcb.remote = remote;
        tcb.remote_port = remote_port;
        tcb.own_mss = tcb.own_mss.min(path_limit(&remote) as u16);
        tcb.snd_una = iss;
        tcb.snd_nxt = iss.wrapping_add(1);
        tcb.snd_max = tcb.snd_nxt;
        tcb.state = State::SynSent;
        tcb.rtt_start = now.max(1);
        tcb.emit(sink, iss, SYN, &[]);
        tcb.arm(now);
        Ok(())
    }

    fn allocate_port(&mut self) -> u16 {
        loop {
            let port = self.next_port;
            self.next_port = if port == 65_535 { 49_152 } else { port + 1 };
            if !self.port_in_use(port) {
                return port;
            }
        }
    }

    pub fn write(
        &mut self,
        index: usize,
        data: &[u8],
        now: u64,
        sink: &mut dyn Sink,
    ) -> Result<usize, u64> {
        let tcb = self
            .tcbs
            .get_mut(index)
            .filter(|tcb| tcb.used)
            .ok_or(EINVAL)?;
        if tcb.error != 0 {
            return Err(tcb.error);
        }
        match tcb.state {
            State::Established | State::CloseWait => {}
            State::SynSent | State::SynReceived => return Err(EAGAIN),
            State::Closed | State::Listen => return Err(ENOTCONN),
            _ => return Err(EPIPE),
        }
        if tcb.fin_queued {
            return Err(EPIPE);
        }
        if data.is_empty() {
            return Ok(0);
        }
        let taken = tcb.send.push(data);
        if taken == 0 {
            return Err(EAGAIN);
        }
        tcb.output(now, sink);
        Ok(taken)
    }

    pub fn read(
        &mut self,
        index: usize,
        buffer: &mut [u8],
        sink: &mut dyn Sink,
    ) -> Result<usize, u64> {
        let tcb = self
            .tcbs
            .get_mut(index)
            .filter(|tcb| tcb.used)
            .ok_or(EINVAL)?;
        if tcb.recv.len > 0 {
            let count = tcb.recv.peek(0, buffer);
            tcb.recv.consume(count);
            let free = tcb.recv.free();
            if matches!(
                tcb.state,
                State::Established | State::FinWait1 | State::FinWait2
            ) && free >= tcb.advertised + MSS.min(BUFFER / 2)
            {
                tcb.send_ack(sink);
            }
            return Ok(count);
        }
        no_data(tcb)
    }

    /// Like `read`, but leaves the data in place.
    pub fn peek(&mut self, index: usize, buffer: &mut [u8]) -> Result<usize, u64> {
        let tcb = self
            .tcbs
            .get_mut(index)
            .filter(|tcb| tcb.used)
            .ok_or(EINVAL)?;
        if tcb.recv.len > 0 {
            return Ok(tcb.recv.peek(0, buffer));
        }
        no_data(tcb)
    }

    pub fn shutdown_write(&mut self, index: usize, now: u64, sink: &mut dyn Sink) {
        if let Some(tcb) = self.tcbs.get_mut(index).filter(|tcb| tcb.used) {
            match tcb.state {
                State::Established | State::CloseWait => {
                    tcb.fin_queued = true;
                    tcb.output(now, sink);
                }
                _ => {}
            }
        }
    }

    pub fn close(&mut self, index: usize, now: u64, sink: &mut dyn Sink) {
        let Some(tcb) = self.tcbs.get(index).filter(|tcb| tcb.used).copied() else {
            return;
        };
        match tcb.state {
            State::Closed | State::SynSent => self.tcbs[index] = Tcb::EMPTY,
            State::Listen => {
                for child in self.tcbs.iter_mut() {
                    if child.used && child.parent == Some(index) {
                        if child.accepted {
                            child.parent = None;
                        } else {
                            *child = Tcb::EMPTY;
                        }
                    }
                }
                self.tcbs[index] = Tcb::EMPTY;
            }
            State::SynReceived | State::Established | State::CloseWait => {
                let tcb = &mut self.tcbs[index];
                tcb.orphan = true;
                tcb.recv.consume(tcb.recv.len);
                tcb.fin_queued = true;
                tcb.output(now, sink);
            }
            State::TimeWait => self.tcbs[index] = Tcb::EMPTY,
            _ => {
                self.tcbs[index].orphan = true;
            }
        }
    }

    /// Closes a connection abruptly: a reset goes to the peer and the socket
    /// is gone (a zero-time linger).
    pub fn abort(&mut self, index: usize, now: u64, sink: &mut dyn Sink) {
        let Some(tcb) = self.tcbs.get_mut(index).filter(|tcb| tcb.used) else {
            return;
        };
        if matches!(
            tcb.state,
            State::SynReceived
                | State::Established
                | State::FinWait1
                | State::FinWait2
                | State::CloseWait
                | State::Closing
                | State::LastAck
        ) {
            let seq = tcb.snd_nxt;
            tcb.emit(sink, seq, RST | ACK, &[]);
            self.tcbs[index] = Tcb::EMPTY;
        } else {
            self.close(index, now, sink);
        }
    }

    pub fn readable(&self, index: usize) -> bool {
        let Some(tcb) = self.tcbs.get(index).filter(|tcb| tcb.used) else {
            return false;
        };
        if tcb.state == State::Listen {
            return self.tcbs.iter().any(|child| {
                child.used
                    && child.parent == Some(index)
                    && !child.accepted
                    && child.state != State::SynReceived
            });
        }
        tcb.recv.len > 0 || tcb.peer_closed || tcb.error != 0
    }

    pub fn writable(&self, index: usize) -> bool {
        let Some(tcb) = self.tcbs.get(index).filter(|tcb| tcb.used) else {
            return false;
        };
        tcb.error != 0
            || (matches!(tcb.state, State::Established | State::CloseWait)
                && !tcb.fin_queued
                && tcb.send.free() > 0)
    }

    pub fn hung_up(&self, index: usize) -> bool {
        self.tcbs
            .get(index)
            .filter(|tcb| tcb.used)
            .is_some_and(|tcb| {
                tcb.error != 0
                    || matches!(
                        tcb.state,
                        State::Closed | State::TimeWait | State::LastAck | State::Closing
                    ) && tcb.remote_port != 0
            })
    }

    pub fn input(&mut self, segment: &Incoming, now: u64, sink: &mut dyn Sink) {
        let exact = self.tcbs.iter().position(|tcb| {
            tcb.used
                && !matches!(tcb.state, State::Listen)
                && tcb.local_port == segment.local_port
                && tcb.remote_port == segment.remote_port
                && tcb.remote == segment.remote
                && tcb.state != State::Closed
        });
        if let Some(index) = exact {
            self.tcbs[index].input(segment, now, sink);
            return;
        }
        let listener = self.tcbs.iter().position(|tcb| {
            tcb.used
                && tcb.state == State::Listen
                && tcb.local_port == segment.local_port
                && (tcb.dual || ip::is_v4(&segment.remote))
        });
        if let Some(listener) = listener
            && segment.flags & (SYN | ACK | RST) == SYN
        {
            let pending = self
                .tcbs
                .iter()
                .filter(|tcb| tcb.used && tcb.parent == Some(listener) && !tcb.accepted)
                .count();
            if pending >= BACKLOG {
                return;
            }
            let Some(child) = self.tcbs.iter().position(|tcb| !tcb.used) else {
                return;
            };
            let iss = initial_sequence(now, segment.local_port, segment.remote_port);
            let tcb = &mut self.tcbs[child];
            *tcb = Tcb {
                used: true,
                state: State::SynReceived,
                parent: Some(listener),
                local_port: segment.local_port,
                remote: segment.remote,
                remote_port: segment.remote_port,
                snd_una: iss,
                snd_nxt: iss.wrapping_add(1),
                snd_max: iss.wrapping_add(1),
                snd_wnd: segment.window as u32,
                rcv_nxt: segment.seq.wrapping_add(1),
                mss: negotiated_mss(segment.mss).min(path_limit(&segment.remote)),
                own_mss: self.local_mss.min(path_limit(&segment.remote) as u16),
                own_sack: self.sack,
                sack_ok: self.sack && segment.sack.permitted,
                ..Tcb::EMPTY
            };
            tcb.emit(sink, iss, SYN | ACK, &[]);
            tcb.arm(now);
            return;
        }
        if segment.flags & RST != 0 {
            return;
        }
        let (seq, ack, flags) = if segment.flags & ACK != 0 {
            (segment.ack, 0, RST)
        } else {
            let length = segment.payload.len() as u32
                + (segment.flags & SYN != 0) as u32
                + (segment.flags & FIN != 0) as u32;
            (0, segment.seq.wrapping_add(length), RST | ACK)
        };
        sink.transmit(&Outgoing {
            remote: segment.remote,
            local_port: segment.local_port,
            remote_port: segment.remote_port,
            seq,
            ack,
            flags,
            window: 0,
            mss: 0,
            sack: Sack::default(),
            payload: &[],
        });
    }

    pub fn tick(&mut self, now: u64, sink: &mut dyn Sink) {
        for tcb in self.tcbs.iter_mut() {
            if !tcb.used {
                continue;
            }
            if tcb.state == State::TimeWait && now >= tcb.time_wait_until {
                tcb.close_connection(0);
                continue;
            }
            tcb.handle_timeout(now, sink);
            if tcb.state == State::Established
                || matches!(tcb.state, State::CloseWait | State::FinWait1)
            {
                tcb.output(now, sink);
            }
        }
    }
}

/// What a read finds when no data is waiting: a pending error, end of
/// stream, nothing yet, or a socket that was never connected.
fn no_data(tcb: &mut Tcb) -> Result<usize, u64> {
    if tcb.error != 0 {
        let error = tcb.error;
        tcb.error = 0;
        return Err(error);
    }
    if tcb.peer_closed {
        return Ok(0);
    }
    match tcb.state {
        State::Established | State::SynSent | State::SynReceived => Err(EAGAIN),
        State::Closed | State::Listen => Err(ENOTCONN),
        _ => Ok(0),
    }
}

fn initial_sequence(now: u64, local_port: u16, remote_port: u16) -> u32 {
    let mixed = now / 4_000 + ((local_port as u64) << 16) + remote_port as u64;
    (mixed as u32).wrapping_mul(0x9e37_79b9) ^ 0x5bd1_e995
}

#[cfg(feature = "boot-test")]
pub use self::simulation::self_test;

#[cfg(feature = "boot-test")]
mod simulation {
    use super::*;
    use crate::sync::TicketLock;

    const CLIENT: Address = ip::v4([10, 0, 0, 1]);
    const SERVER: Address = ip::v4([10, 0, 0, 2]);
    const QUEUE: usize = 96;

    #[derive(Clone, Copy)]
    struct Frame {
        local_port: u16,
        remote_port: u16,
        seq: u32,
        ack: u32,
        flags: u8,
        window: u16,
        mss: u16,
        sack: Sack,
        length: usize,
        data: [u8; MSS],
    }

    impl Frame {
        const EMPTY: Self = Self {
            local_port: 0,
            remote_port: 0,
            seq: 0,
            ack: 0,
            flags: 0,
            window: 0,
            mss: 0,
            sack: Sack {
                permitted: false,
                count: 0,
                blocks: [(0, 0); MAX_SACK],
            },
            length: 0,
            data: [0; MSS],
        };
    }

    struct Wire {
        frames: [Frame; QUEUE],
        count: usize,
        data_frames: u32,
        max_payload: usize,
    }

    impl Wire {
        const EMPTY: Self = Self {
            frames: [Frame::EMPTY; QUEUE],
            count: 0,
            data_frames: 0,
            max_payload: 0,
        };
    }

    impl Sink for Wire {
        fn transmit(&mut self, segment: &Outgoing) -> bool {
            if self.count == QUEUE {
                return true;
            }
            let frame = &mut self.frames[self.count];
            frame.local_port = segment.local_port;
            frame.remote_port = segment.remote_port;
            frame.seq = segment.seq;
            frame.ack = segment.ack;
            frame.flags = segment.flags;
            frame.window = segment.window;
            frame.mss = segment.mss;
            frame.sack = segment.sack;
            frame.length = segment.payload.len();
            self.max_payload = self.max_payload.max(segment.payload.len());
            frame.data[..frame.length].copy_from_slice(segment.payload);
            self.count += 1;
            true
        }
    }

    struct Simulation {
        client: Tcp,
        server: Tcp,
        to_server: Wire,
        to_client: Wire,
    }

    static SIMULATION: TicketLock<Simulation> = TicketLock::new(Simulation {
        client: Tcp::new(),
        server: Tcp::new(),
        to_server: Wire::EMPTY,
        to_client: Wire::EMPTY,
    });

    #[derive(Clone, Copy)]
    enum Loss {
        None,
        EveryNthData(u32),
        OnlyDataFrame(u32),
        /// Every Nth data frame arrives twice, in both directions.
        Duplicate(u32),
        /// Frames sent in the same step arrive in reverse order, both ways.
        Reverse,
        /// `count` data frames in a row are lost, starting with number `first`.
        Burst(u32, u32),
    }

    impl Loss {
        fn backward(self) -> Self {
            match self {
                Loss::Duplicate(_) | Loss::Reverse => self,
                _ => Loss::None,
            }
        }
    }

    struct Outcome {
        completed: bool,
        client_counters: Counters,
        peak_cwnd: u32,
        low_ssthresh: u32,
        elapsed_ms: u64,
        max_payload: usize,
        selective: u32,
        negotiated: bool,
        upload_frames: u32,
    }

    fn pattern(offset: usize) -> u8 {
        (offset.wrapping_mul(31).wrapping_add(7)) as u8
    }

    fn deliver(
        wire: &mut Wire,
        target: &mut Tcp,
        from: Address,
        loss: Loss,
        now: u64,
        reply: &mut Wire,
    ) {
        let count = wire.count;
        wire.count = 0;
        for position in 0..count {
            let index = if matches!(loss, Loss::Reverse) {
                count - 1 - position
            } else {
                position
            };
            let frame = wire.frames[index];
            let mut copies = 1;
            if frame.length > 0 {
                wire.data_frames += 1;
                let drop = match loss {
                    Loss::EveryNthData(period) => wire.data_frames.is_multiple_of(period),
                    Loss::OnlyDataFrame(number) => wire.data_frames == number,
                    Loss::Burst(first, count) => {
                        wire.data_frames >= first && wire.data_frames < first + count
                    }
                    _ => false,
                };
                if drop {
                    continue;
                }
                if let Loss::Duplicate(period) = loss
                    && wire.data_frames.is_multiple_of(period)
                {
                    copies = 2;
                }
            }
            for _ in 0..copies {
                target.input(
                    &Incoming {
                        remote: from,
                        remote_port: frame.local_port,
                        local_port: frame.remote_port,
                        seq: frame.seq,
                        ack: frame.ack,
                        flags: frame.flags,
                        window: frame.window,
                        mss: frame.mss,
                        sack: frame.sack,
                        payload: &frame.data[..frame.length],
                    },
                    now,
                    reply,
                );
            }
        }
    }

    fn transfer(
        simulation: &mut Simulation,
        loss: Loss,
        upload: usize,
        download: usize,
        server_read_rate: usize,
        server_mss: u16,
    ) -> Outcome {
        transfer_with(
            simulation,
            loss,
            upload,
            download,
            server_read_rate,
            server_mss,
            true,
        )
    }

    fn transfer_with(
        simulation: &mut Simulation,
        loss: Loss,
        upload: usize,
        download: usize,
        server_read_rate: usize,
        server_mss: u16,
        sack: bool,
    ) -> Outcome {
        simulation.client.reset();
        simulation.server.reset();
        simulation.client.set_sack(sack);
        simulation.server.set_sack(sack);
        simulation.to_server.count = 0;
        simulation.to_client.count = 0;
        simulation.to_server.data_frames = 0;
        simulation.to_client.data_frames = 0;
        simulation.to_server.max_payload = 0;
        simulation.to_client.max_payload = 0;
        let Simulation {
            client,
            server,
            to_server,
            to_client,
        } = simulation;
        let mut now = 1_000_000u64;
        if server_mss != 0 {
            server.set_local_mss(server_mss);
        }
        let (Some(listener), Some(socket)) = (server.socket(), client.socket()) else {
            return failed();
        };
        if server.bind(listener, 80).is_err() || server.listen(listener).is_err() {
            return failed();
        }
        if client.connect(socket, SERVER, 80, now, to_server).is_err() {
            return failed();
        }

        let mut accepted = None;
        let mut uploaded = 0usize;
        let mut downloaded_by_server = 0usize;
        let mut received_by_server = 0usize;
        let mut received_by_client = 0usize;
        let mut client_closed = false;
        let mut server_closed = false;
        let mut peak_cwnd = 0;
        let mut low_ssthresh = u32::MAX;
        let mut intact = true;
        let mut buffer = [0u8; 512];
        let mut completed = false;
        let mut negotiated = false;
        let start = now;

        while now < start + 120_000_000_000 {
            now += 1_000_000;
            deliver(to_server, server, CLIENT, loss, now, to_client);
            deliver(to_client, client, SERVER, loss.backward(), now, to_server);

            if accepted.is_none() {
                accepted = server.accept(listener);
            }
            if client.state(socket) == Some(State::Established) && !client_closed {
                while uploaded < upload {
                    let end = (uploaded + buffer.len()).min(upload);
                    for (index, byte) in buffer[..end - uploaded].iter_mut().enumerate() {
                        *byte = pattern(uploaded + index);
                    }
                    match client.write(socket, &buffer[..end - uploaded], now, to_server) {
                        Ok(count) => uploaded += count,
                        Err(_) => break,
                    }
                }
                loop {
                    match client.read(socket, &mut buffer, to_server) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => {
                            for (index, byte) in buffer[..count].iter().enumerate() {
                                if *byte != pattern(received_by_client + index + 1_000_000) {
                                    intact = false;
                                }
                            }
                            received_by_client += count;
                        }
                    }
                }
                if uploaded == upload && received_by_client == download {
                    client.close(socket, now, to_server);
                    client_closed = true;
                }
            }
            if let Some(connection) = accepted
                && !server_closed
            {
                let mut budget = server_read_rate;
                while budget > 0 {
                    let amount = budget.min(buffer.len());
                    match server.read(connection, &mut buffer[..amount], to_client) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => {
                            for (index, byte) in buffer[..count].iter().enumerate() {
                                if *byte != pattern(received_by_server + index) {
                                    intact = false;
                                }
                            }
                            received_by_server += count;
                            budget -= count;
                        }
                    }
                }
                while downloaded_by_server < download {
                    let end = (downloaded_by_server + buffer.len()).min(download);
                    for (index, byte) in buffer[..end - downloaded_by_server].iter_mut().enumerate()
                    {
                        *byte = pattern(downloaded_by_server + index + 1_000_000);
                    }
                    match server.write(
                        connection,
                        &buffer[..end - downloaded_by_server],
                        now,
                        to_client,
                    ) {
                        Ok(count) => downloaded_by_server += count,
                        Err(_) => break,
                    }
                }
                if received_by_server == upload
                    && downloaded_by_server == download
                    && server.readable(connection)
                    && server.state(connection) == Some(State::CloseWait)
                {
                    server.close(connection, now, to_client);
                    server_closed = true;
                }
            }
            client.tick(now, to_server);
            server.tick(now, to_client);
            negotiated |= client.sack_negotiated(socket)
                && accepted.is_some_and(|connection| server.sack_negotiated(connection));
            if let Some((window, threshold)) = client.congestion(socket) {
                peak_cwnd = peak_cwnd.max(window);
                low_ssthresh = low_ssthresh.min(threshold);
            }
            if client_closed
                && server_closed
                && matches!(client.state(socket), Some(State::TimeWait | State::Closed))
                && server
                    .state(accepted.unwrap_or(0))
                    .is_none_or(|state| state == State::Closed)
            {
                completed = true;
                break;
            }
        }
        let counters = client.counters(socket).unwrap_or(Counters {
            retransmits: 0,
            fast_retransmits: 0,
            timeouts: 0,
        });
        let server_counters = accepted
            .and_then(|connection| server.counters(connection))
            .unwrap_or(Counters {
                retransmits: 0,
                fast_retransmits: 0,
                timeouts: 0,
            });
        Outcome {
            completed: completed
                && intact
                && received_by_server == upload
                && received_by_client == download,
            client_counters: Counters {
                retransmits: counters.retransmits + server_counters.retransmits,
                fast_retransmits: counters.fast_retransmits + server_counters.fast_retransmits,
                timeouts: counters.timeouts + server_counters.timeouts,
            },
            peak_cwnd,
            low_ssthresh,
            elapsed_ms: (now - start) / 1_000_000,
            max_payload: to_server.max_payload,
            selective: client.sack_retransmits(socket)
                + accepted.map_or(0, |connection| server.sack_retransmits(connection)),
            negotiated,
            upload_frames: to_server.data_frames,
        }
    }

    fn failed() -> Outcome {
        Outcome {
            completed: false,
            client_counters: Counters {
                retransmits: 0,
                fast_retransmits: 0,
                timeouts: 0,
            },
            peak_cwnd: 0,
            low_ssthresh: 0,
            elapsed_ms: 0,
            max_payload: 0,
            selective: 0,
            negotiated: false,
            upload_frames: 0,
        }
    }

    /// Out-of-order segments are kept and reported as SACK blocks (most recent
    /// first, merged when adjacent), a segment filling the gap moves the
    /// cumulative acknowledgement over everything contiguous, and the bytes
    /// read afterwards are the original stream.
    fn out_of_order_receiver(simulation: &mut Simulation) -> bool {
        simulation.client.reset();
        simulation.server.reset();
        simulation.to_server.count = 0;
        simulation.to_client.count = 0;
        let Simulation {
            client,
            server,
            to_server,
            to_client,
        } = simulation;
        let (Some(listener), Some(socket)) = (server.socket(), client.socket()) else {
            return false;
        };
        let mut now = 1_000_000u64;
        if server.bind(listener, 80).is_err()
            || server.listen(listener).is_err()
            || client.connect(socket, SERVER, 80, now, to_server).is_err()
        {
            return false;
        }
        let mut connection = None;
        for _ in 0..50 {
            now += 1_000_000;
            deliver(to_server, server, CLIENT, Loss::None, now, to_client);
            deliver(to_client, client, SERVER, Loss::None, now, to_server);
            connection = connection.or_else(|| server.accept(listener));
        }
        let Some(connection) = connection else {
            return false;
        };
        if !client.sack_negotiated(socket) || !server.sack_negotiated(connection) {
            return false;
        }
        to_server.count = 0;
        to_client.count = 0;
        let start = server.tcbs[connection].rcv_nxt;
        let client_port = client.tcbs[socket].local_port;
        let client_next = client.tcbs[socket].rcv_nxt;
        let segment = |offset: u32| {
            let mut data = [0u8; 1460];
            for (index, byte) in data.iter_mut().enumerate() {
                *byte = pattern(offset as usize + index);
            }
            data
        };
        let feed = |server: &mut Tcp, to_client: &mut Wire, offset: u32| -> Frame {
            let data = segment(offset);
            to_client.count = 0;
            server.input(
                &Incoming {
                    remote: CLIENT,
                    remote_port: client_port,
                    local_port: 80,
                    seq: start.wrapping_add(offset),
                    ack: client_next,
                    flags: ACK,
                    window: 65_535,
                    mss: 0,
                    sack: Sack::default(),
                    payload: &data,
                },
                now,
                to_client,
            );
            to_client.frames[to_client.count.saturating_sub(1)]
        };
        let block = |frame: &Frame, index: usize| {
            let (first, last) = frame.sack.blocks[index];
            (first.wrapping_sub(start), last.wrapping_sub(start))
        };
        let first = feed(server, to_client, 1460);
        let one = first.ack == start && first.sack.count == 1 && block(&first, 0) == (1460, 2920);
        let second = feed(server, to_client, 2920);
        let merged =
            second.ack == start && second.sack.count == 1 && block(&second, 0) == (1460, 4380);
        let third = feed(server, to_client, 5840);
        let two = third.ack == start
            && third.sack.count == 2
            && block(&third, 0) == (5840, 7300)
            && block(&third, 1) == (1460, 4380);
        let filled = feed(server, to_client, 0);
        let cumulative = filled.ack == start.wrapping_add(4380)
            && filled.sack.count == 1
            && block(&filled, 0) == (5840, 7300);
        let last = feed(server, to_client, 4380);
        let complete = last.ack == start.wrapping_add(7300) && last.sack.count == 0;
        let mut received = [0u8; 8000];
        let mut total = 0;
        while let Ok(count) = server.read(connection, &mut received[total..], to_client) {
            if count == 0 {
                break;
            }
            total += count;
        }
        let intact = total == 7300
            && received[..total]
                .iter()
                .enumerate()
                .all(|(index, byte)| *byte == pattern(index));
        one && merged && two && cumulative && complete && intact
    }

    fn refused(simulation: &mut Simulation) -> bool {
        simulation.client.reset();
        simulation.server.reset();
        simulation.to_server.count = 0;
        simulation.to_client.count = 0;
        let Simulation {
            client,
            server,
            to_server,
            to_client,
        } = simulation;
        let Some(socket) = client.socket() else {
            return false;
        };
        let mut now = 1_000_000u64;
        if client.connect(socket, SERVER, 9, now, to_server).is_err() {
            return false;
        }
        for _ in 0..20 {
            now += 1_000_000;
            deliver(to_server, server, CLIENT, Loss::None, now, to_client);
            deliver(to_client, client, SERVER, Loss::None, now, to_server);
        }
        let mut byte = [0u8; 1];
        client.state(socket) == Some(State::Closed)
            && client.error(socket) == ECONNREFUSED
            && client.read(socket, &mut byte, to_server) == Err(ECONNREFUSED)
            && client.write(socket, b"x", now, to_server) == Err(ENOTCONN)
    }

    fn unreachable_peer(simulation: &mut Simulation) -> bool {
        simulation.client.reset();
        simulation.to_server.count = 0;
        let Simulation {
            client, to_server, ..
        } = simulation;
        let Some(socket) = client.socket() else {
            return false;
        };
        let mut now = 1_000_000u64;
        if client.connect(socket, SERVER, 80, now, to_server).is_err() {
            return false;
        }
        let mut syns = 0;
        for _ in 0..120_000 {
            now += 1_000_000;
            syns += to_server.count;
            to_server.count = 0;
            client.tick(now, to_server);
            if client.state(socket) == Some(State::Closed) {
                break;
            }
        }
        syns += to_server.count;
        to_server.count = 0;
        client.error(socket) == ETIMEDOUT && syns == 1 + MAX_SYN_RETRIES as usize
    }

    fn hostile_input(simulation: &mut Simulation) -> bool {
        simulation.client.reset();
        simulation.server.reset();
        simulation.to_server.count = 0;
        simulation.to_client.count = 0;
        let Simulation {
            client,
            server,
            to_server,
            to_client,
        } = simulation;
        let (Some(listener), Some(socket)) = (server.socket(), client.socket()) else {
            return false;
        };
        if server.bind(listener, 80).is_err() || server.listen(listener).is_err() {
            return false;
        }
        let mut now = 1_000_000u64;
        if client.connect(socket, SERVER, 80, now, to_server).is_err() {
            return false;
        }
        for _ in 0..10 {
            now += 1_000_000;
            deliver(to_server, server, CLIENT, Loss::None, now, to_client);
            deliver(to_client, client, SERVER, Loss::None, now, to_server);
        }
        let Some(connection) = server.accept(listener) else {
            return false;
        };
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut payload = [0u8; 300];
        for round in 0..6_000u32 {
            now += 100_000;
            let random = next();
            let length = (random >> 40) as usize % payload.len();
            for byte in payload[..length].iter_mut() {
                *byte = next() as u8;
            }
            let (target, remote, local_port, remote_port, sink): (
                &mut Tcp,
                Address,
                u16,
                u16,
                &mut Wire,
            ) = if random & 1 == 0 {
                (&mut *server, CLIENT, 80, 49_152, &mut *to_client)
            } else {
                (&mut *client, SERVER, 49_152, 80, &mut *to_server)
            };
            target.input(
                &Incoming {
                    remote,
                    remote_port,
                    local_port,
                    seq: if round % 3 == 0 {
                        random as u32
                    } else {
                        (random >> 8) as u32 & 0x3ff
                    },
                    ack: (random >> 16) as u32 & 0xfff,
                    flags: (random >> 32) as u8 & 0x1f,
                    window: (random >> 24) as u16,
                    mss: if random & 4 == 0 {
                        (random >> 44) as u16
                    } else {
                        0
                    },
                    sack: Sack {
                        permitted: random & 8 == 0,
                        count: (random >> 50) as usize % 6,
                        blocks: core::array::from_fn(|index| {
                            let seed = random.rotate_left(index as u32 * 13 + 7);
                            if seed & 1 == 0 {
                                (seed as u32 & 0xfff, (seed >> 20) as u32 & 0xfff)
                            } else {
                                ((seed >> 8) as u32, (seed >> 32) as u32)
                            }
                        }),
                    },
                    payload: &payload[..length],
                },
                now,
                sink,
            );
            to_server.count = 0;
            to_client.count = 0;
            client.tick(now, to_server);
            server.tick(now, to_client);
            to_server.count = 0;
            to_client.count = 0;
            let mut sink_buffer = [0u8; 64];
            let _ = server.read(connection, &mut sink_buffer, to_client);
            let _ = client.read(socket, &mut sink_buffer, to_server);
        }
        let oversized = |tcp: &Tcp| {
            tcp.tcbs
                .iter()
                .any(|tcb| tcb.used && (tcb.send.len > BUFFER || tcb.recv.len > BUFFER))
        };
        !oversized(client) && !oversized(server)
    }

    pub struct Report {
        pub clean: bool,
        pub lossy: bool,
        pub burst_loss: bool,
        pub slow_reader: bool,
        pub refused: bool,
        pub unreachable: bool,
        pub hostile: bool,
        pub reclaim: bool,
        pub sack_receiver: bool,
        pub sack_recovery: bool,
        pub retransmits: u32,
        pub fast_retransmits: u32,
        pub peak_cwnd: u32,
        pub verified: bool,
    }

    pub fn self_test() -> Report {
        let mut simulation = SIMULATION.lock();
        let clean = transfer(
            &mut simulation,
            Loss::None,
            60_000,
            20_000,
            usize::MAX / 2,
            0,
        );
        let clean_ok = clean.completed
            && clean.client_counters.retransmits == 0
            && clean.peak_cwnd > INITIAL_CWND * 3
            && clean.low_ssthresh == MAX_CWND
            && clean.max_payload == MSS;

        let lossy = transfer(
            &mut simulation,
            Loss::EveryNthData(6),
            60_000,
            20_000,
            usize::MAX / 2,
            0,
        );
        let lossy_ok = lossy.completed
            && lossy.client_counters.retransmits > 0
            && lossy.low_ssthresh < MAX_CWND;

        let burst = transfer(
            &mut simulation,
            Loss::OnlyDataFrame(9),
            60_000,
            1_000,
            usize::MAX / 2,
            0,
        );
        let burst_ok = burst.completed
            && burst.client_counters.fast_retransmits >= 1
            && burst.client_counters.timeouts == 0;

        let selective = transfer_with(
            &mut simulation,
            Loss::Burst(9, 3),
            60_000,
            1_000,
            usize::MAX / 2,
            0,
            true,
        );
        let plain = transfer_with(
            &mut simulation,
            Loss::Burst(9, 3),
            60_000,
            1_000,
            usize::MAX / 2,
            0,
            false,
        );
        let lossy_plain = transfer_with(
            &mut simulation,
            Loss::EveryNthData(6),
            60_000,
            20_000,
            usize::MAX / 2,
            0,
            false,
        );
        let sack_recovery = selective.completed
            && plain.completed
            && lossy_plain.completed
            && selective.negotiated
            && !plain.negotiated
            && selective.client_counters.timeouts == 0
            && selective.selective >= 3
            && plain.selective == 0
            && selective.upload_frames + 10 <= plain.upload_frames
            && lossy.upload_frames <= lossy_plain.upload_frames
            && lossy.selective > 0;
        let sack_receiver = out_of_order_receiver(&mut simulation);

        let slow = transfer(&mut simulation, Loss::None, 40_000, 5_000, 300, 0);
        let slow_ok = slow.completed && slow.elapsed_ms > 100;

        let small = transfer(
            &mut simulation,
            Loss::None,
            20_000,
            2_000,
            usize::MAX / 2,
            536,
        );
        let small_ok = small.completed && small.max_payload > 0 && small.max_payload <= 536;
        let duplicated = transfer(
            &mut simulation,
            Loss::Duplicate(3),
            40_000,
            10_000,
            usize::MAX / 2,
            0,
        );
        let reordered = transfer(
            &mut simulation,
            Loss::Reverse,
            40_000,
            10_000,
            usize::MAX / 2,
            0,
        );
        let chaos_ok = duplicated.completed && reordered.completed;
        let reclaim_ok = {
            let Simulation { client, .. } = &mut *simulation;
            let waiting = (0..SOCKETS).any(|index| client.state(index) == Some(State::TimeWait));
            let mut filler = 0;
            while client.socket().is_some() && filler < SOCKETS {
                filler += 1;
            }
            waiting && filler > SOCKETS - 1
        };
        let refused_ok = refused(&mut simulation);
        let unreachable_ok = unreachable_peer(&mut simulation);
        let hostile_ok = hostile_input(&mut simulation);
        Report {
            clean: clean_ok,
            lossy: lossy_ok,
            burst_loss: burst_ok,
            slow_reader: slow_ok,
            refused: refused_ok,
            unreachable: unreachable_ok,
            hostile: hostile_ok,
            reclaim: reclaim_ok,
            sack_receiver,
            sack_recovery,
            retransmits: lossy.client_counters.retransmits,
            fast_retransmits: burst.client_counters.fast_retransmits,
            peak_cwnd: clean.peak_cwnd,
            verified: clean_ok
                && lossy_ok
                && burst_ok
                && slow_ok
                && refused_ok
                && unreachable_ok
                && hostile_ok
                && reclaim_ok
                && small_ok
                && chaos_ok
                && sack_receiver
                && sack_recovery,
        }
    }
}
