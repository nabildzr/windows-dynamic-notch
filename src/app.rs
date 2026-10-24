//! Application state machine: decides which of the ten modes the notch
//! shows, routes input, drives animations and schedules frames/timers.
//!
//! The event loop runs in `ControlFlow::Wait`; a frame is only rendered
//! when something changed or a spring is still moving, and the next wake-up
//! is the earliest pending deadline (collapse delay, popup expiry, blink,
//! minute change of the clock, timer second, next lyric line...).

use std::ffi::c_void;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDate, Timelike};
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use vello::Scene;
use vello::kurbo::{Affine, Circle, Point, Rect, Vec2};
use vello::peniko::{Color, Fill};
use windows::Win32::Foundation::{HWND, RECT};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{CursorIcon, Window, WindowAttributes, WindowId, WindowLevel};

use crate::animation::{ColorSpring, Rng, SpringConfig, SpringValue};
use crate::renderer::shapes::SHOULDER_RADIUS;
use crate::renderer::text::TextEngine;
use crate::renderer::{self, FrameResult, GpuContext, Renderer};
use crate::services::bluetooth::{AudioDevice, BluetoothEvent};
use crate::services::keys::LockKeys;
use crate::services::privacy::PrivacyState;
use crate::services::lyrics::{LyricLine, LyricsRequest, LyricsResult};
use crate::services::media::{Artwork, MediaCommand, MediaSnapshot};
use crate::services::power::PowerStatus;
use crate::services::storage::{Document, LoadedDocuments};
use crate::services::system::SystemStats;
use crate::services::audio::VolumeState;
use crate::services::controls::ControlsState;
use crate::services::notifications::Notification;
use crate::services::weather::{Location, Weather, WeatherEvent};
use crate::ui::clipboard::{ClipboardData, ClipboardHistory};
use crate::ui::stopwatch::{ClockKind, Stopwatch};
use crate::services::{Inbox, ServiceEvent, Services, sound};
use crate::ui::indicator::Indicator;
use crate::ui::lyrics::LyricsState;
use crate::ui::resting::Eyes;
use crate::ui::shelf::Shelf;
use crate::ui::timer::{Phase, Pomodoro, PomodoroData};
use crate::ui::todo::{TodoData, TodoList};
use crate::ui::water::{WaterData, WaterEntry, WaterTracker};
use crate::ui::{self, DropState, Feedback, Hit, HitRegion, Mode, Model, NotchShape, Page, Painter};
use crate::window::dragdrop::{self, DragEvent};
use crate::window::fullscreen::{self, ForegroundHooks};
use crate::window::tray::{self, TrayIcon};
use crate::window::{CANVAS_HEIGHT, CANVAS_WIDTH, Edge};
use crate::window::win32::{self, CanvasPlacement, HOTKEY_NEXT, HOTKEY_PREV, MenuCommand, MenuState, NotchWindow};

const COLLAPSE_DELAY: Duration = Duration::from_millis(300);
const POPUP_DURATION: Duration = Duration::from_secs(4);
const POPUP_LINGER: Duration = Duration::from_millis(1500);
const PEEK_DURATION: Duration = Duration::from_secs(4);
const DROP_LINGER: Duration = Duration::from_millis(1800);
/// Extra input region around the notch so anti-aliased edges are never
/// clipped by the (aliased) window region.
const REGION_MARGIN: f64 = 4.0;
/// Frame interval for ambient-only animation (sound wave, progress bar);
/// springs still run at the display refresh rate.
const AMBIENT_FRAME: Duration = Duration::from_millis(40);
/// Lyrics animate more (karaoke sweep, glow), so they get smoother frames.
const LYRICS_FRAME: Duration = Duration::from_millis(25);
const CARET_BLINK: Duration = Duration::from_millis(530);
const WHEEL_STEP: f64 = 60.0;
const WHEEL_COOLDOWN: Duration = Duration::from_millis(380);
const DRAG_THRESHOLD: f64 = 5.0;
/// How long a fullscreen window must stay in front before the notch hides.
const FULLSCREEN_SETTLE: Duration = Duration::from_millis(500);
/// Resident memory housekeeping: once the notch has been idle for a few
/// seconds, pages touched only during initialization or a burst of
/// animation (shader compiler scratch, driver setup) are released from the
/// working set. They are faulted back in transparently if ever needed.
const IDLE_TRIM_AFTER: Duration = Duration::from_secs(3);
const TRIM_MIN_INTERVAL: Duration = Duration::from_secs(10);
const TRIM_THRESHOLD_MB: f64 = 32.0;

/// Events delivered to the winit loop from other threads, COM callbacks and
/// the subclassed window procedure.
pub enum UserEvent {
    /// Services queued data in the inbox.
    Services,
    /// Background GPU initialization finished.
    Gpu(Result<Box<GpuContext>, String>),
    /// Font database loaded.
    FontsReady,
    Hotkey(i32),
    DisplayChanged,
    SessionEnding,
    Drag(DragEvent),
    /// The foreground window changed or moved (fullscreen detection).
    ForegroundChanged,
    /// The tray icon was clicked.
    TrayMenu,
    /// Explorer restarted; the tray icon must be added again.
    TrayRecreate,
    /// The clipboard content changed (clipboard history).
    ClipboardChanged,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    /// Music page shows the lyrics view.
    lyrics: bool,
    /// Screen edge the notch hangs from (top by default, Y = 0).
    edge: Edge,
    /// Keep a taskbar docked on that edge visible by attaching the notch to
    /// the taskbar instead of the screen edge.
    #[serde(alias = "below_top_taskbar")]
    avoid_taskbar: bool,
    /// Warn once when the battery (not charging) drops to the threshold.
    low_battery: bool,
    /// Warning level in percent, 1..=90.
    low_battery_threshold: u8,
    /// Popup when Caps Lock / Num Lock change.
    lock_indicator: bool,
    /// Popup for volume, mute and brightness changes.
    media_indicator: bool,
    /// Green (camera) and orange (microphone) dots while they are in use.
    privacy_indicator: bool,
    /// Hide while a game or other fullscreen app is in front.
    hide_in_fullscreen: bool,
    /// GDI device name of the display the notch is on (primary when unset).
    monitor: Option<String>,
    /// Widgets removed from the carousel.
    hidden_pages: Vec<Page>,
    accent: Accent,
    /// Music and lyrics glow in the dominant color of the album art.
    album_colors: bool,
    /// Warn once when C: fills up past the threshold.
    disk_warning: bool,
    /// Percent used, 50..=99.
    disk_threshold: u8,
    /// Popups for notifications from other apps.
    notifications: bool,
    /// Record copied texts.
    clipboard: bool,
    /// City for the weather widget.
    weather: Option<Location>,
}

/// Theme colors (glows, gradients, highlights).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Accent {
    #[default]
    Aurora,
    Ocean,
    Violet,
    Rose,
    Mint,
    Sunset,
    Mono,
}

impl Accent {
    const ALL: [Accent; 7] = [Accent::Aurora, Accent::Ocean, Accent::Violet, Accent::Rose, Accent::Mint, Accent::Sunset, Accent::Mono];

    fn name(self) -> &'static str {
        match self {
            Accent::Aurora => "Aurora",
            Accent::Ocean => "Ocean",
            Accent::Violet => "Violet",
            Accent::Rose => "Rose",
            Accent::Mint => "Mint",
            Accent::Sunset => "Sunset",
            Accent::Mono => "Mono",
        }
    }

    /// Primary and companion color.
    fn colors(self) -> (Color, Color) {
        use crate::ui::rgb;
        match self {
            Accent::Aurora => (ui::palette::CYAN, ui::palette::PURPLE),
            Accent::Ocean => (rgb(0x38BDF8), rgb(0x2563EB)),
            Accent::Violet => (rgb(0xA78BFA), rgb(0xD946EF)),
            Accent::Rose => (rgb(0xFB7185), rgb(0xF472B6)),
            Accent::Mint => (rgb(0x34D399), rgb(0x22D3EE)),
            Accent::Sunset => (rgb(0xFBBF24), rgb(0xF97316)),
            Accent::Mono => (rgb(0xE5E7EB), rgb(0x9CA3AF)),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            lyrics: false,
            edge: Edge::Top,
            avoid_taskbar: false,
            low_battery: true,
            low_battery_threshold: 20,
            lock_indicator: true,
            media_indicator: true,
            privacy_indicator: true,
            hide_in_fullscreen: true,
            monitor: None,
            hidden_pages: Vec::new(),
            accent: Accent::Aurora,
            album_colors: true,
            disk_warning: true,
            disk_threshold: 90,
            notifications: true,
            clipboard: true,
            weather: None,
        }
    }
}

struct Transition {
    from: Mode,
    progress: SpringValue,
    /// -1/+1 when sliding between pages, 0 for resizes.
    direction: f64,
}

struct Popup {
    mode: Mode,
    until: Instant,
    /// Hovering a popup keeps it on screen.
    paused: bool,
}

struct Press {
    hit: Hit,
    origin: Point,
}

/// `--demo`: cycles through every mode with sample data (presentation and
/// visual testing). Nothing is persisted in demo mode.
struct Demo {
    index: usize,
    next_at: Instant,
    /// `--mode <name>` pins one mode instead of cycling.
    pinned: Option<Mode>,
}

const DEMO_SEQUENCE: [Mode; 18] = [
    Mode::Resting,
    Mode::Closed,
    Mode::Music,
    Mode::Lyrics,
    Mode::Timer,
    Mode::Water,
    Mode::Shelf,
    Mode::Stopwatch,
    Mode::Weather,
    Mode::Clipboard,
    Mode::Todo,
    Mode::Controls,
    Mode::System,
    Mode::Calendar,
    Mode::Headphones,
    Mode::Charging,
    Mode::Indicator,
    Mode::Notification,
];
const DEMO_STEP: Duration = Duration::from_millis(2600);

pub struct App {
    _rt: Handle,
    proxy: EventLoopProxy<UserEvent>,
    inbox: Inbox,
    services: Services,
    started: Instant,
    rng: Rng,

    window: Option<Arc<Window>>,
    native: Option<NotchWindow>,
    tray: Option<TrayIcon>,
    _foreground_hooks: Option<ForegroundHooks>,
    renderer: Option<Renderer>,
    placement: CanvasPlacement,
    text: TextEngine,
    scene: Scene,

    mode: Mode,
    /// Edge the notch is currently attached to (follows `settings.edge`).
    edge: Edge,
    notch: NotchShape,
    transition: Option<Transition>,
    feedback: Feedback,
    hits: Vec<HitRegion>,
    page: Page,
    /// Widget used last in this session; cleared when a new song starts.
    session_page: Option<Page>,
    page_indicator: SpringValue,
    eyes: Eyes,
    shelf_scroll: SpringValue,
    todo_scroll: SpringValue,
    /// Shelf drop-zone highlight while files hover over the notch.
    drop_tint: ColorSpring,
    drop_amount: SpringValue,

    cursor: Option<Point>,
    cursor_icon: CursorIcon,
    hovered: bool,
    expanded: bool,
    collapse_at: Option<Instant>,
    peek_until: Option<Instant>,
    popup: Option<Popup>,
    pressed: Option<Press>,
    drop: DropState,
    scrub: Option<f64>,
    modifiers: ModifiersState,
    caret_epoch: Instant,
    wheel_accum: f64,
    wheel_ready_at: Instant,
    minutes_accum: f64,

    settings: Settings,
    loaded: bool,
    power: PowerStatus,
    power_known: bool,
    media: Option<MediaSnapshot>,
    track_key: Option<String>,
    artwork: Option<Arc<Artwork>>,
    lyrics: LyricsState,
    timer: Pomodoro,
    water: WaterTracker,
    shelf: Shelf,
    todo: TodoList,
    headphones: Vec<AudioDevice>,
    indicator: Option<Indicator>,
    system: SystemStats,
    calendar_offset: i32,
    lock_keys: Option<LockKeys>,
    privacy: PrivacyState,
    /// The low-battery warning was shown for the current discharge.
    low_battery_warned: bool,
    /// A fullscreen app came to the front at this moment (hide pending).
    fullscreen_since: Option<Instant>,
    /// The disk-full warning was shown for the current crossing.
    disk_warned: bool,
    clipboard: ClipboardHistory,
    clipboard_scroll: SpringValue,
    /// Text this app put on the clipboard itself (not recorded again).
    clipboard_own: Option<String>,
    stopwatch: Stopwatch,
    weather: Option<Weather>,
    weather_error: Option<String>,
    controls: ControlsState,
    volume: VolumeState,
    /// Slider position while the volume slider is dragged.
    volume_preview: Option<f32>,
    volume_sent_at: Instant,
    notification: Option<Notification>,
    /// Animated album-art color and whether one is known for this track.
    album_color: ColorSpring,
    album_known: bool,
    /// Widgets currently in the carousel.
    pages: Vec<Page>,
    today: NaiveDate,

    last_frame: Option<Instant>,
    last_activity: Instant,
    frames_since_idle: u32,
    last_trim: Option<Instant>,
    trim_pending: bool,
    animating: bool,
    dirty: bool,
    next_ambient: Option<Instant>,
    next_clock_tick: Instant,
    next_visual: Option<Instant>,
    demo: Option<Demo>,
    demo_audio: bool,
    /// `--pin <mode>`: keeps one mode on screen with live data (testing).
    pinned: Option<Mode>,
}

impl App {
    pub fn new(rt: Handle, proxy: EventLoopProxy<UserEvent>, inbox: Inbox, services: Services) -> Self {
        let now = Instant::now();
        let mut rng = Rng::seeded();
        let fonts_proxy = proxy.clone();
        let text = TextEngine::new(move || {
            let _ = fonts_proxy.send_event(UserEvent::FontsReady);
        });
        let eyes = Eyes::new(now, &mut rng);
        let args: Vec<String> = std::env::args().collect();
        let mode_arg = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .and_then(|i| args.get(i + 1))
                .and_then(|name| DEMO_SEQUENCE.iter().copied().find(|m| format!("{m:?}").eq_ignore_ascii_case(name)))
        };
        let pinned = mode_arg("--mode");
        let live_pin = mode_arg("--pin");
        let demo = (args.iter().any(|a| a == "--demo") || pinned.is_some())
            .then(|| Demo { index: 0, next_at: now + DEMO_STEP * 2, pinned });
        let mut app = Self {
            _rt: rt,
            proxy,
            inbox,
            services,
            started: now,
            rng,
            window: None,
            native: None,
            tray: None,
            _foreground_hooks: None,
            renderer: None,
            placement: win32::canvas_placement(Edge::Top, false, None),
            text,
            scene: Scene::new(),
            mode: Mode::Resting,
            edge: Edge::Top,
            notch: NotchShape::new(Mode::Resting, Edge::Top),
            transition: None,
            feedback: Feedback::default(),
            hits: Vec::new(),
            page: Page::Music,
            session_page: None,
            page_indicator: SpringValue::normalized(0.0, SpringConfig::HOVER),
            eyes,
            shelf_scroll: SpringValue::new(0.0, SpringConfig::HOVER),
            todo_scroll: SpringValue::new(0.0, SpringConfig::HOVER),
            drop_tint: ColorSpring::new(ui::palette::ZONE_BORDER, SpringConfig::HOVER),
            drop_amount: SpringValue::normalized(0.0, SpringConfig::HOVER),
            cursor: None,
            cursor_icon: CursorIcon::Default,
            hovered: false,
            expanded: false,
            collapse_at: None,
            peek_until: None,
            popup: None,
            pressed: None,
            drop: DropState::None,
            scrub: None,
            modifiers: ModifiersState::empty(),
            caret_epoch: now,
            wheel_accum: 0.0,
            wheel_ready_at: now,
            minutes_accum: 0.0,
            settings: Settings::default(),
            loaded: false,
            power: crate::services::power::query(),
            power_known: false,
            media: None,
            track_key: None,
            artwork: None,
            lyrics: LyricsState::default(),
            timer: Pomodoro::new(PomodoroData::default()),
            water: WaterTracker::new(WaterData::default(), now),
            shelf: Shelf::default(),
            todo: TodoList::new(TodoData::default()),
            headphones: Vec::new(),
            indicator: None,
            system: SystemStats::default(),
            calendar_offset: 0,
            lock_keys: None,
            privacy: PrivacyState::default(),
            low_battery_warned: false,
            fullscreen_since: None,
            disk_warned: false,
            clipboard: ClipboardHistory::default(),
            clipboard_scroll: SpringValue::new(0.0, SpringConfig::HOVER),
            clipboard_own: None,
            stopwatch: Stopwatch::default(),
            weather: None,
            weather_error: None,
            controls: ControlsState::default(),
            volume: VolumeState { level: 0.0, muted: false },
            volume_preview: None,
            volume_sent_at: now,
            notification: None,
            album_color: ColorSpring::new(ui::palette::INDIGO, SpringConfig::GENTLE),
            album_known: false,
            pages: Page::ALL.to_vec(),
            today: Local::now().date_naive(),
            last_frame: None,
            last_activity: now,
            frames_since_idle: 0,
            last_trim: None,
            // Armed by the first rendered frames, never during GPU start-up.
            trim_pending: false,
            animating: false,
            dirty: true,
            next_ambient: None,
            next_clock_tick: next_minute(now),
            next_visual: None,
            demo,
            demo_audio: false,
            pinned: live_pin,
        };
        if app.demo.is_some() {
            app.load_demo_data(now);
        }
        let shown = pinned.or(live_pin);
        if let Some(page) = shown.and_then(Mode::page) {
            app.page = page;
            app.page_indicator.snap(page.index_in(&app.pages) as f32);
            app.settings.lyrics = shown == Some(Mode::Lyrics);
        }
        app
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        renderer::log_memory("startup");
        self.edge = self.settings.edge;
        self.notch.set_edge(self.edge);
        let placement = win32::canvas_placement(self.edge, self.settings.avoid_taskbar, self.settings.monitor.as_deref());
        let attributes = WindowAttributes::default()
            .with_title("Windows Dynamic Notch")
            .with_decorations(false)
            .with_resizable(false)
            .with_visible(false)
            .with_active(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(PhysicalSize::new(placement.width, placement.height))
            .with_position(PhysicalPosition::new(placement.x, placement.y))
            .with_no_redirection_bitmap(true)
            .with_drag_and_drop(false)
            .with_skip_taskbar(true)
            .with_undecorated_shadow(false)
            .with_class_name("WindowsDynamicNotch");
        let window = Arc::new(event_loop.create_window(attributes).map_err(|err| format!("create_window: {err}"))?);
        let hwnd = hwnd_of(&window)?;
        let native = NotchWindow::attach(hwnd, self.proxy.clone()).map_err(|err| format!("window setup: {err}"))?;
        // SAFETY: the composition visual lives in `native`, which outlives the renderer.
        let renderer =
            unsafe { Renderer::new(native.composition().visual_ptr(), placement.width, placement.height) }?;

        let instance = renderer.instance();
        let proxy = self.proxy.clone();
        std::thread::Builder::new()
            .name("gpu-init".into())
            .spawn(move || {
                let result = GpuContext::create(&instance).map(Box::new);
                let _ = proxy.send_event(UserEvent::Gpu(result));
            })
            .map_err(|err| format!("cannot spawn gpu-init: {err}"))?;

        native.place(&placement);
        self.tray = Some(TrayIcon::new(hwnd));
        self._foreground_hooks = Some(ForegroundHooks::install());
        self.placement = placement;
        self.window = Some(window);
        self.native = Some(native);
        self.renderer = Some(renderer);
        self.update_mode();
        self.sync_input_geometry();
        self.update_fullscreen();
        Ok(())
    }

    // ----------------------------------------------------------------- modes

    fn audio_playing(&self) -> bool {
        self.demo_audio || self.media.as_ref().is_some_and(|m| m.playing)
    }

    /// Music playing or a focus timer counting down.
    fn has_activity(&self) -> bool {
        self.audio_playing() || self.timer.running()
    }

    fn resolve_mode(&self) -> Mode {
        if let Some(Demo { pinned: Some(mode), .. }) = &self.demo {
            return *mode;
        }
        if let Some(mode) = self.pinned {
            return mode;
        }
        if self.drop == DropState::Hovering {
            return Mode::Shelf;
        }
        if self.expanded {
            return self.page_mode();
        }
        if let Some(popup) = &self.popup {
            return popup.mode;
        }
        if self.has_activity() { Mode::Closed } else { Mode::Resting }
    }

    fn page_mode(&self) -> Mode {
        match self.page {
            Page::Music if self.settings.lyrics => Mode::Lyrics,
            Page::Music => Mode::Music,
            Page::Timer => Mode::Timer,
            Page::Water => Mode::Water,
            Page::Stopwatch => Mode::Stopwatch,
            Page::Weather => Mode::Weather,
            Page::Shelf => Mode::Shelf,
            Page::Clipboard => Mode::Clipboard,
            Page::Todo => Mode::Todo,
            Page::Controls => Mode::Controls,
            Page::System => Mode::System,
            Page::Calendar => Mode::Calendar,
        }
    }

    fn update_mode(&mut self) {
        let target = self.resolve_mode();
        if target == self.mode {
            return;
        }
        let from = self.mode;
        let direction = match (from.page(), target.page()) {
            (Some(a), Some(b)) if a != b => (b.index_in(&self.pages) as f64 - a.index_in(&self.pages) as f64).signum(),
            _ => 0.0,
        };
        log::debug!("mode {from:?} -> {target:?}");
        if from == Mode::Todo && self.todo.editing.is_some() {
            self.finish_editing(true);
        }
        if from == Mode::Water {
            self.water.settings_open = false;
        }
        if from == Mode::Calendar {
            self.calendar_offset = 0;
        }
        // System stats are only sampled while the widget is visible.
        if self.demo.is_none() {
            self.services.system.set_active(target == Mode::System);
            if target == Mode::Controls {
                self.services.controls.refresh();
            }
        }
        self.mode = target;
        self.notch.set_mode(target);
        let mut progress = SpringValue::normalized(0.0, SpringConfig::HOVER);
        progress.set_target(1.0);
        self.transition = Some(Transition { from, progress, direction });
        if target == Mode::Resting {
            self.eyes = Eyes::new(Instant::now(), &mut self.rng);
        }
        if target != Mode::Resting
            && let Some(native) = &self.native
        {
            native.raise();
        }
        self.feedback.set_hovered(None);
        self.ensure_lyrics();
        self.update_hotkeys();
        self.sync_input_geometry();
        self.dirty = true;
    }

    fn preferred_page(&self) -> Page {
        let wanted = match self.session_page {
            Some(page) => page,
            None if self.audio_playing() => Page::Music,
            None if !self.shelf.items.is_empty() => Page::Shelf,
            None => Page::Timer,
        };
        if self.pages.contains(&wanted) { wanted } else { self.pages.first().copied().unwrap_or(Page::Music) }
    }

    /// Recomputes the carousel from the widget settings.
    fn refresh_pages(&mut self) {
        let pages: Vec<Page> = Page::ALL.into_iter().filter(|p| !self.settings.hidden_pages.contains(p)).collect();
        self.pages = if pages.is_empty() { vec![Page::Music] } else { pages };
        if !self.pages.contains(&self.page) {
            self.page = self.pages[0];
            self.update_mode();
        }
        self.page_indicator.snap(self.page.index_in(&self.pages) as f32);
        self.dirty = true;
    }

    fn expand(&mut self) {
        if !self.expanded {
            self.expanded = true;
            self.page = self.preferred_page();
            self.page_indicator.snap(self.page.index_in(&self.pages) as f32);
        }
        self.update_mode();
    }

    fn collapse(&mut self) {
        self.expanded = false;
        self.peek_until = None;
        self.update_mode();
    }

    /// Opens a widget for a few seconds without user interaction (timer
    /// finished, water reminder).
    fn peek(&mut self, page: Page) {
        if self.expanded || self.drop != DropState::None || !self.pages.contains(&page) {
            return;
        }
        self.popup = None;
        self.page = page;
        self.page_indicator.snap(page.index_in(&self.pages) as f32);
        self.expanded = true;
        self.peek_until = Some(Instant::now() + PEEK_DURATION);
        self.update_mode();
    }

    fn set_page(&mut self, page: Page) {
        if page == self.page {
            return;
        }
        self.page = page;
        self.session_page = Some(page);
        self.page_indicator.set_target(page.index_in(&self.pages) as f32);
        self.update_mode();
    }

    fn navigate(&mut self, delta: i32) {
        if !self.mode.is_expanded() {
            return;
        }
        if let Some(page) = self.page.offset_in(&self.pages, delta) {
            self.set_page(page);
        }
    }

    fn show_popup(&mut self, mode: Mode) {
        if self.expanded || self.drop != DropState::None {
            return;
        }
        self.popup = Some(Popup { mode, until: Instant::now() + POPUP_DURATION, paused: self.hovered });
        self.update_mode();
    }

    /// Short status popup (volume, brightness, lock keys, low battery). A
    /// new value replaces the one on screen and restarts its timer.
    fn show_indicator(&mut self, indicator: Indicator) {
        if self.expanded || self.drop != DropState::None || self.native.as_ref().is_some_and(NotchWindow::is_hidden) {
            return;
        }
        let paused = self.popup.as_ref().is_some_and(|p| p.paused);
        self.indicator = Some(indicator);
        self.popup = Some(Popup { mode: Mode::Indicator, until: Instant::now() + indicator.duration(), paused });
        self.update_mode();
        self.dirty = true;
    }

    /// Warns once per discharge when the battery reaches the threshold.
    fn check_low_battery(&mut self) {
        let power = self.power;
        let Some(percent) = power.percent.filter(|_| power.has_battery) else { return };
        let threshold = self.settings.low_battery_threshold.clamp(1, 90);
        if power.ac_online || power.charging || percent > threshold {
            self.low_battery_warned = false;
            return;
        }
        if self.settings.low_battery && self.loaded && !self.low_battery_warned {
            self.low_battery_warned = true;
            log::info!("battery low: {percent}% (threshold {threshold}%)");
            sound::play_chime();
            self.show_indicator(Indicator::LowBattery(percent));
        }
    }

    /// Hides the notch while a fullscreen app is in front of its monitor
    /// (when enabled) and brings it back afterwards.
    /// Hiding waits until the fullscreen window has stayed in front for a
    /// moment (app switching briefly shows monitor-sized windows); showing
    /// again is immediate.
    fn update_fullscreen(&mut self) {
        let Some(native) = self.native.as_ref() else { return };
        let active = self.settings.hide_in_fullscreen && fullscreen::fullscreen_app_active(native.hwnd());
        if !active {
            self.fullscreen_since = None;
            self.set_hidden(false);
        } else if !native.is_hidden() {
            let now = Instant::now();
            match self.fullscreen_since {
                Some(since) if now >= since + FULLSCREEN_SETTLE => {
                    self.fullscreen_since = None;
                    self.set_hidden(true);
                }
                Some(_) => {}
                None => self.fullscreen_since = Some(now),
            }
        }
    }

    fn set_hidden(&mut self, hide: bool) {
        let Some(native) = self.native.as_mut() else { return };
        if hide == native.is_hidden() {
            return;
        }
        log::info!("fullscreen app {}", if hide { "in front: hiding" } else { "gone: showing" });
        native.set_hidden(hide);
        if hide {
            self.hovered = false;
            self.cursor = None;
            self.pressed = None;
            self.popup = None;
            self.finish_editing(true);
            self.collapse();
        }
        self.dirty = true;
    }

    /// Manual access to a popup from the expanded notch.
    fn open_popup(&mut self, mode: Mode) {
        self.expanded = false;
        self.collapse_at = None;
        self.peek_until = None;
        self.popup = Some(Popup { mode, until: Instant::now() + POPUP_DURATION, paused: self.hovered });
        self.update_mode();
    }

    fn ensure_lyrics(&mut self) {
        if self.mode != Mode::Lyrics || self.demo.is_some() {
            return;
        }
        let Some(media) = &self.media else { return };
        let key = media.track_key();
        if self.lyrics.begin(&key, Instant::now()) {
            self.services.lyrics.request(LyricsRequest {
                key,
                title: media.title.clone(),
                artist: media.artist.clone(),
                album: media.album.clone(),
                duration: media.duration,
            });
        }
    }

    // ----------------------------------------------------------------- input

    fn logical(&self, position: PhysicalPosition<f64>) -> Point {
        Point::new(position.x / self.placement.scale, position.y / self.placement.scale)
    }

    fn hit_at_cursor(&self) -> Option<Hit> {
        self.cursor.and_then(|c| ui::hit_at(&self.hits, c))
    }

    fn on_cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        let point = self.logical(position);
        self.cursor = Some(point);
        self.collapse_at = None;
        if !self.hovered {
            self.hovered = true;
            self.peek_until = None;
            match &mut self.popup {
                Some(popup) => popup.paused = true,
                None => self.expand(),
            }
            self.update_hotkeys();
        }
        if let Some(press) = &self.pressed {
            match press.hit {
                Hit::MediaScrubber => {
                    self.scrub = self.scrub_fraction(point);
                    self.dirty = true;
                }
                Hit::CtlVolume => {
                    self.drag_volume(point, false);
                }
                Hit::ShelfItem(id) if (point - press.origin).hypot() > DRAG_THRESHOLD => {
                    self.drag_out(id);
                    return;
                }
                _ => {}
            }
        }
        let hit = ui::hit_at(&self.hits, point);
        if self.feedback.set_hovered(hit) {
            self.dirty = true;
        }
        self.update_cursor_icon(hit);
    }

    fn update_cursor_icon(&mut self, hit: Option<Hit>) {
        let icon = match hit {
            Some(Hit::TodoList) | None => CursorIcon::Default,
            Some(Hit::TodoInput) => CursorIcon::Text,
            Some(_) => CursorIcon::Pointer,
        };
        if icon != self.cursor_icon {
            self.cursor_icon = icon;
            if let Some(window) = &self.window {
                window.set_cursor(icon);
            }
        }
    }

    fn on_cursor_left(&mut self) {
        self.cursor = None;
        self.hovered = false;
        if self.pressed.as_ref().is_some_and(|p| !matches!(p.hit, Hit::MediaScrubber | Hit::CtlVolume)) {
            self.pressed = None;
            self.feedback.set_pressed(None);
        }
        self.feedback.set_hovered(None);
        if self.expanded || self.popup.is_some() {
            self.collapse_at = Some(Instant::now() + COLLAPSE_DELAY);
        }
        self.update_hotkeys();
        self.dirty = true;
    }

    fn scrub_fraction(&self, point: Point) -> Option<f64> {
        let region = self.hits.iter().find(|r| r.id == Hit::MediaScrubber)?;
        Some(((point.x - region.rect.x0) / region.rect.width().max(1.0)).clamp(0.0, 1.0))
    }

    fn on_mouse_input(&mut self, event_loop: &ActiveEventLoop, state: ElementState, button: MouseButton) {
        let hit = self.hit_at_cursor();
        match (button, state) {
            (MouseButton::Left, ElementState::Pressed) => {
                if self.todo.editing.is_some() && hit != Some(Hit::TodoInput) && hit != Some(Hit::TodoAdd) {
                    self.finish_editing(true);
                }
                self.pressed = hit.zip(self.cursor).map(|(hit, origin)| Press { hit, origin });
                self.feedback.set_pressed(hit);
                if hit == Some(Hit::MediaScrubber) {
                    self.scrub = self.cursor.and_then(|c| self.scrub_fraction(c));
                }
                if hit == Some(Hit::CtlVolume)
                    && let Some(cursor) = self.cursor
                {
                    self.drag_volume(cursor, true);
                }
            }
            (MouseButton::Left, ElementState::Released) => {
                let press = self.pressed.take();
                self.feedback.set_pressed(None);
                if let Some(press) = press {
                    if press.hit == Hit::MediaScrubber {
                        self.finish_scrub();
                    } else if press.hit == Hit::CtlVolume {
                        if let Some(level) = self.volume_preview.take() {
                            self.services.audio.set_level(level);
                            self.volume.level = level;
                        }
                    } else if hit == Some(press.hit) {
                        self.on_click(press.hit);
                    }
                }
            }
            (MouseButton::Right, ElementState::Released) => self.context_menu(event_loop),
            _ => {}
        }
        self.dirty = true;
    }

    /// Volume slider drag: preview locally, send to the device at most
    /// every 50 ms.
    fn drag_volume(&mut self, point: Point, force: bool) {
        let Some(region) = self.hits.iter().find(|r| r.id == Hit::CtlVolume) else { return };
        let (x0, x1) = (region.rect.x0 + 8.0, region.rect.x1 - 8.0);
        let level = ((point.x - x0) / (x1 - x0).max(1.0)).clamp(0.0, 1.0) as f32;
        self.volume_preview = Some(level);
        let now = Instant::now();
        if force || now.duration_since(self.volume_sent_at) >= Duration::from_millis(50) {
            self.volume_sent_at = now;
            self.services.audio.set_level(level);
        }
        self.dirty = true;
    }

    fn finish_scrub(&mut self) {
        let Some(fraction) = self.scrub.take() else { return };
        let Some(media) = self.media.as_mut() else { return };
        let target = media.duration.mul_f64(fraction);
        // Optimistic update until the session reports the new timeline.
        media.position = target;
        media.sampled_at = Instant::now();
        self.services.media.send(MediaCommand::Seek(target));
    }

    fn on_click(&mut self, hit: Hit) {
        let now = Instant::now();
        let local = Local::now();
        let today = local.date_naive();
        match hit {
            Hit::NavPrev => self.navigate(-1),
            Hit::NavNext => self.navigate(1),
            Hit::PageDot(index) => {
                if let Some(page) = self.pages.get(index as usize).copied() {
                    self.set_page(page);
                }
            }
            Hit::StatusBattery => self.open_popup(Mode::Charging),
            Hit::StatusWave => self.open_popup(Mode::Headphones),
            Hit::Popup => {
                self.popup = None;
                self.expand();
            }
            Hit::MediaPrev => self.services.media.send(MediaCommand::Previous),
            Hit::MediaNext => self.services.media.send(MediaCommand::Next),
            Hit::MediaPlayPause => {
                self.services.media.send(MediaCommand::PlayPause);
                if let Some(media) = self.media.as_mut() {
                    media.position = media.position_at(now);
                    media.sampled_at = now;
                    media.playing = !media.playing;
                }
            }
            Hit::MediaLyrics => {
                self.settings.lyrics = !self.settings.lyrics;
                self.save_settings();
                self.update_mode();
            }
            Hit::MediaScrubber | Hit::TimerDigits => {}
            Hit::TimerReset => self.timer.reset(),
            Hit::TimerBreak => self.timer.select(Phase::ShortBreak),
            Hit::TimerFocus => self.timer.select(Phase::Focus),
            Hit::TimerPlayPause => {
                self.timer.toggle(now);
                self.update_mode();
            }
            Hit::WaterToggle => {
                self.water.toggle_reminder(now);
                self.save_water();
            }
            Hit::WaterDrink => {
                self.water.drink(local, now);
                self.save_water();
            }
            Hit::WaterSettings => self.water.settings_open = true,
            Hit::WaterDone => self.water.settings_open = false,
            Hit::WaterUndo => {
                if self.water.undo(today) {
                    self.save_water();
                }
            }
            Hit::WaterGoalDec | Hit::WaterGoalInc => {
                self.water.adjust_goal(if hit == Hit::WaterGoalInc { 1 } else { -1 });
                self.save_water();
            }
            Hit::WaterCupDec | Hit::WaterCupInc => {
                self.water.adjust_cup(if hit == Hit::WaterCupInc { 1 } else { -1 });
                self.save_water();
            }
            Hit::WaterIntervalDec | Hit::WaterIntervalInc => {
                self.water.adjust_interval(if hit == Hit::WaterIntervalInc { 1 } else { -1 });
                self.save_water();
            }
            Hit::ShelfClear => {
                self.shelf.clear();
                self.shelf_scroll.snap(0.0);
                self.save_shelf();
            }
            Hit::ShelfItem(id) => {
                if let Some(item) = self.shelf.get(id)
                    && !win32::open_path(&item.path)
                {
                    log::warn!("cannot open {}", item.path.display());
                }
            }
            Hit::ShelfRemove(id) => {
                if self.shelf.remove(id) {
                    let max = self.shelf.max_scroll() as f32;
                    self.shelf_scroll.set_target(self.shelf_scroll.target().min(max));
                    self.save_shelf();
                }
            }
            Hit::TodoList | Hit::TodoInput => {}
            Hit::TodoAdd => {
                if self.todo.editing.is_none() {
                    self.todo.begin_add();
                    self.todo_scroll.set_target(0.0);
                    self.start_text_input();
                }
            }
            Hit::TodoEdit => {
                self.finish_editing(true);
                self.todo.edit_mode = !self.todo.edit_mode;
            }
            Hit::TodoCheck(id) => {
                self.todo.toggle(id, today);
                self.save_todo();
            }
            Hit::TodoText(id) => {
                if self.todo.edit_mode {
                    self.todo.begin_rename(id);
                    self.start_text_input();
                } else {
                    self.todo.toggle(id, today);
                    self.save_todo();
                }
            }
            Hit::CalendarPrev => self.calendar_offset = (self.calendar_offset - 1).max(-240),
            Hit::CalendarNext => self.calendar_offset = (self.calendar_offset + 1).min(240),
            Hit::CalendarToday => self.calendar_offset = 0,
            Hit::ClipList => {}
            Hit::ClipItem(id) => self.copy_clip(id),
            Hit::ClipPin(id) => {
                if self.clipboard.toggle_pin(id) {
                    self.save_clipboard();
                }
            }
            Hit::ClipClear => {
                self.clipboard.clear();
                self.clipboard_scroll.snap(0.0);
                self.save_clipboard();
            }
            Hit::SwModeStopwatch => self.stopwatch.kind = ClockKind::Stopwatch,
            Hit::SwModeTimer => self.stopwatch.kind = ClockKind::Countdown,
            Hit::SwStartPause => self.stopwatch.toggle(now),
            Hit::SwLapReset => self.stopwatch.lap_or_reset(now),
            Hit::SwMinus => {
                self.stopwatch.adjust(-60);
            }
            Hit::SwPlus => {
                self.stopwatch.adjust(60);
            }
            Hit::SwDigits => {}
            Hit::WeatherCity => self.edit_weather_city(),
            Hit::CtlWifi => self.services.controls.toggle_wifi(),
            Hit::CtlBluetooth => self.services.controls.toggle_bluetooth(),
            Hit::CtlPower => {
                if let Some(mode) = self.controls.power {
                    let next = mode.next();
                    // Optimistic until the service confirms.
                    self.controls.power = Some(next);
                    self.services.controls.set_power(next);
                }
            }
            Hit::CtlFocus => {
                if !win32::open_uri("ms-settings:notifications") {
                    log::warn!("cannot open notification settings");
                }
            }
            Hit::CtlMute => self.services.audio.toggle_mute(),
            Hit::CtlVolume => {}            Hit::TodoDelete(id) => {
                self.todo.delete(id);
                let max = self.todo.max_scroll() as f32;
                self.todo_scroll.set_target(self.todo_scroll.target().min(max));
                self.save_todo();
            }
        }
    }

    fn drag_out(&mut self, id: u64) {
        self.pressed = None;
        self.feedback.set_pressed(None);
        let (Some(item), Some(native)) = (self.shelf.get(id), &self.native) else { return };
        let path = item.path.clone();
        match dragdrop::drag_file_out(native.hwnd(), &path) {
            Ok(effect) => {
                log::info!("dragged {} out (effect {:#x})", path.display(), effect.0);
                if !path.exists() {
                    self.shelf.remove(id);
                    self.save_shelf();
                }
            }
            Err(err) => log::warn!("drag out failed: {err}"),
        }
        // The modal drag loop consumed the button release and cursor moves.
        self.hovered = false;
        self.cursor = None;
        self.feedback.set_hovered(None);
        self.collapse_at = Some(Instant::now() + COLLAPSE_DELAY);
        self.update_hotkeys();
        self.dirty = true;
    }

    fn on_wheel(&mut self, delta: MouseScrollDelta) {
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x as f64 * 40.0, y as f64 * 40.0),
            MouseScrollDelta::PixelDelta(pos) => (pos.x / self.placement.scale, pos.y / self.placement.scale),
        };
        let now = Instant::now();
        if dx.abs() > dy.abs() {
            if self.mode == Mode::Shelf && self.shelf.max_scroll() > 0.0 && self.hit_at_cursor().is_some_and(|h| matches!(h, Hit::ShelfItem(_) | Hit::ShelfRemove(_))) {
                self.scroll_shelf(dx);
                return;
            }
            self.wheel_accum += dx;
            if now >= self.wheel_ready_at && self.wheel_accum.abs() >= WHEEL_STEP {
                // winit reports positive x when content should move right,
                // i.e. the user scrolled toward the previous page.
                self.navigate(if self.wheel_accum < 0.0 { 1 } else { -1 });
                self.wheel_accum = 0.0;
                self.wheel_ready_at = now + WHEEL_COOLDOWN;
            }
            return;
        }
        match self.mode {
            Mode::Todo => {
                let max = self.todo.max_scroll() as f32;
                let target = (self.todo_scroll.target() - dy as f32).clamp(0.0, max);
                self.todo_scroll.set_target(target);
                self.dirty = true;
            }
            Mode::Shelf => self.scroll_shelf(dy),
            Mode::Clipboard => {
                let max = self.clipboard.max_scroll() as f32;
                let target = (self.clipboard_scroll.target() - dy as f32).clamp(0.0, max);
                self.clipboard_scroll.set_target(target);
                self.dirty = true;
            }
            Mode::Stopwatch if self.hit_at_cursor() == Some(Hit::SwDigits) => {
                self.minutes_accum += dy;
                let steps = (self.minutes_accum / 40.0).trunc();
                if steps != 0.0 {
                    self.minutes_accum -= steps * 40.0;
                    if self.stopwatch.adjust(steps as i64 * 60) {
                        self.dirty = true;
                    }
                }
            }
            Mode::Controls if self.hit_at_cursor() == Some(Hit::CtlVolume) => {
                let level = (self.volume.level + (dy / 40.0) as f32 * 0.02).clamp(0.0, 1.0);
                self.volume.level = level;
                self.services.audio.set_level(level);
                self.dirty = true;
            }
            Mode::Timer if self.hit_at_cursor() == Some(Hit::TimerDigits) => {
                self.minutes_accum += dy;
                let steps = (self.minutes_accum / 40.0).trunc();
                if steps != 0.0 {
                    self.minutes_accum -= steps * 40.0;
                    if self.timer.adjust_minutes(steps as i32) {
                        self.save_pomodoro();
                        self.dirty = true;
                    }
                }
            }
            _ => {}
        }
    }

    fn scroll_shelf(&mut self, delta: f64) {
        let max = self.shelf.max_scroll() as f32;
        let target = (self.shelf_scroll.target() - delta as f32).clamp(0.0, max);
        self.shelf_scroll.set_target(target);
        self.dirty = true;
    }

    fn on_key(&mut self, event: &KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }
        if self.todo.editing.is_some() {
            self.on_text_key(event);
            return;
        }
        match &event.logical_key {
            Key::Named(NamedKey::ArrowLeft) => self.navigate(-1),
            Key::Named(NamedKey::ArrowRight) => self.navigate(1),
            Key::Named(NamedKey::Escape) => self.collapse(),
            _ => {}
        }
    }

    fn on_text_key(&mut self, event: &KeyEvent) {
        enum Action {
            Commit,
            Cancel,
            Paste,
            Edit,
        }
        let ctrl = self.modifiers.control_key();
        let action = {
            let Some(input) = self.todo.editing.as_mut() else { return };
            match &event.logical_key {
                Key::Named(NamedKey::Enter) => Action::Commit,
                Key::Named(NamedKey::Escape) => Action::Cancel,
                Key::Named(NamedKey::Backspace) => {
                    input.backspace();
                    Action::Edit
                }
                Key::Named(NamedKey::Delete) => {
                    input.delete();
                    Action::Edit
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    input.left();
                    Action::Edit
                }
                Key::Named(NamedKey::ArrowRight) => {
                    input.right();
                    Action::Edit
                }
                Key::Named(NamedKey::Home) => {
                    input.home();
                    Action::Edit
                }
                Key::Named(NamedKey::End) => {
                    input.end();
                    Action::Edit
                }
                Key::Character(c) if ctrl && c.eq_ignore_ascii_case("v") => Action::Paste,
                _ => {
                    if !ctrl && let Some(text) = &event.text {
                        input.insert(text);
                    }
                    Action::Edit
                }
            }
        };
        match action {
            Action::Commit => self.finish_editing(true),
            Action::Cancel => self.finish_editing(false),
            Action::Paste => {
                let text = self.native.as_ref().and_then(|n| win32::clipboard_text(n.hwnd()));
                if let (Some(text), Some(input)) = (text, self.todo.editing.as_mut()) {
                    input.insert(text.lines().next().unwrap_or_default());
                }
            }
            Action::Edit => {}
        }
        self.caret_epoch = Instant::now();
        self.dirty = true;
    }

    fn start_text_input(&mut self) {
        if let Some(native) = self.native.as_mut() {
            native.set_keyboard_focus(true);
        }
        if let Some(window) = &self.window {
            window.set_ime_allowed(true);
        }
        self.caret_epoch = Instant::now();
        self.update_hotkeys();
        self.dirty = true;
    }

    fn finish_editing(&mut self, commit: bool) {
        if self.todo.editing.is_none() {
            return;
        }
        let changed = if commit {
            self.todo.commit()
        } else {
            self.todo.cancel();
            false
        };
        if changed {
            self.save_todo();
        }
        if let Some(window) = &self.window {
            window.set_ime_allowed(false);
        }
        if let Some(native) = self.native.as_mut() {
            native.set_keyboard_focus(false);
        }
        self.update_hotkeys();
        if !self.hovered {
            self.collapse_at = Some(Instant::now() + COLLAPSE_DELAY);
        }
        self.dirty = true;
    }

    fn context_menu(&mut self, event_loop: &ActiveEventLoop) {
        let monitors = win32::monitors();
        let current = self.settings.monitor.clone();
        let on_monitor = |m: &win32::MonitorEntry| match &current {
            Some(device) => monitors.iter().any(|x| &x.device == device) && &m.device == device,
            None => m.primary,
        };
        let s = &self.settings;
        let state = MenuState {
            edge: s.edge,
            avoid_taskbar: s.avoid_taskbar,
            taskbar_on_edge: self.placement.taskbar_inset > 0,
            monitors: monitors.iter().map(|m| (m.clone(), on_monitor(m))).collect(),
            widgets: Page::ALL.iter().map(|p| (p.name(), !s.hidden_pages.contains(p))).collect(),
            accents: Accent::ALL.iter().map(|a| (a.name(), *a == s.accent)).collect(),
            album_colors: s.album_colors,
            low_battery: s.low_battery,
            low_battery_threshold: s.low_battery_threshold,
            disk_warning: s.disk_warning,
            disk_threshold: s.disk_threshold,
            lock_indicator: s.lock_indicator,
            media_indicator: s.media_indicator,
            privacy_indicator: s.privacy_indicator,
            notifications: s.notifications,
            clipboard: s.clipboard,
            hide_in_fullscreen: s.hide_in_fullscreen,
            weather_city: s.weather.as_ref().map(|l| l.name.clone()),
        };
        let Some(native) = self.native.as_mut() else { return };
        let command = native.show_context_menu(&state);
        let scale = self.placement.scale;
        let settings = &mut self.settings;
        match command {
            Some(MenuCommand::ToggleLowBattery) => {
                settings.low_battery = !settings.low_battery;
                self.low_battery_warned = false;
                self.save_settings();
                self.check_low_battery();
            }
            Some(MenuCommand::EditLowBatteryThreshold) => {
                let current = settings.low_battery_threshold as u32;
                if let Some(value) = tray::prompt_number(
                    "Low battery warning",
                    "Warn when the battery drops to this level (1-90%):",
                    current,
                    1,
                    90,
                    scale,
                ) {
                    self.settings.low_battery_threshold = value as u8;
                    self.settings.low_battery = true;
                    self.low_battery_warned = false;
                    self.save_settings();
                    self.check_low_battery();
                }
            }
            Some(MenuCommand::ToggleDiskWarning) => {
                settings.disk_warning = !settings.disk_warning;
                self.disk_warned = false;
                self.save_settings();
            }
            Some(MenuCommand::EditDiskThreshold) => {
                let current = settings.disk_threshold as u32;
                if let Some(value) = tray::prompt_number(
                    "Disk space warning",
                    "Warn when disk C: is this full (50-99% used):",
                    current,
                    50,
                    99,
                    scale,
                ) {
                    self.settings.disk_threshold = value as u8;
                    self.settings.disk_warning = true;
                    self.disk_warned = false;
                    self.save_settings();
                    self.check_disk(self.system.disk_free, self.system.disk_total);
                }
            }
            Some(MenuCommand::ToggleLockIndicator) => {
                settings.lock_indicator = !settings.lock_indicator;
                self.save_settings();
            }
            Some(MenuCommand::ToggleMediaIndicator) => {
                settings.media_indicator = !settings.media_indicator;
                self.save_settings();
            }
            Some(MenuCommand::TogglePrivacyIndicator) => {
                settings.privacy_indicator = !settings.privacy_indicator;
                self.save_settings();
            }
            Some(MenuCommand::ToggleNotifications) => {
                settings.notifications = !settings.notifications;
                self.services.notifications.set_enabled(settings.notifications);
                self.save_settings();
            }
            Some(MenuCommand::ToggleClipboard) => {
                settings.clipboard = !settings.clipboard;
                self.save_settings();
            }
            Some(MenuCommand::ToggleHideInFullscreen) => {
                settings.hide_in_fullscreen = !settings.hide_in_fullscreen;
                self.save_settings();
                self.update_fullscreen();
            }
            Some(MenuCommand::ToggleAlbumColors) => {
                settings.album_colors = !settings.album_colors;
                self.save_settings();
            }
            Some(MenuCommand::SetAccent(index)) => {
                if let Some(accent) = Accent::ALL.get(index) {
                    settings.accent = *accent;
                    self.save_settings();
                }
            }
            Some(MenuCommand::ToggleWidget(index)) => {
                if let Some(page) = Page::ALL.get(index).copied() {
                    if let Some(position) = settings.hidden_pages.iter().position(|p| *p == page) {
                        settings.hidden_pages.remove(position);
                    } else if settings.hidden_pages.len() + 1 < Page::ALL.len() {
                        // At least one widget always stays.
                        settings.hidden_pages.push(page);
                    }
                    self.save_settings();
                    self.refresh_pages();
                }
            }
            Some(MenuCommand::SetMonitor(index)) => {
                if let Some(monitor) = monitors.get(index) {
                    settings.monitor = (!monitor.primary).then(|| monitor.device.clone());
                    self.save_settings();
                    self.refresh_placement();
                }
            }
            Some(MenuCommand::EditWeatherCity) => self.edit_weather_city(),
            other => self.apply_menu_command(event_loop, other),
        }
        self.dirty = true;
    }

    fn edit_weather_city(&mut self) {
        let current = self.settings.weather.as_ref().map(|l| l.name.clone()).unwrap_or_default();
        if let Some(city) = tray::prompt_text("Weather", "City for the weather widget:", &current, self.placement.scale) {
            self.weather_error = Some("Looking up\u{2026}".into());
            self.services.weather.lookup(city);
        }
        self.dirty = true;
    }

    fn apply_menu_command(&mut self, event_loop: &ActiveEventLoop, command: Option<MenuCommand>) {
        match command {
            Some(MenuCommand::Quit) => event_loop.exit(),
            Some(MenuCommand::ToggleAutostart) => {
                let enable = !win32::autostart_enabled();
                if let Err(err) = win32::set_autostart(enable) {
                    log::error!("cannot change autostart: {err}");
                }
            }
            Some(MenuCommand::ToggleAvoidTaskbar) => {
                self.settings.avoid_taskbar = !self.settings.avoid_taskbar;
                self.save_settings();
                self.refresh_placement();
            }
            Some(MenuCommand::SetEdge(edge)) if edge != self.settings.edge => {
                self.settings.edge = edge;
                self.save_settings();
                self.refresh_placement();
            }
            _ => {}
        }
    }
    fn update_hotkeys(&mut self) {
        let enable = self.hovered && self.mode.is_expanded() && self.todo.editing.is_none();
        if let Some(native) = self.native.as_mut() {
            native.set_navigation_hotkeys(enable);
        }
    }

    fn on_drag(&mut self, event: DragEvent) {
        let now = Instant::now();
        let hovering = matches!(event, DragEvent::Enter);
        self.drop_tint.set_target(if hovering { ui::palette::ZONE_ACTIVE } else { ui::palette::ZONE_BORDER });
        self.drop_amount.set_target(if hovering { 1.0 } else { 0.0 });
        match event {
            DragEvent::Enter => {
                self.drop = DropState::Hovering;
                self.collapse_at = None;
                self.peek_until = None;
                self.popup = None;
                self.expanded = true;
                self.page = Page::Shelf;
                self.page_indicator.snap(Page::Shelf.index() as f32);
                self.update_mode();
            }
            DragEvent::Leave => {
                self.drop = DropState::None;
                self.update_mode();
                if !self.hovered {
                    self.collapse_at = Some(now + COLLAPSE_DELAY);
                }
            }
            DragEvent::Drop(files) => {
                self.drop = DropState::None;
                let mut added = false;
                for path in files {
                    if let Some(id) = self.shelf.add(path.clone()) {
                        self.services.shelf.inspect(id, path);
                        added = true;
                    }
                }
                if added {
                    let max = self.shelf.max_scroll() as f32;
                    self.shelf_scroll.set_target(max);
                    self.save_shelf();
                }
                self.session_page = Some(Page::Shelf);
                self.update_mode();
                if !self.hovered {
                    self.collapse_at = Some(now + DROP_LINGER);
                }
            }
        }
        self.dirty = true;
    }

    // -------------------------------------------------------------- services

    fn on_service(&mut self, event: ServiceEvent) {
        match event {
            ServiceEvent::Loaded(documents) => self.apply_documents(*documents),
            ServiceEvent::Power(status) => {
                let plugged_in = self.power_known && !self.power.ac_online && status.ac_online;
                self.power = status;
                self.power_known = true;
                if plugged_in && status.has_battery {
                    self.show_popup(Mode::Charging);
                }
                self.check_low_battery();
            }
            ServiceEvent::Volume { state, initial } => {
                self.volume = state;
                if !initial && self.settings.media_indicator {
                    self.show_indicator(Indicator::Volume { level: state.level, muted: state.muted });
                }
            }
            ServiceEvent::Brightness { level, initial } => {
                if !initial && self.settings.media_indicator {
                    self.show_indicator(Indicator::Brightness(level));
                }
            }
            ServiceEvent::LockKeys { state, initial } => {
                let previous = self.lock_keys.replace(state);
                if let Some(previous) = previous.filter(|_| !initial && self.settings.lock_indicator) {
                    if previous.caps != state.caps {
                        self.show_indicator(Indicator::CapsLock(state.caps));
                    } else if previous.num != state.num {
                        self.show_indicator(Indicator::NumLock(state.num));
                    }
                }
            }
            ServiceEvent::System(stats) => self.system = *stats,
            ServiceEvent::Disk { free, total } => {
                self.system.disk_free = free;
                self.system.disk_total = total;
                self.check_disk(free, total);
            }
            ServiceEvent::Weather(event) => match event {
                WeatherEvent::Updated(weather) => {
                    self.weather = Some(weather);
                    self.weather_error = None;
                }
                WeatherEvent::Failed(err) => {
                    if self.weather.is_none() {
                        self.weather_error = Some(short_error(&err));
                    }
                }
                WeatherEvent::Located(Ok(location)) => {
                    log::info!("weather location: {}", location.name);
                    self.weather = None;
                    self.weather_error = None;
                    self.services.weather.set_location(Some(location.clone()));
                    self.settings.weather = Some(location);
                    self.save_settings();
                }
                WeatherEvent::Located(Err(err)) => self.weather_error = Some(short_error(&err)),
            },
            ServiceEvent::Controls(state) => self.controls = state,
            ServiceEvent::Notification(notification) => {
                if self.settings.notifications && self.demo.is_none() {
                    self.show_notification(notification);
                }
            }
            ServiceEvent::NotificationsUnavailable(reason) => {
                log::warn!("notification popups unavailable: {reason}");
            }
            ServiceEvent::Privacy(state) => {
                if state != self.privacy {
                    log::info!("privacy: camera {} microphone {} screen {}", state.camera, state.microphone, state.screen);
                }
                self.privacy = state;
            }
            ServiceEvent::Media(snapshot) if self.demo.is_none() => self.apply_media(snapshot),
            ServiceEvent::Artwork(artwork) if self.demo.is_none() => {
                self.artwork = artwork;
                self.update_album_color();
            }
            ServiceEvent::Lyrics { key, result } => self.lyrics.apply(key, result),
            ServiceEvent::Bluetooth(event) => match event {
                BluetoothEvent::Connected(device) => {
                    self.upsert_headphones(device);
                    self.show_popup(Mode::Headphones);
                }
                BluetoothEvent::Updated(device) => self.upsert_headphones(device),
                BluetoothEvent::Disconnected(id) => self.headphones.retain(|d| d.id != id),
            },
            ServiceEvent::ShelfFile(info) => {
                if !self.shelf.apply(info) {
                    self.save_shelf();
                }
            }
            ServiceEvent::Media(_) | ServiceEvent::Artwork(_) => {}
        }
        self.dirty = true;
    }

    fn upsert_headphones(&mut self, device: AudioDevice) {
        self.headphones.retain(|d| d.id != device.id);
        self.headphones.push(device);
    }

    fn apply_media(&mut self, snapshot: Option<MediaSnapshot>) {
        let was_active = self.has_activity();
        let key = snapshot.as_ref().map(MediaSnapshot::track_key);
        if key != self.track_key {
            if snapshot.as_ref().is_some_and(|s| s.playing) && !self.expanded {
                // A new song starts: the next hover opens the player.
                self.session_page = None;
            }
            if self.artwork.as_ref().is_some_and(|a| Some(&a.key) != key.as_ref()) {
                self.artwork = None;
                self.update_album_color();
            }
            self.track_key = key;
        }
        self.media = snapshot;
        self.ensure_lyrics();
        if was_active != self.has_activity() {
            self.update_mode();
        }
    }

    fn apply_documents(&mut self, documents: LoadedDocuments) {
        if self.demo.is_some() {
            return;
        }
        let now = Instant::now();
        self.settings = documents.parse(Document::Settings);
        self.refresh_placement();
        if !self.timer.running() {
            self.timer = Pomodoro::new(documents.parse(Document::Pomodoro));
        }
        self.water = WaterTracker::new(documents.parse(Document::Water), now);
        self.shelf = Shelf::from_data(documents.parse(Document::Shelf));
        for item in &self.shelf.items {
            self.services.shelf.inspect(item.id, item.path.clone());
        }
        self.todo = TodoList::new(documents.parse(Document::Todo));
        self.clipboard = ClipboardHistory::new(documents.parse::<ClipboardData>(Document::Clipboard));
        self.refresh_pages();
        self.services.weather.set_location(self.settings.weather.clone());
        self.services.notifications.set_enabled(self.settings.notifications);
        self.loaded = true;
        self.update_fullscreen();
        self.check_low_battery();
        if self.todo.rollover(self.today) {
            self.save_todo();
        }
        log::info!(
            "state loaded: {} tasks, {} shelf items, {} water entries",
            self.todo.data.tasks.len(),
            self.shelf.items.len(),
            self.water.data.log.len()
        );
    }

    fn save_settings(&self) {
        if self.loaded {
            self.services.storage.save(Document::Settings, &self.settings);
        }
    }

    fn save_todo(&self) {
        if self.loaded {
            self.services.storage.save(Document::Todo, &self.todo.data);
        }
    }

    fn save_water(&self) {
        if self.loaded {
            self.services.storage.save(Document::Water, &self.water.data);
        }
    }

    fn save_pomodoro(&self) {
        if self.loaded {
            self.services.storage.save(Document::Pomodoro, &self.timer.data);
        }
    }

    fn save_clipboard(&self) {
        if self.loaded {
            self.services.storage.save(Document::Clipboard, &self.clipboard.data);
        }
    }

    fn save_shelf(&self) {
        if self.loaded {
            self.services.storage.save(Document::Shelf, &self.shelf.to_data());
        }
    }

    /// Synchronous save of everything (exit, session end).
    fn flush_state(&self) {
        if !self.loaded {
            return;
        }
        let storage = &self.services.storage;
        storage.save_now(Document::Settings, &self.settings);
        storage.save_now(Document::Todo, &self.todo.data);
        storage.save_now(Document::Water, &self.water.data);
        storage.save_now(Document::Pomodoro, &self.timer.data);
        storage.save_now(Document::Shelf, &self.shelf.to_data());
        storage.save_now(Document::Clipboard, &self.clipboard.data);
    }

    /// Re-reads monitor, DPI and taskbar geometry (display changes, taskbar
    /// moved, setting toggled) and moves the canvas when needed.
    fn refresh_placement(&mut self) {
        let edge = self.settings.edge;
        if self.edge != edge {
            self.edge = edge;
            self.notch.set_edge(edge);
            self.transition = None;
        }
        let placement = win32::canvas_placement(edge, self.settings.avoid_taskbar, self.settings.monitor.as_deref());
        if placement == self.placement {
            self.sync_input_geometry();
            self.dirty = true;
            return;
        }
        log::info!("canvas placement ({edge:?}): {placement:?}");
        self.placement = placement;
        if let Some(native) = &self.native {
            native.place(&placement);
        }
        if let Some(renderer) = &mut self.renderer {
            renderer.resize(placement.width, placement.height);
        }
        self.sync_input_geometry();
        self.dirty = true;
    }

    // -------------------------------------------------------------- geometry

    /// Publishes the notch bounds to `WM_NCHITTEST` and shrinks the window
    /// region to them so the rest of the canvas is click-through.
    fn sync_input_geometry(&mut self) {
        let Some(native) = self.native.as_mut() else { return };
        let scale = self.placement.scale;
        let body = self.notch.body();
        native.set_hit_rect(to_physical(body, scale));
        let bounds = body.union(self.notch.target());
        let (x0, x1) = (bounds.x0 - SHOULDER_RADIUS - REGION_MARGIN, bounds.x1 + SHOULDER_RADIUS + REGION_MARGIN);
        // The region always reaches the attached edge (the shoulders live
        // there) and extends a small margin past the free edge.
        let region = match self.edge {
            Edge::Top => Rect::new(x0, 0.0, x1, bounds.y1 + REGION_MARGIN),
            Edge::Bottom => Rect::new(x0, bounds.y0 - REGION_MARGIN, x1, CANVAS_HEIGHT),
        };
        native.set_input_region(quantize(to_physical(region, scale), 4));
    }

    // ------------------------------------------------------------ scheduling

    fn clock_visible(&self) -> bool {
        self.mode != Mode::Resting && !self.mode.is_popup()
    }

    fn ambient_animation(&self) -> bool {
        self.audio_playing() && self.mode != Mode::Resting && !self.mode.is_popup()
    }

    fn caret_visible(&self, now: Instant) -> bool {
        (now.duration_since(self.caret_epoch).as_millis() / CARET_BLINK.as_millis()).is_multiple_of(2)
    }

    /// Next moment the picture changes without any input.
    fn visual_deadline(&self, now: Instant) -> Option<Instant> {
        let mut deadline: Option<Instant> = None;
        let mut consider = |t: Option<Instant>| {
            if let Some(t) = t {
                deadline = Some(deadline.map_or(t, |d| d.min(t)));
            }
        };
        if self.timer.running() && matches!(self.mode, Mode::Timer | Mode::Closed) {
            consider(self.timer.next_tick(now));
        }
        if self.mode == Mode::Lyrics {
            consider(self.media.as_ref().and_then(|media| self.lyrics.next_change(media, now)));
        }
        if self.mode == Mode::Stopwatch && self.stopwatch.running() {
            // Hundredths on the stopwatch, seconds on the countdown.
            let step = if self.stopwatch.stopwatch_running() { Duration::from_millis(40) } else { Duration::from_millis(250) };
            consider(Some(now + step));
        }
        if let Some((_, until)) = self.clipboard.copied
            && until > now
        {
            consider(Some(until));
        }
        if self.mode == Mode::Todo && self.todo.editing.is_some() {
            let elapsed = now.duration_since(self.caret_epoch);
            let blinks = elapsed.as_millis() / CARET_BLINK.as_millis() + 1;
            consider(Some(self.caret_epoch + CARET_BLINK * blinks as u32));
        }
        deadline
    }

    fn tick(&mut self, now: Instant) {
        self.advance_demo(now);
        let local = Local::now();
        let today = local.date_naive();
        if today != self.today {
            self.today = today;
            if self.todo.rollover(today) {
                self.save_todo();
            }
            self.water.prune(today);
            self.save_water();
            self.dirty = true;
        }
        if self.collapse_at.is_some_and(|at| now >= at) {
            self.collapse_at = None;
            if !self.hovered && self.drop == DropState::None && self.todo.editing.is_none() {
                if let Some(popup) = &mut self.popup {
                    popup.paused = false;
                    popup.until = popup.until.max(now + POPUP_LINGER);
                }
                self.collapse();
            }
        }
        if self.peek_until.is_some_and(|at| now >= at) {
            self.peek_until = None;
            if !self.hovered {
                self.collapse();
            }
        }
        if self.popup.as_ref().is_some_and(|p| !p.paused && now >= p.until) {
            self.popup = None;
            self.update_mode();
        }
        if let Some(completed) = self.timer.update(now, today) {
            log::info!("{completed:?} finished");
            sound::play_chime();
            self.save_pomodoro();
            self.peek(Page::Timer);
            self.update_mode();
            self.dirty = true;
        }
        if self.stopwatch.update(now) {
            log::info!("countdown finished");
            sound::play_chime();
            self.peek(Page::Stopwatch);
            self.dirty = true;
        }
        if self.loaded
            && let Some(due) = self.water.next_reminder(now, local)
            && now >= due
        {
            self.water.mark_reminded(now);
            if !self.expanded && self.popup.is_none() {
                sound::play_chime();
                self.peek(Page::Water);
            }
        }
        if self.ambient_animation() {
            if self.next_ambient.is_none_or(|t| now >= t) {
                self.dirty = true;
                let interval = if self.mode == Mode::Lyrics { LYRICS_FRAME } else { AMBIENT_FRAME };
                self.next_ambient = Some(now + interval);
            }
        } else {
            self.next_ambient = None;
        }
        if now >= self.next_clock_tick {
            self.next_clock_tick = next_minute(now);
            if self.clock_visible() {
                self.dirty = true;
            }
        }
        if self.mode == Mode::Resting && now >= self.eyes.next_deadline() {
            self.dirty = true;
        }
        if self.fullscreen_since.is_some_and(|t| now >= t + FULLSCREEN_SETTLE) {
            self.update_fullscreen();
        }
        if self.next_visual.is_some_and(|t| now >= t) {
            self.next_visual = None;
            self.dirty = true;
        }
        if self.trim_pending && !self.animating && now >= self.last_activity + IDLE_TRIM_AFTER {
            self.trim_pending = false;
            self.last_trim = Some(now);
            let (_, working_set) = renderer::memory_usage();
            if working_set > TRIM_THRESHOLD_MB {
                renderer::trim_working_set();
                let (_, after) = renderer::memory_usage();
                log::debug!("working set trimmed: {working_set:.1} MB -> {after:.1} MB");
            }
        }
    }

    fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let mut deadline: Option<Instant> = None;
        let mut consider = |t: Option<Instant>| {
            if let Some(t) = t {
                deadline = Some(deadline.map_or(t, |d| d.min(t)));
            }
        };
        consider(self.collapse_at);
        consider(self.peek_until);
        consider(self.popup.as_ref().filter(|p| !p.paused).map(|p| p.until));
        if self.mode == Mode::Resting {
            consider(Some(self.eyes.next_deadline()));
        }
        if self.clock_visible() {
            consider(Some(self.next_clock_tick));
        }
        consider(self.next_ambient);
        consider(self.next_visual);
        consider(self.fullscreen_since.map(|t| t + FULLSCREEN_SETTLE));
        consider(self.timer.end());
        consider(self.stopwatch.countdown_end());
        if self.loaded {
            consider(self.water.next_reminder(now, Local::now()));
        }
        consider(Some(next_midnight(now)));
        if self.trim_pending {
            consider(Some(self.last_activity + IDLE_TRIM_AFTER));
        }
        consider(self.demo.as_ref().filter(|d| d.pinned.is_none()).map(|d| d.next_at));
        deadline
    }

    fn redraw(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else { return };
        if !renderer.is_ready() {
            return;
        }
        let now = Instant::now();
        let dt = match self.last_frame {
            Some(previous) if self.animating => (now - previous).as_secs_f32(),
            _ => 1.0 / 120.0,
        };
        self.last_frame = Some(now);
        self.dirty = false;

        let mut animating = self.notch.step(dt);
        if let Some(transition) = &mut self.transition {
            animating |= transition.progress.step(dt);
            if transition.progress.is_at_rest() {
                self.transition = None;
            }
        }
        animating |= self.feedback.step(dt);
        animating |= self.page_indicator.step(dt);
        animating |= self.shelf_scroll.step(dt);
        animating |= self.todo_scroll.step(dt);
        animating |= self.drop_tint.step(dt);
        animating |= self.drop_amount.step(dt);
        animating |= self.clipboard_scroll.step(dt);
        animating |= self.album_color.step(dt);
        if self.mode == Mode::Lyrics {
            let target = self.lyrics.target(self.media.as_ref(), now);
            if (self.lyrics.scroll.target() - target).abs() > f32::EPSILON {
                self.lyrics.scroll.set_target(target);
            }
            animating |= self.lyrics.scroll.step(dt);
        }
        let resting_visible =
            self.mode == Mode::Resting || self.transition.as_ref().is_some_and(|t| t.from == Mode::Resting);
        if resting_visible {
            animating |= self.eyes.update(now, dt, &mut self.rng);
        }

        let artwork = self
            .artwork
            .as_ref()
            .filter(|art| self.track_key.as_ref() == Some(&art.key) || self.demo.is_some())
            .map(|art| &art.image);
        let model = Model {
            now,
            edge: self.edge,
            clock: Local::now(),
            power: self.power,
            media: self.media.as_ref(),
            artwork,
            scrub: self.scrub,
            lyrics: &self.lyrics,
            timer: &self.timer,
            water: &self.water,
            shelf: &self.shelf,
            todo: &self.todo,
            headphones: self.headphones.last(),
            indicator: self.indicator,
            system: &self.system,
            calendar_offset: self.calendar_offset,
            pages: &self.pages,
            clipboard: &self.clipboard,
            clipboard_enabled: self.settings.clipboard,
            clipboard_scroll: self.clipboard_scroll.value() as f64,
            stopwatch: &self.stopwatch,
            weather: self.weather.as_ref(),
            weather_city: self.settings.weather.as_ref().map(|l| l.name.as_str()),
            weather_error: self.weather_error.as_deref(),
            controls: self.controls,
            volume: self.volume,
            volume_preview: self.volume_preview,
            notification: self.notification.as_ref(),
            accent: self.settings.accent.colors().0,
            accent2: self.settings.accent.colors().1,
            album: (self.settings.album_colors && self.album_known && artwork.is_some()).then(|| self.album_color.color()),
            drop: self.drop,
            drop_tint: self.drop_tint.color(),
            drop_amount: self.drop_amount.value(),
            shelf_scroll: self.shelf_scroll.value() as f64,
            todo_scroll: self.todo_scroll.value() as f64,
            audio_playing: self.demo_audio || self.media.as_ref().is_some_and(|m| m.playing),
            page: self.page,
            page_indicator: self.page_indicator.value(),
            eyes: self.eyes.pose(now),
            caret_visible: self.caret_visible(now),
        };

        self.scene.reset();
        self.text.begin_frame();
        let mut hits = Vec::new();
        let scale = self.placement.scale;
        let cx = CANVAS_WIDTH / 2.0;
        let time = (now - self.started).as_secs_f64();
        let outline = self.notch.path();
        let base = Affine::scale(scale);
        self.scene.fill(Fill::NonZero, base, Color::BLACK, None, &outline);
        self.scene.push_clip_layer(Fill::NonZero, base, &outline);

        let frame = FrameParams { model: &model, scale, center_x: cx, time, edge: self.edge };
        let (incoming_offset, incoming_scale, incoming_alpha) = match &self.transition {
            Some(transition) => {
                let p = transition.progress.value().clamp(0.0, 1.0) as f64;
                let alpha = ((p - 0.15) / 0.85).clamp(0.0, 1.0) as f32;
                let outgoing_alpha = (1.0 - p * 1.8).max(0.0) as f32;
                if outgoing_alpha > 0.0 {
                    let (offset, zoom) = if transition.direction != 0.0 {
                        (Vec2::new(-transition.direction * 26.0 * p, 0.0), 1.0)
                    } else {
                        (Vec2::ZERO, 1.0 - 0.05 * p)
                    };
                    draw_mode(
                        &mut self.scene,
                        &mut self.text,
                        &self.feedback,
                        None,
                        transition.from,
                        &frame,
                        offset,
                        zoom,
                        outgoing_alpha,
                    );
                }
                if transition.direction != 0.0 {
                    (Vec2::new(transition.direction * 26.0 * (1.0 - p), 0.0), 1.0, alpha)
                } else {
                    (Vec2::ZERO, 0.94 + 0.06 * p, alpha)
                }
            }
            None => (Vec2::ZERO, 1.0, 1.0),
        };
        draw_mode(
            &mut self.scene,
            &mut self.text,
            &self.feedback,
            Some(&mut hits),
            self.mode,
            &frame,
            incoming_offset,
            incoming_scale,
            incoming_alpha,
        );
        if self.settings.privacy_indicator {
            draw_privacy_dots(&mut self.scene, base, self.notch.body(), self.edge, self.privacy);
        }
        self.scene.pop_layer();
        self.hits = hits;

        if let Some(cursor) = self.cursor {
            let hit = ui::hit_at(&self.hits, cursor);
            if self.feedback.set_hovered(hit) {
                animating = true;
            }
            self.update_cursor_icon(hit);
        }
        self.sync_input_geometry();

        let Some(renderer) = self.renderer.as_mut() else { return };
        if renderer.render(&self.scene) == FrameResult::Skipped {
            self.dirty = true;
        }
        self.animating = animating;
        // Ambient frames (sound wave, lyrics clock) do not count as activity:
        // they reuse the same memory every frame.
        if animating || self.frames_since_idle < 2 {
            self.last_activity = now;
        }
        if self.last_trim.is_none_or(|t| now.duration_since(t) >= TRIM_MIN_INTERVAL) {
            self.trim_pending = true;
        }
        self.frames_since_idle = if animating { 0 } else { self.frames_since_idle.saturating_add(1) };
        self.next_visual = self.visual_deadline(now);
        if animating && let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    // ------------------------------------------------------------------ demo

    fn advance_demo(&mut self, now: Instant) {
        let Some(demo) = &mut self.demo else { return };
        if demo.pinned.is_some() || now < demo.next_at {
            return;
        }
        demo.index = (demo.index + 1) % DEMO_SEQUENCE.len();
        demo.next_at = now + DEMO_STEP;
        let mode = DEMO_SEQUENCE[demo.index];
        self.demo_audio = mode != Mode::Resting;
        self.expanded = mode.is_expanded();
        self.popup = mode.is_popup().then(|| Popup { mode, until: now + DEMO_STEP * 4, paused: true });
        if let Some(page) = mode.page() {
            self.settings.lyrics = mode == Mode::Lyrics;
            self.page = page;
            self.page_indicator.set_target(page.index_in(&self.pages) as f32);
        }
        self.update_mode();
    }

    /// Sample content for `--demo` (never persisted).
    fn load_demo_data(&mut self, now: Instant) {
        let local = Local::now();
        let today = local.date_naive();
        self.demo_audio = true;
        let media = MediaSnapshot {
            app_id: "Demo.Player".into(),
            title: "Paper Sky".into(),
            artist: "Lumen Coast".into(),
            album: "Summer Flows".into(),
            playing: true,
            position: Duration::from_secs(193),
            duration: Duration::from_secs(256),
            sampled_at: now,
            rate: 1.0,
            can_play_pause: true,
            can_next: true,
            can_previous: true,
            can_seek: true,
        };
        let key = media.track_key();
        let lines = [
            "Streetlights hum a quiet tune",
            "Paper planes across the moon",
            "Counting stars along the shore",
            "Till the tide comes in once more",
            "Hold the morning, let it stay",
            "Fold the night and fly away",
            "",
            "Paper planes across the moon",
        ];
        let lyric_lines: Vec<LyricLine> = lines
            .iter()
            .enumerate()
            .map(|(i, text)| LyricLine { time: Duration::from_secs(180 + i as u64 * 6), text: text.to_string() })
            .collect();
        self.lyrics.begin(&key, now);
        self.lyrics.apply(key.clone(), LyricsResult::Synced(Arc::new(lyric_lines)));
        self.artwork = crate::services::media::decode_artwork(&demo_artwork_png())
            .map(|image| Arc::new(Artwork { key: key.clone(), image }));
        self.track_key = Some(key);
        self.media = Some(media);

        let mut todo = TodoData::default();
        for (id, (text, done)) in
            [("Revise the report", true), ("Reply to Dina", true), ("Post weekly update", true), ("Book flights", true), ("Water the plants", false)]
                .iter()
                .enumerate()
        {
            todo.tasks.push(crate::ui::todo::Task {
                id: id as u64 + 1,
                text: text.to_string(),
                done: *done,
                completed_on: done.then_some(today),
            });
        }
        self.todo = TodoList::new(todo);
        let mut water = WaterData::default();
        for (hours_ago, ml) in [(7i64, 200), (6, 150), (5, 150), (4, 100), (3, 100), (2, 150), (1, 300), (0, 350)] {
            let at = local - chrono::Duration::hours(hours_ago);
            if at.date_naive() == today {
                water.log.push(WaterEntry { at, ml });
            }
        }
        self.water = WaterTracker::new(water, now);
        self.headphones.push(AudioDevice { id: "demo".into(), name: "AirBuds Pro".into(), battery: Some(70) });
        let gb = 1024 * 1024 * 1024;
        self.system = SystemStats {
            cpu: 0.23,
            memory: 0.62,
            memory_used: 10 * gb,
            memory_total: 16 * gb,
            disk: 0.81,
            disk_free: 91 * gb,
            disk_total: 476 * gb,
            download: 2.4 * 1024.0 * 1024.0,
            upload: 310.0 * 1024.0,
            cpu_history: (0..48).map(|i| (0.2 + 0.15 * ((i as f32) * 0.45).sin() + 0.1 * ((i as f32) * 1.3).cos()).max(0.02)).collect(),
            ready: true,
        };
        self.update_album_color();
        self.indicator = Some(Indicator::Volume { level: 0.64, muted: false });
        self.volume = VolumeState { level: 0.64, muted: false };
        self.privacy = PrivacyState { camera: true, microphone: true, screen: false };
        self.controls = ControlsState {
            wifi: Some(true),
            bluetooth: Some(false),
            power: Some(crate::services::controls::PowerMode::Balanced),
        };
        self.notification = Some(Notification {
            app: "Messages".into(),
            title: "Dina".into(),
            body: "Dinner at 7? I booked the table by the window.".into(),
            icon: None,
        });
        self.weather = Some(Weather {
            location: "Jakarta, Indonesia".into(),
            temperature: 31.4,
            feels_like: 35.0,
            code: 2,
            is_day: true,
            humidity: 66.0,
            wind: 12.0,
            high: 33.0,
            low: 25.0,
            hours: [(15u32, 32.0f32, 2u8), (16, 31.0, 3), (17, 30.0, 80), (18, 28.0, 95), (19, 27.0, 61)]
                .into_iter()
                .map(|(hour, temperature, code)| crate::services::weather::HourForecast { hour, temperature, code, is_day: hour < 18 })
                .collect(),
        });
        let mut clipboard = ClipboardHistory::default();
        for text in ["git push origin main", "https://open-meteo.com/en/docs", "Meeting moved to 3 PM", "SELECT * FROM users WHERE active = 1;"] {
            clipboard.record(text, local - chrono::Duration::minutes(7));
        }
        if let Some(id) = clipboard.data.entries.last().map(|e| e.id) {
            clipboard.toggle_pin(id);
        }
        self.clipboard = clipboard;
        self.settings.clipboard = true;
        self.stopwatch.toggle(now - Duration::from_millis(83_450));
        for lap in [21_300u64, 43_900, 64_100] {
            self.stopwatch.laps.push(Duration::from_millis(lap));
        }
    }
}

impl App {
    /// Warns once per crossing when C: is fuller than the threshold.
    fn check_disk(&mut self, free: u64, total: u64) {
        if total == 0 {
            return;
        }
        let used = 100.0 * (1.0 - free as f64 / total as f64);
        let threshold = self.settings.disk_threshold.clamp(50, 99) as f64;
        if used < threshold {
            self.disk_warned = false;
            return;
        }
        if self.settings.disk_warning && self.loaded && !self.disk_warned {
            self.disk_warned = true;
            let free_gb = free as f64 / (1024.0 * 1024.0 * 1024.0);
            log::info!("disk C: {used:.0}% used ({free_gb:.1} GB free)");
            self.show_indicator(Indicator::DiskFull { percent: used.round() as u8, free_gb: free_gb as f32 });
        }
    }

    fn show_notification(&mut self, notification: Notification) {
        let hidden = self.native.as_ref().is_some_and(NotchWindow::is_hidden);
        if self.expanded || self.drop != DropState::None || hidden {
            return;
        }
        self.notification = Some(notification);
        self.popup = Some(Popup { mode: Mode::Notification, until: Instant::now() + Duration::from_secs(5), paused: self.hovered });
        self.update_mode();
        self.dirty = true;
    }

    /// Records the new clipboard text (unless private or written by us).
    fn on_clipboard_changed(&mut self) {
        if !self.settings.clipboard || !self.loaded || self.demo.is_some() {
            return;
        }
        let Some(native) = &self.native else { return };
        if win32::clipboard_is_private() {
            log::debug!("clipboard content marked private; not recorded");
            return;
        }
        let Some(text) = win32::clipboard_text(native.hwnd()) else { return };
        if self.clipboard_own.take().is_some_and(|own| own == text) {
            return;
        }
        if self.clipboard.record(&text, Local::now()) {
            self.save_clipboard();
            self.dirty = true;
        }
    }

    fn copy_clip(&mut self, id: u64) {
        let (Some(entry), Some(native)) = (self.clipboard.get(id), &self.native) else { return };
        let text = entry.text.clone();
        if win32::set_clipboard_text(native.hwnd(), &text) {
            self.clipboard_own = Some(text.clone());
            self.clipboard.record(&text, Local::now());
            self.clipboard.copied = Some((id, Instant::now() + Duration::from_millis(1200)));
            self.save_clipboard();
        }
        self.dirty = true;
    }

    /// Glow color for the current album art (or none).
    fn update_album_color(&mut self) {
        let color = self.artwork.as_ref().and_then(|art| ui::common::dominant_color(&art.image));
        self.album_known = color.is_some();
        if let Some(color) = color {
            self.album_color.set_target(color);
        }
        self.dirty = true;
    }
}

/// First part of an error message, short enough for the widget.
fn short_error(err: &str) -> String {
    let text = err.split(" for url").next().unwrap_or(err);
    if text.chars().count() > 60 { format!("{}\u{2026}", text.chars().take(60).collect::<String>()) } else { text.to_string() }
}

/// Small generated PNG used as album art in demo mode.
fn demo_artwork_png() -> Vec<u8> {
    let size = 96u32;
    let mut img = image::RgbaImage::new(size, size);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let stripe = if (y / 6) % 2 == 0 { 232 } else { 214 };
        let (dx, dy) = (x as f32 - 40.0, y as f32 - 50.0);
        let disc = (dx * dx + dy * dy).sqrt();
        *px = if disc < 22.0 {
            image::Rgba([46, 118, 230, 255])
        } else if disc < 26.0 {
            image::Rgba([90, 150, 240, 255])
        } else {
            image::Rgba([stripe - 30, stripe - 6, 250, 255])
        };
    }
    let mut bytes = Vec::new();
    let _ = image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png);
    bytes
}

struct FrameParams<'a> {
    model: &'a Model<'a>,
    scale: f64,
    center_x: f64,
    time: f64,
    edge: Edge,
}

#[allow(clippy::too_many_arguments)]
fn draw_mode(
    scene: &mut Scene,
    text: &mut TextEngine,
    feedback: &Feedback,
    hits: Option<&mut Vec<HitRegion>>,
    mode: Mode,
    frame: &FrameParams,
    offset: Vec2,
    zoom: f64,
    alpha: f32,
) {
    // Content is laid out top-down in the mode's box and anchored (and
    // zoomed) at the edge the notch is attached to.
    let size = mode.size();
    let local = match frame.edge {
        Edge::Top => {
            Affine::translate((frame.center_x + offset.x, offset.y))
                * Affine::scale(zoom)
                * Affine::translate((-size.width / 2.0, 0.0))
        }
        Edge::Bottom => {
            Affine::translate((frame.center_x + offset.x, CANVAS_HEIGHT + offset.y))
                * Affine::scale(zoom)
                * Affine::translate((-size.width / 2.0, -size.height))
        }
    };
    let mut painter = Painter { scene, text, feedback, hits, dpi: frame.scale, local, alpha, time: frame.time, hit_clips: Vec::new() };
    ui::draw_view(mode, &mut painter, frame.model);
}

/// Green dot while the camera is in use, orange while the microphone is,
/// red while the screen is recorded or shared,
/// centered in the strip along the attached edge (above every view's
/// content).
fn draw_privacy_dots(scene: &mut Scene, base: Affine, body: Rect, edge: Edge, privacy: PrivacyState) {
    const CAMERA: Color = Color::from_rgb8(0x30, 0xD1, 0x58);
    const MICROPHONE: Color = Color::from_rgb8(0xFF, 0x9F, 0x0A);
    const SCREEN: Color = Color::from_rgb8(0xFF, 0x37, 0x5F);
    let dots: Vec<Color> = [(privacy.camera, CAMERA), (privacy.microphone, MICROPHONE), (privacy.screen, SCREEN)]
        .into_iter()
        .filter_map(|(on, color)| on.then_some(color))
        .collect();
    if dots.is_empty() {
        return;
    }
    let y = match edge {
        Edge::Top => body.y0 + 6.0,
        Edge::Bottom => body.y1 - 6.0,
    };
    let gap = 9.0;
    let x0 = body.center().x - gap * (dots.len() - 1) as f64 / 2.0;
    for (i, color) in dots.into_iter().enumerate() {
        scene.fill(Fill::NonZero, base, color, None, &Circle::new(Point::new(x0 + gap * i as f64, y), 3.0));
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        if let Err(err) = self.create_window(event_loop) {
            log::error!("{err}");
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::CursorMoved { position, .. } => self.on_cursor_moved(position),
            WindowEvent::CursorLeft { .. } => self.on_cursor_left(),
            WindowEvent::MouseInput { state, button, .. } => self.on_mouse_input(event_loop, state, button),
            WindowEvent::MouseWheel { delta, .. } => self.on_wheel(delta),
            WindowEvent::KeyboardInput { event, .. } => self.on_key(&event),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::Ime(Ime::Commit(text)) => {
                if let Some(input) = self.todo.editing.as_mut() {
                    input.insert(&text);
                    self.caret_epoch = Instant::now();
                    self.dirty = true;
                }
            }
            WindowEvent::Focused(false) => {
                if self.todo.editing.is_some() {
                    self.finish_editing(true);
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
                self.dirty = true;
            }
            WindowEvent::ScaleFactorChanged { .. } => self.refresh_placement(),
            WindowEvent::CloseRequested => event_loop.exit(),
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Services => {
                for event in self.inbox.drain() {
                    self.on_service(event);
                }
            }
            UserEvent::Gpu(Ok(context)) => {
                if let Some(renderer) = &mut self.renderer {
                    match renderer.attach(*context) {
                        Ok(()) => {
                            if let Some(native) = &self.native {
                                native.composition().commit();
                            }
                            // A popup raised during start-up (low battery)
                            // gets its full time on screen once it is drawn.
                            if let Some(popup) = &mut self.popup {
                                let duration = match (popup.mode, self.indicator) {
                                    (Mode::Indicator, Some(indicator)) => indicator.duration(),
                                    _ => POPUP_DURATION,
                                };
                                popup.until = Instant::now() + duration;
                            }
                            self.dirty = true;
                        }
                        Err(err) => {
                            log::error!("renderer setup failed: {err}");
                            event_loop.exit();
                        }
                    }
                }
            }
            UserEvent::Gpu(Err(err)) => {
                log::error!("GPU initialization failed: {err}");
                event_loop.exit();
            }
            UserEvent::FontsReady => self.dirty = true,
            UserEvent::Hotkey(id) => match id {
                HOTKEY_PREV => self.navigate(-1),
                HOTKEY_NEXT => self.navigate(1),
                _ => {}
            },
            UserEvent::DisplayChanged => self.refresh_placement(),
            UserEvent::SessionEnding => self.flush_state(),
            UserEvent::Drag(event) => self.on_drag(event),
            UserEvent::ForegroundChanged => self.update_fullscreen(),
            UserEvent::TrayMenu => self.context_menu(event_loop),
            UserEvent::ClipboardChanged => self.on_clipboard_changed(),
            UserEvent::TrayRecreate => {
                if let Some(tray) = &mut self.tray {
                    tray.add();
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        self.tick(now);
        // Nothing is drawn while hidden for a fullscreen app; the next frame
        // after showing again catches up.
        let hidden = self.native.as_ref().is_some_and(NotchWindow::is_hidden);
        if !hidden
            && (self.dirty || self.animating)
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
        event_loop.set_control_flow(match self.next_deadline(now) {
            Some(deadline) => ControlFlow::WaitUntil(deadline),
            None => ControlFlow::Wait,
        });
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.flush_state();
        log::info!("exiting");
    }
}

fn hwnd_of(window: &Window) -> Result<HWND, String> {
    match window.window_handle().map_err(|err| err.to_string())?.as_raw() {
        RawWindowHandle::Win32(handle) => Ok(HWND(handle.hwnd.get() as *mut c_void)),
        _ => Err("not a Win32 window".into()),
    }
}

fn to_physical(rect: Rect, scale: f64) -> RECT {
    RECT {
        left: (rect.x0 * scale).floor() as i32,
        top: (rect.y0 * scale).floor() as i32,
        right: (rect.x1 * scale).ceil() as i32,
        bottom: (rect.y1 * scale).ceil() as i32,
    }
}

/// Rounds a rectangle outward to multiples of `step` to avoid re-applying
/// the window region for sub-pixel changes.
fn quantize(rect: RECT, step: i32) -> RECT {
    RECT {
        left: rect.left.div_euclid(step) * step,
        top: rect.top.div_euclid(step) * step,
        right: (rect.right + step - 1).div_euclid(step) * step,
        bottom: (rect.bottom + step - 1).div_euclid(step) * step,
    }
}

fn next_minute(now: Instant) -> Instant {
    let local = Local::now();
    let into_minute = local.second() as f64 + local.nanosecond() as f64 / 1e9;
    now + Duration::from_secs_f64((60.0 - into_minute).max(0.05) + 0.02)
}

fn next_midnight(now: Instant) -> Instant {
    let local = Local::now();
    let seconds = local.num_seconds_from_midnight() as u64;
    now + Duration::from_secs(86_400 - seconds.min(86_399)) + Duration::from_millis(200)
}
