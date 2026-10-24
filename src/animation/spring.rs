//! Damped harmonic oscillator used for every animated value in the notch.
//!
//! Each frame the solver integrates
//!
//! ```text
//! F_spring  = -stiffness * (current - target)
//! F_damping = -damping * velocity
//! a         = (F_spring + F_damping) / mass
//! ```
//!
//! with semi-implicit Euler in fixed sub-steps, so the motion is identical at
//! 60, 120 or 144 Hz and stays stable when a frame arrives late.

use peniko::Color;
use vello::peniko;

/// Largest integration step. Smaller than one frame at 240 Hz.
const MAX_SUBSTEP: f32 = 1.0 / 480.0;
/// Frames longer than this (sleep, debugger, window drag) are clamped so a
/// spring never jumps across its whole path in a single step.
const MAX_FRAME_DT: f32 = 1.0 / 20.0;
/// Rest threshold from the specification, in the spring's own units.
pub const DEFAULT_PRECISION: f32 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringConfig {
    pub mass: f32,
    pub stiffness: f32,
    pub damping: f32,
}

impl SpringConfig {
    /// iOS-like expansion of the notch outline.
    pub const EXPAND: Self = Self { mass: 1.0, stiffness: 280.0, damping: 26.0 };
    /// Hover feedback on elements and buttons.
    pub const HOVER: Self = Self { mass: 1.0, stiffness: 400.0, damping: 30.0 };
    /// Snappy motion for small indicators (blinks, toggles).
    pub const SNAPPY: Self = Self { mass: 1.0, stiffness: 700.0, damping: 46.0 };
    /// Slow, soft motion for ambient elements such as the lyric scroller.
    pub const GENTLE: Self = Self { mass: 1.0, stiffness: 170.0, damping: 24.0 };
}

#[derive(Clone, Copy, Debug)]
pub struct SpringValue {
    current: f32,
    target: f32,
    velocity: f32,
    config: SpringConfig,
    precision: f32,
}

impl SpringValue {
    pub fn new(value: f32, config: SpringConfig) -> Self {
        Self { current: value, target: value, velocity: 0.0, config, precision: DEFAULT_PRECISION }
    }

    /// Spring for normalized quantities (opacity, progress), where the default
    /// 0.05 threshold would cause a visible jump when snapping to rest.
    pub fn normalized(value: f32, config: SpringConfig) -> Self {
        Self { precision: 0.001, ..Self::new(value, config) }
    }

    pub fn value(&self) -> f32 {
        self.current
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Jumps to `value` without animating.
    pub fn snap(&mut self, value: f32) {
        self.current = value;
        self.target = value;
        self.velocity = 0.0;
    }

    /// True when both the distance to the target and the velocity are below
    /// the rest threshold (0.05 by default, as required by the spec).
    pub fn is_at_rest(&self) -> bool {
        (self.current - self.target).abs() < self.precision && self.velocity.abs() < self.precision
    }

    /// Advances the simulation by `dt` seconds. Returns `true` while moving.
    pub fn step(&mut self, dt: f32) -> bool {
        if self.is_at_rest() {
            self.current = self.target;
            self.velocity = 0.0;
            return false;
        }
        let dt = dt.clamp(0.0, MAX_FRAME_DT);
        let steps = (dt / MAX_SUBSTEP).ceil().max(1.0) as u32;
        let h = dt / steps as f32;
        let SpringConfig { mass, stiffness, damping } = self.config;
        for _ in 0..steps {
            let f_spring = -stiffness * (self.current - self.target);
            let f_damping = -damping * self.velocity;
            let acceleration = (f_spring + f_damping) / mass;
            self.velocity += acceleration * h;
            self.current += self.velocity * h;
        }
        if self.is_at_rest() {
            self.current = self.target;
            self.velocity = 0.0;
            false
        } else {
            true
        }
    }
}

/// Animated rectangle (position and dimensions) built from four springs.
#[derive(Clone, Copy, Debug)]
pub struct RectSpring {
    pub x: SpringValue,
    pub y: SpringValue,
    pub width: SpringValue,
    pub height: SpringValue,
}

impl RectSpring {
    pub fn new(rect: vello::kurbo::Rect, config: SpringConfig) -> Self {
        Self {
            x: SpringValue::new(rect.x0 as f32, config),
            y: SpringValue::new(rect.y0 as f32, config),
            width: SpringValue::new(rect.width() as f32, config),
            height: SpringValue::new(rect.height() as f32, config),
        }
    }

    pub fn set_target(&mut self, rect: vello::kurbo::Rect) {
        self.x.set_target(rect.x0 as f32);
        self.y.set_target(rect.y0 as f32);
        self.width.set_target(rect.width() as f32);
        self.height.set_target(rect.height() as f32);
    }

    pub fn snap(&mut self, rect: vello::kurbo::Rect) {
        self.x.snap(rect.x0 as f32);
        self.y.snap(rect.y0 as f32);
        self.width.snap(rect.width() as f32);
        self.height.snap(rect.height() as f32);
    }

    pub fn rect(&self) -> vello::kurbo::Rect {
        let (x, y) = (self.x.value() as f64, self.y.value() as f64);
        vello::kurbo::Rect::new(x, y, x + self.width.value().max(1.0) as f64, y + self.height.value().max(1.0) as f64)
    }

    pub fn target(&self) -> vello::kurbo::Rect {
        let (x, y) = (self.x.target() as f64, self.y.target() as f64);
        vello::kurbo::Rect::new(x, y, x + self.width.target() as f64, y + self.height.target() as f64)
    }

    pub fn step(&mut self, dt: f32) -> bool {
        // Non-short-circuiting so every component advances every frame.
        self.x.step(dt) | self.y.step(dt) | self.width.step(dt) | self.height.step(dt)
    }
}

/// Animated RGBA color; each channel is a normalized spring.
#[derive(Clone, Copy, Debug)]
pub struct ColorSpring {
    channels: [SpringValue; 4],
}

impl ColorSpring {
    pub fn new(color: Color, config: SpringConfig) -> Self {
        let c = color.components;
        Self { channels: c.map(|v| SpringValue::normalized(v, config)) }
    }

    pub fn set_target(&mut self, color: Color) {
        for (spring, v) in self.channels.iter_mut().zip(color.components) {
            spring.set_target(v);
        }
    }

    pub fn color(&self) -> Color {
        Color::new(self.channels.map(|s| s.value().clamp(0.0, 1.0)))
    }

    pub fn step(&mut self, dt: f32) -> bool {
        let mut moving = false;
        for spring in &mut self.channels {
            moving |= spring.step(dt);
        }
        moving
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settles_on_target() {
        let mut s = SpringValue::new(0.0, SpringConfig::EXPAND);
        s.set_target(260.0);
        let mut frames = 0;
        while s.step(1.0 / 120.0) {
            frames += 1;
            assert!(frames < 1000, "spring never settled");
        }
        assert_eq!(s.value(), 260.0);
        assert!(s.is_at_rest());
    }

    #[test]
    fn frame_rate_independent() {
        let mut a = SpringValue::new(0.0, SpringConfig::HOVER);
        let mut b = a;
        a.set_target(100.0);
        b.set_target(100.0);
        for _ in 0..60 {
            a.step(1.0 / 60.0);
        }
        for _ in 0..144 {
            b.step(1.0 / 144.0);
        }
        assert!((a.value() - b.value()).abs() < 0.5);
    }
}
