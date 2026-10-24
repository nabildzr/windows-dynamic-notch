//! Short indicator popups: volume and mute, screen brightness, Caps Lock /
//! Num Lock state, and the low-battery warning.

use std::time::Duration;

use vello::kurbo::{Circle, Point, Rect, RoundedRect};

use super::common::{mix, progress_ring};
use super::{HAlign, Mode, Model, Painter, VAlign, palette, rgba};
use crate::renderer::icons::Icon;
use crate::renderer::shapes::capsule;
use crate::renderer::text::TextStyle;

const NAME: TextStyle = TextStyle::new(14.0, TextStyle::BOLD);
const DETAIL: TextStyle = TextStyle::new(12.0, TextStyle::REGULAR);
const VALUE: TextStyle = TextStyle::new(13.0, TextStyle::SEMIBOLD).tabular();
const KEY: TextStyle = TextStyle::new(13.0, TextStyle::BOLD);
const PERCENT: TextStyle = TextStyle::new(20.0, TextStyle::SEMIBOLD).tabular();

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Indicator {
    Volume { level: f32, muted: bool },
    Brightness(u8),
    CapsLock(bool),
    NumLock(bool),
    LowBattery(u8),
    /// Percent used and gigabytes left on C:.
    DiskFull { percent: u8, free_gb: f32 },
}

impl Indicator {
    /// How long the popup stays up after the last change.
    pub fn duration(&self) -> Duration {
        match self {
            Indicator::LowBattery(_) | Indicator::DiskFull { .. } => Duration::from_secs(6),
            _ => Duration::from_millis(1600),
        }
    }
}

pub fn draw(p: &mut Painter, m: &Model) {
    let Some(indicator) = m.indicator else { return };
    let size = Mode::Indicator.size();
    let cy = size.height / 2.0;
    match indicator {
        Indicator::Volume { level, muted } => {
            let silent = muted || level <= 0.005;
            let label = if muted { "Muted".to_string() } else { format!("{}", (level * 100.0).round() as u32) };
            level_row(p, size.width, cy, if silent { Icon::SpeakerMuted } else { Icon::Speaker }, if muted { 0.0 } else { level }, &label);
        }
        Indicator::Brightness(level) => {
            level_row(p, size.width, cy, Icon::Sun, level as f32 / 100.0, &format!("{level}"));
        }
        Indicator::CapsLock(on) => toggle_row(p, size.width, cy, "A", "Caps Lock", on),
        Indicator::NumLock(on) => toggle_row(p, size.width, cy, "1", "Num Lock", on),
        Indicator::LowBattery(percent) => {
            let ring = Point::new(30.0, cy);
            p.fill(&Circle::new(ring, 11.0), rgba(0xFF453A, 0.18));
            progress_ring(p, ring, 13.5, 3.2, percent as f64 / 100.0, palette::RED);
            p.text("!", &KEY, ring, HAlign::Center, VAlign::Center, palette::WHITE);
            p.text("Low Battery", &NAME, Point::new(54.0, cy - 8.0), HAlign::Left, VAlign::Center, palette::WHITE);
            p.text("Connect your charger", &DETAIL, Point::new(54.0, cy + 9.0), HAlign::Left, VAlign::Center, palette::GRAY);
            p.text(&format!("{percent}%"), &PERCENT, Point::new(size.width - 20.0, cy), HAlign::Right, VAlign::Center, palette::RED);
        }
        Indicator::DiskFull { percent, free_gb } => {
            const ORANGE: vello::peniko::Color = super::rgb(0xFF9F0A);
            let ring = Point::new(30.0, cy);
            p.fill(&Circle::new(ring, 11.0), ORANGE.multiply_alpha(0.18));
            progress_ring(p, ring, 13.5, 3.2, percent as f64 / 100.0, ORANGE);
            p.text("!", &KEY, ring, HAlign::Center, VAlign::Center, palette::WHITE);
            p.text("Disk C: almost full", &NAME, Point::new(54.0, cy - 8.0), HAlign::Left, VAlign::Center, palette::WHITE);
            p.text(&format!("{free_gb:.1} GB free"), &DETAIL, Point::new(54.0, cy + 9.0), HAlign::Left, VAlign::Center, palette::GRAY);
            p.text(&format!("{percent}%"), &PERCENT, Point::new(size.width - 20.0, cy), HAlign::Right, VAlign::Center, ORANGE);
        }
    }
}

fn level_row(p: &mut Painter, width: f64, cy: f64, icon: Icon, level: f32, label: &str) {
    p.icon(icon, Point::new(30.0, cy), 20.0, palette::WHITE);
    let track = Rect::new(54.0, cy - 3.0, width - 62.0, cy + 3.0);
    p.fill(&capsule(track), rgba(0xFFFFFF, 0.2));
    let level = level.clamp(0.0, 1.0) as f64;
    if level > 0.0 {
        let filled = Rect::new(track.x0, track.y0, (track.x0 + track.width() * level).max(track.x0 + track.height()), track.y1);
        p.fill(&capsule(filled), palette::WHITE);
    }
    p.text(label, &VALUE, Point::new(width - 20.0, cy), HAlign::Right, VAlign::Center, palette::WHITE);
}

fn toggle_row(p: &mut Painter, width: f64, cy: f64, glyph: &str, name: &str, on: bool) {
    let key = Rect::from_center_size(Point::new(30.0, cy), (24.0, 24.0));
    let tint = if on { palette::GREEN } else { rgba(0xFFFFFF, 0.55) };
    p.fill(&RoundedRect::from_rect(key, 6.0), mix(rgba(0xFFFFFF, 0.08), tint.multiply_alpha(0.25), 0.6));
    p.stroke(&RoundedRect::from_rect(key.inflate(-0.75, -0.75), 5.5), 1.5, tint);
    p.text(glyph, &KEY, key.center(), HAlign::Center, VAlign::Center, palette::WHITE);
    p.text(name, &NAME, Point::new(54.0, cy), HAlign::Left, VAlign::Center, palette::WHITE);
    let state = if on { "On" } else { "Off" };
    let rect = p.text(state, &VALUE, Point::new(width - 20.0, cy), HAlign::Right, VAlign::Center, if on { palette::WHITE } else { palette::GRAY });
    p.fill(&Circle::new(Point::new(rect.x0 - 9.0, cy), 3.5), if on { palette::GREEN } else { rgba(0xFFFFFF, 0.3) });
}
