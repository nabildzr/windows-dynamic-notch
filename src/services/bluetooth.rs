//! Bluetooth audio monitor: watches classic Bluetooth devices that connect
//! or disconnect, keeps the audio/video class ones (headphones, earbuds,
//! speakers) and reads their battery level from the PnP property Windows
//! exposes for hands-free/AVRCP battery reporting.

use std::collections::HashMap;
use std::time::Duration;

use tokio::runtime::Handle;
use tokio::sync::mpsc::{self, UnboundedSender};
use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothMajorClass};
use windows::Devices::Enumeration::{
    DeviceInformation, DeviceInformationKind, DeviceInformationUpdate, DeviceWatcher,
};
use windows::Foundation::{IPropertyValue, TypedEventHandler};
use windows::core::{GUID, HSTRING, IInspectable, Interface};
use windows_collections::IIterable;

use super::{EventBus, ServiceEvent};

const CONTAINER_ID: &str = "System.Devices.Aep.ContainerId";
/// DEVPKEY_Bluetooth_BatteryLevel (hands-free / AVRCP battery reporting).
const BATTERY_LEVEL: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub battery: Option<u8>,
}

#[derive(Clone, Debug)]
pub enum BluetoothEvent {
    /// A device connected while the app runs (shows the popup).
    Connected(AudioDevice),
    /// Present at startup or refreshed information (no popup).
    Updated(AudioDevice),
    Disconnected(String),
}

#[derive(Debug)]
enum Signal {
    Added(String),
    Removed(String),
    EnumerationCompleted,
    Refresh(String),
}

pub fn spawn(rt: &Handle, bus: EventBus) {
    rt.spawn(async move {
        if let Err(err) = run(bus).await {
            log::warn!("bluetooth monitor stopped: {err}");
        }
    });
}

async fn run(bus: EventBus) -> windows::core::Result<()> {
    let selector = BluetoothDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected)?;
    let watcher = DeviceInformation::CreateWatcherAqsFilter(&selector)?;
    let (tx, mut signals) = mpsc::unbounded_channel::<Signal>();

    let added = tx.clone();
    watcher.Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(move |_, info| {
        if let Some(info) = info.as_ref() {
            let _ = added.send(Signal::Added(info.Id()?.to_string()));
        }
        Ok(())
    }))?;
    let removed = tx.clone();
    watcher.Removed(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, update| {
        if let Some(update) = update.as_ref() {
            let _ = removed.send(Signal::Removed(update.Id()?.to_string()));
        }
        Ok(())
    }))?;
    let completed = tx.clone();
    watcher.EnumerationCompleted(&TypedEventHandler::<DeviceWatcher, IInspectable>::new(move |_, _| {
        let _ = completed.send(Signal::EnumerationCompleted);
        Ok(())
    }))?;
    watcher.Start()?;

    let mut enumerated = false;
    let mut devices: HashMap<String, AudioDevice> = HashMap::new();
    while let Some(signal) = signals.recv().await {
        match signal {
            Signal::EnumerationCompleted => enumerated = true,
            Signal::Added(id) => {
                let Some(device) = audio_device(&id).await else { continue };
                log::info!("bluetooth audio device {:?} (battery {:?})", device.name, device.battery);
                devices.insert(id.clone(), device.clone());
                if enumerated {
                    bus.send(ServiceEvent::Bluetooth(BluetoothEvent::Connected(device)));
                    // The battery level arrives a few seconds after the
                    // hands-free profile connects.
                    schedule_refresh(&tx, &id, Duration::from_secs(3));
                    schedule_refresh(&tx, &id, Duration::from_secs(9));
                } else {
                    bus.send(ServiceEvent::Bluetooth(BluetoothEvent::Updated(device)));
                }
            }
            Signal::Refresh(id) => {
                let Some(known) = devices.get_mut(&id) else { continue };
                let battery = battery_level(&id).await;
                if battery.is_some() && battery != known.battery {
                    known.battery = battery;
                    bus.send(ServiceEvent::Bluetooth(BluetoothEvent::Updated(known.clone())));
                }
            }
            Signal::Removed(id) => {
                if devices.remove(&id).is_some() {
                    bus.send(ServiceEvent::Bluetooth(BluetoothEvent::Disconnected(id)));
                }
            }
        }
    }
    let _ = watcher.Stop();
    Ok(())
}

fn schedule_refresh(tx: &UnboundedSender<Signal>, id: &str, delay: Duration) {
    let tx = tx.clone();
    let id = id.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        let _ = tx.send(Signal::Refresh(id));
    });
}

async fn audio_device(id: &str) -> Option<AudioDevice> {
    let device = BluetoothDevice::FromIdAsync(&HSTRING::from(id)).ok()?.await.ok()?;
    let class = device.ClassOfDevice().ok()?;
    if class.MajorClass().ok()? != BluetoothMajorClass::AudioVideo {
        return None;
    }
    let name = device.Name().map(|n| n.to_string()).unwrap_or_default();
    Some(AudioDevice {
        id: id.to_string(),
        name: if name.trim().is_empty() { "Bluetooth audio".to_string() } else { name },
        battery: battery_level(id).await,
    })
}

fn property_names(names: &[&str]) -> IIterable<HSTRING> {
    names.iter().map(|name| HSTRING::from(*name)).collect::<Vec<_>>().into()
}

/// Battery percentage reported by any PnP node of the device's container.
async fn battery_level(id: &str) -> Option<u8> {
    // The stock `IIterable` is not `Send`; it must be dropped before awaiting.
    let lookup = {
        let names = property_names(&[CONTAINER_ID]);
        DeviceInformation::CreateFromIdAsyncAdditionalProperties(&HSTRING::from(id), &names).ok()?
    };
    let info = lookup.await.ok()?;
    let container = {
        let value = info.Properties().ok()?.Lookup(&HSTRING::from(CONTAINER_ID)).ok()?;
        let guid: GUID = value.cast::<IPropertyValue>().ok()?.GetGuid().ok()?;
        guid
    };
    let filter = HSTRING::from(format!("System.Devices.ContainerId:=\"{{{container:?}}}\""));
    let search = {
        let names = property_names(&[BATTERY_LEVEL]);
        DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&filter, &names, DeviceInformationKind::Device)
            .ok()?
    };
    let nodes = search.await.ok()?;
    let key = HSTRING::from(BATTERY_LEVEL);
    for node in nodes {
        let Ok(properties) = node.Properties() else { continue };
        let Ok(value) = properties.Lookup(&key) else { continue };
        if let Ok(level) = value.cast::<IPropertyValue>().and_then(|v| v.GetUInt8())
            && level <= 100
        {
            return Some(level);
        }
    }
    None
}
