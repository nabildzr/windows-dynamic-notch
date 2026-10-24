//! Closed (compact active) mode: battery ring, clock and sound wave. While a
//! focus timer runs, the countdown takes the clock's place in red.

use vello::kurbo::Point;

use super::common::{battery_ring, soundwave};
use super::{HAlign, Mode, Model, Painter, VAlign, palette};
use crate::renderer::text::TextStyle;

const CLOCK: TextStyle = TextStyle::new(14.0, TextStyle::BOLD).tabular();

pub fn draw(p: &mut Painter, m: &Model) {
    let size = Mode::Closed.size();
    let cy = size.height / 2.0 + 0.5;
    battery_ring(p, Point::new(24.0, cy), 7.0, 2.4, &m.power);
    let (text, color) = if m.timer.running() {
        (m.timer.display(m.now), palette::RED)
    } else {
        (m.clock.format("%H:%M").to_string(), palette::WHITE)
    };
    p.text(&text, &CLOCK, Point::new(size.width / 2.0, cy), HAlign::Center, VAlign::Center, color);
    soundwave(p, Point::new(size.width - 26.0, cy), 14.0, 2.4, 2.2, m.audio_playing, palette::WHITE);
}
