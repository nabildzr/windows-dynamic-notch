//! System tray icon and the small input dialog used by menu settings.

use std::cell::RefCell;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_WINDOW, CreateFontIndirectW, DeleteObject, GetMonitorInfoW, HBRUSH, HFONT, HGDIOBJ, MONITOR_DEFAULTTOPRIMARY,
    MONITORINFO, MonitorFromPoint,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::EM_SETSEL;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::Shell::{NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, BS_DEFPUSHBUTTON, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, ES_AUTOHSCROLL, ES_CENTER, ES_NUMBER, GetMessageW, GetSystemMetrics, GetWindowTextW, HICON, HMENU,
    IDI_APPLICATION, IMAGE_ICON, IsDialogMessageW, LR_DEFAULTCOLOR, LoadIconW, LoadImageW, MSG,
    NONCLIENTMETRICSW, PostQuitMessage, RegisterClassW, SM_CXSMICON, SPI_GETNONCLIENTMETRICS,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageW, SetForegroundWindow, SystemParametersInfoW, TranslateMessage,
    WM_CLOSE, WM_COMMAND, WM_SETFONT, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_EX_DLGMODALFRAME, WS_EX_TOPMOST,
    WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::{HSTRING, PCWSTR, w};

use super::win32::WM_TRAY;

/// Notification-area icon; right or left click opens the notch menu.
pub struct TrayIcon {
    hwnd: HWND,
    icon: HICON,
    added: bool,
}

impl TrayIcon {
    pub fn new(hwnd: HWND) -> Self {
        let mut tray = Self { hwnd, icon: load_small_icon(), added: false };
        tray.add();
        tray
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: self.icon,
            ..Default::default()
        };
        for (slot, unit) in data.szTip.iter_mut().zip("Windows Dynamic Notch".encode_utf16()) {
            *slot = unit;
        }
        data
    }

    /// (Re-)adds the icon, e.g. after Explorer restarted.
    pub fn add(&mut self) {
        let data = self.data();
        self.added = unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool();
        if !self.added {
            log::warn!("cannot add the tray icon");
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        if self.added {
            let data = self.data();
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            }
        }
    }
}

fn load_small_icon() -> HICON {
    unsafe {
        let size = GetSystemMetrics(SM_CXSMICON);
        let module = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0));
        // Resource 1 is the application icon embedded by build.rs.
        // MAKEINTRESOURCE(1)
        let id = PCWSTR(std::ptr::without_provenance(1));
        match LoadImageW(module, id, IMAGE_ICON, size, size, LR_DEFAULTCOLOR) {
            Ok(handle) => HICON(handle.0),
            Err(_) => LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
        }
    }
}

const ID_OK: usize = 1;
const ID_CANCEL: usize = 2;

/// What the dialog accepts.
#[derive(Clone, Copy, Default)]
enum Accept {
    /// A whole number in the range.
    Range(u32, u32),
    /// Any non-empty text.
    #[default]
    Text,
}

#[derive(Default)]
struct Prompt {
    edit: HWND,
    accept: Accept,
    result: Option<String>,
    done: bool,
}

thread_local! {
    static PROMPT: RefCell<Prompt> = RefCell::new(Prompt::default());
}

fn read_value(edit: HWND, accept: Accept) -> Option<String> {
    let mut buffer = [0u16; 128];
    let len = unsafe { GetWindowTextW(edit, &mut buffer) } as usize;
    let text = String::from_utf16_lossy(&buffer[..len]).trim().to_string();
    match accept {
        Accept::Range(min, max) => text.parse::<u32>().ok().filter(|v| (min..=max).contains(v)).map(|v| v.to_string()),
        Accept::Text => (!text.is_empty()).then_some(text),
    }
}

unsafe extern "system" fn prompt_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = wparam.0 & 0xFFFF;
            if id == ID_OK {
                let accepted = PROMPT.with(|p| {
                    let mut p = p.borrow_mut();
                    match read_value(p.edit, p.accept) {
                        Some(value) => {
                            p.result = Some(value);
                            p.done = true;
                            true
                        }
                        None => false,
                    }
                });
                if accepted {
                    unsafe {
                        let _ = DestroyWindow(hwnd);
                    }
                } else {
                    unsafe {
                        let edit = PROMPT.with(|p| p.borrow().edit);
                        SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                        let _ = SetFocus(Some(edit));
                    }
                }
                return LRESULT(0);
            }
            if id == ID_CANCEL {
                PROMPT.with(|p| p.borrow_mut().done = true);
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return LRESULT(0);
            }
        }
        WM_CLOSE => {
            PROMPT.with(|p| p.borrow_mut().done = true);
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Modal dialog asking for a whole number in `min..=max`. Returns `None`
/// when cancelled.
pub fn prompt_number(title: &str, label: &str, current: u32, min: u32, max: u32, scale: f64) -> Option<u32> {
    prompt(title, label, &current.to_string(), Accept::Range(min, max), scale)?.parse().ok()
}

/// Modal dialog asking for a line of text. Returns `None` when cancelled.
pub fn prompt_text(title: &str, label: &str, current: &str, scale: f64) -> Option<String> {
    prompt(title, label, current, Accept::Text, scale)
}

fn prompt(title: &str, label: &str, current: &str, accept: Accept, scale: f64) -> Option<String> {
    unsafe {
        let module = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0))?;
        let class = w!("WindowsDynamicNotch.Prompt");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(prompt_proc),
            hInstance: module,
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut _),
            lpszClassName: class,
            ..Default::default()
        };
        // Fails harmlessly when the class already exists.
        RegisterClassW(&wc);

        let px = |v: f64| (v * scale).round() as i32;
        let style = WS_POPUP | WS_CAPTION | WS_SYSMENU;
        let ex_style = WS_EX_DLGMODALFRAME | WS_EX_TOPMOST;
        let mut frame = RECT { left: 0, top: 0, right: px(320.0), bottom: px(132.0) };
        let _ = AdjustWindowRectEx(&mut frame, style, false, ex_style);
        let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
        let monitor = MonitorFromPoint(Default::default(), MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let work = info.rcWork;
        let x = work.left + (work.right - work.left - width) / 2;
        let y = work.top + (work.bottom - work.top - height) / 2;
        let dialog = CreateWindowExW(
            ex_style,
            class,
            &HSTRING::from(title),
            style,
            x,
            y,
            width,
            height,
            None,
            None,
            Some(module),
            None,
        )
        .ok()?;

        let font = message_font(scale);
        let child = |class: PCWSTR, text: &str, extra: u32, rect: (f64, f64, f64, f64), id: usize| -> HWND {
            let hwnd = CreateWindowExW(
                Default::default(),
                class,
                &HSTRING::from(text),
                WS_CHILD | WS_VISIBLE | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(extra),
                px(rect.0),
                px(rect.1),
                px(rect.2),
                px(rect.3),
                Some(dialog),
                Some(HMENU(id as *mut _)),
                Some(module),
                None,
            )
            .unwrap_or_default();
            SendMessageW(hwnd, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
            hwnd
        };
        child(w!("STATIC"), label, 0, (16.0, 14.0, 288.0, 36.0), 0);
        let numeric = matches!(accept, Accept::Range(..));
        let (edit_style, edit_width) = if numeric {
            (WS_BORDER.0 | WS_TABSTOP.0 | ES_NUMBER as u32 | ES_CENTER as u32, 80.0)
        } else {
            (WS_BORDER.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32, 288.0)
        };
        let edit = child(w!("EDIT"), current, edit_style, (16.0, 54.0, edit_width, 26.0), 100);
        if numeric {
            child(w!("STATIC"), "%", 0, (102.0, 58.0, 20.0, 22.0), 0);
        }
        child(w!("BUTTON"), "OK", WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32, (132.0, 92.0, 82.0, 28.0), ID_OK);
        child(w!("BUTTON"), "Cancel", WS_TABSTOP.0, (222.0, 92.0, 82.0, 28.0), ID_CANCEL);

        PROMPT.with(|p| *p.borrow_mut() = Prompt { edit, accept, result: None, done: false });
        let _ = SetForegroundWindow(dialog);
        let _ = SetFocus(Some(edit));
        SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));

        let mut msg = MSG::default();
        while !PROMPT.with(|p| p.borrow().done) {
            let status = GetMessageW(&mut msg, None, 0, 0);
            if status.0 <= 0 {
                if status.0 == 0 {
                    // Keep the WM_QUIT for the main loop.
                    PostQuitMessage(msg.wParam.0 as i32);
                }
                break;
            }
            if !IsDialogMessageW(dialog, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let _ = DeleteObject(HGDIOBJ(font.0));
        PROMPT.with(|p| p.borrow_mut().result.take())
    }
}

/// The system dialog font at the given scale.
unsafe fn message_font(scale: f64) -> HFONT {
    let mut metrics = NONCLIENTMETRICSW { cbSize: size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            Some(&mut metrics as *mut NONCLIENTMETRICSW as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let mut font = metrics.lfMessageFont;
        font.lfHeight = -((15.0 * scale).round() as i32);
        CreateFontIndirectW(&font)
    }
}
