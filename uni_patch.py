import io
import re


def load(p):
    return io.open(p, encoding='utf-8', newline='').read().replace('\r\n', '\n')


def save(p, s):
    io.open(p, 'w', encoding='utf-8', newline='\n').write(s)


def rep(s, old, new, count=1):
    assert old in s, old[:90]
    return s.replace(old, new, count)


# ---------------- desktop.rs ----------------
p = 'kernel/src/desktop.rs'
s = load(p)

# 1. Enum variant type
s = rep(s, "    Character(u8),\n}", "    Character(char),\n}")

# 2. KeyDecoder text_mode branch: decode full char via keymap::translate
s = rep(s, """        if self.text_mode {
            return match code {
                0x01 => Some(DesktopKey::Escape),
                0x0f => Some(DesktopKey::Tab),
                0x1c => Some(DesktopKey::Activate),
                0x0e => Some(DesktopKey::Backspace),
                _ => shell::scancode_character(code, self.shift, self.caps)
                    .map(DesktopKey::Character),
            };
        }""", """        if self.text_mode {
            return match code {
                0x01 => Some(DesktopKey::Escape),
                0x0f => Some(DesktopKey::Tab),
                0x1c => Some(DesktopKey::Activate),
                0x0e => Some(DesktopKey::Backspace),
                _ => match crate::keymap::translate(code, self.shift, self.caps) {
                    crate::keymap::Outcome::One(ch) => Some(DesktopKey::Character(ch)),
                    // An accent that did not combine: the mark itself has
                    // nowhere to go through this single-key interface, so
                    // (as with the old ASCII-only path) only the character
                    // typed after it comes through.
                    crate::keymap::Outcome::Two(_, second) => Some(DesktopKey::Character(second)),
                    crate::keymap::Outcome::Nothing => None,
                },
            };
        }""")

# 3. Login
s = rep(s, """                DesktopKey::Character(byte) => {
                    if self.push_login_byte(byte) {
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    }
                }
                _ => DesktopAction::Idle,
            };
        }
        if self.app == DesktopApp::Files && self.files_mode == FilesMode::NamingFolder {""",
        """                DesktopKey::Character(ch) => {
                    if ch.is_ascii() && self.push_login_byte(ch as u8) {
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    }
                }
                _ => DesktopAction::Idle,
            };
        }
        if self.app == DesktopApp::Files && self.files_mode == FilesMode::NamingFolder {""")

# 4. Files naming + Notes naming (both push_name_byte, identical bodies)
s = rep(s, """                DesktopKey::Character(byte) => {
                    if self.push_name_byte(byte) {
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    }
                }
                _ => DesktopAction::Idle,
            };
        }""", """                DesktopKey::Character(ch) => {
                    if ch.is_ascii() && self.push_name_byte(ch as u8) {
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    }
                }
                _ => DesktopAction::Idle,
            };
        }""", 2)

# 5. Notes editing: push_notes_byte -> push_notes_char (UTF-8)
s = rep(s, """                DesktopKey::Character(byte) => {
                    if self.push_notes_byte(byte) {
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    }
                }
                _ => DesktopAction::Idle,
            };
        }""", """                DesktopKey::Character(ch) => {
                    if self.push_notes_char(ch) {
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    }
                }
                _ => DesktopAction::Idle,
            };
        }""")

# 6. Keyboard screen search field (ASCII layout names)
s = rep(s, "                    DesktopKey::Character(byte) => return self.keyboard_type(byte),",
        """                    DesktopKey::Character(ch) => {
                        return if ch.is_ascii() {
                            self.keyboard_type(ch as u8)
                        } else {
                            DesktopAction::Idle
                        };
                    }""")

# 7. Setup Username/Password/Confirm
s = rep(s, """                DesktopKey::Character(byte) => {
                    let pushed = match self.screen {
                        Screen::Username => self.push_username_byte(byte),
                        Screen::Password => self.push_setup_password_byte(byte),
                        Screen::Confirm => self.push_confirm_byte(byte),
                        _ => false,
                    };""", """                DesktopKey::Character(ch) => {
                    let pushed = if ch.is_ascii() {
                        match self.screen {
                            Screen::Username => self.push_username_byte(ch as u8),
                            Screen::Password => self.push_setup_password_byte(ch as u8),
                            Screen::Confirm => self.push_confirm_byte(ch as u8),
                            _ => false,
                        }
                    } else {
                        false
                    };""")

# 8. Store search
s = rep(s, """                DesktopKey::Character(byte) => {
                    return if store.detail.is_none() && store.push_query(byte) {
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    };
                }""", """                DesktopKey::Character(ch) => {
                    return if ch.is_ascii() && store.detail.is_none() && store.push_query(ch as u8)
                    {
                        DesktopAction::Redraw
                    } else {
                        DesktopAction::Idle
                    };
                }""")

# 9. Browser URL
s = rep(s, """            DesktopKey::Character(byte) => {
                if self.app == DesktopApp::Browser && self.url_active && self.push_url_byte(byte) {
                    return DesktopAction::Redraw;
                }
                return DesktopAction::Idle;
            }""", """            DesktopKey::Character(ch) => {
                if self.app == DesktopApp::Browser
                    && self.url_active
                    && ch.is_ascii()
                    && self.push_url_byte(ch as u8)
                {
                    return DesktopAction::Redraw;
                }
                return DesktopAction::Idle;
            }""")

# 10. Terminal input character + backspace
s = rep(s, """                    DesktopKey::Character(byte) => {
                        redraw |= state.push_terminal_input_byte(byte);
                    }
                    DesktopKey::Backspace => {
                        state.terminal_input_len = state.terminal_input_len.saturating_sub(1);
                        redraw = true;
                    }""", """                    DesktopKey::Character(ch) => {
                        redraw |= state.push_terminal_input_char(ch);
                    }
                    DesktopKey::Backspace => {
                        state.terminal_input_len =
                            utf8_backspace(&state.terminal_input[..state.terminal_input_len]);
                        redraw = true;
                    }""")

# 11. push_terminal_input_byte -> push_terminal_input_char (UTF-8 aware)
s = rep(s, """    fn push_terminal_input_byte(&mut self, byte: u8) -> bool {
        if self.terminal_input_len >= SHELL_LINE_MAX || !byte.is_ascii_graphic() && byte != b' ' {
            return false;
        }
        self.terminal_input[self.terminal_input_len] = byte;
        self.terminal_input_len += 1;
        true
    }""", """    fn push_terminal_input_char(&mut self, ch: char) -> bool {
        if ch.is_ascii() && !ch.is_ascii_graphic() && ch != ' ' {
            return false;
        }
        let mut buffer = [0u8; 4];
        let encoded = ch.encode_utf8(&mut buffer).as_bytes();
        if self.terminal_input_len + encoded.len() > SHELL_LINE_MAX {
            return false;
        }
        self.terminal_input[self.terminal_input_len..self.terminal_input_len + encoded.len()]
            .copy_from_slice(encoded);
        self.terminal_input_len += encoded.len();
        true
    }""")

# 12. push_notes_byte -> push_notes_char (UTF-8 aware, via Text::push_str_checked)
old_notes = "    fn push_notes_byte(&mut self, byte: u8) -> bool {\n"
old_notes += "        if byte != b'\\n' && !(byte.is_ascii_graphic() || byte == b' ') {\n"
old_notes += "            return false;\n"
old_notes += "        }\n"
old_notes += "        self.notes_content.push_byte(byte)\n"
old_notes += "    }"
new_notes = "    fn push_notes_char(&mut self, ch: char) -> bool {\n"
new_notes += "        if ch.is_ascii() && ch != '\\n' && !(ch.is_ascii_graphic() || ch == ' ') {\n"
new_notes += "            return false;\n"
new_notes += "        }\n"
new_notes += "        let mut buffer = [0u8; 4];\n"
new_notes += "        self.notes_content\n"
new_notes += "            .push_str_checked(ch.encode_utf8(&mut buffer))\n"
new_notes += "    }"
s = rep(s, old_notes, new_notes)

# 13. utf8_backspace helper
s = rep(s, "fn ease_out_milli(progress_milli: u32) -> u32 {",
        """/// Removes the last complete UTF-8 scalar value's bytes from the end of
/// `bytes` (1-4 bytes, not just the last byte, so backspacing a
/// multi-byte character - e.g. an accented letter typed on a non-US
/// layout - erases the whole character in one press). Returns the new
/// length.
fn utf8_backspace(bytes: &[u8]) -> usize {
    let mut len = bytes.len();
    if len == 0 {
        return 0;
    }
    len -= 1;
    while len > 0 && bytes[len] & 0xc0 == 0x80 {
        len -= 1;
    }
    len
}

fn ease_out_milli(progress_milli: u32) -> u32 {""", 1)

save(p, s)

# ---------------- desktop_search.rs ----------------
p = 'kernel/src/desktop_search.rs'
s = load(p)
s = rep(s, """            DesktopKey::Character(byte)
                if self.search_len < SEARCH_MAX && (byte.is_ascii_graphic() || byte == b' ') =>
            {
                self.search_input[self.search_len] = byte;""",
        """            DesktopKey::Character(ch)
                if ch.is_ascii()
                    && self.search_len < SEARCH_MAX
                    && (ch.is_ascii_graphic() || ch == ' ') =>
            {
                self.search_input[self.search_len] = ch as u8;""")
save(p, s)

# ---------------- self-tests (desktop.rs) ----------------
p = 'kernel/src/desktop.rs'
s = load(p)
s = rep(s, "    walk.handle(DesktopKey::Character(b's'));\n    walk.handle(DesktopKey::Character(b'w'));",
        "    walk.handle(DesktopKey::Character('s'));\n    walk.handle(DesktopKey::Character('w'));")


def fix_loop(match):
    word = match.group(1)
    receiver = match.group(2)
    return 'for ch in "' + word + '".chars() {\n        ' + receiver + \
        '.handle(DesktopKey::Character(ch));\n    }'


pattern = re.compile(
    r'for byte in b"([^"]+)" \{\n\s*(\w+)\.handle\(DesktopKey::Character\(\*byte\)\);\n\s*\}'
)
s, count = pattern.subn(fix_loop, s)
assert count == 8, count

s = rep(s, "real.handle(DesktopKey::Character(b'x'));", "real.handle(DesktopKey::Character('x'));")

save(p, s)
print("patched ok")
