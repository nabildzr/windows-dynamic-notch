//! Quick controls: Wi-Fi and Bluetooth radios (WinRT `Windows.Devices.Radios`)
//! and the Windows power mode (power plan overlay: best power efficiency,
//! balanced, best performance).
//!
//! Radios report their own state changes; the power mode is re-read when
//! the widget opens or after it was changed here.

use std::sync::mpsc::{self, Sender};

use tokio::runtime::Handle;
use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};
use windows::Foundation::TypedEventHandler;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{GUID, s, w};

use super::{EventBus, ServiceEvent};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PowerMode {
    Efficiency,
    #[default]
    Balanced,
    Performance,
}

impl PowerMode {
    const EFFICIENCY: GUID = GUID::from_u128(0x961cc777_2547_4f9d_8174_7d86181b8a7a);
    const PERFORMANCE: GUID = GUID::from_u128(0xded574b5_45a0_4f42_8737_46345c09c238);

    fn guid(self) -> GUID {
        match self {
            PowerMode::Efficiency => Self::EFFICIENCY,
            PowerMode::Balanced => GUID::zeroed(),
            PowerMode::Performance => Self::PERFORMANCE,
        }
    }

    fn from_guid(guid: GUID) -> Self {
        match guid {
            g if g == Self::EFFICIENCY => PowerMode::Efficiency,
            g if g == Self::PERFORMANCE => PowerMode::Performance,
            _ => PowerMode::Balanced,
        }
    }

    pub fn next(self) -> Self {
        match self {
            PowerMode::Efficiency => PowerMode::Balanced,
            PowerMode::Balanced => PowerMode::Performance,
            PowerMode::Performance => PowerMode::Efficiency,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PowerMode::Efficiency => "Efficiency",
            PowerMode::Balanced => "Balanced",
            PowerMode::Performance => "Performance",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ControlsState {
    /// `None` when the device has no such radio (or access was denied).
    pub wifi: Option<bool>,
    pub bluetooth: Option<bool>,
    pub power: Option<PowerMode>,
}

enum Signal {
    Refresh,
    ToggleWifi,
    ToggleBluetooth,
    SetPower(PowerMode),
}

#[derive(Clone)]
pub struct ControlsHandle(Sender<Signal>);

impl ControlsHandle {
    pub fn refresh(&self) {
        let _ = self.0.send(Signal::Refresh);
    }

    pub fn toggle_wifi(&self) {
        let _ = self.0.send(Signal::ToggleWifi);
    }

    pub fn toggle_bluetooth(&self) {
        let _ = self.0.send(Signal::ToggleBluetooth);
    }

    pub fn set_power(&self, mode: PowerMode) {
        let _ = self.0.send(Signal::SetPower(mode));
    }
}

type GetOverlay = unsafe extern "system" fn(*mut GUID) -> u32;
type SetOverlay = unsafe extern "system" fn(GUID) -> u32;

/// The overlay functions are exported by powrprof.dll (Windows 10 1709+)
/// but missing from older import libraries, so they are resolved at run
/// time.
struct PowerApi {
    get: GetOverlay,
    set: SetOverlay,
}

impl PowerApi {
    fn load() -> Option<Self> {
        unsafe {
            let module = LoadLibraryW(w!("powrprof.dll")).ok()?;
            let get = GetProcAddress(module, s!("PowerGetEffectiveOverlayScheme"))?;
            let set = GetProcAddress(module, s!("PowerSetActiveOverlayScheme"))?;
            type Raw = unsafe extern "system" fn() -> isize;
            Some(Self {
                get: std::mem::transmute::<Raw, GetOverlay>(get),
                set: std::mem::transmute::<Raw, SetOverlay>(set),
            })
        }
    }

    fn current(&self) -> Option<PowerMode> {
        let mut guid = GUID::zeroed();
        (unsafe { (self.get)(&mut guid) } == 0).then(|| PowerMode::from_guid(guid))
    }

    fn apply(&self, mode: PowerMode) -> bool {
        unsafe { (self.set)(mode.guid()) == 0 }
    }
}

pub fn spawn(rt: &Handle, bus: EventBus) -> ControlsHandle {
    let (tx, rx) = mpsc::channel();
    let handle = ControlsHandle(tx.clone());
    rt.spawn_blocking(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let power = PowerApi::load();
        let radios = load_radios(&tx);
        let read = |radios: &[Radio]| {
            let state_of = |kind: RadioKind| {
                radios
                    .iter()
                    .find(|r| r.Kind().is_ok_and(|k| k == kind))
                    .and_then(|r| r.State().ok())
                    .map(|s| s == RadioState::On)
            };
            ControlsState {
                wifi: state_of(RadioKind::WiFi),
                bluetooth: state_of(RadioKind::Bluetooth),
                power: power.as_ref().and_then(PowerApi::current),
            }
        };
        let mut last = read(&radios);
        bus.send(ServiceEvent::Controls(last));
        while let Ok(signal) = rx.recv() {
            match signal {
                Signal::Refresh => {}
                Signal::ToggleWifi | Signal::ToggleBluetooth => {
                    let kind = if matches!(signal, Signal::ToggleWifi) { RadioKind::WiFi } else { RadioKind::Bluetooth };
                    if let Some(radio) = radios.iter().find(|r| r.Kind().is_ok_and(|k| k == kind)) {
                        let on = radio.State().is_ok_and(|s| s == RadioState::On);
                        let target = if on { RadioState::Off } else { RadioState::On };
                        match radio.SetStateAsync(target).and_then(|op| op.join()) {
                            Ok(RadioAccessStatus::Allowed) => {}
                            Ok(status) => log::warn!("radio change refused: {status:?}"),
                            Err(err) => log::warn!("radio change failed: {err}"),
                        }
                    }
                }
                Signal::SetPower(mode) => {
                    if let Some(api) = &power
                        && !api.apply(mode)
                    {
                        log::warn!("cannot set power mode {mode:?}");
                    }
                }
            }
            let state = read(&radios);
            if state != last {
                last = state;
                bus.send(ServiceEvent::Controls(state));
            }
        }
    });
    handle
}

fn load_radios(tx: &Sender<Signal>) -> Vec<Radio> {
    let result = (|| -> windows::core::Result<Vec<Radio>> {
        let access = Radio::RequestAccessAsync()?.join()?;
        if access != RadioAccessStatus::Allowed {
            log::warn!("radio access: {access:?}");
        }
        let radios: Vec<Radio> = Radio::GetRadiosAsync()?.join()?.into_iter().collect();
        for radio in &radios {
            let tx = tx.clone();
            radio.StateChanged(&TypedEventHandler::new(move |_, _| {
                let _ = tx.send(Signal::Refresh);
                Ok(())
            }))?;
        }
        Ok(radios)
    })();
    result.unwrap_or_else(|err| {
        log::warn!("radios unavailable: {err}");
        Vec::new()
    })
}
