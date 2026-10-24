//! Camera, microphone and screen capture usage, as recorded by Windows in the
//! CapabilityAccessManager consent store (the same data behind the system
//! privacy icons). An app is using the device while its `LastUsedTimeStop`
//! is 0. The worker sleeps in `RegNotifyChangeKeyValue`, so it only wakes
//! when an app starts or stops using a device.

use tokio::runtime::Handle;
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, WAIT_FAILED};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, KEY_READ, REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME, RegCloseKey,
    RegEnumKeyExW, RegNotifyChangeKeyValue, RegOpenKeyExW, RegQueryValueExW,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForMultipleObjects};
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

use super::{EventBus, ServiceEvent};

const CONSENT_STORE: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrivacyState {
    pub camera: bool,
    pub microphone: bool,
    /// The screen is being recorded or shared.
    pub screen: bool,
}

pub fn spawn(rt: &Handle, bus: EventBus) {
    rt.spawn_blocking(move || {
        if let Err(err) = run(&bus) {
            log::warn!("privacy monitor stopped: {err}");
        }
    });
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn open(path: &str) -> Option<Key> {
    let mut key = HKEY::default();
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(path), None, KEY_READ | KEY_NOTIFY, &mut key) };
    (status == ERROR_SUCCESS).then_some(Key(key))
}

fn run(bus: &EventBus) -> Result<(), String> {
    let camera = open(&format!(r"{CONSENT_STORE}\webcam")).ok_or("webcam consent store missing")?;
    let microphone = open(&format!(r"{CONSENT_STORE}\microphone")).ok_or("microphone consent store missing")?;
    // Windows.Graphics.Capture sessions (screen recording / sharing); the
    // store only exists once some app has captured the screen.
    let screen: Vec<Key> = ["graphicsCaptureProgrammatic", "graphicsCaptureWithoutBorder"]
        .iter()
        .filter_map(|name| open(&format!(r"{CONSENT_STORE}\{name}")))
        .collect();
    let keys: Vec<&Key> = [&camera, &microphone].into_iter().chain(screen.iter()).collect();
    let mut events = Vec::with_capacity(keys.len());
    for _ in &keys {
        events.push(unsafe { CreateEventW(None, false, false, PCWSTR::null()) }.map_err(|e| e.to_string())?);
    }
    let mut last: Option<PrivacyState> = None;
    let result = loop {
        // Re-arm before reading so no change between read and wait is lost.
        for (key, event) in keys.iter().zip(&events) {
            let filter = REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_CHANGE_NAME;
            let status = unsafe { RegNotifyChangeKeyValue(key.0, true, filter, Some(*event), true) };
            if status != ERROR_SUCCESS {
                break;
            }
        }
        let state = PrivacyState {
            camera: in_use(camera.0),
            microphone: in_use(microphone.0),
            screen: screen.iter().any(|key| in_use(key.0)),
        };
        if last != Some(state) {
            last = Some(state);
            bus.send(ServiceEvent::Privacy(state));
        }
        if unsafe { WaitForMultipleObjects(&events, false, INFINITE) } == WAIT_FAILED {
            break Err("wait failed".to_string());
        }
    };
    for event in events {
        unsafe {
            let _ = CloseHandle(event);
        }
    }
    result
}
/// True if any app under `key` (packaged apps directly, desktop apps under
/// `NonPackaged`) is currently using the device.
fn in_use(key: HKEY) -> bool {
    for name in subkeys(key) {
        let mut child = HKEY::default();
        if unsafe { RegOpenKeyExW(key, &HSTRING::from(name.as_str()), None, KEY_READ, &mut child) } != ERROR_SUCCESS {
            continue;
        }
        let child = Key(child);
        let active = if name.eq_ignore_ascii_case("NonPackaged") { in_use(child.0) } else { app_active(child.0) };
        if active {
            return true;
        }
    }
    false
}

fn app_active(key: HKEY) -> bool {
    let read = |name: PCWSTR| -> Option<u64> {
        let mut value = 0u64;
        let mut size = size_of::<u64>() as u32;
        let status =
            unsafe { RegQueryValueExW(key, name, None, None, Some(&mut value as *mut u64 as *mut u8), Some(&mut size)) };
        (status == ERROR_SUCCESS).then_some(value)
    };
    matches!((read(w!("LastUsedTimeStart")), read(w!("LastUsedTimeStop"))), (Some(start), Some(0)) if start != 0)
}

fn subkeys(key: HKEY) -> Vec<String> {
    let mut names = Vec::new();
    let mut buffer = [0u16; 512];
    for index in 0.. {
        let mut len = buffer.len() as u32;
        let status =
            unsafe { RegEnumKeyExW(key, index, Some(PWSTR(buffer.as_mut_ptr())), &mut len, None, None, None, None) };
        if status != ERROR_SUCCESS {
            break;
        }
        names.push(String::from_utf16_lossy(&buffer[..len as usize]));
    }
    names
}
