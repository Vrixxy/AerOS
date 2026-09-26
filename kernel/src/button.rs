use crate::aerui::{
    ButtonInteraction, CornerRadii, Event, FrostReport, FrostStyle, Insets, InteractionResponse,
    Painter, Point, PointerButton, Rect, Rgba, Scale, Size,
};
use crate::font::RasterFont;

use core::sync::atomic::{AtomicUsize, Ordering};

const PRESS_NS: u64 = 80_000_000;
/// Colour-work block size for button glass; the desktop raises it while
/// things are moving so animation frames stay fast.
static PAINT_STEP: AtomicUsize = AtomicUsize::new(1);

pub fn set_paint_step(step: usize) {
    PAINT_STEP.store(step.max(1), Ordering::Relaxed);
}

pub const RELEASE_NS: u64 = 150_000_000;

#[derive(Clone, Copy)]
pub struct ButtonStyle {
    pub minimum_size: Size,
    pub padding: Insets,
    pub gap: i32,
    pub radii: CornerRadii,
    pub surface: FrostStyle,
    pub font: Option<RasterFont>,
    pub font_size: i32,
    pub label_color: Rgba,
}

#[derive(Clone, Copy)]
pub struct ButtonStyles {
    pub resting: ButtonStyle,
    pub hovered: ButtonStyle,
    pub pressed: ButtonStyle,
    pub focused: ButtonStyle,
    pub selected: ButtonStyle,
    pub disabled: ButtonStyle,
}

impl ButtonStyles {
    pub fn figma_glass() -> Self {
        let base = FrostStyle {
            blur_radius: 20,
            saturation_percent: 120,
            brightness_percent: 108,
            tint: Rgba::new(196, 196, 196, 51),
            border: Rgba::new(0, 0, 0, 255),
            border_width: 1,
            inner_highlight: Rgba::new(255, 255, 255, 36),
            inner_shadow: Rgba::new(0, 0, 0, 64),
            inner_shadow_offset: Point::new(9, 9),
            inner_shadow_softness: 16,
            shadow: Rgba::transparent(),
            shadow_offset: Point::new(0, 0),
            shadow_spread: 0,
            shadow_softness: 0,
            noise_alpha: 8,
        };
        let layout = ButtonStyle {
            minimum_size: Size::new(50, 50),
            padding: Insets::all(10),
            gap: 8,
            radii: CornerRadii::all(17),
            surface: base,
            font: None,
            font_size: 14,
            label_color: Rgba::new(12, 19, 23, 255),
        };
        Self {
            resting: layout,
            hovered: ButtonStyle {
                surface: FrostStyle {
                    brightness_percent: 115,
                    tint: Rgba::new(220, 220, 220, 70),
                    border: Rgba::new(60, 60, 60, 255),
                    ..base
                },
                ..layout
            },
            pressed: ButtonStyle {
                surface: FrostStyle {
                    brightness_percent: 95,
                    tint: Rgba::new(160, 160, 160, 90),
                    inner_shadow: Rgba::new(0, 0, 0, 100),
                    ..base
                },
                ..layout
            },
            focused: ButtonStyle {
                surface: FrostStyle {
                    border: Rgba::new(241, 248, 255, 220),
                    brightness_percent: 112,
                    ..base
                },
                ..layout
            },
            selected: ButtonStyle {
                surface: FrostStyle {
                    tint: Rgba::new(100, 160, 255, 80),
                    border: Rgba::new(80, 140, 255, 220),
                    brightness_percent: 110,
                    ..base
                },
                ..layout
            },
            disabled: ButtonStyle {
                surface: FrostStyle {
                    blur_radius: 12,
                    saturation_percent: 60,
                    brightness_percent: 80,
                    tint: Rgba::new(196, 196, 196, 30),
                    border: Rgba::new(0, 0, 0, 120),
                    inner_highlight: Rgba::transparent(),
                    inner_shadow: Rgba::transparent(),
                    noise_alpha: 4,
                    ..base
                },
                label_color: Rgba::new(98, 105, 110, 255),
                ..layout
            },
        }
    }

    pub fn with_font(mut self, font: RasterFont) -> Self {
        self.resting.font = Some(font);
        self.hovered.font = Some(font);
        self.pressed.font = Some(font);
        self.focused.font = Some(font);
        self.selected.font = Some(font);
        self.disabled.font = Some(font);
        self
    }

    pub fn with_font_size(mut self, size: i32) -> Self {
        self.resting.font_size = size;
        self.hovered.font_size = size;
        self.pressed.font_size = size;
        self.focused.font_size = size;
        self.selected.font_size = size;
        self.disabled.font_size = size;
        self
    }
}

pub struct Button<'a> {
    pub bounds: Rect,
    pub label: &'a str,
    pub semantic_label: &'a str,
    pub styles: &'a ButtonStyles,
    pub interaction: ButtonInteraction,
    pub selected: bool,
    press_at_ns: u64,
    release_at_ns: u64,
}

impl<'a> Button<'a> {
    pub const fn new(
        bounds: Rect,
        label: &'a str,
        semantic_label: &'a str,
        styles: &'a ButtonStyles,
    ) -> Self {
        Self {
            bounds,
            label,
            semantic_label,
            styles,
            interaction: ButtonInteraction::new(),
            selected: false,
            press_at_ns: 0,
            release_at_ns: 0,
        }
    }

    pub fn with_transition(mut self, press_at_ns: u64, release_at_ns: u64) -> Self {
        self.press_at_ns = press_at_ns;
        self.release_at_ns = release_at_ns;
        self
    }

    pub fn measure(&self, scale: Scale) -> Size {
        let style = self.current_style();
        let mut logical = style.minimum_size;
        if let Some(font) = style.font
            && !self.label.is_empty()
        {
            logical.width = logical.width.max(
                font.text_width(self.label, style.font_size)
                    .saturating_add(style.padding.left)
                    .saturating_add(style.padding.right),
            );
            logical.height = logical.height.max(
                style
                    .font_size
                    .saturating_add(style.padding.top)
                    .saturating_add(style.padding.bottom),
            );
        }
        scale.size(logical)
    }

    pub fn handle(&mut self, event: Event, now_ns: u64) -> InteractionResponse {
        let was_pressed = self.interaction.pressed;
        let response = self.interaction.handle(self.bounds, event);
        self.record_transition(was_pressed, now_ns);
        response
    }

    pub fn handle_scaled(
        &mut self,
        event: Event,
        now_ns: u64,
        scale: Scale,
        offset: Point,
    ) -> InteractionResponse {
        let was_pressed = self.interaction.pressed;
        let response = self
            .interaction
            .handle(self.physical_bounds(scale, offset), event);
        self.record_transition(was_pressed, now_ns);
        response
    }

    pub fn paint(&self, painter: &mut Painter<'_>, scale: Scale, now_ns: u64) -> FrostReport {
        self.paint_translated(painter, scale, Point::new(0, 0), now_ns)
    }

    pub fn paint_translated(
        &self,
        painter: &mut Painter<'_>,
        scale: Scale,
        offset: Point,
        now_ns: u64,
    ) -> FrostReport {
        let style = self.current_style();
        let bounds = self.physical_bounds(scale, offset);
        let radii = scale.radii(style.radii).fit(bounds);
        let noise_seed = (self.bounds.x as u32).wrapping_mul(0x9e37_79b9)
            ^ (self.bounds.y as u32).wrapping_mul(0x517c_c1b7);
        let surface = self.animated_surface(style, now_ns).scaled(scale);
        let step = PAINT_STEP.load(Ordering::Relaxed);
        let report = if step >= 4 {
            // Mid-animation: flat glass tint, no blur.
            painter.fill_rounded_rect(bounds, radii, surface.tint);
            if surface.border_width > 0 {
                painter.stroke_rounded_rect(bounds, radii, surface.border_width, surface.border);
            }
            FrostReport {
                pixels: 0,
                blur_radius: 0,
                captured: true,
                clipped: false,
            }
        } else {
            painter.frosted_rounded_rect_stepped(bounds, radii, surface, noise_seed, step)
        };
        if self.interaction.focused && !self.interaction.disabled {
            self.paint_focus_ring(painter, scale, bounds, radii);
        }
        self.paint_label(painter, scale, style, bounds);
        report
    }

    pub fn physical_bounds(&self, scale: Scale, offset: Point) -> Rect {
        let mut bounds = scale.rect(self.bounds);
        bounds.x = bounds.x.saturating_add(offset.x);
        bounds.y = bounds.y.saturating_add(offset.y);
        bounds
    }

    fn current_style(&self) -> &ButtonStyle {
        if self.interaction.disabled {
            &self.styles.disabled
        } else if self.interaction.pressed {
            &self.styles.pressed
        } else if self.selected {
            &self.styles.selected
        } else if self.interaction.hovered {
            &self.styles.hovered
        } else if self.interaction.focused {
            &self.styles.focused
        } else {
            &self.styles.resting
        }
    }

    fn record_transition(&mut self, was_pressed: bool, now_ns: u64) {
        if !was_pressed && self.interaction.pressed {
            self.press_at_ns = now_ns;
            self.release_at_ns = 0;
        } else if was_pressed && !self.interaction.pressed {
            self.release_at_ns = now_ns;
        }
    }

    fn animated_surface(&self, style: &ButtonStyle, now_ns: u64) -> FrostStyle {
        let mut surface = style.surface;
        if self.press_at_ns > 0 && self.interaction.pressed {
            let elapsed = now_ns.saturating_sub(self.press_at_ns);
            if elapsed < PRESS_NS {
                let t = (elapsed.saturating_mul(255) / PRESS_NS) as u8;
                surface.tint = lerp_rgba(
                    self.styles.resting.surface.tint,
                    self.styles.pressed.surface.tint,
                    t,
                );
                surface.brightness_percent = lerp_u16(
                    self.styles.resting.surface.brightness_percent,
                    self.styles.pressed.surface.brightness_percent,
                    t,
                );
            }
        }
        if self.release_at_ns > 0 && !self.interaction.pressed {
            let elapsed = now_ns.saturating_sub(self.release_at_ns);
            if elapsed < RELEASE_NS {
                let t = (elapsed.saturating_mul(255) / RELEASE_NS) as u8;
                surface.tint = lerp_rgba(self.styles.pressed.surface.tint, style.surface.tint, t);
                surface.brightness_percent = lerp_u16(
                    self.styles.pressed.surface.brightness_percent,
                    style.surface.brightness_percent,
                    t,
                );
            }
        }
        surface
    }

    fn paint_focus_ring(
        &self,
        painter: &mut Painter<'_>,
        scale: Scale,
        bounds: Rect,
        radii: CornerRadii,
    ) {
        let gap = scale.logical(2);
        let ring = bounds.expand(gap);
        let ring_radii = CornerRadii::new(
            radii.top_left + gap,
            radii.top_right + gap,
            radii.bottom_right + gap,
            radii.bottom_left + gap,
        );
        painter.stroke_rounded_rect(ring, ring_radii, 1, Rgba::new(247, 252, 255, 230));
    }

    fn paint_label(
        &self,
        painter: &mut Painter<'_>,
        scale: Scale,
        style: &ButtonStyle,
        bounds: Rect,
    ) {
        let Some(font) = style.font else {
            return;
        };
        if self.label.is_empty() {
            return;
        }
        let height = scale.logical(style.font_size);
        if height <= 0 {
            return;
        }
        let padding = Insets {
            top: scale.logical(style.padding.top),
            right: scale.logical(style.padding.right),
            bottom: scale.logical(style.padding.bottom),
            left: scale.logical(style.padding.left),
        };
        let Some(content) = bounds.inset(padding) else {
            return;
        };
        let label_width = font.text_width(self.label, height);
        let x = content.x + content.width.saturating_sub(label_width) / 2;
        let y = content.y + content.height.saturating_sub(height) / 2;
        painter.text(
            font,
            Point::new(x, y),
            self.label,
            height,
            style.label_color.color(),
        );
    }
}

#[derive(Clone, Copy)]
pub struct ButtonReport {
    pub pointer: bool,
    pub keyboard: bool,
    pub disabled: bool,
    pub scaled: bool,
    pub verified: bool,
}

pub fn self_test() -> ButtonReport {
    let styles = ButtonStyles::figma_glass();
    let mut button = Button::new(Rect::new(10, 20, 50, 50), "", "test", &styles);
    let pressed = button.handle(
        Event::PointerPressed {
            position: Point::new(20, 30),
            button: PointerButton::Primary,
        },
        10,
    );
    let released = button.handle(
        Event::PointerReleased {
            position: Point::new(20, 30),
            button: PointerButton::Primary,
        },
        PRESS_NS + 10,
    );
    button.handle(Event::FocusChanged(true), PRESS_NS + 20);
    button.handle(Event::KeyPressed(crate::aerui::Key::Enter), PRESS_NS + 30);
    let key = button.handle(Event::KeyReleased(crate::aerui::Key::Enter), PRESS_NS + 40);
    button.interaction.set_disabled(true);
    let denied = button.handle(
        Event::PointerPressed {
            position: Point::new(20, 30),
            button: PointerButton::Primary,
        },
        PRESS_NS + 50,
    );
    let scale = Scale::from_milli(1_500).unwrap_or(Scale::ONE);
    let physical = button.physical_bounds(scale, Point::new(3, 4));
    let pointer = pressed.capture_pointer && released.release_pointer && released.activated;
    let keyboard = key.activated;
    let disabled = !denied.activated && !button.interaction.pressed;
    let scaled =
        physical == Rect::new(18, 34, 75, 75) && button.measure(scale) == Size::new(75, 75);
    ButtonReport {
        pointer,
        keyboard,
        disabled,
        scaled,
        verified: pointer && keyboard && disabled && scaled,
    }
}

fn lerp_rgba(a: Rgba, b: Rgba, t: u8) -> Rgba {
    Rgba::new(
        lerp_u8(a.red, b.red, t),
        lerp_u8(a.green, b.green, t),
        lerp_u8(a.blue, b.blue, t),
        lerp_u8(a.alpha, b.alpha, t),
    )
}

fn lerp_u8(a: u8, b: u8, t: u8) -> u8 {
    let inverse = 255u16.saturating_sub(t as u16);
    ((a as u16 * inverse + b as u16 * t as u16 + 127) / 255) as u8
}

fn lerp_u16(a: u16, b: u16, t: u8) -> u16 {
    let inverse = 255u32.saturating_sub(t as u32);
    ((a as u32 * inverse + b as u32 * t as u32 + 127) / 255) as u16
}
