use super::*;

const DOUBLE_CLICK_NS: u64 = 450_000_000;

impl DesktopState {
    /// Keyboard control of the Files grid; `None` leaves the key to the
    /// generic desktop handling.
    pub(super) fn files_key(&mut self, key: DesktopKey) -> Option<DesktopAction> {
        let count = self.files_entry_count;
        match key {
            DesktopKey::Left | DesktopKey::Right | DesktopKey::Up | DesktopKey::Down => {
                if count == 0 {
                    return Some(DesktopAction::Idle);
                }
                let columns = FILES_GRID_COLUMNS as usize;
                let at = self.files_selected;
                self.files_selected = if at >= count {
                    0
                } else {
                    match key {
                        DesktopKey::Left => at.saturating_sub(1),
                        DesktopKey::Right => (at + 1).min(count - 1),
                        DesktopKey::Up if at >= columns => at - columns,
                        DesktopKey::Down if at + columns < count => at + columns,
                        _ => at,
                    }
                };
                self.focus_visible = false;
                self.files_status.clear();
                Some(DesktopAction::Redraw)
            }
            DesktopKey::Activate if !self.focus_visible && self.files_selected < count => {
                Some(self.files_open_selected())
            }
            DesktopKey::Backspace => {
                self.files_up();
                Some(DesktopAction::Redraw)
            }
            DesktopKey::Delete if self.files_selected < count => {
                self.files_delete_selected();
                Some(DesktopAction::Redraw)
            }
            DesktopKey::Rename => {
                self.files_begin_rename();
                Some(DesktopAction::Redraw)
            }
            _ => None,
        }
    }

    pub(super) fn files_open_selected(&mut self) -> DesktopAction {
        let Some(row) = self.files_entries.get(self.files_selected).copied() else {
            return DesktopAction::Idle;
        };
        if row.is_dir {
            self.files_navigate(row.name_str());
        } else {
            self.files_open_text(row.name_str());
        }
        DesktopAction::Redraw
    }

    /// A click selects; a second click on the same item soon after opens it.
    pub(super) fn files_click_item(&mut self, index: usize) -> DesktopAction {
        let now = crate::time::monotonic_nanoseconds();
        let again = self.files_selected == index
            && now.saturating_sub(self.files_click_at_ns) < DOUBLE_CLICK_NS;
        self.files_click_at_ns = now;
        self.files_selected = index;
        self.files_status.clear();
        if again {
            return self.files_open_selected();
        }
        DesktopAction::Redraw
    }

    fn files_open_text(&mut self, name: &str) {
        let path = self.files_child_path(name);
        let mut buffer = [0u8; NOTE_MAX_BYTES];
        match shell::read_file(path.as_str(), &mut buffer) {
            Ok(length) => {
                let bytes = &buffer[..length];
                let text = match core::str::from_utf8(bytes) {
                    Ok(text) if !bytes.contains(&0) => text,
                    _ => {
                        self.files_set_status("not a text file");
                        return;
                    }
                };
                self.notes_content.clear();
                let _ = self.notes_content.push_str_checked(text);
                self.notes_current_name.clear();
                let _ = self.notes_current_name.push_str_checked(name);
                self.notes_target = path;
                self.notes_mode = NotesMode::Editing;
                self.notes_set_status("");
                self.set_app(DesktopApp::Notes);
            }
            Err(failure) => self.files_set_status(shell::vfs_error(failure)),
        }
    }

    /// Ctrl+C / Ctrl+X: remembers the selected item; a cut one moves when
    /// pasted, a copied one is duplicated (folders can only be moved).
    pub(super) fn files_copy_selected(&mut self, cut: bool) {
        let Some(row) = self.files_entries.get(self.files_selected).copied() else {
            self.files_set_status("no item selected");
            return;
        };
        if row.is_dir && !cut {
            self.files_set_status("folders can be moved (cut), not copied");
            return;
        }
        self.files_clip = self.files_child_path(row.name_str());
        self.files_clip_cut = cut;
        self.files_set_status(if cut {
            "cut - open a folder and paste to move it"
        } else {
            "copied - open a folder and paste"
        });
    }

    /// Ctrl+V: puts the remembered item in the folder being shown. An item
    /// that would replace another is refused.
    pub(super) fn files_paste(&mut self) {
        if self.files_clip.as_str().is_empty() {
            self.files_set_status("nothing to paste");
            return;
        }
        let source = self.files_clip;
        let name = source.as_str().rsplit('/').next().unwrap_or("");
        let target = self.files_child_path(name);
        if source.as_str() == target.as_str() {
            self.files_set_status("already here");
            return;
        }
        if vfs::metadata(target.as_str()).is_ok() {
            self.files_set_status("a file with that name is already here");
            return;
        }
        if self.files_clip_cut {
            match vfs::rename_noreplace(source.as_str(), target.as_str()) {
                Ok(()) => {
                    self.files_clip.clear();
                    self.files_set_status("moved");
                }
                Err(failure) => self.files_set_status(shell::vfs_error(failure)),
            }
        } else {
            match copy_file(source.as_str(), target.as_str()) {
                Ok(()) => self.files_set_status("copied"),
                Err(message) => self.files_set_status(message),
            }
        }
        self.files_reload();
        if let Some(index) =
            (0..self.files_entry_count).find(|&i| self.files_entries[i].name_str() == name)
        {
            self.files_selected = index;
        }
    }

    pub(super) fn files_begin_rename(&mut self) {
        let Some(row) = self.files_entries.get(self.files_selected).copied() else {
            self.files_set_status("no item selected");
            return;
        };
        self.name_input.clear();
        if !self.name_input.push_str_checked(row.name_str()) {
            self.name_input.clear();
            self.files_set_status("name too long to rename");
            return;
        }
        self.files_mode = FilesMode::Renaming;
    }

    pub(super) fn files_rename_selected(&mut self, new_name: &str) {
        let Some(row) = self.files_entries.get(self.files_selected).copied() else {
            self.files_set_status("no item selected");
            return;
        };
        if new_name.is_empty() {
            self.files_set_status("name cannot be empty");
            return;
        }
        if new_name == row.name_str() {
            return;
        }
        let from = self.files_child_path(row.name_str());
        let to = self.files_child_path(new_name);
        match vfs::rename_noreplace(from.as_str(), to.as_str()) {
            Ok(()) => self.files_set_status("renamed"),
            Err(failure) => self.files_set_status(shell::vfs_error(failure)),
        }
        self.files_reload();
        if let Some(index) =
            (0..self.files_entry_count).find(|&i| self.files_entries[i].name_str() == new_name)
        {
            self.files_selected = index;
        }
    }

    /// Keyboard control of the Trash list: arrows select, Enter restores,
    /// Delete removes for good.
    pub(super) fn trash_key(&mut self, key: DesktopKey) -> Option<DesktopAction> {
        let count = self.trash_entry_count;
        match key {
            DesktopKey::Up | DesktopKey::Down => {
                if count == 0 {
                    return Some(DesktopAction::Idle);
                }
                let at = self.trash_selected;
                self.trash_selected = if at >= count {
                    0
                } else if key == DesktopKey::Down {
                    (at + 1).min(count - 1)
                } else {
                    at.saturating_sub(1)
                };
                self.focus_visible = false;
                Some(DesktopAction::Redraw)
            }
            DesktopKey::Activate if !self.focus_visible && self.trash_selected < count => {
                self.trash_restore_selected();
                Some(DesktopAction::Redraw)
            }
            DesktopKey::Delete if self.trash_selected < count => {
                self.trash_delete_forever_selected();
                Some(DesktopAction::Redraw)
            }
            _ => None,
        }
    }

    /// Keyboard control of the Notes list; `None` leaves the key alone.
    pub(super) fn notes_list_key(&mut self, key: DesktopKey) -> Option<DesktopAction> {
        let count = self.notes_entry_count;
        match key {
            DesktopKey::Up | DesktopKey::Down => {
                if count == 0 {
                    return Some(DesktopAction::Idle);
                }
                let at = self.notes_selected;
                self.notes_selected = if at >= count {
                    0
                } else if key == DesktopKey::Down {
                    (at + 1).min(count - 1)
                } else {
                    at.saturating_sub(1)
                };
                self.focus_visible = false;
                Some(DesktopAction::Redraw)
            }
            DesktopKey::Activate if !self.focus_visible => {
                if let Some(row) = self.notes_entries.get(self.notes_selected).copied()
                    && self.notes_selected < count
                {
                    self.notes_open(row.name_str());
                } else {
                    self.notes_mode = NotesMode::Naming;
                    self.name_input.clear();
                }
                Some(DesktopAction::Redraw)
            }
            DesktopKey::Delete if self.notes_selected < count => {
                let row = self.notes_entries[self.notes_selected];
                self.notes_delete(row.name_str());
                Some(DesktopAction::Redraw)
            }
            _ => None,
        }
    }
}

#[cfg_attr(not(feature = "boot-test"), allow(dead_code))]
/// Duplicates a file in chunks; a failed copy leaves no partial target.
fn copy_file(source: &str, target: &str) -> Result<(), &'static str> {
    let input = vfs::open_file(source, false, false, false, 0, false).map_err(shell::vfs_error)?;
    let output = match vfs::open_file(target, true, true, false, 0o644, true) {
        Ok(output) => output,
        Err(failure) => {
            let _ = vfs::close(input);
            return Err(shell::vfs_error(failure));
        }
    };
    let mut chunk = [0u8; 4096];
    let mut failure = None;
    loop {
        match vfs::read(input, &mut chunk) {
            Ok(0) => break,
            Ok(count) => {
                if vfs::write(output, &chunk[..count], false) != Ok(count) {
                    failure = Some("not enough room");
                    break;
                }
            }
            Err(error) => {
                failure = Some(shell::vfs_error(error));
                break;
            }
        }
    }
    let _ = vfs::close(input);
    let _ = vfs::close(output);
    match failure {
        None => Ok(()),
        Some(message) => {
            let _ = vfs::remove(target, false);
            Err(message)
        }
    }
}

pub(super) fn files_self_test() -> bool {
    const BASE: &str = "/tmp/FTB";
    const SOURCE: &str = "/tmp/FTB/FTEST1.TXT";
    const RENAMED: &str = "/tmp/FTB/FTEST2.TXT";
    let _ = vfs::create_directory(BASE, 0o777);
    let _ = vfs::remove(SOURCE, false);
    let _ = vfs::remove(RENAMED, false);
    let Ok(descriptor) = vfs::open_file(SOURCE, true, false, true, 0o644, true) else {
        return false;
    };
    let wrote = vfs::write(descriptor, b"hello files", false).is_ok();
    let _ = vfs::close(descriptor);

    let mut state = DesktopState::new();
    state.screen = Screen::Desktop;
    state.open_files();
    state.files_navigate(BASE);
    let listed =
        (0..state.files_entry_count).find(|&i| state.files_entries[i].name_str() == "FTEST1.TXT");
    let found = wrote && listed.is_some();
    state.files_selected = listed.unwrap_or(usize::MAX);

    state.handle(DesktopKey::Rename);
    let prompted =
        state.files_mode == FilesMode::Renaming && state.name_input.as_str() == "FTEST1.TXT";
    state.name_input.clear();
    for ch in "FTEST2.TXT".chars() {
        state.handle(DesktopKey::Character(ch));
    }
    state.handle(DesktopKey::Activate);
    let renamed = state.files_mode == FilesMode::Browsing
        && vfs::metadata(RENAMED).is_ok()
        && vfs::metadata(SOURCE).is_err()
        && state
            .files_entries
            .get(state.files_selected)
            .map(|row| row.name_str())
            == Some("FTEST2.TXT");

    state.handle(DesktopKey::Activate);
    let opened = state.app == DesktopApp::Notes
        && state.notes_mode == NotesMode::Editing
        && state.notes_content.as_str() == "hello files"
        && state.notes_target.as_str() == RENAMED;
    state.handle(DesktopKey::Character('!'));
    state.handle(DesktopKey::Escape);
    let mut buffer = [0u8; 32];
    let saved_back = shell::read_file(RENAMED, &mut buffer)
        .map(|length| &buffer[..length] == b"hello files!")
        .unwrap_or(false);

    state.set_app(DesktopApp::Files);
    let stepped = state.handle(DesktopKey::Right) == DesktopAction::Redraw;
    let up = state.handle(DesktopKey::Backspace) == DesktopAction::Redraw
        && state.files_path.as_str() == "/tmp";
    // Copy, paste (refusing a name in use) and cut-and-paste between folders.
    let _ = vfs::create_directory("/tmp/FTB/FTD", 0o777);
    let _ = vfs::create_directory("/tmp/FTB/FTD2", 0o777);
    let mut clip = DesktopState::new();
    clip.screen = Screen::Desktop;
    clip.open_files();
    clip.files_navigate(BASE);
    clip.files_selected = (0..clip.files_entry_count)
        .find(|&i| clip.files_entries[i].name_str() == "FTEST2.TXT")
        .unwrap_or(usize::MAX);
    clip.files_copy_selected(false);
    clip.files_paste();
    let refused = clip.files_status.as_str().contains("already here");
    clip.files_navigate("/tmp/FTB/FTD");
    clip.files_paste();
    let mut copied_bytes = [0u8; 32];
    let copied = shell::read_file("/tmp/FTB/FTD/FTEST2.TXT", &mut copied_bytes)
        .map(|length| &copied_bytes[..length] == b"hello files!")
        .unwrap_or(false);
    clip.files_selected = (0..clip.files_entry_count)
        .find(|&i| clip.files_entries[i].name_str() == "FTEST2.TXT")
        .unwrap_or(usize::MAX);
    clip.files_copy_selected(true);
    clip.files_navigate("/tmp/FTB/FTD2");
    clip.files_paste();
    let moved = vfs::metadata("/tmp/FTB/FTD2/FTEST2.TXT").is_ok()
        && vfs::metadata("/tmp/FTB/FTD/FTEST2.TXT").is_err();
    let _ = vfs::remove("/tmp/FTB/FTD2/FTEST2.TXT", false);
    let _ = vfs::remove("/tmp/FTB/FTD", true);
    let _ = vfs::remove("/tmp/FTB/FTD2", true);
    let clipboard = refused && copied && moved;
    let _ = vfs::remove(RENAMED, false);
    let _ = vfs::remove(SOURCE, false);
    let _ = vfs::remove(BASE, true);
    let mut lists = DesktopState::new();
    lists.screen = Screen::Desktop;
    lists.set_app(DesktopApp::Trash);
    lists.trash_entry_count = 2;
    let first =
        lists.handle(DesktopKey::Down) == DesktopAction::Redraw && lists.trash_selected == 0;
    lists.handle(DesktopKey::Down);
    lists.handle(DesktopKey::Down);
    let trash = first && lists.trash_selected == 1;
    lists.set_app(DesktopApp::Notes);
    lists.notes_mode = NotesMode::List;
    lists.notes_entry_count = 3;
    lists.handle(DesktopKey::Down);
    lists.handle(DesktopKey::Down);
    lists.handle(DesktopKey::Up);
    let notes = lists.notes_selected == 0;
    let mut dock = DesktopState::new();
    dock.screen = Screen::Desktop;
    let center = |index: usize| {
        let bounds = dock_icon_rect(index);
        Point::new(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2)
    };
    dock.set_app(DesktopApp::Settings);
    dock.click(center(1));
    let from_settings = dock.app == DesktopApp::Terminal;
    dock.open_browser();
    dock.click(center(3));
    let from_browser = dock.app == DesktopApp::Files;
    let docked = from_settings && from_browser && dock.wifi == DesktopState::new().wifi;
    found
        && prompted
        && renamed
        && opened
        && saved_back
        && stepped
        && up
        && trash
        && notes
        && docked
        && clipboard
}
