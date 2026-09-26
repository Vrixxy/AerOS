//! Screen capture: grabs the framebuffer and saves it as a PNG picture in
//! `/home/Pictures` (named after the local date and time).

// The Screenshot tool (UI pending) calls `save`.
#![allow(dead_code)]

use core::fmt::Write;

use crate::framebuffer::FrameBuffer;
use crate::image::{Image, ImageError};
use crate::shell::Text;
use crate::vfs;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScreenshotError {
    Image(ImageError),
    /// No place to keep it (the home volume isn't mounted).
    NoStorage,
    Write,
}

/// Copies the screen into a picture.
pub fn capture(frame: &FrameBuffer) -> Result<Image, ImageError> {
    let (width, height) = (frame.width(), frame.height());
    let mut image = Image::new(width, height)?;
    let pixels = image.rgba_mut();
    for y in 0..height {
        for x in 0..width {
            let color = frame
                .color_at(x as i32, y as i32)
                .unwrap_or(crate::framebuffer::Color::rgb(0, 0, 0));
            let at = (y * width + x) * 4;
            pixels[at] = color.red;
            pixels[at + 1] = color.green;
            pixels[at + 2] = color.blue;
            pixels[at + 3] = 255;
        }
    }
    Ok(image)
}

/// Writes a picture to `path` as PNG.
pub fn save_png(image: &Image, path: &str, alpha: bool) -> Result<(), ScreenshotError> {
    let (file, length) = crate::png::encode(image.width, image.height, image.rgba(), alpha)
        .map_err(ScreenshotError::Image)?;
    let descriptor =
        vfs::open_file(path, true, false, true, 0o644, true).map_err(|_| ScreenshotError::Write)?;
    let bytes = &file.as_slice()[..length];
    let mut written = 0;
    let mut ok = true;
    while written < bytes.len() {
        let count = (bytes.len() - written).min(16 * 1024);
        match vfs::write(descriptor, &bytes[written..written + count], false) {
            Ok(done) if done > 0 => written += done,
            _ => {
                ok = false;
                break;
            }
        }
    }
    let _ = vfs::close(descriptor);
    if ok {
        Ok(())
    } else {
        Err(ScreenshotError::Write)
    }
}

/// Captures the screen into `/home/Pictures/Screenshot <date> <time>.png`
/// (a counter is added if that second already has one). Returns the path.
pub fn save(frame: &FrameBuffer) -> Result<Text<96>, ScreenshotError> {
    if crate::datafs::route("/home").is_none() {
        return Err(ScreenshotError::NoStorage);
    }
    let image = capture(frame).map_err(ScreenshotError::Image)?;
    let now = crate::rtc::local_date_time();
    let second = crate::rtc::local_seconds() % 60;
    for attempt in 0..100u32 {
        let mut path: Text<96> = Text::new();
        let _ = write!(
            path,
            "/home/Pictures/Screenshot {:04}-{:02}-{:02} {:02}.{:02}.{:02}",
            now.year, now.month, now.day, now.hour, now.minute, second
        );
        if attempt > 0 {
            let _ = write!(path, " ({attempt})");
        }
        let _ = path.push_str_checked(".png");
        if vfs::metadata(path.as_str()).is_ok() {
            continue;
        }
        save_png(&image, path.as_str(), false)?;
        return Ok(path);
    }
    Err(ScreenshotError::Write)
}

/// Screenshots the boot screen, then reads the file back through the picture
/// decoder and compares.
#[cfg(feature = "boot-test")]
pub fn self_test(frame: &FrameBuffer) {
    use crate::serial;
    let Ok(path) = save(frame) else {
        serial::line("AEROS_SCREENSHOT saved=false verified=false");
        serial::line("AEROS_SCREENSHOT_INVARIANT_FAILURE");
        crate::arch::halt_forever();
    };
    let mut verified = false;
    let mut size = 0u64;
    if let Ok(metadata) = vfs::metadata(path.as_str()) {
        size = metadata.size;
        if let Ok(descriptor) = vfs::open_file(path.as_str(), false, false, false, 0, false) {
            if let Some(mut buffer) = crate::memory::PageBuffer::new(size as usize) {
                let mut total = 0usize;
                while total < size as usize {
                    match vfs::read(descriptor, &mut buffer.as_mut_slice()[total..]) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => total += count,
                    }
                }
                if total == size as usize
                    && let (Ok(decoded), Ok(original)) = (
                        crate::image::decode(&buffer.as_slice()[..total]),
                        capture(frame),
                    )
                {
                    verified = decoded.width == original.width
                        && decoded.height == original.height
                        && decoded.rgba() == original.rgba();
                }
            }
            let _ = vfs::close(descriptor);
        }
    }
    serial::format(format_args!(
        "AEROS_SCREENSHOT saved=true file_bytes={} width={} height={} verified={}\n",
        size,
        frame.width(),
        frame.height(),
        verified
    ));
    if !verified {
        serial::line("AEROS_SCREENSHOT_INVARIANT_FAILURE");
        crate::arch::halt_forever();
    }
    let _ = vfs::remove(path.as_str(), false);
}
