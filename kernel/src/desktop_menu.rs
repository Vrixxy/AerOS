use super::*;

const OPEN_NS: u64 = 300_000_000;
const NOTE_NS: u64 = 2_400_000_000;

/// Hit boxes of the two corner controls (logical units): the feather at the
/// top left opens the system menu, the gear at the top right the quick
/// settings.
pub(super) const FEATHER: Rect = Rect::new(2, 2, 48, 40);
const FEATHER_ART: Rect = Rect::new(6, 6, 30, 30);
pub(super) const GEAR: Rect = Rect::new(DESIGN_WIDTH - 44, 2, 42, 40);
const GEAR_ART: Rect = Rect::new(712, 6, 28, 28);

pub(super) const MENU_PANEL: Rect = Rect::new(21, 35, 113, 147);
const MENU_ROWS: [(&str, bool); 9] = [
    ("Parcel", false),
    ("Change Background", false),
    ("Settings", true),
    ("Store", false),
    ("Recent Apps", true),
    ("Quit all apps", true),
    ("Shutdown", true),
    ("Restart", false),
    ("Sleep", false),
];

pub(super) const PARCEL_PANEL: Rect = Rect::new(206, 66, 340, 258);
const PEOPLE: usize = 4;
const UPLOAD_SLOT: usize = PEOPLE;

/// Each row's top and height, from the Figma menu.
const MENU_ROW_SPANS: [(i32, i32); 9] = [
    (45, 14),
    (63, 13),
    (79, 13),
    (92, 13),
    (107, 13),
    (123, 13),
    (139, 12),
    (152, 12),
    (165, 12),
];

fn menu_row(index: usize) -> Rect {
    let (top, height) = MENU_ROW_SPANS[index.min(MENU_ROW_SPANS.len() - 1)];
    Rect::new(35, top, 82, height)
}

fn person_circle(index: usize) -> Rect {
    Rect::new(
        PARCEL_PANEL.x + 28 + index as i32 * 76,
        PARCEL_PANEL.y + 24,
        58,
        58,
    )
}

fn upload_tile() -> Rect {
    Rect::new(
        PARCEL_PANEL.x + 20,
        PARCEL_PANEL.y + 124,
        PARCEL_PANEL.width - 40,
        112,
    )
}

/// Whether the corner controls are on screen for this frame: only over the
/// bare desktop, never under a window, a launcher panel or a full-screen mode.
pub(super) fn corners_shown(
    state: &DesktopState,
    window_closing: bool,
    window_opening: bool,
    sliding: Option<Overlay>,
) -> bool {
    state.screen == Screen::Desktop
        && state.app == DesktopApp::None
        && !window_closing
        && !window_opening
        && !state.seamless.enabled
        && state.overlay != Overlay::Apps
        && sliding != Some(Overlay::Apps)
}

impl DesktopState {
    pub(super) fn corners_active(&self) -> bool {
        self.screen == Screen::Desktop
            && self.app == DesktopApp::None
            && !self.seamless.enabled
            && self.overlay != Overlay::Apps
            && !self.power_open
            && self.power_action == shellui::PowerAction::None
    }

    pub(super) fn toggle_menu(&mut self) -> DesktopAction {
        if self.screen != Screen::Desktop {
            return DesktopAction::Idle;
        }
        if self.menu_open || self.parcel_open {
            self.menu_open = false;
            self.parcel_open = false;
            return DesktopAction::Redraw;
        }
        let now = crate::time::monotonic_nanoseconds();
        self.set_overlay(Overlay::None);
        self.ctx_open = false;
        self.clip_open = false;
        self.menu_open = true;
        self.menu_at_ns = now;
        self.menu_focus = 0;
        self.motion_until_ns = self.motion_until_ns.max(now + OPEN_NS + 60_000_000);
        DesktopAction::Redraw
    }

    fn open_parcel(&mut self) {
        let now = crate::time::monotonic_nanoseconds();
        self.menu_open = false;
        self.parcel_open = true;
        self.parcel_at_ns = now;
        self.parcel_focus = UPLOAD_SLOT;
        self.parcel_note_until_ns = 0;
        self.motion_until_ns = self.motion_until_ns.max(now + OPEN_NS + 60_000_000);
    }

    fn quit_all_apps(&mut self) {
        self.open_apps = 0;
        self.set_overlay(Overlay::None);
        self.set_app(DesktopApp::None);
    }

    fn menu_activate(&mut self, index: usize) {
        self.menu_open = false;
        match index {
            0 => self.open_parcel(),
            1 => self.open_backgrounds(),
            2 => {
                self.set_overlay(Overlay::None);
                self.set_app(DesktopApp::Settings);
            }
            3 => {
                self.set_overlay(Overlay::None);
                self.open_store();
            }
            4 => {
                self.switcher_step(false);
            }
            5 => self.quit_all_apps(),
            6 => self.start_power_action(shellui::PowerAction::Shutdown),
            7 => self.start_power_action(shellui::PowerAction::Restart),
            _ => self.start_power_action(shellui::PowerAction::Sleep),
        }
    }

    fn parcel_activate(&mut self, slot: usize) {
        if slot == UPLOAD_SLOT {
            self.parcel_open = false;
            self.set_overlay(Overlay::None);
            self.open_files();
            return;
        }
        let now = crate::time::monotonic_nanoseconds();
        self.parcel_focus = slot;
        self.parcel_note_until_ns = now + NOTE_NS;
        self.motion_until_ns = self.motion_until_ns.max(now + NOTE_NS);
    }

    pub(super) fn menu_key(&mut self, key: DesktopKey) -> Option<DesktopAction> {
        if self.parcel_open {
            match key {
                DesktopKey::Escape | DesktopKey::Menu => self.parcel_open = false,
                DesktopKey::Left | DesktopKey::Up => {
                    self.parcel_focus = (self.parcel_focus + PEOPLE) % (PEOPLE + 1);
                }
                DesktopKey::Right | DesktopKey::Down | DesktopKey::Tab => {
                    self.parcel_focus = (self.parcel_focus + 1) % (PEOPLE + 1);
                }
                DesktopKey::Activate => {
                    let slot = self.parcel_focus;
                    self.parcel_activate(slot);
                }
                _ => return Some(DesktopAction::Idle),
            }
            return Some(DesktopAction::Redraw);
        }
        if !self.menu_open {
            return None;
        }
        let count = MENU_ROWS.len();
        match key {
            DesktopKey::Escape | DesktopKey::Menu => self.menu_open = false,
            DesktopKey::Down | DesktopKey::Tab => self.menu_focus = (self.menu_focus + 1) % count,
            DesktopKey::Up => self.menu_focus = (self.menu_focus + count - 1) % count,
            DesktopKey::Activate => {
                let index = self.menu_focus;
                self.menu_activate(index);
            }
            _ => return Some(DesktopAction::Idle),
        }
        Some(DesktopAction::Redraw)
    }

    pub(super) fn menu_click(&mut self, point: Point) -> Option<DesktopAction> {
        if self.parcel_open {
            for index in 0..PEOPLE {
                if person_circle(index).contains(point) {
                    self.parcel_activate(index);
                    return Some(DesktopAction::Redraw);
                }
            }
            if upload_tile().contains(point) {
                self.parcel_activate(UPLOAD_SLOT);
                return Some(DesktopAction::Redraw);
            }
            if !PARCEL_PANEL.contains(point) {
                self.parcel_open = false;
            }
            return Some(DesktopAction::Redraw);
        }
        if self.menu_open {
            for index in 0..MENU_ROWS.len() {
                if menu_row(index).contains(point) {
                    self.menu_activate(index);
                    return Some(DesktopAction::Redraw);
                }
            }
            if !MENU_PANEL.contains(point) {
                self.menu_open = false;
            }
            return Some(DesktopAction::Redraw);
        }
        if !self.corners_active() {
            return None;
        }
        if FEATHER.contains(point) {
            return Some(self.toggle_menu());
        }
        if GEAR.contains(point) {
            let target = if self.overlay == Overlay::Quick {
                Overlay::None
            } else {
                Overlay::Quick
            };
            self.set_overlay(target);
            self.focus_visible = false;
            return Some(DesktopAction::Redraw);
        }
        None
    }
}

pub(super) fn draw_corners(painter: &mut Painter<'_>, layout: Layout, state: &DesktopState) {
    let lit = state.menu_open || state.parcel_open;
    painter.draw_rgba_scaled_alpha(
        layout.rect(FEATHER_ART),
        desktop_icon(7),
        DESKTOP_ICON_SIZE,
        DESKTOP_ICON_SIZE,
        if lit { 255 } else { 235 },
    );
    painter.draw_rgba_scaled_alpha(
        layout.rect(GEAR_ART),
        desktop_icon(8),
        DESKTOP_ICON_SIZE,
        DESKTOP_ICON_SIZE,
        if state.overlay == Overlay::Quick {
            255
        } else {
            235
        },
    );
}
/// The menu's dark glass: the wallpaper frosted under a 10% white fill.
fn panel_glass(painter: &mut Painter<'_>, layout: Layout, panel: Rect, radius: i32) {
    let _ = frost(
        painter,
        layout,
        panel,
        CornerRadii::all(radius),
        FrostStyle {
            blur_radius: 14,
            saturation_percent: 100,
            brightness_percent: 100,
            tint: Rgba::new(255, 255, 255, 26),
            border: Rgba::new(255, 255, 255, 64),
            border_width: 1,
            inner_highlight: Rgba::new(255, 255, 255, 30),
            inner_shadow: Rgba::transparent(),
            inner_shadow_offset: Point::new(0, 0),
            inner_shadow_softness: 0,
            shadow: Rgba::new(0, 0, 0, 50),
            shadow_offset: Point::new(0, 4),
            shadow_spread: 0,
            shadow_softness: 8,
            noise_alpha: 3,
        },
        0x3e_0001,
    );
}

pub(super) fn draw_menu(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now: u64,
) {
    if !state.menu_open {
        return;
    }
    let pop = ease_out_back_milli(progress_of(now, state.menu_at_ns, OPEN_NS)).max(0);
    let width = (MENU_PANEL.width * pop / 1000).max(12);
    let height = (MENU_PANEL.height * pop / 1000).max(12);
    panel_glass(
        painter,
        layout,
        Rect::new(MENU_PANEL.x, MENU_PANEL.y, width, height),
        18,
    );
    if pop < 800 {
        return;
    }
    let ink = Color::rgb(255, 255, 255);
    for (index, (label, gap)) in MENU_ROWS.iter().enumerate() {
        let row = menu_row(index);
        let focused = state.menu_focus == index;
        if *gap {
            painter.fill_rounded_rect(
                layout.rect(Rect::new(32, row.y - 2, 86, 1)),
                layout.radii(CornerRadii::all(0)),
                Rgba::new(255, 255, 255, 110),
            );
        }
        if index == 0 {
            painter.fill_rounded_rect(
                layout.rect(row),
                layout.radii(CornerRadii::all(row.height / 2)),
                Rgba::new(10, 153, 255, 255),
            );
        } else if focused {
            painter.fill_rounded_rect(
                layout.rect(row),
                layout.radii(CornerRadii::all(row.height / 2)),
                Rgba::new(255, 255, 255, 46),
            );
        }
        if focused {
            painter.stroke_rounded_rect(
                layout.rect(row.expand(1)),
                layout.radii(CornerRadii::all(row.height / 2 + 1)),
                2,
                Rgba::new(255, 255, 255, 220),
            );
        }
        let size = 9;
        text(
            painter,
            layout,
            font,
            Point::new(row.x + 11, row.y + (row.height - size) / 2),
            label,
            size,
            ink,
        );
        if index == 4 {
            text(
                painter,
                layout,
                font,
                Point::new(row.right() - 12, row.y + (row.height - size) / 2),
                ">",
                size,
                ink,
            );
        }
    }
}

pub(super) fn draw_parcel(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now: u64,
) {
    if !state.parcel_open {
        return;
    }
    let pop = ease_out_back_milli(progress_of(now, state.parcel_at_ns, OPEN_NS)).max(0);
    let width =
        (PARCEL_PANEL.width * (700 + pop * 3 / 10) / 1000).clamp(20, PARCEL_PANEL.width + 14);
    let height =
        (PARCEL_PANEL.height * (700 + pop * 3 / 10) / 1000).clamp(20, PARCEL_PANEL.height + 10);
    let panel = Rect::new(
        PARCEL_PANEL.x + (PARCEL_PANEL.width - width) / 2,
        PARCEL_PANEL.y + (PARCEL_PANEL.height - height) / 2,
        width,
        height,
    );
    let radii = layout.radii(CornerRadii::all(36));
    painter.fill_rounded_rect(
        layout.rect(Rect::new(panel.x, panel.y + 5, panel.width, panel.height)),
        radii,
        Rgba::new(0, 0, 0, 60),
    );
    painter.fill_rounded_rect(layout.rect(panel), radii, Rgba::new(4, 4, 4, 250));
    if pop < 800 {
        return;
    }
    let grey = Rgba::new(217, 217, 217, 255);
    for index in 0..PEOPLE {
        let circle = person_circle(index);
        let focused = state.parcel_focus == index;
        painter.fill_rounded_rect(
            layout.rect(circle),
            layout.radii(CornerRadii::all(circle.width / 2)),
            grey,
        );
        if focused {
            painter.stroke_rounded_rect(
                layout.rect(circle.expand(3)),
                layout.radii(CornerRadii::all(circle.width / 2 + 3)),
                2,
                Rgba::new(10, 153, 255, 255),
            );
        }
        centered_text(
            painter,
            layout,
            font,
            Rect::new(circle.x - 8, circle.bottom() + 8, circle.width + 16, 16),
            "Person",
            9,
            Color::rgb(255, 255, 255),
        );
    }
    let tile = upload_tile();
    painter.fill_rounded_rect(layout.rect(tile), layout.radii(CornerRadii::all(36)), grey);
    if state.parcel_focus == UPLOAD_SLOT {
        painter.stroke_rounded_rect(
            layout.rect(tile.expand(3)),
            layout.radii(CornerRadii::all(39)),
            2,
            Rgba::new(10, 153, 255, 255),
        );
    }
    let ink = Rgba::new(8, 8, 8, 255);
    let icon = Rect::new(tile.x + 30, tile.y + (tile.height - 58) / 2, 58, 58);
    painter.stroke_rounded_rect(
        layout.rect(icon),
        layout.radii(CornerRadii::all(17)),
        layout.scale.logical(4).max(2) as u8,
        ink,
    );
    let cx = icon.x + icon.width / 2;
    let cy = icon.y + icon.height / 2;
    painter.fill_rounded_rect(
        layout.rect(Rect::new(cx - 2, cy - 12, 4, 28)),
        layout.radii(CornerRadii::all(2)),
        ink,
    );
    for step in 0..10 {
        for side in [-1, 1] {
            painter.fill_rounded_rect(
                layout.rect(Rect::new(cx + side * step - 2, cy - 14 + step, 4, 4)),
                layout.radii(CornerRadii::all(2)),
                ink,
            );
        }
    }
    text(
        painter,
        layout,
        font,
        Point::new(tile.x + 122, tile.y + (tile.height - 12) / 2),
        "Upload your file",
        12,
        Color::rgb(16, 16, 16),
    );
    if now < state.parcel_note_until_ns {
        let note = Rect::new(PARCEL_PANEL.x + 60, PARCEL_PANEL.bottom() + 10, 220, 24);
        painter.fill_rounded_rect(
            layout.rect(note),
            layout.radii(CornerRadii::all(12)),
            Rgba::new(4, 4, 4, 236),
        );
        centered_text(
            painter,
            layout,
            font,
            note,
            "No devices found nearby yet",
            10,
            Color::rgb(255, 255, 255),
        );
    }
}
