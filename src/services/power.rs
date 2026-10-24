//! Battery and charger monitoring.
//!
//! `PowerSettingRegisterNotification` (callback mode, no window needed)
//! signals AC/DC and battery-percentage changes; the tokio task then reads
//! `GetSystemPowerStatus` and publishes a [`PowerStatus`]. A slow periodic
//! refresh covers drivers that do not raise notifications.

use std::ffi::c_void;
use std::time::Duration;

use tokio::runtime::Handle;
use tokio::sync::mpsc::{self, UnboundedSender};
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
use windows::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, GetSystemPowerStatus, HPOWERNOTIFY, POWERBROADCAST_SETTING,
    PowerSettingRegisterNotification,
    PowerSettingUnregisterNotification, SYSTEM_POWER_STATUS,
};
use windows::Win32::UI::WindowsAndMessaging::DEVICE_NOTIFY_CALLBACK;
use windows::core::GUID;

use super::{EventBus, ServiceEvent};

const GUID_ACDC_POWER_SOURCE: GUID = GUID::from_u128(0x5d3e9a59_e9d5_4b00_a6bd_ff34ff516548);
const GUID_BATTERY_PERCENTAGE_REMAINING: GUID = GUID::from_u128(0xa7ad8041_b45a_4cae_87a3_eecbb468a9e1);
const GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS: GUID = GUID::from_u128(0x8ffee2c6_2d01_46be_adb9_398addc5b4ff);
const REFRESH: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerStatus {
    /// Remaining charge, `None` when unknown.
    pub percent: Option<u8>,
    /// Battery is being charged.
    pub charging: bool,
    /// Connected to AC power.
    pub ac_online: bool,
    /// The system has a battery at all (desktops do not).
    pub has_battery: bool,
}

impl Default for PowerStatus {
    fn default() -> Self {
        Self { percent: None, charging: false, ac_online: true, has_battery: false }
    }
}

impl PowerStatus {
    /// Fraction for the battery ring; desktops show a full ring.
    pub fn level(&self) -> f64 {
        match (self.has_battery, self.percent) {
            (true, Some(p)) => p as f64 / 100.0,
            _ => 1.0,
        }
    }
}

pub fn query() -> PowerStatus {
    let mut status = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut status) }.is_err() {
        return PowerStatus::default();
    }
    let has_battery = status.BatteryFlag != 128 && status.BatteryFlag != 255;
    PowerStatus {
        percent: (status.BatteryLifePercent <= 100).then_some(status.BatteryLifePercent),
        charging: status.BatteryFlag & 8 != 0,
        ac_online: status.ACLineStatus == 1,
        has_battery,
    }
}

/// What a power-setting notification reported.
enum Signal {
    /// AC line or battery level changed; re-read the power status.
    Power,
    /// Built-in display brightness in percent.
    Brightness(u8),
}

/// Keeps the power-setting registrations alive; unregisters on drop.
struct Registration {
    handles: Vec<HPOWERNOTIFY>,
    _params: Box<DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS>,
    _sender: Box<UnboundedSender<Signal>>,
}

// SAFETY: the raw pointers only reference the boxed fields owned by the
// registration itself; the notification handles may be released from any
// thread.
unsafe impl Send for Registration {}

impl Drop for Registration {
    fn drop(&mut self) {
        for handle in self.handles.drain(..) {
            unsafe {
                let _ = PowerSettingUnregisterNotification(handle);
            }
        }
    }
}

unsafe extern "system" fn power_callback(context: *const c_void, _kind: u32, setting: *const c_void) -> u32 {
    if context.is_null() {
        return ERROR_SUCCESS.0;
    }
    let sender = unsafe { &*(context as *const UnboundedSender<Signal>) };
    let setting = setting as *const POWERBROADCAST_SETTING;
    let brightness = unsafe { setting.as_ref() }
        .filter(|s| s.PowerSetting == GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS && s.DataLength >= 4)
        .map(|s| unsafe { std::ptr::read_unaligned(s.Data.as_ptr() as *const u32) });
    let _ = sender.send(match brightness {
        Some(level) => Signal::Brightness(level.min(100) as u8),
        None => Signal::Power,
    });
    ERROR_SUCCESS.0
}

fn register(sender: UnboundedSender<Signal>) -> Registration {
    let sender = Box::new(sender);
    let params = Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
        Callback: Some(power_callback),
        Context: &*sender as *const UnboundedSender<Signal> as *mut c_void,
    });
    let mut handles = Vec::new();
    for guid in [GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING, GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS] {
        let mut handle: *mut c_void = std::ptr::null_mut();
        let result = unsafe {
            PowerSettingRegisterNotification(
                &guid,
                DEVICE_NOTIFY_CALLBACK,
                HANDLE(&*params as *const DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS as *mut c_void),
                &mut handle,
            )
        };
        if result == ERROR_SUCCESS {
            handles.push(HPOWERNOTIFY(handle as _));
        } else {
            log::warn!("PowerSettingRegisterNotification({guid:?}) failed: {result:?}");
        }
    }
    Registration { handles, _params: params, _sender: sender }
}

pub fn spawn(rt: &Handle, bus: EventBus) {
    rt.spawn(async move {
        let (tx, mut rx) = mpsc::unbounded_channel::<Signal>();
        let _registration = register(tx);
        let mut refresh = tokio::time::interval(REFRESH);
        let mut last: Option<PowerStatus> = None;
        // The first brightness notification only reports the current value.
        let mut brightness_known = false;
        loop {
            tokio::select! {
                _ = refresh.tick() => {}
                signal = rx.recv() => {
                    match signal {
                        None => break,
                        Some(Signal::Brightness(level)) => {
                            bus.send(ServiceEvent::Brightness { level, initial: !brightness_known });
                            brightness_known = true;
                            continue;
                        }
                        Some(Signal::Power) => {
                            // Coalesce bursts (AC + percentage often fire together).
                            tokio::time::sleep(Duration::from_millis(150)).await;
                            while let Ok(queued) = rx.try_recv() {
                                if let Signal::Brightness(level) = queued {
                                    bus.send(ServiceEvent::Brightness { level, initial: !brightness_known });
                                    brightness_known = true;
                                }
                            }
                        }
                    }
                }
            }
            let status = query();
            if last != Some(status) {
                last = Some(status);
                bus.send(ServiceEvent::Power(status));
            }
        }
    });
}
