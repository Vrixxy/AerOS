use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::UI::Controls::Dialogs::{CC_ANYCOLOR, CC_ENABLEHOOK, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, HSTRING, PCWSTR, w};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

pub const TITLE: &str = "Animated Wallpapers";

#[derive(Clone, Copy)]
pub struct Monitor {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

unsafe extern "system" fn mon_cb(m: HMONITOR, _dc: HDC, _r: *mut RECT, l: LPARAM) -> BOOL {
    unsafe {
        let list = &mut *(l.0 as *mut Vec<Monitor>);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(m, &mut mi).as_bool() {
            let r = mi.rcMonitor;
            let mon = Monitor { x: r.left, y: r.top, w: (r.right - r.left) as u32, h: (r.bottom - r.top) as u32 };
            if mi.dwFlags & 1 != 0 {
                list.insert(0, mon); // primary first
            } else {
                list.push(mon);
            }
        }
    }
    BOOL(1)
}

pub fn monitors() -> Vec<Monitor> {
    let mut v: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(mon_cb), LPARAM(&mut v as *mut _ as isize));
    }
    if v.is_empty() {
        let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN) as u32, GetSystemMetrics(SM_CYSCREEN) as u32) };
        v.push(Monitor { x: 0, y: 0, w, h });
    }
    v
}

pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p).ok()? };
    Some((p.x, p.y))
}

static FOUND: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn enum_classic(hwnd: HWND, _l: LPARAM) -> BOOL {
    unsafe {
        if FindWindowExW(Some(hwnd), None, w!("SHELLDLL_DefView"), PCWSTR::null()).is_ok() {
            if let Ok(ww) = FindWindowExW(None, Some(hwnd), w!("WorkerW"), PCWSTR::null()) {
                FOUND.store(ww.0 as isize, Ordering::SeqCst);
            }
        }
    }
    BOOL(1)
}

/// Returns the parent window and, on the newer Windows 11 layout, the icon layer to sit just beneath.
fn wallpaper_host() -> Option<(HWND, Option<HWND>)> {
    unsafe {
        let progman = FindWindowW(w!("Progman"), PCWSTR::null()).ok()?;
        let mut res = 0usize;
        let _ = SendMessageTimeoutW(progman, 0x052C, WPARAM(0xD), LPARAM(1), SMTO_NORMAL, 1000, Some(&mut res));
        let _ = SendMessageTimeoutW(progman, 0x052C, WPARAM(0), LPARAM(0), SMTO_NORMAL, 1000, Some(&mut res));
        let mode = std::env::var("AW_MODE").unwrap_or_default();
        if mode == "workerw" {
            if let Ok(ww) = FindWindowExW(Some(progman), None, w!("WorkerW"), PCWSTR::null()) {
                return Some((ww, None));
            }
        }
        if mode == "bottom" {
            return Some((progman, Some(HWND_BOTTOM)));
        }
        if mode == "top" {
            return Some((progman, Some(HWND_TOP)));
        }
        if let Ok(defview) = FindWindowExW(Some(progman), None, w!("SHELLDLL_DefView"), PCWSTR::null()) {
            // newer layout: the real wallpaper is a WorkerW under the icons, so go between the two
            return Some((progman, Some(defview)));
        }
        FOUND.store(0, Ordering::SeqCst);
        let _ = EnumWindows(Some(enum_classic), LPARAM(0));
        let f = FOUND.load(Ordering::SeqCst);
        if f != 0 { Some((HWND(f as *mut _), None)) } else { None }
    }
}

static EMBEDDED: AtomicBool = AtomicBool::new(false);

/// True when the wallpaper window lives inside the desktop (like Lively), false when it is the plain top-level fallback.
pub fn embedded() -> bool {
    EMBEDDED.load(Ordering::SeqCst)
}

pub fn attach_behind_icons(window: &Window, m: Monitor) -> bool {
    let Ok(handle) = window.window_handle() else { return false };
    let RawWindowHandle::Win32(h) = handle.as_raw() else { return false };
    let hwnd = HWND(h.hwnd.get() as *mut _);
    if std::env::var("AW_MODE").as_deref() != Ok("toplevel") && attach_embedded(hwnd, m) {
        EMBEDDED.store(true, Ordering::SeqCst);
        return true;
    }
    attach_toplevel(hwnd, m)
}

fn attach_embedded(hwnd: HWND, m: Monitor) -> bool {
    let Some((host, below)) = wallpaper_host() else { return false };
    unsafe {
        let mut r = RECT::default();
        let _ = GetWindowRect(host, &mut r);
        let bad = WS_POPUP.0 | WS_CAPTION.0 | WS_THICKFRAME.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0;
        let style = (GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & !bad) | WS_CHILD.0 | WS_VISIBLE.0 | WS_CLIPSIBLINGS.0;
        SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let ex = (ex & !WS_EX_APPWINDOW.0) | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
        let _ = SetParent(hwnd, Some(host));
        let _ = SetWindowPos(
            hwnd,
            Some(below.unwrap_or(HWND_BOTTOM)),
            m.x - r.left,
            m.y - r.top,
            m.w as i32,
            m.h as i32 - 1,
            SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_SHOWWINDOW,
        );
        GetParent(hwnd).is_ok_and(|p| p == host)
    }
}

fn class_of(h: HWND) -> String {
    let mut b = [0u16; 64];
    let n = unsafe { GetClassNameW(h, &mut b) } as usize;
    String::from_utf16_lossy(&b[..n])
}

/// True when the foreground window covers (nearly) the whole monitor, e.g. a game or maximised app.
pub fn covered_by_fullscreen(m: Monitor) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() || !IsWindowVisible(fg).as_bool() || IsIconic(fg).as_bool() {
            return false;
        }
        let c = class_of(fg);
        if matches!(c.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Windows.UI.Core.CoreWindow") {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        if pid == std::process::id() {
            return false;
        }
        let mut r = RECT::default();
        if GetWindowRect(fg, &mut r).is_err() {
            return false;
        }
        let ix = (r.right.min(m.x + m.w as i32) - r.left.max(m.x)).max(0) as i64;
        let iy = (r.bottom.min(m.y + m.h as i32) - r.top.max(m.y)).max(0) as i64;
        ix * iy * 100 >= (m.w as i64 * m.h as i64) * 96
    }
}

pub fn battery_saver_on() -> bool {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s).is_ok() && s.SystemStatusFlag == 1 }
}

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_NAME: &str = "AnimatedWallpapers";

pub fn startup_enabled() -> bool {
    unsafe {
        let mut size = 0u32;
        RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), &HSTRING::from(RUN_NAME), RRF_RT_REG_SZ, None, None, Some(&mut size)).is_ok()
    }
}

pub fn set_startup(on: bool) {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), None, KEY_SET_VALUE, &mut key).is_err() {
            return;
        }
        if on {
            if let Ok(exe) = std::env::current_exe() {
                let val = format!("\"{}\"", exe.display());
                let wide: Vec<u16> = val.encode_utf16().chain(std::iter::once(0)).collect();
                let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
                let _ = RegSetValueExW(key, &HSTRING::from(RUN_NAME), None, REG_SZ, Some(bytes));
            }
        } else {
            let _ = RegDeleteValueW(key, &HSTRING::from(RUN_NAME));
        }
        let _ = RegCloseKey(key);
    }
}

static PREVIEW: AtomicU32 = AtomicU32::new(0);

/// The colour currently shown in the open colour dialog, updated while the user drags.
pub fn preview() -> Option<[u8; 3]> {
    let v = PREVIEW.load(Ordering::SeqCst);
    (v != 0).then(|| [((v >> 16) & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, (v & 0xFF) as u8])
}

unsafe extern "system" fn colour_hook(hdlg: HWND, msg: u32, wp: WPARAM, _lp: LPARAM) -> usize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetTimer(Some(hdlg), 1, 40, None);
            }
            WM_TIMER if wp.0 == 1 => {
                // the dialog's Red, Green and Blue boxes (ids 706 to 708) follow the picker as it moves
                let mut v = [0u32; 3];
                for (i, id) in [706, 707, 708].into_iter().enumerate() {
                    let mut ok = BOOL(0);
                    v[i] = GetDlgItemInt(hdlg, id, Some(&mut ok), false);
                    if !ok.as_bool() || v[i] > 255 {
                        return 0;
                    }
                }
                PREVIEW.store(0x0100_0000 | v[0] << 16 | v[1] << 8 | v[2], Ordering::SeqCst);
            }
            WM_DESTROY => {
                let _ = KillTimer(Some(hdlg), 1);
            }
            _ => {}
        }
        0
    }
}

/// Windows colour dialog with the full palette open. Blocks until closed, so call it off the main thread.
pub fn pick_colour(initial: [u8; 3]) -> Option<[u8; 3]> {
    PREVIEW.store(0, Ordering::SeqCst);
    let mut custom = [COLORREF(0x00FF_FFFF); 16];
    let mut cc = CHOOSECOLORW {
        lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
        rgbResult: COLORREF(initial[0] as u32 | (initial[1] as u32) << 8 | (initial[2] as u32) << 16),
        lpCustColors: custom.as_mut_ptr(),
        Flags: CC_FULLOPEN | CC_RGBINIT | CC_ANYCOLOR | CC_ENABLEHOOK,
        lpfnHook: Some(colour_hook),
        ..Default::default()
    };
    let ok = unsafe { ChooseColorW(&mut cc).as_bool() };
    PREVIEW.store(0, Ordering::SeqCst);
    if !ok {
        return None;
    }
    let c = cc.rgbResult.0;
    Some([(c & 0xFF) as u8, ((c >> 8) & 0xFF) as u8, ((c >> 16) & 0xFF) as u8])
}

pub fn open_folder(path: &std::path::Path) {
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(path.as_os_str()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

unsafe extern "system" fn close_cb(hwnd: HWND, l: LPARAM) -> BOOL {
    unsafe {
        close_if_ours(hwnd, l);
        let _ = EnumChildWindows(Some(hwnd), Some(close_child), l);
    }
    BOOL(1)
}

unsafe extern "system" fn close_child(hwnd: HWND, l: LPARAM) -> BOOL {
    unsafe { close_if_ours(hwnd, l) };
    BOOL(1)
}

unsafe fn close_if_ours(hwnd: HWND, l: LPARAM) {
    unsafe {
        let mut buf = [0u16; 32];
        let n = GetWindowTextW(hwnd, &mut buf) as usize;
        if String::from_utf16_lossy(&buf[..n]) == TITLE {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid != l.0 as u32 {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
    }
}

/// Asks every other running copy of the app to close.
pub fn close_others() {
    unsafe {
        let _ = EnumWindows(Some(close_cb), LPARAM(std::process::id() as isize));
    }
}

/// Show Desktop and "minimize all" shrink this window into a small stub at the bottom-left. Put it straight back.
fn hwnd_of(window: &Window) -> Option<HWND> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::Win32(h) = handle.as_raw() else { return None };
    Some(HWND(h.hwnd.get() as *mut _))
}

pub fn is_minimized(window: &Window) -> bool {
    hwnd_of(window).is_some_and(|h| unsafe { IsIconic(h).as_bool() })
}

/// Only call this once the window has stayed minimized for a moment, so the minimize has finished first.
pub fn restore_minimized(window: &Window, m: Monitor) {
    let Some(hwnd) = hwnd_of(window) else { return };
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        attach_toplevel(hwnd, m);
    }
}

/// Show Desktop raises the desktop window above this one. Keep this window directly above the desktop window
/// (so it hides the plain wallpaper but stays under every app).
pub fn keep_above_desktop(windows: &[&Window]) {
    let ours: Vec<HWND> = windows.iter().filter_map(|w| hwnd_of(w)).collect();
    if ours.is_empty() {
        return;
    }
    unsafe {
        let Ok(progman) = FindWindowW(w!("Progman"), PCWSTR::null()) else { return };
        let mut run = 0;
        let mut cur = GetWindow(progman, GW_HWNDPREV).ok();
        while let Some(c) = cur {
            if !ours.contains(&c) {
                break;
            }
            run += 1;
            cur = GetWindow(c, GW_HWNDPREV).ok();
        }
        if run == ours.len() {
            return;
        }
        let mut prev = cur.unwrap_or(HWND_TOP);
        for w in &ours {
            let _ = SetWindowPos(*w, Some(prev), 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE);
            prev = *w;
        }
    }
}

/// Fallback: a plain borderless window sitting just above the desktop and below every other window.
fn attach_toplevel(hwnd: HWND, m: Monitor) -> bool {
    unsafe {
        let Ok(progman) = FindWindowW(w!("Progman"), PCWSTR::null()) else { return false };
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let ex = (ex & !WS_EX_APPWINDOW.0) | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
        SetWindowPos(hwnd, Some(progman), m.x, m.y, m.w as i32, m.h as i32, SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_SHOWWINDOW).is_ok()
    }
}
