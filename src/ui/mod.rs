//! Notch UI: modes and their dimensions, the drawing context shared by all
//! views, hit regions and hover feedback.

pub mod calendar;
pub mod charging;
pub mod clipboard;
pub mod closed;
pub mod common;
pub mod controls;
pub mod headphones;
pub mod indicator;
pub mod lyrics;
pub mod music;
pub mod notification;
pub mod resting;
pub mod shelf;
pub mod stopwatch;
pub mod system;
pub mod timer;
pub mod todo;
pub mod water;
pub mod weather;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use vello::Scene;
use vello::kurbo::{Affine, Cap, Join, Point, Rect, Shape, Size, Stroke};
use vello::peniko::{Color, Fill, Gradient, ImageBrush, ImageData};

use crate::animation::{RectSpring, SpringConfig, SpringValue};
use crate::renderer::icons::{self, Icon, Mark};
use crate::renderer::shapes::{SHOULDER_RADIUS, notch_path};
use crate::renderer::text::{TextEngine, TextLayout, TextStyle};
use crate::services::bluetooth::AudioDevice;
use crate::services::media::MediaSnapshot;
use crate::services::power::PowerStatus;
use crate::window::{CANVAS_HEIGHT, CANVAS_WIDTH, Edge};

/// The notch states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Resting,
    Closed,
    Music,
    Lyrics,
    Timer,
    Water,
    Shelf,
    Stopwatch,
    Weather,
    Clipboard,
    Todo,
    Controls,
    System,
    Calendar,
    Headphones,
    Charging,
    /// Short popup: volume, brightness, lock keys, low battery.
    Indicator,
    /// Notification from another app.
    Notification,
}

impl Mode {
    /// Notch dimensions in logical pixels.
    pub fn size(self) -> Size {
        match self {
            Mode::Resting => Size::new(160.0, 36.0),
            Mode::Closed => Size::new(190.0, 36.0),
            Mode::Music
            | Mode::Timer
            | Mode::Stopwatch
            | Mode::Water
            | Mode::Weather
            | Mode::Shelf
            | Mode::Clipboard
            | Mode::Todo
            | Mode::Controls
            | Mode::System
            | Mode::Calendar => {
                Size::new(420.0, 175.0)
            }
            Mode::Lyrics => Size::new(540.0, 175.0),
            Mode::Headphones | Mode::Charging => Size::new(310.0, 76.0),
            Mode::Indicator => Size::new(300.0, 52.0),
            Mode::Notification => Size::new(380.0, 76.0),
        }
    }

    /// Transient popups that are not part of the widget carousel.
    pub fn is_popup(self) -> bool {
        matches!(self, Mode::Headphones | Mode::Charging | Mode::Indicator | Mode::Notification)
    }

    /// Bottom corner radius (continuous corners).
    pub fn corner_radius(self) -> f64 {
        match self {
            Mode::Resting | Mode::Closed => 18.0,
            Mode::Headphones | Mode::Charging | Mode::Notification => 26.0,
            Mode::Indicator => 22.0,
            _ => 32.0,
        }
    }

    pub fn is_expanded(self) -> bool {
        self.page().is_some()
    }

    pub fn page(self) -> Option<Page> {
        match self {
            Mode::Music | Mode::Lyrics => Some(Page::Music),
            Mode::Timer => Some(Page::Timer),
            Mode::Water => Some(Page::Water),
            Mode::Shelf => Some(Page::Shelf),
            Mode::Stopwatch => Some(Page::Stopwatch),
            Mode::Weather => Some(Page::Weather),
            Mode::Clipboard => Some(Page::Clipboard),
            Mode::Todo => Some(Page::Todo),
            Mode::Controls => Some(Page::Controls),
            Mode::System => Some(Page::System),
            Mode::Calendar => Some(Page::Calendar),
            _ => None,
        }
    }
}

/// Widgets reachable with the arrows / pagination dots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Page {
    Music,
    Timer,
    Stopwatch,
    Water,
    Weather,
    Shelf,
    Clipboard,
    Todo,
    Controls,
    System,
    Calendar,
}

impl Page {
    pub const ALL: [Page; 11] = [
        Page::Music,
        Page::Timer,
        Page::Stopwatch,
        Page::Water,
        Page::Weather,
        Page::Shelf,
        Page::Clipboard,
        Page::Todo,
        Page::Controls,
        Page::System,
        Page::Calendar,
    ];

    pub fn index(self) -> usize {
        Page::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }

    /// Position among the widgets the user enabled.
    pub fn index_in(self, pages: &[Page]) -> usize {
        pages.iter().position(|p| *p == self).unwrap_or(0)
    }

    pub fn offset_in(self, pages: &[Page], delta: i32) -> Option<Page> {
        let index = pages.iter().position(|p| *p == self)? as i32 + delta;
        if index < 0 { None } else { pages.get(index as usize).copied() }
    }

    pub fn name(self) -> &'static str {
        match self {
            Page::Music => "Music",
            Page::Timer => "Focus timer",
            Page::Stopwatch => "Stopwatch",
            Page::Water => "Water",
            Page::Weather => "Weather",
            Page::Shelf => "Shelf",
            Page::Clipboard => "Clipboard",
            Page::Todo => "To-do",
            Page::Controls => "Controls",
            Page::System => "System",
            Page::Calendar => "Calendar",
        }
    }
}

/// Interactive element identifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hit {
    NavPrev,
    NavNext,
    PageDot(u8),
    StatusBattery,
    StatusWave,
    MediaPrev,
    MediaPlayPause,
    MediaNext,
    MediaLyrics,
    MediaScrubber,
    TimerReset,
    TimerBreak,
    TimerPlayPause,
    TimerFocus,
    TimerDigits,
    WaterToggle,
    WaterDrink,
    WaterSettings,
    WaterDone,
    WaterUndo,
    WaterGoalDec,
    WaterGoalInc,
    WaterCupDec,
    WaterCupInc,
    WaterIntervalDec,
    WaterIntervalInc,
    ShelfClear,
    ShelfItem(u64),
    ShelfRemove(u64),
    TodoAdd,
    TodoEdit,
    TodoList,
    TodoInput,
    TodoCheck(u64),
    TodoText(u64),
    TodoDelete(u64),
    CalendarPrev,
    CalendarNext,
    CalendarToday,
    ClipList,
    ClipItem(u64),
    ClipPin(u64),
    ClipClear,
    SwModeStopwatch,
    SwModeTimer,
    SwStartPause,
    SwLapReset,
    SwMinus,
    SwPlus,
    SwDigits,
    WeatherCity,
    CtlWifi,
    CtlBluetooth,
    CtlPower,
    CtlFocus,
    CtlMute,
    CtlVolume,
    Popup,
}

#[derive(Clone, Copy, Debug)]
pub struct HitRegion {
    pub id: Hit,
    /// Canvas logical coordinates.
    pub rect: Rect,
}

pub fn hit_at(regions: &[HitRegion], point: Point) -> Option<Hit> {
    // Later registrations are drawn on top.
    regions.iter().rev().find(|r| r.rect.contains(point)).map(|r| r.id)
}

/// Hover and press springs per interactive element.
#[derive(Default)]
pub struct Feedback {
    hover: HashMap<Hit, SpringValue>,
    press: HashMap<Hit, SpringValue>,
    hovered: Option<Hit>,
    pressed: Option<Hit>,
}

impl Feedback {
    /// Returns `true` when the visual state changed.
    pub fn set_hovered(&mut self, id: Option<Hit>) -> bool {
        if self.hovered == id {
            return false;
        }
        if let Some(old) = self.hovered
            && let Some(spring) = self.hover.get_mut(&old)
        {
            spring.set_target(0.0);
        }
        if let Some(new) = id {
            self.hover.entry(new).or_insert_with(|| SpringValue::normalized(0.0, SpringConfig::HOVER)).set_target(1.0);
        }
        self.hovered = id;
        true
    }

    pub fn set_pressed(&mut self, id: Option<Hit>) -> bool {
        if self.pressed == id {
            return false;
        }
        if let Some(old) = self.pressed
            && let Some(spring) = self.press.get_mut(&old)
        {
            spring.set_target(0.0);
        }
        if let Some(new) = id {
            self.press.entry(new).or_insert_with(|| SpringValue::normalized(0.0, SpringConfig::SNAPPY)).set_target(1.0);
        }
        self.pressed = id;
        true
    }

    pub fn hover(&self, id: Hit) -> f32 {
        self.hover.get(&id).map_or(0.0, |s| s.value().clamp(0.0, 1.0))
    }

    pub fn press(&self, id: Hit) -> f32 {
        self.press.get(&id).map_or(0.0, |s| s.value().clamp(0.0, 1.0))
    }

    pub fn step(&mut self, dt: f32) -> bool {
        let mut moving = false;
        for spring in self.hover.values_mut().chain(self.press.values_mut()) {
            moving |= spring.step(dt);
        }
        self.hover.retain(|_, s| !(s.is_at_rest() && s.target() == 0.0));
        self.press.retain(|_, s| !(s.is_at_rest() && s.target() == 0.0));
        moving
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VAlign {
    /// Centers the cap height of the first line (or a multi-line block) on
    /// the anchor.
    Center,
}

/// Drawing context of one view. Views work in their own local space whose
/// origin is the top-left corner of the mode's box (see [`Mode::size`]).
pub struct Painter<'a> {
    pub scene: &'a mut Scene,
    pub text: &'a mut TextEngine,
    pub feedback: &'a Feedback,
    pub hits: Option<&'a mut Vec<HitRegion>>,
    /// Physical pixels per logical pixel.
    pub dpi: f64,
    /// Local view space to canvas logical space.
    pub local: Affine,
    /// Opacity multiplier for everything drawn.
    pub alpha: f32,
    /// Seconds since start, for ambient animation.
    pub time: f64,
    /// Bounds (canvas coordinates) of the active clips; hit regions are
    /// cut to the innermost one so hidden content is never clickable.
    pub hit_clips: Vec<Rect>,
}

impl Painter<'_> {
    pub fn transform(&self) -> Affine {
        Affine::scale(self.dpi) * self.local
    }

    fn faded(&self, color: Color) -> Color {
        color.multiply_alpha(self.alpha)
    }

    pub fn fill(&mut self, shape: &impl Shape, color: Color) {
        let (xf, color) = (self.transform(), self.faded(color));
        self.scene.fill(Fill::NonZero, xf, color, None, shape);
    }

    pub fn fill_gradient(&mut self, shape: &impl Shape, gradient: &Gradient) {
        let xf = self.transform();
        let gradient = gradient.clone().multiply_alpha(self.alpha);
        self.scene.fill(Fill::NonZero, xf, &gradient, None, shape);
    }

    /// Fills `shape` with `image` scaled to cover `rect`.
    pub fn fill_image(&mut self, shape: &impl Shape, image: &ImageData, rect: Rect) {
        let brush = ImageBrush::new(image.clone())
            .with_quality(vello::peniko::ImageQuality::High)
            .with_alpha(self.alpha);
        let scale = (rect.width() / image.width as f64).max(rect.height() / image.height as f64);
        let brush_transform = Affine::translate(rect.origin().to_vec2()) * Affine::scale(scale);
        let xf = self.transform();
        self.scene.fill(Fill::NonZero, xf, &brush, Some(brush_transform), shape);
    }

    pub fn stroke(&mut self, shape: &impl Shape, width: f64, color: Color) {
        let style = Stroke::new(width).with_caps(Cap::Round).with_join(Join::Round);
        self.stroke_with(shape, &style, color);
    }

    pub fn stroke_with(&mut self, shape: &impl Shape, style: &Stroke, color: Color) {
        let (xf, color) = (self.transform(), self.faded(color));
        self.scene.stroke(style, xf, color, None, shape);
    }

    /// Soft gaussian glow shaped like a rounded rectangle.
    pub fn glow(&mut self, rect: Rect, radius: f64, std_dev: f64, color: Color) {
        let (xf, color) = (self.transform(), self.faded(color));
        self.scene.draw_blurred_rounded_rect(xf, rect, color, radius, std_dev);
    }

    pub fn push_clip(&mut self, shape: &impl Shape) {
        let mut bounds = self.local.transform_rect_bbox(shape.bounding_box());
        if let Some(outer) = self.hit_clips.last() {
            bounds = bounds.intersect(*outer);
        }
        self.hit_clips.push(bounds);
        let xf = self.transform();
        self.scene.push_clip_layer(Fill::NonZero, xf, shape);
    }

    pub fn pop_clip(&mut self) {
        self.hit_clips.pop();
        self.scene.pop_layer();
    }

    pub fn layout(&mut self, text: &str, style: &TextStyle) -> Option<Arc<TextLayout>> {
        self.text.layout(text, style)
    }

    pub fn measure(&mut self, text: &str, style: &TextStyle) -> Size {
        self.text.layout(text, style).map_or(Size::ZERO, |l| l.size())
    }

    /// Draws a laid-out text with its top-left corner at `origin`.
    pub fn draw_layout(&mut self, layout: &TextLayout, origin: Point, color: Color) {
        let xf = self.transform() * Affine::translate(origin.to_vec2());
        layout.draw(self.scene, xf, self.faded(color));
    }

    /// Draws a laid-out text filled with `gradient` (in layout space).
    pub fn draw_layout_gradient(&mut self, layout: &TextLayout, origin: Point, gradient: &Gradient) {
        let xf = self.transform() * Affine::translate(origin.to_vec2());
        let brush = vello::peniko::Brush::Gradient(gradient.clone().multiply_alpha(self.alpha));
        layout.draw_brush(self.scene, xf, &brush);
    }

    /// Draws `text` aligned to `anchor`; returns the occupied rectangle.
    pub fn text(&mut self, text: &str, style: &TextStyle, anchor: Point, h: HAlign, v: VAlign, color: Color) -> Rect {
        let Some(layout) = self.text.layout(text, style) else {
            return Rect::from_origin_size(anchor, (0.0, 0.0));
        };
        let origin = text_origin(&layout, anchor, h, v);
        self.draw_layout(&layout, origin, color);
        Rect::from_origin_size(origin, layout.size())
    }

    pub fn icon(&mut self, icon: Icon, center: Point, size: f64, color: Color) {
        let scale = size / 24.0;
        let local = Affine::translate(center.to_vec2()) * Affine::scale(scale) * Affine::translate((-12.0, -12.0));
        let xf = self.transform() * local;
        let color = self.faded(color);
        for mark in icons::marks(icon).iter() {
            match mark {
                Mark::Fill(path) => self.scene.fill(Fill::NonZero, xf, color, None, path),
                Mark::FillEvenOdd(path) => self.scene.fill(Fill::EvenOdd, xf, color, None, path),
                Mark::Stroke(path, width) => {
                    let style = Stroke::new(*width).with_caps(Cap::Round).with_join(Join::Round);
                    self.scene.stroke(&style, xf, color, None, path);
                }
            }
        }
    }

    /// Registers an interactive rectangle (local coordinates).
    pub fn hit(&mut self, id: Hit, rect: Rect) {
        let mut canvas_rect = self.local.transform_rect_bbox(rect);
        if let Some(clip) = self.hit_clips.last() {
            canvas_rect = canvas_rect.intersect(*clip);
            if canvas_rect.area() <= 0.0 {
                return;
            }
        }
        if let Some(hits) = self.hits.as_deref_mut() {
            hits.push(HitRegion { id, rect: canvas_rect });
        }
    }

    pub fn interactive(&self) -> bool {
        self.hits.is_some()
    }

    pub fn hover(&self, id: Hit) -> f32 {
        if self.interactive() { self.feedback.hover(id) } else { 0.0 }
    }

    pub fn press(&self, id: Hit) -> f32 {
        if self.interactive() { self.feedback.press(id) } else { 0.0 }
    }

    /// Runs `f` with an additional local transform.
    pub fn with_transform<R>(&mut self, transform: Affine, f: impl FnOnce(&mut Self) -> R) -> R {
        let saved = self.local;
        self.local *= transform;
        let result = f(self);
        self.local = saved;
        result
    }

    pub fn with_alpha<R>(&mut self, alpha: f32, f: impl FnOnce(&mut Self) -> R) -> R {
        let saved = self.alpha;
        self.alpha *= alpha.clamp(0.0, 1.0);
        let result = f(self);
        self.alpha = saved;
        result
    }
}

pub fn text_origin(layout: &TextLayout, anchor: Point, h: HAlign, v: VAlign) -> Point {
    let x = match h {
        HAlign::Left => anchor.x,
        HAlign::Center => anchor.x - layout.width as f64 / 2.0,
        HAlign::Right => anchor.x - layout.width as f64,
    };
    let y = match v {
        VAlign::Center if layout.lines > 1 => anchor.y - layout.height as f64 / 2.0,
        VAlign::Center => anchor.y - layout.baseline as f64 + layout.size as f64 * 0.36,
    };
    Point::new(x, y)
}

/// Shelf drag state shown by the views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropState {
    None,
    /// Files are being dragged over the notch.
    Hovering,
}

/// Everything the views display, borrowed from the application state for
/// the duration of one frame.
pub struct Model<'a> {
    pub now: Instant,
    /// Screen edge the notch hangs from; views keep their layout but put
    /// edge effects (the resting glow) on the free edge.
    pub edge: Edge,
    pub clock: chrono::DateTime<chrono::Local>,
    pub power: PowerStatus,
    pub media: Option<&'a MediaSnapshot>,
    pub artwork: Option<&'a ImageData>,
    /// Position preview while the scrubber is dragged (fraction 0..1).
    pub scrub: Option<f64>,
    pub lyrics: &'a lyrics::LyricsState,
    pub timer: &'a timer::Pomodoro,
    pub water: &'a water::WaterTracker,
    pub shelf: &'a shelf::Shelf,
    pub todo: &'a todo::TodoList,
    pub headphones: Option<&'a AudioDevice>,
    pub indicator: Option<indicator::Indicator>,
    pub system: &'a crate::services::system::SystemStats,
    /// Months between the calendar's shown month and the current one.
    pub calendar_offset: i32,
    /// Widgets the user enabled, in carousel order.
    pub pages: &'a [Page],
    pub clipboard: &'a clipboard::ClipboardHistory,
    pub clipboard_enabled: bool,
    pub clipboard_scroll: f64,
    pub stopwatch: &'a stopwatch::Stopwatch,
    pub weather: Option<&'a crate::services::weather::Weather>,
    pub weather_city: Option<&'a str>,
    pub weather_error: Option<&'a str>,
    pub controls: crate::services::controls::ControlsState,
    pub volume: crate::services::audio::VolumeState,
    /// Volume while the slider is dragged.
    pub volume_preview: Option<f32>,
    pub notification: Option<&'a crate::services::notifications::Notification>,
    /// Theme accent color and its companion (glows, gradients).
    pub accent: Color,
    pub accent2: Color,
    /// Animated color taken from the album art (music views), when enabled.
    pub album: Option<Color>,
    pub drop: DropState,
    /// Animated highlight of the drop zone (border color, 0..1 intensity).
    pub drop_tint: Color,
    pub drop_amount: f32,
    /// Horizontal scroll of the shelf items, vertical scroll of the tasks.
    pub shelf_scroll: f64,
    pub todo_scroll: f64,
    pub audio_playing: bool,
    pub page: Page,
    /// Animated position of the active pagination dot.
    pub page_indicator: f32,
    pub eyes: resting::EyesPose,
    pub caret_visible: bool,
}

/// Animated notch outline: a spring-driven rectangle hanging from the top
/// (or bottom) edge of the canvas, centered horizontally, plus its corner
/// radius.
pub struct NotchShape {
    pub rect: RectSpring,
    pub radius: SpringValue,
    edge: Edge,
    mode: Mode,
}

impl NotchShape {
    fn rect_for(mode: Mode, edge: Edge) -> Rect {
        let size = mode.size();
        let cx = CANVAS_WIDTH / 2.0;
        match edge {
            Edge::Top => Rect::new(cx - size.width / 2.0, 0.0, cx + size.width / 2.0, size.height),
            Edge::Bottom => {
                Rect::new(cx - size.width / 2.0, CANVAS_HEIGHT - size.height, cx + size.width / 2.0, CANVAS_HEIGHT)
            }
        }
    }

    pub fn new(mode: Mode, edge: Edge) -> Self {
        Self {
            rect: RectSpring::new(Self::rect_for(mode, edge), SpringConfig::EXPAND),
            radius: SpringValue::new(mode.corner_radius() as f32, SpringConfig::EXPAND),
            edge,
            mode,
        }
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.rect.set_target(Self::rect_for(mode, self.edge));
        self.radius.set_target(mode.corner_radius() as f32);
    }

    /// Moves the notch to another screen edge (no animation across the canvas).
    pub fn set_edge(&mut self, edge: Edge) {
        self.edge = edge;
        self.rect.snap(Self::rect_for(self.mode, edge));
    }

    pub fn step(&mut self, dt: f32) -> bool {
        self.rect.step(dt) | self.radius.step(dt)
    }

    /// Body rectangle (without shoulders) in canvas coordinates.
    pub fn body(&self) -> Rect {
        self.rect.rect()
    }

    /// Where the body is heading.
    pub fn target(&self) -> Rect {
        self.rect.target()
    }

    /// Outline in canvas coordinates. The shape is built against the top
    /// edge; at the bottom it is mirrored so the shoulders flare into the
    /// bottom edge and the rounded corners face up.
    pub fn path(&self) -> vello::kurbo::BezPath {
        let body = self.body();
        let mut path =
            notch_path(body.center().x, body.width(), body.height(), self.radius.value() as f64, SHOULDER_RADIUS);
        if self.edge == Edge::Bottom {
            path.apply_affine(Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, CANVAS_HEIGHT]));
        }
        path
    }
}

/// Draws the content of `mode` with the painter's current transform.
pub fn draw_view(mode: Mode, p: &mut Painter, m: &Model) {
    match mode {
        Mode::Resting => resting::draw(p, m),
        Mode::Closed => closed::draw(p, m),
        Mode::Music => music::draw(p, m),
        Mode::Lyrics => lyrics::draw(p, m),
        Mode::Timer => timer::draw(p, m),
        Mode::Water => water::draw(p, m),
        Mode::Shelf => shelf::draw(p, m),
        Mode::Todo => todo::draw(p, m),
        Mode::System => system::draw(p, m),
        Mode::Calendar => calendar::draw(p, m),
        Mode::Stopwatch => stopwatch::draw(p, m),
        Mode::Weather => weather::draw(p, m),
        Mode::Clipboard => clipboard::draw(p, m),
        Mode::Controls => controls::draw(p, m),
        Mode::Notification => notification::draw(p, m),
        Mode::Headphones => headphones::draw(p, m),
        Mode::Charging => charging::draw(p, m),
        Mode::Indicator => indicator::draw(p, m),
    }
}

/// Pagination dots of the enabled widgets.
pub fn page_dots(p: &mut Painter, m: &Model, width: f64) {
    common::pagination(p, Point::new(width / 2.0, 161.0), m.pages.len(), m.page_indicator);
}

/// Side arrows of the widget pages (hidden at the ends of the carousel).
pub fn page_navigation(p: &mut Painter, m: &Model, width: f64, y: f64) {
    if m.page.offset_in(m.pages, -1).is_some() {
        common::nav_button(p, Hit::NavPrev, Point::new(26.0, y), Icon::ChevronLeft);
    }
    if m.page.offset_in(m.pages, 1).is_some() {
        common::nav_button(p, Hit::NavNext, Point::new(width - 26.0, y), Icon::ChevronRight);
    }
}

/// Converts a `#RRGGBB` constant into a color.
pub const fn rgb(hex: u32) -> Color {
    Color::from_rgb8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub const fn rgba(hex: u32, alpha: f32) -> Color {
    let c = rgb(hex);
    Color::new([c.components[0], c.components[1], c.components[2], alpha])
}

pub mod palette {
    use super::{rgb, rgba};
    use vello::peniko::Color;

    pub const WHITE: Color = Color::WHITE;
    pub const GREEN: Color = rgb(0x34C759);
    pub const RED: Color = rgb(0xFF453A);
    pub const BLUE: Color = rgb(0x38BDF8);
    pub const BLUE_DEEP: Color = rgb(0x0A84FF);
    pub const GRAY: Color = rgb(0x8E8E93);
    pub const DIVIDER: Color = rgba(0xFFFFFF, 0.10);
    pub const ZONE_BORDER: Color = rgb(0x4A4A4E);
    pub const ZONE_ACTIVE: Color = rgba(0x7DB4FF, 0.9);
    pub const CYAN: Color = rgb(0x22D3EE);
    pub const PURPLE: Color = rgb(0xA855F7);
    pub const INDIGO: Color = rgb(0x6366F1);
}
