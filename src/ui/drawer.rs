//! The limbo drawer: windows in limbo peek out as thin thumbnails at the right edge.
//!
//! Hovering one slides it out and shows its title. The slide times itself rather than using an
//! `Animation`, so it plays even with animations turned off.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use niri_config::{Color, LimboEdge};
use ordered_float::NotNan;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::Color32F;
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::niri_render_elements;
use crate::render_helpers::label::{render_pill, LabelCache};
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::render_helpers::RenderCtx;
use crate::ui::mru::{push_window_thumbnail, ThumbnailElement};
use crate::utils::with_toplevel_role;
use crate::window::mapped::MappedId;
use crate::window::Mapped;

/// Height of a card, in logical pixels, when they all fit.
const CARD_H: f64 = 120.;
/// Smallest a card gets when there are many; past that, only some show and scrolling cycles
/// through them.
const MIN_CARD_H: f64 = 48.;
/// Widest a card gets, in logical pixels.
const CARD_MAX_W: f64 = 240.;
/// Gap between cards, in logical pixels.
const GAP: f64 = 8.;
/// Share of the output height (or width, at the bottom) the cards may take up.
const MAX_STACK: f64 = 0.7;
/// How much of a card peeks out when it isn't hovered, in logical pixels.
const PEEK: f64 = 10.;
/// How much of a card peeks out when its window wants attention.
const URGENT_PEEK: f64 = 24.;
/// How long a card takes to slide all the way out or back in.
const SLIDE: Duration = Duration::from_millis(150);
/// Outline drawn behind each card, so it stands out on any background.
const OUTLINE: f64 = 1.;
const OUTLINE_COLOR: [f32; 4] = [0., 0., 0., 0.6];
/// Outline of a card whose window wants attention, in the urgent colour.
const URGENT_OUTLINE: f64 = 3.;
/// Opacity of the cards.
const OPACITY: f32 = 0.8;
/// Gap between a slid-out card and the edge of the screen, in millimetres.
pub const MARGIN_MM: f64 = 5.;
/// Title shown next to the hovered card.
const CAPTION_FONT: &str = "sans 13px";
const CAPTION_RADIUS: f64 = 12.;
const CAPTION_FILL: [f64; 4] = [0.12, 0.12, 0.12, 0.7];
/// Gap between the hovered card and its title, in logical pixels.
const CAPTION_GAP: f64 = 8.;
/// Longest title shown, in characters.
const CAPTION_MAX_CHARS: usize = 60;
/// How far a card has to slide out before its title shows.
const CAPTION_EXTEND: f64 = 0.5;

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

/// Where the drawer goes on an output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawerPlacement {
    pub edge: LimboEdge,
    /// Gap between a slid-out card and the edge, in logical pixels.
    pub margin: f64,
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
    DrawerRenderElement<R> => {
        Thumbnail = ThumbnailElement<R>,
        SolidColor = SolidColorRenderElement,
        Texture = PrimaryGpuTextureRenderElement,
    }
}

#[derive(Debug, Default)]
pub struct Drawer {
    /// Windows in the drawer, newest first.
    order: Vec<MappedId>,
    /// The card under the pointer.
    hovered: Option<(Output, MappedId)>,
    /// Whether every card slides out, as while the window hints are shown.
    popped: bool,
    /// Index of the first card shown, when they don't all fit.
    offset: usize,
    /// How far each card has slid out, from 0 to 1.
    extend: HashMap<MappedId, f64>,
    last_tick: Option<Duration>,
    /// Outline behind each card, kept so that it keeps its element id.
    outlines: RefCell<HashMap<MappedId, SolidColorBuffer>>,
    /// Titles by window, title and scale.
    captions: LabelCache<(MappedId, String, NotNan<f64>)>,
}

impl Drawer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Matches the drawer to the windows in limbo, newest first.
    pub fn sync(&mut self, present: &[MappedId]) {
        // Show a newly poofed window, which goes first.
        if present.first() != self.order.first() {
            self.offset = 0;
        }
        self.offset = self.offset.checked_rem(present.len()).unwrap_or(0);
        self.order = present.to_vec();
        self.extend.retain(|id, _| present.contains(id));
        self.outlines.get_mut().retain(|id, _| present.contains(id));
        if self
            .hovered
            .as_ref()
            .is_some_and(|(_, id)| !present.contains(id))
        {
            self.hovered = None;
        }
    }

    /// Windows in the drawer, newest first.
    pub fn order(&self) -> &[MappedId] {
        &self.order
    }

    pub fn hovered(&self) -> Option<MappedId> {
        self.hovered.as_ref().map(|(_, id)| *id)
    }

    /// Updates the card under the pointer. Returns whether it changed.
    pub fn set_hovered(&mut self, new: Option<(Output, MappedId)>) -> bool {
        if self.hovered == new {
            return false;
        }
        self.hovered = new;
        true
    }

    /// Slides every card out, or lets them slide back in.
    pub fn set_popped(&mut self, popped: bool) {
        self.popped = popped;
    }

    /// Whether this card should be slid out.
    fn is_out(&self, id: MappedId) -> bool {
        self.popped || self.hovered() == Some(id)
    }

    /// Cycles the cards by this many, when they don't all fit.
    pub fn scroll(&mut self, steps: i32) {
        let Ok(len) = i64::try_from(self.order.len()) else {
            return;
        };
        if len == 0 {
            return;
        }
        let offset = i64::try_from(self.offset).unwrap_or(0);
        self.offset = usize::try_from((offset + i64::from(steps)).rem_euclid(len)).unwrap_or(0);
    }

    /// Where the shown cards go on an output, for these windows in drawer order.
    pub fn slots(
        &self,
        output_size: Size<f64, Logical>,
        items: &[Item],
        placement: DrawerPlacement,
    ) -> Vec<Slot> {
        slots(output_size, items, placement, self.offset, |id| {
            self.extend.get(&id).copied().unwrap_or(0.)
        })
    }

    /// Whether only some of the cards fit on an output.
    pub fn overflows(
        &self,
        output_size: Size<f64, Logical>,
        items: &[Item],
        placement: DrawerPlacement,
    ) -> bool {
        self.slots(output_size, items, placement).len() < items.len()
    }

    /// The card under this point, in output-local coordinates.
    pub fn window_under(
        &self,
        output_size: Size<f64, Logical>,
        items: &[Item],
        placement: DrawerPlacement,
        pos: Point<f64, Logical>,
    ) -> Option<MappedId> {
        self.slots(output_size, items, placement)
            .into_iter()
            .find(|slot| slot.hit.contains(pos))
            .map(|slot| slot.id)
    }

    pub fn advance_animations(&mut self, now: Duration) {
        let dt = self
            .last_tick
            .map_or(Duration::ZERO, |last| now.saturating_sub(last));
        self.last_tick = Some(now);
        let step = dt.as_secs_f64() / SLIDE.as_secs_f64();

        for &id in &self.order {
            let out = self.is_out(id);
            let extend = self.extend.entry(id).or_insert(0.);
            if out {
                *extend = (*extend + step).min(1.);
            } else {
                *extend = (*extend - step).max(0.);
            }
        }
        self.extend.retain(|_, extend| *extend > 0.);
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.order.iter().any(|&id| {
            let extend = self.extend.get(&id).copied().unwrap_or(0.);
            if self.is_out(id) {
                extend < 1.
            } else {
                extend > 0.
            }
        })
    }

    /// Renders the cards on an output, for these windows in drawer order.
    #[allow(clippy::too_many_arguments)]
    pub fn render<R: NiriRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        output_size: Size<f64, Logical>,
        windows: &[&Mapped],
        placement: DrawerPlacement,
        urgent_color: Color,
        scale: f64,
        push: &mut dyn FnMut(DrawerRenderElement<R>),
    ) {
        let items: Vec<_> = windows.iter().map(|mapped| Item::new(mapped)).collect();
        let slots = self.slots(output_size, &items, placement);

        // The title of the hovered card, on top of everything.
        let hovered = self
            .hovered()
            .filter(|id| self.extend.get(id).is_some_and(|e| *e > CAPTION_EXTEND));
        let hovered = hovered.and_then(|id| {
            let slot = slots.iter().find(|slot| slot.id == id)?;
            let mapped = windows.iter().find(|mapped| mapped.id() == id)?;
            Some((slot, mapped))
        });
        if let Some((slot, mapped)) = hovered {
            if let Some(elem) =
                self.render_caption(ctx.renderer, mapped, slot.card, placement, scale)
            {
                push(DrawerRenderElement::Texture(elem));
            }
        }

        for slot in slots {
            let Some(mapped) = windows.iter().find(|mapped| mapped.id() == slot.id) else {
                continue;
            };
            let urgent = crate::layout::LayoutElement::is_urgent(*mapped);
            push_window_thumbnail(
                ctx.r(),
                mapped,
                window_size(mapped),
                slot.card,
                scale,
                OPACITY,
                &mut |elem| push(DrawerRenderElement::Thumbnail(elem)),
            );

            let (width, color): (f64, Color32F) = if urgent {
                (URGENT_OUTLINE, urgent_color.into())
            } else {
                (OUTLINE, OUTLINE_COLOR.into())
            };
            let mut outline = slot.card;
            outline.loc -= Point::from((width, width));
            outline.size += Size::from((width * 2., width * 2.));
            let mut outlines = self.outlines.borrow_mut();
            let buffer = outlines
                .entry(slot.id)
                .or_insert_with(|| SolidColorBuffer::new(outline.size, OUTLINE_COLOR));
            buffer.update(outline.size, color);
            push(DrawerRenderElement::SolidColor(
                SolidColorRenderElement::from_buffer(
                    buffer,
                    outline.loc,
                    OPACITY,
                    Kind::Unspecified,
                ),
            ));
        }
    }
}

impl Drawer {
    /// Renders the title of a window next to its card.
    fn render_caption<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        mapped: &Mapped,
        card: Rectangle<f64, Logical>,
        placement: DrawerPlacement,
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

        let loc = caption_loc(card, texture.logical_size(), placement.edge, CAPTION_GAP);
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

/// Cuts text to at most `max` characters, ending in an ellipsis if it was cut.
fn truncate(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// Where the title of a card goes: beside it, on the side facing into the screen, centred on it.
fn caption_loc(
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

/// Size of a window for its card.
pub fn window_size(mapped: &Mapped) -> Size<f64, Logical> {
    let size = crate::layout::LayoutElement::size(mapped).to_f64();
    Size::from((size.w.max(1.), size.h.max(1.)))
}

/// Size of a card for a window, at most `card_h` high and `CARD_MAX_W` wide, keeping its
/// aspect ratio.
fn card_size(size: Size<f64, Logical>, card_h: f64) -> Size<f64, Logical> {
    let aspect = size.w / size.h;
    let w = f64::min(aspect * card_h, CARD_MAX_W).max(1.);
    Size::from((w, (w / aspect).max(1.)))
}

fn slots(
    output_size: Size<f64, Logical>,
    items: &[Item],
    placement: DrawerPlacement,
    offset: usize,
    extend: impl Fn(MappedId) -> f64,
) -> Vec<Slot> {
    if items.is_empty() {
        return Vec::new();
    }

    // At the left and right edges, cards stack top to bottom; at the bottom, left to right.
    let in_row = placement.edge == LimboEdge::Bottom;
    let along = |size: Size<f64, Logical>| if in_row { size.w } else { size.h };

    // Shrink the cards if they don't all fit.
    let room = along(output_size) * MAX_STACK;
    let total = |items: &[&Item], card_h: f64| -> f64 {
        items
            .iter()
            .map(|item| along(card_size(item.size, card_h)))
            .sum::<f64>()
            + GAP * (items.len() as f64 - 1.)
    };
    let mut shown_items: Vec<&Item> = items.iter().collect();
    let mut card_h = CARD_H;
    let full = total(&shown_items, card_h);
    if full > room {
        let gaps = GAP * (items.len() as f64 - 1.);
        card_h = (card_h * (room - gaps) / (full - gaps)).max(1.);

        // Too many to stay legible: show as many as fit at the smallest size, starting at the
        // offset and wrapping round.
        if card_h < MIN_CARD_H {
            card_h = MIN_CARD_H;
            let n = items.len();
            shown_items.clear();
            let mut used = 0.;
            for i in 0..n {
                let item = &items[(offset + i) % n];
                let len = along(card_size(item.size, card_h));
                let next = if shown_items.is_empty() {
                    len
                } else {
                    used + GAP + len
                };
                if !shown_items.is_empty() && next > room {
                    break;
                }
                used = next;
                shown_items.push(item);
            }
        }
    }

    let mut pos = ((along(output_size) - total(&shown_items, card_h)) / 2.).max(0.);
    let output = Rectangle::from_size(output_size);

    shown_items
        .into_iter()
        .map(|item| {
            let id = item.id;
            let size = card_size(item.size, card_h);
            // How far into the screen the card reaches.
            let depth = if in_row { size.h } else { size.w };
            let peek = if item.urgent { URGENT_PEEK } else { PEEK };
            let shown = peek + extend(id).clamp(0., 1.) * (depth + placement.margin - peek);

            let (card, edge_side) = match placement.edge {
                LimboEdge::Right => {
                    let x = output_size.w - shown;
                    let card = Rectangle::new(Point::from((x, pos)), size);
                    let gap = Rectangle::new(card.loc, Size::from((shown, size.h)));
                    (card, gap)
                }
                LimboEdge::Left => {
                    let x = shown - depth;
                    let card = Rectangle::new(Point::from((x, pos)), size);
                    let gap = Rectangle::new(Point::from((0., pos)), Size::from((shown, size.h)));
                    (card, gap)
                }
                LimboEdge::Bottom => {
                    let y = output_size.h - shown;
                    let card = Rectangle::new(Point::from((pos, y)), size);
                    let gap = Rectangle::new(card.loc, Size::from((size.w, shown)));
                    (card, gap)
                }
            };
            let hit = edge_side.intersection(output).unwrap_or_default();

            pos += along(size) + GAP;
            Slot { id, card, hit }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<MappedId> {
        (0..n).map(|_| MappedId::next()).collect()
    }

    fn output() -> Size<f64, Logical> {
        Size::from((1920., 1080.))
    }

    const MARGIN: f64 = 19.;

    fn at(edge: LimboEdge) -> DrawerPlacement {
        DrawerPlacement {
            edge,
            margin: MARGIN,
        }
    }

    fn items(ids: &[MappedId]) -> Vec<Item> {
        ids.iter()
            .map(|id| Item {
                id: *id,
                size: Size::from((800., 600.)),
                urgent: false,
            })
            .collect()
    }

    #[test]
    fn right_cards_stack_centered_and_peek() {
        let ids = ids(2);
        let slots = slots(output(), &items(&ids), at(LimboEdge::Right), 0, |_| 0.);

        // 120 high, 160 wide, 8 apart, centered.
        assert_eq!(slots[0].card.size, Size::from((160., 120.)));
        assert_eq!(slots[0].card.loc.y, (1080. - 248.) / 2.);
        assert_eq!(slots[1].card.loc.y, slots[0].card.loc.y + 128.);

        // Only PEEK shows.
        assert_eq!(slots[0].card.loc.x, 1920. - PEEK);
        assert_eq!(slots[0].hit.size.w, PEEK);
    }

    #[test]
    fn hovered_card_lifts_off_the_edge() {
        let ids = ids(1);
        let items = items(&ids);

        let right = slots(output(), &items, at(LimboEdge::Right), 0, |_| 1.)[0];
        assert_eq!(right.card.loc.x + right.card.size.w, 1920. - MARGIN);
        // The gap to the edge still counts as the card.
        assert_eq!(right.hit.loc.x, right.card.loc.x);
        assert_eq!(right.hit.loc.x + right.hit.size.w, 1920.);

        let left = slots(output(), &items, at(LimboEdge::Left), 0, |_| 1.)[0];
        assert_eq!(left.card.loc.x, MARGIN);
        assert_eq!(left.hit.loc.x, 0.);
        assert_eq!(left.hit.size.w, MARGIN + left.card.size.w);

        let bottom = slots(output(), &items, at(LimboEdge::Bottom), 0, |_| 1.)[0];
        assert_eq!(bottom.card.loc.y + bottom.card.size.h, 1080. - MARGIN);
        assert_eq!(bottom.hit.loc.y + bottom.hit.size.h, 1080.);
    }

    #[test]
    fn left_cards_peek_from_the_left() {
        let ids = ids(1);
        let slot = slots(output(), &items(&ids), at(LimboEdge::Left), 0, |_| 0.)[0];
        assert_eq!(slot.card.loc.x + slot.card.size.w, PEEK);
        assert_eq!(
            slot.hit,
            Rectangle::new((0., slot.card.loc.y).into(), (PEEK, 120.).into())
        );
    }

    #[test]
    fn bottom_cards_form_a_centered_row() {
        let ids = ids(2);
        let slots = slots(output(), &items(&ids), at(LimboEdge::Bottom), 0, |_| 0.);
        assert_eq!(slots[0].card.loc.y, 1080. - PEEK);
        assert_eq!(slots[0].card.loc.x, (1920. - 328.) / 2.);
        assert_eq!(slots[1].card.loc.x, slots[0].card.loc.x + 168.);
        assert_eq!(slots[0].hit.size.h, PEEK);
    }

    #[test]
    fn many_cards_shrink_to_fit() {
        let ids = ids(30);
        let items = items(&ids);

        let right = slots(output(), &items, at(LimboEdge::Right), 0, |_| 0.);
        let last = right.last().unwrap().card;
        assert!(right[0].card.size.h < CARD_H);
        assert!(right[0].card.loc.y >= 0.);
        assert!(last.loc.y + last.size.h <= 1080.);

        let bottom = slots(output(), &items, at(LimboEdge::Bottom), 0, |_| 0.);
        let last = bottom.last().unwrap().card;
        assert!(bottom[0].card.size.h < CARD_H);
        assert!(bottom[0].card.loc.x >= 0.);
        assert!(last.loc.x + last.size.w <= 1920. + 0.01);
    }

    #[test]
    fn wide_windows_keep_their_aspect() {
        let size = card_size(Size::from((3000., 500.)), CARD_H);
        assert_eq!(size.w, CARD_MAX_W);
        assert_eq!(size.h, CARD_MAX_W / 6.);
    }

    #[test]
    fn window_under_hits_only_the_strip() {
        let mut drawer = Drawer::new();
        let ids = ids(1);
        drawer.sync(&ids);
        let items = items(&ids);
        let right = at(LimboEdge::Right);
        let y = 540.;
        assert_eq!(
            drawer.window_under(output(), &items, right, Point::from((1915., y))),
            Some(ids[0])
        );
        assert_eq!(
            drawer.window_under(output(), &items, right, Point::from((1900., y))),
            None
        );
        assert_eq!(
            drawer.window_under(output(), &items, right, Point::from((1915., 10.))),
            None
        );
    }

    #[test]
    fn popped_drawer_slides_every_card_out() {
        let mut drawer = Drawer::new();
        let ids = ids(2);
        drawer.sync(&ids);
        drawer.advance_animations(Duration::ZERO);

        drawer.set_popped(true);
        assert!(drawer.are_animations_ongoing());
        drawer.advance_animations(SLIDE);
        assert!(ids.iter().all(|id| drawer.extend[id] == 1.));
        assert!(!drawer.are_animations_ongoing());

        drawer.set_popped(false);
        drawer.advance_animations(SLIDE * 2);
        assert!(drawer.extend.is_empty());
        assert!(!drawer.are_animations_ongoing());
    }

    #[test]
    fn urgent_cards_peek_further() {
        let ids = ids(2);
        let mut items = items(&ids);
        items[1].urgent = true;
        let slots = slots(output(), &items, at(LimboEdge::Right), 0, |_| 0.);
        assert_eq!(slots[0].card.loc.x, 1920. - PEEK);
        assert_eq!(slots[1].card.loc.x, 1920. - URGENT_PEEK);
        assert_eq!(slots[1].hit.size.w, URGENT_PEEK);
    }

    #[test]
    fn too_many_cards_show_a_window_onto_them() {
        let ids = ids(30);
        let items = items(&ids);

        let shown = slots(output(), &items, at(LimboEdge::Right), 0, |_| 0.);
        assert!(shown.len() < 30);
        assert!(shown.iter().all(|slot| slot.card.size.h == MIN_CARD_H));
        let last = shown.last().unwrap().card;
        assert!(shown[0].card.loc.y >= 0.);
        assert!(last.loc.y + last.size.h <= 1080.);
        assert_eq!(shown[0].id, ids[0]);

        // An offset starts further in, and wraps round to the start.
        let n = shown.len();
        let shifted = slots(output(), &items, at(LimboEdge::Right), 1, |_| 0.);
        assert_eq!(shifted[0].id, ids[1]);
        let wrapped = slots(output(), &items, at(LimboEdge::Right), 29, |_| 0.);
        assert_eq!(wrapped[0].id, ids[29]);
        assert_eq!(wrapped[1].id, ids[0]);
        assert_eq!(wrapped.len(), n);
    }

    #[test]
    fn scroll_wraps_and_sync_keeps_it_in_range() {
        let mut drawer = Drawer::new();
        let ids = ids(3);
        drawer.sync(&ids);
        drawer.scroll(-1);
        assert_eq!(drawer.offset, 2);
        drawer.scroll(2);
        assert_eq!(drawer.offset, 1);

        // Losing a window keeps the offset in range.
        drawer.scroll(1);
        drawer.sync(&ids[..2]);
        assert_eq!(drawer.offset, 0);

        // A new window shows from the start.
        drawer.scroll(1);
        let mut more = vec![MappedId::next()];
        more.extend(&ids[..2]);
        drawer.sync(&more);
        assert_eq!(drawer.offset, 0);
    }

    #[test]
    fn caption_sits_beside_the_card() {
        let card = Rectangle::new(Point::from((1700., 500.)), Size::from((200., 100.)));
        let size = Size::from((300., 24.));
        assert_eq!(
            caption_loc(card, size, LimboEdge::Right, 8.),
            Point::from((1392., 538.))
        );
        let card = Rectangle::new(Point::from((20., 500.)), Size::from((200., 100.)));
        assert_eq!(
            caption_loc(card, size, LimboEdge::Left, 8.),
            Point::from((228., 538.))
        );
        let card = Rectangle::new(Point::from((800., 900.)), Size::from((200., 100.)));
        assert_eq!(
            caption_loc(card, size, LimboEdge::Bottom, 8.),
            Point::from((750., 868.))
        );
    }

    #[test]
    fn long_titles_are_cut() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("ééééé", 3), "ééé…");
    }

    #[test]
    fn sync_forgets_windows_that_left() {
        let mut drawer = Drawer::new();
        let ids = ids(2);
        drawer.sync(&ids);
        drawer.set_hovered(None);
        drawer.extend.insert(ids[0], 0.5);
        drawer.sync(&ids[1..]);
        assert_eq!(drawer.order(), [ids[1]]);
        assert!(!drawer.extend.contains_key(&ids[0]));
    }
}
