//! Volume and mute of the default playback device through Core Audio.
//!
//! `IAudioEndpointVolumeCallback` fires whenever the volume or mute state
//! changes (media keys, the Windows flyout, any app), and
//! `IMMNotificationClient` re-attaches when the default device changes.
//! Event driven: nothing runs while the volume stays put.

use std::sync::mpsc::{self, Sender};

use tokio::runtime::Handle;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, MMDeviceEnumerator, eConsole, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::core::{PCWSTR, implement};

use super::{EventBus, ServiceEvent};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeState {
    /// 0..=1
    pub level: f32,
    pub muted: bool,
}

enum Signal {
    Changed(VolumeState),
    DefaultDeviceChanged,
    SetLevel(f32),
    ToggleMute,
}

/// Changes the volume of the default playback device.
#[derive(Clone)]
pub struct AudioHandle(Sender<Signal>);

impl AudioHandle {
    pub fn set_level(&self, level: f32) {
        let _ = self.0.send(Signal::SetLevel(level.clamp(0.0, 1.0)));
    }

    pub fn toggle_mute(&self) {
        let _ = self.0.send(Signal::ToggleMute);
    }
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCallback {
    tx: Sender<Signal>,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        if let Some(data) = unsafe { data.as_ref() } {
            let state = VolumeState { level: data.fMasterVolume.clamp(0.0, 1.0), muted: data.bMuted.as_bool() };
            let _ = self.tx.send(Signal::Changed(state));
        }
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DeviceWatcher {
    tx: Sender<Signal>,
}

impl IMMNotificationClient_Impl for DeviceWatcher_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> windows::core::Result<()> {
        if flow == eRender && role == eConsole {
            let _ = self.tx.send(Signal::DefaultDeviceChanged);
        }
        Ok(())
    }

    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> windows::core::Result<()> {
        Ok(())
    }
}

pub fn spawn(rt: &Handle, bus: EventBus) -> AudioHandle {
    let (tx, rx) = mpsc::channel();
    let handle = AudioHandle(tx.clone());
    rt.spawn_blocking(move || {
        if let Err(err) = run(&bus, tx, rx) {
            log::warn!("volume monitor stopped: {err}");
        }
    });
    handle
}

fn run(bus: &EventBus, tx: Sender<Signal>, rx: mpsc::Receiver<Signal>) -> windows::core::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let enumerator: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    let watcher: IMMNotificationClient = DeviceWatcher { tx: tx.clone() }.into();
    unsafe { enumerator.RegisterEndpointNotificationCallback(&watcher)? };
    let callback: IAudioEndpointVolumeCallback = VolumeCallback { tx }.into();
    let mut endpoint = attach(&enumerator, &callback, bus);
    while let Ok(signal) = rx.recv() {
        match signal {
            Signal::Changed(state) => bus.send(ServiceEvent::Volume { state, initial: false }),
            Signal::DefaultDeviceChanged => {
                if let Some(old) = endpoint.take() {
                    unsafe {
                        let _ = old.UnregisterControlChangeNotify(&callback);
                    }
                }
                endpoint = attach(&enumerator, &callback, bus);
            }
            Signal::SetLevel(level) => {
                if let Some(volume) = &endpoint {
                    unsafe {
                        let _ = volume.SetMasterVolumeLevelScalar(level, std::ptr::null());
                        if level > 0.0 && volume.GetMute().is_ok_and(|m| m.as_bool()) {
                            let _ = volume.SetMute(false, std::ptr::null());
                        }
                    }
                }
            }
            Signal::ToggleMute => {
                if let Some(volume) = &endpoint {
                    unsafe {
                        let muted = volume.GetMute().is_ok_and(|m| m.as_bool());
                        let _ = volume.SetMute(!muted, std::ptr::null());
                    }
                }
            }
        }
    }
    Ok(())
}

/// Subscribes to the current default playback device and reports its state
/// (without showing an indicator).
fn attach(
    enumerator: &IMMDeviceEnumerator,
    callback: &IAudioEndpointVolumeCallback,
    bus: &EventBus,
) -> Option<IAudioEndpointVolume> {
    unsafe {
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
        volume.RegisterControlChangeNotify(callback).ok()?;
        let state = VolumeState {
            level: volume.GetMasterVolumeLevelScalar().unwrap_or(0.0),
            muted: volume.GetMute().map(|m| m.as_bool()).unwrap_or(false),
        };
        bus.send(ServiceEvent::Volume { state, initial: true });
        Some(volume)
    }
}
