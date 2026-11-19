//! Toolkit-free UI for the avatar app, laid out from the Figma frame
//! (1237x751): a glowing circular preview on the left; on the right a panel
//! with a scrollable row of category tabs over a scrollable two-column grid
//! of the chosen category's options.
//!
//! Follows AerOS's UI contract: measurement (`Layout`), input (`Ui::handle`)
//! and painting (`paint`) are separate, nothing allocates, activation is
//! returned as data, and drawing goes through the `Painter` trait so AerOS
//! (via `aerui`) and the host preview each supply their own backend.

use crate::model::{
    Avatar, BrowStyle, EyeStyle, FacialHair, Glasses, HairStyle, HeadShape, MouthStyle, NoseStyle,
    Outfit, Rgb,
};
use crate::{Command, Expression};

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
    fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.x + other.w
            && other.x < self.x + self.w
            && self.y < other.y + other.h
            && other.y < self.y + self.h
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
    Eyes,
    Brows,
    Nose,
    Mouth,
    Beard,
    Glasses,
    Style,
    Mood,
}

const MOODS: [&str; 7] = [
    "Neutral",
    "Happy",
    "Laugh",
    "Sad",
    "Angry",
    "Surprised",
    "Wink",
];

impl Category {
    pub const ALL: [Category; 10] = [
        Category::Face,
        Category::Hair,
        Category::Eyes,
        Category::Brows,
        Category::Nose,
        Category::Mouth,
        Category::Beard,
        Category::Glasses,
        Category::Style,
        Category::Mood,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Category::Face => "Face",
            Category::Hair => "Hair",
            Category::Eyes => "Eyes",
            Category::Brows => "Brows",
            Category::Nose => "Nose",
            Category::Mouth => "Mouth",
            Category::Beard => "Beard",
            Category::Glasses => "Glasses",
            Category::Style => "Style",
            Category::Mood => "Mood",
        }
    }

    pub fn option_count(self) -> usize {
        match self {
            Category::Face => HeadShape::ALL.len(),
            Category::Hair => HairStyle::ALL.len(),
            Category::Eyes => EyeStyle::ALL.len(),
            Category::Brows => BrowStyle::ALL.len(),
            Category::Nose => NoseStyle::ALL.len(),
            Category::Mouth => MouthStyle::ALL.len(),
            Category::Beard => FacialHair::ALL.len(),
            Category::Glasses => Glasses::ALL.len(),
            Category::Style => Outfit::ALL.len(),
            Category::Mood => Expression::PRESETS.len().min(MOODS.len()),
        }
    }

    pub fn option_label(self, index: usize) -> &'static str {
        let pick = |labels: &[&'static str]| labels.get(index).copied().unwrap_or("");
        match self {
            Category::Face => HeadShape::ALL.get(index).map_or("", |v| v.label()),
            Category::Hair => HairStyle::ALL.get(index).map_or("", |v| v.label()),
            Category::Eyes => EyeStyle::ALL.get(index).map_or("", |v| v.label()),
            Category::Brows => BrowStyle::ALL.get(index).map_or("", |v| v.label()),
            Category::Nose => NoseStyle::ALL.get(index).map_or("", |v| v.label()),
            Category::Mouth => MouthStyle::ALL.get(index).map_or("", |v| v.label()),
            Category::Beard => FacialHair::ALL.get(index).map_or("", |v| v.label()),
            Category::Glasses => Glasses::ALL.get(index).map_or("", |v| v.label()),
            Category::Style => Outfit::ALL.get(index).map_or("", |v| v.label()),
            Category::Mood => pick(&MOODS),
        }
    }

    /// Which option the avatar wears now (`mood` is the app's preset index).
    pub fn current(self, avatar: &Avatar, mood: usize) -> usize {
        fn at<T: PartialEq>(all: &[T], value: &T) -> usize {
            all.iter().position(|v| v == value).unwrap_or(0)
        }
        match self {
            Category::Face => at(HeadShape::ALL, &avatar.head),
            Category::Hair => at(HairStyle::ALL, &avatar.hair),
            Category::Eyes => at(EyeStyle::ALL, &avatar.eyes),
            Category::Brows => at(BrowStyle::ALL, &avatar.brows),
            Category::Nose => at(NoseStyle::ALL, &avatar.nose),
            Category::Mouth => at(MouthStyle::ALL, &avatar.mouth),
            Category::Beard => at(FacialHair::ALL, &avatar.facial_hair),
            Category::Glasses => at(Glasses::ALL, &avatar.glasses),
            Category::Style => at(Outfit::ALL, &avatar.outfit),
            Category::Mood => mood,
        }
    }

    /// The command that puts option `index` on the avatar.
    pub fn command(self, index: usize) -> Option<Command> {
        Some(match self {
            Category::Face => Command::SetHead(*HeadShape::ALL.get(index)?),
            Category::Hair => Command::SetHair(*HairStyle::ALL.get(index)?),
            Category::Eyes => Command::SetEyes(*EyeStyle::ALL.get(index)?),
            Category::Brows => Command::SetBrows(*BrowStyle::ALL.get(index)?),
            Category::Nose => Command::SetNose(*NoseStyle::ALL.get(index)?),
            Category::Mouth => Command::SetMouth(*MouthStyle::ALL.get(index)?),
            Category::Beard => Command::SetFacialHair(*FacialHair::ALL.get(index)?),
            Category::Glasses => Command::SetGlasses(*Glasses::ALL.get(index)?),
            Category::Style => Command::SetOutfit(*Outfit::ALL.get(index)?),
            Category::Mood => Command::SetExpression(Expression::PRESETS.get(index)?.1),
        })
    }
}

/// Positions in logical pixels, scaled from the 1237x751 Figma frame.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub bounds: Rect,
    pub preview: Circle,
    pub glow_offset: i32,
    pub glow_blur: i32,
    pub panel: Rect,
    pub panel_radius: i32,
    pub pill: Rect,
    pub pill_radius: i32,
    pub tab_width: i32,
    pub tab_pad: i32,
    /// The visible part of the option grid (two rows of tiles).
    pub grid: Rect,
    pub tile_w: i32,
    pub tile_h: i32,
    pub col_gap: i32,
    pub row_gap: i32,
    pub tile_radius: i32,
}

impl Layout {
    pub fn compute(bounds: Rect) -> Layout {
        let sx = |v: i32| (v as i64 * bounds.w as i64 / 1237) as i32;
        let sy = |v: i32| (v as i64 * bounds.h as i64 / 751) as i32;
        let s = |v: i32| sx(v).min(sy(v));
        let at = |x: i32, y: i32, w: i32, h: i32| {
            Rect::new(bounds.x + sx(x), bounds.y + sy(y), sx(w), sy(h))
        };
        let pill = at(747, 67, 436, 79);
        let grid = at(743, 249, 439, 441);
        Layout {
            bounds,
            preview: Circle {
                cx: bounds.x + sx(749) / 2,
                cy: bounds.y + sy(737) / 2,
                r: s(541) / 2,
            },
            glow_offset: s(22),
            glow_blur: s(62),
            panel: at(704, 26, 507, 699),
            panel_radius: s(65),
            pill,
            pill_radius: s(53).min(pill.h / 2),
            tab_width: pill.h * 3 / 2,
            tab_pad: pill.h / 5,
            grid,
            tile_w: sx(211),
            tile_h: sy(211),
            col_gap: sx(17),
            row_gap: sy(19),
            tile_radius: s(53),
        }
    }

    pub fn tab(&self, index: usize, scroll: i32) -> Rect {
        let inset = (self.pill.h / 8).max(2);
        Rect::new(
            self.pill.x + self.tab_pad + index as i32 * self.tab_width - scroll,
            self.pill.y + inset,
            self.tab_width,
            self.pill.h - inset * 2,
        )
    }

    pub fn tile(&self, index: usize, scroll: i32) -> Rect {
        let (col, row) = ((index % 2) as i32, (index / 2) as i32);
        Rect::new(
            self.grid.x + col * (self.tile_w + self.col_gap),
            self.grid.y + row * (self.tile_h + self.row_gap) - scroll,
            self.tile_w,
            self.tile_h,
        )
    }

    pub fn max_tab_scroll(&self) -> i32 {
        let content = self.tab_pad * 2 + Category::ALL.len() as i32 * self.tab_width;
        (content - self.pill.w).max(0)
    }

    pub fn max_grid_scroll(&self, category: Category) -> i32 {
        let rows = category.option_count().div_ceil(2) as i32;
        let content = rows * (self.tile_h + self.row_gap) - self.row_gap;
        (content - self.grid.h).max(0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    None,
    Preview,
    Tab(usize),
    Option(usize),
}

#[derive(Clone, Copy, Debug)]
pub enum Event {
    Move {
        x: i32,
        y: i32,
    },
    Press {
        x: i32,
        y: i32,
    },
    Release {
        x: i32,
        y: i32,
    },
    /// Wheel notches, positive = up (or left over the tabs).
    Wheel {
        x: i32,
        y: i32,
        delta: i32,
    },
}

/// What a completed click means; the app turns these into `Command`s.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    SelectCategory(Category),
    Choose(Category, usize),
    /// Tapping the avatar rolls a new random one.
    Randomize,
}

pub struct Ui {
    pub layout: Layout,
    pub hover: Hit,
    pub pressed: Hit,
    pub category: Category,
    pub tab_scroll: i32,
    pub grid_scroll: i32,
}

impl Ui {
    pub fn new(bounds: Rect) -> Ui {
        Ui {
            layout: Layout::compute(bounds),
            hover: Hit::None,
            pressed: Hit::None,
            category: Category::Face,
            tab_scroll: 0,
            grid_scroll: 0,
        }
    }

    pub fn resize(&mut self, bounds: Rect) {
        self.layout = Layout::compute(bounds);
        self.clamp_scroll();
    }

    pub fn clamp_scroll(&mut self) {
        self.tab_scroll = self.tab_scroll.clamp(0, self.layout.max_tab_scroll());
        self.grid_scroll = self
            .grid_scroll
            .clamp(0, self.layout.max_grid_scroll(self.category));
    }

    pub fn hit_test(&self, x: i32, y: i32) -> Hit {
        let l = &self.layout;
        if l.preview.contains(x, y) {
            return Hit::Preview;
        }
        if l.pill.contains(x, y) {
            for index in 0..Category::ALL.len() {
                if l.tab(index, self.tab_scroll).contains(x, y) {
                    return Hit::Tab(index);
                }
            }
            return Hit::None;
        }
        if l.grid.contains(x, y) {
            for index in 0..self.category.option_count() {
                if l.tile(index, self.grid_scroll).contains(x, y) {
                    return Hit::Option(index);
                }
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
                    Hit::Tab(index) => {
                        let category = Category::ALL[index];
                        if category != self.category {
                            self.category = category;
                            self.grid_scroll = 0;
                        }
                        Some(Action::SelectCategory(category))
                    }
                    Hit::Option(index) => Some(Action::Choose(self.category, index)),
                    Hit::Preview => Some(Action::Randomize),
                    Hit::None => None,
                }
            }
            Event::Wheel { x, y, delta } => {
                let l = self.layout;
                if l.pill.contains(x, y) {
                    self.tab_scroll -= delta * l.tab_width / 2;
                } else if l.panel.contains(x, y) {
                    self.grid_scroll -= delta * (l.tile_h + l.row_gap) / 2;
                }
                self.clamp_scroll();
                None
            }
        }
    }
}

pub mod theme {
    use crate::model::Rgb;
    pub const DESKTOP: Rgb = Rgb(45, 45, 45);
    pub const WINDOW: Rgb = Rgb(6, 6, 6);
    pub const PREVIEW: Rgb = Rgb(217, 217, 217);
    pub const STROKE: Rgb = Rgb(0, 0, 0);
    pub const PANEL: Rgb = Rgb(22, 22, 22);
    pub const ITEM: Rgb = Rgb(39, 39, 39);
    pub const ITEM_HOVER: Rgb = Rgb(50, 50, 50);
    pub const ITEM_PRESSED: Rgb = Rgb(30, 30, 30);
    pub const ITEM_SELECTED: Rgb = Rgb(70, 70, 70);
    pub const TEXT: Rgb = Rgb(235, 235, 235);
    pub const TEXT_DIM: Rgb = Rgb(150, 150, 150);
}

/// Drawing backend. `avatar` must render the live avatar clipped to the circle.
pub trait Painter {
    fn round_rect(&mut self, r: Rect, radius: i32, color: Rgb);
    fn circle(&mut self, c: Circle, color: Rgb);
    fn avatar(&mut self, c: Circle);
    fn label(&mut self, r: Rect, text: &str, color: Rgb);
    /// A white glow under the circle, `offset` down and `blur` wide.
    fn glow(&mut self, _c: Circle, _offset: i32, _blur: i32) {}
    /// Restricts later drawing to `r` (`None` lifts it).
    fn clip(&mut self, _r: Option<Rect>) {}
}

/// `current` is the option the avatar wears in `ui.category`.
pub fn paint(ui: &Ui, current: usize, p: &mut impl Painter) {
    use theme::*;
    let l = &ui.layout;
    p.round_rect(l.bounds, 0, WINDOW);
    p.glow(l.preview, l.glow_offset, l.glow_blur);
    p.circle(l.preview, STROKE);
    let inner = Circle {
        r: l.preview.r - 1,
        ..l.preview
    };
    p.circle(inner, PREVIEW);
    p.avatar(inner);
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

    p.round_rect(l.pill, l.pill_radius, ITEM);
    p.clip(Some(l.pill));
    for (index, category) in Category::ALL.iter().enumerate() {
        let tab = l.tab(index, ui.tab_scroll);
        if !tab.overlaps(&l.pill) {
            continue;
        }
        let chosen = *category == ui.category;
        if chosen || ui.hover == Hit::Tab(index) {
            let fill = if chosen { ITEM_SELECTED } else { ITEM_HOVER };
            p.round_rect(tab, tab.h / 2, fill);
        }
        p.label(tab, category.label(), if chosen { TEXT } else { TEXT_DIM });
    }
    p.clip(None);

    p.clip(Some(l.grid));
    for index in 0..ui.category.option_count() {
        let tile = l.tile(index, ui.grid_scroll);
        if !tile.overlaps(&l.grid) {
            continue;
        }
        let hit = Hit::Option(index);
        p.round_rect(tile, l.tile_radius, state(hit, index == current));
        p.label(tile, ui.category.option_label(index), TEXT);
    }
    p.clip(None);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click(ui: &mut Ui, x: i32, y: i32) -> Option<Action> {
        ui.handle(Event::Press { x, y });
        ui.handle(Event::Release { x, y })
    }

    #[test]
    fn click_option_tab_and_preview() {
        let mut ui = Ui::new(Rect::new(0, 0, 836, 507));
        let t = ui.layout.tile(1, 0);
        assert_eq!(
            click(&mut ui, t.x + t.w / 2, t.y + t.h / 2),
            Some(Action::Choose(Category::Face, 1))
        );
        let tab = ui.layout.tab(1, 0);
        assert_eq!(
            click(&mut ui, tab.x + tab.w / 2, tab.y + tab.h / 2),
            Some(Action::SelectCategory(Category::Hair))
        );
        assert_eq!(ui.category, Category::Hair);
        let c = ui.layout.preview;
        assert_eq!(click(&mut ui, c.cx, c.cy), Some(Action::Randomize));
    }

    #[test]
    fn release_elsewhere_cancels() {
        let mut ui = Ui::new(Rect::new(0, 0, 836, 507));
        let t = ui.layout.tile(0, 0);
        ui.handle(Event::Press {
            x: t.x + 5,
            y: t.y + 5,
        });
        assert_eq!(ui.handle(Event::Release { x: 0, y: 0 }), None);
    }

    #[test]
    fn wheel_scrolls_grid_and_tabs() {
        let mut ui = Ui::new(Rect::new(0, 0, 836, 507));
        ui.category = Category::Hair;
        let g = ui.layout.grid;
        ui.handle(Event::Wheel {
            x: g.x + 5,
            y: g.y + 5,
            delta: -10,
        });
        assert_eq!(ui.grid_scroll, ui.layout.max_grid_scroll(Category::Hair));
        assert!(ui.grid_scroll > 0);
        let last = ui.layout.tile(5, ui.grid_scroll);
        assert_eq!(ui.hit_test(last.x + 3, last.y + last.h / 2), Hit::Option(5));
        let p = ui.layout.pill;
        ui.handle(Event::Wheel {
            x: p.x + 5,
            y: p.y + 5,
            delta: -20,
        });
        assert_eq!(ui.tab_scroll, ui.layout.max_tab_scroll());
        assert!(ui.tab_scroll > 0);
    }

    #[test]
    fn every_option_has_a_command() {
        for category in Category::ALL {
            for index in 0..category.option_count() {
                assert!(category.command(index).is_some());
                assert!(!category.option_label(index).is_empty());
            }
        }
    }
}
