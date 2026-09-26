use super::*;

pub(super) const POWER_OPEN_NS: u64 = 420_000_000;
pub(super) const POWER_CLOSE_NS: u64 = 260_000_000;
pub(super) const POWER_ACTION_NS: u64 = 2_600_000_000;
pub(super) const POWER_PILLS: [&str; 3] = ["Terminal", "Task manager", "Lock"];
pub(super) const POWER_FLYOUT: [&str; 3] = ["Restart", "Shutdown", "Sleep"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PowerAction {
    None,
    Restart,
    Shutdown,
    Sleep,
}

pub(super) fn pill_rect(index: usize) -> Rect {
    Rect::new(229, 46 + index as i32 * 104, 294, 76)
}

pub(super) fn power_button_rect() -> Rect {
    Rect::new(640, 384, 62, 62)
}

pub(super) fn flyout_panel() -> Rect {
    Rect::new(588, 254, 116, 122)
}

pub(super) fn flyout_rect(index: usize) -> Rect {
    Rect::new(596, 262 + index as i32 * 36, 100, 30)
}

impl DesktopState {
    pub(super) fn power_visible(&self, now: u64) -> bool {
        self.power_open || now < self.power_at_ns.saturating_add(POWER_CLOSE_NS)
    }

    /// 0 = gone, 1000 = fully shown, easing both ways.
    pub(super) fn power_progress(&self, now: u64) -> i32 {
        if self.power_open {
            ease_out_milli(progress_of(now, self.power_at_ns, POWER_OPEN_NS)) as i32
        } else if now < self.power_at_ns.saturating_add(POWER_CLOSE_NS) {
            1000 - ease_out_milli(progress_of(now, self.power_at_ns, POWER_CLOSE_NS)) as i32
        } else {
            0
        }
    }

    pub(super) fn open_power_menu(&mut self) {
        if self.power_open || self.screen != Screen::Desktop {
            return;
        }
        let now = crate::time::monotonic_nanoseconds();
        self.set_overlay(Overlay::None);
        self.clip_open = false;
        self.power_open = true;
        self.power_at_ns = now;
        self.power_focus = 0;
        self.power_flyout = false;
        self.motion_until_ns = self.motion_until_ns.max(now + POWER_OPEN_NS + 80_000_000);
    }

    pub(super) fn close_power_menu(&mut self) {
        if !self.power_open {
            return;
        }
        let now = crate::time::monotonic_nanoseconds();
        self.power_open = false;
        self.power_flyout = false;
        self.power_at_ns = now;
        self.motion_until_ns = self.motion_until_ns.max(now + POWER_CLOSE_NS + 60_000_000);
    }

    pub(super) fn start_power_action(&mut self, action: PowerAction) {
        let now = crate::time::monotonic_nanoseconds();
        self.power_open = false;
        self.power_flyout = false;
        self.power_at_ns = 0;
        self.power_action = action;
        self.power_action_at_ns = now;
        self.motion_until_ns = self
            .motion_until_ns
            .max(now + POWER_ACTION_NS + 200_000_000);
    }

    fn power_activate(&mut self, slot: usize) -> DesktopAction {
        let now = crate::time::monotonic_nanoseconds();
        match slot {
            0 => {
                self.close_power_menu();
                self.set_app(DesktopApp::Terminal);
            }
            1 => {
                self.close_power_menu();
                self.set_app(DesktopApp::Terminal);
                let command = b"ps";
                self.terminal_input = [0; SHELL_LINE_MAX];
                self.terminal_input[..command.len()].copy_from_slice(command);
                self.terminal_input_len = command.len();
            }
            2 => {
                self.close_power_menu();
                self.lock_session();
            }
            3 => {
                self.power_flyout = !self.power_flyout;
                self.power_flyout_ns = now;
                self.motion_until_ns = self.motion_until_ns.max(now + 420_000_000);
                if self.power_flyout {
                    self.power_focus = 4;
                }
            }
            4 => self.start_power_action(PowerAction::Restart),
            5 => self.start_power_action(PowerAction::Shutdown),
            _ => self.start_power_action(PowerAction::Sleep),
        }
        DesktopAction::Redraw
    }

    /// Keys while the power menu is up. Returns None when the key is not
    /// for the menu.
    pub(super) fn power_key(&mut self, key: DesktopKey) -> Option<DesktopAction> {
        if self.power_action != PowerAction::None {
            if self.power_action == PowerAction::Sleep {
                self.power_action = PowerAction::None;
                self.lock_session();
                return Some(DesktopAction::Redraw);
            }
            return Some(DesktopAction::Idle);
        }
        if !self.power_open {
            return None;
        }
        let count = if self.power_flyout { 7 } else { 4 };
        let now = crate::time::monotonic_nanoseconds();
        let step = |focus: usize, forward: bool| -> usize {
            if focus >= 4 {
                if forward {
                    if focus == 6 { 4 } else { focus + 1 }
                } else if focus == 4 {
                    6
                } else {
                    focus - 1
                }
            } else if forward {
                (focus + 1) % 4
            } else {
                (focus + 3) % 4
            }
        };
        let _ = count;
        match key {
            DesktopKey::Escape => {
                if self.power_flyout {
                    self.power_flyout = false;
                    self.power_flyout_ns = now;
                    self.power_focus = 3;
                    self.motion_until_ns = self.motion_until_ns.max(now + 420_000_000);
                } else {
                    self.close_power_menu();
                }
            }
            DesktopKey::Down | DesktopKey::Tab | DesktopKey::Right => {
                self.power_focus = step(self.power_focus, true);
            }
            DesktopKey::Up | DesktopKey::Left => {
                self.power_focus = step(self.power_focus, false);
            }
            DesktopKey::Activate => {
                let slot = self.power_focus;
                return Some(self.power_activate(slot));
            }
            DesktopKey::Power => self.close_power_menu(),
            _ => return Some(DesktopAction::Idle),
        }
        Some(DesktopAction::Redraw)
    }

    pub(super) fn power_click(&mut self, point: Point) -> Option<DesktopAction> {
        if self.power_action != PowerAction::None {
            if self.power_action == PowerAction::Sleep {
                self.power_action = PowerAction::None;
                self.lock_session();
                return Some(DesktopAction::Redraw);
            }
            return Some(DesktopAction::Idle);
        }
        if !self.power_open {
            return None;
        }
        if self.power_flyout {
            for index in 0..3 {
                if flyout_rect(index).contains(point) {
                    return Some(self.power_activate(4 + index));
                }
            }
        }
        if power_button_rect().contains(point) {
            return Some(self.power_activate(3));
        }
        for index in 0..3 {
            if pill_rect(index).contains(point) {
                return Some(self.power_activate(index));
            }
        }
        if self.power_flyout && !flyout_panel().contains(point) {
            self.power_flyout = false;
            self.power_flyout_ns = crate::time::monotonic_nanoseconds();
            return Some(DesktopAction::Redraw);
        }
        if !self.power_flyout {
            self.close_power_menu();
        }
        Some(DesktopAction::Redraw)
    }

    /// Runs the chosen power action once its screen has been up long enough.
    pub(super) fn power_due(&self, now: u64) -> Option<PowerAction> {
        match self.power_action {
            PowerAction::Restart | PowerAction::Shutdown
                if now >= self.power_action_at_ns + POWER_ACTION_NS =>
            {
                Some(self.power_action)
            }
            _ => None,
        }
    }
}

fn veil_style(progress: i32) -> FrostStyle {
    FrostStyle {
        blur_radius: (24 * progress / 1000).clamp(1, 24) as u8,
        saturation_percent: 100 + (12 * progress / 1000) as u16,
        brightness_percent: 100 - (6 * progress / 1000) as u16,
        tint: Rgba::new(28, 40, 52, (58 * progress / 1000) as u8),
        border: Rgba::transparent(),
        border_width: 0,
        inner_highlight: Rgba::transparent(),
        inner_shadow: Rgba::new(0, 0, 0, (60 * progress / 1000) as u8),
        inner_shadow_offset: Point::new(0, 0),
        inner_shadow_softness: 26,
        shadow: Rgba::transparent(),
        shadow_offset: Point::new(0, 0),
        shadow_spread: 0,
        shadow_softness: 0,
        noise_alpha: 4,
    }
}

#[allow(clippy::too_many_arguments)]
fn menu_pill(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    bounds: Rect,
    label: &str,
    focused: bool,
    now: u64,
    seed: u32,
) {
    let radius = CornerRadii::all(bounds.height / 2);
    let mut style = setup_field_style();
    style.tint = Rgba::new(210, 226, 236, 56);
    style.border = Rgba::new(255, 255, 255, 120);
    style.inner_shadow = Rgba::new(0, 0, 0, 34);
    let _ = frost_mode(painter, layout, bounds, radius, style, seed, true);
    painter.fill_rounded_rect(
        layout.rect(bounds),
        layout.radii(radius),
        Rgba::new(255, 255, 255, 30),
    );
    if focused {
        let pulse = 90 + (wave_milli(now / 5_000_000) * 40 / 1000);
        painter.fill_rounded_rect(
            layout.rect(bounds),
            layout.radii(radius),
            Rgba::new(255, 255, 255, 46),
        );
        painter.stroke_rounded_rect(
            layout.rect(bounds.expand(3)),
            layout.radii(CornerRadii::all(bounds.height / 2 + 3)),
            2,
            Rgba::new(255, 255, 255, (pulse + 60).clamp(0, 255) as u8),
        );
    }
    centered_text(
        painter,
        layout,
        font,
        bounds,
        label,
        bounds.height * 36 / 100,
        Color::rgb(12, 18, 24),
    );
}

fn power_icon(painter: &mut Painter<'_>, layout: Layout, bounds: Rect, ink: Rgba, base: Rgba) {
    let cx = bounds.x + bounds.width / 2;
    let cy = bounds.y + bounds.height / 2;
    let outer = bounds.width * 30 / 100;
    let ring = Rect::new(cx - outer, cy - outer + 2, outer * 2, outer * 2);
    painter.stroke_rounded_rect(
        layout.rect(ring),
        layout.radii(CornerRadii::all(outer)),
        (layout.scale.logical(5)).clamp(2, 12) as u8,
        ink,
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(cx - 9, ring.y - 3, 18, outer / 2 + 2)),
        layout.radii(CornerRadii::all(0)),
        base,
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(cx - 2, cy - outer - 4, 5, outer + 4)),
        layout.radii(CornerRadii::all(2)),
        ink,
    );
}

pub(super) fn draw_power_menu(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    screen: Rect,
    state: &DesktopState,
    now: u64,
) -> bool {
    let progress = state.power_progress(now);
    if progress <= 0 {
        return true;
    }
    let settled = progress >= 1000 && now >= state.motion_until_ns;
    let cache = unsafe { &mut *PANEL_CACHE.0.get() };
    let cache_len = frame_pixels(screen);
    let key = dock_cache_key(layout, screen) ^ 0x70e1_0000 ^ ((state.app as u64 + 1) << 40);
    let mut cached = false;
    if settled
        && cache_len <= PANEL_CACHE_PIXELS
        && PANEL_CACHE_KEY.load(Ordering::Acquire) == key
        && painter.write_region(screen, &cache[..cache_len])
    {
        cached = true;
    }
    let mut ok = true;
    if !cached {
        let step = if settled { 3 } else { 5 };
        ok &= painter
            .frosted_rounded_rect_stepped(
                screen,
                CornerRadii::all(0),
                layout.frost(veil_style(progress)),
                0xae70_0001,
                step,
            )
            .captured;
        if settled
            && cache_len <= PANEL_CACHE_PIXELS
            && painter.read_region(screen, &mut cache[..cache_len])
        {
            PANEL_CACHE_KEY.store(key, Ordering::Release);
        }
    }
    let age = if state.power_open {
        now.saturating_sub(state.power_at_ns)
    } else {
        u64::MAX / 4
    };
    for (index, label) in POWER_PILLS.iter().enumerate() {
        let delay = index as u64 * 70_000_000;
        let arrive = ease_out_back_milli(progress_of(age.saturating_sub(delay), 0, 520_000_000));
        let lift = (1000 - arrive.min(1000)) * 46 / 1000;
        let mut bounds = pill_rect(index);
        bounds.y += lift + (1000 - progress) * 14 / 1000;
        if arrive <= 0 || progress < 60 {
            continue;
        }
        let squeeze = (1000 - arrive.clamp(0, 1000)) * 24 / 1000;
        bounds = Rect::new(
            bounds.x + squeeze,
            bounds.y,
            bounds.width - squeeze * 2,
            bounds.height,
        );
        menu_pill(
            painter,
            layout,
            font,
            bounds,
            label,
            state.power_focus == index,
            now,
            0xae70_0010 + index as u32,
        );
    }
    let button = power_button_rect();
    let button_pop =
        ease_out_back_milli(progress_of(age.saturating_sub(260_000_000), 0, 520_000_000)).max(0);
    let size = (button.width * button_pop / 1000).max(4);
    let shown = Rect::new(
        button.x + (button.width - size) / 2,
        button.y + (button.height - size) / 2,
        size,
        size,
    );
    let base = Rgba::new(64, 74, 84, 255);
    painter.fill_rounded_rect(
        layout.rect(shown.expand(2)),
        layout.radii(CornerRadii::all(size / 2 + 2)),
        Rgba::new(0, 0, 0, 40),
    );
    painter.fill_rounded_rect(
        layout.rect(shown),
        layout.radii(CornerRadii::all(size / 2)),
        base,
    );
    if state.power_focus == 3 || state.power_flyout {
        painter.stroke_rounded_rect(
            layout.rect(shown.expand(3)),
            layout.radii(CornerRadii::all(size / 2 + 3)),
            2,
            Rgba::new(255, 255, 255, 190),
        );
    }
    if size > 30 {
        power_icon(painter, layout, shown, Rgba::new(232, 236, 240, 255), base);
    }
    let fly = if state.power_flyout {
        ease_out_back_milli(progress_of(now, state.power_flyout_ns, 420_000_000))
    } else {
        1000 - ease_out_milli(progress_of(now, state.power_flyout_ns, 260_000_000)) as i32
    };
    if fly > 30 && state.power_progress(now) > 0 {
        let panel = flyout_panel();
        let scale = fly.clamp(0, 1150);
        let width = panel.width * scale / 1000;
        let height = panel.height * scale / 1000;
        let shown = Rect::new(
            panel.right() - width,
            panel.bottom() - height,
            width,
            height,
        );
        let mut style = setup_card_style();
        style.tint = Rgba::new(214, 228, 238, 92);
        style.border = Rgba::new(255, 255, 255, 90);
        style.border_width = 1;
        let _ = frost_mode(
            painter,
            layout,
            shown,
            CornerRadii::all(22),
            style,
            0xae70_0020,
            true,
        );
        if fly > 700 {
            for (index, label) in POWER_FLYOUT.iter().enumerate() {
                let rect = flyout_rect(index);
                let focused = state.power_focus == 4 + index;
                painter.fill_rounded_rect(
                    layout.rect(rect),
                    layout.radii(CornerRadii::all(15)),
                    if focused {
                        Rgba::new(255, 255, 255, 96)
                    } else {
                        Rgba::new(255, 255, 255, 60)
                    },
                );
                painter.stroke_rounded_rect(
                    layout.rect(rect),
                    layout.radii(CornerRadii::all(15)),
                    1,
                    Rgba::new(10, 16, 22, if focused { 230 } else { 150 }),
                );
                centered_text(
                    painter,
                    layout,
                    font,
                    rect,
                    label,
                    12,
                    Color::rgb(10, 16, 22),
                );
            }
        }
    }
    ok
}

/// The dark screen shown while the machine restarts or shuts down (or, for
/// sleep, just black until a key is pressed).
pub(super) fn draw_power_action(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    screen: Rect,
    state: &DesktopState,
    now: u64,
) {
    let elapsed = now.saturating_sub(state.power_action_at_ns);
    let alpha = (elapsed * 255 / 520_000_000).min(255) as u8;
    match state.power_action {
        PowerAction::Sleep => {
            painter.fill_rounded_rect(screen, CornerRadii::all(0), Rgba::new(0, 0, 0, alpha));
        }
        PowerAction::Restart => flow::draw_boot_screen(
            painter,
            layout,
            font,
            screen,
            elapsed / 1_000_000,
            None,
            "Restarting...",
            alpha,
        ),
        PowerAction::Shutdown => flow::draw_boot_screen(
            painter,
            layout,
            font,
            screen,
            elapsed / 1_000_000,
            None,
            "Shutting down...",
            alpha,
        ),
        PowerAction::None => {}
    }
}

/// The screen shown after too many wrong passwords: the wallpaper melts to
/// a blur around a slowly breathing dark disc.
pub(super) fn draw_lockout(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    screen: Rect,
    seconds: u64,
    now: u64,
    entered_ns: u64,
) {
    let entered = now.saturating_sub(entered_ns);
    let appear = ease_out_milli(progress_of(entered, 0, 700_000_000)) as i32;
    let _ = painter.frosted_rounded_rect_stepped(
        screen,
        CornerRadii::all(0),
        layout.frost(veil_style(appear)),
        0xae71_0001,
        if entered < 900_000_000 { 5 } else { 3 },
    );
    let breathe = wave_milli(now / 5_500_000);
    let radius = 46 + breathe * 5 / 1000;
    let center = Point::new(376, 236);
    for layer in 0..9i32 {
        let r = radius + 30 - layer * 4;
        let alpha = (10 + layer * 8) * appear / 1000;
        painter.fill_rounded_rect(
            layout.rect(Rect::new(center.x - r, center.y - r, r * 2, r * 2)),
            layout.radii(CornerRadii::all(r)),
            Rgba::new(6, 10, 14, alpha.clamp(0, 255) as u8),
        );
    }
    centered_text(
        painter,
        layout,
        font,
        Rect::new(0, 62 - (1000 - appear) * 16 / 1000, DESIGN_WIDTH, 44),
        "Too many wrong password attempts",
        30,
        Color::rgb(16, 22, 30),
    );
    let mut line = ClockBuffer::new();
    if seconds >= 90 {
        let _ = write!(line, "Disabled for {} minutes", seconds.div_ceil(60));
    } else if seconds == 1 {
        let _ = write!(line, "Disabled for 1 second");
    } else {
        let _ = write!(line, "Disabled for {} seconds", seconds);
    }
    centered_text(
        painter,
        layout,
        font,
        Rect::new(0, 358 + (1000 - appear) * 16 / 1000, DESIGN_WIDTH, 40),
        line.as_str(),
        26,
        Color::rgb(16, 22, 30),
    );
}
