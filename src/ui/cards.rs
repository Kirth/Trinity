//! Window cards: live thumbnails of windows with their app icon, outline and title, laid out
//! along a screen edge. Shared by the limbo drawer and the workspace strip.

use std::cell::RefCell;
use std::collections::HashMap;

use niri_config::{Color, LimboEdge};
use niri_ipc::{Annotation, CardStatus};
use ordered_float::NotNan;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::Color32F;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::niri_render_elements;
use crate::render_helpers::border::BorderRenderElement;
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
/// Badge in the corner of a card, from a provider.
const BADGE_FONT: &str = "sans bold 11px";
const BADGE_RADIUS: f64 = 9.;
const BADGE_FILL: [f64; 4] = [0.1, 0.1, 0.1, 0.85];
/// Gap between the badge and the card's corner, in logical pixels.
const BADGE_INSET: f64 = 6.;
/// Height of the progress bar along the bottom of a card, in logical pixels.
const PROGRESS_H: f64 = 3.;
const PROGRESS_COLOR: [f32; 4] = [0.45, 0.7, 1., 0.9];
/// Width of the outline of a card whose window has a status from a provider.
const STATUS_OUTLINE: f64 = 2.;
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
    /// Which app the window belongs to, to stack the cards of one app together.
    pub group: u64,
}

impl Item {
    pub fn new(mapped: &Mapped) -> Self {
        use std::hash::{Hash as _, Hasher as _};

        let app_id = with_toplevel_role(mapped.toplevel(), |role| role.app_id.clone());
        let mut hasher = std::hash::DefaultHasher::new();
        app_id.hash(&mut hasher);
        Self {
            id: mapped.id(),
            size: window_size(mapped),
            urgent: crate::layout::LayoutElement::is_urgent(mapped),
            group: hasher.finish(),
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
    /// Which app the card's window belongs to, as in [`Item::group`].
    pub group: u64,
    /// On the front card of a stack of cards of one app: how many there are.
    pub stack: usize,
    /// Whether the card is tucked behind another in a stack, showing only its bottom.
    pub tucked: bool,
}

niri_render_elements! {
    CardRenderElement<R> => {
        Thumbnail = ThumbnailElement<R>,
        Border = BorderRenderElement,
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
    /// Badges by text and scale.
    badges: LabelCache<(String, NotNan<f64>)>,
    /// Progress bar on each card, kept so that it keeps its element id.
    progress: RefCell<HashMap<MappedId, SolidColorBuffer>>,
}

/// What a window's annotations amount to on its card.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct CardInfo {
    /// The badges of every provider, together.
    pub badge: Option<String>,
    /// The most pressing status.
    pub status: Option<CardStatus>,
    /// The texts of every provider, one per line.
    pub text: Option<String>,
    /// The progress of the first provider that has one.
    pub progress: Option<f64>,
}

impl CardInfo {
    pub fn new(mapped: &Mapped) -> Self {
        Self::from_annotations(mapped.annotations().map(|(_, a)| a))
    }

    pub fn from_annotations<'a>(annotations: impl Iterator<Item = &'a Annotation>) -> Self {
        let mut badges = Vec::new();
        let mut texts = Vec::new();
        let mut info = Self::default();
        for a in annotations {
            badges.extend(a.badge.clone());
            texts.extend(a.text.clone());
            info.status = info.status.max(a.status);
            info.progress = info.progress.or(a.progress);
        }
        info.badge = (!badges.is_empty()).then(|| badges.join(" · "));
        info.text = (!texts.is_empty()).then(|| texts.join("\n"));
        info
    }
}

/// Colour of a card's outline for a status.
pub fn status_color(status: CardStatus) -> Color {
    match status {
        CardStatus::Done => Color::from_rgba8_unpremul(0x5f, 0xc8, 0x7a, 0xff),
        CardStatus::Working => Color::from_rgba8_unpremul(0x5a, 0x9c, 0xf2, 0xff),
        CardStatus::Waiting => Color::from_rgba8_unpremul(0xf2, 0xb2, 0x3a, 0xff),
        CardStatus::Error => Color::from_rgba8_unpremul(0xe5, 0x4b, 0x4b, 0xff),
    }
}

impl CardDecorations {
    /// Renders the outline behind a card: in the urgent colour if its window wants attention,
    /// otherwise in the colour of its status.
    pub fn render_outline(
        &self,
        id: MappedId,
        card: Rectangle<f64, Logical>,
        urgent: Option<Color>,
        status: Option<CardStatus>,
        opacity: f32,
    ) -> SolidColorRenderElement {
        let (width, color): (f64, Color32F) = match (urgent, status) {
            (Some(color), _) => (URGENT_OUTLINE, color.into()),
            (None, Some(status)) => (STATUS_OUTLINE, status_color(status).into()),
            (None, None) => (OUTLINE, OUTLINE_COLOR.into()),
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
        self.progress.get_mut().retain(|id, _| present(*id));
    }

    /// Renders a provider's badge in the top corner of a card, on the side facing into the
    /// screen.
    pub fn render_badge<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        text: &str,
        card: Rectangle<f64, Logical>,
        bottom: bool,
        scale: f64,
        opacity: f32,
    ) -> Option<PrimaryGpuTextureRenderElement> {
        let key = (text.to_owned(), NotNan::new(scale).ok()?);
        let buffer = self.badges.get(key, || {
            let markup = pango::glib::markup_escape_text(text);
            render_pill(&markup, BADGE_FONT, BADGE_RADIUS, BADGE_FILL, scale)
        })?;
        let texture =
            TextureBuffer::from_memory_buffer(renderer.as_gles_renderer(), &buffer).ok()?;
        let size = texture.logical_size();
        let mut loc = badge_loc(card, size);
        if bottom {
            loc.y = card.loc.y + card.size.h - BADGE_INSET - size.h;
        }
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

    /// Renders a progress bar along the bottom of a card.
    pub fn render_progress(
        &self,
        id: MappedId,
        card: Rectangle<f64, Logical>,
        progress: f64,
        opacity: f32,
    ) -> SolidColorRenderElement {
        let rect = progress_rect(card, progress);
        let mut bars = self.progress.borrow_mut();
        let buffer = bars
            .entry(id)
            .or_insert_with(|| SolidColorBuffer::new(rect.size, PROGRESS_COLOR));
        buffer.resize(rect.size);
        SolidColorRenderElement::from_buffer(buffer, rect.loc, opacity, Kind::Unspecified)
    }
}

impl CardDecorations {
    /// Renders the title of a window next to its card, with a provider's text below it.
    pub fn render_caption<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        mapped: &Mapped,
        text: Option<&str>,
        card: Rectangle<f64, Logical>,
        edge: LimboEdge,
        scale: f64,
    ) -> Option<PrimaryGpuTextureRenderElement> {
        let title = with_toplevel_role(mapped.toplevel(), |role| role.title.clone())?;
        let title = title.trim();
        if title.is_empty() && text.is_none() {
            return None;
        }

        let mut markup =
            pango::glib::markup_escape_text(&truncate(title, CAPTION_MAX_CHARS)).to_string();
        if let Some(text) = text {
            let text = pango::glib::markup_escape_text(text);
            markup.push_str(&format!(
                "\n<span size=\"smaller\" alpha=\"75%\">{text}</span>"
            ));
        }
        let key = (mapped.id(), markup.clone(), NotNan::new(scale).ok()?);
        let buffer = self.captions.get(key, || {
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

/// Where a badge goes on a card: in the top-right corner.
pub fn badge_loc(card: Rectangle<f64, Logical>, size: Size<f64, Logical>) -> Point<f64, Logical> {
    Point::from((
        card.loc.x + card.size.w - BADGE_INSET - size.w,
        card.loc.y + BADGE_INSET,
    ))
}

/// Where the progress bar goes on a card: along its bottom edge, as long as the progress.
pub fn progress_rect(card: Rectangle<f64, Logical>, progress: f64) -> Rectangle<f64, Logical> {
    let h = PROGRESS_H.min(card.size.h);
    Rectangle::new(
        Point::from((card.loc.x, card.loc.y + card.size.h - h)),
        Size::from(((card.size.w * progress.clamp(0., 1.)).max(1.), h)),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> Rectangle<f64, Logical> {
        Rectangle::new(Point::from((100., 50.)), Size::from((200., 120.)))
    }

    #[test]
    fn annotations_add_up() {
        let a = Annotation {
            badge: Some(String::from("12 tabs")),
            status: Some(CardStatus::Working),
            progress: Some(0.25),
            ..Default::default()
        };
        let b = Annotation {
            badge: Some(String::from("3")),
            status: Some(CardStatus::Waiting),
            text: Some(String::from("needs approval")),
            progress: Some(0.75),
        };
        let info = CardInfo::from_annotations([&a, &b].into_iter());
        assert_eq!(info.badge.as_deref(), Some("12 tabs · 3"));
        // The most pressing status wins.
        assert_eq!(info.status, Some(CardStatus::Waiting));
        assert_eq!(info.text.as_deref(), Some("needs approval"));
        assert_eq!(info.progress, Some(0.25));

        assert_eq!(
            CardInfo::from_annotations(std::iter::empty()),
            CardInfo::default()
        );
    }

    #[test]
    fn badge_sits_in_the_top_right_corner() {
        let loc = badge_loc(card(), Size::from((40., 18.)));
        assert_eq!(
            loc,
            Point::from((300. - BADGE_INSET - 40., 50. + BADGE_INSET))
        );
    }

    #[test]
    fn progress_runs_along_the_bottom() {
        let rect = progress_rect(card(), 0.5);
        assert_eq!(rect.loc, Point::from((100., 170. - PROGRESS_H)));
        assert_eq!(rect.size, Size::from((100., PROGRESS_H)));
        // Kept in range, and always visible.
        assert_eq!(progress_rect(card(), 3.).size.w, 200.);
        assert_eq!(progress_rect(card(), 0.).size.w, 1.);
    }
}
