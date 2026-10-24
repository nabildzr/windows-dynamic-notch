//! Toast notifications from other apps (`UserNotificationListener`), shown
//! as a short popup in the notch.
//!
//! Windows asks the user once for "notification access". The listener's
//! change event needs a packaged app, so the list is read every two
//! seconds instead, and only while the feature is enabled. Notifications
//! that already existed at startup are not shown.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::runtime::Handle;
use vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use windows::Foundation::Size;
use windows::Storage::Streams::DataReader;
use windows::UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::Interface;
use windows_future::IAsyncOperation;

use super::{EventBus, ServiceEvent};

const POLL: Duration = Duration::from_secs(2);
const ICON: u32 = 64;

#[derive(Clone)]
pub struct Notification {
    pub app: String,
    pub title: String,
    pub body: String,
    pub icon: Option<Arc<ImageData>>,
}

impl std::fmt::Debug for Notification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notification").field("app", &self.app).finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct NotificationsHandle(Arc<AtomicBool>);

impl NotificationsHandle {
    pub fn set_enabled(&self, enabled: bool) {
        self.0.store(enabled, Ordering::Release);
    }
}

pub fn spawn(rt: &Handle, bus: EventBus) -> NotificationsHandle {
    let enabled = Arc::new(AtomicBool::new(false));
    let flag = enabled.clone();
    rt.spawn_blocking(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        // Wait for the settings before asking for access.
        while !flag.load(Ordering::Acquire) {
            std::thread::sleep(POLL);
        }
        let listener = match open_listener() {
            Ok(listener) => listener,
            Err(err) => {
                log::warn!("notification access unavailable: {err}");
                bus.send(ServiceEvent::NotificationsUnavailable(err));
                return;
            }
        };
        log::info!("notification access granted");
        let mut seen: Option<HashSet<u32>> = None;
        loop {
            std::thread::sleep(POLL);
            if !flag.load(Ordering::Acquire) {
                continue;
            }
            let Ok(list) = listener.GetNotificationsAsync(NotificationKinds::Toast).and_then(|op| op.join()) else {
                continue;
            };
            let current: Vec<UserNotification> = list.into_iter().collect();
            let ids: HashSet<u32> = current.iter().filter_map(|n| n.Id().ok()).collect();
            if let Some(known) = &seen {
                for notification in &current {
                    let Ok(id) = notification.Id() else { continue };
                    if !known.contains(&id)
                        && let Some(parsed) = parse(notification)
                    {
                        log::debug!("notification from {}", parsed.app);
                        bus.send(ServiceEvent::Notification(parsed));
                    }
                }
            }
            seen = Some(ids);
        }
    });
    NotificationsHandle(enabled)
}

fn open_listener() -> Result<UserNotificationListener, String> {
    let listener = UserNotificationListener::Current().map_err(|e| e.to_string())?;
    let status = listener.RequestAccessAsync().and_then(|op| op.join()).map_err(|e| e.to_string())?;
    if status == UserNotificationListenerAccessStatus::Allowed {
        Ok(listener)
    } else {
        Err(format!("access {status:?} (Settings > Privacy > Notifications)"))
    }
}

fn parse(notification: &UserNotification) -> Option<Notification> {
    let info = notification.AppInfo().ok()?.DisplayInfo().ok()?;
    let app = info.DisplayName().ok()?.to_string();
    let binding = notification
        .Notification()
        .ok()?
        .Visual()
        .ok()?
        .GetBinding(&KnownNotificationBindings::ToastGeneric().ok()?)
        .ok()?;
    let texts: Vec<String> =
        binding.GetTextElements().ok()?.into_iter().filter_map(|t| t.Text().ok()).map(|t| t.to_string()).collect();
    let title = texts.first().cloned().unwrap_or_else(|| app.clone());
    let body = texts.iter().skip(1).map(|t| t.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" \u{00B7} ");
    let icon = info
        .GetLogo(Size { Width: ICON as f32, Height: ICON as f32 })
        .ok()
        .and_then(|logo| read_icon(&logo))
        .map(Arc::new);
    Some(Notification { app, title, body, icon })
}

fn read_icon(reference: &windows::Storage::Streams::RandomAccessStreamReference) -> Option<ImageData> {
    let stream = reference.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > 4 << 20 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    let load: IAsyncOperation<u32> = reader.LoadAsync(size as u32).ok()?.cast().ok()?;
    let loaded = load.join().ok()?;
    let mut bytes = vec![0u8; loaded as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    let image = image::load_from_memory(&bytes).ok()?.resize(ICON, ICON, image::imageops::FilterType::Triangle).to_rgba8();
    let (width, height) = image.dimensions();
    Some(ImageData {
        data: Blob::from(image.into_raw()),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    })
}
