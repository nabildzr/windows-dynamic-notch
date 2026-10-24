//! Building blocks shared by several modes: battery ring, sound wave,
//! status bar, navigation arrows, pagination dots and buttons.

use vello::kurbo::{Circle, Point, Rect};
use vello::peniko::Color;

use super::{HAlign, Hit, Model, Painter, VAlign, palette, rgba};
use crate::renderer::icons::Icon;
use crate::renderer::shapes::{capsule, progress_arc, ring};
use crate::renderer::text::TextStyle;
use crate::services::power::PowerStatus;

pub const STATUS_Y: f64 = 20.0;
pub const TITLE_STYLE: TextStyle = TextStyle::new(15.0, TextStyle::BOLD);

/// Linear blend between two colors.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let (ca, cb) = (a.components, b.components);
    Color::new([
        ca[0] + (cb[0] - ca[0]) * t,
        ca[1] + (cb[1] - ca[1]) * t,
        ca[2] + (cb[2] - ca[2]) * t,
        ca[3] + (cb[3] - ca[3]) * t,
    ])
}

pub fn battery_color(power: &PowerStatus) -> Color {
    let low = power.has_battery && !power.charging && !power.ac_online && power.percent.is_some_and(|p| p <= 20);
    if low { palette::RED } else { palette::GREEN }
}

/// Progress ring with a dim track of the same hue.
pub fn progress_ring(p: &mut Painter, center: Point, radius: f64, width: f64, level: f64, color: Color) {
    p.stroke(&ring(center, radius), width, color.multiply_alpha(0.22));
    if level > 0.004 {
        p.stroke(&progress_arc(center, radius, level), width, color);
    }
}

pub fn battery_ring(p: &mut Painter, center: Point, radius: f64, width: f64, power: &PowerStatus) {
    progress_ring(p, center, radius, width, power.level(), battery_color(power));
}

const WAVE_REST: [f64; 5] = [0.45, 0.8, 0.62, 0.95, 0.7];
const WAVE_FREQ: [f64; 5] = [5.3, 7.9, 6.4, 8.8, 4.6];
const WAVE_PHASE: [f64; 5] = [0.0, 1.7, 3.1, 0.9, 2.4];

fn wave_level(index: usize, time: f64) -> f64 {
    let a = (time * WAVE_FREQ[index] + WAVE_PHASE[index]).sin();
    let b = (time * WAVE_FREQ[index] * 0.53 + WAVE_PHASE[index] * 1.9).sin();
    (0.62 + 0.26 * a + 0.14 * b).clamp(0.34, 1.0)
}

/// Five vertical bars; they dance while audio plays and rest otherwise.
pub fn soundwave(p: &mut Painter, center: Point, height: f64, bar: f64, gap: f64, playing: bool, color: Color) {
    let total = 5.0 * bar + 4.0 * gap;
    let x0 = center.x - total / 2.0;
    for (i, rest) in WAVE_REST.iter().enumerate() {
        let level = if playing { wave_level(i, p.time) } else { rest * 0.75 };
        let h = (height * level).max(bar);
        let x = x0 + i as f64 * (bar + gap);
        p.fill(&capsule(Rect::new(x, center.y - h / 2.0, x + bar, center.y + h / 2.0)), color);
    }
}

/// Top row of every expanded widget: title on the left, battery ring with
/// date and time in the middle, sound wave on the right.
pub fn status_bar(p: &mut Painter, m: &Model, width: f64, title: &[(&str, Color)]) {
    let y = STATUS_Y;
    let mut title_end = 20.0;
    for (text, color) in title {
        let rect = p.text(text, &TITLE_STYLE, Point::new(title_end, y), HAlign::Left, VAlign::Center, *color);
        title_end = rect.x1;
    }
    let has_title = !title.is_empty();

    let date = m.clock.format("%a %d %b %Y").to_string();
    let time = m.clock.format("%H:%M").to_string();
    let date_style = TextStyle::new(13.0, TextStyle::REGULAR).tabular();
    let time_style = TextStyle::new(13.0, TextStyle::BOLD).tabular();
    let date_w = p.measure(&date, &date_style).width;
    let time_w = p.measure(&time, &time_style).width;
    let ring_d = 14.0;
    let total = ring_d + 8.0 + date_w + 5.0 + time_w;
    let mut x = width / 2.0 - total / 2.0;
    if has_title {
        x = x.max(title_end + 14.0);
    }
    let ring_center = Point::new(x + ring_d / 2.0, y);
    battery_ring(p, ring_center, ring_d / 2.0 - 1.0, 2.2, &m.power);
    p.hit(Hit::StatusBattery, Rect::from_center_size(ring_center, (22.0, 22.0)));
    x += ring_d + 8.0;
    p.text(&date, &date_style, Point::new(x, y), HAlign::Left, VAlign::Center, palette::WHITE);
    x += date_w + 5.0;
    p.text(&time, &time_style, Point::new(x, y), HAlign::Left, VAlign::Center, palette::WHITE);

    let wave_center = Point::new(width - 30.0, y);
    let hover = p.hover(Hit::StatusWave);
    soundwave(p, wave_center, 13.0, 2.2, 2.2, m.audio_playing, mix(palette::WHITE.multiply_alpha(0.9), palette::WHITE, hover));
    p.hit(Hit::StatusWave, Rect::from_center_size(wave_center, (26.0, 22.0)));
}

/// Round dark-gray arrow button at the side of the widget.
pub fn nav_button(p: &mut Painter, id: Hit, center: Point, icon: Icon) {
    let hover = p.hover(id) as f64;
    let press = p.press(id) as f64;
    let radius = 12.0 * (1.0 + 0.08 * hover - 0.1 * press);
    p.fill(&Circle::new(center, radius), rgba(0xFFFFFF, 0.2 + 0.12 * hover as f32));
    p.icon(icon, center, 14.0, palette::WHITE);
    p.hit(id, Rect::from_center_size(center, (30.0, 30.0)));
}

/// Page indicator: the active page is a pill, the others dots. `position`
/// is the animated page index so the pill glides between dots.
pub fn pagination(p: &mut Painter, center: Point, count: usize, position: f32) {
    const DOT: f64 = 4.0;
    const ACTIVE: f64 = 16.0;
    const GAP: f64 = 4.0;
    let weights: Vec<f64> = (0..count).map(|i| (1.0 - (i as f32 - position).abs()).clamp(0.0, 1.0) as f64).collect();
    let widths: Vec<f64> = weights.iter().map(|w| DOT + (ACTIVE - DOT) * w).collect();
    let total: f64 = widths.iter().sum::<f64>() + GAP * (count.saturating_sub(1)) as f64;
    let mut x = center.x - total / 2.0;
    for (i, (width, weight)) in widths.iter().zip(&weights).enumerate() {
        let rect = Rect::new(x, center.y - DOT / 2.0, x + width, center.y + DOT / 2.0);
        let id = Hit::PageDot(i as u8);
        let color = mix(rgba(0xFFFFFF, 0.35 + 0.25 * p.hover(id)), palette::WHITE, *weight as f32);
        p.fill(&capsule(rect), color);
        p.hit(id, rect.inflate(GAP / 2.0, 6.0));
        x += width + GAP;
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ButtonStyle {
    pub fill: Color,
    pub hover_fill: Color,
    pub content: Color,
}

impl ButtonStyle {
    pub const GRAY: ButtonStyle =
        ButtonStyle { fill: rgba(0xFFFFFF, 0.13), hover_fill: rgba(0xFFFFFF, 0.22), content: palette::WHITE };

    pub const fn solid(color: Color) -> ButtonStyle {
        let c = color.components;
        ButtonStyle {
            fill: color,
            hover_fill: Color::new([c[0] + (1.0 - c[0]) * 0.12, c[1] + (1.0 - c[1]) * 0.12, c[2] + (1.0 - c[2]) * 0.12, c[3]]),
            content: palette::WHITE,
        }
    }
}

/// Pill button with an optional leading icon.
pub fn capsule_button(
    p: &mut Painter,
    id: Hit,
    rect: Rect,
    icon: Option<Icon>,
    label: &str,
    style: ButtonStyle,
    font: &TextStyle,
) {
    let hover = p.hover(id);
    let press = p.press(id) as f64;
    let rect = rect.inflate(-rect.width() * 0.03 * press, -rect.height() * 0.03 * press);
    p.fill(&capsule(rect), mix(style.fill, style.hover_fill, hover));
    let icon_size = (rect.height() * 0.5).min(15.0);
    let label_w = if label.is_empty() { 0.0 } else { p.measure(label, font).width };
    let gap = if icon.is_some() && label_w > 0.0 { 6.0 } else { 0.0 };
    let content_w = icon.map_or(0.0, |_| icon_size) + gap + label_w;
    let mut x = rect.center().x - content_w / 2.0;
    let cy = rect.center().y;
    if let Some(icon) = icon {
        p.icon(icon, Point::new(x + icon_size / 2.0, cy), icon_size, style.content);
        x += icon_size + gap;
    }
    if label_w > 0.0 {
        p.text(label, font, Point::new(x, cy), HAlign::Left, VAlign::Center, style.content);
    }
    p.hit(id, rect);
}

/// Circular icon button.
pub fn round_button(p: &mut Painter, id: Hit, center: Point, radius: f64, icon: Icon, icon_size: f64, style: ButtonStyle) {
    let hover = p.hover(id);
    let press = p.press(id) as f64;
    let r = radius * (1.0 - 0.08 * press);
    p.fill(&Circle::new(center, r), mix(style.fill, style.hover_fill, hover));
    p.icon(icon, center, icon_size, style.content);
    p.hit(id, Rect::from_center_size(center, (radius * 2.0 + 4.0, radius * 2.0 + 4.0)));
}

/// Borderless icon button with a soft hover halo (media transport).
pub fn icon_button(p: &mut Painter, id: Hit, center: Point, icon: Icon, size: f64, color: Color, enabled: bool) {
    let hover = if enabled { p.hover(id) } else { 0.0 };
    let press = if enabled { p.press(id) as f64 } else { 0.0 };
    if hover > 0.01 {
        p.fill(&Circle::new(center, size * 0.95), rgba(0xFFFFFF, 0.12 * hover));
    }
    let color = if enabled { color } else { color.multiply_alpha(0.35) };
    p.icon(icon, center, size * (1.0 - 0.12 * press), color);
    if enabled {
        p.hit(id, Rect::from_center_size(center, (size * 1.9, size * 1.9)));
    }
}

/// The most prominent vivid color of an image (album art), brightened so
/// it works as a glow on black. `None` for (nearly) gray images.
pub fn dominant_color(image: &vello::peniko::ImageData) -> Option<Color> {
    let data = image.data.data();
    // Hue buckets weighted by saturation and brightness.
    let mut buckets = [(0.0f64, 0.0f64, 0.0f64, 0.0f64); 12];
    let (pixels, _) = data.as_chunks::<4>();
    for pixel in pixels.iter().step_by(3) {
        let (r, g, b) = (pixel[0] as f64 / 255.0, pixel[1] as f64 / 255.0, pixel[2] as f64 / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        if max < 0.15 || delta < 0.08 {
            continue;
        }
        let saturation = delta / max;
        let hue = if max == r {
            ((g - b) / delta).rem_euclid(6.0)
        } else if max == g {
            (b - r) / delta + 2.0
        } else {
            (r - g) / delta + 4.0
        } / 6.0;
        let weight = saturation * saturation * max;
        let bucket = &mut buckets[((hue * 12.0) as usize).min(11)];
        bucket.0 += r * weight;
        bucket.1 += g * weight;
        bucket.2 += b * weight;
        bucket.3 += weight;
    }
    let total: f64 = buckets.iter().map(|b| b.3).sum();
    let best = buckets.iter().max_by(|a, b| a.3.total_cmp(&b.3))?;
    // Ignore images whose color is a few stray pixels.
    if best.3 <= 0.0 || best.3 < total * 0.12 || total < (data.len() / 12) as f64 * 0.02 {
        return None;
    }
    let (r, g, b) = (best.0 / best.3, best.1 / best.3, best.2 / best.3);
    let scale = 0.92 / r.max(g).max(b).max(0.01);
    let to8 = |v: f64| (v * scale).clamp(0.0, 1.0) as f32;
    Some(Color::new([to8(r), to8(g), to8(b), 1.0]))
}
