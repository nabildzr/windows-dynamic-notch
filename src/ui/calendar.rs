//! Calendar widget: today's date on the left, a browsable month grid on the
//! right (Monday first).

use chrono::{Datelike, Months, NaiveDate};
use vello::kurbo::{Circle, Point, Rect};

use super::common::{icon_button, mix, status_bar};
use super::{HAlign, Hit, Mode, Model, Painter, VAlign, page_navigation, palette, rgba};
use crate::renderer::icons::Icon;
use crate::renderer::text::TextStyle;

const WEEKDAY: TextStyle = TextStyle::new(13.0, TextStyle::SEMIBOLD);
const DAY: TextStyle = TextStyle::new(46.0, TextStyle::BOLD).tabular();
const MONTH: TextStyle = TextStyle::new(13.0, TextStyle::MEDIUM);
const SMALL: TextStyle = TextStyle::new(11.0, TextStyle::REGULAR).tabular();
const HEADER: TextStyle = TextStyle::new(12.5, TextStyle::BOLD);
const LETTER: TextStyle = TextStyle::new(10.0, TextStyle::SEMIBOLD);
const CELL: TextStyle = TextStyle::new(11.0, TextStyle::MEDIUM).tabular();
const CELL_TODAY: TextStyle = TextStyle::new(11.0, TextStyle::BOLD).tabular();

const GRID_X: f64 = 210.0;
const COLUMN: f64 = 22.5;
const ROW: f64 = 13.0;
const GRID_Y: f64 = 81.0;

/// First day of the month `offset` months away from `today`'s month.
pub fn shown_month(today: NaiveDate, offset: i32) -> NaiveDate {
    let first = today.with_day(1).unwrap_or(today);
    if offset >= 0 {
        first.checked_add_months(Months::new(offset as u32)).unwrap_or(first)
    } else {
        first.checked_sub_months(Months::new(offset.unsigned_abs())).unwrap_or(first)
    }
}

fn days_in_month(first: NaiveDate) -> u32 {
    first.checked_add_months(Months::new(1)).map_or(30, |next| (next - first).num_days() as u32)
}

pub fn draw(p: &mut Painter, m: &Model) {
    let width = Mode::Calendar.size().width;
    status_bar(p, m, width, &[("Calendar", palette::WHITE)]);
    let today = m.clock.date_naive();

    // Today.
    let x = 52.0;
    p.text(&today.format("%A").to_string(), &WEEKDAY, Point::new(x, 54.0), HAlign::Left, VAlign::Center, palette::RED);
    p.text(&today.day().to_string(), &DAY, Point::new(x - 2.0, 88.0), HAlign::Left, VAlign::Center, palette::WHITE);
    p.text(&today.format("%B %Y").to_string(), &MONTH, Point::new(x, 120.0), HAlign::Left, VAlign::Center, palette::WHITE);
    let year_days = if today.leap_year() { 366.0 } else { 365.0 };
    let info = format!("Week {} \u{00B7} Day {}", today.iso_week().week(), today.ordinal());
    p.text(&info, &SMALL, Point::new(x, 137.0), HAlign::Left, VAlign::Center, palette::GRAY);
    // Year progress.
    let bar = Rect::new(x, 146.0, 186.0, 148.5);
    p.fill(&crate::renderer::shapes::capsule(bar), rgba(0xFFFFFF, 0.12));
    let done = Rect::new(bar.x0, bar.y0, bar.x0 + bar.width() * today.ordinal() as f64 / year_days, bar.y1);
    p.fill(&crate::renderer::shapes::capsule(done), palette::RED.multiply_alpha(0.85));

    p.fill(&Rect::new(198.0, 42.0, 199.0, 150.0), palette::DIVIDER);

    // Month grid.
    let first = shown_month(today, m.calendar_offset);
    let current = m.calendar_offset == 0;
    let header = first.format("%B %Y").to_string();
    let hover = p.hover(Hit::CalendarToday);
    let color = if current { palette::WHITE } else { mix(palette::BLUE, palette::WHITE, hover) };
    let label = p.text(&header, &HEADER, Point::new(GRID_X, 50.0), HAlign::Left, VAlign::Center, color);
    if !current {
        // Clicking the month name jumps back to today.
        p.hit(Hit::CalendarToday, label.inflate(4.0, 4.0));
    }
    icon_button(p, Hit::CalendarPrev, Point::new(338.0, 50.0), Icon::ChevronLeft, 14.0, palette::WHITE, true);
    icon_button(p, Hit::CalendarNext, Point::new(360.0, 50.0), Icon::ChevronRight, 14.0, palette::WHITE, true);

    let column_x = |c: usize| GRID_X + COLUMN * c as f64 + COLUMN / 2.0 - 3.0;
    for (c, letter) in ["M", "T", "W", "T", "F", "S", "S"].iter().enumerate() {
        let tint = if c >= 5 { rgba(0xFFFFFF, 0.38) } else { palette::GRAY };
        p.text(letter, &LETTER, Point::new(column_x(c), 67.0), HAlign::Center, VAlign::Center, tint);
    }
    let lead = first.weekday().num_days_from_monday() as usize;
    for day in 1..=days_in_month(first) {
        let slot = lead + day as usize - 1;
        let (row, col) = (slot / 7, slot % 7);
        let center = Point::new(column_x(col), GRID_Y + ROW * row as f64);
        let is_today = current && day == today.day();
        if is_today {
            p.fill(&Circle::new(center, 7.6), palette::RED);
        }
        let (style, tint) = if is_today {
            (&CELL_TODAY, palette::WHITE)
        } else if col >= 5 {
            (&CELL, rgba(0xFFFFFF, 0.45))
        } else {
            (&CELL, rgba(0xFFFFFF, 0.88))
        };
        p.text(&day.to_string(), style, center, HAlign::Center, VAlign::Center, tint);
    }

    page_navigation(p, m, width, 86.0);
    super::page_dots(p, m, width);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_offsets() {
        let today = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        assert_eq!(shown_month(today, 0), NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
        assert_eq!(shown_month(today, 1), NaiveDate::from_ymd_opt(2026, 2, 1).unwrap());
        assert_eq!(shown_month(today, -1), NaiveDate::from_ymd_opt(2025, 12, 1).unwrap());
        assert_eq!(days_in_month(NaiveDate::from_ymd_opt(2028, 2, 1).unwrap()), 29);
    }
}
