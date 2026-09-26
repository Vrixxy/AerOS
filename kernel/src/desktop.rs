use core::cell::UnsafeCell;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};

use crate::aerui::{
    CornerRadii, FrostReport, FrostStyle, Painter, Point, Rect, Rgba, Scale, fit_rect,
};
use crate::arch;
use crate::button::{self, Button, ButtonStyles};
use crate::font::{FontCatalog, RasterFont};
use crate::framebuffer::{Color, FrameBuffer, FrameBufferInfo};
use crate::keyboard;
use crate::serial;
use crate::shell;
use crate::vfs;

#[path = "desktop_flow.rs"]
mod flow;
#[path = "desktop_panels.rs"]
mod panels;
#[path = "desktop_search.rs"]
mod search;
#[path = "desktop_shell.rs"]
mod shellui;

const DESIGN_WIDTH: i32 = 752;
const DESIGN_HEIGHT: i32 = 458;
const WALLPAPER_WIDTH: usize = 4_148;
const WALLPAPER_HEIGHT: usize = 2_228;
const WALLPAPER: &[u8] = include_bytes!("../../assets/wallpapers/aeros-mountains.rgb565");
const MAX_DESKTOP_PIXELS: usize = 1_920 * 1_080;
const MAX_URL: usize = 128;
const MAX_PATH: usize = 96;
/// Set by the self-test so it exercises the built-in HTTP path even when a
/// Linux guest exists.
static FORCE_NATIVE_FETCH: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
/// Text lines visible in the browser page area, and one line's height.
const BROWSER_VISIBLE_LINES: usize = 17;
const BROWSER_LINE_HEIGHT: i32 = 14;
/// Width of one page character in thousandths of a logical pixel (measured
/// from the font when the page is drawn; used to find clicked links).
static BROWSER_CHAR_W_MILLI: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(6600);
const BROWSER_FAVORITES: [(&str, &str); 6] = [
    ("Wikipedia", "https://en.wikipedia.org"),
    ("DuckDuckGo", "https://lite.duckduckgo.com/lite"),
    ("Debian", "https://www.debian.org"),
    ("GitHub", "https://github.com"),
    ("Hacker News", "https://news.ycombinator.com"),
    ("Example", "https://example.com"),
];
const DOCK_HOLD_NS: u64 = 90_000_000;
const MOTION_TICK_HZ: u32 = 240;
/// How many TSC cycles the Linux guest runs per desktop loop iteration.
const LINUX_SLICE_TSC: u64 = 6_000_000;

/// F9 in the full Linux window: show the guest screen 1:1 (readable text)
/// instead of shrunk to fit; the view pans with the pointer.
static LINUX_ZOOM: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static LINUX_PAN_X: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);
static LINUX_PAN_Y: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);

fn linux_zoomed() -> bool {
    LINUX_ZOOM.load(core::sync::atomic::Ordering::Relaxed)
}

fn linux_pan() -> (i32, i32) {
    (
        LINUX_PAN_X.load(core::sync::atomic::Ordering::Relaxed),
        LINUX_PAN_Y.load(core::sync::atomic::Ordering::Relaxed),
    )
}
const OVERLAY_TRANSITION_NS: u64 = 450_000_000;
const OVERLAY_SLIDE_PX: i32 = 350;
const APP_ICON_FALL_NS: u64 = 140_000_000;
const APP_ICON_FALL_STAGGER_NS: u64 = 8_000_000;
const APP_ICON_FALL_PX: i32 = 22;
const DOCK_FALL_NS: u64 = 300_000_000;
const DOCK_FALL_STAGGER_NS: u64 = 25_000_000;
const DOCK_FALL_HEIGHT_PX: i32 = 60;
const WINDOW_OPEN_NS: u64 = 150_000_000;
const LOCK_LOGIN_SCROLL_NS: u64 = 320_000_000;
const SCREEN_TRANSITION_NS: u64 = 220_000_000;
const SCREEN_FADE_MAX_ALPHA: u32 = 130;
const MAX_NAME: usize = 20;
const LOGIN_ERROR_NS: u64 = 1_500_000_000;
/// Idle time on the desktop before the session locks itself.
const AUTO_LOCK_NS: u64 = 300_000_000_000;
const SHELL_LINE_MAX: usize = 56;
const SHELL_HISTORY_LINES: usize = 14;
const FILES_MAX_ENTRIES: usize = 12;
const FILES_GRID_COLUMNS: i32 = 3;
const FILES_GRID_ROWS: i32 = 4;
const NOTES_MAX_ENTRIES: usize = 7;
const TRASH_MAX_ENTRIES: usize = 6;
const NAME_INPUT_MAX: usize = 32;
const NOTE_MAX_BYTES: usize = 4_096;
/// Where Trash and Notes live: on the real home volume when there is one
/// (long names, any size, kept between runs), else in the small /data area.
fn trash_directory() -> &'static str {
    if crate::datafs::route("/home").is_some() {
        "/home/Trash"
    } else {
        "/data/.trash"
    }
}

fn notes_directory() -> &'static str {
    if crate::datafs::route("/home").is_some() {
        "/home/Notes"
    } else {
        "/data/Notes"
    }
}

fn ease_out_milli(progress_milli: u32) -> u32 {
    let inverse = 1000 - progress_milli.min(1000);
    1000 - (inverse * inverse) / 1000
}
const DOCK_BUTTONS: [&str; 7] = ["", "", "", "", "", "", ""];
const DOCK_NAMES: [&str; 7] = [
    "Apps",
    "Terminal",
    "Quick settings",
    "Files",
    "Browser",
    "Notes",
    "Trash",
];
const APP_LABELS: [&str; 20] = [
    "", "", "", "", "", "L", "M", "P", "E", "I", "D", "V", "R", "L", "H", "W", "G", "K", "U", "S",
];
const APP_NAMES: [&str; 20] = [
    "Terminal",
    "Files",
    "Browser",
    "Notes",
    "Settings",
    "Linux",
    "Mail",
    "Photos",
    "Editor",
    "Messages",
    "Downloads",
    "Video",
    "Reader",
    "Calendar",
    "Health",
    "Weather",
    "Games",
    "Clock",
    "Contacts",
    "Store",
];

struct DesktopBuffer(UnsafeCell<[u32; MAX_DESKTOP_PIXELS]>);

unsafe impl Sync for DesktopBuffer {}

static DESKTOP_BUFFER: DesktopBuffer = DesktopBuffer(UnsafeCell::new([0; MAX_DESKTOP_PIXELS]));
static SHADOW_BUFFER: DesktopBuffer = DesktopBuffer(UnsafeCell::new([0; MAX_DESKTOP_PIXELS]));

fn shadow_slice() -> &'static mut [u32] {
    let pointer = SHADOW_BUFFER.0.get() as *mut u32;
    unsafe { core::slice::from_raw_parts_mut(pointer, MAX_DESKTOP_PIXELS) }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    Apps,
    Quick,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DesktopApp {
    None,
    Browser,
    Settings,
    Files,
    Notes,
    Trash,
    Terminal,
    Linux,
    Store,
}

impl DesktopApp {
    const fn name(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::None => "none",
            Self::Browser => "browser",
            Self::Settings => "settings",
            Self::Files => "files",
            Self::Notes => "notes",
            Self::Trash => "trash",
            Self::Terminal => "terminal",
            Self::Store => "store",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Language,
    Keyboard,
    Username,
    Password,
    Confirm,
    Welcome,
    Lock,
    Login,
    Desktop,
}

impl Screen {
    const fn name(self) -> &'static str {
        match self {
            Self::Language => "language",
            Self::Keyboard => "keyboard",
            Self::Username => "username",
            Self::Password => "password",
            Self::Confirm => "confirm",
            Self::Welcome => "welcome",
            Self::Lock => "lock",
            Self::Login => "login",
            Self::Desktop => "desktop",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Language => Self::Keyboard,
            Self::Keyboard => Self::Username,
            Self::Username => Self::Password,
            Self::Password => Self::Confirm,
            Self::Confirm => Self::Welcome,
            Self::Welcome => Self::Lock,
            Self::Lock => Self::Login,
            Self::Login | Self::Desktop => Self::Desktop,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BrowserPhase {
    Editing,
    Loading,
    Loaded,
    DnsFailed,
    ConnectFailed,
    HttpError,
    HttpsUnsupported,
    BadUrl,
}

impl BrowserPhase {
    const fn name(self) -> &'static str {
        match self {
            Self::Editing => "editing",
            Self::Loading => "loading",
            Self::Loaded => "loaded",
            Self::DnsFailed => "dns-failed",
            Self::ConnectFailed => "connect-failed",
            Self::HttpError => "http-error",
            Self::HttpsUnsupported => "https-unsupported",
            Self::BadUrl => "bad-url",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FilesMode {
    Browsing,
    NamingFolder,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NotesMode {
    List,
    Naming,
    Editing,
}

#[derive(Clone, Copy)]
struct DirEntryRow {
    name: [u8; 48],
    name_len: u8,
    is_dir: bool,
    size: u64,
}

impl DirEntryRow {
    const EMPTY: Self = Self {
        name: [0; 48],
        name_len: 0,
        is_dir: false,
        size: 0,
    };

    fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }
}

fn list_directory(path: &str, rows: &mut [DirEntryRow]) -> (usize, bool) {
    for row in rows.iter_mut() {
        *row = DirEntryRow::EMPTY;
    }
    let Ok(descriptor) = vfs::open_directory(path) else {
        return (0, false);
    };
    let mut count = 0;
    let mut overflowed = false;
    let mut child: shell::Text<256> = shell::Text::new();
    loop {
        match vfs::next_directory_entry(descriptor) {
            Ok(Some(entry)) => {
                if count == rows.len() {
                    overflowed = true;
                    continue;
                }
                let name_len = (entry.name_len as usize).min(48);
                let mut row = DirEntryRow::EMPTY;
                row.name[..name_len].copy_from_slice(&entry.name[..name_len]);
                row.name_len = name_len as u8;
                row.is_dir = entry.kind == 4;
                let name = core::str::from_utf8(&entry.name[..name_len]).unwrap_or("");
                if shell::normalize_path(path, name, &mut child).is_ok()
                    && let Ok(metadata) = vfs::metadata(child.as_str())
                {
                    row.size = metadata.size;
                }
                rows[count] = row;
                count += 1;
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    let _ = vfs::close(descriptor);
    (count, overflowed)
}

/// Wraps `text` into `lines` at `chars_per_line`, breaking on the last space
/// before the limit when one exists. Pure byte-index slicing on purpose (no
/// font measurement in the loop) - callers pass ASCII-only text, so byte
/// offsets are always char boundaries and this stays O(n) even on a
/// pathological no-space input instead of the O(n^2) a pixel-precise
/// measure-and-shrink loop would risk on this compositor's tight per-frame
/// budget.
fn wrap_monospace<'a>(text: &'a str, chars_per_line: usize, lines: &mut [&'a str]) -> usize {
    let chars_per_line = chars_per_line.max(4);
    let mut count = 0;
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            if count < lines.len() {
                lines[count] = "";
                count += 1;
            }
            continue;
        }
        let bytes = paragraph.as_bytes();
        let mut start = 0;
        while start < paragraph.len() {
            let mut end = (start + chars_per_line).min(paragraph.len());
            if end < paragraph.len()
                && let Some(space) = paragraph[start..end].rfind(' ')
                && space > 0
            {
                end = start + space;
            }
            if end <= start {
                end = (start + 1).min(paragraph.len());
            }
            if count == lines.len() {
                return count;
            }
            lines[count] = paragraph[start..end].trim_end();
            count += 1;
            start = end;
            while bytes.get(start) == Some(&b' ') {
                start += 1;
            }
        }
    }
    count
}

/// Finds where the HTTP response body starts, i.e. just past the blank line
/// that ends the status line + headers (`\r\n\r\n`, or a bare `\n\n` from a
/// server that skips the CR). Falls back to 0 (treat the whole response as
/// body) if no such blank line is found, rather than silently dropping
/// everything - a malformed/unexpected response should still show *something*
/// instead of an empty page.
fn http_body_offset(response: &[u8]) -> usize {
    if let Some(index) = response.windows(4).position(|window| window == b"\r\n\r\n") {
        return index + 4;
    }
    if let Some(index) = response.windows(2).position(|window| window == b"\n\n") {
        return index + 2;
    }
    0
}

/// Cheap, heuristic check for `Transfer-Encoding: chunked` in the header
/// block: a plain case-insensitive substring search rather than parsing out
/// the specific header, since this is a text preview, not a security
/// boundary - a server that happens to mention "chunked" in an unrelated
/// header value would just get run through `dechunk` needlessly, which is
/// harmless (real chunk-size lines won't appear, so it returns 0 bytes and
/// the page shows as empty - `dechunk`'s own encoding validation already
/// leaves partial garbage instead in the far more common case: an oversized
/// hex-looking token, mid-chunk).
fn is_chunked(headers: &[u8]) -> bool {
    headers
        .windows(7)
        .any(|window| window.eq_ignore_ascii_case(b"chunked"))
}

/// Decodes HTTP/1.1 chunked transfer-encoding: each chunk is a hex size
/// line (optional `;extension` ignored), `\r\n`, that many bytes of data,
/// then `\r\n`, repeated until a zero-size chunk. Stops (rather than
/// panicking or reading out of bounds) on any framing it doesn't
/// recognize - a `net::http_get` response that got truncated mid-chunk
/// (the 2048-byte read cap, or a slow/dropped connection) is expected, not
/// a bug, so this returns whatever it managed to decode so far.
fn dechunk(input: &[u8], out: &mut [u8]) -> usize {
    let mut position = 0;
    let mut written = 0;
    while position < input.len() && written < out.len() {
        let Some(line_length) = input[position..]
            .windows(2)
            .position(|window| window == b"\r\n")
        else {
            break;
        };
        let size_line = &input[position..position + line_length];
        let size_text = size_line
            .split(|byte| *byte == b';')
            .next()
            .unwrap_or(size_line);
        let Ok(size_text) = core::str::from_utf8(size_text) else {
            break;
        };
        let Ok(size) = usize::from_str_radix(size_text.trim(), 16) else {
            break;
        };
        position += line_length + 2;
        if size == 0 {
            break;
        }
        let available = input.len().saturating_sub(position);
        let take = size.min(available).min(out.len() - written);
        out[written..written + take].copy_from_slice(&input[position..position + take]);
        written += take;
        if take < size {
            break;
        }
        position += size;
        if input[position..].starts_with(b"\r\n") {
            position += 2;
        } else {
            break;
        }
    }
    written
}

/// Common block-level HTML elements: ones that always start on a new line
/// visually, regardless of surrounding whitespace in the source. Used only
/// to decide where `extract_body_text` needs to insert a separating space
/// that the source bytes don't provide - not an exhaustive HTML5 element
/// list, just the tags real-world pages actually use for layout.
fn is_block_tag(name: &str) -> bool {
    const BLOCK_TAGS: [&str; 24] = [
        "p",
        "div",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "li",
        "ul",
        "ol",
        "br",
        "tr",
        "td",
        "th",
        "table",
        "section",
        "article",
        "header",
        "footer",
        "nav",
        "blockquote",
        "pre",
        "hr",
    ];
    BLOCK_TAGS.iter().any(|tag| name.eq_ignore_ascii_case(tag))
}

/// Strips tags (and `<script>`/`<style>`/`<title>` contents - title is
/// already shown separately, so it's excluded here to avoid duplicating it
/// into the body preview) from `html`, collapses
/// runs of whitespace to single spaces, and sanitizes anything outside
/// printable ASCII to a space - the renderer's bitmap fonts only cover
/// ASCII, and keeping the output pure ASCII also keeps every downstream byte
/// offset a valid char boundary for `wrap_monospace`. HTML entities are left
/// literal (`&amp;` stays `&amp;`) - a deliberate simplification, not a bug.
fn extract_body_text(html: &[u8], out: &mut [u8]) -> usize {
    let mut in_tag = false;
    let mut skip_content = false;
    let mut tag_buffer = [0u8; 8];
    let mut tag_len = 0usize;
    let mut written = 0usize;
    let mut last_was_space = true;
    let mut index = 0usize;
    while index < html.len() && written < out.len() {
        let byte = html[index];
        if byte == b'<' {
            in_tag = true;
            tag_len = 0;
            index += 1;
            continue;
        }
        if in_tag {
            if byte == b'>' {
                in_tag = false;
                let tag = core::str::from_utf8(&tag_buffer[..tag_len]).unwrap_or("");
                let name = tag.trim_start_matches('/');
                if name.eq_ignore_ascii_case("script")
                    || name.eq_ignore_ascii_case("style")
                    || name.eq_ignore_ascii_case("title")
                {
                    skip_content = !tag.starts_with('/');
                }
                // Block-level elements always start a new line even with no
                // source whitespace between them (e.g. "</h1><p>") - insert
                // one so words from adjacent blocks don't run together.
                // Inline tags (b, i, span, a, ...) deliberately do not do
                // this, matching the existing "world</b>!" self-test case.
                let tag_name = name.split(' ').next().unwrap_or(name);
                if is_block_tag(tag_name) && !last_was_space && written < out.len() {
                    out[written] = b' ';
                    written += 1;
                    last_was_space = true;
                }
            } else if tag_len < tag_buffer.len() {
                tag_buffer[tag_len] = byte;
                tag_len += 1;
            }
            index += 1;
            continue;
        }
        if skip_content {
            index += 1;
            continue;
        }
        let printable = if (0x20..=0x7e).contains(&byte) && byte != b' ' {
            Some(byte)
        } else {
            None
        };
        match printable {
            Some(byte) => {
                out[written] = byte;
                written += 1;
                last_was_space = false;
            }
            None if !last_was_space => {
                out[written] = b' ';
                written += 1;
                last_was_space = true;
            }
            None => {}
        }
        index += 1;
    }
    while written > 0 && out[written - 1] == b' ' {
        written -= 1;
    }
    written
}

pub fn text_processing_self_test() -> bool {
    let html = b"<html><head><title>t</title><style>a{color:red}</style></head>\
<body>  Hello,   <b>world</b>!<script>evil()</script>  Bye.</body></html>";
    let mut out = [0u8; 128];
    let length = extract_body_text(html, &mut out);
    let extracted = core::str::from_utf8(&out[..length]).unwrap_or("");
    let strip_ok = extracted == "Hello, world! Bye."
        && !extracted.contains("evil")
        && !extracted.contains("color");

    let block_html = b"<h1>Example Domain</h1><p>This domain is for use.</p>";
    let mut block_out = [0u8; 128];
    let block_length = extract_body_text(block_html, &mut block_out);
    let block_extracted = core::str::from_utf8(&block_out[..block_length]).unwrap_or("");
    let block_spacing_ok = block_extracted == "Example Domain This domain is for use.";

    let long = "the quick brown fox jumps over the lazy dog and keeps running";
    let mut lines: [&str; 8] = [""; 8];
    let count = wrap_monospace(long, 12, &mut lines);
    let wrap_ok = count > 1
        && lines[..count].iter().all(|line| line.chars().count() <= 12)
        && lines[..count].iter().all(|line| !line.starts_with(' '));
    let rejoined_ok = {
        let mut rejoined: [u8; 128] = [0; 128];
        let mut written = 0;
        for (index, line) in lines[..count].iter().enumerate() {
            if index != 0 {
                rejoined[written] = b' ';
                written += 1;
            }
            let bytes = line.as_bytes();
            rejoined[written..written + bytes.len()].copy_from_slice(bytes);
            written += bytes.len();
        }
        core::str::from_utf8(&rejoined[..written]).unwrap_or("") == long
    };

    let no_space_bytes = [b'a'; 40];
    let no_space = core::str::from_utf8(&no_space_bytes).unwrap_or("");
    let mut overflow_lines: [&str; 2] = [""; 2];
    let overflow_count = wrap_monospace(no_space, 12, &mut overflow_lines);
    let bounded_ok = overflow_count <= overflow_lines.len();

    let crlf_response = b"HTTP/1.1 200 OK\r\nServer: x\r\n\r\n<p>hi</p>";
    let crlf_ok = &crlf_response[http_body_offset(crlf_response)..] == b"<p>hi</p>";
    let lf_response = b"HTTP/1.1 200 OK\nServer: x\n\n<p>hi</p>";
    let lf_ok = &lf_response[http_body_offset(lf_response)..] == b"<p>hi</p>";
    let headerless = b"<p>no headers here</p>";
    let fallback_ok = http_body_offset(headerless) == 0;
    let body_offset_ok = crlf_ok && lf_ok && fallback_ok;

    let chunked_headers = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
    let chunked_body = b"4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
    let chunked_ok = is_chunked(chunked_headers) && !is_chunked(b"HTTP/1.1 200 OK\r\n\r\n");
    let mut dechunked = [0u8; 32];
    let dechunked_len = dechunk(chunked_body, &mut dechunked);
    let dechunk_ok = &dechunked[..dechunked_len] == b"Wikipedia";
    let mut tiny_out = [0u8; 3];
    let truncated_len = dechunk(chunked_body, &mut tiny_out);
    let dechunk_bounded_ok = truncated_len <= tiny_out.len();

    strip_ok
        && block_spacing_ok
        && wrap_ok
        && rejoined_ok
        && bounded_ok
        && body_offset_ok
        && chunked_ok
        && dechunk_ok
        && dechunk_bounded_ok
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UrlParse<'a> {
    Http { host: &'a str, path: &'a str },
    Https,
    Invalid,
}

fn parse_url(input: &str) -> UrlParse<'_> {
    let trimmed = input.trim();
    let rest = if let Some(stripped) = trimmed.strip_prefix("http://") {
        stripped
    } else if trimmed.starts_with("https://") {
        return UrlParse::Https;
    } else {
        trimmed
    };
    let (host, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    let host = host.split('@').next_back().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty()
        || host.len() > MAX_URL
        || path.len() > MAX_PATH
        || !host.contains('.')
        || host.starts_with('.')
        || host.ends_with('.')
        || !host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
        || !path.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return UrlParse::Invalid;
    }
    UrlParse::Http { host, path }
}

impl Overlay {
    const fn name(self) -> &'static str {
        match self {
            Self::None => "desktop",
            Self::Apps => "apps",
            Self::Quick => "quick",
        }
    }
}

#[derive(Clone, Copy)]
struct DesktopState {
    screen: Screen,
    overlay: Overlay,
    dock_focus: usize,
    app_focus: usize,
    focus_visible: bool,
    app: DesktopApp,
    browser_address: [u8; 4],
    browser_online: bool,
    browser_pending: bool,
    browser_http_status: u16,
    browser_http_bytes: usize,
    browser_title: [u8; 64],
    browser_title_len: usize,
    browser_heading: [u8; 64],
    browser_heading_len: usize,
    browser_body: shell::Text<2048>,
    url_input: [u8; MAX_URL],
    url_len: usize,
    url_active: bool,
    browser_phase: BrowserPhase,
    browser_host: [u8; MAX_URL],
    browser_host_len: usize,
    browser_path: [u8; MAX_PATH],
    browser_path_len: usize,
    wifi: bool,
    bluetooth: bool,
    microphone: bool,
    battery_saver: bool,
    dock_press_ns: [u64; DOCK_BUTTONS.len()],
    dock_release_ns: [u64; DOCK_BUTTONS.len()],
    motion_until_ns: u64,
    music_active: bool,
    music_playing: bool,
    music_track: u8,
    music_elapsed_ns: u64,
    music_started_ns: u64,
    music_shown_ns: u64,
    music_pause_ns: u64,
    music_expand_at_ns: u64,
    music_expand_until_ns: u64,
    music_track_ns: u64,
    volume: u8,
    volume_prev: u8,
    muted: bool,
    volume_event_ns: u64,
    volume_hud_until_ns: u64,
    overlay_open_at_ns: u64,
    screen_transition_at_ns: u64,
    username_input: [u8; MAX_NAME],
    username_len: usize,
    setup_password_input: [u8; MAX_NAME],
    setup_password_len: usize,
    confirm_input: [u8; MAX_NAME],
    confirm_len: usize,
    /// Salted password hash made at setup (the typed password is wiped).
    credential: Option<crate::auth::Credential>,
    lockout: crate::auth::Lockout,
    /// Password-hash work factor (lowered only by the self-tests).
    kdf_iterations: u32,
    /// Why the current setup step was refused ("" = nothing to show).
    setup_error: &'static str,
    setup_error_ns: u64,
    account_unsaved: bool,
    setup_typed_ns: u64,
    search_open: bool,
    search_at_ns: u64,
    search_input: [u8; search::SEARCH_MAX],
    search_len: usize,
    search_focus: usize,
    search_typed_ns: u64,
    hover_index: usize,
    hover_prev: usize,
    hover_at_ns: u64,
    quick_toggle_ns: [u64; 4],
    quick_focus: usize,
    brightness: u8,
    brightness_prev: u8,
    brightness_event_ns: u64,
    brightness_hud_until_ns: u64,
    notif_seen: u32,
    toast_at_ns: u64,
    notif_open: bool,
    notif_at_ns: u64,
    ctx_open: bool,
    ctx_at_ns: u64,
    ctx_origin: Point,
    ctx_focus: usize,
    clip_at_ns: u64,
    power_open: bool,
    power_at_ns: u64,
    power_focus: usize,
    power_flyout: bool,
    power_flyout_ns: u64,
    power_action: shellui::PowerAction,
    power_action_at_ns: u64,
    kb_search: [u8; flow::KB_SEARCH_MAX],
    kb_search_len: usize,
    kb_cursor: usize,
    kb_scroll_from: i32,
    kb_scroll_target: i32,
    kb_scroll_at_ns: u64,
    login_input: [u8; MAX_NAME],
    login_len: usize,
    login_error_until_ns: u64,
    login_typed_ns: u64,
    persist_account: bool,
    terminal_input: [u8; SHELL_LINE_MAX],
    terminal_input_len: usize,
    terminal_lines: [[u8; SHELL_LINE_MAX]; SHELL_HISTORY_LINES],
    terminal_line_lens: [usize; SHELL_HISTORY_LINES],
    terminal_line_count: usize,
    terminal_elevated: bool,
    /// 0 = typing live, not browsing history. N>0 = showing the command
    /// N-1 entries back from the most recent (Up increments, Down
    /// decrements back to 0, which restores whatever was being typed
    /// before history browsing started).
    terminal_history_index: usize,
    terminal_draft: [u8; SHELL_LINE_MAX],
    terminal_draft_len: usize,
    seamless: Seamless,
    clip_open: bool,
    /// The last pointer input looked like a finger (a jump, not a glide).
    touch_mode: bool,
    /// The on-screen keyboard is showing.
    osk_open: bool,
    osk_shift: bool,
    osk_symbols: bool,
    /// The user hid the keyboard for the current text field.
    osk_dismissed: bool,
    /// The store's search field was tapped (so the touch keyboard may show).
    store_search_focus: bool,
    /// Scancodes the keyboard has queued that are not from real keys.
    osk_pending: u32,
    /// Shield toast: the last event shown, its text and when it goes away.
    shield_seen: u32,
    shield_text: [u8; 56],
    shield_len: usize,
    shield_until_ns: u64,
    clip_selected: usize,
    /// Clipboard generation last handed to the guest (MAX = never).
    clip_synced: u32,
    dock_entrance_at_ns: u64,
    window_open_at_ns: u64,
    previous_screen: Screen,
    overlay_closing: Overlay,
    overlay_close_at_ns: u64,
    app_closing: DesktopApp,
    app_close_at_ns: u64,
    files_path: shell::Text<256>,
    files_entries: [DirEntryRow; FILES_MAX_ENTRIES],
    files_entry_count: usize,
    files_overflow: bool,
    files_selected: usize,
    files_mode: FilesMode,
    files_status: shell::Text<80>,
    notes_entries: [DirEntryRow; NOTES_MAX_ENTRIES],
    notes_entry_count: usize,
    notes_overflow: bool,
    notes_mode: NotesMode,
    notes_current_name: shell::Text<40>,
    notes_content: shell::Text<NOTE_MAX_BYTES>,
    notes_status: shell::Text<80>,
    trash_entries: [DirEntryRow; TRASH_MAX_ENTRIES],
    trash_entry_count: usize,
    trash_overflow: bool,
    trash_selected: usize,
    trash_status: shell::Text<80>,
    name_input: shell::Text<NAME_INPUT_MAX>,
}

impl DesktopState {
    const fn new() -> Self {
        Self {
            screen: Screen::Language,
            overlay: Overlay::None,
            dock_focus: 0,
            app_focus: 0,
            focus_visible: false,
            app: DesktopApp::None,
            browser_address: [0; 4],
            browser_online: false,
            browser_pending: false,
            browser_http_status: 0,
            browser_http_bytes: 0,
            browser_title: [0; 64],
            browser_title_len: 0,
            browser_heading: [0; 64],
            browser_heading_len: 0,
            browser_body: shell::Text::new(),
            url_input: [0; MAX_URL],
            url_len: 0,
            url_active: false,
            browser_phase: BrowserPhase::Editing,
            browser_host: [0; MAX_URL],
            browser_host_len: 0,
            browser_path: [0; MAX_PATH],
            browser_path_len: 0,
            wifi: true,
            bluetooth: true,
            microphone: true,
            battery_saver: false,
            dock_press_ns: [0; DOCK_BUTTONS.len()],
            dock_release_ns: [0; DOCK_BUTTONS.len()],
            motion_until_ns: 0,
            music_active: false,
            music_playing: false,
            music_track: 0,
            music_elapsed_ns: 0,
            music_started_ns: 0,
            music_shown_ns: 0,
            music_pause_ns: 0,
            music_expand_at_ns: 0,
            music_expand_until_ns: 0,
            music_track_ns: 0,
            volume: 42,
            volume_prev: 42,
            muted: false,
            volume_event_ns: 0,
            volume_hud_until_ns: 0,
            overlay_open_at_ns: 0,
            screen_transition_at_ns: 0,
            username_input: [0; MAX_NAME],
            username_len: 0,
            setup_password_input: [0; MAX_NAME],
            setup_password_len: 0,
            confirm_input: [0; MAX_NAME],
            confirm_len: 0,
            credential: None,
            lockout: crate::auth::Lockout::new(),
            kdf_iterations: crate::auth::ITERATIONS,
            setup_error: "",
            setup_error_ns: 0,
            account_unsaved: false,
            setup_typed_ns: 0,
            search_open: false,
            search_at_ns: 0,
            search_input: [0; search::SEARCH_MAX],
            search_len: 0,
            search_focus: 0,
            search_typed_ns: 0,
            hover_index: usize::MAX,
            hover_prev: usize::MAX,
            hover_at_ns: 0,
            quick_toggle_ns: [0; 4],
            quick_focus: 0,
            brightness: 100,
            brightness_prev: 100,
            brightness_event_ns: 0,
            brightness_hud_until_ns: 0,
            notif_seen: 0,
            toast_at_ns: 0,
            notif_open: false,
            notif_at_ns: 0,
            ctx_open: false,
            ctx_at_ns: 0,
            ctx_origin: Point::new(0, 0),
            ctx_focus: 0,
            clip_at_ns: 0,
            power_open: false,
            power_at_ns: 0,
            power_focus: 0,
            power_flyout: false,
            power_flyout_ns: 0,
            power_action: shellui::PowerAction::None,
            power_action_at_ns: 0,
            kb_search: [0; flow::KB_SEARCH_MAX],
            kb_search_len: 0,
            kb_cursor: 0,
            kb_scroll_from: 0,
            kb_scroll_target: 0,
            kb_scroll_at_ns: 0,
            login_input: [0; MAX_NAME],
            login_len: 0,
            login_error_until_ns: 0,
            login_typed_ns: 0,
            persist_account: false,
            terminal_input: [0; SHELL_LINE_MAX],
            terminal_input_len: 0,
            terminal_lines: [[0; SHELL_LINE_MAX]; SHELL_HISTORY_LINES],
            terminal_line_lens: [0; SHELL_HISTORY_LINES],
            terminal_line_count: 0,
            terminal_elevated: false,
            terminal_history_index: 0,
            terminal_draft: [0; SHELL_LINE_MAX],
            terminal_draft_len: 0,
            seamless: Seamless::new(),
            clip_open: false,
            touch_mode: false,
            osk_open: false,
            osk_shift: false,
            osk_symbols: false,
            osk_dismissed: false,
            store_search_focus: false,
            osk_pending: 0,
            shield_seen: 0,
            shield_text: [0; 56],
            shield_len: 0,
            shield_until_ns: 0,
            clip_selected: 0,
            clip_synced: u32::MAX,
            dock_entrance_at_ns: 0,
            window_open_at_ns: 0,
            previous_screen: Screen::Language,
            overlay_closing: Overlay::None,
            overlay_close_at_ns: 0,
            app_closing: DesktopApp::None,
            app_close_at_ns: 0,
            files_path: shell::Text::new(),
            files_entries: [DirEntryRow::EMPTY; FILES_MAX_ENTRIES],
            files_entry_count: 0,
            files_overflow: false,
            files_selected: usize::MAX,
            files_mode: FilesMode::Browsing,
            files_status: shell::Text::new(),
            notes_entries: [DirEntryRow::EMPTY; NOTES_MAX_ENTRIES],
            notes_entry_count: 0,
            notes_overflow: false,
            notes_mode: NotesMode::List,
            notes_current_name: shell::Text::new(),
            notes_content: shell::Text::new(),
            notes_status: shell::Text::new(),
            trash_entries: [DirEntryRow::EMPTY; TRASH_MAX_ENTRIES],
            trash_entry_count: 0,
            trash_overflow: false,
            trash_selected: usize::MAX,
            trash_status: shell::Text::new(),
            name_input: shell::Text::new(),
        }
    }

    fn toggle_maximize(&mut self) {
        if !can_maximize(self.app) {
            return;
        }
        let next = if is_maximized(self.app) {
            0
        } else {
            self.app as u8 + 1
        };
        MAXIMIZED_APP.store(next, Ordering::Relaxed);
        let now = crate::time::monotonic_nanoseconds();
        self.window_open_at_ns = now;
        self.motion_until_ns = self.motion_until_ns.max(now.saturating_add(WINDOW_OPEN_NS));
    }

    fn set_app(&mut self, target: DesktopApp) {
        if target != self.app {
            MAXIMIZED_APP.store(0, Ordering::Relaxed);
        }
        if target != DesktopApp::None && self.app != target {
            let now = crate::time::monotonic_nanoseconds();
            self.window_open_at_ns = now;
            self.motion_until_ns = self.motion_until_ns.max(now.saturating_add(WINDOW_OPEN_NS));
        }
        if target == DesktopApp::None && self.app != DesktopApp::None {
            let now = crate::time::monotonic_nanoseconds();
            self.app_closing = self.app;
            self.app_close_at_ns = now;
            self.motion_until_ns = self.motion_until_ns.max(now.saturating_add(WINDOW_OPEN_NS));
        }
        self.app = target;
    }

    fn set_overlay(&mut self, target: Overlay) {
        let now = crate::time::monotonic_nanoseconds();
        if target != Overlay::None && self.overlay != target {
            self.hover_prev = usize::MAX;
            self.hover_index = usize::MAX;
            self.overlay_open_at_ns = now;
            self.motion_until_ns = self
                .motion_until_ns
                .max(now.saturating_add(OVERLAY_TRANSITION_NS));
        }
        if target == Overlay::None && self.overlay != Overlay::None {
            self.overlay_closing = self.overlay;
            self.overlay_close_at_ns = now;
            self.motion_until_ns = self
                .motion_until_ns
                .max(now.saturating_add(OVERLAY_TRANSITION_NS));
        }
        self.overlay = target;
    }

    /// Whether something on screen is waiting for typed text.
    fn text_input_active(&self) -> bool {
        self.search_open
            || (self.app == DesktopApp::Browser && self.url_active)
            || (self.app == DesktopApp::Store
                && self.overlay == Overlay::None
                && crate::store::get().detail.is_none())
            || self.app == DesktopApp::Terminal
            || (self.app == DesktopApp::Files && self.files_mode == FilesMode::NamingFolder)
            || (self.app == DesktopApp::Notes && self.notes_mode != NotesMode::List)
            || matches!(
                self.screen,
                Screen::Keyboard
                    | Screen::Username
                    | Screen::Password
                    | Screen::Confirm
                    | Screen::Login
            )
    }

    /// Whether the touch keyboard should be up: text is wanted, and in the
    /// store only once its search field was tapped.
    fn osk_wanted(&self) -> bool {
        self.text_input_active() && (self.app != DesktopApp::Store || self.store_search_focus)
    }

    fn osk_hit(&self, point: Point) -> Option<OskKey> {
        let (keys, count) = osk_keys(self.osk_symbols);
        keys[..count]
            .iter()
            .find(|(rect, _)| rect.contains(point))
            .map(|(_, key)| *key)
    }

    /// Queue a key as if it had been typed on a keyboard, so every text field
    /// (and the Linux guest) handles it the way it handles real keys.
    fn osk_tap(&mut self, code: u8, shift: bool) {
        let mut queued = 0;
        if shift {
            keyboard::push_scancode(0x2a);
            queued += 1;
        }
        keyboard::push_scancode(code);
        keyboard::push_scancode(code | 0x80);
        queued += 2;
        if shift {
            keyboard::push_scancode(0xaa);
            queued += 1;
        }
        self.osk_pending += queued;
    }

    fn osk_type_char(&mut self, wanted: u8) {
        for code in 0x02..=0x35u8 {
            for shift in [false, true] {
                if shell::scancode_character(code, shift, false) == Some(wanted) {
                    self.osk_tap(code, shift);
                    return;
                }
            }
        }
    }

    fn osk_press(&mut self, key: OskKey) {
        match key {
            OskKey::Char(byte) => {
                let byte = if self.osk_shift && byte.is_ascii_lowercase() {
                    byte.to_ascii_uppercase()
                } else {
                    byte
                };
                self.osk_type_char(byte);
                self.osk_shift = false;
            }
            OskKey::Shift => self.osk_shift = !self.osk_shift,
            OskKey::Backspace => self.osk_tap(0x0e, false),
            OskKey::Symbols => self.osk_symbols = !self.osk_symbols,
            OskKey::Space => self.osk_tap(0x39, false),
            OskKey::Enter => self.osk_tap(0x1c, false),
            OskKey::Hide => self.osk_dismissed = true,
        }
    }

    fn set_screen(&mut self, target: Screen) {
        if self.screen != target {
            let now = crate::time::monotonic_nanoseconds();
            self.screen_transition_at_ns = now;
            self.motion_until_ns = self
                .motion_until_ns
                .max(now.saturating_add(SCREEN_TRANSITION_NS));
            if self.screen == Screen::Lock && target == Screen::Login {
                self.motion_until_ns = self
                    .motion_until_ns
                    .max(now.saturating_add(LOCK_LOGIN_SCROLL_NS));
            }
            if target == Screen::Desktop {
                self.dock_entrance_at_ns = now;
                let last_start = (DOCK_BUTTONS.len() as u64 - 1) * DOCK_FALL_STAGGER_NS;
                self.motion_until_ns = self
                    .motion_until_ns
                    .max(now.saturating_add(last_start + DOCK_FALL_NS));
            }
            self.previous_screen = self.screen;
        }
        self.screen = target;
    }

    fn handle(&mut self, key: DesktopKey) -> DesktopAction {
        match key {
            DesktopKey::VolumeUp => {
                self.volume_step(6);
                return DesktopAction::Redraw;
            }
            DesktopKey::VolumeDown => {
                self.volume_step(-6);
                return DesktopAction::Redraw;
            }
            DesktopKey::Mute => {
                self.toggle_mute();
                return DesktopAction::Redraw;
            }
            DesktopKey::PlayPause => {
                self.music_toggle();
                return DesktopAction::Redraw;
            }
            DesktopKey::NextTrack => {
                self.music_skip(1);
                return DesktopAction::Redraw;
            }
            DesktopKey::PrevTrack => {
                self.music_skip(-1);
                return DesktopAction::Redraw;
            }
            DesktopKey::BrightnessUp => {
                self.brightness_step(8);
                return DesktopAction::Redraw;
            }
            DesktopKey::BrightnessDown => {
                self.brightness_step(-8);
                return DesktopAction::Redraw;
            }
            _ => {}
        }
        if let Some(action) = self.power_key(key) {
            return action;
        }
        if self.screen == Screen::Desktop {
            if let Some(action) = self.search_key(key) {
                return action;
            }
            if let Some(action) = self.notify_key(key) {
                return action;
            }
            if let Some(action) = self.context_key(key) {
                return action;
            }
            if let Some(action) = self.quick_key(key) {
                return action;
            }
        }
        if self.screen == Screen::Login {
            return match key {
                // Escape only clears what was typed; it never unlocks.
                DesktopKey::Escape => {
                    self.wipe_login();
                    DesktopAction::Redraw
                }
                DesktopKey::Activate => self.try_login(),
                DesktopKey::Backspace => {
                    self.login_len = self.login_len.saturating_sub(1);
                    DesktopAction::Redraw
                }
                DesktopKey::Character(byte) => {
                    if self.push_login_byte(byte) {
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    }
                }
                _ => DesktopAction::Idle,
            };
        }
        if self.app == DesktopApp::Files && self.files_mode == FilesMode::NamingFolder {
            return match key {
                DesktopKey::Escape => {
                    self.files_mode = FilesMode::Browsing;
                    self.name_input.clear();
                    DesktopAction::Redraw
                }
                DesktopKey::Activate => {
                    let mut name: shell::Text<NAME_INPUT_MAX> = shell::Text::new();
                    let _ = name.push_str_checked(self.name_input.as_str());
                    self.name_input.clear();
                    self.files_mode = FilesMode::Browsing;
                    self.files_new_folder(name.as_str());
                    DesktopAction::Redraw
                }
                DesktopKey::Backspace => {
                    self.name_input.backspace();
                    DesktopAction::Redraw
                }
                DesktopKey::Character(byte) => {
                    if self.push_name_byte(byte) {
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    }
                }
                _ => DesktopAction::Idle,
            };
        }
        if self.app == DesktopApp::Notes && self.notes_mode == NotesMode::Naming {
            return match key {
                DesktopKey::Escape => {
                    self.notes_mode = NotesMode::List;
                    self.name_input.clear();
                    DesktopAction::Redraw
                }
                DesktopKey::Activate => {
                    let mut name: shell::Text<NAME_INPUT_MAX> = shell::Text::new();
                    let _ = name.push_str_checked(self.name_input.as_str());
                    self.name_input.clear();
                    self.notes_start_new(name.as_str());
                    DesktopAction::Redraw
                }
                DesktopKey::Backspace => {
                    self.name_input.backspace();
                    DesktopAction::Redraw
                }
                DesktopKey::Character(byte) => {
                    if self.push_name_byte(byte) {
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    }
                }
                _ => DesktopAction::Idle,
            };
        }
        if self.app == DesktopApp::Notes && self.notes_mode == NotesMode::Editing {
            return match key {
                DesktopKey::Escape => {
                    self.notes_mode = NotesMode::List;
                    self.notes_content.clear();
                    DesktopAction::Redraw
                }
                DesktopKey::Activate => {
                    if self.notes_content.push_byte(b'\n') {
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    }
                }
                DesktopKey::Backspace => {
                    self.notes_content.backspace();
                    DesktopAction::Redraw
                }
                DesktopKey::Character(byte) => {
                    if self.push_notes_byte(byte) {
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    }
                }
                _ => DesktopAction::Idle,
            };
        }
        if self.screen != Screen::Desktop {
            // Setup cannot be skipped and hotkeys do nothing until the
            // account exists and the user has signed in.
            if self.screen == Screen::Lock {
                // Any key wakes the lock screen to the sign-in prompt.
                self.set_screen(Screen::Login);
                return DesktopAction::Redraw;
            }
            if self.screen == Screen::Keyboard {
                match key {
                    DesktopKey::Up => return self.keyboard_move(-1),
                    DesktopKey::Down => return self.keyboard_move(1),
                    DesktopKey::Character(byte) => return self.keyboard_type(byte),
                    DesktopKey::Backspace => {
                        self.kb_search_len = self.kb_search_len.saturating_sub(1);
                        self.keyboard_refilter();
                        return DesktopAction::Redraw;
                    }
                    DesktopKey::Escape => {
                        self.kb_search_len = 0;
                        self.keyboard_refilter();
                        return DesktopAction::Redraw;
                    }
                    DesktopKey::Tab | DesktopKey::Right => return DesktopAction::Idle,
                    _ => {}
                }
            }
            return match key {
                DesktopKey::Escape => {
                    self.wipe_setup_field();
                    DesktopAction::Redraw
                }
                DesktopKey::Character(byte) => {
                    let pushed = match self.screen {
                        Screen::Username => self.push_username_byte(byte),
                        Screen::Password => self.push_setup_password_byte(byte),
                        Screen::Confirm => self.push_confirm_byte(byte),
                        _ => false,
                    };
                    if pushed {
                        self.setup_error = "";
                        self.setup_typed_ns = crate::time::monotonic_nanoseconds();
                        self.motion_until_ns =
                            self.motion_until_ns.max(self.setup_typed_ns + 300_000_000);
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    }
                }
                DesktopKey::Backspace => {
                    match self.screen {
                        Screen::Username => self.username_len = self.username_len.saturating_sub(1),
                        Screen::Password => {
                            self.setup_password_len = self.setup_password_len.saturating_sub(1);
                            self.setup_password_input[self.setup_password_len] = 0;
                        }
                        Screen::Confirm => {
                            self.confirm_len = self.confirm_len.saturating_sub(1);
                            self.confirm_input[self.confirm_len] = 0;
                        }
                        _ => {}
                    }
                    self.setup_error = "";
                    DesktopAction::Redraw
                }
                DesktopKey::Activate | DesktopKey::Tab | DesktopKey::Right | DesktopKey::Down => {
                    self.advance_setup()
                }
                _ => DesktopAction::Idle,
            };
        }
        if self.app == DesktopApp::Browser && self.overlay == Overlay::None && !self.url_active {
            let now = crate::time::monotonic_nanoseconds();
            let web = crate::web::get();
            match key {
                DesktopKey::Up => {
                    web.scroll_by(-3, BROWSER_VISIBLE_LINES);
                    return DesktopAction::Redraw;
                }
                DesktopKey::Down => {
                    web.scroll_by(3, BROWSER_VISIBLE_LINES);
                    return DesktopAction::Redraw;
                }
                DesktopKey::Left => {
                    web.back(now);
                    return DesktopAction::Redraw;
                }
                DesktopKey::Right => {
                    web.forward(now);
                    return DesktopAction::Redraw;
                }
                _ => {}
            }
        }
        if self.app == DesktopApp::Store && self.overlay == Overlay::None {
            let store = crate::store::get();
            match key {
                DesktopKey::Escape if store.detail.is_some() => {
                    store.detail = None;
                    return DesktopAction::Redraw;
                }
                DesktopKey::Escape if !store.query().is_empty() => {
                    while store.pop_query() {}
                    return DesktopAction::Redraw;
                }
                DesktopKey::Backspace => {
                    return if store.detail.is_none() && store.pop_query() {
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    };
                }
                DesktopKey::Character(byte) => {
                    return if store.detail.is_none() && store.push_query(byte) {
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    };
                }
                DesktopKey::Up => {
                    store.scroll_rows(-1);
                    return DesktopAction::Redraw;
                }
                DesktopKey::Down => {
                    store.scroll_rows(1);
                    return DesktopAction::Redraw;
                }
                DesktopKey::Tab => {
                    let next = if store.tab == crate::store::StoreTab::Installed {
                        crate::store::StoreTab::Discover
                    } else {
                        crate::store::StoreTab::Installed
                    };
                    store.switch_tab(next);
                    return DesktopAction::Redraw;
                }
                DesktopKey::Activate => {
                    let target = store.detail.or_else(|| store.shown_index(0));
                    if let Some(index) = target {
                        if store.detail.is_some() {
                            self.store_primary(index);
                        } else {
                            self.store_open(index);
                        }
                    }
                    return DesktopAction::Redraw;
                }
                _ => {}
            }
        }
        match key {
            DesktopKey::Terminal => {
                self.set_overlay(Overlay::None);
                self.set_app(DesktopApp::Terminal);
            }
            DesktopKey::Browser => {
                self.set_overlay(Overlay::None);
                self.open_browser();
            }
            DesktopKey::Settings => {
                self.set_overlay(Overlay::None);
                self.set_app(DesktopApp::Settings);
            }
            DesktopKey::Linux => {
                self.set_overlay(Overlay::None);
                self.seamless.enabled = false;
                self.set_app(DesktopApp::Linux);
            }
            DesktopKey::Store => {
                self.set_overlay(Overlay::None);
                self.open_store();
            }
            DesktopKey::Seamless => {
                self.set_overlay(Overlay::None);
                self.set_app(DesktopApp::None);
                self.seamless.enabled = true;
            }
            DesktopKey::Power => self.open_power_menu(),
            DesktopKey::Notifications => self.toggle_notify_center(),
            DesktopKey::Search => self.open_search(),
            DesktopKey::BrightnessUp | DesktopKey::BrightnessDown => return DesktopAction::Idle,
            DesktopKey::Apps => {
                self.set_overlay(if self.overlay == Overlay::Apps {
                    Overlay::None
                } else {
                    Overlay::Apps
                });
                self.focus_visible = true;
            }
            DesktopKey::Quick => {
                self.set_overlay(if self.overlay == Overlay::Quick {
                    Overlay::None
                } else {
                    Overlay::Quick
                });
                self.focus_visible = true;
            }
            DesktopKey::Backspace => {
                if self.app == DesktopApp::Browser && self.url_active {
                    self.url_len = self.url_len.saturating_sub(1);
                    return DesktopAction::Redraw;
                }
                return DesktopAction::Idle;
            }
            DesktopKey::VolumeUp
            | DesktopKey::VolumeDown
            | DesktopKey::Mute
            | DesktopKey::PlayPause
            | DesktopKey::NextTrack
            | DesktopKey::PrevTrack => return DesktopAction::Idle,
            DesktopKey::Character(byte) => {
                if self.app == DesktopApp::Browser && self.url_active && self.push_url_byte(byte) {
                    return DesktopAction::Redraw;
                }
                return DesktopAction::Idle;
            }
            DesktopKey::Escape => {
                if self.app == DesktopApp::Browser && self.url_active {
                    self.url_active = false;
                    if self.browser_phase == BrowserPhase::Editing {
                        self.set_app(DesktopApp::None);
                    }
                    return DesktopAction::Redraw;
                }
                self.set_overlay(Overlay::None);
                self.set_app(DesktopApp::None);
            }
            DesktopKey::Tab | DesktopKey::Right => {
                if self.app == DesktopApp::Browser && self.url_active {
                    if key == DesktopKey::Tab {
                        self.url_active = false;
                    }
                    return DesktopAction::Redraw;
                }
                self.focus_visible = true;
                if self.overlay == Overlay::Apps {
                    self.app_focus = (self.app_focus + 1) % APP_LABELS.len();
                } else {
                    self.dock_focus = (self.dock_focus + 1) % DOCK_BUTTONS.len();
                }
            }
            DesktopKey::Left => {
                self.focus_visible = true;
                if self.overlay == Overlay::Apps {
                    self.app_focus = (self.app_focus + APP_LABELS.len() - 1) % APP_LABELS.len();
                } else {
                    self.dock_focus =
                        (self.dock_focus + DOCK_BUTTONS.len() - 1) % DOCK_BUTTONS.len();
                }
            }
            DesktopKey::Down => {
                self.focus_visible = true;
                if self.overlay == Overlay::Apps {
                    self.app_focus = (self.app_focus + 5).min(APP_LABELS.len() - 1);
                }
            }
            DesktopKey::Up => {
                self.focus_visible = true;
                if self.overlay == Overlay::Apps {
                    self.app_focus = self.app_focus.saturating_sub(5);
                }
            }
            DesktopKey::Activate => {
                if self.app == DesktopApp::Settings && self.overlay == Overlay::None {
                    self.wifi = !self.wifi;
                    return DesktopAction::Redraw;
                }
                if self.app == DesktopApp::Browser && self.overlay == Overlay::None {
                    if self.url_active {
                        self.submit_url();
                    } else {
                        self.url_active = true;
                        self.browser_phase = BrowserPhase::Editing;
                    }
                    return DesktopAction::Redraw;
                }
                if self.overlay == Overlay::Apps {
                    self.set_overlay(Overlay::None);
                    match self.app_focus {
                        0 => self.set_app(DesktopApp::Terminal),
                        1 => self.open_files(),
                        2 => self.open_browser(),
                        3 => self.open_notes(),
                        4 => self.set_app(DesktopApp::Settings),
                        5 => self.set_app(DesktopApp::Linux),
                        19 => self.open_store(),
                        _ => self.open_files(),
                    }
                } else if self.overlay == Overlay::Quick {
                    self.wifi = !self.wifi;
                } else {
                    let now = crate::time::monotonic_nanoseconds();
                    self.dock_press_ns[self.dock_focus] = now;
                    self.dock_release_ns[self.dock_focus] = now.saturating_add(DOCK_HOLD_NS);
                    self.motion_until_ns = self.motion_until_ns.max(
                        now.saturating_add(DOCK_HOLD_NS)
                            .saturating_add(button::RELEASE_NS),
                    );
                    match self.dock_focus {
                        0 => self.set_overlay(Overlay::Apps),
                        1 => self.set_app(DesktopApp::Terminal),
                        2 => self.set_app(DesktopApp::Settings),
                        3 => self.open_files(),
                        4 => self.open_browser(),
                        5 => self.open_notes(),
                        _ => self.open_trash(),
                    }
                }
            }
        }
        DesktopAction::Redraw
    }

    fn click(&mut self, point: Point) -> DesktopAction {
        if self.osk_open {
            if let Some(key) = self.osk_hit(point) {
                self.osk_press(key);
                return DesktopAction::Redraw;
            }
            if osk_rect().contains(point) {
                return DesktopAction::Idle;
            }
        }
        if let Some(action) = self.power_click(point) {
            return action;
        }
        if self.screen == Screen::Desktop {
            if let Some(action) = self.search_click(point) {
                return action;
            }
            if let Some(action) = self.notify_click(point) {
                return action;
            }
            if let Some(action) = self.context_click(point) {
                return action;
            }
        }
        if let Some(action) = self.music_hub_click(point) {
            return action;
        }
        self.music_click_away();
        if self.screen == Screen::Login {
            return DesktopAction::Idle;
        }
        if self.screen != Screen::Desktop {
            if self.screen == Screen::Lock {
                self.set_screen(Screen::Login);
                return DesktopAction::Redraw;
            }
            return self.setup_click(point);
        }
        if self.app != DesktopApp::None && self.overlay == Overlay::None {
            let base = window_base_rect(self.app);
            let (minimize, maximize, close) = window_control_rects(base);
            if close.contains(point) || minimize.contains(point) {
                // Minimize hides the window; its state is kept for next time.
                self.set_app(DesktopApp::None);
                return DesktopAction::Redraw;
            }
            if maximize.contains(point) {
                self.toggle_maximize();
                return DesktopAction::Redraw;
            }
        }
        if self.app == DesktopApp::Browser
            && self.overlay == Overlay::None
            && let Some(action) = self.browser_click(point)
        {
            return action;
        }
        if self.app == DesktopApp::Store
            && self.overlay == Overlay::None
            && let Some(action) = self.store_click(point)
        {
            return action;
        }
        if self.app == DesktopApp::Files
            && self.overlay == Overlay::None
            && self.files_mode == FilesMode::Browsing
        {
            let base = window_base_rect(DesktopApp::Files);
            let content_x = base.x + 10;
            let content_y = base.y + 36;
            let sidebar_width = 106;
            for (index, path) in ["/", "/data", "/tmp"].iter().enumerate() {
                let bounds = Rect::new(
                    content_x + 8,
                    content_y + 28 + index as i32 * 32,
                    sidebar_width - 16,
                    26,
                );
                if bounds.contains(point) {
                    let mut name: shell::Text<256> = shell::Text::new();
                    let _ = name.push_str_checked(path);
                    self.files_path = name;
                    self.files_selected = usize::MAX;
                    self.files_status.clear();
                    self.files_reload();
                    return DesktopAction::Redraw;
                }
            }
            if Rect::new(content_x + 8, content_y + 148, sidebar_width - 16, 26).contains(point) {
                self.files_mode = FilesMode::NamingFolder;
                self.name_input.clear();
                return DesktopAction::Redraw;
            }
            if Rect::new(content_x + 8, content_y + 182, sidebar_width - 16, 26).contains(point) {
                self.files_delete_selected();
                return DesktopAction::Redraw;
            }
            let main_x = content_x + sidebar_width + 12;
            let main_width = base.x + base.width - 10 - main_x;
            if Rect::new(main_x, content_y, 44, 24).contains(point) {
                self.files_up();
                return DesktopAction::Redraw;
            }
            let content_bottom = base.y + base.height - 10;
            let status_y = content_bottom - 14;
            let grid_top = content_y + 32;
            let cell_width = main_width / FILES_GRID_COLUMNS;
            let cell_height = ((status_y - 4) - grid_top) / FILES_GRID_ROWS;
            for index in 0..self.files_entry_count {
                let column = index as i32 % FILES_GRID_COLUMNS;
                let line = index as i32 / FILES_GRID_COLUMNS;
                let bounds = Rect::new(
                    main_x + column * cell_width,
                    grid_top + line * cell_height,
                    cell_width - 8,
                    cell_height - 6,
                );
                if bounds.contains(point) {
                    let row = self.files_entries[index];
                    if row.is_dir {
                        let mut name: shell::Text<48> = shell::Text::new();
                        let _ = name.push_str_checked(row.name_str());
                        self.files_navigate(name.as_str());
                    } else {
                        self.files_selected = index;
                        self.files_status.clear();
                    }
                    return DesktopAction::Redraw;
                }
            }
        }
        if self.app == DesktopApp::Notes && self.overlay == Overlay::None {
            let base = window_base_rect(DesktopApp::Notes);
            let content_x = base.x + 14;
            let content_y = base.y + 36;
            let content_right = base.x + base.width - 14;
            if self.notes_mode == NotesMode::Editing {
                if Rect::new(content_right - 56, content_y - 4, 56, 26).contains(point) {
                    self.notes_save();
                    return DesktopAction::Redraw;
                }
            } else if self.notes_mode == NotesMode::List {
                if Rect::new(content_x, content_y, content_right - content_x, 28).contains(point) {
                    self.notes_mode = NotesMode::Naming;
                    self.name_input.clear();
                    return DesktopAction::Redraw;
                }
                let content_bottom = base.y + base.height - 14;
                let list_top = content_y + 38;
                let status_y = content_bottom - 4;
                let row_height = ((status_y - 16) - list_top) / NOTES_MAX_ENTRIES as i32;
                for index in 0..self.notes_entry_count {
                    let bounds = Rect::new(
                        content_x,
                        list_top + index as i32 * row_height,
                        content_right - content_x,
                        row_height - 6,
                    );
                    let close = Rect::new(bounds.x + bounds.width - 30, bounds.y + 10, 20, 20);
                    if close.contains(point) {
                        let mut name: shell::Text<48> = shell::Text::new();
                        let _ = name.push_str_checked(self.notes_entries[index].name_str());
                        self.notes_delete(name.as_str());
                        return DesktopAction::Redraw;
                    }
                    if bounds.contains(point) {
                        let mut name: shell::Text<48> = shell::Text::new();
                        let _ = name.push_str_checked(self.notes_entries[index].name_str());
                        self.notes_open(name.as_str());
                        return DesktopAction::Redraw;
                    }
                }
            }
        }
        if self.app == DesktopApp::Trash && self.overlay == Overlay::None {
            let base = window_base_rect(DesktopApp::Trash);
            let content_x = base.x + 14;
            let content_y = base.y + 36;
            let content_right = base.x + base.width - 14;
            if Rect::new(content_right - 90, content_y - 4, 90, 24).contains(point) {
                self.trash_empty();
                return DesktopAction::Redraw;
            }
            let content_bottom = base.y + base.height - 14;
            let list_top = content_y + 30;
            let status_y = content_bottom - 4;
            let row_height = ((status_y - 16) - list_top) / TRASH_MAX_ENTRIES as i32;
            for index in 0..self.trash_entry_count {
                let bounds = Rect::new(
                    content_x,
                    list_top + index as i32 * row_height,
                    content_right - content_x,
                    row_height - 4,
                );
                let restore = Rect::new(bounds.x + bounds.width - 52, bounds.y, 24, 22);
                let delete = Rect::new(bounds.x + bounds.width - 24, bounds.y, 24, 22);
                if restore.contains(point) {
                    self.trash_selected = index;
                    self.trash_restore_selected();
                    return DesktopAction::Redraw;
                }
                if delete.contains(point) {
                    self.trash_selected = index;
                    self.trash_delete_forever_selected();
                    return DesktopAction::Redraw;
                }
            }
        }
        if Rect::new(503, 377, 216, 55).contains(point) && self.overlay == Overlay::None {
            self.toggle_notify_center();
            return DesktopAction::Redraw;
        }
        if self.overlay != Overlay::None {
            // The dock stays clickable while a panel is open.
            for index in 0..DOCK_BUTTONS.len() {
                let bounds = Rect::new(44 + index as i32 * 64, 380, 50, 50);
                if !bounds.contains(point) {
                    continue;
                }
                let toggles_open_panel = (index == 0 && self.overlay == Overlay::Apps)
                    || (index == 2 && self.overlay == Overlay::Quick);
                if toggles_open_panel {
                    self.set_overlay(Overlay::None);
                    return DesktopAction::Redraw;
                }
                self.dock_focus = index;
                self.focus_visible = true;
                self.set_overlay(Overlay::None);
                return self.handle(DesktopKey::Activate);
            }
        }
        if self.overlay == Overlay::Apps {
            for index in 0..APP_LABELS.len().min(6) {
                let bounds = Rect::new(58 + index as i32 * 64, 48, 50, 50);
                if bounds.contains(point) {
                    self.app_focus = index;
                    self.focus_visible = true;
                    return self.handle(DesktopKey::Activate);
                }
            }
            if Rect::new(25, 20, 702, 420).contains(point) {
                return DesktopAction::Idle;
            }
            // Clicking away from the panel closes it.
            self.set_overlay(Overlay::None);
            return DesktopAction::Redraw;
        }
        if self.overlay == Overlay::Quick {
            if panels::QUICK_PANEL.contains(point) {
                return self.quick_click(point);
            }
            self.set_overlay(Overlay::None);
            return DesktopAction::Redraw;
        }
        for index in 0..DOCK_BUTTONS.len() {
            let bounds = Rect::new(44 + index as i32 * 64, 380, 50, 50);
            if bounds.contains(point) {
                self.dock_focus = index;
                self.focus_visible = true;
                self.set_overlay(Overlay::None);
                return self.handle(DesktopKey::Activate);
            }
        }
        DesktopAction::Idle
    }

    /// The address the user sees (typed text or the loaded page's).
    fn browser_click(&mut self, point: Point) -> Option<DesktopAction> {
        let base = window_base_rect(DesktopApp::Browser);
        let now = crate::time::monotonic_nanoseconds();
        let web = crate::web::get();
        let bar_y = base.y + 34;
        let back = Rect::new(base.x + 14, bar_y, 28, 26);
        let forward = Rect::new(base.x + 46, bar_y, 28, 26);
        let reload = Rect::new(base.right() - 44, bar_y, 28, 26);
        let pill = Rect::new(base.x + 84, bar_y, base.width - 84 - 58, 26);
        if back.contains(point) {
            web.back(now);
            return Some(DesktopAction::Redraw);
        }
        if forward.contains(point) {
            web.forward(now);
            return Some(DesktopAction::Redraw);
        }
        if reload.contains(point) {
            web.reload(now);
            return Some(DesktopAction::Redraw);
        }
        if pill.contains(point) {
            self.url_active = true;
            if web.state == crate::web::WebState::Blank {
                self.browser_phase = BrowserPhase::Editing;
            }
            // Editing starts from the address of the page on screen.
            let current = web.url();
            if !current.is_empty() && self.url_len == 0 {
                let bytes = current.as_bytes();
                let len = bytes.len().min(MAX_URL);
                self.url_input[..len].copy_from_slice(&bytes[..len]);
                self.url_len = len;
            }
            return Some(DesktopAction::Redraw);
        }
        let content = browser_content_rect(base);
        if !content.contains(point) {
            return None;
        }
        if web.state == crate::web::WebState::Blank {
            for (index, (_, address)) in BROWSER_FAVORITES.iter().enumerate() {
                if browser_favorite_rect(content, index).contains(point) {
                    let bytes = address.as_bytes();
                    self.url_input[..bytes.len()].copy_from_slice(bytes);
                    self.url_len = bytes.len();
                    self.url_active = false;
                    web.navigate(address, now);
                    self.browser_phase = BrowserPhase::Loading;
                    return Some(DesktopAction::Redraw);
                }
            }
            return None;
        }
        // A click on a `[n]` link marker in the page text.
        let char_width = BROWSER_CHAR_W_MILLI
            .load(core::sync::atomic::Ordering::Relaxed)
            .max(1) as i64;
        let row = ((point.y - content.y - 8) / BROWSER_LINE_HEIGHT).max(0) as usize;
        let line_index = web.scroll + row;
        let column = (((point.x - content.x - 12) as i64) * 1000 / char_width).max(0) as usize;
        let line = web.line(line_index).as_bytes();
        let mut at = 0;
        while at < line.len() {
            if line[at] == b'[' {
                let mut end = at + 1;
                while end < line.len() && line[end].is_ascii_digit() {
                    end += 1;
                }
                if end > at + 1 && end < line.len() && line[end] == b']' {
                    if (at..=end).contains(&column) {
                        let number = core::str::from_utf8(&line[at + 1..end])
                            .ok()
                            .and_then(|digits| digits.parse::<u32>().ok())?;
                        let mut target = [0u8; crate::web::URL_MAX];
                        let text = web.reference(number)?;
                        let len = text.len().min(target.len());
                        target[..len].copy_from_slice(&text.as_bytes()[..len]);
                        let target = core::str::from_utf8(&target[..len]).ok()?;
                        self.url_len = 0;
                        web.navigate(target, now);
                        self.browser_phase = BrowserPhase::Loading;
                        return Some(DesktopAction::Redraw);
                    }
                    at = end;
                }
            }
            at += 1;
        }
        None
    }

    /// The main button of an app: Open if it is installed, else Get.
    fn store_primary(&mut self, index: usize) {
        let store = crate::store::get();
        let now = crate::time::monotonic_nanoseconds();
        if !crate::svm::linux_agent_ready() {
            // Linux isn't running: start it (its windows and app list follow).
            self.show_linux_apps();
            return;
        }
        match store.tab {
            crate::store::StoreTab::Installed => {
                if store.launch(index) {
                    self.show_linux_apps();
                }
            }
            crate::store::StoreTab::Discover => {
                if let Some(installed) = store.installed_index(&crate::store::CATALOG[index]) {
                    if store.launch(installed) {
                        self.show_linux_apps();
                    }
                } else {
                    store.install(index, now);
                }
            }
        }
    }

    /// A grid item was chosen: an installed app opens, a catalogue app
    /// shows its page.
    fn store_open(&mut self, index: usize) {
        let store = crate::store::get();
        match store.tab {
            crate::store::StoreTab::Installed => self.store_primary(index),
            crate::store::StoreTab::Discover => store.detail = Some(index),
        }
    }

    fn store_click(&mut self, point: Point) -> Option<DesktopAction> {
        use crate::store::{GRID_COLUMNS, GRID_ROWS, StoreTab};
        let base = window_base_rect(DesktopApp::Store);
        let store = crate::store::get();
        if let Some(index) = store.detail {
            if store_back_rect(base).contains(point) {
                store.detail = None;
                return Some(DesktopAction::Redraw);
            }
            if store_get_rect(base).contains(point) {
                self.store_primary(index);
                return Some(DesktopAction::Redraw);
            }
            for (slot, other) in store.similar(index).into_iter().enumerate() {
                if other < crate::store::CATALOG.len()
                    && store_similar_rect(base, slot).contains(point)
                {
                    store.detail = Some(other);
                    return Some(DesktopAction::Redraw);
                }
            }
            return None;
        }
        if store_search_rect(base).contains(point) {
            self.store_search_focus = true;
            self.osk_dismissed = false;
            return Some(DesktopAction::Redraw);
        }
        self.store_search_focus = false;
        if store_toggle_rect(base).contains(point) {
            let next = if store.tab == StoreTab::Installed {
                StoreTab::Discover
            } else {
                StoreTab::Installed
            };
            store.switch_tab(next);
            return Some(DesktopAction::Redraw);
        }
        if store.tab == StoreTab::Installed
            && !crate::svm::linux_agent_ready()
            && store_start_button_rect(base).contains(point)
        {
            self.show_linux_apps();
            return Some(DesktopAction::Redraw);
        }
        for slot in 0..GRID_COLUMNS * GRID_ROWS {
            let (column, row) = (slot % GRID_COLUMNS, slot / GRID_COLUMNS);
            let position = (store.scroll + row) * GRID_COLUMNS + column;
            let Some(index) = store.shown_index(position) else {
                break;
            };
            if store_cell_rect(base, column, row).contains(point) {
                self.store_open(index);
                return Some(DesktopAction::Redraw);
            }
        }
        None
    }

    fn open_browser(&mut self) {
        self.set_app(DesktopApp::Browser);
        self.set_overlay(Overlay::None);
        // Straight to the address bar; a page already open stays as it was,
        // otherwise the start page (favourites) shows.
        self.url_active = crate::web::get().state == crate::web::WebState::Blank;
        if self.url_active {
            self.browser_phase = BrowserPhase::Editing;
        }
    }

    fn open_store(&mut self) {
        self.set_app(DesktopApp::Store);
        self.set_overlay(Overlay::None);
        self.url_active = false;
    }

    /// Enters seamless mode so Linux windows show as AerOS windows.
    fn show_linux_apps(&mut self) {
        self.set_overlay(Overlay::None);
        self.set_app(DesktopApp::None);
        self.seamless.enabled = true;
    }

    fn start_load(&mut self) {
        // With the Linux guest available the page is fetched there (real TLS).
        if crate::svm::linux_ready()
            && self.url_len > 0
            && !FORCE_NATIVE_FETCH.load(core::sync::atomic::Ordering::Relaxed)
        {
            let now = crate::time::monotonic_nanoseconds();
            let mut scratch = [0u8; MAX_URL];
            scratch[..self.url_len].copy_from_slice(&self.url_input[..self.url_len]);
            let typed = core::str::from_utf8(&scratch[..self.url_len]).unwrap_or("");
            crate::web::get().navigate(typed, now);
            self.browser_phase = BrowserPhase::Loading;
            return;
        }
        self.browser_online = false;
        self.browser_http_status = 0;
        self.browser_http_bytes = 0;
        self.browser_title.fill(0);
        self.browser_title_len = 0;
        self.browser_heading.fill(0);
        self.browser_heading_len = 0;
        self.browser_body.clear();
        self.browser_address = [0; 4];
        let mut scratch = [0u8; MAX_URL];
        scratch[..self.url_len].copy_from_slice(&self.url_input[..self.url_len]);
        let input = core::str::from_utf8(&scratch[..self.url_len]).unwrap_or("");
        match parse_url(input) {
            UrlParse::Http { host, path } => {
                self.browser_host_len = host.len();
                self.browser_host[..host.len()].copy_from_slice(host.as_bytes());
                self.browser_path_len = path.len();
                self.browser_path[..path.len()].copy_from_slice(path.as_bytes());
                self.browser_phase = BrowserPhase::Loading;
                self.browser_pending = true;
            }
            UrlParse::Https => self.browser_phase = BrowserPhase::HttpsUnsupported,
            UrlParse::Invalid => self.browser_phase = BrowserPhase::BadUrl,
        }
    }

    fn url_str(&self) -> &str {
        core::str::from_utf8(&self.url_input[..self.url_len]).unwrap_or("")
    }

    fn push_url_byte(&mut self, byte: u8) -> bool {
        if self.url_len >= MAX_URL || !(byte.is_ascii_graphic() || byte == b' ') {
            return false;
        }
        self.url_input[self.url_len] = byte;
        self.url_len += 1;
        true
    }

    fn submit_url(&mut self) {
        self.url_active = false;
        self.start_load();
    }

    fn open_files(&mut self) {
        self.set_overlay(Overlay::None);
        self.set_app(DesktopApp::Files);
        if self.files_path.as_str().is_empty() {
            self.files_path.push_str_checked("/");
        }
        self.files_mode = FilesMode::Browsing;
        self.files_reload();
    }

    fn files_reload(&mut self) {
        let path: shell::Text<256> = self.files_path;
        let (count, overflow) = list_directory(path.as_str(), &mut self.files_entries);
        self.files_entry_count = count;
        self.files_overflow = overflow;
        if self.files_selected >= count {
            self.files_selected = usize::MAX;
        }
    }

    fn files_set_status(&mut self, message: &str) {
        self.files_status.clear();
        let _ = self.files_status.push_str_checked(message);
    }

    fn files_child_path(&self, name: &str) -> shell::Text<256> {
        let mut joined: shell::Text<256> = shell::Text::new();
        let _ = shell::normalize_path(self.files_path.as_str(), name, &mut joined);
        joined
    }

    fn files_navigate(&mut self, name: &str) {
        let joined = self.files_child_path(name);
        self.files_path = joined;
        self.files_selected = usize::MAX;
        self.files_status.clear();
        self.files_reload();
    }

    fn files_up(&mut self) {
        if self.files_path.as_str() != "/" {
            self.files_navigate("..");
        }
    }

    fn files_delete_selected(&mut self) {
        let Some(row) = self.files_entries.get(self.files_selected).copied() else {
            self.files_set_status("no item selected");
            return;
        };
        let target = self.files_child_path(row.name_str());
        if row.is_dir {
            match vfs::remove(target.as_str(), true) {
                Ok(()) => self.files_set_status("folder removed"),
                Err(failure) => self.files_set_status(shell::vfs_error(failure)),
            }
        } else {
            let mut buffer = [0u8; NOTE_MAX_BYTES];
            match shell::read_file(target.as_str(), &mut buffer) {
                Ok(length) => {
                    let _ = vfs::create_directory(trash_directory(), 0o777);
                    let mut trash_path: shell::Text<256> = shell::Text::new();
                    let _ =
                        shell::normalize_path(trash_directory(), row.name_str(), &mut trash_path);
                    match vfs::open_file(trash_path.as_str(), true, false, true, 0o644, true) {
                        Ok(descriptor) => {
                            let write_result = vfs::write(descriptor, &buffer[..length], false);
                            let _ = vfs::close(descriptor);
                            match write_result {
                                Ok(_) => match vfs::remove(target.as_str(), false) {
                                    Ok(()) => self.files_set_status("moved to trash"),
                                    Err(failure) => {
                                        self.files_set_status(shell::vfs_error(failure))
                                    }
                                },
                                Err(failure) => self.files_set_status(shell::vfs_error(failure)),
                            }
                        }
                        Err(failure) => self.files_set_status(shell::vfs_error(failure)),
                    }
                }
                Err(failure) => self.files_set_status(shell::vfs_error(failure)),
            }
        }
        self.files_selected = usize::MAX;
        self.files_reload();
    }

    fn files_new_folder(&mut self, name: &str) {
        if name.is_empty() {
            self.files_set_status("name cannot be empty");
            return;
        }
        let target = self.files_child_path(name);
        match vfs::create_directory(target.as_str(), 0o777) {
            Ok(()) => self.files_set_status("folder created"),
            Err(failure) => self.files_set_status(shell::vfs_error(failure)),
        }
        self.files_reload();
    }

    fn push_name_byte(&mut self, byte: u8) -> bool {
        if byte == b'/' || !(byte.is_ascii_graphic() || byte == b' ') {
            return false;
        }
        // Names typed here always end up persisted under /data, which only
        // accepts short, uppercase 8.3-style names (see
        // VfsError::PersistNameUnsupported) - auto-uppercasing what's typed
        // means the dialog just works instead of surfacing that constraint
        // as an error after the fact.
        self.name_input.push_byte(byte.to_ascii_uppercase())
    }

    fn open_notes(&mut self) {
        self.set_overlay(Overlay::None);
        self.set_app(DesktopApp::Notes);
        self.notes_mode = NotesMode::List;
        self.notes_reload();
    }

    fn notes_reload(&mut self) {
        let (count, overflow) = list_directory(notes_directory(), &mut self.notes_entries);
        self.notes_entry_count = count;
        self.notes_overflow = overflow;
    }

    fn notes_set_status(&mut self, message: &str) {
        self.notes_status.clear();
        let _ = self.notes_status.push_str_checked(message);
    }

    fn notes_path_for(&self, name: &str) -> shell::Text<256> {
        let mut joined: shell::Text<256> = shell::Text::new();
        let _ = shell::normalize_path(notes_directory(), name, &mut joined);
        joined
    }

    fn notes_open(&mut self, name: &str) {
        let path = self.notes_path_for(name);
        let mut buffer = [0u8; NOTE_MAX_BYTES];
        match shell::read_file(path.as_str(), &mut buffer) {
            Ok(length) => {
                self.notes_content.clear();
                if let Ok(text) = core::str::from_utf8(&buffer[..length]) {
                    let _ = self.notes_content.push_str_checked(text);
                }
                self.notes_current_name.clear();
                let _ = self.notes_current_name.push_str_checked(name);
                self.notes_mode = NotesMode::Editing;
                self.notes_set_status("");
            }
            Err(failure) => self.notes_set_status(shell::vfs_error(failure)),
        }
    }

    fn notes_start_new(&mut self, name: &str) {
        if name.is_empty() {
            self.notes_set_status("name cannot be empty");
            return;
        }
        self.notes_current_name.clear();
        let _ = self.notes_current_name.push_str_checked(name);
        self.notes_content.clear();
        self.notes_mode = NotesMode::Editing;
    }

    fn notes_save(&mut self) {
        let _ = vfs::create_directory(notes_directory(), 0o777);
        let path = self.notes_path_for(self.notes_current_name.as_str());
        match vfs::open_file(path.as_str(), true, false, true, 0o644, true) {
            Ok(descriptor) => {
                let result = vfs::write(descriptor, self.notes_content.as_str().as_bytes(), false);
                let _ = vfs::close(descriptor);
                match result {
                    Ok(_) => self.notes_set_status("saved"),
                    Err(failure) => self.notes_set_status(shell::vfs_error(failure)),
                }
            }
            Err(failure) => self.notes_set_status(shell::vfs_error(failure)),
        }
        self.notes_mode = NotesMode::List;
        self.notes_reload();
    }

    fn notes_delete(&mut self, name: &str) {
        let path = self.notes_path_for(name);
        match vfs::remove(path.as_str(), false) {
            Ok(()) => self.notes_set_status("note deleted"),
            Err(failure) => self.notes_set_status(shell::vfs_error(failure)),
        }
        self.notes_reload();
    }

    fn push_notes_byte(&mut self, byte: u8) -> bool {
        if byte != b'\n' && !(byte.is_ascii_graphic() || byte == b' ') {
            return false;
        }
        self.notes_content.push_byte(byte)
    }

    fn open_trash(&mut self) {
        self.set_overlay(Overlay::None);
        self.set_app(DesktopApp::Trash);
        self.trash_reload();
    }

    fn trash_reload(&mut self) {
        let (count, overflow) = list_directory(trash_directory(), &mut self.trash_entries);
        self.trash_entry_count = count;
        self.trash_overflow = overflow;
        if self.trash_selected >= count {
            self.trash_selected = usize::MAX;
        }
    }

    fn trash_set_status(&mut self, message: &str) {
        self.trash_status.clear();
        let _ = self.trash_status.push_str_checked(message);
    }

    fn trash_path_for(&self, name: &str) -> shell::Text<256> {
        let mut joined: shell::Text<256> = shell::Text::new();
        let _ = shell::normalize_path(trash_directory(), name, &mut joined);
        joined
    }

    fn trash_restore_selected(&mut self) {
        let Some(row) = self.trash_entries.get(self.trash_selected).copied() else {
            self.trash_set_status("no item selected");
            return;
        };
        let source = self.trash_path_for(row.name_str());
        let mut buffer = [0u8; NOTE_MAX_BYTES];
        match shell::read_file(source.as_str(), &mut buffer) {
            Ok(length) => {
                let mut destination: shell::Text<256> = shell::Text::new();
                let _ = shell::normalize_path("/data", row.name_str(), &mut destination);
                match vfs::open_file(destination.as_str(), true, false, true, 0o644, true) {
                    Ok(descriptor) => {
                        let result = vfs::write(descriptor, &buffer[..length], false);
                        let _ = vfs::close(descriptor);
                        match result {
                            Ok(_) => match vfs::remove(source.as_str(), false) {
                                Ok(()) => self.trash_set_status("restored to /data"),
                                Err(failure) => self.trash_set_status(shell::vfs_error(failure)),
                            },
                            Err(failure) => self.trash_set_status(shell::vfs_error(failure)),
                        }
                    }
                    Err(failure) => self.trash_set_status(shell::vfs_error(failure)),
                }
            }
            Err(failure) => self.trash_set_status(shell::vfs_error(failure)),
        }
        self.trash_selected = usize::MAX;
        self.trash_reload();
    }

    fn trash_delete_forever_selected(&mut self) {
        let Some(row) = self.trash_entries.get(self.trash_selected).copied() else {
            self.trash_set_status("no item selected");
            return;
        };
        let target = self.trash_path_for(row.name_str());
        match vfs::remove(target.as_str(), row.is_dir) {
            Ok(()) => self.trash_set_status("deleted forever"),
            Err(failure) => self.trash_set_status(shell::vfs_error(failure)),
        }
        self.trash_selected = usize::MAX;
        self.trash_reload();
    }

    fn trash_empty(&mut self) {
        for index in 0..self.trash_entry_count {
            let row = self.trash_entries[index];
            let target = self.trash_path_for(row.name_str());
            let _ = vfs::remove(target.as_str(), row.is_dir);
        }
        self.trash_set_status("trash emptied");
        self.trash_selected = usize::MAX;
        self.trash_reload();
    }

    fn browser_host_str(&self) -> &str {
        core::str::from_utf8(&self.browser_host[..self.browser_host_len]).unwrap_or("")
    }

    fn browser_path_str(&self) -> &str {
        if self.browser_path_len == 0 {
            return "/";
        }
        core::str::from_utf8(&self.browser_path[..self.browser_path_len]).unwrap_or("/")
    }

    fn push_username_byte(&mut self, byte: u8) -> bool {
        if self.username_len >= MAX_NAME || !byte.is_ascii_alphanumeric() {
            return false;
        }
        self.username_input[self.username_len] = byte;
        self.username_len += 1;
        true
    }

    fn push_setup_password_byte(&mut self, byte: u8) -> bool {
        if self.setup_password_len >= MAX_NAME || !byte.is_ascii_graphic() {
            return false;
        }
        self.setup_password_input[self.setup_password_len] = byte;
        self.setup_password_len += 1;
        true
    }

    fn push_confirm_byte(&mut self, byte: u8) -> bool {
        if self.confirm_len >= MAX_NAME || !byte.is_ascii_graphic() {
            return false;
        }
        self.confirm_input[self.confirm_len] = byte;
        self.confirm_len += 1;
        true
    }

    fn wipe_login(&mut self) {
        crate::auth::wipe(&mut self.login_input);
        self.login_len = 0;
    }

    fn wipe_setup_secrets(&mut self) {
        crate::auth::wipe(&mut self.setup_password_input);
        crate::auth::wipe(&mut self.confirm_input);
        self.setup_password_len = 0;
        self.confirm_len = 0;
    }

    /// Escape in a setup step clears that step's field.
    fn wipe_setup_field(&mut self) {
        match self.screen {
            Screen::Username => self.username_len = 0,
            Screen::Password => {
                crate::auth::wipe(&mut self.setup_password_input);
                self.setup_password_len = 0;
            }
            Screen::Confirm => {
                crate::auth::wipe(&mut self.confirm_input);
                self.confirm_len = 0;
            }
            _ => {}
        }
        self.setup_error = "";
    }

    /// Moves setup forward only when the current step is acceptable: a
    /// username, a password that passes the policy, and the same password
    /// typed again. Confirming turns it into a salted hash and wipes the text.
    fn advance_setup(&mut self) -> DesktopAction {
        match self.screen {
            Screen::Keyboard => {
                let mut matches = [0usize; 16];
                let count = flow::keyboard_matches(self, &mut matches);
                if count == 0 {
                    return DesktopAction::Idle;
                }
                let chosen = matches[self.kb_cursor.min(count - 1)];
                if self.persist_account {
                    crate::settings::set_keyboard_layout(chosen);
                } else {
                    crate::keymap::set_layout(chosen);
                }
            }
            Screen::Username if self.username_len == 0 => {
                self.fail_setup("Choose a username to continue");
                return DesktopAction::Redraw;
            }
            Screen::Password => {
                if let Err(reason) = crate::auth::check_password(
                    &self.setup_password_input[..self.setup_password_len],
                    &self.username_input[..self.username_len],
                ) {
                    self.fail_setup(reason);
                    return DesktopAction::Redraw;
                }
            }
            Screen::Confirm => {
                let same = crate::auth::constant_time_eq(
                    &self.setup_password_input[..self.setup_password_len],
                    &self.confirm_input[..self.confirm_len],
                );
                if !same {
                    crate::auth::wipe(&mut self.confirm_input);
                    self.confirm_len = 0;
                    self.fail_setup("Passwords do not match");
                    return DesktopAction::Redraw;
                }
                self.credential = Some(crate::auth::Credential::new(
                    &self.setup_password_input[..self.setup_password_len],
                    self.kdf_iterations,
                ));
                self.wipe_setup_secrets();
                self.account_unsaved = true;
            }
            _ => {}
        }
        self.setup_error = "";
        self.set_screen(self.screen.next());
        DesktopAction::Redraw
    }

    fn fail_setup(&mut self, reason: &'static str) {
        self.setup_error = reason;
        self.setup_error_ns = crate::time::monotonic_nanoseconds();
        self.motion_until_ns = self.motion_until_ns.max(self.setup_error_ns + 460_000_000);
    }

    fn keyboard_refilter(&mut self) {
        let mut matches = [0usize; 16];
        let count = flow::keyboard_matches(self, &mut matches);
        self.kb_cursor = 0;
        self.keyboard_scroll_to(0, count);
    }

    fn keyboard_scroll_to(&mut self, cursor: usize, count: usize) {
        let now = crate::time::monotonic_nanoseconds();
        let current = flow::keyboard_scroll_milli(self, now);
        let visible = flow::KB_VISIBLE;
        let max_first = (count as i32 - visible).max(0);
        let mut first = self.kb_scroll_target;
        if (cursor as i32) < first {
            first = cursor as i32;
        } else if cursor as i32 >= first + visible {
            first = cursor as i32 - visible + 1;
        }
        self.kb_scroll_from = current;
        self.kb_scroll_target = first.clamp(0, max_first);
        self.kb_scroll_at_ns = now;
        self.motion_until_ns = self.motion_until_ns.max(now + 320_000_000);
    }

    fn keyboard_move(&mut self, step: i32) -> DesktopAction {
        let mut matches = [0usize; 16];
        let count = flow::keyboard_matches(self, &mut matches);
        if count == 0 {
            return DesktopAction::Idle;
        }
        let next = (self.kb_cursor as i32 + step).clamp(0, count as i32 - 1) as usize;
        if next == self.kb_cursor {
            return DesktopAction::Idle;
        }
        self.kb_cursor = next;
        self.keyboard_scroll_to(next, count);
        DesktopAction::Redraw
    }

    fn keyboard_type(&mut self, byte: u8) -> DesktopAction {
        if self.kb_search_len >= flow::KB_SEARCH_MAX || !byte.is_ascii_graphic() {
            return DesktopAction::Idle;
        }
        self.kb_search[self.kb_search_len] = byte;
        self.kb_search_len += 1;
        self.keyboard_refilter();
        DesktopAction::Redraw
    }

    fn setup_click(&mut self, point: Point) -> DesktopAction {
        match self.screen {
            Screen::Language => {
                if flow::LANGUAGE_ENGLISH.contains(point) {
                    self.advance_setup()
                } else {
                    DesktopAction::Idle
                }
            }
            Screen::Keyboard => {
                let now = crate::time::monotonic_nanoseconds();
                let scroll = flow::keyboard_scroll_milli(self, now);
                let mut matches = [0usize; 16];
                let count = flow::keyboard_matches(self, &mut matches);
                for position in 0..count {
                    let y = flow::KB_LIST.y + position as i32 * flow::KB_ROW_PITCH
                        - scroll * flow::KB_ROW_PITCH / 1000;
                    let row =
                        Rect::new(flow::KB_LIST.x, y, flow::KB_LIST.width, flow::KB_ROW_HEIGHT);
                    if row.contains(point) && flow::KB_LIST.contains(point) {
                        if self.kb_cursor == position {
                            return self.advance_setup();
                        }
                        self.kb_cursor = position;
                        self.keyboard_scroll_to(position, count);
                        return DesktopAction::Redraw;
                    }
                }
                DesktopAction::Idle
            }
            Screen::Welcome => self.advance_setup(),
            _ => DesktopAction::Idle,
        }
    }

    /// Back to the sign-in screen: overlays close, the clipboard (which may
    /// hold a secret) is emptied and the guest gets no input until sign-in.
    fn lock_session(&mut self) -> bool {
        if self.screen != Screen::Desktop || self.credential.is_none() {
            return false;
        }
        self.wipe_login();
        self.clip_open = false;
        self.set_overlay(Overlay::None);
        crate::clipboard::get().clear();
        self.set_screen(Screen::Login);
        true
    }

    fn push_login_byte(&mut self, byte: u8) -> bool {
        if self.login_len >= MAX_NAME || !byte.is_ascii_graphic() {
            return false;
        }
        self.login_input[self.login_len] = byte;
        self.login_len += 1;
        self.login_typed_ns = crate::time::monotonic_nanoseconds();
        self.motion_until_ns = self.motion_until_ns.max(self.login_typed_ns + 260_000_000);
        true
    }

    fn username_str(&self) -> &str {
        core::str::from_utf8(&self.username_input[..self.username_len]).unwrap_or("")
    }

    fn display_name(&self) -> &str {
        let name = self.username_str();
        if name.is_empty() { "User" } else { name }
    }

    fn try_login(&mut self) -> DesktopAction {
        let now = crate::time::monotonic_nanoseconds();
        if self.lockout.is_locked(now) {
            // Throttled: the attempt is not even checked.
            self.wipe_login();
            return DesktopAction::Redraw;
        }
        let had_input = self.login_len > 0;
        let matches_password = self.login_len > 0
            && self
                .credential
                .as_ref()
                .is_some_and(|credential| credential.verify(&self.login_input[..self.login_len]));
        self.wipe_login();
        serial::format(format_args!(
            "AEROS_LOGIN ok={} failures={} check_ms={}
",
            matches_password,
            self.lockout.failures(),
            crate::time::monotonic_nanoseconds().saturating_sub(now) / 1_000_000
        ));
        if matches_password {
            self.lockout.record_success();
            self.set_screen(Screen::Desktop);
        } else {
            let now = crate::time::monotonic_nanoseconds();
            if had_input {
                self.lockout.record_failure(now);
            }
            self.login_error_until_ns = now.saturating_add(LOGIN_ERROR_NS);
            self.motion_until_ns = self
                .motion_until_ns
                .max(self.login_error_until_ns)
                .max(self.lockout.locked_until_ns());
        }
        DesktopAction::Redraw
    }

    /// The text field that owns the keyboard right now (Notes editor, name
    /// prompts, terminal line, browser address), if any. Passwords never
    /// take part in copy and paste.
    fn clip_field(&self) -> Option<ClipField> {
        if self.screen != Screen::Desktop {
            return None;
        }
        match self.app {
            DesktopApp::Notes if self.notes_mode == NotesMode::Editing => Some(ClipField::Note),
            DesktopApp::Notes if self.notes_mode == NotesMode::Naming => Some(ClipField::Name),
            DesktopApp::Files if self.files_mode == FilesMode::NamingFolder => {
                Some(ClipField::Name)
            }
            DesktopApp::Terminal => Some(ClipField::Terminal),
            DesktopApp::Browser if self.url_active => Some(ClipField::Url),
            _ => None,
        }
    }

    fn clip_field_bytes(&self, field: ClipField) -> &[u8] {
        match field {
            ClipField::Note => self.notes_content.as_str().as_bytes(),
            ClipField::Name => self.name_input.as_str().as_bytes(),
            ClipField::Terminal => &self.terminal_input[..self.terminal_input_len],
            ClipField::Url => &self.url_input[..self.url_len],
        }
    }

    fn clip_field_clear(&mut self, field: ClipField) {
        match field {
            ClipField::Note => self.notes_content.clear(),
            ClipField::Name => self.name_input.clear(),
            ClipField::Terminal => self.terminal_input_len = 0,
            ClipField::Url => self.url_len = 0,
        }
    }

    /// Types `bytes` into the field (characters the field wouldn't accept
    /// from the keyboard are skipped). Returns whether anything changed.
    fn clip_field_paste(&mut self, field: ClipField, bytes: &[u8]) -> bool {
        let mut changed = false;
        for &byte in bytes {
            let byte = if byte == b'\n' && field != ClipField::Note {
                b' '
            } else {
                byte
            };
            changed |= match field {
                ClipField::Note => self.push_notes_byte(byte),
                ClipField::Name => self.push_name_byte(byte),
                ClipField::Terminal => self.push_terminal_input_byte(byte),
                ClipField::Url => self.push_url_byte(byte),
            };
        }
        changed
    }

    fn push_terminal_input_byte(&mut self, byte: u8) -> bool {
        if self.terminal_input_len >= SHELL_LINE_MAX || !byte.is_ascii_graphic() && byte != b' ' {
            return false;
        }
        self.terminal_input[self.terminal_input_len] = byte;
        self.terminal_input_len += 1;
        true
    }

    fn terminal_input_str(&self) -> &str {
        core::str::from_utf8(&self.terminal_input[..self.terminal_input_len]).unwrap_or("")
    }

    fn terminal_push_line(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let take = bytes.len().min(SHELL_LINE_MAX);
        if self.terminal_line_count == SHELL_HISTORY_LINES {
            self.terminal_lines.copy_within(1.., 0);
            self.terminal_line_lens.copy_within(1.., 0);
            self.terminal_line_count -= 1;
        }
        let slot = self.terminal_line_count;
        self.terminal_lines[slot] = [0; SHELL_LINE_MAX];
        self.terminal_lines[slot][..take].copy_from_slice(&bytes[..take]);
        self.terminal_line_lens[slot] = take;
        self.terminal_line_count += 1;
    }

    fn terminal_line_str(&self, index: usize) -> &str {
        core::str::from_utf8(&self.terminal_lines[index][..self.terminal_line_lens[index]])
            .unwrap_or("")
    }
}

fn terminal_submit(state: &mut DesktopState, shell: &mut shell::Shell<'_>) -> shell::Control {
    let mut command: shell::Text<SHELL_LINE_MAX> = shell::Text::new();
    let _ = command.push_str_checked(state.terminal_input_str());
    state.terminal_input = [0; SHELL_LINE_MAX];
    state.terminal_input_len = 0;
    state.terminal_history_index = 0;

    let prefix = if shell.is_elevated() {
        "root"
    } else {
        state.display_name()
    };
    let mut prompt_line: shell::Text<128> = shell::Text::new();
    let _ = write!(prompt_line, "{}> {}", prefix, command.as_str());
    state.terminal_push_line(prompt_line.as_str());

    let mut output: shell::Text<{ shell::MAX_OUTPUT }> = shell::Text::new();
    let control = shell.execute(command.as_str(), &mut output);
    state.terminal_elevated = shell.is_elevated();
    let (executed, status, elevated, denied) = shell.diagnostics();
    serial::format(format_args!(
        "AEROS_SHELL_EXEC sequence={executed} status={status} elevations={elevated} denied={denied} verified=true\n",
    ));
    for line in output.as_str().split('\n') {
        if !line.is_empty() {
            state.terminal_push_line(line);
        }
    }
    control
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DesktopAction {
    Redraw,
    Idle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DesktopKey {
    Tab,
    Activate,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Apps,
    Quick,
    Power,
    Notifications,
    Search,
    BrightnessUp,
    BrightnessDown,
    Terminal,
    Browser,
    Settings,
    Linux,
    Seamless,
    Store,
    Backspace,
    VolumeUp,
    VolumeDown,
    Mute,
    PlayPause,
    NextTrack,
    PrevTrack,
    Character(u8),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClipField {
    Note,
    Name,
    Terminal,
    Url,
}

struct KeyDecoder {
    extended: bool,
    shift: bool,
    caps: bool,
    text_mode: bool,
}

impl KeyDecoder {
    const fn new() -> Self {
        Self {
            extended: false,
            shift: false,
            caps: false,
            text_mode: false,
        }
    }

    fn set_text_mode(&mut self, text_mode: bool) {
        self.text_mode = text_mode;
    }

    fn feed(&mut self, scancode: u8) -> Option<DesktopKey> {
        if scancode == 0xe0 {
            self.extended = true;
            return None;
        }
        let released = scancode & 0x80 != 0;
        let code = scancode & 0x7f;
        // Right Alt is AltGr: it selects the third/fourth layer of the layout.
        if self.extended && code == 0x38 {
            crate::keymap::set_altgr(!released);
            self.extended = false;
            return None;
        }
        if code == 0x2a || code == 0x36 {
            self.shift = !released;
            self.extended = false;
            return None;
        }
        if released {
            self.extended = false;
            return None;
        }
        if code == 0x3a {
            self.caps = !self.caps;
            return None;
        }
        if self.extended {
            self.extended = false;
            return match code {
                0x4b => Some(DesktopKey::Left),
                0x4d => Some(DesktopKey::Right),
                0x48 => Some(DesktopKey::Up),
                0x50 => Some(DesktopKey::Down),
                0x5b | 0x5c => Some(DesktopKey::Apps),
                0x5e => Some(DesktopKey::Power),
                0x30 => Some(DesktopKey::VolumeUp),
                0x2e => Some(DesktopKey::VolumeDown),
                0x20 => Some(DesktopKey::Mute),
                0x22 => Some(DesktopKey::PlayPause),
                0x19 => Some(DesktopKey::NextTrack),
                0x10 => Some(DesktopKey::PrevTrack),
                _ => None,
            };
        }
        if self.text_mode {
            return match code {
                0x01 => Some(DesktopKey::Escape),
                0x0f => Some(DesktopKey::Tab),
                0x1c => Some(DesktopKey::Activate),
                0x0e => Some(DesktopKey::Backspace),
                _ => shell::scancode_character(code, self.shift, self.caps)
                    .map(DesktopKey::Character),
            };
        }
        match code {
            0x01 => Some(DesktopKey::Escape),
            0x0f => Some(DesktopKey::Tab),
            0x1c | 0x39 => Some(DesktopKey::Activate),
            0x1e => Some(DesktopKey::Apps),
            0x10 => Some(DesktopKey::Quick),
            0x19 => Some(DesktopKey::Power),
            0x31 => Some(DesktopKey::Notifications),
            0x35 => Some(DesktopKey::Search),
            0x3f => Some(DesktopKey::BrightnessDown),
            0x40 => Some(DesktopKey::BrightnessUp),
            0x14 => Some(DesktopKey::Terminal),
            0x30 => Some(DesktopKey::Browser),
            0x1f => Some(DesktopKey::Settings),
            0x26 => Some(DesktopKey::Linux),
            0x25 => Some(DesktopKey::Seamless),
            0x22 => Some(DesktopKey::Store),
            0x0d => Some(DesktopKey::VolumeUp),
            0x0c => Some(DesktopKey::VolumeDown),
            0x32 => Some(DesktopKey::PlayPause),
            0x34 => Some(DesktopKey::NextTrack),
            0x33 => Some(DesktopKey::PrevTrack),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
struct Layout {
    scale: Scale,
    offset: Point,
}

impl Layout {
    fn new(frame: &FrameBuffer) -> Self {
        let width_scale = frame.width().saturating_mul(1_000) / DESIGN_WIDTH as usize;
        let height_scale = frame.height().saturating_mul(1_000) / DESIGN_HEIGHT as usize;
        let milli = width_scale.min(height_scale).clamp(500, 2_000) as u16;
        let scale = Scale::from_milli(milli).unwrap_or(Scale::ONE);
        Self::with_scale(frame, scale)
    }

    fn with_scale(frame: &FrameBuffer, scale: Scale) -> Self {
        let content_width = scale.logical(DESIGN_WIDTH);
        let content_height = scale.logical(DESIGN_HEIGHT);
        Self {
            scale,
            offset: Point::new(
                (frame.width() as i32 - content_width) / 2,
                (frame.height() as i32 - content_height) / 2,
            ),
        }
    }

    fn rect(self, value: Rect) -> Rect {
        let mut result = self.scale.rect(value);
        result.x = result.x.saturating_add(self.offset.x);
        result.y = result.y.saturating_add(self.offset.y);
        result
    }

    fn point(self, value: Point) -> Point {
        Point::new(
            self.offset.x.saturating_add(self.scale.logical(value.x)),
            self.offset.y.saturating_add(self.scale.logical(value.y)),
        )
    }

    fn radii(self, value: CornerRadii) -> CornerRadii {
        self.scale.radii(value)
    }

    fn frost(self, value: FrostStyle) -> FrostStyle {
        value.scaled(self.scale)
    }

    fn to_logical(self, value: Point) -> Point {
        Point::new(
            self.scale.invert(value.x.saturating_sub(self.offset.x)),
            self.scale.invert(value.y.saturating_sub(self.offset.y)),
        )
    }
}

#[derive(Clone, Copy)]
pub struct DesktopReport {
    pub wallpaper: bool,
    pub dock: bool,
    pub app_switcher: bool,
    pub quick_settings: bool,
    pub window: bool,
    pub button: bool,
    pub input: bool,
    pub verified: bool,
}

/// Checks the window open/close animation curves frame by frame (a
/// screenshot can't catch a 150 ms animation mid-flight): the close is the
/// exact mirror of the open (920 -> 1000 permille and back), both move
/// monotonically, both stay centred, and "no close time" means no animation.
pub fn window_animation_self_test() -> bool {
    let base = Rect::new(100, 100, 1000, 500);
    let start = 1_000_000u64;
    let mut ok = true;
    let mut last_open = 0;
    let mut last_close = i32::MAX;
    for step in 0..=8u64 {
        let now = start + WINDOW_OPEN_NS * step / 8;
        let open = window_open_scale(base, start, now);
        let close = window_close_scale(base, start, now);
        ok &= (open.width + close.width - 1920).abs() <= 2;
        ok &= open.width >= last_open && close.width <= last_close;
        ok &= (open.x * 2 + open.width - (base.x * 2 + base.width)).abs() <= 1;
        ok &= (close.y * 2 + close.height - (base.y * 2 + base.height)).abs() <= 1;
        if step == 0 {
            ok &= open.width == 920 && close.width == 1000;
        }
        if step == 8 {
            ok &= open.width == 1000 && close.width == 920;
        }
        last_open = open.width;
        last_close = close.width;
    }
    ok &= window_close_scale(base, 0, start).width == 1000;
    ok
}

pub fn render_self_test(real_frame: &mut FrameBuffer, fonts: &FontCatalog) -> DesktopReport {
    let button_report = button::self_test();
    let input = input_self_test();
    let Some(ui_font) = fonts.ui() else {
        return empty_report(button_report.verified, input);
    };
    let Some(mono_font) = fonts.mono() else {
        return empty_report(button_report.verified, input);
    };
    let info = real_frame.info();
    let required = info.stride.saturating_mul(info.height);
    let mut staging = if required <= MAX_DESKTOP_PIXELS {
        let address = unsafe { (*DESKTOP_BUFFER.0.get()).as_mut_ptr() };
        unsafe {
            FrameBuffer::new(FrameBufferInfo {
                address,
                size: required.saturating_mul(core::mem::size_of::<u32>()),
                ..info
            })
        }
    } else {
        unsafe { FrameBuffer::new(info) }
    };
    let frame = &mut staging;
    draw_wallpaper(frame, None);
    let layout = Layout::with_scale(frame, Scale::ONE);
    let state = DesktopState::new();
    let now = crate::time::monotonic_nanoseconds();
    let (dock, controls) = {
        let mut painter = Painter::new(frame);
        draw_dock(&mut painter, layout, ui_font, &state, true, now)
    };
    let apps = {
        let mut painter = Painter::new(frame);
        draw_app_switcher(&mut painter, layout, ui_font, &state, now, false, 0, false)
    };
    let quick = {
        let mut painter = Painter::new(frame);
        panels::draw_quick(&mut painter, layout, ui_font, &state, now)
    };
    let window = {
        let mut window_state = state;
        window_state.app = DesktopApp::Settings;
        let mut painter = Painter::new(frame);
        let settings = draw_window(
            &mut painter,
            layout,
            ui_font,
            mono_font,
            &window_state,
            now,
            false,
        );
        window_state.app = DesktopApp::Browser;
        let browser = draw_window(
            &mut painter,
            layout,
            ui_font,
            mono_font,
            &window_state,
            now,
            false,
        );
        window_state.app = DesktopApp::Terminal;
        let terminal = draw_window(
            &mut painter,
            layout,
            ui_font,
            mono_font,
            &window_state,
            now,
            false,
        );
        settings && browser && terminal
    };
    let session = {
        let mut painter = Painter::new(frame);
        let mut screen_state = state;
        screen_state.screen = Screen::Language;
        let language = draw_setup(&mut painter, layout, ui_font, &screen_state);
        screen_state.screen = Screen::Keyboard;
        let keyboard = draw_setup(&mut painter, layout, ui_font, &screen_state);
        screen_state.screen = Screen::Welcome;
        let welcome = draw_setup(&mut painter, layout, ui_font, &screen_state);
        screen_state.screen = Screen::Password;
        let password = draw_setup(&mut painter, layout, ui_font, &screen_state);
        let lock = draw_lock(&mut painter, layout, ui_font, &screen_state, now, false);
        let signin = draw_signin(&mut painter, layout, ui_font, &screen_state, now, false);
        language && keyboard && welcome && password && lock && signin
    };
    let shell_ui = {
        let bounds = Rect::new(0, 0, frame.width() as i32, frame.height() as i32);
        let mut painter = Painter::new(frame);
        let mut ui_state = state;
        ui_state.screen = Screen::Desktop;
        ui_state.power_open = true;
        ui_state.power_at_ns = now.saturating_sub(2_000_000_000);
        ui_state.power_flyout = true;
        ui_state.power_flyout_ns = now.saturating_sub(2_000_000_000);
        ui_state.ctx_open = true;
        ui_state.ctx_at_ns = now.saturating_sub(2_000_000_000);
        ui_state.ctx_origin = Point::new(200, 120);
        ui_state.search_open = true;
        ui_state.search_at_ns = now.saturating_sub(2_000_000_000);
        ui_state.notif_open = true;
        ui_state.notif_at_ns = now.saturating_sub(2_000_000_000);
        crate::notify::push(0, "Self test", "A notification for the render check");
        let power = shellui::draw_power_menu(&mut painter, layout, ui_font, bounds, &ui_state, now);
        shellui::draw_lockout(&mut painter, layout, ui_font, bounds, 42, now, now);
        panels::draw_toast(&mut painter, layout, ui_font, &ui_state, now);
        panels::draw_notify_stack(&mut painter, layout, ui_font, &ui_state, now);
        panels::draw_context(&mut painter, layout, ui_font, &ui_state, now);
        panels::draw_clipboard_panel(&mut painter, layout, ui_font, &ui_state, now);
        panels::draw_hud(&mut painter, layout, &ui_state, now);
        search::draw_search(&mut painter, layout, ui_font, &ui_state, now);
        crate::notify::clear();
        power
    };
    draw_cursor_at(frame, 40, 40);
    let wallpaper = wallpaper_valid();
    let dock = dock && controls;
    let app_switcher = apps;
    let quick_settings = quick;
    let verified = wallpaper
        && dock
        && app_switcher
        && quick_settings
        && window
        && session
        && shell_ui
        && button_report.verified
        && input;
    DesktopReport {
        wallpaper,
        dock,
        app_switcher,
        quick_settings,
        window,
        button: button_report.verified,
        input,
        verified,
    }
}

pub fn run(frame: &mut FrameBuffer, fonts: &FontCatalog, shell_info: shell::SystemInfo<'_>) -> ! {
    let mut state = DesktopState::new();
    #[cfg(not(feature = "boot-test"))]
    {
        state.persist_account = true;
        if state.load_account() {
            state.screen = Screen::Lock;
        }
    }
    {
        // Boot splash first; setup then fades in from dark once it ends.
        let started = crate::time::monotonic_nanoseconds().max(1);
        BOOT_SPLASH_START_NS.store(started, Ordering::Relaxed);
        crate::audio::play_effect(crate::sfx::Effect::Boot);
        state.screen_transition_at_ns = started + BOOT_SPLASH_NS;
        state.motion_until_ns = started + BOOT_SPLASH_NS + SCREEN_TRANSITION_NS + 100_000_000;
    }
    let mut shell = shell::Shell::new(shell_info, true);
    let mut decoder = KeyDecoder::new();
    let mut cursor = CursorSprite::new();
    crate::mouse::set_bounds(frame.width(), frame.height());
    let _ = present_desktop_mode(frame, fonts, &state, true);
    cursor.paint(frame, cursor_target(frame));
    serial::line(
        "AEROS_DESKTOP_RUNTIME active=true keyboard_navigation=true terminal_hotkey=t atomic_present=true session=setup->login->desktop pointer=true",
    );
    arch::enable_interrupts();
    arch::apic::start_timer(MOTION_TICK_HZ);
    let mut minute = crate::rtc::unix_seconds() / 60;
    let mut pointer_generation = crate::mouse::state().generation;
    let mut pointer_down = false;
    let mut right_was_down = false;
    let mut idle_ticks: u32 = 0;
    let mut settle_repaints: u32 = 12;
    let mut motion_animating = false;
    let mut last_ambient_ns = 0u64;
    let mut last_pointer_event_ns = 0u64;
    let mut last_paint_cost_ns = 0u64;
    let mut blink_phase = true;
    let mut setup_caret = true;
    let mut linux_screen_hash = 0u64;
    let mut linux_last_paint_ns = 0u64;
    let mut guest_ptr = GuestPointer::new();
    let mut seam_generation_seen = 0u32;
    let mut last_web_paint_ns = 0u64;
    // Clipboard shortcuts: modifier state, the guest's clipboard sequence we
    // last took, a scratch buffer, and a paste to replay into the guest once
    // its agent has taken over the new clipboard text.
    let mut ext_pending = false;
    let mut ctrl_down = false;
    let mut meta_down = false;
    let mut swallow_v = false;
    let mut meta_used = false;
    let mut guest_clip_seen = 0u32;
    let clip_buf = crate::clipboard::scratch();
    let mut guest_paste_at: Option<u64> = None;
    let mut swallow_l = false;
    let mut last_pointer_pos = (0i32, 0i32);
    let mut touch_down: Option<(i32, i32)> = None;
    let mut touch_last_y = 0i32;
    let mut touch_moved = false;
    let mut last_input_ns = crate::time::monotonic_nanoseconds();
    loop {
        let mut redraw = false;
        let splash_active = boot_splash(crate::time::monotonic_nanoseconds()).is_some();
        if let Some(action) = state.power_due(crate::time::monotonic_nanoseconds()) {
            match action {
                shellui::PowerAction::Restart => {
                    serial::line("AEROS_REBOOT_REQUESTED source=power-menu authorized=true");
                    arch::reboot();
                }
                _ => {
                    let power = crate::power::current();
                    serial::line("AEROS_SHUTDOWN source=power-menu filesystems=safe");
                    if power.ready {
                        crate::power::shutdown(&power);
                    } else {
                        arch::halt_forever();
                    }
                }
            }
        }
        if state.account_unsaved
            && state.screen == Screen::Welcome
            && crate::time::monotonic_nanoseconds() >= state.screen_transition_at_ns + 700_000_000
        {
            state.account_unsaved = false;
            state.save_account();
        }
        if !splash_active
            && state.screen == Screen::Welcome
            && crate::time::monotonic_nanoseconds()
                >= state.screen_transition_at_ns + flow::WELCOME_NS
        {
            state.set_screen(Screen::Lock);
            redraw = true;
        }
        while let Some(scancode) = keyboard::pop_scancode() {
            if splash_active {
                continue;
            }
            if state.osk_pending > 0 {
                state.osk_pending -= 1;
            } else if state.touch_mode {
                state.touch_mode = false;
                redraw = true;
            }
            last_input_ns = crate::time::monotonic_nanoseconds();
            let was_extended = ext_pending;
            ext_pending = scancode == 0xe0;
            let code = scancode & 0x7f;
            let released = scancode & 0x80 != 0;
            if scancode != 0xe0 {
                if code == 0x1d {
                    ctrl_down = !released;
                }
                if was_extended && (code == 0x5b || code == 0x5c) {
                    meta_down = !released;
                } else if meta_down && !released {
                    // Super was used as a modifier: releasing it later must
                    // not also open the app switcher.
                    meta_used = true;
                }
            }
            // Print Screen: save a picture of the screen to /home/Pictures.
            if was_extended && code == 0x37 && !released && state.screen == Screen::Desktop {
                match crate::screenshot::save(frame) {
                    Ok(path) => {
                        serial::format(format_args!(
                            "AEROS_SCREENSHOT_SAVED {}
",
                            path.as_str()
                        ));
                        crate::notify::push(3, "Screenshot", "Saved to your Pictures folder");
                    }
                    Err(_) => serial::line("AEROS_SCREENSHOT_FAILED"),
                }
                continue;
            }
            if state.screen == Screen::Desktop {
                // Super+L: lock the session.
                if scancode != 0xe0 && code == 0x26 && !was_extended {
                    if !released && meta_down {
                        meta_used = true;
                        swallow_l = true;
                        redraw |= state.lock_session();
                        continue;
                    }
                    if released && swallow_l {
                        swallow_l = false;
                        continue;
                    }
                }
                // Super+V: clipboard history.
                if scancode != 0xe0 && code == 0x2f && !was_extended {
                    if !released && meta_down {
                        state.clip_open = !state.clip_open;
                        state.clip_at_ns = crate::time::monotonic_nanoseconds();
                        state.motion_until_ns =
                            state.motion_until_ns.max(state.clip_at_ns + 700_000_000);
                        state.clip_selected = 0;
                        state.set_overlay(Overlay::None);
                        meta_used = true;
                        swallow_v = true;
                        redraw = true;
                        continue;
                    }
                    if released && swallow_v {
                        swallow_v = false;
                        continue;
                    }
                }
                if state.clip_open {
                    // The popup owns the keyboard: nothing reaches the guest
                    // or the desktop until it closes.
                    if scancode != 0xe0 && !released {
                        match (was_extended, code) {
                            (_, 0x01) => state.clip_open = false,
                            (true, 0x48) => {
                                state.clip_selected = state.clip_selected.saturating_sub(1)
                            }
                            (true, 0x50)
                                if state.clip_selected + 1 < crate::clipboard::get().len() =>
                            {
                                state.clip_selected += 1;
                            }
                            (true, 0x53) => {
                                crate::clipboard::get().clear();
                                state.clip_selected = 0;
                            }
                            (false, 0x1c) | (false, 0x02..=0x0a) => {
                                let index = if code == 0x1c {
                                    state.clip_selected
                                } else {
                                    (code - 0x02) as usize
                                };
                                if index < crate::clipboard::get().len() {
                                    crate::clipboard::get().select(index);
                                    // Hand the pick to the guest even when it
                                    // was already the newest entry.
                                    state.clip_synced = u32::MAX;
                                    state.clip_open = false;
                                    if guest_has_keyboard(&state) {
                                        guest_paste_at = Some(
                                            crate::time::monotonic_nanoseconds() + 150_000_000,
                                        );
                                    } else if let Some(field) = state.clip_field() {
                                        let len = crate::clipboard::get().entry(0).len();
                                        clip_buf[..len]
                                            .copy_from_slice(crate::clipboard::get().entry(0));
                                        state.clip_field_paste(field, &clip_buf[..len]);
                                    }
                                }
                            }
                            _ => {}
                        }
                        redraw = true;
                    }
                    continue;
                }
            }
            // With the Linux window up, the keyboard belongs to the guest:
            // every scancode byte goes to its emulated PS/2 controller
            // untouched, except F10, which is the way back to AerOS.
            if state.screen == Screen::Desktop
                && crate::svm::linux_ready()
                && (state.app == DesktopApp::Linux
                    || (state.seamless.enabled && state.seamless.focus != 0))
            {
                if scancode == 0x43 && state.app == DesktopApp::Linux {
                    // F9: 1:1 / fit-to-window.
                    LINUX_ZOOM.fetch_xor(true, core::sync::atomic::Ordering::Relaxed);
                    redraw = true;
                } else if scancode == 0x44 {
                    if state.seamless.enabled {
                        state.seamless.focus = 0;
                    } else {
                        state.set_app(DesktopApp::None);
                    }
                    redraw = true;
                } else {
                    crate::svm::linux_feed_scancode(scancode);
                }
                continue;
            }
            // Seamless mode with no guest window focused: single-key launchers
            // (k = leave seamless mode, x = terminal, n = browser).
            if state.seamless.enabled
                && state.screen == Screen::Desktop
                && state.overlay == Overlay::None
                && scancode < 0x80
            {
                match scancode {
                    0x25 => {
                        state.seamless.enabled = false;
                        redraw = true;
                        continue;
                    }
                    0x2d => {
                        crate::svm::linux_command(
                            3,
                            [0; 4],
                            b"xterm -geometry 58x20 -fa 'DejaVu Sans Mono' -fs 10 -bg '#0d1117' -fg '#d7dee6'",
                        );
                        continue;
                    }
                    0x31 => {
                        crate::svm::linux_command(3, [0; 4], b"netsurf-gtk");
                        continue;
                    }
                    0x14 => {
                        // Network self-test inside the guest; the result shows
                        // up in the guest's status line on the serial log.
                        crate::svm::linux_command(
                            3,
                            [0; 4],
                            b"busybox nslookup example.com 2>&1|tail -n 2;busybox wget -T 12 -qO- http://example.com 2>&1|head -c 160",
                        );
                        continue;
                    }
                    0x21 => {
                        // Only present in the full image; a no-op otherwise.
                        crate::svm::linux_command(3, [0; 4], b"aeros-firefox");
                        continue;
                    }
                    _ => {}
                }
            }
            // Ctrl+C / Ctrl+X / Ctrl+V in AerOS's own text fields (a focused
            // Linux window got the keys above and handles them itself).
            if ctrl_down
                && !was_extended
                && !released
                && matches!(code, 0x2d..=0x2f)
                && let Some(field) = state.clip_field()
            {
                if code == 0x2f {
                    if let Some(current) = crate::clipboard::get().current() {
                        let len = current.len();
                        clip_buf[..len].copy_from_slice(current);
                        state.clip_field_paste(field, &clip_buf[..len]);
                    }
                } else {
                    let len = state.clip_field_bytes(field).len().min(clip_buf.len());
                    clip_buf[..len].copy_from_slice(&state.clip_field_bytes(field)[..len]);
                    crate::clipboard::get().push(&clip_buf[..len]);
                    if code == 0x2d {
                        state.clip_field_clear(field);
                    }
                }
                redraw = true;
                continue;
            }
            if was_extended && (code == 0x5b || code == 0x5c) {
                // Super alone (press and release with nothing in between)
                // opens the app switcher; Super+key is a shortcut.
                let _ = decoder.feed(scancode);
                if !released {
                    meta_used = false;
                } else if !meta_used {
                    redraw |= matches!(state.handle(DesktopKey::Apps), DesktopAction::Redraw);
                }
                continue;
            }
            decoder.set_text_mode(state.text_input_active());
            let Some(key) = decoder.feed(scancode) else {
                continue;
            };
            if state.app == DesktopApp::Terminal
                && matches!(
                    key,
                    DesktopKey::Character(_)
                        | DesktopKey::Backspace
                        | DesktopKey::Activate
                        | DesktopKey::Up
                        | DesktopKey::Down
                )
            {
                match key {
                    DesktopKey::Character(byte) => {
                        redraw |= state.push_terminal_input_byte(byte);
                    }
                    DesktopKey::Backspace => {
                        state.terminal_input_len = state.terminal_input_len.saturating_sub(1);
                        redraw = true;
                    }
                    DesktopKey::Up => {
                        if state.terminal_history_index == 0 {
                            state.terminal_draft = state.terminal_input;
                            state.terminal_draft_len = state.terminal_input_len;
                        }
                        if let Some(entry) = shell.history_entry(state.terminal_history_index) {
                            state.terminal_input = [0; SHELL_LINE_MAX];
                            let bytes = entry.as_bytes();
                            let count = bytes.len().min(SHELL_LINE_MAX);
                            state.terminal_input[..count].copy_from_slice(&bytes[..count]);
                            state.terminal_input_len = count;
                            state.terminal_history_index += 1;
                            redraw = true;
                        }
                    }
                    DesktopKey::Down if state.terminal_history_index > 0 => {
                        state.terminal_history_index -= 1;
                        if state.terminal_history_index == 0 {
                            state.terminal_input = state.terminal_draft;
                            state.terminal_input_len = state.terminal_draft_len;
                        } else if let Some(entry) =
                            shell.history_entry(state.terminal_history_index - 1)
                        {
                            state.terminal_input = [0; SHELL_LINE_MAX];
                            let bytes = entry.as_bytes();
                            let count = bytes.len().min(SHELL_LINE_MAX);
                            state.terminal_input[..count].copy_from_slice(&bytes[..count]);
                            state.terminal_input_len = count;
                        }
                        redraw = true;
                    }
                    DesktopKey::Activate => {
                        match terminal_submit(&mut state, &mut shell) {
                            shell::Control::Reboot => {
                                serial::line("AEROS_REBOOT_REQUESTED source=shell authorized=true");
                                arch::reboot();
                            }
                            shell::Control::Halt => {
                                let power = crate::power::current();
                                if power.ready {
                                    serial::line(
                                        "AEROS_SHUTDOWN source=shell filesystems=safe method=acpi",
                                    );
                                    crate::power::shutdown(&power);
                                } else {
                                    serial::line(
                                        "AEROS_SHUTDOWN source=shell filesystems=safe method=halt",
                                    );
                                    arch::halt_forever();
                                }
                            }
                            shell::Control::Clear => {
                                state.terminal_lines = [[0; SHELL_LINE_MAX]; SHELL_HISTORY_LINES];
                                state.terminal_line_lens = [0; SHELL_HISTORY_LINES];
                                state.terminal_line_count = 0;
                            }
                            shell::Control::None => {}
                        }
                        redraw = true;
                    }
                    _ => {}
                }
                continue;
            }
            match state.handle(key) {
                DesktopAction::Redraw => redraw = true,
                DesktopAction::Idle => {}
            }
        }
        if let Some(at) = guest_paste_at
            && crate::time::monotonic_nanoseconds() >= at
        {
            guest_paste_at = None;
            // Ctrl+V into whatever the guest has focused.
            for code in [0x1d, 0x2f, 0xaf, 0x9d] {
                crate::svm::linux_feed_scancode(code);
            }
        }
        if crate::svm::linux_ready() {
            if let Some(len) = crate::svm::linux_clipboard_take(&mut guest_clip_seen, clip_buf)
                && crate::clipboard::get().push(&clip_buf[..len])
            {
                state.clip_synced = crate::clipboard::get().generation;
                redraw |= state.clip_open;
            }
            if crate::clipboard::get().generation != state.clip_synced {
                let generation = crate::clipboard::get().generation;
                if let Some(current) = crate::clipboard::get().current() {
                    crate::svm::linux_clipboard_set(current);
                }
                state.clip_synced = generation;
            }
        }
        let pointer = crate::mouse::state();
        let now_pointer_ns = crate::time::monotonic_nanoseconds();
        let pointer_moved = pointer.generation != pointer_generation;
        let packets = pointer.generation.saturating_sub(pointer_generation);
        pointer_generation = pointer.generation;
        {
            // A finger lands: the pointer jumps to a new place in a packet or
            // two. A mouse glides through many small steps.
            let finger_idle = now_pointer_ns.saturating_sub(last_pointer_event_ns) > 450_000_000;
            if pointer_moved {
                last_pointer_event_ns = now_pointer_ns;
            }
            let jump = (pointer.x - last_pointer_pos.0)
                .abs()
                .max((pointer.y - last_pointer_pos.1).abs());
            if pointer_moved && jump >= 60 && packets <= 6 && finger_idle && pointer.left {
                if !state.touch_mode {
                    state.touch_mode = true;
                    redraw = true;
                    serial::line("AEROS_TOUCH mode=on");
                }
            } else if pointer_moved && jump > 0 && jump < 24 && !pointer.left && state.touch_mode {
                state.touch_mode = false;
                redraw = true;
                serial::line("AEROS_TOUCH mode=off");
            }
            last_pointer_pos = (pointer.x, pointer.y);
            if state.app != DesktopApp::Store || crate::store::get().detail.is_some() {
                state.store_search_focus = false;
            }
            let active = state.osk_wanted();
            if !active {
                state.osk_dismissed = false;
                state.osk_symbols = false;
                state.osk_shift = false;
            }
            let want = state.touch_mode && active && !state.osk_dismissed && !splash_active;
            if want != state.osk_open {
                state.osk_open = want;
                redraw = true;
            }
        }
        let now_ns = crate::time::monotonic_nanoseconds();
        if pointer_moved || pointer.left {
            last_input_ns = now_ns;
        }
        if state.screen == Screen::Desktop
            && now_ns.saturating_sub(last_input_ns) >= AUTO_LOCK_NS
            && state.lock_session()
        {
            redraw = true;
        }
        if state.app == DesktopApp::Linux && linux_zoomed() {
            let before = linux_pan();
            update_linux_pan(frame, &pointer);
            redraw |= linux_pan() != before;
        }
        let press_edge = pointer.left && !pointer_down;
        if state.seamless.enabled
            && crate::svm::linux_ready()
            && let Some(windows) = crate::svm::linux_windows()
            && windows.generation != seam_generation_seen
        {
            seam_generation_seen = windows.generation;
            if state.seamless.sync(&windows) {
                redraw = true;
            }
        }
        let mut seam_consumed = false;
        let mut seam_over = false;
        if state.seamless.enabled {
            let outcome = seamless_pointer(frame, &mut state, &pointer, press_edge, &mut guest_ptr);
            seam_consumed = outcome.consumed;
            seam_over = outcome.over;
            redraw |= outcome.redraw;
        }
        // Every press since the last frame, so a tap that came and went
        // between two frames still counts.
        let mut presses = [(0i32, 0i32); 8];
        let mut press_count = 0;
        while let Some(point) = crate::mouse::take_press() {
            if press_count < presses.len() {
                presses[press_count] = point;
                press_count += 1;
            }
        }
        if state.seamless.enabled {
            if press_edge && !seam_consumed && !splash_active {
                let layout = Layout::new(frame);
                let logical = layout.to_logical(Point::new(pointer.x, pointer.y));
                match state.click(logical) {
                    DesktopAction::Redraw => redraw = true,
                    DesktopAction::Idle => {}
                }
            }
        } else if !splash_active {
            for (index, &(press_x, press_y)) in presses[..press_count].iter().enumerate() {
                let still_down = pointer.left && index + 1 == press_count;
                if state.touch_mode && still_down {
                    // A finger that is still down: act when it lifts, unless
                    // it turns into a swipe.
                    touch_down = Some((press_x, press_y));
                    touch_last_y = press_y;
                    touch_moved = false;
                } else {
                    let layout = Layout::new(frame);
                    let logical = layout.to_logical(Point::new(press_x, press_y));
                    match state.click(logical) {
                        DesktopAction::Redraw => redraw = true,
                        DesktopAction::Idle => {}
                    }
                }
            }
        }
        if let Some((start_x, start_y)) = touch_down {
            if pointer.left {
                if (pointer.x - start_x).abs().max((pointer.y - start_y).abs()) > 20 {
                    touch_moved = true;
                }
                let step = pointer.y - touch_last_y;
                if touch_moved && step.abs() >= 60 {
                    let direction = if step < 0 { 1 } else { -1 };
                    if state.app == DesktopApp::Store && state.overlay == Overlay::None {
                        crate::store::get().scroll_rows(direction);
                        redraw = true;
                    } else if state.app == DesktopApp::Browser && state.overlay == Overlay::None {
                        crate::web::get().scroll_by(direction * 3, BROWSER_VISIBLE_LINES);
                        redraw = true;
                    }
                    touch_last_y = pointer.y;
                }
            } else {
                touch_down = None;
                if !touch_moved && !splash_active {
                    let layout = Layout::new(frame);
                    let logical = layout.to_logical(Point::new(start_x, start_y));
                    match state.click(logical) {
                        DesktopAction::Redraw => redraw = true,
                        DesktopAction::Idle => {}
                    }
                }
            }
        }
        pointer_down = pointer.left;
        let over_linux =
            seam_over || forward_pointer_to_linux(frame, &state, &pointer, &mut guest_ptr);
        // Scroll wheel: only when it is over guest content, else discard it so
        // stale motion doesn't fire later.
        let wheel = crate::mouse::take_wheel();
        if wheel != 0 && over_linux {
            crate::svm::linux_mouse_wheel(pointer_buttons(&pointer), wheel);
        } else if wheel != 0 && state.app == DesktopApp::Store && state.overlay == Overlay::None {
            crate::store::get().scroll_rows(wheel);
        } else if wheel != 0 && state.app == DesktopApp::Browser && state.overlay == Overlay::None {
            crate::web::get().scroll_by(wheel * 3, BROWSER_VISIBLE_LINES);
            redraw = true;
        }
        if over_linux != guest_ptr.over {
            cursor.erase(frame);
            cursor.hidden = over_linux;
            if !over_linux {
                cursor.paint(frame, cursor_target(frame));
            }
            guest_ptr.over = over_linux;
        }
        {
            let now = crate::time::monotonic_nanoseconds();
            let generation = crate::notify::generation();
            if generation != state.notif_seen {
                state.notif_seen = generation;
                if state.screen == Screen::Desktop && !state.notif_open {
                    state.notice_arrived(now);
                }
                redraw = true;
            }
            if pointer_moved && !splash_active {
                let layout = Layout::new(frame);
                let logical = layout.to_logical(Point::new(pointer.x, pointer.y));
                if state.hover_dock(logical, now) {
                    redraw = true;
                }
            }
            let right_edge = pointer.right && !right_was_down;
            right_was_down = pointer.right;
            if right_edge
                && !splash_active
                && state.screen == Screen::Desktop
                && state.app == DesktopApp::None
                && state.overlay == Overlay::None
                && !state.power_open
                && !state.clip_open
            {
                let layout = Layout::new(frame);
                let logical = layout.to_logical(Point::new(pointer.x, pointer.y));
                if logical.y < 350 {
                    state.open_context_menu(logical);
                    redraw = true;
                }
            }
        }
        {
            // Realtime protection: periodic sweep, and a toast for anything
            // the Shield just did.
            let now = crate::time::monotonic_nanoseconds();
            crate::antivirus::tick(now);
            let seq = crate::antivirus::event_seq();
            if seq != state.shield_seen {
                state.shield_seen = seq;
                if let Some(event) = crate::antivirus::latest_event() {
                    let mut text = ClockBuffer56::new();
                    let _ = write!(text, "Shield: {}  {}", event.kind, event.name);
                    state.shield_text = text.bytes;
                    state.shield_len = text.len;
                    crate::notify::push(1, "AerOS Shield", text.as_str());
                    redraw = true;
                }
            }
        }
        let current_minute = crate::rtc::unix_seconds() / 60;
        if current_minute != minute {
            minute = current_minute;
            redraw = true;
        }
        {
            let now_ns = crate::time::monotonic_nanoseconds();
            if state.ambient_active(now_ns)
                && now_ns.saturating_sub(last_ambient_ns)
                    >= AMBIENT_FRAME_NS.max(last_paint_cost_ns * 2)
                && !keyboard::has_pending()
            {
                last_ambient_ns = now_ns;
                redraw = true;
            }
        }
        crate::audio::pump();
        crate::xhci::poll();
        crate::datafs::poll();
        crate::ntp::tick(crate::time::monotonic_nanoseconds());
        crate::keymap::sync_guest();
        let motion_animating_now = crate::time::monotonic_nanoseconds() < state.motion_until_ns;
        if motion_animating_now || motion_animating {
            redraw = true;
        }
        motion_animating = motion_animating_now;
        if matches!(
            state.screen,
            Screen::Keyboard | Screen::Username | Screen::Password | Screen::Confirm
        ) || state.search_open
        {
            let caret_now = (crate::time::monotonic_nanoseconds() / 520_000_000).is_multiple_of(2);
            if caret_now != setup_caret {
                setup_caret = caret_now;
                redraw = true;
            }
        }
        if state.app == DesktopApp::Terminal {
            let blink_phase_now =
                (crate::time::monotonic_nanoseconds() / 530_000_000).is_multiple_of(2);
            if blink_phase_now != blink_phase {
                blink_phase = blink_phase_now;
                redraw = true;
            }
        }
        if crate::svm::linux_ready() {
            let now_ns = crate::time::monotonic_nanoseconds();
            let web = crate::web::get();
            if state.app == DesktopApp::Browser || web.busy() {
                let before = (web.state, web.line_count());
                web.tick(now_ns);
                let animating = web.busy()
                    && now_ns.saturating_sub(last_web_paint_ns)
                        > if web.state == crate::web::WebState::Starting {
                            300_000_000
                        } else {
                            120_000_000
                        };
                if (web.state, web.line_count()) != before || animating {
                    last_web_paint_ns = now_ns;
                    redraw |= state.app == DesktopApp::Browser;
                }
            }
            let store = crate::store::get();
            let before = (store.count, store.install);
            store.tick();
            let installing = store.install == crate::store::InstallState::Installing
                && now_ns.saturating_sub(last_web_paint_ns) > 500_000_000;
            if state.app == DesktopApp::Store
                && ((store.count, store.install) != before || installing)
            {
                last_web_paint_ns = now_ns;
                redraw = true;
            }
        }
        if redraw {
            let paint_start = crate::time::monotonic_nanoseconds();
            let verified = present_desktop(frame, fonts, &state);
            last_paint_cost_ns = crate::time::monotonic_nanoseconds().saturating_sub(paint_start);
            cursor.erase(frame);
            cursor.paint(frame, cursor_target(frame));
            serial::format(format_args!(
                "AEROS_DESKTOP_REDRAW overlay={} window={} verified={} app={} screen={} cost_us={}\n",
                state.overlay.name(),
                state.app != DesktopApp::None,
                verified,
                state.app.name(),
                state.screen.name(),
                last_paint_cost_ns / 1000
            ));
        } else if pointer_moved {
            cursor.erase(frame);
            cursor.paint(frame, cursor_target(frame));
        }
        if settle_repaints > 0 {
            idle_ticks = idle_ticks.wrapping_add(1);
            if idle_ticks >= 90 {
                idle_ticks = 0;
                settle_repaints -= 1;
                let _ = present_desktop_mode(frame, fonts, &state, true);
                cursor.erase(frame);
                cursor.paint(frame, cursor_target(frame));
            }
        }
        if state.browser_pending {
            state.browser_pending = false;
            let mut host_buffer = [0u8; MAX_URL];
            let mut path_buffer = [0u8; MAX_PATH];
            host_buffer[..state.browser_host_len]
                .copy_from_slice(&state.browser_host[..state.browser_host_len]);
            path_buffer[..state.browser_path_len]
                .copy_from_slice(&state.browser_path[..state.browser_path_len]);
            let host = core::str::from_utf8(&host_buffer[..state.browser_host_len]).unwrap_or("");
            let path = core::str::from_utf8(&path_buffer[..state.browser_path_len]).unwrap_or("/");
            let lookup = crate::net::resolve(host);
            state.browser_address = lookup.address;
            if !lookup.verified {
                state.browser_phase = BrowserPhase::DnsFailed;
                state.browser_online = false;
            } else {
                let mut response = [0u8; 2048];
                let http = crate::net::http_get(lookup.address, host, path, &mut response);
                state.browser_online = http.connected && http.received;
                state.browser_http_status = http.status;
                state.browser_http_bytes = http.bytes;
                if !http.connected {
                    state.browser_phase = BrowserPhase::ConnectFailed;
                } else {
                    let body_start = http_body_offset(&response[..http.bytes]);
                    let raw_body = &response[body_start..http.bytes];
                    let mut dechunk_scratch = [0u8; 2048];
                    let html: &[u8] = if is_chunked(&response[..body_start]) {
                        let length = dechunk(raw_body, &mut dechunk_scratch);
                        &dechunk_scratch[..length]
                    } else {
                        raw_body
                    };
                    state.browser_title_len = extract_html_element(
                        html,
                        b"<title>",
                        b"</title>",
                        &mut state.browser_title,
                    );
                    state.browser_heading_len =
                        extract_html_element(html, b"<h1>", b"</h1>", &mut state.browser_heading);
                    let mut body_scratch = [0u8; 2048];
                    let body_len = extract_body_text(html, &mut body_scratch);
                    state.browser_body.clear();
                    if let Ok(body_text) = core::str::from_utf8(&body_scratch[..body_len]) {
                        let _ = state.browser_body.push_str_checked(body_text);
                    }
                    state.browser_phase = if http.received && (200..400).contains(&http.status) {
                        BrowserPhase::Loaded
                    } else {
                        BrowserPhase::HttpError
                    };
                }
            }
            let verified = present_desktop(frame, fonts, &state);
            cursor.erase(frame);
            cursor.paint(frame, cursor_target(frame));
            serial::format(format_args!(
                "AEROS_BROWSER host={} path={} dns={} tcp={} http_status={} bytes={} phase={} verified={}\n",
                host,
                path,
                lookup.verified,
                state.browser_online,
                state.browser_http_status,
                state.browser_http_bytes,
                state.browser_phase.name(),
                verified
            ));
        }
        // The guest also runs while the browser or the store is open: it
        // fetches the pages and lists the apps.
        if (state.app == DesktopApp::Linux
            || state.seamless.enabled
            || state.app == DesktopApp::Browser
            || state.app == DesktopApp::Store)
            && crate::svm::linux_ready()
        {
            let started = tsc();
            // While the browser or an install waits on the guest, give it
            // longer slices: it is the one with work to do.
            let waiting = crate::web::get().busy()
                || crate::store::get().install == crate::store::InstallState::Installing;
            crate::svm::linux_pump(if waiting {
                LINUX_SLICE_TSC * 50
            } else {
                LINUX_SLICE_TSC
            });
            // Repaint when the guest's screen changed, at most ~15 times a
            // second, so a busy console doesn't starve input handling.
            let now_ns = crate::time::monotonic_nanoseconds();
            let animating = loading_animating(&state);
            let interval = if animating { 33_000_000 } else { 66_000_000 };
            if now_ns.saturating_sub(linux_last_paint_ns) >= interval {
                // The guest's screen only matters when it is shown; otherwise (the
                // browser or the store is open and the guest just works in the
                // background) its changes must not trigger repaints, which would
                // starve it of CPU.
                let shown = state.app == DesktopApp::Linux || state.seamless.enabled;
                let mut signature = if shown {
                    linux_screen_signature(state.seamless.enabled)
                } else {
                    0
                };
                if animating {
                    signature ^= now_ns / 33_000_000;
                }
                if signature != linux_screen_hash {
                    linux_screen_hash = signature;
                    linux_last_paint_ns = now_ns;
                    present_desktop(frame, fonts, &state);
                    cursor.erase(frame);
                    cursor.paint(frame, cursor_target(frame));
                }
            }
            // A guest that used its whole slice is busy: go straight back
            // to it. One that idled in HLT gets to sleep until the next tick.
            if tsc().wrapping_sub(started) < LINUX_SLICE_TSC / 2 {
                arch::wait_for_interrupt();
            }
        } else {
            arch::wait_for_interrupt();
        }
    }
}

/// Where the guest's screen sits on the physical display right now.
fn linux_screen_rect(frame: &FrameBuffer, guest_width: usize, guest_height: usize) -> Rect {
    let layout = Layout::new(frame);
    let base = window_base_rect(DesktopApp::Linux);
    let screen = Rect::new(base.x + 10, base.y + 34, base.width - 20, base.height - 44);
    let full = layout.rect(screen);
    if linux_zoomed() {
        // 1:1: the view is as big as the window (or the guest screen).
        Rect::new(
            full.x,
            full.y,
            full.width.min(guest_width as i32),
            full.height.min(guest_height as i32),
        )
    } else {
        fit_rect(full, guest_width, guest_height)
    }
}

/// Zoomed view: pans so the pointer's position inside the window maps to the
/// same relative position on the guest screen (left edge = left of the guest
/// screen, right edge = right).
fn update_linux_pan(frame: &FrameBuffer, pointer: &crate::mouse::MouseState) {
    if !linux_zoomed() {
        return;
    }
    let Some((_, width, height, _)) = crate::svm::linux_framebuffer() else {
        return;
    };
    let rect = linux_screen_rect(frame, width, height);
    let layout = Layout::new(frame);
    let base = window_base_rect(DesktopApp::Linux);
    let area = layout.rect(Rect::new(
        base.x + 10,
        base.y + 34,
        base.width - 20,
        base.height - 44,
    ));
    let spare_x = (width as i32 - rect.width).max(0);
    let spare_y = (height as i32 - rect.height).max(0);
    let fx = ((pointer.x - area.x).clamp(0, area.width) as i64 * spare_x as i64
        / area.width.max(1) as i64) as i32;
    let fy = ((pointer.y - area.y).clamp(0, area.height) as i64 * spare_y as i64
        / area.height.max(1) as i64) as i32;
    LINUX_PAN_X.store(fx, core::sync::atomic::Ordering::Relaxed);
    LINUX_PAN_Y.store(fy, core::sync::atomic::Ordering::Relaxed);
}

/// Where the pointer we last fed the guest is believed to be, and whether it
/// was over guest content on the previous frame.
#[derive(Clone, Copy)]
struct GuestPointer {
    pos: (i32, i32),
    buttons: u8,
    epoch: u32,
    over: bool,
}

impl GuestPointer {
    const fn new() -> Self {
        Self {
            pos: (0, 0),
            buttons: 0,
            epoch: 0,
            over: false,
        }
    }
}

fn pointer_buttons(pointer: &crate::mouse::MouseState) -> u8 {
    pointer.left as u8 | ((pointer.right as u8) << 1) | ((pointer.middle as u8) << 2)
}

fn release_guest_buttons(guest: &mut GuestPointer) {
    if guest.over && guest.buttons != 0 {
        crate::svm::linux_mouse_feed(0, 0, 0);
        guest.buttons = 0;
    }
}

/// Feeds one pointer position (guest-screen pixels) and button state to the
/// guest's PS/2 mouse. The guest only understands relative motion and its
/// cursor's real position is unknowable from here (its driver may not have
/// been listening yet, X may re-centre it during start-up, packets can be
/// dropped), so a movement that starts after a pause - or on entry, or after
/// the guest re-enabled its mouse - first slams the guest cursor into the
/// top-left corner (it clamps there) and then moves it to the target: an
/// absolute placement built from relative motion. Within a continuous
/// movement plain deltas keep it in step.
fn send_guest_pointer(
    guest: &mut GuestPointer,
    target: (i32, i32),
    buttons: u8,
    screen: (i32, i32),
    host: (i32, i32),
) {
    if target == guest.pos && buttons == guest.buttons {
        return;
    }
    static LAST_EVENT_NS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let now = crate::time::monotonic_nanoseconds();
    let idle = now.saturating_sub(LAST_EVENT_NS.swap(now, core::sync::atomic::Ordering::Relaxed))
        > 400_000_000;
    let epoch = crate::svm::linux_mouse_epoch();
    let resync = !guest.over || idle || epoch != guest.epoch;
    guest.epoch = epoch;
    let held = guest.buttons;
    if resync {
        // Moves happen with the previous button state, the new state is
        // applied at the destination, so a click never lands in the corner.
        crate::svm::linux_mouse_feed(-screen.0 - 64, -screen.1 - 64, held);
        guest.pos = (0, 0);
    }
    let (dx, dy) = (target.0 - guest.pos.0, target.1 - guest.pos.1);
    static SENT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let sent = SENT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if sent.is_multiple_of(40) {
        serial::format(format_args!(
            "AEROS_LINUX_POINTER host=({},{}) guest=({},{}) buttons={} sent={}\n",
            host.0, host.1, target.0, target.1, buttons, sent
        ));
    }
    if dx != 0 || dy != 0 {
        crate::svm::linux_mouse_feed(dx, dy, held);
        guest.pos = target;
    }
    // A click shorter than the guest can notice (press and release inside one
    // input slice, e.g. a synthetic click) is stretched: the button stays down
    // for a minimum time so the pointer move that precedes it is processed
    // first and X sees a real press.
    static PRESSED_NS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    if buttons != held {
        if held == 0 {
            PRESSED_NS.store(now, core::sync::atomic::Ordering::Relaxed);
        } else if buttons == 0
            && now.saturating_sub(PRESSED_NS.load(core::sync::atomic::Ordering::Relaxed))
                < 80_000_000
        {
            return;
        }
        crate::svm::linux_mouse_feed(0, 0, buttons);
        guest.buttons = buttons;
    }
}

/// Turns the AerOS pointer into PS/2 mouse packets for the guest while it is
/// over the full-desktop Linux screen. Returns whether the pointer is over
/// the guest screen.
fn forward_pointer_to_linux(
    frame: &FrameBuffer,
    state: &DesktopState,
    pointer: &crate::mouse::MouseState,
    guest: &mut GuestPointer,
) -> bool {
    let active = state.app == DesktopApp::Linux
        && state.screen == Screen::Desktop
        && state.overlay == Overlay::None
        && crate::svm::linux_ready();
    let geometry = if active {
        crate::svm::linux_framebuffer()
    } else {
        None
    };
    let Some((_, width, height, _)) = geometry else {
        release_guest_buttons(guest);
        return false;
    };
    let rect = linux_screen_rect(frame, width, height);
    if !rect.contains(Point::new(pointer.x, pointer.y)) {
        release_guest_buttons(guest);
        return false;
    }
    let target = if linux_zoomed() {
        let (pan_x, pan_y) = linux_pan();
        (
            (pan_x + pointer.x - rect.x).clamp(0, width as i32 - 1),
            (pan_y + pointer.y - rect.y).clamp(0, height as i32 - 1),
        )
    } else {
        (
            (((pointer.x - rect.x) as i64 * width as i64 / rect.width as i64) as i32)
                .clamp(0, width as i32 - 1),
            (((pointer.y - rect.y) as i64 * height as i64 / rect.height as i64) as i32)
                .clamp(0, height as i32 - 1),
        )
    };
    send_guest_pointer(
        guest,
        target,
        pointer_buttons(pointer),
        (width as i32, height as i32),
        (pointer.x, pointer.y),
    );
    true
}

// ---- Seamless mode: each guest X window as its own AerOS window ----------

const SEAM_SLOTS: usize = 16;

#[derive(Clone, Copy)]
struct SeamSlot {
    id: u32,
    /// Top-left of the AerOS frame, in logical desktop coordinates.
    x: i32,
    y: i32,
}

/// Host-side bookkeeping for the guest's windows: where each AerOS frame
/// sits, the stacking order, which one has keyboard focus, and a drag in
/// progress. The windows' contents/geometry come from the guest agent's
/// table (`svm::linux_windows`).
#[derive(Clone, Copy)]
struct Seamless {
    enabled: bool,
    slots: [SeamSlot; SEAM_SLOTS],
    /// Window ids back to front; 0 = unused entry.
    order: [u32; SEAM_SLOTS],
    focus: u32,
    drag: u32,
    grab: (i32, i32),
    /// Window being resized from its bottom-right grip, the pointer and the
    /// window size when the drag began, and the last size sent to the guest.
    resize: u32,
    resize_from: (i32, i32, u32, u32),
    resize_sent: (u32, u32, u64),
}

impl Seamless {
    const fn new() -> Self {
        Self {
            enabled: false,
            slots: [SeamSlot { id: 0, x: 0, y: 0 }; SEAM_SLOTS],
            order: [0; SEAM_SLOTS],
            focus: 0,
            drag: 0,
            grab: (0, 0),
            resize: 0,
            resize_from: (0, 0, 0, 0),
            resize_sent: (0, 0, 0),
        }
    }

    fn slot(&self, id: u32) -> Option<SeamSlot> {
        self.slots
            .iter()
            .copied()
            .find(|slot| slot.id == id && id != 0)
    }

    fn remove_from_order(&mut self, id: u32) {
        let mut kept = 0;
        for index in 0..SEAM_SLOTS {
            if self.order[index] != id {
                self.order[kept] = self.order[index];
                kept += 1;
            }
        }
        for entry in &mut self.order[kept..] {
            *entry = 0;
        }
    }

    fn raise(&mut self, id: u32) {
        self.remove_from_order(id);
        if let Some(entry) = self.order.iter_mut().find(|entry| **entry == 0) {
            *entry = id;
        }
    }

    /// Reconciles the slots with the guest's current window list: new windows
    /// get a cascaded position and the keyboard, vanished ones are dropped.
    /// Returns whether anything changed.
    fn sync(&mut self, windows: &crate::svm::GuestWindows) -> bool {
        let live = &windows.windows[..windows.count];
        let mut changed = false;
        for index in 0..SEAM_SLOTS {
            let id = self.slots[index].id;
            if id != 0 && !live.iter().any(|window| window.id == id) {
                self.remove_from_order(id);
                if self.focus == id {
                    self.focus = 0;
                }
                if self.drag == id {
                    self.drag = 0;
                }
                if self.resize == id {
                    self.resize = 0;
                }
                self.slots[index].id = 0;
                changed = true;
            }
        }
        for window in live {
            if self.slot(window.id).is_some() {
                continue;
            }
            // Cascade new windows inside the visible desktop: the guest's own
            // layout can be far bigger than the AerOS display, so its
            // coordinates say nothing about where a window fits here.
            let used = self.slots.iter().filter(|slot| slot.id != 0).count() as i32;
            let step = used % 8;
            let origin = Point::new(10 + step * 34, 14 + step * 28);
            if let Some(free) = self.slots.iter_mut().find(|slot| slot.id == 0) {
                *free = SeamSlot {
                    id: window.id,
                    x: origin.x,
                    y: origin.y,
                };
                self.raise(window.id);
                self.focus = window.id;
                changed = true;
            }
        }
        changed
    }
}

struct SeamGeometry {
    chrome: Rect,
    content: Rect,
    close: Rect,
    grip: Rect,
}

/// Physical-pixel layout of one guest window's AerOS frame. The content area
/// is the guest window's own size, 1:1, so text stays sharp.
fn seam_geometry(layout: Layout, slot: SeamSlot, window: &crate::svm::GuestWindow) -> SeamGeometry {
    let margin = layout.scale.logical(6);
    let title_height = layout.scale.logical(30);
    let bottom = layout.scale.logical(12);
    let origin = layout.point(Point::new(slot.x, slot.y));
    let (width, height) = (window.width as i32, window.height as i32);
    let chrome = Rect::new(
        origin.x,
        origin.y,
        width + 2 * margin,
        height + title_height + bottom,
    );
    SeamGeometry {
        chrome,
        content: Rect::new(origin.x + margin, origin.y + title_height, width, height),
        close: Rect::new(
            chrome.right() - layout.scale.logical(26),
            origin.y + layout.scale.logical(8),
            layout.scale.logical(16),
            layout.scale.logical(16),
        ),
        grip: Rect::new(
            chrome.right() - layout.scale.logical(22),
            chrome.bottom() - bottom - layout.scale.logical(4),
            layout.scale.logical(22),
            bottom + layout.scale.logical(4),
        ),
    }
}

#[derive(Clone, Copy)]
enum SeamHit {
    Close(u32),
    Grip(u32),
    Title(u32),
    Content(u32),
}

/// The top-most guest window under `point` (physical pixels), and the part.
fn seam_hit(
    layout: Layout,
    seam: &Seamless,
    windows: &crate::svm::GuestWindows,
    point: Point,
) -> Option<SeamHit> {
    for &id in seam.order.iter().rev().filter(|id| **id != 0) {
        let (Some(slot), Some(window)) = (
            seam.slot(id),
            windows.windows[..windows.count]
                .iter()
                .find(|window| window.id == id),
        ) else {
            continue;
        };
        let geometry = seam_geometry(layout, slot, window);
        if geometry.close.contains(point) {
            return Some(SeamHit::Close(id));
        }
        if geometry.grip.contains(point) && !geometry.content.contains(point) {
            return Some(SeamHit::Grip(id));
        }
        if geometry.content.contains(point) {
            return Some(SeamHit::Content(id));
        }
        if geometry.chrome.contains(point) {
            return Some(SeamHit::Title(id));
        }
    }
    None
}

/// Whether keystrokes currently go to a Linux guest window.
fn guest_has_keyboard(state: &DesktopState) -> bool {
    state.screen == Screen::Desktop
        && crate::svm::linux_ready()
        && (state.app == DesktopApp::Linux || (state.seamless.enabled && state.seamless.focus != 0))
}

/// The Super+V clipboard history popup.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OskKey {
    Char(u8),
    Shift,
    Backspace,
    Symbols,
    Space,
    Enter,
    Hide,
}

const OSK_KEY_H: i32 = 30;
const OSK_GAP: i32 = 5;
const OSK_X: i32 = 16;
const OSK_W: i32 = 720;

fn osk_rect() -> Rect {
    let height = 4 * OSK_KEY_H + 3 * OSK_GAP + 16;
    Rect::new(OSK_X - 8, DESIGN_HEIGHT - height - 4, OSK_W + 16, height)
}

/// Every key of the on-screen keyboard with its rectangle (logical
/// coordinates), for drawing and for hit testing.
fn osk_keys(symbols: bool) -> ([(Rect, OskKey); 40], usize) {
    let mut keys = [(Rect::new(0, 0, 0, 0), OskKey::Space); 40];
    let mut count = 0;
    let top = osk_rect().y + 8;
    let unit = (OSK_W - 9 * OSK_GAP) / 10;
    let rows: [&[u8]; 3] = if symbols {
        [b"1234567890", b"-/:;()&@\"", b".,?!'#="]
    } else {
        [b"qwertyuiop", b"asdfghjkl", b"zxcvbnm"]
    };
    for (row, letters) in rows.iter().enumerate().take(2) {
        let y = top + row as i32 * (OSK_KEY_H + OSK_GAP);
        let width = letters.len() as i32 * unit + (letters.len() as i32 - 1) * OSK_GAP;
        let mut x = OSK_X + (OSK_W - width) / 2;
        for &letter in letters.iter() {
            keys[count] = (Rect::new(x, y, unit, OSK_KEY_H), OskKey::Char(letter));
            count += 1;
            x += unit + OSK_GAP;
        }
    }
    let y = top + 2 * (OSK_KEY_H + OSK_GAP);
    let wide = unit * 3 / 2;
    let letters = rows[2];
    let width = 2 * wide + letters.len() as i32 * unit + (letters.len() as i32 + 1) * OSK_GAP;
    let mut x = OSK_X + (OSK_W - width) / 2;
    keys[count] = (Rect::new(x, y, wide, OSK_KEY_H), OskKey::Shift);
    count += 1;
    x += wide + OSK_GAP;
    for &letter in letters.iter() {
        keys[count] = (Rect::new(x, y, unit, OSK_KEY_H), OskKey::Char(letter));
        count += 1;
        x += unit + OSK_GAP;
    }
    keys[count] = (Rect::new(x, y, wide, OSK_KEY_H), OskKey::Backspace);
    count += 1;
    let y = top + 3 * (OSK_KEY_H + OSK_GAP);
    let side = unit * 3 / 2;
    let enter = unit * 2;
    let space = OSK_W - side - unit - unit - enter - 4 * OSK_GAP;
    let mut x = OSK_X;
    keys[count] = (Rect::new(x, y, side, OSK_KEY_H), OskKey::Symbols);
    count += 1;
    x += side + OSK_GAP;
    keys[count] = (Rect::new(x, y, unit, OSK_KEY_H), OskKey::Hide);
    count += 1;
    x += unit + OSK_GAP;
    keys[count] = (Rect::new(x, y, space, OSK_KEY_H), OskKey::Space);
    count += 1;
    x += space + OSK_GAP;
    keys[count] = (Rect::new(x, y, unit, OSK_KEY_H), OskKey::Char(b'.'));
    count += 1;
    x += unit + OSK_GAP;
    keys[count] = (Rect::new(x, y, enter, OSK_KEY_H), OskKey::Enter);
    count += 1;
    (keys, count)
}

/// The on-screen keyboard (shown for finger input when text is wanted).
fn draw_osk(painter: &mut Painter<'_>, layout: Layout, ui_font: RasterFont, state: &DesktopState) {
    let panel = osk_rect();
    painter.fill_rounded_rect(
        layout.rect(panel),
        layout.radii(CornerRadii::all(18)),
        Rgba::new(22, 27, 33, 240),
    );
    let (keys, count) = osk_keys(state.osk_symbols);
    for (rect, key) in &keys[..count] {
        let fill = match key {
            OskKey::Enter => Rgba::new(64, 140, 205, 255),
            OskKey::Shift if state.osk_shift => Rgba::new(226, 232, 238, 255),
            OskKey::Char(_) | OskKey::Space => Rgba::new(74, 82, 92, 255),
            _ => Rgba::new(48, 55, 64, 255),
        };
        painter.fill_rounded_rect(layout.rect(*rect), layout.radii(CornerRadii::all(8)), fill);
        let mut single = [0u8; 1];
        let label = match key {
            OskKey::Char(byte) => {
                single[0] = if state.osk_shift && byte.is_ascii_lowercase() {
                    byte.to_ascii_uppercase()
                } else {
                    *byte
                };
                core::str::from_utf8(&single).unwrap_or("?")
            }
            OskKey::Shift => "Shift",
            OskKey::Backspace => "Del",
            OskKey::Symbols => {
                if state.osk_symbols {
                    "ABC"
                } else {
                    "?123"
                }
            }
            OskKey::Space => "Space",
            OskKey::Enter => "Enter",
            OskKey::Hide => "Hide",
        };
        let ink = if matches!(key, OskKey::Shift) && state.osk_shift {
            Color::rgb(20, 26, 32)
        } else {
            Color::rgb(240, 243, 246)
        };
        centered_text(painter, layout, ui_font, *rect, label, 14, ink);
    }
}

static LOADING_SINCE_NS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static LOADING_DONE_NS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The guest has shown a window at least once: the loading screen is over for good.
static LINUX_SEEN_UP: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Fade in / out of the loading screen.
const LOADING_FADE_NS: u64 = 400_000_000;

/// When the desktop runtime started (0 = not yet): the boot splash is timed
/// from here.
static BOOT_SPLASH_START_NS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// How long the boot splash is on screen; the last part fades to dark, and
/// the setup screen then fades in from dark.
const BOOT_SPLASH_NS: u64 = 3_200_000_000;
const BOOT_SPLASH_FADE_NS: u64 = 500_000_000;

/// The boot splash for this frame, or `None` once it is over: the view
/// (progress and timing) and how far it has faded to dark (0 = not at all).
fn boot_splash(now: u64) -> Option<(crate::loading::LoadingView, u8)> {
    let start = BOOT_SPLASH_START_NS.load(core::sync::atomic::Ordering::Relaxed);
    if start == 0 {
        return None;
    }
    let elapsed = now.saturating_sub(start);
    if elapsed >= BOOT_SPLASH_NS {
        return None;
    }
    let done = (elapsed * 1000 / BOOT_SPLASH_NS) as u32;
    let inverse = 1000 - done;
    let progress = (1000 - inverse * inverse / 1000) as u16;
    let view = crate::loading::LoadingView {
        kind: crate::loading::LoadingKind::System,
        stage: "Starting AerOS",
        progress_permille: progress,
        elapsed_secs: (elapsed / 1_000_000_000) as u32,
        elapsed_ms: elapsed / 1_000_000,
    };
    let fade_start = BOOT_SPLASH_NS - BOOT_SPLASH_FADE_NS;
    let dark = if elapsed > fade_start {
        ((elapsed - fade_start) * 255 / BOOT_SPLASH_FADE_NS) as u8
    } else {
        0
    };
    Some((view, dark))
}

/// The boot splash: the setup card's frosted glass over the wallpaper, a
/// rounded tile with the logo, a tip, and a pill progress bar.
fn draw_boot_splash(
    painter: &mut Painter<'_>,
    layout: Layout,
    ui_font: RasterFont,
    _mono_font: RasterFont,
    view: &crate::loading::LoadingView,
    screen: Rect,
    dark: u8,
) {
    flow::draw_boot_screen(
        painter,
        layout,
        ui_font,
        screen,
        view.elapsed_ms,
        Some(view.progress_permille),
        "",
        255,
    );
    draw_screen_fade(painter, screen, dark);
}

/// The loading screen for this frame: what to show and how opaque it is
/// (255 = fully covering the desktop). It fades in when seamless mode is
/// opened before the Linux desktop is up, and fades out (with the bar
/// filled) once the guest's windows appear.
fn loading_overlay(state: &DesktopState, now: u64) -> Option<(crate::loading::LoadingView, u8)> {
    use core::sync::atomic::Ordering::Relaxed;
    if !state.seamless.enabled || state.screen != Screen::Desktop {
        LOADING_SINCE_NS.store(0, Relaxed);
        LOADING_DONE_NS.store(0, Relaxed);
        return None;
    }
    let mut since = LOADING_SINCE_NS.load(Relaxed);
    // "Up" means the first window exists (the agent's table alone appears a
    // few seconds earlier, with nothing in it yet).
    let up = crate::svm::linux_windows().is_some_and(|windows| windows.count > 0);
    if up {
        LINUX_SEEN_UP.store(true, Relaxed);
    }
    if up || LINUX_SEEN_UP.load(Relaxed) {
        // Only fade out if a loading screen was actually showing.
        if since == 0 {
            return None;
        }
        let mut done = LOADING_DONE_NS.load(Relaxed);
        if done == 0 {
            done = now;
            LOADING_DONE_NS.store(now, Relaxed);
        }
        let t = now.saturating_sub(done);
        if t >= LOADING_FADE_NS {
            LOADING_SINCE_NS.store(0, Relaxed);
            LOADING_DONE_NS.store(0, Relaxed);
            return None;
        }
        let view = crate::loading::LoadingView {
            kind: crate::loading::LoadingKind::Linux,
            stage: "Ready",
            progress_permille: 1000,
            elapsed_secs: 0,
            elapsed_ms: now.saturating_sub(since) / 1_000_000,
        };
        let alpha = 255 - (t * 255 / LOADING_FADE_NS) as u32;
        return Some((view, alpha as u8));
    }
    LOADING_DONE_NS.store(0, Relaxed);
    if since == 0 {
        since = now;
        LOADING_SINCE_NS.store(now, Relaxed);
    }
    let view = crate::loading::linux_view(since, now)?;
    let fade_in = (now.saturating_sub(since) * 255 / LOADING_FADE_NS).min(255) as u8;
    Some((view, fade_in))
}

/// Whether the loading screen is on screen (or fading), so the desktop keeps
/// repainting it for its animation.
fn loading_animating(state: &DesktopState) -> bool {
    state.seamless.enabled
        && (LOADING_SINCE_NS.load(core::sync::atomic::Ordering::Relaxed) != 0
            || !LINUX_SEEN_UP.load(core::sync::atomic::Ordering::Relaxed))
}

/// Scales a colour towards black (the loading screen is black, so this is
/// also its fade).
fn dim(color: Color, alpha: u8) -> Color {
    let scale = |channel: u8| (channel as u32 * alpha as u32 / 255) as u8;
    Color::rgb(scale(color.red), scale(color.green), scale(color.blue))
}

/// The loading screen, in the style of a macOS startup: solid black, the
/// logo centred, a slim progress bar under it and a dim tip line. The bar
/// follows the progress smoothly, a highlight glides along its fill, and tips
/// cross-fade. `alpha` fades the whole screen in or out.
fn draw_loading(
    painter: &mut Painter<'_>,
    layout: Layout,
    ui_font: RasterFont,
    _mono_font: RasterFont,
    screen: Rect,
    view: &crate::loading::LoadingView,
    alpha: u8,
) {
    flow::draw_boot_screen(
        painter,
        layout,
        ui_font,
        screen,
        view.elapsed_ms,
        Some(view.progress_permille),
        "",
        alpha,
    );
    if view.is_slow() {
        let note = "Taking longer than usual...";
        let height = layout.scale.logical(12);
        let width = ui_font.text_width(note, height);
        painter.text(
            ui_font,
            Point::new(
                screen.x + (screen.width - width) / 2,
                screen.y + screen.height * 2 / 3 + layout.scale.logical(60),
            ),
            note,
            height,
            dim(Color::rgb(120, 120, 126), alpha),
        );
    }
}

fn draw_seamless(
    painter: &mut Painter<'_>,
    layout: Layout,
    ui_font: RasterFont,
    state: &DesktopState,
) {
    let seam = &state.seamless;
    let Some(windows) = crate::svm::linux_windows() else {
        return; // the loading overlay covers this (see `loading_overlay`)
    };
    let framebuffer = crate::svm::linux_framebuffer();
    for &id in seam.order.iter().filter(|id| **id != 0) {
        let (Some(slot), Some(window)) = (
            seam.slot(id),
            windows.windows[..windows.count]
                .iter()
                .find(|window| window.id == id),
        ) else {
            continue;
        };
        let geometry = seam_geometry(layout, slot, window);
        let radii = layout.radii(CornerRadii::all(12));
        painter.fill_rounded_rect(geometry.chrome, radii, Rgba::new(244, 246, 248, 248));
        let border = if id == seam.focus {
            Rgba::new(64, 140, 205, 255)
        } else {
            Rgba::new(188, 197, 206, 230)
        };
        painter.stroke_rounded_rect(geometry.chrome, radii, 2, border);
        let length = window.title.iter().position(|b| *b == 0).unwrap_or(64);
        let title = core::str::from_utf8(&window.title[..length]).unwrap_or("");
        let fit = (geometry.chrome.width / layout.scale.logical(8).max(1)).max(4) as usize;
        let title = title
            .get(..title.len().min(fit.saturating_sub(4)))
            .unwrap_or(title);
        text(
            painter,
            layout,
            ui_font,
            Point::new(slot.x + 14, slot.y + 8),
            title,
            13,
            Color::rgb(18, 23, 26),
        );
        painter.fill_rounded_rect(
            geometry.close,
            layout.radii(CornerRadii::all(7)),
            Rgba::opaque(198, 31, 18),
        );
        // Resize grip: two short diagonal ticks in the bottom-right corner.
        for step in [4, 9] {
            painter.fill_rounded_rect(
                Rect::new(
                    geometry.chrome.right() - layout.scale.logical(step + 4),
                    geometry.chrome.bottom() - layout.scale.logical(4),
                    layout.scale.logical(step),
                    layout.scale.logical(2).max(1),
                ),
                layout.radii(CornerRadii::all(0)),
                Rgba::new(120, 132, 144, 220),
            );
        }
        painter.fill_rounded_rect(
            geometry.content,
            layout.radii(CornerRadii::all(0)),
            Rgba::opaque(0, 0, 0),
        );
        if let Some((pixels, screen_width, screen_height, stride)) = framebuffer {
            let crop_x = window.x.clamp(0, screen_width as i32);
            let crop_y = window.y.clamp(0, screen_height as i32);
            let crop_w = (window.width as i32)
                .min(screen_width as i32 - crop_x)
                .max(0);
            let crop_h = (window.height as i32)
                .min(screen_height as i32 - crop_y)
                .max(0);
            if crop_w > 0 && crop_h > 0 {
                let dest = Rect::new(geometry.content.x, geometry.content.y, crop_w, crop_h);
                // SAFETY: the crop is clamped to the guest screen, whose
                // pixels the hypervisor guarantees for FB_PITCH * FB_HEIGHT.
                unsafe {
                    painter.blit_scaled(
                        dest,
                        pixels.add(crop_y as usize * stride + crop_x as usize),
                        crop_w as usize,
                        crop_h as usize,
                        stride,
                    );
                }
            }
        }
    }
    if seam.focus == 0 {
        text(
            painter,
            layout,
            ui_font,
            Point::new(28, 4),
            "Linux apps    X terminal    N NetSurf    F Firefox    K back    Super+V clipboard    click a window to type",
            11,
            Color::rgb(240, 244, 248),
        );
    }
}

struct SeamOutcome {
    /// The press belonged to a guest window: don't also treat it as a click
    /// on whatever AerOS element is underneath.
    consumed: bool,
    /// The pointer is over guest window content (the host cursor steps aside).
    over: bool,
    redraw: bool,
}

/// Pointer handling for seamless mode: focus, raise, drag by the frame,
/// close, and forwarding to the guest while over a window's content.
fn seamless_pointer(
    frame: &FrameBuffer,
    state: &mut DesktopState,
    pointer: &crate::mouse::MouseState,
    press_edge: bool,
    guest: &mut GuestPointer,
) -> SeamOutcome {
    let mut outcome = SeamOutcome {
        consumed: false,
        over: false,
        redraw: false,
    };
    let active = state.screen == Screen::Desktop
        && state.overlay == Overlay::None
        && crate::svm::linux_ready();
    let windows = if active {
        crate::svm::linux_windows()
    } else {
        None
    };
    let Some(windows) = windows else {
        release_guest_buttons(guest);
        return outcome;
    };
    let layout = Layout::new(frame);
    let point = Point::new(pointer.x, pointer.y);
    if state.seamless.drag != 0 {
        if pointer.left {
            let origin = layout.to_logical(Point::new(
                point.x - state.seamless.grab.0,
                point.y - state.seamless.grab.1,
            ));
            let id = state.seamless.drag;
            if let Some(slot) = state.seamless.slots.iter_mut().find(|slot| slot.id == id) {
                slot.x = origin.x;
                slot.y = origin.y.max(0);
            }
            outcome.consumed = true;
            outcome.redraw = true;
            return outcome;
        }
        state.seamless.drag = 0;
    }
    if state.seamless.resize != 0 {
        let id = state.seamless.resize;
        if let Some(window) = windows.windows[..windows.count]
            .iter()
            .find(|window| window.id == id)
        {
            let (start_x, start_y, start_w, start_h) = state.seamless.resize_from;
            // Physical pixels on the host are 1:1 with guest pixels here.
            let max_w = (windows.screen_width as i32 - window.x).max(120);
            let max_h = (windows.screen_height as i32 - window.y).max(80);
            let width = (start_w as i32 + point.x - start_x).clamp(120, max_w) as u32;
            let height = (start_h as i32 + point.y - start_y).clamp(80, max_h) as u32;
            let (sent_w, sent_h, sent_at) = state.seamless.resize_sent;
            let now = tsc();
            let released = !pointer.left;
            // Throttled while dragging (one command slot), exact on release.
            if (width, height) != (sent_w, sent_h)
                && (released || now.saturating_sub(sent_at) > 150_000_000)
            {
                crate::svm::linux_command(5, [id, width, height, 0], b"");
                state.seamless.resize_sent = (width, height, now);
            }
        }
        if !pointer.left {
            state.seamless.resize = 0;
        }
        outcome.consumed = true;
        outcome.redraw = true;
        return outcome;
    }
    let hit = seam_hit(layout, &state.seamless, &windows, point);
    if press_edge {
        match hit {
            Some(SeamHit::Grip(id)) => {
                state.seamless.raise(id);
                state.seamless.focus = id;
                crate::svm::linux_command(2, [id, 0, 0, 0], b"");
                if let Some(window) = windows.windows[..windows.count]
                    .iter()
                    .find(|window| window.id == id)
                {
                    state.seamless.resize = id;
                    state.seamless.resize_from = (point.x, point.y, window.width, window.height);
                    state.seamless.resize_sent = (window.width, window.height, tsc());
                }
                outcome.consumed = true;
                outcome.redraw = true;
            }
            Some(SeamHit::Close(id)) => {
                crate::svm::linux_command(1, [id, 0, 0, 0], b"");
                outcome.consumed = true;
            }
            Some(SeamHit::Title(id)) => {
                state.seamless.raise(id);
                state.seamless.focus = id;
                crate::svm::linux_command(2, [id, 0, 0, 0], b"");
                if let (Some(slot), Some(window)) = (
                    state.seamless.slot(id),
                    windows.windows[..windows.count]
                        .iter()
                        .find(|window| window.id == id),
                ) {
                    let geometry = seam_geometry(layout, slot, window);
                    state.seamless.drag = id;
                    state.seamless.grab =
                        (point.x - geometry.chrome.x, point.y - geometry.chrome.y);
                }
                outcome.consumed = true;
                outcome.redraw = true;
            }
            Some(SeamHit::Content(id)) => {
                state.seamless.raise(id);
                state.seamless.focus = id;
                outcome.consumed = true;
                outcome.redraw = true;
            }
            None => {
                if state.seamless.focus != 0 {
                    state.seamless.focus = 0;
                    outcome.redraw = true;
                }
            }
        }
    }
    if let Some(SeamHit::Content(id)) = hit
        && let (Some(slot), Some(window)) = (
            state.seamless.slot(id),
            windows.windows[..windows.count]
                .iter()
                .find(|window| window.id == id),
        )
    {
        let geometry = seam_geometry(layout, slot, window);
        let (screen_w, screen_h) = (windows.screen_width as i32, windows.screen_height as i32);
        let target = (
            (window.x + point.x - geometry.content.x).clamp(0, screen_w - 1),
            (window.y + point.y - geometry.content.y).clamp(0, screen_h - 1),
        );
        send_guest_pointer(
            guest,
            target,
            pointer_buttons(pointer),
            (screen_w, screen_h),
            (point.x, point.y),
        );
        outcome.over = true;
    } else {
        release_guest_buttons(guest);
    }
    outcome
}

fn tsc() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// Cheap change detector for the guest framebuffer: a sparse sample of its
/// pixels, so an idle console costs almost nothing to check. In seamless mode
/// only the pixels inside the guest windows are sampled - changes anywhere
/// else on the (large) guest screen are invisible on the host anyway and must
/// not trigger repaints.
fn linux_screen_signature(seamless: bool) -> u64 {
    let Some((pixels, width, height, stride)) = crate::svm::linux_framebuffer() else {
        return 0;
    };
    let windows = crate::svm::linux_windows();
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut sample = |x0: usize, y0: usize, x1: usize, y1: usize| {
        let mut y = y0;
        while y < y1.min(height) {
            let mut x = x0;
            while x < x1.min(width) {
                let pixel = unsafe { core::ptr::read_volatile(pixels.add(y * stride + x)) };
                hash = (hash ^ pixel as u64).wrapping_mul(0x0000_0100_0000_01b3);
                x += 3;
            }
            y += 2;
        }
    };
    match (&windows, seamless) {
        (Some(windows), true) => {
            for window in &windows.windows[..windows.count] {
                let x0 = window.x.max(0) as usize;
                let y0 = window.y.max(0) as usize;
                sample(
                    x0,
                    y0,
                    x0.saturating_add(window.width as usize),
                    y0.saturating_add(window.height as usize),
                );
            }
        }
        _ => sample(0, 0, width, height),
    }
    if let Some(windows) = windows {
        hash = (hash ^ windows.generation as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn present_desktop(frame: &mut FrameBuffer, fonts: &FontCatalog, state: &DesktopState) -> bool {
    present_desktop_mode(frame, fonts, state, false)
}

fn present_desktop_mode(
    frame: &mut FrameBuffer,
    fonts: &FontCatalog,
    state: &DesktopState,
    full: bool,
) -> bool {
    let info = frame.info();
    let required = info.stride.saturating_mul(info.height);
    if required > MAX_DESKTOP_PIXELS {
        return draw_desktop(frame, fonts, state);
    }
    let address = unsafe { (*DESKTOP_BUFFER.0.get()).as_mut_ptr() };
    let staging_info = FrameBufferInfo {
        address,
        size: required.saturating_mul(core::mem::size_of::<u32>()),
        width: info.width,
        height: info.height,
        stride: info.stride,
        format: info.format,
    };
    let mut staging = unsafe { FrameBuffer::new(staging_info) };
    if !draw_desktop(&mut staging, fonts, state) {
        return false;
    }
    let shadow = &mut shadow_slice()[..required];
    if full {
        frame.present_full(&staging, shadow)
    } else {
        frame.present_diff(&staging, shadow)
    }
}

fn draw_desktop(frame: &mut FrameBuffer, fonts: &FontCatalog, state: &DesktopState) -> bool {
    let now = crate::time::monotonic_nanoseconds();
    let acting = state.power_action != shellui::PowerAction::None;
    let covered = acting
        && now.saturating_sub(state.power_action_at_ns) >= 560_000_000
        && state.power_action != shellui::PowerAction::Sleep;
    let asleep = acting
        && state.power_action == shellui::PowerAction::Sleep
        && now.saturating_sub(state.power_action_at_ns) >= 560_000_000;
    let mut ok = true;
    if !covered && !asleep {
        ok = draw_desktop_base(frame, fonts, state);
    }
    let Some(ui_font) = fonts.ui() else {
        return ok;
    };
    let layout = Layout::new(frame);
    let screen_bounds = Rect::new(0, 0, frame.width() as i32, frame.height() as i32);
    let mut painter = Painter::new(frame);
    if state.screen == Screen::Desktop && state.power_visible(now) && !acting {
        ok &= shellui::draw_power_menu(&mut painter, layout, ui_font, screen_bounds, state, now);
    }
    if state.screen == Screen::Desktop && !acting {
        search::draw_dock_preview(&mut painter, layout, state, now);
        search::draw_search(&mut painter, layout, ui_font, state, now);
        panels::draw_toast(&mut painter, layout, ui_font, state, now);
        panels::draw_notify_stack(&mut painter, layout, ui_font, state, now);
        panels::draw_context(&mut painter, layout, ui_font, state, now);
    }
    if acting {
        shellui::draw_power_action(&mut painter, layout, ui_font, screen_bounds, state, now);
    }
    ok
}

fn draw_desktop_base(frame: &mut FrameBuffer, fonts: &FontCatalog, state: &DesktopState) -> bool {
    let now = crate::time::monotonic_nanoseconds();
    let moving = now < state.motion_until_ns;
    button::set_paint_step(if moving { 4 } else { 2 });
    MOTION_CHEAP.store(moving, Ordering::Relaxed);
    let layout = Layout::new(frame);
    let scrolling = state.screen != Screen::Desktop
        && state.previous_screen == Screen::Lock
        && state.screen == Screen::Login
        && now.saturating_sub(state.screen_transition_at_ns) < LOCK_LOGIN_SCROLL_NS;
    let opening = state.screen == Screen::Desktop
        && state.app != DesktopApp::None
        && now.saturating_sub(state.window_open_at_ns) < WINDOW_OPEN_NS;
    let window_closing = state.screen == Screen::Desktop
        && state.app_closing != DesktopApp::None
        && state.app != state.app_closing
        && now.saturating_sub(state.app_close_at_ns) < WINDOW_OPEN_NS;
    let sliding_overlay = if state.screen != Screen::Desktop {
        None
    } else if matches!(state.overlay, Overlay::Apps | Overlay::Quick)
        && now.saturating_sub(state.overlay_open_at_ns) < OVERLAY_TRANSITION_NS
    {
        Some(state.overlay)
    } else if matches!(state.overlay_closing, Overlay::Apps | Overlay::Quick)
        && state.overlay != state.overlay_closing
        && now.saturating_sub(state.overlay_close_at_ns) < OVERLAY_TRANSITION_NS
    {
        Some(state.overlay_closing)
    } else {
        None
    };
    // Clip regions passed to draw_wallpaper are in PHYSICAL framebuffer
    // pixels, so every logical rect here must go through layout.rect()
    // before use — skipping that undersizes the clip whenever scale != 1x
    // and leaves stale (un-erased) previous-frame pixels behind, which
    // reads as ghosting/duplication while the element is moving.
    let wallpaper_region = if scrolling {
        let card = layout.rect(LOGIN_CARD);
        Some(Rect::new(card.x, 0, card.width, frame.height() as i32))
    } else if opening {
        Some(layout.rect(window_base_rect(state.app)))
    } else if window_closing {
        Some(layout.rect(window_base_rect(state.app_closing)))
    } else if let Some(overlay) = sliding_overlay {
        let physical = layout.rect(overlay_panel_rect(overlay));
        let extra = layout.scale.logical(OVERLAY_SLIDE_PX);
        Some(Rect::new(
            physical.x,
            physical.y,
            physical.width,
            physical.height + extra,
        ))
    } else {
        None
    };
    let wallpaper_region = if state.power_visible(now)
        || state.power_action != shellui::PowerAction::None
        || state.top_layers_active(now)
    {
        None
    } else {
        wallpaper_region
    };
    if let Some((view, dark)) = boot_splash(now)
        && let (Some(ui_font), Some(mono_font)) = (fonts.ui(), fonts.mono())
    {
        draw_wallpaper(frame, None);
        let wallpaper = wallpaper_valid();
        let screen_bounds = Rect::new(0, 0, frame.width() as i32, frame.height() as i32);
        let mut painter = Painter::new(frame);
        draw_boot_splash(
            &mut painter,
            layout,
            ui_font,
            mono_font,
            &view,
            screen_bounds,
            dark,
        );
        return wallpaper;
    }
    // A fully opaque loading screen hides the whole desktop, so draw only it:
    // that is cheap enough to animate at 30 fps without starving the guest of
    // the CPU time it needs to finish starting.
    if let Some((view, 255)) = loading_overlay(state, now)
        && let (Some(ui_font), Some(mono_font)) = (fonts.ui(), fonts.mono())
    {
        let screen_bounds = Rect::new(0, 0, frame.width() as i32, frame.height() as i32);
        let mut painter = Painter::new(frame);
        draw_loading(
            &mut painter,
            layout,
            ui_font,
            mono_font,
            screen_bounds,
            &view,
            255,
        );
        return true;
    }
    draw_wallpaper(frame, wallpaper_region);
    let wallpaper = wallpaper_valid();
    let Some(ui_font) = fonts.ui() else {
        return false;
    };
    let Some(mono_font) = fonts.mono() else {
        return false;
    };
    let screen_bounds = Rect::new(0, 0, frame.width() as i32, frame.height() as i32);
    let fade_alpha = screen_fade_alpha(state.screen_transition_at_ns, now);
    let mut painter = Painter::new(frame);
    if state.screen != Screen::Desktop {
        let session = if scrolling {
            let elapsed = now.saturating_sub(state.screen_transition_at_ns);
            let progress_milli = ((elapsed * 1000) / LOCK_LOGIN_SCROLL_NS) as u32;
            let eased = ease_out_milli(progress_milli) as i32;
            let scroll = LOGIN_CARD.height * eased / 1000;
            let lock_ok = draw_lock(
                &mut painter,
                shift_layout_y(layout, -scroll),
                ui_font,
                state,
                now,
                true,
            );
            let login_ok = draw_signin(
                &mut painter,
                shift_layout_y(layout, LOGIN_CARD.height - scroll),
                ui_font,
                state,
                now,
                true,
            );
            lock_ok && login_ok
        } else {
            match state.screen {
                Screen::Lock => draw_lock(&mut painter, layout, ui_font, state, now, false),
                Screen::Login if state.lockout.seconds_left(now) > 0 => {
                    shellui::draw_lockout(
                        &mut painter,
                        layout,
                        ui_font,
                        screen_bounds,
                        state.lockout.seconds_left(now),
                        now,
                        state.login_error_until_ns.saturating_sub(LOGIN_ERROR_NS),
                    );
                    true
                }
                Screen::Login => {
                    let raised = if state.osk_open {
                        shift_layout_y(layout, -70)
                    } else {
                        layout
                    };
                    draw_signin(&mut painter, raised, ui_font, state, now, false)
                }
                _ => draw_setup(&mut painter, layout, ui_font, state),
            }
        };
        if !scrolling {
            draw_screen_fade(&mut painter, screen_bounds, fade_alpha);
        }
        draw_music_hub(&mut painter, layout, ui_font, state, now);
        panels::draw_hud(&mut painter, layout, state, now);
        if state.osk_open {
            draw_osk(&mut painter, layout, ui_font, state);
        }
        return wallpaper && session;
    }
    // A settled app-switcher frame (many frosted buttons) never changes until
    // the focus moves or the minute ticks, so the whole frame is replayed.
    let panel_static = state.overlay == Overlay::Apps
        && !state.power_visible(now)
        && !state.top_layers_active(now)
        && !moving
        && sliding_overlay.is_none()
        && state.app == DesktopApp::None
        && !window_closing
        && !state.seamless.enabled
        && !state.clip_open
        && !state.osk_open
        && !state.music_visible(now)
        && now >= state.volume_hud_until_ns + VOLUME_SLIDE_NS
        && now >= state.shield_until_ns
        && fade_alpha == 0
        && loading_overlay(state, now).is_none()
        && now.saturating_sub(state.overlay_open_at_ns) > OVERLAY_TRANSITION_NS + 100_000_000;
    let screen_len = (frame_pixels(screen_bounds)).min(PANEL_CACHE_PIXELS);
    let panel_key = dock_cache_key(layout, screen_bounds)
        ^ ((state.app_focus as u64 + 1) << 8)
        ^ ((state.focus_visible as u64) << 20)
        ^ 0x9a7e_0000;
    let panel_cache = unsafe { &mut *PANEL_CACHE.0.get() };
    if panel_static
        && screen_len == frame_pixels(screen_bounds)
        && PANEL_CACHE_KEY.load(Ordering::Acquire) == panel_key
        && painter.write_region(screen_bounds, &panel_cache[..screen_len])
    {
        return wallpaper;
    }
    if state.seamless.enabled {
        draw_seamless(&mut painter, layout, ui_font, state);
    }
    if state.app != DesktopApp::None {
        let _ = draw_window(&mut painter, layout, ui_font, mono_font, state, now, false);
    } else if window_closing {
        let mut closing_state = *state;
        closing_state.app = state.app_closing;
        let _ = draw_window(
            &mut painter,
            layout,
            ui_font,
            mono_font,
            &closing_state,
            now,
            true,
        );
    }
    // Background and content share one shifted layout (glass + icons move
    // as one rigid unit — see overlay_slide_layout), so nothing appears to
    // detach from the panel while it's moving.
    let apps_active = state.overlay == Overlay::Apps || sliding_overlay == Some(Overlay::Apps);
    if apps_active {
        let opening = state.overlay == Overlay::Apps;
        let at_ns = if opening {
            state.overlay_open_at_ns
        } else {
            state.overlay_close_at_ns
        };
        let panel_layout = overlay_slide_layout(layout, opening, at_ns, now);
        // While sliding, nothing may show below the dock's bottom edge: the
        // panel rises out of (and sinks back into) the dock, which is drawn
        // in front of it, instead of coming up from the screen edge.
        let sliding = sliding_overlay == Some(Overlay::Apps);
        if sliding {
            // The visible part ends at the dock's top edge (logical y 369)
            // and the limit eases down to the dock's bottom (440, where the
            // settled panel ends) as it settles, so lifting the clip at the
            // end changes nothing on screen.
            let reveal = overlay_reveal_milli(opening, at_ns, now).clamp(0, 1000);
            let limit = 369 + (440 - 369) * reveal / 1000;
            let clip_bottom = layout.point(Point::new(0, limit)).y;
            painter.set_clip(Rect::new(0, 0, screen_bounds.width, clip_bottom));
        }
        let _ = draw_app_switcher(
            &mut painter,
            panel_layout,
            ui_font,
            state,
            now,
            opening,
            at_ns,
            sliding,
        );
        if sliding {
            painter.reset_clip();
        }
    }
    if state.overlay == Overlay::Quick || sliding_overlay == Some(Overlay::Quick) {
        let opening = state.overlay == Overlay::Quick;
        let at_ns = if opening {
            state.overlay_open_at_ns
        } else {
            state.overlay_close_at_ns
        };
        let panel_layout = overlay_slide_layout(layout, opening, at_ns, now);
        let _ = panels::draw_quick(&mut painter, panel_layout, ui_font, state, now);
    }
    if state.clip_open {
        panels::draw_clipboard_panel(&mut painter, layout, ui_font, state, now);
    }
    let outer_dock = !apps_active;
    let dock_cacheable = outer_dock
        && state.overlay == Overlay::None
        && sliding_overlay.is_none()
        && !matches!(
            state.app,
            DesktopApp::Browser | DesktopApp::Store | DesktopApp::Linux
        )
        && !window_closing
        && !opening
        && !state.seamless.enabled
        && !state.clip_open
        && dock_is_settled(state, now);
    let dock_region = {
        let full = layout.rect(Rect::new(8, 350, 736, 108));
        full.intersect(screen_bounds).unwrap_or(full)
    };
    let dock_key = dock_cache_key(layout, dock_region)
        ^ ((state.app as u64 + 1) << 8)
        ^ ((state.focus_visible as u64) << 20)
        ^ ((state.dock_focus as u64) << 24)
        ^ ((is_maximized(state.app) as u64) << 30)
        ^ ((state.hover_index.wrapping_add(1) as u64) << 34);
    let cache = unsafe { &mut *DOCK_CACHE.0.get() };
    let cache_len = (dock_region.width.max(0) as usize) * (dock_region.height.max(0) as usize);
    let (dock, controls) = if dock_cacheable
        && cache_len <= cache.len()
        && DOCK_CACHE_KEY.load(Ordering::Acquire) == dock_key
        && painter.write_region(dock_region, &cache[..cache_len])
    {
        (true, true)
    } else {
        let result = draw_dock(&mut painter, layout, ui_font, state, outer_dock, now);
        if dock_cacheable
            && !moving
            && cache_len <= cache.len()
            && painter.read_region(dock_region, &mut cache[..cache_len])
        {
            DOCK_CACHE_KEY.store(dock_key, Ordering::Release);
        }
        result
    };
    draw_music_hub(&mut painter, layout, ui_font, state, now);
    panels::draw_hud(&mut painter, layout, state, now);
    // The keyboard sits over everything, dock included.
    if state.osk_open {
        draw_osk(&mut painter, layout, ui_font, state);
    }
    if let Some((view, alpha)) = loading_overlay(state, now) {
        draw_loading(
            &mut painter,
            layout,
            ui_font,
            mono_font,
            screen_bounds,
            &view,
            alpha,
        );
    }
    if panel_static
        && screen_len == frame_pixels(screen_bounds)
        && painter.read_region(screen_bounds, &mut panel_cache[..screen_len])
    {
        PANEL_CACHE_KEY.store(panel_key, Ordering::Release);
    }
    draw_screen_fade(&mut painter, screen_bounds, fade_alpha);
    wallpaper && dock && controls
}

fn screen_fade_alpha(transition_at_ns: u64, now_ns: u64) -> u8 {
    if transition_at_ns == 0 {
        return 0;
    }
    let elapsed = now_ns.saturating_sub(transition_at_ns);
    if elapsed >= SCREEN_TRANSITION_NS {
        return 0;
    }
    let progress_milli = ((elapsed * 1000) / SCREEN_TRANSITION_NS) as u32;
    let eased = ease_out_milli(progress_milli);
    (SCREEN_FADE_MAX_ALPHA.saturating_sub(SCREEN_FADE_MAX_ALPHA * eased / 1000)) as u8
}

fn draw_screen_fade(painter: &mut Painter<'_>, bounds: Rect, alpha: u8) {
    if alpha == 0 {
        return;
    }
    painter.fill_rounded_rect(bounds, CornerRadii::all(0), Rgba::new(6, 10, 16, alpha));
}

fn shift_layout_y(layout: Layout, logical_dy: i32) -> Layout {
    let mut shifted = layout;
    shifted.offset.y = shifted
        .offset
        .y
        .saturating_add(layout.scale.logical(logical_dy));
    shifted
}

fn overlay_panel_rect(overlay: Overlay) -> Rect {
    if overlay == Overlay::Apps {
        Rect::new(25, 20, 702, 420)
    } else {
        panels::QUICK_PANEL
    }
}

/// 0 = fully hidden (before opening / end of closing), 1000 = fully shown
/// (settled open / start of closing), eased either direction.
fn overlay_reveal_milli(opening: bool, at_ns: u64, now_ns: u64) -> i32 {
    let elapsed = now_ns.saturating_sub(at_ns);
    let progress_milli = if elapsed >= OVERLAY_TRANSITION_NS {
        1000
    } else {
        ((elapsed * 1000) / OVERLAY_TRANSITION_NS) as u32
    };
    let eased = ease_out_milli(progress_milli) as i32;
    if opening { eased } else { 1000 - eased }
}

/// Translates the whole panel (background + everything drawn on it) as one
/// rigid unit, starting from behind the dock and rising to its resting
/// position — the panel is tall enough that its starting position is
/// mostly off-canvas, which is fine, it just means most of it is clipped
/// away by the screen bounds until it slides into view.
fn overlay_slide_layout(layout: Layout, opening: bool, at_ns: u64, now_ns: u64) -> Layout {
    let reveal_milli = overlay_reveal_milli(opening, at_ns, now_ns);
    let slide_logical = OVERLAY_SLIDE_PX * (1000 - reveal_milli) / 1000;
    let mut shifted = layout;
    shifted.offset.y = shifted
        .offset
        .y
        .saturating_add(layout.scale.logical(slide_logical));
    shifted
}

fn app_icon_fall_offset(panel_opened_at_ns: u64, index: usize, now_ns: u64) -> i32 {
    let start = panel_opened_at_ns.saturating_add(index as u64 * APP_ICON_FALL_STAGGER_NS);
    if now_ns < start {
        return APP_ICON_FALL_PX;
    }
    let elapsed = now_ns.saturating_sub(start);
    let progress_milli = if elapsed >= APP_ICON_FALL_NS {
        return 0;
    } else {
        ((elapsed * 1000) / APP_ICON_FALL_NS) as u32
    };
    let eased = ease_out_milli(progress_milli) as i32;
    APP_ICON_FALL_PX * (1000 - eased) / 1000
}

fn window_open_scale(base: Rect, opened_at_ns: u64, now_ns: u64) -> Rect {
    if opened_at_ns == 0 {
        return base;
    }
    let elapsed = now_ns.saturating_sub(opened_at_ns);
    let progress_milli = if elapsed >= WINDOW_OPEN_NS {
        1000
    } else {
        ((elapsed * 1000) / WINDOW_OPEN_NS) as u32
    };
    let eased = ease_out_milli(progress_milli) as i32;
    let scale_milli = 920 + 80 * eased / 1000;
    let width = base.width * scale_milli / 1000;
    let height = base.height * scale_milli / 1000;
    let center_x = base.x + base.width / 2;
    let center_y = base.y + base.height / 2;
    Rect::new(center_x - width / 2, center_y - height / 2, width, height)
}

/// The mirror image of `window_open_scale`: shrinks from full size down to
/// the same 920/1000 floor the open animation grows up from, over the same
/// duration, so closing reads as the reverse of opening rather than an
/// unrelated effect.
fn window_close_scale(base: Rect, closed_at_ns: u64, now_ns: u64) -> Rect {
    if closed_at_ns == 0 {
        return base;
    }
    let elapsed = now_ns.saturating_sub(closed_at_ns);
    let progress_milli = if elapsed >= WINDOW_OPEN_NS {
        1000
    } else {
        ((elapsed * 1000) / WINDOW_OPEN_NS) as u32
    };
    let eased = ease_out_milli(progress_milli) as i32;
    let scale_milli = 1000 - 80 * eased / 1000;
    let width = base.width * scale_milli / 1000;
    let height = base.height * scale_milli / 1000;
    let center_x = base.x + base.width / 2;
    let center_y = base.y + base.height / 2;
    Rect::new(center_x - width / 2, center_y - height / 2, width, height)
}

fn dock_fall_offset(entrance_at_ns: u64, index: usize, now_ns: u64) -> i32 {
    let start = entrance_at_ns.saturating_add(index as u64 * DOCK_FALL_STAGGER_NS);
    let elapsed = now_ns.saturating_sub(start);
    let progress_milli = if now_ns < start {
        0
    } else if elapsed >= DOCK_FALL_NS {
        1000
    } else {
        ((elapsed * 1000) / DOCK_FALL_NS) as u32
    };
    let eased = ease_out_milli(progress_milli);
    -(DOCK_FALL_HEIGHT_PX * (1000 - eased as i32) / 1000)
}

fn draw_dock(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    outer: bool,
    now_ns: u64,
) -> (bool, bool) {
    let surface = if outer {
        frost(
            painter,
            layout,
            Rect::new(25, 369, 702, 71),
            CornerRadii::all(30),
            dock_style(),
            0xaed0_0001,
        )
        .captured
    } else {
        true
    };
    let styles = ButtonStyles::figma_glass()
        .with_font(font)
        .with_font_size(13);
    let mut controls = true;
    for index in 0..DOCK_BUTTONS.len() {
        let press_at = state.dock_press_ns[index];
        let release_at = state.dock_release_ns[index];
        let held = press_at != 0 && now_ns < release_at;
        let mut fall_offset = dock_fall_offset(state.dock_entrance_at_ns, index, now_ns);
        let opens = match index {
            1 => DesktopApp::Terminal,
            3 => DesktopApp::Files,
            4 => DesktopApp::Browser,
            5 => DesktopApp::Notes,
            6 => DesktopApp::Trash,
            _ => DesktopApp::None,
        };
        if opens != DesktopApp::None
            && state.app == opens
            && now_ns.saturating_sub(state.window_open_at_ns) < 620_000_000
        {
            let p = progress_of(now_ns, state.window_open_at_ns, 620_000_000) as u64;
            fall_offset -=
                (wave_milli(p * 3 / 2 + 500).abs() * (1000 - p as i32) / 1000) * 14 / 1000;
        }
        fall_offset -= state.dock_lift(index, now_ns);
        let icon_rect = Rect::new(44 + index as i32 * 64, 380 + fall_offset, 50, 50);
        let mut button = Button::new(icon_rect, DOCK_BUTTONS[index], DOCK_NAMES[index], &styles)
            .with_transition(
                if held { press_at } else { 0 },
                if held { 0 } else { release_at },
            );
        button.selected = state.focus_visible && state.dock_focus == index;
        button.interaction.focused = state.focus_visible && state.dock_focus == index;
        button.interaction.pressed = held;
        controls &= button
            .paint_translated(painter, layout.scale, layout.offset, now_ns)
            .captured;
        draw_icon(painter, layout, index, icon_rect);
    }
    let time_bounds = Rect::new(503, 377, 216, 55);
    controls &= frost(
        painter,
        layout,
        time_bounds,
        CornerRadii::all(27),
        time_style(),
        0xaed0_0002,
    )
    .captured;
    let time = time_text();
    let date = date_text();
    centered_text(
        painter,
        layout,
        font,
        Rect::new(503, 377, 103, 55),
        time.as_str(),
        15,
        Color::rgb(9, 18, 22),
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(606, 387, 2, 35)),
        layout.radii(CornerRadii::all(1)),
        Rgba::new(235, 247, 250, 145),
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(608, 387, 1, 35)),
        layout.radii(CornerRadii::all(1)),
        Rgba::new(32, 57, 66, 70),
    );
    centered_text(
        painter,
        layout,
        font,
        Rect::new(609, 377, 110, 55),
        date.as_str(),
        14,
        Color::rgb(9, 18, 22),
    );
    if state.focus_visible && outer {
        let center = 69 + state.dock_focus as i32 * 64;
        draw_hint_pill(
            painter,
            layout,
            font,
            Point::new(center, 356),
            DOCK_NAMES[state.dock_focus],
        );
    }
    controls &= dock_layout_valid();
    (surface, controls)
}

fn draw_hint_pill(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    center: Point,
    label: &str,
) {
    let width = (label.len() as i32 * 7 + 20).clamp(48, 200);
    let bounds = Rect::new(center.x - width / 2, center.y - 10, width, 20);
    painter.fill_rounded_rect(
        layout.rect(bounds),
        layout.radii(CornerRadii::all(10)),
        Rgba::new(12, 16, 20, 205),
    );
    centered_text(
        painter,
        layout,
        font,
        bounds,
        label,
        11,
        Color::rgb(238, 242, 245),
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_app_switcher(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now_ns: u64,
    opening: bool,
    panel_opened_at_ns: u64,
    cheap: bool,
) -> bool {
    // While the panel moves it is still real glass, but the colour work is
    // done per 3x3 block (about 9x cheaper): the full-quality blur of a
    // panel this size made the slide choppy.
    let surface = painter
        .frosted_rounded_rect_stepped(
            layout.rect(Rect::new(25, 20, 702, 420)),
            layout.radii(CornerRadii::all(30)),
            {
                let mut style = layout.frost(dock_style());
                if cheap {
                    style.shadow = Rgba::transparent();
                    style.shadow_spread = 0;
                    style.shadow_softness = 0;
                }
                style
            },
            0xaea0_0001,
            if cheap { 4 } else { 2 },
        )
        .captured;
    let styles = ButtonStyles::figma_glass()
        .with_font(font)
        .with_font_size(13);
    let mut controls = true;
    for (index, label) in APP_LABELS.iter().enumerate() {
        let column = index % 5;
        let row = index / 5;
        let fall = if opening {
            app_icon_fall_offset(panel_opened_at_ns, index, now_ns)
        } else {
            0
        };
        let icon_layout = shift_layout_y(layout, fall);
        let mut button = Button::new(
            Rect::new(58 + column as i32 * 64, 48 + row as i32 * 67, 50, 50),
            label,
            "Application",
            &styles,
        );
        button.selected = state.focus_visible && state.app_focus == index;
        button.interaction.focused = state.focus_visible && state.app_focus == index;
        controls &= button
            .paint_translated(painter, icon_layout.scale, icon_layout.offset, now_ns)
            .captured;
        if index < 5 {
            draw_icon(
                painter,
                icon_layout,
                [1, 3, 4, 5, 2][index],
                Rect::new(58 + column as i32 * 64, 48 + row as i32 * 67, 50, 50),
            );
        }
    }
    for row in 0..4 {
        for column in 0..3 {
            let bounds = layout.rect(Rect::new(482 + column * 65, 56 + row * 67, 28, 23));
            painter.fill_rounded_rect(
                bounds,
                layout.radii(CornerRadii::all(4)),
                Rgba::new(203, 225, 255, 155),
            );
            painter.fill_rounded_rect(
                layout.rect(Rect::new(482 + column * 65, 53 + row * 67, 13, 7)),
                layout.radii(CornerRadii::all(3)),
                Rgba::new(203, 225, 255, 155),
            );
        }
    }
    if state.focus_visible {
        let column = state.app_focus % 5;
        let row = state.app_focus / 5;
        draw_hint_pill(
            painter,
            layout,
            font,
            Point::new(83 + column as i32 * 64, 44 + row as i32 * 67),
            APP_NAMES[state.app_focus.min(APP_NAMES.len() - 1)],
        );
    }
    surface && controls
}

fn draw_icon(painter: &mut Painter<'_>, layout: Layout, kind: usize, bounds: Rect) {
    let x = bounds.x + 13;
    let y = bounds.y + 13;
    let ink = Rgba::new(10, 25, 31, 235);
    match kind {
        0 => {
            for row in 0..2 {
                for column in 0..2 {
                    painter.fill_rounded_rect(
                        layout.rect(Rect::new(x + column * 13, y + row * 13, 9, 9)),
                        layout.radii(CornerRadii::all(3)),
                        ink,
                    );
                }
            }
        }
        1 => {
            painter.fill_rounded_rect(
                layout.rect(Rect::new(x, y + 2, 25, 21)),
                layout.radii(CornerRadii::all(4)),
                ink,
            );
            painter.fill_rounded_rect(
                layout.rect(Rect::new(x + 5, y + 16, 11, 2)),
                layout.radii(CornerRadii::all(1)),
                Rgba::new(220, 240, 244, 235),
            );
        }
        2 => {
            painter.stroke_rounded_rect(
                layout.rect(Rect::new(x + 2, y + 2, 21, 21)),
                layout.radii(CornerRadii::all(11)),
                layout.scale.logical(3).max(1) as u8,
                ink,
            );
            painter.fill_rounded_rect(
                layout.rect(Rect::new(x + 10, y + 10, 5, 5)),
                layout.radii(CornerRadii::all(3)),
                ink,
            );
        }
        3 => {
            painter.fill_rounded_rect(
                layout.rect(Rect::new(x, y + 6, 26, 17)),
                layout.radii(CornerRadii::all(4)),
                ink,
            );
            painter.fill_rounded_rect(
                layout.rect(Rect::new(x + 2, y + 3, 11, 6)),
                layout.radii(CornerRadii::all(2)),
                ink,
            );
        }
        4 => {
            painter.stroke_rounded_rect(
                layout.rect(Rect::new(x, y, 26, 26)),
                layout.radii(CornerRadii::all(13)),
                layout.scale.logical(3).max(1) as u8,
                Rgba::new(18, 91, 145, 240),
            );
            painter.fill_rounded_rect(
                layout.rect(Rect::new(x + 9, y + 9, 8, 8)),
                layout.radii(CornerRadii::all(4)),
                Rgba::new(61, 166, 91, 240),
            );
        }
        5 => {
            painter.fill_rounded_rect(
                layout.rect(Rect::new(x + 3, y, 20, 26)),
                layout.radii(CornerRadii::all(4)),
                Rgba::new(235, 242, 244, 230),
            );
            for row in 0..3 {
                painter.fill_rounded_rect(
                    layout.rect(Rect::new(x + 7, y + 7 + row * 6, 12, 2)),
                    layout.radii(CornerRadii::all(1)),
                    ink,
                );
            }
        }
        _ => {
            painter.stroke_rounded_rect(
                layout.rect(Rect::new(x + 5, y + 5, 16, 20)),
                layout.radii(CornerRadii::new(2, 2, 5, 5)),
                layout.scale.logical(2).max(1) as u8,
                ink,
            );
            painter.fill_rounded_rect(
                layout.rect(Rect::new(x + 3, y + 2, 20, 3)),
                layout.radii(CornerRadii::all(2)),
                ink,
            );
        }
    }
}

/// Which app's window is maximized (0 = none, else the app's number + 1).
static MAXIMIZED_APP: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// Windows whose contents follow their size, so they can be maximized.
fn can_maximize(app: DesktopApp) -> bool {
    matches!(
        app,
        DesktopApp::Store | DesktopApp::Terminal | DesktopApp::Linux
    )
}

fn is_maximized(app: DesktopApp) -> bool {
    MAXIMIZED_APP.load(Ordering::Relaxed) == app as u8 + 1
}

/// The three controls at the right of a window's title bar (minimize,
/// maximize, close), as touch-sized hit boxes.
fn window_control_rects(window: Rect) -> (Rect, Rect, Rect) {
    let box_at = |from_right: i32| {
        Rect::new(
            window.x + window.width - from_right - 12,
            window.y + 2,
            24,
            24,
        )
    };
    (box_at(72), box_at(46), box_at(20))
}

fn window_base_rect(app: DesktopApp) -> Rect {
    if is_maximized(app) {
        return Rect::new(10, 10, 732, 354);
    }
    match app {
        DesktopApp::Terminal => Rect::new(14, 15, 724, 340),
        DesktopApp::Files => Rect::new(70, 36, 610, 300),
        DesktopApp::Notes => Rect::new(232, 18, 360, 356),
        DesktopApp::Trash => Rect::new(256, 90, 336, 260),
        // 602x320 of content: the guest screen is scaled down to fit.
        DesktopApp::Linux => Rect::new(65, 10, 622, 364),
        DesktopApp::Browser => Rect::new(60, 16, 632, 350),
        DesktopApp::Store => Rect::new(80, 18, 592, 346),
        _ => Rect::new(113, 32, 531, 303),
    }
}

fn window_style_for(app: DesktopApp) -> FrostStyle {
    // Only the window's own frame reads as glass now - it is the "highlight"
    // edge, not the surface content sits on. Every app draws a solid content
    // card on top (see draw_files/draw_notes/draw_trash/draw_browser), so
    // these tints only need to be light enough to tint the frame margin
    // still visible around/between those cards.
    let base = window_style();
    match app {
        DesktopApp::Files => FrostStyle {
            tint: Rgba::new(140, 195, 235, 40),
            border: Rgba::new(205, 232, 250, 130),
            ..base
        },
        DesktopApp::Notes => FrostStyle {
            blur_radius: 14,
            tint: Rgba::new(238, 205, 150, 38),
            border: Rgba::new(248, 226, 190, 140),
            inner_highlight: Rgba::new(255, 245, 220, 40),
            ..base
        },
        DesktopApp::Trash => FrostStyle {
            blur_radius: 22,
            saturation_percent: 55,
            brightness_percent: 92,
            tint: Rgba::new(150, 120, 120, 44),
            border: Rgba::new(190, 160, 160, 120),
            ..base
        },
        DesktopApp::Browser => FrostStyle {
            blur_radius: 24,
            tint: Rgba::new(150, 220, 220, 36),
            border: Rgba::new(200, 240, 240, 120),
            ..base
        },
        _ => base,
    }
}

fn window_titlebar_color(app: DesktopApp) -> Rgba {
    match app {
        DesktopApp::Files => Rgba::new(235, 244, 250, 245),
        DesktopApp::Notes => Rgba::new(250, 241, 222, 248),
        DesktopApp::Trash => Rgba::new(238, 231, 231, 240),
        _ => Rgba::new(231, 231, 231, 246),
    }
}

/// Minimize, maximize and close, drawn as in the design: small light glyphs
/// with a thin dark outline (a pill, a rounded square and a crossed pair of bars).
fn draw_window_controls(painter: &mut Painter<'_>, layout: Layout, window: Rect) {
    let outline = Rgba::opaque(73, 73, 73);
    let fill = Rgba::opaque(217, 217, 217);
    let centre_y = window.y + 14;
    let round = |value: i32| layout.radii(CornerRadii::all(value));
    let minimize = Rect::new(window.x + window.width - 72 - 8, centre_y - 3, 16, 7);
    painter.fill_rounded_rect(layout.rect(minimize), round(3), outline);
    painter.fill_rounded_rect(
        layout.rect(Rect::new(
            minimize.x + 1,
            minimize.y + 1,
            minimize.width - 2,
            minimize.height - 2,
        )),
        round(2),
        fill,
    );
    let maximize = Rect::new(window.x + window.width - 46 - 8, centre_y - 8, 16, 16);
    painter.fill_rounded_rect(layout.rect(maximize), round(5), outline);
    painter.fill_rounded_rect(
        layout.rect(Rect::new(
            maximize.x + 1,
            maximize.y + 1,
            maximize.width - 2,
            maximize.height - 2,
        )),
        round(4),
        fill,
    );
    let centre_x = window.x + window.width - 20;
    for step in -5..=5 {
        for sign in [1, -1] {
            painter.fill_rounded_rect(
                layout.rect(Rect::new(
                    centre_x + step - 3,
                    centre_y + sign * step - 3,
                    6,
                    6,
                )),
                round(3),
                outline,
            );
        }
    }
    for step in -5..=5 {
        for sign in [1, -1] {
            painter.fill_rounded_rect(
                layout.rect(Rect::new(
                    centre_x + step - 2,
                    centre_y + sign * step - 2,
                    4,
                    4,
                )),
                round(2),
                fill,
            );
        }
    }
}

fn draw_window(
    painter: &mut Painter<'_>,
    layout: Layout,
    ui_font: RasterFont,
    mono_font: RasterFont,
    state: &DesktopState,
    now_ns: u64,
    closing: bool,
) -> bool {
    let base = window_base_rect(state.app);
    let outer = if closing {
        window_close_scale(base, state.app_close_at_ns, now_ns)
    } else {
        window_open_scale(base, state.window_open_at_ns, now_ns)
    };
    let opening = !closing && now_ns.saturating_sub(state.window_open_at_ns) < WINDOW_OPEN_NS;
    let blur_off =
        opening || closing || (state.app == DesktopApp::Browser && crate::web::get().busy());
    // A settled window's frosted body only depends on the wallpaper, so it is
    // rendered once and replayed (every keystroke used to re-blur it).
    let cacheable = !opening && !closing && !blur_off && !state.seamless.enabled;
    let margin = layout.scale.logical(36);
    let region = {
        let physical = layout.rect(outer);
        Rect::new(
            physical.x - margin,
            physical.y - margin,
            physical.width + margin * 2,
            physical.height + margin * 2,
        )
    };
    let region = region
        .intersect(Rect::new(
            0,
            0,
            painter.frame_width(),
            painter.frame_height(),
        ))
        .unwrap_or(region);
    let cache = unsafe { &mut *WINDOW_CACHE.0.get() };
    let cache_len = (region.width.max(0) as usize) * (region.height.max(0) as usize);
    let mut key = dock_cache_key(layout, region) ^ 0x71d0_0000;
    key ^= (state.app as u64 + 1) * 0x9e37_79b9;
    let captured = if cacheable
        && cache_len <= cache.len()
        && WINDOW_CACHE_KEY.load(Ordering::Acquire) == key
        && painter.write_region(region, &cache[..cache_len])
    {
        true
    } else {
        let captured = frost_mode(
            painter,
            layout,
            outer,
            CornerRadii::all(30),
            window_style_for(state.app),
            0xaec5_0001,
            blur_off,
        )
        .captured;
        if cacheable
            && captured
            && cache_len <= cache.len()
            && painter.read_region(region, &mut cache[..cache_len])
        {
            WINDOW_CACHE_KEY.store(key, Ordering::Release);
        }
        captured
    };
    painter.fill_rounded_rect(
        layout.rect(Rect::new(outer.x, outer.y, outer.width, 28)),
        layout.radii(CornerRadii::new(14, 14, 0, 0)),
        window_titlebar_color(state.app),
    );
    text(
        painter,
        layout,
        ui_font,
        Point::new(outer.x + 15, outer.y + 6),
        match state.app {
            DesktopApp::Browser => "Browser",
            DesktopApp::Settings => "Settings",
            DesktopApp::Files => "Files",
            DesktopApp::Notes => "Notes",
            DesktopApp::Trash => "Trash",
            DesktopApp::Terminal => "Shell",
            DesktopApp::Linux => "Linux",
            DesktopApp::Store => "App Store",
            DesktopApp::None => "AerOS",
        },
        13,
        Color::rgb(18, 23, 26),
    );
    draw_window_controls(painter, layout, outer);
    if state.app == DesktopApp::Browser {
        draw_browser(painter, layout, ui_font, mono_font, state);
        return captured;
    }
    if state.app == DesktopApp::Store {
        draw_store(painter, layout, ui_font, mono_font, state);
        return captured;
    }
    if state.app == DesktopApp::Settings {
        draw_settings(painter, layout, ui_font, state);
        return captured;
    }
    if state.app == DesktopApp::Terminal {
        draw_shell_window(painter, layout, mono_font, state, outer, now_ns);
        return captured;
    }
    if state.app == DesktopApp::Files {
        draw_files(painter, layout, ui_font, mono_font, state, outer);
        return captured;
    }
    if state.app == DesktopApp::Notes {
        draw_notes(painter, layout, ui_font, mono_font, state, outer);
        return captured;
    }
    if state.app == DesktopApp::Trash {
        draw_trash(painter, layout, ui_font, mono_font, state, outer);
        return captured;
    }
    if state.app == DesktopApp::Linux {
        draw_linux(painter, layout, ui_font, outer);
        return captured;
    }
    text(
        painter,
        layout,
        mono_font,
        Point::new(136, 82),
        "      /\\",
        17,
        Color::rgb(19, 88, 112),
    );
    text(
        painter,
        layout,
        mono_font,
        Point::new(136, 104),
        "  AerOS  /__\\",
        17,
        Color::rgb(19, 88, 112),
    );
    text(
        painter,
        layout,
        mono_font,
        Point::new(136, 137),
        "Native x86-64 kernel",
        13,
        Color::rgb(20, 45, 55),
    );
    text(
        painter,
        layout,
        mono_font,
        Point::new(136, 178),
        "User@AerOS:~$ _",
        15,
        Color::rgb(24, 66, 76),
    );
    text(
        painter,
        layout,
        mono_font,
        Point::new(366, 84),
        "UI  AerUI",
        11,
        Color::rgb(23, 50, 59),
    );
    text(
        painter,
        layout,
        mono_font,
        Point::new(366, 102),
        "Shell  aersh",
        11,
        Color::rgb(23, 50, 59),
    );
    captured
}

const AEROS_LOGO: &str = r"                                                 .-+%@@#.     :#@#-.
                                            .-%@@@@@@@@=    :#@@@@@@%:.
                                        ..=@@@@@@@@@@@%.  .*@@@@@@@@@@@-.
                                      .=#@@@@@@@@@@@@@+ :*@@@@@@@@@@@@@@%-.
                                    .+@@@@@@@@@@@@@@@@:*@@@@@@@@@@@@@@@@@@%=.
                                  .*@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@#-.
                                 =@@@@@@@@@@@@@@@@@@@@@@@@@@@@@= .+@@@@@@@@@@@@@@@@%###%@@@@@@@%=
                               :%@@@@@@@@@@@@@@@@@@@@@@@@@@@@@+.  -%@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@-
                             .=@@@@@@@@@@@@@@@@%**@@@@@@@@@@@*.  .*@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@*
                            .*@@@@@@@@@@@@@@@@*.  -%@@@@@@@@%:   +@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@+
                           .*@@@@@@@@@@@@@@@@#:  .+@@@@@@@@@-   -@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@:
                           +@@@@@@@@@@@@@@@@%-   -@@@@@@@@@+   .@@@@@@@@@@@@@@@@@%=:-%@@@@@@@@@@@=
                          -@@@@@@@@@@@@@@@@@+   .@@@@@@@@@*   .#@@@@@@@@@@@@@#=.      =@@@@@@@@@+
                         .@@@@@@@@@@@@@@@@@#   .%@@@@@@@@%.  .*@@@@@@@@@#+-.          +@@@@@@@@=.
     .@@@@.              +@@@@@@@@@@@@@@@@%.  .*@@@@@@@@@-. .=@@@@%*=-.           .-+%@@@@@@@@=.
   @@@@.@@@@@@@@@@@@.   .#@@@@@@@@@@@@@@@@:   =@@@@@@@@@+.  .=-:..            .=#%@@@@@@@@@@%-
   @@           .....@-=+#%@@@@@@@@@@@@@@=.  -%@@@@%#+=:..                .-*@@@@@@@@@@@@@@@#.
   @@.                                                            ...-*@@@@@@@@@@@@@@@@@@@-
    @@@@@@@..                                                     .:=#%@@@@@@@@@@@@@@@@@*:
        ..@@@@@@@@@@@@..                           .:-=+*#%@@#+-.      .-+#@@@@@@@@@@@%-.
                  ...@@@-=%%%%%%%#######+:.      .:=#@@@@@@@@@@@@%#=.      .-%@@@@@@@=.
                          =@@@@@@@@@@@@@@@@@%=.      ..:*@@@@@@@@@@@@@@+:   :#@@@@@+.
                           =@@@@@@@@@@@@@@@@@@@@@+:..      .+%@@@@@@@@@@@@@@@@@@@+.
                           .=@@@@@@@@@@@@@@@@@@@@@@@@*-:.    :#@@@@@@@@@@@@@@@%=
                            .:#@@@@@@@@@@@=@@@@@@@@@@@@@@%+--*@@@@@@@@@@@@@@#-.
                              .-%@@@@@@@@*.+@@@@@@@@@@@@@@@@@@@#=%@@@@@@@%=.
                                .:*@@@@@@-..#@@@@@@@@@@@@@@@@@@@ .:#@@@=..
                                    :#@@%:  :%@@@@@@@@@@@@@@@@@@:
                                       ...  .=@@@@@@@@@@@@@%%*=:
                                              ...::::::..";

/// The Linux guest as an ordinary AerOS window: same chrome as every other
/// app, with the guest's framebuffer drawn straight into the content area.
fn draw_linux(painter: &mut Painter<'_>, layout: Layout, ui_font: RasterFont, outer: Rect) {
    let screen = Rect::new(
        outer.x + 10,
        outer.y + 34,
        outer.width - 20,
        outer.height - 44,
    );
    painter.fill_rounded_rect(
        layout.rect(screen),
        layout.radii(CornerRadii::all(8)),
        Rgba::opaque(10, 12, 16),
    );
    // Key hints in the title bar, next to the window buttons' left edge.
    text(
        painter,
        layout,
        ui_font,
        Point::new(outer.x + 90, outer.y + 8),
        "F10 back   F9 zoom 1:1",
        11,
        Color::rgb(90, 100, 110),
    );
    match crate::svm::linux_framebuffer() {
        Some((pixels, width, height, stride)) if crate::svm::linux_ready() => {
            // SAFETY: the pointer/geometry come straight from the hypervisor's
            // framebuffer region, which is always FB_PITCH * FB_HEIGHT bytes.
            unsafe {
                if linux_zoomed() {
                    let area = layout.rect(screen);
                    let view_w = (area.width as usize).min(width);
                    let view_h = (area.height as usize).min(height);
                    let (pan_x, pan_y) = linux_pan();
                    let pan_x = (pan_x.max(0) as usize).min(width - view_w);
                    let pan_y = (pan_y.max(0) as usize).min(height - view_h);
                    painter.blit_scaled(
                        Rect::new(area.x, area.y, view_w as i32, view_h as i32),
                        pixels.add(pan_y * stride + pan_x),
                        view_w,
                        view_h,
                        stride,
                    );
                } else {
                    painter.blit_scaled(layout.rect(screen), pixels, width, height, stride);
                }
            }
        }
        _ => {
            let light = Color::rgb(214, 222, 230);
            text(
                painter,
                layout,
                ui_font,
                Point::new(screen.x + 20, screen.y + 22),
                "Linux is not running",
                15,
                light,
            );
            text(
                painter,
                layout,
                ui_font,
                Point::new(screen.x + 20, screen.y + 50),
                "Build with --features linux-guest and put VMLINUZ,",
                11,
                Color::rgb(150, 162, 174),
            );
            text(
                painter,
                layout,
                ui_font,
                Point::new(screen.x + 20, screen.y + 68),
                "INITRD and ROOTFS on the boot volume.",
                11,
                Color::rgb(150, 162, 174),
            );
        }
    }
}

fn draw_shell_window(
    painter: &mut Painter<'_>,
    layout: Layout,
    mono_font: RasterFont,
    state: &DesktopState,
    outer: Rect,
    now_ns: u64,
) {
    let content_x = outer.x + 16;
    let content_y = outer.y + 44;
    text(
        painter,
        layout,
        mono_font,
        Point::new(content_x, content_y),
        AEROS_LOGO,
        4,
        Color::rgb(19, 88, 112),
    );

    // Real dark-terminal card for the functional right column, distinct
    // from the light-glass chrome every other app shares - this is the one
    // app where the content itself should look like an actual terminal,
    // not another frosted panel with text on it. Sized to actually fit
    // SHELL_LINE_MAX (40 monospace characters) without clipping real
    // command output like `help`'s column listing - narrower than that
    // and real shell output gets cut off mid-word, not just decoration.
    let card_x = content_x + 300;
    let card = Rect::new(
        card_x,
        content_y - 10,
        (outer.x + outer.width - 14) - card_x,
        (outer.y + outer.height - 12) - (content_y - 10),
    );
    painter.fill_rounded_rect(
        layout.rect(card),
        layout.radii(CornerRadii::all(12)),
        Rgba::new(14, 17, 22, 250),
    );
    painter.stroke_rounded_rect(
        layout.rect(card),
        layout.radii(CornerRadii::all(12)),
        layout.scale.logical(1).max(1) as u8,
        Rgba::new(60, 72, 82, 150),
    );

    let ram_mb = crate::memory::global_stats()
        .map(|stats| stats.allocated_pages * 4096 / (1024 * 1024))
        .unwrap_or(0);
    let group = if state.terminal_elevated {
        "root"
    } else {
        "standard"
    };
    let mut header: shell::Text<96> = shell::Text::new();
    let _ = write!(
        header,
        "{}@aeros  RAM {}MB  {}",
        state.display_name(),
        ram_mb,
        group
    );
    text(
        painter,
        layout,
        mono_font,
        Point::new(card.x + 12, card.y + 9),
        header.as_str(),
        10,
        Color::rgb(150, 160, 168),
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(card.x + 12, card.y + 24, card.width - 24, 1)),
        CornerRadii::all(0),
        Rgba::new(70, 82, 92, 160),
    );

    let scrollback_top = card.y + 32;
    let clip = layout.rect(Rect::new(
        card.x,
        scrollback_top,
        card.width,
        card.height - 40,
    ));
    let previous_clip = painter.clip();
    painter.set_clip(clip);
    let visible = state.terminal_line_count.min(SHELL_HISTORY_LINES);
    let first = state.terminal_line_count - visible;
    for row in 0..visible {
        text(
            painter,
            layout,
            mono_font,
            Point::new(card.x + 12, scrollback_top + row as i32 * 16),
            state.terminal_line_str(first + row),
            11,
            Color::rgb(206, 212, 218),
        );
    }
    painter.set_clip(previous_clip);

    let prefix = if state.terminal_elevated {
        "root"
    } else {
        state.display_name()
    };
    let prompt_y = scrollback_top + visible as i32 * 16;
    let mut prompt: shell::Text<48> = shell::Text::new();
    let _ = write!(prompt, "{prefix}>");
    text(
        painter,
        layout,
        mono_font,
        Point::new(card.x + 12, prompt_y),
        prompt.as_str(),
        11,
        Color::rgb(90, 205, 145),
    );
    // Text width measurement only works in physical pixels, so everything
    // from here on works in the same physical space `layout.point` maps
    // into - not the logical design coordinates the rest of this function
    // uses - to place the typed text and blinking cursor immediately after
    // the measured prompt prefix.
    let physical_size = layout.scale.logical(11);
    let prompt_origin = layout.point(Point::new(card.x + 12, prompt_y));
    let prefix_width = mono_font.text_width(prompt.as_str(), physical_size);
    let typed_x = prompt_origin.x + prefix_width + layout.scale.logical(6);
    painter.text(
        mono_font,
        Point::new(typed_x, prompt_origin.y),
        state.terminal_input_str(),
        physical_size,
        Color::rgb(230, 233, 236),
    );
    if (now_ns / 530_000_000).is_multiple_of(2) {
        let typed_width = mono_font.text_width(state.terminal_input_str(), physical_size);
        let cursor_width = mono_font
            .text_width(">", physical_size)
            .max(layout.scale.logical(6));
        painter.fill_rounded_rect(
            Rect::new(
                typed_x + typed_width,
                prompt_origin.y,
                cursor_width,
                layout.scale.logical(14),
            ),
            CornerRadii::all(1),
            Rgba::new(230, 233, 236, 220),
        );
    }
}

fn browser_content_rect(base: Rect) -> Rect {
    Rect::new(base.x + 10, base.y + 70, base.width - 20, base.height - 80)
}

fn browser_favorite_rect(content: Rect, index: usize) -> Rect {
    let column = (index % 3) as i32;
    let row = (index / 3) as i32;
    Rect::new(
        content.x + 28 + column * 190,
        content.y + 64 + row * 80,
        176,
        66,
    )
}

fn tile_color(seed: usize) -> Rgba {
    const PALETTE: [(u8, u8, u8); 6] = [
        (76, 163, 224),
        (231, 111, 81),
        (94, 187, 120),
        (160, 120, 220),
        (240, 180, 60),
        (70, 190, 190),
    ];
    let (red, green, blue) = PALETTE[seed % PALETTE.len()];
    Rgba::opaque(red, green, blue)
}

/// A round-cornered toolbar button with a text glyph.
fn browser_nav_button(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    bounds: Rect,
    glyph: &str,
    enabled: bool,
) {
    painter.fill_rounded_rect(
        layout.rect(bounds),
        layout.radii(CornerRadii::all(9)),
        Rgba::new(226, 231, 236, if enabled { 230 } else { 120 }),
    );
    let tone = if enabled {
        Color::rgb(52, 62, 72)
    } else {
        Color::rgb(160, 168, 176)
    };
    centered_text(painter, layout, font, bounds, glyph, 17, tone);
}

/// Draws one line of page text, colouring `[n]` link markers blue.
#[allow(clippy::too_many_arguments)]
fn draw_page_line(
    painter: &mut Painter<'_>,
    font: RasterFont,
    origin: Point,
    line: &str,
    px: i32,
    char_width: i32,
    plain: Color,
    link: Color,
) {
    let bytes = line.as_bytes();
    let mut x = origin.x;
    let mut start = 0usize;
    let mut at = 0usize;
    let flush = |painter: &mut Painter<'_>, from: usize, to: usize, color: Color, x: &mut i32| {
        if to > from
            && let Ok(segment) = core::str::from_utf8(&bytes[from..to])
        {
            painter.text(font, Point::new(*x, origin.y), segment, px, color);
        }
        *x += char_width * (to - from) as i32;
    };
    while at < bytes.len() {
        if bytes[at] == b'[' {
            let mut end = at + 1;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > at + 1 && end < bytes.len() && bytes[end] == b']' {
                flush(painter, start, at, plain, &mut x);
                flush(painter, at, end + 1, link, &mut x);
                start = end + 1;
                at = end + 1;
                continue;
            }
        }
        at += 1;
    }
    flush(painter, start, bytes.len(), plain, &mut x);
}

/// The browser: a Safari-style toolbar (back, forward, address pill, reload)
/// over a white page area. Pages come from the Linux guest as text.
fn draw_browser(
    painter: &mut Painter<'_>,
    layout: Layout,
    ui_font: RasterFont,
    mono_font: RasterFont,
    state: &DesktopState,
) {
    let base = window_base_rect(DesktopApp::Browser);
    let web = crate::web::get();
    let now = crate::time::monotonic_nanoseconds();
    let bar_y = base.y + 34;
    browser_nav_button(
        painter,
        layout,
        ui_font,
        Rect::new(base.x + 14, bar_y, 28, 26),
        "<",
        web.can_go_back(),
    );
    browser_nav_button(
        painter,
        layout,
        ui_font,
        Rect::new(base.x + 46, bar_y, 28, 26),
        ">",
        web.can_go_forward(),
    );
    // Reload: a ring with a gap.
    let reload = Rect::new(base.right() - 44, bar_y, 28, 26);
    browser_nav_button(painter, layout, ui_font, reload, "", true);
    let ring = Rect::new(reload.x + 7, reload.y + 5, 14, 14);
    let ring_color = Rgba::opaque(52, 62, 72);
    painter.fill_rounded_rect(
        layout.rect(ring),
        layout.radii(CornerRadii::all(7)),
        ring_color,
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(ring.x + 2, ring.y + 2, 10, 10)),
        layout.radii(CornerRadii::all(5)),
        Rgba::new(226, 231, 236, 255),
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(ring.x + 8, ring.y - 1, 7, 6)),
        layout.radii(CornerRadii::all(0)),
        Rgba::new(226, 231, 236, 255),
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(ring.x + 10, ring.y, 5, 3)),
        layout.radii(CornerRadii::all(1)),
        ring_color,
    );
    // Address pill.
    let pill = Rect::new(base.x + 84, bar_y, base.width - 84 - 58, 26);
    let pill_radii = layout.radii(CornerRadii::all(13));
    painter.fill_rounded_rect(
        layout.rect(pill),
        pill_radii,
        if state.url_active {
            Rgba::opaque(255, 255, 255)
        } else {
            Rgba::new(226, 231, 236, 235)
        },
    );
    if web.busy() {
        let elapsed_ms = now.saturating_sub(web.started_ns) / 1_000_000;
        let progress = crate::loading::creeping_progress(elapsed_ms) as i32;
        painter.fill_rounded_rect(
            layout.rect(Rect::new(
                pill.x,
                pill.y,
                (pill.width * progress / 1000).max(26),
                pill.height,
            )),
            pill_radii,
            Rgba::new(64, 140, 205, 70),
        );
    }
    painter.stroke_rounded_rect(
        layout.rect(pill),
        pill_radii,
        layout
            .scale
            .logical(if state.url_active { 2 } else { 1 })
            .max(1) as u8,
        if state.url_active {
            Rgba::new(31, 108, 205, 230)
        } else {
            Rgba::new(150, 160, 170, 120)
        },
    );
    if state.url_active {
        let mut bar = [0u8; MAX_URL + 1];
        let shown = state.url_str().as_bytes();
        let visible = shown.len().min(MAX_URL - 8).min(64);
        bar[..visible].copy_from_slice(&shown[shown.len() - visible..]);
        bar[visible] = b'_';
        if let Ok(bar_text) = core::str::from_utf8(&bar[..visible + 1]) {
            text(
                painter,
                layout,
                ui_font,
                Point::new(pill.x + 14, pill.y + 6),
                bar_text,
                13,
                Color::rgb(25, 32, 40),
            );
        }
    } else {
        let address = web.url();
        let (label, tone) = if address.is_empty() {
            ("Search or enter website name", Color::rgb(120, 130, 140))
        } else {
            (crate::web::host_of(address), Color::rgb(30, 38, 46))
        };
        let width = ui_font.text_width(label, layout.scale.logical(13));
        let lock = address.starts_with("https://");
        let total = width + if lock { layout.scale.logical(14) } else { 0 };
        let start = layout.rect(pill).x + (layout.rect(pill).width - total) / 2;
        if lock {
            let body = Rect::new(
                pill.x + pill.width / 2 - (total / 2) / 2 - 2,
                pill.y + 11,
                8,
                6,
            );
            let lock_color = Rgba::opaque(110, 120, 130);
            let _ = body;
            let lock_x = pixel_to_logical(layout, start);
            painter.fill_rounded_rect(
                layout.rect(Rect::new(lock_x, pill.y + 11, 8, 6)),
                layout.radii(CornerRadii::all(2)),
                lock_color,
            );
            painter.stroke_rounded_rect(
                layout.rect(Rect::new(lock_x + 1, pill.y + 7, 6, 6)),
                layout.radii(CornerRadii::all(3)),
                1,
                lock_color,
            );
        }
        painter.text(
            ui_font,
            Point::new(
                start + if lock { layout.scale.logical(14) } else { 0 },
                layout.point(Point::new(0, pill.y + 6)).y,
            ),
            label,
            layout.scale.logical(13),
            tone,
        );
    }
    // Page area.
    let content = browser_content_rect(base);
    let card = layout.rect(content);
    painter.fill_rounded_rect(
        card,
        layout.radii(CornerRadii::all(10)),
        Rgba::new(253, 253, 254, 245),
    );
    painter.stroke_rounded_rect(
        card,
        layout.radii(CornerRadii::all(10)),
        1,
        Rgba::new(150, 160, 170, 90),
    );
    let previous_clip = painter.clip();
    painter.set_clip(card);
    let dark = Color::rgb(28, 34, 40);
    let muted = Color::rgb(112, 122, 132);
    let native_fallback = !crate::svm::linux_ready();
    match web.state {
        _ if native_fallback => {
            draw_browser_legacy_body(painter, layout, ui_font, mono_font, state);
        }
        crate::web::WebState::Blank => {
            text(
                painter,
                layout,
                ui_font,
                Point::new(content.x + 28, content.y + 26),
                "Favorites",
                17,
                dark,
            );
            for (index, (name, address)) in BROWSER_FAVORITES.iter().enumerate() {
                let tile = browser_favorite_rect(content, index);
                painter.fill_rounded_rect(
                    layout.rect(tile),
                    layout.radii(CornerRadii::all(14)),
                    Rgba::new(238, 242, 246, 255),
                );
                let icon = Rect::new(tile.x + 12, tile.y + 13, 40, 40);
                painter.fill_rounded_rect(
                    layout.rect(icon),
                    layout.radii(CornerRadii::all(10)),
                    tile_color(index),
                );
                let initial = &name[..1];
                centered_text(
                    painter,
                    layout,
                    ui_font,
                    icon,
                    initial,
                    20,
                    Color::rgb(255, 255, 255),
                );
                text(
                    painter,
                    layout,
                    ui_font,
                    Point::new(tile.x + 62, tile.y + 15),
                    name,
                    14,
                    dark,
                );
                text(
                    painter,
                    layout,
                    ui_font,
                    Point::new(tile.x + 62, tile.y + 35),
                    crate::web::host_of(address),
                    10,
                    muted,
                );
            }
            text(
                painter,
                layout,
                ui_font,
                Point::new(content.x + 28, content.y + 232),
                "Pages are fetched and drawn as text by the Linux system.",
                11,
                muted,
            );
        }
        crate::web::WebState::Starting | crate::web::WebState::Loading => {
            let (title, note) = if web.state == crate::web::WebState::Starting {
                (
                    "Starting the Linux web engine...",
                    "The first page takes about a minute; later pages are quick.",
                )
            } else {
                ("Loading...", crate::web::host_of(web.url()))
            };
            centered_text(
                painter,
                layout,
                ui_font,
                Rect::new(content.x, content.y + 96, content.width, 24),
                title,
                17,
                dark,
            );
            centered_text(
                painter,
                layout,
                ui_font,
                Rect::new(content.x, content.y + 124, content.width, 20),
                note,
                12,
                muted,
            );
            // A soft indeterminate bar.
            let track = Rect::new(content.x + content.width / 2 - 90, content.y + 158, 180, 4);
            painter.fill_rounded_rect(
                layout.rect(track),
                layout.radii(CornerRadii::all(2)),
                Rgba::new(150, 160, 170, 70),
            );
            let phase = (now / 1_000_000 % 1400) as i32;
            let head = track.x + (track.width + 60) * phase / 1400 - 60;
            let left = head.max(track.x);
            let right = (head + 60).min(track.right());
            if right > left {
                painter.fill_rounded_rect(
                    layout.rect(Rect::new(left, track.y, right - left, track.height)),
                    layout.radii(CornerRadii::all(2)),
                    Rgba::opaque(64, 140, 205),
                );
            }
        }
        crate::web::WebState::Loaded | crate::web::WebState::Failed => {
            let px = layout.scale.logical(11);
            let char_width = mono_font.text_width("M", px).max(1);
            BROWSER_CHAR_W_MILLI.store(
                (char_width as i64 * 1_000_000 / layout.scale.logical(1000).max(1) as i64) as u32,
                core::sync::atomic::Ordering::Relaxed,
            );
            let origin_x = card.x + layout.scale.logical(12);
            let mut y = card.y + layout.scale.logical(8);
            let failed = web.state == crate::web::WebState::Failed;
            if failed {
                text(
                    painter,
                    layout,
                    ui_font,
                    Point::new(content.x + 12, content.y + 10),
                    "The page could not be opened",
                    14,
                    Color::rgb(178, 52, 40),
                );
                y += layout.scale.logical(26);
            }
            for row in 0..BROWSER_VISIBLE_LINES {
                let line = web.line(web.scroll + row);
                draw_page_line(
                    painter,
                    mono_font,
                    Point::new(origin_x, y),
                    line,
                    px,
                    char_width,
                    dark,
                    Color::rgb(31, 108, 205),
                );
                y += layout.scale.logical(BROWSER_LINE_HEIGHT);
            }
            // A thin overlay scrollbar.
            let total = web.line_count();
            if total > BROWSER_VISIBLE_LINES {
                let track = Rect::new(content.right() - 8, content.y + 10, 4, content.height - 20);
                let thumb_h = (track.height * BROWSER_VISIBLE_LINES as i32 / total as i32).max(24);
                let travel = track.height - thumb_h;
                let max_scroll = (total - BROWSER_VISIBLE_LINES).max(1) as i32;
                let thumb_y = track.y + travel * web.scroll as i32 / max_scroll;
                painter.fill_rounded_rect(
                    layout.rect(Rect::new(track.x, thumb_y, track.width, thumb_h)),
                    layout.radii(CornerRadii::all(2)),
                    Rgba::new(90, 100, 110, 150),
                );
            }
        }
    }
    painter.set_clip(previous_clip);
}

/// Converts a physical x back to logical units (for icon placement).
fn pixel_to_logical(layout: Layout, physical_x: i32) -> i32 {
    let per_thousand = layout.scale.logical(1000).max(1) as i64;
    ((physical_x - layout.offset.x) as i64 * 1000 / per_thousand) as i32
}

fn store_search_rect(base: Rect) -> Rect {
    Rect::new(base.x + (base.width - 220) / 2, base.y + 40, 220, 26)
}

fn store_toggle_rect(base: Rect) -> Rect {
    Rect::new(base.x + 22, base.y + 40, 84, 26)
}

/// Column and row spacing of the app grid (it grows with the window).
fn store_grid_pitch(base: Rect) -> (i32, i32) {
    ((base.width - 24) / 3, (base.height - 98) / 4)
}

fn store_cell_rect(base: Rect, column: usize, row: usize) -> Rect {
    let (across, down) = store_grid_pitch(base);
    Rect::new(
        base.x + 12 + column as i32 * across,
        base.y + 80 + row as i32 * down,
        across - 4,
        down - 4,
    )
}

fn store_back_rect(base: Rect) -> Rect {
    Rect::new(base.x + 18, base.y + 36, 70, 18)
}

fn store_get_rect(base: Rect) -> Rect {
    Rect::new(base.x + 98, base.y + 104, 90, 24)
}

fn store_group_rect(base: Rect) -> Rect {
    Rect::new(base.x + base.width - 292, base.y + 58, 270, 66)
}

fn store_about_rect(base: Rect) -> Rect {
    Rect::new(
        base.x + 22,
        base.y + 142,
        base.width / 2 - 46,
        base.height - 160,
    )
}

fn store_similar_panel(base: Rect) -> Rect {
    Rect::new(
        base.x + base.width / 2 - 10,
        base.y + 142,
        base.width / 2 - 12,
        base.height - 160,
    )
}

fn store_similar_rect(base: Rect, slot: usize) -> Rect {
    let panel = store_similar_panel(base);
    let across = (panel.width - 16) / 2;
    let down = (panel.height - 44) / 2;
    Rect::new(
        panel.x + 10 + (slot % 2) as i32 * across,
        panel.y + 36 + (slot / 2) as i32 * down,
        across - 2,
        58,
    )
}

fn store_start_button_rect(base: Rect) -> Rect {
    Rect::new(base.x + base.width / 2 - 60, base.y + 190, 120, 34)
}

/// The first `count` bytes of an ASCII string.
fn clip_text(value: &str, count: usize) -> &str {
    value.get(..count.min(value.len())).unwrap_or(value)
}

/// Draws `value` wrapped to `width_chars` columns; returns the lines used.
#[allow(clippy::too_many_arguments)]
fn draw_wrapped(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    origin: Point,
    width_chars: usize,
    max_lines: usize,
    value: &str,
    height: i32,
    color: Color,
) -> usize {
    let mut rest = value;
    let mut lines = 0;
    while !rest.is_empty() && lines < max_lines {
        let mut cut = rest.len().min(width_chars);
        if cut < rest.len()
            && let Some(space) = rest[..cut].rfind(' ')
        {
            cut = space;
        }
        let (line, tail) = rest.split_at(cut);
        text(
            painter,
            layout,
            font,
            Point::new(origin.x, origin.y + lines as i32 * (height + 4)),
            line,
            height,
            color,
        );
        lines += 1;
        rest = tail.trim_start();
    }
    lines
}

/// A rounded app icon: a coloured tile with the app's initial.
fn draw_store_icon(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    bounds: Rect,
    name: &str,
    seed: usize,
) {
    if let Some(icon) = crate::store::icon_for_name(name) {
        let physical = layout.rect(bounds);
        painter.draw_rgba_scaled(
            physical,
            icon,
            crate::store::ICON_SIZE,
            crate::store::ICON_SIZE,
        );
        return;
    }
    painter.fill_rounded_rect(
        layout.rect(bounds),
        layout.radii(CornerRadii::all(bounds.width / 4)),
        tile_color(seed),
    );
    if let Some(initial) = name.get(..1) {
        centered_text(
            painter,
            layout,
            font,
            bounds,
            initial,
            bounds.height * 2 / 5,
            Color::rgb(255, 255, 255),
        );
    }
}

/// The app store (styled after the mockup): a search grid of apps, and a
/// page per app with Get, size, age rating and similar apps. It sits directly
/// on the window's glass (no backdrop of its own), like the Shell window.
fn draw_store(
    painter: &mut Painter<'_>,
    layout: Layout,
    ui_font: RasterFont,
    _mono_font: RasterFont,
    _state: &DesktopState,
) {
    use crate::store::{CATALOG, GRID_COLUMNS, GRID_ROWS, StoreTab};
    let base = window_base_rect(DesktopApp::Store);
    let store = crate::store::get();
    let ink = Color::rgb(18, 23, 26);
    let muted = Color::rgb(66, 82, 94);
    let link = Color::rgb(28, 104, 186);
    let accent = Rgba::opaque(64, 140, 205);
    // Frosted white cards on the glass.
    let card = Rgba::new(255, 255, 255, 120);
    let pill = Rgba::new(255, 255, 255, 175);
    let edge = Rgba::new(255, 255, 255, 170);
    let running = crate::svm::linux_agent_ready();
    let stroke = layout.scale.logical(1).max(1) as u8;

    if let Some(index) = store.detail {
        let entry = &CATALOG[index];
        text(
            painter,
            layout,
            ui_font,
            Point::new(base.x + 22, base.y + 38),
            "< Store",
            12,
            link,
        );
        draw_store_icon(
            painter,
            layout,
            ui_font,
            Rect::new(base.x + 22, base.y + 58, 64, 64),
            entry.name,
            index,
        );
        text(
            painter,
            layout,
            ui_font,
            Point::new(base.x + 98, base.y + 58),
            entry.name,
            20,
            ink,
        );
        if entry.verified {
            let name_width = ui_font.text_width(entry.name, layout.scale.logical(20));
            let badge_x = base.x + 98 + pixel_to_logical(layout, layout.offset.x + name_width) + 8;
            let badge = Rect::new(badge_x, base.y + 62, 16, 16);
            painter.fill_rounded_rect(
                layout.rect(badge),
                layout.radii(CornerRadii::all(8)),
                Rgba::opaque(58, 150, 230),
            );
            centered_text(
                painter,
                layout,
                ui_font,
                badge,
                "v",
                11,
                Color::rgb(255, 255, 255),
            );
        }
        text(
            painter,
            layout,
            ui_font,
            Point::new(base.x + 98, base.y + 84),
            entry.blurb,
            12,
            muted,
        );
        let installing = store.installing == Some(index);
        let have = store.installed_index(entry).is_some();
        let get = store_get_rect(base);
        let (label, fill, tone) = if have {
            ("Open", accent, Color::rgb(255, 255, 255))
        } else if installing {
            ("Installing", Rgba::new(255, 255, 255, 200), link)
        } else {
            ("Get", Rgba::opaque(44, 52, 62), Color::rgb(245, 247, 249))
        };
        painter.fill_rounded_rect(layout.rect(get), layout.radii(CornerRadii::all(12)), fill);
        centered_text(painter, layout, ui_font, get, label, 12, tone);
        // Size / age rating group.
        let group = store_group_rect(base);
        painter.fill_rounded_rect(layout.rect(group), layout.radii(CornerRadii::all(30)), card);
        painter.stroke_rounded_rect(
            layout.rect(group),
            layout.radii(CornerRadii::all(30)),
            stroke,
            edge,
        );
        for (slot, (title, value)) in [("Size", entry.size), ("Age Rating", entry.age)]
            .into_iter()
            .enumerate()
        {
            let column = Rect::new(group.x + 14 + slot as i32 * 122, group.y + 8, 114, 18);
            centered_text(painter, layout, ui_font, column, title, 12, ink);
            let value_pill = Rect::new(column.x + 12, group.y + 32, 90, 22);
            painter.fill_rounded_rect(
                layout.rect(value_pill),
                layout.radii(CornerRadii::all(11)),
                pill,
            );
            centered_text(painter, layout, ui_font, value_pill, value, 12, ink);
        }
        // Longer description.
        let about = store_about_rect(base);
        painter.fill_rounded_rect(layout.rect(about), layout.radii(CornerRadii::all(18)), card);
        painter.stroke_rounded_rect(
            layout.rect(about),
            layout.radii(CornerRadii::all(18)),
            stroke,
            edge,
        );
        let used = draw_wrapped(
            painter,
            layout,
            ui_font,
            Point::new(about.x + 16, about.y + 14),
            (about.width * 36 / 250).max(20) as usize,
            7,
            entry.long,
            11,
            ink,
        );
        let mut y = about.y + 14 + used as i32 * 15 + 8;
        for (label, value) in [
            ("Category", entry.category),
            ("From", "Flathub (Flatpak)"),
            ("ID", entry.flatpak),
        ] {
            text(
                painter,
                layout,
                ui_font,
                Point::new(about.x + 16, y),
                label,
                10,
                muted,
            );
            text(
                painter,
                layout,
                ui_font,
                Point::new(about.x + 76, y),
                clip_text(value, 28),
                10,
                ink,
            );
            y += 15;
        }
        // Similar apps.
        let similar = store_similar_panel(base);
        painter.fill_rounded_rect(
            layout.rect(similar),
            layout.radii(CornerRadii::all(24)),
            card,
        );
        painter.stroke_rounded_rect(
            layout.rect(similar),
            layout.radii(CornerRadii::all(24)),
            stroke,
            edge,
        );
        centered_text(
            painter,
            layout,
            ui_font,
            Rect::new(similar.x, similar.y + 8, similar.width, 20),
            "Similar Apps",
            13,
            ink,
        );
        for (slot, other) in store.similar(index).into_iter().enumerate() {
            let Some(entry) = CATALOG.get(other) else {
                continue;
            };
            let cell = store_similar_rect(base, slot);
            draw_store_icon(
                painter,
                layout,
                ui_font,
                Rect::new(cell.x + 2, cell.y + 6, 44, 44),
                entry.name,
                other,
            );
            text(
                painter,
                layout,
                ui_font,
                Point::new(cell.x + 52, cell.y + 12),
                clip_text(entry.name, 11),
                11,
                ink,
            );
            text(
                painter,
                layout,
                ui_font,
                Point::new(cell.x + 52, cell.y + 28),
                clip_text(entry.blurb, 16),
                9,
                muted,
            );
        }
        draw_store_status(painter, layout, ui_font, base, store.install, muted, false);
        return;
    }

    // Grid: toggle, search pill, status.
    let toggle = store_toggle_rect(base);
    painter.fill_rounded_rect(
        layout.rect(toggle),
        layout.radii(CornerRadii::all(13)),
        pill,
    );
    painter.stroke_rounded_rect(
        layout.rect(toggle),
        layout.radii(CornerRadii::all(13)),
        stroke,
        edge,
    );
    centered_text(
        painter,
        layout,
        ui_font,
        toggle,
        if store.tab == StoreTab::Installed {
            "Installed"
        } else {
            "Discover"
        },
        12,
        ink,
    );
    let search = store_search_rect(base);
    painter.fill_rounded_rect(
        layout.rect(search),
        layout.radii(CornerRadii::all(13)),
        Rgba::opaque(248, 249, 251),
    );
    let query = store.query();
    if query.is_empty() {
        centered_text(
            painter,
            layout,
            ui_font,
            search,
            "Search for an App",
            11,
            Color::rgb(120, 126, 134),
        );
    } else {
        text(
            painter,
            layout,
            ui_font,
            Point::new(search.x + 14, search.y + 6),
            query,
            13,
            Color::rgb(28, 32, 38),
        );
    }
    draw_store_status(
        painter,
        layout,
        ui_font,
        base,
        store.install,
        muted,
        !running,
    );

    let shown = store.shown();
    if store.tab == StoreTab::Installed && !running {
        centered_text(
            painter,
            layout,
            ui_font,
            Rect::new(base.x, base.y + 130, base.width, 24),
            "Linux isn't running",
            17,
            ink,
        );
        centered_text(
            painter,
            layout,
            ui_font,
            Rect::new(base.x, base.y + 158, base.width, 20),
            "Start it to see the apps you have.",
            12,
            muted,
        );
        let button = store_start_button_rect(base);
        painter.fill_rounded_rect(
            layout.rect(button),
            layout.radii(CornerRadii::all(17)),
            accent,
        );
        centered_text(
            painter,
            layout,
            ui_font,
            button,
            "Start Linux",
            13,
            Color::rgb(255, 255, 255),
        );
        return;
    }
    if shown == 0 {
        centered_text(
            painter,
            layout,
            ui_font,
            Rect::new(base.x, base.y + 160, base.width, 22),
            if query.is_empty() {
                "No apps found yet"
            } else {
                "No apps match your search"
            },
            15,
            muted,
        );
        return;
    }
    for slot in 0..GRID_COLUMNS * GRID_ROWS {
        let (column, row) = (slot % GRID_COLUMNS, slot / GRID_COLUMNS);
        let position = (store.scroll + row) * GRID_COLUMNS + column;
        let Some(index) = store.shown_index(position) else {
            break;
        };
        let cell = store_cell_rect(base, column, row);
        let (name, sub) = match store.tab {
            StoreTab::Installed => {
                let app = store.app(index);
                (
                    app.map_or("", |app| app.name_str()),
                    app.map_or("", |app| app.category_str()),
                )
            }
            StoreTab::Discover => (CATALOG[index].name, CATALOG[index].blurb),
        };
        if position == store.selected && !store.query().is_empty() {
            painter.fill_rounded_rect(
                layout.rect(cell),
                layout.radii(CornerRadii::all(12)),
                Rgba::new(255, 255, 255, 90),
            );
        }
        draw_store_icon(
            painter,
            layout,
            ui_font,
            Rect::new(cell.x + 6, cell.y + 7, 44, 44),
            name,
            index + name.len(),
        );
        text(
            painter,
            layout,
            ui_font,
            Point::new(cell.x + 58, cell.y + 12),
            clip_text(name, 16),
            14,
            ink,
        );
        text(
            painter,
            layout,
            ui_font,
            Point::new(cell.x + 58, cell.y + 31),
            clip_text(sub, 26),
            10,
            muted,
        );
    }
}

/// The one-line install status at the top right of the store.
fn draw_store_status(
    painter: &mut Painter<'_>,
    layout: Layout,
    ui_font: RasterFont,
    base: Rect,
    install: crate::store::InstallState,
    muted: Color,
    linux_off: bool,
) {
    use crate::store::InstallState;
    let message = match install {
        InstallState::Installing => "Installing... this can take a while",
        InstallState::Done => "Done - see it under Installed",
        InstallState::Failed => "Install failed - check the network",
        InstallState::Idle if linux_off => "Linux is off - Get starts it",
        InstallState::Idle => "",
    };
    text(
        painter,
        layout,
        ui_font,
        Point::new(base.x + base.width - 178, base.y + 47),
        message,
        10,
        muted,
    );
}

fn draw_browser_legacy_body(
    painter: &mut Painter<'_>,
    layout: Layout,
    ui_font: RasterFont,
    mono_font: RasterFont,
    state: &DesktopState,
) {
    let _ = state.url_active;
    painter.fill_rounded_rect(
        layout.rect(Rect::new(133, 125, 9, 9)),
        layout.radii(CornerRadii::all(5)),
        browser_phase_dot(state.browser_phase),
    );
    let mut status: shell::Text<96> = shell::Text::new();
    if state.browser_phase == BrowserPhase::Loaded {
        let _ = write!(
            status,
            "{}  -  HTTP {}  -  {} bytes",
            browser_phase_headline(state.browser_phase),
            state.browser_http_status,
            state.browser_http_bytes
        );
    } else {
        let _ = write!(status, "{}", browser_phase_headline(state.browser_phase));
    }
    text(
        painter,
        layout,
        ui_font,
        Point::new(149, 121),
        status.as_str(),
        14,
        Color::rgb(15, 49, 61),
    );
    let card = Rect::new(132, 140, 493, 158);
    let card_fill = if browser_phase_is_error(state.browser_phase) {
        Rgba::new(255, 244, 242, 235)
    } else {
        Rgba::new(252, 253, 254, 235)
    };
    painter.fill_rounded_rect(
        layout.rect(card),
        layout.radii(CornerRadii::all(14)),
        card_fill,
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(card.x, card.y, 5, card.height)),
        layout.radii(CornerRadii::new(14, 0, 0, 14)),
        browser_phase_dot(state.browser_phase),
    );
    painter.stroke_rounded_rect(
        layout.rect(card),
        layout.radii(CornerRadii::all(14)),
        layout.scale.logical(1).max(1) as u8,
        Rgba::new(120, 140, 148, 90),
    );
    let clip = layout.rect(card);
    let previous_clip = painter.clip();
    painter.set_clip(clip);
    let title = core::str::from_utf8(&state.browser_title[..state.browser_title_len]).unwrap_or("");
    let body_top = if title.is_empty() {
        card.y + 16
    } else {
        text(
            painter,
            layout,
            ui_font,
            Point::new(card.x + 16, card.y + 14),
            title,
            16,
            Color::rgb(17, 49, 59),
        );
        card.y + 42
    };
    if state.browser_phase == BrowserPhase::Loaded {
        let char_width = mono_font.text_width("M", layout.scale.logical(11)).max(1);
        let margin = layout.scale.logical(32);
        let chars_per_line = ((clip.width - margin).max(char_width) / char_width).max(4) as usize;
        let mut lines: [&str; 8] = [""; 8];
        let count = wrap_monospace(state.browser_body.as_str(), chars_per_line, &mut lines);
        if count == 0 {
            text(
                painter,
                layout,
                mono_font,
                Point::new(card.x + 16, body_top),
                "(no readable text on this page)",
                12,
                Color::rgb(90, 106, 112),
            );
        } else {
            for (row, line) in lines[..count].iter().enumerate() {
                text(
                    painter,
                    layout,
                    mono_font,
                    Point::new(card.x + 16, body_top + row as i32 * 16),
                    line,
                    12,
                    Color::rgb(38, 71, 80),
                );
            }
        }
    } else {
        text(
            painter,
            layout,
            mono_font,
            Point::new(card.x + 16, body_top),
            browser_phase_detail(state.browser_phase),
            12,
            if browser_phase_is_error(state.browser_phase) {
                Color::rgb(150, 60, 50)
            } else {
                Color::rgb(90, 106, 112)
            },
        );
    }
    painter.set_clip(previous_clip);
    let mut trail: shell::Text<256> = shell::Text::new();
    let _ = write!(
        trail,
        "{}{}  -  {}.{}.{}.{}",
        state.browser_host_str(),
        state.browser_path_str(),
        state.browser_address[0],
        state.browser_address[1],
        state.browser_address[2],
        state.browser_address[3]
    );
    text(
        painter,
        layout,
        mono_font,
        Point::new(133, 306),
        trail.as_str(),
        10,
        Color::rgb(70, 90, 98),
    );
}

fn small_button(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    bounds: Rect,
    label: &str,
) {
    painter.fill_rounded_rect(
        layout.rect(bounds),
        layout.radii(CornerRadii::all(8)),
        Rgba::new(223, 242, 251, 170),
    );
    painter.stroke_rounded_rect(
        layout.rect(bounds),
        layout.radii(CornerRadii::all(8)),
        layout.scale.logical(1).max(1) as u8,
        Rgba::new(120, 140, 148, 110),
    );
    centered_text(
        painter,
        layout,
        font,
        bounds,
        label,
        12,
        Color::rgb(15, 49, 61),
    );
}

fn format_bytes(size: u64, out: &mut shell::Text<24>) {
    if size < 1024 {
        let _ = write!(out, "{size} B");
    } else if size < 1024 * 1024 {
        let _ = write!(out, "{} KB", size / 1024);
    } else {
        let _ = write!(out, "{} MB", size / (1024 * 1024));
    }
}

fn draw_entry_icon(painter: &mut Painter<'_>, layout: Layout, origin: Point, is_dir: bool) {
    if is_dir {
        painter.fill_rounded_rect(
            layout.rect(Rect::new(origin.x, origin.y, 15, 12)),
            layout.radii(CornerRadii::new(2, 4, 4, 4)),
            Rgba::new(76, 163, 224, 230),
        );
    } else {
        painter.fill_rounded_rect(
            layout.rect(Rect::new(origin.x + 2, origin.y, 11, 14)),
            layout.radii(CornerRadii::all(2)),
            Rgba::new(150, 170, 178, 220),
        );
    }
}

fn draw_entry_icon_large(painter: &mut Painter<'_>, layout: Layout, origin: Point, is_dir: bool) {
    if is_dir {
        painter.fill_rounded_rect(
            layout.rect(Rect::new(origin.x, origin.y, 26, 20)),
            layout.radii(CornerRadii::new(3, 7, 7, 7)),
            Rgba::new(76, 163, 224, 235),
        );
        painter.fill_rounded_rect(
            layout.rect(Rect::new(origin.x, origin.y - 4, 12, 6)),
            layout.radii(CornerRadii::new(3, 3, 0, 0)),
            Rgba::new(76, 163, 224, 235),
        );
    } else {
        painter.fill_rounded_rect(
            layout.rect(Rect::new(origin.x + 3, origin.y - 2, 20, 24)),
            layout.radii(CornerRadii::all(3)),
            Rgba::new(255, 255, 255, 235),
        );
        painter.stroke_rounded_rect(
            layout.rect(Rect::new(origin.x + 3, origin.y - 2, 20, 24)),
            layout.radii(CornerRadii::all(3)),
            layout.scale.logical(1).max(1) as u8,
            Rgba::new(150, 170, 178, 220),
        );
        for row in 0..3 {
            painter.fill_rounded_rect(
                layout.rect(Rect::new(origin.x + 7, origin.y + 3 + row * 5, 12, 2)),
                layout.radii(CornerRadii::all(1)),
                Rgba::new(160, 178, 186, 220),
            );
        }
    }
}

fn draw_files(
    painter: &mut Painter<'_>,
    layout: Layout,
    ui_font: RasterFont,
    mono_font: RasterFont,
    state: &DesktopState,
    outer: Rect,
) {
    if state.files_mode == FilesMode::NamingFolder {
        draw_name_prompt(
            painter,
            layout,
            ui_font,
            outer,
            "New folder name",
            state.name_input.as_str(),
        );
        return;
    }
    let content_x = outer.x + 10;
    let content_y = outer.y + 36;
    let content_right = outer.x + outer.width - 10;
    let content_bottom = outer.y + outer.height - 10;

    let sidebar_width = 106;
    let sidebar = Rect::new(
        content_x,
        content_y,
        sidebar_width,
        content_bottom - content_y,
    );
    painter.fill_rounded_rect(
        layout.rect(sidebar),
        layout.radii(CornerRadii::all(14)),
        Rgba::opaque(228, 241, 250),
    );
    text(
        painter,
        layout,
        ui_font,
        Point::new(sidebar.x + 12, sidebar.y + 8),
        "Places",
        11,
        Color::rgb(60, 95, 110),
    );
    let quick_labels = ["Home", "Data", "Temp"];
    let quick_paths = ["/", "/data", "/tmp"];
    for (index, label) in quick_labels.iter().enumerate() {
        let bounds = Rect::new(
            sidebar.x + 8,
            sidebar.y + 28 + index as i32 * 32,
            sidebar_width - 16,
            26,
        );
        let active = state.files_path.as_str() == quick_paths[index];
        painter.fill_rounded_rect(
            layout.rect(bounds),
            layout.radii(CornerRadii::all(8)),
            if active {
                Rgba::new(110, 180, 228, 220)
            } else {
                Rgba::new(255, 255, 255, 95)
            },
        );
        text(
            painter,
            layout,
            ui_font,
            Point::new(bounds.x + 10, bounds.y + 5),
            label,
            12,
            if active {
                Color::rgb(8, 28, 42)
            } else {
                Color::rgb(35, 70, 85)
            },
        );
    }
    small_button(
        painter,
        layout,
        ui_font,
        Rect::new(sidebar.x + 8, sidebar.y + 148, sidebar_width - 16, 26),
        "New folder",
    );
    small_button(
        painter,
        layout,
        ui_font,
        Rect::new(sidebar.x + 8, sidebar.y + 182, sidebar_width - 16, 26),
        "Delete",
    );

    let main_x = sidebar.x + sidebar_width + 12;
    let main_width = content_right - main_x;
    small_button(
        painter,
        layout,
        ui_font,
        Rect::new(main_x, content_y, 44, 24),
        "Up",
    );
    let mut path_line: shell::Text<96> = shell::Text::new();
    let _ = path_line.push_str_checked(state.files_path.as_str());
    text(
        painter,
        layout,
        mono_font,
        Point::new(main_x + 52, content_y + 5),
        path_line.as_str(),
        12,
        Color::rgb(25, 65, 76),
    );

    let grid_top = content_y + 32;
    let status_y = content_bottom - 14;
    let grid_bounds = Rect::new(main_x, grid_top, main_width, (status_y - 4) - grid_top);
    painter.fill_rounded_rect(
        layout.rect(grid_bounds),
        layout.radii(CornerRadii::all(14)),
        Rgba::opaque(250, 252, 253),
    );
    let clip = layout.rect(grid_bounds);
    let previous_clip = painter.clip();
    painter.set_clip(clip);
    let cell_width = main_width / FILES_GRID_COLUMNS;
    let cell_height = grid_bounds.height / FILES_GRID_ROWS;
    for index in 0..state.files_entry_count {
        let row = state.files_entries[index];
        let column = index as i32 % FILES_GRID_COLUMNS;
        let line = index as i32 / FILES_GRID_COLUMNS;
        let bounds = Rect::new(
            main_x + column * cell_width,
            grid_top + line * cell_height,
            cell_width - 8,
            cell_height - 6,
        );
        if index == state.files_selected {
            painter.fill_rounded_rect(
                layout.rect(bounds),
                layout.radii(CornerRadii::all(10)),
                Rgba::new(150, 205, 240, 150),
            );
        }
        draw_entry_icon_large(
            painter,
            layout,
            Point::new(bounds.x + bounds.width / 2 - 12, bounds.y + 8),
            row.is_dir,
        );
        let mut label: shell::Text<20> = shell::Text::new();
        let full = row.name_str();
        if full.len() > 13 {
            let _ = label.push_str_checked(&full[..12]);
            let _ = label.push_str_checked(".");
        } else {
            let _ = label.push_str_checked(full);
        }
        centered_text(
            painter,
            layout,
            ui_font,
            Rect::new(bounds.x, bounds.y + cell_height - 26, bounds.width, 16),
            label.as_str(),
            11,
            Color::rgb(20, 45, 55),
        );
    }
    painter.set_clip(previous_clip);
    if state.files_entry_count == 0 {
        text(
            painter,
            layout,
            mono_font,
            Point::new(main_x + 4, grid_top + 8),
            "(empty)",
            12,
            Color::rgb(90, 106, 112),
        );
    }
    if let Some(row) = state.files_entries.get(state.files_selected).copied() {
        let mut line: shell::Text<96> = shell::Text::new();
        if row.is_dir {
            let _ = write!(line, "{}  -  folder", row.name_str());
        } else {
            let mut size: shell::Text<24> = shell::Text::new();
            format_bytes(row.size, &mut size);
            let _ = write!(line, "{}  -  {}", row.name_str(), size.as_str());
        }
        text(
            painter,
            layout,
            mono_font,
            Point::new(main_x, status_y),
            line.as_str(),
            11,
            Color::rgb(38, 71, 80),
        );
    } else if state.files_overflow {
        let mut line: shell::Text<96> = shell::Text::new();
        let _ = write!(
            line,
            "showing first {} items - more exist on disk",
            state.files_entry_count
        );
        text(
            painter,
            layout,
            mono_font,
            Point::new(main_x, status_y),
            line.as_str(),
            11,
            Color::rgb(160, 110, 40),
        );
    } else if !state.files_status.as_str().is_empty() {
        text(
            painter,
            layout,
            mono_font,
            Point::new(main_x, status_y),
            state.files_status.as_str(),
            11,
            Color::rgb(15, 49, 61),
        );
    } else {
        text(
            painter,
            layout,
            mono_font,
            Point::new(main_x, status_y),
            "Select an item",
            11,
            Color::rgb(120, 135, 140),
        );
    }
}

fn draw_name_prompt(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    outer: Rect,
    label: &str,
    typed: &str,
) {
    let card = Rect::new(
        outer.x + 20,
        outer.y + outer.height / 2 - 50,
        outer.width - 40,
        100,
    );
    painter.fill_rounded_rect(
        layout.rect(card),
        layout.radii(CornerRadii::all(18)),
        Rgba::new(252, 253, 254, 245),
    );
    text(
        painter,
        layout,
        font,
        Point::new(card.x + 22, card.y + 18),
        label,
        14,
        Color::rgb(15, 49, 61),
    );
    let field = Rect::new(card.x + 22, card.y + 46, card.width - 44, 34);
    painter.fill_rounded_rect(
        layout.rect(field),
        layout.radii(CornerRadii::all(10)),
        Rgba::new(244, 249, 250, 220),
    );
    painter.stroke_rounded_rect(
        layout.rect(field),
        layout.radii(CornerRadii::all(10)),
        layout.scale.logical(2).max(1) as u8,
        Rgba::new(31, 108, 138, 220),
    );
    let mut shown: shell::Text<40> = shell::Text::new();
    let _ = shown.push_str_checked(typed);
    let _ = shown.push_byte(b'_');
    text(
        painter,
        layout,
        font,
        Point::new(field.x + 14, field.y + 8),
        shown.as_str(),
        14,
        Color::rgb(25, 45, 52),
    );
    text(
        painter,
        layout,
        font,
        Point::new(card.x + 22, card.y + 84),
        "Enter to confirm - Escape to cancel",
        11,
        Color::rgb(90, 106, 112),
    );
}

fn draw_notes(
    painter: &mut Painter<'_>,
    layout: Layout,
    ui_font: RasterFont,
    mono_font: RasterFont,
    state: &DesktopState,
    outer: Rect,
) {
    if state.notes_mode == NotesMode::Naming {
        draw_name_prompt(
            painter,
            layout,
            ui_font,
            outer,
            "New note name",
            state.name_input.as_str(),
        );
        return;
    }
    let content_x = outer.x + 14;
    let content_y = outer.y + 36;
    let content_right = outer.x + outer.width - 14;
    let content_bottom = outer.y + outer.height - 14;
    if state.notes_mode == NotesMode::Editing {
        text(
            painter,
            layout,
            ui_font,
            Point::new(content_x, content_y),
            state.notes_current_name.as_str(),
            16,
            Color::rgb(90, 62, 20),
        );
        small_button(
            painter,
            layout,
            ui_font,
            Rect::new(content_right - 56, content_y - 4, 56, 26),
            "Save",
        );
        let card = Rect::new(
            content_x,
            content_y + 32,
            content_right - content_x,
            content_bottom - content_y - 32,
        );
        painter.fill_rounded_rect(
            layout.rect(card),
            layout.radii(CornerRadii::all(10)),
            Rgba::new(255, 250, 236, 245),
        );
        let clip = layout.rect(card);
        let previous_clip = painter.clip();
        painter.set_clip(clip);
        for line_index in 0..12 {
            painter.fill_rounded_rect(
                layout.rect(Rect::new(
                    card.x + 10,
                    card.y + 30 + line_index * 20,
                    card.width - 20,
                    1,
                )),
                CornerRadii::all(0),
                Rgba::new(224, 205, 160, 90),
            );
        }
        let char_width = mono_font.text_width("M", layout.scale.logical(13)).max(1);
        let margin = layout.scale.logical(24);
        let chars_per_line = ((clip.width - margin).max(char_width) / char_width).max(4) as usize;
        let mut lines: [&str; 13] = [""; 13];
        let count = wrap_monospace(state.notes_content.as_str(), chars_per_line, &mut lines);
        for (row, line) in lines[..count].iter().enumerate() {
            text(
                painter,
                layout,
                mono_font,
                Point::new(card.x + 12, card.y + 16 + row as i32 * 20),
                line,
                13,
                Color::rgb(70, 50, 20),
            );
        }
        painter.set_clip(previous_clip);
        if !state.notes_status.as_str().is_empty() {
            text(
                painter,
                layout,
                mono_font,
                Point::new(content_x, content_bottom + 2),
                state.notes_status.as_str(),
                11,
                Color::rgb(90, 62, 20),
            );
        }
        return;
    }
    small_button(
        painter,
        layout,
        ui_font,
        Rect::new(content_x, content_y, content_right - content_x, 28),
        "+ New note",
    );
    let list_top = content_y + 38;
    let status_y = content_bottom - 4;
    let row_height = ((status_y - 16) - list_top) / NOTES_MAX_ENTRIES as i32;
    let list_bounds = Rect::new(
        content_x,
        list_top,
        content_right - content_x,
        (status_y - 16) - list_top,
    );
    painter.fill_rounded_rect(
        layout.rect(list_bounds),
        layout.radii(CornerRadii::all(10)),
        Rgba::opaque(253, 249, 240),
    );
    let list_clip = layout.rect(list_bounds);
    let previous_clip = painter.clip();
    painter.set_clip(list_clip);
    for index in 0..state.notes_entry_count {
        let row = state.notes_entries[index];
        let bounds = Rect::new(
            content_x,
            list_top + index as i32 * row_height,
            content_right - content_x,
            row_height - 6,
        );
        painter.fill_rounded_rect(
            layout.rect(bounds),
            layout.radii(CornerRadii::new(4, 10, 10, 4)),
            Rgba::new(255, 248, 232, 210),
        );
        painter.fill_rounded_rect(
            layout.rect(Rect::new(bounds.x, bounds.y, 5, bounds.height)),
            layout.radii(CornerRadii::new(4, 0, 0, 4)),
            Rgba::new(224, 168, 60, 230),
        );
        text(
            painter,
            layout,
            ui_font,
            Point::new(bounds.x + 16, bounds.y + 6),
            row.name_str(),
            13,
            Color::rgb(70, 50, 20),
        );
        let mut size: shell::Text<24> = shell::Text::new();
        format_bytes(row.size, &mut size);
        text(
            painter,
            layout,
            mono_font,
            Point::new(bounds.x + 16, bounds.y + 24),
            size.as_str(),
            10,
            Color::rgb(140, 110, 70),
        );
        let close = Rect::new(bounds.x + bounds.width - 30, bounds.y + 10, 20, 20);
        painter.fill_rounded_rect(
            layout.rect(close),
            layout.radii(CornerRadii::all(10)),
            Rgba::new(210, 120, 100, 200),
        );
        centered_text(
            painter,
            layout,
            ui_font,
            close,
            "X",
            11,
            Color::rgb(255, 250, 246),
        );
    }
    painter.set_clip(previous_clip);
    if state.notes_entry_count == 0 {
        text(
            painter,
            layout,
            mono_font,
            Point::new(content_x + 4, list_top + 10),
            "No notes yet",
            12,
            Color::rgb(150, 120, 80),
        );
    }
    if state.notes_overflow {
        let mut line: shell::Text<96> = shell::Text::new();
        let _ = write!(
            line,
            "showing first {} notes - more exist on disk",
            state.notes_entry_count
        );
        text(
            painter,
            layout,
            mono_font,
            Point::new(content_x, status_y),
            line.as_str(),
            10,
            Color::rgb(160, 110, 40),
        );
    } else if !state.notes_status.as_str().is_empty() {
        text(
            painter,
            layout,
            mono_font,
            Point::new(content_x, status_y),
            state.notes_status.as_str(),
            11,
            Color::rgb(90, 62, 20),
        );
    }
}

fn draw_trash(
    painter: &mut Painter<'_>,
    layout: Layout,
    ui_font: RasterFont,
    mono_font: RasterFont,
    state: &DesktopState,
    outer: Rect,
) {
    let content_x = outer.x + 14;
    let content_y = outer.y + 36;
    let content_right = outer.x + outer.width - 14;
    let content_bottom = outer.y + outer.height - 14;
    text(
        painter,
        layout,
        ui_font,
        Point::new(content_x, content_y),
        "Deleted items",
        13,
        Color::rgb(80, 70, 70),
    );
    small_button(
        painter,
        layout,
        ui_font,
        Rect::new(content_right - 90, content_y - 4, 90, 24),
        "Empty all",
    );
    let list_top = content_y + 30;
    let status_y = content_bottom - 4;
    let row_height = ((status_y - 16) - list_top) / TRASH_MAX_ENTRIES as i32;
    let list_bounds = Rect::new(
        content_x,
        list_top,
        content_right - content_x,
        (status_y - 16) - list_top,
    );
    painter.fill_rounded_rect(
        layout.rect(list_bounds),
        layout.radii(CornerRadii::all(10)),
        Rgba::opaque(248, 244, 244),
    );
    let list_clip = layout.rect(list_bounds);
    let previous_clip = painter.clip();
    painter.set_clip(list_clip);
    for index in 0..state.trash_entry_count {
        let row = state.trash_entries[index];
        let bounds = Rect::new(
            content_x,
            list_top + index as i32 * row_height,
            content_right - content_x,
            row_height - 4,
        );
        painter.fill_rounded_rect(
            layout.rect(bounds),
            layout.radii(CornerRadii::all(8)),
            Rgba::opaque(255, 255, 255),
        );
        draw_entry_icon(
            painter,
            layout,
            Point::new(bounds.x, bounds.y + 4),
            row.is_dir,
        );
        let mut label: shell::Text<28> = shell::Text::new();
        let full = row.name_str();
        if full.len() > 16 {
            let _ = label.push_str_checked(&full[..14]);
            let _ = label.push_str_checked(".");
        } else {
            let _ = label.push_str_checked(full);
        }
        text(
            painter,
            layout,
            ui_font,
            Point::new(bounds.x + 22, bounds.y + 3),
            label.as_str(),
            12,
            Color::rgb(80, 65, 65),
        );
        let restore = Rect::new(bounds.x + bounds.width - 52, bounds.y, 24, 22);
        let delete = Rect::new(bounds.x + bounds.width - 24, bounds.y, 24, 22);
        painter.fill_rounded_rect(
            layout.rect(restore),
            layout.radii(CornerRadii::all(6)),
            Rgba::new(150, 190, 160, 180),
        );
        centered_text(
            painter,
            layout,
            ui_font,
            restore,
            "R",
            11,
            Color::rgb(30, 55, 35),
        );
        painter.fill_rounded_rect(
            layout.rect(delete),
            layout.radii(CornerRadii::all(6)),
            Rgba::new(200, 130, 120, 190),
        );
        centered_text(
            painter,
            layout,
            ui_font,
            delete,
            "X",
            12,
            Color::rgb(60, 20, 15),
        );
    }
    painter.set_clip(previous_clip);
    if state.trash_entry_count == 0 {
        text(
            painter,
            layout,
            mono_font,
            Point::new(content_x, list_top + 6),
            "Trash is empty",
            12,
            Color::rgb(130, 115, 115),
        );
    }
    if state.trash_overflow {
        let mut line: shell::Text<96> = shell::Text::new();
        let _ = write!(
            line,
            "showing first {} items - more exist on disk",
            state.trash_entry_count
        );
        text(
            painter,
            layout,
            mono_font,
            Point::new(content_x, status_y),
            line.as_str(),
            10,
            Color::rgb(150, 90, 90),
        );
    } else if !state.trash_status.as_str().is_empty() {
        text(
            painter,
            layout,
            mono_font,
            Point::new(content_x, status_y),
            state.trash_status.as_str(),
            11,
            Color::rgb(90, 70, 70),
        );
    }
}

fn browser_phase_headline(phase: BrowserPhase) -> &'static str {
    match phase {
        BrowserPhase::Editing => "Type a URL, press Enter",
        BrowserPhase::Loading => "Loading",
        BrowserPhase::Loaded => "Page loaded",
        BrowserPhase::DnsFailed => "DNS lookup failed",
        BrowserPhase::ConnectFailed => "Connection failed",
        BrowserPhase::HttpError => "Server returned an error",
        BrowserPhase::HttpsUnsupported => "HTTPS is not supported yet",
        BrowserPhase::BadUrl => "That URL is not valid",
    }
}

fn browser_phase_is_error(phase: BrowserPhase) -> bool {
    matches!(
        phase,
        BrowserPhase::DnsFailed
            | BrowserPhase::ConnectFailed
            | BrowserPhase::HttpError
            | BrowserPhase::HttpsUnsupported
            | BrowserPhase::BadUrl
    )
}

fn browser_phase_dot(phase: BrowserPhase) -> Rgba {
    match phase {
        BrowserPhase::Loaded => Rgba::new(70, 180, 120, 230),
        BrowserPhase::Loading => Rgba::new(80, 140, 210, 230),
        BrowserPhase::Editing => Rgba::new(150, 165, 175, 200),
        _ => Rgba::new(200, 80, 70, 230),
    }
}

fn browser_phase_detail(phase: BrowserPhase) -> &'static str {
    match phase {
        BrowserPhase::Editing => "Type a URL above and press Enter to load it.",
        BrowserPhase::Loading => "Resolving and fetching the page...",
        BrowserPhase::Loaded => "",
        BrowserPhase::DnsFailed => "Could not resolve this host name to an address.",
        BrowserPhase::ConnectFailed => "The TCP connection to the server failed or timed out.",
        BrowserPhase::HttpError => "The server responded, but with an error status.",
        BrowserPhase::HttpsUnsupported => {
            "This browser only speaks plain HTTP - try the http:// version of the site."
        }
        BrowserPhase::BadUrl => "That does not look like a valid host name and path.",
    }
}

fn setup_card_style() -> FrostStyle {
    FrostStyle {
        blur_radius: 16,
        saturation_percent: 106,
        brightness_percent: 94,
        tint: Rgba::new(12, 12, 12, 51),
        border: Rgba::transparent(),
        border_width: 0,
        inner_highlight: Rgba::new(255, 255, 255, 22),
        inner_shadow: Rgba::new(0, 0, 0, 64),
        inner_shadow_offset: Point::new(9, 9),
        inner_shadow_softness: 18,
        shadow: Rgba::new(0, 0, 0, 60),
        shadow_offset: Point::new(0, 6),
        shadow_spread: 1,
        shadow_softness: 12,
        noise_alpha: 5,
    }
}

fn setup_field_style() -> FrostStyle {
    FrostStyle {
        blur_radius: 12,
        saturation_percent: 104,
        brightness_percent: 96,
        tint: Rgba::new(12, 12, 12, 51),
        border: Rgba::new(255, 255, 255, 235),
        border_width: 1,
        inner_highlight: Rgba::new(255, 255, 255, 20),
        inner_shadow: Rgba::new(0, 0, 0, 64),
        inner_shadow_offset: Point::new(9, 9),
        inner_shadow_softness: 18,
        shadow: Rgba::transparent(),
        shadow_offset: Point::new(0, 0),
        shadow_spread: 0,
        shadow_softness: 0,
        noise_alpha: 4,
    }
}

fn draw_setup(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
) -> bool {
    let card = flow::CARD;
    let now = crate::time::monotonic_nanoseconds();
    let region = layout
        .rect(Rect::new(80, 0, 592, 458))
        .intersect(Rect::new(0, 0, 4096, 4096))
        .unwrap_or_else(|| layout.rect(card));
    let cache = unsafe { &mut *SETUP_CACHE.0.get() };
    let cache_len = (region.width.max(0) as usize) * (region.height.max(0) as usize);
    let key = dock_cache_key(layout, region) ^ 0x5e70_0000;
    let captured = if cache_len <= cache.len()
        && SETUP_CACHE_KEY.load(Ordering::Acquire) == key
        && painter.write_region(region, &cache[..cache_len])
    {
        true
    } else {
        let captured = frost(
            painter,
            layout,
            card,
            CornerRadii::all(88),
            setup_card_style(),
            0xae5e_0001,
        )
        .captured;
        if captured
            && cache_len <= cache.len()
            && painter.read_region(region, &mut cache[..cache_len])
        {
            SETUP_CACHE_KEY.store(key, Ordering::Release);
        }
        captured
    };
    let content = match state.screen {
        Screen::Language => flow::draw_language(painter, layout, font, state, now),
        Screen::Keyboard => flow::draw_keyboard(painter, layout, font, state, now),
        Screen::Welcome => flow::draw_welcome(painter, layout, font, state, now),
        _ => flow::draw_credentials(painter, layout, font, state, now),
    };
    captured && content
}

const LOGIN_CARD: Rect = Rect::new(96, 12, 560, 434);

const CURSOR_ROWS: [i32; 20] = [
    1, 2, 3, 4, 6, 7, 9, 10, 12, 13, 15, 16, 18, 15, 12, 9, 6, 4, 2, 1,
];
const CURSOR_WIDTH: usize = 22;
const CURSOR_HEIGHT: usize = 24;

fn draw_cursor_at(frame: &mut FrameBuffer, x: i32, y: i32) {
    for (row, width) in CURSOR_ROWS.iter().enumerate() {
        let ry = y + row as i32;
        for col in -1..=*width {
            frame.blend(x + col, ry, Color::rgb(6, 8, 10), 205);
        }
    }
    for (row, width) in CURSOR_ROWS.iter().enumerate() {
        let ry = y + 1 + row as i32;
        for col in 1..*width {
            frame.blend(x + col, ry, Color::rgb(226, 229, 232), 190);
        }
    }
    for row in 0..7 {
        frame.blend(x + 1, y + 1 + row, Color::rgb(255, 255, 255), 150);
        frame.blend(x + 2, y + 1 + row, Color::rgb(255, 255, 255), 90);
    }
}

struct CursorSprite {
    x: i32,
    y: i32,
    visible: bool,
    /// Set while the pointer is over the Linux screen: the guest draws its
    /// own cursor there, so the host one steps aside.
    hidden: bool,
}

impl CursorSprite {
    const fn new() -> Self {
        Self {
            x: 0,
            y: 0,
            visible: false,
            hidden: false,
        }
    }

    fn erase(&mut self, frame: &mut FrameBuffer) {
        if self.visible {
            let required = frame.info().stride.saturating_mul(frame.height());
            if required <= MAX_DESKTOP_PIXELS {
                frame.restore_region(
                    &shadow_slice()[..required],
                    self.x - 1,
                    self.y,
                    CURSOR_WIDTH + 2,
                    CURSOR_HEIGHT,
                );
            }
            self.visible = false;
        }
    }

    fn paint(&mut self, frame: &mut FrameBuffer, at: Point) {
        if self.hidden {
            return;
        }
        self.x = at.x;
        self.y = at.y;
        draw_cursor_at(frame, at.x, at.y);
        self.visible = true;
    }
}

fn cursor_target(frame: &FrameBuffer) -> Point {
    if crate::mouse::ready() {
        let pointer = crate::mouse::state();
        Point::new(pointer.x, pointer.y)
    } else {
        Point::new(frame.width() as i32 / 2, frame.height() as i32 / 2)
    }
}

fn extract_html_element(
    document: &[u8],
    opening: &[u8],
    closing: &[u8],
    output: &mut [u8],
) -> usize {
    let Some(start) = document
        .windows(opening.len())
        .position(|window| window.eq_ignore_ascii_case(opening))
        .map(|position| position + opening.len())
    else {
        return 0;
    };
    let Some(length) = document[start..]
        .windows(closing.len())
        .position(|window| window.eq_ignore_ascii_case(closing))
    else {
        return 0;
    };
    let length = length.min(output.len());
    output[..length].copy_from_slice(&document[start..start + length]);
    length
}

fn draw_settings(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
) {
    painter.fill_rounded_rect(
        layout.rect(Rect::new(128, 73, 139, 242)),
        layout.radii(CornerRadii::all(18)),
        Rgba::new(222, 237, 241, 110),
    );
    for (index, label) in ["Network", "Bluetooth", "Display", "Sound", "Power"]
        .iter()
        .enumerate()
    {
        text(
            painter,
            layout,
            font,
            Point::new(147, 91 + index as i32 * 40),
            label,
            12,
            Color::rgb(19, 49, 58),
        );
    }
    text(
        painter,
        layout,
        font,
        Point::new(296, 85),
        "Network",
        22,
        Color::rgb(15, 43, 53),
    );
    draw_toggle(
        painter,
        layout,
        font,
        Rect::new(296, 127, 118, 42),
        if state.wifi { "Wi-Fi on" } else { "Wi-Fi off" },
        state.wifi,
    );
    text(
        painter,
        layout,
        font,
        Point::new(296, 193),
        "AerOS uses the native e1000e driver",
        11,
        Color::rgb(31, 68, 79),
    );
    text(
        painter,
        layout,
        font,
        Point::new(296, 218),
        "DHCP, IPv4, DNS and UDP verified",
        11,
        Color::rgb(31, 68, 79),
    );
}

fn draw_toggle(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    bounds: Rect,
    label: &str,
    enabled: bool,
) {
    painter.fill_rounded_rect(
        layout.rect(bounds),
        layout.radii(CornerRadii::all(21)),
        if enabled {
            Rgba::new(223, 242, 251, 150)
        } else {
            Rgba::new(169, 184, 191, 100)
        },
    );
    centered_text(
        painter,
        layout,
        font,
        bounds,
        label,
        13,
        Color::rgb(9, 23, 29),
    );
}

fn centered_text(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    bounds: Rect,
    value: &str,
    height: i32,
    color: Color,
) {
    let physical = layout.rect(bounds);
    let physical_height = layout.scale.logical(height);
    let width = font.text_width(value, physical_height);
    painter.text(
        font,
        Point::new(
            physical.x + physical.width.saturating_sub(width) / 2,
            physical.y + physical.height.saturating_sub(physical_height) / 2,
        ),
        value,
        physical_height,
        color,
    );
}

fn text(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    origin: Point,
    value: &str,
    height: i32,
    color: Color,
) {
    painter.text(
        font,
        layout.point(origin),
        value,
        layout.scale.logical(height),
        color,
    );
}

fn frost(
    painter: &mut Painter<'_>,
    layout: Layout,
    bounds: Rect,
    radii: CornerRadii,
    style: FrostStyle,
    seed: u32,
) -> FrostReport {
    frost_mode(
        painter,
        layout,
        bounds,
        radii,
        style,
        seed,
        MOTION_CHEAP.load(Ordering::Relaxed),
    )
}

/// Skips the per-pixel blur sampling and paints a flat tint instead. The blur
/// pass is the most expensive part of a redraw (a full box-blur over every
/// pixel under the shape), so mid-motion frames use this to stay near the
/// frame budget; the real frosted look reappears once motion settles.
fn frost_mode(
    painter: &mut Painter<'_>,
    layout: Layout,
    bounds: Rect,
    radii: CornerRadii,
    style: FrostStyle,
    seed: u32,
    cheap: bool,
) -> FrostReport {
    // Big shapes use block-wise colour work (2x2 settled, 3x3 while moving):
    // the blur is smooth, so it is hard to see, and it is 4-9x cheaper.
    let large = bounds.width * bounds.height > 60_000;
    let step = if cheap {
        3
    } else if large {
        2
    } else {
        1
    };
    painter.frosted_rounded_rect_stepped(
        layout.rect(bounds),
        layout.radii(radii),
        layout.frost(style),
        seed,
        step,
    )
}

fn dock_style() -> FrostStyle {
    FrostStyle {
        blur_radius: 14,
        saturation_percent: 115,
        brightness_percent: 105,
        tint: Rgba::new(196, 196, 196, 51),
        border: Rgba::new(0, 0, 0, 255),
        border_width: 1,
        inner_highlight: Rgba::new(255, 255, 255, 34),
        inner_shadow: Rgba::new(0, 0, 0, 64),
        inner_shadow_offset: Point::new(9, 9),
        inner_shadow_softness: 18,
        shadow: Rgba::new(0, 0, 0, 45),
        shadow_offset: Point::new(0, 4),
        shadow_spread: 1,
        shadow_softness: 8,
        noise_alpha: 7,
    }
}

fn time_style() -> FrostStyle {
    FrostStyle {
        blur_radius: 9,
        saturation_percent: 110,
        brightness_percent: 103,
        tint: Rgba::new(188, 206, 212, 75),
        border: Rgba::new(239, 248, 250, 90),
        border_width: 1,
        inner_highlight: Rgba::new(255, 255, 255, 25),
        inner_shadow: Rgba::new(0, 0, 0, 28),
        inner_shadow_offset: Point::new(4, 4),
        inner_shadow_softness: 10,
        shadow: Rgba::transparent(),
        shadow_offset: Point::new(0, 0),
        shadow_spread: 0,
        shadow_softness: 0,
        noise_alpha: 4,
    }
}

fn window_style() -> FrostStyle {
    FrostStyle {
        blur_radius: 18,
        saturation_percent: 108,
        brightness_percent: 108,
        tint: Rgba::new(188, 222, 232, 70),
        border: Rgba::new(230, 245, 249, 115),
        border_width: 1,
        inner_highlight: Rgba::new(255, 255, 255, 34),
        inner_shadow: Rgba::new(0, 0, 0, 35),
        inner_shadow_offset: Point::new(6, 6),
        inner_shadow_softness: 9,
        shadow: Rgba::new(0, 0, 0, 55),
        shadow_offset: Point::new(0, 5),
        shadow_spread: 1,
        shadow_softness: 10,
        noise_alpha: 5,
    }
}

struct WallpaperCache(UnsafeCell<[u32; MAX_DESKTOP_PIXELS]>);

unsafe impl Sync for WallpaperCache {}

static WALLPAPER_CACHE: WallpaperCache = WallpaperCache(UnsafeCell::new([0; MAX_DESKTOP_PIXELS]));
static WALLPAPER_CACHE_KEY: AtomicU64 = AtomicU64::new(0);

/// `region`, when given, restricts the repaint to that sub-rectangle instead
/// of the full screen. Only safe to use once the wallpaper is already on
/// screen from a prior full paint (e.g. mid-animation) — a cold cache always
/// repaints everything regardless of `region`.
fn draw_wallpaper(frame: &mut FrameBuffer, region: Option<Rect>) {
    if !wallpaper_valid() || frame.width() == 0 || frame.height() == 0 {
        frame.vertical_gradient(Color::rgb(91, 174, 216), Color::rgb(17, 66, 76));
        return;
    }
    let info = frame.info();
    let required = info.stride.saturating_mul(info.height);
    if required <= MAX_DESKTOP_PIXELS {
        let key =
            ((info.width as u64) << 40) ^ ((info.height as u64) << 20) ^ info.stride as u64 ^ 1;
        let cache = unsafe { &mut *WALLPAPER_CACHE.0.get() };
        let was_cached = WALLPAPER_CACHE_KEY.load(Ordering::Acquire) == key;
        if !was_cached {
            render_wallpaper_into(frame, &mut cache[..required]);
            WALLPAPER_CACHE_KEY.store(key, Ordering::Release);
        }
        if was_cached && let Some(clip) = region {
            let screen = Rect::new(0, 0, info.width as i32, info.height as i32);
            if let Some(area) = screen.intersect(clip) {
                let _ = frame.blit_packed_region(
                    &cache[..required],
                    area.x as usize,
                    area.y as usize,
                    area.width as usize,
                    area.height as usize,
                );
                return;
            }
        }
        let _ = frame.blit_packed(&cache[..required]);
        return;
    }
    render_wallpaper_direct(frame);
}

fn render_wallpaper_into(frame: &FrameBuffer, cache: &mut [u32]) {
    let info = frame.info();
    let (start_x, start_y, visible_width, visible_height, width_divisor, height_divisor) =
        wallpaper_projection(info.width, info.height);
    for y in 0..info.height {
        let source_y = start_y + visible_height * y as u64 / height_divisor;
        let row = y * info.stride;
        for x in 0..info.width {
            let source_x = start_x + visible_width * x as u64 / width_divisor;
            let index = row + x;
            if index < cache.len() {
                cache[index] = frame.pack_color(sample_wallpaper(source_x, source_y));
            }
        }
    }
}

fn wallpaper_projection(width: usize, height: usize) -> (u64, u64, u64, u64, u64, u64) {
    let source_width = WALLPAPER_WIDTH as u64;
    let source_height = WALLPAPER_HEIGHT as u64;
    let destination_width_u64 = width as u64;
    let destination_height_u64 = height as u64;
    let full_width = (source_width.saturating_sub(1)) << 16;
    let full_height = (source_height.saturating_sub(1)) << 16;
    let (start_x, start_y, visible_width, visible_height) =
        if destination_width_u64 * source_height > destination_height_u64 * source_width {
            let scaled = full_width * destination_height_u64 / destination_width_u64;
            (
                0,
                (full_height.saturating_sub(scaled)) / 2,
                full_width,
                scaled,
            )
        } else {
            let scaled = full_height * destination_width_u64 / destination_height_u64;
            (
                (full_width.saturating_sub(scaled)) / 2,
                0,
                scaled,
                full_height,
            )
        };
    (
        start_x,
        start_y,
        visible_width,
        visible_height,
        width.saturating_sub(1).max(1) as u64,
        height.saturating_sub(1).max(1) as u64,
    )
}

fn render_wallpaper_direct(frame: &mut FrameBuffer) {
    let destination_width = frame.width();
    let destination_height = frame.height();
    let (start_x, start_y, visible_width, visible_height, width_divisor, height_divisor) =
        wallpaper_projection(destination_width, destination_height);
    for y in 0..destination_height {
        let source_y = start_y + visible_height * y as u64 / height_divisor;
        for x in 0..destination_width {
            let source_x = start_x + visible_width * x as u64 / width_divisor;
            frame.pixel(x as i32, y as i32, sample_wallpaper(source_x, source_y));
        }
    }
}

fn sample_wallpaper(x: u64, y: u64) -> Color {
    let x0 = ((x >> 16) as usize).min(WALLPAPER_WIDTH - 1);
    let y0 = ((y >> 16) as usize).min(WALLPAPER_HEIGHT - 1);
    let x1 = (x0 + 1).min(WALLPAPER_WIDTH - 1);
    let y1 = (y0 + 1).min(WALLPAPER_HEIGHT - 1);
    let fx = (x & 0xffff) as u32;
    let fy = (y & 0xffff) as u32;
    let top = blend_color(source_color(x0, y0), source_color(x1, y0), fx);
    let bottom = blend_color(source_color(x0, y1), source_color(x1, y1), fx);
    blend_color(top, bottom, fy)
}

fn blend_color(first: Color, second: Color, amount: u32) -> Color {
    let inverse = 65_536u32.saturating_sub(amount);
    Color::rgb(
        ((first.red as u32 * inverse + second.red as u32 * amount) >> 16) as u8,
        ((first.green as u32 * inverse + second.green as u32 * amount) >> 16) as u8,
        ((first.blue as u32 * inverse + second.blue as u32 * amount) >> 16) as u8,
    )
}

fn source_color(x: usize, y: usize) -> Color {
    let offset = (y * WALLPAPER_WIDTH + x) * 2;
    let value = u16::from_le_bytes([WALLPAPER[offset], WALLPAPER[offset + 1]]);
    Color::rgb(
        (((value >> 11) & 0x1f) as u32 * 255 / 31) as u8,
        (((value >> 5) & 0x3f) as u32 * 255 / 63) as u8,
        ((value & 0x1f) as u32 * 255 / 31) as u8,
    )
}

fn wallpaper_valid() -> bool {
    WALLPAPER.len() == WALLPAPER_WIDTH * WALLPAPER_HEIGHT * 2
        && source_color(0, 0) != source_color(0, WALLPAPER_HEIGHT - 1)
        && source_color(WALLPAPER_WIDTH / 2, 0)
            != source_color(WALLPAPER_WIDTH / 2, WALLPAPER_HEIGHT - 1)
}

fn input_self_test() -> bool {
    let mut decoder = KeyDecoder::new();
    let tab = decoder.feed(0x0f) == Some(DesktopKey::Tab);
    let prefix = decoder.feed(0xe0).is_none();
    let right = decoder.feed(0x4d) == Some(DesktopKey::Right);
    let terminal = decoder.feed(0x14) == Some(DesktopKey::Terminal);
    let mut state = DesktopState::new();
    state.screen = Screen::Desktop;
    let redraw =
        state.handle(DesktopKey::Apps) == DesktopAction::Redraw && state.overlay == Overlay::Apps;
    let launch = state.handle(DesktopKey::Activate) == DesktopAction::Redraw
        && state.app == DesktopApp::Terminal;
    tab && prefix
        && right
        && terminal
        && redraw
        && launch
        && url_parse_self_test()
        && session_self_test()
        && pointer_self_test()
}

fn pointer_self_test() -> bool {
    let mut launch = DesktopState::new();
    launch.screen = Screen::Desktop;
    let terminal = launch.click(Point::new(133, 405)) == DesktopAction::Redraw
        && launch.app == DesktopApp::Terminal;
    let mut apps = DesktopState::new();
    apps.screen = Screen::Desktop;
    apps.click(Point::new(69, 405));
    let opened = apps.overlay == Overlay::Apps;
    let mut wake = DesktopState::new();
    let ignored =
        wake.click(Point::new(120, 120)) == DesktopAction::Idle && wake.screen == Screen::Language;
    let advanced = wake.click(Point::new(376, 182)) == DesktopAction::Redraw
        && wake.screen == Screen::Keyboard;
    let scale = Scale::from_milli(1_700).unwrap_or(Scale::ONE);
    let round_trip = (scale.invert(scale.logical(200)) - 200).abs() <= 1;
    terminal && opened && ignored && advanced && round_trip
}

fn session_self_test() -> bool {
    let mut walk = DesktopState::new();
    // Setup cannot be walked through with empty fields.
    walk.handle(DesktopKey::Activate);
    let english_done = walk.screen == Screen::Keyboard;
    walk.handle(DesktopKey::Character(b's'));
    walk.handle(DesktopKey::Character(b'w'));
    let mut hits = [0usize; 16];
    let filtered = flow::keyboard_matches(&walk, &mut hits) == 1 && walk.kb_search_len == 2;
    walk.handle(DesktopKey::Backspace);
    walk.handle(DesktopKey::Backspace);
    walk.handle(DesktopKey::Down);
    walk.handle(DesktopKey::Down);
    let moved = walk.kb_cursor == 2;
    walk.handle(DesktopKey::Up);
    walk.handle(DesktopKey::Activate);
    let language_done = english_done && filtered && moved && walk.screen == Screen::Username;
    let empty_name_blocked = walk.handle(DesktopKey::Activate) == DesktopAction::Redraw
        && walk.screen == Screen::Username;
    for byte in b"aer" {
        walk.handle(DesktopKey::Character(*byte));
    }
    walk.handle(DesktopKey::Activate);
    let empty_password_blocked = walk.handle(DesktopKey::Activate) == DesktopAction::Redraw
        && walk.screen == Screen::Password;
    for byte in b"abc" {
        walk.handle(DesktopKey::Character(*byte));
    }
    walk.handle(DesktopKey::Activate);
    let weak_blocked = walk.screen == Screen::Password;
    let click_blocked =
        walk.click(Point::new(120, 120)) == DesktopAction::Idle && walk.screen == Screen::Password;
    let escape_blocked = walk.handle(DesktopKey::Escape) == DesktopAction::Redraw
        && walk.screen == Screen::Password
        && walk.setup_password_len == 0;
    let hotkey_blocked =
        walk.handle(DesktopKey::Browser) == DesktopAction::Idle && walk.screen == Screen::Password;

    // Full setup with a mismatching confirmation, then a matching one.
    let mut real = DesktopState::new();
    real.kdf_iterations = 32;
    real.handle(DesktopKey::Activate);
    real.handle(DesktopKey::Activate);
    for byte in b"aer" {
        real.handle(DesktopKey::Character(*byte));
    }
    let username_captured = real.username_str() == "aer";
    real.handle(DesktopKey::Activate);
    for byte in b"tr1cky-Pass" {
        real.handle(DesktopKey::Character(*byte));
    }
    real.handle(DesktopKey::Activate);
    let on_confirm = real.screen == Screen::Confirm;
    for byte in b"tr1cky-Pasz" {
        real.handle(DesktopKey::Character(*byte));
    }
    real.handle(DesktopKey::Activate);
    let mismatch_blocked = real.screen == Screen::Confirm && real.confirm_len == 0;
    for byte in b"tr1cky-Pass" {
        real.handle(DesktopKey::Character(*byte));
    }
    real.handle(DesktopKey::Activate);
    let welcomed = real.screen == Screen::Welcome;
    real.handle(DesktopKey::Activate);
    let hashed = welcomed
        && real.screen == Screen::Lock
        && real.credential.is_some()
        && real.setup_password_len == 0
        && real.setup_password_input == [0; MAX_NAME];
    real.handle(DesktopKey::Character(b'x'));
    let lock_wakes = real.screen == Screen::Login;
    // A bare Enter and Escape must not sign in.
    let bare_enter_blocked =
        real.handle(DesktopKey::Activate) == DesktopAction::Redraw && real.screen == Screen::Login;
    let escape_no_bypass =
        real.handle(DesktopKey::Escape) == DesktopAction::Redraw && real.screen == Screen::Login;
    let click_does_not_sign_in =
        real.click(Point::new(376, 320)) == DesktopAction::Idle && real.screen == Screen::Login;
    for byte in b"wrong" {
        real.handle(DesktopKey::Character(*byte));
    }
    real.handle(DesktopKey::Activate);
    let wrong_password_blocked = real.screen == Screen::Login && real.login_len == 0;
    for byte in b"tr1cky-Pass" {
        real.handle(DesktopKey::Character(*byte));
    }
    real.handle(DesktopKey::Activate);
    let correct_password_signs_in = real.screen == Screen::Desktop;
    let relocks = real.lock_session() && real.screen == Screen::Login;

    let mut hotkey = DesktopState::new();
    hotkey.screen = Screen::Desktop;
    let dismissed = hotkey.handle(DesktopKey::Browser) == DesktopAction::Redraw
        && hotkey.screen == Screen::Desktop
        && hotkey.app == DesktopApp::Browser;
    let no_account_no_lock = !hotkey.lock_session();
    language_done
        && empty_name_blocked
        && empty_password_blocked
        && weak_blocked
        && click_blocked
        && escape_blocked
        && hotkey_blocked
        && username_captured
        && on_confirm
        && mismatch_blocked
        && hashed
        && lock_wakes
        && bare_enter_blocked
        && escape_no_bypass
        && click_does_not_sign_in
        && wrong_password_blocked
        && correct_password_signs_in
        && relocks
        && dismissed
        && no_account_no_lock
        && crate::auth::self_test()
}

fn url_parse_self_test() -> bool {
    let simple = matches!(
        parse_url("example.com"),
        UrlParse::Http {
            host: "example.com",
            path: "/"
        }
    );
    let scheme_and_path = matches!(
        parse_url("http://aeros.dev/status"),
        UrlParse::Http {
            host: "aeros.dev",
            path: "/status"
        }
    );
    let secure = matches!(parse_url("https://example.com"), UrlParse::Https);
    let empty = matches!(parse_url(""), UrlParse::Invalid);
    let spaces = matches!(parse_url("no dots here"), UrlParse::Invalid);
    let bare = matches!(parse_url("localhost"), UrlParse::Invalid);
    let mut state = DesktopState::new();
    state.app = DesktopApp::Browser;
    state.url_active = true;
    let typed = state.push_url_byte(b'a') && !state.push_url_byte(0x07);
    let submit = {
        state.url_len = 0;
        for byte in b"aeros.dev/x" {
            state.push_url_byte(*byte);
        }
        FORCE_NATIVE_FETCH.store(true, core::sync::atomic::Ordering::Relaxed);
        state.submit_url();
        FORCE_NATIVE_FETCH.store(false, core::sync::atomic::Ordering::Relaxed);
        state.browser_pending
            && state.browser_phase == BrowserPhase::Loading
            && state.browser_host_str() == "aeros.dev"
            && state.browser_path_str() == "/x"
    };
    simple && scheme_and_path && secure && empty && spaces && bare && typed && submit
}

fn time_text() -> ClockBuffer {
    let current = crate::rtc::local_date_time();
    let mut result = ClockBuffer::new();
    let _ = write!(result, "{:02}:{:02}", current.hour, current.minute);
    result
}

fn date_text() -> ClockBuffer {
    let current = crate::rtc::local_date_time();
    let mut result = ClockBuffer::new();
    let _ = write!(
        result,
        "{:02}.{:02}.{:02}",
        current.day,
        current.month,
        current.year % 100
    );
    result
}

fn dock_layout_valid() -> bool {
    let dock = Rect::new(25, 369, 702, 71);
    let clock = Rect::new(503, 377, 216, 55);
    let first = Rect::new(44, 380, 50, 50);
    let last = Rect::new(428, 380, 50, 50);
    dock.contains(Point::new(first.x, first.y))
        && dock.contains(Point::new(last.right() - 1, last.bottom() - 1))
        && dock.contains(Point::new(clock.x, clock.y))
        && dock.contains(Point::new(clock.right() - 1, clock.bottom() - 1))
        && last.intersect(clock).is_none()
}

fn empty_report(button: bool, input: bool) -> DesktopReport {
    DesktopReport {
        wallpaper: false,
        dock: false,
        app_switcher: false,
        quick_settings: false,
        window: false,
        button,
        input,
        verified: false,
    }
}

/// A fixed 56-byte text buffer for the Shield toast.
struct ClockBuffer56 {
    bytes: [u8; 56],
    len: usize,
}

impl ClockBuffer56 {
    const fn new() -> Self {
        Self {
            bytes: [0; 56],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl Write for ClockBuffer56 {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        // Truncate rather than fail: a long name still shows its start.
        let room = self.bytes.len() - self.len;
        let take = value.len().min(room);
        self.bytes[self.len..self.len + take].copy_from_slice(&value.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

struct ClockBuffer {
    bytes: [u8; 24],
    len: usize,
}

impl ClockBuffer {
    const fn new() -> Self {
        Self {
            bytes: [0; 24],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl Write for ClockBuffer {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let end = self.len.checked_add(value.len()).ok_or(fmt::Error)?;
        if end > self.bytes.len() {
            return Err(fmt::Error);
        }
        self.bytes[self.len..end].copy_from_slice(value.as_bytes());
        self.len = end;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Motion helpers, the music hub, the volume HUD and the lock/login screens.
// ---------------------------------------------------------------------------

const MUSIC_TRACKS: [(&str, &str); 4] = [
    ("Neon Drive", "AerOS Sessions"),
    ("Glass Horizon", "Frost Ensemble"),
    ("Low Orbit", "Kernel Panic"),
    ("Blue Hour", "Dock and Roll"),
];
const MUSIC_COLORS: [(u8, u8, u8); 4] = [
    (72, 156, 58),
    (66, 112, 208),
    (206, 86, 124),
    (222, 158, 56),
];
const MUSIC_TRACK_NS: u64 = 200_000_000_000;
const MUSIC_EXPAND_NS: u64 = 4_500_000_000;
const MUSIC_SLIDE_NS: u64 = 520_000_000;
const MUSIC_PAUSE_LINGER_NS: u64 = 2_500_000_000;
const VOLUME_HUD_NS: u64 = 1_900_000_000;
const VOLUME_SLIDE_NS: u64 = 380_000_000;
/// Redraw interval for ambient (always moving) elements.
const AMBIENT_FRAME_NS: u64 = 45_000_000;

fn ease_out_back_milli(progress_milli: u32) -> i32 {
    let u = progress_milli.min(1000) as i64 - 1000;
    (1000 + 2702 * u * u * u / 1_000_000_000 + 1702 * u * u / 1_000_000) as i32
}

fn ease_in_out_milli(progress_milli: u32) -> i32 {
    let t = progress_milli.min(1000) as i64;
    (t * t * (3000 - 2 * t) / 1_000_000) as i32
}

/// Smooth periodic wave: phase in 0..1000, result in -1000..=1000.
fn wave_milli(phase_milli: u64) -> i32 {
    let p = (phase_milli % 1000) as i64;
    let t = if p < 500 { p * 2 } else { (1000 - p) * 2 };
    let s = t * t * (3000 - 2 * t) / 1_000_000;
    (s * 2 - 1000) as i32
}

fn lerp_i32(from: i32, to: i32, t_milli: i32) -> i32 {
    from + ((to - from) as i64 * t_milli as i64 / 1000) as i32
}

fn progress_of(now_ns: u64, start_ns: u64, duration_ns: u64) -> u32 {
    (now_ns.saturating_sub(start_ns).min(duration_ns) * 1000 / duration_ns) as u32
}

fn mix_color(from: Color, to: Color, t_milli: i32) -> Color {
    let t = t_milli.clamp(0, 1000);
    let mix = |a: u8, b: u8| lerp_i32(a as i32, b as i32, t) as u8;
    Color::rgb(
        mix(from.red, to.red),
        mix(from.green, to.green),
        mix(from.blue, to.blue),
    )
}

fn weekday_text() -> &'static str {
    const NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    NAMES[((crate::rtc::local_seconds() / 86_400 + 4) % 7) as usize]
}

const ACCOUNT_PATH: &str = "/data/ACCOUNT.DAT";
const ACCOUNT_MAGIC: &[u8; 4] = b"AEA1";

impl DesktopState {
    /// Keeps the account (name + salted password hash, never the password)
    /// on the data disk so setup only happens once.
    fn save_account(&self) {
        let Some(credential) = self.credential else {
            return;
        };
        if !self.persist_account {
            return;
        }
        let mut buffer = [0u8; 5 + MAX_NAME + crate::auth::Credential::STORED_LEN];
        buffer[..4].copy_from_slice(ACCOUNT_MAGIC);
        buffer[4] = self.username_len as u8;
        buffer[5..5 + self.username_len].copy_from_slice(&self.username_input[..self.username_len]);
        let mut length = 5 + self.username_len;
        buffer[length..length + crate::auth::Credential::STORED_LEN]
            .copy_from_slice(&credential.to_bytes());
        length += crate::auth::Credential::STORED_LEN;
        let saved = match vfs::open_file(ACCOUNT_PATH, true, false, true, 0o600, true) {
            Ok(descriptor) => {
                let written = vfs::write(descriptor, &buffer[..length], false).is_ok();
                let _ = vfs::close(descriptor);
                written
            }
            Err(_) => false,
        };
        serial::format(format_args!("AEROS_ACCOUNT saved={saved}\n"));
    }

    #[cfg_attr(feature = "boot-test", allow(dead_code))]
    fn load_account(&mut self) -> bool {
        let mut buffer = [0u8; 5 + MAX_NAME + crate::auth::Credential::STORED_LEN + 1];
        let Ok(descriptor) = vfs::open_file(ACCOUNT_PATH, false, false, false, 0, false) else {
            return false;
        };
        let read = vfs::read(descriptor, &mut buffer);
        let _ = vfs::close(descriptor);
        let Ok(length) = read else {
            return false;
        };
        let data = &buffer[..length];
        if data.len() < 5 || &data[..4] != ACCOUNT_MAGIC {
            return false;
        }
        let name_len = data[4] as usize;
        if name_len == 0
            || name_len > MAX_NAME
            || data.len() != 5 + name_len + crate::auth::Credential::STORED_LEN
        {
            return false;
        }
        let Some(credential) = crate::auth::Credential::from_bytes(&data[5 + name_len..]) else {
            return false;
        };
        self.username_input[..name_len].copy_from_slice(&data[5..5 + name_len]);
        self.username_len = name_len;
        self.credential = Some(credential);
        true
    }

    fn apply_volume(&self) {
        crate::audio::set_volume(if self.muted { 0 } else { self.volume });
    }

    fn music_now_elapsed(&self, now: u64) -> u64 {
        if self.music_playing {
            now.saturating_sub(self.music_started_ns)
        } else {
            self.music_elapsed_ns
        }
    }

    fn music_visible(&self, now: u64) -> bool {
        self.music_active
            && (self.music_playing
                || now < self.music_pause_ns.saturating_add(MUSIC_PAUSE_LINGER_NS))
    }

    fn music_expanded(&self, now: u64) -> bool {
        now < self.music_expand_until_ns
    }

    fn music_bump(&mut self, now: u64, expand: bool) {
        if expand {
            if !self.music_expanded(now) {
                self.music_expand_at_ns = now;
            }
            self.music_expand_until_ns = now + MUSIC_EXPAND_NS;
        }
        self.music_track_ns = now;
        self.motion_until_ns = self
            .motion_until_ns
            .max(self.music_expand_until_ns + 400_000_000)
            .max(now + 700_000_000);
    }

    fn music_toggle(&mut self) {
        let now = crate::time::monotonic_nanoseconds();
        if self.music_playing {
            self.music_elapsed_ns = now.saturating_sub(self.music_started_ns);
            self.music_playing = false;
            self.music_pause_ns = now;
            crate::audio::stop();
        } else {
            if !self.music_visible(now) {
                self.music_shown_ns = now;
            }
            self.music_active = true;
            self.music_playing = true;
            self.music_started_ns = now.saturating_sub(self.music_elapsed_ns);
            self.apply_volume();
            if self.music_elapsed_ns == 0 {
                crate::audio::play_song(self.music_track as usize);
            } else {
                crate::audio::resume();
            }
        }
        self.music_bump(now, true);
    }

    fn music_skip(&mut self, step: i32) {
        let now = crate::time::monotonic_nanoseconds();
        if !self.music_visible(now) {
            self.music_shown_ns = now;
        }
        let count = MUSIC_TRACKS.len() as i32;
        self.music_track = (self.music_track as i32 + step).rem_euclid(count) as u8;
        self.music_active = true;
        self.music_playing = true;
        self.music_elapsed_ns = 0;
        self.music_started_ns = now;
        self.apply_volume();
        crate::audio::play_song(self.music_track as usize);
        self.music_bump(now, true);
    }

    fn volume_step(&mut self, delta: i32) {
        let now = crate::time::monotonic_nanoseconds();
        self.volume_prev = self.shown_volume(now);
        self.muted = false;
        self.volume = (self.volume as i32 + delta).clamp(0, 100) as u8;
        self.apply_volume();
        self.volume_event_ns = now;
        self.volume_hud_until_ns = now + VOLUME_HUD_NS;
        self.motion_until_ns = self.motion_until_ns.max(self.volume_hud_until_ns);
    }

    fn toggle_mute(&mut self) {
        let now = crate::time::monotonic_nanoseconds();
        self.volume_prev = self.shown_volume(now);
        self.muted = !self.muted;
        self.apply_volume();
        self.volume_event_ns = now;
        self.volume_hud_until_ns = now + VOLUME_HUD_NS;
        self.motion_until_ns = self.motion_until_ns.max(self.volume_hud_until_ns);
    }

    /// The level the HUD draws: eases from the previous level to the new one.
    fn shown_volume(&self, now: u64) -> u8 {
        let target = if self.muted { 0 } else { self.volume as i32 };
        let t = ease_out_milli(progress_of(now, self.volume_event_ns, 240_000_000));
        lerp_i32(self.volume_prev as i32, target, t as i32).clamp(0, 100) as u8
    }

    /// Something floating above the desktop is on screen or moving, so the
    /// whole frame is repainted (no partial-region shortcuts).
    fn top_layers_active(&self, now: u64) -> bool {
        self.toast_visible(now)
            || self.notify_center_visible(now)
            || self.search_visible(now)
            || self.preview_wanted()
            || self.ctx_open
            || now < self.brightness_hud_until_ns + VOLUME_SLIDE_NS
            || now < self.volume_hud_until_ns + VOLUME_SLIDE_NS
            || self.clip_open
    }

    fn ambient_active(&self, now: u64) -> bool {
        self.music_visible(now)
            || matches!(self.screen, Screen::Lock | Screen::Login | Screen::Welcome)
            || self.power_action != shellui::PowerAction::None
            || now < self.volume_hud_until_ns + VOLUME_SLIDE_NS
    }

    /// Geometry of the music hub in design coordinates for this instant:
    /// (bounds, morph 0..=1000 where 1000 = fully expanded).
    fn music_hub_geometry(&self, now: u64) -> (Rect, i32) {
        let morph = if self.music_expanded(now) {
            ease_out_back_milli(progress_of(now, self.music_expand_at_ns, 520_000_000))
        } else {
            1000 - ease_out_milli(progress_of(now, self.music_expand_until_ns, 320_000_000)) as i32
        }
        .clamp(-120, 1120);
        let width = lerp_i32(176, 262, morph);
        let height = lerp_i32(27, 96, morph).max(20);
        let appear = ease_out_back_milli(progress_of(now, self.music_shown_ns, MUSIC_SLIDE_NS));
        let leave = if self.music_playing {
            0
        } else {
            ease_in_out_milli(progress_of(
                now,
                (self.music_pause_ns + MUSIC_PAUSE_LINGER_NS).saturating_sub(400_000_000),
                400_000_000,
            ))
        };
        let rise = (1000 - appear.min(1000)) + leave;
        let y = -(height * rise.clamp(0, 1200) / 1000);
        (
            Rect::new(376 - width / 2, y, width, height),
            morph.clamp(0, 1000),
        )
    }

    /// A click anywhere outside the expanded hub folds it back up.
    fn music_click_away(&mut self) {
        let now = crate::time::monotonic_nanoseconds();
        if self.screen == Screen::Desktop && self.music_expanded(now) {
            self.music_expand_until_ns = now;
            self.motion_until_ns = self.motion_until_ns.max(now + 500_000_000);
        }
    }

    fn music_hub_click(&mut self, point: Point) -> Option<DesktopAction> {
        let now = crate::time::monotonic_nanoseconds();
        if !self.music_visible(now) {
            return None;
        }
        let (bounds, morph) = self.music_hub_geometry(now);
        if !bounds.contains(point) {
            return None;
        }
        if self.music_expanded(now) && morph > 700 {
            let controls_y = bounds.y + 56;
            let button = |slot: i32| Rect::new(bounds.x + 76 + slot * 44, controls_y, 40, 28);
            if button(0).contains(point) {
                self.music_skip(-1);
            } else if button(1).contains(point) {
                self.music_toggle();
            } else if button(2).contains(point) {
                self.music_skip(1);
            }
        } else {
            self.music_bump(now, true);
        }
        self.motion_until_ns = self.motion_until_ns.max(now + 700_000_000);
        Some(DesktopAction::Redraw)
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_triangle(
    painter: &mut Painter<'_>,
    layout: Layout,
    x: i32,
    cy: i32,
    w: i32,
    h: i32,
    right: bool,
    color: Rgba,
) {
    for i in 0..w {
        let span = (h * (w - i) / w).max(1);
        let column = if right { x + i } else { x + w - 1 - i };
        painter.fill_rounded_rect(
            layout.rect(Rect::new(column, cy - span / 2, 1, span)),
            layout.radii(CornerRadii::all(0)),
            color,
        );
    }
}

fn eq_level(playing: bool, now: u64, bar: u64) -> i32 {
    if playing {
        300 + 700 * (wave_milli(now / 3_000_000 + bar * 170) + 1000) / 2000
    } else {
        180
    }
}

fn draw_music_hub(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now: u64,
) {
    if !state.music_visible(now) {
        return;
    }
    let (bounds, morph) = state.music_hub_geometry(now);
    if bounds.y + bounds.height <= 0 {
        return;
    }
    let bottom = lerp_i32(13, 30, morph);
    painter.fill_rounded_rect(
        layout.rect(Rect::new(
            bounds.x + 2,
            bounds.y,
            bounds.width - 4,
            bounds.height + 4,
        )),
        layout.radii(CornerRadii::new(0, 0, bottom + 2, bottom + 2)),
        Rgba::new(0, 0, 0, 34),
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(
            bounds.x,
            bounds.y - 8,
            bounds.width,
            bounds.height + 8,
        )),
        layout.radii(CornerRadii::new(0, 0, bottom, bottom)),
        Rgba::new(4, 6, 8, 248),
    );
    let (r, g, b) = MUSIC_COLORS[state.music_track as usize % MUSIC_COLORS.len()];
    let track = MUSIC_TRACKS[state.music_track as usize % MUSIC_TRACKS.len()];
    let since_track = now.saturating_sub(state.music_track_ns);
    // Track changes make the art square pop.
    let pop = if since_track < 420_000_000 {
        ease_out_back_milli(progress_of(now, state.music_track_ns, 420_000_000)) - 1000
    } else {
        0
    };
    if morph < 500 {
        let art = 14 + pop / 120;
        painter.fill_rounded_rect(
            layout.rect(Rect::new(
                bounds.x + 9 - (art - 14) / 2,
                bounds.y + 10 - (art - 14) / 2,
                art,
                art,
            )),
            layout.radii(CornerRadii::all(4)),
            Rgba::new(r, g, b, 255),
        );
        text(
            painter,
            layout,
            font,
            Point::new(bounds.x + 30, bounds.y + 11),
            track.0,
            9,
            Color::rgb(232, 236, 240),
        );
        for bar in 0..4u64 {
            let h = 3 + 10 * eq_level(state.music_playing, now, bar) / 1000;
            painter.fill_rounded_rect(
                layout.rect(Rect::new(
                    bounds.x + bounds.width - 30 + bar as i32 * 5,
                    bounds.y + 19 - h,
                    3,
                    h,
                )),
                layout.radii(CornerRadii::all(1)),
                Rgba::new(r, g, b, 255),
            );
        }
    } else {
        let art = 62 + pop / 30;
        painter.fill_rounded_rect(
            layout.rect(Rect::new(
                bounds.x + 14 - (art - 62) / 2,
                bounds.y + 14 - (art - 62) / 2,
                art,
                art,
            )),
            layout.radii(CornerRadii::all(16)),
            Rgba::new(r, g, b, 255),
        );
        painter.fill_rounded_rect(
            layout.rect(Rect::new(bounds.x + 22, bounds.y + 22, 20, 20)),
            layout.radii(CornerRadii::all(10)),
            Rgba::new(255, 255, 255, 46),
        );
        text(
            painter,
            layout,
            font,
            Point::new(bounds.x + 86, bounds.y + 14),
            track.0,
            12,
            Color::rgb(244, 246, 248),
        );
        text(
            painter,
            layout,
            font,
            Point::new(bounds.x + 86, bounds.y + 32),
            track.1,
            9,
            Color::rgb(150, 158, 166),
        );
        let white = Rgba::new(244, 246, 248, 255);
        let cy = bounds.y + 70;
        draw_triangle(painter, layout, bounds.x + 88, cy, 8, 12, false, white);
        draw_triangle(painter, layout, bounds.x + 95, cy, 8, 12, false, white);
        if state.music_playing {
            for offset in [0, 6] {
                painter.fill_rounded_rect(
                    layout.rect(Rect::new(bounds.x + 130 + offset, cy - 7, 4, 14)),
                    layout.radii(CornerRadii::all(1)),
                    white,
                );
            }
        } else {
            draw_triangle(painter, layout, bounds.x + 131, cy, 10, 14, true, white);
        }
        draw_triangle(painter, layout, bounds.x + 172, cy, 8, 12, true, white);
        draw_triangle(painter, layout, bounds.x + 179, cy, 8, 12, true, white);
        let elapsed = state.music_now_elapsed(now) % MUSIC_TRACK_NS;
        let fill = (elapsed * 230 / MUSIC_TRACK_NS) as i32;
        painter.fill_rounded_rect(
            layout.rect(Rect::new(
                bounds.x + 16,
                bounds.y + bounds.height - 12,
                230,
                3,
            )),
            layout.radii(CornerRadii::all(1)),
            Rgba::new(255, 255, 255, 50),
        );
        painter.fill_rounded_rect(
            layout.rect(Rect::new(
                bounds.x + 16,
                bounds.y + bounds.height - 12,
                fill.max(2),
                3,
            )),
            layout.radii(CornerRadii::all(1)),
            Rgba::new(r, g, b, 255),
        );
        for bar in 0..4u64 {
            let h = 3 + 14 * eq_level(state.music_playing, now, bar) / 1000;
            painter.fill_rounded_rect(
                layout.rect(Rect::new(
                    bounds.x + bounds.width - 30 + bar as i32 * 5,
                    bounds.y + 28 - h,
                    3,
                    h,
                )),
                layout.radii(CornerRadii::all(1)),
                Rgba::new(r, g, b, 255),
            );
        }
    }
}

fn draw_leaf_mark(painter: &mut Painter<'_>, layout: Layout, center: Point, radius: i32, now: u64) {
    painter.fill_rounded_rect(
        layout.rect(Rect::new(
            center.x - radius,
            center.y - radius,
            radius * 2,
            radius * 2,
        )),
        layout.radii(CornerRadii::all(radius)),
        Rgba::new(4, 5, 6, 252),
    );
    let mask = crate::logo::rgba();
    let box_w = (radius * 190 / 100).max(4);
    let box_h = box_w * crate::logo::HEIGHT as i32 / crate::logo::WIDTH as i32;
    let shimmer = (now / 8_000_000) as i32;
    let step = 2;
    let mut y = -box_h / 2;
    while y < box_h / 2 {
        let mut x = -box_w / 2;
        while x < box_w / 2 {
            let u = ((x + box_w / 2) * crate::logo::WIDTH as i32 / box_w) as usize;
            let v = ((y + box_h / 2) * crate::logo::HEIGHT as i32 / box_h) as usize;
            let covered = u < crate::logo::WIDTH
                && v < crate::logo::HEIGHT
                && mask[(v * crate::logo::WIDTH + u) * 4 + 3] > 140;
            if covered {
                let glint = ((x * 3 + y * 5 + shimmer) & 63) < 4;
                let alpha = if glint { 255 } else { 205 };
                painter.fill_rounded_rect(
                    layout.rect(Rect::new(center.x + x, center.y + y, 2, 2)),
                    layout.radii(CornerRadii::all(1)),
                    Rgba::new(230, 236, 240, alpha),
                );
            }
            x += step;
        }
        y += step;
    }
}

/// The glass square holding the account mark, with a floating bob and a
/// slow pulsing halo. `size` already includes any pop-in scaling.
fn draw_account_tile(
    painter: &mut Painter<'_>,
    layout: Layout,
    center: Point,
    size: i32,
    now: u64,
    cheap: bool,
    seed: u32,
) -> bool {
    let bob = wave_milli(now / 3_000_000) * 3 / 1000;
    let halo_phase = (now / 2_400_000) % 1000;
    let halo = (halo_phase as i32) * 14 / 1000;
    let halo_alpha = (70 - (halo_phase as i32) * 70 / 1000).max(0) as u8;
    let tile = Rect::new(center.x - size / 2, center.y - size / 2 + bob, size, size);
    let radius = size * 28 / 100;
    painter.stroke_rounded_rect(
        layout.rect(Rect::new(
            tile.x - halo,
            tile.y - halo,
            tile.width + halo * 2,
            tile.height + halo * 2,
        )),
        layout.radii(CornerRadii::all(radius + halo)),
        2,
        Rgba::new(255, 255, 255, halo_alpha),
    );
    let _ = frost_mode(
        painter,
        layout,
        tile,
        CornerRadii::all(radius),
        setup_card_style(),
        seed,
        // The tile sits on a smooth wallpaper, so a flat tint reads the
        // same as the blur and keeps every animated frame cheap.
        true,
    );
    let _ = cheap;
    painter.fill_rounded_rect(
        layout.rect(tile),
        layout.radii(CornerRadii::all(radius)),
        Rgba::new(150, 156, 160, 52),
    );
    draw_leaf_mark(
        painter,
        layout,
        Point::new(tile.x + size / 2, tile.y + size / 2),
        size * 36 / 100,
        now,
    );
    true
}

fn draw_lock(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now: u64,
    cheap: bool,
) -> bool {
    let entered = now.saturating_sub(state.screen_transition_at_ns);
    let rise = 1000 - ease_out_milli(progress_of(entered, 0, 700_000_000)) as i32;
    let pop = ease_out_back_milli(progress_of(entered, 120_000_000, 720_000_000)).max(0);
    let clock = time_text();
    let breathe = wave_milli(now / 4_000_000).abs();
    centered_text(
        painter,
        layout,
        font,
        Rect::new(LOGIN_CARD.x, 40 + rise * 26 / 1000, LOGIN_CARD.width, 64),
        clock.as_str(),
        56,
        mix_color(
            Color::rgb(255, 255, 255),
            Color::rgb(226, 238, 246),
            breathe,
        ),
    );
    let mut date = ClockBuffer::new();
    let current = crate::rtc::local_date_time();
    let _ = write!(
        date,
        "{}  {}/{}/{:02}",
        weekday_text(),
        current.day,
        current.month,
        current.year % 100
    );
    centered_text(
        painter,
        layout,
        font,
        Rect::new(LOGIN_CARD.x, 102 + rise * 34 / 1000, LOGIN_CARD.width, 18),
        date.as_str(),
        11,
        Color::rgb(232, 238, 242),
    );
    let captured = draw_account_tile(
        painter,
        layout,
        Point::new(376, 236),
        (128 * pop / 1000).max(8),
        now,
        cheap,
        0xae5e_0020,
    );
    let hint_glow = (wave_milli(now / 5_000_000) + 1000) / 2;
    centered_text(
        painter,
        layout,
        font,
        Rect::new(LOGIN_CARD.x, 336 + rise * 30 / 1000, LOGIN_CARD.width, 22),
        state.display_name(),
        16,
        Color::rgb(244, 247, 249),
    );
    centered_text(
        painter,
        layout,
        font,
        Rect::new(LOGIN_CARD.x, 362 + rise * 40 / 1000, LOGIN_CARD.width, 20),
        "Press any key",
        11,
        mix_color(
            Color::rgb(190, 200, 208),
            Color::rgb(255, 255, 255),
            hint_glow,
        ),
    );
    captured
}

fn draw_signin(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now_ns: u64,
    cheap: bool,
) -> bool {
    let entered = now_ns.saturating_sub(state.screen_transition_at_ns);
    let rise = 1000 - ease_out_milli(progress_of(entered, 0, 600_000_000)) as i32;
    let pop = ease_out_back_milli(progress_of(entered, 60_000_000, 680_000_000)).max(0);
    centered_text(
        painter,
        layout,
        font,
        Rect::new(LOGIN_CARD.x, 44 + rise * 24 / 1000, LOGIN_CARD.width, 34),
        "Welcome back",
        26,
        Color::rgb(248, 250, 252),
    );
    let captured = draw_account_tile(
        painter,
        layout,
        Point::new(376, 170),
        (112 * pop / 1000).max(8),
        now_ns,
        cheap,
        0xae5e_0021,
    );
    centered_text(
        painter,
        layout,
        font,
        Rect::new(LOGIN_CARD.x, 240 + rise * 30 / 1000, LOGIN_CARD.width, 22),
        state.display_name(),
        14,
        Color::rgb(240, 244, 247),
    );
    let error_active = now_ns < state.login_error_until_ns;
    let error_started = state.login_error_until_ns.saturating_sub(LOGIN_ERROR_NS);
    let shake = if error_active && now_ns.saturating_sub(error_started) < 520_000_000 {
        let p = progress_of(now_ns, error_started, 520_000_000) as i64;
        (wave_milli((p * 4) as u64) as i64 * 13 * (1000 - p) / 1_000_000) as i32
    } else {
        0
    };
    let grow = ease_out_back_milli(progress_of(entered, 220_000_000, 620_000_000)).max(0);
    let width = (220 * grow / 1000).max(40);
    let field = Rect::new(376 - width / 2 + shake, 296 + rise * 34 / 1000, width, 48);
    painter.fill_rounded_rect(
        layout.rect(Rect::new(
            field.x - 2,
            field.y + 3,
            field.width + 4,
            field.height + 2,
        )),
        layout.radii(CornerRadii::all(26)),
        Rgba::new(0, 0, 0, 34),
    );
    let flash = if error_active {
        mix_color(
            Color::rgb(255, 255, 255),
            Color::rgb(250, 208, 202),
            wave_milli(now_ns / 2_000_000).abs(),
        )
    } else {
        Color::rgb(255, 255, 255)
    };
    painter.fill_rounded_rect(
        layout.rect(field),
        layout.radii(CornerRadii::all(24)),
        Rgba::new(flash.red, flash.green, flash.blue, 244),
    );
    if error_active {
        let pop = ease_out_back_milli(progress_of(now_ns, error_started, 360_000_000)).max(0);
        panels::icon_cross(
            painter,
            layout,
            Point::new(field.x + width - 30, field.y + 24),
            (11 * pop / 1000).max(3),
            Rgba::new(232, 54, 42, 255),
        );
    }
    // Typed characters appear as dots that pop in one by one.
    let dots = state.login_len.min(14) as i32;
    if dots == 0 {
        let blink = (now_ns / 500_000_000).is_multiple_of(2);
        if blink && width > 100 {
            painter.fill_rounded_rect(
                layout.rect(Rect::new(field.x + width / 2 - 1, field.y + 14, 2, 20)),
                layout.radii(CornerRadii::all(1)),
                Rgba::new(70, 84, 92, 200),
            );
        }
    } else {
        let step = 13;
        let start = field.x + width / 2 - (dots * step) / 2 + step / 2;
        for index in 0..dots {
            let pop_dot = if index == dots - 1 {
                ease_out_back_milli(progress_of(now_ns, state.login_typed_ns, 220_000_000)).max(0)
            } else {
                1000
            };
            let size = (9 * pop_dot / 1000).max(2);
            painter.fill_rounded_rect(
                layout.rect(Rect::new(
                    start + index * step - size / 2,
                    field.y + 24 - size / 2,
                    size,
                    size,
                )),
                layout.radii(CornerRadii::all(size / 2)),
                Rgba::new(38, 52, 60, 255),
            );
        }
    }
    let wait = state.lockout.seconds_left(now_ns);
    let mut wait_text = ClockBuffer::new();
    let hint = if wait > 0 {
        let _ = write!(wait_text, "Locked - wait {wait}s");
        wait_text.as_str()
    } else if error_active {
        "Incorrect password"
    } else {
        "Press Enter to sign in"
    };
    let hint_color = if error_active || wait > 0 {
        Color::rgb(238, 132, 124)
    } else {
        Color::rgb(222, 228, 234)
    };
    centered_text(
        painter,
        layout,
        font,
        Rect::new(LOGIN_CARD.x, 362 + rise * 40 / 1000, LOGIN_CARD.width, 22),
        hint,
        11,
        hint_color,
    );
    captured
}

struct DockCache(UnsafeCell<[u32; 320_000]>);

unsafe impl Sync for DockCache {}

static DOCK_CACHE: DockCache = DockCache(UnsafeCell::new([0; 320_000]));
static DOCK_CACHE_KEY: AtomicU64 = AtomicU64::new(0);

/// The dock is drawn from static art and the clock, so once its entrance and
/// press animations are over it looks identical from frame to frame.
fn dock_is_settled(state: &DesktopState, now: u64) -> bool {
    let entrance_end = state.dock_entrance_at_ns
        + DOCK_FALL_NS
        + DOCK_BUTTONS.len() as u64 * DOCK_FALL_STAGGER_NS
        + 200_000_000;
    now > entrance_end
        && now > state.hover_at_ns.saturating_add(460_000_000)
        && state
            .dock_release_ns
            .iter()
            .all(|release| now > release.saturating_add(400_000_000))
}

fn dock_cache_key(layout: Layout, region: Rect) -> u64 {
    let time = time_text();
    let date = date_text();
    let mut key: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |value: u64| {
        key ^= value;
        key = key.wrapping_mul(0x100_0000_01b3);
    };
    for byte in time.as_str().bytes().chain(date.as_str().bytes()) {
        mix(byte as u64);
    }
    mix(region.x as u64);
    mix(region.y as u64);
    mix(region.width as u64);
    mix(region.height as u64);
    mix(layout.offset.x as u64);
    mix(layout.offset.y as u64);
    mix(wallpaper_valid() as u64);
    key | 1
}

struct SetupCache(UnsafeCell<[u32; 1_200_000]>);

unsafe impl Sync for SetupCache {}

static SETUP_CACHE: SetupCache = SetupCache(UnsafeCell::new([0; 1_200_000]));
static SETUP_CACHE_KEY: AtomicU64 = AtomicU64::new(0);

struct WindowCache(UnsafeCell<[u32; 1_800_000]>);

unsafe impl Sync for WindowCache {}

static WINDOW_CACHE: WindowCache = WindowCache(UnsafeCell::new([0; 1_800_000]));
static WINDOW_CACHE_KEY: AtomicU64 = AtomicU64::new(0);

const PANEL_CACHE_PIXELS: usize = 1_920 * 1_080;

struct PanelCache(UnsafeCell<[u32; PANEL_CACHE_PIXELS]>);

unsafe impl Sync for PanelCache {}

static PANEL_CACHE: PanelCache = PanelCache(UnsafeCell::new([0; PANEL_CACHE_PIXELS]));
static PANEL_CACHE_KEY: AtomicU64 = AtomicU64::new(0);

fn frame_pixels(bounds: Rect) -> usize {
    (bounds.width.max(0) as usize) * (bounds.height.max(0) as usize)
}

/// True while an animation is running: frosted panels drop the blur for a flat
/// tint so each frame stays cheap (the real glass returns once motion ends).
static MOTION_CHEAP: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
