// Included into `mod linux` by svm.rs (feature `linux-smp`): the guest's local
// APIC in x2APIC (MSR) mode, a second virtual CPU, and the MP table that tells
// Linux about them.
//
// There is one host thread, so the two vCPUs are time-sliced: the scheduler in
// `run_for` switches between them on HLT, PAUSE, host interrupts and after a
// fixed number of exits. The BSP owns the legacy devices (8259, PIT, UART,
// keyboard, virtio); each vCPU has its own APIC (IRR/ISR/TPR, LVT, timer).
// x2APIC mode is switched on before the guest starts (IA32_APIC_BASE.EXTD),
// which is why no APIC MMIO page has to be decoded.

const NUM_VCPUS: usize = 2;
const MP_TABLE_GPA: u64 = 0xf_0000;
/// TSC cycles per APIC timer count at divide-by-1 (the guest calibrates the
/// timer against the TSC anyway, so any consistent value works).
const APIC_TSC_PER_COUNT: u64 = 16;
/// Exits a vCPU may run before the other one gets a turn.
const SMP_SLICE_EXITS: u32 = 600;

const APIC_BASE_ADDRESS: u64 = 0xfee0_0000;
const APIC_BASE_BSP: u64 = 1 << 8;
const APIC_BASE_EXTD: u64 = 1 << 10;
const APIC_BASE_ENABLE: u64 = 1 << 11;
const MSR_APIC_BASE: u32 = 0x1b;

#[derive(Clone, Copy)]
struct Apic {
    id: u32,
    base: u64,
    irr: [u32; 8],
    isr: [u32; 8],
    tpr: u32,
    sivr: u32,
    /// [0] CMCI, [1] timer, [2] thermal, [3] perf, [4] LINT0, [5] LINT1, [6] error.
    lvt: [u32; 7],
    icr: u64,
    timer_initial: u32,
    timer_divide: u32,
    /// TSC value at which the timer fires; 0 = not running.
    timer_deadline: u64,
    timer_period: u64,
}

impl Apic {
    const fn new(id: u32) -> Self {
        Self {
            id,
            base: APIC_BASE_ADDRESS
                | APIC_BASE_ENABLE
                | APIC_BASE_EXTD
                | if id == 0 { APIC_BASE_BSP } else { 0 },
            irr: [0; 8],
            isr: [0; 8],
            tpr: 0,
            sivr: 0xff,
            lvt: [0x1_0000; 7],
            icr: 0,
            timer_initial: 0,
            timer_divide: 0,
            timer_deadline: 0,
            timer_period: 0,
        }
    }

    fn highest(bits: &[u32; 8]) -> Option<u8> {
        (0..8usize)
            .rev()
            .find(|word| bits[*word] != 0)
            .map(|word| (word * 32 + 31 - bits[word].leading_zeros() as usize) as u8)
    }

    fn set(bits: &mut [u32; 8], vector: u8) {
        bits[(vector >> 5) as usize] |= 1 << (vector & 31);
    }

    fn clear(bits: &mut [u32; 8], vector: u8) {
        bits[(vector >> 5) as usize] &= !(1 << (vector & 31));
    }

    fn divisor(&self) -> u64 {
        match (self.timer_divide & 3) | ((self.timer_divide >> 1) & 4) {
            0 => 2,
            1 => 4,
            2 => 8,
            3 => 16,
            4 => 32,
            5 => 64,
            6 => 128,
            _ => 1,
        }
    }

    /// Processor priority: the higher of the task priority and the class of
    /// the interrupt in service.
    fn ppr(&self) -> u32 {
        let in_service = Self::highest(&self.isr).map_or(0, |vector| vector as u32 & 0xf0);
        (self.tpr & 0xf0).max(in_service)
    }

    fn enabled(&self) -> bool {
        self.sivr & 0x100 != 0
    }

    /// The vector that could be delivered right now, if any.
    fn deliverable(&self) -> Option<u8> {
        if !self.enabled() {
            return None;
        }
        let vector = Self::highest(&self.irr)?;
        ((vector as u32 & 0xf0) > self.ppr()).then_some(vector)
    }
}

#[derive(Clone, Copy)]
struct Vcpu {
    vmcb: u64,
    gpr: [u64; 14],
    /// Physical address of this vCPU's XSAVE area while it is not running.
    fpu: u64,
    /// False for the AP until the BSP's INIT+SIPI starts it.
    started: bool,
    /// Executed HLT and nothing has woken it yet.
    halted: bool,
    /// Received INIT and waits for the STARTUP IPI.
    sipi_wait: bool,
    /// An NMI IPI arrived and is waiting for the guest to be able to take it.
    nmi_pending: bool,
    /// An injected NMI's handler hasn't executed IRET yet: further NMIs wait.
    nmi_blocked: bool,
    apic: Apic,
}

impl Vcpu {
    const fn new(id: u32, vmcb: u64, fpu: u64) -> Self {
        Self {
            vmcb,
            gpr: [0; 14],
            fpu,
            started: id == 0,
            halted: false,
            sipi_wait: false,
            nmi_pending: false,
            nmi_blocked: false,
            apic: Apic::new(id),
        }
    }
}

fn is_apic_msr(msr: u32) -> bool {
    msr == MSR_APIC_BASE || (0x800..=0x8ff).contains(&msr)
}

/// Intercept read+write of the APIC MSRs (0x1b and the x2APIC range): the
/// MSR permission map has two bits per MSR (read, write) for MSRs 0..0x1fff.
fn intercept_apic_msrs(msrpm: u64) {
    let mark = |msr: u32| {
        let byte = (msr / 4) as u64;
        let bit = (msr % 4) * 2;
        let current = read_u8(msrpm, byte);
        unsafe { write_u8(msrpm, byte, current | (3 << bit)) };
    };
    mark(MSR_APIC_BASE);
    for msr in 0x800..=0x8ffu32 {
        mark(msr);
    }
}

/// Writes the MP configuration (floating pointer + table) Linux scans for in
/// the BIOS area: two processors (APIC ids 0 and 1) and one ISA bus, no IOAPIC
/// (the guest runs with `noapic` and keeps using the 8259).
fn build_mp_table(ram: u64) {
    let signature = __cpuid_count(1, 0);
    let table = MP_TABLE_GPA + 0x10;
    let mut bytes = [0u8; 44 + 20 * NUM_VCPUS + 8];
    bytes[0..4].copy_from_slice(b"PCMP");
    let length = bytes.len() as u16;
    bytes[4..6].copy_from_slice(&length.to_le_bytes());
    bytes[6] = 4; // spec revision 1.4
    bytes[8..16].copy_from_slice(b"AEROS   ");
    bytes[16..28].copy_from_slice(b"AerOS-VM    ");
    let entries = (NUM_VCPUS + 1) as u16;
    bytes[34..36].copy_from_slice(&entries.to_le_bytes());
    bytes[36..40].copy_from_slice(&(APIC_BASE_ADDRESS as u32).to_le_bytes());
    for id in 0..NUM_VCPUS {
        let at = 44 + 20 * id;
        bytes[at] = 0; // processor entry
        bytes[at + 1] = id as u8; // local APIC id
        bytes[at + 2] = 0x14; // local APIC version
        bytes[at + 3] = 1 | if id == 0 { 2 } else { 0 }; // enabled, BSP
        bytes[at + 4..at + 8].copy_from_slice(&signature.eax.to_le_bytes());
        bytes[at + 8..at + 12].copy_from_slice(&signature.edx.to_le_bytes());
    }
    let bus = 44 + 20 * NUM_VCPUS;
    bytes[bus] = 1; // bus entry
    bytes[bus + 1] = 0;
    bytes[bus + 2..bus + 8].copy_from_slice(b"ISA   ");
    let sum = bytes.iter().fold(0u8, |acc, byte| acc.wrapping_add(*byte));
    bytes[7] = 0u8.wrapping_sub(sum);
    for (index, byte) in bytes.iter().enumerate() {
        unsafe { write_u8(ram, table + index as u64, *byte) };
    }
    // Floating pointer structure (16 bytes, 16-byte aligned).
    let mut pointer = [0u8; 16];
    pointer[0..4].copy_from_slice(b"_MP_");
    pointer[4..8].copy_from_slice(&(table as u32).to_le_bytes());
    pointer[8] = 1; // length in paragraphs
    pointer[9] = 4; // spec revision
    let sum = pointer.iter().fold(0u8, |acc, byte| acc.wrapping_add(*byte));
    pointer[10] = 0u8.wrapping_sub(sum);
    for (index, byte) in pointer.iter().enumerate() {
        unsafe { write_u8(ram, MP_TABLE_GPA + index as u64, *byte) };
    }
}

impl Machine {
    fn apic_msr_read(&mut self, msr: u32) -> u64 {
        let now = rdtsc();
        let apic = &self.vcpus[self.cur].apic;
        match msr {
            MSR_APIC_BASE => apic.base,
            0x802 => apic.id as u64,
            0x803 => 0x0005_0014,
            0x808 => apic.tpr as u64,
            0x80a => apic.ppr() as u64,
            0x80d => (1u64 << (apic.id & 15)) | ((apic.id as u64 >> 4) << 16),
            0x80f => apic.sivr as u64,
            0x810..=0x817 => apic.isr[(msr - 0x810) as usize] as u64,
            0x820..=0x827 => apic.irr[(msr - 0x820) as usize] as u64,
            0x82f => apic.lvt[0] as u64,
            0x830 => apic.icr,
            0x832..=0x837 => apic.lvt[(msr - 0x831) as usize] as u64,
            0x838 => apic.timer_initial as u64,
            0x839 => {
                if apic.timer_deadline == 0 {
                    0
                } else {
                    let left = apic.timer_deadline.saturating_sub(now);
                    (left / (apic.divisor() * APIC_TSC_PER_COUNT)).min(u32::MAX as u64)
                }
            }
            0x83e => apic.timer_divide as u64,
            _ => 0,
        }
    }

    fn apic_msr_write(&mut self, msr: u32, value: u64) {
        let index = self.cur;
        match msr {
            MSR_APIC_BASE => self.vcpus[index].apic.base = value,
            0x808 => self.vcpus[index].apic.tpr = value as u32 & 0xff,
            0x80b => {
                let apic = &mut self.vcpus[index].apic;
                if let Some(vector) = Apic::highest(&apic.isr) {
                    Apic::clear(&mut apic.isr, vector);
                }
            }
            0x80f => self.vcpus[index].apic.sivr = value as u32,
            0x82f => self.vcpus[index].apic.lvt[0] = value as u32,
            0x830 => self.apic_ipi(value),
            0x832..=0x837 => self.vcpus[index].apic.lvt[(msr - 0x831) as usize] = value as u32,
            0x838 => {
                let apic = &mut self.vcpus[index].apic;
                apic.timer_initial = value as u32;
                if value as u32 == 0 {
                    apic.timer_deadline = 0;
                    apic.timer_period = 0;
                } else {
                    let cycles = value as u32 as u64 * apic.divisor() * APIC_TSC_PER_COUNT;
                    apic.timer_deadline = rdtsc() + cycles;
                    // Bit 17 of the timer LVT = periodic.
                    apic.timer_period = if apic.lvt[1] & (1 << 17) != 0 { cycles } else { 0 };
                }
            }
            0x83e => self.vcpus[index].apic.timer_divide = value as u32,
            0x83f => self.apic_deliver(index, value as u8),
            _ => {}
        }
    }

    fn apic_deliver(&mut self, index: usize, vector: u8) {
        if vector >= 16 {
            Apic::set(&mut self.vcpus[index].apic.irr, vector);
        }
    }

    fn apic_timer_poll(&mut self, index: usize, now: u64) {
        let apic = &mut self.vcpus[index].apic;
        if apic.timer_deadline == 0 || now < apic.timer_deadline {
            return;
        }
        let vector = (apic.lvt[1] & 0xff) as u8;
        let masked = apic.lvt[1] & 0x1_0000 != 0;
        if apic.timer_period != 0 {
            apic.timer_deadline += apic.timer_period;
            if apic.timer_deadline <= now {
                apic.timer_deadline = now + apic.timer_period;
            }
        } else {
            apic.timer_deadline = 0;
        }
        if !masked {
            self.apic_deliver(index, vector);
        }
    }

    /// Injects the highest deliverable APIC interrupt into the current vCPU
    /// if the guest can take one now. True if one was injected.
    fn apic_inject(&mut self) -> bool {
        if self.vcpus[self.cur].nmi_pending && self.inject_nmi() {
            return true;
        }
        let Some(vector) = self.vcpus[self.cur].apic.deliverable() else {
            return false;
        };
        let rflags = unsafe { read_u64(self.vmcb, 0x570) };
        let int_state = unsafe { read_u64(self.vmcb, 0x068) };
        let vintr = unsafe { read_u64(self.vmcb, 0x060) };
        if rflags & (1 << 9) == 0 || int_state & 1 != 0 || vintr & (1 << 8) != 0 {
            return false;
        }
        let apic = &mut self.vcpus[self.cur].apic;
        Apic::clear(&mut apic.irr, vector);
        Apic::set(&mut apic.isr, vector);
        self.inject_vector(vector);
        true
    }

    fn apic_has_deliverable(&self, index: usize) -> bool {
        self.vcpus[index].nmi_pending || self.vcpus[index].apic.deliverable().is_some()
    }

    /// Injects a pending NMI (vector 2, event type 2) into the current vCPU
    /// unless an event is already queued, the guest is in an interrupt shadow
    /// or another NMI is still being serviced. While the handler runs the
    /// IRET intercept is armed so the exit at its IRET unblocks NMIs again.
    fn inject_nmi(&mut self) -> bool {
        let int_state = unsafe { read_u64(self.vmcb, 0x068) };
        let queued = read_u32(self.vmcb, 0x0a8);
        if self.vcpus[self.cur].nmi_blocked || int_state & 1 != 0 || queued & (1 << 31) != 0 {
            return false;
        }
        unsafe { write_u32(self.vmcb, 0x0a8, (1 << 31) | (2 << 8) | 2) };
        let intercepts = read_u32(self.vmcb, 0x00c);
        unsafe { write_u32(self.vmcb, 0x00c, intercepts | (1 << 20)) };
        self.vcpus[self.cur].nmi_pending = false;
        self.vcpus[self.cur].nmi_blocked = true;
        true
    }

    /// The NMI handler's IRET completed.
    fn nmi_done(&mut self) {
        self.vcpus[self.cur].nmi_blocked = false;
        let intercepts = read_u32(self.vmcb, 0x00c);
        unsafe { write_u32(self.vmcb, 0x00c, intercepts & !(1 << 20)) };
    }

    /// x2APIC ICR write: fixed IPIs, INIT and STARTUP are what Linux uses.
    fn apic_ipi(&mut self, icr: u64) {
        self.vcpus[self.cur].apic.icr = icr;
        let vector = icr as u8;
        let mode = (icr >> 8) & 7;
        let shorthand = (icr >> 18) & 3;
        let destination = (icr >> 32) as u32;
        for target in 0..NUM_VCPUS {
            let selected = match shorthand {
                0 if icr & (1 << 11) != 0 => {
                    // Logical destination (cluster x2APIC): cluster id in the
                    // high half, one bit per CPU in the low half.
                    let id = self.vcpus[target].apic.id;
                    destination == 0xffff_ffff
                        || (destination >> 16 == id >> 4
                            && destination & 0xffff & (1 << (id & 15)) != 0)
                }
                0 => destination == self.vcpus[target].apic.id || destination == 0xffff_ffff,
                1 => target == self.cur,
                2 => true,
                _ => target != self.cur,
            };
            if !selected {
                continue;
            }
            if mode == 5 || mode == 6 {
                crate::serial::format(format_args!(
                    "AEROS_VM_SMP ipi mode={} from={} to={} vector={:#x}\n",
                    mode, self.cur, target, vector
                ));
            }
            match mode {
                0 | 1 => self.apic_deliver(target, vector),
                4 => self.vcpus[target].nmi_pending = true,
                5 if target != 0 => {
                    // INIT: the AP stops and waits for STARTUP.
                    self.vcpus[target].started = false;
                    self.vcpus[target].sipi_wait = true;
                }
                6 if target != 0 && self.vcpus[target].sipi_wait => self.start_ap(target, vector),
                _ => {}
            }
        }
    }

    /// STARTUP IPI: begin the AP in real mode at `vector << 12`.
    fn start_ap(&mut self, index: usize, vector: u8) {
        let vmcb = self.vcpus[index].vmcb;
        let base = (vector as u64) << 12;
        unsafe {
            segment(vmcb, 0x400, 0, 0x0093, 0xffff, 0); // ES
            segment(vmcb, 0x410, (vector as u16) << 8, 0x009b, 0xffff, base); // CS
            segment(vmcb, 0x420, 0, 0x0093, 0xffff, 0); // SS
            segment(vmcb, 0x430, 0, 0x0093, 0xffff, 0); // DS
            segment(vmcb, 0x440, 0, 0x0093, 0xffff, 0); // FS
            segment(vmcb, 0x450, 0, 0x0093, 0xffff, 0); // GS
            segment(vmcb, 0x460, 0, 0, 0xffff, 0); // GDTR
            segment(vmcb, 0x470, 0, 0, 0x3ff, 0); // IDTR
            segment(vmcb, 0x480, 0, 0x0082, 0xffff, 0); // LDTR
            segment(vmcb, 0x490, 0, 0x008b, 0xffff, 0); // TR
            write_u8(vmcb, 0x4cb, 0); // CPL
            write_u64(vmcb, 0x4d0, EFER_SVME);
            write_u64(vmcb, 0x548, 0); // CR4
            write_u64(vmcb, 0x550, 0); // CR3
            write_u64(vmcb, 0x558, 0x6000_0010); // CR0: cache disabled, no PE/PG
            write_u64(vmcb, 0x560, 0x0000_0400); // DR7
            write_u64(vmcb, 0x568, 0xffff_0ff0); // DR6
            write_u64(vmcb, 0x570, 0x0000_0002); // RFLAGS
            write_u64(vmcb, 0x578, 0); // RIP
            write_u64(vmcb, 0x5d8, 0); // RSP
            write_u64(vmcb, 0x5f8, 0); // RAX
            write_u64(vmcb, 0x060, 1 << 24); // V_INTR_MASKING, nothing pending
            write_u64(vmcb, 0x068, 0); // interrupt shadow
            write_u32(vmcb, 0x0c0, 0); // VMCB clean bits: reload everything
            write_u64(vmcb, 0x668, 0x0007_0406_0007_0406); // guest PAT
        }
        self.vcpus[index].gpr = [0; 14];
        self.vcpus[index].started = true;
        self.vcpus[index].halted = false;
        self.vcpus[index].sipi_wait = false;
    }

    fn vcpu_runnable(&self, index: usize) -> bool {
        self.vcpus[index].started && !self.vcpus[index].halted
    }

    /// Makes `to` the running vCPU: its registers, VMCB and FPU state.
    fn switch_vcpu(&mut self, to: usize) {
        let from = self.cur;
        if from == to {
            return;
        }
        self.vcpus[from].gpr = self.gpr;
        unsafe {
            crate::arch::fpu::save_context(self.vcpus[from].fpu as *mut u8);
            crate::arch::fpu::restore_context(self.vcpus[to].fpu as *const u8);
        }
        self.gpr = self.vcpus[to].gpr;
        self.vmcb = self.vcpus[to].vmcb;
        self.cur = to;
        self.slice_exits = 0;
    }

    /// Wakes halted vCPUs that have something to do (an interrupt, a legacy
    /// device interrupt for the BSP) and fires expired APIC timers.
    fn wake_vcpus(&mut self, now: u64) {
        for index in 0..NUM_VCPUS {
            self.apic_timer_poll(index, now);
            if self.vcpus[index].started && self.vcpus[index].halted {
                let legacy = index == 0 && self.has_pending_irq();
                if legacy || self.apic_has_deliverable(index) {
                    self.vcpus[index].halted = false;
                }
            }
        }
    }

    /// Picks the vCPU to run next. False when every vCPU is halted.
    fn smp_schedule(&mut self, now: u64) -> bool {
        // Pointer packets waiting for room in the controller buffer.
        if self.mouse_event_len > 0 {
            self.mouse_flush();
        }
        self.blk_poll();
        self.wake_vcpus(now);
        let current = self.cur;
        let other = 1 - current;
        let current_ok = self.vcpu_runnable(current);
        let other_ok = self.vcpu_runnable(other);
        if current_ok && (self.slice_exits < SMP_SLICE_EXITS || !other_ok) {
            return true;
        }
        if other_ok {
            self.switch_vcpu(other);
            return true;
        }
        false
    }
}
