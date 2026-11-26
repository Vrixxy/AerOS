use super::*;

pub(super) const SEARCH_MAX: usize = 24;
const OPEN_NS: u64 = 300_000_000;
const CLOSE_NS: u64 = 240_000_000;
pub(super) const SEARCH_PILL: Rect = Rect::new(166, 64, 420, 60);
const ROW_HEIGHT: i32 = 34;

/// One Spotlight result.
#[derive(Clone, Copy)]
enum Hit {
    /// The typed text is arithmetic; the value in millionths.
    Calc(i64),
    Action(usize),
    App(usize),
    Setting(u8),
    Web,
}

/// System actions Spotlight can run: the label (off, on) and the words that
/// find it.
const ACTIONS: [(&str, &str, &str); 13] = [
    ("Lock screen", "Lock screen", "lock"),
    ("Restart", "Restart", "restart reboot"),
    ("Shut down", "Shut down", "shut down shutdown power off turn off"),
    ("Sleep", "Sleep", "sleep suspend"),
    ("Take a screenshot", "Take a screenshot", "screenshot screen shot capture print"),
    ("Turn Wi-Fi on", "Turn Wi-Fi off", "wifi wi-fi wireless internet network"),
    ("Turn Bluetooth on", "Turn Bluetooth off", "bluetooth"),
    ("Mute", "Unmute", "mute unmute sound volume audio"),
    ("Turn on battery saver", "Turn off battery saver", "battery saver power save"),
    ("Turn on airplane mode", "Turn off airplane mode", "airplane flight mode"),
    ("Show Linux apps", "Show Linux apps", "linux apps debian"),
    ("Turn on game mode", "Turn off game mode", "game mode gaming crosshair play"),
    ("Next terminal theme", "Next terminal theme", "terminal theme colours colors style look"),
];

fn action_on(state: &DesktopState, index: usize) -> bool {
    match index {
        5 => state.wifi,
        6 => state.bluetooth,
        7 => state.muted,
        8 => state.battery_saver,
        9 => state.airplane,
        11 => game_mode(),
        _ => false,
    }
}

fn action_label(state: &DesktopState, index: usize) -> &'static str {
    let (off, on, _) = ACTIONS[index];
    if action_on(state, index) { on } else { off }
}

/// Whether `needle` starts a word of `haystack` or appears in `label`.
fn finds(needle: &[u8], label: &str, words: &str) -> bool {
    let contains = |text: &str| {
        text.as_bytes()
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
    };
    contains(label)
        || words.split(' ').any(|word| {
            word.len() >= needle.len() && word.as_bytes()[..needle.len()].eq_ignore_ascii_case(needle)
        })
}

// ---- calculator: + - * / % ^ and parentheses over millionths ----

const ONE: i64 = 1_000_000;

struct Calc<'a> {
    text: &'a [u8],
    at: usize,
}

impl Calc<'_> {
    fn skip(&mut self) {
        while self.text.get(self.at) == Some(&b' ') {
            self.at += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip();
        self.text.get(self.at).copied()
    }

    fn sum(&mut self) -> Option<i64> {
        let mut value = self.product()?;
        loop {
            match self.peek() {
                Some(b'+') => {
                    self.at += 1;
                    value = value.checked_add(self.product()?)?;
                }
                Some(b'-') => {
                    self.at += 1;
                    value = value.checked_sub(self.product()?)?;
                }
                _ => return Some(value),
            }
        }
    }

    fn product(&mut self) -> Option<i64> {
        let mut value = self.power()?;
        loop {
            match self.peek() {
                Some(b'*' | b'x') => {
                    self.at += 1;
                    let right = self.power()?;
                    value = (value as i128 * right as i128 / ONE as i128).try_into().ok()?;
                }
                Some(b'/') => {
                    self.at += 1;
                    let right = self.power()?;
                    if right == 0 {
                        return None;
                    }
                    value = (value as i128 * ONE as i128 / right as i128).try_into().ok()?;
                }
                Some(b'%') => {
                    self.at += 1;
                    let right = self.power()?;
                    if right == 0 {
                        return None;
                    }
                    value %= right;
                }
                _ => return Some(value),
            }
        }
    }

    fn power(&mut self) -> Option<i64> {
        let base = self.unary()?;
        if self.peek() == Some(b'^') {
            self.at += 1;
            let exponent = self.power()?;
            if exponent % ONE != 0 || !(0..=64 * ONE).contains(&exponent) {
                return None;
            }
            let mut value = ONE;
            for _ in 0..exponent / ONE {
                value = (value as i128 * base as i128 / ONE as i128).try_into().ok()?;
            }
            return Some(value);
        }
        Some(base)
    }

    fn unary(&mut self) -> Option<i64> {
        match self.peek()? {
            b'-' => {
                self.at += 1;
                Some(-self.unary()?)
            }
            b'(' => {
                self.at += 1;
                let value = self.sum()?;
                (self.peek() == Some(b')')).then(|| self.at += 1)?;
                Some(value)
            }
            _ => self.number(),
        }
    }

    fn number(&mut self) -> Option<i64> {
        self.skip();
        let (mut whole, mut fraction, mut scale, mut digits) = (0i64, 0i64, ONE, 0);
        let mut dot = false;
        while let Some(&byte) = self.text.get(self.at) {
            match byte {
                b'0'..=b'9' if !dot => whole = whole.checked_mul(10)?.checked_add((byte - b'0') as i64)?,
                b'0'..=b'9' => {
                    if scale > 1 {
                        scale /= 10;
                        fraction += (byte - b'0') as i64 * scale;
                    }
                }
                b'.' | b',' if !dot => dot = true,
                _ => break,
            }
            digits += 1;
            self.at += 1;
        }
        if digits == 0 {
            return None;
        }
        whole.checked_mul(ONE)?.checked_add(fraction)
    }
}

/// The value of `text` when it is arithmetic with at least one operator.
fn calculate(text: &[u8]) -> Option<i64> {
    let operator = text.iter().skip(1).any(|byte| b"+-*/%^x(".contains(byte));
    if !operator || !text.iter().any(u8::is_ascii_digit) {
        return None;
    }
    let mut calc = Calc { text, at: 0 };
    let value = calc.sum()?;
    calc.skip();
    (calc.at == text.len()).then_some(value)
}

fn format_number(value: i64, out: &mut ClockBuffer) {
    let negative = value < 0;
    let magnitude = value.unsigned_abs();
    let whole = magnitude / ONE as u64;
    let mut fraction = magnitude % ONE as u64;
    let _ = write!(out, "{}{}", if negative { "-" } else { "" }, whole);
    if fraction != 0 {
        let mut digits = 6;
        while fraction % 10 == 0 {
            fraction /= 10;
            digits -= 1;
        }
        let _ = write!(out, ".{:0width$}", fraction, width = digits);
    }
}

/// Everything the typed text finds, best first: the sum, matching actions,
/// apps, settings pages, then a web search.
fn results(state: &DesktopState, out: &mut [Hit; 8]) -> usize {
    let needle = &state.search_input[..state.search_len];
    let mut count = 0;
    let mut add = |hit: Hit, count: &mut usize| {
        if *count < out.len() {
            out[*count] = hit;
            *count += 1;
        }
    };
    if let Some(value) = calculate(needle) {
        add(Hit::Calc(value), &mut count);
    }
    if needle.len() >= 2 {
        for index in 0..ACTIONS.len() {
            if finds(needle, action_label(state, index), ACTIONS[index].2) {
                add(Hit::Action(index), &mut count);
            }
        }
    }
    for index in 0..launcher_len() {
        let name = launcher_name(index);
        if needle.is_empty() || finds(needle, name, "") {
            add(Hit::App(index), &mut count);
        }
    }
    if needle.len() >= 2 {
        for (page, name) in settings_ui::PAGES.iter().enumerate() {
            if finds(needle, name, "settings") {
                add(Hit::Setting(page as u8), &mut count);
            }
        }
    }
    if !needle.is_empty() && calculate(needle).is_none() {
        if count == 8 {
            count -= 1;
        }
        add(Hit::Web, &mut count);
    }
    count
}

/// How many result rows fit between the pill and the dock.
fn rows_that_fit() -> usize {
    ((DOCK_RECT.y - 8 - (SEARCH_PILL.bottom() + 12) + 8) / (ROW_HEIGHT + 8)).max(1) as usize
}

fn row_rect(slot: usize) -> Rect {
    Rect::new(
        SEARCH_PILL.x + 22,
        SEARCH_PILL.bottom() + 12 + slot as i32 * (ROW_HEIGHT + 8),
        SEARCH_PILL.width - 44,
        ROW_HEIGHT,
    )
}

impl DesktopState {
    pub(super) fn search_visible(&self, now: u64) -> bool {
        self.search_open || now < self.search_at_ns.saturating_add(CLOSE_NS)
    }

    pub(super) fn open_search(&mut self) {
        if self.search_open || self.screen != Screen::Desktop {
            return;
        }
        let now = crate::time::monotonic_nanoseconds();
        self.set_overlay(Overlay::None);
        self.close_popups();
        self.search_open = true;
        self.search_at_ns = now;
        self.search_len = 0;
        self.search_focus = 0;
        self.motion_until_ns = self.motion_until_ns.max(now + OPEN_NS + 80_000_000);
    }

    pub(super) fn close_search(&mut self) {
        if !self.search_open {
            return;
        }
        let now = crate::time::monotonic_nanoseconds();
        self.search_open = false;
        self.search_at_ns = now;
        self.motion_until_ns = self.motion_until_ns.max(now + CLOSE_NS + 60_000_000);
    }

    fn search_run(&mut self, hit: Hit) {
        self.close_search();
        let now = crate::time::monotonic_nanoseconds();
        match hit {
            Hit::Calc(value) => {
                let mut answer = ClockBuffer::new();
                format_number(value, &mut answer);
                crate::clipboard::get().push(answer.as_str().as_bytes());
                crate::notify::push(1, "Copied", answer.as_str());
            }
            Hit::App(entry) => self.launch_entry(entry),
            Hit::Setting(page) => {
                self.set_overlay(Overlay::None);
                self.set_app(DesktopApp::Settings);
                self.settings_show_page(page);
            }
            Hit::Web => {
                let mut url = [0u8; MAX_URL];
                let prefix = b"https://lite.duckduckgo.com/lite/?q=";
                url[..prefix.len()].copy_from_slice(prefix);
                let mut len = prefix.len();
                for &byte in &self.search_input[..self.search_len] {
                    let encoded: &[u8] = match byte {
                        b' ' => b"+",
                        b'&' => b"%26",
                        b'#' => b"%23",
                        b'+' => b"%2B",
                        b'?' => b"%3F",
                        _ => core::slice::from_ref(&byte),
                    };
                    if len + encoded.len() > MAX_URL {
                        break;
                    }
                    url[len..len + encoded.len()].copy_from_slice(encoded);
                    len += encoded.len();
                }
                self.url_input[..len].copy_from_slice(&url[..len]);
                self.url_len = len;
                self.set_overlay(Overlay::None);
                self.set_app(DesktopApp::Browser);
                self.url_active = false;
                self.start_load();
            }
            Hit::Action(index) => match index {
                0 => {
                    self.lock_session();
                }
                1 => self.start_power_action(shellui::PowerAction::Restart),
                2 => self.start_power_action(shellui::PowerAction::Shutdown),
                3 => self.start_power_action(shellui::PowerAction::Sleep),
                4 => self.screenshot_at_ns = now + 450_000_000,
                5 => self.quick_toggle(0),
                6 => self.quick_toggle(1),
                7 => self.toggle_mute(),
                8 => self.set_battery_saver(!self.battery_saver),
                9 => self.quick_toggle(4),
                11 => set_game_mode(!game_mode()),
                12 => {
                    let next = (TERMINAL_THEME.load(Ordering::Relaxed) + 1) % TERMINAL_THEMES.len();
                    TERMINAL_THEME.store(next, Ordering::Relaxed);
                    crate::notify::push(1, "Terminal theme", TERMINAL_THEMES[next].name);
                }
                _ => self.show_linux_apps(),
            },
        }
    }

    pub(super) fn search_key(&mut self, key: DesktopKey) -> Option<DesktopAction> {
        if !self.search_open {
            return None;
        }
        let mut hits = [Hit::Web; 8];
        let count = results(self, &mut hits);
        match key {
            DesktopKey::Escape => self.close_search(),
            DesktopKey::Backspace => {
                self.search_len = self.search_len.saturating_sub(1);
                self.search_focus = 0;
            }
            DesktopKey::Character(ch)
                if ch.is_ascii()
                    && self.search_len < SEARCH_MAX
                    && (ch.is_ascii_graphic() || ch == ' ') =>
            {
                self.search_input[self.search_len] = ch as u8;
                self.search_len += 1;
                self.search_focus = 0;
                self.search_typed_ns = crate::time::monotonic_nanoseconds();
                self.motion_until_ns = self.motion_until_ns.max(self.search_typed_ns + 300_000_000);
            }
            DesktopKey::Down | DesktopKey::Tab if count > 0 => {
                self.search_focus = (self.search_focus + 1) % count;
            }
            DesktopKey::Up if count > 0 => {
                self.search_focus = (self.search_focus + count - 1) % count;
            }
            DesktopKey::Activate if count > 0 => {
                self.search_run(hits[self.search_focus.min(count - 1)]);
            }
            _ => {}
        }
        Some(DesktopAction::Redraw)
    }

    pub(super) fn search_click(&mut self, point: Point) -> Option<DesktopAction> {
        if !self.search_open {
            return None;
        }
        let mut hits = [Hit::Web; 8];
        let count = results(self, &mut hits);
        for (slot, &hit) in hits.iter().enumerate().take(count.min(rows_that_fit())) {
            if row_rect(slot).contains(point) {
                self.search_run(hit);
                return Some(DesktopAction::Redraw);
            }
        }
        if !SEARCH_PILL.contains(point) {
            self.close_search();
        }
        Some(DesktopAction::Redraw)
    }
}

pub(super) fn draw_search(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now: u64,
) {
    if !state.search_visible(now) {
        return;
    }
    let progress = if state.search_open {
        ease_out_back_milli(progress_of(now, state.search_at_ns, OPEN_NS))
    } else {
        1000 - ease_in_out_milli(progress_of(now, state.search_at_ns, CLOSE_NS))
    };
    let rise = (1000 - progress) * 46 / 1000;
    let grow = progress.clamp(0, 1200);
    let width = SEARCH_PILL.width * (600 + grow * 4 / 10) / 1000;
    let mut pill = Rect::new(
        SEARCH_PILL.x + (SEARCH_PILL.width - width) / 2,
        SEARCH_PILL.y - rise,
        width,
        SEARCH_PILL.height,
    );
    pill.y = pill.y.max(-30);
    let mut style = setup_field_style();
    style.tint = Rgba::new(0, 0, 0, 51);
    style.border = Rgba::new(0, 0, 0, 255);
    style.border_width = 1;
    style.inner_highlight = Rgba::new(255, 255, 255, 30);
    let radii = CornerRadii::all(pill.height / 2);
    let _ = frost_mode(painter, layout, pill, radii, style, 0xae72_0001, true);
    let mut line = ClockBuffer::new();
    let query = core::str::from_utf8(&state.search_input[..state.search_len]).unwrap_or("");
    if state.search_len == 0 {
        let _ = line.write_str("Search");
    } else {
        let _ = line.write_str(query);
    }
    if state.search_open && (now / 520_000_000).is_multiple_of(2) {
        let _ = line.write_str("_");
    }
    let x = pill.x + 42;
    text(
        painter,
        layout,
        font,
        Point::new(x, pill.y + pill.height / 2 - 9),
        line.as_str(),
        18,
        if state.search_len == 0 {
            Color::rgb(232, 232, 236)
        } else {
            Color::rgb(255, 255, 255)
        },
    );
    let since_typed = now.saturating_sub(state.search_typed_ns);
    if since_typed < 260_000_000 {
        let pulse = 260_000_000 - since_typed;
        painter.stroke_rounded_rect(
            layout.rect(pill.expand((pulse / 50_000_000) as i32 + 1)),
            layout.radii(CornerRadii::all(pill.height / 2 + 4)),
            2,
            Rgba::new(255, 255, 255, (pulse * 140 / 260_000_000) as u8),
        );
    }
    if progress < 700 {
        return;
    }
    let mut hits = [Hit::Web; 8];
    let count = results(state, &mut hits);
    let age = if state.search_open {
        now.saturating_sub(state.search_at_ns)
    } else {
        0
    };
    for (slot, &hit) in hits.iter().enumerate().take(count.min(rows_that_fit())) {
        let delay = 140_000_000 + slot as u64 * 45_000_000;
        let arrive = ease_out_milli(progress_of(age.saturating_sub(delay), 0, 260_000_000)) as i32;
        if arrive <= 0 {
            continue;
        }
        let mut row = row_rect(slot);
        row.y += (1000 - arrive) * 16 / 1000;
        let focused = slot == state.search_focus;
        let radius = layout.radii(CornerRadii::all(ROW_HEIGHT / 2));
        painter.fill_rounded_rect(
            layout.rect(row),
            radius,
            Rgba::new(
                if focused { 255 } else { 226 },
                if focused { 255 } else { 238 },
                if focused { 255 } else { 244 },
                (arrive * if focused { 150 } else { 84 } / 1000) as u8,
            ),
        );
        painter.stroke_rounded_rect(
            layout.rect(row),
            radius,
            1,
            Rgba::new(255, 255, 255, (arrive * 120 / 1000) as u8),
        );
        let mut label = ClockBuffer::new();
        let kind = match hit {
            Hit::Calc(value) => {
                let _ = label.write_str("= ");
                format_number(value, &mut label);
                "Enter copies"
            }
            Hit::Action(index) => {
                let _ = label.write_str(action_label(state, index));
                "Action"
            }
            Hit::App(index) => {
                let _ = label.write_str(launcher_name(index));
                "App"
            }
            Hit::Setting(page) => {
                let _ = label.write_str(settings_ui::PAGES[page as usize]);
                "Settings"
            }
            Hit::Web => {
                let _ = label.write_str("Search the web for \"");
                let query = core::str::from_utf8(&state.search_input[..state.search_len]).unwrap_or("");
                let _ = label.write_str(query.get(..query.len().min(24)).unwrap_or(""));
                let _ = label.write_str("\"");
                "Web"
            }
        };
        text(
            painter,
            layout,
            font,
            Point::new(row.x + 18, row.y + ROW_HEIGHT / 2 - 7),
            label.as_str(),
            14,
            Color::rgb(14, 22, 28),
        );
        text(
            painter,
            layout,
            font,
            Point::new(row.right() - 96, row.y + ROW_HEIGHT / 2 - 5),
            kind,
            10,
            Color::rgb(36, 44, 52),
        );
    }
    if count > 0 && state.search_open {
        // The focus ring glides between rows.
        let target = layout.rect(row_rect(state.search_focus.min(count.min(rows_that_fit()) - 1)).expand(2));
        let y = motion::tween(motion::tween_key(9, 0), target.y, motion::QUICK, now);
        painter.stroke_rounded_rect(
            Rect::new(target.x, y, target.width, target.height),
            layout.radii(CornerRadii::all(ROW_HEIGHT / 2 + 2)),
            2,
            Rgba::new(255, 255, 255, 190),
        );
    }
    if count == 0 {
        centered_text(
            painter,
            layout,
            font,
            Rect::new(
                SEARCH_PILL.x,
                SEARCH_PILL.bottom() + 14,
                SEARCH_PILL.width,
                30,
            ),
            "Nothing found",
            14,
            Color::rgb(30, 44, 54),
        );
    }
}

// ------------------------------------------------------------ dock hover

pub(super) fn dock_app(index: usize) -> DesktopApp {
    match index {
        1 => DesktopApp::Terminal,
        3 => DesktopApp::Files,
        4 => DesktopApp::Browser,
        5 => DesktopApp::Notes,
        6 => DesktopApp::Trash,
        _ => DesktopApp::None,
    }
}

impl DesktopState {
    /// Called with the pointer position (design coordinates) when it moves.
    pub(super) fn hover_dock(&mut self, point: Point, now: u64) -> bool {
        if self.screen == Screen::Desktop && self.overlay == Overlay::Apps {
            let mut shown = [0; LAUNCHER_PAGE];
            let count = launcher_shown(self, &mut shown);
            let tile = (0..count)
                .find(|&slot| launcher_tile(slot).contains(point))
                .map(|slot| shown[slot]);
            let changed = match tile {
                Some(index) => !self.launcher_hover || self.app_focus != index,
                None => self.launcher_hover,
            };
            if let Some(index) = tile {
                self.app_focus = index;
            }
            self.launcher_hover = tile.is_some();
            if changed {
                self.focus_visible = false;
            }
            return changed;
        }
        self.launcher_hover = false;
        let mut over = usize::MAX;
        if self.screen == Screen::Desktop && self.overlay == Overlay::None {
            for index in 0..DOCK_BUTTONS.len() {
                if Rect::new(
                    dock_icon_rect(index).x,
                    DOCK_ICON_Y - 10,
                    DOCK_ICON_SIZE,
                    DOCK_ICON_SIZE + 12,
                )
                .contains(point)
                {
                    over = index;
                }
            }
        }
        if over == self.hover_index {
            return false;
        }
        self.hover_prev = self.hover_index;
        self.hover_index = over;
        self.hover_at_ns = now;
        self.motion_until_ns = self.motion_until_ns.max(now + 700_000_000);
        true
    }

    /// How far (logical px) dock button `index` floats up for the pointer:
    /// the hovered one springs up, its neighbours follow a little, and the
    /// one the pointer just left settles back down.
    pub(super) fn dock_lift(&self, index: usize, now: u64) -> i32 {
        if self.overlay != Overlay::None
            || self.overlay_closing != Overlay::None
                && now < self.overlay_close_at_ns + OVERLAY_TRANSITION_NS
        {
            return 0;
        }
        let spring = |distance: usize, height: i32, forward: bool| -> i32 {
            let strength = match distance {
                0 => 1000,
                1 => 380,
                2 => 110,
                _ => 0,
            };
            let progress = if forward {
                ease_out_back_milli(progress_of(now, self.hover_at_ns, 320_000_000)).max(0)
            } else {
                1000 - ease_out_milli(progress_of(now, self.hover_at_ns, 260_000_000)) as i32
            };
            height * strength / 1000 * progress / 1000
        };
        let mut lift = 0;
        if self.hover_index != usize::MAX {
            lift = lift.max(spring(self.hover_index.abs_diff(index), 8, true));
        }
        if self.hover_prev != usize::MAX && self.hover_prev != self.hover_index {
            lift = lift.max(spring(self.hover_prev.abs_diff(index), 8, false));
        }
        lift
    }

    pub(super) fn preview_wanted(&self) -> bool {
        self.hover_index != usize::MAX
            && self.app != DesktopApp::None
            && dock_app(self.hover_index) == self.app
    }

    pub(super) fn preview_target(&self, now: u64) -> Option<(usize, i32)> {
        if self.hover_index == usize::MAX
            || self.screen != Screen::Desktop
            || self.overlay != Overlay::None
            || self.power_open
        {
            return None;
        }
        let app = dock_app(self.hover_index);
        if app == DesktopApp::None || self.app != app {
            return None;
        }
        let waited = now.saturating_sub(self.hover_at_ns);
        if waited < 260_000_000 {
            return None;
        }
        let pop = ease_out_back_milli(progress_of(waited, 260_000_000, 380_000_000));
        Some((self.hover_index, pop))
    }
}

/// A small live copy of the hovered app's window, floating above its dock
/// button.
pub(super) fn draw_dock_preview(
    painter: &mut Painter<'_>,
    layout: Layout,
    state: &DesktopState,
    now: u64,
) {
    let Some((index, pop)) = state.preview_target(now) else {
        return;
    };
    let window = layout.rect(window_base_rect(state.app));
    let cache = panel_buffer();
    let length = frame_pixels(window);
    if length == 0 || length > screen_capacity() {
        return;
    }
    if !painter.read_region(window, &mut cache[..length]) {
        return;
    }
    PANEL_CACHE_KEY.store(0, Ordering::Release);
    let width = 168 * pop.max(0) / 1000;
    let height = width * window.height / window.width.max(1);
    let center = dock_icon_rect(index).x + DOCK_ICON_SIZE / 2;
    let outer = Rect::new(
        center - width / 2 - 6,
        344 - height - 12,
        width + 12,
        height + 12,
    );
    if width < 24 {
        return;
    }
    let radii = layout.radii(CornerRadii::all(16));
    painter.fill_rounded_rect(
        layout.rect(Rect::new(outer.x, outer.y + 3, outer.width, outer.height)),
        radii,
        Rgba::new(0, 0, 0, 50),
    );
    painter.fill_rounded_rect(layout.rect(outer), radii, Rgba::new(226, 228, 230, 250));
    painter.stroke_rounded_rect(layout.rect(outer), radii, 1, Rgba::new(28, 30, 32, 120));
    let inner = layout.rect(Rect::new(outer.x + 6, outer.y + 6, width, height));
    unsafe {
        painter.blit_scaled(
            inner,
            cache.as_ptr(),
            window.width as usize,
            window.height as usize,
            window.width as usize,
        );
    }
}
