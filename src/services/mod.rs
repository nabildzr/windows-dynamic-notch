//! Background services.
//!
//! I/O, WinRT calls and network requests run on the tokio runtime and
//! report back through an mpsc channel. Sending also wakes the winit event
//! loop (once per batch) through its proxy, so the UI thread can sleep in
//! `ControlFlow::Wait` without polling.

pub mod audio;
pub mod bluetooth;
pub mod controls;
pub mod keys;
pub mod lyrics;
pub mod media;
pub mod notifications;
pub mod power;
pub mod privacy;
pub mod shelf;
pub mod sound;
pub mod storage;
pub mod system;
pub mod weather;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::runtime::Handle;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use winit::event_loop::EventLoopProxy;

use crate::app::UserEvent;

/// Data pushed from services to the UI thread.
#[derive(Debug)]
pub enum ServiceEvent {
    Loaded(Box<storage::LoadedDocuments>),
    Power(power::PowerStatus),
    Media(Option<media::MediaSnapshot>),
    Artwork(Option<Arc<media::Artwork>>),
    Lyrics { key: String, result: lyrics::LyricsResult },
    Bluetooth(bluetooth::BluetoothEvent),
    ShelfFile(shelf::ShelfFileInfo),
    /// `initial` values describe the state at startup (no indicator).
    Volume { state: audio::VolumeState, initial: bool },
    Brightness { level: u8, initial: bool },
    LockKeys { state: keys::LockKeys, initial: bool },
    Privacy(privacy::PrivacyState),
    System(Box<system::SystemStats>),
    /// Free and total bytes of C: (periodic check for the disk warning).
    Disk { free: u64, total: u64 },
    Weather(weather::WeatherEvent),
    Controls(controls::ControlsState),
    Notification(notifications::Notification),
    NotificationsUnavailable(String),
}

#[derive(Clone)]
pub struct EventBus {
    tx: UnboundedSender<ServiceEvent>,
    proxy: EventLoopProxy<UserEvent>,
    wake_pending: Arc<AtomicBool>,
}

impl EventBus {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> (Self, Inbox) {
        let (tx, rx) = mpsc::unbounded_channel();
        let wake_pending = Arc::new(AtomicBool::new(false));
        (Self { tx, proxy, wake_pending: wake_pending.clone() }, Inbox { rx, wake_pending })
    }

    pub fn send(&self, event: ServiceEvent) {
        if self.tx.send(event).is_ok() && !self.wake_pending.swap(true, Ordering::AcqRel) {
            let _ = self.proxy.send_event(UserEvent::Services);
        }
    }
}

/// UI-side end of the service channel.
pub struct Inbox {
    rx: UnboundedReceiver<ServiceEvent>,
    wake_pending: Arc<AtomicBool>,
}

impl Inbox {
    pub fn drain(&mut self) -> Vec<ServiceEvent> {
        self.wake_pending.store(false, Ordering::Release);
        let mut events = Vec::new();
        while let Ok(event) = self.rx.try_recv() {
            events.push(event);
        }
        events
    }
}

/// Command endpoints of the running services.
pub struct Services {
    pub storage: storage::StorageHandle,
    pub media: media::MediaHandle,
    pub lyrics: lyrics::LyricsHandle,
    pub shelf: shelf::ShelfHandle,
    pub system: system::SystemHandle,
    pub audio: audio::AudioHandle,
    pub weather: weather::WeatherHandle,
    pub controls: controls::ControlsHandle,
    pub notifications: notifications::NotificationsHandle,
}

pub fn start(rt: &Handle, bus: EventBus) -> Services {
    let storage = storage::spawn(rt, bus.clone());
    power::spawn(rt, bus.clone());
    bluetooth::spawn(rt, bus.clone());
    let audio = audio::spawn(rt, bus.clone());
    let weather = weather::spawn(rt, bus.clone());
    let controls = controls::spawn(rt, bus.clone());
    let notifications = notifications::spawn(rt, bus.clone());
    keys::spawn(rt, bus.clone());
    privacy::spawn(rt, bus.clone());
    let media = media::spawn(rt, bus.clone());
    let lyrics = lyrics::spawn(rt, bus.clone());
    let system = system::spawn(rt, bus.clone());
    let shelf = shelf::spawn(rt, bus);
    Services { storage, media, lyrics, shelf, system, audio, weather, controls, notifications }
}
