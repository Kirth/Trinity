//! Ace-window-style hints: a label on every visible window, typing it focuses the window.

use std::cell::RefCell;
use std::collections::HashMap;
use std::f64::consts::PI;

use niri_config::Color;
use ordered_float::NotNan;
use pangocairo::cairo::{self, ImageSurface};
use pangocairo::pango::FontDescription;
use smithay::backend::renderer::element::Kind;
use smithay::reexports::gbm::Format as Fourcc;
use smithay::utils::{Logical, Point, Transform};

use crate::render_helpers::memory::MemoryBuffer;
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::utils::to_physical_precise_round;
use crate::window::mapped::MappedId;

/// Keys used for the labels, home row first.
const KEYS: &[char] = &['a', 's', 'd', 'f', 'j', 'k', 'l'];
const FONT: &str = "sans bold 15px";
/// Radius of the label circle, in logical pixels.
const RADIUS: f64 = 14.;
const OUTLINE: f64 = 2.;
/// Offset of the label from the window's top-left corner, in logical pixels.
pub const LABEL_OFFSET: f64 = 6.;

/// Result of pressing a key while the hints are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintOutcome {
    /// More keys are needed.
    Pending,
    /// The label of this window was typed.
    Selected(MappedId),
    /// The key doesn't match any label.
    Cancelled,
}

#[derive(Debug)]
struct Session {
    targets: Vec<(String, MappedId)>,
    typed: String,
}

/// Label textures by label, number of typed characters and scale.
type TextureCache = HashMap<(String, usize, NotNan<f64>), Option<MemoryBuffer>>;

pub struct AceHints {
    session: Option<Session>,
    textures: RefCell<TextureCache>,
}

/// Assigns labels to `n` targets: one key each if they fit, otherwise two keys each.
///
/// Targets past what two keys can label get no label.
pub fn assign_labels(n: usize, keys: &[char]) -> Vec<String> {
    let k = keys.len();
    if n <= k {
        keys[..n].iter().map(|c| c.to_string()).collect()
    } else {
        (0..n.min(k * k))
            .map(|i| [keys[i / k], keys[i % k]].iter().collect())
            .collect()
    }
}

impl AceHints {
    pub fn new() -> Self {
        Self {
            session: None,
            textures: RefCell::new(HashMap::new()),
        }
    }

    pub fn is_open(&self) -> bool {
        self.session.is_some()
    }

    /// Shows the hints for these windows, in label order.
    pub fn open(&mut self, windows: Vec<MappedId>) {
        let labels = assign_labels(windows.len(), KEYS);
        let targets = labels.into_iter().zip(windows).collect();
        self.session = Some(Session {
            targets,
            typed: String::new(),
        });
    }

    pub fn close(&mut self) {
        self.session = None;
    }

    /// Handles a key press. Closes the hints unless more keys are needed.
    pub fn press(&mut self, c: char) -> HintOutcome {
        let Some(session) = &mut self.session else {
            return HintOutcome::Cancelled;
        };

        session.typed.push(c.to_ascii_lowercase());
        let typed = &session.typed;

        let outcome = if let Some((_, id)) = session.targets.iter().find(|(l, _)| l == typed) {
            HintOutcome::Selected(*id)
        } else if session
            .targets
            .iter()
            .any(|(l, _)| l.starts_with(typed.as_str()))
        {
            HintOutcome::Pending
        } else {
            HintOutcome::Cancelled
        };

        if outcome != HintOutcome::Pending {
            self.close();
        }
        outcome
    }

    /// The label to show for a window, with the number of characters already typed, or `None`
    /// if the window has no label or no longer matches.
    pub fn label_for(&self, id: MappedId) -> Option<(&str, usize)> {
        let session = self.session.as_ref()?;
        let (label, _) = session.targets.iter().find(|(_, x)| *x == id)?;
        label
            .starts_with(session.typed.as_str())
            .then_some((label.as_str(), session.typed.len()))
    }

    /// Renders the label of each window at its location, in output-local logical coordinates.
    pub fn render<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        scale: f64,
        color: Color,
        windows: impl Iterator<Item = (MappedId, Point<f64, Logical>)>,
        push: &mut dyn FnMut(PrimaryGpuTextureRenderElement),
    ) {
        if self.session.is_none() {
            return;
        }
        let _span = tracy_client::span!("AceHints::render");

        let mut textures = self.textures.borrow_mut();
        for (id, loc) in windows {
            let Some((label, typed)) = self.label_for(id) else {
                continue;
            };

            let key = (label.to_owned(), typed, NotNan::new(scale).unwrap());
            let buffer = textures
                .entry(key)
                .or_insert_with(|| render_label(label, typed, scale, color).ok());
            let Some(buffer) = buffer else {
                continue;
            };

            let Ok(texture) =
                TextureBuffer::from_memory_buffer(renderer.as_gles_renderer(), buffer)
            else {
                continue;
            };

            let loc = loc + Point::from((LABEL_OFFSET, LABEL_OFFSET));
            let loc = loc.to_physical_precise_round(scale).to_logical(scale);
            let elem = TextureRenderElement::from_texture_buffer(
                texture,
                loc,
                1.,
                None,
                None,
                Kind::Unspecified,
            );
            push(PrimaryGpuTextureRenderElement(elem));
        }
    }
}

impl Default for AceHints {
    fn default() -> Self {
        Self::new()
    }
}

/// Draws a label as white text in a filled, outlined circle (a pill for longer labels). Already
/// typed characters are dimmed.
fn render_label(
    label: &str,
    typed: usize,
    scale: f64,
    color: Color,
) -> anyhow::Result<MemoryBuffer> {
    let _span = tracy_client::span!("ace_hints::render_label");

    let mut font = FontDescription::from_string(FONT);
    font.set_absolute_size(to_physical_precise_round(scale, font.size()));

    let (done, rest) = label.split_at(typed.min(label.len()));
    let markup = format!("<span alpha='45%'>{done}</span>{rest}");

    let surface = ImageSurface::create(cairo::Format::ARgb32, 0, 0)?;
    let cr = cairo::Context::new(&surface)?;
    let layout = pangocairo::functions::create_layout(&cr);
    layout.context().set_round_glyph_positions(false);
    layout.set_font_description(Some(&font));
    layout.set_markup(&markup);
    let (text_w, text_h) = layout.pixel_size();

    let r = (RADIUS * scale).round();
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
    cr.set_source_rgba(
        f64::from(color.r),
        f64::from(color.g),
        f64::from(color.b),
        f64::from(color.a.max(0.9)),
    );
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
    layout.set_markup(&markup);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u64) -> Vec<MappedId> {
        (0..n).map(|_| MappedId::next()).collect()
    }

    #[test]
    fn labels_are_single_keys_when_they_fit() {
        assert_eq!(assign_labels(1, KEYS), ["a"]);
        assert_eq!(assign_labels(7, KEYS), ["a", "s", "d", "f", "j", "k", "l"]);
    }

    #[test]
    fn labels_are_two_keys_when_they_dont_fit() {
        let labels = assign_labels(8, KEYS);
        assert_eq!(labels.len(), 8);
        assert_eq!(&labels[..3], ["aa", "as", "ad"]);
        assert_eq!(labels[7], "sa");

        assert_eq!(assign_labels(49, KEYS).last().unwrap(), "ll");
        // Past 49 windows, the rest get no label.
        assert_eq!(assign_labels(50, KEYS).len(), 49);
    }

    #[test]
    fn single_key_picks() {
        let mut hints = AceHints::new();
        let windows = ids(3);
        hints.open(windows.clone());
        assert_eq!(hints.press('s'), HintOutcome::Selected(windows[1]));
        assert!(!hints.is_open());
    }

    #[test]
    fn two_keys_pick_and_first_key_filters() {
        let mut hints = AceHints::new();
        let windows = ids(9);
        hints.open(windows.clone());

        assert_eq!(hints.press('s'), HintOutcome::Pending);
        assert!(hints.is_open());
        // "aa" no longer matches, "sa" does.
        assert_eq!(hints.label_for(windows[0]), None);
        assert_eq!(hints.label_for(windows[7]), Some(("sa", 1)));

        assert_eq!(hints.press('s'), HintOutcome::Selected(windows[8]));
        assert!(!hints.is_open());
    }

    #[test]
    fn wrong_key_cancels() {
        let mut hints = AceHints::new();
        hints.open(ids(3));
        assert_eq!(hints.press('q'), HintOutcome::Cancelled);
        assert!(!hints.is_open());
    }
}
