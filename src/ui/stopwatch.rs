//! Stopwatch with laps, and a free countdown timer (any length from 10
//! seconds to 12 hours) next to the Pomodoro widget.

use std::time::{Duration, Instant};

use vello::kurbo::{Point, Rect};

use super::common::{ButtonStyle, capsule_button, mix, progress_ring, round_button, status_bar};
use super::{HAlign, Hit, Mode, Model, Painter, VAlign, page_navigation, palette, rgb, rgba};
use crate::renderer::icons::Icon;
use crate::renderer::shapes::capsule;
use crate::renderer::text::TextStyle;

const DIGITS: TextStyle = TextStyle::new(38.0, TextStyle::SEMIBOLD).tabular();
const FRACTION: TextStyle = TextStyle::new(22.0, TextStyle::MEDIUM).tabular();
const SEGMENT: TextStyle = TextStyle::new(11.5, TextStyle::SEMIBOLD);
const BUTTON: TextStyle = TextStyle::new(12.0, TextStyle::SEMIBOLD);
const LAP: TextStyle = TextStyle::new(11.5, TextStyle::MEDIUM).tabular();
const LABEL: TextStyle = TextStyle::new(11.5, TextStyle::REGULAR);

const MAX_LAPS: usize = 99;
const MIN_COUNTDOWN: Duration = Duration::from_secs(10);
const MAX_COUNTDOWN: Duration = Duration::from_secs(12 * 3600);
const AMBER: vello::peniko::Color = rgb(0xFF9F0A);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockKind {
    Stopwatch,
    Countdown,
}

pub struct Stopwatch {
    pub kind: ClockKind,
    started: Option<Instant>,
    accumulated: Duration,
    pub laps: Vec<Duration>,
    pub countdown: Duration,
    countdown_started: Option<Instant>,
    countdown_left: Duration,
}

impl Default for Stopwatch {
    fn default() -> Self {
        Self {
            kind: ClockKind::Stopwatch,
            started: None,
            accumulated: Duration::ZERO,
            laps: Vec::new(),
            countdown: Duration::from_secs(5 * 60),
            countdown_started: None,
            countdown_left: Duration::from_secs(5 * 60),
        }
    }
}

impl Stopwatch {
    pub fn elapsed(&self, now: Instant) -> Duration {
        self.accumulated + self.started.map_or(Duration::ZERO, |s| now.saturating_duration_since(s))
    }

    pub fn stopwatch_running(&self) -> bool {
        self.started.is_some()
    }

    pub fn countdown_running(&self) -> bool {
        self.countdown_started.is_some()
    }

    pub fn running(&self) -> bool {
        self.stopwatch_running() || self.countdown_running()
    }

    pub fn remaining(&self, now: Instant) -> Duration {
        match self.countdown_started {
            Some(start) => self.countdown_left.saturating_sub(now.saturating_duration_since(start)),
            None => self.countdown_left,
        }
    }

    pub fn toggle(&mut self, now: Instant) {
        match self.kind {
            ClockKind::Stopwatch => match self.started.take() {
                Some(start) => self.accumulated += now.saturating_duration_since(start),
                None => self.started = Some(now),
            },
            ClockKind::Countdown => match self.countdown_started.take() {
                Some(start) => self.countdown_left = self.countdown_left.saturating_sub(now.saturating_duration_since(start)),
                None => {
                    if self.countdown_left.is_zero() {
                        self.countdown_left = self.countdown;
                    }
                    self.countdown_started = Some(now);
                }
            },
        }
    }

    /// Lap while running, reset while paused.
    pub fn lap_or_reset(&mut self, now: Instant) {
        match self.kind {
            ClockKind::Stopwatch if self.stopwatch_running() => {
                if self.laps.len() < MAX_LAPS {
                    self.laps.push(self.elapsed(now));
                }
            }
            ClockKind::Stopwatch => {
                self.accumulated = Duration::ZERO;
                self.laps.clear();
            }
            ClockKind::Countdown => {
                self.countdown_started = None;
                self.countdown_left = self.countdown;
            }
        }
    }

    /// Changes the countdown length by `minutes` (only while it is not
    /// counting). Returns `true` when changed.
    pub fn adjust(&mut self, seconds: i64) -> bool {
        if self.countdown_running() {
            return false;
        }
        let current = self.countdown.as_secs() as i64;
        let step_base = if current + seconds < 60 { 10 } else { 60 };
        let target = ((current + seconds) / step_base * step_base).max(1) as u64;
        let target = Duration::from_secs(target).clamp(MIN_COUNTDOWN, MAX_COUNTDOWN);
        if target == self.countdown {
            return false;
        }
        self.countdown = target;
        self.countdown_left = target;
        true
    }

    /// Returns `true` once when the countdown reaches zero.
    pub fn update(&mut self, now: Instant) -> bool {
        if self.countdown_running() && self.remaining(now).is_zero() {
            self.countdown_started = None;
            self.countdown_left = Duration::ZERO;
            return true;
        }
        false
    }

    pub fn countdown_end(&self) -> Option<Instant> {
        self.countdown_started.map(|start| start + self.countdown_left)
    }
}

/// "12:34" plus ".56" hundredths (or "1:02:03" for long runs).
fn split_time(duration: Duration, hundredths: bool) -> (String, String) {
    let total = duration.as_secs();
    let main = if total >= 3600 {
        format!("{}:{:02}:{:02}", total / 3600, (total / 60) % 60, total % 60)
    } else {
        format!("{:02}:{:02}", total / 60, total % 60)
    };
    let fraction = if hundredths && total < 3600 { format!(".{:02}", duration.subsec_millis() / 10) } else { String::new() };
    (main, fraction)
}

pub fn draw(p: &mut Painter, m: &Model) {
    let width = Mode::Stopwatch.size().width;
    let watch = m.stopwatch;
    status_bar(p, m, width, &[("Stopwatch", palette::WHITE)]);
    page_navigation(p, m, width, 86.0);

    // Segmented control: Stopwatch | Timer.
    let segment = Rect::new(52.0, 42.0, 200.0, 64.0);
    p.fill(&capsule(segment), rgba(0xFFFFFF, 0.10));
    let half = segment.width() / 2.0;
    let active = if watch.kind == ClockKind::Stopwatch { 0.0 } else { 1.0 };
    let knob = Rect::new(segment.x0 + 2.0 + half * active, segment.y0 + 2.0, segment.x0 + half * (active + 1.0) - 2.0, segment.y1 - 2.0);
    p.fill(&capsule(knob), rgba(0xFFFFFF, 0.22));
    for (i, (label, id)) in [("Stopwatch", Hit::SwModeStopwatch), ("Timer", Hit::SwModeTimer)].into_iter().enumerate() {
        let rect = Rect::new(segment.x0 + half * i as f64, segment.y0, segment.x0 + half * (i + 1) as f64, segment.y1);
        let selected = (i as f64 - active).abs() < 0.5;
        let color = if selected { palette::WHITE } else { mix(palette::GRAY, palette::WHITE, p.hover(id)) };
        p.text(label, &SEGMENT, rect.center(), HAlign::Center, VAlign::Center, color);
        p.hit(id, rect);
    }

    match watch.kind {
        ClockKind::Stopwatch => stopwatch(p, m),
        ClockKind::Countdown => countdown(p, m),
    }
    super::page_dots(p, m, width);
}

fn digits(p: &mut Painter, main: &str, fraction: &str, x: f64, y: f64, color: vello::peniko::Color) {
    let rect = p.text(main, &DIGITS, Point::new(x, y), HAlign::Left, VAlign::Center, color);
    if !fraction.is_empty() {
        p.text(fraction, &FRACTION, Point::new(rect.x1 + 1.0, y + 4.0), HAlign::Left, VAlign::Center, color.multiply_alpha(0.7));
    }
}

fn stopwatch(p: &mut Painter, m: &Model) {
    let watch = m.stopwatch;
    let elapsed = watch.elapsed(m.now);
    let (main, fraction) = split_time(elapsed, true);
    digits(p, &main, &fraction, 54.0, 92.0, palette::WHITE);

    let running = watch.stopwatch_running();
    let (left_label, left_icon) = if running { ("Lap", Icon::Flag) } else { ("Reset", Icon::Reset) };
    let enabled = running || !elapsed.is_zero();
    p.with_alpha(if enabled { 1.0 } else { 0.4 }, |p| {
        capsule_button(p, Hit::SwLapReset, Rect::new(52.0, 118.0, 132.0, 144.0), Some(left_icon), left_label, ButtonStyle::GRAY, &BUTTON);
    });
    let style = if running { ButtonStyle::solid(AMBER) } else { ButtonStyle::solid(palette::GREEN) };
    round_button(p, Hit::SwStartPause, Point::new(160.0, 131.0), 14.0, if running { Icon::Pause } else { Icon::Play }, 13.0, style);

    // Laps, newest first.
    p.fill(&Rect::new(222.0, 42.0, 223.0, 146.0), palette::DIVIDER);
    if watch.laps.is_empty() {
        p.text("Laps appear here", &LABEL, Point::new(296.0, 94.0), HAlign::Center, VAlign::Center, palette::GRAY);
        return;
    }
    let splits: Vec<Duration> = watch.laps.iter().scan(Duration::ZERO, |previous, total| {
        let split = *total - *previous;
        *previous = *total;
        Some(split)
    }).collect();
    let fastest = splits.iter().min().copied();
    let slowest = splits.iter().max().copied();
    for (row, index) in (0..splits.len()).rev().take(5).enumerate() {
        let y = 52.0 + row as f64 * 19.0;
        let split = splits[index];
        let color = if splits.len() > 2 && Some(split) == fastest {
            palette::GREEN
        } else if splits.len() > 2 && Some(split) == slowest {
            palette::RED
        } else {
            palette::WHITE
        };
        p.text(&format!("Lap {}", index + 1), &LAP, Point::new(234.0, y), HAlign::Left, VAlign::Center, palette::GRAY);
        let (main, fraction) = split_time(split, true);
        p.text(&format!("{main}{fraction}"), &LAP, Point::new(364.0, y), HAlign::Right, VAlign::Center, color);
    }
}

fn countdown(p: &mut Painter, m: &Model) {
    let watch = m.stopwatch;
    let remaining = watch.remaining(m.now);
    let running = watch.countdown_running();
    // Round up so "00:01" is shown during the last second.
    let shown = if running { Duration::from_secs(remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0)) } else { remaining };
    let (main, _) = split_time(shown, false);
    let color = if remaining.is_zero() { AMBER } else { palette::WHITE };
    digits(p, &main, "", 54.0, 92.0, color);
    let digits_rect = Rect::new(50.0, 70.0, 210.0, 112.0);
    p.hit(Hit::SwDigits, digits_rect);

    let paused = !running && remaining < watch.countdown && !remaining.is_zero();
    capsule_button(p, Hit::SwLapReset, Rect::new(52.0, 118.0, 132.0, 144.0), Some(Icon::Reset), "Reset", ButtonStyle::GRAY, &BUTTON);
    let style = if running { ButtonStyle::solid(AMBER) } else { ButtonStyle::solid(palette::GREEN) };
    round_button(p, Hit::SwStartPause, Point::new(160.0, 131.0), 14.0, if running { Icon::Pause } else { Icon::Play }, 13.0, style);

    // Ring with +/- minute buttons.
    p.fill(&Rect::new(222.0, 42.0, 223.0, 146.0), palette::DIVIDER);
    let center = Point::new(296.0, 90.0);
    let total = watch.countdown.as_secs_f64().max(1.0);
    let fraction = if running || paused { remaining.as_secs_f64() / total } else { 1.0 };
    p.fill(&vello::kurbo::Circle::new(center, 30.0), AMBER.multiply_alpha(0.08));
    progress_ring(p, center, 33.0, 5.0, fraction, AMBER);
    let (length, _) = split_time(watch.countdown, false);
    p.text(&length, &LAP, center, HAlign::Center, VAlign::Center, palette::WHITE);
    let adjustable = !running;
    p.with_alpha(if adjustable { 1.0 } else { 0.35 }, |p| {
        round_button(p, Hit::SwMinus, Point::new(244.0, 90.0), 11.0, Icon::Minus, 11.0, ButtonStyle::GRAY);
        round_button(p, Hit::SwPlus, Point::new(348.0, 90.0), 11.0, Icon::Plus, 11.0, ButtonStyle::GRAY);
    });
    p.text(
        if adjustable { "Scroll the digits to adjust" } else { "Counting down" },
        &LABEL,
        Point::new(296.0, 140.0),
        HAlign::Center,
        VAlign::Center,
        palette::GRAY,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopwatch_laps_and_reset() {
        let start = Instant::now();
        let mut watch = Stopwatch::default();
        watch.toggle(start);
        watch.lap_or_reset(start + Duration::from_secs(3));
        watch.toggle(start + Duration::from_secs(5));
        assert_eq!(watch.elapsed(start + Duration::from_secs(9)), Duration::from_secs(5));
        assert_eq!(watch.laps, vec![Duration::from_secs(3)]);
        watch.lap_or_reset(start);
        assert!(watch.laps.is_empty());
        assert_eq!(watch.elapsed(start), Duration::ZERO);
    }

    #[test]
    fn countdown_finishes_once() {
        let start = Instant::now();
        let mut watch = Stopwatch { kind: ClockKind::Countdown, ..Default::default() };
        assert!(watch.adjust(-4 * 60));
        assert_eq!(watch.countdown, Duration::from_secs(60));
        watch.toggle(start);
        assert!(!watch.update(start + Duration::from_secs(30)));
        assert!(watch.update(start + Duration::from_secs(61)));
        assert!(!watch.update(start + Duration::from_secs(62)));
    }

    #[test]
    fn formats_time() {
        assert_eq!(split_time(Duration::from_millis(83_450), true), ("01:23".into(), ".45".into()));
        assert_eq!(split_time(Duration::from_secs(3723), true), ("1:02:03".into(), String::new()));
    }
}
