use crate::font::RasterFont;
use crate::framebuffer::{Color, FrameBuffer};
use crate::sync::TicketLock;

pub const MAX_FROST_PIXELS: usize = 1_048_576;
const MAX_BLUR_RADIUS: u8 = 32;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: i32,
    pub height: i32,
}

impl Size {
    pub const fn new(width: i32, height: i32) -> Self {
        Self { width, height }
    }

    pub fn valid(self) -> bool {
        self.width >= 0 && self.height >= 0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn from_origin_size(origin: Point, size: Size) -> Self {
        Self::new(origin.x, origin.y, size.width, size.height)
    }

    pub fn size(self) -> Size {
        Size::new(self.width, self.height)
    }

    pub fn right(self) -> i32 {
        self.x.saturating_add(self.width)
    }

    pub fn bottom(self) -> i32 {
        self.y.saturating_add(self.height)
    }

    pub fn contains(self, point: Point) -> bool {
        self.width > 0
            && self.height > 0
            && point.x >= self.x
            && point.y >= self.y
            && point.x < self.right()
            && point.y < self.bottom()
    }

    pub fn intersect(self, other: Self) -> Option<Self> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > left && bottom > top).then_some(Self::new(left, top, right - left, bottom - top))
    }

    pub fn inset(self, insets: Insets) -> Option<Self> {
        let width = self
            .width
            .checked_sub(insets.left)?
            .checked_sub(insets.right)?;
        let height = self
            .height
            .checked_sub(insets.top)?
            .checked_sub(insets.bottom)?;
        (width >= 0 && height >= 0).then_some(Self::new(
            self.x.checked_add(insets.left)?,
            self.y.checked_add(insets.top)?,
            width,
            height,
        ))
    }

    pub fn expand(self, amount: i32) -> Self {
        Self::new(
            self.x.saturating_sub(amount),
            self.y.saturating_sub(amount),
            self.width.saturating_add(amount.saturating_mul(2)),
            self.height.saturating_add(amount.saturating_mul(2)),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Insets {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

impl Insets {
    pub const fn all(value: i32) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }

    pub const fn symmetric(vertical: i32, horizontal: i32) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CornerRadii {
    pub top_left: i32,
    pub top_right: i32,
    pub bottom_right: i32,
    pub bottom_left: i32,
}

impl CornerRadii {
    pub const fn all(value: i32) -> Self {
        Self {
            top_left: value,
            top_right: value,
            bottom_right: value,
            bottom_left: value,
        }
    }

    pub const fn new(top_left: i32, top_right: i32, bottom_right: i32, bottom_left: i32) -> Self {
        Self {
            top_left,
            top_right,
            bottom_right,
            bottom_left,
        }
    }

    pub fn fit(self, bounds: Rect) -> Self {
        if bounds.width <= 0 || bounds.height <= 0 {
            return Self::all(0);
        }
        let mut radii = [
            self.top_left.max(0),
            self.top_right.max(0),
            self.bottom_right.max(0),
            self.bottom_left.max(0),
        ];
        let horizontal = (radii[0] + radii[1]).max(radii[3] + radii[2]).max(1);
        let vertical = (radii[0] + radii[3]).max(radii[1] + radii[2]).max(1);
        let numerator = if horizontal > bounds.width || vertical > bounds.height {
            (bounds.width as i64 * 1_000 / horizontal as i64)
                .min(bounds.height as i64 * 1_000 / vertical as i64)
                .clamp(0, 1_000)
        } else {
            1_000
        };
        for radius in &mut radii {
            *radius = (*radius as i64 * numerator / 1_000) as i32;
        }
        Self::new(radii[0], radii[1], radii[2], radii[3])
    }

    pub fn inset(self, amount: i32) -> Self {
        Self::new(
            self.top_left.saturating_sub(amount).max(0),
            self.top_right.saturating_sub(amount).max(0),
            self.bottom_right.saturating_sub(amount).max(0),
            self.bottom_left.saturating_sub(amount).max(0),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

impl Rgba {
    pub const fn new(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    pub const fn opaque(red: u8, green: u8, blue: u8) -> Self {
        Self::new(red, green, blue, 255)
    }

    pub const fn transparent() -> Self {
        Self::new(0, 0, 0, 0)
    }

    pub fn color(self) -> Color {
        Color::rgb(self.red, self.green, self.blue)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Scale(u16);

impl Scale {
    pub const ONE: Self = Self(1_024);

    pub fn from_milli(value: u16) -> Option<Self> {
        (500..=4_000)
            .contains(&value)
            .then_some(Self(((value as u32 * 1_024 + 500) / 1_000) as u16))
    }

    pub fn logical(self, value: i32) -> i32 {
        let scaled = value as i64 * self.0 as i64;
        if scaled >= 0 {
            ((scaled + 512) / 1_024) as i32
        } else {
            ((scaled - 512) / 1_024) as i32
        }
    }

    pub fn invert(self, value: i32) -> i32 {
        let divisor = self.0.max(1) as i64;
        let scaled = value as i64 * 1_024;
        if scaled >= 0 {
            ((scaled + divisor / 2) / divisor) as i32
        } else {
            ((scaled - divisor / 2) / divisor) as i32
        }
    }

    pub fn size(self, value: Size) -> Size {
        Size::new(self.logical(value.width), self.logical(value.height))
    }

    pub fn rect(self, value: Rect) -> Rect {
        Rect::new(
            self.logical(value.x),
            self.logical(value.y),
            self.logical(value.width),
            self.logical(value.height),
        )
    }

    pub fn radii(self, value: CornerRadii) -> CornerRadii {
        CornerRadii::new(
            self.logical(value.top_left),
            self.logical(value.top_right),
            self.logical(value.bottom_right),
            self.logical(value.bottom_left),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FrostStyle {
    pub blur_radius: u8,
    pub saturation_percent: u16,
    pub brightness_percent: u16,
    pub tint: Rgba,
    pub border: Rgba,
    pub border_width: u8,
    pub inner_highlight: Rgba,
    pub inner_shadow: Rgba,
    pub inner_shadow_offset: Point,
    pub inner_shadow_softness: u8,
    pub shadow: Rgba,
    pub shadow_offset: Point,
    pub shadow_spread: u8,
    pub shadow_softness: u8,
    pub noise_alpha: u8,
}

impl FrostStyle {
    pub const CLEAR: Self = Self {
        blur_radius: 0,
        saturation_percent: 100,
        brightness_percent: 100,
        tint: Rgba::transparent(),
        border: Rgba::transparent(),
        border_width: 0,
        inner_highlight: Rgba::transparent(),
        inner_shadow: Rgba::transparent(),
        inner_shadow_offset: Point::new(0, 0),
        inner_shadow_softness: 0,
        shadow: Rgba::transparent(),
        shadow_offset: Point::new(0, 0),
        shadow_spread: 0,
        shadow_softness: 0,
        noise_alpha: 0,
    };

    pub fn valid(self) -> bool {
        self.blur_radius <= MAX_BLUR_RADIUS
            && (0..=300).contains(&self.saturation_percent)
            && (0..=300).contains(&self.brightness_percent)
            && self.border_width <= 16
            && self.inner_shadow_softness <= 32
            && self.shadow_spread <= 32
            && self.shadow_softness <= 32
    }

    pub fn scaled(mut self, scale: Scale) -> Self {
        self.blur_radius = scale
            .logical(self.blur_radius as i32)
            .clamp(0, MAX_BLUR_RADIUS as i32) as u8;
        self.border_width = scale.logical(self.border_width as i32).clamp(0, 16) as u8;
        self.inner_shadow_offset = Point::new(
            scale.logical(self.inner_shadow_offset.x),
            scale.logical(self.inner_shadow_offset.y),
        );
        self.inner_shadow_softness = scale
            .logical(self.inner_shadow_softness as i32)
            .clamp(0, 32) as u8;
        self.shadow_offset = Point::new(
            scale.logical(self.shadow_offset.x),
            scale.logical(self.shadow_offset.y),
        );
        self.shadow_spread = scale.logical(self.shadow_spread as i32).clamp(0, 32) as u8;
        self.shadow_softness = scale.logical(self.shadow_softness as i32).clamp(0, 32) as u8;
        self
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
    Middle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter,
    Space,
    Tab,
    Escape,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Event {
    PointerMoved(Point),
    PointerPressed {
        position: Point,
        button: PointerButton,
    },
    PointerReleased {
        position: Point,
        button: PointerButton,
    },
    PointerLeft,
    FocusChanged(bool),
    KeyPressed(Key),
    KeyReleased(Key),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct InteractionResponse {
    pub activated: bool,
    pub redraw: bool,
    pub capture_pointer: bool,
    pub release_pointer: bool,
}

impl InteractionResponse {
    const NONE: Self = Self {
        activated: false,
        redraw: false,
        capture_pointer: false,
        release_pointer: false,
    };
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ButtonInteraction {
    pub hovered: bool,
    pub pressed: bool,
    pub focused: bool,
    pub disabled: bool,
    pointer_armed: bool,
    keyboard_armed: bool,
}

impl ButtonInteraction {
    pub const fn new() -> Self {
        Self {
            hovered: false,
            pressed: false,
            focused: false,
            disabled: false,
            pointer_armed: false,
            keyboard_armed: false,
        }
    }

    pub fn set_disabled(&mut self, disabled: bool) -> bool {
        let changed = self.disabled != disabled;
        self.disabled = disabled;
        if disabled {
            self.hovered = false;
            self.pressed = false;
            self.pointer_armed = false;
            self.keyboard_armed = false;
        }
        changed
    }

    pub fn handle(&mut self, bounds: Rect, event: Event) -> InteractionResponse {
        if self.disabled {
            return InteractionResponse::NONE;
        }
        match event {
            Event::PointerMoved(position) => {
                let hovered = bounds.contains(position);
                let pressed = self.pointer_armed && hovered;
                let redraw = hovered != self.hovered || pressed != self.pressed;
                self.hovered = hovered;
                self.pressed = pressed;
                InteractionResponse {
                    redraw,
                    ..InteractionResponse::NONE
                }
            }
            Event::PointerPressed {
                position,
                button: PointerButton::Primary,
            } if bounds.contains(position) => {
                self.hovered = true;
                self.pressed = true;
                self.pointer_armed = true;
                InteractionResponse {
                    redraw: true,
                    capture_pointer: true,
                    ..InteractionResponse::NONE
                }
            }
            Event::PointerReleased {
                position,
                button: PointerButton::Primary,
            } if self.pointer_armed => {
                let activated = bounds.contains(position);
                self.hovered = activated;
                self.pressed = false;
                self.pointer_armed = false;
                InteractionResponse {
                    activated,
                    redraw: true,
                    release_pointer: true,
                    ..InteractionResponse::NONE
                }
            }
            Event::PointerLeft => {
                let redraw = self.hovered || self.pressed;
                self.hovered = false;
                self.pressed = false;
                InteractionResponse {
                    redraw,
                    ..InteractionResponse::NONE
                }
            }
            Event::FocusChanged(focused) => {
                let redraw = focused != self.focused || !focused && self.pressed;
                self.focused = focused;
                if !focused {
                    self.keyboard_armed = false;
                    if !self.pointer_armed {
                        self.pressed = false;
                    }
                }
                InteractionResponse {
                    redraw,
                    ..InteractionResponse::NONE
                }
            }
            Event::KeyPressed(Key::Enter | Key::Space) if self.focused => {
                let redraw = !self.keyboard_armed || !self.pressed;
                self.keyboard_armed = true;
                self.pressed = true;
                InteractionResponse {
                    redraw,
                    ..InteractionResponse::NONE
                }
            }
            Event::KeyReleased(Key::Enter | Key::Space) if self.focused && self.keyboard_armed => {
                self.keyboard_armed = false;
                self.pressed = false;
                InteractionResponse {
                    activated: true,
                    redraw: true,
                    ..InteractionResponse::NONE
                }
            }
            _ => InteractionResponse::NONE,
        }
    }
}

impl Default for ButtonInteraction {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
struct FrostPixel {
    red: u8,
    green: u8,
    blue: u8,
}

impl FrostPixel {
    const BLACK: Self = Self {
        red: 0,
        green: 0,
        blue: 0,
    };

    fn from_color(color: Color) -> Self {
        Self {
            red: color.red,
            green: color.green,
            blue: color.blue,
        }
    }

    fn color(self) -> Color {
        Color::rgb(self.red, self.green, self.blue)
    }
}

struct FrostScratch {
    source: [FrostPixel; MAX_FROST_PIXELS],
    horizontal: [FrostPixel; MAX_FROST_PIXELS],
}

static FROST_SCRATCH: TicketLock<FrostScratch> = TicketLock::new(FrostScratch {
    source: [FrostPixel::BLACK; MAX_FROST_PIXELS],
    horizontal: [FrostPixel::BLACK; MAX_FROST_PIXELS],
});

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FrostReport {
    pub pixels: usize,
    pub blur_radius: u8,
    pub captured: bool,
    pub clipped: bool,
}

/// The largest wxh-aspect rectangle that fits inside dest, centred.
pub fn fit_rect(dest: Rect, source_width: usize, source_height: usize) -> Rect {
    if source_width == 0 || source_height == 0 {
        return Rect::new(dest.x, dest.y, 0, 0);
    }
    let fit_width = dest
        .width
        .min((dest.height as i64 * source_width as i64 / source_height as i64) as i32);
    let fit_height = dest
        .height
        .min((dest.width as i64 * source_height as i64 / source_width as i64) as i32);
    Rect::new(
        dest.x + (dest.width - fit_width) / 2,
        dest.y + (dest.height - fit_height) / 2,
        fit_width,
        fit_height,
    )
}

pub struct Painter<'a> {
    frame: &'a mut FrameBuffer,
    clip: Rect,
}

impl<'a> Painter<'a> {
    pub fn new(frame: &'a mut FrameBuffer) -> Self {
        let clip = Rect::new(0, 0, frame.width() as i32, frame.height() as i32);
        Self { frame, clip }
    }

    pub fn clip(&self) -> Rect {
        self.clip
    }

    pub fn set_clip(&mut self, clip: Rect) -> bool {
        let screen = Rect::new(0, 0, self.frame.width() as i32, self.frame.height() as i32);
        let Some(clipped) = screen.intersect(clip) else {
            return false;
        };
        self.clip = clipped;
        true
    }

    pub fn reset_clip(&mut self) {
        self.clip = Rect::new(0, 0, self.frame.width() as i32, self.frame.height() as i32);
    }

    /// Draws an XRGB8888 image (another machine's framebuffer) into `dest`,
    /// scaled to fit while keeping its aspect ratio and centred. Shrinking
    /// averages each 2x2 source block so small text stays legible.
    ///
    /// # Safety
    /// source must point to at least source_stride * source_height`n    /// readable u32s.
    pub unsafe fn blit_scaled(
        &mut self,
        dest: Rect,
        source: *const u32,
        source_width: usize,
        source_height: usize,
        source_stride: usize,
    ) {
        if source.is_null() || source_width == 0 || source_height == 0 {
            return;
        }
        let fitted = fit_rect(dest, source_width, source_height);
        if fitted.width <= 0 || fitted.height <= 0 {
            return;
        }
        let Some(area) = fitted.intersect(self.clip) else {
            return;
        };
        // Near 1:1 (or enlarging): plain sampling keeps text sharp; only a real
        // reduction needs the 2x2 average.
        let smooth = (fitted.width as usize) * 10 < source_width * 9;
        let read = |x: usize, y: usize| -> u32 {
            let x = x.min(source_width - 1);
            let y = y.min(source_height - 1);
            unsafe { core::ptr::read_volatile(source.add(y * source_stride + x)) }
        };
        for y in area.y..area.bottom() {
            let sy = (y - fitted.y) as usize * source_height / fitted.height as usize;
            for x in area.x..area.right() {
                let sx = (x - fitted.x) as usize * source_width / fitted.width as usize;
                let taps: &[(usize, usize)] = if smooth {
                    &[(0, 0), (1, 0), (0, 1), (1, 1)]
                } else {
                    &[(0, 0)]
                };
                let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
                for &(dx, dy) in taps {
                    let pixel = read(sx + dx, sy + dy);
                    r += (pixel >> 16) & 0xff;
                    g += (pixel >> 8) & 0xff;
                    b += pixel & 0xff;
                }
                let count = taps.len() as u32;
                self.frame.pixel(
                    x,
                    y,
                    Color::rgb((r / count) as u8, (g / count) as u8, (b / count) as u8),
                );
            }
        }
    }

    pub fn fill_rounded_rect(&mut self, bounds: Rect, radii: CornerRadii, color: Rgba) {
        let Some(area) = bounds.intersect(self.clip) else {
            return;
        };
        let radii = radii.fit(bounds);
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let coverage = rounded_coverage(bounds, radii, Point::new(x, y));
                if coverage != 0 {
                    self.frame
                        .blend(x, y, color.color(), multiply_alpha(color.alpha, coverage));
                }
            }
        }
    }

    pub fn stroke_rounded_rect(
        &mut self,
        bounds: Rect,
        radii: CornerRadii,
        width: u8,
        color: Rgba,
    ) {
        if width == 0 || color.alpha == 0 {
            return;
        }
        let Some(area) = bounds.intersect(self.clip) else {
            return;
        };
        let radii = radii.fit(bounds);
        let inner = bounds.inset(Insets::all(width as i32));
        let inner_radii = radii.inset(width as i32);
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let point = Point::new(x, y);
                let outer_coverage = rounded_coverage(bounds, radii, point);
                let inner_coverage = inner
                    .map(|inner| rounded_coverage(inner, inner_radii, point))
                    .unwrap_or(0);
                let coverage = outer_coverage.saturating_sub(inner_coverage);
                if coverage != 0 {
                    self.frame
                        .blend(x, y, color.color(), multiply_alpha(color.alpha, coverage));
                }
            }
        }
    }

    pub fn text(
        &mut self,
        font: RasterFont,
        origin: Point,
        value: &str,
        height: i32,
        color: Color,
    ) {
        font.draw(self.frame, origin.x, origin.y, value, height, color);
    }

    pub fn frosted_rounded_rect(
        &mut self,
        bounds: Rect,
        radii: CornerRadii,
        style: FrostStyle,
        noise_seed: u32,
    ) -> FrostReport {
        let clipped = bounds.intersect(self.clip);
        let Some(area) = clipped else {
            return FrostReport {
                pixels: 0,
                blur_radius: style.blur_radius,
                captured: false,
                clipped: true,
            };
        };
        if !style.valid() {
            return FrostReport {
                pixels: 0,
                blur_radius: style.blur_radius,
                captured: false,
                clipped: area != bounds,
            };
        }
        let pixels = area.width as usize * area.height as usize;
        if pixels > MAX_FROST_PIXELS {
            self.fill_rounded_rect(bounds, radii, style.tint);
            self.stroke_rounded_rect(bounds, radii, style.border_width, style.border);
            return FrostReport {
                pixels,
                blur_radius: style.blur_radius,
                captured: false,
                clipped: area != bounds,
            };
        }
        let fitted = radii.fit(bounds);
        self.paint_shadow(bounds, fitted, style);
        let mut scratch = FROST_SCRATCH.lock();
        capture(self.frame, area, &mut scratch.source[..pixels]);
        {
            let FrostScratch { source, horizontal } = &mut *scratch;
            horizontal_blur(
                &source[..pixels],
                &mut horizontal[..pixels],
                area.width as usize,
                area.height as usize,
                style.blur_radius as usize,
            );
        }
        let width = area.width as usize;
        let height = area.height as usize;
        let radius = style.blur_radius as usize;
        for local_x in 0..width {
            let mut start = 0usize;
            let mut end = radius.min(height - 1);
            let mut red = 0u32;
            let mut green = 0u32;
            let mut blue = 0u32;
            for sample_y in start..=end {
                let pixel = scratch.horizontal[sample_y * width + local_x];
                red += pixel.red as u32;
                green += pixel.green as u32;
                blue += pixel.blue as u32;
            }
            for local_y in 0..height {
                let x = area.x + local_x as i32;
                let y = area.y + local_y as i32;
                let coverage = rounded_coverage(bounds, fitted, Point::new(x, y));
                if coverage != 0 {
                    let samples = (end - start + 1) as u32;
                    let pixel = FrostPixel {
                        red: (red / samples) as u8,
                        green: (green / samples) as u8,
                        blue: (blue / samples) as u8,
                    };
                    let color = transform_frost(pixel, style, x, y, noise_seed).color();
                    if coverage == 255 {
                        self.frame.pixel(x, y, color);
                    } else {
                        self.frame.blend(x, y, color, coverage);
                    }
                }
                if local_y + 1 == height {
                    continue;
                }
                let next = local_y + 1;
                let next_start = next.saturating_sub(radius);
                let next_end = next.saturating_add(radius).min(height - 1);
                if next_start > start {
                    let pixel = scratch.horizontal[start * width + local_x];
                    red -= pixel.red as u32;
                    green -= pixel.green as u32;
                    blue -= pixel.blue as u32;
                }
                if next_end > end {
                    let pixel = scratch.horizontal[next_end * width + local_x];
                    red += pixel.red as u32;
                    green += pixel.green as u32;
                    blue += pixel.blue as u32;
                }
                start = next_start;
                end = next_end;
            }
        }
        drop(scratch);
        self.paint_inner_shadow(bounds, fitted, style);
        self.stroke_rounded_rect(bounds, fitted, style.border_width, style.border);
        if style.inner_highlight.alpha != 0 {
            self.stroke_rounded_rect(
                bounds
                    .inset(Insets::all(style.border_width as i32))
                    .unwrap_or(bounds),
                fitted.inset(style.border_width as i32),
                1,
                style.inner_highlight,
            );
        }
        FrostReport {
            pixels,
            blur_radius: style.blur_radius,
            captured: true,
            clipped: area != bounds,
        }
    }

    fn paint_shadow(&mut self, bounds: Rect, radii: CornerRadii, style: FrostStyle) {
        if style.shadow.alpha == 0 {
            return;
        }
        let layers = style.shadow_softness.clamp(1, 16);
        for layer in (0..layers).rev() {
            let expansion = style.shadow_spread as i32 + layer as i32;
            let shifted = Rect::new(
                bounds.x + style.shadow_offset.x,
                bounds.y + style.shadow_offset.y,
                bounds.width,
                bounds.height,
            )
            .expand(expansion);
            let mut shadow = style.shadow;
            shadow.alpha = (style.shadow.alpha as u16 / layers as u16).max(1) as u8;
            self.fill_rounded_rect(
                shifted,
                CornerRadii::new(
                    radii.top_left + expansion,
                    radii.top_right + expansion,
                    radii.bottom_right + expansion,
                    radii.bottom_left + expansion,
                ),
                shadow,
            );
        }
    }

    fn paint_inner_shadow(&mut self, bounds: Rect, radii: CornerRadii, style: FrostStyle) {
        if style.inner_shadow.alpha == 0 || style.inner_shadow_softness == 0 {
            return;
        }
        let previous = self.clip;
        let Some(mask) = previous.intersect(bounds) else {
            return;
        };
        self.clip = mask;
        let softness = style.inner_shadow_softness.clamp(1, 32) as i32;
        let denominator = (softness * softness) as u32;
        for y in mask.y..mask.bottom() {
            for x in mask.x..mask.right() {
                let point = Point::new(x, y);
                let coverage = rounded_coverage(bounds, radii, point);
                if coverage == 0 {
                    continue;
                }
                let shifted = Point::new(
                    x.saturating_sub(style.inner_shadow_offset.x),
                    y.saturating_sub(style.inner_shadow_offset.y),
                );
                let distance = rounded_edge_distance(bounds, radii, shifted).min(softness);
                let remaining = softness - distance;
                if remaining == 0 {
                    continue;
                }
                let falloff = (remaining * remaining) as u32;
                let alpha = (style.inner_shadow.alpha as u32 * falloff / denominator) as u8;
                self.frame.blend(
                    x,
                    y,
                    style.inner_shadow.color(),
                    multiply_alpha(alpha, coverage),
                );
            }
        }
        self.clip = previous;
    }
}

fn rounded_coverage(bounds: Rect, radii: CornerRadii, point: Point) -> u8 {
    if !bounds.contains(point) {
        return 0;
    }
    let corners = [
        (
            Rect::new(bounds.x, bounds.y, radii.top_left, radii.top_left),
            Point::new(bounds.x + radii.top_left, bounds.y + radii.top_left),
            radii.top_left,
        ),
        (
            Rect::new(
                bounds.right() - radii.top_right,
                bounds.y,
                radii.top_right,
                radii.top_right,
            ),
            Point::new(bounds.right() - radii.top_right, bounds.y + radii.top_right),
            radii.top_right,
        ),
        (
            Rect::new(
                bounds.right() - radii.bottom_right,
                bounds.bottom() - radii.bottom_right,
                radii.bottom_right,
                radii.bottom_right,
            ),
            Point::new(
                bounds.right() - radii.bottom_right,
                bounds.bottom() - radii.bottom_right,
            ),
            radii.bottom_right,
        ),
        (
            Rect::new(
                bounds.x,
                bounds.bottom() - radii.bottom_left,
                radii.bottom_left,
                radii.bottom_left,
            ),
            Point::new(
                bounds.x + radii.bottom_left,
                bounds.bottom() - radii.bottom_left,
            ),
            radii.bottom_left,
        ),
    ];
    for (corner, center, radius) in corners {
        if radius > 0 && corner.contains(point) {
            let mut samples = 0u16;
            let center_x = center.x as i64 * 8;
            let center_y = center.y as i64 * 8;
            let radius = radius as i64 * 8;
            let radius_squared = radius * radius;
            for offset_y in [1i64, 3, 5, 7] {
                for offset_x in [1i64, 3, 5, 7] {
                    let dx = point.x as i64 * 8 + offset_x - center_x;
                    let dy = point.y as i64 * 8 + offset_y - center_y;
                    if dx * dx + dy * dy <= radius_squared {
                        samples += 1;
                    }
                }
            }
            return ((samples * 255 + 8) / 16) as u8;
        }
    }
    255
}

fn rounded_edge_distance(bounds: Rect, radii: CornerRadii, point: Point) -> i32 {
    if !bounds.contains(point) || rounded_coverage(bounds, radii, point) == 0 {
        return 0;
    }
    let mut distance = (point.x - bounds.x)
        .min(bounds.right() - 1 - point.x)
        .min(point.y - bounds.y)
        .min(bounds.bottom() - 1 - point.y)
        .max(0);
    let corners = [
        (
            Point::new(bounds.x + radii.top_left, bounds.y + radii.top_left),
            radii.top_left,
            point.x < bounds.x + radii.top_left && point.y < bounds.y + radii.top_left,
        ),
        (
            Point::new(bounds.right() - radii.top_right, bounds.y + radii.top_right),
            radii.top_right,
            point.x >= bounds.right() - radii.top_right && point.y < bounds.y + radii.top_right,
        ),
        (
            Point::new(
                bounds.right() - radii.bottom_right,
                bounds.bottom() - radii.bottom_right,
            ),
            radii.bottom_right,
            point.x >= bounds.right() - radii.bottom_right
                && point.y >= bounds.bottom() - radii.bottom_right,
        ),
        (
            Point::new(
                bounds.x + radii.bottom_left,
                bounds.bottom() - radii.bottom_left,
            ),
            radii.bottom_left,
            point.x < bounds.x + radii.bottom_left
                && point.y >= bounds.bottom() - radii.bottom_left,
        ),
    ];
    for (center, radius, active) in corners {
        if active && radius > 0 {
            let dx = (point.x - center.x) as i64;
            let dy = (point.y - center.y) as i64;
            let radial = integer_sqrt((dx * dx + dy * dy) as u64) as i32;
            distance = distance.min(radius.saturating_sub(radial).max(0));
        }
    }
    distance
}

fn integer_sqrt(value: u64) -> u64 {
    if value < 2 {
        return value;
    }
    let mut current = value;
    let mut next = current.div_ceil(2);
    while next < current {
        current = next;
        next = (current + value / current) / 2;
    }
    current
}

fn multiply_alpha(first: u8, second: u8) -> u8 {
    ((first as u16 * second as u16 + 127) / 255) as u8
}

fn capture(frame: &FrameBuffer, area: Rect, destination: &mut [FrostPixel]) {
    for y in 0..area.height as usize {
        for x in 0..area.width as usize {
            destination[y * area.width as usize + x] = frame
                .color_at(area.x + x as i32, area.y + y as i32)
                .map(FrostPixel::from_color)
                .unwrap_or(FrostPixel::BLACK);
        }
    }
}

fn horizontal_blur(
    source: &[FrostPixel],
    destination: &mut [FrostPixel],
    width: usize,
    height: usize,
    radius: usize,
) {
    for y in 0..height {
        let mut start = 0usize;
        let mut end = radius.min(width - 1);
        let mut red = 0u32;
        let mut green = 0u32;
        let mut blue = 0u32;
        for sample_x in start..=end {
            let pixel = source[y * width + sample_x];
            red += pixel.red as u32;
            green += pixel.green as u32;
            blue += pixel.blue as u32;
        }
        for x in 0..width {
            let samples = (end - start + 1) as u32;
            destination[y * width + x] = FrostPixel {
                red: (red / samples) as u8,
                green: (green / samples) as u8,
                blue: (blue / samples) as u8,
            };
            if x + 1 == width {
                continue;
            }
            let next = x + 1;
            let next_start = next.saturating_sub(radius);
            let next_end = next.saturating_add(radius).min(width - 1);
            if next_start > start {
                let pixel = source[y * width + start];
                red -= pixel.red as u32;
                green -= pixel.green as u32;
                blue -= pixel.blue as u32;
            }
            if next_end > end {
                let pixel = source[y * width + next_end];
                red += pixel.red as u32;
                green += pixel.green as u32;
                blue += pixel.blue as u32;
            }
            start = next_start;
            end = next_end;
        }
    }
}

fn transform_frost(pixel: FrostPixel, style: FrostStyle, x: i32, y: i32, seed: u32) -> FrostPixel {
    let luma = (pixel.red as i32 * 54 + pixel.green as i32 * 183 + pixel.blue as i32 * 19) / 256;
    let saturation = style.saturation_percent as i32;
    let brightness = style.brightness_percent as i32;
    let tint_alpha = style.tint.alpha as i32;
    let mut channels = [pixel.red as i32, pixel.green as i32, pixel.blue as i32];
    let tint = [
        style.tint.red as i32,
        style.tint.green as i32,
        style.tint.blue as i32,
    ];
    let noise = noise(x, y, seed) as i32 - 128;
    for (index, channel) in channels.iter_mut().enumerate() {
        *channel = luma + (*channel - luma) * saturation / 100;
        *channel = *channel * brightness / 100;
        *channel = (*channel * (255 - tint_alpha) + tint[index] * tint_alpha) / 255;
        *channel += noise * style.noise_alpha as i32 / 255;
        *channel = (*channel).clamp(0, 255);
    }
    FrostPixel {
        red: channels[0] as u8,
        green: channels[1] as u8,
        blue: channels[2] as u8,
    }
}

fn noise(x: i32, y: i32, seed: u32) -> u8 {
    let mut value =
        seed ^ (x as u32).wrapping_mul(0x9e37_79b9) ^ (y as u32).wrapping_mul(0x85eb_ca6b);
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    (value ^ value >> 16) as u8
}

#[derive(Clone, Copy)]
pub struct UiCoreReport {
    pub geometry: bool,
    pub scaling: bool,
    pub interaction: bool,
    pub frost: bool,
    pub verified: bool,
}

#[derive(Clone, Copy)]
pub struct UiRenderReport {
    pub pixels: usize,
    pub captured: bool,
    pub changed: bool,
    pub corner_clipped: bool,
    pub verified: bool,
}

pub fn render_self_test(frame: &mut FrameBuffer) -> UiRenderReport {
    if frame.width() < 64 || frame.height() < 64 {
        return UiRenderReport {
            pixels: 0,
            captured: false,
            changed: false,
            corner_clipped: false,
            verified: false,
        };
    }
    frame.clear(Color::rgb(8, 16, 28));
    for y in 8..40 {
        for x in 8..56 {
            let color = if (x / 4 + y / 4) & 1 == 0 {
                Color::rgb(34, 91, 168)
            } else {
                Color::rgb(184, 71, 122)
            };
            frame.pixel(x, y, color);
        }
    }
    let bounds = Rect::new(8, 8, 48, 32);
    let corner_before = frame.color_at(8, 8);
    let center_before = frame.color_at(32, 24);
    let style = FrostStyle {
        blur_radius: 4,
        saturation_percent: 125,
        brightness_percent: 108,
        tint: Rgba::new(170, 210, 255, 72),
        border: Rgba::new(240, 248, 255, 130),
        border_width: 1,
        inner_highlight: Rgba::new(255, 255, 255, 80),
        noise_alpha: 3,
        ..FrostStyle::CLEAR
    };
    let report = {
        let mut painter = Painter::new(frame);
        painter.frosted_rounded_rect(bounds, CornerRadii::new(12, 6, 14, 3), style, 0xaec0_0001)
    };
    let changed = frame.color_at(32, 24) != center_before;
    let corner_clipped = frame.color_at(8, 8) == corner_before;
    UiRenderReport {
        pixels: report.pixels,
        captured: report.captured,
        changed,
        corner_clipped,
        verified: report.captured && report.pixels == 48 * 32 && changed && corner_clipped,
    }
}

pub fn self_test() -> UiCoreReport {
    let bounds = Rect::new(10, 20, 200, 80);
    let geometry = bounds.contains(Point::new(10, 20))
        && bounds.contains(Point::new(209, 99))
        && !bounds.contains(Point::new(210, 100))
        && bounds
            .inset(Insets::symmetric(10, 20))
            .is_some_and(|value| value == Rect::new(30, 30, 160, 60))
        && CornerRadii::new(100, 100, 100, 100).fit(bounds) == CornerRadii::all(40);
    let scaling = Scale::from_milli(1_500).is_some_and(|scale| {
        scale.logical(10) == 15 && scale.size(Size::new(100, 40)) == Size::new(150, 60)
    });
    let mut state = ButtonInteraction::new();
    let pressed = state.handle(
        bounds,
        Event::PointerPressed {
            position: Point::new(50, 40),
            button: PointerButton::Primary,
        },
    );
    let released = state.handle(
        bounds,
        Event::PointerReleased {
            position: Point::new(50, 40),
            button: PointerButton::Primary,
        },
    );
    state.handle(bounds, Event::FocusChanged(true));
    state.handle(bounds, Event::KeyPressed(Key::Space));
    let keyboard = state.handle(bounds, Event::KeyReleased(Key::Space));
    state.set_disabled(true);
    let disabled = state.handle(
        bounds,
        Event::PointerPressed {
            position: Point::new(50, 40),
            button: PointerButton::Primary,
        },
    );
    let interaction = pressed.capture_pointer
        && released.release_pointer
        && released.activated
        && keyboard.activated
        && !disabled.activated
        && !state.pressed;
    let mut blur_source = [FrostPixel::BLACK; 3];
    blur_source[1].red = 255;
    let mut blur_output = [FrostPixel::BLACK; 3];
    horizontal_blur(&blur_source, &mut blur_output, 3, 1, 1);
    let frost = FrostStyle::CLEAR.valid()
        && !FrostStyle {
            blur_radius: MAX_BLUR_RADIUS + 1,
            ..FrostStyle::CLEAR
        }
        .valid()
        && blur_output[0].red == 127
        && blur_output[1].red == 85
        && blur_output[2].red == 127
        && noise(10, 20, 30) != noise(11, 20, 30);
    UiCoreReport {
        geometry,
        scaling,
        interaction,
        frost,
        verified: geometry && scaling && interaction && frost,
    }
}
