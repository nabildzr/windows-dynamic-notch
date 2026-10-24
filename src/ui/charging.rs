//! Charging popup: shown for a few seconds when the charger is plugged in
//! (or when the battery ring in the status bar is clicked).

use vello::kurbo::{Circle, Point, Rect};

use super::common::{battery_color, progress_ring};
use super::headphones::{NAME, STATUS};
use super::{HAlign, Hit, Mode, Model, Painter, VAlign, palette, rgb};
use crate::renderer::icons::Icon;
use crate::renderer::text::TextStyle;

const PERCENT: TextStyle = TextStyle::new(27.0, TextStyle::REGULAR).tabular();

pub fn draw(p: &mut Painter, m: &Model) {
    let size = Mode::Charging.size();
    let cy = size.height / 2.0;
    let power = m.power;
    let ring = Point::new(38.0, cy);
    let color = if power.ac_online { palette::GREEN } else { battery_color(&power) };
    p.fill(&Circle::new(ring, 13.0), rgb(0x0B2414));
    progress_ring(p, ring, 15.5, 3.6, power.level(), color);
    p.icon(Icon::Bolt, ring, 15.0, palette::WHITE);

    let (title, subtitle) = match (power.has_battery, power.ac_online, power.charging, power.percent) {
        (false, _, _, _) => ("Plugged in", "No battery present"),
        (true, true, _, Some(100)) => ("Charged", "Charger connected"),
        (true, true, true, _) => ("Charging", "Charger connected"),
        (true, true, false, _) => ("Plugged in", "Charger connected"),
        (true, false, _, _) => ("On battery", "Charger disconnected"),
    };
    p.text(title, &NAME, Point::new(66.0, cy - 9.0), HAlign::Left, VAlign::Center, palette::WHITE);
    p.text(subtitle, &STATUS, Point::new(66.0, cy + 10.0), HAlign::Left, VAlign::Center, palette::GRAY);
    let percent = match (power.has_battery, power.percent) {
        (true, Some(percent)) => format!("{percent}%"),
        _ => "AC".to_string(),
    };
    p.text(&percent, &PERCENT, Point::new(size.width - 22.0, cy), HAlign::Right, VAlign::Center, palette::WHITE);
    p.hit(Hit::Popup, Rect::new(0.0, 0.0, size.width, size.height));
}
