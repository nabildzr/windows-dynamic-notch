//! Native Win32 configuration of the notch window.
//!
//! * Window styles: `WS_POPUP`, top-most, tool window (no taskbar / Alt+Tab
//!   entry), layered, no activation on click, no redirection bitmap.
//! * Per-pixel transparency through a DirectComposition visual tree that
//!   hosts the wgpu (DX12) swap chain, plus `DwmExtendFrameIntoClientArea(-1)`.
//! * Dynamic hit-testing: the original window procedure is subclassed with
//!   `SetWindowLongPtrW(GWLP_WNDPROC)` and `WM_NCHITTEST` answers `HTCLIENT`
//!   only inside the notch outline. `HTTRANSPARENT` alone only forwards input
//!   to windows of the *same thread*, so the window region is additionally
//!   shrunk to the notch bounds; outside of it clicks reach other apps.

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU32, Ordering};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, RemoveClipboardFormatListener, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow, SW_SHOWNORMAL};
use windows::core::{BOOL, HSTRING};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice2, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dwm::{
    DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_EXCLUDED_FROM_PEEK, DWMWA_NCRENDERING_POLICY,
    DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWINDOWATTRIBUTE,
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    CreateRectRgn, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
    MONITORINFOEXW, MonitorFromPoint, ScreenToClient, SetWindowRgn,
};
use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_LEFT, VK_RIGHT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallWindowProcW, HMENU, WM_CLIPBOARDUPDATE, CreatePopupMenu, DestroyMenu, GWL_EXSTYLE, GWL_STYLE, GWLP_WNDPROC,
    GetCursorPos, GetForegroundWindow, GetWindowLongPtrW, HWND_TOPMOST, MA_NOACTIVATE, MF_CHECKED, MF_GRAYED,
    MF_POPUP, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, PostMessageW, RegisterWindowMessageW, SW_HIDE, SW_SHOWNOACTIVATE,
    ShowWindow, WM_APP, WM_CONTEXTMENU, WM_LBUTTONUP, WM_RBUTTONUP, SPI_SETWORKAREA, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetForegroundWindow, SetLayeredWindowAttributes,
    SetWindowLongPtrW, SetWindowPos, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_DISPLAYCHANGE,
    WM_DPICHANGED, WM_HOTKEY, WM_MOUSEACTIVATE, WM_NCHITTEST, WM_NULL, WM_QUERYENDSESSION, WM_SETTINGCHANGE,
    WNDPROC, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE, LWA_ALPHA,
};
use windows::core::{PCWSTR, w};
use winit::event_loop::EventLoopProxy;

use super::dragdrop::DropRegistration;
use super::{CANVAS_HEIGHT, CANVAS_WIDTH, Edge};
use crate::app::UserEvent;

const HT_CLIENT: isize = 1;
const HT_TRANSPARENT: isize = -1;

pub const HOTKEY_PREV: i32 = 0x4E01;
pub const HOTKEY_NEXT: i32 = 0x4E02;

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("WindowsDynamicNotch");

// State shared with the subclassed window procedure. Everything runs on the
// UI thread, the atomics just make the statics `Sync`.
static ORIGINAL_WNDPROC: AtomicIsize = AtomicIsize::new(0);
static HIT_LEFT: AtomicI32 = AtomicI32::new(0);
static HIT_TOP: AtomicI32 = AtomicI32::new(0);
static HIT_RIGHT: AtomicI32 = AtomicI32::new(0);
static HIT_BOTTOM: AtomicI32 = AtomicI32::new(0);
static ALLOW_ACTIVATION: AtomicBool = AtomicBool::new(false);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// Callback message of the notification-area icon.
pub const WM_TRAY: u32 = WM_APP + 1;

thread_local! {
    static PROXY: RefCell<Option<EventLoopProxy<UserEvent>>> = const { RefCell::new(None) };
}

pub(crate) fn post(event: UserEvent) {
    PROXY.with(|proxy| {
        if let Some(proxy) = proxy.borrow().as_ref() {
            let _ = proxy.send_event(event);
        }
    });
}

/// Physical placement of the static canvas window on the primary monitor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasPlacement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    /// Thickness of the taskbar (or another app bar) docked on the edge the
    /// notch is attached to, in physical pixels; 0 when there is none.
    pub taskbar_inset: i32,
}

/// A connected display, as listed in the "Display" menu.
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorEntry {
    /// GDI device name such as `\\.\DISPLAY1` (stable while connected).
    pub device: String,
    pub width: i32,
    pub height: i32,
    pub primary: bool,
}

struct FoundMonitor {
    handle: HMONITOR,
    entry: MonitorEntry,
}

fn enumerate_monitors() -> Vec<FoundMonitor> {
    unsafe extern "system" fn collect(monitor: HMONITOR, _dc: HDC, _rect: *mut RECT, data: LPARAM) -> BOOL {
        let list = unsafe { &mut *(data.0 as *mut Vec<FoundMonitor>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) }.as_bool() {
            let len = info.szDevice.iter().position(|c| *c == 0).unwrap_or(info.szDevice.len());
            let rect = info.monitorInfo.rcMonitor;
            list.push(FoundMonitor {
                handle: monitor,
                entry: MonitorEntry {
                    device: String::from_utf16_lossy(&info.szDevice[..len]),
                    width: rect.right - rect.left,
                    height: rect.bottom - rect.top,
                    primary: info.monitorInfo.dwFlags & 1 != 0,
                },
            });
        }
        true.into()
    }
    let mut list: Vec<FoundMonitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut list as *mut Vec<FoundMonitor> as isize));
    }
    list
}

/// Connected displays, primary first.
pub fn monitors() -> Vec<MonitorEntry> {
    let mut list: Vec<MonitorEntry> = enumerate_monitors().into_iter().map(|m| m.entry).collect();
    list.sort_by_key(|m| (!m.primary, m.device.clone()));
    list
}

/// The display named `device`, or the primary one when it is not connected.
fn monitor_handle(device: Option<&str>) -> HMONITOR {
    device
        .and_then(|name| enumerate_monitors().into_iter().find(|m| m.entry.device == name).map(|m| m.handle))
        .unwrap_or_else(|| unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) })
}

/// Centers the 700x260 logical canvas horizontally on the chosen monitor
/// (primary by default), using its effective DPI, flush with the top
/// (Y = 0) or bottom edge. With `avoid_taskbar`, a taskbar docked on that
/// edge is kept visible: the canvas starts at the matching edge of the
/// monitor's work area instead.
pub fn canvas_placement(edge: Edge, avoid_taskbar: bool, device: Option<&str>) -> CanvasPlacement {
    unsafe {
        let monitor = monitor_handle(device);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).is_err() {
            dpi_x = 96;
        }
        let scale = (dpi_x as f64 / 96.0).max(0.5);
        let rect = info.rcMonitor;
        let width = (CANVAS_WIDTH * scale).round() as i32;
        let height = (CANVAS_HEIGHT * scale).round() as i32;
        let monitor_width = rect.right - rect.left;
        // The work area excludes app bars: a taskbar at the top pushes its top
        // edge down, one at the bottom pulls its bottom edge up. Auto-hidden
        // taskbars reserve nothing.
        let limit = (rect.bottom - rect.top) / 3;
        let taskbar_inset = match edge {
            Edge::Top => info.rcWork.top - rect.top,
            Edge::Bottom => rect.bottom - info.rcWork.bottom,
        }
        .clamp(0, limit);
        let offset = if avoid_taskbar { taskbar_inset } else { 0 };
        let y = match edge {
            Edge::Top => rect.top + offset,
            Edge::Bottom => rect.bottom - height - offset,
        };
        CanvasPlacement {
            x: rect.left + (monitor_width - width) / 2,
            y,
            width: width as u32,
            height: height as u32,
            scale,
            taskbar_inset,
        }
    }
}

/// Prevents a second copy of the app from running.
pub struct SingleInstance(HANDLE);

impl SingleInstance {
    pub fn acquire() -> Option<Self> {
        unsafe {
            let handle = CreateMutexW(None, true, w!("Local\\WindowsDynamicNotch.SingleInstance")).ok()?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                return None;
            }
            Some(Self(handle))
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// OLE (single-threaded apartment) is required on the UI thread for
/// `RegisterDragDrop` / `DoDragDrop`.
pub fn init_ole() -> windows::core::Result<()> {
    unsafe { OleInitialize(None) }
}

pub fn uninit_ole() {
    unsafe { OleUninitialize() }
}

/// DirectComposition tree: HWND target -> root visual -> swap chain content.
pub struct Composition {
    device: IDCompositionDevice,
    _target: IDCompositionTarget,
    visual: IDCompositionVisual,
}

impl Composition {
    fn new(hwnd: HWND) -> windows::core::Result<Self> {
        unsafe {
            let device: IDCompositionDevice = DCompositionCreateDevice2(None)?;
            let target = device.CreateTargetForHwnd(hwnd, true)?;
            let visual = device.CreateVisual()?;
            target.SetRoot(&visual)?;
            device.Commit()?;
            Ok(Self { device, _target: target, visual })
        }
    }

    /// Raw `IDCompositionVisual*` handed to wgpu, which sets the swap chain
    /// as the visual's content.
    pub fn visual_ptr(&self) -> *mut c_void {
        windows::core::Interface::as_raw(&self.visual)
    }

    pub fn commit(&self) {
        unsafe {
            if let Err(err) = self.device.Commit() {
                log::warn!("IDCompositionDevice::Commit failed: {err}");
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    Quit,
    ToggleAutostart,
    ToggleAvoidTaskbar,
    SetEdge(Edge),
    /// Index into [`MenuState::monitors`].
    SetMonitor(usize),
    /// Index into [`MenuState::widgets`].
    ToggleWidget(usize),
    /// Index into [`MenuState::accents`].
    SetAccent(usize),
    ToggleAlbumColors,
    ToggleLowBattery,
    EditLowBatteryThreshold,
    ToggleDiskWarning,
    EditDiskThreshold,
    ToggleLockIndicator,
    ToggleMediaIndicator,
    TogglePrivacyIndicator,
    ToggleNotifications,
    ToggleClipboard,
    ToggleHideInFullscreen,
    EditWeatherCity,
}

/// Current values shown as check marks in the context menu.
#[derive(Clone, Debug)]
pub struct MenuState {
    pub edge: Edge,
    pub avoid_taskbar: bool,
    /// A taskbar (or app bar) is docked on the edge the notch is attached to.
    pub taskbar_on_edge: bool,
    /// Connected displays and whether the notch is on each.
    pub monitors: Vec<(MonitorEntry, bool)>,
    /// Widget names and whether each is shown.
    pub widgets: Vec<(&'static str, bool)>,
    /// Accent names and which one is selected.
    pub accents: Vec<(&'static str, bool)>,
    pub album_colors: bool,
    pub low_battery: bool,
    pub low_battery_threshold: u8,
    pub disk_warning: bool,
    pub disk_threshold: u8,
    pub lock_indicator: bool,
    pub media_indicator: bool,
    pub privacy_indicator: bool,
    pub notifications: bool,
    pub clipboard: bool,
    pub hide_in_fullscreen: bool,
    pub weather_city: Option<String>,
}

const MENU_QUIT: usize = 1;
const MENU_AUTOSTART: usize = 2;
const MENU_AVOID_TASKBAR: usize = 3;
const MENU_EDGE_TOP: usize = 4;
const MENU_EDGE_BOTTOM: usize = 5;
const MENU_LOW_BATTERY: usize = 6;
const MENU_LOW_BATTERY_THRESHOLD: usize = 7;
const MENU_LOCK_INDICATOR: usize = 8;
const MENU_MEDIA_INDICATOR: usize = 9;
const MENU_PRIVACY_INDICATOR: usize = 10;
const MENU_HIDE_FULLSCREEN: usize = 11;
const MENU_ALBUM_COLORS: usize = 12;
const MENU_DISK_WARNING: usize = 13;
const MENU_DISK_THRESHOLD: usize = 14;
const MENU_NOTIFICATIONS: usize = 15;
const MENU_CLIPBOARD: usize = 16;
const MENU_WEATHER_CITY: usize = 17;
const MENU_MONITOR_BASE: usize = 100;
const MENU_WIDGET_BASE: usize = 200;
const MENU_ACCENT_BASE: usize = 300;

/// Builds the menu shared by the notch's right click and the tray icon.
unsafe fn build_menu(state: &MenuState) -> Option<HMENU> {
    unsafe {
        let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
        let item = |menu: HMENU, on: bool, id: usize, text: &str| {
            let _ = AppendMenuW(menu, MF_STRING | check(on), id, &HSTRING::from(text));
        };
        let submenu = |menu: HMENU, child: HMENU, text: &str| {
            // Submenus are destroyed together with their parent.
            let _ = AppendMenuW(menu, MF_POPUP, child.0 as usize, &HSTRING::from(text));
        };
        let separator = |menu: HMENU| {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        };
        let menu = CreatePopupMenu().ok()?;

        let position = CreatePopupMenu().ok()?;
        item(position, state.edge == Edge::Top, MENU_EDGE_TOP, "Top");
        item(position, state.edge == Edge::Bottom, MENU_EDGE_BOTTOM, "Bottom");
        separator(position);
        let mut avoid = MF_STRING | check(state.avoid_taskbar);
        if !state.taskbar_on_edge && !state.avoid_taskbar {
            // No taskbar on this edge; the option stays visible but inert.
            avoid |= MF_GRAYED;
        }
        let _ = AppendMenuW(position, avoid, MENU_AVOID_TASKBAR, w!("Keep taskbar visible"));
        if state.monitors.len() > 1 {
            separator(position);
            for (i, (monitor, on)) in state.monitors.iter().enumerate() {
                let name = monitor.device.trim_start_matches(r"\\.\");
                let primary = if monitor.primary { ", primary" } else { "" };
                let label = format!("{name} ({}\u{00D7}{}{primary})", monitor.width, monitor.height);
                item(position, *on, MENU_MONITOR_BASE + i, &label);
            }
        }
        submenu(menu, position, "Position");

        let widgets = CreatePopupMenu().ok()?;
        for (i, (name, on)) in state.widgets.iter().enumerate() {
            item(widgets, *on, MENU_WIDGET_BASE + i, name);
        }
        submenu(menu, widgets, "Widgets");

        let appearance = CreatePopupMenu().ok()?;
        for (i, (name, on)) in state.accents.iter().enumerate() {
            let _ = AppendMenuW(
                appearance,
                MF_STRING | if *on { MF_CHECKED } else { MF_UNCHECKED },
                MENU_ACCENT_BASE + i,
                &HSTRING::from(format!("Accent: {name}")),
            );
        }
        separator(appearance);
        item(appearance, state.album_colors, MENU_ALBUM_COLORS, "Music colors from album art");
        submenu(menu, appearance, "Appearance");

        let alerts = CreatePopupMenu().ok()?;
        item(alerts, state.low_battery, MENU_LOW_BATTERY, "Warn when battery is low");
        item(alerts, false, MENU_LOW_BATTERY_THRESHOLD, &format!("Battery warning level: {}%\u{2026}", state.low_battery_threshold));
        separator(alerts);
        item(alerts, state.disk_warning, MENU_DISK_WARNING, "Warn when disk C: is almost full");
        item(alerts, false, MENU_DISK_THRESHOLD, &format!("Disk warning level: {}% used\u{2026}", state.disk_threshold));
        submenu(menu, alerts, "Alerts");
        separator(menu);

        item(menu, state.media_indicator, MENU_MEDIA_INDICATOR, "Show volume && brightness");
        item(menu, state.lock_indicator, MENU_LOCK_INDICATOR, "Show Caps Lock && Num Lock");
        item(menu, state.privacy_indicator, MENU_PRIVACY_INDICATOR, "Camera, microphone && screen dots");
        item(menu, state.notifications, MENU_NOTIFICATIONS, "Notification popups");
        item(menu, state.clipboard, MENU_CLIPBOARD, "Clipboard history");
        item(menu, state.hide_in_fullscreen, MENU_HIDE_FULLSCREEN, "Hide in fullscreen apps");
        let city = state.weather_city.as_deref().unwrap_or("not set");
        item(menu, false, MENU_WEATHER_CITY, &format!("Weather city: {city}\u{2026}"));
        separator(menu);
        item(menu, autostart_enabled(), MENU_AUTOSTART, "Launch at startup");
        separator(menu);
        item(menu, false, MENU_QUIT, "Quit Dynamic Notch");
        Some(menu)
    }
}

fn menu_command(id: usize) -> Option<MenuCommand> {
    Some(match id {
        MENU_QUIT => MenuCommand::Quit,
        MENU_AUTOSTART => MenuCommand::ToggleAutostart,
        MENU_AVOID_TASKBAR => MenuCommand::ToggleAvoidTaskbar,
        MENU_EDGE_TOP => MenuCommand::SetEdge(Edge::Top),
        MENU_EDGE_BOTTOM => MenuCommand::SetEdge(Edge::Bottom),
        MENU_LOW_BATTERY => MenuCommand::ToggleLowBattery,
        MENU_LOW_BATTERY_THRESHOLD => MenuCommand::EditLowBatteryThreshold,
        MENU_LOCK_INDICATOR => MenuCommand::ToggleLockIndicator,
        MENU_MEDIA_INDICATOR => MenuCommand::ToggleMediaIndicator,
        MENU_PRIVACY_INDICATOR => MenuCommand::TogglePrivacyIndicator,
        MENU_HIDE_FULLSCREEN => MenuCommand::ToggleHideInFullscreen,
        MENU_ALBUM_COLORS => MenuCommand::ToggleAlbumColors,
        MENU_DISK_WARNING => MenuCommand::ToggleDiskWarning,
        MENU_DISK_THRESHOLD => MenuCommand::EditDiskThreshold,
        MENU_NOTIFICATIONS => MenuCommand::ToggleNotifications,
        MENU_CLIPBOARD => MenuCommand::ToggleClipboard,
        MENU_WEATHER_CITY => MenuCommand::EditWeatherCity,
        id if (MENU_MONITOR_BASE..MENU_WIDGET_BASE).contains(&id) => MenuCommand::SetMonitor(id - MENU_MONITOR_BASE),
        id if (MENU_WIDGET_BASE..MENU_ACCENT_BASE).contains(&id) => MenuCommand::ToggleWidget(id - MENU_WIDGET_BASE),
        id if id >= MENU_ACCENT_BASE => MenuCommand::SetAccent(id - MENU_ACCENT_BASE),
        _ => return None,
    })
}
/// Win32 state of the notch window.
pub struct NotchWindow {
    hwnd: HWND,
    composition: Composition,
    _drop_target: Option<DropRegistration>,
    region: Option<RECT>,
    hotkeys: bool,
    keyboard_focus: bool,
    previous_foreground: Option<HWND>,
    hidden: bool,
}

impl NotchWindow {
    /// Applies styles, DWM attributes, the WndProc subclass and
    /// DirectComposition to the HWND that winit created.
    pub fn attach(hwnd: HWND, proxy: EventLoopProxy<UserEvent>) -> windows::core::Result<Self> {
        PROXY.with(|slot| *slot.borrow_mut() = Some(proxy));
        unsafe {
            // Broadcast when Explorer (re)starts; the tray icon must be re-added.
            TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
            let original = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, notch_wndproc as *const () as isize);
            ORIGINAL_WNDPROC.store(original, Ordering::Relaxed);

            let style = WS_POPUP | WS_CLIPSIBLINGS | WS_CLIPCHILDREN;
            SetWindowLongPtrW(hwnd, GWL_STYLE, style.0 as isize);
            let ex_style = WS_EX_TOPMOST
                | WS_EX_TOOLWINDOW
                | WS_EX_NOACTIVATE
                | WS_EX_LAYERED
                | WS_EX_NOREDIRECTIONBITMAP;
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style.0 as isize);
            // A layered window stays invisible until its attributes are set;
            // full opacity lets the DirectComposition alpha decide per pixel.
            SetLayeredWindowAttributes(hwnd, Default::default(), 255, LWA_ALPHA)?;

            let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
            DwmExtendFrameIntoClientArea(hwnd, &margins)?;
            set_dwm_attribute(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, &1i32);
            set_dwm_attribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &DWMWCP_DONOTROUND);
            set_dwm_attribute(hwnd, DWMWA_NCRENDERING_POLICY, &DWMNCRP_DISABLED);
            set_dwm_attribute(hwnd, DWMWA_EXCLUDED_FROM_PEEK, &1i32);
            // DWMWA_COLOR_NONE: no Windows 11 accent border around the popup.
            set_dwm_attribute(hwnd, DWMWA_BORDER_COLOR, &0xFFFF_FFFEu32);
        }
        let composition = Composition::new(hwnd)?;
        // WM_CLIPBOARDUPDATE for the clipboard history (event driven).
        if let Err(err) = unsafe { AddClipboardFormatListener(hwnd) } {
            log::warn!("clipboard listener unavailable: {err}");
        }
        let drop_target = DropRegistration::register(hwnd)
            .inspect_err(|err| log::warn!("drag and drop unavailable: {err}"))
            .ok();
        Ok(Self {
            hwnd,
            composition,
            _drop_target: drop_target,
            region: None,
            hotkeys: false,
            keyboard_focus: false,
            previous_foreground: None,
            hidden: false,
        })
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn composition(&self) -> &Composition {
        &self.composition
    }

    /// Moves the canvas to its spot on the primary monitor (top or bottom
    /// center), keeps it top-most and shows it without activating it.
    pub fn place(&self, placement: &CanvasPlacement) {
        let show = if self.hidden { Default::default() } else { SWP_SHOWWINDOW };
        unsafe {
            if let Err(err) = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                placement.x,
                placement.y,
                placement.width as i32,
                placement.height as i32,
                SWP_NOACTIVATE | SWP_FRAMECHANGED | show,
            ) {
                log::warn!("SetWindowPos failed: {err}");
            }
            if !self.hidden {
                let style = GetWindowLongPtrW(self.hwnd, GWL_STYLE) | WS_VISIBLE.0 as isize;
                SetWindowLongPtrW(self.hwnd, GWL_STYLE, style);
            }
        }
    }

    /// Steps aside for fullscreen apps. A hidden window takes no input, so
    /// the whole screen edge belongs to the app again.
    pub fn set_hidden(&mut self, hidden: bool) {
        if hidden == self.hidden {
            return;
        }
        self.hidden = hidden;
        unsafe {
            let _ = ShowWindow(self.hwnd, if hidden { SW_HIDE } else { SW_SHOWNOACTIVATE });
        }
        if !hidden {
            self.raise();
        }
    }

    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// Re-asserts the top-most z-order (other top-most windows such as
    /// flyouts can be raised above the notch).
    pub fn raise(&self) {
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// Bounding box (client pixels) of the notch geometry, consulted by
    /// `WM_NCHITTEST`.
    pub fn set_hit_rect(&self, rect: RECT) {
        HIT_LEFT.store(rect.left, Ordering::Relaxed);
        HIT_TOP.store(rect.top, Ordering::Relaxed);
        HIT_RIGHT.store(rect.right, Ordering::Relaxed);
        HIT_BOTTOM.store(rect.bottom, Ordering::Relaxed);
    }

    /// Restricts where the window exists for the input system. Everything
    /// outside this rectangle is delivered to the windows underneath,
    /// regardless of which process owns them.
    pub fn set_input_region(&mut self, rect: RECT) {
        if self.region == Some(rect) {
            return;
        }
        self.region = Some(rect);
        unsafe {
            let region = CreateRectRgn(rect.left, rect.top, rect.right, rect.bottom);
            // The system owns the region after a successful call.
            if SetWindowRgn(self.hwnd, Some(region), false) == 0 {
                log::warn!("SetWindowRgn failed");
            }
        }
    }

    /// While the cursor rests on the expanded notch, the arrow keys switch
    /// widgets without stealing focus from the foreground application.
    pub fn set_navigation_hotkeys(&mut self, enabled: bool) {
        if enabled == self.hotkeys {
            return;
        }
        self.hotkeys = enabled;
        unsafe {
            if enabled {
                if let Err(err) = RegisterHotKey(Some(self.hwnd), HOTKEY_PREV, MOD_NOREPEAT, VK_LEFT.0 as u32) {
                    log::debug!("RegisterHotKey(Left) failed: {err}");
                }
                if let Err(err) = RegisterHotKey(Some(self.hwnd), HOTKEY_NEXT, MOD_NOREPEAT, VK_RIGHT.0 as u32) {
                    log::debug!("RegisterHotKey(Right) failed: {err}");
                }
            } else {
                let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_PREV);
                let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_NEXT);
            }
        }
    }

    /// The window never activates on click (media buttons must not steal
    /// focus). Text entry temporarily lifts that restriction.
    pub fn set_keyboard_focus(&mut self, enabled: bool) {
        if enabled == self.keyboard_focus {
            return;
        }
        self.keyboard_focus = enabled;
        ALLOW_ACTIVATION.store(enabled, Ordering::Relaxed);
        unsafe {
            let ex_style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            if enabled {
                let foreground = GetForegroundWindow();
                self.previous_foreground = (foreground != self.hwnd && !foreground.is_invalid()).then_some(foreground);
                SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex_style & !(WS_EX_NOACTIVATE.0 as isize));
                let _ = SetForegroundWindow(self.hwnd);
            } else {
                SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex_style | WS_EX_NOACTIVATE.0 as isize);
                if GetForegroundWindow() == self.hwnd
                    && let Some(previous) = self.previous_foreground.take()
                {
                    let _ = SetForegroundWindow(previous);
                }
            }
        }
    }

    /// Right-click / tray menu. Returns the chosen command.
    pub fn show_context_menu(&mut self, state: &MenuState) -> Option<MenuCommand> {
        unsafe {
            let menu = build_menu(state)?;
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            // The menu only dismisses correctly when its owner is foreground.
            let had_focus = self.keyboard_focus;
            self.set_keyboard_focus(true);
            let command = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
                cursor.x,
                cursor.y,
                None,
                self.hwnd,
                None,
            );
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            if !had_focus {
                self.set_keyboard_focus(false);
            }
            menu_command(command.0 as usize)
        }
    }
}
impl Drop for NotchWindow {
    fn drop(&mut self) {
        self.set_navigation_hotkeys(false);
        unsafe {
            let _ = RemoveClipboardFormatListener(self.hwnd);
        }
    }
}

unsafe fn set_dwm_attribute<T>(hwnd: HWND, attribute: DWMWINDOWATTRIBUTE, value: &T) {
    let result = unsafe {
        DwmSetWindowAttribute(hwnd, attribute, value as *const T as *const c_void, size_of::<T>() as u32)
    };
    if let Err(err) = result {
        log::debug!("DwmSetWindowAttribute({}) failed: {err}", attribute.0);
    }
}

fn point_from_lparam(lparam: LPARAM) -> POINT {
    POINT { x: (lparam.0 & 0xFFFF) as i16 as i32, y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32 }
}

fn inside_hit_rect(point: POINT) -> bool {
    point.x >= HIT_LEFT.load(Ordering::Relaxed)
        && point.x < HIT_RIGHT.load(Ordering::Relaxed)
        && point.y >= HIT_TOP.load(Ordering::Relaxed)
        && point.y < HIT_BOTTOM.load(Ordering::Relaxed)
}

unsafe extern "system" fn notch_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => {
            let mut point = point_from_lparam(lparam);
            unsafe {
                let _ = ScreenToClient(hwnd, &mut point);
            }
            return LRESULT(if inside_hit_rect(point) { HT_CLIENT } else { HT_TRANSPARENT });
        }
        WM_MOUSEACTIVATE if !ALLOW_ACTIVATION.load(Ordering::Relaxed) => {
            return LRESULT(MA_NOACTIVATE as isize);
        }
        WM_HOTKEY => {
            post(UserEvent::Hotkey(wparam.0 as i32));
            return LRESULT(0);
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED => post(UserEvent::DisplayChanged),
        WM_SETTINGCHANGE if wparam.0 == SPI_SETWORKAREA.0 as usize => post(UserEvent::DisplayChanged),
        WM_QUERYENDSESSION => post(UserEvent::SessionEnding),
        WM_CLIPBOARDUPDATE => {
            post(UserEvent::ClipboardChanged);
            return LRESULT(0);
        }
        WM_TRAY => {
            let event = (lparam.0 & 0xFFFF) as u32;
            if matches!(event, WM_LBUTTONUP | WM_RBUTTONUP | WM_CONTEXTMENU) {
                post(UserEvent::TrayMenu);
            }
            return LRESULT(0);
        }
        _ if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::Relaxed) => post(UserEvent::TrayRecreate),
        _ => {}
    }
    unsafe {
        let original: WNDPROC = std::mem::transmute(ORIGINAL_WNDPROC.load(Ordering::Relaxed));
        CallWindowProcW(original, hwnd, msg, wparam, lparam)
    }
}

/// Opens a file with its default application.
pub fn open_path(path: &Path) -> bool {
    unsafe {
        // We received the click, so we may hand foreground rights on.
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        let result = ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(path.as_os_str()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        result.0 as isize > 32
    }
}

/// Unicode text currently on the clipboard.
pub fn clipboard_text(hwnd: HWND) -> Option<String> {
    unsafe {
        OpenClipboard(Some(hwnd)).ok()?;
        let text = (|| {
            let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let global = HGLOBAL(handle.0);
            let ptr = GlobalLock(global) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let mut len = 0usize;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(global);
            Some(text)
        })();
        let _ = CloseClipboard();
        text
    }
}

/// Replaces the clipboard with `text`.
pub fn set_clipboard_text(hwnd: HWND, text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return false;
        }
        let ok = (|| {
            EmptyClipboard().ok()?;
            let global = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2).ok()?;
            let ptr = GlobalLock(global) as *mut u16;
            if ptr.is_null() {
                return None;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            let _ = GlobalUnlock(global);
            // The system owns the memory after a successful call.
            SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(global.0))).ok()?;
            Some(())
        })()
        .is_some();
        let _ = CloseClipboard();
        ok
    }
}

/// Password managers and other sensitive sources mark their clipboard
/// content as not to be recorded (the same convention Windows' own
/// clipboard history honors).
pub fn clipboard_is_private() -> bool {
    unsafe {
        let exclude = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
        let ignore = RegisterClipboardFormatW(w!("Clipboard Viewer Ignore"));
        let history = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
        if [exclude, ignore].into_iter().any(|f| f != 0 && IsClipboardFormatAvailable(f).is_ok()) {
            return true;
        }
        // "CanIncludeInClipboardHistory" = 0 opts out as well.
        history != 0 && IsClipboardFormatAvailable(history).is_ok() && clipboard_dword(history) == Some(0)
    }
}

unsafe fn clipboard_dword(format: u32) -> Option<u32> {
    unsafe {
        OpenClipboard(None).ok()?;
        let value = (|| {
            let handle = GetClipboardData(format).ok()?;
            let ptr = GlobalLock(HGLOBAL(handle.0)) as *const u32;
            if ptr.is_null() {
                return None;
            }
            let value = *ptr;
            let _ = GlobalUnlock(HGLOBAL(handle.0));
            Some(value)
        })();
        let _ = CloseClipboard();
        value
    }
}

/// Opens a `ms-settings:` page or any URI with its default handler.
pub fn open_uri(uri: &str) -> bool {
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        let result = ShellExecuteW(None, w!("open"), &HSTRING::from(uri), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        result.0 as isize > 32
    }
}

/// `HKCU\...\Run` entry pointing at the current executable.
pub fn autostart_enabled() -> bool {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_QUERY_VALUE, &mut key).is_err() {
            return false;
        }
        let found = RegQueryValueExW(key, RUN_VALUE, None, None, None, None).is_ok();
        let _ = RegCloseKey(key);
        found
    }
}

pub fn set_autostart(enabled: bool) -> windows::core::Result<()> {
    unsafe {
        let mut key = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key).ok()?;
        let result = if enabled {
            let exe = std::env::current_exe().map_err(|_| windows::core::Error::from_thread())?;
            let command = format!("\"{}\"", exe.display());
            let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
            RegSetValueExW(key, RUN_VALUE, None, REG_SZ, Some(bytes)).ok()
        } else {
            RegDeleteValueW(key, RUN_VALUE).ok()
        };
        let _ = RegCloseKey(key);
        result
    }
}
