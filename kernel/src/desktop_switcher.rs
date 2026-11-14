use super::*;

// Sizes from the Figma switcher (scaled to the 752-unit design width).
const CARD_WIDTH: i32 = 207;
const CARD_HEIGHT: i32 = 186;
const GAP: i32 = 70;
const PADDING: i32 = 50;
const TOP: i32 = 64;
const BOTTOM: i32 = 33;
const MAX_APPS: usize = 9;

const ALL_APPS: [DesktopApp; MAX_APPS] = [
    DesktopApp::Terminal,
    DesktopApp::Files,
    DesktopApp::Browser,
    DesktopApp::Notes,
    DesktopApp::Settings,
    DesktopApp::Store,
    DesktopApp::Trash,
    DesktopApp::Linux,
    DesktopApp::Avatar,
];

fn app_bit(app: DesktopApp) -> u16 {
    1 << (app as u8)
}

fn app_title(app: DesktopApp) -> &'static str {
    match app {
        DesktopApp::Terminal => "Terminal",
        DesktopApp::Files => "Files",
        DesktopApp::Browser => "Browser",
        DesktopApp::Notes => "Notes",
        DesktopApp::Settings => "Settings",
        DesktopApp::Store => "Store",
        DesktopApp::Trash => "Trash",
        DesktopApp::Linux => "Linux",
        DesktopApp::Avatar => "Avatar",
        DesktopApp::None => "",
    }
}

/// The dock icon art an app uses, when it has one.
fn app_icon(app: DesktopApp) -> Option<usize> {
    match app {
        DesktopApp::Terminal => Some(1),
        DesktopApp::Files => Some(3),
        DesktopApp::Browser => Some(4),
        DesktopApp::Notes => Some(5),
        DesktopApp::Trash => Some(6),
        _ => None,
    }
}

impl DesktopState {
    pub(super) fn app_is_open(&self, app: DesktopApp) -> bool {
        app != DesktopApp::None && self.open_apps & app_bit(app) != 0
    }

    /// The apps that were opened and not closed, in a fixed order.
    pub(super) fn open_app_list(&self) -> ([DesktopApp; MAX_APPS], usize) {
        let mut list = [DesktopApp::None; MAX_APPS];
        let mut count = 0;
        for app in ALL_APPS {
            if self.open_apps & app_bit(app) != 0 {
                list[count] = app;
                count += 1;
            }
        }
        (list, count)
    }

    pub(super) fn app_close(&mut self) {
        self.open_apps &= !app_bit(self.app);
        self.set_app(DesktopApp::None);
    }

    /// Alt+Tab: opens the switcher on the next app, or moves along it.
    pub(super) fn switcher_step(&mut self, back: bool) -> bool {
        let (list, count) = self.open_app_list();
        if count == 0 {
            return false;
        }
        let now = crate::time::monotonic_nanoseconds();
        if !self.switcher_open {
            let current = list[..count].iter().position(|app| *app == self.app);
            self.switcher_index = match (current, back) {
                (Some(index), false) => (index + 1) % count,
                (Some(index), true) => (index + count - 1) % count,
                (None, _) => 0,
            };
            self.switcher_open = true;
            self.switcher_at_ns = now;
            self.motion_until_ns = self.motion_until_ns.max(now + 360_000_000);
            return true;
        }
        self.switcher_index = if back {
            (self.switcher_index + count - 1) % count
        } else {
            (self.switcher_index + 1) % count
        };
        true
    }

    /// Releasing Alt (or Enter) switches to the highlighted app.
    pub(super) fn switcher_commit(&mut self) -> bool {
        if !self.switcher_open {
            return false;
        }
        self.switcher_open = false;
        let (list, count) = self.open_app_list();
        if count > 0 {
            let target = list[self.switcher_index.min(count - 1)];
            if target != self.app {
                self.set_overlay(Overlay::None);
                self.set_app(target);
            }
        }
        true
    }

    pub(super) fn switcher_cancel(&mut self) -> bool {
        let was_open = self.switcher_open;
        self.switcher_open = false;
        was_open
    }

    pub(super) fn switcher_click(&mut self, point: Point) -> Option<DesktopAction> {
        if !self.switcher_open {
            return None;
        }
        let (_, count) = self.open_app_list();
        let panel = switcher_panel(count);
        for index in 0..count {
            if switcher_card(panel, index, count).contains(point) {
                self.switcher_index = index;
                self.switcher_commit();
                return Some(DesktopAction::Redraw);
            }
        }
        self.switcher_cancel();
        Some(DesktopAction::Redraw)
    }
}

/// How much the cards shrink (in thousandths) so `count` of them fit.
fn card_scale(count: usize) -> i32 {
    let count = count.max(1) as i32;
    let natural = count * CARD_WIDTH + (count - 1) * GAP;
    ((DESIGN_WIDTH - 40 - PADDING * 2) * 1000 / natural).min(1000)
}

fn switcher_panel(count: usize) -> Rect {
    let scale = card_scale(count);
    let count = count.max(1) as i32;
    let width = PADDING * 2 + (count * CARD_WIDTH + (count - 1) * GAP) * scale / 1000;
    let height = TOP + BOTTOM + CARD_HEIGHT * scale / 1000;
    Rect::new((DESIGN_WIDTH - width) / 2, 58, width, height)
}

fn switcher_card(panel: Rect, index: usize, count: usize) -> Rect {
    let scale = card_scale(count);
    Rect::new(
        panel.x + PADDING + index as i32 * (CARD_WIDTH + GAP) * scale / 1000,
        panel.y + TOP,
        CARD_WIDTH * scale / 1000,
        CARD_HEIGHT * scale / 1000,
    )
}

pub(super) fn draw_switcher(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now: u64,
) {
    if !state.switcher_open {
        return;
    }
    let (list, count) = state.open_app_list();
    if count == 0 {
        return;
    }
    let pop = ease_out_back_milli(progress_of(now, state.switcher_at_ns, 300_000_000)).max(0);
    let full = switcher_panel(count);
    let width = (full.width * (700 + pop * 3 / 10) / 1000).clamp(40, full.width + 12);
    let height = (full.height * (700 + pop * 3 / 10) / 1000).clamp(30, full.height + 8);
    let panel = Rect::new(
        full.x + (full.width - width) / 2,
        full.y + (full.height - height) / 2,
        width,
        height,
    );
    painter.fill_rounded_rect(
        layout.rect(panel),
        layout.radii(CornerRadii::all(70.min(panel.height / 2))),
        Rgba::new(0, 0, 0, 51),
    );
    if pop < 800 {
        return;
    }
    for (index, app) in list[..count].iter().enumerate() {
        let card = switcher_card(full, index, count);
        let selected = index == state.switcher_index.min(count - 1);
        text(
            painter,
            layout,
            font,
            Point::new(card.x + 8, card.y - 26),
            app_title(*app),
            19,
            Color::rgb(255, 255, 255),
        );
        painter.fill_rounded_rect(
            layout.rect(card),
            layout.radii(CornerRadii::all(card.width / 5)),
            Rgba::opaque(0xd9, 0xd9, 0xd9),
        );
        let side = (card.width * 31 / 100).max(24);
        let icon = Rect::new(
            card.x + (card.width - side) / 2,
            card.y + (card.height - side) / 2,
            side,
            side,
        );
        match app_icon(*app) {
            Some(kind) => draw_icon(painter, layout, kind, icon),
            None => centered_text(
                painter,
                layout,
                font,
                icon,
                &app_title(*app)[..1],
                30,
                Color::rgb(0, 0, 0),
            ),
        }
        if selected {
            let pill = Rect::new(card.x + card.width / 2 - 11, card.bottom() + 8, 22, 7);
            painter.fill_rounded_rect(
                layout.rect(pill),
                layout.radii(CornerRadii::all(3)),
                Rgba::opaque(0, 0, 0),
            );
            painter.fill_rounded_rect(
                layout.rect(pill.expand(-1)),
                layout.radii(CornerRadii::all(3)),
                Rgba::opaque(0xd9, 0xd9, 0xd9),
            );
        }
    }
}
