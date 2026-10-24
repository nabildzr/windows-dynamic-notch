//! Headphones popup: Bluetooth audio device name and battery level.

use vello::kurbo::{Circle, Point, Rect};
use vello::peniko::Gradient;

use super::common::progress_ring;
use super::{HAlign, Hit, Mode, Model, Painter, VAlign, palette, rgb, rgba};
use crate::renderer::icons::Icon;
use crate::renderer::text::TextStyle;

pub const NAME: TextStyle = TextStyle::new(15.5, TextStyle::BOLD);
pub const STATUS: TextStyle = TextStyle::new(12.5, TextStyle::REGULAR).tabular();

pub fn draw(p: &mut Painter, m: &Model) {
    let size = Mode::Headphones.size();
    let cy = size.height / 2.0;
    let ring = Point::new(38.0, cy);
    p.fill(&Circle::new(ring, 13.0), rgb(0x0B1B2B));
    let (name, status, level) = match m.headphones {
        Some(device) => {
            let status = match device.battery {
                Some(battery) => format!("Connected \u{00B7} {battery}%"),
                None => "Connected".to_string(),
            };
            (device.name.clone(), status, device.battery.map_or(1.0, |b| b as f64 / 100.0))
        }
        None => ("No headphones".to_string(), "Bluetooth audio not connected".to_string(), 0.0),
    };
    progress_ring(p, ring, 15.5, 3.6, level, palette::BLUE);
    p.icon(Icon::Bluetooth, ring, 15.0, palette::WHITE);

    p.text(&name, &NAME.max_width(170.0), Point::new(66.0, cy - 9.0), HAlign::Left, VAlign::Center, palette::WHITE);
    p.text(&status, &STATUS.max_width(170.0), Point::new(66.0, cy + 10.0), HAlign::Left, VAlign::Center, palette::GRAY);

    // Headphone badge with a dark blue rim.
    let badge = Point::new(size.width - 40.0, cy);
    p.glow(Rect::from_center_size(badge, (40.0, 40.0)), 20.0, 6.0, rgba(0x38BDF8, 0.22));
    let fill = Gradient::new_radial(badge, 20.0).with_stops([rgb(0x10263A), rgb(0x07121D)]);
    p.fill_gradient(&Circle::new(badge, 19.0), &fill);
    p.stroke(&Circle::new(badge, 18.0), 2.0, rgb(0x1E4D72));
    p.icon(Icon::Headphones, badge, 18.0, palette::WHITE);

    p.hit(Hit::Popup, Rect::new(0.0, 0.0, size.width, size.height));
}
