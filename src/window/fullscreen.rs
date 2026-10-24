//! Detects games and other fullscreen apps on the notch's monitor so the
//! notch can step aside (hidden windows receive no input at all).
//!
//! Out-of-context WinEvent hooks report foreground changes and moves/resizes
//! of the foreground window; nothing is polled.

use std::sync::atomic::{AtomicBool, Ordering};

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromWindow};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::Shell::{QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_LOCATIONCHANGE, EVENT_SYSTEM_FOREGROUND, GWL_STYLE, GetClassNameW, GetForegroundWindow,
    GWL_EXSTYLE, GetWindowLongPtrW, GetWindowRect, IsWindowVisible, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS, WS_CAPTION, WS_EX_TOOLWINDOW,
};

use crate::app::UserEvent;

/// Coalesces the (very frequent) location events into one pending check.
static PENDING: AtomicBool = AtomicBool::new(false);

/// Shell surfaces that briefly cover the whole monitor without being an
/// app: desktop, taskbars, task switcher / Task View, the transparent
/// staging window Windows 11 shows while switching apps, Start and search.
const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "ForegroundStaging",
    "XamlExplorerHostIslandWindow",
    "MultitaskingViewFrame",
    "TaskSwitcherWnd",
    "Windows.UI.Core.CoreWindow",
    "LockScreenControllerProxyWindow",
    "TopLevelWindowForOverflowXamlIsland",
];

unsafe extern "system" fn on_win_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    if event == EVENT_OBJECT_LOCATIONCHANGE && (object != OBJID_WINDOW.0 || hwnd != unsafe { GetForegroundWindow() }) {
        return;
    }
    if !PENDING.swap(true, Ordering::AcqRel) {
        super::win32::post(UserEvent::ForegroundChanged);
    }
}

/// Keeps the hooks installed; they are delivered on the installing (UI)
/// thread through its message loop.
pub struct ForegroundHooks(Vec<HWINEVENTHOOK>);

impl ForegroundHooks {
    pub fn install() -> Self {
        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        let hooks = [EVENT_SYSTEM_FOREGROUND, EVENT_OBJECT_LOCATIONCHANGE]
            .into_iter()
            .map(|event| unsafe { SetWinEventHook(event, event, None, Some(on_win_event), 0, 0, flags) })
            .filter(|hook| !hook.is_invalid())
            .collect();
        Self(hooks)
    }
}

impl Drop for ForegroundHooks {
    fn drop(&mut self) {
        for hook in self.0.drain(..) {
            unsafe {
                let _ = UnhookWinEvent(hook);
            }
        }
    }
}

/// True while a fullscreen app (exclusive D3D, presentation mode, or a
/// borderless window covering the whole monitor the notch is on) is in front.
pub fn fullscreen_app_active(own: HWND) -> bool {
    PENDING.store(false, Ordering::Release);
    unsafe {
        if let Ok(state) = SHQueryUserNotificationState()
            && (state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_PRESENTATION_MODE)
        {
            return true;
        }
        let foreground = GetForegroundWindow();
        if foreground.is_invalid() || foreground == own {
            return false;
        }
        let mut class = [0u16; 64];
        let len = GetClassNameW(foreground, &mut class) as usize;
        let class = String::from_utf16_lossy(&class[..len]);
        if SHELL_CLASSES.contains(&class.as_str()) || !IsWindowVisible(foreground).as_bool() {
            return false;
        }
        let ex_style = GetWindowLongPtrW(foreground, GWL_EXSTYLE) as u32;
        if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
            return false;
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(foreground, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4);
        if cloaked != 0 {
            return false;
        }
        let monitor = MonitorFromWindow(foreground, MONITOR_DEFAULTTONULL);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        // Only apps on the notch's own monitor matter.
        let own_monitor = MonitorFromWindow(own, MONITOR_DEFAULTTONEAREST);
        if monitor.is_invalid() || monitor != own_monitor || !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }
        let mut rect = RECT::default();
        if GetWindowRect(foreground, &mut rect).is_err() {
            return false;
        }
        let m = info.rcMonitor;
        let covers = rect.left <= m.left && rect.top <= m.top && rect.right >= m.right && rect.bottom >= m.bottom;
        // Maximized windows with a title bar can cover the monitor when the
        // taskbar auto-hides; they are not fullscreen.
        let style = GetWindowLongPtrW(foreground, GWL_STYLE) as u32;
        let fullscreen = covers && style & WS_CAPTION.0 != WS_CAPTION.0;
        if fullscreen {
            log::debug!("fullscreen candidate: class {class:?}");
        }
        fullscreen    }
}
