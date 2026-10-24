//! Quick controls: Wi-Fi, Bluetooth, power mode and Do Not Disturb toggles
//! plus a volume slider with mute.

use vello::kurbo::{Circle, Point, Rect, RoundedRect};
use vello::peniko::Color;

use super::common::{mix, status_bar};
use super::{HAlign, Hit, Mode, Model, Painter, VAlign, page_navigation, palette, rgba};
use crate::renderer::icons::Icon;
use crate::renderer::shapes::capsule;
use crate::renderer::text::TextStyle;
use crate::services::controls::PowerMode;

const NAME: TextStyle = TextStyle::new(11.5, TextStyle::SEMIBOLD);
const STATE: TextStyle = TextStyle::new(10.5, TextStyle::REGULAR);
const VALUE: TextStyle = TextStyle::new(12.0, TextStyle::SEMIBOLD).tabular();

/// Horizontal extent of the volume track (local coordinates).
pub const VOLUME_TRACK: Rect = Rect::new(84.0, 128.0, 326.0, 134.0);

struct Tile {
    id: Hit,
    icon: Icon,
    name: &'static str,
    state: String,
    on: Option<bool>,
    color: Color,
}

pub fn draw(p: &mut Painter, m: &Model) {
    let width = Mode::Controls.size().width;
    status_bar(p, m, width, &[("Controls", palette::WHITE)]);
    page_navigation(p, m, width, 86.0);
    let c = m.controls;
    let on_off = |v: Option<bool>| match v {
        Some(true) => "On".to_string(),
        Some(false) => "Off".to_string(),
        None => "Unavailable".to_string(),
    };
    let power = c.power;
    let tiles = [
        Tile { id: Hit::CtlWifi, icon: Icon::Wifi, name: "Wi-Fi", state: on_off(c.wifi), on: c.wifi, color: palette::BLUE_DEEP },
        Tile {
            id: Hit::CtlBluetooth,
            icon: Icon::Bluetooth,
            name: "Bluetooth",
            state: on_off(c.bluetooth),
            on: c.bluetooth,
            color: palette::BLUE_DEEP,
        },
        Tile {
            id: Hit::CtlPower,
            icon: match power {
                Some(PowerMode::Efficiency) => Icon::Leaf,
                Some(PowerMode::Performance) => Icon::Gauge,
                _ => Icon::Bolt,
            },
            name: "Power",
            state: power.map_or("Unavailable".into(), |p| p.label().to_string()),
            on: power.map(|p| p != PowerMode::Balanced),
            color: match power {
                Some(PowerMode::Efficiency) => palette::GREEN,
                _ => rgba(0xFF9F0A, 1.0),
            },
        },
        Tile {
            id: Hit::CtlFocus,
            icon: Icon::BellOff,
            name: "Focus",
            state: "Do not disturb".into(),
            on: Some(false),
            color: palette::INDIGO,
        },
    ];
    let tile_w = 74.0;
    let gap = (320.0 - tile_w * 4.0) / 3.0;
    for (i, tile) in tiles.iter().enumerate() {
        let x = 50.0 + i as f64 * (tile_w + gap);
        let rect = Rect::new(x, 42.0, x + tile_w, 110.0);
        let hover = p.hover(tile.id);
        let press = p.press(tile.id) as f64;
        let rect = rect.inflate(-2.0 * press, -2.0 * press);
        let enabled = tile.on.is_some();
        p.fill(&RoundedRect::from_rect(rect, 14.0), mix(rgba(0xFFFFFF, 0.08), rgba(0xFFFFFF, 0.14), hover));
        let center = Point::new(rect.center().x, rect.y0 + 22.0);
        let active = tile.on == Some(true);
        p.fill(&Circle::new(center, 14.0), if active { tile.color } else { rgba(0xFFFFFF, 0.14) });
        p.icon(tile.icon, center, 16.0, if enabled { palette::WHITE } else { palette::GRAY });
        p.text(tile.name, &NAME.max_width(tile_w as f32 - 8.0), Point::new(rect.center().x, rect.y0 + 46.0), HAlign::Center, VAlign::Center, palette::WHITE);
        p.text(&tile.state, &STATE, Point::new(rect.center().x, rect.y0 + 59.0), HAlign::Center, VAlign::Center, palette::GRAY);
        if enabled {
            p.hit(tile.id, rect);
        }
    }

    // Volume.
    let volume = m.volume;
    let level = m.volume_preview.unwrap_or(volume.level).clamp(0.0, 1.0) as f64;
    let mute_center = Point::new(64.0, 131.0);
    let mute_hover = p.hover(Hit::CtlMute);
    p.fill(&Circle::new(mute_center, 12.0), rgba(0xFFFFFF, 0.08 + 0.1 * mute_hover));
    let silent = volume.muted || level <= 0.005;
    p.icon(if silent { Icon::SpeakerMuted } else { Icon::Speaker }, mute_center, 15.0, palette::WHITE);
    p.hit(Hit::CtlMute, Rect::from_center_size(mute_center, (26.0, 26.0)));
    let track = VOLUME_TRACK;
    let hover = p.hover(Hit::CtlVolume) as f64;
    let thickness = 6.0 + 2.0 * hover;
    let track = Rect::new(track.x0, track.center().y - thickness / 2.0, track.x1, track.center().y + thickness / 2.0);
    p.fill(&capsule(track), rgba(0xFFFFFF, 0.18));
    if level > 0.0 && !volume.muted {
        let filled = Rect::new(track.x0, track.y0, (track.x0 + track.width() * level).max(track.x0 + track.height()), track.y1);
        p.fill(&capsule(filled), m.accent);
    }
    let knob = Point::new(track.x0 + track.width() * level, track.center().y);
    p.fill(&Circle::new(knob, 6.0 + 1.5 * hover), palette::WHITE);
    p.hit(Hit::CtlVolume, Rect::new(track.x0 - 8.0, track.y0 - 10.0, track.x1 + 8.0, track.y1 + 10.0));
    let label = if volume.muted { "Muted".to_string() } else { format!("{}", (level * 100.0).round() as u32) };
    p.text(&label, &VALUE, Point::new(368.0, 131.0), HAlign::Right, VAlign::Center, palette::WHITE);
    super::page_dots(p, m, width);
}
