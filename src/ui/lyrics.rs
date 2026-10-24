//! Lyrics mode: compact player on the left, live karaoke lines from LRCLIB
//! on the right. The sung line is bright, bold and slightly larger; the
//! others are dimmed and glide upward as the song progresses. The sung line
//! glows softly and fills from left to right (karaoke) as it is sung.

use std::sync::Arc;
use std::time::{Duration, Instant};

use vello::kurbo::{Affine, Circle, Point, Rect};
use vello::peniko::{Color, Gradient};

use super::common::icon_button;
use super::music::{self, SUBTITLE};
use super::{HAlign, Hit, Mode, Model, Painter, VAlign, palette, rgba};
use crate::animation::{SpringConfig, SpringValue};
use crate::renderer::icons::Icon;
use crate::renderer::text::{TextLayout, TextStyle};
use crate::services::lyrics::{LyricLine, LyricsResult};
use crate::services::media::MediaSnapshot;

const TITLE: TextStyle = TextStyle::new(15.0, TextStyle::BOLD);
const LINE: TextStyle = TextStyle::new(15.0, TextStyle::BOLD).wrapped(226.0, Some(3)).line_height(1.18);
const MESSAGE: TextStyle = TextStyle::new(13.5, TextStyle::MEDIUM);
/// Top of the current line inside the notch.
const ANCHOR_Y: f64 = 66.0;
const GAP: f64 = 9.0;
/// Halo color of the sung line.
const GLOW: Color = Color::from_rgb8(0x9F, 0xC8, 0xFF);
/// Highlight a line slightly before its timestamp so it feels on time.
const LEAD: Duration = Duration::from_millis(150);

#[derive(Clone, Debug)]
pub enum LyricsStatus {
    Idle,
    Loading,
    Synced(Arc<Vec<LyricLine>>),
    Plain(Arc<Vec<String>>),
    Instrumental,
    NotFound,
    Unavailable,
}

pub struct LyricsState {
    /// Track the status belongs to.
    pub key: Option<String>,
    pub status: LyricsStatus,
    /// Animated (fractional) index of the current line.
    pub scroll: SpringValue,
    pub requested_at: Option<Instant>,
}

impl Default for LyricsState {
    fn default() -> Self {
        Self {
            key: None,
            status: LyricsStatus::Idle,
            scroll: SpringValue::normalized(0.0, SpringConfig::GENTLE),
            requested_at: None,
        }
    }
}

impl LyricsState {
    pub fn apply(&mut self, key: String, result: LyricsResult) {
        if self.key.as_deref() != Some(key.as_str()) {
            return;
        }
        self.status = match result {
            LyricsResult::Synced(lines) => LyricsStatus::Synced(lines),
            LyricsResult::Plain(lines) => LyricsStatus::Plain(lines),
            LyricsResult::Instrumental => LyricsStatus::Instrumental,
            LyricsResult::NotFound => LyricsStatus::NotFound,
            LyricsResult::Unavailable => LyricsStatus::Unavailable,
        };
        self.requested_at = None;
    }

    /// Starts tracking a new track; returns `true` when lyrics should be
    /// requested.
    pub fn begin(&mut self, key: &str, now: Instant) -> bool {
        if self.key.as_deref() == Some(key) {
            // Retry after an offline failure, at most once a minute.
            let retry = matches!(self.status, LyricsStatus::Unavailable)
                && self.requested_at.is_none_or(|at| now.duration_since(at) > Duration::from_secs(60));
            if retry {
                self.status = LyricsStatus::Loading;
                self.requested_at = Some(now);
            }
            return retry;
        }
        self.key = Some(key.to_string());
        self.status = LyricsStatus::Loading;
        self.requested_at = Some(now);
        self.scroll.snap(-1.0);
        true
    }

    /// Fractional line index the scroller should glide to.
    pub fn target(&self, media: Option<&MediaSnapshot>, now: Instant) -> f32 {
        let Some(media) = media else { return 0.0 };
        let position = media.position_at(now) + LEAD;
        match &self.status {
            LyricsStatus::Synced(lines) => match lines.partition_point(|line| line.time <= position) {
                0 => -0.7,
                n => (n - 1) as f32,
            },
            LyricsStatus::Plain(lines) if media.duration > Duration::ZERO && !lines.is_empty() => {
                let fraction = position.as_secs_f64() / media.duration.as_secs_f64();
                (fraction * (lines.len() - 1) as f64) as f32
            }
            _ => 0.0,
        }
    }

    /// Index of the sung line and how far into it the song is (0..1, until
    /// the next line starts), for the karaoke sweep.
    pub fn line_progress(&self, media: &MediaSnapshot, now: Instant) -> Option<(usize, f32)> {
        let LyricsStatus::Synced(lines) = &self.status else { return None };
        let position = media.position_at(now) + LEAD;
        let index = lines.partition_point(|line| line.time <= position).checked_sub(1)?;
        let start = lines[index].time;
        let end = lines.get(index + 1).map_or(start + Duration::from_secs(5), |next| next.time);
        let span = end.saturating_sub(start).as_secs_f32().max(0.3);
        // The sweep finishes a little before the next line so it reads as sung.
        let progress = ((position - start).as_secs_f32() / (span * 0.85)).clamp(0.0, 1.0);
        Some((index, progress))
    }
    /// When the next synced line starts (for frame scheduling).
    pub fn next_change(&self, media: &MediaSnapshot, now: Instant) -> Option<Instant> {
        let LyricsStatus::Synced(lines) = &self.status else { return None };
        if !media.playing {
            return None;
        }
        let position = media.position_at(now) + LEAD;
        let next = lines.iter().find(|line| line.time > position)?;
        let wait = (next.time - position).div_f64(media.rate.max(0.1));
        Some(now + wait)
    }
}

pub fn draw(p: &mut Painter, m: &Model) {
    let size = Mode::Lyrics.size();
    ambient(p, m);

    music::artwork(p, m, Rect::new(16.0, 16.0, 72.0, 72.0), 12.0);
    let (title, subtitle) = music::titles(m);
    p.text(&title, &TITLE.max_width(158.0), Point::new(86.0, 36.0), HAlign::Left, VAlign::Center, palette::WHITE);
    p.text(&subtitle, &SUBTITLE.max_width(158.0), Point::new(86.0, 54.0), HAlign::Left, VAlign::Center, palette::GRAY);
    music::progress_row(p, m, 16.0, 244.0, 100.0);
    icon_button(p, Hit::MediaLyrics, Point::new(32.0, 136.0), Icon::LyricsFilled, 19.0, palette::WHITE, true);
    music::transport(p, m, Point::new(142.0, 136.0), 46.0, 20.0);

    let divider = Gradient::new_linear((0.0, 22.0), (0.0, 153.0)).with_stops([
        (0.0, Color::TRANSPARENT),
        (0.5, palette::DIVIDER),
        (1.0, Color::TRANSPARENT),
    ]);
    p.fill_gradient(&Rect::new(256.0, 22.0, 257.0, 153.0), &divider);
    let area = Rect::new(270.0, 0.0, size.width - 12.0, size.height);
    if m.media.is_none() {
        message(p, area, "Play a song to see its lyrics");
        return;
    }
    match &m.lyrics.status {
        LyricsStatus::Synced(lines) => {
            let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
            scroller(p, m, &texts, area);
        }
        LyricsStatus::Plain(lines) => {
            let texts: Vec<&str> = lines.iter().map(String::as_str).collect();
            scroller(p, m, &texts, area);
        }
        LyricsStatus::Idle | LyricsStatus::Loading => {
            dots(p, Point::new(area.center().x, 80.0), 1.0, p.time);
            message_at(p, area, 104.0, "Searching lyrics\u{2026}");
        }
        LyricsStatus::Instrumental => message(p, area, "\u{266A}  Instrumental"),
        LyricsStatus::NotFound => message(p, area, "No lyrics found for this track"),
        LyricsStatus::Unavailable => message(p, area, "Lyrics unavailable right now"),
    }
}

/// Slowly drifting pools of colored light behind the lyrics; they breathe
/// with the music while it plays.
fn ambient(p: &mut Painter, m: &Model) {
    let t = if m.audio_playing { p.time } else { 0.0 };
    let drift = |speed: f64, phase: f64, amount: f64| (t * speed + phase).sin() * amount;
    let pulse = 0.85 + 0.15 * (t * 2.1).sin() as f32;
    let a = Point::new(470.0 + drift(0.31, 0.0, 60.0), 175.0 + drift(0.23, 1.3, 18.0));
    // Album colors when available, the blue/violet theme otherwise.
    let (deep, mid, bright) = match m.album {
        Some(c) => (c.multiply_alpha(0.62), super::common::mix(c, m.accent2, 0.35).multiply_alpha(0.34), super::common::mix(c, Color::WHITE, 0.3).multiply_alpha(0.22)),
        None => (rgba(0x1E3A8A, 0.85), rgba(0x7C3AED, 0.30), rgba(0x06B6D4, 0.22)),
    };
    p.glow(Rect::from_center_size(a, (300.0, 120.0)), 80.0, 46.0, deep.multiply_alpha(pulse));
    let b = Point::new(400.0 + drift(0.27, 2.1, 70.0), 150.0 + drift(0.37, 0.4, 14.0));
    p.glow(Rect::from_center_size(b, (170.0, 70.0)), 50.0, 34.0, mid.multiply_alpha(pulse));
    let c = Point::new(520.0 + drift(0.19, 4.0, 40.0), 160.0 + drift(0.29, 2.6, 12.0));
    p.glow(Rect::from_center_size(c, (150.0, 60.0)), 40.0, 30.0, bright.multiply_alpha(pulse));
}

fn message(p: &mut Painter, area: Rect, text: &str) {
    message_at(p, area, 88.0, text);
}

fn message_at(p: &mut Painter, area: Rect, y: f64, text: &str) {
    p.text(text, &MESSAGE, Point::new(area.center().x, y), HAlign::Center, VAlign::Center, palette::GRAY);
}

fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Three dots that swell one after another (intro, interludes, loading).
fn dots(p: &mut Painter, center: Point, emphasis: f64, time: f64) {
    for i in 0..3 {
        let phase = (time * 3.2 - i as f64 * 0.55).sin() * 0.5 + 0.5;
        let radius = 2.6 + 1.4 * phase * emphasis;
        let alpha = (0.35 + 0.65 * emphasis * (0.45 + 0.55 * phase)) as f32;
        let at = Point::new(center.x + (i as f64 - 1.0) * 13.0, center.y);
        if emphasis > 0.2 {
            p.fill(&Circle::new(at, radius + 3.0), GLOW.multiply_alpha(0.18 * alpha * emphasis as f32));
        }
        p.fill(&Circle::new(at, radius), palette::WHITE.multiply_alpha(alpha));
    }
}

/// Soft glow around a text layout: a halo plus two rings of faint copies.
fn glow_text(p: &mut Painter, layout: &TextLayout, strength: f32, glow: Color) {
    let w = layout.width as f64;
    let h = layout.height as f64;
    p.glow(Rect::new(-4.0, 2.0, w + 4.0, h - 2.0), 12.0, 10.0, glow.multiply_alpha(0.22 * strength));
    for (radius, alpha) in [(1.4, 0.16), (2.8, 0.07)] {
        for k in 0..8 {
            let angle = k as f64 * std::f64::consts::FRAC_PI_4;
            let offset = Point::new(angle.cos() * radius, angle.sin() * radius);
            p.draw_layout(layout, offset, glow.multiply_alpha(alpha * strength));
        }
    }
}

/// The sung line: dim base, bright fill sweeping left to right row by row.
fn karaoke(p: &mut Painter, layout: &TextLayout, progress: f32, alpha: f32) {
    p.draw_layout(layout, Point::ZERO, palette::WHITE.multiply_alpha(0.38 * alpha));
    let rows = layout.lines.max(1);
    let row_h = layout.height as f64 / rows as f64;
    let w = layout.width.max(1.0);
    let soft = 22.0 / w;
    for row in 0..rows {
        let fill = (progress * rows as f32 - row as f32).clamp(0.0, 1.0);
        if fill <= 0.0 {
            break;
        }
        let edge = fill * (1.0 + soft);
        let gradient = Gradient::new_linear((0.0, 0.0), (w as f64, 0.0)).with_stops([
            (0.0, palette::WHITE.multiply_alpha(alpha)),
            ((edge - soft).clamp(0.0, 0.999), palette::WHITE.multiply_alpha(alpha)),
            (edge.clamp(0.001, 1.0), Color::TRANSPARENT),
            (1.0, Color::TRANSPARENT),
        ]);
        let band = Rect::new(-6.0, row as f64 * row_h, w as f64 + 6.0, (row + 1) as f64 * row_h + 2.0);
        p.push_clip(&band);
        p.draw_layout_gradient(layout, Point::ZERO, &gradient);
        p.pop_clip();
    }
}

fn scroller(p: &mut Painter, m: &Model, lines: &[&str], area: Rect) {
    if lines.is_empty() {
        return;
    }
    let scroll = m.lyrics.scroll.value() as f64;
    let target = m.lyrics.scroll.target() as f64;
    let karaoke_line = m.media.and_then(|media| m.lyrics.line_progress(media, m.now));
    let center = scroll.round().clamp(0.0, (lines.len() - 1) as f64) as usize;
    let first = center.saturating_sub(5);
    let last = (center + 6).min(lines.len());
    let mut layouts = Vec::with_capacity(last - first);
    for text in &lines[first..last] {
        let blank = text.trim().is_empty();
        layouts.push(if blank { None } else { p.layout(text, &LINE) });
    }
    let heights: Vec<f64> = layouts.iter().map(|l| l.as_ref().map_or(22.0, |l| l.height as f64)).collect();
    let mut tops = Vec::with_capacity(heights.len() + 1);
    tops.push(0.0);
    for height in &heights {
        tops.push(tops.last().copied().unwrap_or(0.0) + height + GAP);
    }
    // Interpolate the anchor between the two lines around `scroll`.
    let base = scroll.floor();
    let frac = scroll - base;
    let index_top = |index: f64| -> f64 {
        let local = index - first as f64;
        if local < 0.0 {
            local * (heights.first().copied().unwrap_or(18.0) + GAP)
        } else {
            let i = (local as usize).min(tops.len() - 1);
            tops[i]
        }
    };
    let anchor = index_top(base) + (index_top(base + 1.0) - index_top(base)) * frac;
    let offset = ANCHOR_Y - anchor;
    // While gliding to a new line the others trail behind elastically,
    // more the farther away they are.
    let lag = (target - scroll).clamp(-1.0, 1.0);
    let breath = 0.8 + 0.2 * (p.time * 2.4).sin() as f32;
    let glow = m.album.map_or(GLOW, |c| super::common::mix(c, Color::WHITE, 0.45));

    if scroll < -0.2 {
        // Before the first line: intro dots in place of the coming lyrics.
        let emphasis = smoothstep(-0.2, -0.7, scroll);
        dots(p, Point::new(area.x0 + 20.0, ANCHOR_Y + 9.0), emphasis, p.time);
    }

    for (k, layout) in layouts.iter().enumerate() {
        let index = (first + k) as f64;
        let distance = index - scroll;
        let y = tops[k] + offset + lag * distance * 7.0;
        let mid = y + heights[k] / 2.0;
        if y > area.y1 + 4.0 || y + heights[k] < area.y0 - 4.0 {
            continue;
        }
        let emphasis = (1.0 - distance.abs()).clamp(0.0, 1.0);
        let fade = smoothstep(area.y0 - 6.0, area.y0 + 30.0, mid) * (1.0 - smoothstep(area.y1 - 34.0, area.y1 + 4.0, mid));
        let alpha = ((0.26 + 0.74 * emphasis) * fade) as f32;
        if alpha <= 0.01 {
            continue;
        }
        // Lines further away shrink a little and drift right, like depth.
        let zoom = 0.94 + 0.10 * emphasis;
        let indent = (1.0 - emphasis) * 4.0;
        let Some(layout) = layout.clone() else {
            p.with_transform(Affine::translate((area.x0 + indent, y)), |p| {
                dots(p, Point::new(20.0, heights[k] / 2.0), emphasis * fade, p.time);
            });
            continue;
        };
        let sung = karaoke_line.filter(|(line, _)| *line == first + k).map(|(_, progress)| progress);
        p.with_transform(Affine::translate((area.x0 + indent, y)) * Affine::scale(zoom), |p| {
            if emphasis > 0.05 {
                glow_text(p, &layout, emphasis as f32 * fade as f32 * breath, glow);
            }
            match sung {
                Some(progress) if emphasis > 0.5 => karaoke(p, &layout, progress, alpha),
                _ => p.draw_layout(&layout, Point::ZERO, palette::WHITE.multiply_alpha(alpha)),
            }
        });
    }
}