//! Water reminder: today's intake against a goal, an hourly chart and
//! optional periodic reminders.

use std::time::{Duration, Instant};

use chrono::{DateTime, Local, NaiveDate, Timelike};
use serde::{Deserialize, Serialize};
use vello::kurbo::{Circle, Line, Point, Rect, Stroke};
use vello::peniko::Gradient;

use super::common::{ButtonStyle, capsule_button, mix, round_button, status_bar};
use super::{HAlign, Hit, Mode, Model, Painter, VAlign, page_navigation, palette, rgb, rgba};
use crate::renderer::icons::Icon;
use crate::renderer::shapes::capsule;
use crate::renderer::text::TextStyle;

const AMOUNT: TextStyle = TextStyle::new(30.0, TextStyle::BOLD).tabular();
const UNIT: TextStyle = TextStyle::new(14.0, TextStyle::MEDIUM).tabular();
const SMALL: TextStyle = TextStyle::new(12.5, TextStyle::REGULAR).tabular();
const SMALL_BOLD: TextStyle = TextStyle::new(12.5, TextStyle::BOLD).tabular();
const AXIS: TextStyle = TextStyle::new(10.5, TextStyle::REGULAR).tabular();
const BUTTON: TextStyle = TextStyle::new(13.0, TextStyle::MEDIUM);
const KEEP_DAYS: i64 = 14;
const CHART_HOURS: u32 = 9;
const DRINK_BLUE: vello::peniko::Color = rgb(0x1D5FD1);

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct WaterEntry {
    pub at: DateTime<Local>,
    pub ml: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WaterData {
    pub goal_ml: u32,
    pub cup_ml: u32,
    pub reminder: bool,
    pub interval_minutes: u32,
    pub log: Vec<WaterEntry>,
}

impl Default for WaterData {
    fn default() -> Self {
        Self { goal_ml: 1500, cup_ml: 250, reminder: true, interval_minutes: 60, log: Vec::new() }
    }
}

pub struct WaterTracker {
    pub data: WaterData,
    pub settings_open: bool,
    last_prompt: Instant,
}

impl WaterTracker {
    pub fn new(data: WaterData, now: Instant) -> Self {
        let mut tracker = Self { data, settings_open: false, last_prompt: now };
        tracker.prune(Local::now().date_naive());
        tracker
    }

    pub fn today_ml(&self, today: NaiveDate) -> u32 {
        self.data.log.iter().filter(|e| e.at.date_naive() == today).map(|e| e.ml).sum()
    }

    pub fn drink(&mut self, now: DateTime<Local>, instant: Instant) {
        self.data.log.push(WaterEntry { at: now, ml: self.data.cup_ml });
        self.last_prompt = instant;
    }

    pub fn undo(&mut self, today: NaiveDate) -> bool {
        match self.data.log.iter().rposition(|e| e.at.date_naive() == today) {
            Some(index) => {
                self.data.log.remove(index);
                true
            }
            None => false,
        }
    }

    pub fn prune(&mut self, today: NaiveDate) {
        self.data.log.retain(|e| (today - e.at.date_naive()).num_days() < KEEP_DAYS);
    }

    pub fn toggle_reminder(&mut self, instant: Instant) {
        self.data.reminder = !self.data.reminder;
        self.last_prompt = instant;
    }

    pub fn adjust_goal(&mut self, steps: i32) {
        self.data.goal_ml = (self.data.goal_ml as i32 + steps * 250).clamp(500, 5000) as u32;
    }

    pub fn adjust_cup(&mut self, steps: i32) {
        self.data.cup_ml = (self.data.cup_ml as i32 + steps * 50).clamp(50, 1000) as u32;
    }

    pub fn adjust_interval(&mut self, steps: i32) {
        self.data.interval_minutes = (self.data.interval_minutes as i32 + steps * 15).clamp(15, 240) as u32;
    }

    /// Milliliters per hour for the chart window ending at the current hour.
    pub fn hourly(&self, now: DateTime<Local>) -> (u32, Vec<u32>) {
        let today = now.date_naive();
        let start = now.hour().saturating_sub(CHART_HOURS - 1).min(24 - CHART_HOURS);
        let mut buckets = vec![0u32; CHART_HOURS as usize];
        for entry in self.data.log.iter().filter(|e| e.at.date_naive() == today) {
            let hour = entry.at.hour();
            if (start..start + CHART_HOURS).contains(&hour) {
                buckets[(hour - start) as usize] += entry.ml;
            }
        }
        (start, buckets)
    }

    /// When the next reminder is due (only while the goal is not reached,
    /// between 07:00 and 22:00).
    pub fn next_reminder(&self, instant: Instant, now: DateTime<Local>) -> Option<Instant> {
        if !self.data.reminder || self.today_ml(now.date_naive()) >= self.data.goal_ml {
            return None;
        }
        let interval = Duration::from_secs(self.data.interval_minutes as u64 * 60);
        let last_drink = self.data.log.last().and_then(|e| (now - e.at).to_std().ok()).and_then(|age| instant.checked_sub(age));
        let anchor = last_drink.map_or(self.last_prompt, |drink| drink.max(self.last_prompt));
        let due = anchor + interval;
        let hour = now.hour();
        if (7..22).contains(&hour) {
            Some(due.max(instant))
        } else {
            // Wait until 07:00.
            let hours_left = if hour >= 22 { 24 - hour + 7 } else { 7 - hour };
            let wake = instant + Duration::from_secs((hours_left * 3600) as u64 - (now.minute() * 60) as u64);
            Some(due.max(wake))
        }
    }

    pub fn mark_reminded(&mut self, instant: Instant) {
        self.last_prompt = instant;
    }
}

fn liters(ml: u32) -> String {
    let liters = ml as f64 / 1000.0;
    if ml.is_multiple_of(100) { format!("{liters:.1}") } else { format!("{liters:.2}") }
}

pub fn draw(p: &mut Painter, m: &Model) {
    let width = Mode::Water.size().width;
    status_bar(p, m, width, &[("Water", palette::WHITE), (" Reminder", palette::BLUE)]);
    page_navigation(p, m, width, 86.0);
    if m.water.settings_open {
        settings(p, m);
    } else {
        summary(p, m);
    }
    p.fill(&Rect::new(256.0, 42.0, 257.0, 146.0), palette::DIVIDER);
    chart(p, m);
    super::page_dots(p, m, width);
}

fn summary(p: &mut Painter, m: &Model) {
    let water = m.water;
    let today = m.clock.date_naive();
    let drank = water.today_ml(today);
    let goal = water.data.goal_ml.max(1);
    let percent = ((drank as f64 / goal as f64) * 100.0).round() as u32;

    let amount = p.text(&liters(drank), &AMOUNT, Point::new(52.0, 58.0), HAlign::Left, VAlign::Center, palette::WHITE);
    p.text(
        &format!("/ {} L", liters(goal)),
        &UNIT,
        Point::new(amount.x1 + 6.0, 62.0),
        HAlign::Left,
        VAlign::Center,
        rgba(0xFFFFFF, 0.72),
    );

    // Reminder toggle with a bell in the knob.
    let on = water.data.reminder;
    let toggle = Rect::new(204.0, 47.0, 246.0, 71.0);
    let hover = p.hover(Hit::WaterToggle);
    let track = if on { mix(palette::BLUE_DEEP, rgb(0x3B9BFF), hover) } else { mix(rgba(0xFFFFFF, 0.2), rgba(0xFFFFFF, 0.28), hover) };
    p.fill(&capsule(toggle), track);
    let knob = Point::new(if on { toggle.x1 - 12.0 } else { toggle.x0 + 12.0 }, toggle.center().y);
    p.fill(&Circle::new(knob, 10.0), palette::WHITE);
    p.icon(if on { Icon::Bell } else { Icon::BellOff }, knob, 11.0, if on { palette::BLUE_DEEP } else { palette::GRAY });
    p.hit(Hit::WaterToggle, toggle.inflate(3.0, 3.0));

    let y = 88.0;
    let mut x = p.text("Today \u{00B7} ", &SMALL, Point::new(52.0, y), HAlign::Left, VAlign::Center, palette::GRAY).x1;
    x = p.text(&format!("{percent}%"), &SMALL_BOLD, Point::new(x, y), HAlign::Left, VAlign::Center, palette::BLUE).x1;
    let status = if drank >= goal { " \u{00B7} Goal reached".to_string() } else { format!(" \u{00B7} {} ml to go", goal - drank) };
    p.text(&status, &SMALL, Point::new(x, y), HAlign::Left, VAlign::Center, palette::GRAY);

    let bar = Rect::new(52.0, 102.0, 246.0, 107.0);
    p.fill(&capsule(bar), rgba(0xFFFFFF, 0.14));
    let fraction = (drank as f64 / goal as f64).clamp(0.0, 1.0);
    if fraction > 0.0 {
        let filled = Rect::new(bar.x0, bar.y0, bar.x0 + bar.width() * fraction, bar.y1);
        let gradient = Gradient::new_linear((bar.x0, 0.0), (bar.x1, 0.0)).with_stops([palette::CYAN, palette::BLUE_DEEP]);
        p.fill_gradient(&capsule(filled), &gradient);
    }

    capsule_button(
        p,
        Hit::WaterDrink,
        Rect::new(52.0, 120.0, 134.0, 146.0),
        Some(Icon::Droplet),
        "Drink",
        ButtonStyle::solid(DRINK_BLUE),
        &BUTTON,
    );
    capsule_button(
        p,
        Hit::WaterSettings,
        Rect::new(140.0, 120.0, 246.0, 146.0),
        Some(Icon::Sliders),
        "Settings",
        ButtonStyle::GRAY,
        &BUTTON,
    );
}

fn settings(p: &mut Painter, m: &Model) {
    let data = &m.water.data;
    let rows = [
        ("Daily goal", format!("{} L", liters(data.goal_ml)), Hit::WaterGoalDec, Hit::WaterGoalInc),
        ("Cup size", format!("{} ml", data.cup_ml), Hit::WaterCupDec, Hit::WaterCupInc),
        ("Remind every", format!("{} min", data.interval_minutes), Hit::WaterIntervalDec, Hit::WaterIntervalInc),
    ];
    for (i, (label, value, dec, inc)) in rows.iter().enumerate() {
        let y = 50.0 + i as f64 * 25.0;
        p.text(label, &SMALL, Point::new(52.0, y), HAlign::Left, VAlign::Center, palette::GRAY);
        round_button(p, *dec, Point::new(150.0, y), 9.5, Icon::Minus, 11.0, ButtonStyle::GRAY);
        p.text(value, &SMALL_BOLD, Point::new(192.0, y), HAlign::Center, VAlign::Center, palette::WHITE);
        round_button(p, *inc, Point::new(234.0, y), 9.5, Icon::Plus, 11.0, ButtonStyle::GRAY);
    }
    capsule_button(
        p,
        Hit::WaterDone,
        Rect::new(52.0, 122.0, 124.0, 146.0),
        Some(Icon::Check),
        "Done",
        ButtonStyle::solid(DRINK_BLUE),
        &BUTTON,
    );
    capsule_button(
        p,
        Hit::WaterUndo,
        Rect::new(130.0, 122.0, 246.0, 146.0),
        Some(Icon::Undo),
        "Undo drink",
        ButtonStyle::GRAY,
        &BUTTON,
    );
}

fn chart(p: &mut Painter, m: &Model) {
    let (start, buckets) = m.water.hourly(m.clock);
    let peak = buckets.iter().copied().max().unwrap_or(0);
    let scale_max = (peak.div_ceil(200) * 200).max(400) as f64;
    let (x0, top, bottom) = (266.0, 50.0, 122.0);
    let (bar_w, gap) = (5.6, 3.2);
    let x_end = x0 + CHART_HOURS as f64 * (bar_w + gap) - gap;

    // Dashed grid lines at the top and half of the scale.
    let dash = Stroke::new(1.0).with_dashes(0.0, [2.0, 3.0]);
    for fraction in [1.0, 0.5] {
        let y = bottom - (bottom - top) * fraction;
        p.stroke_with(&Line::new((x0 - 2.0, y), (x_end + 2.0, y)), &dash, rgba(0xFFFFFF, 0.18));
        let label = format!("{}", (scale_max * fraction) as u32);
        p.text(&label, &AXIS, Point::new(x_end + 8.0, y), HAlign::Left, VAlign::Center, palette::GRAY);
    }
    p.text("0 ml", &AXIS, Point::new(x_end + 8.0, bottom), HAlign::Left, VAlign::Center, palette::GRAY);

    let current_hour = m.clock.hour();
    for (i, ml) in buckets.iter().enumerate() {
        let x = x0 + i as f64 * (bar_w + gap);
        p.fill(&capsule(Rect::new(x, top, x + bar_w, bottom)), rgba(0xFFFFFF, 0.08));
        if *ml > 0 {
            let height = ((bottom - top) * (*ml as f64 / scale_max)).max(bar_w);
            let bar = Rect::new(x, bottom - height, x + bar_w, bottom);
            let gradient = Gradient::new_linear((0.0, bar.y0), (0.0, bar.y1)).with_stops([rgb(0x5AC8FA), palette::BLUE_DEEP]);
            p.fill_gradient(&capsule(bar), &gradient);
        }
        let hour = start + i as u32;
        if i % 3 == 0 {
            let color = if hour == current_hour { palette::WHITE } else { palette::GRAY };
            p.text(&format!("{hour:02}"), &AXIS, Point::new(x + bar_w / 2.0, 136.0), HAlign::Center, VAlign::Center, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn hourly_buckets() {
        let now = Local.with_ymd_and_hms(2026, 10, 24, 15, 30, 0).unwrap();
        let mut tracker = WaterTracker::new(WaterData::default(), Instant::now());
        tracker.data.log.push(WaterEntry { at: Local.with_ymd_and_hms(2026, 10, 24, 9, 5, 0).unwrap(), ml: 250 });
        tracker.data.log.push(WaterEntry { at: Local.with_ymd_and_hms(2026, 10, 24, 15, 0, 0).unwrap(), ml: 300 });
        tracker.data.log.push(WaterEntry { at: Local.with_ymd_and_hms(2026, 10, 23, 15, 0, 0).unwrap(), ml: 999 });
        let (start, buckets) = tracker.hourly(now);
        assert_eq!(start, 7);
        assert_eq!(buckets[2], 250);
        assert_eq!(buckets[8], 300);
        assert_eq!(tracker.today_ml(now.date_naive()), 550);
    }
}
