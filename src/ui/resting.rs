//! Resting mode: a small black pill with a blue-violet glow along its free
//! edge and two eyes that blink every 3-6 s and sometimes glance sideways.

use std::time::{Duration, Instant};

use vello::kurbo::{Point, Rect};
use vello::peniko::{Color, Gradient};

use super::{Mode, Model, Painter};
use crate::animation::{Rng, SpringConfig, SpringValue, ease_in_out};
use crate::renderer::shapes::capsule;
use crate::window::Edge;

const CLOSE_TIME: f32 = 0.075;
const OPEN_TIME: f32 = 0.115;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyesPose {
    /// 1 = open, ~0.1 = closed.
    pub openness: f32,
    /// Horizontal glance offset in pixels.
    pub offset: f32,
}

impl Default for EyesPose {
    fn default() -> Self {
        Self { openness: 1.0, offset: 0.0 }
    }
}

pub struct Eyes {
    next_blink: Instant,
    blink_started: Option<Instant>,
    glance: SpringValue,
    glance_until: Option<Instant>,
    next_glance: Instant,
}

impl Eyes {
    pub fn new(now: Instant, rng: &mut Rng) -> Self {
        Self {
            next_blink: now + Duration::from_secs_f32(rng.range(3.0, 6.0)),
            blink_started: None,
            glance: SpringValue::new(0.0, SpringConfig::HOVER),
            glance_until: None,
            next_glance: now + Duration::from_secs_f32(rng.range(6.0, 12.0)),
        }
    }

    /// Advances the blink/glance timeline. Returns `true` while frames are
    /// needed.
    pub fn update(&mut self, now: Instant, dt: f32, rng: &mut Rng) -> bool {
        let mut animating = false;
        if self.blink_started.is_none() && now >= self.next_blink {
            self.blink_started = Some(now);
        }
        if let Some(start) = self.blink_started {
            if (now - start).as_secs_f32() >= CLOSE_TIME + OPEN_TIME {
                self.blink_started = None;
                // Occasionally blink twice in a row.
                let pause = if rng.chance(0.18) { rng.range(0.16, 0.26) } else { rng.range(3.0, 6.0) };
                self.next_blink = now + Duration::from_secs_f32(pause);
            } else {
                animating = true;
            }
        }
        match self.glance_until {
            None if now >= self.next_glance => {
                let side = if rng.chance(0.5) { -1.0 } else { 1.0 };
                self.glance.set_target(side * rng.range(2.2, 3.6));
                self.glance_until = Some(now + Duration::from_secs_f32(rng.range(0.8, 1.6)));
            }
            Some(until) if now >= until => {
                self.glance.set_target(0.0);
                self.glance_until = None;
                self.next_glance = now + Duration::from_secs_f32(rng.range(7.0, 14.0));
            }
            _ => {}
        }
        animating |= self.glance.step(dt);
        animating
    }

    pub fn pose(&self, now: Instant) -> EyesPose {
        let openness = match self.blink_started {
            Some(start) => {
                let t = (now - start).as_secs_f32();
                if t < CLOSE_TIME {
                    1.0 - 0.9 * ease_in_out(t / CLOSE_TIME)
                } else {
                    0.1 + 0.9 * ease_in_out((t - CLOSE_TIME) / OPEN_TIME)
                }
            }
            None => 1.0,
        };
        EyesPose { openness, offset: self.glance.value() }
    }

    /// Next moment the eyes need a frame even if nothing else happens.
    pub fn next_deadline(&self) -> Instant {
        let glance = self.glance_until.unwrap_or(self.next_glance);
        self.next_blink.min(glance)
    }
}

pub fn draw(p: &mut Painter, m: &Model) {
    let size = Mode::Resting.size();
    let (w, h) = (size.width, size.height);
    let cx = w / 2.0;

    // Soft light bleeding in from the free edge (the bottom edge when the
    // notch hangs from the top of the screen, the top edge otherwise).
    // `band(inside, outside)` spans that edge, `inside` px into the pill.
    let band = |inside: f64, outside: f64| match m.edge {
        Edge::Top => (h - inside, h + outside),
        Edge::Bottom => (-outside, inside),
    };
    let (y0, y1) = band(3.0, 7.0);
    let (left, right) = (m.accent, m.accent2);
    let middle = super::common::mix(left, right, 0.5);
    p.glow(Rect::new(8.0, y0, cx, y1), 6.0, 5.0, left.multiply_alpha(0.55));
    p.glow(Rect::new(cx, y0, w - 8.0, y1), 6.0, 5.0, right.multiply_alpha(0.55));
    let (y0, y1) = band(2.0, 6.0);
    p.glow(Rect::new(cx - 36.0, y0, cx + 36.0, y1), 6.0, 5.0, middle.multiply_alpha(0.5));
    let line = Gradient::new_linear((0.0, 0.0), (w, 0.0)).with_stops([
        (0.0, Color::TRANSPARENT),
        (0.18, left),
        (0.5, middle),
        (0.82, right),
        (1.0, Color::TRANSPARENT),
    ]);
    let (y0, y1) = band(1.6, 0.0);
    p.fill_gradient(&Rect::new(6.0, y0, w - 6.0, y1), &line);

    let pose = m.eyes;
    let eye_w = 8.0;
    let eye_h = 13.0 * pose.openness.max(0.12) as f64;
    for dx in [-11.0, 11.0] {
        let center = Point::new(cx + dx + pose.offset as f64, h / 2.0 + 0.5);
        p.glow(Rect::from_center_size(center, (eye_w, eye_h)), eye_w / 2.0, 2.5, Color::WHITE.multiply_alpha(0.35));
        p.fill(&capsule(Rect::from_center_size(center, (eye_w, eye_h))), Color::WHITE);
    }
}
