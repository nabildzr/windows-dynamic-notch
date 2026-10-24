//! Focus timer (Pomodoro): 25 minutes of focus, then a short break.
//!
//! The countdown is driven by an end `Instant`, so it stays exact no matter
//! how rarely frames are rendered; the UI only wakes when the displayed
//! second changes.

use std::time::{Duration, Instant};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use vello::kurbo::{Circle, Point, Rect};
use vello::peniko::Gradient;

use super::common::{ButtonStyle, capsule_button, progress_ring, round_button, status_bar};
use super::{HAlign, Hit, Mode, Model, Painter, VAlign, page_navigation, palette, rgb, rgba};
use crate::renderer::icons::Icon;
use crate::renderer::text::TextStyle;

const DIGITS: TextStyle = TextStyle::new(32.0, TextStyle::BOLD).tabular();
const LABEL: TextStyle = TextStyle::new(13.0, TextStyle::REGULAR);
const BUTTON: TextStyle = TextStyle::new(12.5, TextStyle::MEDIUM);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    #[default]
    Focus,
    ShortBreak,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PomodoroData {
    pub focus_minutes: u32,
    pub break_minutes: u32,
    pub completed_on: Option<NaiveDate>,
    pub completed_today: u32,
}

impl Default for PomodoroData {
    fn default() -> Self {
        Self { focus_minutes: 25, break_minutes: 5, completed_on: None, completed_today: 0 }
    }
}

pub struct Pomodoro {
    pub data: PomodoroData,
    pub phase: Phase,
    remaining: Duration,
    ends_at: Option<Instant>,
}

impl Pomodoro {
    pub fn new(data: PomodoroData) -> Self {
        let mut timer = Self { data, phase: Phase::Focus, remaining: Duration::ZERO, ends_at: None };
        timer.remaining = timer.length(Phase::Focus);
        timer
    }

    pub fn length(&self, phase: Phase) -> Duration {
        let minutes = match phase {
            Phase::Focus => self.data.focus_minutes,
            Phase::ShortBreak => self.data.break_minutes,
        };
        Duration::from_secs(minutes.max(1) as u64 * 60)
    }

    pub fn running(&self) -> bool {
        self.ends_at.is_some()
    }

    pub fn remaining(&self, now: Instant) -> Duration {
        match self.ends_at {
            Some(end) => end.saturating_duration_since(now),
            None => self.remaining,
        }
    }

    /// Fraction of the phase still to go (1 = full ring).
    pub fn progress(&self, now: Instant) -> f64 {
        self.remaining(now).as_secs_f64() / self.length(self.phase).as_secs_f64()
    }

    pub fn display(&self, now: Instant) -> String {
        let secs = self.remaining(now).as_secs_f64().ceil() as u64;
        format!("{:02}:{:02}", secs / 60, secs % 60)
    }

    pub fn toggle(&mut self, now: Instant) {
        match self.ends_at.take() {
            Some(end) => self.remaining = end.saturating_duration_since(now),
            None => {
                if self.remaining.is_zero() {
                    self.remaining = self.length(self.phase);
                }
                self.ends_at = Some(now + self.remaining);
            }
        }
    }

    pub fn reset(&mut self) {
        self.ends_at = None;
        self.remaining = self.length(self.phase);
    }

    pub fn select(&mut self, phase: Phase) {
        self.phase = phase;
        self.reset();
    }

    /// Changes the length of the current phase while it is not running.
    pub fn adjust_minutes(&mut self, delta: i32) -> bool {
        if self.running() {
            return false;
        }
        let (minutes, max) = match self.phase {
            Phase::Focus => (&mut self.data.focus_minutes, 120),
            Phase::ShortBreak => (&mut self.data.break_minutes, 60),
        };
        let next = (*minutes as i32 + delta).clamp(1, max) as u32;
        if next == *minutes {
            return false;
        }
        *minutes = next;
        self.reset();
        true
    }

    /// Finishes the phase when its time is up; returns the completed phase.
    pub fn update(&mut self, now: Instant, today: NaiveDate) -> Option<Phase> {
        let end = self.ends_at?;
        if now < end {
            return None;
        }
        let completed = self.phase;
        if completed == Phase::Focus {
            if self.data.completed_on != Some(today) {
                self.data.completed_on = Some(today);
                self.data.completed_today = 0;
            }
            self.data.completed_today += 1;
        }
        self.phase = match completed {
            Phase::Focus => Phase::ShortBreak,
            Phase::ShortBreak => Phase::Focus,
        };
        self.reset();
        Some(completed)
    }

    /// Moment the phase ends, if running.
    pub fn end(&self) -> Option<Instant> {
        self.ends_at
    }

    /// Next moment the displayed seconds change.
    pub fn next_tick(&self, now: Instant) -> Option<Instant> {
        let end = self.ends_at?;
        let left = end.saturating_duration_since(now);
        let into_second = Duration::from_nanos(left.subsec_nanos() as u64);
        Some(now + into_second + Duration::from_millis(4))
    }
}

pub fn draw(p: &mut Painter, m: &Model) {
    let width = Mode::Timer.size().width;
    let timer = m.timer;
    status_bar(p, m, width, &[("Pomodoro", palette::WHITE)]);
    page_navigation(p, m, width, 86.0);

    // Glowing red progress ring with a stopwatch.
    let ring = Point::new(84.0, 86.0);
    p.glow(Rect::from_center_size(ring, (64.0, 64.0)), 32.0, 9.0, palette::RED.multiply_alpha(0.32));
    let inner = Gradient::new_radial(ring, 28.0).with_stops([rgb(0x4A0E0B), rgb(0x1A0504)]);
    p.fill_gradient(&Circle::new(ring, 27.0), &inner);
    progress_ring(p, ring, 30.0, 6.0, timer.progress(m.now), palette::RED);
    p.icon(Icon::Stopwatch, ring, 28.0, palette::WHITE);

    let digits_center = Point::new(258.0, 74.0);
    let digits = timer.display(m.now);
    let rect = p.text(&digits, &DIGITS, digits_center, HAlign::Center, VAlign::Center, palette::WHITE);
    p.hit(Hit::TimerDigits, rect.inflate(4.0, 4.0));
    round_button(p, Hit::TimerReset, Point::new(rect.x1 + 24.0, digits_center.y - 1.0), 13.0, Icon::Reset, 14.0, ButtonStyle::GRAY);
    let label = match timer.phase {
        Phase::Focus => "Focus time",
        Phase::ShortBreak => "Short break",
    };
    p.text(label, &LABEL, Point::new(digits_center.x, 101.0), HAlign::Center, VAlign::Center, palette::GRAY);

    let row_y = 134.0;
    capsule_button(
        p,
        Hit::TimerBreak,
        Rect::new(132.0, row_y - 13.0, 236.0, row_y + 13.0),
        Some(Icon::Coffee),
        "Short Break",
        selected_style(timer.phase == Phase::ShortBreak),
        &BUTTON,
    );
    capsule_button(
        p,
        Hit::TimerPlayPause,
        Rect::new(242.0, row_y - 13.0, 296.0, row_y + 13.0),
        Some(if timer.running() { Icon::Pause } else { Icon::Play }),
        "",
        ButtonStyle::solid(palette::RED),
        &BUTTON,
    );
    capsule_button(
        p,
        Hit::TimerFocus,
        Rect::new(302.0, row_y - 13.0, 404.0, row_y + 13.0),
        Some(Icon::Target),
        "Focus Time",
        selected_style(timer.phase == Phase::Focus),
        &BUTTON,
    );
    super::page_dots(p, m, width);
}

fn selected_style(selected: bool) -> ButtonStyle {
    if selected {
        ButtonStyle { fill: rgba(0xFFFFFF, 0.2), hover_fill: rgba(0xFFFFFF, 0.28), content: palette::WHITE }
    } else {
        ButtonStyle::GRAY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_down_and_switches_phase() {
        let mut timer = Pomodoro::new(PomodoroData::default());
        let start = Instant::now();
        let today = NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
        timer.toggle(start);
        assert_eq!(timer.display(start), "25:00");
        assert_eq!(timer.display(start + Duration::from_millis(1500)), "24:59");
        assert!(timer.update(start + Duration::from_secs(60), today).is_none());
        assert_eq!(timer.update(start + Duration::from_secs(25 * 60), today), Some(Phase::Focus));
        assert_eq!(timer.phase, Phase::ShortBreak);
        assert!(!timer.running());
        assert_eq!(timer.data.completed_today, 1);
    }

    #[test]
    fn pause_keeps_remaining_time() {
        let mut timer = Pomodoro::new(PomodoroData::default());
        let start = Instant::now();
        timer.toggle(start);
        timer.toggle(start + Duration::from_secs(90));
        assert_eq!(timer.display(start + Duration::from_secs(500)), "23:30");
    }
}
