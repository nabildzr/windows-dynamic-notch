//! Text shaping (cosmic-text) and glyph rendering (vello).
//!
//! Font selection: Inter (body) and Inter Display (large type), embedded in
//! the executable (SIL Open Font License, see `assets/fonts/OFL.txt`).
//! Segoe UI Variable / Segoe UI remain as fallbacks for glyphs Inter lacks.
//! Numbers are shaped with the OpenType `tnum` feature so clocks and timers
//! never jitter.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

use cosmic_text::skrifa::{FontRef, MetadataProvider, Tag};
use cosmic_text::{
    Attrs, Buffer, Ellipsize, EllipsizeHeightLimit, Family, FeatureTag, FontFeatures, FontSystem, Metrics, Shaping,
    Weight, Wrap, fontdb,
};
use vello::kurbo::{Affine, Size};
use vello::peniko::{Color, Fill, FontData};
use vello::{Glyph, Scene};

const DISPLAY_FAMILIES: [&str; 5] = ["Inter Display", "Inter", "Segoe UI Variable Display", "Segoe UI Variable", "Segoe UI"];
const TEXT_FAMILIES: [&str; 4] = ["Inter", "Segoe UI Variable Text", "Segoe UI Variable", "Segoe UI"];

/// Inter faces compiled into the binary, so no font has to be installed.
const EMBEDDED_FONTS: [&[u8]; 8] = [
    include_bytes!("../../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/Inter-Bold.ttf"),
    include_bytes!("../../assets/fonts/InterDisplay-Regular.ttf"),
    include_bytes!("../../assets/fonts/InterDisplay-Medium.ttf"),
    include_bytes!("../../assets/fonts/InterDisplay-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/InterDisplay-Bold.ttf"),
];
const CACHE_LIMIT: usize = 384;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontRole {
    /// Large type (>= 20 px): titles, clock digits.
    Display,
    /// Body and small type.
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    pub size: f32,
    pub weight: u16,
    pub role: FontRole,
    pub tabular: bool,
    pub max_width: Option<f32>,
    pub wrap: bool,
    pub max_lines: Option<usize>,
    pub line_height: f32,
}

impl TextStyle {
    pub const REGULAR: u16 = 400;
    pub const MEDIUM: u16 = 500;
    pub const SEMIBOLD: u16 = 600;
    pub const BOLD: u16 = 700;

    pub const fn new(size: f32, weight: u16) -> Self {
        Self {
            size,
            weight,
            role: if size >= 20.0 { FontRole::Display } else { FontRole::Text },
            tabular: false,
            max_width: None,
            wrap: false,
            max_lines: None,
            line_height: 1.22,
        }
    }

    /// Tabular (fixed-width) figures.
    pub const fn tabular(mut self) -> Self {
        self.tabular = true;
        self
    }

    /// Single line, truncated with an ellipsis beyond `width`.
    pub const fn max_width(mut self, width: f32) -> Self {
        self.max_width = Some(width);
        self
    }

    /// Word-wrapped to `width`, optionally limited to `max_lines`.
    pub const fn wrapped(mut self, width: f32, max_lines: Option<usize>) -> Self {
        self.max_width = Some(width);
        self.wrap = true;
        self.max_lines = max_lines;
        self
    }

    pub const fn line_height(mut self, factor: f32) -> Self {
        self.line_height = factor;
        self
    }
}

#[derive(Clone, Debug)]
struct GlyphRun {
    font: FontData,
    coords: Arc<[i16]>,
    size: f32,
    glyphs: Vec<Glyph>,
}

/// Shaped, positioned text. Coordinates are relative to the top-left of
/// the text box.
#[derive(Clone, Debug, Default)]
pub struct TextLayout {
    pub width: f32,
    pub height: f32,
    /// Baseline of the first line, from the top of the box.
    pub baseline: f32,
    pub lines: usize,
    pub size: f32,
    runs: Vec<GlyphRun>,
}

impl TextLayout {
    pub fn size(&self) -> Size {
        Size::new(self.width as f64, self.height as f64)
    }

    /// Draws the glyphs with `transform` mapping layout space to pixels.
    pub fn draw(&self, scene: &mut Scene, transform: Affine, color: Color) {
        self.draw_brush(scene, transform, &vello::peniko::Brush::Solid(color));
    }

    /// Like [`Self::draw`] with any brush; gradients are in layout space.
    pub fn draw_brush(&self, scene: &mut Scene, transform: Affine, brush: &vello::peniko::Brush) {
        for run in &self.runs {
            scene
                .draw_glyphs(&run.font)
                .font_size(run.size)
                .transform(transform)
                .normalized_coords(&run.coords)
                .brush(brush)
                .draw(Fill::NonZero, run.glyphs.iter().copied());
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct LayoutKey {
    text: Box<str>,
    size: u32,
    weight: u16,
    role: FontRole,
    tabular: bool,
    width: Option<u32>,
    wrap: bool,
    max_lines: Option<usize>,
    line_height: u32,
}

impl LayoutKey {
    fn new(text: &str, style: &TextStyle) -> Self {
        Self {
            text: text.into(),
            size: style.size.to_bits(),
            weight: style.weight,
            role: style.role,
            tabular: style.tabular,
            width: style.max_width.map(f32::to_bits),
            wrap: style.wrap,
            max_lines: style.max_lines,
            line_height: style.line_height.to_bits(),
        }
    }
}

struct CacheEntry {
    layout: Arc<TextLayout>,
    last_used: u64,
}

pub struct TextEngine {
    system: Option<FontSystem>,
    pending: Option<Receiver<fontdb::Database>>,
    display_family: String,
    text_family: String,
    cache: HashMap<LayoutKey, CacheEntry>,
    coords: HashMap<(fontdb::ID, u16), Arc<[i16]>>,
    frame: u64,
}

impl TextEngine {
    /// Starts loading the font database on a background thread; `on_ready`
    /// runs on that thread once the database is available.
    pub fn new(on_ready: impl FnOnce() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("font-loader".into()).spawn(move || {
            let started = Instant::now();
            let mut db = fontdb::Database::new();
            for font in EMBEDDED_FONTS {
                db.load_font_source(fontdb::Source::Binary(Arc::new(font)));
            }
            db.load_system_fonts();
            for dir in extra_font_dirs() {
                if dir.is_dir() {
                    db.load_fonts_dir(&dir);
                }
            }
            log::info!("font database: {} faces in {:?}", db.len(), started.elapsed());
            let _ = tx.send(db);
            on_ready();
        });
        if let Err(err) = spawned {
            log::error!("failed to start font loader: {err}");
        }
        Self {
            system: None,
            pending: Some(rx),
            display_family: "Segoe UI".into(),
            text_family: "Segoe UI".into(),
            cache: HashMap::new(),
            coords: HashMap::new(),
            frame: 0,
        }
    }

    /// Installs the font database if the loader finished. Returns `true`
    /// when text can be shaped.
    pub fn poll_ready(&mut self) -> bool {
        if self.system.is_some() {
            return true;
        }
        let Some(rx) = &self.pending else { return false };
        match rx.try_recv() {
            Ok(db) => {
                self.pending = None;
                self.install(db);
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                false
            }
        }
    }

    fn install(&mut self, mut db: fontdb::Database) {
        let has_family = |db: &fontdb::Database, name: &str| {
            db.faces().any(|face| face.families.iter().any(|(family, _)| family.eq_ignore_ascii_case(name)))
        };
        self.display_family = DISPLAY_FAMILIES
            .iter()
            .find(|name| has_family(&db, name))
            .map_or("Segoe UI", |name| *name)
            .to_string();
        self.text_family =
            TEXT_FAMILIES.iter().find(|name| has_family(&db, name)).map_or("Segoe UI", |name| *name).to_string();
        log::info!("fonts: display = {:?}, text = {:?}", self.display_family, self.text_family);
        db.set_sans_serif_family(self.text_family.clone());
        self.system = Some(FontSystem::new_with_locale_and_db(user_locale(), db));
    }

    /// Called once per rendered frame; evicts layouts that were not used
    /// recently.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        if self.cache.len() > CACHE_LIMIT {
            let horizon = self.frame.saturating_sub(240);
            self.cache.retain(|_, entry| entry.last_used >= horizon);
        }
    }

    /// Shapes `text` (cached). `None` until the font database is loaded.
    pub fn layout(&mut self, text: &str, style: &TextStyle) -> Option<Arc<TextLayout>> {
        if !self.poll_ready() {
            return None;
        }
        let key = LayoutKey::new(text, style);
        if let Some(entry) = self.cache.get_mut(&key) {
            entry.last_used = self.frame;
            return Some(entry.layout.clone());
        }
        let layout = Arc::new(self.shape(text, style));
        self.cache.insert(key, CacheEntry { layout: layout.clone(), last_used: self.frame });
        Some(layout)
    }

    fn shape(&mut self, text: &str, style: &TextStyle) -> TextLayout {
        let Self { system, display_family, text_family, coords, .. } = self;
        let Some(system) = system.as_mut() else { return TextLayout::default() };
        let family = match style.role {
            FontRole::Display => display_family.as_str(),
            FontRole::Text => text_family.as_str(),
        };
        let mut features = FontFeatures::new();
        if style.tabular {
            features.enable(FeatureTag::new(b"tnum"));
        }
        let attrs = Attrs::new().family(Family::Name(family)).weight(Weight(style.weight)).font_features(features);
        let line_height = (style.size * style.line_height).round().max(1.0);
        let mut buffer = Buffer::new(system, Metrics::new(style.size, line_height));
        if style.wrap {
            buffer.set_wrap(Wrap::WordOrGlyph);
            if let Some(lines) = style.max_lines {
                buffer.set_ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(lines)));
            }
        } else {
            buffer.set_wrap(Wrap::None);
            if style.max_width.is_some() {
                buffer.set_ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)));
            }
        }
        buffer.set_size(style.max_width, None);
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(system, false);

        let mut layout = TextLayout { size: style.size, ..Default::default() };
        let mut current: Option<(fontdb::ID, u16, u32)> = None;
        for run in buffer.layout_runs() {
            if layout.lines == 0 {
                layout.baseline = run.line_y;
            }
            layout.lines += 1;
            layout.width = layout.width.max(run.line_w);
            layout.height = layout.height.max(run.line_top + run.line_height);
            for glyph in run.glyphs {
                let key = (glyph.font_id, glyph.font_weight.0, glyph.font_size.to_bits());
                if current != Some(key) {
                    let Some(font) = system.get_font(glyph.font_id, glyph.font_weight) else { continue };
                    let data = font.as_peniko();
                    let run_coords = coords
                        .entry((glyph.font_id, glyph.font_weight.0))
                        .or_insert_with(|| variation_coords(&data, glyph.font_weight.0))
                        .clone();
                    layout.runs.push(GlyphRun { font: data, coords: run_coords, size: glyph.font_size, glyphs: Vec::new() });
                    current = Some(key);
                }
                if let Some(out) = layout.runs.last_mut() {
                    out.glyphs.push(Glyph {
                        id: glyph.glyph_id as u32,
                        x: glyph.x + glyph.font_size * glyph.x_offset,
                        y: run.line_y + glyph.y - glyph.font_size * glyph.y_offset,
                    });
                }
            }
        }
        layout
    }
}

/// Normalized variation coordinates matching cosmic-text's shaping location
/// (only the `wght` axis is set), so variable fonts render at the same
/// weight they were shaped with.
fn variation_coords(font: &FontData, weight: u16) -> Arc<[i16]> {
    let Ok(font_ref) = FontRef::from_index(font.data.data(), font.index) else { return Arc::from([]) };
    let axes = font_ref.axes();
    if axes.is_empty() {
        return Arc::from([]);
    }
    let location = axes.location([(Tag::new(b"wght"), weight as f32)]);
    location.coords().iter().map(|c| c.to_bits()).collect()
}

fn extra_font_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![crate::services::storage::data_dir().join("fonts")];
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())) {
        dirs.push(exe_dir.join("fonts"));
    }
    dirs
}

fn user_locale() -> String {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buffer = [0u16; 85];
    let len = unsafe { GetUserDefaultLocaleName(&mut buffer) };
    if len > 1 { String::from_utf16_lossy(&buffer[..(len as usize - 1)]) } else { "en-US".to_string() }
}
