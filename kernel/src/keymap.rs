//! Keyboard layouts: turns key positions (PS/2 set-1 scancodes) into
//! characters for the chosen layout, with AltGr and dead keys (accents that
//! combine with the next letter).
//!
//! The legacy text path (`shell::scancode_character`) still returns ASCII
//! bytes only: with a non-US layout it produces the layout's ASCII characters
//! (y/z swapped on German, `"` and `@` swapped on UK, ...) and skips the
//! accented ones until the text inputs take UTF-8.

// The Settings app (UI pending) lists and selects layouts through the accessors.
#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

pub const DEAD_ACUTE: char = '\u{e001}';
pub const DEAD_GRAVE: char = '\u{e002}';
pub const DEAD_CIRCUMFLEX: char = '\u{e003}';
pub const DEAD_DIAERESIS: char = '\u{e004}';
pub const DEAD_TILDE: char = '\u{e005}';

/// Key groups of the main block, each a run of consecutive scancodes:
/// 0: ` (0x29)  1: digits 0x02..=0x0d  2: top row 0x10..=0x1b
/// 3: home row 0x1e..=0x28  4: \ / # (0x2b)  5: bottom row 0x2c..=0x35
/// 6: the extra ISO key next to left shift (0x56).
/// Each group has four columns: normal, shift, AltGr, shift+AltGr. A '\0'
/// (or a short string) means "no character".
const GROUPS: usize = 7;
type Keys = [[&'static str; 4]; GROUPS];

pub struct Layout {
    pub name: &'static str,
    pub code: &'static str,
    keys: Keys,
}

const NONE: [&str; 4] = ["", "", "", ""];

const US: Keys = [
    ["`", "~", "", ""],
    ["1234567890-=", "!@#$%^&*()_+", "", ""],
    ["qwertyuiop[]", "QWERTYUIOP{}", "", ""],
    ["asdfghjkl;'", "ASDFGHJKL:\"", "", ""],
    ["\\", "|", "", ""],
    ["zxcvbnm,./", "ZXCVBNM<>?", "", ""],
    NONE,
];

const UK: Keys = [
    ["`", "¬", "¦", ""],
    [
        "1234567890-=",
        "!\"£$%^&*()_+",
        "\0\0\0€\0\0\0\0\0\0\0\0",
        "",
    ],
    [
        "qwertyuiop[]",
        "QWERTYUIOP{}",
        "\0\0é\0\0\0úíó\0\0\0",
        "\0\0É\0\0\0ÚÍÓ\0\0\0",
    ],
    [
        "asdfghjkl;'",
        "ASDFGHJKL:@",
        "á\0\0\0\0\0\0\0\0\0\0",
        "Á\0\0\0\0\0\0\0\0\0\0",
    ],
    ["#", "~", "", ""],
    ["zxcvbnm,./", "ZXCVBNM<>?", "", ""],
    ["\\", "|", "", ""],
];

const DE: Keys = [
    ["\u{e003}", "°", "", ""],
    [
        "1234567890ß\u{e001}",
        "!\"§$%&/()=?\u{e002}",
        "\0²³\0\0\0{[]}\\\0",
        "",
    ],
    ["qwertzuiopü+", "QWERTZUIOPÜ*", "@\0€\0\0\0\0\0\0\0\0~", ""],
    ["asdfghjklöä", "ASDFGHJKLÖÄ", "", ""],
    ["#", "'", "", ""],
    ["yxcvbnm,.-", "YXCVBNM;:_", "\0\0\0\0\0\0µ\0\0\0", ""],
    ["<", ">", "|", ""],
];

const FR: Keys = [
    ["²", "", "", ""],
    ["&é\"'(-è_çà)=", "1234567890°+", "\0~#{[|`\\^@]}", ""],
    [
        "azertyuiop\u{e003}$",
        "AZERTYUIOP\u{e004}£",
        "\0\0€\0\0\0\0\0\0\0\0¤",
        "",
    ],
    ["qsdfghjklmù", "QSDFGHJKLM%", "", ""],
    ["*", "µ", "", ""],
    ["wxcvbn,;:!", "WXCVBN?./§", "", ""],
    ["<", ">", "", ""],
];

const ES: Keys = [
    ["º", "ª", "\\", ""],
    ["1234567890'¡", "!\"·$%&/()=?¿", "|@#~€¬\0\0\0\0\0\0", ""],
    [
        "qwertyuiop\u{e002}+",
        "QWERTYUIOP\u{e003}*",
        "\0\0€\0\0\0\0\0\0\0[]",
        "",
    ],
    [
        "asdfghjklñ\u{e001}",
        "ASDFGHJKLÑ\u{e004}",
        "\0\0\0\0\0\0\0\0\0\0{",
        "",
    ],
    ["ç", "Ç", "}", ""],
    ["zxcvbnm,.-", "ZXCVBNM;:_", "", ""],
    ["<", ">", "", ""],
];

const IT: Keys = [
    ["\\", "|", "", ""],
    [
        "1234567890'ì",
        "!\"£$%&/()=?^",
        "\0\0\0\0€\0\0\0\0\0\0\0",
        "",
    ],
    ["qwertyuiopè+", "QWERTYUIOPé*", "\0\0€\0\0\0\0\0\0\0[]", ""],
    ["asdfghjklòà", "ASDFGHJKLç°", "\0\0\0\0\0\0\0\0\0@#", ""],
    ["ù", "§", "", ""],
    ["zxcvbnm,.-", "ZXCVBNM;:_", "", ""],
    ["<", ">", "", ""],
];

const PT: Keys = [
    ["\\", "|", "", ""],
    ["1234567890'«", "!\"#$%&/()=?»", "\0@£§€\0{[]}\0\0", ""],
    [
        "qwertyuiop+\u{e001}",
        "QWERTYUIOP*\u{e002}",
        "\0\0€\0\0\0\0\0\0\0\0\0",
        "",
    ],
    ["asdfghjklçº", "ASDFGHJKLÇª", "", ""],
    ["\u{e005}", "\u{e003}", "", ""],
    ["zxcvbnm,.-", "ZXCVBNM;:_", "", ""],
    ["<", ">", "", ""],
];

const SE: Keys = [
    ["§", "½", "", ""],
    [
        "1234567890+\u{e001}",
        "!\"#¤%&/()=?\u{e002}",
        "\0@£$€\0{[]}\\\0",
        "",
    ],
    [
        "qwertyuiopå\u{e004}",
        "QWERTYUIOPÅ\u{e003}",
        "\0\0€\0\0\0\0\0\0\0\0\u{e005}",
        "",
    ],
    ["asdfghjklöä", "ASDFGHJKLÖÄ", "", ""],
    ["'", "*", "", ""],
    ["zxcvbnm,.-", "ZXCVBNM;:_", "\0\0\0\0\0\0µ\0\0\0", ""],
    ["<", ">", "|", ""],
];

const PL: Keys = [
    ["`", "~", "", ""],
    ["1234567890-=", "!@#$%^&*()_+", "", ""],
    [
        "qwertyuiop[]",
        "QWERTYUIOP{}",
        "\0\0ę\0\0\0\0\0ó\0\0\0",
        "\0\0Ę\0\0\0\0\0Ó\0\0\0",
    ],
    [
        "asdfghjkl;'",
        "ASDFGHJKL:\"",
        "ąś\0\0\0\0\0\0ł\0\0",
        "ĄŚ\0\0\0\0\0\0Ł\0\0",
    ],
    ["\\", "|", "", ""],
    [
        "zxcvbnm,./",
        "ZXCVBNM<>?",
        "żźć\0\0ń\0\0\0\0",
        "ŻŹĆ\0\0Ń\0\0\0\0",
    ],
    NONE,
];

const TR: Keys = [
    ["\"", "é", "", ""],
    ["1234567890*-", "!'^+%&/()=?_", "\0\0\0\0\0\0{[]}\\|", ""],
    ["qwertyuıopğü", "QWERTYUIOPĞÜ", "@\0€\0\0\0\0\0\0\0\0~", ""],
    ["asdfghjklşi", "ASDFGHJKLŞİ", "", ""],
    [",", ";", "", ""],
    ["zxcvbnmöç.", "ZXCVBNMÖÇ:", "", ""],
    ["<", ">", "|", ""],
];

const RU: Keys = [
    ["ё", "Ё", "", ""],
    ["1234567890-=", "!\"№;%:?*()_+", "", ""],
    ["йцукенгшщзхъ", "ЙЦУКЕНГШЩЗХЪ", "", ""],
    ["фывапролджэ", "ФЫВАПРОЛДЖЭ", "", ""],
    ["\\", "/", "", ""],
    ["ячсмитьбю.", "ЯЧСМИТЬБЮ,", "", ""],
    NONE,
];

const EL: Keys = [
    ["`", "~", "", ""],
    ["1234567890-=", "!@#$%^&*()_+", "", ""],
    [";ςερτυθιοπ[]", ":΅ΕΡΤΥΘΙΟΠ{}", "", ""],
    ["ασδφγηξκλ΄'", "ΑΣΔΦΓΗΞΚΛ¨\"", "", ""],
    ["\\", "|", "", ""],
    ["ζχψωβνμ,./", "ΖΧΨΩΒΝΜ<>?", "", ""],
    NONE,
];

pub static LAYOUTS: [Layout; 12] = [
    Layout {
        name: "English (US)",
        code: "us",
        keys: US,
    },
    Layout {
        name: "English (UK)",
        code: "gb",
        keys: UK,
    },
    Layout {
        name: "German",
        code: "de",
        keys: DE,
    },
    Layout {
        name: "French",
        code: "fr",
        keys: FR,
    },
    Layout {
        name: "Spanish",
        code: "es",
        keys: ES,
    },
    Layout {
        name: "Italian",
        code: "it",
        keys: IT,
    },
    Layout {
        name: "Portuguese",
        code: "pt",
        keys: PT,
    },
    Layout {
        name: "Swedish",
        code: "se",
        keys: SE,
    },
    Layout {
        name: "Polish",
        code: "pl",
        keys: PL,
    },
    Layout {
        name: "Turkish",
        code: "tr",
        keys: TR,
    },
    Layout {
        name: "Russian",
        code: "ru",
        keys: RU,
    },
    Layout {
        name: "Greek",
        code: "gr",
        keys: EL,
    },
];

static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static ALTGR: AtomicBool = AtomicBool::new(false);
/// The dead key waiting for the next character (0 = none).
static PENDING: AtomicU32 = AtomicU32::new(0);

pub fn layout_count() -> usize {
    LAYOUTS.len()
}

pub fn layout_index() -> usize {
    ACTIVE.load(Ordering::Acquire)
}

pub fn layout_name(index: usize) -> &'static str {
    LAYOUTS.get(index).map_or("?", |layout| layout.name)
}

pub fn layout_code(index: usize) -> &'static str {
    LAYOUTS.get(index).map_or("us", |layout| layout.code)
}

pub fn find_layout(code: &str) -> Option<usize> {
    LAYOUTS
        .iter()
        .take(layout_count())
        .position(|layout| layout.code == code)
}

pub fn set_layout(index: usize) {
    if index < layout_count() {
        ACTIVE.store(index, Ordering::Release);
        PENDING.store(0, Ordering::Release);
    }
}

/// Layout last pushed to the Linux guest (usize::MAX = none yet).
static GUEST_APPLIED: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Keeps the Linux guest's X keyboard layout in step with AerOS's: once the
/// guest's desktop is up, asks its agent to run `setxkbmap <layout>` whenever
/// the layout differs from the one it was last given. Called from the
/// desktop loop.
pub fn sync_guest() {
    #[cfg(feature = "linux-guest")]
    {
        let wanted = layout_index();
        if GUEST_APPLIED.load(Ordering::Relaxed) == wanted
            || crate::svm::linux_windows().is_none_or(|windows| windows.count == 0)
        {
            return;
        }
        let mut command = [0u8; 40];
        let text = b"setxkbmap ";
        command[..text.len()].copy_from_slice(text);
        let code = layout_code(wanted).as_bytes();
        command[text.len()..text.len() + code.len()].copy_from_slice(code);
        crate::svm::linux_command(3, [0; 4], &command[..text.len() + code.len() + 1]);
        GUEST_APPLIED.store(wanted, Ordering::Relaxed);
    }
}

/// The Right Alt key (AltGr) went down or up.
pub fn set_altgr(down: bool) {
    ALTGR.store(down, Ordering::Release);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// Nothing to type (modifier, dead key waiting, unmapped key).
    Nothing,
    One(char),
    /// An accent that didn't combine, then the character typed after it.
    Two(char, char),
}

fn key_at(layout: &Layout, code: u8) -> Option<(usize, usize)> {
    match code {
        0x29 => Some((0, 0)),
        0x02..=0x0d => Some((1, (code - 0x02) as usize)),
        0x10..=0x1b => Some((2, (code - 0x10) as usize)),
        0x1e..=0x28 => Some((3, (code - 0x1e) as usize)),
        0x2b => Some((4, 0)),
        0x2c..=0x35 => Some((5, (code - 0x2c) as usize)),
        0x56 => Some((6, 0)),
        _ => None,
    }
    .filter(|(group, index)| {
        layout.keys[*group]
            .iter()
            .any(|column| column.chars().nth(*index).is_some_and(|c| c != '\0'))
    })
}

fn character(layout: &Layout, group: usize, index: usize, column: usize) -> Option<char> {
    layout.keys[group][column]
        .chars()
        .nth(index)
        .filter(|c| *c != '\0')
}

fn accent_of(dead: char) -> char {
    match dead {
        DEAD_ACUTE => '´',
        DEAD_GRAVE => '`',
        DEAD_CIRCUMFLEX => '^',
        DEAD_DIAERESIS => '¨',
        _ => '~',
    }
}

/// The letter an accent turns `base` into, if it combines.
fn combine(dead: char, base: char) -> Option<char> {
    let (from, to): (&str, &str) = match dead {
        DEAD_ACUTE => ("aeiouyAEIOUYcCnNsSzZ", "áéíóúýÁÉÍÓÚÝćĆńŃśŚźŹ"),
        DEAD_GRAVE => ("aeiouAEIOU", "àèìòùÀÈÌÒÙ"),
        DEAD_CIRCUMFLEX => ("aeiouAEIOU", "âêîôûÂÊÎÔÛ"),
        DEAD_DIAERESIS => ("aeiouyAEIOUY", "äëïöüÿÄËÏÖÜŸ"),
        DEAD_TILDE => ("anoANO", "ãñõÃÑÕ"),
        _ => return None,
    };
    from.chars()
        .position(|c| c == base)
        .and_then(|at| to.chars().nth(at))
}

/// What the key at `code` types in the active layout with these modifiers.
pub fn translate(code: u8, shift: bool, caps: bool) -> Outcome {
    let layout = &LAYOUTS[layout_index().min(LAYOUTS.len() - 1)];
    let altgr = ALTGR.load(Ordering::Acquire);
    let typed = if code == 0x39 {
        Some(' ')
    } else if let Some((group, index)) = key_at(layout, code) {
        let normal = character(layout, group, index, 0);
        let mut chosen = if altgr {
            character(layout, group, index, if shift { 3 } else { 2 })
                .or_else(|| character(layout, group, index, 2))
        } else {
            None
        };
        if chosen.is_none() && !altgr {
            // Caps lock shifts letters only (the shifted form of a letter key).
            let letter = normal.is_some_and(|c| c.is_alphabetic());
            let shifted = shift ^ (caps && letter);
            chosen = if shifted {
                character(layout, group, index, 1).or(normal)
            } else {
                normal
            };
        }
        chosen
    } else {
        None
    };
    let Some(typed) = typed else {
        return Outcome::Nothing;
    };
    let pending = PENDING.swap(0, Ordering::AcqRel);
    let is_dead = ('\u{e001}'..='\u{e005}').contains(&typed);
    if pending != 0 {
        let dead = char::from_u32(pending).unwrap_or(DEAD_ACUTE);
        if is_dead {
            // Two accent keys in a row type the accent itself (or the second one waits).
            return if typed == dead {
                Outcome::One(accent_of(dead))
            } else {
                PENDING.store(typed as u32, Ordering::Release);
                Outcome::One(accent_of(dead))
            };
        }
        if typed == ' ' {
            return Outcome::One(accent_of(dead));
        }
        return match combine(dead, typed) {
            Some(composed) => Outcome::One(composed),
            None => Outcome::Two(accent_of(dead), typed),
        };
    }
    if is_dead {
        PENDING.store(typed as u32, Ordering::Release);
        return Outcome::Nothing;
    }
    Outcome::One(typed)
}

/// ASCII-only view for the legacy text inputs: the layout's ASCII characters
/// (composed accents and other non-ASCII characters yield nothing yet).
pub fn ascii(code: u8, shift: bool, caps: bool) -> Option<u8> {
    match translate(code, shift, caps) {
        Outcome::One(character) if character.is_ascii() => Some(character as u8),
        Outcome::Two(_, second) if second.is_ascii() => Some(second as u8),
        _ => None,
    }
}

#[cfg(feature = "boot-test")]
pub fn self_test() {
    use crate::serial;
    let mut failures = 0u32;
    let mut check = |layout: &str, code: u8, shift: bool, altgr: bool, expected: char| {
        set_layout(find_layout(layout).unwrap_or(0));
        set_altgr(altgr);
        let got = translate(code, shift, false);
        if got != Outcome::One(expected) {
            failures += 1;
            serial::format(format_args!(
                "AEROS_KEYMAP mismatch layout={} code={:#04x} shift={} altgr={} got={:?} want={:?}\n",
                layout, code, shift, altgr, got, expected
            ));
        }
    };
    // US baseline.
    check("us", 0x1e, false, false, 'a');
    check("us", 0x1e, true, false, 'A');
    check("us", 0x03, true, false, '@');
    // UK: " and @ swapped, pound sign, # key.
    check("gb", 0x03, true, false, '"');
    check("gb", 0x28, true, false, '@');
    check("gb", 0x04, true, false, '£');
    check("gb", 0x2b, false, false, '#');
    // German: y/z swapped, AltGr symbols, ß, umlauts.
    check("de", 0x15, false, false, 'z');
    check("de", 0x2c, false, false, 'y');
    check("de", 0x10, false, true, '@');
    check("de", 0x0c, false, false, 'ß');
    check("de", 0x1a, false, false, 'ü');
    check("de", 0x03, true, false, '"');
    check("de", 0x09, false, true, '[');
    // French AZERTY.
    check("fr", 0x10, false, false, 'a');
    check("fr", 0x1e, false, false, 'q');
    check("fr", 0x03, false, false, 'é');
    check("fr", 0x03, false, true, '~');
    check("fr", 0x02, true, false, '1');
    check("fr", 0x32, false, false, ',');
    // Spanish, Italian, Portuguese, Swedish, Polish, Turkish.
    check("es", 0x03, false, true, '@');
    check("es", 0x27, false, false, 'ñ');
    check("it", 0x1a, false, false, 'è');
    check("pt", 0x27, false, false, 'ç');
    check("se", 0x1a, false, false, 'å');
    check("pl", 0x1e, false, true, 'ą');
    check("pl", 0x2c, true, true, 'Ż');
    check("tr", 0x17, false, false, 'ı');
    check("tr", 0x28, true, false, 'İ');
    check("ru", 0x10, false, false, 'й');
    check("gr", 0x11, false, false, 'ς');
    // Dead keys: ´ then e = é; ` then a = à; ^ then space = ^; ´ then x = ´x.
    set_layout(find_layout("de").unwrap_or(0));
    set_altgr(false);
    let dead_acute = translate(0x0d, false, false);
    let composed = translate(0x12, false, false);
    let dead_grave = translate(0x0d, true, false);
    let grave_a = translate(0x1e, false, false);
    let circumflex = translate(0x29, false, false);
    let spaced = translate(0x39, false, false);
    let acute_again = translate(0x0d, false, false);
    let uncombined = translate(0x2d, false, false);
    let dead_ok = dead_acute == Outcome::Nothing
        && composed == Outcome::One('é')
        && dead_grave == Outcome::Nothing
        && grave_a == Outcome::One('à')
        && circumflex == Outcome::Nothing
        && spaced == Outcome::One('^')
        && acute_again == Outcome::Nothing
        && uncombined == Outcome::Two('´', 'x');
    // Caps lock capitalizes letters, not digits or punctuation.
    set_layout(0);
    let caps_letter = translate(0x1e, false, true);
    let caps_digit = translate(0x02, false, true);
    let caps_ok = caps_letter == Outcome::One('A') && caps_digit == Outcome::One('1');
    // ASCII view for the legacy inputs.
    set_layout(find_layout("de").unwrap_or(0));
    let ascii_ok = ascii(0x15, false, false) == Some(b'z') && ascii(0x1a, false, false).is_none();
    set_layout(0);
    set_altgr(false);
    let verified = failures == 0 && dead_ok && caps_ok && ascii_ok;
    serial::format(format_args!(
        "AEROS_KEYMAP layouts={} mismatches={} dead_keys={} caps={} ascii={} verified={}\n",
        layout_count(),
        failures,
        dead_ok,
        caps_ok,
        ascii_ok,
        verified
    ));
    if !verified {
        serial::line("AEROS_KEYMAP_INVARIANT_FAILURE");
        crate::arch::halt_forever();
    }
}
