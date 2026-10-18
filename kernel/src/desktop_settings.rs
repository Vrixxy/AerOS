use super::*;

const PAGES: [&str; 8] = [
    "Network", "Display", "Sound", "Keyboard", "Time", "Power", "Account", "About",
];
const PAGE_NETWORK: u8 = 0;
const PAGE_DISPLAY: u8 = 1;
const PAGE_SOUND: u8 = 2;
const PAGE_KEYBOARD: u8 = 3;
const PAGE_TIME: u8 = 4;
const PAGE_POWER: u8 = 5;
const PAGE_ACCOUNT: u8 = 6;

const ROW: i32 = 28;
const ROW_GAP: i32 = 4;
const INK: Color = Color::rgb(15, 43, 53);
const INK_SOFT: Color = Color::rgb(31, 68, 79);
const ACCENT: Rgba = Rgba::new(46, 120, 232, 255);

fn sidebar_row(base: Rect, index: usize) -> Rect {
    Rect::new(base.x + 14, base.y + 46 + index as i32 * 30, 120, 26)
}

fn pane(base: Rect) -> Rect {
    Rect::new(
        base.x + 150,
        base.y + 48,
        (base.width - 164).max(120),
        (base.height - 62).max(100),
    )
}

fn slider_track(pane: Rect, y: i32) -> Rect {
    Rect::new(pane.x, pane.y + y, pane.width - 24, 12)
}

fn time_toggle(pane: Rect) -> Rect {
    Rect::new(pane.x, pane.y + 34, 250, 36)
}

fn toggle_rect(pane: Rect, y: i32) -> Rect {
    Rect::new(pane.x, pane.y + y, 156, 36)
}

fn list_top(page: u8) -> i32 {
    if page == PAGE_TIME { 84 } else { 34 }
}

fn visible_rows(page: u8, pane: Rect) -> usize {
    (((pane.height - list_top(page)) / (ROW + ROW_GAP)).max(1) as usize).min(8)
}

fn list_row(pane: Rect, page: u8, slot: usize) -> Rect {
    Rect::new(
        pane.x,
        pane.y + list_top(page) + slot as i32 * (ROW + ROW_GAP),
        pane.width - 24,
        ROW,
    )
}

fn power_button(pane: Rect, index: usize) -> Rect {
    let width = (pane.width - 36) / 2;
    Rect::new(
        pane.x + (index % 2) as i32 * (width + 12),
        pane.y + 44 + (index / 2) as i32 * 56,
        width,
        44,
    )
}

fn account_button(pane: Rect, index: usize) -> Rect {
    let width = (pane.width - 36) / 2;
    Rect::new(
        pane.x + (index % 2) as i32 * (width + 12),
        pane.y + 84,
        width,
        44,
    )
}

fn list_len(page: u8) -> usize {
    match page {
        PAGE_KEYBOARD => crate::keymap::layout_count(),
        PAGE_TIME => crate::timezone::ZONES.len(),
        _ => 0,
    }
}

fn list_selected(page: u8) -> usize {
    match page {
        PAGE_KEYBOARD => crate::keymap::layout_index(),
        PAGE_TIME => crate::rtc::timezone_index(),
        _ => 0,
    }
}

fn list_label(page: u8, index: usize) -> &'static str {
    match page {
        PAGE_KEYBOARD => crate::keymap::layout_name(index),
        _ => crate::timezone::ZONES[index].name,
    }
}

impl DesktopState {
    fn settings_apply(&mut self, page: u8, index: usize) {
        match page {
            PAGE_KEYBOARD => crate::settings::set_keyboard_layout(index),
            PAGE_TIME => crate::settings::set_timezone(index),
            _ => {}
        }
    }

    fn settings_show_page(&mut self, page: u8) {
        self.settings_page = page % PAGES.len() as u8;
        self.settings_cursor = list_selected(self.settings_page);
        self.settings_scroll = 0;
        let rows = visible_rows(
            self.settings_page,
            pane(window_base_rect(DesktopApp::Settings)),
        );
        if self.settings_cursor >= rows {
            self.settings_scroll = self.settings_cursor + 1 - rows;
        }
    }

    pub(super) fn settings_click(&mut self, point: Point) -> Option<DesktopAction> {
        let base = window_base_rect(DesktopApp::Settings);
        for index in 0..PAGES.len() {
            if sidebar_row(base, index).contains(point) {
                self.settings_show_page(index as u8);
                return Some(DesktopAction::Redraw);
            }
        }
        let pane = pane(base);
        let page = self.settings_page;
        let now = crate::time::monotonic_nanoseconds();
        match page {
            PAGE_NETWORK if toggle_rect(pane, 110).contains(point) => {
                self.wifi = !self.wifi;
                if self.wifi {
                    self.airplane = false;
                }
                return Some(DesktopAction::Redraw);
            }
            PAGE_DISPLAY => {
                let track = slider_track(pane, 62).expand(8);
                if track.contains(point) {
                    let raw = (point.x - pane.x) * 100 / (pane.width - 24).max(1);
                    self.set_brightness(raw.clamp(8, 100) as u8, now);
                    return Some(DesktopAction::Redraw);
                }
            }
            PAGE_SOUND => {
                let track = slider_track(pane, 62).expand(8);
                if track.contains(point) {
                    let raw = (point.x - pane.x) * 100 / (pane.width - 24).max(1);
                    self.set_volume_to(raw.clamp(0, 100) as u8);
                    return Some(DesktopAction::Redraw);
                }
                if toggle_rect(pane, 104).contains(point) {
                    self.toggle_mute();
                    return Some(DesktopAction::Redraw);
                }
            }
            PAGE_KEYBOARD | PAGE_TIME => {
                if page == PAGE_TIME && time_toggle(pane).contains(point) {
                    crate::settings::set_ntp_enabled(!crate::settings::ntp_enabled());
                    return Some(DesktopAction::Redraw);
                }
                let rows = visible_rows(page, pane);
                for slot in 0..rows {
                    let index = self.settings_scroll + slot;
                    if index < list_len(page) && list_row(pane, page, slot).contains(point) {
                        self.settings_cursor = index;
                        self.settings_apply(page, index);
                        return Some(DesktopAction::Redraw);
                    }
                }
            }
            PAGE_POWER => {
                for index in 0..4 {
                    if power_button(pane, index).contains(point) {
                        match index {
                            0 => {
                                self.lock_session();
                            }
                            1 => self.start_power_action(shellui::PowerAction::Sleep),
                            2 => self.start_power_action(shellui::PowerAction::Restart),
                            _ => self.start_power_action(shellui::PowerAction::Shutdown),
                        }
                        return Some(DesktopAction::Redraw);
                    }
                }
            }
            PAGE_ACCOUNT => {
                if account_button(pane, 0).contains(point) {
                    self.lock_session();
                    return Some(DesktopAction::Redraw);
                }
                if account_button(pane, 1).contains(point) {
                    self.set_app(DesktopApp::Terminal);
                    let command = b"passwd";
                    self.terminal_input[..command.len()].copy_from_slice(command);
                    self.terminal_input_len = command.len();
                    return Some(DesktopAction::Redraw);
                }
            }
            _ => {}
        }
        None
    }

    pub(super) fn settings_key(&mut self, key: DesktopKey) -> Option<DesktopAction> {
        let page = self.settings_page;
        let now = crate::time::monotonic_nanoseconds();
        let base = window_base_rect(DesktopApp::Settings);
        match key {
            DesktopKey::Tab => {
                self.settings_show_page(page + 1);
                Some(DesktopAction::Redraw)
            }
            DesktopKey::Left | DesktopKey::Right => {
                let step = if key == DesktopKey::Right { 5 } else { -5 };
                match page {
                    PAGE_DISPLAY => {
                        self.brightness_step(step);
                    }
                    PAGE_SOUND => self.volume_step(step),
                    _ => {
                        let next = if key == DesktopKey::Right {
                            page + 1
                        } else {
                            page + PAGES.len() as u8 - 1
                        };
                        self.settings_show_page(next);
                    }
                }
                let _ = now;
                Some(DesktopAction::Redraw)
            }
            DesktopKey::Up | DesktopKey::Down => {
                let len = list_len(page);
                if len == 0 {
                    return None;
                }
                let rows = visible_rows(page, pane(base));
                self.settings_cursor = if key == DesktopKey::Down {
                    (self.settings_cursor + 1).min(len - 1)
                } else {
                    self.settings_cursor.saturating_sub(1)
                };
                if self.settings_cursor < self.settings_scroll {
                    self.settings_scroll = self.settings_cursor;
                } else if self.settings_cursor >= self.settings_scroll + rows {
                    self.settings_scroll = self.settings_cursor + 1 - rows;
                }
                Some(DesktopAction::Redraw)
            }
            DesktopKey::Activate => {
                match page {
                    PAGE_NETWORK => {
                        self.wifi = !self.wifi;
                        if self.wifi {
                            self.airplane = false;
                        }
                    }
                    PAGE_SOUND => self.toggle_mute(),
                    PAGE_KEYBOARD | PAGE_TIME => {
                        let index = self.settings_cursor;
                        if index < list_len(page) {
                            self.settings_apply(page, index);
                        }
                    }
                    _ => return None,
                }
                Some(DesktopAction::Redraw)
            }
            _ => None,
        }
    }
}

fn draw_toggle_row(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    bounds: Rect,
    label: &str,
    on: bool,
) {
    painter.fill_rounded_rect(
        layout.rect(bounds),
        layout.radii(CornerRadii::all(bounds.height / 2)),
        if on {
            Rgba::new(223, 242, 251, 190)
        } else {
            Rgba::new(169, 184, 191, 120)
        },
    );
    let knob = Rect::new(
        if on {
            bounds.right() - bounds.height + 4
        } else {
            bounds.x + 4
        },
        bounds.y + 4,
        bounds.height - 8,
        bounds.height - 8,
    );
    painter.fill_rounded_rect(
        layout.rect(knob),
        layout.radii(CornerRadii::all(knob.height / 2)),
        if on {
            ACCENT
        } else {
            Rgba::new(250, 251, 252, 255)
        },
    );
    centered_text(
        painter,
        layout,
        font,
        bounds,
        label,
        13,
        Color::rgb(9, 23, 29),
    );
}

fn draw_slider(painter: &mut Painter<'_>, layout: Layout, track: Rect, percent: u8) {
    painter.fill_rounded_rect(
        layout.rect(track),
        layout.radii(CornerRadii::all(6)),
        Rgba::new(169, 184, 191, 150),
    );
    let filled = (track.width * percent as i32 / 100).max(12);
    painter.fill_rounded_rect(
        layout.rect(Rect::new(track.x, track.y, filled, track.height)),
        layout.radii(CornerRadii::all(6)),
        ACCENT,
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(
            track.x + filled - 10,
            track.y - 4,
            20,
            track.height + 8,
        )),
        layout.radii(CornerRadii::all(10)),
        Rgba::new(250, 251, 252, 255),
    );
}

fn value_line(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    pane: Rect,
    y: i32,
    label: &str,
    value: &str,
) {
    text(
        painter,
        layout,
        font,
        Point::new(pane.x, pane.y + y),
        label,
        12,
        INK_SOFT,
    );
    text(
        painter,
        layout,
        font,
        Point::new(pane.x + 110, pane.y + y),
        value,
        12,
        INK,
    );
}

pub(super) fn draw_settings(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
) {
    let base = window_base_rect(DesktopApp::Settings);
    let sidebar = Rect::new(base.x + 8, base.y + 42, 132, (base.height - 54).max(60));
    painter.fill_rounded_rect(
        layout.rect(sidebar),
        layout.radii(CornerRadii::all(18)),
        Rgba::new(222, 237, 241, 110),
    );
    let page = state.settings_page;
    for (index, label) in PAGES.iter().enumerate() {
        let row = sidebar_row(base, index);
        if index as u8 == page {
            painter.fill_rounded_rect(
                layout.rect(row),
                layout.radii(CornerRadii::all(15)),
                Rgba::new(255, 255, 255, 190),
            );
        }
        text(
            painter,
            layout,
            font,
            Point::new(row.x + 14, row.y + 6),
            label,
            12,
            INK,
        );
    }
    let pane = pane(base);
    text(
        painter,
        layout,
        font,
        Point::new(pane.x, pane.y - 2),
        PAGES[page as usize % PAGES.len()],
        20,
        INK,
    );
    match page {
        PAGE_NETWORK => {
            let ready = crate::net::is_ready();
            value_line(
                painter,
                layout,
                font,
                pane,
                40,
                "Status",
                if ready { "Connected" } else { "Not connected" },
            );
            let ip = crate::net::local_address();
            let mut address = ClockBuffer::new();
            let _ = write!(address, "{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
            value_line(
                painter,
                layout,
                font,
                pane,
                64,
                "Address",
                if ready { address.as_str() } else { "-" },
            );
            draw_toggle_row(
                painter,
                layout,
                font,
                toggle_rect(pane, 110),
                if state.wifi { "Wi-Fi on" } else { "Wi-Fi off" },
                state.wifi,
            );
        }
        PAGE_DISPLAY => {
            text(
                painter,
                layout,
                font,
                Point::new(pane.x, pane.y + 36),
                "Brightness",
                12,
                INK_SOFT,
            );
            draw_slider(painter, layout, slider_track(pane, 62), state.brightness);
            let mut percent = ClockBuffer::new();
            let _ = write!(percent, "{}%", state.brightness);
            text(
                painter,
                layout,
                font,
                Point::new(pane.x + pane.width - 70, pane.y + 36),
                percent.as_str(),
                12,
                INK,
            );
            value_line(
                painter,
                layout,
                font,
                pane,
                104,
                "Cursor",
                if crate::virtio_gpu::cursor_active() {
                    "Hardware (GPU)"
                } else {
                    "Software"
                },
            );
            value_line(
                painter,
                layout,
                font,
                pane,
                128,
                "Glass",
                if crate::virtio_gpu::frost_available() {
                    "Blurred on the GPU"
                } else {
                    "Blurred on the CPU"
                },
            );
        }
        PAGE_SOUND => {
            text(
                painter,
                layout,
                font,
                Point::new(pane.x, pane.y + 36),
                "Volume",
                12,
                INK_SOFT,
            );
            draw_slider(
                painter,
                layout,
                slider_track(pane, 62),
                if state.muted { 0 } else { state.volume },
            );
            let mut percent = ClockBuffer::new();
            let _ = write!(percent, "{}%", if state.muted { 0 } else { state.volume });
            text(
                painter,
                layout,
                font,
                Point::new(pane.x + pane.width - 70, pane.y + 36),
                percent.as_str(),
                12,
                INK,
            );
            draw_toggle_row(
                painter,
                layout,
                font,
                toggle_rect(pane, 104),
                if state.muted { "Muted" } else { "Sound on" },
                !state.muted,
            );
        }
        PAGE_KEYBOARD | PAGE_TIME => {
            if page == PAGE_TIME {
                draw_toggle_row(
                    painter,
                    layout,
                    font,
                    time_toggle(pane),
                    if crate::settings::ntp_enabled() {
                        "Set time automatically"
                    } else {
                        "Time set manually"
                    },
                    crate::settings::ntp_enabled(),
                );
            }
            let rows = visible_rows(page, pane);
            let selected = list_selected(page);
            for slot in 0..rows {
                let index = state.settings_scroll + slot;
                if index >= list_len(page) {
                    break;
                }
                let row = list_row(pane, page, slot);
                let current = index == selected;
                painter.fill_rounded_rect(
                    layout.rect(row),
                    layout.radii(CornerRadii::all(row.height / 2)),
                    if current {
                        Rgba::new(223, 242, 251, 220)
                    } else {
                        Rgba::new(255, 255, 255, 90)
                    },
                );
                if index == state.settings_cursor {
                    painter.stroke_rounded_rect(
                        layout.rect(row),
                        layout.radii(CornerRadii::all(row.height / 2)),
                        2,
                        ACCENT,
                    );
                }
                text(
                    painter,
                    layout,
                    font,
                    Point::new(row.x + 16, row.y + 7),
                    list_label(page, index),
                    13,
                    INK,
                );
                if current {
                    painter.fill_rounded_rect(
                        layout.rect(Rect::new(row.right() - 24, row.y + 10, 8, 8)),
                        layout.radii(CornerRadii::all(4)),
                        ACCENT,
                    );
                }
            }
        }
        PAGE_POWER => {
            for (index, label) in ["Lock", "Sleep", "Restart", "Shut down"].iter().enumerate() {
                let button = power_button(pane, index);
                painter.fill_rounded_rect(
                    layout.rect(button),
                    layout.radii(CornerRadii::all(22)),
                    Rgba::new(223, 242, 251, 200),
                );
                centered_text(painter, layout, font, button, label, 14, INK);
            }
        }
        PAGE_ACCOUNT => {
            value_line(
                painter,
                layout,
                font,
                pane,
                40,
                "User name",
                state.username_str(),
            );
            for (index, label) in ["Lock now", "Change password"].iter().enumerate() {
                let button = account_button(pane, index);
                painter.fill_rounded_rect(
                    layout.rect(button),
                    layout.radii(CornerRadii::all(22)),
                    Rgba::new(223, 242, 251, 200),
                );
                centered_text(painter, layout, font, button, label, 14, INK);
            }
            text(
                painter,
                layout,
                font,
                Point::new(pane.x, pane.y + 150),
                "Forgot your password? Use the link on the sign-in screen.",
                11,
                INK_SOFT,
            );
        }
        _ => {
            // About (the last page)
            let cpu = crate::arch::CpuInfo::detect();
            let uptime_s = crate::time::monotonic_nanoseconds() / 1_000_000_000;
            let mut up = ClockBuffer::new();
            let _ = write!(
                up,
                "{}h {:02}m {:02}s",
                uptime_s / 3600,
                uptime_s / 60 % 60,
                uptime_s % 60
            );
            value_line(
                painter,
                layout,
                font,
                pane,
                40,
                "System",
                "AerOS (x86-64, UEFI)",
            );
            let processor = if cpu.brand().is_empty() {
                cpu.vendor()
            } else {
                cpu.brand()
            };
            value_line(painter, layout, font, pane, 64, "Processor", processor);
            value_line(painter, layout, font, pane, 88, "Uptime", up.as_str());
            value_line(
                painter,
                layout,
                font,
                pane,
                112,
                "Time zone",
                crate::timezone::ZONES[crate::rtc::timezone_index() % crate::timezone::ZONES.len()]
                    .name,
            );
            value_line(
                painter,
                layout,
                font,
                pane,
                136,
                "Account",
                state.username_str(),
            );
        }
    }
}
