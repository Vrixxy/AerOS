//! The Terminal's screen while a program runs in it: a VT100-style grid the
//! program's output draws on (cursor addressing, erase, scroll regions,
//! insert and delete, the alternate screen full-screen programs use), and the
//! answers to the questions it asks the terminal (where the cursor is, what
//! the terminal is), which go back to its input.

pub(super) const ROWS: usize = super::SHELL_HISTORY_LINES;
pub(super) const COLS: usize = super::SHELL_LINE_MAX;
const PARAMS: usize = 8;

type Grid = [[u8; COLS]; ROWS];

/// A cell's attributes: bold, reverse video, and in the high four bits its
/// colour (an ANSI colour number + 1; 0 is the terminal's own).
pub(super) const BOLD: u8 = 1;
pub(super) const REVERSE: u8 = 2;

pub(super) fn colour(attribute: u8) -> Option<u8> {
    (attribute >> 4).checked_sub(1)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Parse {
    Ground,
    Escape,
    Csi,
    Osc,
    /// `ESC (`, `ESC )` and the like take one more byte, the character set.
    Charset,
}

#[derive(Clone, Copy)]
pub(super) struct Screen {
    pub(super) cells: [[u8; COLS]; ROWS],
    pub(super) attrs: [[u8; COLS]; ROWS],
    /// What the next character is written with (SGR).
    attribute: u8,
    pub(super) row: usize,
    pub(super) col: usize,
    /// The last column was written: the next character starts a new line.
    wrap_pending: bool,
    top: usize,
    bottom: usize,
    saved: (usize, usize),
    /// The main screen while the alternate one is shown.
    main: Option<(Grid, Grid, usize, usize)>,
    /// Arrow keys are sent as `ESC O x` (DECCKM) instead of `ESC [ x`.
    pub(super) app_cursor: bool,
    pub(super) cursor_visible: bool,
    /// A line feed returns to the first column too (ONLCR for output that
    /// did not pass a terminal).
    pub(super) crlf: bool,
    parse: Parse,
    params: [u16; PARAMS],
    count: usize,
    private: bool,
    replies: [u8; 32],
    replies_len: usize,
}

impl Screen {
    pub(super) const EMPTY: Self = Self {
        cells: [[b' '; COLS]; ROWS],
        attrs: [[0; COLS]; ROWS],
        attribute: 0,
        row: 0,
        col: 0,
        wrap_pending: false,
        top: 0,
        bottom: ROWS - 1,
        saved: (0, 0),
        main: None,
        app_cursor: false,
        cursor_visible: true,
        crlf: true,
        parse: Parse::Ground,
        params: [0; PARAMS],
        count: 0,
        private: false,
        replies: [0; 32],
        replies_len: 0,
    };

    /// Starts from the lines already on show, the cursor on a fresh line
    /// below them (the oldest scroll away when they fill the screen).
    pub(super) fn load(&mut self, lines: &[&[u8]]) {
        *self = Self::EMPTY;
        let keep = lines.len().min(ROWS - 1);
        for (row, line) in lines[lines.len() - keep..].iter().enumerate() {
            let length = line.len().min(COLS);
            self.cells[row][..length].copy_from_slice(&line[..length]);
        }
        self.row = keep;
    }

    /// The rows in use: up to the cursor or the last row with text, which
    /// is what the scrollback keeps once the program is done.
    pub(super) fn used_rows(&self) -> usize {
        let written = self
            .cells
            .iter()
            .rposition(|line| line.iter().any(|&cell| cell != b' '))
            .map_or(0, |row| row + 1);
        let cursor = if self.col > 0 { self.row + 1 } else { self.row };
        written.max(cursor).min(ROWS)
    }

    /// Row `row` without its trailing blanks.
    pub(super) fn line(&self, row: usize) -> &[u8] {
        let line = &self.cells[row];
        let end = line
            .iter()
            .rposition(|&cell| cell != b' ')
            .map_or(0, |at| at + 1);
        &line[..end]
    }

    /// Leaves the alternate screen if the program did not.
    pub(super) fn finish(&mut self) {
        if let Some((cells, attrs, row, col)) = self.main.take() {
            self.cells = cells;
            self.attrs = attrs;
            self.row = row;
            self.col = col;
        }
    }

    /// Takes the answers owed to the program.
    pub(super) fn take_replies(&mut self, out: &mut [u8]) -> usize {
        let count = self.replies_len.min(out.len());
        out[..count].copy_from_slice(&self.replies[..count]);
        self.replies_len = 0;
        count
    }

    fn reply(&mut self, bytes: &[u8]) {
        let room = self.replies.len() - self.replies_len;
        let count = bytes.len().min(room);
        self.replies[self.replies_len..self.replies_len + count].copy_from_slice(&bytes[..count]);
        self.replies_len += count;
    }

    pub(super) fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.feed_byte(byte);
        }
    }

    fn feed_byte(&mut self, byte: u8) {
        match self.parse {
            Parse::Escape => {
                self.parse = Parse::Ground;
                match byte {
                    b'[' => {
                        self.parse = Parse::Csi;
                        self.params = [0; PARAMS];
                        self.count = 0;
                        self.private = false;
                    }
                    b']' => self.parse = Parse::Osc,
                    b'(' | b')' | b'*' | b'+' | b'#' => self.parse = Parse::Charset,
                    b'7' => self.saved = (self.row, self.col),
                    b'8' => {
                        (self.row, self.col) = self.saved;
                        self.wrap_pending = false;
                    }
                    b'D' => self.line_feed(),
                    b'E' => {
                        self.col = 0;
                        self.line_feed();
                    }
                    b'M' => self.reverse_index(),
                    b'c' => *self = Self::EMPTY,
                    _ => {}
                }
            }
            Parse::Charset => self.parse = Parse::Ground,
            Parse::Osc => {
                if byte == 0x07 {
                    self.parse = Parse::Ground;
                } else if byte == 0x1b {
                    self.parse = Parse::Escape;
                }
            }
            Parse::Csi => match byte {
                b'0'..=b'9' => {
                    let slot = self.count.min(PARAMS - 1);
                    self.params[slot] = self.params[slot]
                        .saturating_mul(10)
                        .saturating_add(u16::from(byte - b'0'));
                }
                b';' => self.count = (self.count + 1).min(PARAMS - 1),
                b'?' | b'>' | b'=' => self.private = true,
                0x40..=0x7e => {
                    self.parse = Parse::Ground;
                    self.count += 1;
                    self.csi(byte);
                }
                _ => {}
            },
            Parse::Ground => match byte {
                0x1b => self.parse = Parse::Escape,
                b'\r' => {
                    self.col = 0;
                    self.wrap_pending = false;
                }
                b'\n' | 0x0b | 0x0c => {
                    self.wrap_pending = false;
                    if self.crlf {
                        self.col = 0;
                    }
                    self.line_feed();
                }
                0x08 => {
                    self.col = self.col.saturating_sub(1);
                    self.wrap_pending = false;
                }
                b'\t' => {
                    self.col = ((self.col / 8 + 1) * 8).min(COLS - 1);
                    self.wrap_pending = false;
                }
                0x20..=0x7e => self.put(byte),
                // SO/SI pick the line-drawing set and back: drawn as ASCII.
                0x0e | 0x0f => {}
                _ => {}
            },
        }
    }

    fn put(&mut self, byte: u8) {
        if self.wrap_pending {
            self.wrap_pending = false;
            self.col = 0;
            self.line_feed();
        }
        self.cells[self.row][self.col] = byte;
        self.attrs[self.row][self.col] = self.attribute;
        if self.col + 1 == COLS {
            self.wrap_pending = true;
        } else {
            self.col += 1;
        }
    }

    fn line_feed(&mut self) {
        if self.row == self.bottom {
            self.scroll_up(1);
        } else if self.row + 1 < ROWS {
            self.row += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.row == self.top {
            self.scroll_down(1);
        } else {
            self.row = self.row.saturating_sub(1);
        }
    }

    /// Columns `from..to` of `row` become blank (and plain).
    fn blank(&mut self, row: usize, from: usize, to: usize) {
        self.cells[row][from..to].fill(b' ');
        self.attrs[row][from..to].fill(0);
    }

    fn blank_rows(&mut self, from: usize, to: usize) {
        for row in from..to {
            self.blank(row, 0, COLS);
        }
    }

    /// Lines `top..=bottom` move up `count`, blank lines come in at the bottom.
    fn scroll_up(&mut self, count: usize) {
        let (top, bottom) = (self.top, self.bottom);
        let count = count.min(bottom + 1 - top);
        self.cells.copy_within(top + count..=bottom, top);
        self.attrs.copy_within(top + count..=bottom, top);
        self.blank_rows(bottom + 1 - count, bottom + 1);
    }

    fn scroll_down(&mut self, count: usize) {
        let (top, bottom) = (self.top, self.bottom);
        let count = count.min(bottom + 1 - top);
        self.cells.copy_within(top..=bottom - count, top + count);
        self.attrs.copy_within(top..=bottom - count, top + count);
        self.blank_rows(top, top + count);
    }

    /// SGR: bold, reverse video and the eight colours (bright ones too);
    /// 256-colour and RGB colours end the list unread.
    fn select_rendition(&mut self) {
        for index in 0..self.count.max(1) {
            match self.params[index] {
                0 => self.attribute = 0,
                1 => self.attribute |= BOLD,
                22 => self.attribute &= !BOLD,
                7 => self.attribute |= REVERSE,
                27 => self.attribute &= !REVERSE,
                code @ (30..=37 | 90..=97) => {
                    let number = (code % 10) as u8;
                    self.attribute = self.attribute & 0x0f | (number + 1) << 4;
                }
                39 => self.attribute &= 0x0f,
                38 | 48 => break,
                _ => {}
            }
        }
    }

    fn param(&self, index: usize, default: u16) -> usize {
        let value = if index < self.count {
            self.params[index]
        } else {
            0
        };
        usize::from(if value == 0 { default } else { value })
    }

    fn csi(&mut self, last: u8) {
        let first = self.param(0, 1);
        self.wrap_pending = false;
        if self.private {
            let set = match last {
                b'h' => true,
                b'l' => false,
                _ => return,
            };
            for index in 0..self.count {
                match self.params[index] {
                    1 => self.app_cursor = set,
                    25 => self.cursor_visible = set,
                    47 | 1047 | 1049 => self.alternate(set),
                    _ => {}
                }
            }
            return;
        }
        match last {
            b'A' => self.row = self.row.saturating_sub(first).max(self.top.min(self.row)),
            b'B' => self.row = (self.row + first).min(self.bottom.max(self.row)),
            b'C' => self.col = (self.col + first).min(COLS - 1),
            b'D' => self.col = self.col.saturating_sub(first),
            b'E' => {
                self.row = (self.row + first).min(ROWS - 1);
                self.col = 0;
            }
            b'F' => {
                self.row = self.row.saturating_sub(first);
                self.col = 0;
            }
            b'G' | b'`' => self.col = (first - 1).min(COLS - 1),
            b'd' => self.row = (first - 1).min(ROWS - 1),
            b'H' | b'f' => {
                self.row = (first - 1).min(ROWS - 1);
                self.col = (self.param(1, 1) - 1).min(COLS - 1);
            }
            b'J' => {
                let (row, col) = (self.row, self.col);
                match self.param(0, 0) {
                    0 => {
                        self.blank(row, col, COLS);
                        self.blank_rows(row + 1, ROWS);
                    }
                    1 => {
                        self.blank_rows(0, row);
                        self.blank(row, 0, col + 1);
                    }
                    _ => self.blank_rows(0, ROWS),
                }
            }
            b'K' => {
                let (row, col) = (self.row, self.col);
                match self.param(0, 0) {
                    0 => self.blank(row, col, COLS),
                    1 => self.blank(row, 0, col + 1),
                    _ => self.blank(row, 0, COLS),
                }
            }
            b'm' => self.select_rendition(),
            b'L' | b'M' if (self.top..=self.bottom).contains(&self.row) => {
                let saved_top = self.top;
                self.top = self.row;
                if last == b'L' {
                    self.scroll_down(first);
                } else {
                    self.scroll_up(first);
                }
                self.top = saved_top;
                self.col = 0;
            }
            b'P' => {
                let (row, col) = (self.row, self.col);
                let count = first.min(COLS - col);
                self.cells[row].copy_within(col + count.., col);
                self.attrs[row].copy_within(col + count.., col);
                self.blank(row, COLS - count, COLS);
            }
            b'@' => {
                let (row, col) = (self.row, self.col);
                let count = first.min(COLS - col);
                self.cells[row].copy_within(col..COLS - count, col + count);
                self.attrs[row].copy_within(col..COLS - count, col + count);
                self.blank(row, col, col + count);
            }
            b'X' => {
                let (row, col) = (self.row, self.col);
                let end = (col + first).min(COLS);
                self.blank(row, col, end);
            }
            b'S' => self.scroll_up(first),
            b'T' => self.scroll_down(first),
            b'r' => {
                let top = self.param(0, 1) - 1;
                let bottom = self.param(1, ROWS as u16).min(ROWS) - 1;
                if top < bottom {
                    (self.top, self.bottom) = (top, bottom);
                    (self.row, self.col) = (0, 0);
                }
            }
            b's' => self.saved = (self.row, self.col),
            b'u' => (self.row, self.col) = self.saved,
            b'n' => match self.param(0, 0) {
                5 => self.reply(b"\x1b[0n"),
                6 => {
                    let mut answer = [0u8; 16];
                    let length = position_reply(self.row + 1, self.col + 1, &mut answer);
                    self.reply(&answer[..length]);
                }
                _ => {}
            },
            b'c' => self.reply(b"\x1b[?1;2c"),
            _ => {}
        }
    }

    fn alternate(&mut self, on: bool) {
        if on && self.main.is_none() {
            self.main = Some((self.cells, self.attrs, self.row, self.col));
            self.blank_rows(0, ROWS);
            (self.row, self.col) = (0, 0);
        } else if !on {
            self.finish();
        }
        (self.top, self.bottom) = (0, ROWS - 1);
    }
}

/// `ESC [ row ; col R`.
fn position_reply(row: usize, col: usize, out: &mut [u8; 16]) -> usize {
    let mut length = 0;
    let mut push = |byte: u8| {
        out[length] = byte;
        length += 1;
    };
    push(0x1b);
    push(b'[');
    for (index, value) in [row, col].into_iter().enumerate() {
        if index == 1 {
            push(b';');
        }
        if value >= 10 {
            push(b'0' + (value / 10 % 10) as u8);
        }
        push(b'0' + (value % 10) as u8);
    }
    push(b'R');
    length
}

/// Drives the screen through what a line editor and a full-screen program
/// send: cursor addressing, erase, scrolling, insert and delete, the
/// alternate screen, and the cursor-position question.
#[cfg(feature = "boot-test")]
pub(super) fn self_test() -> bool {
    let mut screen = Screen::EMPTY;
    screen.load(&[b"earlier output".as_slice()]);
    screen.feed(b"$ ls\r\nfile\x1b[6n");
    let mut reply = [0u8; 32];
    let length = screen.take_replies(&mut reply);
    let position = &reply[..length] == b"\x1b[3;5R";
    let kept = screen.line(0) == b"earlier output" && screen.line(1) == b"$ ls";
    screen.feed(b"\x1b[?1049h\x1b[H\x1b[2Jtop line\x1b[14;1Hbottom\x1b[1;5Hx");
    let addressed = screen.line(0) == b"top xine" && screen.line(13) == b"bottom";
    screen.feed(b"\x1b[2;13r\x1b[13;1H\nnext\x1b[1;14r");
    let scrolled = screen.line(12) == b"next" && screen.line(0) == b"top xine";
    screen.feed(b"\x1b[1;1H\x1b[2P\x1b[1;1H\x1b[3@");
    let edited = screen.line(0) == b"   p xine";
    screen.feed(b"\x1b[?1049l");
    let restored = screen.line(0) == b"earlier output" && screen.line(2) == b"file";
    screen.feed(b"\rfi\x1b[Kx\r\n");
    let erased = screen.line(2) == b"fix" && screen.used_rows() == 3;
    screen.feed(b"a\x1b[7mB\x1b[1;31mC\x1b[0mD\x1b[4G\x1b[P");
    let row = screen.row;
    let attributes = screen.line(row) == b"aBC"
        && screen.attrs[row][..4] == [0, REVERSE, REVERSE | BOLD | (1 + 1) << 4, 0]
        && colour(screen.attrs[row][2]) == Some(1);
    position && kept && addressed && scrolled && edited && restored && erased && attributes
}
