//! Weather widget: current conditions on the left, the next hours on the
//! right (Open-Meteo).

use vello::kurbo::{Point, Rect};
use vello::peniko::Color;

use super::common::{ButtonStyle, capsule_button, status_bar};
use super::{HAlign, Hit, Mode, Model, Painter, VAlign, page_navigation, palette, rgb, rgba};
use crate::renderer::icons::Icon;
use crate::renderer::text::TextStyle;
use crate::services::weather::describe;

const TEMP: TextStyle = TextStyle::new(40.0, TextStyle::SEMIBOLD).tabular();
const CONDITION: TextStyle = TextStyle::new(13.0, TextStyle::SEMIBOLD);
const DETAIL: TextStyle = TextStyle::new(11.0, TextStyle::REGULAR).tabular();
const HOUR: TextStyle = TextStyle::new(11.0, TextStyle::MEDIUM).tabular();
const HOUR_TEMP: TextStyle = TextStyle::new(12.5, TextStyle::SEMIBOLD).tabular();
const BUTTON: TextStyle = TextStyle::new(12.0, TextStyle::SEMIBOLD);
const HINT: TextStyle = TextStyle::new(12.5, TextStyle::MEDIUM);

/// Icon and tint for a WMO weather code.
pub fn icon_for(code: u8, is_day: bool) -> (Icon, Color) {
    const SUN: Color = rgb(0xFFD60A);
    const MOON: Color = rgb(0xD8DEE9);
    const CLOUD: Color = rgb(0xC7CDD6);
    const RAIN: Color = rgb(0x64B5F6);
    match code {
        0 | 1 if is_day => (Icon::Sun, SUN),
        0 | 1 => (Icon::Moon, MOON),
        2 if is_day => (Icon::CloudSun, CLOUD),
        2 => (Icon::CloudMoon, CLOUD),
        3 => (Icon::Cloud, CLOUD),
        45 | 48 => (Icon::Fog, CLOUD),
        51..=67 | 80..=82 => (Icon::Rain, RAIN),
        71..=77 | 85 | 86 => (Icon::Snow, rgb(0xE3F2FD)),
        95..=99 => (Icon::Thunder, rgb(0xFFCA28)),
        _ => (Icon::Cloud, CLOUD),
    }
}

fn degrees(value: f32) -> String {
    format!("{}\u{00B0}", value.round() as i32)
}

pub fn draw(p: &mut Painter, m: &Model) {
    let width = Mode::Weather.size().width;
    status_bar(p, m, width, &[("Weather", palette::WHITE)]);
    page_navigation(p, m, width, 86.0);

    match (m.weather, m.weather_city) {
        (Some(weather), _) => {
            let (icon, tint) = icon_for(weather.code, weather.is_day);
            p.glow(Rect::new(40.0, 50.0, 110.0, 106.0), 36.0, 24.0, tint.multiply_alpha(0.16));
            p.icon(icon, Point::new(70.0, 76.0), 34.0, tint);
            let temp = p.text(&degrees(weather.temperature), &TEMP, Point::new(94.0, 74.0), HAlign::Left, VAlign::Center, palette::WHITE);
            let side = temp.x1 + 8.0;
            let side_w = (246.0 - side).max(40.0) as f32;
            p.text(describe(weather.code), &CONDITION.max_width(side_w), Point::new(side, 66.0), HAlign::Left, VAlign::Center, palette::WHITE);
            p.text(
                &format!("H {}  L {}", degrees(weather.high), degrees(weather.low)),
                &DETAIL,
                Point::new(side, 83.0),
                HAlign::Left,
                VAlign::Center,
                palette::GRAY,
            );
            p.icon(Icon::Location, Point::new(58.0, 118.0), 12.0, palette::GRAY);
            p.text(&weather.location, &DETAIL.max_width(176.0), Point::new(67.0, 118.0), HAlign::Left, VAlign::Center, palette::GRAY);
            p.text(
                &format!("Feels {} \u{00B7} {}% \u{00B7} {} km/h", degrees(weather.feels_like), weather.humidity.round(), weather.wind.round()),
                &DETAIL,
                Point::new(52.0, 136.0),
                HAlign::Left,
                VAlign::Center,
                palette::GRAY,
            );

            p.fill(&Rect::new(254.0, 42.0, 255.0, 146.0), palette::DIVIDER);
            let column = (368.0 - 260.0) / 4.0;
            for (i, hour) in weather.hours.iter().take(4).enumerate() {
                let x = 260.0 + column * i as f64 + column / 2.0;
                p.text(&format!("{:02}", hour.hour), &HOUR, Point::new(x, 58.0), HAlign::Center, VAlign::Center, palette::GRAY);
                let (icon, tint) = icon_for(hour.code, hour.is_day);
                p.icon(icon, Point::new(x, 88.0), 20.0, tint);
                p.text(&degrees(hour.temperature), &HOUR_TEMP, Point::new(x, 118.0), HAlign::Center, VAlign::Center, palette::WHITE);
            }
            p.hit(Hit::WeatherCity, Rect::new(50.0, 108.0, 246.0, 128.0));
        }
        (None, Some(city)) => {
            let error = m.weather_error.unwrap_or("Loading\u{2026}");
            p.text(city, &HINT, Point::new(210.0, 84.0), HAlign::Center, VAlign::Center, palette::WHITE);
            p.text(error, &DETAIL.max_width(300.0), Point::new(210.0, 104.0), HAlign::Center, VAlign::Center, palette::GRAY);
            capsule_button(p, Hit::WeatherCity, Rect::new(160.0, 118.0, 260.0, 142.0), Some(Icon::Location), "Change", ButtonStyle::GRAY, &BUTTON);
        }
        (None, None) => {
            p.icon(Icon::CloudSun, Point::new(210.0, 66.0), 30.0, rgba(0xFFFFFF, 0.9));
            p.text("Where are you?", &HINT, Point::new(210.0, 96.0), HAlign::Center, VAlign::Center, palette::WHITE);
            capsule_button(p, Hit::WeatherCity, Rect::new(155.0, 112.0, 265.0, 138.0), Some(Icon::Location), "Set city", ButtonStyle::GRAY, &BUTTON);
        }
    }
    super::page_dots(p, m, width);
}
