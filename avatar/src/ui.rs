//! Toolkit-free UI for the avatar app, laid out from the Figma wireframe:
//! big circular preview on the left, a panel on the right with a pill bar
//! (name) over a 2x2 grid of category tiles.
//!
//! Follows AerOS's UI contract: measurement (`Layout`), input (`Ui::handle`)
//! and painting (`paint`) are separate, nothing allocates, activation is
//! returned as data, and drawing goes through the `Painter` trait so AerOS
//! (via `aerui`) and the host preview each supply their own backend.

use crate::model::Rgb;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect { x, y, w, h }
    }
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Circle {
    pub cx: i32,
    pub cy: i32,
    pub r: i32,
}

impl Circle {
    pub fn contains(&self, px: i32, py: i32) -> bool {
        let (dx, dy) = ((px - self.cx) as i64, (py - self.cy) as i64);
        dx * dx + dy * dy <= (self.r as i64) * (self.r as i64)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Face,
    Hair,
    Style,
    Mood,
}

impl Category {
    pub const ALL: [Category; 4] = [
        Category::Face,
        Category::Hair,
        Category::Style,
        Category::Mood,
    ];
    pub const fn label(self) -> &'static str {
        match self {
            Category::Face => "Face",
            Category::Hair => "Hair",
            Category::Style => "Style",
            Category::Mood => "Mood",
        }
    }
}

/// Positions in logical pixels, derived from the wireframe's proportions.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub bounds: Rect,
    pub preview: Circle,
    pub panel: Rect,
    pub pill: Rect,
    pub tiles: [Rect; 4],
    pub panel_radius: i32,
    pub tile_radius: i32,
}

impl Layout {
    pub fn compute(bounds: Rect) -> Layout {
        let (w, h) = (bounds.w as f32, bounds.h as f32);
        let px = |f: f32| bounds.x + (w * f) as i32;
        let py = |f: f32| bounds.y + (h * f) as i32;

        let r = (h * 0.361) as i32;
        let preview = Circle {
            cx: px(0.303),
            cy: py(0.491),
            r,
        };

        let panel = Rect::new(
            px(0.5700),
            py(0.0355),
            (w * 0.4079) as i32,
            (h * 0.9310) as i32,
        );
        let inset = (panel.w as f32 * 0.0825) as i32;
        let inner_w = panel.w - inset * 2;

        let pill = Rect::new(
            panel.x + inset,
            panel.y + (panel.h as f32 * 0.0585) as i32,
            inner_w,
            (panel.h as f32 * 0.112) as i32,
        );

        let gap = (panel.w as f32 * 0.035) as i32;
        let tile_w = (inner_w - gap) / 2;
        let tile_h = (panel.h as f32 * 0.301) as i32;
        let grid_y = panel.y + (panel.h as f32 * 0.3194) as i32;
        let mut tiles = [Rect::new(0, 0, 0, 0); 4];
        for (i, t) in tiles.iter_mut().enumerate() {
            let (col, row) = ((i % 2) as i32, (i / 2) as i32);
            *t = Rect::new(
                pill.x + col * (tile_w + gap),
                grid_y + row * (tile_h + gap),
                tile_w,
                tile_h,
            );
        }

        Layout {
            bounds,
            preview,
            panel,
            pill,
            tiles,
            panel_radius: (panel.w as f32 * 0.12) as i32,
            tile_radius: (tile_w as f32 * 0.27) as i32,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    None,
    Preview,
    Pill,
    Tile(Category),
}

#[derive(Clone, Copy, Debug)]
pub enum Event {
    Move { x: i32, y: i32 },
    Press { x: i32, y: i32 },
    Release { x: i32, y: i32 },
}

/// What a completed click means; the app turns these into `Command`s.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    OpenCategory(Category),
    EditName,
    /// Tapping the avatar rolls a new random one.
    Randomize,
}

pub struct Ui {
    pub layout: Layout,
    pub hover: Hit,
    pub pressed: Hit,
    pub selected: Option<Category>,
}

impl Ui {
    pub fn new(bounds: Rect) -> Ui {
        Ui {
            layout: Layout::compute(bounds),
            hover: Hit::None,
            pressed: Hit::None,
            selected: None,
        }
    }

    pub fn resize(&mut self, bounds: Rect) {
        self.layout = Layout::compute(bounds);
    }

    pub fn hit_test(&self, x: i32, y: i32) -> Hit {
        let l = &self.layout;
        if l.preview.contains(x, y) {
            return Hit::Preview;
        }
        if l.pill.contains(x, y) {
            return Hit::Pill;
        }
        for (i, t) in l.tiles.iter().enumerate() {
            if t.contains(x, y) {
                return Hit::Tile(Category::ALL[i]);
            }
        }
        Hit::None
    }

    pub fn handle(&mut self, event: Event) -> Option<Action> {
        match event {
            Event::Move { x, y } => {
                self.hover = self.hit_test(x, y);
                None
            }
            Event::Press { x, y } => {
                self.pressed = self.hit_test(x, y);
                None
            }
            Event::Release { x, y } => {
                let hit = self.hit_test(x, y);
                let was = core::mem::replace(&mut self.pressed, Hit::None);
                if hit != was {
                    return None;
                }
                match hit {
                    Hit::Tile(c) => {
                        self.selected = if self.selected == Some(c) {
                            None
                        } else {
                            Some(c)
                        };
                        Some(Action::OpenCategory(c))
                    }
                    Hit::Pill => Some(Action::EditName),
                    Hit::Preview => Some(Action::Randomize),
                    Hit::None => None,
                }
            }
        }
    }
}

pub mod theme {
    use crate::model::Rgb;
    pub const DESKTOP: Rgb = Rgb(45, 45, 45);
    pub const WINDOW: Rgb = Rgb(5, 5, 5);
    pub const PREVIEW: Rgb = Rgb(217, 217, 217);
    pub const PANEL: Rgb = Rgb(24, 24, 24);
    pub const ITEM: Rgb = Rgb(40, 40, 40);
    pub const ITEM_HOVER: Rgb = Rgb(52, 52, 52);
    pub const ITEM_PRESSED: Rgb = Rgb(30, 30, 30);
    pub const ITEM_SELECTED: Rgb = Rgb(66, 66, 66);
}

/// Drawing backend. `avatar` must render the live avatar clipped to the circle.
pub trait Painter {
    fn round_rect(&mut self, r: Rect, radius: i32, color: Rgb);
    fn circle(&mut self, c: Circle, color: Rgb);
    fn avatar(&mut self, c: Circle);
    fn label(&mut self, r: Rect, text: &str, color: Rgb);
}

pub fn paint(ui: &Ui, name: &str, p: &mut impl Painter) {
    use theme::*;
    let l = &ui.layout;
    p.round_rect(l.bounds, 0, WINDOW);
    p.circle(l.preview, PREVIEW);
    p.avatar(l.preview);
    p.round_rect(l.panel, l.panel_radius, PANEL);

    let state = |hit: Hit, selected: bool| {
        if ui.pressed == hit && ui.hover == hit {
            ITEM_PRESSED
        } else if selected {
            ITEM_SELECTED
        } else if ui.hover == hit {
            ITEM_HOVER
        } else {
            ITEM
        }
    };

    p.round_rect(l.pill, l.pill.h / 2, state(Hit::Pill, false));
    p.label(l.pill, name, Rgb(235, 235, 235));
    for (i, c) in Category::ALL.iter().enumerate() {
        let hit = Hit::Tile(*c);
        p.round_rect(
            l.tiles[i],
            l.tile_radius,
            state(hit, ui.selected == Some(*c)),
        );
        p.label(l.tiles[i], c.label(), Rgb(235, 235, 235));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_tile_and_preview() {
        let mut ui = Ui::new(Rect::new(0, 0, 836, 507));
        let t = ui.layout.tiles[1];
        let (x, y) = (t.x + t.w / 2, t.y + t.h / 2);
        ui.handle(Event::Press { x, y });
        assert_eq!(
            ui.handle(Event::Release { x, y }),
            Some(Action::OpenCategory(Category::Hair))
        );
        let c = ui.layout.preview;
        ui.handle(Event::Press { x: c.cx, y: c.cy });
        assert_eq!(
            ui.handle(Event::Release { x: c.cx, y: c.cy }),
            Some(Action::Randomize)
        );
    }

    #[test]
    fn release_elsewhere_cancels() {
        let mut ui = Ui::new(Rect::new(0, 0, 836, 507));
        let t = ui.layout.tiles[0];
        ui.handle(Event::Press {
            x: t.x + 5,
            y: t.y + 5,
        });
        assert_eq!(ui.handle(Event::Release { x: 0, y: 0 }), None);
    }
}
