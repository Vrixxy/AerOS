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

const DESIGN_WIDTH: i32 = 752;
const DESIGN_HEIGHT: i32 = 458;
const WALLPAPER_WIDTH: usize = 4_148;
const WALLPAPER_HEIGHT: usize = 2_228;
const WALLPAPER: &[u8] = include_bytes!("../../assets/wallpapers/aeros-mountains.rgb565");
const MAX_DESKTOP_PIXELS: usize = 1_920 * 1_080;
const MAX_URL: usize = 128;
const MAX_PATH: usize = 96;
const DEFAULT_URL: &str = "example.com";
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
const SHELL_LINE_MAX: usize = 56;
const SHELL_HISTORY_LINES: usize = 14;
const FILES_MAX_ENTRIES: usize = 12;
const FILES_GRID_COLUMNS: i32 = 3;
const FILES_GRID_ROWS: i32 = 4;
const NOTES_MAX_ENTRIES: usize = 7;
const TRASH_MAX_ENTRIES: usize = 6;
const NAME_INPUT_MAX: usize = 32;
const NOTE_MAX_BYTES: usize = 4_096;
const TRASH_DIRECTORY: &str = "/data/.trash";
const NOTES_DIRECTORY: &str = "/data/Notes";

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
    "", "", "", "", "", "L", "M", "P", "E", "I", "D", "V", "R", "L", "H", "W", "G", "K", "U", "X",
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
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Language,
    Username,
    Password,
    Lock,
    Login,
    Desktop,
}

impl Screen {
    const fn name(self) -> &'static str {
        match self {
            Self::Language => "language",
            Self::Username => "username",
            Self::Password => "password",
            Self::Lock => "lock",
            Self::Login => "login",
            Self::Desktop => "desktop",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Language => Self::Username,
            Self::Username => Self::Password,
            Self::Password => Self::Lock,
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
    overlay_open_at_ns: u64,
    screen_transition_at_ns: u64,
    username_input: [u8; MAX_NAME],
    username_len: usize,
    setup_password_input: [u8; MAX_NAME],
    setup_password_len: usize,
    login_input: [u8; MAX_NAME],
    login_len: usize,
    login_error_until_ns: u64,
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
            overlay_open_at_ns: 0,
            screen_transition_at_ns: 0,
            username_input: [0; MAX_NAME],
            username_len: 0,
            setup_password_input: [0; MAX_NAME],
            setup_password_len: 0,
            login_input: [0; MAX_NAME],
            login_len: 0,
            login_error_until_ns: 0,
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

    fn set_app(&mut self, target: DesktopApp) {
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
        if self.screen == Screen::Login {
            return match key {
                DesktopKey::Escape => {
                    self.login_len = 0;
                    self.set_screen(Screen::Desktop);
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
            match key {
                DesktopKey::Escape => {
                    self.set_screen(Screen::Desktop);
                    return DesktopAction::Redraw;
                }
                DesktopKey::Character(byte) if self.screen == Screen::Username => {
                    return if self.push_username_byte(byte) {
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    };
                }
                DesktopKey::Character(byte) if self.screen == Screen::Password => {
                    return if self.push_setup_password_byte(byte) {
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    };
                }
                DesktopKey::Backspace if self.screen == Screen::Username => {
                    self.username_len = self.username_len.saturating_sub(1);
                    return DesktopAction::Redraw;
                }
                DesktopKey::Backspace if self.screen == Screen::Password => {
                    self.setup_password_len = self.setup_password_len.saturating_sub(1);
                    return DesktopAction::Redraw;
                }
                DesktopKey::Activate | DesktopKey::Tab | DesktopKey::Right | DesktopKey::Down => {
                    self.set_screen(self.screen.next());
                    return DesktopAction::Redraw;
                }
                _ => {
                    if self.screen == Screen::Lock {
                        self.set_screen(Screen::Login);
                        return DesktopAction::Redraw;
                    }
                    self.set_screen(Screen::Desktop);
                }
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
            DesktopKey::Seamless => {
                self.set_overlay(Overlay::None);
                self.set_app(DesktopApp::None);
                self.seamless.enabled = true;
            }
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
        if self.screen == Screen::Login {
            return DesktopAction::Idle;
        }
        if self.screen != Screen::Desktop {
            self.set_screen(if self.screen == Screen::Lock {
                Screen::Login
            } else {
                self.screen.next()
            });
            return DesktopAction::Redraw;
        }
        if self.app != DesktopApp::None && self.overlay == Overlay::None {
            let base = window_base_rect(self.app);
            let close = Rect::new(base.x + base.width - 75, base.y + 7, 14, 14);
            if close.contains(point) {
                self.set_app(DesktopApp::None);
                return DesktopAction::Redraw;
            }
        }
        if self.app == DesktopApp::Browser
            && self.overlay == Overlay::None
            && Rect::new(132, 75, 493, 38).contains(point)
        {
            self.url_active = true;
            self.browser_phase = BrowserPhase::Editing;
            return DesktopAction::Redraw;
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
        if self.overlay == Overlay::Apps {
            for index in 0..APP_LABELS.len().min(6) {
                let bounds = Rect::new(58 + index as i32 * 64, 48, 50, 50);
                if bounds.contains(point) {
                    self.app_focus = index;
                    self.focus_visible = true;
                    return self.handle(DesktopKey::Activate);
                }
            }
            return DesktopAction::Idle;
        }
        if self.overlay == Overlay::Quick {
            if Rect::new(327, 26, 314, 328).contains(point) {
                return self.handle(DesktopKey::Activate);
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

    fn open_browser(&mut self) {
        self.set_app(DesktopApp::Browser);
        self.set_overlay(Overlay::None);
        self.url_active = true;
        if self.url_len == 0 {
            self.url_input[..DEFAULT_URL.len()].copy_from_slice(DEFAULT_URL.as_bytes());
            self.url_len = DEFAULT_URL.len();
        }
        self.start_load();
    }

    fn start_load(&mut self) {
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
                    let _ = vfs::create_directory(TRASH_DIRECTORY, 0o777);
                    let mut trash_path: shell::Text<256> = shell::Text::new();
                    let _ = shell::normalize_path(TRASH_DIRECTORY, row.name_str(), &mut trash_path);
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
        let (count, overflow) = list_directory(NOTES_DIRECTORY, &mut self.notes_entries);
        self.notes_entry_count = count;
        self.notes_overflow = overflow;
    }

    fn notes_set_status(&mut self, message: &str) {
        self.notes_status.clear();
        let _ = self.notes_status.push_str_checked(message);
    }

    fn notes_path_for(&self, name: &str) -> shell::Text<256> {
        let mut joined: shell::Text<256> = shell::Text::new();
        let _ = shell::normalize_path(NOTES_DIRECTORY, name, &mut joined);
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
        let _ = vfs::create_directory(NOTES_DIRECTORY, 0o777);
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
        let (count, overflow) = list_directory(TRASH_DIRECTORY, &mut self.trash_entries);
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
        let _ = shell::normalize_path(TRASH_DIRECTORY, name, &mut joined);
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

    fn push_login_byte(&mut self, byte: u8) -> bool {
        if self.login_len >= MAX_NAME || !byte.is_ascii_graphic() {
            return false;
        }
        self.login_input[self.login_len] = byte;
        self.login_len += 1;
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
        let matches_password = self.login_len > 0
            && self.login_input[..self.login_len]
                == self.setup_password_input[..self.setup_password_len];
        self.login_len = 0;
        self.login_input = [0; MAX_NAME];
        if matches_password {
            self.set_screen(Screen::Desktop);
        } else {
            let now = crate::time::monotonic_nanoseconds();
            self.login_error_until_ns = now.saturating_add(LOGIN_ERROR_NS);
            self.motion_until_ns = self.motion_until_ns.max(self.login_error_until_ns);
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
    Terminal,
    Browser,
    Settings,
    Linux,
    Seamless,
    Backspace,
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
            0x14 => Some(DesktopKey::Terminal),
            0x30 => Some(DesktopKey::Browser),
            0x1f => Some(DesktopKey::Settings),
            0x26 => Some(DesktopKey::Linux),
            0x25 => Some(DesktopKey::Seamless),
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
        draw_app_switcher(&mut painter, layout, ui_font, &state, now, false, 0)
    };
    let quick = {
        let mut painter = Painter::new(frame);
        draw_quick_settings(&mut painter, layout, ui_font, &state, now)
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
        screen_state.screen = Screen::Password;
        let password = draw_setup(&mut painter, layout, ui_font, &screen_state);
        let lock = draw_lock(&mut painter, layout, ui_font, &screen_state, false);
        let signin = draw_signin(&mut painter, layout, ui_font, &screen_state, now, false);
        language && password && lock && signin
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
    let mut idle_ticks: u32 = 0;
    let mut settle_repaints: u32 = 12;
    let mut motion_animating = false;
    let mut blink_phase = true;
    let mut linux_screen_hash = 0u64;
    let mut linux_last_paint_ns = 0u64;
    let mut guest_ptr = GuestPointer::new();
    let mut seam_generation_seen = 0u32;
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
    loop {
        let mut redraw = false;
        while let Some(scancode) = keyboard::pop_scancode() {
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
            if state.screen == Screen::Desktop {
                // Super+V: clipboard history.
                if scancode != 0xe0 && code == 0x2f && !was_extended {
                    if !released && meta_down {
                        state.clip_open = !state.clip_open;
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
            decoder.set_text_mode(
                (state.app == DesktopApp::Browser && state.url_active)
                    || state.app == DesktopApp::Terminal
                    || (state.app == DesktopApp::Files
                        && state.files_mode == FilesMode::NamingFolder)
                    || (state.app == DesktopApp::Notes && state.notes_mode != NotesMode::List)
                    || matches!(
                        state.screen,
                        Screen::Username | Screen::Password | Screen::Login
                    ),
            );
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
        let pointer_moved = pointer.generation != pointer_generation;
        pointer_generation = pointer.generation;
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
        if press_edge && !seam_consumed {
            let layout = Layout::new(frame);
            let logical = layout.to_logical(Point::new(pointer.x, pointer.y));
            match state.click(logical) {
                DesktopAction::Redraw => redraw = true,
                DesktopAction::Idle => {}
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
        }
        if over_linux != guest_ptr.over {
            cursor.erase(frame);
            cursor.hidden = over_linux;
            if !over_linux {
                cursor.paint(frame, cursor_target(frame));
            }
            guest_ptr.over = over_linux;
        }
        let current_minute = crate::rtc::unix_seconds() / 60;
        if current_minute != minute {
            minute = current_minute;
            redraw = true;
        }
        let motion_animating_now = crate::time::monotonic_nanoseconds() < state.motion_until_ns;
        if motion_animating_now || motion_animating {
            redraw = true;
        }
        motion_animating = motion_animating_now;
        if state.app == DesktopApp::Terminal {
            let blink_phase_now =
                (crate::time::monotonic_nanoseconds() / 530_000_000).is_multiple_of(2);
            if blink_phase_now != blink_phase {
                blink_phase = blink_phase_now;
                redraw = true;
            }
        }
        if redraw {
            cursor.erase(frame);
            let verified = present_desktop(frame, fonts, &state);
            cursor.paint(frame, cursor_target(frame));
            serial::format(format_args!(
                "AEROS_DESKTOP_REDRAW overlay={} window={} verified={} app={} screen={}\n",
                state.overlay.name(),
                state.app != DesktopApp::None,
                verified,
                state.app.name(),
                state.screen.name()
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
                cursor.erase(frame);
                let _ = present_desktop_mode(frame, fonts, &state, true);
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
            cursor.erase(frame);
            let verified = present_desktop(frame, fonts, &state);
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
        if (state.app == DesktopApp::Linux || state.seamless.enabled) && crate::svm::linux_ready() {
            let started = tsc();
            crate::svm::linux_pump(LINUX_SLICE_TSC);
            // Repaint when the guest's screen changed, at most ~15 times a
            // second, so a busy console doesn't starve input handling.
            let now_ns = crate::time::monotonic_nanoseconds();
            if now_ns.saturating_sub(linux_last_paint_ns) >= 66_000_000 {
                let signature = linux_screen_signature(state.seamless.enabled);
                if signature != linux_screen_hash {
                    linux_screen_hash = signature;
                    linux_last_paint_ns = now_ns;
                    cursor.erase(frame);
                    present_desktop(frame, fonts, &state);
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
fn draw_clipboard(
    painter: &mut Painter<'_>,
    layout: Layout,
    ui_font: RasterFont,
    state: &DesktopState,
) {
    let count = crate::clipboard::get().len();
    let rows = count.max(1) as i32;
    let panel = Rect::new(176, 60, 400, 78 + rows * 26);
    let radii = layout.radii(CornerRadii::all(16));
    painter.fill_rounded_rect(layout.rect(panel), radii, Rgba::new(244, 246, 248, 246));
    painter.stroke_rounded_rect(layout.rect(panel), radii, 2, Rgba::new(64, 140, 205, 255));
    text(
        painter,
        layout,
        ui_font,
        Point::new(panel.x + 18, panel.y + 12),
        "Clipboard",
        16,
        Color::rgb(18, 23, 26),
    );
    if count == 0 {
        text(
            painter,
            layout,
            ui_font,
            Point::new(panel.x + 18, panel.y + 46),
            "Nothing copied yet",
            13,
            Color::rgb(90, 100, 110),
        );
    }
    for index in 0..count {
        let row = Rect::new(
            panel.x + 10,
            panel.y + 40 + index as i32 * 26,
            panel.width - 20,
            24,
        );
        if index == state.clip_selected {
            painter.fill_rounded_rect(
                layout.rect(row),
                layout.radii(CornerRadii::all(8)),
                Rgba::new(64, 140, 205, 70),
            );
        }
        // One line: the first ~48 characters, newlines shown as spaces.
        let mut label = [b' '; 56];
        label[0] = b'1' + index as u8;
        let mut used = 3;
        for &byte in crate::clipboard::get().entry(index) {
            if used == label.len() {
                break;
            }
            if byte.is_ascii_graphic() || byte == b' ' {
                label[used] = byte;
                used += 1;
            } else if byte == b'\n' || byte == b'\r' || byte == b'\t' {
                used += 1;
            }
        }
        let label = core::str::from_utf8(&label[..used]).unwrap_or("");
        text(
            painter,
            layout,
            ui_font,
            Point::new(row.x + 8, row.y + 4),
            label,
            13,
            Color::rgb(18, 23, 26),
        );
    }
    text(
        painter,
        layout,
        ui_font,
        Point::new(panel.x + 18, panel.y + panel.height - 22),
        "Up/Down or 1-9 choose   Enter paste   Del clear   Esc close",
        11,
        Color::rgb(90, 100, 110),
    );
}

/// The loading screen: the frosted card, the AerOS logo in a rounded frame,
/// a tip line and a pill progress bar. `view` says what is loading and how far
/// along it is.
fn draw_loading(
    painter: &mut Painter<'_>,
    layout: Layout,
    ui_font: RasterFont,
    mono_font: RasterFont,
    view: &crate::loading::LoadingView,
) {
    draw_login_card(painter, layout, 0xae5e_0030, false);
    // Logo frame.
    let frame = Rect::new(236, 24, 282, 226);
    let radii = layout.radii(CornerRadii::all(44));
    painter.fill_rounded_rect(layout.rect(frame), radii, Rgba::new(255, 255, 255, 34));
    painter.stroke_rounded_rect(layout.rect(frame), radii, 1, Rgba::new(235, 246, 250, 120));
    text(
        painter,
        layout,
        mono_font,
        Point::new(frame.x + 8, frame.y + 28),
        AEROS_LOGO,
        4,
        Color::rgb(10, 62, 84),
    );
    // Tip line.
    centered_text(
        painter,
        layout,
        ui_font,
        Rect::new(LOGIN_CARD.x, 270, LOGIN_CARD.width, 26),
        crate::loading::tip(view.elapsed_secs),
        19,
        Color::rgb(22, 30, 38),
    );
    // Progress pill.
    let track = Rect::new(204, 314, 344, 46);
    let pill = layout.radii(CornerRadii::all(23));
    painter.fill_rounded_rect(layout.rect(track), pill, Rgba::new(255, 255, 255, 46));
    let fill_width = (track.width * view.progress_permille as i32 / 1000).max(track.height);
    painter.fill_rounded_rect(
        layout.rect(Rect::new(track.x, track.y, fill_width, track.height)),
        pill,
        Rgba::new(255, 255, 255, 120),
    );
    painter.stroke_rounded_rect(layout.rect(track), pill, 1, Rgba::new(235, 246, 250, 130));
    if view.is_slow() {
        centered_text(
            painter,
            layout,
            ui_font,
            Rect::new(LOGIN_CARD.x, 372, LOGIN_CARD.width, 20),
            "Taking longer than usual...",
            13,
            Color::rgb(60, 72, 84),
        );
    }
}

fn draw_seamless(
    painter: &mut Painter<'_>,
    layout: Layout,
    ui_font: RasterFont,
    mono_font: RasterFont,
    state: &DesktopState,
) {
    let seam = &state.seamless;
    let Some(windows) = crate::svm::linux_windows() else {
        // The guest's desktop isn't up yet: show the loading screen.
        static LOADING_SINCE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        let now = crate::time::monotonic_nanoseconds();
        let mut since = LOADING_SINCE.load(core::sync::atomic::Ordering::Relaxed);
        if since == 0 {
            since = now;
            LOADING_SINCE.store(now, core::sync::atomic::Ordering::Relaxed);
        }
        if let Some(view) = crate::loading::linux_view(since, now) {
            draw_loading(painter, layout, ui_font, mono_font, &view);
        }
        return;
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
                Screen::Lock => draw_lock(&mut painter, layout, ui_font, state, false),
                Screen::Login => draw_signin(&mut painter, layout, ui_font, state, now, false),
                _ => draw_setup(&mut painter, layout, ui_font, state),
            }
        };
        if !scrolling {
            draw_screen_fade(&mut painter, screen_bounds, fade_alpha);
        }
        return wallpaper && session;
    }
    if state.seamless.enabled {
        draw_seamless(&mut painter, layout, ui_font, mono_font, state);
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
        let _ = draw_app_switcher(
            &mut painter,
            panel_layout,
            ui_font,
            state,
            now,
            opening,
            at_ns,
        );
    }
    if state.overlay == Overlay::Quick || sliding_overlay == Some(Overlay::Quick) {
        let opening = state.overlay == Overlay::Quick;
        let at_ns = if opening {
            state.overlay_open_at_ns
        } else {
            state.overlay_close_at_ns
        };
        let panel_layout = overlay_slide_layout(layout, opening, at_ns, now);
        let _ = draw_quick_settings(&mut painter, panel_layout, ui_font, state, now);
    }
    if state.clip_open {
        draw_clipboard(&mut painter, layout, ui_font, state);
    }
    let outer_dock = !apps_active;
    let (dock, controls) = draw_dock(&mut painter, layout, ui_font, state, outer_dock, now);
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
        Rect::new(327, 26, 314, 328)
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
        let fall_offset = dock_fall_offset(state.dock_entrance_at_ns, index, now_ns);
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

fn draw_app_switcher(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    now_ns: u64,
    opening: bool,
    panel_opened_at_ns: u64,
) -> bool {
    let surface = frost_mode(
        painter,
        layout,
        Rect::new(25, 20, 702, 420),
        CornerRadii::all(30),
        dock_style(),
        0xaea0_0001,
        false,
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

fn draw_quick_settings(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    _now_ns: u64,
) -> bool {
    let panel = Rect::new(327, 26, 314, 328);
    let captured = frost_mode(
        painter,
        layout,
        panel,
        CornerRadii::all(30),
        dock_style(),
        0xae91_0001,
        false,
    )
    .captured;
    draw_toggle(
        painter,
        layout,
        font,
        Rect::new(346, 48, 70, 43),
        "Wi",
        state.wifi,
    );
    draw_toggle(
        painter,
        layout,
        font,
        Rect::new(514, 48, 70, 43),
        "BT",
        state.bluetooth,
    );
    draw_toggle(
        painter,
        layout,
        font,
        Rect::new(346, 105, 70, 43),
        "Mic",
        state.microphone,
    );
    draw_toggle(
        painter,
        layout,
        font,
        Rect::new(514, 105, 70, 43),
        "Eco",
        state.battery_saver,
    );
    draw_slider(painter, layout, font, 347, 178, "Brightness", 42);
    draw_slider(painter, layout, font, 347, 244, "Volume", 42);
    captured
}

fn window_base_rect(app: DesktopApp) -> Rect {
    match app {
        DesktopApp::Terminal => Rect::new(14, 15, 724, 340),
        DesktopApp::Files => Rect::new(70, 36, 610, 300),
        DesktopApp::Notes => Rect::new(232, 18, 360, 356),
        DesktopApp::Trash => Rect::new(256, 90, 336, 260),
        // 602x320 of content: the guest screen is scaled down to fit.
        DesktopApp::Linux => Rect::new(65, 10, 622, 364),
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
        _ => Rgba::new(243, 244, 246, 242),
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
    let captured = frost_mode(
        painter,
        layout,
        outer,
        CornerRadii::all(30),
        window_style_for(state.app),
        0xaec5_0001,
        opening,
    )
    .captured;
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
            DesktopApp::None => "AerOS",
        },
        13,
        Color::rgb(18, 23, 26),
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(outer.x + outer.width - 75, outer.y + 7, 14, 14)),
        layout.radii(CornerRadii::all(7)),
        Rgba::opaque(198, 31, 18),
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(outer.x + outer.width - 54, outer.y + 6, 17, 17)),
        layout.radii(CornerRadii::all(5)),
        Rgba::opaque(20, 145, 18),
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(outer.x + outer.width - 30, outer.y + 12, 15, 4)),
        layout.radii(CornerRadii::all(2)),
        Rgba::opaque(213, 142, 10),
    );
    if state.app == DesktopApp::Browser {
        draw_browser(painter, layout, ui_font, mono_font, state);
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

fn draw_browser(
    painter: &mut Painter<'_>,
    layout: Layout,
    ui_font: RasterFont,
    mono_font: RasterFont,
    state: &DesktopState,
) {
    painter.fill_rounded_rect(
        layout.rect(Rect::new(132, 75, 493, 38)),
        layout.radii(CornerRadii::all(19)),
        Rgba::new(244, 249, 250, 205),
    );
    painter.stroke_rounded_rect(
        layout.rect(Rect::new(132, 75, 493, 38)),
        layout.radii(CornerRadii::all(19)),
        layout
            .scale
            .logical(if state.url_active { 2 } else { 1 })
            .max(1) as u8,
        if state.url_active {
            Rgba::new(31, 108, 138, 220)
        } else {
            Rgba::new(120, 140, 148, 120)
        },
    );
    let mut bar = [0u8; MAX_URL + 1];
    let shown = state.url_str().as_bytes();
    let visible = shown.len().min(MAX_URL - 8);
    bar[..visible].copy_from_slice(&shown[shown.len() - visible..]);
    let mut bar_len = visible;
    if state.url_active {
        bar[bar_len] = b'_';
        bar_len += 1;
    }
    if let Ok(bar_text) = core::str::from_utf8(&bar[..bar_len]) {
        text(
            painter,
            layout,
            ui_font,
            Point::new(154, 86),
            bar_text,
            13,
            Color::rgb(25, 45, 52),
        );
    }
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
    let card = Rect::new(96, 12, 560, 434);
    let captured = frost(
        painter,
        layout,
        card,
        CornerRadii::all(88),
        setup_card_style(),
        0xae5e_0001,
    )
    .captured;
    centered_text(
        painter,
        layout,
        font,
        Rect::new(card.x, 40, card.width, 34),
        "AerOS Setup",
        27,
        Color::rgb(244, 246, 248),
    );
    let heading = match state.screen {
        Screen::Username => "Set a Username",
        Screen::Password => "Set a Password",
        _ => "Select a keyboard language",
    };
    centered_text(
        painter,
        layout,
        font,
        Rect::new(card.x, 132, card.width, 28),
        heading,
        20,
        Color::rgb(238, 241, 244),
    );
    let mut fields = true;
    if state.screen == Screen::Language {
        let search = Rect::new(226, 196, 300, 46);
        fields &= frost(
            painter,
            layout,
            search,
            CornerRadii::all(23),
            setup_field_style(),
            0xae5e_0002,
        )
        .captured;
        centered_text(
            painter,
            layout,
            font,
            search,
            "Search",
            15,
            Color::rgb(232, 236, 239),
        );
        for row in 0..2 {
            let bounds = Rect::new(226, 258 + row * 54, 300, 46);
            fields &= frost(
                painter,
                layout,
                bounds,
                CornerRadii::all(23),
                setup_field_style(),
                0xae5e_0010 + row as u32,
            )
            .captured;
            centered_text(
                painter,
                layout,
                font,
                bounds,
                "Keyboard Language in region",
                12,
                Color::rgb(228, 232, 236),
            );
        }
    } else {
        let field = Rect::new(206, 214, 340, 58);
        fields &= frost(
            painter,
            layout,
            field,
            CornerRadii::all(29),
            setup_field_style(),
            0xae5e_0003,
        )
        .captured;
        let caret = if state.screen == Screen::Password {
            masked_caret(state.setup_password_len)
        } else {
            typed_caret(&state.username_input[..state.username_len])
        };
        centered_text(
            painter,
            layout,
            font,
            field,
            caret.as_str(),
            15,
            Color::rgb(236, 239, 242),
        );
    }
    centered_text(
        painter,
        layout,
        font,
        Rect::new(card.x, 392, card.width, 24),
        "Press Enter to continue",
        12,
        Color::rgb(214, 220, 226),
    );
    captured && fields
}

fn draw_avatar(painter: &mut Painter<'_>, layout: Layout, center: Point, radius: i32) {
    painter.fill_rounded_rect(
        layout.rect(Rect::new(
            center.x - radius,
            center.y - radius,
            radius * 2,
            radius * 2,
        )),
        layout.radii(CornerRadii::all(radius)),
        Rgba::new(244, 247, 249, 245),
    );
    let head = radius * 13 / 20;
    painter.fill_rounded_rect(
        layout.rect(Rect::new(
            center.x - head / 2,
            center.y - radius * 11 / 20,
            head,
            head,
        )),
        layout.radii(CornerRadii::all(head / 2)),
        Rgba::new(150, 158, 164, 235),
    );
    let shoulder_w = radius * 11 / 10;
    let shoulder_h = radius * 7 / 10;
    painter.fill_rounded_rect(
        layout.rect(Rect::new(
            center.x - shoulder_w / 2,
            center.y + radius / 20,
            shoulder_w,
            shoulder_h,
        )),
        layout.radii(CornerRadii::new(
            shoulder_w / 2,
            shoulder_w / 2,
            radius * 3 / 20,
            radius * 3 / 20,
        )),
        Rgba::new(150, 158, 164, 235),
    );
}

const LOGIN_CARD: Rect = Rect::new(96, 12, 560, 434);

fn draw_login_card(painter: &mut Painter<'_>, layout: Layout, seed: u32, cheap: bool) -> bool {
    frost_mode(
        painter,
        layout,
        LOGIN_CARD,
        CornerRadii::all(88),
        setup_card_style(),
        seed,
        cheap,
    )
    .captured
}

fn draw_lock(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    state: &DesktopState,
    cheap: bool,
) -> bool {
    let captured = draw_login_card(painter, layout, 0xae5e_0020, cheap);
    let clock = time_text();
    centered_text(
        painter,
        layout,
        font,
        Rect::new(LOGIN_CARD.x, 78, LOGIN_CARD.width, 92),
        clock.as_str(),
        66,
        Color::rgb(248, 250, 252),
    );
    draw_avatar(painter, layout, Point::new(376, 258), 64);
    centered_text(
        painter,
        layout,
        font,
        Rect::new(LOGIN_CARD.x, 352, LOGIN_CARD.width, 24),
        state.display_name(),
        17,
        Color::rgb(240, 244, 247),
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
    let captured = draw_login_card(painter, layout, 0xae5e_0021, cheap);
    centered_text(
        painter,
        layout,
        font,
        Rect::new(LOGIN_CARD.x, 76, LOGIN_CARD.width, 34),
        "Welcome back",
        26,
        Color::rgb(246, 249, 251),
    );
    draw_avatar(painter, layout, Point::new(376, 172), 46);
    centered_text(
        painter,
        layout,
        font,
        Rect::new(LOGIN_CARD.x, 238, LOGIN_CARD.width, 22),
        state.display_name(),
        14,
        Color::rgb(238, 242, 245),
    );
    let field = Rect::new(266, 296, 220, 48);
    painter.fill_rounded_rect(
        layout.rect(field),
        layout.radii(CornerRadii::all(24)),
        Rgba::new(240, 243, 245, 240),
    );
    let caret = masked_caret(state.login_len);
    centered_text(
        painter,
        layout,
        font,
        field,
        caret.as_str(),
        14,
        Color::rgb(38, 52, 60),
    );
    let error_active = now_ns < state.login_error_until_ns;
    let hint = if error_active {
        "Incorrect password"
    } else {
        "Press Enter to sign in"
    };
    let hint_color = if error_active {
        Color::rgb(232, 128, 120)
    } else {
        Color::rgb(218, 224, 230)
    };
    centered_text(
        painter,
        layout,
        font,
        Rect::new(LOGIN_CARD.x, 358, LOGIN_CARD.width, 22),
        hint,
        11,
        hint_color,
    );
    captured
}

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

fn draw_slider(
    painter: &mut Painter<'_>,
    layout: Layout,
    font: RasterFont,
    x: i32,
    y: i32,
    label: &str,
    amount: i32,
) {
    text(
        painter,
        layout,
        font,
        Point::new(x, y),
        label,
        9,
        Color::rgb(25, 47, 54),
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(x, y + 18, 179, 17)),
        layout.radii(CornerRadii::all(9)),
        Rgba::new(236, 246, 249, 110),
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(x + 6, y + 24, 167, 5)),
        layout.radii(CornerRadii::all(3)),
        Rgba::new(235, 245, 248, 170),
    );
    painter.fill_rounded_rect(
        layout.rect(Rect::new(x + amount, y + 18, 18, 18)),
        layout.radii(CornerRadii::all(9)),
        Rgba::new(180, 203, 211, 245),
    );
    painter.stroke_rounded_rect(
        layout.rect(Rect::new(x + amount, y + 18, 18, 18)),
        layout.radii(CornerRadii::all(9)),
        1,
        Rgba::new(61, 89, 99, 100),
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
    frost_mode(painter, layout, bounds, radii, style, seed, false)
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
    if !cheap {
        return painter.frosted_rounded_rect(
            layout.rect(bounds),
            layout.radii(radii),
            layout.frost(style),
            seed,
        );
    }
    let physical_bounds = layout.rect(bounds);
    let physical_radii = layout.radii(radii);
    let physical_style = layout.frost(style);
    // Flat tint instead of the real per-pixel blur sampling (capture +
    // multi-pass blur), which is the expensive part. Keeping the fill means
    // the panel reads the same as its settled appearance while moving —
    // only the blur texture is missing, not the whole translucent backing.
    painter.fill_rounded_rect(physical_bounds, physical_radii, physical_style.tint);
    painter.stroke_rounded_rect(
        physical_bounds,
        physical_radii,
        physical_style.border_width,
        physical_style.border,
    );
    FrostReport {
        pixels: bounds.width as usize * bounds.height as usize,
        blur_radius: physical_style.blur_radius,
        captured: false,
        clipped: false,
    }
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
    let advanced = wake.click(Point::new(120, 120)) == DesktopAction::Redraw
        && wake.screen == Screen::Username;
    let scale = Scale::from_milli(1_700).unwrap_or(Scale::ONE);
    let round_trip = (scale.invert(scale.logical(200)) - 200).abs() <= 1;
    terminal && opened && advanced && round_trip
}

fn session_self_test() -> bool {
    let mut walk = DesktopState::new();
    let steps = [
        Screen::Username,
        Screen::Password,
        Screen::Lock,
        Screen::Login,
    ];
    let mut advanced = true;
    for expected in steps {
        walk.handle(DesktopKey::Activate);
        advanced &= walk.screen == expected;
    }
    // A bare Enter with nothing typed must NOT sign in -- this is the exact
    // click-through-to-desktop bug being fixed, so it's the one invariant
    // this test exists to protect.
    let bare_enter_blocked =
        walk.handle(DesktopKey::Activate) == DesktopAction::Redraw && walk.screen == Screen::Login;
    for byte in b"wrong" {
        walk.handle(DesktopKey::Character(*byte));
    }
    walk.handle(DesktopKey::Activate);
    let wrong_password_blocked = walk.screen == Screen::Login && walk.login_len == 0;
    let click_does_not_sign_in =
        walk.click(Point::new(376, 320)) == DesktopAction::Idle && walk.screen == Screen::Login;

    let mut real = DesktopState::new();
    real.handle(DesktopKey::Activate);
    for byte in b"aer" {
        real.handle(DesktopKey::Character(*byte));
    }
    let username_captured = real.username_str() == "aer";
    real.handle(DesktopKey::Activate);
    for byte in b"secret" {
        real.handle(DesktopKey::Character(*byte));
    }
    real.handle(DesktopKey::Activate);
    real.handle(DesktopKey::Activate);
    for byte in b"secret" {
        real.handle(DesktopKey::Character(*byte));
    }
    real.handle(DesktopKey::Activate);
    let correct_password_signs_in = real.screen == Screen::Desktop;

    let mut lock = DesktopState::new();
    lock.handle(DesktopKey::Activate);
    lock.handle(DesktopKey::Activate);
    lock.handle(DesktopKey::Activate);
    let any_key_unlocks = lock.screen == Screen::Lock
        && lock.handle(DesktopKey::Character(b'x')) == DesktopAction::Redraw
        && lock.screen == Screen::Login;
    let mut skipped = DesktopState::new();
    skipped.handle(DesktopKey::Escape);
    let escape_skips = skipped.screen == Screen::Desktop;
    let mut hotkey = DesktopState::new();
    let dismissed = hotkey.handle(DesktopKey::Browser) == DesktopAction::Redraw
        && hotkey.screen == Screen::Desktop
        && hotkey.app == DesktopApp::Browser;
    advanced
        && bare_enter_blocked
        && wrong_password_blocked
        && click_does_not_sign_in
        && username_captured
        && correct_password_signs_in
        && any_key_unlocks
        && escape_skips
        && dismissed
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
        state.submit_url();
        state.browser_pending
            && state.browser_phase == BrowserPhase::Loading
            && state.browser_host_str() == "aeros.dev"
            && state.browser_path_str() == "/x"
    };
    simple && scheme_and_path && secure && empty && spaces && bare && typed && submit
}

fn typed_caret(bytes: &[u8]) -> ClockBuffer {
    let mut result = ClockBuffer::new();
    let _ = result.write_str(core::str::from_utf8(bytes).unwrap_or(""));
    let _ = result.write_str("_");
    result
}

fn masked_caret(len: usize) -> ClockBuffer {
    let mut result = ClockBuffer::new();
    for _ in 0..len.min(MAX_NAME) {
        let _ = result.write_str("*");
    }
    let _ = result.write_str("_");
    result
}

fn time_text() -> ClockBuffer {
    let current = crate::rtc::utc_date_time();
    let mut result = ClockBuffer::new();
    let _ = write!(result, "{:02}:{:02}", current.hour, current.minute);
    result
}

fn date_text() -> ClockBuffer {
    let current = crate::rtc::utc_date_time();
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
