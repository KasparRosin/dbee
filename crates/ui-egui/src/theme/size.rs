//! Spacing, heights and radii, and the sizes of the shared controls (`components`).

pub const SPACE_4: f32 = 4.0;
pub const SPACE_8: f32 = 8.0;
pub const SPACE_12: f32 = 12.0;
pub const SPACE_16: f32 = 16.0;
pub const SPACE_24: f32 = 24.0;
pub const SPACE_32: f32 = 32.0;
pub const INSET: f32 = 12.0;
pub const BAR_H: f32 = 36.0;
pub const SLIM_BAR_H: f32 = 30.0;
pub const ROW_H: f32 = 26.0;
pub const GRID_HEADER_H: f32 = 30.0;
pub const BUTTON_H: f32 = 28.0;
pub const RADIUS_CONTROL: u8 = 2;
pub const RADIUS_FLOAT: u8 = 4;
pub const SPINNER: f32 = 14.0;

pub const KEYCAP_H: f32 = 18.0;
/// Between the keycap's edge and its key.
pub const KEYCAP_PAD_X: f32 = 4.0;
pub const CHIP_H: f32 = 18.0;
/// Between the chip's edge and what it holds.
pub const CHIP_PAD_X: f32 = 6.0;
/// The available chip's square mark, and the gap after it.
pub const CHIP_MARK: f32 = 6.0;
pub const CHIP_GAP: f32 = 6.0;

/// Keyboard focus (foundations' Focus rule): a 2 px `ink` edge or box, or a 1 px `on_ink` line.
pub const FOCUS_W: f32 = 2.0;
pub const FOCUS_LINE_W: f32 = 1.0;
/// How far the box around a control with no edge reaches out from it.
pub const FOCUS_AROUND: f32 = 4.0;
/// How far a bar button's box and the `on_ink` line sit inside the control.
pub const FOCUS_INSET: f32 = 2.0;
pub const RADIUS_FOCUS_AROUND: u8 = 4;
pub const RADIUS_FOCUS_LINE: u8 = 1;
