//! MediaTek MT7921 Wi-Fi (PCI 14c3:7961, the card in many AMD laptops).
//! Powers the chip up, uploads its firmware from the boot partition
//! (`MT7961P.BIN` patch, `MT7961R.BIN` RAM code, from linux-firmware), scans,
//! and joins open and WPA2-Personal networks as a station at legacy rates.
//! Polled. Once joined, `nic` carries Ethernet frames through it like a wired
//! card. The register map and firmware protocol follow Linux's mt76 driver.
//! Every step is logged to `WIFI.LOG` on the boot partition; a `WIFIOFF.TXT`
//! there skips the driver.

use core::fmt::Write as _;
use core::hint::spin_loop;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, Ordering, compiler_fence};

use crate::memory::FrameAllocator;
use crate::pci::PciInventory;
use crate::sync::TicketLock;
use crate::wpa;

const VENDOR: u16 = 0x14c3;
const DEVICE: u16 = 0x7961;

const FIXED_MAP: [(u32, u32, u32); 44] = [
    (0x820d_0000, 0x30000, 0x10000),
    (0x820e_d000, 0x24800, 0x00800),
    (0x820e_4000, 0x21000, 0x00400),
    (0x820e_7000, 0x21e00, 0x00200),
    (0x820e_b000, 0x24200, 0x00400),
    (0x820e_2000, 0x20800, 0x00400),
    (0x820e_3000, 0x20c00, 0x00400),
    (0x820e_5000, 0x21400, 0x00800),
    (0x0040_0000, 0x80000, 0x10000),
    (0x0041_0000, 0x90000, 0x10000),
    (0x4000_0000, 0x70000, 0x10000),
    (0x5400_0000, 0x02000, 0x01000),
    (0x5500_0000, 0x03000, 0x01000),
    (0x5800_0000, 0x06000, 0x01000),
    (0x5900_0000, 0x07000, 0x01000),
    (0x7c00_0000, 0xf0000, 0x10000),
    (0x7c02_0000, 0xd0000, 0x10000),
    (0x7c06_0000, 0xe0000, 0x10000),
    (0x8002_0000, 0xb0000, 0x10000),
    (0x8102_0000, 0xc0000, 0x10000),
    (0x820c_0000, 0x08000, 0x04000),
    (0x820c_8000, 0x0c000, 0x02000),
    (0x820c_c000, 0x0e000, 0x01000),
    (0x820c_d000, 0x0f000, 0x01000),
    (0x7403_0000, 0x10000, 0x10000),
    (0x820c_e000, 0x21c00, 0x00200),
    (0x820c_f000, 0x22000, 0x01000),
    (0x820e_0000, 0x20000, 0x00400),
    (0x820e_1000, 0x20400, 0x00200),
    (0x820e_9000, 0x23400, 0x00200),
    (0x820e_a000, 0x24000, 0x00200),
    (0x820e_c000, 0x24600, 0x00200),
    (0x820f_0000, 0xa0000, 0x00400),
    (0x820f_1000, 0xa0600, 0x00200),
    (0x820f_2000, 0xa0800, 0x00400),
    (0x820f_3000, 0xa0c00, 0x00400),
    (0x820f_4000, 0xa1000, 0x00400),
    (0x820f_5000, 0xa1400, 0x00800),
    (0x820f_7000, 0xa1e00, 0x00200),
    (0x820f_9000, 0xa3400, 0x00200),
    (0x820f_a000, 0xa4000, 0x00200),
    (0x820f_b000, 0xa4200, 0x00400),
    (0x820f_c000, 0xa4600, 0x00200),
    (0x820f_d000, 0xa4800, 0x00800),
];
const HIF_REMAP_L1: u32 = 0xfe24c;
const REMAP_BASE_L1: u32 = 0x40000;

const CONN_ON_LPCTL: u32 = 0x7c06_0010;
const CONN_ON_MISC: u32 = 0x7c06_00f0;
const TOP_LPCR_HOST_BAND0: u32 = 0x1806_0010;
const HW_CHIPID: u32 = 0x7001_0200;
const HW_REV: u32 = 0x7001_0204;
const WFSYS_RESET: u32 = 0x1800_0140;
const SWDEF_MODE: u32 = 0x0041_f23c;
const PCIE_MAC_INT_ENABLE: u32 = 0x10188;
const PCIE_MAC_PM: u32 = 0x10194;
const DMASHDL_SW_CONTROL: u32 = 0x7c02_6004;
const WFDMA_DUMMY_CR: u32 = 0x5400_0120;

const WFDMA0: u32 = 0xd4000;
const WFDMA0_RST: u32 = WFDMA0 + 0x100;
const MCU2HOST_SW_INT_ENA: u32 = WFDMA0 + 0x1f4;
const HOST_INT_ENA: u32 = WFDMA0 + 0x204;
const GLO_CFG: u32 = WFDMA0 + 0x208;
const RST_DTX_PTR: u32 = WFDMA0 + 0x20c;
const GLO_CFG_EXT0: u32 = WFDMA0 + 0x2b0;
const PRI_DLY_INT_CFG0: u32 = WFDMA0 + 0x2f0;
const TX_RING_BASE: u32 = WFDMA0 + 0x300;
const RX_RING_BASE: u32 = WFDMA0 + 0x500;
const TX_EXT_CTRL: u32 = WFDMA0 + 0x600;
const RX_EXT_CTRL: u32 = WFDMA0 + 0x680;

const GLO_TX_DMA_EN: u32 = 1 << 0;
const GLO_TX_BUSY: u32 = 1 << 1;
const GLO_RX_DMA_EN: u32 = 1 << 2;
const GLO_RX_BUSY: u32 = 1 << 3;
const GLO_ENABLE_BITS: u32 = (3 << 4)
    | (1 << 6)
    | (1 << 11)
    | (1 << 12)
    | (1 << 13)
    | (1 << 15)
    | (1 << 21)
    | (1 << 28)
    | (1 << 30);
const GLO_DISABLE_BITS: u32 =
    GLO_TX_DMA_EN | GLO_RX_DMA_EN | (1 << 15) | (1 << 21) | (1 << 27) | (1 << 28);

const DMA_DONE: u32 = 1 << 31;
const LAST_SEC0: u32 = 1 << 30;

const BUFFER: u64 = 2048;
const TX_DATA_RING: u32 = 0;
const TX_FWDL_RING: u32 = 16;
const TX_MCU_RING: u32 = 17;
const TX_DATA_COUNT: u32 = 64;
const TX_MCU_COUNT: u32 = 32;
const RX_COUNTS: [u32; 3] = [16, 64, 32];
const RX_RINGS: [u32; 3] = [0, 2, 4];
const DMA_PAGES: u64 = 2 + 56 + 16 + 32;

const VIF_WCID: u8 = 19;
/// LDPC, 20/40 MHz, SM power save off, short GI 20/40, TX STBC, one RX STBC stream.
const OUR_HT_CAP: u16 = 0x01ef;
/// 3895-byte MPDUs, 80 MHz, RX LDPC, short GI 80, TX STBC, one RX STBC stream, 1 MiB A-MPDU.
const OUR_VHT_CAP: u32 = 0x0380_01b0;
const AP_WCID: u8 = 1;

const MCU_SEQ_OFFSET: usize = 29;
const RXD_HEADER: usize = 36;

const LOG_BYTES: usize = 16 * 1024;
const QUEUE: usize = 16;
const FRAME_MAX: usize = 1600;
pub const MAX_NETWORKS: usize = 32;

#[derive(Clone, Copy)]
enum Cmd {
    Legacy(u8),
    Ext(u8),
    Ce(u8),
    Uni(u16),
}

const CMD_TARGET_ADDRESS_LEN: Cmd = Cmd::Legacy(0x01);
const CMD_FW_START: Cmd = Cmd::Legacy(0x02);
const CMD_NIC_POWER: Cmd = Cmd::Legacy(0x04);
const CMD_PATCH_START: Cmd = Cmd::Legacy(0x05);
const CMD_PATCH_FINISH: Cmd = Cmd::Legacy(0x07);
const CMD_PATCH_SEM: Cmd = Cmd::Legacy(0x10);
const EXT_EFUSE_BUFFER_MODE: Cmd = Cmd::Ext(0x21);
const EXT_PROTECT_CTRL: Cmd = Cmd::Ext(0x3e);
const EXT_MAC_INIT_CTRL: Cmd = Cmd::Ext(0x46);
const EXT_SET_RX_PATH: Cmd = Cmd::Ext(0x4e);
const CE_START_HW_SCAN: Cmd = Cmd::Ce(0x03);
const CE_GET_NIC_CAPAB: Cmd = Cmd::Ce(0x8a);
const UNI_DEV_INFO: Cmd = Cmd::Uni(0x01);
const UNI_BSS_INFO: Cmd = Cmd::Uni(0x02);
const UNI_STA_REC: Cmd = Cmd::Uni(0x03);
const UNI_ROC: Cmd = Cmd::Uni(0x27);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Security {
    Open,
    Wpa2,
    Unsupported,
}

#[derive(Clone, Copy)]
pub struct Network {
    pub ssid: [u8; 32],
    pub ssid_len: usize,
    pub bssid: [u8; 6],
    pub channel: u8,
    pub rssi: i16,
    pub security: Security,
    capability: u16,
    interval: u16,
    dtim: u8,
    rates: [u8; 8],
    rates_len: usize,
    ext_rates: [u8; 16],
    ext_rates_len: usize,
    group_cipher: u8,
    ht_cap: u16,
    ht_ampdu: u8,
    ht_mcs: [u8; 2],
    ht_secondary: u8,
    ht_wide: bool,
    vht_cap: u32,
    vht_rx_mcs: u16,
    vht_tx_mcs: u16,
    vht_width: u8,
    vht_center: u8,
    wmm: bool,
}

impl Network {
    pub const EMPTY: Self = Self {
        ssid: [0; 32],
        ssid_len: 0,
        bssid: [0; 6],
        channel: 0,
        rssi: -100,
        security: Security::Open,
        capability: 0,
        interval: 100,
        dtim: 1,
        rates: [0; 8],
        rates_len: 0,
        ext_rates: [0; 16],
        ext_rates_len: 0,
        group_cipher: 4,
        ht_cap: 0,
        ht_ampdu: 0,
        ht_mcs: [0; 2],
        ht_secondary: 0,
        ht_wide: false,
        vht_cap: 0,
        vht_rx_mcs: 0xffff,
        vht_tx_mcs: 0xffff,
        vht_width: 0,
        vht_center: 0,
        wmm: false,
    };

    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.ssid[..self.ssid_len]).unwrap_or("(unnamed network)")
    }

    pub fn bars(&self) -> u8 {
        match self.rssi {
            -55.. => 3,
            -67..=-56 => 2,
            _ => 1,
        }
    }

    fn band_5g(&self) -> bool {
        self.channel > 14
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Absent,
    Starting,
    Idle,
    Connecting,
    Connected,
}

#[derive(Clone, Copy)]
pub struct Status {
    pub link: Link,
    pub scanning: bool,
    pub ssid: [u8; 32],
    pub ssid_len: usize,
    pub message: &'static str,
}

#[derive(Clone, Copy)]
struct Ring {
    regs: u32,
    desc: u64,
    count: u32,
    buffers: u64,
    head: u32,
}

impl Ring {
    const EMPTY: Self = Self {
        regs: 0,
        desc: 0,
        count: 0,
        buffers: 0,
        head: 0,
    };
}

struct Wifi {
    mmio: u64,
    mac: [u8; 6],
    tx_data: Ring,
    tx_fwdl: Ring,
    tx_mcu: Ring,
    rx: [Ring; 3],
    seq: u8,
    link: Link,
    message: &'static str,
    scanning: bool,
    scan_deadline: u64,
    networks: [Network; MAX_NETWORKS],
    network_count: usize,
    roc_granted: bool,
    roc_token: u8,
    bss: Network,
    aid: u16,
    keys: bool,
    lost: bool,
    tx_busy: [bool; TX_DATA_COUNT as usize],
    mgmt: [u8; 64],
    mgmt_len: usize,
    mgmt_subtype: u8,
    eapol: [u8; wpa::MAX_FRAME],
    eapol_len: usize,
    queue: [[u8; FRAME_MAX]; QUEUE],
    queue_len: [usize; QUEUE],
    queue_head: usize,
    queue_count: usize,
    supplicant: Option<wpa::Supplicant>,
    ht: bool,
    vht: bool,
    qos: bool,
    width: u8,
    center: u8,
    addba: Option<[u8; 7]>,
}

impl Wifi {
    const EMPTY: Self = Self {
        mmio: 0,
        mac: [0; 6],
        tx_data: Ring::EMPTY,
        tx_fwdl: Ring::EMPTY,
        tx_mcu: Ring::EMPTY,
        rx: [Ring::EMPTY; 3],
        seq: 0,
        link: Link::Absent,
        message: "",
        scanning: false,
        scan_deadline: 0,
        networks: [Network::EMPTY; MAX_NETWORKS],
        network_count: 0,
        roc_granted: false,
        roc_token: 0,
        bss: Network::EMPTY,
        aid: 0,
        keys: false,
        lost: false,
        tx_busy: [false; TX_DATA_COUNT as usize],
        mgmt: [0; 64],
        mgmt_len: 0,
        mgmt_subtype: 0,
        eapol: [0; wpa::MAX_FRAME],
        eapol_len: 0,
        queue: [[0; FRAME_MAX]; QUEUE],
        queue_len: [0; QUEUE],
        queue_head: 0,
        queue_count: 0,
        supplicant: None,
        ht: false,
        vht: false,
        qos: false,
        width: 0,
        center: 0,
        addba: None,
    };
}

static WIFI: TicketLock<Wifi> = TicketLock::new(Wifi::EMPTY);
static PRESENT: AtomicBool = AtomicBool::new(false);
static CONNECTED: AtomicBool = AtomicBool::new(false);

struct Log {
    bytes: [u8; LOG_BYTES],
    len: usize,
}

static LOG: TicketLock<Log> = TicketLock::new(Log {
    bytes: [0; LOG_BYTES],
    len: 0,
});

struct Line {
    bytes: [u8; 192],
    len: usize,
}

impl core::fmt::Write for Line {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let take = text.len().min(self.bytes.len() - self.len);
        self.bytes[self.len..self.len + take].copy_from_slice(&text.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

fn note(arguments: core::fmt::Arguments<'_>) {
    let mut line = Line {
        bytes: [0; 192],
        len: 0,
    };
    let seconds = crate::time::monotonic_nanoseconds() / 1_000_000;
    let _ = write!(line, "[{}.{:03}] ", seconds / 1000, seconds % 1000);
    let _ = line.write_fmt(arguments);
    let _ = line.write_str("\n");
    if let Ok(text) = core::str::from_utf8(&line.bytes[..line.len]) {
        crate::serial::format(format_args!("AEROS_WIFI {text}"));
    }
    let mut log = LOG.lock();
    if log.len + line.len > LOG_BYTES {
        log.len = 0;
    }
    let at = log.len;
    log.bytes[at..at + line.len].copy_from_slice(&line.bytes[..line.len]);
    log.len += line.len;
}

macro_rules! note {
    ($($argument:tt)*) => { note(format_args!($($argument)*)) };
}

/// Saves the log to `WIFI.LOG` on the boot partition.
pub fn save_log() {
    let log = LOG.lock();
    if log.len != 0 {
        let _ = crate::fat::write_root_file(b"WIFI    LOG", &log.bytes[..log.len]);
    }
}

fn now_ms() -> u64 {
    crate::time::monotonic_nanoseconds() / 1_000_000
}

fn sleep_ms(ms: u64) {
    let end = now_ms() + ms;
    while now_ms() < end {
        spin_loop();
    }
}

fn le16(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

fn le32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn put16(data: &mut [u8], at: usize, value: u16) {
    data[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

impl Wifi {
    fn raw_read(&self, offset: u32) -> u32 {
        // SAFETY: `offset` is inside BAR0 (1 MiB), which is identity-mapped MMIO.
        unsafe { read_volatile((self.mmio + offset as u64) as *const u32) }
    }

    fn raw_write(&self, offset: u32, value: u32) {
        // SAFETY: as in `raw_read`.
        unsafe { write_volatile((self.mmio + offset as u64) as *mut u32, value) }
    }

    fn map(&self, address: u32) -> u32 {
        if address < 0x10_0000 {
            return address;
        }
        for (physical, mapped, size) in FIXED_MAP {
            if address >= physical && address - physical <= size {
                return mapped + (address - physical);
            }
        }
        if (0x1800_0000..0x18c0_0000).contains(&address)
            || (0x7000_0000..0x7800_0000).contains(&address)
            || (0x7c00_0000..0x7c40_0000).contains(&address)
        {
            let current = self.raw_read(HIF_REMAP_L1);
            self.raw_write(HIF_REMAP_L1, (current & !0xffff) | (address >> 16));
            let _ = self.raw_read(HIF_REMAP_L1);
            return REMAP_BASE_L1 + (address & 0xffff);
        }
        0
    }

    fn read(&self, address: u32) -> u32 {
        self.raw_read(self.map(address))
    }

    fn write(&self, address: u32, value: u32) {
        self.raw_write(self.map(address), value)
    }

    fn set(&self, address: u32, bits: u32) {
        let offset = self.map(address);
        self.raw_write(offset, self.raw_read(offset) | bits);
    }

    fn clear(&self, address: u32, bits: u32) {
        let offset = self.map(address);
        self.raw_write(offset, self.raw_read(offset) & !bits);
    }

    fn poll(&self, address: u32, mask: u32, value: u32, ms: u64) -> bool {
        let end = now_ms() + ms;
        loop {
            if self.read(address) & mask == value {
                return true;
            }
            if now_ms() >= end {
                return false;
            }
            spin_loop();
        }
    }

    fn power_up(&self) -> bool {
        let mut firmware_own = false;
        for _ in 0..10 {
            self.write(CONN_ON_LPCTL, 1);
            if self.poll(CONN_ON_LPCTL, 4, 4, 50) {
                firmware_own = true;
                break;
            }
        }
        let mut driver_own = false;
        for _ in 0..10 {
            self.write(CONN_ON_LPCTL, 2);
            if self.poll(CONN_ON_LPCTL, 4, 0, 50) {
                driver_own = true;
                break;
            }
        }
        note!(
            "power: firmware_own={} driver_own={}",
            firmware_own,
            driver_own
        );
        driver_own
    }

    fn wfsys_reset(&self) -> bool {
        self.clear(WFSYS_RESET, 1);
        sleep_ms(50);
        self.set(WFSYS_RESET, 1);
        self.poll(WFSYS_RESET, 1 << 4, 1 << 4, 500)
    }

    fn ring_desc(ring: &Ring, index: u32) -> *mut u32 {
        (ring.desc + index as u64 * 16) as *mut u32
    }

    fn write_desc(ring: &Ring, index: u32, buf0: u32, ctrl: u32) {
        let desc = Self::ring_desc(ring, index);
        // SAFETY: the descriptor lies in this ring's DMA page.
        unsafe {
            write_volatile(desc, buf0);
            write_volatile(desc.add(2), 0);
            write_volatile(desc.add(3), 0);
            compiler_fence(Ordering::SeqCst);
            write_volatile(desc.add(1), ctrl);
        }
        compiler_fence(Ordering::SeqCst);
    }

    fn read_ctrl(ring: &Ring, index: u32) -> u32 {
        // SAFETY: as in `write_desc`.
        unsafe { read_volatile(Self::ring_desc(ring, index).add(1)) }
    }

    fn setup_tx(&self, ring: &mut Ring) {
        for index in 0..ring.count {
            Self::write_desc(ring, index, 0, DMA_DONE);
        }
        self.write(ring.regs + 4, ring.count);
        self.write(ring.regs, ring.desc as u32);
        self.write(ring.regs + 8, 0);
        self.write(ring.regs + 12, 0);
        ring.head = 0;
    }

    fn setup_rx(&self, ring: &mut Ring) {
        for index in 0..ring.count {
            Self::write_desc(
                ring,
                index,
                (ring.buffers + index as u64 * BUFFER) as u32,
                (BUFFER as u32) << 16,
            );
        }
        self.write(ring.regs + 4, ring.count);
        self.write(ring.regs, ring.desc as u32);
        self.write(ring.regs + 12, 0);
        self.write(ring.regs + 8, ring.count - 1);
        ring.head = 0;
    }

    fn dma_disable(&self) -> bool {
        self.clear(GLO_CFG, GLO_DISABLE_BITS);
        let idle = self.poll(GLO_CFG, GLO_TX_BUSY | GLO_RX_BUSY, 0, 100);
        self.clear(GLO_CFG_EXT0, 1 << 6);
        self.set(DMASHDL_SW_CONTROL, 1 << 28);
        self.clear(WFDMA0_RST, (1 << 4) | (1 << 5));
        self.set(WFDMA0_RST, (1 << 4) | (1 << 5));
        idle
    }

    fn dma_enable(&self) {
        for (ring, base) in [(0u32, 0x0u32), (2, 0x40), (3, 0x80), (4, 0xc0), (5, 0x100)] {
            self.write(RX_EXT_CTRL + ring * 4, (base << 16) | 4);
        }
        for (ring, base) in [
            (0u32, 0x140u32),
            (1, 0x180),
            (2, 0x1c0),
            (3, 0x200),
            (4, 0x240),
            (5, 0x280),
            (6, 0x2c0),
            (16, 0x340),
            (17, 0x380),
        ] {
            self.write(TX_EXT_CTRL + ring * 4, (base << 16) | 4);
        }
        self.write(RST_DTX_PTR, !0);
        self.write(PRI_DLY_INT_CFG0, 0);
        self.set(GLO_CFG, GLO_ENABLE_BITS);
        self.set(GLO_CFG, GLO_TX_DMA_EN | GLO_RX_DMA_EN);
        self.set(WFDMA_DUMMY_CR, 1 << 1);
        self.set(MCU2HOST_SW_INT_ENA, 1);
    }

    /// Queues one buffer on a TX ring, waiting while the ring is full.
    fn tx_push(&mut self, which: u32, buffer: u64, len: usize) -> bool {
        let mut ring = match which {
            TX_FWDL_RING => self.tx_fwdl,
            TX_MCU_RING => self.tx_mcu,
            _ => self.tx_data,
        };
        let next = (ring.head + 1) % ring.count;
        let end = now_ms() + 500;
        while self.read(ring.regs + 12) == next {
            if now_ms() > end {
                note!("tx ring {} stuck", which);
                return false;
            }
            spin_loop();
        }
        Self::write_desc(
            &ring,
            ring.head,
            buffer as u32,
            ((len as u32) << 16) | LAST_SEC0,
        );
        ring.head = next;
        self.write(ring.regs + 8, ring.head);
        match which {
            TX_FWDL_RING => self.tx_fwdl = ring,
            TX_MCU_RING => self.tx_mcu = ring,
            _ => self.tx_data = ring,
        }
        true
    }

    fn mcu_send(&mut self, cmd: Cmd, payload: &[u8]) -> Option<u8> {
        let header = if let Cmd::Uni(_) = cmd { 48 } else { 64 };
        let total = header + payload.len();
        if total > BUFFER as usize {
            return None;
        }
        self.seq = (self.seq + 1) & 0xf;
        if self.seq == 0 {
            self.seq = 1;
        }
        let seq = self.seq;
        let slot = self.tx_mcu.buffers + self.tx_mcu.head as u64 * BUFFER;
        // SAFETY: one of the MCU ring's own 2 KiB DMA buffers.
        let buffer = unsafe { core::slice::from_raw_parts_mut(slot as *mut u8, total) };
        buffer[..header].fill(0);
        buffer[header..].copy_from_slice(payload);
        put32(buffer, 0, total as u32 | (2 << 23) | (0x20 << 25));
        put32(buffer, 4, (1 << 31) | (1 << 16));
        put16(buffer, 32, (total - 32) as u16);
        match cmd {
            Cmd::Uni(id) => {
                put16(buffer, 34, id);
                buffer[37] = 0xa0;
                buffer[39] = seq;
                buffer[43] = 7;
            }
            Cmd::Legacy(id) | Cmd::Ext(id) | Cmd::Ce(id) => {
                put16(buffer, 34, 0x8000);
                buffer[37] = 0xa0;
                buffer[39] = seq;
                match cmd {
                    Cmd::Ext(_) => {
                        buffer[36] = 0xed;
                        buffer[38] = 1;
                        buffer[41] = id;
                        buffer[43] = 1;
                    }
                    Cmd::Ce(_) => {
                        buffer[36] = id;
                        buffer[38] = 1;
                    }
                    _ => {
                        buffer[36] = id;
                        buffer[38] = 3;
                    }
                }
            }
        }
        self.tx_push(TX_MCU_RING, slot, total).then_some(seq)
    }

    /// Sends a command and copies its response (with the 36-byte event header) into `out`.
    fn mcu_call(&mut self, cmd: Cmd, payload: &[u8], out: &mut [u8]) -> Option<usize> {
        let seq = self.mcu_send(cmd, payload)?;
        let end = now_ms() + 3000;
        while now_ms() < end {
            if let Some(len) = self.service(seq, out) {
                return Some(len);
            }
            spin_loop();
        }
        None
    }

    fn mcu_ok(&mut self, what: &str, cmd: Cmd, payload: &[u8]) -> bool {
        let mut response = [0u8; 256];
        let Some(len) = self.mcu_call(cmd, payload, &mut response) else {
            note!("{}: no answer", what);
            return false;
        };
        if let Cmd::Uni(_) = cmd
            && len >= RXD_HEADER + 8
        {
            let status = le32(&response, RXD_HEADER + 4);
            if status != 0 {
                note!("{}: status {}", what, status);
                return false;
            }
        }
        true
    }

    /// Drains every RX ring once. A response to `want` is copied into `out`.
    fn service(&mut self, want: u8, out: &mut [u8]) -> Option<usize> {
        let mut answer = None;
        for which in 0..3 {
            let mut budget = self.rx[which].count;
            while budget > 0 {
                budget -= 1;
                let ring = self.rx[which];
                let ctrl = Self::read_ctrl(&ring, ring.head);
                if ctrl & DMA_DONE == 0 {
                    break;
                }
                let len = (((ctrl >> 16) & 0x3fff) as usize).min(BUFFER as usize);
                let address = ring.buffers + ring.head as u64 * BUFFER;
                if ctrl & LAST_SEC0 != 0 && len >= 8 {
                    // SAFETY: the device finished writing this RX buffer.
                    let data = unsafe { core::slice::from_raw_parts(address as *const u8, len) };
                    if let Some(got) = self.dispatch(data, want, out) {
                        answer = Some(got);
                    }
                }
                Self::write_desc(&ring, ring.head, address as u32, (BUFFER as u32) << 16);
                self.write(ring.regs + 8, ring.head);
                self.rx[which].head = (ring.head + 1) % ring.count;
                if answer.is_some() {
                    return answer;
                }
            }
        }
        answer
    }

    fn dispatch(&mut self, data: &[u8], want: u8, out: &mut [u8]) -> Option<usize> {
        let rxd0 = le32(data, 0);
        let kind = rxd0 >> 27;
        let flag = (rxd0 >> 16) & 0xf;
        match kind {
            7 if flag != 1 => {
                if data.len() < RXD_HEADER {
                    return None;
                }
                let eid = data[28];
                let seq = data[MCU_SEQ_OFFSET];
                let option = data[30];
                let unsolicited = option & 4 != 0
                    || seq == 0
                    || matches!(eid, 0x0d | 0x0f | 0x13 | 0x23 | 0x27 | 0x07 | 0x96 | 0xf0);
                if !unsolicited && want != 0 && seq == want {
                    let len = data.len().min(out.len());
                    out[..len].copy_from_slice(&data[..len]);
                    return Some(len);
                }
                self.event(eid, option, data);
            }
            6 => self.tx_free(data),
            2 | 7 | 8 => self.frame(data),
            _ => {}
        }
        None
    }

    fn event(&mut self, eid: u8, option: u8, _data: &[u8]) {
        if option & 4 != 0 {
            if eid == 0x27 {
                self.roc_granted = true;
            }
            return;
        }
        match eid {
            0x0d if self.scanning => {
                self.scanning = false;
                note!("scan done: {} networks", self.network_count);
            }
            0x13 if self.link == Link::Connected => {
                note!("beacon loss");
                self.lost = true;
            }
            _ => {}
        }
    }

    fn tx_free(&mut self, data: &[u8]) {
        let mut count = ((le32(data, 0) >> 16) & 0x3ff) as usize;
        let mut index = 0;
        while index < count && 8 + 4 * index + 4 <= data.len() {
            let info = le32(data, 8 + 4 * index);
            if info & (1 << 31) != 0 {
                count += 1;
            } else {
                let token = ((info >> 16) & 0x7fff) as usize;
                if token < self.tx_busy.len() {
                    self.tx_busy[token] = false;
                }
            }
            index += 1;
        }
    }

    fn frame(&mut self, data: &[u8]) {
        if data.len() < 24 {
            return;
        }
        let rxd1 = le32(data, 4);
        let rxd2 = le32(data, 8);
        let rxd3 = le32(data, 12);
        if rxd1 & ((1 << 27) | (1 << 25)) != 0 {
            return;
        }
        // Decrypted by the hardware with the installed key: a cipher was
        // applied and neither its kind nor its length was off.
        let decrypted = (rxd1 >> 16) & 0x1f != 0 && rxd1 & ((1 << 23) | (1 << 24)) == 0;
        let total = ((le32(data, 0) & 0xffff) as usize).clamp(24, data.len());
        let data = &data[..total];
        let mut at = 24;
        let mut rcpi = 0u32;
        if rxd1 & (1 << 14) != 0 {
            at += 16;
        }
        if rxd1 & (1 << 11) != 0 {
            at += 16;
        }
        if rxd1 & (1 << 12) != 0 {
            at += 8;
        }
        if rxd1 & (1 << 13) != 0 {
            if at + 8 <= data.len() {
                rcpi = le32(data, at + 4) & 0xff;
            }
            at += 8;
            if rxd1 & (1 << 15) != 0 {
                if at + 28 <= data.len() {
                    rcpi = le32(data, at + 24) & 0xff;
                }
                at += 72;
            }
        }
        at += ((rxd2 >> 14) & 3) as usize * 2;
        if at >= data.len() {
            return;
        }
        let body = &data[at..];
        let rssi = rcpi as i16 / 2 - 110;
        let channel = ((rxd3 >> 8) & 0xff) as u8;
        if rxd2 & (1 << 13) != 0 {
            if body.len() >= 14 {
                let ethertype = u16::from_be_bytes([body[12], body[13]]);
                self.deliver(&body[..6], &body[6..12], ethertype, &body[14..], decrypted);
            }
            return;
        }
        if body.len() < 24 {
            return;
        }
        let fc = body[0];
        let flags = body[1];
        let kind = (fc >> 2) & 3;
        let subtype = fc >> 4;
        let from_bss = self.link != Link::Idle && body[10..16] == self.bss.bssid;
        match kind {
            0 => match subtype {
                8 | 5 => self.scan_result(body, channel, rssi),
                13 if from_bss
                    && body[4..10] == self.mac
                    && body.len() >= 33
                    && body[24] == 3
                    && body[25] == 0 =>
                {
                    let mut request = [0u8; 7];
                    request.copy_from_slice(&body[26..33]);
                    self.addba = Some(request);
                }
                11 | 1 | 3 if from_bss && body[4..10] == self.mac => {
                    let len = (body.len() - 24).min(self.mgmt.len());
                    self.mgmt[..len].copy_from_slice(&body[24..24 + len]);
                    self.mgmt_len = len;
                    self.mgmt_subtype = subtype;
                }
                10 | 12 if from_bss && self.link == Link::Connected => {
                    note!(
                        "disconnected by the access point (reason {})",
                        body.get(24).copied().unwrap_or(0)
                    );
                    self.lost = true;
                }
                _ => {}
            },
            2 if from_bss && flags & 3 == 2 && subtype & 4 == 0 => {
                let mut header = 24;
                if subtype & 8 != 0 {
                    header += 2;
                    if flags & 0x80 != 0 {
                        header += 4;
                    }
                }
                if body.len() < header + 8 {
                    return;
                }
                let mut payload = &body[header..];
                let snap = |bytes: &[u8]| {
                    bytes.len() >= 8 && bytes[..3] == [0xaa, 0xaa, 0x03] && bytes[3..5] == [0, 0]
                };
                if !snap(payload) && flags & 0x40 != 0 && snap(&payload[8.min(payload.len())..]) {
                    payload = &payload[8..];
                }
                if !snap(payload) {
                    return;
                }
                let ethertype = u16::from_be_bytes([payload[6], payload[7]]);
                let destination: [u8; 6] = body[4..10].try_into().unwrap_or([0; 6]);
                let source: [u8; 6] = body[16..22].try_into().unwrap_or([0; 6]);
                self.deliver(&destination, &source, ethertype, &payload[8..], decrypted);
            }
            _ => {}
        }
    }

    fn deliver(
        &mut self,
        destination: &[u8],
        source: &[u8],
        ethertype: u16,
        payload: &[u8],
        decrypted: bool,
    ) {
        if ethertype == 0x888e {
            let len = payload.len().min(self.eapol.len());
            self.eapol[..len].copy_from_slice(&payload[..len]);
            self.eapol_len = len;
            return;
        }
        if self.link != Link::Connected || payload.len() + 14 > FRAME_MAX {
            return;
        }
        // On a protected network a plain frame is someone else's forgery.
        if self.bss.security != Security::Open && !decrypted {
            return;
        }
        if self.queue_count == QUEUE {
            self.queue_head = (self.queue_head + 1) % QUEUE;
            self.queue_count -= 1;
        }
        let slot = (self.queue_head + self.queue_count) % QUEUE;
        let frame = &mut self.queue[slot];
        frame[..6].copy_from_slice(&destination[..6]);
        frame[6..12].copy_from_slice(&source[..6]);
        frame[12..14].copy_from_slice(&ethertype.to_be_bytes());
        frame[14..14 + payload.len()].copy_from_slice(payload);
        self.queue_len[slot] = 14 + payload.len();
        self.queue_count += 1;
    }

    fn scan_result(&mut self, body: &[u8], rx_channel: u8, rssi: i16) {
        if body.len() < 36 {
            return;
        }
        let mut network = Network::EMPTY;
        network.bssid.copy_from_slice(&body[16..22]);
        network.interval = le16(body, 32);
        network.capability = le16(body, 34);
        network.rssi = rssi;
        network.channel = rx_channel;
        let mut rsn: Option<&[u8]> = None;
        let mut at = 36;
        while at + 2 <= body.len() {
            let id = body[at];
            let len = body[at + 1] as usize;
            let Some(value) = body.get(at + 2..at + 2 + len) else {
                break;
            };
            match id {
                0 if len <= 32 => {
                    network.ssid[..len].copy_from_slice(value);
                    network.ssid_len = len;
                }
                1 => {
                    let take = len.min(8);
                    network.rates[..take].copy_from_slice(&value[..take]);
                    network.rates_len = take;
                }
                3 if len >= 1 => network.channel = value[0],
                5 if len >= 2 => network.dtim = value[1].max(1),
                48 => rsn = Some(value),
                45 if len >= 19 => {
                    network.ht_cap = le16(value, 0);
                    network.ht_ampdu = value[2];
                    network.ht_mcs = [value[3], value[4]];
                }
                61 if len >= 2 => {
                    network.ht_secondary = value[1] & 3;
                    network.ht_wide = value[1] & 4 != 0;
                }
                191 if len >= 12 => {
                    network.vht_cap = le32(value, 0);
                    network.vht_rx_mcs = le16(value, 4);
                    network.vht_tx_mcs = le16(value, 8);
                }
                192 if len >= 2 => {
                    network.vht_width = value[0];
                    network.vht_center = value[1];
                }
                221 if len >= 5 && value[..5] == [0x00, 0x50, 0xf2, 0x02, 0x01] => {
                    network.wmm = true;
                }
                50 => {
                    let take = len.min(16);
                    network.ext_rates[..take].copy_from_slice(&value[..take]);
                    network.ext_rates_len = take;
                }
                _ => {}
            }
            at += 2 + len;
        }
        if network.ssid_len == 0
            || network.ssid[..network.ssid_len]
                .iter()
                .all(|byte| *byte == 0)
        {
            return;
        }
        network.security = match rsn {
            Some(rsn) => parse_rsn(rsn, &mut network.group_cipher),
            None if network.capability & 0x10 != 0 => Security::Unsupported,
            None => Security::Open,
        };
        if let Some(existing) = self.networks[..self.network_count]
            .iter_mut()
            .find(|known| known.bssid == network.bssid)
        {
            *existing = network;
            return;
        }
        if self.network_count < MAX_NETWORKS {
            self.networks[self.network_count] = network;
            self.network_count += 1;
        } else if let Some(weakest) = self.networks.iter_mut().min_by_key(|known| known.rssi)
            && weakest.rssi < network.rssi
        {
            *weakest = network;
        }
    }

    fn firmware(
        &mut self,
        frames: &mut FrameAllocator,
        name: &[u8; 11],
        pages: u64,
    ) -> Option<(u64, usize)> {
        let size = crate::fat::root_file_size(name)? as usize;
        if size as u64 > pages * 4096 {
            note!("firmware file too large: {}", size);
            return None;
        }
        let memory = frames.allocate_dma(pages, 1)?;
        let address = memory.address();
        if address + pages * 4096 > 0x1_0000_0000 {
            note!("firmware buffer above 4 GiB");
            frames.release_contiguous(address, pages);
            return None;
        }
        let read = crate::fat::load_root_file(name, 0, address, pages * 4096)? as usize;
        if read != size {
            note!("firmware read {} of {} bytes", read, size);
            frames.release_contiguous(address, pages);
            return None;
        }
        Some((address, size))
    }

    fn scatter(&mut self, address: u64, len: usize) -> bool {
        let mut sent = 0;
        while sent < len {
            let chunk = (len - sent).min(4096);
            self.seq = (self.seq + 1) & 0xf;
            if !self.tx_push(TX_FWDL_RING, address + sent as u64, chunk) {
                return false;
            }
            sent += chunk;
        }
        true
    }

    fn download(&mut self, what: &str, address: u32, len: u32, mode: u32) -> bool {
        let mut request = [0u8; 12];
        put32(&mut request, 0, address);
        put32(&mut request, 4, len);
        put32(&mut request, 8, mode);
        let cmd = if address == 0x20_0000 || address == 0x90_0000 {
            CMD_PATCH_START
        } else {
            CMD_TARGET_ADDRESS_LEN
        };
        self.mcu_ok(what, cmd, &request)
    }

    fn load_patch(&mut self, base: u64, size: usize) -> bool {
        // SAFETY: the firmware buffer this driver just filled.
        let file = unsafe { core::slice::from_raw_parts(base as *const u8, size) };
        let mut response = [0u8; 64];
        let Some(len) = self.mcu_call(CMD_PATCH_SEM, &1u32.to_le_bytes(), &mut response) else {
            note!("patch semaphore: no answer");
            return false;
        };
        let semaphore = if len > 32 { response[32] } else { 0xff };
        note!("patch semaphore {}", semaphore);
        if semaphore == 1 {
            return true;
        }
        if semaphore != 2 || size < 96 {
            return false;
        }
        let regions = u32::from_be_bytes(file[44..48].try_into().unwrap_or([0; 4])) as usize;
        note!(
            "patch build {}, {} regions",
            core::str::from_utf8(&file[..14]).unwrap_or("?"),
            regions
        );
        let mut ok = true;
        for index in 0..regions {
            let at = 96 + index * 64;
            if at + 64 > size {
                ok = false;
                break;
            }
            let be = |offset: usize| {
                u32::from_be_bytes(
                    file[at + offset..at + offset + 4]
                        .try_into()
                        .unwrap_or([0; 4]),
                )
            };
            if be(0) & 0xffff != 2 {
                ok = false;
                break;
            }
            let offset = be(4) as usize;
            let address = be(12);
            let len = be(16);
            let key = be(20);
            let mut mode = 1u32 << 31;
            if key != u32::MAX {
                match key >> 24 {
                    1 => mode |= 1 | (((key & 0xff) << 1) & 6) | (1 << 3),
                    2 => mode |= 1 | (1 << 6) | (1 << 3),
                    _ => {}
                }
            }
            if offset + len as usize > size
                || !self.download("patch region", address, len, mode)
                || !self.scatter(base + offset as u64, len as usize)
            {
                ok = false;
                break;
            }
        }
        if ok {
            let finish = self.mcu_call(CMD_PATCH_FINISH, &[0; 4], &mut response);
            ok = matches!(finish, Some(len) if len > 32 && response[32] == 0);
            note!("patch finish ok={}", ok);
        }
        let released = self.mcu_call(CMD_PATCH_SEM, &0u32.to_le_bytes(), &mut response);
        note!(
            "patch semaphore release {}",
            released.map_or(0xff, |len| if len > 32 { response[32] } else { 0xfe })
        );
        ok
    }

    fn load_ram(&mut self, base: u64, size: usize) -> bool {
        // SAFETY: the firmware buffer this driver just filled.
        let file = unsafe { core::slice::from_raw_parts(base as *const u8, size) };
        if size < 36 {
            return false;
        }
        let trailer = size - 36;
        let regions = file[trailer + 2] as usize;
        note!(
            "ram firmware {} built {}, {} regions",
            core::str::from_utf8(&file[trailer + 7..trailer + 17]).unwrap_or("?"),
            core::str::from_utf8(&file[trailer + 17..trailer + 32]).unwrap_or("?"),
            regions
        );
        let mut offset = 0usize;
        let mut override_address = 0u32;
        for index in 0..regions {
            let Some(at) = trailer.checked_sub((regions - index) * 40) else {
                return false;
            };
            let address = le32(file, at + 16);
            let len = le32(file, at + 20) as usize;
            let features = file[at + 24];
            if features & (1 << 6) == 0 {
                if features & (1 << 5) != 0 {
                    override_address = address;
                }
                let mut mode = 1u32 << 31;
                if features & 1 != 0 {
                    mode |= 1 | (1 << 3);
                }
                if features & (1 << 4) != 0 {
                    mode |= 1 << 6;
                }
                mode |= (((features >> 1) & 3) as u32) << 1;
                if offset + len > size
                    || !self.download("ram region", address, len as u32, mode)
                    || !self.scatter(base + offset as u64, len)
                {
                    return false;
                }
            }
            offset += len;
        }
        let mut request = [0u8; 8];
        put32(&mut request, 0, u32::from(override_address != 0));
        put32(&mut request, 4, override_address);
        self.mcu_ok("firmware start", CMD_FW_START, &request)
    }

    fn bring_up(&mut self, frames: &mut FrameAllocator) -> bool {
        if !self.power_up() {
            return false;
        }
        let chip = self.read(HW_CHIPID);
        let revision = self.read(HW_REV) & 0xff;
        note!("chip {:#x} revision {:#x}", chip, revision);
        if chip & 0xffff != 0x7961 {
            return false;
        }
        if !self.wfsys_reset() {
            note!("wfsys reset timed out");
            return false;
        }
        self.write(HOST_INT_ENA, 0);
        self.write(PCIE_MAC_INT_ENABLE, 0xff);
        let Some(memory) = frames.allocate_dma(DMA_PAGES, 1) else {
            note!("no DMA memory");
            return false;
        };
        let base = memory.address();
        if base + DMA_PAGES * 4096 > 0x1_0000_0000 {
            note!("DMA memory above 4 GiB: {:#x}", base);
            return false;
        }
        // SAFETY: freshly allocated, identity-mapped pages.
        unsafe { core::ptr::write_bytes(base as *mut u8, 0, (DMA_PAGES * 4096) as usize) };
        let mut buffers = base + 2 * 4096;
        let mut take = |count: u32| {
            let start = buffers;
            buffers += count as u64 * BUFFER;
            start
        };
        let mut rx = [Ring::EMPTY; 3];
        for (index, ring) in rx.iter_mut().enumerate() {
            ring.regs = RX_RING_BASE + RX_RINGS[index] * 0x10;
            ring.count = RX_COUNTS[index];
            ring.desc = base + [0x800u64, 0x1000, 0xa00][index];
            ring.buffers = take(ring.count);
        }
        let mut tx_mcu = Ring {
            regs: TX_RING_BASE + TX_MCU_RING * 0x10,
            desc: base + 0x600,
            count: TX_MCU_COUNT,
            buffers: take(TX_MCU_COUNT),
            head: 0,
        };
        let mut tx_data = Ring {
            regs: TX_RING_BASE + TX_DATA_RING * 0x10,
            desc: base,
            count: TX_DATA_COUNT,
            buffers: take(TX_DATA_COUNT),
            head: 0,
        };
        let mut tx_fwdl = Ring {
            regs: TX_RING_BASE + TX_FWDL_RING * 0x10,
            desc: base + 0x400,
            count: 32,
            buffers: 0,
            head: 0,
        };
        if !self.dma_disable() {
            note!("DMA did not go idle");
        }
        self.setup_tx(&mut tx_data);
        self.write(TX_EXT_CTRL, 4);
        self.setup_tx(&mut tx_mcu);
        self.setup_tx(&mut tx_fwdl);
        for ring in rx.iter_mut() {
            self.setup_rx(ring);
        }
        self.tx_data = tx_data;
        self.tx_mcu = tx_mcu;
        self.tx_fwdl = tx_fwdl;
        self.rx = rx;
        self.dma_enable();
        note!("DMA rings at {:#x}", base);

        self.write(SWDEF_MODE, 0);
        let lpcr = TOP_LPCR_HOST_BAND0;
        self.write(lpcr, 2);
        if !self.poll(lpcr, 1, 0, 500) {
            note!("driver own timed out");
            return false;
        }
        self.set(PCIE_MAC_PM, 1 << 8);
        let _ = self.mcu_send(CMD_NIC_POWER, &[1, 0, 0, 0]);
        if !self.poll(CONN_ON_MISC, 7, 1, 1000) {
            note!(
                "MCU not ready for download (misc {:#x})",
                self.read(CONN_ON_MISC)
            );
        }
        let Some((patch, patch_size)) = self.firmware(frames, b"MT7961P BIN", 32) else {
            note!("MT7961P.BIN missing from the boot partition");
            return false;
        };
        let patched = self.load_patch(patch, patch_size);
        frames.release_contiguous(patch, 32);
        if !patched {
            return false;
        }
        let Some((ram, ram_size)) = self.firmware(frames, b"MT7961R BIN", 256) else {
            note!("MT7961R.BIN missing from the boot partition");
            return false;
        };
        let started = self.load_ram(ram, ram_size);
        frames.release_contiguous(ram, 256);
        if !started {
            return false;
        }
        if !self.poll(CONN_ON_MISC, 3, 3, 1500) {
            note!(
                "firmware did not report ready (misc {:#x})",
                self.read(CONN_ON_MISC)
            );
            return false;
        }
        note!("firmware running");
        self.capabilities() && self.start_radio()
    }

    fn capabilities(&mut self) -> bool {
        let mut response = [0u8; 512];
        let Some(len) = self.mcu_call(CE_GET_NIC_CAPAB, &[], &mut response) else {
            note!("capabilities: no answer");
            return false;
        };
        let data = &response[..len];
        if data.len() < RXD_HEADER + 4 {
            return false;
        }
        let elements = le16(data, RXD_HEADER) as usize;
        let mut at = RXD_HEADER + 4;
        for _ in 0..elements {
            if at + 8 > data.len() {
                break;
            }
            let kind = le32(data, at);
            let size = le32(data, at + 4) as usize;
            at += 8;
            if at + size > data.len() {
                break;
            }
            if kind == 7 && size >= 6 {
                self.mac.copy_from_slice(&data[at..at + 6]);
            }
            at += size;
        }
        let mac = self.mac;
        note!(
            "address {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0],
            mac[1],
            mac[2],
            mac[3],
            mac[4],
            mac[5]
        );
        mac != [0; 6]
    }

    fn start_radio(&mut self) -> bool {
        let efuse = [0u8, 1, 0, 0];
        let mut rts = [0u8; 12];
        rts[0] = 1;
        put32(&mut rts, 4, 0x92b);
        put32(&mut rts, 8, 2);
        if !self.mcu_ok("efuse mode", EXT_EFUSE_BUFFER_MODE, &efuse)
            || !self.mcu_ok("rts threshold", EXT_PROTECT_CTRL, &rts)
            || !self.mcu_ok("mac enable", EXT_MAC_INIT_CTRL, &[1, 0, 0, 0])
            || !self.set_channel(EXT_SET_RX_PATH, 1)
        {
            return false;
        }
        let mut device = [0u8; 16];
        put16(&mut device, 4, 0);
        put16(&mut device, 6, 12);
        device[8] = 1;
        device[10..16].copy_from_slice(&self.mac);
        let mut basic = [0u8; 36];
        self.bss_basic(&mut basic, true, false);
        self.mcu_ok("device info", UNI_DEV_INFO, &device)
            && self.mcu_ok("bss info", UNI_BSS_INFO, &basic)
    }

    fn set_channel(&mut self, cmd: Cmd, channel: u8) -> bool {
        let mut request = [0u8; 76];
        request[0] = channel;
        request[1] = channel;
        request[3] = 2;
        request[4] = 3;
        request[10] = u8::from(channel > 14);
        self.mcu_ok("channel", cmd, &request)
    }

    /// The BSS_INFO basic element (with its 4-byte header) for this station.
    fn bss_basic(&self, out: &mut [u8; 36], adding_device: bool, joined: bool) {
        out.fill(0);
        put16(out, 4, 0);
        put16(out, 6, 32);
        out[8] = 1;
        put32(out, 12, 0x10001);
        out[16] = u8::from(!joined);
        if joined {
            out[18..24].copy_from_slice(&self.bss.bssid);
            put16(out, 26, self.bss.interval);
            out[28] = self.bss.dtim;
            out[29] = if self.bss.band_5g() { 0x31 } else { 0x0e };
            put16(out, 32, if self.bss.band_5g() { 0x38 } else { 0x13 });
        } else if !adding_device {
            out[16] = 1;
        }
        put16(out, 24, VIF_WCID as u16);
        put16(out, 30, VIF_WCID as u16);
    }

    fn start_scan(&mut self) -> bool {
        let mut request = [0u8; 1186];
        request[0] = 1;
        request[2] = 1;
        request[3] = 1;
        request[5] = 2;
        request[6] = 1 << 5;
        request[7] = 1;
        request[1110..1116].fill(0xff);
        self.network_count = 0;
        let sent = self.mcu_send(CE_START_HW_SCAN, &request).is_some();
        if sent {
            self.scanning = true;
            self.scan_deadline = now_ms() + 12_000;
        }
        sent
    }

    fn sta_record(&mut self, wcid: u8, state: u8, with_peer: bool) -> bool {
        let mut request = [0u8; 256];
        request[1] = wcid;
        request[4] = 1;
        request[5] = if with_peer { 0 } else { 0xe };
        let mut at = 8;
        let mut tlvs = 0u16;
        let rates = rate_bitmaps(&self.bss);
        let peer_ht = with_peer && self.ht;
        let peer_vht = with_peer && self.vht;
        if with_peer {
            put16(&mut request, at, 0);
            put16(&mut request, at + 2, 20);
            put32(&mut request, at + 4, 0x10002);
            request[at + 8] = 2;
            request[at + 9] = u8::from(self.qos);
            put16(&mut request, at + 10, self.aid);
            request[at + 12..at + 18].copy_from_slice(&self.bss.bssid);
            put16(&mut request, at + 18, if state == 0 { 3 } else { 1 });
            at += 20;
            tlvs += 1;
            if peer_ht {
                put16(&mut request, at, 9);
                put16(&mut request, at + 2, 8);
                put16(&mut request, at + 4, self.bss.ht_cap & OUR_HT_CAP);
                at += 8;
                tlvs += 1;
            }
            if peer_vht {
                put16(&mut request, at, 10);
                put16(&mut request, at + 2, 16);
                put32(&mut request, at + 4, self.bss.vht_cap & OUR_VHT_CAP);
                put16(&mut request, at + 8, self.bss.vht_rx_mcs);
                put16(&mut request, at + 10, self.bss.vht_tx_mcs);
                at += 16;
                tlvs += 1;
            }
            let mut phy_type = if self.bss.band_5g() { 8 } else { 3 };
            if peer_ht {
                phy_type |= 0x10;
            }
            if peer_vht {
                phy_type |= 0x20;
            }
            put16(&mut request, at, 0x15);
            put16(&mut request, at + 2, 12);
            put16(&mut request, at + 4, rates.0);
            request[at + 6] = phy_type;
            request[at + 7] = if peer_ht { self.bss.ht_ampdu & 0x1f } else { 0 };
            at += 12;
            put16(&mut request, at, 1);
            put16(&mut request, at + 2, 16);
            put16(&mut request, at + 4, rates.2);
            if peer_ht {
                request[at + 6..at + 8].copy_from_slice(&self.bss.ht_mcs);
            }
            at += 16;
            put16(&mut request, at, 7);
            put16(&mut request, at + 2, 12);
            request[at + 8] = state;
            if peer_vht {
                let streams = if self.bss.ht_mcs[1] != 0 { 2 } else { 1 };
                request[at + 9] = self.width | ((streams - 1) << 4);
            }
            at += 12;
            tlvs += 3;
        }
        let wtbl_len = 52 + if peer_ht { 12 } else { 0 } + if peer_vht { 12 } else { 0 };
        put16(&mut request, at, 13);
        put16(&mut request, at + 2, wtbl_len);
        let wtbl = at + 4;
        request[wtbl] = wcid;
        request[wtbl + 1] = 1;
        put16(
            &mut request,
            wtbl + 2,
            3 + u16::from(peer_ht) + u16::from(peer_vht),
        );
        if peer_ht {
            let ht = wtbl + 48;
            put16(&mut request, ht, 2);
            put16(&mut request, ht + 2, 12);
            request[ht + 4] = 1;
            request[ht + 5] = u8::from(self.bss.ht_cap & 1 != 0);
            request[ht + 6] = self.bss.ht_ampdu & 3;
            request[ht + 7] = (self.bss.ht_ampdu >> 2) & 7;
        }
        if peer_vht {
            let vht = wtbl + 60;
            put16(&mut request, vht, 3);
            put16(&mut request, vht + 2, 12);
            request[vht + 4] = u8::from(self.bss.vht_cap & 0x10 != 0);
            request[vht + 6] = 1;
        }
        let generic = wtbl + 8;
        put16(&mut request, generic, 0);
        put16(&mut request, generic + 2, 20);
        request[generic + 4..generic + 10].copy_from_slice(&self.bss.bssid);
        request[generic + 10] = if with_peer { 0 } else { 0xe };
        request[generic + 13] = u8::from(with_peer && self.qos);
        put16(
            &mut request,
            generic + 16,
            if with_peer { self.aid } else { 0 },
        );
        let rx = generic + 20;
        put16(&mut request, rx, 1);
        put16(&mut request, rx + 2, 12);
        request[rx + 5] = 1;
        request[rx + 6] = 1;
        request[rx + 7] = 1;
        let translation = rx + 12;
        put16(&mut request, translation, 6);
        put16(&mut request, translation + 2, 8);
        request[translation + 4] = 1;
        request[translation + 6] = 1;
        at = wtbl + wtbl_len as usize - 4;
        tlvs += 1;
        put16(&mut request, 2, tlvs);
        self.mcu_ok("station record", UNI_STA_REC, &request[..at])
    }

    fn install_key(
        &mut self,
        wcid: u8,
        with_peer: bool,
        cipher: u8,
        index: u8,
        key: &[u8],
    ) -> bool {
        let mut request = [0u8; 8 + 44];
        request[1] = wcid;
        put16(&mut request, 2, 1);
        request[4] = 1;
        request[5] = if with_peer { 0 } else { 0xe };
        put16(&mut request, 8, 0x11);
        put16(&mut request, 10, 44);
        request[13] = 1;
        request[16] = cipher;
        request[17] = 36;
        request[18] = index;
        request[19] = key.len() as u8;
        request[20..20 + key.len()].copy_from_slice(key);
        if cipher == 4 && key.len() == 32 {
            request[36..44].copy_from_slice(&key[24..32]);
            request[44..52].copy_from_slice(&key[16..24]);
        }
        self.mcu_ok("key", UNI_STA_REC, &request)
    }

    fn roc(&mut self, acquire: bool) {
        let mut request = [0u8; 28];
        if acquire {
            self.roc_token = self.roc_token.wrapping_add(1);
            self.roc_granted = false;
            let channel = self.bss.channel;
            put16(&mut request, 4, 0);
            put16(&mut request, 6, 24);
            request[9] = self.roc_token;
            request[10] = channel;
            request[12] = if channel > 14 { 2 } else { 1 };
            request[14] = channel;
            request[17] = channel;
            put32(&mut request, 20, 3000);
            request[24] = 0xff;
            let _ = self.mcu_send(UNI_ROC, &request);
        } else {
            put16(&mut request, 4, 1);
            put16(&mut request, 6, 12);
            request[9] = self.roc_token;
            request[10] = 0xff;
            let _ = self.mcu_send(UNI_ROC, &request[..16]);
        }
    }

    /// Sends one 802.11 frame. Management frames go out at the lowest basic rate.
    fn send_80211(&mut self, frame: &[u8], wcid: u8, protect: bool) -> bool {
        if frame.len() < 24 || frame.len() + 128 > BUFFER as usize {
            return false;
        }
        let slot = self.tx_data.head;
        let end = now_ms() + 50;
        while self.tx_busy[slot as usize] && now_ms() < end {
            let mut nothing = [0u8; 0];
            let _ = self.service(0, &mut nothing);
        }
        let base = self.tx_data.buffers + slot as u64 * BUFFER;
        // SAFETY: this slot's 2 KiB DMA buffer.
        let buffer = unsafe { core::slice::from_raw_parts_mut(base as *mut u8, BUFFER as usize) };
        buffer[..128].fill(0);
        buffer[128..128 + frame.len()].copy_from_slice(frame);
        let fc = frame[0];
        let kind = ((fc >> 2) & 3) as u32;
        let subtype = (fc >> 4) as u32;
        let data = kind == 2;
        let multicast = frame[4] & 1 != 0;
        let fixed = !data || multicast;
        let header = if data && subtype & 8 != 0 { 26 } else { 24 };
        let queue = if data { 1u32 } else { 0x10 };
        put32(buffer, 0, (frame.len() as u32 + 32) | (queue << 25));
        put32(
            buffer,
            4,
            (1 << 31) | wcid as u32 | (2 << 16) | ((header as u32 / 2) << 11),
        );
        let mut dw2 = (kind << 4) | subtype | (u32::from(multicast) << 10);
        let mut dw3 = 15 << 11;
        let mut dw6 = 0;
        let mut dw7 = (kind << 20) | (subtype << 16);
        if fixed {
            dw2 |= (1 << 31) | (1 << 13);
            dw3 |= 1 << 28;
            let rate = if self.bss.band_5g() { 0x4b } else { 0 };
            dw6 |= (1 << 2) | (rate << 16);
            dw7 |= 24 << 11;
        }
        if protect {
            dw3 |= 2;
        }
        put32(buffer, 8, dw2);
        put32(buffer, 12, dw3);
        put32(buffer, 24, dw6);
        put32(buffer, 28, dw7);
        put16(buffer, 32, slot as u16 | 0x8000);
        put32(buffer, 40, (base + 128) as u32);
        put16(buffer, 44, (frame.len() as u16 & 0xfff) | 0x8000);
        self.tx_busy[slot as usize] = true;
        self.tx_push(TX_DATA_RING, base, 64)
    }

    fn send_mgmt(&mut self, subtype: u8, body: &[u8]) -> bool {
        let mut frame = [0u8; 256];
        if body.len() + 24 > frame.len() {
            return false;
        }
        frame[0] = subtype << 4;
        frame[4..10].copy_from_slice(&self.bss.bssid);
        frame[10..16].copy_from_slice(&self.mac);
        frame[16..22].copy_from_slice(&self.bss.bssid);
        frame[24..24 + body.len()].copy_from_slice(body);
        self.send_80211(&frame[..24 + body.len()], AP_WCID, false)
    }

    fn send_data(
        &mut self,
        destination: &[u8],
        source: &[u8],
        ethertype: u16,
        payload: &[u8],
    ) -> bool {
        let mut frame = [0u8; 24 + 8 + 1520];
        if payload.len() > 1520 {
            return false;
        }
        let header = if self.qos { 26 } else { 24 };
        frame[0] = if self.qos { 0x88 } else { 0x08 };
        frame[1] = 0x01 | if self.keys { 0x40 } else { 0 };
        frame[4..10].copy_from_slice(&self.bss.bssid);
        frame[10..16].copy_from_slice(&source[..6]);
        frame[16..22].copy_from_slice(&destination[..6]);
        frame[header..header + 6].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0]);
        frame[header + 6..header + 8].copy_from_slice(&ethertype.to_be_bytes());
        frame[header + 8..header + 8 + payload.len()].copy_from_slice(payload);
        let keys = self.keys;
        self.send_80211(&frame[..header + 8 + payload.len()], AP_WCID, keys)
    }

    fn wait_mgmt(&mut self, subtype: u8, ms: u64) -> Option<usize> {
        let end = now_ms() + ms;
        let mut nothing = [0u8; 0];
        while now_ms() < end {
            let _ = self.service(0, &mut nothing);
            if self.mgmt_len != 0 && self.mgmt_subtype == subtype {
                let len = self.mgmt_len;
                self.mgmt_len = 0;
                return Some(len);
            }
            spin_loop();
        }
        None
    }

    fn wait_eapol(&mut self, ms: u64) -> Option<usize> {
        let end = now_ms() + ms;
        let mut nothing = [0u8; 0];
        while now_ms() < end {
            let _ = self.service(0, &mut nothing);
            if self.eapol_len != 0 {
                let len = self.eapol_len;
                self.eapol_len = 0;
                return Some(len);
            }
            spin_loop();
        }
        None
    }

    fn join(&mut self, network: Network, pmk: Option<[u8; 32]>) -> Result<(), &'static str> {
        self.bss = network;
        self.aid = 0;
        self.keys = false;
        self.lost = false;
        self.mgmt_len = 0;
        self.eapol_len = 0;
        self.supplicant = None;
        self.addba = None;
        self.qos = network.wmm;
        self.ht = network.wmm && network.ht_cap != 0;
        self.vht = self.ht && network.band_5g() && network.vht_cap != 0;
        self.width = 0;
        self.center = network.channel;
        if network.band_5g() {
            if self.vht && network.vht_width >= 1 && network.vht_center != 0 {
                self.width = 2;
                self.center = network.vht_center;
            } else if self.ht && network.ht_wide && network.ht_secondary != 0 {
                self.width = 1;
                self.center = if network.ht_secondary == 1 {
                    network.channel + 2
                } else {
                    network.channel - 2
                };
            }
        }
        note!(
            "link: ht={} vht={} qos={} width={} center={}",
            self.ht,
            self.vht,
            self.qos,
            20u32 << self.width,
            self.center
        );
        note!(
            "joining \"{}\" on channel {} ({} dBm)",
            network.name(),
            network.channel,
            network.rssi
        );
        if !self.sta_record(AP_WCID, 0, true) {
            return Err("The card refused the network");
        }
        self.roc(true);
        let end = now_ms() + 1000;
        let mut nothing = [0u8; 0];
        while !self.roc_granted && now_ms() < end {
            let _ = self.service(0, &mut nothing);
        }
        note!("channel granted={}", self.roc_granted);
        let mut authenticated = false;
        for _ in 0..3 {
            if !self.send_mgmt(11, &[0, 0, 1, 0, 0, 0]) {
                continue;
            }
            if let Some(len) = self.wait_mgmt(11, 400) {
                let status = if len >= 6 {
                    le16(&self.mgmt, 4)
                } else {
                    0xffff
                };
                note!("authentication status {}", status);
                authenticated = status == 0;
                break;
            }
        }
        if !authenticated {
            self.roc(false);
            return Err("The network didn't answer");
        }
        let mut body = [0u8; 160];
        put16(&mut body, 0, (network.capability & 0x0431) | 1);
        put16(&mut body, 2, 10);
        let mut at = 4;
        body[at] = 0;
        body[at + 1] = network.ssid_len as u8;
        body[at + 2..at + 2 + network.ssid_len].copy_from_slice(&network.ssid[..network.ssid_len]);
        at += 2 + network.ssid_len;
        for (id, rates, len) in [
            (1u8, &network.rates[..], network.rates_len),
            (50, &network.ext_rates[..], network.ext_rates_len),
        ] {
            let start = at;
            body[at] = id;
            at += 2;
            for rate in &rates[..len] {
                if rate & 0x7f != 127 && rate & 0x7f != 126 {
                    body[at] = *rate;
                    at += 1;
                }
            }
            body[start + 1] = (at - start - 2) as u8;
            if at == start + 2 {
                at = start;
            }
        }
        let rsn = rsn_element(network.group_cipher);
        if pmk.is_some() {
            body[at..at + rsn.len()].copy_from_slice(&rsn);
            at += rsn.len();
        }
        if self.ht {
            let mut caps = [0u8; 28];
            caps[0] = 45;
            caps[1] = 26;
            let cap = if network.band_5g() {
                OUR_HT_CAP
            } else {
                OUR_HT_CAP & !2
            };
            put16(&mut caps, 2, cap);
            caps[4] = 0x03;
            caps[5] = 0xff;
            caps[6] = 0xff;
            caps[17] = 0x01;
            body[at..at + 28].copy_from_slice(&caps);
            at += 28;
        }
        if self.vht {
            let mut caps = [0u8; 14];
            caps[0] = 191;
            caps[1] = 12;
            put32(&mut caps, 2, OUR_VHT_CAP);
            put16(&mut caps, 6, 0xfffa);
            put16(&mut caps, 10, 0xfffa);
            body[at..at + 14].copy_from_slice(&caps);
            at += 14;
        }
        if self.qos {
            body[at..at + 9].copy_from_slice(&[221, 7, 0x00, 0x50, 0xf2, 0x02, 0x00, 0x01, 0x00]);
            at += 9;
        }
        let mut associated = false;
        for _ in 0..3 {
            if !self.send_mgmt(0, &body[..at]) {
                continue;
            }
            if let Some(len) = self.wait_mgmt(1, 600) {
                let status = if len >= 6 {
                    le16(&self.mgmt, 2)
                } else {
                    0xffff
                };
                self.aid = if len >= 6 {
                    le16(&self.mgmt, 4) & 0x3fff
                } else {
                    0
                };
                note!("association status {} aid {}", status, self.aid);
                associated = status == 0;
                if !associated {
                    self.roc(false);
                    return Err(if status == 17 {
                        "The network is full"
                    } else {
                        "The network turned AerOS away"
                    });
                }
                break;
            }
        }
        if !associated {
            self.roc(false);
            return Err("The network didn't answer");
        }
        let mut basic = [0u8; 36];
        self.bss_basic(&mut basic, false, true);
        let mut bss = [0u8; 44];
        bss[..36].copy_from_slice(&basic);
        put16(&mut bss, 36, 15);
        put16(&mut bss, 38, 8);
        bss[40] = u8::from(self.qos);
        let mut rlm = [0u8; 20];
        put16(&mut rlm, 4, 2);
        put16(&mut rlm, 6, 16);
        rlm[8] = network.channel;
        rlm[9] = self.center;
        rlm[11] = self.width;
        rlm[15] = if self.width > 0 { 4 } else { 0 };
        rlm[16] = match self.center.cmp(&network.channel) {
            core::cmp::Ordering::Greater => 1,
            core::cmp::Ordering::Less => 3,
            core::cmp::Ordering::Equal => 0,
        };
        rlm[12] = 2;
        rlm[13] = 3;
        rlm[14] = 1;
        rlm[17] = u8::from(network.band_5g());
        if !self.mcu_ok("bss join", UNI_BSS_INFO, &bss)
            || !self.mcu_ok("bss channel", UNI_BSS_INFO, &rlm)
            || !self.sta_record(AP_WCID, 2, true)
            || !self.sta_record(VIF_WCID, 2, false)
        {
            self.roc(false);
            return Err("The card refused the network");
        }
        if let Some(pmk) = pmk {
            let mut snonce = [0u8; 32];
            if !crate::random::fill(&mut snonce) {
                for chunk in snonce.chunks_mut(8) {
                    chunk.copy_from_slice(&crate::random::next_u64().to_le_bytes());
                }
            }
            self.supplicant = wpa::Supplicant::new(pmk, network.bssid, self.mac, snonce, &rsn);
            let mut done = false;
            let mut out = [0u8; wpa::MAX_FRAME];
            let end = now_ms() + 4000;
            while !done && now_ms() < end {
                let Some(len) = self.wait_eapol(end.saturating_sub(now_ms())) else {
                    break;
                };
                let frame = self.eapol;
                let Some(supplicant) = self.supplicant.as_mut() else {
                    break;
                };
                match supplicant.handle(&frame[..len], &mut out) {
                    Ok(wpa::Reply::Send(send)) => {
                        note!("handshake message 1 -> 2");
                        let (bssid, mac) = (self.bss.bssid, self.mac);
                        self.send_data(&bssid, &mac, 0x888e, &out[..send]);
                    }
                    Ok(wpa::Reply::Complete {
                        send,
                        tk,
                        gtk,
                        gtk_length,
                        gtk_index,
                    }) => {
                        note!("handshake message 3 -> 4");
                        let (bssid, mac) = (self.bss.bssid, self.mac);
                        self.send_data(&bssid, &mac, 0x888e, &out[..send]);
                        sleep_ms(20);
                        let group = if network.group_cipher == 2 { 4 } else { 5 };
                        let installed = self.install_key(AP_WCID, true, 5, 0, &tk)
                            && self.install_key(
                                VIF_WCID,
                                false,
                                group,
                                gtk_index,
                                &gtk[..gtk_length],
                            );
                        note!("keys installed={}", installed);
                        if !installed {
                            self.roc(false);
                            return Err("The card refused the keys");
                        }
                        self.keys = true;
                        done = true;
                    }
                    Err(error) => {
                        note!("handshake error {:?}", error);
                        if matches!(error, wpa::Error::BadMic) {
                            self.roc(false);
                            return Err("Wrong password");
                        }
                    }
                }
            }
            if !done {
                self.roc(false);
                return Err("Wrong password, or the network didn't finish signing in");
            }
        }
        self.roc(false);
        self.link = Link::Connected;
        note!("connected");
        Ok(())
    }

    /// The access point is changing the group key.
    fn rekey(&mut self) {
        let len = core::mem::take(&mut self.eapol_len);
        let frame = self.eapol;
        let mut out = [0u8; wpa::MAX_FRAME];
        let Some(supplicant) = self.supplicant.as_mut() else {
            return;
        };
        match supplicant.handle_group(&frame[..len], &mut out) {
            Ok((send, gtk, length, index)) => {
                let (bssid, mac) = (self.bss.bssid, self.mac);
                self.send_data(&bssid, &mac, 0x888e, &out[..send]);
                let group = if self.bss.group_cipher == 2 { 4 } else { 5 };
                let installed = self.install_key(VIF_WCID, false, group, index, &gtk[..length]);
                note!(
                    "group key renewed (index {}) installed={}",
                    index,
                    installed
                );
            }
            Err(error) => note!("key message ignored: {:?}", error),
        }
    }

    fn leave(&mut self) {
        if self.link == Link::Connected || self.link == Link::Connecting {
            let _ = self.send_mgmt(12, &[3, 0]);
        }
        self.link = Link::Idle;
        self.keys = false;
        self.queue_count = 0;
        CONNECTED.store(false, Ordering::Release);
    }
}

fn parse_rsn(rsn: &[u8], group_cipher: &mut u8) -> Security {
    if rsn.len() < 8 || rsn[2..5] != [0x00, 0x0f, 0xac] {
        return Security::Unsupported;
    }
    *group_cipher = rsn[5];
    let pairwise = le16(rsn, 6) as usize;
    let mut at = 8;
    let mut ccmp = false;
    for _ in 0..pairwise {
        if rsn
            .get(at..at + 4)
            .is_some_and(|suite| suite == [0x00, 0x0f, 0xac, 4])
        {
            ccmp = true;
        }
        at += 4;
    }
    if at + 2 > rsn.len() {
        return Security::Unsupported;
    }
    let akms = le16(rsn, at) as usize;
    at += 2;
    let mut psk = false;
    for _ in 0..akms {
        if rsn
            .get(at..at + 4)
            .is_some_and(|suite| suite == [0x00, 0x0f, 0xac, 2])
        {
            psk = true;
        }
        at += 4;
    }
    let capabilities = if at + 2 <= rsn.len() {
        le16(rsn, at)
    } else {
        0
    };
    let group_ok = matches!(*group_cipher, 2 | 4);
    if ccmp && psk && group_ok && capabilities & 0x40 == 0 {
        Security::Wpa2
    } else {
        Security::Unsupported
    }
}

fn rsn_element(group_cipher: u8) -> [u8; 22] {
    [
        48,
        20,
        1,
        0,
        0x00,
        0x0f,
        0xac,
        group_cipher,
        1,
        0,
        0x00,
        0x0f,
        0xac,
        4,
        1,
        0,
        0x00,
        0x0f,
        0xac,
        2,
        0,
        0,
    ]
}

/// (basic rates, supported rates, RA legacy bitmap) in the firmware's indexing.
fn rate_bitmaps(network: &Network) -> (u16, u16, u16) {
    const TABLE_24: [u8; 12] = [2, 4, 11, 22, 12, 18, 24, 36, 48, 72, 96, 108];
    let table: &[u8] = if network.band_5g() {
        &TABLE_24[4..]
    } else {
        &TABLE_24
    };
    let mut basic = 0u16;
    let mut supported = 0u16;
    for rate in network.rates[..network.rates_len]
        .iter()
        .chain(&network.ext_rates[..network.ext_rates_len])
    {
        if let Some(index) = table.iter().position(|known| *known == rate & 0x7f) {
            supported |= 1 << index;
            if rate & 0x80 != 0 {
                basic |= 1 << index;
            }
        }
    }
    if supported == 0 {
        supported = (1 << table.len()) - 1;
    }
    if basic == 0 {
        basic = 1;
    }
    let legacy = if network.band_5g() {
        supported << 6
    } else {
        ((supported >> 4) << 6) | (supported & 0xf)
    };
    (basic, supported, legacy)
}

/// Finds the card, uploads its firmware and starts the radio.
pub fn initialize(pci: &PciInventory, frames: &mut FrameAllocator) -> bool {
    let Some(device) = pci
        .devices()
        .iter()
        .copied()
        .find(|device| device.vendor == VENDOR && device.device == DEVICE)
    else {
        return false;
    };
    if crate::fat::root_file_size(b"WIFIOFF TXT").is_some() {
        note!("skipped: WIFIOFF.TXT is on the boot partition");
        save_log();
        return false;
    }
    let Some(mmio) = crate::pci::bar_address(&device, 0, 0).filter(|address| *address != 0) else {
        note!("no memory BAR");
        save_log();
        return false;
    };
    note!(
        "MT7921 at {:02x}:{:02x}.{} BAR {:#x}",
        device.bus,
        device.slot,
        device.function,
        mmio
    );
    if !pci.enable_memory_bus_master(device) {
        note!("could not enable the device");
        save_log();
        return false;
    }
    let command = crate::pci::config_read(device.bus, device.slot, device.function, 4, 2);
    crate::pci::config_write(
        device.bus,
        device.slot,
        device.function,
        4,
        2,
        command | 0x406,
    );
    let ok = {
        let mut wifi = WIFI.lock();
        wifi.mmio = mmio;
        wifi.link = Link::Starting;
        let ok = wifi.bring_up(frames);
        wifi.link = if ok { Link::Idle } else { Link::Absent };
        wifi.message = if ok { "" } else { "Wi-Fi didn't start" };
        ok
    };
    note!("ready={}", ok);
    save_log();
    PRESENT.store(ok, Ordering::Release);
    if ok {
        let _ = scan();
    }
    ok
}

pub fn present() -> bool {
    PRESENT.load(Ordering::Acquire)
}

pub fn connected() -> bool {
    CONNECTED.load(Ordering::Acquire)
}

pub fn mac_address() -> Option<[u8; 6]> {
    present().then(|| WIFI.lock().mac)
}

pub fn scan() -> bool {
    if !present() {
        return false;
    }
    let mut wifi = WIFI.lock();
    if wifi.scanning || wifi.link == Link::Connecting {
        return true;
    }
    wifi.start_scan()
}

/// Handles whatever the card sent: scan results, events, frames. Cheap when idle.
pub fn poll() {
    if !present() {
        return;
    }
    let lost = {
        let mut wifi = WIFI.lock();
        let mut nothing = [0u8; 0];
        let _ = wifi.service(0, &mut nothing);
        if wifi.scanning && now_ms() > wifi.scan_deadline {
            wifi.scanning = false;
            note!("scan timed out with {} networks", wifi.network_count);
        }
        if let Some(request) = wifi.addba.take() {
            let mut reply = [
                3u8, 1, request[0], 37, 0, request[1], request[2], request[3], request[4],
            ];
            reply[5] &= !1;
            let _ = wifi.send_mgmt(13, &reply);
        }
        if wifi.link == Link::Connected && wifi.eapol_len != 0 {
            wifi.rekey();
        }
        let lost = wifi.lost && wifi.link == Link::Connected;
        if lost {
            wifi.lost = false;
            wifi.leave();
            wifi.message = "Connection lost";
        }
        lost
    };
    if lost {
        crate::nic::wifi_lost();
        save_log();
    }
}

pub fn status() -> Status {
    if !present() {
        return Status {
            link: Link::Absent,
            scanning: false,
            ssid: [0; 32],
            ssid_len: 0,
            message: "",
        };
    }
    let wifi = WIFI.lock();
    Status {
        link: wifi.link,
        scanning: wifi.scanning,
        ssid: wifi.bss.ssid,
        ssid_len: if wifi.link == Link::Idle {
            0
        } else {
            wifi.bss.ssid_len
        },
        message: wifi.message,
    }
}

/// The networks heard in the last scan, strongest first, one per name.
pub fn networks(out: &mut [Network]) -> usize {
    let wifi = WIFI.lock();
    let mut count = 0;
    for network in &wifi.networks[..wifi.network_count] {
        if let Some(same) = out[..count]
            .iter_mut()
            .find(|known| known.ssid_len == network.ssid_len && known.ssid == network.ssid)
        {
            if network.rssi > same.rssi {
                *same = *network;
            }
        } else if count < out.len() {
            out[count] = *network;
            count += 1;
        }
    }
    out[..count].sort_unstable_by_key(|network| core::cmp::Reverse(network.rssi));
    count
}

/// Joins `network` (blocking, a few seconds), then asks the router for an address.
pub fn connect(network: &Network, passphrase: &str) -> Result<(), &'static str> {
    if !present() {
        return Err("No Wi-Fi card");
    }
    let pmk = match network.security {
        Security::Open => None,
        Security::Wpa2 => Some(
            wpa::passphrase_to_pmk(passphrase, &network.ssid[..network.ssid_len])
                .ok_or("Passwords are 8 to 63 characters")?,
        ),
        Security::Unsupported => return Err("AerOS can't join this kind of network yet"),
    };
    let result = {
        let mut wifi = WIFI.lock();
        wifi.leave();
        wifi.link = Link::Connecting;
        wifi.message = "Connecting...";
        let result = wifi.join(*network, pmk);
        if result.is_err() {
            wifi.link = Link::Idle;
        }
        wifi.message = match result {
            Ok(()) => "",
            Err(reason) => reason,
        };
        result
    };
    if result.is_ok() {
        CONNECTED.store(true, Ordering::Release);
        crate::nic::wifi_up();
        let mac = mac_address().unwrap_or([0; 6]);
        let addressed = crate::net::configure_dhcp(mac);
        note!("dhcp ok={}", addressed);
        if !addressed {
            WIFI.lock().message = "Connected, but the router gave no address";
        }
    }
    save_log();
    result
}

pub fn disconnect() {
    if present() {
        WIFI.lock().leave();
        crate::nic::wifi_lost();
    }
}

pub fn transmit(frame: &[u8]) -> bool {
    if !connected() || frame.len() < 14 {
        return false;
    }
    let mut wifi = WIFI.lock();
    if wifi.link != Link::Connected {
        return false;
    }
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    wifi.send_data(&frame[..6], &frame[6..12], ethertype, &frame[14..])
}

pub fn try_receive(destination: &mut [u8]) -> Option<usize> {
    if !connected() {
        return None;
    }
    let mut wifi = WIFI.lock();
    if wifi.queue_count == 0 {
        let mut nothing = [0u8; 0];
        let _ = wifi.service(0, &mut nothing);
    }
    if wifi.queue_count == 0 {
        return None;
    }
    let slot = wifi.queue_head;
    let len = wifi.queue_len[slot].min(destination.len());
    destination[..len].copy_from_slice(&wifi.queue[slot][..len]);
    wifi.queue_head = (slot + 1) % QUEUE;
    wifi.queue_count -= 1;
    Some(len)
}
