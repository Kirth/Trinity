//! Side view: small, translucent floating windows anchored to a screen corner.
//!
//! A side-view window (think picture-in-picture video) lives in the floating space, keeps to its
//! corner when its size or the working area changes, follows workspace switches on its monitor,
//! and is skipped by directional focus.

use smithay::utils::{Logical, Point, Size};

use super::scrolling::ColumnWidth;
use super::SizeFrac;

/// Corner of the working area that a side-view window is anchored to.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    #[default]
    BottomRight,
    BottomLeft,
}

impl Corner {
    /// Next corner, clockwise.
    pub fn next(self) -> Self {
        match self {
            Corner::TopLeft => Corner::TopRight,
            Corner::TopRight => Corner::BottomRight,
            Corner::BottomRight => Corner::BottomLeft,
            Corner::BottomLeft => Corner::TopLeft,
        }
    }

    /// Previous corner, counter-clockwise.
    pub fn prev(self) -> Self {
        match self {
            Corner::TopLeft => Corner::BottomLeft,
            Corner::TopRight => Corner::TopLeft,
            Corner::BottomRight => Corner::TopRight,
            Corner::BottomLeft => Corner::BottomRight,
        }
    }

    pub fn is_left(self) -> bool {
        matches!(self, Corner::TopLeft | Corner::BottomLeft)
    }

    pub fn is_top(self) -> bool {
        matches!(self, Corner::TopLeft | Corner::TopRight)
    }

    /// Corner of the quadrant containing a point given as a fraction of the working area.
    pub fn nearest(center: Point<f64, SizeFrac>) -> Self {
        match (center.x < 0.5, center.y < 0.5) {
            (true, true) => Corner::TopLeft,
            (false, true) => Corner::TopRight,
            (false, false) => Corner::BottomRight,
            (true, false) => Corner::BottomLeft,
        }
    }
}

/// Side-view state of a tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SideView {
    pub corner: Corner,
    /// What to put back when leaving side view.
    pub restore: SideViewRestore,
}

/// Tile state from before it entered side view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SideViewRestore {
    /// Column width and full-width flag if the tile was tiled, `None` if it was floating.
    pub tiled: Option<(ColumnWidth, bool)>,
    pub floating_pos: Option<Point<f64, SizeFrac>>,
    pub floating_window_size: Option<Size<i32, Logical>>,
    pub floating_preset_width_idx: Option<usize>,
    pub floating_preset_height_idx: Option<usize>,
}

/// Anchor of a side-view tile in the floating space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub corner: Corner,
    /// Distance from the working area edges.
    pub margin: f64,
}
