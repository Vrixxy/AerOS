use crate::font::RasterFont;
use crate::framebuffer::{Color, FrameBuffer};
use crate::sync::TicketLock;

pub const MAX_FROST_PIXELS: usize = 1_048_576;
const MAX_BLUR_RADIUS: u8 = 32;
/// Below this many pixels the CPU blur is cheaper than a GPU round trip.
const GPU_FROST_MIN_PIXELS: usize = 3_000;

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

/// Nanoseconds spent per frost stage since the last `frost_profile_take`:
/// shadow, capture, horizontal blur, vertical blur + colour + write, edges.
static FROST_PROFILE: [core::sync::atomic::AtomicU64; 5] = [
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
];

fn frost_prof(slot: usize, since: u64) -> u64 {
    let now = crate::time::monotonic_nanoseconds();
    FROST_PROFILE[slot].fetch_add(
        now.saturating_sub(since),
        core::sync::atomic::Ordering::Relaxed,
    );
    now
}

pub fn frost_profile_take() -> [u64; 5] {
    let mut out = [0u64; 5];
    for (slot, value) in FROST_PROFILE.iter().enumerate() {
        out[slot] = value.swap(0, core::sync::atomic::Ordering::Relaxed) / 1000;
    }
    out
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

    /// Blends one pixel (respecting the clip).
    pub fn blend_pixel(&mut self, x: i32, y: i32, color: Color, alpha: u8) {
        if x >= self.clip.x && x < self.clip.right() && y >= self.clip.y && y < self.clip.bottom() {
            self.frame.blend(x, y, color, alpha);
        }
    }

    pub fn frame_width(&self) -> i32 {
        self.frame.width() as i32
    }

    pub fn frame_height(&self) -> i32 {
        self.frame.height() as i32
    }

    /// Draws an `sw` x `sh` straight-alpha RGBA bitmap scaled into `dest`
    /// with bilinear sampling (alpha-weighted so transparent edges stay clean).
    pub fn draw_rgba_scaled(&mut self, dest: Rect, src: &[u8], sw: usize, sh: usize) {
        self.draw_rgba_scaled_alpha(dest, src, sw, sh, 255);
    }

    /// `draw_rgba_scaled` with an extra overall opacity (0..=255).
    pub fn draw_rgba_scaled_alpha(
        &mut self,
        dest: Rect,
        src: &[u8],
        sw: usize,
        sh: usize,
        opacity: u8,
    ) {
        if opacity == 0 {
            return;
        }
        if dest.width <= 0 || dest.height <= 0 || src.len() < sw * sh * 4 || sw < 2 || sh < 2 {
            return;
        }
        let Some(area) = dest.intersect(self.clip) else {
            return;
        };
        let max_x = (sw as i64 - 1) * 256;
        let max_y = (sh as i64 - 1) * 256;
        for y in area.y..area.bottom() {
            let v = ((((y - dest.y) * 2 + 1) as i64 * sh as i64 * 128) / dest.height as i64 - 128)
                .clamp(0, max_y);
            let (y0, fy) = ((v >> 8) as usize, v & 255);
            let y1 = (y0 + 1).min(sh - 1);
            for x in area.x..area.right() {
                let u = ((((x - dest.x) * 2 + 1) as i64 * sw as i64 * 128) / dest.width as i64
                    - 128)
                    .clamp(0, max_x);
                let (x0, fx) = ((u >> 8) as usize, u & 255);
                let x1 = (x0 + 1).min(sw - 1);
                let taps = [
                    (x0, y0, (256 - fx) * (256 - fy)),
                    (x1, y0, fx * (256 - fy)),
                    (x0, y1, (256 - fx) * fy),
                    (x1, y1, fx * fy),
                ];
                let (mut sum_a, mut sum_r, mut sum_g, mut sum_b) = (0i64, 0i64, 0i64, 0i64);
                for (tx, ty, weight) in taps {
                    let pixel = &src[(ty * sw + tx) * 4..(ty * sw + tx) * 4 + 4];
                    let wa = weight * pixel[3] as i64;
                    sum_a += wa;
                    sum_r += wa * pixel[0] as i64;
                    sum_g += wa * pixel[1] as i64;
                    sum_b += wa * pixel[2] as i64;
                }
                if sum_a == 0 {
                    continue;
                }
                let alpha = ((sum_a / 65536).clamp(0, 255) * opacity as i64 / 255) as u8;
                if alpha == 0 {
                    continue;
                }
                let color = Color::rgb(
                    (sum_r / sum_a) as u8,
                    (sum_g / sum_a) as u8,
                    (sum_b / sum_a) as u8,
                );
                self.frame.blend(x, y, color, alpha);
            }
        }
    }

    /// Copies a screen rectangle out to `dst` (row-major, `width * height`
    /// pixels). Used to cache expensive, unchanging surfaces such as the dock.
    pub fn read_region(&self, bounds: Rect, dst: &mut [u32]) -> bool {
        let info = self.frame.info();
        let (Ok(x), Ok(y), Ok(w), Ok(h)) = (
            usize::try_from(bounds.x),
            usize::try_from(bounds.y),
            usize::try_from(bounds.width),
            usize::try_from(bounds.height),
        ) else {
            return false;
        };
        if x + w > info.width || y + h > info.height || w * h > dst.len() {
            return false;
        }
        for row in 0..h {
            let start = (y + row) * info.stride + x;
            for col in 0..w {
                // SAFETY: bounds were checked against the framebuffer size.
                dst[row * w + col] =
                    unsafe { core::ptr::read_volatile(info.address.add(start + col)) };
            }
        }
        true
    }

    /// Writes back a rectangle saved with `read_region`.
    pub fn write_region(&mut self, bounds: Rect, src: &[u32]) -> bool {
        let info = self.frame.info();
        let (Ok(x), Ok(y), Ok(w), Ok(h)) = (
            usize::try_from(bounds.x),
            usize::try_from(bounds.y),
            usize::try_from(bounds.width),
            usize::try_from(bounds.height),
        ) else {
            return false;
        };
        if x + w > info.width || y + h > info.height || w * h > src.len() {
            return false;
        }
        for row in 0..h {
            let start = (y + row) * info.stride + x;
            for col in 0..w {
                // SAFETY: bounds were checked against the framebuffer size.
                unsafe {
                    core::ptr::write_volatile(info.address.add(start + col), src[row * w + col])
                };
            }
        }
        true
    }

    pub fn fill_rounded_rect(&mut self, bounds: Rect, radii: CornerRadii, color: Rgba) {
        let Some(area) = bounds.intersect(self.clip) else {
            return;
        };
        let radii = radii.fit(bounds);
        // Rows between the corner bands are fully covered, so they skip the
        // per-pixel corner test (most of a large panel).
        let band_top = bounds.y + radii.top_left.max(radii.top_right);
        let band_bottom = bounds.bottom() - radii.bottom_left.max(radii.bottom_right);
        // Columns that no corner square reaches are plain fill on every row.
        let plain_left = bounds.x + radii.top_left.max(radii.bottom_left);
        let plain_right = bounds.right() - radii.top_right.max(radii.bottom_right);
        for y in area.y..area.bottom() {
            if y >= band_top && y < band_bottom {
                for x in area.x..area.right() {
                    self.frame.blend(x, y, color.color(), color.alpha);
                }
                continue;
            }
            for x in area.x..area.right() {
                if x >= plain_left && x < plain_right {
                    self.frame.blend(x, y, color.color(), color.alpha);
                    continue;
                }
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
        let band_top = bounds.y + radii.top_left.max(radii.top_right);
        let band_bottom = bounds.bottom() - radii.bottom_left.max(radii.bottom_right);
        let edge = width as i32;
        let plain_left = bounds.x + radii.top_left.max(radii.bottom_left);
        let plain_right = bounds.right() - radii.top_right.max(radii.bottom_right);
        for y in area.y..area.bottom() {
            if y >= band_top && y < band_bottom {
                // Straight sides: only the two edge strips are inked.
                for x in area.x..area.right().min(bounds.x + edge) {
                    self.frame.blend(x, y, color.color(), color.alpha);
                }
                for x in area.x.max(bounds.right() - edge)..area.right() {
                    self.frame.blend(x, y, color.color(), color.alpha);
                }
                continue;
            }
            for x in area.x..area.right() {
                if x >= plain_left && x < plain_right {
                    // Between the corner squares only the top and bottom
                    // edge strips are inked.
                    if y < bounds.y + edge || y >= bounds.bottom() - edge {
                        self.frame.blend(x, y, color.color(), color.alpha);
                    }
                    continue;
                }
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
        self.frosted_rounded_rect_stepped(bounds, radii, style, noise_seed, 1)
    }

    /// Like `frosted_rounded_rect`, but the per-pixel colour work (saturation,
    /// noise, tint) is done once per `step` x `step` block. The blur itself is
    /// smooth, so at 2-3 pixels the difference is hard to see, while the cost
    /// drops by `step` squared: for panels that are moving.
    pub fn frosted_rounded_rect_stepped(
        &mut self,
        bounds: Rect,
        radii: CornerRadii,
        style: FrostStyle,
        noise_seed: u32,
        step: usize,
    ) -> FrostReport {
        let step = step.max(1);
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
        // The drop shadow is up to 16 full-size blended layers - by far the
        // costliest part for a big panel - so a moving panel (step > 1) goes
        // without it and gets it back when it settles.
        let t0 = crate::time::monotonic_nanoseconds();
        if step == 1 {
            self.paint_shadow(bounds, fitted, style);
        }
        let t1 = frost_prof(0, t0);
        if pixels >= GPU_FROST_MIN_PIXELS
            && crate::virtio_gpu::frost_available()
            && self.frost_on_gpu(area, bounds, fitted, style)
        {
            self.frost_edges(bounds, fitted, style);
            return FrostReport {
                pixels,
                blur_radius: style.blur_radius,
                captured: true,
                clipped: area != bounds,
            };
        }
        let mut scratch = FROST_SCRATCH.lock();
        capture(self.frame, area, &mut scratch.source[..pixels]);
        let t2 = frost_prof(1, t1);
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
        let t3 = frost_prof(2, t2);
        let width = area.width as usize;
        let height = area.height as usize;
        let radius = style.blur_radius as usize;
        let corner = fitted
            .top_left
            .max(fitted.top_right)
            .max(fitted.bottom_right)
            .max(fitted.bottom_left);
        for local_x in (0..width).step_by(step) {
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
                if local_y % step == 0 {
                    // One colour for the whole step x step block; each pixel
                    // still gets its own edge coverage.
                    let samples = (end - start + 1) as u32;
                    let mut block_color = None;
                    for offset_y in 0..step.min(height - local_y) {
                        for offset_x in 0..step.min(width - local_x) {
                            let x = area.x + (local_x + offset_x) as i32;
                            let y = area.y + (local_y + offset_y) as i32;
                            let coverage = if (x >= bounds.x + corner
                                && x < bounds.right() - corner)
                                || (y >= bounds.y + corner && y < bounds.bottom() - corner)
                            {
                                255 // inside the straight part of the shape
                            } else {
                                rounded_coverage(bounds, fitted, Point::new(x, y))
                            };
                            if coverage == 0 {
                                continue;
                            }
                            let color = *block_color.get_or_insert_with(|| {
                                let pixel = FrostPixel {
                                    red: (red / samples) as u8,
                                    green: (green / samples) as u8,
                                    blue: (blue / samples) as u8,
                                };
                                transform_frost(pixel, style, x, y, noise_seed).color()
                            });
                            if coverage == 255 {
                                self.frame.pixel(x, y, color);
                            } else {
                                self.frame.blend(x, y, color, coverage);
                            }
                        }
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
        let t4 = frost_prof(3, t3);
        self.frost_edges(bounds, fitted, style);
        frost_prof(4, t4);
        FrostReport {
            pixels,
            blur_radius: style.blur_radius,
            captured: true,
            clipped: area != bounds,
        }
    }

    /// The inner shadow, border and highlight painted over a frosted shape.
    fn frost_edges(&mut self, bounds: Rect, fitted: CornerRadii, style: FrostStyle) {
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
    }

    /// The whole frosted fill (blur, grade, shape edge) on the GPU, written
    /// straight from the GPU's rows. False, with the frame untouched, when
    /// the GPU cannot do it.
    fn frost_on_gpu(
        &mut self,
        area: Rect,
        bounds: Rect,
        fitted: CornerRadii,
        style: FrostStyle,
    ) -> bool {
        let width = area.width as usize;
        let height = area.height as usize;
        let grade = crate::virtio_gpu::Grade {
            saturation: style.saturation_percent as f32 / 100.0,
            brightness: style.brightness_percent as f32 / 100.0,
            tint: [
                style.tint.red as f32 / 255.0,
                style.tint.green as f32 / 255.0,
                style.tint.blue as f32 / 255.0,
                style.tint.alpha as f32 / 255.0,
            ],
        };
        let corner = fitted
            .top_left
            .max(fitted.top_right)
            .max(fitted.bottom_right)
            .max(fitted.bottom_left);
        if area.x < 0
            || area.y < 0
            || area.right() > self.frame.width() as i32
            || area.bottom() > self.frame.height() as i32
        {
            return false;
        }
        let frame_pointer: *mut FrameBuffer = &mut *self.frame;
        let (x0, y0) = (area.x as usize, area.y as usize);
        crate::virtio_gpu::frost_pass(
            width as u32,
            height as u32,
            style.blur_radius as u32,
            grade,
            &mut |bytes, stride| {
                // SAFETY: the pointer is to the frame this painter holds;
                // the two closures run one after the other.
                let frame = unsafe { &*frame_pointer };
                frame.read_bgra_region(x0, y0, width, height, bytes, stride);
            },
            &mut |bytes, stride| {
                let frame = unsafe { &mut *frame_pointer };
                let straight_left = (bounds.x + corner).max(area.x);
                let straight_right = (bounds.right() - corner).min(area.right());
                for local_y in 0..height {
                    let y = area.y + local_y as i32;
                    let row = &bytes[local_y * stride..local_y * stride + width * 4];
                    let in_band = y >= bounds.y + corner && y < bounds.bottom() - corner;
                    if in_band {
                        frame.write_bgra_row(x0, y as usize, row);
                        continue;
                    }
                    let mid_start = (straight_left - area.x).clamp(0, width as i32) as usize;
                    let mid_end = (straight_right - area.x).clamp(0, width as i32) as usize;
                    let (mid_start, mid_end) = if mid_end > mid_start {
                        (mid_start, mid_end)
                    } else {
                        (width, width)
                    };
                    for local_x in (0..mid_start).chain(mid_end..width) {
                        let x = area.x + local_x as i32;
                        let coverage = rounded_coverage(bounds, fitted, Point::new(x, y));
                        if coverage == 0 {
                            continue;
                        }
                        let at = local_x * 4;
                        let color = Color::rgb(row[at + 2], row[at + 1], row[at]);
                        if coverage == 255 {
                            frame.pixel(x, y, color);
                        } else {
                            frame.blend(x, y, color, coverage);
                        }
                    }
                    if mid_end > mid_start {
                        frame.write_bgra_row(
                            x0 + mid_start,
                            y as usize,
                            &row[mid_start * 4..mid_end * 4],
                        );
                    }
                }
            },
        )
    }

    /// The drop shadow: what stacking `shadow_softness` growing copies of the
    /// panel would give, worked out once per pixel from its distance to the
    /// (offset) panel instead of by filling the whole stack.
    fn paint_shadow(&mut self, bounds: Rect, radii: CornerRadii, style: FrostStyle) {
        if style.shadow.alpha == 0 {
            return;
        }
        let layers = style.shadow_softness.clamp(1, 16) as i32;
        let layer_alpha = (style.shadow.alpha as u32 / layers as u32).max(1);
        let spread = style.shadow_spread as i32;
        // table[n]: the alpha of n translucent layers laid over each other.
        let mut table = [0u8; 17];
        let mut keep = 255u32;
        for entry in table.iter_mut().take(layers as usize + 1).skip(1) {
            keep = keep * (255 - layer_alpha) / 255;
            *entry = (255 - keep) as u8;
        }
        let shape = Rect::new(
            bounds.x + style.shadow_offset.x,
            bounds.y + style.shadow_offset.y,
            bounds.width,
            bounds.height,
        );
        let reach = spread + layers;
        let Some(area) = shape.expand(reach).intersect(self.clip) else {
            return;
        };
        let corner = radii
            .top_left
            .max(radii.top_right)
            .max(radii.bottom_right)
            .max(radii.bottom_left);
        let color = style.shadow.color();
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let count = if x >= shape.x + corner
                    && x < shape.right() - corner
                    && y >= shape.y
                    && y < shape.bottom()
                    || y >= shape.y + corner
                        && y < shape.bottom() - corner
                        && x >= shape.x
                        && x < shape.right()
                {
                    layers
                } else {
                    let distance = outside_distance(shape, radii, Point::new(x, y));
                    (layers - (distance - spread).max(0)).clamp(0, layers)
                };
                let alpha = table[count as usize];
                if alpha != 0 {
                    self.frame.blend(x, y, color, alpha);
                }
            }
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
        // Only a band along the edges gets any shadow (a pixel whose shifted
        // position is `softness` or more from every edge is skipped below), so
        // the interior rectangle is not even visited.
        // (Kept clear of the rounded corners too, where the distance to the
        // arc can be smaller than the distance to the straight edges.)
        let corner = radii
            .top_left
            .max(radii.top_right)
            .max(radii.bottom_right)
            .max(radii.bottom_left);
        let inset = softness.max(corner);
        let interior_x0 = bounds.x + inset + style.inner_shadow_offset.x;
        let interior_x1 = bounds.right() - inset + style.inner_shadow_offset.x;
        let interior_y0 = bounds.y + inset + style.inner_shadow_offset.y;
        let interior_y1 = bounds.bottom() - inset + style.inner_shadow_offset.y;
        for y in mask.y..mask.bottom() {
            let mut x = mask.x;
            while x < mask.right() {
                if y >= interior_y0 && y < interior_y1 && x >= interior_x0 && x < interior_x1 {
                    x = interior_x1.min(mask.right());
                    continue;
                }
                let px = x;
                x += 1;
                let x = px;
                let point = Point::new(x, y);
                let shifted = Point::new(
                    x.saturating_sub(style.inner_shadow_offset.x),
                    y.saturating_sub(style.inner_shadow_offset.y),
                );
                let straight = |p: Point| {
                    bounds.contains(p)
                        && ((p.x >= bounds.x + corner && p.x < bounds.right() - corner)
                            || (p.y >= bounds.y + corner && p.y < bounds.bottom() - corner))
                };
                let (coverage, distance) = if straight(point) && straight(shifted) {
                    let edge = (shifted.x - bounds.x)
                        .min(bounds.right() - 1 - shifted.x)
                        .min(shifted.y - bounds.y)
                        .min(bounds.bottom() - 1 - shifted.y)
                        .max(0);
                    (255u8, edge.min(softness))
                } else {
                    let coverage = rounded_coverage(bounds, radii, point);
                    if coverage == 0 {
                        continue;
                    }
                    (
                        coverage,
                        rounded_edge_distance(bounds, radii, shifted).min(softness),
                    )
                };
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

/// Pixels between `point` and the rounded rectangle (0 inside or on it).
fn outside_distance(bounds: Rect, radii: CornerRadii, point: Point) -> i32 {
    let dx = (bounds.x - point.x)
        .max(point.x - (bounds.right() - 1))
        .max(0);
    let dy = (bounds.y - point.y)
        .max(point.y - (bounds.bottom() - 1))
        .max(0);
    let corners = [
        (
            radii.top_left,
            bounds.x + radii.top_left,
            bounds.y + radii.top_left,
            point.x < bounds.x + radii.top_left && point.y < bounds.y + radii.top_left,
        ),
        (
            radii.top_right,
            bounds.right() - radii.top_right,
            bounds.y + radii.top_right,
            point.x >= bounds.right() - radii.top_right && point.y < bounds.y + radii.top_right,
        ),
        (
            radii.bottom_right,
            bounds.right() - radii.bottom_right,
            bounds.bottom() - radii.bottom_right,
            point.x >= bounds.right() - radii.bottom_right
                && point.y >= bounds.bottom() - radii.bottom_right,
        ),
        (
            radii.bottom_left,
            bounds.x + radii.bottom_left,
            bounds.bottom() - radii.bottom_left,
            point.x < bounds.x + radii.bottom_left
                && point.y >= bounds.bottom() - radii.bottom_left,
        ),
    ];
    for (radius, center_x, center_y, active) in corners {
        if active && radius > 0 {
            let ox = (point.x - center_x) as i64;
            let oy = (point.y - center_y) as i64;
            let radial = integer_sqrt((ox * ox + oy * oy) as u64) as i32;
            return (radial - radius).max(0);
        }
    }
    dx.max(dy)
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
