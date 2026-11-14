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
const INK: Color = Color::rgb(245, 245, 245);
const INK_SOFT: Color = Color::rgb(165, 165, 165);
const ACCENT: Rgba = Rgba::new(46, 120, 232, 255);
const OUTLINE: Rgba = Rgba::new(235, 235, 235, 255);
const PILL: Rgba = Rgba::new(222, 222, 222, 255);
const ON: Rgba = Rgba::new(16, 170, 16, 255);

// Sizes from the Figma Settings frame (1237 px wide), scaled to the 752-unit
// design width.
const SETTING_ROW: Rect = Rect::new(0, 0, 152, 37);
const INFO_BOX: Rect = Rect::new(0, 0, 179, 108);
const SLIDER_ROW: Rect = Rect::new(0, 0, 239, 43);
const STACK_GAP: i32 = 12;

fn sidebar(base: Rect) -> Rect {
    Rect::new(base.x + 15, base.y + 23, 102, 286)
}

fn sidebar_row(base: Rect, index: usize) -> Rect {
    Rect::new(base.x + 25, base.y + 30 + index as i32 * 3526 / 100, 82, 24)
}

fn pane(base: Rect) -> Rect {
    Rect::new(base.x + 144, base.y + 61, 390, (base.height - 75).max(100))
}

fn scrollbar(base: Rect) -> Rect {
    Rect::new(base.right() - 17, base.y + 66, 17, 99)
}

fn toggle_rect(pane: Rect, y: i32) -> Rect {
    Rect::new(pane.x, pane.y + y, SETTING_ROW.width, SETTING_ROW.height)
}

fn time_toggle(pane: Rect) -> Rect {
    Rect::new(pane.x, pane.y, SLIDER_ROW.width, SETTING_ROW.height)
}

fn info_rect(pane: Rect, y: i32) -> Rect {
    Rect::new(pane.x, pane.y + y, INFO_BOX.width, INFO_BOX.height)
}

fn slider_row(pane: Rect, y: i32) -> Rect {
    Rect::new(pane.x, pane.y + y, SLIDER_ROW.width, SLIDER_ROW.height)
}

fn slider_track(pane: Rect, y: i32) -> Rect {
    let row = slider_row(pane, y);
    Rect::new(row.x + 76, row.y + 19, 146, 4)
}

fn slider_percent(track: Rect, x: i32) -> i32 {
    (x - track.x) * 100 / track.width.max(1)
}

/// Where the second item of a page's stack starts.
const fn below(height: i32) -> i32 {
    height + STACK_GAP
}

fn list_top(page: u8) -> i32 {
    if page == PAGE_TIME {
        below(SETTING_ROW.height)
    } else {
        0
    }
}

fn visible_rows(page: u8, pane: Rect) -> usize {
    (((pane.height - list_top(page)) / (ROW + ROW_GAP)).max(1) as usize).min(8)
}

fn list_row(pane: Rect, page: u8, slot: usize) -> Rect {
    Rect::new(
        pane.x,
        pane.y + list_top(page) + slot as i32 * (ROW + ROW_GAP),
        SLIDER_ROW.width,
        ROW,
    )
}

fn power_button(pane: Rect, index: usize) -> Rect {
    Rect::new(
        pane.x,
        pane.y + index as i32 * below(SETTING_ROW.height),
        SETTING_ROW.width,
        SETTING_ROW.height,
    )
}

fn account_button(pane: Rect, index: usize) -> Rect {
    Rect::new(
        pane.x,
        pane.y + below(INFO_BOX.height) + index as i32 * below(SETTING_ROW.height),
        SETTING_ROW.width,
        SETTING_ROW.height,
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
        if self.settings_page == PAGE_NETWORK && self.wifi && !self.airplane {
            let _ = crate::mt7921::scan();
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
            PAGE_NETWORK if crate::mt7921::present() => return self.wifi_click(pane, point),
            PAGE_NETWORK if toggle_rect(pane, 0).contains(point) => {
                self.wifi = !self.wifi;
                if self.wifi {
                    self.airplane = false;
                }
                return Some(DesktopAction::Redraw);
            }
            PAGE_DISPLAY => {
                if slider_row(pane, 0).contains(point) {
                    let raw = slider_percent(slider_track(pane, 0), point.x);
                    self.set_brightness(raw.clamp(8, 100) as u8, now);
                    return Some(DesktopAction::Redraw);
                }
            }
            PAGE_SOUND => {
                if slider_row(pane, 0).contains(point) {
                    let raw = slider_percent(slider_track(pane, 0), point.x);
                    self.set_volume_to(raw.clamp(0, 100) as u8);
                    return Some(DesktopAction::Redraw);
                }
                if toggle_rect(pane, below(SLIDER_ROW.height)).contains(point) {
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
        if page == PAGE_NETWORK && self.wifi_choice.is_some() {
            return self.wifi_key(key);
        }
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
    painter.stroke_rounded_rect(
        layout.rect(bounds),
        layout.radii(CornerRadii::all(bounds.height / 2)),
        1,
        OUTLINE,
    );
    let switch = Rect::new(bounds.right() - 69, bounds.y + 5, 65, 27);
    painter.fill_rounded_rect(
        layout.rect(switch),
        layout.radii(CornerRadii::all(switch.height / 2)),
        if on { ON } else { Rgba::new(80, 80, 80, 255) },
    );
    let knob = Rect::new(
        if on { switch.x + 32 } else { switch.x + 3 },
        switch.y + 3,
        30,
        21,
    );
    painter.fill_rounded_rect(
        layout.rect(knob),
        layout.radii(CornerRadii::all(knob.height / 2)),
        Rgba::new(255, 255, 255, 255),
    );
    text(
        painter,
        layout,
        font,
        Point::new(bounds.x + 12, bounds.y + 12),
        label,
        10,
        INK,
    );
}

fn draw_info_box(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    bounds: Rect,
    lines: &[(&str, &str)],
) {
    painter.stroke_rounded_rect(
        layout.rect(bounds),
        layout.radii(CornerRadii::all(18)),
        1,
        OUTLINE,
    );
    for (index, (label, value)) in lines.iter().enumerate() {
        let y = bounds.y + 13 + index as i32 * 17;
        text(
            painter,
            layout,
            font,
            Point::new(bounds.x + 18, y),
            label,
            10,
            INK_SOFT,
        );
        text(
            painter,
            layout,
            font,
            Point::new(bounds.x + 78, y),
            value,
            10,
            INK,
        );
    }
}

fn draw_slider_row(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    bounds: Rect,
    track: Rect,
    label: &str,
    percent: u8,
) {
    painter.stroke_rounded_rect(
        layout.rect(bounds),
        layout.radii(CornerRadii::all(bounds.height / 2)),
        1,
        OUTLINE,
    );
    text(
        painter,
        layout,
        font,
        Point::new(bounds.x + 12, bounds.y + 16),
        label,
        10,
        INK,
    );
    painter.fill_rounded_rect(
        layout.rect(track),
        layout.radii(CornerRadii::all(2)),
        Rgba::new(225, 225, 225, 255),
    );
    let filled = track.width * i32::from(percent.min(100)) / 100;
    painter.fill_rounded_rect(
        layout.rect(Rect::new(track.x, track.y, filled.max(4), track.height)),
        layout.radii(CornerRadii::all(2)),
        Rgba::new(150, 150, 150, 255),
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(track.x + filled - 10, track.y - 9, 21, 21)),
        layout.radii(CornerRadii::all(10)),
        Rgba::new(70, 70, 70, 255),
    );
}
/// The small blue icon at the left of sidebar row `page`.
fn draw_page_icon(painter: &mut Painter<'_>, layout: Layout, page: usize, row: Rect) {
    let x = row.x + 7;
    let y = row.y + row.height / 2;
    let part = |dx: i32, dy: i32, width: i32, height: i32| {
        layout.rect(Rect::new(x + dx, y + dy, width, height))
    };
    let round = |value: i32| layout.radii(CornerRadii::all(value));
    let thin = layout.scale.logical(2).max(1) as u8;
    let art = match page {
        0 => Some(9),
        1 => Some(15),
        3 => Some(16),
        4 => Some(17),
        5 => Some(18),
        6 => Some(19),
        7 => Some(20),
        _ => None,
    };
    if let Some(index) = art {
        painter.draw_rgba_scaled(
            layout.rect(Rect::new(row.x + 4, row.y + (row.height - 16) / 2, 16, 16)),
            desktop_icon(index),
            DESKTOP_ICON_SIZE,
            DESKTOP_ICON_SIZE,
        );
        return;
    }
    match page {
        0 => {
            for (step, height) in [3, 6, 9].iter().enumerate() {
                painter.fill_rounded_rect(
                    part(step as i32 * 4, 5 - height, 3, *height),
                    round(1),
                    ACCENT,
                );
            }
        }
        1 => {
            painter.fill_rounded_rect(part(0, -6, 12, 9), round(2), ACCENT);
            painter.fill_rounded_rect(part(4, 4, 4, 2), round(1), ACCENT);
        }
        2 => {
            painter.fill_rounded_rect(part(0, -2, 4, 5), round(1), ACCENT);
            painter.fill_rounded_rect(part(3, -5, 4, 11), round(2), ACCENT);
            painter.fill_rounded_rect(part(9, -3, 2, 7), round(1), ACCENT);
        }
        3 => {
            painter.stroke_rounded_rect(part(0, -5, 13, 10), round(2), thin, ACCENT);
            painter.fill_rounded_rect(part(3, 0, 7, 2), round(1), ACCENT);
        }
        4 => {
            painter.stroke_rounded_rect(part(0, -6, 12, 12), round(6), thin, ACCENT);
            painter.fill_rounded_rect(part(5, -3, 2, 4), round(1), ACCENT);
            painter.fill_rounded_rect(part(5, 0, 4, 2), round(1), ACCENT);
        }
        5 => {
            painter.stroke_rounded_rect(part(0, -5, 12, 12), round(6), thin, ACCENT);
            painter.fill_rounded_rect(part(5, -7, 2, 7), round(1), ACCENT);
        }
        6 => {
            painter.fill_rounded_rect(part(3, -6, 6, 6), round(3), ACCENT);
            painter.fill_rounded_rect(part(0, 1, 12, 6), round(3), ACCENT);
        }
        _ => {
            painter.stroke_rounded_rect(part(0, -6, 12, 12), round(6), thin, ACCENT);
            painter.fill_rounded_rect(part(5, -3, 2, 2), round(1), ACCENT);
            painter.fill_rounded_rect(part(5, 0, 2, 4), round(1), ACCENT);
        }
    }
}

pub(super) fn draw_settings(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
) {
    let base = window_base_rect(DesktopApp::Settings);
    painter.fill_rounded_rect(
        layout.rect(sidebar(base)),
        layout.radii(CornerRadii::all(16)),
        Rgba::new(58, 58, 58, 255),
    );
    let page = state.settings_page;
    for (index, label) in PAGES.iter().enumerate() {
        let row = sidebar_row(base, index);
        painter.fill_rounded_rect(
            layout.rect(row),
            layout.radii(CornerRadii::all(row.height / 2)),
            if index as u8 == page {
                Rgba::new(255, 255, 255, 255)
            } else {
                PILL
            },
        );
        draw_page_icon(painter, layout, index, row);
        centered_text(
            painter,
            layout,
            font,
            Rect::new(row.x + 12, row.y, row.width - 12, row.height),
            label,
            12,
            Color::rgb(10, 10, 10),
        );
    }
    let bar = scrollbar(base);
    painter.fill_rounded_rect(
        layout.rect(bar),
        layout.radii(CornerRadii::all(bar.width / 2)),
        Rgba::new(235, 235, 235, 255),
    );
    let pane = pane(base);
    text(
        painter,
        layout,
        font,
        Point::new(pane.x, pane.y - 31),
        PAGES[page as usize % PAGES.len()],
        14,
        INK,
    );
    match page {
        PAGE_NETWORK if crate::mt7921::present() => draw_wifi(painter, layout, font, state, pane),
        PAGE_NETWORK => {
            draw_toggle_row(
                painter,
                layout,
                font,
                toggle_rect(pane, 0),
                "Wi-Fi",
                state.wifi,
            );
            let ready = crate::net::is_ready();
            let ip = crate::net::local_address();
            let mut address = ClockBuffer::new();
            let _ = write!(address, "{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
            draw_info_box(
                painter,
                layout,
                font,
                info_rect(pane, below(SETTING_ROW.height)),
                &[
                    ("Status", if ready { "Connected" } else { "Not connected" }),
                    ("Address", if ready { address.as_str() } else { "-" }),
                ],
            );
        }
        PAGE_DISPLAY => {
            draw_slider_row(
                painter,
                layout,
                font,
                slider_row(pane, 0),
                slider_track(pane, 0),
                "Brightness",
                state.brightness,
            );
            draw_info_box(
                painter,
                layout,
                font,
                info_rect(pane, below(SLIDER_ROW.height)),
                &[
                    (
                        "Cursor",
                        if crate::virtio_gpu::cursor_active() {
                            "Hardware (GPU)"
                        } else {
                            "Software"
                        },
                    ),
                    (
                        "Glass",
                        if crate::virtio_gpu::frost_available() {
                            "Blurred on the GPU"
                        } else {
                            "Blurred on the CPU"
                        },
                    ),
                ],
            );
        }
        PAGE_SOUND => {
            draw_slider_row(
                painter,
                layout,
                font,
                slider_row(pane, 0),
                slider_track(pane, 0),
                "Volume",
                if state.muted { 0 } else { state.volume },
            );
            draw_toggle_row(
                painter,
                layout,
                font,
                toggle_rect(pane, below(SLIDER_ROW.height)),
                "Sound",
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
                    "Set time automatically",
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
                if index == selected {
                    painter.fill_rounded_rect(
                        layout.rect(row),
                        layout.radii(CornerRadii::all(row.height / 2)),
                        Rgba::new(255, 255, 255, 40),
                    );
                }
                painter.stroke_rounded_rect(
                    layout.rect(row),
                    layout.radii(CornerRadii::all(row.height / 2)),
                    if index == state.settings_cursor { 2 } else { 1 },
                    if index == state.settings_cursor {
                        ACCENT
                    } else {
                        OUTLINE
                    },
                );
                text(
                    painter,
                    layout,
                    font,
                    Point::new(row.x + 14, row.y + 8),
                    list_label(page, index),
                    11,
                    INK,
                );
                if index == selected {
                    painter.fill_rounded_rect(
                        layout.rect(Rect::new(row.right() - 22, row.y + 10, 8, 8)),
                        layout.radii(CornerRadii::all(4)),
                        ON,
                    );
                }
            }
        }
        PAGE_POWER => {
            let mut charge = ClockBuffer::new();
            crate::battery::describe(
                crate::battery::level(crate::time::monotonic_nanoseconds()),
                &mut charge,
            );
            draw_info_box(
                painter,
                layout,
                font,
                Rect::new(
                    pane.x + SETTING_ROW.width + 24,
                    pane.y,
                    INFO_BOX.width,
                    INFO_BOX.height / 2,
                ),
                &[
                    ("Battery", charge.as_str()),
                    ("Saver", if state.battery_saver { "On" } else { "Off" }),
                ],
            );
            for (index, label) in ["Lock", "Sleep", "Restart", "Shut down"].iter().enumerate() {
                let button = power_button(pane, index);
                painter.stroke_rounded_rect(
                    layout.rect(button),
                    layout.radii(CornerRadii::all(button.height / 2)),
                    1,
                    OUTLINE,
                );
                centered_text(painter, layout, font, button, label, 12, INK);
            }
        }
        PAGE_ACCOUNT => {
            draw_info_box(
                painter,
                layout,
                font,
                info_rect(pane, 0),
                &[
                    ("User name", state.username_str()),
                    ("Password", "Forgot it? Use the sign-in screen"),
                ],
            );
            for (index, label) in ["Lock now", "Change password"].iter().enumerate() {
                let button = account_button(pane, index);
                painter.stroke_rounded_rect(
                    layout.rect(button),
                    layout.radii(CornerRadii::all(button.height / 2)),
                    1,
                    OUTLINE,
                );
                centered_text(painter, layout, font, button, label, 12, INK);
            }
        }
        _ => {
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
            let (total, used) = crate::sysmon::memory();
            let mut memory = ClockBuffer::new();
            let gib = |bytes: u64| (bytes / (1 << 20) * 10 / 1024) as u32;
            let _ = write!(
                memory,
                "{}.{} of {}.{} GB",
                gib(used) / 10,
                gib(used) % 10,
                gib(total) / 10,
                gib(total) % 10
            );
            let processor = if cpu.brand().is_empty() {
                cpu.vendor()
            } else {
                cpu.brand()
            };
            draw_info_box(
                painter,
                layout,
                font,
                Rect::new(pane.x, pane.y, SLIDER_ROW.width, INFO_BOX.height + 17),
                &[
                    ("System", "AerOS (x86-64, UEFI)"),
                    ("Processor", processor),
                    ("Memory", memory.as_str()),
                    ("Uptime", up.as_str()),
                    (
                        "Time zone",
                        crate::timezone::ZONES
                            [crate::rtc::timezone_index() % crate::timezone::ZONES.len()]
                        .name,
                    ),
                    ("Account", state.username_str()),
                ],
            );
        }
    }
}

pub(super) const WIFI_ROWS: usize = 6;
const WIFI_LIST_TOP: i32 = 49;
const WIFI_ROW_WIDTH: i32 = 360;

fn wifi_row(pane: Rect, slot: usize) -> Rect {
    Rect::new(
        pane.x,
        pane.y + WIFI_LIST_TOP + slot as i32 * (ROW + ROW_GAP),
        WIFI_ROW_WIDTH,
        ROW,
    )
}

fn wifi_forget_button(row: Rect) -> Rect {
    Rect::new(row.right() - 104, row.y + 4, 62, row.height - 8)
}

/// The saved password that would join `network`, if one fits it.
fn wifi_saved(network: &crate::mt7921::Network) -> Option<crate::wifi_known::Known> {
    let ssid = &network.ssid[..network.ssid_len.min(network.ssid.len())];
    let saved = crate::wifi_known::lookup(ssid)?;
    let fits = match network.security {
        crate::mt7921::Security::Open => saved.passphrase().is_empty(),
        crate::mt7921::Security::Wpa2 => saved.passphrase().len() >= 8,
        crate::mt7921::Security::Unsupported => false,
    };
    fits.then_some(saved)
}

fn wifi_joined(status: &crate::mt7921::Status, network: &crate::mt7921::Network) -> bool {
    status.link == crate::mt7921::Link::Connected
        && status.ssid_len == network.ssid_len
        && status.ssid == network.ssid
}

fn wifi_refresh_button(pane: Rect) -> Rect {
    Rect::new(pane.x + WIFI_ROW_WIDTH - 86, pane.y + 6, 86, 25)
}

fn wifi_field(pane: Rect) -> Rect {
    Rect::new(pane.x, pane.y + WIFI_LIST_TOP + 24, 280, 30)
}

fn wifi_reveal_button(pane: Rect) -> Rect {
    let field = wifi_field(pane);
    Rect::new(field.right() + 8, field.y + 2, 64, 26)
}

fn wifi_button(pane: Rect, index: usize) -> Rect {
    Rect::new(
        pane.x + index as i32 * 112,
        pane.y + WIFI_LIST_TOP + 68,
        100,
        30,
    )
}

impl DesktopState {
    /// Polls the card, follows the Wi-Fi switch, and refreshes the list.
    /// True when the screen should be redrawn.
    pub(super) fn wifi_tick(&mut self) -> bool {
        crate::mt7921::poll();
        let wanted = self.wifi && !self.airplane;
        if wanted != self.wifi_applied {
            self.wifi_applied = wanted;
            if wanted {
                let _ = crate::mt7921::scan();
            } else {
                self.wifi_choice = None;
                crate::mt7921::disconnect();
            }
        }
        let status = crate::mt7921::status();
        if self.wifi_choice.is_none() {
            let mut found = [crate::mt7921::Network::EMPTY; crate::mt7921::MAX_NETWORKS];
            let count = crate::mt7921::networks(&mut found).min(WIFI_ROWS);
            self.wifi_rows[..count].copy_from_slice(&found[..count]);
            self.wifi_row_count = if wanted { count } else { 0 };
        }
        let mut signature = status.link as u64
            ^ (u64::from(status.scanning) << 4)
            ^ ((status.message.as_ptr() as u64) << 8)
            ^ ((self.wifi_row_count as u64) << 40)
            ^ (u64::from(crate::net::is_ready()) << 48);
        for (index, network) in self.wifi_rows[..self.wifi_row_count].iter().enumerate() {
            signature = signature.rotate_left(7)
                ^ u64::from(network.ssid[0])
                ^ (u64::from(network.bars()) << 8)
                ^ ((index as u64) << 12);
        }
        let changed = signature != self.wifi_signature;
        self.wifi_signature = signature;
        let visible = (self.app == DesktopApp::Settings && self.settings_page == PAGE_NETWORK)
            || self.overlay == Overlay::Quick;
        changed && visible
    }

    /// Joins a network this machine has joined before, without being asked:
    /// looks for one in the scan results (scanning now and then while there
    /// is none) and queues the connection with its saved password. Joining
    /// takes a few seconds in which nothing else moves, so it waits for a
    /// pause in the input (`idle_ns`), though not forever; after a few failed
    /// attempts it leaves the choice to the user.
    pub(super) fn wifi_auto(&mut self, now: u64, idle_ns: u64) {
        const RESCAN_NS: u64 = 20_000_000_000;
        const RETRY_NS: u64 = 30_000_000_000;
        const PAUSE_NS: u64 = 1_500_000_000;
        const PATIENCE_NS: u64 = 15_000_000_000;
        const ATTEMPTS: u8 = 3;
        let status = crate::mt7921::status();
        let wanted = self.wifi && !self.airplane && self.wifi_applied;
        if !wanted
            || status.link != crate::mt7921::Link::Idle
            || status.scanning
            || self.wifi_choice.is_some()
            || self.wifi_pending
            || self.wifi_auto_failures >= ATTEMPTS
            || now < self.wifi_auto_next_ns
            || !crate::wifi_known::any()
        {
            self.wifi_auto_ready_ns = 0;
            return;
        }
        let mut found = [crate::mt7921::Network::EMPTY; crate::mt7921::MAX_NETWORKS];
        let count = crate::mt7921::networks(&mut found);
        let known = found[..count]
            .iter()
            .find_map(|network| Some((*network, wifi_saved(network)?)));
        let Some((network, saved)) = known else {
            let _ = crate::mt7921::scan();
            self.wifi_auto_next_ns = now + RESCAN_NS;
            self.wifi_auto_ready_ns = 0;
            return;
        };
        if self.wifi_auto_ready_ns == 0 {
            self.wifi_auto_ready_ns = now;
        }
        if idle_ns < PAUSE_NS && now.saturating_sub(self.wifi_auto_ready_ns) < PATIENCE_NS {
            return;
        }
        let passphrase = saved.passphrase();
        self.wifi_password.fill(0);
        self.wifi_password[..passphrase.len()].copy_from_slice(passphrase);
        self.wifi_password_len = passphrase.len();
        self.wifi_choice = Some(network);
        self.wifi_pending = true;
        self.wifi_auto_attempt = true;
        self.wifi_auto_ready_ns = 0;
        self.wifi_auto_next_ns = now + RETRY_NS;
    }

    /// Warns once at 10% and once at 5% while the battery runs down; at 5%
    /// it also turns on battery saver and saves open work.
    pub(super) fn battery_tick(&mut self, now: u64) {
        let Some(level) = crate::battery::level(now) else {
            return;
        };
        if level.charging || level.percent > 15 {
            self.battery_warned = 100;
            return;
        }
        let mut body = ClockBuffer56::new();
        if level.percent <= 5 && self.battery_warned > 5 {
            self.battery_warned = 5;
            if !self.battery_saver {
                self.set_battery_saver(true);
            }
            self.notes_autosave();
            self.prefs_flush();
            let _ = write!(body, "{}% left. Plug in now.", level.percent);
            crate::notify::push(0, "Battery very low", body.as_str());
        } else if level.percent <= 10 && self.battery_warned > 10 {
            self.battery_warned = 10;
            let _ = write!(body, "{}% left. Plug in your charger soon.", level.percent);
            crate::notify::push(0, "Battery low", body.as_str());
        }
    }

    fn current_prefs(&self) -> crate::settings::Prefs {
        crate::settings::Prefs {
            volume: self.volume,
            muted: self.muted,
            brightness: self.brightness,
            wifi: self.wifi,
            bluetooth: self.bluetooth,
            airplane: self.airplane,
            battery_saver: self.battery_saver,
            adblock_off: self.adblock_off,
        }
    }

    /// Puts back the sound, brightness and switches of the last session.
    #[cfg_attr(feature = "boot-test", allow(dead_code))]
    pub(super) fn apply_saved_prefs(&mut self) {
        let Some(prefs) = crate::settings::prefs() else {
            return;
        };
        self.volume = prefs.volume;
        self.volume_prev = prefs.volume;
        self.muted = prefs.muted;
        self.apply_volume();
        self.brightness = prefs.brightness;
        self.brightness_prev = prefs.brightness;
        crate::framebuffer::set_brightness(prefs.brightness);
        self.wifi = prefs.wifi;
        self.bluetooth = prefs.bluetooth;
        self.airplane = prefs.airplane;
        self.adblock_off = prefs.adblock_off;
        if prefs.battery_saver {
            self.set_battery_saver(true);
        }
        self.prefs_saved = Some(prefs);
        serial::format(format_args!(
            "AEROS_PREFS restored=true volume={} muted={} brightness={} wifi={} bluetooth={} airplane={} battery_saver={}\n",
            prefs.volume,
            prefs.muted,
            prefs.brightness,
            prefs.wifi,
            prefs.bluetooth,
            prefs.airplane,
            prefs.battery_saver
        ));
    }

    /// Writes the preferences once a change has stood for a moment (a slider
    /// being dragged changes many times a second).
    pub(super) fn prefs_tick(&mut self, now: u64) {
        const SETTLE_NS: u64 = 1_500_000_000;
        if !self.persist_account || self.screen != Screen::Desktop {
            return;
        }
        let current = self.current_prefs();
        if self.prefs_saved == Some(current) {
            self.prefs_pending = None;
        } else if self.prefs_pending != Some(current) {
            self.prefs_pending = Some(current);
            self.prefs_changed_ns = now;
        } else if now.saturating_sub(self.prefs_changed_ns) >= SETTLE_NS {
            self.prefs_flush();
        }
    }

    /// Writes the preferences now if they differ from what is on disk.
    pub(super) fn prefs_flush(&mut self) {
        if !self.persist_account || self.screen != Screen::Desktop {
            return;
        }
        let current = self.current_prefs();
        if self.prefs_saved != Some(current) {
            let saved = crate::settings::set_prefs(current);
            if saved {
                self.prefs_saved = Some(current);
            }
            serial::format(format_args!(
                "AEROS_PREFS saved={saved} volume={} brightness={}\n",
                current.volume, current.brightness
            ));
        }
        self.prefs_pending = None;
    }

    /// Runs a connection the user asked for, after the "Connecting" frame is shown.
    pub(super) fn wifi_run_pending(&mut self) -> bool {
        if !self.wifi_pending {
            return false;
        }
        self.wifi_pending = false;
        let Some(network) = self.wifi_choice.take() else {
            return true;
        };
        let password =
            core::str::from_utf8(&self.wifi_password[..self.wifi_password_len]).unwrap_or("");
        let joined = crate::mt7921::connect(&network, password).is_ok();
        let automatic = core::mem::take(&mut self.wifi_auto_attempt);
        if joined {
            self.wifi_auto_failures = 0;
            if self.persist_account {
                let ssid = &network.ssid[..network.ssid_len.min(network.ssid.len())];
                crate::wifi_known::remember(ssid, password.as_bytes());
            }
        } else if automatic {
            self.wifi_auto_failures = self.wifi_auto_failures.saturating_add(1);
        }
        self.wifi_password.fill(0);
        self.wifi_password_len = 0;
        self.wifi_reveal = false;
        true
    }

    fn wifi_pick(&mut self, network: crate::mt7921::Network) {
        let status = crate::mt7921::status();
        if wifi_joined(&status, &network) {
            crate::mt7921::disconnect();
            // Left on purpose: not joined again behind the user's back.
            self.wifi_auto_failures = u8::MAX;
            return;
        }
        self.wifi_choice = Some(network);
        self.wifi_password.fill(0);
        self.wifi_password_len = 0;
        if let Some(saved) = wifi_saved(&network) {
            let passphrase = saved.passphrase();
            self.wifi_password[..passphrase.len()].copy_from_slice(passphrase);
            self.wifi_password_len = passphrase.len();
            self.wifi_pending = true;
        } else if network.security != crate::mt7921::Security::Wpa2 {
            self.wifi_pending = true;
        }
    }

    fn wifi_click(&mut self, pane: Rect, point: Point) -> Option<DesktopAction> {
        if toggle_rect(pane, 0).contains(point) {
            self.wifi = !self.wifi;
            if self.wifi {
                self.airplane = false;
            }
            return Some(DesktopAction::Redraw);
        }
        if let Some(network) = self.wifi_choice {
            if network.security != crate::mt7921::Security::Wpa2 || self.wifi_pending {
                return None;
            }
            if wifi_reveal_button(pane).contains(point) {
                self.wifi_reveal = !self.wifi_reveal;
            } else if wifi_button(pane, 0).contains(point) {
                self.wifi_pending = self.wifi_password_len >= 8;
            } else if wifi_button(pane, 1).contains(point) {
                self.wifi_choice = None;
            } else {
                return None;
            }
            return Some(DesktopAction::Redraw);
        }
        if !self.wifi || self.airplane {
            return None;
        }
        if wifi_refresh_button(pane).contains(point) {
            let _ = crate::mt7921::scan();
            return Some(DesktopAction::Redraw);
        }
        let status = crate::mt7921::status();
        for slot in 0..self.wifi_row_count {
            let row = wifi_row(pane, slot);
            if row.contains(point) {
                let network = self.wifi_rows[slot];
                if !wifi_joined(&status, &network)
                    && wifi_forget_button(row).contains(point)
                    && wifi_saved(&network).is_some()
                {
                    let ssid = &network.ssid[..network.ssid_len.min(network.ssid.len())];
                    crate::wifi_known::forget(ssid);
                    return Some(DesktopAction::Redraw);
                }
                self.wifi_pick(network);
                return Some(DesktopAction::Redraw);
            }
        }
        None
    }

    fn wifi_key(&mut self, key: DesktopKey) -> Option<DesktopAction> {
        if self.wifi_pending {
            return Some(DesktopAction::Idle);
        }
        match key {
            DesktopKey::Escape => self.wifi_choice = None,
            DesktopKey::Activate => self.wifi_pending = self.wifi_password_len >= 8,
            DesktopKey::Backspace => {
                self.wifi_password_len = self.wifi_password_len.saturating_sub(1)
            }
            DesktopKey::Character(ch) if (' '..='~').contains(&ch) => {
                if self.wifi_password_len < 63 {
                    self.wifi_password[self.wifi_password_len] = ch as u8;
                    self.wifi_password_len += 1;
                }
            }
            _ => return Some(DesktopAction::Idle),
        }
        Some(DesktopAction::Redraw)
    }
}

fn draw_signal(painter: &mut Painter<'_>, layout: Layout, at: Point, bars: u8) {
    for bar in 0..3u8 {
        let height = 5 + bar as i32 * 4;
        let rect = Rect::new(at.x + bar as i32 * 6, at.y + 13 - height, 4, height);
        painter.fill_rounded_rect(
            layout.rect(rect),
            layout.radii(CornerRadii::all(1)),
            if bar < bars {
                Rgba::new(245, 245, 245, 255)
            } else {
                Rgba::new(110, 110, 110, 255)
            },
        );
    }
}

fn draw_button(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    bounds: Rect,
    label: &str,
    filled: bool,
) {
    let radii = layout.radii(CornerRadii::all(bounds.height / 2));
    if filled {
        painter.fill_rounded_rect(layout.rect(bounds), radii, ACCENT);
    } else {
        painter.stroke_rounded_rect(layout.rect(bounds), radii, 1, OUTLINE);
    }
    centered_text(painter, layout, font, bounds, label, 11, INK);
}

fn wifi_status_line(state: &DesktopState, on: bool, line: &mut ClockBuffer) {
    use crate::mt7921::Link;
    let status = crate::mt7921::status();
    if !on {
        let _ = write!(line, "Wi-Fi is off");
    } else if state.wifi_pending || status.link == Link::Connecting {
        let _ = write!(line, "Connecting...");
    } else if status.link == Link::Connected && crate::net::is_ready() {
        let ip = crate::net::local_address();
        let _ = write!(line, "{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    } else if !status.message.is_empty() {
        let _ = write!(line, "{}", status.message);
    } else if status.scanning {
        let _ = write!(line, "Looking for networks...");
    }
}

fn draw_wifi(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    pane: Rect,
) {
    use crate::mt7921::Security;
    let on = state.wifi && !state.airplane;
    draw_toggle_row(painter, layout, font, toggle_rect(pane, 0), "Wi-Fi", on);
    let mut line = ClockBuffer::new();
    wifi_status_line(state, on, &mut line);
    text(
        painter,
        layout,
        font,
        Point::new(pane.x + SETTING_ROW.width + 12, pane.y + 13),
        line.as_str(),
        10,
        INK_SOFT,
    );
    if !on {
        return;
    }
    let status = crate::mt7921::status();
    if let Some(network) = state.wifi_choice {
        if network.security != Security::Wpa2 || state.wifi_pending {
            return;
        }
        let mut prompt = ClockBuffer::new();
        let _ = write!(prompt, "Password for {}", network.name());
        text(
            painter,
            layout,
            font,
            Point::new(pane.x, pane.y + WIFI_LIST_TOP + 4),
            prompt.as_str(),
            11,
            INK,
        );
        let field = wifi_field(pane);
        painter.stroke_rounded_rect(
            layout.rect(field),
            layout.radii(CornerRadii::all(field.height / 2)),
            2,
            ACCENT,
        );
        let len = state.wifi_password_len;
        let mut shown = [b'*'; 64];
        if state.wifi_reveal {
            shown[..len].copy_from_slice(&state.wifi_password[..len]);
        }
        let mut typed = ClockBuffer::new();
        let _ = write!(
            typed,
            "{}_",
            core::str::from_utf8(&shown[..len]).unwrap_or("")
        );
        text(
            painter,
            layout,
            font,
            Point::new(field.x + 14, field.y + 9),
            typed.as_str(),
            11,
            INK,
        );
        let reveal = if state.wifi_reveal { "Hide" } else { "Show" };
        draw_button(
            painter,
            layout,
            font,
            wifi_reveal_button(pane),
            reveal,
            false,
        );
        draw_button(
            painter,
            layout,
            font,
            wifi_button(pane, 0),
            "Connect",
            len >= 8,
        );
        draw_button(painter, layout, font, wifi_button(pane, 1), "Cancel", false);
        return;
    }
    let refresh = if status.scanning {
        "Looking..."
    } else {
        "Refresh"
    };
    draw_button(
        painter,
        layout,
        font,
        wifi_refresh_button(pane),
        refresh,
        false,
    );
    if state.wifi_row_count == 0 {
        let empty = if status.scanning {
            "Looking for networks..."
        } else {
            "No networks found. Try Refresh."
        };
        text(
            painter,
            layout,
            font,
            Point::new(pane.x, pane.y + WIFI_LIST_TOP + 8),
            empty,
            11,
            INK_SOFT,
        );
    }
    for (slot, network) in state.wifi_rows[..state.wifi_row_count].iter().enumerate() {
        let row = wifi_row(pane, slot);
        let radii = layout.radii(CornerRadii::all(row.height / 2));
        let joined = wifi_joined(&status, network);
        let saved = !joined && wifi_saved(network).is_some();
        if joined {
            painter.fill_rounded_rect(layout.rect(row), radii, Rgba::new(255, 255, 255, 40));
        }
        painter.stroke_rounded_rect(
            layout.rect(row),
            radii,
            1,
            if joined { ACCENT } else { OUTLINE },
        );
        text(
            painter,
            layout,
            font,
            Point::new(row.x + 14, row.y + 8),
            network.name(),
            11,
            INK,
        );
        let note = match (joined, network.security) {
            (true, _) => "Connected - click to disconnect",
            _ if saved => "Saved",
            (false, Security::Wpa2) => "Password",
            (false, Security::Open) => "Open",
            (false, Security::Unsupported) => "Not supported yet",
        };
        text(
            painter,
            layout,
            font,
            Point::new(row.right() - 200, row.y + 9),
            note,
            9,
            INK_SOFT,
        );
        if saved {
            draw_button(
                painter,
                layout,
                font,
                wifi_forget_button(row),
                "Forget",
                false,
            );
        }
        draw_signal(
            painter,
            layout,
            Point::new(row.right() - 30, row.y + 7),
            network.bars(),
        );
    }
}
