//! Music mode: media controller for whatever is playing on the PC.

use std::time::Duration;

use vello::kurbo::{Circle, Point, Rect};
use vello::peniko::Gradient;

use super::common::{icon_button, status_bar};
use super::{HAlign, Hit, Mode, Model, Painter, VAlign, page_navigation, palette, rgb, rgba};
use crate::renderer::icons::Icon;
use crate::renderer::shapes::{capsule, squircle};
use crate::renderer::text::TextStyle;
use crate::services::media::format_time;

const ART: Rect = Rect::new(16.0, 14.0, 72.0, 70.0);
pub const TITLE: TextStyle = TextStyle::new(16.0, TextStyle::BOLD);
pub const SUBTITLE: TextStyle = TextStyle::new(12.5, TextStyle::REGULAR);
const TIME: TextStyle = TextStyle::new(13.0, TextStyle::MEDIUM).tabular();

pub fn draw(p: &mut Painter, m: &Model) {
    let width = Mode::Music.size().width;
    if let Some(color) = m.album {
        // Light from the album art pooling under the player.
        p.glow(Rect::new(-20.0, 120.0, 200.0, 230.0), 70.0, 44.0, color.multiply_alpha(0.42));
        p.glow(Rect::new(10.0, 10.0, 80.0, 74.0), 18.0, 16.0, color.multiply_alpha(0.35));
    }
    status_bar(p, m, width, &[]);
    artwork(p, m, ART, 12.0);
    let (title, subtitle) = titles(m);
    p.text(&title, &TITLE.max_width(270.0), Point::new(86.0, 45.0), HAlign::Left, VAlign::Center, palette::WHITE);
    p.text(&subtitle, &SUBTITLE.max_width(270.0), Point::new(86.0, 63.0), HAlign::Left, VAlign::Center, palette::GRAY);
    page_navigation(p, m, width, 56.0);
    progress_row(p, m, 16.0, width - 16.0, 96.0);
    icon_button(p, Hit::MediaLyrics, Point::new(32.0, 128.0), Icon::Lyrics, 19.0, palette::WHITE, true);
    transport(p, m, Point::new(width / 2.0, 128.0), 56.0, 22.0);
    super::page_dots(p, m, width);
}

/// Title and "artist - album" (or the empty state).
pub fn titles(m: &Model) -> (String, String) {
    match m.media {
        Some(media) => {
            let title = if media.title.is_empty() { "Unknown title".to_string() } else { media.title.clone() };
            (title, media.subtitle())
        }
        None => ("Nothing playing".to_string(), "Play music in any app".to_string()),
    }
}

/// Album art with continuous corners, or a gradient placeholder.
pub fn artwork(p: &mut Painter, m: &Model, rect: Rect, radius: f64) {
    let shape = squircle(rect, radius);
    match m.artwork {
        Some(image) => p.fill_image(&shape, image, rect),
        None => {
            let gradient = Gradient::new_linear((rect.x0, rect.y0), (rect.x1, rect.y1))
                .with_stops([rgb(0x3B82F6), rgb(0x8B5CF6)]);
            p.fill_gradient(&shape, &gradient);
            p.icon(Icon::MusicNote, rect.center(), rect.width() * 0.46, rgba(0xFFFFFF, 0.92));
        }
    }
}

/// Elapsed time, scrubber and remaining time on one row.
pub fn progress_row(p: &mut Painter, m: &Model, x0: f64, x1: f64, y: f64) {
    let (position, duration, can_seek) = match m.media {
        Some(media) => {
            let position = match m.scrub {
                Some(fraction) => media.duration.mul_f64(fraction.clamp(0.0, 1.0)),
                None => media.position_at(m.now),
            };
            (position, media.duration, media.can_seek)
        }
        None => (Duration::ZERO, Duration::ZERO, false),
    };
    let known = duration > Duration::ZERO;
    let elapsed = if known { format_time(position) } else { "--:--".to_string() };
    let remaining = if known { format!("-{}", format_time(duration.saturating_sub(position))) } else { "--:--".to_string() };
    let left = p.text(&elapsed, &TIME, Point::new(x0, y), HAlign::Left, VAlign::Center, palette::WHITE);
    let right = p.text(&remaining, &TIME, Point::new(x1, y), HAlign::Right, VAlign::Center, palette::WHITE);

    let bar_x0 = left.x1.max(x0 + 38.0) + 12.0;
    let bar_x1 = right.x0.min(x1 - 44.0) - 12.0;
    let hover = p.hover(Hit::MediaScrubber).max(if m.scrub.is_some() { 1.0 } else { 0.0 }) as f64;
    let thickness = 4.0 + 2.0 * hover;
    let track = Rect::new(bar_x0, y - thickness / 2.0, bar_x1, y + thickness / 2.0);
    p.fill(&capsule(track), rgba(0xFFFFFF, 0.24));
    let fraction = if known { (position.as_secs_f64() / duration.as_secs_f64()).clamp(0.0, 1.0) } else { 0.0 };
    if fraction > 0.0 {
        let filled = Rect::new(track.x0, track.y0, track.x0 + track.width() * fraction, track.y1);
        let fill = m.album.map_or(palette::WHITE, |c| super::common::mix(c, palette::WHITE, 0.45));
        p.fill(&capsule(filled), fill);
    }
    if can_seek {
        if hover > 0.02 {
            let knob = Point::new(track.x0 + track.width() * fraction, y);
            p.fill(&Circle::new(knob, 5.5 * hover), palette::WHITE);
        }
        // The hit rectangle spans exactly the bar horizontally, so the app
        // maps the cursor to a position with (x - x0) / width.
        p.hit(Hit::MediaScrubber, Rect::new(track.x0, y - 10.0, track.x1, y + 10.0));
    }
}

/// Previous / play-pause / next.
pub fn transport(p: &mut Painter, m: &Model, center: Point, spacing: f64, size: f64) {
    let media = m.media;
    let playing = media.is_some_and(|media| media.playing);
    icon_button(
        p,
        Hit::MediaPrev,
        Point::new(center.x - spacing, center.y),
        Icon::Previous,
        size * 0.82,
        palette::WHITE,
        media.is_some_and(|media| media.can_previous),
    );
    icon_button(
        p,
        Hit::MediaPlayPause,
        center,
        if playing { Icon::Pause } else { Icon::Play },
        size,
        palette::WHITE,
        media.is_some_and(|media| media.can_play_pause),
    );
    icon_button(
        p,
        Hit::MediaNext,
        Point::new(center.x + spacing, center.y),
        Icon::Next,
        size * 0.82,
        palette::WHITE,
        media.is_some_and(|media| media.can_next),
    );
}
