//! `SECCOMP_SET_MODE_FILTER`: a classic-BPF validator and interpreter over the
//! `seccomp_data` record (`nr`, `arch`, `instruction_pointer`, six arguments).

pub const MAX_INSTRUCTIONS: usize = 64;
pub const DATA_BYTES: usize = 64;
pub const AUDIT_ARCH_X86_64: u32 = 0xc000_003e;

pub const RET_KILL_PROCESS: u32 = 0x8000_0000;
pub const RET_ERRNO: u32 = 0x0005_0000;
pub const RET_TRACE: u32 = 0x7ff0_0000;
pub const RET_LOG: u32 = 0x7ffc_0000;
pub const RET_ALLOW: u32 = 0x7fff_0000;

const CLASS_LD: u16 = 0x00;
const CLASS_LDX: u16 = 0x01;
const CLASS_ST: u16 = 0x02;
const CLASS_STX: u16 = 0x03;
const CLASS_ALU: u16 = 0x04;
const CLASS_JMP: u16 = 0x05;
const CLASS_RET: u16 = 0x06;
const CLASS_MISC: u16 = 0x07;

const MODE_IMM: u16 = 0x00;
const MODE_ABS: u16 = 0x20;
const MODE_MEM: u16 = 0x60;
const MODE_LEN: u16 = 0x80;
const SIZE_WORD: u16 = 0x00;
const SOURCE_X: u16 = 0x08;
const MEMORY_WORDS: usize = 16;

#[derive(Clone, Copy)]
pub struct Instruction {
    pub code: u16,
    pub jt: u8,
    pub jf: u8,
    pub k: u32,
}

impl Instruction {
    pub const EMPTY: Self = Self {
        code: 0,
        jt: 0,
        jf: 0,
        k: 0,
    };

    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            code: u16::from_le_bytes([bytes[0], bytes[1]]),
            jt: bytes[2],
            jf: bytes[3],
            k: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Filter {
    pub length: usize,
    pub instructions: [Instruction; MAX_INSTRUCTIONS],
}

impl Filter {
    pub fn new(program: &[Instruction]) -> Option<Self> {
        if !validate(program) {
            return None;
        }
        let mut instructions = [Instruction::EMPTY; MAX_INSTRUCTIONS];
        instructions[..program.len()].copy_from_slice(program);
        Some(Self {
            length: program.len(),
            instructions,
        })
    }
}

/// Builds the `seccomp_data` record handed to a filter.
pub fn data_bytes(number: u32, instruction_pointer: u64, arguments: &[u64; 6]) -> [u8; DATA_BYTES] {
    let mut data = [0u8; DATA_BYTES];
    data[..4].copy_from_slice(&number.to_le_bytes());
    data[4..8].copy_from_slice(&AUDIT_ARCH_X86_64.to_le_bytes());
    data[8..16].copy_from_slice(&instruction_pointer.to_le_bytes());
    for (index, argument) in arguments.iter().enumerate() {
        data[16 + index * 8..24 + index * 8].copy_from_slice(&argument.to_le_bytes());
    }
    data
}

/// Checks the program is safe to run: only instructions seccomp allows,
/// every jump forward and inside the program, and a final return.
pub fn validate(program: &[Instruction]) -> bool {
    if program.is_empty() || program.len() > MAX_INSTRUCTIONS {
        return false;
    }
    for (pc, instruction) in program.iter().enumerate() {
        let class = instruction.code & 0x07;
        let valid = match class {
            CLASS_LD => match instruction.code & 0xe0 {
                MODE_ABS => {
                    instruction.code & 0x18 == SIZE_WORD
                        && instruction.k % 4 == 0
                        && (instruction.k as usize) < DATA_BYTES
                }
                MODE_IMM | MODE_LEN => true,
                MODE_MEM => (instruction.k as usize) < MEMORY_WORDS,
                _ => false,
            },
            CLASS_LDX => match instruction.code & 0xe0 {
                MODE_IMM | MODE_LEN => true,
                MODE_MEM => (instruction.k as usize) < MEMORY_WORDS,
                _ => false,
            },
            CLASS_ST | CLASS_STX => (instruction.k as usize) < MEMORY_WORDS,
            CLASS_ALU => {
                let operation = instruction.code & 0xf0;
                let known = matches!(
                    operation,
                    0x00 | 0x10 | 0x20 | 0x30 | 0x40 | 0x50 | 0x60 | 0x70 | 0x80 | 0x90 | 0xa0
                );
                let constant_divisor_zero = instruction.code & SOURCE_X == 0
                    && matches!(operation, 0x30 | 0x90)
                    && instruction.k == 0;
                known && !constant_divisor_zero
            }
            CLASS_JMP => {
                let remaining = program.len() - pc - 1;
                match instruction.code & 0xf0 {
                    0x00 => (instruction.k as usize) < remaining,
                    0x10 | 0x20 | 0x30 | 0x40 => {
                        (instruction.jt as usize) < remaining
                            && (instruction.jf as usize) < remaining
                    }
                    _ => false,
                }
            }
            CLASS_RET => matches!(instruction.code & 0x18, 0x00 | 0x10),
            CLASS_MISC => matches!(instruction.code & 0xf8, 0x00 | 0x80),
            _ => false,
        };
        if !valid {
            return false;
        }
    }
    program[program.len() - 1].code & 0x07 == CLASS_RET
}

/// Runs a validated filter and returns its `SECCOMP_RET_*` value.
pub fn run(filter: &Filter, data: &[u8; DATA_BYTES]) -> u32 {
    let program = &filter.instructions[..filter.length];
    let mut accumulator = 0u32;
    let mut index_register = 0u32;
    let mut memory = [0u32; MEMORY_WORDS];
    let mut pc = 0usize;
    while pc < program.len() {
        let instruction = program[pc];
        pc += 1;
        let use_x = instruction.code & SOURCE_X != 0;
        match instruction.code & 0x07 {
            CLASS_LD => {
                accumulator = match instruction.code & 0xe0 {
                    MODE_ABS => {
                        let at = instruction.k as usize;
                        u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
                    }
                    MODE_IMM => instruction.k,
                    MODE_LEN => DATA_BYTES as u32,
                    _ => memory[instruction.k as usize],
                };
            }
            CLASS_LDX => {
                index_register = match instruction.code & 0xe0 {
                    MODE_IMM => instruction.k,
                    MODE_LEN => DATA_BYTES as u32,
                    _ => memory[instruction.k as usize],
                };
            }
            CLASS_ST => memory[instruction.k as usize] = accumulator,
            CLASS_STX => memory[instruction.k as usize] = index_register,
            CLASS_ALU => {
                let operand = if use_x { index_register } else { instruction.k };
                accumulator = match instruction.code & 0xf0 {
                    0x00 => accumulator.wrapping_add(operand),
                    0x10 => accumulator.wrapping_sub(operand),
                    0x20 => accumulator.wrapping_mul(operand),
                    0x30 => match accumulator.checked_div(operand) {
                        Some(value) => value,
                        None => return RET_KILL_PROCESS,
                    },
                    0x40 => accumulator | operand,
                    0x50 => accumulator & operand,
                    0x60 => accumulator.checked_shl(operand).unwrap_or(0),
                    0x70 => accumulator.checked_shr(operand).unwrap_or(0),
                    0x80 => accumulator.wrapping_neg(),
                    0x90 => match accumulator.checked_rem(operand) {
                        Some(value) => value,
                        None => return RET_KILL_PROCESS,
                    },
                    _ => accumulator ^ operand,
                };
            }
            CLASS_JMP => {
                let operand = if use_x { index_register } else { instruction.k };
                let taken = match instruction.code & 0xf0 {
                    0x00 => {
                        pc += instruction.k as usize;
                        continue;
                    }
                    0x10 => accumulator == operand,
                    0x20 => accumulator > operand,
                    0x30 => accumulator >= operand,
                    _ => accumulator & operand != 0,
                };
                pc += if taken {
                    instruction.jt
                } else {
                    instruction.jf
                } as usize;
            }
            CLASS_RET => {
                return if instruction.code & 0x18 == 0x10 {
                    accumulator
                } else {
                    instruction.k
                };
            }
            _ => {
                if instruction.code & 0xf8 == 0x00 {
                    index_register = accumulator;
                } else {
                    accumulator = index_register;
                }
            }
        }
    }
    RET_KILL_PROCESS
}

const fn ins(code: u16, jt: u8, jf: u8, k: u32) -> Instruction {
    Instruction { code, jt, jf, k }
}

pub fn self_test() -> bool {
    let data = |number: u32, first: u64| data_bytes(number, 0, &[first, 0, 0, 0, 0, 0]);

    let deny_getpid = [
        ins(0x20, 0, 0, 0),
        ins(0x15, 0, 1, 39),
        ins(0x06, 0, 0, RET_ERRNO | 1),
        ins(0x06, 0, 0, RET_ALLOW),
    ];
    let Some(filter) = Filter::new(&deny_getpid) else {
        return false;
    };
    let errno_ok = run(&filter, &data(39, 0)) == RET_ERRNO | 1;
    let allow_ok = run(&filter, &data(1, 0)) == RET_ALLOW;

    let arch_and_arg = [
        ins(0x20, 0, 0, 4),
        ins(0x15, 1, 0, AUDIT_ARCH_X86_64),
        ins(0x06, 0, 0, RET_KILL_PROCESS),
        ins(0x20, 0, 0, 0),
        ins(0x15, 0, 4, 1),
        ins(0x20, 0, 0, 16),
        ins(0x15, 0, 2, 2),
        ins(0x06, 0, 0, RET_ERRNO | 13),
        ins(0x06, 0, 0, RET_ALLOW),
        ins(0x06, 0, 0, RET_ALLOW),
    ];
    let Some(second) = Filter::new(&arch_and_arg) else {
        return false;
    };
    let stdout_ok = run(&second, &data(1, 1)) == RET_ALLOW;
    let stderr_blocked = run(&second, &data(1, 2)) == RET_ERRNO | 13;
    let mut foreign = data(1, 1);
    foreign[4] ^= 1;
    let arch_ok = run(&second, &foreign) == RET_KILL_PROCESS;

    let arithmetic = [
        ins(0x20, 0, 0, 0),
        ins(0x54, 0, 0, 0xff),
        ins(0x04, 0, 0, 3),
        ins(0x15, 0, 1, 42),
        ins(0x06, 0, 0, RET_ALLOW),
        ins(0x16, 0, 0, 0),
    ];
    let Some(math) = Filter::new(&arithmetic) else {
        return false;
    };
    let math_ok = run(&math, &data(39, 0)) == RET_ALLOW && run(&math, &data(40, 0)) == 40 + 3;

    let rejected = !validate(&[])
        && !validate(&[ins(0x20, 0, 0, 0)])
        && !validate(&[ins(0x15, 5, 0, 0), ins(0x06, 0, 0, RET_ALLOW)])
        && !validate(&[ins(0x20, 0, 0, 64), ins(0x06, 0, 0, RET_ALLOW)])
        && !validate(&[ins(0x20, 0, 0, 2), ins(0x06, 0, 0, RET_ALLOW)])
        && !validate(&[ins(0x34, 0, 0, 0), ins(0x06, 0, 0, RET_ALLOW)])
        && !validate(&[ins(0x40, 0, 0, 0), ins(0x06, 0, 0, RET_ALLOW)])
        && !validate(&[ins(0x07, 0, 0, 0)]);
    let oversized = !validate(&[ins(0x06, 0, 0, RET_ALLOW); MAX_INSTRUCTIONS + 1]);
    let decoded = Instruction::from_bytes(&[0x15, 0x00, 0x02, 0x03, 0x27, 0x00, 0x00, 0x00]);
    let decode_ok = decoded.code == 0x15 && decoded.jt == 2 && decoded.jf == 3 && decoded.k == 39;

    errno_ok
        && allow_ok
        && stdout_ok
        && stderr_blocked
        && arch_ok
        && math_ok
        && rejected
        && oversized
        && decode_ok
}
