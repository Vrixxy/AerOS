//! Per-process Linux capability sets. A process starts with every capability
//! (all AerOS processes are root) and can only ever give them up: `capset`
//! may shrink the permitted set, and the effective set must stay inside it.

pub const CAP_CHOWN: u32 = 0;
pub const CAP_KILL: u32 = 5;
pub const CAP_NET_BIND_SERVICE: u32 = 10;
pub const CAP_SYS_PTRACE: u32 = 19;
pub const CAP_SYS_ADMIN: u32 = 21;
pub const CAP_SYS_BOOT: u32 = 22;
pub const CAP_SYS_NICE: u32 = 23;
pub const CAP_SYS_TIME: u32 = 25;
const LAST_CAP: u32 = 40;
pub const ALL: u64 = (1 << (LAST_CAP + 1)) - 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Capabilities {
    pub effective: u64,
    pub permitted: u64,
}

impl Capabilities {
    pub const FULL: Self = Self {
        effective: ALL,
        permitted: ALL,
    };

    pub fn has(self, capability: u32) -> bool {
        capability <= LAST_CAP && self.effective & (1 << capability) != 0
    }

    /// The sets a `capset` call asks for, if they are allowed: the new
    /// permitted set must be inside the current one and the new effective set
    /// inside the new permitted one.
    pub fn restrict(self, effective: u64, permitted: u64) -> Option<Self> {
        if permitted & !self.permitted != 0 || effective & !permitted != 0 {
            return None;
        }
        Some(Self {
            effective,
            permitted,
        })
    }

    /// The `__user_cap_data_struct[2]` encoding: effective, permitted and
    /// inheritable words for capabilities 0-31, then 32-63.
    pub fn to_data(self) -> [u8; 24] {
        let words = [
            self.effective as u32,
            self.permitted as u32,
            0,
            (self.effective >> 32) as u32,
            (self.permitted >> 32) as u32,
            0,
        ];
        let mut data = [0u8; 24];
        for (index, word) in words.iter().enumerate() {
            data[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        data
    }

    /// `(effective, permitted)` from the same encoding.
    pub fn from_data(data: &[u8; 24]) -> (u64, u64) {
        let word = |index: usize| {
            u32::from_le_bytes([
                data[index * 4],
                data[index * 4 + 1],
                data[index * 4 + 2],
                data[index * 4 + 3],
            ]) as u64
        };
        (word(0) | word(3) << 32, word(1) | word(4) << 32)
    }
}

pub fn self_test() -> bool {
    let full = Capabilities::FULL;
    let no_nice = full.restrict(ALL & !(1 << CAP_SYS_NICE), ALL & !(1 << CAP_SYS_NICE));
    let dropped = no_nice.is_some_and(|caps| !caps.has(CAP_SYS_NICE) && caps.has(CAP_KILL));
    let cannot_regain = no_nice.is_some_and(|caps| caps.restrict(ALL, ALL).is_none());
    let effective_inside_permitted = full.restrict(0b11, 0b01).is_none();
    let can_shrink_effective_only = full
        .restrict(1 << CAP_KILL, ALL)
        .is_some_and(|caps| caps.has(CAP_KILL) && !caps.has(CAP_SYS_NICE) && caps.permitted == ALL);
    let unknown_bit_refused = full.restrict(1 << 50, 1 << 50).is_none();
    let encoded = Capabilities {
        effective: 0x1_8000_0001,
        permitted: 0x1_ffff_ffff,
    };
    let round_trip =
        Capabilities::from_data(&encoded.to_data()) == (encoded.effective, encoded.permitted);
    let out_of_range = !full.has(41);
    dropped
        && cannot_regain
        && effective_inside_permitted
        && can_shrink_effective_only
        && unknown_bit_refused
        && round_trip
        && out_of_range
}
