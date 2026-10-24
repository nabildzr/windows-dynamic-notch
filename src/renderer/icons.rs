//! Original vector icons designed on a 24x24 grid.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use vello::kurbo::{BezPath, Circle, RoundedRect, Shape};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Play,
    Pause,
    Next,
    Previous,
    ChevronLeft,
    ChevronRight,
    Lyrics,
    LyricsFilled,
    Stopwatch,
    Coffee,
    Target,
    Bell,
    BellOff,
    InboxDown,
    Bluetooth,
    Headphones,
    Bolt,
    Check,
    PlusCircle,
    Pencil,
    Sliders,
    Droplet,
    Reset,
    Close,
    MusicNote,
    Plus,
    Minus,
    Undo,
    Speaker,
    SpeakerMuted,
    Sun,
    Moon,
    Cloud,
    CloudSun,
    CloudMoon,
    Rain,
    Snow,
    Thunder,
    Fog,
    Pin,
    Copy,
    Wifi,
    Leaf,
    Gauge,
    Flag,
    Location,
}

#[derive(Clone, Debug)]
pub enum Mark {
    Fill(BezPath),
    FillEvenOdd(BezPath),
    Stroke(BezPath, f64),
}

thread_local! {
    static CACHE: RefCell<HashMap<Icon, Rc<[Mark]>>> = RefCell::new(HashMap::new());
}

/// Geometry of `icon` in the 24x24 design space.
pub fn marks(icon: Icon) -> Rc<[Mark]> {
    CACHE.with(|cache| cache.borrow_mut().entry(icon).or_insert_with(|| build(icon).into()).clone())
}

fn svg(d: &str) -> BezPath {
    BezPath::from_svg(d).unwrap_or_else(|err| {
        log::error!("invalid icon path {d:?}: {err}");
        BezPath::new()
    })
}

fn fill(d: &str) -> Mark {
    Mark::Fill(svg(d))
}

fn stroke(d: &str, width: f64) -> Mark {
    Mark::Stroke(svg(d), width)
}

fn shape_fill(shape: impl Shape) -> Mark {
    Mark::Fill(shape.to_path(0.05))
}

fn shape_stroke(shape: impl Shape, width: f64) -> Mark {
    Mark::Stroke(shape.to_path(0.05), width)
}

fn build(icon: Icon) -> Vec<Mark> {
    match icon {
        Icon::Play => {
            let d = "M7.6 5.4 L18.6 12 L7.6 18.6 Z";
            vec![fill(d), stroke(d, 1.8)]
        }
        Icon::Pause => vec![
            shape_fill(RoundedRect::new(6.4, 5.0, 10.4, 19.0, 1.4)),
            shape_fill(RoundedRect::new(13.6, 5.0, 17.6, 19.0, 1.4)),
        ],
        Icon::Next => {
            let d = "M3.8 6.4 L11.4 12 L3.8 17.6 Z M12.2 6.4 L19.8 12 L12.2 17.6 Z";
            vec![fill(d), stroke(d, 1.2)]
        }
        Icon::Previous => {
            let d = "M20.2 6.4 L12.6 12 L20.2 17.6 Z M11.8 6.4 L4.2 12 L11.8 17.6 Z";
            vec![fill(d), stroke(d, 1.2)]
        }
        Icon::ChevronLeft => vec![stroke("M14.4 6 L8.4 12 L14.4 18", 2.6)],
        Icon::ChevronRight => vec![stroke("M9.6 6 L15.6 12 L9.6 18", 2.6)],
        Icon::Lyrics => vec![
            stroke("M8 4.6 H16 A4.4 4.4 0 0 1 20.4 9 V12.2 A4.4 4.4 0 0 1 16 16.6 H12.4 L7.6 20.2 V16.6 A4.4 4.4 0 0 1 3.6 12.2 V9 A4.4 4.4 0 0 1 8 4.6 Z", 1.9),
            shape_fill(Circle::new((9.4, 10.6), 1.4)),
            shape_fill(Circle::new((14.6, 10.6), 1.4)),
        ],
        Icon::LyricsFilled => {
            let mut path = svg(
                "M8 4.2 H16 A4.8 4.8 0 0 1 20.8 9 V12.2 A4.8 4.8 0 0 1 16 17 H12.6 L7.2 20.8 V16.9 A4.8 4.8 0 0 1 3.2 12.2 V9 A4.8 4.8 0 0 1 8 4.2 Z",
            );
            path.extend(Circle::new((9.4, 10.6), 1.5).path_elements(0.05));
            path.extend(Circle::new((14.6, 10.6), 1.5).path_elements(0.05));
            vec![Mark::FillEvenOdd(path)]
        }
        Icon::Stopwatch => vec![
            shape_stroke(Circle::new((12.0, 13.4), 7.4), 2.0),
            shape_fill(RoundedRect::new(9.6, 2.4, 14.4, 4.4, 1.0)),
            stroke("M12 4.4 V6", 2.0),
            stroke("M12 13.4 V9.4", 2.0),
            stroke("M18.2 6.6 L19.6 5.2", 1.8),
        ],
        Icon::Coffee => vec![
            stroke("M4.6 9 H15.8 V13.8 A4.6 4.6 0 0 1 11.2 18.4 H9.2 A4.6 4.6 0 0 1 4.6 13.8 Z", 1.8),
            stroke("M15.8 10.4 H17.2 A2.3 2.3 0 0 1 17.2 15 H15.8", 1.8),
            stroke("M8 3.6 V6", 1.6),
            stroke("M12 3.6 V6", 1.6),
        ],
        Icon::Target => vec![
            shape_stroke(Circle::new((12.0, 12.0), 8.4), 1.9),
            shape_stroke(Circle::new((12.0, 12.0), 4.4), 1.9),
            shape_fill(Circle::new((12.0, 12.0), 1.3)),
        ],
        Icon::Bell => vec![
            fill("M12 3.4 C8.4 3.4 6.4 6.1 6.4 9.4 V13.8 L4.7 16.4 H19.3 L17.6 13.8 V9.4 C17.6 6.1 15.6 3.4 12 3.4 Z"),
            fill("M9.7 18.2 A2.3 2.3 0 0 0 14.3 18.2 Z"),
        ],
        Icon::BellOff => vec![
            stroke("M12 3.4 C8.4 3.4 6.4 6.1 6.4 9.4 V13.8 L4.7 16.4 H19.3 L17.6 13.8 V9.4 C17.6 6.1 15.6 3.4 12 3.4 Z", 1.7),
            stroke("M4.5 4.5 L19.5 19.5", 1.9),
        ],
        Icon::InboxDown => vec![
            fill("M3.4 13 H8 L9.5 15.4 H14.5 L16 13 H20.6 V18.6 A1.6 1.6 0 0 1 19 20.2 H5 A1.6 1.6 0 0 1 3.4 18.6 Z"),
            stroke("M12 3 V10.6", 2.2),
            stroke("M8.6 7.6 L12 11 L15.4 7.6", 2.2),
        ],
        Icon::Bluetooth => vec![stroke("M7 7.6 L17 16.4 L12 20.8 V3.2 L17 7.6 L7 16.4", 1.8)],
        Icon::Headphones => vec![
            stroke("M4.2 15.4 V12.2 A7.8 7.8 0 0 1 19.8 12.2 V15.4", 2.0),
            shape_fill(RoundedRect::new(3.0, 13.2, 8.0, 20.4, 2.0)),
            shape_fill(RoundedRect::new(16.0, 13.2, 21.0, 20.4, 2.0)),
        ],
        Icon::Bolt => {
            let d = "M13.6 2.6 L5.6 13.4 H11.4 L10.4 21.4 L18.4 10.6 H12.6 Z";
            vec![fill(d), stroke(d, 1.0)]
        }
        Icon::Check => vec![stroke("M5.6 12.6 L10 17 L18.4 7.6", 2.6)],
        Icon::PlusCircle => {
            let mut path = Circle::new((12.0, 12.0), 9.0).to_path(0.05);
            path.extend(svg("M11 7.4 H13 V11 H16.6 V13 H13 V16.6 H11 V13 H7.4 V11 H11 Z"));
            vec![Mark::FillEvenOdd(path)]
        }
        Icon::Pencil => vec![
            stroke("M15.2 5.2 L18.8 8.8 L9 18.6 L4.6 19.4 L5.4 15 Z", 1.8),
            stroke("M13 7.4 L16.6 11", 1.8),
        ],
        Icon::Sliders => vec![
            stroke("M4 8 H7", 1.8),
            stroke("M11.5 8 H20", 1.8),
            stroke("M4 16 H12.5", 1.8),
            stroke("M17 16 H20", 1.8),
            shape_stroke(Circle::new((9.25, 8.0), 2.25), 1.8),
            shape_stroke(Circle::new((14.75, 16.0), 2.25), 1.8),
        ],
        Icon::Droplet => vec![fill(
            "M12 3 C12 3 5.6 10.2 5.6 14.6 A6.4 6.4 0 0 0 18.4 14.6 C18.4 10.2 12 3 12 3 Z",
        )],
        Icon::Reset => vec![
            stroke("M19.4 13.6 A7.6 7.6 0 1 1 17.2 6.6 L19.4 8.8", 2.1),
            stroke("M19.6 4.4 V9 H15", 2.1),
        ],
        Icon::Close => vec![stroke("M7 7 L17 17 M17 7 L7 17", 2.2)],
        Icon::MusicNote => vec![
            stroke("M9.4 17.4 V6.2 L18.4 4.4 V15.4", 2.0),
            shape_fill(Circle::new((7.0, 17.4), 2.6)),
            shape_fill(Circle::new((16.0, 15.4), 2.6)),
        ],
        Icon::Plus => vec![stroke("M12 6 V18 M6 12 H18", 2.2)],
        Icon::Minus => vec![stroke("M6 12 H18", 2.2)],
        Icon::Undo => vec![
            stroke("M8.4 9.2 H15 A4.6 4.6 0 0 1 15 18.4 H9", 2.0),
            stroke("M11.4 5.8 L8 9.2 L11.4 12.6", 2.0),
        ],
        Icon::Speaker => {
            let body = "M3.6 9.2 H7.4 L12.4 5 V19 L7.4 14.8 H3.6 Z";
            vec![
                fill(body),
                stroke(body, 1.2),
                stroke("M15.6 9 A4.2 4.2 0 0 1 15.6 15", 2.0),
                stroke("M18.2 6.4 A7.8 7.8 0 0 1 18.2 17.6", 2.0),
            ]
        }
        Icon::SpeakerMuted => {
            let body = "M3.6 9.2 H7.4 L12.4 5 V19 L7.4 14.8 H3.6 Z";
            vec![fill(body), stroke(body, 1.2), stroke("M15.6 9.4 L20.6 14.6 M20.6 9.4 L15.6 14.6", 2.0)]
        }
        Icon::Sun => vec![
            shape_fill(Circle::new((12.0, 12.0), 4.2)),
            stroke(
                "M12 2.6 V4.8 M12 19.2 V21.4 M2.6 12 H4.8 M19.2 12 H21.4 M5.4 5.4 L6.9 6.9 M17.1 17.1 L18.6 18.6 M5.4 18.6 L6.9 17.1 M17.1 6.9 L18.6 5.4",
                2.0,
            ),
        ],
        Icon::Moon => vec![fill("M14.6 3.4 A8.8 8.8 0 1 0 20.6 15.8 A7 7 0 0 1 14.6 3.4 Z")],
        Icon::Cloud => vec![fill(CLOUD)],
        Icon::CloudSun => vec![
            shape_fill(Circle::new((8.6, 8.4), 3.4)),
            stroke("M8.6 2.4 V3.4 M3.6 8.4 H2.6 M5 4.8 L4.3 4.1 M12.2 4.8 L12.9 4.1", 1.6),
            fill("M9.4 20 H17.8 A4 4 0 0 0 18.4 12.1 A5.2 5.2 0 0 0 8.6 13.4 A3.3 3.3 0 0 0 9.4 20 Z"),
        ],
        Icon::CloudMoon => vec![
            fill("M9.6 2.8 A5.6 5.6 0 1 0 14.4 10.6 A4.4 4.4 0 0 1 9.6 2.8 Z"),
            fill("M9.4 20 H17.8 A4 4 0 0 0 18.4 12.1 A5.2 5.2 0 0 0 8.6 13.4 A3.3 3.3 0 0 0 9.4 20 Z"),
        ],
        Icon::Rain => vec![fill(CLOUD_HIGH), stroke("M8.4 17.6 L7.4 20.4 M12.4 17.6 L11.4 20.4 M16.4 17.6 L15.4 20.4", 1.8)],
        Icon::Snow => vec![
            fill(CLOUD_HIGH),
            shape_fill(Circle::new((8.0, 19.0), 1.2)),
            shape_fill(Circle::new((12.0, 20.4), 1.2)),
            shape_fill(Circle::new((16.0, 19.0), 1.2)),
        ],
        Icon::Thunder => vec![fill(CLOUD_HIGH), fill("M12.8 15.2 L9.6 19.4 H12 L11 22.6 L14.6 18 H12.2 Z")],
        Icon::Fog => vec![stroke("M4 9 H20 M6 13 H18 M4 17 H20", 2.0)],
        Icon::Pin => vec![
            fill("M9 3.6 H15 L14.2 9.4 L17.4 12.6 V14 H6.6 V12.6 L9.8 9.4 Z"),
            stroke("M12 14 V20.6", 1.8),
        ],
        Icon::Copy => vec![
            shape_stroke(RoundedRect::new(8.4, 8.4, 19.6, 19.6, 2.6), 1.8),
            stroke("M15.6 5.6 V5.4 A1.8 1.8 0 0 0 13.8 3.6 H6.2 A2.6 2.6 0 0 0 3.6 6.2 V13.8 A1.8 1.8 0 0 0 5.4 15.6 H5.6", 1.8),
        ],
        Icon::Wifi => vec![
            stroke("M3.4 9.6 A12.4 12.4 0 0 1 20.6 9.6", 2.0),
            stroke("M6.6 13 A7.8 7.8 0 0 1 17.4 13", 2.0),
            stroke("M9.8 16.4 A3.4 3.4 0 0 1 14.2 16.4", 2.0),
            shape_fill(Circle::new((12.0, 19.4), 1.5)),
        ],
        Icon::Leaf => vec![
            fill("M19.6 4.4 C11 4.4 5 8.4 5 14.6 C5 17 6.2 18.8 6.2 18.8 C12.8 18.8 19.6 15 19.6 4.4 Z"),
            stroke("M4.4 20.2 C7.4 15.4 10.6 12.4 14.6 10", 1.6),
        ],
        Icon::Gauge => vec![
            stroke("M4.2 17 A8.6 8.6 0 1 1 19.8 17", 2.0),
            stroke("M12 14.2 L16 9", 2.2),
            shape_fill(Circle::new((12.0, 14.4), 1.8)),
        ],
        Icon::Flag => vec![stroke("M6 20.6 V4", 1.9), fill("M6 4.4 H17.6 L15.2 8.2 L17.6 12 H6 Z")],
        Icon::Location => {
            let mut path = svg("M12 21.4 C12 21.4 5.2 14.6 5.2 9.6 A6.8 6.8 0 0 1 18.8 9.6 C18.8 14.6 12 21.4 12 21.4 Z");
            path.extend(Circle::new((12.0, 9.6), 2.5).path_elements(0.05));
            vec![Mark::FillEvenOdd(path)]
        }
    }
}

const CLOUD: &str = "M7.2 19 H17.4 A4.4 4.4 0 0 0 18 10.2 A6 6 0 0 0 6.6 11.6 A3.7 3.7 0 0 0 7.2 19 Z";
const CLOUD_HIGH: &str = "M7.2 15.6 H17.4 A4.2 4.2 0 0 0 18 7.2 A5.8 5.8 0 0 0 6.6 8.6 A3.5 3.5 0 0 0 7.2 15.6 Z";