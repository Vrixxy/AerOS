#[derive(Clone, Copy)]
pub enum PixelFormat {
    Rgb,
    Bgr,
    Bitmask { red: u32, green: u32, blue: u32 },
}

#[derive(Clone, Copy)]
pub struct FrameBufferInfo {
    pub address: *mut u32,
    pub size: usize,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub format: PixelFormat,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Color {
    pub const fn rgb(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }
}

pub struct FrameBuffer {
    address: *mut u32,
    pixels: usize,
    width: usize,
    height: usize,
    stride: usize,
    format: PixelFormat,
}

/// Screen brightness in percent, applied as pixels are copied to the screen.
static BRIGHTNESS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(100);
static BRIGHTNESS_SHOWN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(100);

pub fn set_brightness(percent: u8) {
    BRIGHTNESS.store(
        percent.clamp(5, 100) as u32,
        core::sync::atomic::Ordering::Relaxed,
    );
}

fn dim_pixel(value: u32, percent: u32) -> u32 {
    if percent >= 100 {
        return value;
    }
    let scale = |shift: u32| (((value >> shift) & 0xff) * percent / 100) << shift;
    (value & 0xff00_0000) | scale(16) | scale(8) | scale(0)
}

impl FrameBuffer {
    pub unsafe fn new(info: FrameBufferInfo) -> Self {
        Self {
            address: info.address,
            pixels: info.size / core::mem::size_of::<u32>(),
            width: info.width,
            height: info.height,
            stride: info.stride,
            format: info.format,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn info(&self) -> FrameBufferInfo {
        FrameBufferInfo {
            address: self.address,
            size: self.pixels.saturating_mul(core::mem::size_of::<u32>()),
            width: self.width,
            height: self.height,
            stride: self.stride,
            format: self.format,
        }
    }

    pub fn pack_color(&self, color: Color) -> u32 {
        self.pack(color)
    }

    pub fn blit_packed(&mut self, packed: &[u32]) -> bool {
        let active_pixels = self.stride.saturating_mul(self.height);
        if active_pixels > self.pixels || active_pixels > packed.len() {
            return false;
        }
        for (index, value) in packed.iter().enumerate().take(active_pixels) {
            unsafe {
                core::ptr::write_volatile(self.address.add(index), *value);
            }
        }
        true
    }

    /// Same as `blit_packed` but restricted to a sub-rectangle of `packed`
    /// (which must share this framebuffer's stride/height layout). Each
    /// pixel is a separate volatile MMIO write, so this is the cheap way to
    /// refresh a small moving region without paying for the whole screen.
    pub fn blit_packed_region(
        &mut self,
        packed: &[u32],
        x: usize,
        y: usize,
        width: usize,
        height: usize,
    ) -> bool {
        let active_pixels = self.stride.saturating_mul(self.height);
        if active_pixels > self.pixels
            || active_pixels > packed.len()
            || x.saturating_add(width) > self.stride
            || y.saturating_add(height) > self.height
        {
            return false;
        }
        for row in 0..height {
            let row_start = (y + row) * self.stride + x;
            for col in 0..width {
                let index = row_start + col;
                unsafe {
                    core::ptr::write_volatile(self.address.add(index), packed[index]);
                }
            }
        }
        true
    }

    pub fn present_full(&mut self, source: &Self, shadow: &mut [u32]) -> bool {
        let active_pixels = self.stride.saturating_mul(self.height);
        if self.width != source.width
            || self.height != source.height
            || self.stride != source.stride
            || active_pixels > self.pixels
            || active_pixels > source.pixels
            || active_pixels > shadow.len()
        {
            return false;
        }
        for (index, slot) in shadow.iter_mut().enumerate().take(active_pixels) {
            let value = unsafe { core::ptr::read(source.address.add(index)) };
            *slot = value;
            unsafe {
                core::ptr::write_volatile(self.address.add(index), value);
            }
        }
        true
    }

    pub fn present_diff(&mut self, source: &Self, shadow: &mut [u32]) -> bool {
        let active_pixels = self.stride.saturating_mul(self.height);
        if self.width != source.width
            || self.height != source.height
            || self.stride != source.stride
            || active_pixels > self.pixels
            || active_pixels > source.pixels
            || active_pixels > shadow.len()
        {
            return false;
        }
        let percent = BRIGHTNESS.load(core::sync::atomic::Ordering::Relaxed);
        let changed =
            BRIGHTNESS_SHOWN.swap(percent, core::sync::atomic::Ordering::Relaxed) != percent;
        for (index, slot) in shadow.iter_mut().enumerate().take(active_pixels) {
            let value = unsafe { core::ptr::read(source.address.add(index)) };
            if changed || *slot != value {
                *slot = value;
                unsafe {
                    core::ptr::write_volatile(self.address.add(index), dim_pixel(value, percent));
                }
            }
        }
        true
    }

    pub fn restore_region(&mut self, shadow: &[u32], x: i32, y: i32, width: usize, height: usize) {
        for row in 0..height as i32 {
            let py = y + row;
            if py < 0 || py as usize >= self.height {
                continue;
            }
            for col in 0..width as i32 {
                let px = x + col;
                if px < 0 || px as usize >= self.width {
                    continue;
                }
                let index = py as usize * self.stride + px as usize;
                if index < shadow.len() {
                    let percent = BRIGHTNESS.load(core::sync::atomic::Ordering::Relaxed);
                    self.write(px as usize, py as usize, dim_pixel(shadow[index], percent));
                }
            }
        }
    }

    pub fn present_from(&mut self, source: &Self) -> bool {
        let active_pixels = self.stride.saturating_mul(self.height);
        if self.width != source.width
            || self.height != source.height
            || self.stride != source.stride
            || active_pixels > self.pixels
            || active_pixels > source.pixels
        {
            return false;
        }
        for index in 0..active_pixels {
            let value = unsafe { core::ptr::read(source.address.add(index)) };
            unsafe {
                core::ptr::write_volatile(self.address.add(index), value);
            }
        }
        true
    }

    pub fn clear(&mut self, color: Color) {
        let packed = self.pack(color);
        for y in 0..self.height {
            let row = y * self.stride;
            for x in 0..self.width {
                let index = row + x;
                if index < self.pixels {
                    unsafe {
                        core::ptr::write_volatile(self.address.add(index), packed);
                    }
                }
            }
        }
    }

    pub fn vertical_gradient(&mut self, top: Color, bottom: Color) {
        let divisor = self.height.saturating_sub(1).max(1) as u32;
        for y in 0..self.height {
            let amount = y as u32;
            let color = Color::rgb(
                interpolate(top.red, bottom.red, amount, divisor),
                interpolate(top.green, bottom.green, amount, divisor),
                interpolate(top.blue, bottom.blue, amount, divisor),
            );
            self.rectangle(0, y as i32, self.width as i32, 1, color);
        }
    }

    pub fn pixel(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || y < 0 {
            return;
        }
        let x = x as usize;
        let y = y as usize;
        if x >= self.width || y >= self.height {
            return;
        }
        self.write(x, y, self.pack(color));
    }

    pub fn blend(&mut self, x: i32, y: i32, color: Color, alpha: u8) {
        if x < 0 || y < 0 {
            return;
        }
        let x = x as usize;
        let y = y as usize;
        if x >= self.width || y >= self.height {
            return;
        }
        let current = self.unpack(self.read(x, y));
        let alpha = alpha as u16;
        let inverse = 255 - alpha;
        let result = Color::rgb(
            ((current.red as u16 * inverse + color.red as u16 * alpha) / 255) as u8,
            ((current.green as u16 * inverse + color.green as u16 * alpha) / 255) as u8,
            ((current.blue as u16 * inverse + color.blue as u16 * alpha) / 255) as u8,
        );
        self.write(x, y, self.pack(result));
    }

    pub fn color_at(&self, x: i32, y: i32) -> Option<Color> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return None;
        }
        Some(self.unpack(self.read(x as usize, y as usize)))
    }

    pub fn rectangle(&mut self, x: i32, y: i32, width: i32, height: i32, color: Color) {
        if width <= 0 || height <= 0 {
            return;
        }
        let left = x.max(0);
        let top = y.max(0);
        let right = x.saturating_add(width).min(self.width as i32);
        let bottom = y.saturating_add(height).min(self.height as i32);
        let packed = self.pack(color);
        for py in top..bottom {
            for px in left..right {
                self.write(px as usize, py as usize, packed);
            }
        }
    }

    pub fn rounded_rectangle(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        radius: i32,
        color: Color,
    ) {
        let left = x.max(0);
        let top = y.max(0);
        let right = x.saturating_add(width).min(self.width as i32);
        let bottom = y.saturating_add(height).min(self.height as i32);
        for py in top..bottom {
            for px in left..right {
                if rounded_contains(px, py, x, y, width, height, radius) {
                    self.pixel(px, py, color);
                }
            }
        }
    }

    fn read(&self, x: usize, y: usize) -> u32 {
        let index = y.saturating_mul(self.stride).saturating_add(x);
        if index >= self.pixels {
            return 0;
        }
        unsafe { core::ptr::read_volatile(self.address.add(index)) }
    }

    fn write(&mut self, x: usize, y: usize, value: u32) {
        let index = y.saturating_mul(self.stride).saturating_add(x);
        if index < self.pixels {
            unsafe {
                core::ptr::write_volatile(self.address.add(index), value);
            }
        }
    }

    fn pack(&self, color: Color) -> u32 {
        match self.format {
            PixelFormat::Rgb => {
                color.red as u32 | (color.green as u32) << 8 | (color.blue as u32) << 16
            }
            PixelFormat::Bgr => {
                color.blue as u32 | (color.green as u32) << 8 | (color.red as u32) << 16
            }
            PixelFormat::Bitmask { red, green, blue } => {
                pack_channel(color.red, red)
                    | pack_channel(color.green, green)
                    | pack_channel(color.blue, blue)
            }
        }
    }

    fn unpack(&self, value: u32) -> Color {
        match self.format {
            PixelFormat::Rgb => Color::rgb(value as u8, (value >> 8) as u8, (value >> 16) as u8),
            PixelFormat::Bgr => Color::rgb((value >> 16) as u8, (value >> 8) as u8, value as u8),
            PixelFormat::Bitmask { red, green, blue } => Color::rgb(
                unpack_channel(value, red),
                unpack_channel(value, green),
                unpack_channel(value, blue),
            ),
        }
    }
}

fn interpolate(start: u8, end: u8, amount: u32, divisor: u32) -> u8 {
    ((start as u32 * (divisor - amount) + end as u32 * amount) / divisor) as u8
}

fn rounded_contains(
    px: i32,
    py: i32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    radius: i32,
) -> bool {
    if width <= 0 || height <= 0 || px < x || py < y || px >= x + width || py >= y + height {
        return false;
    }
    let radius = radius.max(0).min(width / 2).min(height / 2);
    let cx = if px < x + radius {
        x + radius
    } else if px >= x + width - radius {
        x + width - radius - 1
    } else {
        px
    };
    let cy = if py < y + radius {
        y + radius
    } else if py >= y + height - radius {
        y + height - radius - 1
    } else {
        py
    };
    let dx = px - cx;
    let dy = py - cy;
    dx * dx + dy * dy <= radius * radius
}

fn pack_channel(value: u8, mask: u32) -> u32 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let maximum = mask >> shift;
    ((value as u32 * maximum + 127) / 255) << shift
}

fn unpack_channel(value: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let maximum = mask >> shift;
    (((value & mask) >> shift) * 255 / maximum) as u8
}
