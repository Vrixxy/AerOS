//! The web page model behind the AerOS browser: history, the fetched page as
//! text lines, links, and the request to the Linux guest that does the
//! fetching (its `w3m` speaks TLS, which the kernel's own network stack
//! does not). The drawing lives in `desktop.rs`.

use crate::svm;

pub const PAGE_MAX: usize = 48_000;
pub const URL_MAX: usize = 200;
const HISTORY: usize = 12;
const MAX_LINES: usize = 3_000;
/// Characters per line requested from the guest's renderer.
pub const PAGE_COLS: u32 = 86;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WebState {
    /// Nothing opened yet (the start page shows).
    Blank,
    /// Waiting for the Linux guest's web agent to come up.
    Starting,
    /// The request is out; waiting for the page.
    Loading,
    Loaded,
    Failed,
}

pub struct Web {
    page: [u8; PAGE_MAX],
    page_len: usize,
    line_starts: [u32; MAX_LINES],
    line_count: usize,
    urls: [[u8; URL_MAX]; HISTORY],
    url_lens: [usize; HISTORY],
    history_len: usize,
    history_pos: usize,
    pub state: WebState,
    /// First visible line.
    pub scroll: usize,
    pub started_ns: u64,
    request_pending: bool,
    seen: u32,
}

struct Global(core::cell::UnsafeCell<Web>);

// SAFETY: only the desktop loop touches the browser model.
unsafe impl Sync for Global {}

static WEB: Global = Global(core::cell::UnsafeCell::new(Web::new()));

pub fn get() -> &'static mut Web {
    // SAFETY: single user (the desktop loop).
    unsafe { &mut *WEB.0.get() }
}

impl Web {
    const fn new() -> Self {
        Self {
            page: [0; PAGE_MAX],
            page_len: 0,
            line_starts: [0; MAX_LINES],
            line_count: 0,
            urls: [[0; URL_MAX]; HISTORY],
            url_lens: [0; HISTORY],
            history_len: 0,
            history_pos: 0,
            state: WebState::Blank,
            scroll: 0,
            started_ns: 0,
            request_pending: false,
            seen: 0,
        }
    }

    /// The address being shown (or loaded).
    pub fn url(&self) -> &str {
        if self.history_len == 0 {
            return "";
        }
        core::str::from_utf8(&self.urls[self.history_pos][..self.url_lens[self.history_pos]])
            .unwrap_or("")
    }

    pub fn can_go_back(&self) -> bool {
        self.history_pos > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.history_pos + 1 < self.history_len
    }

    pub fn line_count(&self) -> usize {
        self.line_count
    }

    pub fn line(&self, index: usize) -> &str {
        if index >= self.line_count {
            return "";
        }
        let start = self.line_starts[index] as usize;
        let end = if index + 1 < self.line_count {
            self.line_starts[index + 1] as usize - 1
        } else {
            self.page_len
        };
        core::str::from_utf8(&self.page[start..end.max(start)]).unwrap_or("")
    }

    /// Opens `url` (a new history entry). Anything without a scheme is
    /// given `https://` when it looks like a host name, or searched for.
    pub fn navigate(&mut self, url: &str, now_ns: u64) {
        let mut full = [0u8; URL_MAX];
        let len = normalize(url, &mut full);
        if len == 0 {
            return;
        }
        // A new page drops the forward history.
        self.history_len = self.history_pos + usize::from(self.history_len > 0);
        if self.history_len == HISTORY {
            self.urls.copy_within(1.., 0);
            self.url_lens.copy_within(1.., 0);
            self.history_len -= 1;
        }
        self.urls[self.history_len] = full;
        self.url_lens[self.history_len] = len;
        self.history_pos = self.history_len;
        self.history_len += 1;
        self.begin(now_ns);
    }

    pub fn reload(&mut self, now_ns: u64) {
        if self.history_len > 0 {
            self.begin(now_ns);
        }
    }

    pub fn back(&mut self, now_ns: u64) {
        if self.can_go_back() {
            self.history_pos -= 1;
            self.begin(now_ns);
        }
    }

    pub fn forward(&mut self, now_ns: u64) {
        if self.can_go_forward() {
            self.history_pos += 1;
            self.begin(now_ns);
        }
    }

    fn begin(&mut self, now_ns: u64) {
        self.state = if svm::linux_agent_ready() {
            WebState::Loading
        } else {
            WebState::Starting
        };
        self.request_pending = true;
        self.scroll = 0;
        self.page_len = 0;
        self.line_count = 0;
        self.started_ns = now_ns;
    }

    /// Whether the browser is waiting on the guest (so it must keep running).
    pub fn busy(&self) -> bool {
        matches!(self.state, WebState::Starting | WebState::Loading)
    }

    /// Drives the request and picks up the answer. Call every desktop loop
    /// iteration while the browser is open.
    pub fn tick(&mut self, now_ns: u64) {
        if self.request_pending && svm::linux_agent_ready() {
            let url = self.url();
            if svm::linux_web_request(url.as_bytes(), PAGE_COLS) {
                self.request_pending = false;
                self.state = WebState::Loading;
            }
        }
        if self.request_pending && now_ns.saturating_sub(self.started_ns) > 150_000_000_000 {
            self.fail("Linux did not start, so the page could not be fetched.");
            self.request_pending = false;
            return;
        }
        if self.request_pending {
            return;
        }
        if let Some((status, len)) = svm::linux_web_take(&mut self.seen, &mut self.page) {
            match status {
                2 => self.state = WebState::Loading,
                _ => {
                    self.page_len = len;
                    self.tidy();
                    self.index_lines();
                    self.state = if status == 0 {
                        WebState::Loaded
                    } else {
                        WebState::Failed
                    };
                    self.scroll = 0;
                }
            }
        }
    }

    fn fail(&mut self, message: &str) {
        let bytes = message.as_bytes();
        let len = bytes.len().min(PAGE_MAX);
        self.page[..len].copy_from_slice(&bytes[..len]);
        self.page_len = len;
        self.index_lines();
        self.state = WebState::Failed;
    }

    /// The page arrives as UTF-8; the bitmap fonts only cover ASCII, so common
    /// punctuation is mapped to look-alikes and everything else to `?`.
    fn tidy(&mut self) {
        let mut out = 0usize;
        let mut i = 0usize;
        while i < self.page_len {
            let byte = self.page[i];
            let replacement = if byte < 0x80 {
                i += 1;
                match byte {
                    b'\r' => continue,
                    b'\t' => b' ',
                    other => other,
                }
            } else {
                let width = if byte >= 0xf0 {
                    4
                } else if byte >= 0xe0 {
                    3
                } else {
                    2
                };
                let code = &self.page[i..(i + width).min(self.page_len)];
                i += width;
                match code {
                    [0xe2, 0x80, 0x98 | 0x99] => b'\'',
                    [0xe2, 0x80, 0x9c | 0x9d] => b'"',
                    [0xe2, 0x80, 0x93 | 0x94] => b'-',
                    [0xe2, 0x80, 0xa2] => b'*',
                    [0xc2, 0xa0] => b' ',
                    _ => b'?',
                }
            };
            self.page[out] = replacement;
            out += 1;
        }
        self.page_len = out;
    }

    fn index_lines(&mut self) {
        self.line_count = 0;
        if self.page_len == 0 {
            return;
        }
        self.line_starts[0] = 0;
        self.line_count = 1;
        for i in 0..self.page_len {
            if self.page[i] == b'\n' && i + 1 < self.page_len && self.line_count < MAX_LINES {
                self.line_starts[self.line_count] = i as u32 + 1;
                self.line_count += 1;
            }
        }
    }

    /// The address of reference `number` (the page's `[n]` markers and its
    /// "References" list at the end).
    pub fn reference(&self, number: u32) -> Option<&str> {
        for index in (0..self.line_count).rev() {
            let line = self.line(index);
            let trimmed = line.trim_start();
            let Some(rest) = trimmed.strip_prefix('[') else {
                continue;
            };
            let Some((digits, tail)) = rest.split_once(']') else {
                continue;
            };
            if digits.parse::<u32>().ok() == Some(number) {
                let target = tail.trim();
                if target.contains("://") {
                    return Some(target);
                }
            }
            // The references are a block at the end; stop once we're well above it.
            if self.line_count - index > 400 {
                break;
            }
        }
        None
    }

    pub fn scroll_by(&mut self, lines: i32, visible: usize) {
        let max = self.line_count.saturating_sub(visible);
        let next = (self.scroll as i32 + lines).clamp(0, max as i32);
        self.scroll = next as usize;
    }
}

/// Turns what was typed into an address: keeps `http(s)://...`, adds
/// `https://` to something that looks like a host name, and turns anything
/// else into a search. Returns the length written.
fn normalize(input: &str, out: &mut [u8; URL_MAX]) -> usize {
    let input = input.trim();
    if input.is_empty() {
        return 0;
    }
    let mut len = 0usize;
    let push = |text: &str, out: &mut [u8; URL_MAX], len: &mut usize| {
        for byte in text.bytes() {
            if *len < URL_MAX {
                out[*len] = byte;
                *len += 1;
            }
        }
    };
    let has_scheme = input.contains("://");
    let looks_like_host =
        !input.contains(' ') && (input.contains('.') || input.starts_with("localhost"));
    if has_scheme {
        push(input, out, &mut len);
    } else if looks_like_host {
        push("https://", out, &mut len);
        push(input, out, &mut len);
    } else {
        push("https://duckduckgo.com/lite/?q=", out, &mut len);
        for byte in input.bytes() {
            if byte.is_ascii_alphanumeric() {
                let one = [byte];
                push(core::str::from_utf8(&one).unwrap_or(""), out, &mut len);
            } else if byte == b' ' {
                push("+", out, &mut len);
            } else {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                let escaped = [b'%', HEX[(byte >> 4) as usize], HEX[(byte & 15) as usize]];
                push(core::str::from_utf8(&escaped).unwrap_or(""), out, &mut len);
            }
        }
    }
    len
}

/// The host part of an address, for the address bar's title (Safari style).
pub fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

#[allow(dead_code)]
pub fn is_secure(url: &str) -> bool {
    url.starts_with("https://")
}
