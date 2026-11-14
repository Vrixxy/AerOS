//! The Avatar app: the `avatar-core` engine inside a desktop window.
//! Layout, hit-testing and painting come from `avatar_core::ui`; this file
//! only adapts them to `aerui`'s painter and the desktop's click path.

use super::*;
use crate::sync::TicketLock;
use alloc::vec::Vec;
use avatar_core::gfx::Surface;
use avatar_core::model::{HairStyle, HeadShape, Outfit};
use avatar_core::ui as avui;
use avatar_core::{AppState, Command, Expression};

struct Frame {
    px: Vec<u8>,
    size: usize,
    version: u32,
}

struct AvatarApp {
    app: AppState,
    mood: usize,
    selected: Option<avui::Category>,
    version: u32,
    frame: Option<Frame>,
}

static AVATAR: TicketLock<Option<AvatarApp>> = TicketLock::new(None);

fn next<T: Copy + PartialEq>(all: &[T], current: T) -> T {
    let at = all.iter().position(|v| *v == current).unwrap_or(0);
    all[(at + 1) % all.len()]
}

impl AvatarApp {
    fn new() -> Self {
        Self {
            app: AppState::new(),
            mood: 0,
            selected: None,
            version: 1,
            frame: None,
        }
    }

    fn send(&mut self, command: Command) {
        let _ = self.app.apply(command);
        self.version = self.version.wrapping_add(1);
    }

    fn act(&mut self, action: avui::Action) {
        match action {
            avui::Action::OpenCategory(category) => {
                self.selected = Some(category);
                let avatar = &self.app.avatar;
                match category {
                    avui::Category::Face => {
                        let head = next(HeadShape::ALL, avatar.head);
                        self.send(Command::SetHead(head));
                    }
                    avui::Category::Hair => {
                        let hair = next(HairStyle::ALL, avatar.hair);
                        self.send(Command::SetHair(hair));
                    }
                    avui::Category::Style => {
                        let outfit = next(Outfit::ALL, avatar.outfit);
                        self.send(Command::SetOutfit(outfit));
                    }
                    avui::Category::Mood => {
                        self.mood = (self.mood + 1) % Expression::PRESETS.len();
                        let expression = Expression::PRESETS[self.mood].1;
                        self.send(Command::SetExpression(expression));
                    }
                }
            }
            avui::Action::Randomize => {
                let seed = crate::time::monotonic_nanoseconds();
                self.send(Command::Randomize(seed));
            }
            avui::Action::EditName => {}
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
}

fn rect(r: avui::Rect) -> Rect {
    Rect::new(r.x, r.y, r.w, r.h)
}

fn rgba(c: avatar_core::model::Rgb) -> Rgba {
    Rgba::opaque(c.0, c.1, c.2)
}

impl avui::Painter for Adapter<'_, '_> {
    fn round_rect(&mut self, r: avui::Rect, radius: i32, color: avatar_core::model::Rgb) {
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
}

pub(super) fn draw_avatar(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    outer: Rect,
) {
    let mut view = avui::Ui::new(content_bounds(outer));
    with_app(|app| {
        view.selected = app.selected;
        let preview = view.layout.preview;
        let physical = layout
            .rect(Rect::new(
                preview.cx - preview.r,
                preview.cy - preview.r,
                preview.r * 2,
                preview.r * 2,
            ))
            .width as usize;
        let name = app.app.avatar.name.clone();
        let _ = app.frame(physical);
        let frame = app.frame.as_ref().unwrap();
        let mut adapter = Adapter {
            painter,
            layout,
            font,
            frame,
        };
        avui::paint(&view, &name, &mut adapter);
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
        let mut view = avui::Ui::new(content_bounds(window_base_rect(DesktopApp::Avatar)));
        view.handle(avui::Event::Press {
            x: point.x,
            y: point.y,
        });
        let action = view.handle(avui::Event::Release {
            x: point.x,
            y: point.y,
        })?;
        with_app(|app| app.act(action));
        Some(DesktopAction::Redraw)
    }
}
