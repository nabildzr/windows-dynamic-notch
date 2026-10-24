//! Shelf: a temporary holding area for files. Drop files from Explorer onto
//! the notch, drag them out again into any app, or click to open.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use vello::kurbo::{Circle, Point, Rect, RoundedRect, Stroke};
use vello::peniko::ImageData;

use super::common::{ButtonStyle, TITLE_STYLE, capsule_button, status_bar};
use super::{DropState, HAlign, Hit, Mode, Model, Painter, VAlign, page_navigation, palette, rgb, rgba};
use crate::renderer::icons::Icon;
use crate::renderer::shapes::squircle;
use crate::renderer::text::TextStyle;
use crate::services::shelf::{ShelfFileInfo, format_size};

const ZONE: Rect = Rect::new(50.0, 38.0, 370.0, 150.0);
const ITEM_WIDTH: f64 = 70.0;
const THUMB: f64 = 44.0;
const NAME: TextStyle = TextStyle::new(11.5, TextStyle::BOLD);
const SIZE: TextStyle = TextStyle::new(10.5, TextStyle::REGULAR).tabular();
const HEADLINE: TextStyle = TextStyle::new(15.0, TextStyle::SEMIBOLD);
const HINT: TextStyle = TextStyle::new(11.5, TextStyle::REGULAR);
const CLEAR: TextStyle = TextStyle::new(11.5, TextStyle::MEDIUM);
const MAX_ITEMS: usize = 24;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShelfEntry {
    pub id: u64,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ShelfData {
    pub items: Vec<ShelfEntry>,
}

pub struct ShelfItem {
    pub id: u64,
    pub path: PathBuf,
    pub name: String,
    pub size: Option<u64>,
    pub thumbnail: Option<Arc<ImageData>>,
}

#[derive(Default)]
pub struct Shelf {
    pub items: Vec<ShelfItem>,
    next_id: u64,
}

impl Shelf {
    pub fn from_data(data: ShelfData) -> Self {
        let mut shelf = Shelf::default();
        for entry in data.items {
            shelf.next_id = shelf.next_id.max(entry.id + 1);
            shelf.items.push(ShelfItem::new(entry.id, entry.path));
        }
        shelf
    }

    pub fn to_data(&self) -> ShelfData {
        ShelfData { items: self.items.iter().map(|item| ShelfEntry { id: item.id, path: item.path.clone() }).collect() }
    }

    /// Adds a file (ignoring duplicates); returns its id.
    pub fn add(&mut self, path: PathBuf) -> Option<u64> {
        if self.items.iter().any(|item| item.path == path) {
            return None;
        }
        if self.items.len() >= MAX_ITEMS {
            self.items.remove(0);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.items.push(ShelfItem::new(id, path));
        Some(id)
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|item| item.id != id);
        self.items.len() != before
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn get(&self, id: u64) -> Option<&ShelfItem> {
        self.items.iter().find(|item| item.id == id)
    }

    /// Applies metadata from the shelf service. Returns `false` when the
    /// file vanished and the item was dropped.
    pub fn apply(&mut self, info: ShelfFileInfo) -> bool {
        if !info.exists {
            self.remove(info.id);
            return false;
        }
        if let Some(item) = self.items.iter_mut().find(|item| item.id == info.id) {
            item.name = info.name;
            item.size = info.size;
            item.thumbnail = info.thumbnail;
        }
        true
    }

    /// Maximum horizontal scroll offset of the item row.
    pub fn max_scroll(&self) -> f64 {
        let content = self.items.len() as f64 * ITEM_WIDTH;
        (content - (ZONE.width() - 24.0)).max(0.0)
    }
}

impl ShelfItem {
    fn new(id: u64, path: PathBuf) -> Self {
        let name = file_name(&path);
        Self { id, path, name, size: None, thumbnail: None }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

pub fn draw(p: &mut Painter, m: &Model) {
    let width = Mode::Shelf.size().width;
    let shelf = m.shelf;
    status_bar(p, m, width, &[("Shelf", palette::WHITE)]);
    if !shelf.items.is_empty() {
        let title_w = p.measure("Shelf", &TITLE_STYLE).width;
        let x = 20.0 + title_w + 10.0;
        capsule_button(
            p,
            Hit::ShelfClear,
            Rect::new(x, 10.0, x + 48.0, 30.0),
            None,
            "Clear",
            ButtonStyle::GRAY,
            &CLEAR,
        );
    }
    let hovering = m.drop == DropState::Hovering;
    let amount = m.drop_amount.clamp(0.0, 1.0);
    if amount > 0.01 {
        p.glow(Rect::new(70.0, 120.0, 350.0, 200.0), 60.0, 30.0, rgba(0x2563EB, 0.45 * amount));
    }
    let zone = RoundedRect::from_rect(ZONE, 14.0);
    p.fill(&zone, super::common::mix(rgba(0xFFFFFF, 0.025), rgba(0x0A84FF, 0.10), amount));
    p.stroke_with(&zone, &Stroke::new(1.2).with_dashes(0.0, [5.0, 4.0]), m.drop_tint);

    if shelf.items.is_empty() {
        let center = ZONE.center().x;
        if hovering {
            p.icon(Icon::InboxDown, Point::new(center, 72.0), 28.0, palette::WHITE);
            p.text("Release to add", &HEADLINE, Point::new(center, 104.0), HAlign::Center, VAlign::Center, palette::WHITE);
        } else {
            p.icon(Icon::InboxDown, Point::new(center, 70.0), 26.0, rgba(0xFFFFFF, 0.92));
            p.text("Drop files here", &HEADLINE, Point::new(center, 102.0), HAlign::Center, VAlign::Center, palette::WHITE);
            p.text(
                "Drag out anytime \u{00B7} click to open",
                &HINT,
                Point::new(center, 121.0),
                HAlign::Center,
                VAlign::Center,
                palette::GRAY,
            );
        }
    } else {
        items(p, m, hovering);
        if hovering {
            p.fill(&RoundedRect::from_rect(ZONE.inflate(-1.0, -1.0), 13.0), rgba(0x000000, 0.45));
            p.icon(Icon::InboxDown, Point::new(ZONE.center().x, 80.0), 24.0, palette::WHITE);
            p.text("Release to add", &HEADLINE, Point::new(ZONE.center().x, 110.0), HAlign::Center, VAlign::Center, palette::WHITE);
        }
    }
    // Registered last so the arrows win over items scrolled under them.
    page_navigation(p, m, width, 94.0);
    super::page_dots(p, m, width);
}

fn items(p: &mut Painter, m: &Model, hovering: bool) {
    let scroll = m.shelf_scroll;
    let inner = ZONE.inflate(-6.0, -4.0);
    p.push_clip(&RoundedRect::from_rect(inner, 10.0));
    for (index, item) in m.shelf.items.iter().enumerate() {
        let x = inner.x0 + 6.0 + index as f64 * ITEM_WIDTH - scroll;
        if x + ITEM_WIDTH < inner.x0 || x > inner.x1 {
            continue;
        }
        let center_x = x + ITEM_WIDTH / 2.0;
        let id = Hit::ShelfItem(item.id);
        let remove_id = Hit::ShelfRemove(item.id);
        let remove_hover = p.hover(remove_id);
        let hover = if hovering { 0.0 } else { p.hover(id).max(remove_hover) as f64 };
        let lift = 2.0 * hover - 1.5 * p.press(id) as f64;
        let thumb = Rect::from_center_size(Point::new(center_x, 72.0 - lift), (THUMB, THUMB));
        if hover > 0.01 {
            p.fill(&squircle(thumb.inflate(5.0, 5.0), 14.0), rgba(0xFFFFFF, 0.08 * hover as f32));
        }
        match &item.thumbnail {
            Some(image) => {
                let aspect = image.width as f64 / image.height.max(1) as f64;
                let (w, h) = if aspect >= 1.0 { (THUMB, THUMB / aspect) } else { (THUMB * aspect, THUMB) };
                let fitted = Rect::from_center_size(thumb.center(), (w, h));
                p.fill_image(&squircle(fitted, 9.0), image, fitted);
            }
            None => {
                p.fill(&squircle(thumb, 10.0), rgba(0xFFFFFF, 0.12));
                p.icon(Icon::InboxDown, thumb.center(), 20.0, palette::GRAY);
            }
        }
        p.text(
            &item.name,
            &NAME.max_width(ITEM_WIDTH as f32 - 6.0),
            Point::new(center_x, 109.0),
            HAlign::Center,
            VAlign::Center,
            palette::WHITE,
        );
        let size = item.size.map(format_size).unwrap_or_default();
        p.text(&size, &SIZE, Point::new(center_x, 124.0), HAlign::Center, VAlign::Center, palette::GRAY);
        // Only the visible part of a clipped item is clickable.
        let item_rect = Rect::new(x + 2.0, 44.0, x + ITEM_WIDTH - 2.0, 132.0).intersect(inner);
        if item_rect.area() > 0.0 {
            p.hit(id, item_rect);
        }
        if hover > 0.02 {
            let remove = Point::new(thumb.x1 + 1.0, thumb.y0 - 1.0);
            p.with_alpha(hover as f32, |p| {
                p.fill(&Circle::new(remove, 8.5), super::common::mix(rgb(0x3A3A3C), rgb(0x5A5A5E), remove_hover));
                p.icon(Icon::Close, remove, 10.0, palette::WHITE);
            });
            let remove_rect = Rect::from_center_size(remove, (20.0, 20.0)).intersect(inner);
            if remove_rect.area() > 0.0 {
                p.hit(remove_id, remove_rect);
            }
        }
    }
    p.pop_clip();
}
