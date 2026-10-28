//! Window cards: live thumbnails of windows with their app icon, outline and title, laid out
//! along a screen edge. Shared by the limbo drawer and the workspace strip.

use std::cell::RefCell;
use std::collections::HashMap;

use niri_config::{Color, LimboEdge};
use ordered_float::NotNan;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::Color32F;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::niri_render_elements;
use crate::render_helpers::label::{render_pill, LabelCache};
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::ui::mru::ThumbnailElement;
use crate::utils::app_icon::{load_icon, AppIcons};
use crate::utils::with_toplevel_role;
use crate::window::mapped::MappedId;
use crate::window::Mapped;

/// Smallest a card gets when there are many; past that, only some show and scrolling cycles
/// through them.
pub const MIN_CARD_H: f64 = 48.;
/// Widest a card gets, in logical pixels.
pub const CARD_MAX_W: f64 = 240.;
/// Gap between cards, in logical pixels.
pub const GAP: f64 = 8.;
/// Outline drawn behind each card, so it stands out on any background.
const OUTLINE: f64 = 1.;
const OUTLINE_COLOR: [f32; 4] = [0., 0., 0., 0.6];
/// Outline of a card whose window wants attention, in the urgent colour.
const URGENT_OUTLINE: f64 = 3.;
/// Title shown next to a card.
const CAPTION_FONT: &str = "sans 13px";
const CAPTION_RADIUS: f64 = 12.;
const CAPTION_FILL: [f64; 4] = [0.12, 0.12, 0.12, 0.7];
/// Gap between a card and its title, in logical pixels.
const CAPTION_GAP: f64 = 8.;
/// Longest title shown, in characters.
const CAPTION_MAX_CHARS: usize = 60;
/// Size of the app icon on a card, in logical pixels, at most half the card.
const ICON: f64 = 32.;
/// Gap between the app icon and the card's corner, in logical pixels.
const ICON_INSET: f64 = 6.;

/// A window in the drawer, as far as laying out its card goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Item {
    pub id: MappedId,
    /// Size of the window.
    pub size: Size<f64, Logical>,
    /// Whether the window wants attention.
    pub urgent: bool,
}

impl Item {
    pub fn new(mapped: &Mapped) -> Self {
        Self {
            id: mapped.id(),
            size: window_size(mapped),
            urgent: crate::layout::LayoutElement::is_urgent(mapped),
        }
    }
}

/// Where a card goes on the output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    pub id: MappedId,
    /// The whole card, partly past the right edge.
    pub card: Rectangle<f64, Logical>,
    /// Where the pointer counts as being on the card: the card and the gap between it and the
    /// edge, on the output.
    pub hit: Rectangle<f64, Logical>,
}

niri_render_elements! {
    CardRenderElement<R> => {
        Thumbnail = ThumbnailElement<R>,
        SolidColor = SolidColorRenderElement,
        Texture = PrimaryGpuTextureRenderElement,
    }
}

/// Caches for drawing cards: outlines, titles and app icons.
#[derive(Debug, Default)]
pub struct CardDecorations {
    /// Outline behind each card, kept so that it keeps its element id.
    outlines: RefCell<HashMap<MappedId, SolidColorBuffer>>,
    /// Titles by window, title and scale.
    captions: LabelCache<(MappedId, String, NotNan<f64>)>,
    /// Icon files by app id.
    icons: AppIcons,
    /// Loaded icons by app id and size in physical pixels.
    icon_buffers: LabelCache<(String, i32)>,
}

impl CardDecorations {
    /// Renders the outline behind a card, in the urgent colour if its window wants attention.
    pub fn render_outline(
        &self,
        id: MappedId,
        card: Rectangle<f64, Logical>,
        urgent: Option<Color>,
        opacity: f32,
    ) -> SolidColorRenderElement {
        let (width, color): (f64, Color32F) = match urgent {
            Some(color) => (URGENT_OUTLINE, color.into()),
            None => (OUTLINE, OUTLINE_COLOR.into()),
        };
        let mut outline = card;
        outline.loc -= Point::from((width, width));
        outline.size += Size::from((width * 2., width * 2.));
        let mut outlines = self.outlines.borrow_mut();
        let buffer = outlines
            .entry(id)
            .or_insert_with(|| SolidColorBuffer::new(outline.size, OUTLINE_COLOR));
        buffer.update(outline.size, color);
        SolidColorRenderElement::from_buffer(buffer, outline.loc, opacity, Kind::Unspecified)
    }

    /// Forgets the outlines of cards that are gone.
    pub fn retain(&mut self, present: impl Fn(MappedId) -> bool) {
        self.outlines.get_mut().retain(|id, _| present(*id));
    }
}

impl CardDecorations {
    /// Renders the title of a window next to its card.
    pub fn render_caption<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        mapped: &Mapped,
        card: Rectangle<f64, Logical>,
        edge: LimboEdge,
        scale: f64,
    ) -> Option<PrimaryGpuTextureRenderElement> {
        let title = with_toplevel_role(mapped.toplevel(), |role| role.title.clone())?;
        let title = title.trim();
        if title.is_empty() {
            return None;
        }

        let key = (mapped.id(), title.to_owned(), NotNan::new(scale).ok()?);
        let buffer = self.captions.get(key, || {
            let markup = pango::glib::markup_escape_text(&truncate(title, CAPTION_MAX_CHARS));
            render_pill(&markup, CAPTION_FONT, CAPTION_RADIUS, CAPTION_FILL, scale)
        })?;
        let texture =
            TextureBuffer::from_memory_buffer(renderer.as_gles_renderer(), &buffer).ok()?;

        let loc = caption_loc(card, texture.logical_size(), edge, CAPTION_GAP);
        let loc = loc.to_physical_precise_round(scale).to_logical(scale);
        let elem = TextureRenderElement::from_texture_buffer(
            texture,
            loc,
            1.,
            None,
            None,
            Kind::Unspecified,
        );
        Some(PrimaryGpuTextureRenderElement(elem))
    }
}

impl CardDecorations {
    /// Renders the app icon of a window in the corner of its card.
    pub fn render_icon<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        mapped: &Mapped,
        card: Rectangle<f64, Logical>,
        edge: LimboEdge,
        scale: f64,
        opacity: f32,
    ) -> Option<PrimaryGpuTextureRenderElement> {
        let app_id = with_toplevel_role(mapped.toplevel(), |role| role.app_id.clone())?;
        let size = f64::min(ICON, f64::min(card.size.w, card.size.h) / 2.);
        let px = (size * scale).round() as i32;
        if px < 1 {
            return None;
        }

        let buffer = self.icon_buffers.get((app_id.clone(), px), || {
            let paths = self.icons.paths(&app_id);
            load_icon(&paths, px, scale).ok_or_else(|| anyhow::anyhow!("no icon for {app_id}"))
        })?;
        let texture =
            TextureBuffer::from_memory_buffer(renderer.as_gles_renderer(), &buffer).ok()?;

        let loc = icon_loc(card, texture.logical_size(), edge);
        let loc = loc.to_physical_precise_round(scale).to_logical(scale);
        let elem = TextureRenderElement::from_texture_buffer(
            texture,
            loc,
            opacity,
            None,
            None,
            Kind::Unspecified,
        );
        Some(PrimaryGpuTextureRenderElement(elem))
    }
}

/// Where the app icon goes on a card: in the bottom corner on the side facing into the screen.
pub fn icon_loc(
    card: Rectangle<f64, Logical>,
    size: Size<f64, Logical>,
    edge: LimboEdge,
) -> Point<f64, Logical> {
    let y = card.loc.y + card.size.h - ICON_INSET - size.h;
    match edge {
        LimboEdge::Right | LimboEdge::Bottom => Point::from((card.loc.x + ICON_INSET, y)),
        LimboEdge::Left => Point::from((card.loc.x + card.size.w - ICON_INSET - size.w, y)),
    }
}

/// Cuts text to at most `max` characters, ending in an ellipsis if it was cut.
pub fn truncate(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// Where the title of a card goes: beside it, on the side facing into the screen, centred on it.
pub fn caption_loc(
    card: Rectangle<f64, Logical>,
    size: Size<f64, Logical>,
    edge: LimboEdge,
    gap: f64,
) -> Point<f64, Logical> {
    let center = card.loc + card.size.downscale(2.).to_point();
    match edge {
        LimboEdge::Right => Point::from((card.loc.x - gap - size.w, center.y - size.h / 2.)),
        LimboEdge::Left => Point::from((card.loc.x + card.size.w + gap, center.y - size.h / 2.)),
        LimboEdge::Bottom => Point::from((center.x - size.w / 2., card.loc.y - gap - size.h)),
    }
}

/// Position of a point along the drawer's edge: down it at the sides, across it at the bottom.
pub fn along_edge(edge: LimboEdge, point: Point<f64, Logical>) -> f64 {
    match edge {
        LimboEdge::Left | LimboEdge::Right => point.y,
        LimboEdge::Bottom => point.x,
    }
}

/// Whether a point is further into the screen than a card reaches, by more than `distance`.
pub fn is_past_card(
    edge: LimboEdge,
    card: Rectangle<f64, Logical>,
    point: Point<f64, Logical>,
    distance: f64,
) -> bool {
    match edge {
        LimboEdge::Right => point.x < card.loc.x - distance,
        LimboEdge::Left => point.x > card.loc.x + card.size.w + distance,
        LimboEdge::Bottom => point.y < card.loc.y - distance,
    }
}

/// Where a dragged card goes among the other shown cards, for a pointer at this position along
/// the edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Insert {
    Before(MappedId),
    After(MappedId),
}

/// Where the dragged card goes: before the first other card whose middle is past the pointer,
/// or after the last one.
pub fn insertion_point(
    slots: &[Slot],
    edge: LimboEdge,
    dragged: MappedId,
    along: f64,
) -> Option<Insert> {
    let middle = |slot: &Slot| {
        along_edge(
            edge,
            slot.card.loc + slot.card.size.downscale(2.).to_point(),
        )
    };
    let mut others = slots.iter().filter(|slot| slot.id != dragged);
    let mut last = None;
    for slot in &mut others {
        if middle(slot) > along {
            return Some(Insert::Before(slot.id));
        }
        last = Some(slot.id);
    }
    last.map(Insert::After)
}

/// Size of a window for its card.
pub fn window_size(mapped: &Mapped) -> Size<f64, Logical> {
    let size = crate::layout::LayoutElement::size(mapped).to_f64();
    Size::from((size.w.max(1.), size.h.max(1.)))
}

/// Size of a card for a window, at most `card_h` high and `CARD_MAX_W` wide, keeping its
/// aspect ratio.
pub fn card_size(size: Size<f64, Logical>, card_h: f64) -> Size<f64, Logical> {
    let aspect = size.w / size.h;
    let w = f64::min(aspect * card_h, CARD_MAX_W).max(1.);
    Size::from((w, (w / aspect).max(1.)))
}
