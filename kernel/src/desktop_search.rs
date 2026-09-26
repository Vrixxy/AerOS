use super::*;

pub(super) const SEARCH_MAX: usize = 24;
const OPEN_NS: u64 = 380_000_000;
const CLOSE_NS: u64 = 240_000_000;
pub(super) const SEARCH_PILL: Rect = Rect::new(188, 96, 376, 58);
const ROW_HEIGHT: i32 = 40;

const ENTRIES: [(&str, u8); 8] = [
    ("Terminal", 0),
    ("Files", 1),
    ("Browser", 2),
    ("Notes", 3),
    ("Settings", 4),
    ("Store", 5),
    ("Trash", 6),
    ("Linux", 7),
];

fn matches(state: &DesktopState, out: &mut [usize; 8]) -> usize {
    let needle = &state.search_input[..state.search_len];
    let mut count = 0;
    for (index, (name, _)) in ENTRIES.iter().enumerate() {
        let hit = needle.is_empty()
            || name
                .as_bytes()
                .windows(needle.len())
                .any(|window| window.eq_ignore_ascii_case(needle));
        if hit {
            out[count] = index;
            count += 1;
        }
    }
    count
}

fn row_rect(slot: usize) -> Rect {
    Rect::new(
        SEARCH_PILL.x + 22,
        SEARCH_PILL.bottom() + 12 + slot as i32 * (ROW_HEIGHT + 8),
        SEARCH_PILL.width - 44,
        ROW_HEIGHT,
    )
}

impl DesktopState {
    pub(super) fn search_visible(&self, now: u64) -> bool {
        self.search_open || now < self.search_at_ns.saturating_add(CLOSE_NS)
    }

    pub(super) fn open_search(&mut self) {
        if self.search_open || self.screen != Screen::Desktop {
            return;
        }
        let now = crate::time::monotonic_nanoseconds();
        self.set_overlay(Overlay::None);
        self.search_open = true;
        self.search_at_ns = now;
        self.search_len = 0;
        self.search_focus = 0;
        self.motion_until_ns = self.motion_until_ns.max(now + OPEN_NS + 80_000_000);
    }

    fn close_search(&mut self) {
        if !self.search_open {
            return;
        }
        let now = crate::time::monotonic_nanoseconds();
        self.search_open = false;
        self.search_at_ns = now;
        self.motion_until_ns = self.motion_until_ns.max(now + CLOSE_NS + 60_000_000);
    }

    fn launch_entry(&mut self, entry: u8) {
        self.close_search();
        match entry {
            0 => self.set_app(DesktopApp::Terminal),
            1 => self.open_files(),
            2 => self.open_browser(),
            3 => self.open_notes(),
            4 => self.set_app(DesktopApp::Settings),
            5 => self.open_store(),
            6 => self.open_trash(),
            _ => {
                self.seamless.enabled = false;
                self.set_app(DesktopApp::Linux);
            }
        }
    }

    pub(super) fn search_key(&mut self, key: DesktopKey) -> Option<DesktopAction> {
        if !self.search_open {
            return None;
        }
        let mut hits = [0usize; 8];
        let count = matches(self, &mut hits);
        match key {
            DesktopKey::Escape => self.close_search(),
            DesktopKey::Backspace => {
                self.search_len = self.search_len.saturating_sub(1);
                self.search_focus = 0;
            }
            DesktopKey::Character(byte)
                if self.search_len < SEARCH_MAX && (byte.is_ascii_graphic() || byte == b' ') =>
            {
                self.search_input[self.search_len] = byte;
                self.search_len += 1;
                self.search_focus = 0;
                self.search_typed_ns = crate::time::monotonic_nanoseconds();
                self.motion_until_ns = self.motion_until_ns.max(self.search_typed_ns + 300_000_000);
            }
            DesktopKey::Down | DesktopKey::Tab if count > 0 => {
                self.search_focus = (self.search_focus + 1) % count;
            }
            DesktopKey::Up if count > 0 => {
                self.search_focus = (self.search_focus + count - 1) % count;
            }
            DesktopKey::Activate if count > 0 => {
                let entry = ENTRIES[hits[self.search_focus.min(count - 1)]].1;
                self.launch_entry(entry);
            }
            _ => {}
        }
        Some(DesktopAction::Redraw)
    }

    pub(super) fn search_click(&mut self, point: Point) -> Option<DesktopAction> {
        if !self.search_open {
            return None;
        }
        let mut hits = [0usize; 8];
        let count = matches(self, &mut hits);
        for slot in 0..count {
            if row_rect(slot).contains(point) {
                self.launch_entry(ENTRIES[hits[slot]].1);
                return Some(DesktopAction::Redraw);
            }
        }
        if !SEARCH_PILL.contains(point) {
            self.close_search();
        }
        Some(DesktopAction::Redraw)
    }
}

pub(super) fn draw_search(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now: u64,
) {
    if !state.search_visible(now) {
        return;
    }
    let progress = if state.search_open {
        ease_out_back_milli(progress_of(now, state.search_at_ns, OPEN_NS))
    } else {
        1000 - ease_in_out_milli(progress_of(now, state.search_at_ns, CLOSE_NS))
    };
    let rise = (1000 - progress) * 46 / 1000;
    let grow = progress.clamp(0, 1200);
    let width = SEARCH_PILL.width * (600 + grow * 4 / 10) / 1000;
    let mut pill = Rect::new(
        SEARCH_PILL.x + (SEARCH_PILL.width - width) / 2,
        SEARCH_PILL.y - rise,
        width,
        SEARCH_PILL.height,
    );
    pill.y = pill.y.max(-30);
    let mut style = setup_field_style();
    style.tint = Rgba::new(210, 226, 236, 70);
    style.border = Rgba::new(255, 255, 255, 150);
    let radii = CornerRadii::all(pill.height / 2);
    let _ = frost_mode(painter, layout, pill, radii, style, 0xae72_0001, true);
    painter.fill_rounded_rect(
        layout.rect(pill),
        layout.radii(radii),
        Rgba::new(255, 255, 255, 46),
    );
    let mut line = ClockBuffer::new();
    let query = core::str::from_utf8(&state.search_input[..state.search_len]).unwrap_or("");
    if state.search_len == 0 {
        let _ = line.write_str("Search");
    } else {
        let _ = line.write_str(query);
    }
    if state.search_open && (now / 520_000_000).is_multiple_of(2) {
        let _ = line.write_str("_");
    }
    let x = pill.x + 42;
    text(
        painter,
        layout,
        font,
        Point::new(x, pill.y + pill.height / 2 - 9),
        line.as_str(),
        18,
        if state.search_len == 0 {
            Color::rgb(46, 60, 72)
        } else {
            Color::rgb(14, 20, 26)
        },
    );
    let since_typed = now.saturating_sub(state.search_typed_ns);
    if since_typed < 260_000_000 {
        let pulse = 260_000_000 - since_typed;
        painter.stroke_rounded_rect(
            layout.rect(pill.expand((pulse / 50_000_000) as i32 + 1)),
            layout.radii(CornerRadii::all(pill.height / 2 + 4)),
            2,
            Rgba::new(255, 255, 255, (pulse * 140 / 260_000_000) as u8),
        );
    }
    if progress < 700 {
        return;
    }
    let mut hits = [0usize; 8];
    let count = matches(state, &mut hits);
    let age = if state.search_open {
        now.saturating_sub(state.search_at_ns)
    } else {
        0
    };
    for slot in 0..count {
        let delay = 140_000_000 + slot as u64 * 45_000_000;
        let arrive = ease_out_milli(progress_of(age.saturating_sub(delay), 0, 260_000_000)) as i32;
        if arrive <= 0 {
            continue;
        }
        let mut row = row_rect(slot);
        row.y += (1000 - arrive) * 16 / 1000;
        let focused = slot == state.search_focus;
        let radius = layout.radii(CornerRadii::all(ROW_HEIGHT / 2));
        painter.fill_rounded_rect(
            layout.rect(row),
            radius,
            Rgba::new(
                if focused { 255 } else { 226 },
                if focused { 255 } else { 238 },
                if focused { 255 } else { 244 },
                (arrive * if focused { 150 } else { 84 } / 1000) as u8,
            ),
        );
        painter.stroke_rounded_rect(
            layout.rect(row),
            radius,
            1,
            Rgba::new(255, 255, 255, (arrive * 120 / 1000) as u8),
        );
        if focused {
            painter.stroke_rounded_rect(
                layout.rect(row.expand(2)),
                layout.radii(CornerRadii::all(ROW_HEIGHT / 2 + 2)),
                2,
                Rgba::new(255, 255, 255, 190),
            );
        }
        centered_text(
            painter,
            layout,
            font,
            row,
            ENTRIES[hits[slot]].0,
            14,
            Color::rgb(14, 22, 28),
        );
    }
    if count == 0 {
        centered_text(
            painter,
            layout,
            font,
            Rect::new(
                SEARCH_PILL.x,
                SEARCH_PILL.bottom() + 14,
                SEARCH_PILL.width,
                30,
            ),
            "Nothing found",
            14,
            Color::rgb(30, 44, 54),
        );
    }
}

// ------------------------------------------------------------ dock hover

pub(super) const DOCK_ICON_Y: i32 = 380;

pub(super) fn dock_app(index: usize) -> DesktopApp {
    match index {
        1 => DesktopApp::Terminal,
        3 => DesktopApp::Files,
        4 => DesktopApp::Browser,
        5 => DesktopApp::Notes,
        6 => DesktopApp::Trash,
        _ => DesktopApp::None,
    }
}

impl DesktopState {
    /// Called with the pointer position (design coordinates) when it moves.
    pub(super) fn hover_dock(&mut self, point: Point, now: u64) -> bool {
        let mut over = usize::MAX;
        if self.screen == Screen::Desktop && self.overlay == Overlay::None {
            for index in 0..DOCK_BUTTONS.len() {
                if Rect::new(44 + index as i32 * 64, DOCK_ICON_Y - 10, 50, 62).contains(point) {
                    over = index;
                }
            }
        }
        if over == self.hover_index {
            return false;
        }
        self.hover_prev = self.hover_index;
        self.hover_index = over;
        self.hover_at_ns = now;
        self.motion_until_ns = self.motion_until_ns.max(now + 700_000_000);
        true
    }

    /// How far (logical px) dock button `index` floats up for the pointer:
    /// the hovered one springs up, its neighbours follow a little, and the
    /// one the pointer just left settles back down.
    pub(super) fn dock_lift(&self, index: usize, now: u64) -> i32 {
        if self.overlay != Overlay::None
            || self.overlay_closing != Overlay::None
                && now < self.overlay_close_at_ns + OVERLAY_TRANSITION_NS
        {
            return 0;
        }
        let spring = |distance: usize, height: i32, forward: bool| -> i32 {
            let strength = match distance {
                0 => 1000,
                1 => 380,
                2 => 110,
                _ => 0,
            };
            let progress = if forward {
                ease_out_back_milli(progress_of(now, self.hover_at_ns, 320_000_000)).max(0)
            } else {
                1000 - ease_out_milli(progress_of(now, self.hover_at_ns, 260_000_000)) as i32
            };
            height * strength / 1000 * progress / 1000
        };
        let mut lift = 0;
        if self.hover_index != usize::MAX {
            lift = lift.max(spring(self.hover_index.abs_diff(index), 8, true));
        }
        if self.hover_prev != usize::MAX && self.hover_prev != self.hover_index {
            lift = lift.max(spring(self.hover_prev.abs_diff(index), 8, false));
        }
        lift
    }

    pub(super) fn preview_wanted(&self) -> bool {
        self.hover_index != usize::MAX
            && self.app != DesktopApp::None
            && dock_app(self.hover_index) == self.app
    }

    pub(super) fn preview_target(&self, now: u64) -> Option<(usize, i32)> {
        if self.hover_index == usize::MAX
            || self.screen != Screen::Desktop
            || self.overlay != Overlay::None
            || self.power_open
        {
            return None;
        }
        let app = dock_app(self.hover_index);
        if app == DesktopApp::None || self.app != app {
            return None;
        }
        let waited = now.saturating_sub(self.hover_at_ns);
        if waited < 260_000_000 {
            return None;
        }
        let pop = ease_out_back_milli(progress_of(waited, 260_000_000, 380_000_000));
        Some((self.hover_index, pop))
    }
}

/// A small live copy of the hovered app's window, floating above its dock
/// button.
pub(super) fn draw_dock_preview(
    painter: &mut Painter<'_>,
    layout: Layout,
    state: &DesktopState,
    now: u64,
) {
    let Some((index, pop)) = state.preview_target(now) else {
        return;
    };
    let window = layout.rect(window_base_rect(state.app));
    let cache = unsafe { &mut *PANEL_CACHE.0.get() };
    let length = frame_pixels(window);
    if length == 0 || length > PANEL_CACHE_PIXELS {
        return;
    }
    if !painter.read_region(window, &mut cache[..length]) {
        return;
    }
    PANEL_CACHE_KEY.store(0, Ordering::Release);
    let width = 168 * pop.max(0) / 1000;
    let height = width * window.height / window.width.max(1);
    let center = 44 + index as i32 * 64 + 25;
    let outer = Rect::new(
        center - width / 2 - 6,
        344 - height - 12,
        width + 12,
        height + 12,
    );
    if width < 24 {
        return;
    }
    let radii = layout.radii(CornerRadii::all(16));
    painter.fill_rounded_rect(
        layout.rect(Rect::new(outer.x, outer.y + 3, outer.width, outer.height)),
        radii,
        Rgba::new(0, 0, 0, 50),
    );
    painter.fill_rounded_rect(layout.rect(outer), radii, Rgba::new(226, 228, 230, 250));
    painter.stroke_rounded_rect(layout.rect(outer), radii, 1, Rgba::new(28, 30, 32, 120));
    let inner = layout.rect(Rect::new(outer.x + 6, outer.y + 6, width, height));
    unsafe {
        painter.blit_scaled(
            inner,
            cache.as_ptr(),
            window.width as usize,
            window.height as usize,
            window.width as usize,
        );
    }
}
