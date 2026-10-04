use std::arch::asm;
use std::time::{Duration, Instant};

const PAGE: usize = 4096;
const RUN: Duration = Duration::from_millis(600);

fn yield_now() {
    // SAFETY: sched_yield takes no arguments and touches no memory.
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") 24usize => _,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
}

fn pattern(page: usize, version: u32, offset: usize) -> u8 {
    let x = (page as u32).wrapping_mul(0x9e37_79b1)
        ^ version.wrapping_mul(0x85eb_ca6b)
        ^ (offset as u32).wrapping_mul(0xc2b2_ae35);
    ((x >> 13) ^ (x >> 5)) as u8
}

struct Region {
    data: Vec<u8>,
    versions: Vec<u32>,
}

impl Region {
    fn new(pages: usize) -> Self {
        let mut region = Self {
            data: vec![0; pages * PAGE],
            versions: vec![1; pages],
        };
        for page in 0..pages {
            region.write(page);
        }
        region
    }

    fn pages(&self) -> usize {
        self.versions.len()
    }

    fn write(&mut self, page: usize) {
        self.versions[page] += 1;
        let version = self.versions[page];
        for (offset, byte) in self.data[page * PAGE..(page + 1) * PAGE].iter_mut().enumerate() {
            *byte = pattern(page, version, offset);
        }
    }

    fn check(&self, page: usize) -> bool {
        let version = self.versions[page];
        self.data[page * PAGE..(page + 1) * PAGE]
            .iter()
            .enumerate()
            .all(|(offset, byte)| *byte == pattern(page, version, offset))
    }
}

fn main() {
    // One region in the heap (brk), one in an anonymous mapping.
    let mut regions = [Region::new(24), Region::new(40)];
    let started = Instant::now();
    let mut state = 12345u32;
    let (mut checks, mut writes, mut bad) = (0u32, 0u32, 0u32);
    while started.elapsed() < RUN {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let which = (state >> 24) as usize % regions.len();
        let page = (state >> 8) as usize % regions[which].pages();
        if state & 3 == 0 {
            regions[which].write(page);
            writes += 1;
        } else if !regions[which].check(page) {
            bad += 1;
        }
        checks += 1;
        yield_now();
    }
    for region in &regions {
        for page in 0..region.pages() {
            if !region.check(page) {
                bad += 1;
            }
        }
    }
    println!("AEROS_SWAP_PROGRAM checks={checks} writes={writes} bad={bad}");
    std::process::exit(if bad == 0 { 88 } else { 1 });
}
