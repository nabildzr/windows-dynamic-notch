//! Media session integration through WinRT
//! `GlobalSystemMediaTransportControlsSessionManager`: reads whatever is
//! playing on the PC (title, artist, album, artwork, timeline, playback
//! state) and sends transport commands back to the source app.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::runtime::Handle;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows::Storage::Streams::DataReader;
use windows::core::Interface;
use windows_future::IAsyncOperation;

use super::{EventBus, ServiceEvent};

/// Edge length of the decoded artwork (2x the 56 px thumbnail).
const ART_SIZE: u32 = 128;
const FILETIME_UNIX_OFFSET: i64 = 116_444_736_000_000_000;
const TICKS_PER_SECOND: i64 = 10_000_000;

#[derive(Clone, Debug)]
pub struct MediaSnapshot {
    pub app_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    /// Position at `sampled_at`.
    pub position: Duration,
    pub duration: Duration,
    pub sampled_at: Instant,
    pub rate: f64,
    pub can_play_pause: bool,
    pub can_next: bool,
    pub can_previous: bool,
    pub can_seek: bool,
}

impl MediaSnapshot {
    /// Identity of the track, used for artwork and lyrics caching.
    pub fn track_key(&self) -> String {
        format!("{}\u{1f}{}\u{1f}{}\u{1f}{}", self.app_id, self.artist, self.title, self.album)
    }

    /// Extrapolated playback position (sessions only report the timeline
    /// occasionally).
    pub fn position_at(&self, now: Instant) -> Duration {
        let mut position = self.position;
        if self.playing {
            position += now.saturating_duration_since(self.sampled_at).mul_f64(self.rate.clamp(0.0, 4.0));
        }
        if self.duration > Duration::ZERO { position.min(self.duration) } else { position }
    }

    /// "Artist - Album" line under the title.
    pub fn subtitle(&self) -> String {
        match (self.artist.is_empty(), self.album.is_empty()) {
            (false, false) => format!("{} - {}", self.artist, self.album),
            (false, true) => self.artist.clone(),
            (true, false) => self.album.clone(),
            (true, true) => app_display_name(&self.app_id),
        }
    }

    fn same_as(&self, other: &MediaSnapshot) -> bool {
        let drift = self.position_at(Instant::now()).abs_diff(other.position_at(Instant::now()));
        self.app_id == other.app_id
            && self.title == other.title
            && self.artist == other.artist
            && self.album == other.album
            && self.playing == other.playing
            && self.duration == other.duration
            && (self.rate - other.rate).abs() < 0.01
            && self.can_play_pause == other.can_play_pause
            && self.can_next == other.can_next
            && self.can_previous == other.can_previous
            && self.can_seek == other.can_seek
            && drift < Duration::from_millis(1500)
    }
}

/// Short app name from an AppUserModelID ("Spotify.exe", "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic").
fn app_display_name(app_id: &str) -> String {
    let base = app_id.rsplit(['!', '\\']).next().unwrap_or(app_id);
    let base = base.trim_end_matches(".exe");
    let base = base.rsplit('.').next().unwrap_or(base);
    if base.is_empty() { "Media".to_string() } else { base.to_string() }
}

/// Decoded, square-cropped album art.
pub struct Artwork {
    pub key: String,
    pub image: ImageData,
}

impl std::fmt::Debug for Artwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Artwork").field("key", &self.key).finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Previous,
    Seek(Duration),
}

#[derive(Clone)]
pub struct MediaHandle(UnboundedSender<MediaCommand>);

impl MediaHandle {
    pub fn send(&self, command: MediaCommand) {
        let _ = self.0.send(command);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Signal {
    Sessions,
    Properties,
    State,
}

pub fn spawn(rt: &Handle, bus: EventBus) -> MediaHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    rt.spawn(async move {
        if let Err(err) = run(bus, rx).await {
            log::error!("media service stopped: {err}");
        }
    });
    MediaHandle(tx)
}

fn subscribe_handler<S, A>(tx: &UnboundedSender<Signal>, signal: Signal) -> TypedEventHandler<S, A>
where
    S: windows::core::RuntimeType + 'static,
    A: windows::core::RuntimeType + 'static,
{
    let tx = tx.clone();
    TypedEventHandler::new(move |_, _| {
        let _ = tx.send(signal);
        Ok(())
    })
}

async fn run(bus: EventBus, mut commands: UnboundedReceiver<MediaCommand>) -> windows::core::Result<()> {
    let manager = SessionManager::RequestAsync()?.await?;
    let (tx, mut signals) = mpsc::unbounded_channel::<Signal>();
    manager.CurrentSessionChanged(&subscribe_handler(&tx, Signal::Sessions))?;
    manager.SessionsChanged(&subscribe_handler(&tx, Signal::Sessions))?;

    let mut tracker = Tracker::default();
    tracker.switch_session(&manager, &tx, &bus).await;
    loop {
        let artwork_retry = tracker.artwork_retry;
        tokio::select! {
            signal = signals.recv() => {
                let Some(first) = signal else { break };
                // Sessions often fire several events at once; coalesce them.
                tokio::time::sleep(Duration::from_millis(40)).await;
                let mut sessions = first == Signal::Sessions;
                let mut properties = first == Signal::Properties;
                while let Ok(next) = signals.try_recv() {
                    sessions |= next == Signal::Sessions;
                    properties |= next == Signal::Properties;
                }
                if sessions {
                    tracker.switch_session(&manager, &tx, &bus).await;
                } else {
                    tracker.publish(&bus, properties).await;
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { break };
                tracker.execute(command).await;
            }
            _ = tokio::time::sleep_until(artwork_retry.unwrap_or_else(tokio::time::Instant::now)), if artwork_retry.is_some() => {
                tracker.artwork_retry = None;
                // Some players update the thumbnail shortly after the title.
                tracker.refresh_artwork(&bus).await;
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct Tracker {
    session: Option<Session>,
    tokens: [i64; 3],
    last: Option<MediaSnapshot>,
    art_key: Option<String>,
    art_hash: Option<u64>,
    artwork_retry: Option<tokio::time::Instant>,
}

impl Tracker {
    async fn switch_session(&mut self, manager: &SessionManager, tx: &UnboundedSender<Signal>, bus: &EventBus) {
        if let Some(old) = self.session.take() {
            let _ = old.RemoveMediaPropertiesChanged(self.tokens[0]);
            let _ = old.RemovePlaybackInfoChanged(self.tokens[1]);
            let _ = old.RemoveTimelinePropertiesChanged(self.tokens[2]);
        }
        match manager.GetCurrentSession() {
            Ok(session) => {
                self.tokens = [
                    session.MediaPropertiesChanged(&subscribe_handler(tx, Signal::Properties)).unwrap_or_default(),
                    session.PlaybackInfoChanged(&subscribe_handler(tx, Signal::State)).unwrap_or_default(),
                    session.TimelinePropertiesChanged(&subscribe_handler(tx, Signal::State)).unwrap_or_default(),
                ];
                self.session = Some(session);
                self.publish(bus, true).await;
            }
            Err(_) => {
                self.last = None;
                self.art_key = None;
                self.art_hash = None;
                bus.send(ServiceEvent::Media(None));
            }
        }
    }

    async fn publish(&mut self, bus: &EventBus, properties_changed: bool) {
        let Some(session) = self.session.clone() else { return };
        match snapshot(&session).await {
            Ok(snapshot) if snapshot.title.is_empty() && !snapshot.playing => {
                if self.last.take().is_some() {
                    bus.send(ServiceEvent::Media(None));
                }
            }
            Ok(snapshot) => {
                if self.last.as_ref().is_none_or(|last| !last.same_as(&snapshot)) {
                    bus.send(ServiceEvent::Media(Some(snapshot.clone())));
                }
                let key = snapshot.track_key();
                let new_track = self.art_key.as_deref() != Some(key.as_str());
                self.last = Some(snapshot);
                if new_track {
                    self.art_key = Some(key);
                    self.art_hash = None;
                    self.refresh_artwork(bus).await;
                    self.artwork_retry = Some(tokio::time::Instant::now() + Duration::from_millis(2000));
                } else if properties_changed {
                    self.refresh_artwork(bus).await;
                }
            }
            Err(err) => log::debug!("media snapshot failed: {err}"),
        }
    }

    async fn refresh_artwork(&mut self, bus: &EventBus) {
        let (Some(session), Some(key)) = (self.session.clone(), self.art_key.clone()) else { return };
        let bytes = read_thumbnail(&session).await;
        let hash = bytes.as_deref().map(fnv1a);
        if hash.is_some() && hash == self.art_hash {
            return;
        }
        self.art_hash = hash;
        let image = match bytes {
            Some(bytes) => tokio::task::spawn_blocking(move || decode_artwork(&bytes)).await.ok().flatten(),
            None => None,
        };
        bus.send(ServiceEvent::Artwork(image.map(|image| Arc::new(Artwork { key, image }))));
    }

    async fn execute(&mut self, command: MediaCommand) {
        let Some(session) = self.session.clone() else { return };
        let result = match command {
            MediaCommand::PlayPause => await_bool(session.TryTogglePlayPauseAsync()).await,
            MediaCommand::Next => await_bool(session.TrySkipNextAsync()).await,
            MediaCommand::Previous => await_bool(session.TrySkipPreviousAsync()).await,
            MediaCommand::Seek(position) => {
                let start = session.GetTimelineProperties().and_then(|t| t.StartTime()).map(|t| t.Duration).unwrap_or(0);
                let ticks = start + (position.as_nanos() / 100) as i64;
                await_bool(session.TryChangePlaybackPositionAsync(ticks)).await
            }
        };
        match result {
            Ok(true) => {}
            Ok(false) => log::debug!("{command:?} rejected by the media session"),
            Err(err) => log::warn!("{command:?} failed: {err}"),
        }
    }
}

async fn await_bool(operation: windows::core::Result<IAsyncOperation<bool>>) -> windows::core::Result<bool> {
    operation?.await
}

async fn snapshot(session: &Session) -> windows::core::Result<MediaSnapshot> {
    let properties = session.TryGetMediaPropertiesAsync()?.await?;
    let info = session.GetPlaybackInfo()?;
    let timeline = session.GetTimelineProperties()?;
    let controls = info.Controls()?;
    let playing = info.PlaybackStatus()? == PlaybackStatus::Playing;
    let rate = info.PlaybackRate().and_then(|r| r.Value()).unwrap_or(1.0);
    let start = timeline.StartTime()?.Duration;
    let end = timeline.EndTime()?.Duration;
    let position = timeline.Position()?.Duration;
    let updated = timeline.LastUpdatedTime().map(|t| t.UniversalTime).unwrap_or(0);
    let mut artist = properties.Artist()?.to_string();
    if artist.is_empty() {
        artist = properties.AlbumArtist().map(|a| a.to_string()).unwrap_or_default();
    }
    Ok(MediaSnapshot {
        app_id: session.SourceAppUserModelId().map(|s| s.to_string()).unwrap_or_default(),
        title: properties.Title()?.to_string(),
        artist,
        album: properties.AlbumTitle().map(|a| a.to_string()).unwrap_or_default(),
        playing,
        position: ticks_to_duration(position - start),
        duration: ticks_to_duration(end - start),
        sampled_at: instant_from_filetime(updated),
        rate,
        can_play_pause: controls.IsPlayPauseToggleEnabled().unwrap_or(false)
            || controls.IsPlayEnabled().unwrap_or(false)
            || controls.IsPauseEnabled().unwrap_or(false),
        can_next: controls.IsNextEnabled().unwrap_or(false),
        can_previous: controls.IsPreviousEnabled().unwrap_or(false),
        can_seek: controls.IsPlaybackPositionEnabled().unwrap_or(false) && end > start,
    })
}

fn ticks_to_duration(ticks: i64) -> Duration {
    Duration::from_nanos(ticks.max(0) as u64 * 100)
}

/// Maps a FILETIME (100 ns ticks since 1601) to a local `Instant`.
fn instant_from_filetime(ticks: i64) -> Instant {
    let now = Instant::now();
    if ticks <= 0 {
        return now;
    }
    let unix_ticks = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| (d.as_nanos() / 100) as i64).unwrap_or(0);
    let age = unix_ticks + FILETIME_UNIX_OFFSET - ticks;
    if !(0..=TICKS_PER_SECOND * 3600 * 12).contains(&age) {
        return now;
    }
    now.checked_sub(Duration::from_nanos(age as u64 * 100)).unwrap_or(now)
}

/// Reads the thumbnail stream on a blocking worker (the stream objects are
/// not agile, so they cannot live inside the async task).
async fn read_thumbnail(session: &Session) -> Option<Vec<u8>> {
    let session = session.clone();
    tokio::task::spawn_blocking(move || read_thumbnail_blocking(&session)).await.ok().flatten()
}

fn read_thumbnail_blocking(session: &Session) -> Option<Vec<u8>> {
    let properties = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
    let reference = properties.Thumbnail().ok()?;
    let stream = reference.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > 24 << 20 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    let load: IAsyncOperation<u32> = reader.LoadAsync(size as u32).ok()?.cast().ok()?;
    let loaded = load.join().ok()?;
    let mut buffer = vec![0u8; loaded as usize];
    reader.ReadBytes(&mut buffer).ok()?;
    Some(buffer)
}

/// Decodes and center-crops artwork to a 128x128 RGBA image.
pub fn decode_artwork(bytes: &[u8]) -> Option<ImageData> {
    let image = image::load_from_memory(bytes).ok()?;
    let square = image.resize_to_fill(ART_SIZE, ART_SIZE, image::imageops::FilterType::Triangle).to_rgba8();
    Some(ImageData {
        data: Blob::from(square.into_raw()),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width: ART_SIZE,
        height: ART_SIZE,
    })
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, b| (hash ^ *b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// Formats a duration as `mm:ss` (or `h:mm:ss`).
pub fn format_time(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
    } else {
        format!("{:02}:{:02}", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names() {
        assert_eq!(app_display_name("Spotify.exe"), "Spotify");
        assert_eq!(app_display_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "ZuneMusic");
    }

    #[test]
    fn time_format() {
        assert_eq!(format_time(Duration::from_secs(193)), "03:13");
        assert_eq!(format_time(Duration::from_secs(3725)), "1:02:05");
    }
}
