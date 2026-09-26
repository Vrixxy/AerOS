use super::*;

// ---------------------------------------------------------------- icons

fn isqrt(value: i64) -> i64 {
    if value <= 0 {
        return 0;
    }
    let mut low = 0i64;
    let mut high = 1i64 << 31;
    while low < high {
        let mid = (low + high + 1) / 2;
        if mid * mid <= value {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}

fn segment_d2(u: i32, v: i32, a: (i32, i32), b: (i32, i32)) -> i64 {
    let (dx, dy) = ((b.0 - a.0) as i64, (b.1 - a.1) as i64);
    let (px, py) = ((u - a.0) as i64, (v - a.1) as i64);
    let length = dx * dx + dy * dy;
    let t = if length == 0 {
        0
    } else {
        ((px * dx + py * dy) * 1024 / length).clamp(0, 1024)
    };
    let (ex, ey) = (px - dx * t / 1024, py - dy * t / 1024);
    ex * ex + ey * ey
}

fn near_segments(u: i32, v: i32, points: &[(i32, i32)], radius: i64) -> bool {
    points
        .windows(2)
        .any(|pair| segment_d2(u, v, pair[0], pair[1]) <= radius * radius)
}

/// Paints a shape given as a point test in -1000..=1000 icon space, with
/// 2x2 supersampling, into a square `half` logical pixels each way.
fn paint_shape(
    painter: &mut Painter<'_>,
    layout: Layout,
    center: Point,
    half: i32,
    ink: Rgba,
    inside: &dyn Fn(i32, i32) -> bool,
) {
    let middle = layout.point(center);
    let extent = layout.scale.logical(half).max(4);
    for y in middle.y - extent..middle.y + extent {
        for x in middle.x - extent..middle.x + extent {
            let mut hits = 0u32;
            for sy in 0..2 {
                for sx in 0..2 {
                    let u = ((x - middle.x) * 2 + sx) * 1000 / (2 * extent);
                    let v = ((y - middle.y) * 2 + sy) * 1000 / (2 * extent);
                    if inside(u, v) {
                        hits += 1;
                    }
                }
            }
            if hits > 0 {
                painter.blend_pixel(x, y, ink.color(), (ink.alpha as u32 * hits / 4) as u8);
            }
        }
    }
}

fn arc_hit(dx: i32, dy: i32, radius: i32, thickness: i32) -> bool {
    let distance = isqrt(dx as i64 * dx as i64 + dy as i64 * dy as i64) as i32;
    (distance - radius).abs() <= thickness / 2
}

pub(super) fn icon_wifi(
    painter: &mut Painter<'_>,
    layout: Layout,
    center: Point,
    half: i32,
    ink: Rgba,
) {
    paint_shape(painter, layout, center, half, ink, &|u, v| {
        let (dx, dy) = (u, v - 640);
        if dx * dx + dy * dy <= 120 * 120 {
            return true;
        }
        dy < -60
            && dx.abs() <= -dy * 12 / 10
            && (arc_hit(dx, dy, 360, 130)
                || arc_hit(dx, dy, 690, 130)
                || arc_hit(dx, dy, 1020, 130))
    });
}

pub(super) fn icon_bluetooth(
    painter: &mut Painter<'_>,
    layout: Layout,
    center: Point,
    half: i32,
    ink: Rgba,
) {
    const RUNE: [(i32, i32); 6] = [
        (-430, -350),
        (430, 350),
        (0, 800),
        (0, -800),
        (430, -350),
        (-430, 350),
    ];
    paint_shape(painter, layout, center, half, ink, &|u, v| {
        near_segments(u, v, &RUNE, 62)
    });
}

pub(super) fn icon_antenna(
    painter: &mut Painter<'_>,
    layout: Layout,
    center: Point,
    half: i32,
    ink: Rgba,
    slashed: bool,
) {
    paint_shape(painter, layout, center, half, ink, &|u, v| {
        if u * u + v * v <= 130 * 130 {
            return true;
        }
        if v.abs() <= u.abs() * 15 / 10 && (arc_hit(u, v, 400, 120) || arc_hit(u, v, 760, 120)) {
            return true;
        }
        slashed && segment_d2(u, v, (-820, -820), (820, 820)) <= 62 * 62
    });
}

pub(super) fn icon_battery_body(
    painter: &mut Painter<'_>,
    layout: Layout,
    center: Point,
    half: i32,
    ink: Rgba,
) {
    paint_shape(painter, layout, center, half, ink, &|u, v| {
        let outer = (-900..=700).contains(&u) && v.abs() <= 430;
        let inner = (-780..=580).contains(&u) && v.abs() <= 310;
        let nub = u > 700 && u <= 920 && v.abs() <= 170;
        (outer && !inner) || nub
    });
}

pub(super) fn icon_cross(
    painter: &mut Painter<'_>,
    layout: Layout,
    center: Point,
    half: i32,
    ink: Rgba,
) {
    paint_shape(painter, layout, center, half, ink, &|u, v| {
        segment_d2(u, v, (-720, -720), (720, 720)) <= 170 * 170
            || segment_d2(u, v, (-720, 720), (720, -720)) <= 170 * 170
    });
}

pub(super) fn icon_sun(
    painter: &mut Painter<'_>,
    layout: Layout,
    center: Point,
    half: i32,
    ink: Rgba,
) {
    const DIRECTIONS: [(i32, i32); 8] = [
        (1000, 0),
        (707, 707),
        (0, 1000),
        (-707, 707),
        (-1000, 0),
        (-707, -707),
        (0, -1000),
        (707, -707),
    ];
    paint_shape(painter, layout, center, half, ink, &|u, v| {
        if u * u + v * v <= 330 * 330 {
            return true;
        }
        DIRECTIONS.iter().any(|&(dx, dy)| {
            let a = (dx * 560 / 1000, dy * 560 / 1000);
            let b = (dx * 900 / 1000, dy * 900 / 1000);
            segment_d2(u, v, a, b) <= 70 * 70
        })
    });
}

pub(super) fn icon_speaker(
    painter: &mut Painter<'_>,
    layout: Layout,
    center: Point,
    half: i32,
    ink: Rgba,
    level: i32,
    muted: bool,
) {
    paint_shape(painter, layout, center, half, ink, &|u, v| {
        if (-820..=-420).contains(&u) && v.abs() <= 260 {
            return true;
        }
        if (-420..=120).contains(&u) && v.abs() <= 260 + (u + 420) * 420 / 540 {
            return true;
        }
        if muted {
            return segment_d2(u, v, (240, -300), (900, 300)) <= 70 * 70
                || segment_d2(u, v, (240, 300), (900, -300)) <= 70 * 70;
        }
        let (dx, dy) = (u - 160, v);
        dx > 60
            && dy.abs() <= dx
            && ((level > 0 && arc_hit(dx, dy, 380, 110))
                || (level > 40 && arc_hit(dx, dy, 700, 110))
                || (level > 75 && arc_hit(dx, dy, 1020, 110)))
    });
}

// ------------------------------------------------------- control center

pub(super) const QUICK_PANEL: Rect = Rect::new(356, 20, 284, 282);
pub(super) const BRIGHTNESS_TRACK: Rect = Rect::new(372, 194, 252, 26);
pub(super) const VOLUME_TRACK: Rect = Rect::new(372, 258, 252, 26);

pub(super) fn quick_pill(index: usize) -> Rect {
    Rect::new(
        372 + (index as i32 % 2) * 134,
        42 + (index as i32 / 2) * 68,
        118,
        54,
    )
}

fn shown_toggle(state: &DesktopState, index: usize, on: bool, now: u64) -> i32 {
    let t = ease_out_milli(progress_of(now, state.quick_toggle_ns[index], 300_000_000)) as i32;
    if on { t } else { 1000 - t }
}

pub(super) fn draw_quick(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now: u64,
) -> bool {
    let captured = frost_mode(
        painter,
        layout,
        QUICK_PANEL,
        CornerRadii::all(30),
        dock_style(),
        0xae91_0001,
        false,
    )
    .captured;
    let toggles = [
        state.wifi,
        state.bluetooth,
        state.microphone,
        state.battery_saver,
    ];
    for (index, on) in toggles.into_iter().enumerate() {
        let t = shown_toggle(state, index, on, now);
        let age = now.saturating_sub(state.quick_toggle_ns[index]);
        let pop = if age < 320_000_000 {
            wave_milli(age / 640_000 + 250).max(0) * 7 / 1000
        } else {
            0
        };
        let base = quick_pill(index);
        let bounds = Rect::new(
            base.x - pop,
            base.y - pop / 2,
            base.width + pop * 2,
            base.height + pop,
        );
        let radii = CornerRadii::all(bounds.height / 2);
        let fill = mix_color(Color::rgb(160, 178, 186), Color::rgb(238, 248, 252), t);
        painter.fill_rounded_rect(
            layout.rect(bounds),
            layout.radii(radii),
            Rgba::new(fill.red, fill.green, fill.blue, (78 + t * 90 / 1000) as u8),
        );
        painter.stroke_rounded_rect(
            layout.rect(bounds),
            layout.radii(radii),
            1,
            Rgba::new(255, 255, 255, (70 + t * 90 / 1000) as u8),
        );
        if state.focus_visible && state.quick_focus == index {
            painter.stroke_rounded_rect(
                layout.rect(bounds.expand(3)),
                layout.radii(CornerRadii::all(bounds.height / 2 + 3)),
                2,
                Rgba::new(
                    255,
                    255,
                    255,
                    (170 + wave_milli(now / 4_000_000) * 60 / 1000) as u8,
                ),
            );
        }
        let dark = Color::rgb(14, 20, 26);
        let blue = Color::rgb(36, 98, 232);
        let ink = if index == 3 {
            dark
        } else {
            mix_color(dark, blue, t)
        };
        let ink = Rgba::new(ink.red, ink.green, ink.blue, 255);
        let center = Point::new(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
        match index {
            0 => icon_wifi(painter, layout, center, 15, ink),
            1 => icon_bluetooth(painter, layout, center, 15, ink),
            2 => icon_antenna(painter, layout, center, 16, ink, t < 500),
            _ => {
                icon_battery_body(painter, layout, center, 18, ink);
                let leaf_w = 17;
                let leaf = layout.rect(Rect::new(
                    center.x - 13,
                    center.y - crate::logo::height_for(leaf_w) / 2,
                    leaf_w,
                    crate::logo::height_for(leaf_w),
                ));
                painter.draw_rgba_scaled_alpha(
                    leaf,
                    crate::logo::green_rgba(),
                    crate::logo::WIDTH,
                    crate::logo::HEIGHT,
                    (120 + t * 135 / 1000) as u8,
                );
            }
        }
    }
    let tracks = [
        (
            "Brightness",
            BRIGHTNESS_TRACK,
            state.shown_brightness(now) as i32,
            4usize,
        ),
        (
            "Volume",
            VOLUME_TRACK,
            state.shown_volume(now) as i32,
            5usize,
        ),
    ];
    for (label, track, level, slot) in tracks {
        text(
            painter,
            layout,
            font,
            Point::new(track.x + 2, track.y - 17),
            label,
            11,
            Color::rgb(20, 36, 44),
        );
        let radii = CornerRadii::all(track.height / 2);
        painter.fill_rounded_rect(
            layout.rect(track),
            layout.radii(radii),
            Rgba::new(255, 255, 255, 70),
        );
        painter.stroke_rounded_rect(
            layout.rect(track),
            layout.radii(radii),
            1,
            Rgba::new(255, 255, 255, 110),
        );
        let thumb = track.height + 2;
        let travel = track.width - thumb;
        let x = track.x + travel * level.clamp(0, 100) / 100;
        painter.fill_rounded_rect(
            layout.rect(Rect::new(
                track.x + 3,
                track.y + 3,
                (x - track.x) + thumb / 2,
                track.height - 6,
            )),
            layout.radii(CornerRadii::all((track.height - 6) / 2)),
            Rgba::new(255, 255, 255, 96),
        );
        painter.fill_rounded_rect(
            layout.rect(Rect::new(x - 1, track.y + 3, thumb, thumb)),
            layout.radii(CornerRadii::all(thumb / 2)),
            Rgba::new(0, 0, 0, 40),
        );
        painter.fill_rounded_rect(
            layout.rect(Rect::new(x, track.y - 1, thumb, thumb)),
            layout.radii(CornerRadii::all(thumb / 2)),
            Rgba::new(246, 249, 251, 250),
        );
        if state.focus_visible && state.quick_focus == slot {
            painter.stroke_rounded_rect(
                layout.rect(track.expand(3)),
                layout.radii(CornerRadii::all(track.height / 2 + 3)),
                2,
                Rgba::new(255, 255, 255, 200),
            );
        }
    }
    captured
}

impl DesktopState {
    pub(super) fn shown_brightness(&self, now: u64) -> u8 {
        let t = ease_out_milli(progress_of(now, self.brightness_event_ns, 260_000_000));
        lerp_i32(
            self.brightness_prev as i32,
            self.brightness as i32,
            t as i32,
        )
        .clamp(0, 100) as u8
    }

    pub(super) fn brightness_step(&mut self, delta: i32) {
        let now = crate::time::monotonic_nanoseconds();
        self.set_brightness((self.brightness as i32 + delta).clamp(8, 100) as u8, now);
    }

    pub(super) fn set_brightness(&mut self, value: u8, now: u64) {
        self.brightness_prev = self.shown_brightness(now);
        self.brightness = value.clamp(8, 100);
        crate::framebuffer::set_brightness(self.brightness);
        self.brightness_event_ns = now;
        self.brightness_hud_until_ns = now + VOLUME_HUD_NS;
        self.motion_until_ns = self.motion_until_ns.max(self.brightness_hud_until_ns);
    }

    fn set_volume_to(&mut self, value: u8) {
        let now = crate::time::monotonic_nanoseconds();
        self.volume_prev = self.shown_volume(now);
        self.muted = false;
        self.volume = value.min(100);
        self.apply_volume();
        self.volume_event_ns = now;
        self.volume_hud_until_ns = now + VOLUME_HUD_NS;
        self.motion_until_ns = self.motion_until_ns.max(self.volume_hud_until_ns);
    }

    fn quick_toggle(&mut self, index: usize) {
        let now = crate::time::monotonic_nanoseconds();
        match index {
            0 => self.wifi = !self.wifi,
            1 => self.bluetooth = !self.bluetooth,
            2 => self.microphone = !self.microphone,
            _ => self.battery_saver = !self.battery_saver,
        }
        self.quick_toggle_ns[index] = now;
        self.motion_until_ns = self.motion_until_ns.max(now + 420_000_000);
    }

    pub(super) fn quick_key(&mut self, key: DesktopKey) -> Option<DesktopAction> {
        if self.overlay != Overlay::Quick {
            return None;
        }
        let focus = self.quick_focus;
        self.focus_visible = true;
        match key {
            DesktopKey::Left => {
                if focus == 4 {
                    self.brightness_step(-6);
                } else if focus == 5 {
                    self.set_volume_to((self.volume as i32 - 6).max(0) as u8);
                } else if focus % 2 == 1 {
                    self.quick_focus -= 1;
                }
            }
            DesktopKey::Right => {
                if focus == 4 {
                    self.brightness_step(6);
                } else if focus == 5 {
                    self.set_volume_to((self.volume as i32 + 6).min(100) as u8);
                } else if focus.is_multiple_of(2) {
                    self.quick_focus += 1;
                }
            }
            DesktopKey::Down | DesktopKey::Tab => {
                self.quick_focus = match focus {
                    0 | 1 => focus + 2,
                    2 | 3 => 4,
                    4 => 5,
                    _ => 0,
                };
            }
            DesktopKey::Up => {
                self.quick_focus = match focus {
                    0 | 1 => 5,
                    2 | 3 => focus - 2,
                    4 => 2,
                    _ => 4,
                };
            }
            DesktopKey::Activate => {
                if focus < 4 {
                    self.quick_toggle(focus);
                }
            }
            _ => return None,
        }
        Some(DesktopAction::Redraw)
    }

    /// A click inside the control center panel.
    pub(super) fn quick_click(&mut self, point: Point) -> DesktopAction {
        let now = crate::time::monotonic_nanoseconds();
        for index in 0..4 {
            if quick_pill(index).contains(point) {
                self.quick_focus = index;
                self.quick_toggle(index);
                return DesktopAction::Redraw;
            }
        }
        for (track, slot) in [(BRIGHTNESS_TRACK, 4usize), (VOLUME_TRACK, 5usize)] {
            if track.expand(6).contains(point) {
                let thumb = track.height + 2;
                let value =
                    ((point.x - track.x - thumb / 2) * 100 / (track.width - thumb)).clamp(0, 100);
                self.quick_focus = slot;
                if slot == 4 {
                    self.set_brightness(value.max(8) as u8, now);
                } else {
                    self.set_volume_to(value as u8);
                }
                return DesktopAction::Redraw;
            }
        }
        DesktopAction::Idle
    }
}

// ------------------------------------------------------------------ HUD

#[allow(clippy::too_many_arguments)]
fn hud_bar(
    painter: &mut Painter<'_>,
    layout: Layout,
    left: bool,
    level: i32,
    event_ns: u64,
    until_ns: u64,
    now: u64,
    muted: bool,
) {
    if event_ns == 0 || now >= until_ns + VOLUME_SLIDE_NS {
        return;
    }
    let entering = ease_out_back_milli(progress_of(now, event_ns, VOLUME_SLIDE_NS));
    let leaving = if now > until_ns {
        ease_in_out_milli(progress_of(now, until_ns, VOLUME_SLIDE_NS))
    } else {
        0
    };
    let kick = if now.saturating_sub(event_ns) < 240_000_000 {
        wave_milli(progress_of(now, event_ns, 240_000_000) as u64 * 2)
    } else {
        0
    };
    let slide = ((1000 - entering.min(1000)) + leaving).clamp(0, 1300);
    let width = 54 + kick.abs() / 160;
    let x = if left {
        22 - 96 * slide / 1000 - kick / 300
    } else {
        DESIGN_WIDTH - 22 - width + 96 * slide / 1000 + kick / 300
    };
    let pill = Rect::new(x, 150, width, 150);
    let radii = layout.radii(CornerRadii::all(width / 2));
    painter.fill_rounded_rect(
        layout.rect(Rect::new(
            pill.x - 1,
            pill.y + 4,
            pill.width + 2,
            pill.height + 2,
        )),
        radii,
        Rgba::new(0, 0, 0, 44),
    );
    painter.fill_rounded_rect(layout.rect(pill), radii, Rgba::new(94, 128, 146, 150));
    painter.stroke_rounded_rect(layout.rect(pill), radii, 1, Rgba::new(255, 255, 255, 120));
    let fill_h = (pill.height * level.clamp(0, 100) / 100).max(0);
    if fill_h > 0 {
        let clip = layout.rect(Rect::new(
            pill.x,
            pill.y + pill.height - fill_h,
            pill.width,
            fill_h,
        ));
        if painter.set_clip(clip) {
            painter.fill_rounded_rect(layout.rect(pill), radii, Rgba::new(238, 245, 248, 222));
        }
        painter.reset_clip();
    }
    let ink = if fill_h > 36 {
        Rgba::new(70, 92, 104, 255)
    } else {
        Rgba::new(244, 248, 250, 255)
    };
    let center = Point::new(pill.x + width / 2, pill.y + pill.height - 24);
    if left {
        icon_sun(painter, layout, center, 11, ink);
    } else {
        icon_speaker(painter, layout, center, 12, ink, level, muted || level == 0);
    }
}

pub(super) fn draw_hud(painter: &mut Painter<'_>, layout: Layout, state: &DesktopState, now: u64) {
    hud_bar(
        painter,
        layout,
        true,
        state.shown_brightness(now) as i32,
        state.brightness_event_ns,
        state.brightness_hud_until_ns,
        now,
        false,
    );
    hud_bar(
        painter,
        layout,
        false,
        state.shown_volume(now) as i32,
        state.volume_event_ns,
        state.volume_hud_until_ns,
        now,
        state.muted,
    );
}

// -------------------------------------------------------- notifications

pub(super) const TOAST_NS: u64 = 5_200_000_000;
pub(super) const TOAST_SLIDE_NS: u64 = 520_000_000;
pub(super) const TOAST_RECT: Rect = Rect::new(263, 16, 226, 64);
pub(super) const STACK_FRONT_Y: i32 = 62;
pub(super) const STACK_PEEK: i32 = 22;
const NOTIFY_STACK_NS: u64 = 520_000_000;

/// How much of `text` fits in `width` physical pixels: (byte length, cut).
fn fit_line(font: RasterFont, text: &str, height: i32, width: i32) -> (usize, bool) {
    if font.text_width(text, height) <= width {
        return (text.len(), false);
    }
    let mut end = 0;
    for (index, _) in text.char_indices() {
        if font.text_width(&text[..index], height) > width {
            break;
        }
        end = index;
    }
    (end, true)
}

fn card(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    bounds: Rect,
    item: &crate::notify::Notification,
    full: bool,
) {
    let radii = layout.radii(CornerRadii::all(26));
    painter.fill_rounded_rect(
        layout.rect(Rect::new(
            bounds.x,
            bounds.y + 3,
            bounds.width,
            bounds.height,
        )),
        radii,
        Rgba::new(0, 0, 0, 34),
    );
    painter.fill_rounded_rect(layout.rect(bounds), radii, Rgba::new(219, 221, 223, 252));
    painter.stroke_rounded_rect(layout.rect(bounds), radii, 1, Rgba::new(28, 30, 32, 120));
    painter.fill_rounded_rect(
        layout.rect(Rect::new(bounds.x + 14, bounds.y + 14, 28, 28)),
        layout.radii(CornerRadii::all(9)),
        Rgba::new(8, 8, 9, 255),
    );
    let mut title = ClockBuffer56::new();
    let _ = write!(title, "Notification from {}", item.title());
    text(
        painter,
        layout,
        font,
        Point::new(bounds.x + 52, bounds.y + 15),
        title.as_str(),
        10,
        Color::rgb(22, 24, 26),
    );
    let mut time = ClockBuffer::new();
    let _ = write!(time, "{:02}:{:02}", item.hour, item.minute);
    let unit = layout.scale.logical(1).max(1);
    let time_w = font.text_width(time.as_str(), layout.scale.logical(9)) / unit;
    text(
        painter,
        layout,
        font,
        Point::new(bounds.right() - 16 - time_w, bounds.y + 15),
        time.as_str(),
        9,
        Color::rgb(70, 74, 78),
    );
    if !full {
        return;
    }
    let body = item.body();
    let max_w = (bounds.width - 52 - 16) * unit;
    let (end, cut) = fit_line(font, body, layout.scale.logical(10), max_w);
    let first = if cut {
        body[..end].rfind(' ').unwrap_or(end)
    } else {
        end
    };
    text(
        painter,
        layout,
        font,
        Point::new(bounds.x + 52, bounds.y + 32),
        &body[..first],
        10,
        Color::rgb(36, 38, 40),
    );
    if cut {
        let rest = body[first..].trim_start();
        let (rest_end, rest_cut) =
            fit_line(font, rest, layout.scale.logical(10), max_w - 14 * unit);
        let mut line = ClockBuffer56::new();
        let _ = write!(
            line,
            "{}{}",
            &rest[..rest_end],
            if rest_cut { "..." } else { "" }
        );
        text(
            painter,
            layout,
            font,
            Point::new(bounds.x + 52, bounds.y + 46),
            line.as_str(),
            10,
            Color::rgb(36, 38, 40),
        );
    }
}

impl DesktopState {
    pub(super) fn toast_visible(&self, now: u64) -> bool {
        self.toast_at_ns != 0 && now < self.toast_at_ns + TOAST_NS + TOAST_SLIDE_NS
    }

    pub(super) fn notice_arrived(&mut self, now: u64) {
        self.toast_at_ns = now;
        self.motion_until_ns = self.motion_until_ns.max(now + TOAST_NS + TOAST_SLIDE_NS);
        crate::audio::play_effect(crate::sfx::Effect::Notification);
    }

    pub(super) fn notify_center_visible(&self, now: u64) -> bool {
        self.notif_open || now < self.notif_at_ns.saturating_add(NOTIFY_STACK_NS)
    }

    pub(super) fn toggle_notify_center(&mut self) {
        let now = crate::time::monotonic_nanoseconds();
        self.notif_open = !self.notif_open;
        self.notif_at_ns = now;
        self.toast_at_ns = 0;
        self.motion_until_ns = self.motion_until_ns.max(now + NOTIFY_STACK_NS + 60_000_000);
    }

    /// Keys while the notification stack is up.
    pub(super) fn notify_key(&mut self, key: DesktopKey) -> Option<DesktopAction> {
        if !self.notif_open {
            return None;
        }
        match key {
            DesktopKey::Escape | DesktopKey::Notifications => self.toggle_notify_center(),
            DesktopKey::Backspace => {
                crate::notify::clear();
                self.notif_seen = crate::notify::generation();
                self.toggle_notify_center();
            }
            _ => return None,
        }
        Some(DesktopAction::Redraw)
    }

    pub(super) fn notify_click(&mut self, point: Point) -> Option<DesktopAction> {
        let now = crate::time::monotonic_nanoseconds();
        if self.notif_open {
            let mut items = [crate::notify::Notification::EMPTY; crate::notify::CAPACITY];
            let count = crate::notify::snapshot(&mut items).min(3);
            let hit = (0..count).any(|index| {
                Rect::new(
                    TOAST_RECT.x,
                    STACK_FRONT_Y - STACK_PEEK * index as i32,
                    TOAST_RECT.width,
                    TOAST_RECT.height,
                )
                .contains(point)
            });
            if hit {
                crate::notify::clear();
                self.notif_seen = crate::notify::generation();
            }
            self.toggle_notify_center();
            return Some(DesktopAction::Redraw);
        }
        if self.toast_visible(now) && TOAST_RECT.expand(4).contains(point) {
            self.toggle_notify_center();
            return Some(DesktopAction::Redraw);
        }
        None
    }
}

pub(super) fn draw_toast(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now: u64,
) {
    if !state.toast_visible(now) || state.notif_open {
        return;
    }
    let mut items = [crate::notify::Notification::EMPTY; crate::notify::CAPACITY];
    if crate::notify::snapshot(&mut items) == 0 {
        return;
    }
    let entering = ease_out_back_milli(progress_of(now, state.toast_at_ns, TOAST_SLIDE_NS));
    let leave_at = state.toast_at_ns + TOAST_NS;
    let leaving = if now > leave_at {
        ease_in_out_milli(progress_of(now, leave_at, TOAST_SLIDE_NS))
    } else {
        0
    };
    let rise = (1000 - entering) + leaving;
    let mut bounds = TOAST_RECT;
    bounds.y = TOAST_RECT.y - (TOAST_RECT.y + TOAST_RECT.height + 8) * rise.clamp(0, 1400) / 1000;
    let grow = (entering - 1000).max(0) / 40;
    bounds = Rect::new(
        bounds.x - grow,
        bounds.y,
        bounds.width + grow * 2,
        bounds.height,
    );
    card(painter, layout, font, bounds, &items[0], true);
}

pub(super) fn draw_notify_stack(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now: u64,
) {
    if !state.notify_center_visible(now) {
        return;
    }
    let mut items = [crate::notify::Notification::EMPTY; crate::notify::CAPACITY];
    let count = crate::notify::snapshot(&mut items).min(3);
    let opening = state.notif_open;
    for index in (0..count).rev() {
        let delay = (count - 1 - index) as u64 * 50_000_000;
        let progress = if opening {
            ease_out_back_milli(progress_of(
                now.saturating_sub(delay),
                state.notif_at_ns,
                460_000_000,
            ))
        } else {
            1000 - ease_in_out_milli(progress_of(
                now,
                state.notif_at_ns,
                NOTIFY_STACK_NS - 60_000_000,
            ))
        };
        let rise = 1000 - progress;
        let y = STACK_FRONT_Y - STACK_PEEK * index as i32 - 130 * rise.clamp(-300, 1000) / 1000;
        let bounds = Rect::new(TOAST_RECT.x, y, TOAST_RECT.width, TOAST_RECT.height);
        card(painter, layout, font, bounds, &items[index], index == 0);
    }
    if count == 0 {
        let bounds = Rect::new(TOAST_RECT.x, STACK_FRONT_Y, TOAST_RECT.width, 44);
        let radii = layout.radii(CornerRadii::all(22));
        painter.fill_rounded_rect(layout.rect(bounds), radii, Rgba::new(219, 221, 223, 250));
        centered_text(
            painter,
            layout,
            font,
            bounds,
            "No notifications",
            12,
            Color::rgb(40, 44, 48),
        );
    }
}

// --------------------------------------------------------- context menu

pub(super) const CONTEXT_LABELS: [&str; 4] = [
    "New Folder",
    "New File",
    "Refresh",
    "Open Terminal to this path",
];

pub(super) fn context_panel(origin: Point) -> Rect {
    Rect::new(
        origin.x.clamp(8, DESIGN_WIDTH - 148),
        origin.y.clamp(8, 356 - 176),
        140,
        176,
    )
}

pub(super) fn context_row(panel: Rect, index: usize) -> Rect {
    Rect::new(
        panel.x + 10,
        panel.y + 12 + index as i32 * 40,
        panel.width - 20,
        32,
    )
}

impl DesktopState {
    pub(super) fn open_context_menu(&mut self, at: Point) {
        let now = crate::time::monotonic_nanoseconds();
        self.ctx_open = true;
        self.ctx_at_ns = now;
        self.ctx_origin = at;
        self.ctx_focus = 0;
        self.motion_until_ns = self.motion_until_ns.max(now + 360_000_000);
    }

    fn context_activate(&mut self, index: usize) {
        self.ctx_open = false;
        match index {
            0 => {
                self.open_files();
                self.files_mode = FilesMode::NamingFolder;
                self.name_input.clear();
            }
            1 => {
                self.open_files();
                self.files_new_file();
            }
            2 => {
                self.files_reload();
                self.notes_reload();
            }
            _ => self.set_app(DesktopApp::Terminal),
        }
    }

    fn files_new_file(&mut self) {
        for number in 1..100u32 {
            let mut name: shell::Text<40> = shell::Text::new();
            if number == 1 {
                let _ = name.push_str_checked("New file");
            } else {
                let mut suffix = ClockBuffer::new();
                let _ = write!(suffix, "New file {number}");
                let _ = name.push_str_checked(suffix.as_str());
            }
            let target = self.files_child_path(name.as_str());
            if let Ok(descriptor) = vfs::open_file(target.as_str(), true, true, false, 0o644, true)
            {
                let _ = vfs::close(descriptor);
                self.files_set_status("file created");
                self.files_reload();
                return;
            }
        }
        self.files_set_status("could not create a file here");
    }

    pub(super) fn context_key(&mut self, key: DesktopKey) -> Option<DesktopAction> {
        if !self.ctx_open {
            return None;
        }
        match key {
            DesktopKey::Escape => self.ctx_open = false,
            DesktopKey::Down | DesktopKey::Tab => self.ctx_focus = (self.ctx_focus + 1) % 4,
            DesktopKey::Up => self.ctx_focus = (self.ctx_focus + 3) % 4,
            DesktopKey::Activate => {
                let index = self.ctx_focus;
                self.context_activate(index);
            }
            _ => return None,
        }
        Some(DesktopAction::Redraw)
    }

    pub(super) fn context_click(&mut self, point: Point) -> Option<DesktopAction> {
        if !self.ctx_open {
            return None;
        }
        let panel = context_panel(self.ctx_origin);
        for index in 0..4 {
            if context_row(panel, index).contains(point) {
                self.context_activate(index);
                return Some(DesktopAction::Redraw);
            }
        }
        self.ctx_open = false;
        Some(DesktopAction::Redraw)
    }
}

pub(super) fn draw_context(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now: u64,
) {
    if !state.ctx_open {
        return;
    }
    let full = context_panel(state.ctx_origin);
    let pop = ease_out_back_milli(progress_of(now, state.ctx_at_ns, 300_000_000)).max(0);
    let width = (full.width * pop / 1000).max(12);
    let height = (full.height * pop / 1000).max(12);
    let panel = Rect::new(full.x, full.y, width, height);
    let radii = layout.radii(CornerRadii::all(26));
    painter.fill_rounded_rect(
        layout.rect(Rect::new(panel.x, panel.y + 4, panel.width, panel.height)),
        radii,
        Rgba::new(0, 0, 0, 40),
    );
    painter.fill_rounded_rect(layout.rect(panel), radii, Rgba::new(218, 220, 222, 250));
    painter.stroke_rounded_rect(layout.rect(panel), radii, 1, Rgba::new(28, 30, 32, 110));
    if pop < 800 {
        return;
    }
    for (index, label) in CONTEXT_LABELS.iter().enumerate() {
        let row = context_row(full, index);
        let focused = state.ctx_focus == index;
        let delay = index as u64 * 40_000_000;
        let arrive = ease_out_milli(progress_of(
            now.saturating_sub(delay),
            state.ctx_at_ns + 120_000_000,
            240_000_000,
        )) as i32;
        let inset = (1000 - arrive) * 22 / 1000;
        let shown = Rect::new(row.x + inset, row.y, row.width - inset * 2, row.height);
        painter.fill_rounded_rect(
            layout.rect(shown),
            layout.radii(CornerRadii::all(row.height / 2)),
            if focused {
                Rgba::new(120, 122, 126, 255)
            } else {
                Rgba::new(154, 156, 159, 255)
            },
        );
        if focused {
            painter.stroke_rounded_rect(
                layout.rect(shown.expand(2)),
                layout.radii(CornerRadii::all(row.height / 2 + 2)),
                2,
                Rgba::new(255, 255, 255, 220),
            );
        }
        centered_text(
            painter,
            layout,
            font,
            shown,
            label,
            if index == 3 { 8 } else { 11 },
            if focused {
                Color::rgb(250, 250, 250)
            } else {
                Color::rgb(24, 26, 28)
            },
        );
    }
}

// ------------------------------------------------------------- clipboard

pub(super) fn draw_clipboard_panel(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now: u64,
) {
    let count = crate::clipboard::get().len();
    let pop = ease_out_back_milli(progress_of(now, state.clip_at_ns, 420_000_000)).max(0);
    let full = Rect::new(52, 58, 200, 262);
    let width = (full.width * pop / 1000).max(16);
    let height = (full.height * pop / 1000).max(16);
    let panel = Rect::new(full.x, full.y + (full.height - height) / 2, width, height);
    let radii = layout.radii(CornerRadii::all(40));
    painter.fill_rounded_rect(
        layout.rect(Rect::new(panel.x, panel.y + 4, panel.width, panel.height)),
        radii,
        Rgba::new(0, 0, 0, 40),
    );
    painter.fill_rounded_rect(layout.rect(panel), radii, Rgba::new(218, 220, 222, 252));
    painter.stroke_rounded_rect(layout.rect(panel), radii, 1, Rgba::new(28, 30, 32, 110));
    if pop < 750 {
        return;
    }
    text(
        painter,
        layout,
        font,
        Point::new(full.x + 30, full.y + 18),
        "Clipboard",
        12,
        Color::rgb(24, 26, 28),
    );
    let visible = 4usize;
    let first = if state.clip_selected >= visible {
        state.clip_selected + 1 - visible
    } else {
        0
    };
    if count == 0 {
        centered_text(
            painter,
            layout,
            font,
            Rect::new(full.x, full.y + 60, full.width, 30),
            "Nothing copied yet",
            12,
            Color::rgb(70, 74, 78),
        );
    }
    for slot in 0..visible {
        let index = first + slot;
        if index >= count {
            break;
        }
        let delay = slot as u64 * 45_000_000;
        let arrive = ease_out_milli(progress_of(
            now.saturating_sub(delay),
            state.clip_at_ns + 160_000_000,
            240_000_000,
        )) as i32;
        let row = Rect::new(
            full.x + 18 + (1000 - arrive) * 26 / 1000,
            full.y + 46 + slot as i32 * 50,
            full.width - 60,
            42,
        );
        let selected = index == state.clip_selected;
        painter.fill_rounded_rect(
            layout.rect(row),
            layout.radii(CornerRadii::all(21)),
            if selected {
                Rgba::new(118, 120, 124, 255)
            } else {
                Rgba::new(154, 156, 159, 255)
            },
        );
        if selected {
            painter.stroke_rounded_rect(
                layout.rect(row.expand(2)),
                layout.radii(CornerRadii::all(23)),
                2,
                Rgba::new(255, 255, 255, 230),
            );
        }
        let mut label = ClockBuffer56::new();
        for &byte in crate::clipboard::get().entry(index) {
            if label.len >= 22 {
                break;
            }
            let ch = if byte.is_ascii_graphic() || byte == b' ' {
                byte
            } else if byte == b'\n' || byte == b'\r' || byte == b'\t' {
                b' '
            } else {
                continue;
            };
            let _ = label.write_str(core::str::from_utf8(&[ch]).unwrap_or(" "));
        }
        centered_text(
            painter,
            layout,
            font,
            row,
            label.as_str(),
            11,
            if selected {
                Color::rgb(252, 252, 252)
            } else {
                Color::rgb(24, 26, 28)
            },
        );
    }
    if count > visible {
        let track = Rect::new(full.x + full.width - 32, full.y + 50, 12, 190);
        painter.fill_rounded_rect(
            layout.rect(track),
            layout.radii(CornerRadii::all(6)),
            Rgba::new(190, 192, 194, 255),
        );
        let thumb_h = (track.height * visible as i32 / count as i32).max(30);
        let travel = track.height - thumb_h;
        let y = track.y + travel * first as i32 / (count - visible).max(1) as i32;
        painter.fill_rounded_rect(
            layout.rect(Rect::new(track.x, y, 12, thumb_h)),
            layout.radii(CornerRadii::all(6)),
            Rgba::new(252, 252, 252, 255),
        );
    }
}
