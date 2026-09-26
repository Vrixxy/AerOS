use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

pub const WIDTH: usize = 132;
pub const HEIGHT: usize = 88;
const SAMPLES: usize = 3;
const SOURCE_LEFT: i32 = 20;
const SOURCE_TOP: i32 = 24;
const SOURCE_WIDTH: i32 = 780;
const SOURCE_HEIGHT: i32 = 520;

const LEAF: [(i32, i32); 48] = [
    (462, 32),
    (490, 40),
    (520, 58),
    (550, 85),
    (580, 108),
    (610, 125),
    (650, 131),
    (700, 130),
    (745, 133),
    (775, 148),
    (788, 170),
    (788, 200),
    (778, 235),
    (762, 270),
    (740, 305),
    (712, 340),
    (680, 375),
    (645, 410),
    (610, 443),
    (575, 468),
    (545, 485),
    (520, 496),
    (503, 505),
    (487, 461),
    (479, 493),
    (450, 510),
    (410, 522),
    (375, 526),
    (352, 525),
    (321, 462),
    (310, 497),
    (285, 495),
    (255, 478),
    (225, 450),
    (198, 418),
    (178, 385),
    (165, 350),
    (160, 320),
    (158, 280),
    (165, 240),
    (185, 195),
    (210, 150),
    (245, 108),
    (285, 72),
    (330, 50),
    (370, 40),
    (403, 35),
    (411, 115),
];
const LEAF_POINTS: usize = 48;

const STEM: [(i32, i32); 10] = [
    (58, 303),
    (120, 318),
    (200, 335),
    (280, 345),
    (350, 348),
    (410, 340),
    (470, 322),
    (540, 296),
    (610, 262),
    (628, 250),
];
const STEM_WIDTH: i32 = 44;
const STEM_OUTLINE_WIDTH: i32 = 56;
const STEM_OUTLINE_POINTS: usize = 4;

type Vein = ((i32, i32), (i32, i32), i32);

const VEINS: [Vein; 4] = [
    ((335, 318), (362, 182), 40),
    ((452, 308), (484, 152), 40),
    ((392, 362), (438, 412), 38),
    ((512, 322), (574, 368), 38),
];

struct Pixels(UnsafeCell<[u8; WIDTH * HEIGHT * 4]>);
unsafe impl Sync for Pixels {}

static PIXELS: Pixels = Pixels(UnsafeCell::new([0; WIDTH * HEIGHT * 4]));
static GREEN: Pixels = Pixels(UnsafeCell::new([0; WIDTH * HEIGHT * 4]));
static GREEN_READY: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);

fn inside_leaf(x: i32, y: i32) -> bool {
    let mut inside = false;
    let mut previous = LEAF[LEAF_POINTS - 1];
    for &(ax, ay) in LEAF.iter().take(LEAF_POINTS) {
        let (bx, by) = previous;
        if (ay > y) != (by > y) {
            let cross = (bx - ax) as i64 * (y - ay) as i64 / (by - ay) as i64 + ax as i64;
            if (x as i64) < cross {
                inside = !inside;
            }
        }
        previous = (ax, ay);
    }
    inside
}

fn near_segment(x: i32, y: i32, a: (i32, i32), b: (i32, i32), width: i32) -> bool {
    let (dx, dy) = ((b.0 - a.0) as i64, (b.1 - a.1) as i64);
    let (px, py) = ((x - a.0) as i64, (y - a.1) as i64);
    let length = dx * dx + dy * dy;
    let t = if length == 0 {
        0
    } else {
        ((px * dx + py * dy) * 1024 / length).clamp(0, 1024)
    };
    let (cx, cy) = (dx * t / 1024, dy * t / 1024);
    let (ex, ey) = (px - cx, py - cy);
    let radius = (width / 2) as i64;
    ex * ex + ey * ey <= radius * radius
}

fn near_polyline(x: i32, y: i32, points: &[(i32, i32)], width: i32) -> bool {
    points
        .windows(2)
        .any(|pair| near_segment(x, y, pair[0], pair[1], width))
}

fn covered(x: i32, y: i32) -> bool {
    let white =
        inside_leaf(x, y) || near_polyline(x, y, &STEM[..STEM_OUTLINE_POINTS], STEM_OUTLINE_WIDTH);
    if !white {
        return false;
    }
    if near_polyline(x, y, &STEM, STEM_WIDTH) {
        return false;
    }
    !VEINS
        .iter()
        .any(|(a, b, width)| near_segment(x, y, *a, *b, *width))
}

fn build() {
    let pixels = unsafe { &mut *PIXELS.0.get() };
    for row in 0..HEIGHT {
        for column in 0..WIDTH {
            let mut hits = 0u32;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let fx = (column * SAMPLES + sx) as i32 * 2 + 1;
                    let fy = (row * SAMPLES + sy) as i32 * 2 + 1;
                    let x = SOURCE_LEFT + fx * SOURCE_WIDTH / (2 * (WIDTH * SAMPLES) as i32);
                    let y = SOURCE_TOP + fy * SOURCE_HEIGHT / (2 * (HEIGHT * SAMPLES) as i32);
                    if covered(x, y) {
                        hits += 1;
                    }
                }
            }
            let alpha = (hits * 255 / (SAMPLES * SAMPLES) as u32) as u8;
            let at = (row * WIDTH + column) * 4;
            pixels[at] = 255;
            pixels[at + 1] = 255;
            pixels[at + 2] = 255;
            pixels[at + 3] = alpha;
        }
    }
}

/// The AerOS leaf as straight RGBA (white, alpha = shape), built on first use.
pub fn rgba() -> &'static [u8] {
    if !READY.load(Ordering::Acquire) {
        build();
        READY.store(true, Ordering::Release);
    }
    unsafe { &*PIXELS.0.get() }
}

/// Aspect ratio helper: the height for a given width.
pub fn height_for(width: i32) -> i32 {
    width * HEIGHT as i32 / WIDTH as i32
}

#[cfg(feature = "boot-test")]
pub fn self_test() -> bool {
    let data = rgba();
    let alpha = |x: usize, y: usize| data[(y * WIDTH + x) * 4 + 3];
    let opaque = data.chunks(4).filter(|pixel| pixel[3] > 200).count();
    let clear = data.chunks(4).filter(|pixel| pixel[3] < 20).count();
    let corner = alpha(1, 1);
    let ok = opaque > WIDTH * HEIGHT / 5 && clear > WIDTH * HEIGHT / 5 && corner < 20;
    crate::serial::format(format_args!(
        "AEROS_LOGO opaque={} clear={} verified={}\n",
        opaque, clear, ok
    ));
    ok
}

/// The leaf tinted green (for the battery-saver icon).
pub fn green_rgba() -> &'static [u8] {
    let white = rgba();
    if !GREEN_READY.load(Ordering::Acquire) {
        let green = unsafe { &mut *GREEN.0.get() };
        for (index, pixel) in white.chunks(4).enumerate() {
            green[index * 4] = 92;
            green[index * 4 + 1] = 170;
            green[index * 4 + 2] = 74;
            green[index * 4 + 3] = pixel[3];
        }
        GREEN_READY.store(true, Ordering::Release);
    }
    unsafe { &*GREEN.0.get() }
}
