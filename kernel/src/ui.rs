use core::fmt::{self, Write};

use crate::arch::CpuInfo;
use crate::font::{FontCatalog, RasterFont};
use crate::framebuffer::{Color, FrameBuffer};
use crate::memory::{AllocatorStats, BootMemoryMap};

const WHITE: Color = Color::rgb(244, 247, 255);
const MUTED: Color = Color::rgb(155, 169, 194);
const GREEN: Color = Color::rgb(82, 224, 158);
const PANEL: Color = Color::rgb(22, 31, 49);
const PANEL_EDGE: Color = Color::rgb(43, 58, 84);

pub struct BootHealth<'a> {
    pub cpu: &'a CpuInfo,
    pub memory: &'a BootMemoryMap,
    pub allocator: AllocatorStats,
    pub acpi_valid: bool,
    pub topology_valid: bool,
    pub allocator_valid: bool,
    pub architecture_valid: bool,
    pub timer_valid: bool,
    pub paging_valid: bool,
    pub heap_valid: bool,
    pub user_valid: bool,
    pub vfs_valid: bool,
    pub pci_valid: bool,
    pub storage_valid: bool,
    pub network_valid: bool,
    pub scheduler_valid: bool,
}

pub fn draw_boot_complete(frame: &mut FrameBuffer, fonts: &FontCatalog, health: BootHealth<'_>) {
    frame.clear(Color::rgb(8, 13, 24));
    frame.vertical_gradient(Color::rgb(13, 22, 40), Color::rgb(6, 10, 19));
    let width = frame.width() as i32;
    let height = frame.height() as i32;
    let margin = (width / 14).clamp(28, 96);
    let content_width = width - margin * 2;

    let Some(ui_font) = fonts.ui() else {
        draw_font_failure(frame, margin, width, height);
        return;
    };
    let Some(mono_font) = fonts.mono() else {
        draw_font_failure(frame, margin, width, height);
        return;
    };

    let title = "AerOS";
    let title_width = ui_font.text_width(title, 54);
    ui_font.draw(frame, margin, margin, title, 54, WHITE);
    frame.rounded_rectangle(margin + title_width + 18, margin + 18, 8, 8, 4, GREEN);
    ui_font.draw(
        frame,
        margin,
        margin + 62,
        "Kernel foundation is online",
        23,
        MUTED,
    );

    let panel_y = margin + 116;
    let panel_height = (height - panel_y - margin).max(230);
    frame.rounded_rectangle(margin, panel_y, content_width, panel_height, 18, PANEL_EDGE);
    frame.rounded_rectangle(
        margin + 1,
        panel_y + 1,
        content_width - 2,
        panel_height - 2,
        17,
        PANEL,
    );

    let inner_x = margin + 28;
    let title_y = panel_y + 27;
    let status_y = title_y + 48;
    let right_x = inner_x + content_width / 2;
    ui_font.draw(frame, inner_x, title_y, "Boot health", 25, WHITE);
    let statuses = [
        ("UEFI handoff complete", true),
        ("Physical allocator verified", health.allocator_valid),
        ("ACPI root discovered", health.acpi_valid),
        ("APIC topology verified", health.topology_valid),
        ("GDT, TSS and IDT verified", health.architecture_valid),
        ("Local APIC clock verified", health.timer_valid),
        ("High-half paging verified", health.paging_valid),
        ("Kernel heap verified", health.heap_valid),
        ("Ring-3 syscall path verified", health.user_valid),
        ("VFS and ELF loader verified", health.vfs_valid),
        ("PCI fabric enumerated", health.pci_valid),
        ("Native AHCI storage verified", health.storage_valid),
        ("Native e1000e network verified", health.network_valid),
        ("Preemptive scheduler verified", health.scheduler_valid),
    ];
    for (index, (label, healthy)) in statuses.iter().enumerate() {
        let column = index / 7;
        let row = index % 7;
        status_row(
            frame,
            mono_font,
            if column == 0 { inner_x } else { right_x },
            status_y + row as i32 * 34,
            label,
            *healthy,
        );
    }
    let mut row_y = status_y + 7 * 34 + 25;

    let mut cpu_line = TextBuffer::<128>::new();
    let _ = write!(
        cpu_line,
        "CPU  {}  NX:{}  SMEP:{}  SMAP:{}",
        health.cpu.vendor(),
        yes_no(health.cpu.nx),
        yes_no(health.cpu.smep),
        yes_no(health.cpu.smap)
    );
    mono_font.draw(frame, inner_x, row_y, cpu_line.as_str(), 17, MUTED);
    row_y += 29;

    let mut memory_line = TextBuffer::<128>::new();
    let memory_mib = health.memory.usable_pages().saturating_mul(4) / 1024;
    let _ = write!(
        memory_line,
        "RAM  {} MiB usable  {} regions  {} managed",
        memory_mib,
        health.memory.region_count(),
        health.allocator.managed_regions
    );
    mono_font.draw(frame, inner_x, row_y, memory_line.as_str(), 17, MUTED);
    row_y += 29;

    let mut frame_line = TextBuffer::<128>::new();
    let _ = write!(
        frame_line,
        "PMM  {} pages free  {} allocated  {} free ranges",
        health.allocator.free_pages, health.allocator.allocated_pages, health.allocator.free_ranges
    );
    mono_font.draw(frame, inner_x, row_y, frame_line.as_str(), 17, MUTED);

    let footer_y = panel_y + panel_height - 42;
    let footer = "Plus Jakarta Sans  /  Roboto Mono";
    let footer_x = margin + (content_width - mono_font.text_width(footer, 15)) / 2;
    mono_font.draw(
        frame,
        footer_x,
        footer_y,
        footer,
        15,
        Color::rgb(105, 124, 154),
    );
}

fn status_row(
    frame: &mut FrameBuffer,
    font: RasterFont,
    x: i32,
    y: i32,
    label: &str,
    healthy: bool,
) {
    let color = if healthy {
        GREEN
    } else {
        Color::rgb(255, 183, 77)
    };
    frame.rounded_rectangle(x, y + 5, 12, 12, 6, color);
    font.draw(frame, x + 25, y, label, 18, WHITE);
}

fn draw_font_failure(frame: &mut FrameBuffer, margin: i32, width: i32, height: i32) {
    frame.rounded_rectangle(
        margin,
        margin,
        width - margin * 2,
        height - margin * 2,
        18,
        Color::rgb(91, 35, 45),
    );
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

struct TextBuffer<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> TextBuffer<N> {
    const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> Write for TextBuffer<N> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let available = N.saturating_sub(self.len);
        if value.len() > available {
            return Err(fmt::Error);
        }
        self.bytes[self.len..self.len + value.len()].copy_from_slice(value.as_bytes());
        self.len += value.len();
        Ok(())
    }
}
