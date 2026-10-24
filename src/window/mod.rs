//! Native window integration.

pub mod dragdrop;
pub mod fullscreen;
pub mod tray;
pub mod win32;

use serde::{Deserialize, Serialize};

/// Static canvas size in logical pixels. The OS window never changes size;
/// the notch outline is animated inside it on the GPU.
pub const CANVAS_WIDTH: f64 = 700.0;
pub const CANVAS_HEIGHT: f64 = 260.0;

/// Screen edge the notch hangs from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    /// Attached to the top edge, growing downward (default, Y = 0).
    #[default]
    Top,
    /// Attached to the bottom edge, growing upward.
    Bottom,
}
