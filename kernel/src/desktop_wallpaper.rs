#![allow(clippy::precedence)]

use super::*;
use core::cell::UnsafeCell;

/// Silk wallpapers (rendered from the Silk shader at 2560x1440), then the
/// valley picture, then the user's own picture once they have added one.
const SILK: [&[u8]; 5] = [
    include_bytes!("../../assets/wallpapers/silk-black.png"),
    include_bytes!("../../assets/wallpapers/silk-purple.png"),
    include_bytes!("../../assets/wallpapers/silk-blue.png"),
    include_bytes!("../../assets/wallpapers/silk-grey.png"),
    include_bytes!("../../assets/wallpapers/silk-beige.png"),
];
pub(super) const VALLEY: usize = SILK.len();
pub(super) const CUSTOM: usize = VALLEY + 1;
const DEFAULT: usize = 0;

const CHOICE_PATH: &str = "/data/WALLPAPR.DAT";
const PICTURE_PATH: &str = "/data/WALLPAPR.IMG";
const CHOICE_MAGIC: &[u8; 4] = b"AEW1";
const MAX_PICTURE_BYTES: u64 = 48 * 1024 * 1024;

struct Loaded(UnsafeCell<Option<crate::image::Image>>);

// SAFETY: only the desktop thread touches the wallpaper.
unsafe impl Sync for Loaded {}

static LOADED: Loaded = Loaded(UnsafeCell::new(None));
/// The Silk picture at half size (2x2 averaged), which the live wallpaper
/// samples so its half-resolution pass doesn't alias the thin bright rims.
static HALF: Loaded = Loaded(UnsafeCell::new(None));
static CURRENT: AtomicUsize = AtomicUsize::new(usize::MAX);
static GENERATION: AtomicU64 = AtomicU64::new(1);
static HAS_CUSTOM: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub(super) enum Source {
    Valley,
    Picture(&'static [u8], usize, usize),
}

/// What the wallpaper is drawn from right now (the default is decoded on
/// first use).
pub(super) fn source() -> Source {
    if CURRENT.load(Ordering::Acquire) == usize::MAX {
        let _ = apply(DEFAULT);
    }
    // SAFETY: see `Loaded`.
    match unsafe { &*LOADED.0.get() } {
        Some(image) => Source::Picture(
            // SAFETY: the image lives until the next `apply`, which only the
            // desktop thread calls between frames.
            unsafe { core::slice::from_raw_parts(image.rgba().as_ptr(), image.rgba().len()) },
            image.width,
            image.height,
        ),
        None => Source::Valley,
    }
}

/// Changes whenever a different wallpaper is shown, so caches of what was
/// drawn over the old one are thrown away.
pub(super) fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

pub(super) fn current() -> usize {
    CURRENT.load(Ordering::Acquire).min(CUSTOM)
}

pub(super) fn count() -> usize {
    if HAS_CUSTOM.load(Ordering::Acquire) {
        CUSTOM + 1
    } else {
        CUSTOM
    }
}

fn install(image: Option<crate::image::Image>, index: usize) {
    let half = if index < VALLEY {
        image.as_ref().and_then(halve)
    } else {
        None
    };
    // SAFETY: see `Loaded`.
    unsafe {
        *HALF.0.get() = half;
        *LOADED.0.get() = image;
    }
    CURRENT.store(index, Ordering::Release);
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

fn halve(image: &crate::image::Image) -> Option<crate::image::Image> {
    let (width, height) = (image.width / 2, image.height / 2);
    let mut half = crate::image::Image::new(width, height).ok()?;
    let source = image.rgba();
    let row = image.width * 4;
    let out = half.rgba_mut();
    for y in 0..height {
        for x in 0..width {
            let at = 2 * y * row + 8 * x;
            let to = (y * width + x) * 4;
            for channel in 0..3 {
                let sum = source[at + channel] as u32
                    + source[at + 4 + channel] as u32
                    + source[at + row + channel] as u32
                    + source[at + row + 4 + channel] as u32;
                out[to + channel] = ((sum + 2) / 4) as u8;
            }
            out[to + 3] = 255;
        }
    }
    Some(half)
}

/// The half-size Silk picture, when the current wallpaper has one.
pub(super) fn half_picture() -> Option<(&'static [u8], usize, usize)> {
    // SAFETY: see `Loaded` and `source`.
    unsafe { &*HALF.0.get() }.as_ref().map(|image| {
        (
            // SAFETY: as in `source`.
            unsafe { core::slice::from_raw_parts(image.rgba().as_ptr(), image.rgba().len()) },
            image.width,
            image.height,
        )
    })
}

fn decode_rgba(data: &[u8]) -> Option<crate::image::Image> {
    let image = crate::image::decode(data).ok()?;
    (image.width >= 16 && image.height >= 16).then_some(image)
}

/// Shows wallpaper `index`; false (and nothing changes) if it can't be loaded.
pub(super) fn apply(index: usize) -> bool {
    if index < VALLEY {
        let Some(image) = decode_rgba(SILK[index]) else {
            return false;
        };
        install(Some(image), index);
        return true;
    }
    if index == VALLEY {
        install(None, VALLEY);
        return true;
    }
    let Some(data) = read_all(PICTURE_PATH) else {
        return false;
    };
    let Some(image) = decode_rgba(data.as_slice()) else {
        return false;
    };
    HAS_CUSTOM.store(true, Ordering::Release);
    install(Some(image), CUSTOM);
    true
}

fn read_all(path: &str) -> Option<crate::memory::PageBuffer> {
    let size = vfs::metadata(path).ok()?.size;
    if size == 0 || size > MAX_PICTURE_BYTES {
        return None;
    }
    let mut buffer = crate::memory::PageBuffer::new(size as usize)?;
    let descriptor = vfs::open_file(path, false, false, false, 0, false).ok()?;
    let mut filled = 0usize;
    let target = &mut buffer.as_mut_slice()[..size as usize];
    while filled < target.len() {
        match vfs::read(descriptor, &mut target[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(read) => filled += read,
        }
    }
    let _ = vfs::close(descriptor);
    if filled != size as usize {
        return None;
    }
    Some(buffer)
}

fn write_all(path: &str, data: &[u8]) -> bool {
    let Ok(descriptor) = vfs::open_file(path, true, false, true, 0o600, true) else {
        return false;
    };
    let mut written = 0;
    while written < data.len() {
        match vfs::write(descriptor, &data[written..], false) {
            Ok(0) | Err(_) => break,
            Ok(count) => written += count,
        }
    }
    let _ = vfs::close(descriptor);
    written == data.len()
}

fn save_choice() {
    let record = [
        CHOICE_MAGIC[0],
        CHOICE_MAGIC[1],
        CHOICE_MAGIC[2],
        CHOICE_MAGIC[3],
        current() as u8,
    ];
    let saved = write_all(CHOICE_PATH, &record);
    serial::format(format_args!(
        "AEROS_WALLPAPER saved={saved} index={}\n",
        current()
    ));
}

/// The wallpaper picked last time (after the data disk is up).
#[cfg_attr(feature = "boot-test", allow(dead_code))]
pub(super) fn load_choice() {
    HAS_CUSTOM.store(vfs::metadata(PICTURE_PATH).is_ok(), Ordering::Release);
    let mut record = [0u8; 8];
    let Ok(descriptor) = vfs::open_file(CHOICE_PATH, false, false, false, 0, false) else {
        return;
    };
    let read = vfs::read(descriptor, &mut record);
    let _ = vfs::close(descriptor);
    if read != Ok(5) || &record[..4] != CHOICE_MAGIC {
        return;
    }
    let index = record[4] as usize;
    if index <= CUSTOM && index != CURRENT.load(Ordering::Acquire) && !apply(index) {
        let _ = apply(DEFAULT);
    }
}

/// Makes the picture at `path` the wallpaper, keeping a copy on the data
/// disk so it survives the original being moved or deleted.
pub(super) fn use_picture(path: &str) -> Result<(), &'static str> {
    let data = read_all(path).ok_or("can't read that picture")?;
    let image = decode_rgba(data.as_slice()).ok_or("not a picture AerOS can show")?;
    if !write_all(PICTURE_PATH, data.as_slice()) {
        return Err("couldn't save the picture");
    }
    HAS_CUSTOM.store(true, Ordering::Release);
    install(Some(image), CUSTOM);
    save_choice();
    Ok(())
}

pub(super) fn is_picture(name: &str) -> bool {
    let lower = |suffix: &str| {
        name.len() > suffix.len()
            && name.as_bytes()[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix.as_bytes())
    };
    lower(".png") || lower(".jpg") || lower(".jpeg") || lower(".bmp")
}

// ---- Change Background ----------------------------------------------------

const DOT: i32 = 9;
const DOT_PITCH: i32 = 15;
const DOT_Y: i32 = 415;
const PLUS: i32 = 21;

fn dot_rect(index: usize, total: usize) -> Rect {
    let width = total as i32 * DOT_PITCH - (DOT_PITCH - DOT);
    let left = DESIGN_WIDTH / 2 - width / 2;
    Rect::new(left + index as i32 * DOT_PITCH, DOT_Y, DOT, DOT)
}

fn plus_rect(total: usize) -> Rect {
    let last = dot_rect(total - 1, total);
    Rect::new(last.right() + 9, DOT_Y + DOT / 2 - PLUS / 2, PLUS, PLUS)
}

impl DesktopState {
    pub(super) fn open_backgrounds(&mut self) {
        self.set_overlay(Overlay::None);
        self.set_app(DesktopApp::None);
        self.ctx_open = false;
        self.clip_open = false;
        self.menu_open = false;
        self.bg_open = true;
        self.bg_pick_file = false;
    }

    fn close_backgrounds(&mut self) {
        self.bg_open = false;
        save_choice();
    }

    fn show_background(&mut self, index: usize) {
        if index != current() && !apply(index) {
            self.bg_failed = true;
            return;
        }
        self.bg_failed = false;
    }

    fn add_background(&mut self) {
        self.bg_open = false;
        self.bg_pick_file = true;
        self.open_files();
        self.files_set_status("Open a picture to use it as your background");
    }

    /// Files hands over the picture the user opened while choosing one.
    pub(super) fn background_from_file(&mut self, path: &str) {
        match use_picture(path) {
            Ok(()) => {
                self.bg_pick_file = false;
                self.set_app(DesktopApp::None);
                self.bg_open = true;
            }
            Err(message) => self.files_set_status(message),
        }
    }

    pub(super) fn backgrounds_key(&mut self, key: DesktopKey) -> Option<DesktopAction> {
        if !self.bg_open {
            return None;
        }
        let total = count();
        match key {
            DesktopKey::Left | DesktopKey::Up => {
                self.show_background((current() + total - 1) % total)
            }
            DesktopKey::Right | DesktopKey::Down | DesktopKey::Tab => {
                self.show_background((current() + 1) % total)
            }
            DesktopKey::Escape | DesktopKey::Activate | DesktopKey::Menu => {
                self.close_backgrounds()
            }
            _ => return Some(DesktopAction::Idle),
        }
        Some(DesktopAction::Redraw)
    }

    pub(super) fn backgrounds_click(&mut self, point: Point) -> Option<DesktopAction> {
        if !self.bg_open {
            return None;
        }
        let total = count();
        for index in 0..total {
            if dot_rect(index, total).expand(4).contains(point) {
                self.show_background(index);
                return Some(DesktopAction::Redraw);
            }
        }
        if plus_rect(total).expand(3).contains(point) {
            self.add_background();
        } else {
            self.close_backgrounds();
        }
        Some(DesktopAction::Redraw)
    }
}

pub(super) fn draw_backgrounds(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
) {
    let total = count();
    let shown = current();
    for index in 0..total {
        let dot = dot_rect(index, total);
        let radii = layout.radii(CornerRadii::all(DOT / 2));
        if index == shown {
            painter.fill_rounded_rect(layout.rect(dot), radii, Rgba::new(255, 255, 255, 255));
        } else {
            painter.fill_rounded_rect(layout.rect(dot), radii, Rgba::new(255, 255, 255, 130));
            painter.stroke_rounded_rect(layout.rect(dot), radii, 1, Rgba::new(128, 128, 128, 190));
        }
    }
    let plus = plus_rect(total);
    let _ = frost(
        painter,
        layout,
        plus,
        CornerRadii::all(PLUS / 2),
        plus_style(),
        0xb6_0001,
    );
    let cx = plus.x + PLUS / 2;
    let cy = plus.y + PLUS / 2;
    let bar = layout.radii(CornerRadii::all(1));
    let white = Rgba::new(255, 255, 255, 240);
    painter.fill_rounded_rect(layout.rect(Rect::new(cx - 5, cy - 1, 11, 3)), bar, white);
    painter.fill_rounded_rect(layout.rect(Rect::new(cx - 1, cy - 5, 3, 11)), bar, white);
    if state.bg_failed {
        let note = Rect::new(DESIGN_WIDTH / 2 - 90, DOT_Y - 34, 180, 22);
        painter.fill_rounded_rect(
            layout.rect(note),
            layout.radii(CornerRadii::all(11)),
            Rgba::new(4, 4, 4, 230),
        );
        centered_text(
            painter,
            layout,
            font,
            note,
            "Couldn't load that background",
            10,
            Color::rgb(255, 255, 255),
        );
    }
}

fn plus_style() -> FrostStyle {
    FrostStyle {
        blur_radius: 10,
        saturation_percent: 100,
        brightness_percent: 100,
        tint: Rgba::new(217, 217, 217, 51),
        border: Rgba::new(255, 255, 255, 60),
        border_width: 1,
        inner_highlight: Rgba::new(255, 255, 255, 40),
        inner_shadow: Rgba::transparent(),
        inner_shadow_offset: Point::new(0, 0),
        inner_shadow_softness: 0,
        shadow: Rgba::transparent(),
        shadow_offset: Point::new(0, 0),
        shadow_spread: 0,
        shadow_softness: 0,
        noise_alpha: 0,
    }
}

// ---- Live Silk ---------------------------------------------------------------
// The Silk shader's cursor response, applied to the rendered picture: the
// fabric is pushed away from the pointer, the layers drift with it
// (parallax) and a soft light follows it. Eased like the Windows app's
// `sim.rs`. All fixed point: the kernel has no hardware floating point.

/// exp(-i / 32) in 16.16, i = 0..255.
const EXP_NEG: [u16; 256] = [
    65535, 63520, 61565, 59671, 57835, 56056, 54331, 52660, 51039, 49469, 47947, 46472, 45042,
    43656, 42313, 41011, 39750, 38527, 37341, 36192, 35079, 34000, 32954, 31940, 30957, 30005,
    29081, 28187, 27319, 26479, 25664, 24875, 24109, 23368, 22649, 21952, 21276, 20622, 19987,
    19372, 18776, 18199, 17639, 17096, 16570, 16060, 15566, 15087, 14623, 14173, 13737, 13314,
    12905, 12508, 12123, 11750, 11388, 11038, 10698, 10369, 10050, 9741, 9441, 9151, 8869, 8596,
    8332, 8076, 7827, 7586, 7353, 7127, 6907, 6695, 6489, 6289, 6096, 5908, 5726, 5550, 5380, 5214,
    5054, 4898, 4747, 4601, 4460, 4323, 4190, 4061, 3936, 3815, 3697, 3584, 3473, 3366, 3263, 3162,
    3065, 2971, 2879, 2791, 2705, 2622, 2541, 2463, 2387, 2314, 2243, 2174, 2107, 2042, 1979, 1918,
    1859, 1802, 1746, 1693, 1641, 1590, 1541, 1494, 1448, 1403, 1360, 1318, 1278, 1238, 1200, 1163,
    1128, 1093, 1059, 1027, 995, 964, 935, 906, 878, 851, 825, 800, 775, 751, 728, 706, 684, 663,
    642, 623, 604, 585, 567, 550, 533, 516, 500, 485, 470, 456, 442, 428, 415, 402, 390, 378, 366,
    355, 344, 333, 323, 313, 303, 294, 285, 276, 268, 260, 252, 244, 236, 229, 222, 215, 209, 202,
    196, 190, 184, 178, 173, 168, 162, 157, 153, 148, 143, 139, 135, 131, 127, 123, 119, 115, 112,
    108, 105, 102, 99, 95, 93, 90, 87, 84, 82, 79, 77, 74, 72, 70, 68, 66, 64, 62, 60, 58, 56, 54,
    53, 51, 50, 48, 47, 45, 44, 42, 41, 40, 39, 37, 36, 35, 34, 33, 32, 31, 30, 29, 28, 27, 27, 26,
    25, 24, 23, 23,
];

fn exp_neg(index: i64) -> i64 {
    EXP_NEG[index.clamp(0, 255) as usize] as i64
}

/// exp(-i / 32) for a 16.16 index, interpolated between table entries.
fn exp_smooth(index: i64) -> i64 {
    let whole = index >> 16;
    if whole >= 255 {
        return 0;
    }
    let low = exp_neg(whole);
    low + ((exp_neg(whole + 1) - low) * (index & 0xffff) >> 16)
}

/// 1 - exp(-dt * rate), 16.16, for `rate` in hundredths per second.
fn ease(dt_ns: u64, rate: u64) -> i64 {
    let index = (dt_ns.min(400_000_000) * rate * 32 / 100 / 1_000_000_000) as i64;
    65_536 - exp_neg(index)
}

#[derive(Clone, Copy)]
struct Sim {
    /// Pointer in picture pixels, 16.16.
    mouse: (i64, i64),
    /// Pointer from -1 to 1 on each axis, 16.16.
    par: (i64, i64),
    /// 0 when the pointer is away, 1 (65536) when it is over the desktop.
    field: i64,
    last_ns: u64,
    moving: bool,
}

struct LiveSim(UnsafeCell<Sim>);

// SAFETY: only the desktop thread touches it.
unsafe impl Sync for LiveSim {}

static LIVE: LiveSim = LiveSim(UnsafeCell::new(Sim {
    mouse: (0, 0),
    par: (0, 0),
    field: 0,
    last_ns: 0,
    moving: false,
}));

/// Counts live Silk frames; only the wallpaper's own cache follows it.
static LIVE_FRAME: AtomicU64 = AtomicU64::new(0);

/// Changes with every frame the wallpaper itself draws differently. Glass
/// over it keys on `generation`, which the live Silk only bumps once it has
/// settled, so glass is frosted again at rest rather than on every frame.
pub(super) fn frame_generation() -> u64 {
    generation().wrapping_add(LIVE_FRAME.load(Ordering::Acquire))
}

fn settled(sim: &mut Sim) {
    if sim.moving {
        sim.moving = false;
        GENERATION.fetch_add(1, Ordering::AcqRel);
    }
}

/// What the wallpaper renderer needs to warp the picture this frame.
#[derive(Clone, Copy)]
pub(super) struct Warp {
    mouse: (i64, i64),
    shift: (i64, i64),
    field: i64,
    scale: i64,
}

/// The current warp, or None when the picture lies still.
pub(super) fn warp(source: &Source) -> Option<Warp> {
    let Source::Picture(_, _, height) = *source else {
        return None;
    };
    if current() >= VALLEY {
        return None;
    }
    // SAFETY: see `LiveSim`.
    let sim = unsafe { *LIVE.0.get() };
    let height = height as i64;
    let shift = (
        sim.par.0 * height * 1_573 >> 16,
        sim.par.1 * height * 983 >> 16,
    );
    if sim.field < 64 && shift.0 == 0 && shift.1 == 0 {
        return None;
    }
    Some(Warp {
        mouse: sim.mouse,
        shift,
        field: sim.field,
        // exp index for d^2 / 0.10 with d in picture heights, from squared
        // 8.8 pixel distances: 320 * 2^48 / (65536 * height^2).
        scale: (320i64 << 32) / (height * height),
    })
}

impl Warp {
    /// Where the picture is sampled for the screen point that would
    /// otherwise show `(x, y)` (both 16.16 picture pixels), and how much
    /// cursor light to add there (0..255).
    #[inline]
    pub(super) fn apply(&self, x: u64, y: u64) -> (u64, u64, u32) {
        let x = x as i64;
        let y = y as i64;
        let dx = x - self.mouse.0;
        let dy = y - self.mouse.1;
        let distance = (dx >> 8) * (dx >> 8) + (dy >> 8) * (dy >> 8);
        let index = (distance * self.scale) >> 32;
        let mut light = 0;
        let (mut wx, mut wy) = (x, y);
        if index < 255 << 16 {
            let push = (exp_smooth(index) * self.field >> 16) * 40_632 >> 16;
            wx -= dx * push >> 16;
            wy -= dy * push >> 16;
            light = ((exp_smooth(index * 2) * self.field >> 16) * 9 >> 16) as u32;
        }
        (
            (wx + self.shift.0).max(0) as u64,
            (wy + self.shift.1).max(0) as u64,
            light,
        )
    }
}

/// Moves the simulation on by one frame towards the pointer (in picture
/// pixels, 16.16, and as -1..1 of the screen). True when the picture moved
/// and needs drawing again.
pub(super) fn live_step(
    active: bool,
    pointer: (i64, i64),
    par: (i64, i64),
    now: u64,
    height: i64,
) -> bool {
    // SAFETY: see `LiveSim`.
    let sim = unsafe { &mut *LIVE.0.get() };
    if !active && sim.field < 64 && sim.par == (0, 0) {
        sim.last_ns = now;
        settled(sim);
        return false;
    }
    let dt = now.saturating_sub(sim.last_ns);
    if dt < 30_000_000 {
        return false;
    }
    sim.last_ns = now;
    let field_target = if active { 65_536 } else { 0 };
    let par_target = if active { par } else { (0, 0) };
    if active && sim.field < 3_300 {
        sim.mouse = pointer;
    }
    let km = ease(dt, 700);
    let kf = ease(dt, 320);
    let kp = ease(dt, 220);
    let before = *sim;
    sim.mouse.0 += (pointer.0 - sim.mouse.0) * km >> 16;
    sim.mouse.1 += (pointer.1 - sim.mouse.1) * km >> 16;
    sim.par.0 += (par_target.0 - sim.par.0) * kp >> 16;
    sim.par.1 += (par_target.1 - sim.par.1) * kp >> 16;
    sim.field += (field_target - sim.field) * kf >> 16;
    let settle = |a: i64, b: i64, close: i64| if (a - b).abs() <= close { b } else { a };
    let still = height * 26; // 0.0004 of the picture height, 16.16
    sim.mouse.0 = settle(sim.mouse.0, pointer.0, still);
    sim.mouse.1 = settle(sim.mouse.1, pointer.1, still);
    sim.par.0 = settle(sim.par.0, par_target.0, 65);
    sim.par.1 = settle(sim.par.1, par_target.1, 65);
    sim.field = settle(sim.field, field_target, 131);
    let moved = sim.mouse != before.mouse || sim.par != before.par || sim.field != before.field;
    if moved {
        sim.moving = true;
        LIVE_FRAME.fetch_add(1, Ordering::AcqRel);
    } else {
        settled(sim);
    }
    moved
}

/// Grid columns the live wallpaper can evaluate the warp at (every
/// `LIVE_STEP` pixels): enough for an 8K-wide screen.
pub(super) const LIVE_COLUMNS: usize = 1_100;

struct LiveRows(UnsafeCell<[[i64; 3]; 3 * LIVE_COLUMNS]>);

// SAFETY: only the desktop thread touches it.
unsafe impl Sync for LiveRows {}

static LIVE_ROWS: LiveRows = LiveRows(UnsafeCell::new([[0; 3]; 3 * LIVE_COLUMNS]));

/// Scratch rows for `render_live_wallpaper`.
pub(super) fn live_rows() -> &'static mut [[i64; 3]; 3 * LIVE_COLUMNS] {
    // SAFETY: see `LiveRows`; one wallpaper render at a time.
    unsafe { &mut *LIVE_ROWS.0.get() }
}

/// Half-width sample rows for `render_live_wallpaper` (two of them).
pub(super) const LIVE_HALF: usize = 4 * LIVE_COLUMNS + 2;

struct LiveHalf(UnsafeCell<[u32; 2 * LIVE_HALF]>);

// SAFETY: only the desktop thread touches it.
unsafe impl Sync for LiveHalf {}

static LIVE_HALF_ROWS: LiveHalf = LiveHalf(UnsafeCell::new([0; 2 * LIVE_HALF]));

pub(super) fn live_half() -> &'static mut [u32; 2 * LIVE_HALF] {
    // SAFETY: see `LiveHalf`; one wallpaper render at a time.
    unsafe { &mut *LIVE_HALF_ROWS.0.get() }
}
