use super::*;

const CARD_WIDTH: i32 = 112;
const CARD_HEIGHT: i32 = 98;
const GAP: i32 = 12;
const PADDING: i32 = 18;
const CAPTION: i32 = 30;
const MAX_APPS: usize = 8;

const ALL_APPS: [DesktopApp; MAX_APPS] = [
    DesktopApp::Terminal,
    DesktopApp::Files,
    DesktopApp::Browser,
    DesktopApp::Notes,
    DesktopApp::Settings,
    DesktopApp::Store,
    DesktopApp::Trash,
    DesktopApp::Linux,
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
            if switcher_card(panel, index).contains(point) {
                self.switcher_index = index;
                self.switcher_commit();
                return Some(DesktopAction::Redraw);
            }
        }
        self.switcher_cancel();
        Some(DesktopAction::Redraw)
    }
}

fn switcher_panel(count: usize) -> Rect {
    let count = count.max(1) as i32;
    let width = PADDING * 2 + count * CARD_WIDTH + (count - 1) * GAP;
    let height = PADDING * 2 + CARD_HEIGHT + CAPTION;
    Rect::new((DESIGN_WIDTH - width) / 2, 168, width, height)
}

fn switcher_card(panel: Rect, index: usize) -> Rect {
    Rect::new(
        panel.x + PADDING + index as i32 * (CARD_WIDTH + GAP),
        panel.y + PADDING,
        CARD_WIDTH,
        CARD_HEIGHT,
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
    let _ = frost(
        painter,
        layout,
        panel,
        CornerRadii::all(30),
        window_style(),
        0xaed0_0900,
    );
    if pop < 800 {
        return;
    }
    for (index, app) in list[..count].iter().enumerate() {
        let card = switcher_card(full, index);
        let selected = index == state.switcher_index.min(count - 1);
        painter.fill_rounded_rect(
            layout.rect(card),
            layout.radii(CornerRadii::all(22)),
            if selected {
                Rgba::new(246, 248, 250, 250)
            } else {
                Rgba::new(222, 226, 229, 232)
            },
        );
        if selected {
            painter.stroke_rounded_rect(
                layout.rect(card.expand(2)),
                layout.radii(CornerRadii::all(24)),
                3,
                Rgba::new(46, 120, 232, 255),
            );
        }
        let icon = Rect::new(card.x + card.width / 2 - 26, card.y + 14, 52, 52);
        match app_icon(*app) {
            Some(kind) => draw_icon(painter, layout, kind, icon),
            None => {
                let title = app_title(*app);
                painter.fill_rounded_rect(
                    layout.rect(icon),
                    layout.radii(CornerRadii::all(16)),
                    Rgba::new(40, 48, 56, 255),
                );
                centered_text(
                    painter,
                    layout,
                    font,
                    icon,
                    &title[..1],
                    24,
                    Color::rgb(240, 244, 247),
                );
            }
        }
        centered_text(
            painter,
            layout,
            font,
            Rect::new(card.x, card.y + 70, card.width, 22),
            app_title(*app),
            13,
            Color::rgb(20, 26, 32),
        );
    }
    let current = list[state.switcher_index.min(count - 1)];
    centered_text(
        painter,
        layout,
        font,
        Rect::new(full.x, full.bottom() - CAPTION + 2, full.width, 22),
        app_title(current),
        14,
        Color::rgb(244, 247, 249),
    );
}
