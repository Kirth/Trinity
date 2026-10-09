//! Small text labels in a pill, for overlays like window hints and debug views.

use std::cell::RefCell;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::hash::Hash;

use pangocairo::cairo::{self, ImageSurface};
use pangocairo::pango::FontDescription;
use smithay::reexports::gbm::Format as Fourcc;
use smithay::utils::Transform;

use crate::render_helpers::memory::MemoryBuffer;
use crate::utils::to_physical_precise_round;

/// Width of the white outline around a pill, in logical pixels.
const OUTLINE: f64 = 2.;

/// Rendered labels by key, so each one is drawn only once.
pub struct LabelCache<K> {
    buffers: RefCell<HashMap<K, Option<MemoryBuffer>>>,
}

impl<K: Hash + Eq> LabelCache<K> {
    pub fn new() -> Self {
        Self {
            buffers: RefCell::new(HashMap::new()),
        }
    }

    /// Returns the label for `key`, rendering it the first time.
    pub fn get(
        &self,
        key: K,
        render: impl FnOnce() -> anyhow::Result<MemoryBuffer>,
    ) -> Option<MemoryBuffer> {
        self.buffers
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| render().ok())
            .clone()
    }
}

impl<K: Hash + Eq> Default for LabelCache<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K> std::fmt::Debug for LabelCache<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LabelCache").finish_non_exhaustive()
    }
}

/// Draws white text in a filled pill with a white outline: a circle for short text.
///
/// `fill` is straight (not premultiplied) RGBA, and `radius` is in logical pixels.
pub fn render_pill(
    markup: &str,
    font: &str,
    radius: f64,
    fill: [f64; 4],
    scale: f64,
) -> anyhow::Result<MemoryBuffer> {
    let _span = tracy_client::span!("label::render_pill");

    let mut font = FontDescription::from_string(font);
    font.set_absolute_size(to_physical_precise_round(scale, font.size()));

    let surface = ImageSurface::create(cairo::Format::ARgb32, 0, 0)?;
    let cr = cairo::Context::new(&surface)?;
    let layout = pangocairo::functions::create_layout(&cr);
    layout.context().set_round_glyph_positions(false);
    layout.set_font_description(Some(&font));
    layout.set_markup(markup);
    let (text_w, text_h) = layout.pixel_size();

    let r = (radius * scale).round();
    let height = (r * 2.) as i32;
    let width = i32::max(height, text_w + (r as i32));

    let surface = ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    let cr = cairo::Context::new(&surface)?;

    // A pill: two half circles joined by a rectangle, a circle when the width equals the height.
    let line = (OUTLINE * scale).round();
    let inner = r - line / 2.;
    let (left, right) = (r, f64::from(width) - r);
    cr.new_sub_path();
    cr.arc(right, r, inner, -PI / 2., PI / 2.);
    cr.arc(left, r, inner, PI / 2., PI * 3. / 2.);
    cr.close_path();
    let [red, green, blue, alpha] = fill;
    cr.set_source_rgba(red, green, blue, alpha);
    cr.fill_preserve()?;
    cr.set_source_rgb(1., 1., 1.);
    cr.set_line_width(line);
    cr.stroke()?;

    cr.move_to(
        (f64::from(width - text_w) / 2.).round(),
        (f64::from(height - text_h) / 2.).round(),
    );
    let layout = pangocairo::functions::create_layout(&cr);
    layout.context().set_round_glyph_positions(false);
    layout.set_font_description(Some(&font));
    layout.set_markup(markup);
    cr.set_source_rgb(1., 1., 1.);
    pangocairo::functions::show_layout(&cr, &layout);
    drop(cr);

    let data = surface.take_data().unwrap();
    Ok(MemoryBuffer::new(
        data.to_vec(),
        Fourcc::Argb8888,
        (width, height),
        scale,
        Transform::Normal,
    ))
}
