//! The AerOS app store's model: the Linux applications the guest has
//! installed (found by its agent in the .desktop files), the catalogue of
//! ones that can be added, and the state of an install in progress. The
//! drawing and input live in `desktop.rs`.

use crate::svm::{self, LINUX_MAX_APPS, LinuxApp};

/// One installable application (a Flathub listing).
pub struct CatalogEntry {
    pub name: &'static str,
    /// Flathub application ID.
    pub flatpak: &'static str,
    pub category: &'static str,
    pub blurb: &'static str,
    pub long: &'static str,
    /// Approximate download size and age rating shown on the page.
    pub size: &'static str,
    pub age: &'static str,
    pub verified: bool,
}

/// 64x64 straight-alpha RGBA icons from Flathub, one per catalog entry in
/// order (built by tools\build-store-icons.ps1). An all-transparent icon
/// means the download failed and the store draws a letter tile instead.
pub static ICONS: &[u8] = include_bytes!("../../assets/store-icons.bin");
pub const ICON_SIZE: usize = 64;

/// The bitmap for a catalog app name, if it has real artwork.
pub fn icon_for_name(name: &str) -> Option<&'static [u8]> {
    let index = CATALOG
        .iter()
        .position(|entry| entry.name.eq_ignore_ascii_case(name))?;
    let bytes = ICON_SIZE * ICON_SIZE * 4;
    let icon = ICONS.get(index * bytes..(index + 1) * bytes)?;
    icon.chunks_exact(4)
        .any(|pixel| pixel[3] != 0)
        .then_some(icon)
}

/// Apps from Flathub (installed with flatpak inside the Linux guest).
pub const CATALOG: [CatalogEntry; 24] = [
    CatalogEntry {
        name: "Firefox",
        flatpak: "org.mozilla.firefox",
        category: "Web",
        blurb: "Fast, private web browser",
        long: "Firefox is a fast, independent browser from Mozilla with tracking protection, tabs, sync and thousands of extensions.",
        size: "82 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "VLC",
        flatpak: "org.videolan.VLC",
        category: "Media",
        blurb: "Plays almost any video",
        long: "VLC plays nearly every audio and video format, discs and network streams, with no extra codecs to hunt down.",
        size: "46 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "GIMP",
        flatpak: "org.gimp.GIMP",
        category: "Graphics",
        blurb: "Image editor and painter",
        long: "GIMP is a full-featured image editor: photo retouching, layers, masks, filters, and painting tools.",
        size: "105 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "Inkscape",
        flatpak: "org.inkscape.Inkscape",
        category: "Graphics",
        blurb: "Vector drawing",
        long: "Inkscape draws scalable vector graphics: logos, icons, diagrams and illustrations that stay sharp at any size.",
        size: "92 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "LibreOffice",
        flatpak: "org.libreoffice.LibreOffice",
        category: "Office",
        blurb: "Documents and sheets",
        long: "A complete office suite with a word processor, spreadsheets, presentations, drawings and databases.",
        size: "340 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "Thunderbird",
        flatpak: "org.mozilla.Thunderbird",
        category: "Office",
        blurb: "Email and calendar",
        long: "Thunderbird handles several mail accounts, a calendar and contacts, with filtering and strong privacy defaults.",
        size: "78 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "Audacity",
        flatpak: "org.audacityteam.Audacity",
        category: "Media",
        blurb: "Record and edit audio",
        long: "Audacity records audio and edits multi-track sound: cut, mix, add effects and export in many formats.",
        size: "41 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "Kdenlive",
        flatpak: "org.kde.kdenlive",
        category: "Media",
        blurb: "Video editor",
        long: "A multi-track video editor with titles, transitions, effects, proxy clips and a wide range of export formats.",
        size: "130 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "Text Editor",
        flatpak: "org.gnome.TextEditor",
        category: "Utilities",
        blurb: "Simple text editor",
        long: "A small, quick editor for notes and plain text files, with tabs, search and automatic recovery of unsaved work.",
        size: "3.2 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "Calculator",
        flatpak: "org.gnome.Calculator",
        category: "Utilities",
        blurb: "Everyday calculator",
        long: "Do sums, scientific and programming calculations, and unit and currency conversions, with a scrolling history.",
        size: "4.1 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "Documents",
        flatpak: "org.gnome.Evince",
        category: "Utilities",
        blurb: "PDF and document viewer",
        long: "Reads PDF, PostScript, and comic-book files with search, thumbnails, bookmarks and annotations.",
        size: "6 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "Loupe",
        flatpak: "org.gnome.Loupe",
        category: "Graphics",
        blurb: "Image viewer",
        long: "A fast, simple image viewer with smooth zoom and gestures that opens JPEG, PNG, WebP, SVG and more.",
        size: "5 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "Clocks",
        flatpak: "org.gnome.clocks",
        category: "Utilities",
        blurb: "World clocks and timers",
        long: "Shows the time around the world and sets alarms, a stopwatch and timers.",
        size: "3 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "Characters",
        flatpak: "org.gnome.Characters",
        category: "Utilities",
        blurb: "Find and copy symbols",
        long: "Browse emoji and every Unicode symbol by category or search, then copy them into any app.",
        size: "2 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "KeePassXC",
        flatpak: "org.keepassxc.KeePassXC",
        category: "Utilities",
        blurb: "Offline password manager",
        long: "Stores passwords in an encrypted local database, with a generator, autotype and browser integration.",
        size: "18 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "Telegram",
        flatpak: "org.telegram.desktop",
        category: "Social",
        blurb: "Fast, secure messaging",
        long: "Telegram sends messages, photos and files quickly, with group chats, channels, and cloud sync across devices.",
        size: "62 MB",
        age: "12+",
        verified: false,
    },
    CatalogEntry {
        name: "Discord",
        flatpak: "com.discordapp.Discord",
        category: "Social",
        blurb: "Chat, voice and video",
        long: "Discord is voice, video and text chat for communities and friends, organised into servers and channels.",
        size: "105 MB",
        age: "13+",
        verified: false,
    },
    CatalogEntry {
        name: "Spotify",
        flatpak: "com.spotify.Client",
        category: "Media",
        blurb: "Music streaming",
        long: "Stream millions of songs and podcasts, build playlists and follow artists. Needs an account.",
        size: "132 MB",
        age: "12+",
        verified: false,
    },
    CatalogEntry {
        name: "Obsidian",
        flatpak: "md.obsidian.Obsidian",
        category: "Office",
        blurb: "Linked notes",
        long: "Obsidian keeps notes as local Markdown files and links them into a graph of ideas you can search and explore.",
        size: "118 MB",
        age: "4+",
        verified: false,
    },
    CatalogEntry {
        name: "Blender",
        flatpak: "org.blender.Blender",
        category: "Graphics",
        blurb: "3D modelling and animation",
        long: "Blender is a complete 3D suite: modelling, sculpting, animation, simulation, rendering and video editing.",
        size: "250 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "OBS Studio",
        flatpak: "com.obsproject.Studio",
        category: "Media",
        blurb: "Record and stream",
        long: "OBS Studio records the screen and camera and streams live, with scenes, sources and audio mixing.",
        size: "36 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "Krita",
        flatpak: "org.kde.krita",
        category: "Graphics",
        blurb: "Digital painting",
        long: "Krita is a painting program for illustrators and concept artists, with brush engines and layers.",
        size: "190 MB",
        age: "4+",
        verified: true,
    },
    CatalogEntry {
        name: "Steam",
        flatpak: "com.valvesoftware.Steam",
        category: "Games",
        blurb: "Game store and library",
        long: "Buy, install and play thousands of games, with a friends list and cloud saves. Downloads games separately.",
        size: "4 MB",
        age: "12+",
        verified: true,
    },
    CatalogEntry {
        name: "Mousepad",
        flatpak: "org.xfce.mousepad",
        category: "Utilities",
        blurb: "Lightweight text editor",
        long: "A fast, simple editor with tabs, syntax highlighting, search and replace, and session restore.",
        size: "4.5 MB",
        age: "4+",
        verified: false,
    },
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StoreTab {
    Installed,
    Discover,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InstallState {
    Idle,
    Installing,
    Done,
    Failed,
}

pub struct Store {
    apps: [LinuxApp; LINUX_MAX_APPS],
    pub count: usize,
    pub install: InstallState,
    pub tab: StoreTab,
    /// Selected row in the current tab.
    pub selected: usize,
    /// Catalogue index being installed (for the progress text).
    pub installing: Option<usize>,
    pub started_ns: u64,
    seen: u32,
    /// Search text and the catalogue page open (None = the grid).
    query: [u8; QUERY_MAX],
    query_len: usize,
    pub detail: Option<usize>,
    /// First grid row shown.
    pub scroll: usize,
}

pub const QUERY_MAX: usize = 24;
pub const GRID_COLUMNS: usize = 3;
pub const GRID_ROWS: usize = 4;

struct Global(core::cell::UnsafeCell<Store>);

// SAFETY: only the desktop loop touches the store model.
unsafe impl Sync for Global {}

static STORE: Global = Global(core::cell::UnsafeCell::new(Store::new()));

pub fn get() -> &'static mut Store {
    // SAFETY: single user (the desktop loop).
    unsafe { &mut *STORE.0.get() }
}

impl Store {
    const fn new() -> Self {
        Self {
            apps: [LinuxApp::EMPTY; LINUX_MAX_APPS],
            count: 0,
            install: InstallState::Idle,
            tab: StoreTab::Discover,
            selected: 0,
            installing: None,
            started_ns: 0,
            seen: 0,
            query: [0; QUERY_MAX],
            query_len: 0,
            detail: None,
            scroll: 0,
        }
    }

    pub fn app(&self, index: usize) -> Option<&LinuxApp> {
        (index < self.count).then(|| &self.apps[index])
    }

    pub fn query(&self) -> &str {
        core::str::from_utf8(&self.query[..self.query_len]).unwrap_or("")
    }

    pub fn push_query(&mut self, byte: u8) -> bool {
        if self.query_len < QUERY_MAX && (0x20..0x7f).contains(&byte) {
            self.query[self.query_len] = byte;
            self.query_len += 1;
            self.scroll = 0;
            self.selected = 0;
            return true;
        }
        false
    }

    pub fn pop_query(&mut self) -> bool {
        if self.query_len == 0 {
            return false;
        }
        self.query_len -= 1;
        self.scroll = 0;
        self.selected = 0;
        true
    }

    fn matches(&self, index: usize) -> bool {
        let query = self.query();
        if query.is_empty() {
            return true;
        }
        match self.tab {
            StoreTab::Installed => contains_ignore_case(self.apps[index].name_str(), query),
            StoreTab::Discover => {
                let entry = &CATALOG[index];
                contains_ignore_case(entry.name, query)
                    || contains_ignore_case(entry.blurb, query)
                    || contains_ignore_case(entry.category, query)
            }
        }
    }

    /// Source indexes shown in the grid (after the search filter).
    pub fn shown(&self) -> usize {
        let total = match self.tab {
            StoreTab::Installed => self.count,
            StoreTab::Discover => CATALOG.len(),
        };
        (0..total).filter(|&index| self.matches(index)).count()
    }

    /// Source index of the `position`-th grid item.
    pub fn shown_index(&self, position: usize) -> Option<usize> {
        let total = match self.tab {
            StoreTab::Installed => self.count,
            StoreTab::Discover => CATALOG.len(),
        };
        (0..total)
            .filter(|&index| self.matches(index))
            .nth(position)
    }

    pub fn scroll_rows(&mut self, delta: i32) {
        let rows = self.shown().div_ceil(GRID_COLUMNS);
        let max = rows.saturating_sub(GRID_ROWS) as i32;
        self.scroll = (self.scroll as i32 + delta).clamp(0, max) as usize;
    }

    pub fn switch_tab(&mut self, tab: StoreTab) {
        if self.tab != tab {
            self.tab = tab;
            self.selected = 0;
            self.scroll = 0;
            self.detail = None;
        }
    }

    /// The installed guest app that is this catalogue entry, if any.
    pub fn installed_index(&self, entry: &CatalogEntry) -> Option<usize> {
        (0..self.count).find(|&index| {
            let name = self.apps[index].name_str();
            contains_ignore_case(name, entry.name)
                || self.apps[index].exec_str().contains(entry.flatpak)
        })
    }

    /// Up to four other catalogue entries: same category first.
    pub fn similar(&self, of: usize) -> [usize; 4] {
        let mut out = [usize::MAX; 4];
        let mut n = 0;
        for pass in 0..2 {
            for (index, other) in CATALOG.iter().enumerate() {
                if n == 4 {
                    break;
                }
                let same = other.category == CATALOG[of].category;
                if index != of && !out[..n].contains(&index) && (same == (pass == 0)) {
                    out[n] = index;
                    n += 1;
                }
            }
        }
        out
    }

    /// Picks up a changed application list / install state from the guest.
    pub fn tick(&mut self) {
        if let Some((count, state)) = svm::linux_apps(&mut self.seen, &mut self.apps) {
            self.count = count;
            self.install = match state {
                1 => InstallState::Installing,
                2 => InstallState::Done,
                3 => InstallState::Failed,
                _ => InstallState::Idle,
            };
            if self.install != InstallState::Installing {
                self.installing = None;
            }
        }
    }

    /// Starts installing catalogue entry `index` in the guest.
    pub fn install(&mut self, index: usize, now_ns: u64) -> bool {
        let Some(entry) = CATALOG.get(index) else {
            return false;
        };
        if self.install == InstallState::Installing || !svm::linux_agent_ready() {
            return false;
        }
        let mut command = [0u8; 120];
        let mut len = 0;
        for part in ["aeros-install flathub:", entry.flatpak] {
            for byte in part.bytes() {
                command[len] = byte;
                len += 1;
            }
        }
        svm::linux_command(3, [0; 4], &command[..len]);
        self.install = InstallState::Installing;
        self.installing = Some(index);
        self.started_ns = now_ns;
        true
    }

    /// Launches installed application `index`.
    pub fn launch(&self, index: usize) -> bool {
        let Some(app) = self.app(index) else {
            return false;
        };
        svm::linux_command(3, [0; 4], app.exec_str().as_bytes());
        true
    }
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    let (haystack, needle) = (haystack.as_bytes(), needle.as_bytes());
    if needle.is_empty() || needle.len() > haystack.len() {
        return needle.is_empty();
    }
    haystack
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}
