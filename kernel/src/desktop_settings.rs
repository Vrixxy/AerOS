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
            let processor = if cpu.brand().is_empty() {
                cpu.vendor()
            } else {
                cpu.brand()
            };
            draw_info_box(
                painter,
                layout,
                font,
                Rect::new(pane.x, pane.y, SLIDER_ROW.width, INFO_BOX.height),
                &[
                    ("System", "AerOS (x86-64, UEFI)"),
                    ("Processor", processor),
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
