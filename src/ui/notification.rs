//! Popup for a notification from another app: icon, app name, title and
//! the first line of the message.

use vello::kurbo::{Point, Rect};

use super::{HAlign, Mode, Model, Painter, VAlign, palette, rgba};
use crate::renderer::shapes::squircle;
use crate::renderer::text::TextStyle;

const APP: TextStyle = TextStyle::new(11.0, TextStyle::MEDIUM);
const TITLE: TextStyle = TextStyle::new(13.5, TextStyle::BOLD);
const BODY: TextStyle = TextStyle::new(12.0, TextStyle::REGULAR);
const INITIAL: TextStyle = TextStyle::new(20.0, TextStyle::BOLD);

pub fn draw(p: &mut Painter, m: &Model) {
    let Some(notification) = m.notification else { return };
    let size = Mode::Notification.size();
    let icon = Rect::new(16.0, 16.0, 60.0, 60.0);
    match &notification.icon {
        Some(image) => p.fill_image(&squircle(icon, 12.0), image, icon),
        None => {
            p.fill(&squircle(icon, 12.0), m.accent.multiply_alpha(0.85));
            let initial = notification.app.chars().next().unwrap_or('?').to_uppercase().to_string();
            p.text(&initial, &INITIAL, icon.center(), HAlign::Center, VAlign::Center, palette::WHITE);
        }
    }
    let x = 72.0;
    let max = (size.width - x - 18.0) as f32;
    p.text(&notification.app, &APP.max_width(max), Point::new(x, 22.0), HAlign::Left, VAlign::Center, palette::GRAY);
    p.text(&notification.title, &TITLE.max_width(max), Point::new(x, 39.0), HAlign::Left, VAlign::Center, palette::WHITE);
    if !notification.body.is_empty() {
        p.text(&notification.body, &BODY.max_width(max), Point::new(x, 56.0), HAlign::Left, VAlign::Center, rgba(0xFFFFFF, 0.78));
    }
}
