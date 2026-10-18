use super::*;

pub(super) const CARD: Rect = Rect::new(126, 18, 500, 408);
pub(super) const BACK: Rect = Rect::new(136, 40, 100, 38);
pub(super) const NEXT: Rect = Rect::new(516, 40, 100, 38);
/// The show/hide-password eye inside the password field.
pub(super) const EYE: Rect = Rect::new(508, 202, 32, 30);
/// "Forgot password?" under the sign-in pill, and its confirmation card.
pub(super) const RESET_LINK: Rect = Rect::new(296, 388, 160, 22);
const RESET_CARD: Rect = Rect::new(206, 112, 340, 230);
pub(super) const RESET_CANCEL: Rect = Rect::new(226, 276, 140, 44);
pub(super) const RESET_OK: Rect = Rect::new(386, 276, 140, 44);
const INK: Color = Color::rgb(22, 26, 32);
const INK_SOFT: Color = Color::rgb(86, 96, 108);
pub(super) const WELCOME_NS: u64 = 3_600_000_000;
const SLIDE_NS: u64 = 620_000_000;
const KB_SCROLL_NS: u64 = 280_000_000;
pub(super) const KB_ROW_PITCH: i32 = 52;
pub(super) const KB_ROW_HEIGHT: i32 = 44;
pub(super) const KB_VISIBLE: i32 = 4;
pub(super) const KB_LIST: Rect = Rect::new(216, 178, 320, KB_ROW_PITCH * KB_VISIBLE);
pub(super) const KB_SEARCH_MAX: usize = 12;

pub(super) const LANGUAGE_ENGLISH: Rect = Rect::new(226, 156, 300, 54);

pub(super) fn shift_layout_x(layout: Layout, logical_dx: i32) -> Layout {
    let mut shifted = layout;
    shifted.offset.x = shifted
        .offset
        .x
        .saturating_add(layout.scale.logical(logical_dx));
    shifted
}

fn light_pill(painter: &mut Painter<'_>, layout: Layout, bounds: Rect, tint: Rgba) {
    let radii = layout.radii(CornerRadii::all(bounds.height / 2));
    painter.fill_rounded_rect(layout.rect(bounds), radii, tint);
    painter.stroke_rounded_rect(layout.rect(bounds), radii, 1, Rgba::new(26, 30, 36, 210));
}

fn field_pill(painter: &mut Painter<'_>, layout: Layout, bounds: Rect) {
    light_pill(painter, layout, bounds, Rgba::new(241, 243, 246, 255));
}

fn caret_on(now: u64) -> bool {
    (now / 520_000_000).is_multiple_of(2)
}

fn field_text(bytes: &[u8], masked: bool, now: u64) -> ClockBuffer {
    let mut result = ClockBuffer::new();
    if masked {
        for _ in 0..bytes.len().min(MAX_NAME) {
            let _ = result.write_str("*");
        }
    } else {
        let _ = result.write_str(core::str::from_utf8(bytes).unwrap_or(""));
    }
    let _ = result.write_str(if caret_on(now) { "_" } else { " " });
    result
}

/// How far (logical px) the current step's content still has to slide in
/// from the right; overshoots slightly and settles.
fn slide_in(state: &DesktopState, now: u64) -> i32 {
    let progress = progress_of(now, state.screen_transition_at_ns, SLIDE_NS);
    let eased = ease_out_back_milli(progress);
    (1000 - eased) * 110 / 1000
}

fn chrome(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    heading: &str,
    back: bool,
    next: &str,
) {
    let button = |painter: &mut Painter<'_>, bounds: Rect, label: &str| {
        field_pill(painter, layout, bounds);
        centered_text(painter, layout, font, bounds, label, 14, INK);
    };
    if back {
        button(painter, BACK, "Back");
    }
    if !next.is_empty() {
        button(painter, NEXT, next);
    }
    centered_text(
        painter,
        layout,
        font,
        Rect::new(
            BACK.right() + 6,
            BACK.y,
            NEXT.x - BACK.right() - 12,
            BACK.height,
        ),
        heading,
        17,
        INK,
    );
}

fn hint(painter: &mut Painter<'_>, layout: Layout, font: RasterFont, value: &str, alpha: i32) {
    let tone = mix_color(Color::rgb(168, 176, 184), INK_SOFT, alpha.clamp(0, 1000));
    centered_text(
        painter,
        layout,
        font,
        Rect::new(CARD.x, 396, CARD.width, 22),
        value,
        12,
        tone,
    );
}

fn breathe(now: u64) -> i32 {
    600 + wave_milli(now / 4_000_000) * 400 / 1000
}

pub(super) fn draw_language(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now: u64,
) -> bool {
    let layout = shift_layout_x(layout, slide_in(state, now));
    chrome(painter, layout, font, "Select your language", false, "Next");
    let glow = breathe(now);
    field_pill(painter, layout, LANGUAGE_ENGLISH);
    painter.stroke_rounded_rect(
        layout.rect(LANGUAGE_ENGLISH.expand(3)),
        layout.radii(CornerRadii::all(LANGUAGE_ENGLISH.height / 2 + 3)),
        2,
        Rgba::new(46, 120, 232, (90 + glow * 120 / 1000) as u8),
    );
    centered_text(painter, layout, font, LANGUAGE_ENGLISH, "English", 17, INK);
    centered_text(
        painter,
        layout,
        font,
        Rect::new(CARD.x, 232, CARD.width, 24),
        "More languages coming soon",
        13,
        INK_SOFT,
    );
    hint(painter, layout, font, "Press Enter to continue", glow);
    true
}

pub(super) fn keyboard_matches(state: &DesktopState, out: &mut [usize; 16]) -> usize {
    let needle = &state.kb_search[..state.kb_search_len];
    let mut count = 0;
    for index in 0..crate::keymap::layout_count().min(16) {
        let name = crate::keymap::layout_name(index).as_bytes();
        let matches = needle.is_empty()
            || name
                .windows(needle.len())
                .any(|window| window.eq_ignore_ascii_case(needle));
        if matches {
            out[count] = index;
            count += 1;
        }
    }
    count
}

/// The list's scroll position in thousandths of a row, animated.
pub(super) fn keyboard_scroll_milli(state: &DesktopState, now: u64) -> i32 {
    let target = state.kb_scroll_target * 1000;
    let t = ease_out_milli(progress_of(now, state.kb_scroll_at_ns, KB_SCROLL_NS)) as i32;
    lerp_i32(state.kb_scroll_from, target, t)
}

pub(super) fn draw_keyboard(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now: u64,
) -> bool {
    let layout = shift_layout_x(layout, slide_in(state, now));
    chrome(
        painter,
        layout,
        font,
        "Select your keyboard layout",
        true,
        "Next",
    );
    let search = Rect::new(216, 112, 320, 46);
    field_pill(painter, layout, search);
    let query = field_text(&state.kb_search[..state.kb_search_len], false, now);
    if state.kb_search_len == 0 {
        centered_text(painter, layout, font, search, "Search", 15, INK_SOFT);
    } else {
        centered_text(painter, layout, font, search, query.as_str(), 15, INK);
    }
    let mut matches = [0usize; 16];
    let count = keyboard_matches(state, &mut matches);
    let scroll = keyboard_scroll_milli(state, now);
    let clip = layout.rect(Rect::new(
        KB_LIST.x - 6,
        KB_LIST.y - 2,
        KB_LIST.width + 12,
        KB_LIST.height + 4,
    ));
    if painter.set_clip(clip) {
        for (position, &index) in matches[..count].iter().enumerate() {
            let y = KB_LIST.y + position as i32 * KB_ROW_PITCH - scroll * KB_ROW_PITCH / 1000;
            if y + KB_ROW_HEIGHT < KB_LIST.y - 4 || y > KB_LIST.bottom() + 4 {
                continue;
            }
            let row = Rect::new(KB_LIST.x, y, KB_LIST.width, KB_ROW_HEIGHT);
            let selected = position == state.kb_cursor;
            light_pill(
                painter,
                layout,
                row,
                if selected {
                    Rgba::new(208, 228, 252, 255)
                } else {
                    Rgba::new(241, 243, 246, 255)
                },
            );
            if selected {
                painter.stroke_rounded_rect(
                    layout.rect(row.expand(2)),
                    layout.radii(CornerRadii::all(KB_ROW_HEIGHT / 2 + 2)),
                    2,
                    Rgba::new(46, 120, 232, (120 + breathe(now) * 110 / 1000) as u8),
                );
            }
            if row.y < KB_LIST.y || row.bottom() > KB_LIST.bottom() {
                continue;
            }
            centered_text(
                painter,
                layout,
                font,
                row,
                crate::keymap::layout_name(index),
                14,
                INK,
            );
            if index == crate::keymap::layout_index() {
                painter.fill_rounded_rect(
                    layout.rect(Rect::new(row.x + 16, row.y + KB_ROW_HEIGHT / 2 - 3, 6, 6)),
                    layout.radii(CornerRadii::all(3)),
                    Rgba::new(46, 120, 232, 255),
                );
            }
        }
        if count == 0 {
            centered_text(
                painter,
                layout,
                font,
                KB_LIST,
                "No matching layouts",
                14,
                INK_SOFT,
            );
        }
    }
    painter.reset_clip();
    if count as i32 > KB_VISIBLE {
        let track = Rect::new(KB_LIST.right() + 12, KB_LIST.y, 8, KB_LIST.height);
        painter.fill_rounded_rect(
            layout.rect(track),
            layout.radii(CornerRadii::all(4)),
            Rgba::new(26, 30, 36, 28),
        );
        let total = count as i32 * KB_ROW_PITCH;
        let thumb_h = (track.height * KB_VISIBLE * KB_ROW_PITCH / total).max(24);
        let travel = track.height - thumb_h;
        let max_scroll = (count as i32 - KB_VISIBLE).max(1) * 1000;
        let thumb_y = track.y + travel * scroll.clamp(0, max_scroll) / max_scroll;
        painter.fill_rounded_rect(
            layout.rect(Rect::new(track.x, thumb_y, 8, thumb_h)),
            layout.radii(CornerRadii::all(4)),
            Rgba::new(70, 78, 90, 230),
        );
    }
    hint(
        painter,
        layout,
        font,
        "Type to search, arrows to choose, Enter to continue",
        breathe(now),
    );
    true
}

pub(super) fn draw_credentials(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now: u64,
) -> bool {
    let layout = shift_layout_x(layout, slide_in(state, now));
    let (heading, next) = match state.screen {
        Screen::Username => ("What should we call you?", "Next"),
        Screen::Password => ("Set a password", "Next"),
        _ => ("Confirm your password", "Finish"),
    };
    chrome(painter, layout, font, heading, true, next);
    let field = Rect::new(196, 184, 360, 66);
    field_pill(painter, layout, field);
    let caret = match state.screen {
        Screen::Password => field_text(
            &state.setup_password_input[..state.setup_password_len],
            !state.reveal_password,
            now,
        ),
        Screen::Confirm => field_text(
            &state.confirm_input[..state.confirm_len],
            !state.reveal_password,
            now,
        ),
        _ => field_text(&state.username_input[..state.username_len], false, now),
    };
    centered_text(painter, layout, font, field, caret.as_str(), 18, INK);
    if matches!(state.screen, Screen::Password | Screen::Confirm) {
        draw_eye(painter, layout, EYE, state.reveal_password);
    }
    let since_typed = now.saturating_sub(state.setup_typed_ns);
    if since_typed < 240_000_000 {
        let pulse = 240_000_000 - since_typed;
        painter.stroke_rounded_rect(
            layout.rect(field.expand((pulse / 40_000_000) as i32 + 1)),
            layout.radii(CornerRadii::all(field.height / 2 + 4)),
            2,
            Rgba::new(46, 120, 232, (pulse * 190 / 240_000_000) as u8),
        );
    }
    let (message, tone) = if !state.setup_error.is_empty() {
        (state.setup_error, Color::rgb(206, 52, 40))
    } else {
        (
            match state.screen {
                _ if state.caps_lock => "Caps Lock is on",
                Screen::Password => "8+ characters, not a common password",
                Screen::Confirm => "Type the same password again",
                _ => "Letters and numbers only",
            },
            INK_SOFT,
        )
    };
    let shake = if state.setup_error.is_empty() {
        0
    } else {
        let age = now.saturating_sub(state.setup_error_ns).min(420_000_000);
        ((420_000_000 - age) / 30_000_000) as i32
            * if (age / 45_000_000).is_multiple_of(2) {
                1
            } else {
                -1
            }
    };
    centered_text(
        painter,
        shift_layout_x(layout, shake),
        font,
        Rect::new(CARD.x, 274, CARD.width, 24),
        message,
        13,
        tone,
    );
    hint(
        painter,
        layout,
        font,
        "Press Enter to continue",
        breathe(now),
    );
    true
}

/// The confirmation shown over the sign-in screen.
pub(super) fn draw_reset(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
) {
    painter.fill_rounded_rect(
        layout.rect(Rect::new(-60, -60, DESIGN_WIDTH + 120, 458 + 120)),
        CornerRadii::all(0),
        Rgba::new(0, 0, 0, 90),
    );
    painter.fill_rounded_rect(
        layout.rect(RESET_CARD),
        layout.radii(CornerRadii::all(34)),
        Rgba::new(250, 251, 252, 255),
    );
    centered_text(
        painter,
        layout,
        font,
        Rect::new(RESET_CARD.x, RESET_CARD.y + 22, RESET_CARD.width, 28),
        "Reset this account?",
        19,
        INK,
    );
    for (index, line) in [
        "Your files are kept. You will choose",
        "a new user name and password.",
    ]
    .iter()
    .enumerate()
    {
        centered_text(
            painter,
            layout,
            font,
            Rect::new(
                RESET_CARD.x,
                RESET_CARD.y + 66 + index as i32 * 24,
                RESET_CARD.width,
                22,
            ),
            line,
            13,
            INK_SOFT,
        );
    }
    for (index, (bounds, label)) in [(RESET_CANCEL, "Cancel"), (RESET_OK, "Reset")]
        .iter()
        .enumerate()
    {
        let tint = if index == 1 {
            Rgba::new(250, 222, 218, 255)
        } else {
            Rgba::new(241, 243, 246, 255)
        };
        light_pill(painter, layout, *bounds, tint);
        if state.reset_focus as usize == index {
            painter.stroke_rounded_rect(
                layout.rect(bounds.expand(3)),
                layout.radii(CornerRadii::all(bounds.height / 2 + 3)),
                2,
                Rgba::new(46, 120, 232, 255),
            );
        }
        centered_text(painter, layout, font, *bounds, label, 15, INK);
    }
}

pub(super) fn draw_welcome(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now: u64,
) -> bool {
    let age = now.saturating_sub(state.screen_transition_at_ns);
    let pop = ease_out_back_milli(progress_of(now, state.screen_transition_at_ns, 900_000_000));
    let rise = (1000
        - ease_out_milli(progress_of(now, state.screen_transition_at_ns, 700_000_000)) as i32)
        * 30
        / 1000;
    centered_text(
        painter,
        layout,
        font,
        Rect::new(CARD.x, 52 + rise, CARD.width, 46),
        "Welcome to AerOS",
        36,
        INK,
    );
    let bob = wave_milli(now / 3_200_000) * 5 / 1000;
    let base_w = 250 * pop.max(0) / 1000;
    let width = base_w.max(8);
    let height = crate::logo::height_for(width);
    let center = Point::new(376, 236 + bob);
    let halo_phase = ((now / 2_600_000) % 1000) as i32;
    let halo = 12 + halo_phase * 46 / 1000;
    let halo_alpha = (60 - halo_phase * 60 / 1000).max(0) as u8;
    let ring = Rect::new(
        center.x - width / 2 - halo,
        center.y - height / 2 - halo,
        width + halo * 2,
        height + halo * 2,
    );
    painter.stroke_rounded_rect(
        layout.rect(ring),
        layout.radii(CornerRadii::all(ring.height / 2)),
        2,
        Rgba::new(46, 120, 232, halo_alpha),
    );
    let tile_w = width * 13 / 10;
    let tile = Rect::new(center.x - tile_w / 2, center.y - tile_w / 2, tile_w, tile_w);
    painter.fill_rounded_rect(
        layout.rect(tile),
        layout.radii(CornerRadii::all(tile_w * 27 / 100)),
        Rgba::new(22, 22, 24, (age * 255 / 500_000_000).min(255) as u8),
    );
    let target = layout.rect(Rect::new(
        center.x - width / 2,
        center.y - height / 2,
        width,
        height,
    ));
    let opacity = (age * 255 / 500_000_000).min(255) as u8;
    painter.draw_rgba_scaled_alpha(
        target,
        crate::logo::rgba(),
        crate::logo::WIDTH,
        crate::logo::HEIGHT,
        opacity,
    );
    hint(
        painter,
        layout,
        font,
        "Setting things up for you",
        breathe(now),
    );
    true
}

fn dim_rgba(color: Rgba, alpha: u8) -> Rgba {
    Rgba::new(
        color.red,
        color.green,
        color.blue,
        (color.alpha as u32 * alpha as u32 / 255) as u8,
    )
}

/// The full-screen black start / restart / shutdown screen: the leaf on a
/// dark tile that pops in and breathes, expanding halos, and either a pill
/// progress bar (`progress` = Some) or a caption (`caption` non-empty).
/// `alpha` fades the whole screen in or out.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_boot_screen(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    screen: Rect,
    elapsed_ms: u64,
    progress: Option<u16>,
    caption: &str,
    alpha: u8,
) {
    painter.fill_rounded_rect(screen, CornerRadii::all(0), Rgba::new(0, 0, 0, alpha));
    let unit = |value: i32| layout.scale.logical(value);
    let center_x = screen.x + screen.width / 2;
    let center_y = screen.y + screen.height / 2 - unit(if progress.is_some() { 14 } else { 4 });
    let age = elapsed_ms.min(4_000) * 1_000_000;
    let pop = ease_out_back_milli(progress_of(age, 0, 720_000_000)).max(0);
    let breath = wave_milli(elapsed_ms * 1_000_000 / 3_400_000) * 22 / 1000;
    let fade = (elapsed_ms.min(600) * alpha as u64 / 600).min(255) as u8;
    let size = unit(132) * (pop + breath) / 1000;
    let radius = size * 27 / 100;
    let tile = Rect::new(center_x - size / 2, center_y - size / 2, size, size);
    painter.fill_rounded_rect(
        tile.expand(1),
        CornerRadii::all(radius + 1),
        dim_rgba(Rgba::new(255, 255, 255, 28), fade),
    );
    painter.fill_rounded_rect(
        tile,
        CornerRadii::all(radius),
        dim_rgba(Rgba::new(22, 22, 24, 255), fade),
    );
    let logo_w = size * 66 / 100;
    let logo_h = crate::logo::height_for(logo_w);
    let logo = Rect::new(center_x - logo_w / 2, center_y - logo_h / 2, logo_w, logo_h);
    painter.draw_rgba_scaled_alpha(
        logo,
        crate::logo::rgba(),
        crate::logo::WIDTH,
        crate::logo::HEIGHT,
        fade,
    );
    if let Some(progress) = progress {
        let bar_w = unit(300);
        let bar_h = unit(24);
        let bar = Rect::new(center_x - bar_w / 2, tile.bottom() + unit(46), bar_w, bar_h);
        let pill = CornerRadii::all(bar_h / 2);
        painter.fill_rounded_rect(bar, pill, dim_rgba(Rgba::new(70, 70, 72, 255), fade));
        let fill_w = ((bar_w as i64 * progress as i64 / 1000) as i32).max(bar_h);
        let fill = Rect::new(bar.x, bar.y, fill_w, bar_h);
        painter.fill_rounded_rect(fill, pill, dim_rgba(Rgba::new(176, 176, 180, 255), fade));
    }
    if !caption.is_empty() {
        let height = unit(22);
        let width = font.text_width(caption, height);
        let dots = (elapsed_ms / 400 % 4) as i32;
        painter.text(
            font,
            Point::new(center_x - width / 2, tile.bottom() + unit(38)),
            caption,
            height,
            mix_color(
                Color::rgb(0, 0, 0),
                Color::rgb(240, 240, 242),
                fade as i32 * 1000 / 255,
            ),
        );
        let _ = dots;
    }
}
