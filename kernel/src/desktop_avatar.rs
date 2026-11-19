//! The Avatar app: the `avatar-core` engine inside a desktop window.
//! Layout, hit-testing and painting come from `avatar_core::ui`; this file
//! only adapts them to `aerui`'s painter and the desktop's input path.

use super::*;
use crate::sync::TicketLock;
use alloc::vec::Vec;
use avatar_core::gfx::Surface;
use avatar_core::ui as avui;
use avatar_core::{AppState, Command};

struct Frame {
    px: Vec<u8>,
    size: usize,
    version: u32,
}

struct AvatarApp {
    app: AppState,
    mood: usize,
    category: avui::Category,
    tab_scroll: i32,
    grid_scroll: i32,
    version: u32,
    frame: Option<Frame>,
}

static AVATAR: TicketLock<Option<AvatarApp>> = TicketLock::new(None);

impl AvatarApp {
    fn new() -> Self {
        Self {
            app: AppState::new(),
            mood: 0,
            category: avui::Category::Face,
            tab_scroll: 0,
            grid_scroll: 0,
            version: 1,
            frame: None,
        }
    }

    /// The UI with this app's category and scroll positions.
    fn view(&self, bounds: avui::Rect) -> avui::Ui {
        let mut view = avui::Ui::new(bounds);
        view.category = self.category;
        view.tab_scroll = self.tab_scroll;
        view.grid_scroll = self.grid_scroll;
        view.clamp_scroll();
        view
    }

    fn keep(&mut self, view: &avui::Ui) {
        self.category = view.category;
        self.tab_scroll = view.tab_scroll;
        self.grid_scroll = view.grid_scroll;
    }

    fn send(&mut self, command: Command) {
        let _ = self.app.apply(command);
        self.version = self.version.wrapping_add(1);
    }

    fn act(&mut self, action: avui::Action) {
        match action {
            avui::Action::SelectCategory(_) => {}
            avui::Action::Choose(category, index) => {
                if category == avui::Category::Mood {
                    self.mood = index;
                }
                if let Some(command) = category.command(index) {
                    self.send(command);
                }
            }
            avui::Action::Randomize => {
                let seed = crate::time::monotonic_nanoseconds();
                self.send(Command::Randomize(seed));
            }
        }
    }

    /// The avatar drawn at `size` pixels and clipped to a circle; rendered
    /// again only when the avatar or the size changed.
    fn frame(&mut self, size: usize) -> &Frame {
        let size = size.clamp(64, 720);
        let stale = match &self.frame {
            Some(frame) => frame.size != size || frame.version != self.version,
            None => true,
        };
        if stale {
            let drawn = avatar_core::render::render(
                &self.app.avatar,
                &self.app.expression,
                &self.app.pose,
                size as u32,
                None,
            );
            let mut clipped = Surface::new(size, size);
            let half = size as f32 / 2.0;
            clipped.draw_surface(&drawn, 0, 0, Some((half, half, half)));
            self.frame = Some(Frame {
                px: clipped.px,
                size,
                version: self.version,
            });
        }
        self.frame.as_ref().unwrap()
    }
}

fn with_app<R>(f: impl FnOnce(&mut AvatarApp) -> R) -> R {
    let mut guard = AVATAR.lock();
    f(guard.get_or_insert_with(AvatarApp::new))
}

fn content_bounds(outer: Rect) -> avui::Rect {
    avui::Rect::new(
        outer.x + 10,
        outer.y + 34,
        outer.width - 20,
        outer.height - 44,
    )
}

struct Adapter<'p, 'f> {
    painter: &'p mut Painter<'f>,
    layout: Layout,
    font: RasterFont,
    frame: &'p Frame,
    /// The clip before `clip(Some(..))`, and whether that clip is empty.
    saved_clip: Option<Rect>,
    hidden: bool,
}

fn rect(r: avui::Rect) -> Rect {
    Rect::new(r.x, r.y, r.w, r.h)
}

fn rgba(c: avatar_core::model::Rgb) -> Rgba {
    Rgba::opaque(c.0, c.1, c.2)
}

fn isqrt(value: u64) -> u64 {
    let mut result = 0u64;
    let mut bit = 1u64 << 62;
    let mut rest = value;
    while bit > rest {
        bit >>= 2;
    }
    while bit != 0 {
        if rest >= result + bit {
            rest -= result + bit;
            result = (result >> 1) + bit;
        } else {
            result >>= 1;
        }
        bit >>= 2;
    }
    result
}

impl avui::Painter for Adapter<'_, '_> {
    fn round_rect(&mut self, r: avui::Rect, radius: i32, color: avatar_core::model::Rgb) {
        if self.hidden {
            return;
        }
        self.painter.fill_rounded_rect(
            self.layout.rect(rect(r)),
            self.layout.radii(CornerRadii::all(radius)),
            rgba(color),
        );
    }

    fn circle(&mut self, c: avui::Circle, color: avatar_core::model::Rgb) {
        let bounds = Rect::new(c.cx - c.r, c.cy - c.r, c.r * 2, c.r * 2);
        self.painter.fill_rounded_rect(
            self.layout.rect(bounds),
            self.layout.radii(CornerRadii::all(c.r)),
            rgba(color),
        );
    }

    fn avatar(&mut self, c: avui::Circle) {
        let bounds = Rect::new(c.cx - c.r, c.cy - c.r, c.r * 2, c.r * 2);
        self.painter.draw_rgba_scaled(
            self.layout.rect(bounds),
            &self.frame.px,
            self.frame.size,
            self.frame.size,
        );
    }

    fn label(&mut self, r: avui::Rect, text: &str, color: avatar_core::model::Rgb) {
        if self.hidden {
            return;
        }
        centered_text(
            self.painter,
            self.layout,
            self.font,
            rect(r),
            text,
            14,
            Color::rgb(color.0, color.1, color.2),
        );
    }

    /// Figma's white drop shadow (blur ~ two sigma), as a smooth falloff
    /// around the circle moved down by `offset`; pixels the circle covers
    /// are skipped.
    fn glow(&mut self, c: avui::Circle, offset: i32, blur: i32) {
        let disc = self
            .layout
            .rect(Rect::new(c.cx - c.r, c.cy - c.r, c.r * 2, c.r * 2));
        let scaled = self
            .layout
            .rect(Rect::new(0, 0, offset.max(1), blur.max(2)));
        let radius = disc.width as i64 / 2;
        let (cx, cy) = (disc.x as i64 + radius, disc.y as i64 + radius);
        let shifted = cy + scaled.width as i64;
        let reach = (scaled.height as i64 * 3 / 4).max(2);
        let outer = radius + reach;
        let white = Color::rgb(255, 255, 255);
        for y in shifted - outer..=shifted + outer {
            for x in cx - outer..=cx + outer {
                let (dx, dy0, dy) = (x - cx, y - cy, y - shifted);
                if dx * dx + dy0 * dy0 < radius * radius {
                    continue;
                }
                let distance = isqrt((dx * dx + dy * dy) as u64) as i64 - radius;
                if distance >= reach {
                    continue;
                }
                let u = ((distance + reach).max(0) * 1024 / (2 * reach)).min(1024);
                let smooth = (3 * u * u * 1024 - 2 * u * u * u) / (1024 * 1024);
                let alpha = 255 * (1024 - smooth) / 1024;
                if alpha > 0 {
                    self.painter
                        .blend_pixel(x as i32, y as i32, white, alpha as u8);
                }
            }
        }
    }

    fn clip(&mut self, r: Option<avui::Rect>) {
        match r {
            Some(r) => {
                let base = *self.saved_clip.get_or_insert(self.painter.clip());
                match base.intersect(self.layout.rect(rect(r))) {
                    Some(area) => {
                        self.hidden = !self.painter.set_clip(area);
                    }
                    None => self.hidden = true,
                }
            }
            None => {
                if let Some(base) = self.saved_clip.take() {
                    self.painter.set_clip(base);
                }
                self.hidden = false;
            }
        }
    }
}

pub(super) fn draw_avatar(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    outer: Rect,
) {
    with_app(|app| {
        let view = app.view(content_bounds(outer));
        let preview = view.layout.preview;
        let physical = layout
            .rect(Rect::new(
                preview.cx - preview.r,
                preview.cy - preview.r,
                preview.r * 2,
                preview.r * 2,
            ))
            .width as usize;
        let current = view.category.current(&app.app.avatar, app.mood);
        let _ = app.frame(physical);
        let frame = app.frame.as_ref().unwrap();
        let mut adapter = Adapter {
            painter,
            layout,
            font,
            frame,
            saved_clip: None,
            hidden: false,
        };
        avui::paint(&view, current, &mut adapter);
    });
}

impl DesktopState {
    pub(super) fn open_avatar(&mut self) {
        self.set_app(DesktopApp::Avatar);
        self.set_overlay(Overlay::None);
    }

    pub(super) fn avatar_click(&mut self, point: Point) -> Option<DesktopAction> {
        if self.app != DesktopApp::Avatar || self.overlay != Overlay::None {
            return None;
        }
        let bounds = content_bounds(window_base_rect(DesktopApp::Avatar));
        with_app(|app| {
            let mut view = app.view(bounds);
            view.handle(avui::Event::Press {
                x: point.x,
                y: point.y,
            });
            let action = view.handle(avui::Event::Release {
                x: point.x,
                y: point.y,
            });
            app.keep(&view);
            if let Some(action) = action {
                app.act(action);
                Some(DesktopAction::Redraw)
            } else {
                None
            }
        })
    }

    /// Scrolls the category tabs or the option grid under the pointer.
    pub(super) fn avatar_wheel(&mut self, point: Point, delta: i32) -> bool {
        if self.app != DesktopApp::Avatar || self.overlay != Overlay::None {
            return false;
        }
        let bounds = content_bounds(window_base_rect(DesktopApp::Avatar));
        with_app(|app| {
            let mut view = app.view(bounds);
            let before = (view.tab_scroll, view.grid_scroll);
            view.handle(avui::Event::Wheel {
                x: point.x,
                y: point.y,
                delta,
            });
            app.keep(&view);
            before != (view.tab_scroll, view.grid_scroll)
        })
    }
}
