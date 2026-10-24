//! System widget: CPU, memory and system drive rings, network throughput
//! and a short CPU history.

use vello::kurbo::{BezPath, Circle, Point, Rect};
use vello::peniko::{Color, Gradient};

use super::common::{STATUS_Y, mix, progress_ring, status_bar};
use super::{HAlign, Mode, Model, Painter, VAlign, page_navigation, palette, rgb, rgba};
use crate::renderer::text::TextStyle;
use crate::services::system::{HISTORY, format_rate, gigabytes};

const PERCENT: TextStyle = TextStyle::new(13.0, TextStyle::BOLD).tabular();
const LABEL: TextStyle = TextStyle::new(12.0, TextStyle::SEMIBOLD);
const DETAIL: TextStyle = TextStyle::new(10.5, TextStyle::REGULAR).tabular();
const SMALL: TextStyle = TextStyle::new(11.0, TextStyle::MEDIUM);
const RATE: TextStyle = TextStyle::new(13.0, TextStyle::SEMIBOLD).tabular();

const ORANGE: Color = rgb(0xFF9F0A);

/// Green, then orange and red as a resource fills up.
fn load_color(base: Color, level: f32) -> Color {
    if level >= 0.9 {
        palette::RED
    } else if level >= 0.75 {
        ORANGE
    } else {
        base
    }
}

pub fn draw(p: &mut Painter, m: &Model) {
    let width = Mode::System.size().width;
    status_bar(p, m, width, &[("System", palette::WHITE), (" Monitor", palette::BLUE)]);
    let stats = m.system;
    let ring_y = 76.0;
    let rings = [
        (78.0, stats.cpu, palette::CYAN, "CPU", String::new()),
        (
            151.0,
            stats.memory,
            palette::PURPLE,
            "Memory",
            format!("{} / {} GB", gigabytes(stats.memory_used), gigabytes(stats.memory_total)),
        ),
        (224.0, stats.disk, palette::GREEN, "Disk C:", format!("{} GB free", gigabytes(stats.disk_free))),
    ];
    for (x, level, base, label, detail) in rings {
        let center = Point::new(x, ring_y);
        let color = load_color(base, level);
        p.fill(&Circle::new(center, 20.0), color.multiply_alpha(0.08));
        progress_ring(p, center, 23.0, 5.0, if stats.ready || label != "CPU" { level as f64 } else { 0.0 }, color);
        let value = if stats.ready || label != "CPU" { format!("{}%", (level * 100.0).round() as u32) } else { "\u{2013}".into() };
        p.text(&value, &PERCENT, center, HAlign::Center, VAlign::Center, palette::WHITE);
        p.text(label, &LABEL, Point::new(x, ring_y + 39.0), HAlign::Center, VAlign::Center, palette::WHITE);
        let detail = if label == "CPU" { format!("{} cores", num_cpus()) } else { detail };
        p.text(&detail, &DETAIL, Point::new(x, ring_y + 54.0), HAlign::Center, VAlign::Center, palette::GRAY);
    }

    p.fill(&Rect::new(262.0, 42.0, 263.0, 146.0), palette::DIVIDER);
    let x = 274.0;
    p.text("Network", &SMALL, Point::new(x, STATUS_Y + 30.0), HAlign::Left, VAlign::Center, palette::GRAY);
    let down = p.text("\u{2193}", &RATE, Point::new(x, 70.0), HAlign::Left, VAlign::Center, palette::CYAN);
    p.text(&format_rate(stats.download), &RATE, Point::new(down.x1 + 5.0, 70.0), HAlign::Left, VAlign::Center, palette::WHITE);
    let up = p.text("\u{2191}", &RATE, Point::new(x, 88.0), HAlign::Left, VAlign::Center, palette::PURPLE);
    p.text(&format_rate(stats.upload), &RATE, Point::new(up.x1 + 5.0, 88.0), HAlign::Left, VAlign::Center, palette::WHITE);

    sparkline(p, Rect::new(x, 104.0, 366.0, 142.0), stats.cpu_history.iter().copied());
    page_navigation(p, m, width, 86.0);
    super::page_dots(p, m, width);
}

/// CPU history as a filled line chart, newest sample on the right.
fn sparkline(p: &mut Painter, rect: Rect, samples: impl ExactSizeIterator<Item = f32>) {
    p.fill(&vello::kurbo::RoundedRect::from_rect(rect, 6.0), rgba(0xFFFFFF, 0.04));
    let count = samples.len();
    p.text("CPU history", &DETAIL, Point::new(rect.x0 + 6.0, rect.y0 + 8.0), HAlign::Left, VAlign::Center, rgba(0xFFFFFF, 0.35));
    if count < 2 {
        return;
    }
    let step = rect.width() / (HISTORY - 1) as f64;
    let start = rect.x1 - step * (count - 1) as f64;
    let points: Vec<Point> = samples
        .enumerate()
        .map(|(i, v)| Point::new(start + step * i as f64, rect.y1 - 3.0 - (rect.height() - 8.0) * v.clamp(0.0, 1.0) as f64))
        .collect();
    let mut line = BezPath::new();
    line.move_to(points[0]);
    for pair in points.windows(2) {
        // Smooth the polyline with midpoint quadratic segments.
        let mid = pair[0].midpoint(pair[1]);
        line.quad_to(pair[0], mid);
    }
    line.line_to(points[points.len() - 1]);
    let mut area = line.clone();
    area.line_to(Point::new(points[points.len() - 1].x, rect.y1));
    area.line_to(Point::new(points[0].x, rect.y1));
    area.close_path();
    let fill = Gradient::new_linear((0.0, rect.y0), (0.0, rect.y1))
        .with_stops([palette::CYAN.multiply_alpha(0.35), palette::CYAN.multiply_alpha(0.0)]);
    p.push_clip(&vello::kurbo::RoundedRect::from_rect(rect, 6.0));
    p.fill_gradient(&area, &fill);
    p.stroke(&line, 1.6, mix(palette::CYAN, palette::WHITE, 0.2));
    p.pop_clip();
}

fn num_cpus() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}
