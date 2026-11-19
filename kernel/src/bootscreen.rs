//! Mirrors the serial log onto the screen until the desktop starts
//! (`--features screen-log`), for machines without a serial port.

use crate::font::RasterFont;
use crate::framebuffer::{Color, FrameBuffer, FrameBufferInfo};
use crate::sync::TicketLock;

const TEXT_HEIGHT: i32 = 15;
const LINE_HEIGHT: i32 = 19;
const MARGIN: i32 = 8;

struct Screen {
    info: Option<FrameBufferInfo>,
    font: Option<RasterFont>,
    line: [u8; 240],
    len: usize,
    y: i32,
}

// SAFETY: the framebuffer pointer is only used under the lock.
unsafe impl Send for Screen {}

static SCREEN: TicketLock<Screen> = TicketLock::new(Screen {
    info: None,
    font: None,
    line: [0; 240],
    len: 0,
    y: MARGIN,
});

pub fn start(info: FrameBufferInfo, font: Option<RasterFont>) {
    let mut screen = SCREEN.lock();
    unsafe { FrameBuffer::new(info) }.clear(Color::rgb(0, 0, 0));
    screen.info = Some(info);
    screen.font = font;
    screen.y = MARGIN;
    screen.len = 0;
}

pub fn stop() {
    SCREEN.lock().info = None;
}

pub fn byte(value: u8) {
    let Some(mut screen) = SCREEN.try_lock() else {
        return;
    };
    let (Some(info), Some(font)) = (screen.info, screen.font) else {
        return;
    };
    match value {
        b'\r' => {}
        b'\n' => {
            let mut frame = unsafe { FrameBuffer::new(info) };
            if screen.y + LINE_HEIGHT > info.height as i32 {
                frame.clear(Color::rgb(0, 0, 0));
                screen.y = MARGIN;
            }
            let length = screen.len;
            let text = core::str::from_utf8(&screen.line[..length]).unwrap_or("?");
            let color = if text.contains("FAILURE") || text.contains("ERROR") {
                Color::rgb(255, 120, 110)
            } else {
                Color::rgb(210, 210, 210)
            };
            font.draw(&mut frame, MARGIN, screen.y, text, TEXT_HEIGHT, color);
            screen.y += LINE_HEIGHT;
            screen.len = 0;
        }
        _ => {
            if screen.len < screen.line.len() && value.is_ascii() {
                let at = screen.len;
                screen.line[at] = value;
                screen.len += 1;
            }
        }
    }
}
