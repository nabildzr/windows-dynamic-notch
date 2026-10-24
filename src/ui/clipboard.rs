//! Clipboard history: the last copied texts, newest first. Click an entry to
//! copy it again, pin the ones worth keeping. Content that its source marks
//! as private (password managers) is never recorded.

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use vello::kurbo::{Point, Rect};

use super::common::{ButtonStyle, capsule_button, mix, status_bar};
use super::{HAlign, Hit, Mode, Model, Painter, VAlign, page_navigation, palette, rgba};
use crate::renderer::icons::Icon;
use crate::renderer::shapes::capsule;
use crate::renderer::text::TextStyle;

/// Unpinned entries kept.
const MAX_RECENT: usize = 30;
const MAX_PINNED: usize = 20;
/// Longer texts are stored truncated.
const MAX_CHARS: usize = 4000;

pub const LIST: Rect = Rect::new(50.0, 40.0, 370.0, 150.0);
pub const ROW_HEIGHT: f64 = 24.0;
pub const ROW_GAP: f64 = 4.0;

const ENTRY: TextStyle = TextStyle::new(12.0, TextStyle::MEDIUM);
const AGE: TextStyle = TextStyle::new(10.5, TextStyle::REGULAR).tabular();
const SMALL: TextStyle = TextStyle::new(11.0, TextStyle::MEDIUM);
const HINT: TextStyle = TextStyle::new(12.5, TextStyle::MEDIUM);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClipEntry {
    pub id: u64,
    pub text: String,
    #[serde(default)]
    pub pinned: bool,
    pub at: DateTime<Local>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipboardData {
    pub entries: Vec<ClipEntry>,
}

#[derive(Default)]
pub struct ClipboardHistory {
    pub data: ClipboardData,
    next_id: u64,
    /// Entry just copied back (for the "Copied" check), until then.
    pub copied: Option<(u64, std::time::Instant)>,
}

impl ClipboardHistory {
    pub fn new(data: ClipboardData) -> Self {
        let next_id = data.entries.iter().map(|e| e.id + 1).max().unwrap_or(0);
        Self { data, next_id, copied: None }
    }

    /// Records a copied text (moving an existing equal entry to the top).
    /// Returns `true` when the history changed.
    pub fn record(&mut self, text: &str, now: DateTime<Local>) -> bool {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return false;
        }
        let text: String = if text.chars().count() > MAX_CHARS { text.chars().take(MAX_CHARS).collect() } else { text.to_string() };
        if let Some(index) = self.data.entries.iter().position(|e| e.text == text) {
            if index == 0 {
                return false;
            }
            let mut entry = self.data.entries.remove(index);
            entry.at = now;
            self.data.entries.insert(0, entry);
            return true;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.data.entries.insert(0, ClipEntry { id, text, pinned: false, at: now });
        let mut recent = 0;
        self.data.entries.retain(|e| {
            if e.pinned {
                return true;
            }
            recent += 1;
            recent <= MAX_RECENT
        });
        true
    }

    pub fn get(&self, id: u64) -> Option<&ClipEntry> {
        self.data.entries.iter().find(|e| e.id == id)
    }

    pub fn toggle_pin(&mut self, id: u64) -> bool {
        let pinned = self.data.entries.iter().filter(|e| e.pinned).count();
        match self.data.entries.iter_mut().find(|e| e.id == id) {
            Some(entry) if entry.pinned || pinned < MAX_PINNED => {
                entry.pinned = !entry.pinned;
                true
            }
            _ => false,
        }
    }

    /// Removes everything except pinned entries.
    pub fn clear(&mut self) {
        self.data.entries.retain(|e| e.pinned);
    }

    /// Pinned first, then by recency.
    pub fn ordered(&self) -> Vec<&ClipEntry> {
        let mut list: Vec<&ClipEntry> = self.data.entries.iter().filter(|e| e.pinned).collect();
        list.extend(self.data.entries.iter().filter(|e| !e.pinned));
        list
    }

    pub fn max_scroll(&self) -> f64 {
        let content = self.data.entries.len() as f64 * (ROW_HEIGHT + ROW_GAP) - ROW_GAP;
        (content - LIST.height()).max(0.0)
    }
}

fn age(at: DateTime<Local>, now: DateTime<Local>) -> String {
    let minutes = (now - at).num_minutes();
    match minutes {
        m if m < 1 => "now".into(),
        m if m < 60 => format!("{m}m"),
        m if m < 60 * 24 => format!("{}h", m / 60),
        m => format!("{}d", m / (60 * 24)),
    }
}

/// First line, whitespace collapsed.
fn preview(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn draw(p: &mut Painter, m: &Model) {
    let width = Mode::Clipboard.size().width;
    status_bar(p, m, width, &[("Clipboard", palette::WHITE)]);
    let history = m.clipboard;
    let has_unpinned = history.data.entries.iter().any(|e| !e.pinned);

    if !m.clipboard_enabled {
        message(p, "Clipboard history is off", "Turn it on from the menu");
    } else if history.data.entries.is_empty() {
        p.icon(Icon::Copy, Point::new(LIST.center().x, 72.0), 24.0, rgba(0xFFFFFF, 0.9));
        message(p, "Copy something", "Texts you copy show up here");
    } else {
        list(p, m);
    }
    if has_unpinned {
        // Beside the pagination dots; pinned entries survive a clear.
        capsule_button(p, Hit::ClipClear, Rect::new(318.0, 152.0, 370.0, 170.0), None, "Clear", ButtonStyle::GRAY, &SMALL);
    }
    page_navigation(p, m, width, 86.0);
    super::page_dots(p, m, width);
}

fn message(p: &mut Painter, title: &str, hint: &str) {
    let x = LIST.center().x;
    p.text(title, &HINT, Point::new(x, 100.0), HAlign::Center, VAlign::Center, palette::WHITE);
    p.text(hint, &AGE, Point::new(x, 118.0), HAlign::Center, VAlign::Center, palette::GRAY);
}

fn list(p: &mut Painter, m: &Model) {
    let history = m.clipboard;
    p.hit(Hit::ClipList, LIST);
    let scroll = m.clipboard_scroll.clamp(0.0, history.max_scroll());
    let now_local = m.clock;
    p.push_clip(&Rect::new(LIST.x0 - 2.0, LIST.y0, LIST.x1 + 2.0, LIST.y1));
    let mut y = LIST.y0 - scroll;
    for entry in history.ordered() {
        let row = Rect::new(LIST.x0, y, LIST.x1, y + ROW_HEIGHT);
        y += ROW_HEIGHT + ROW_GAP;
        if row.y1 < LIST.y0 || row.y0 > LIST.y1 {
            continue;
        }
        let id = Hit::ClipItem(entry.id);
        let pin_id = Hit::ClipPin(entry.id);
        let hover = p.hover(id).max(p.hover(pin_id));
        let copied = history.copied.is_some_and(|(copied, until)| copied == entry.id && m.now < until);
        let base = if entry.pinned { m.accent.multiply_alpha(0.16) } else { rgba(0xFFFFFF, 0.07) };
        let tint = if copied { palette::GREEN.multiply_alpha(0.28) } else { mix(base, rgba(0xFFFFFF, 0.16), hover) };
        p.fill(&capsule(row), tint);

        let pin = Point::new(row.x0 + 14.0, row.center().y);
        let pin_hover = p.hover(pin_id);
        let pin_color = if entry.pinned { m.accent } else { rgba(0xFFFFFF, 0.28 + 0.5 * pin_hover) };
        p.icon(Icon::Pin, pin, 12.0, pin_color);
        p.hit(pin_id, Rect::from_center_size(pin, (24.0, ROW_HEIGHT)));

        let label = if copied { "Copied".to_string() } else { age(entry.at, now_local) };
        let right = p.text(&label, &AGE, Point::new(row.x1 - 12.0, row.center().y), HAlign::Right, VAlign::Center, if copied { palette::GREEN } else { palette::GRAY });
        let text_x = row.x0 + 28.0;
        let max_w = (right.x0 - 10.0 - text_x).max(20.0) as f32;
        p.text(&preview(&entry.text), &ENTRY.max_width(max_w), Point::new(text_x, row.center().y), HAlign::Left, VAlign::Center, palette::WHITE);
        p.hit(id, Rect::new(text_x - 4.0, row.y0, row.x1, row.y1));
    }
    p.pop_clip();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_dedupes() {
        let now = Local::now();
        let mut history = ClipboardHistory::default();
        assert!(history.record("hello", now));
        assert!(history.record("world", now));
        assert!(!history.record("world", now));
        assert!(history.record("hello", now));
        assert_eq!(history.data.entries[0].text, "hello");
        assert_eq!(history.data.entries.len(), 2);
        assert!(!history.record("   ", now));
        let id = history.data.entries[1].id;
        assert!(history.toggle_pin(id));
        history.clear();
        assert_eq!(history.data.entries.len(), 1);
        assert!(history.data.entries[0].pinned);
    }

    #[test]
    fn keeps_recent_limit() {
        let now = Local::now();
        let mut history = ClipboardHistory::default();
        for i in 0..50 {
            history.record(&format!("entry {i}"), now);
        }
        assert_eq!(history.data.entries.len(), MAX_RECENT);
    }
}
