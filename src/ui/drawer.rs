//! The limbo drawer: windows in limbo peek out as thin thumbnails at the right edge.
//!
//! Hovering one slides it out. The slide times itself rather than using an `Animation`, so it
//! plays even with animations turned off.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use niri_config::LimboEdge;
use smithay::backend::renderer::element::Kind;
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::niri_render_elements;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::RenderCtx;
use crate::ui::mru::{push_window_thumbnail, ThumbnailElement};
use crate::window::mapped::MappedId;
use crate::window::Mapped;

/// Height of a card, in logical pixels, when they all fit.
const CARD_H: f64 = 120.;
/// Widest a card gets, in logical pixels.
const CARD_MAX_W: f64 = 240.;
/// Gap between cards, in logical pixels.
const GAP: f64 = 8.;
/// Share of the output height (or width, at the bottom) the cards may take up.
const MAX_STACK: f64 = 0.7;
/// How much of a card peeks out when it isn't hovered, in logical pixels.
const PEEK: f64 = 10.;
/// How long a card takes to slide all the way out or back in.
const SLIDE: Duration = Duration::from_millis(150);
/// Outline drawn behind each card, so it stands out on any background.
const OUTLINE: f64 = 1.;
const OUTLINE_COLOR: [f32; 4] = [0., 0., 0., 0.6];
/// Opacity of the cards.
const OPACITY: f32 = 0.8;
/// Gap between a slid-out card and the edge of the screen, in millimetres.
pub const MARGIN_MM: f64 = 5.;

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
    }
}

#[derive(Debug, Default)]
pub struct Drawer {
    /// Windows in the drawer, newest first.
    order: Vec<MappedId>,
    /// The card under the pointer.
    hovered: Option<(Output, MappedId)>,
    /// How far each card has slid out, from 0 to 1.
    extend: HashMap<MappedId, f64>,
    last_tick: Option<Duration>,
    /// Outline behind each card, kept so that it keeps its element id.
    outlines: RefCell<HashMap<MappedId, SolidColorBuffer>>,
}

impl Drawer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Matches the drawer to the windows in limbo, newest first.
    pub fn sync(&mut self, present: &[MappedId]) {
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

    /// Where the cards go on an output, for windows of these sizes in drawer order.
    pub fn slots(
        &self,
        output_size: Size<f64, Logical>,
        items: &[(MappedId, Size<f64, Logical>)],
        placement: DrawerPlacement,
    ) -> Vec<Slot> {
        slots(output_size, items, placement, |id| {
            self.extend.get(&id).copied().unwrap_or(0.)
        })
    }

    /// The card under this point, in output-local coordinates.
    pub fn window_under(
        &self,
        output_size: Size<f64, Logical>,
        items: &[(MappedId, Size<f64, Logical>)],
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

        let hovered = self.hovered();
        for id in &self.order {
            let extend = self.extend.entry(*id).or_insert(0.);
            if Some(*id) == hovered {
                *extend = (*extend + step).min(1.);
            } else {
                *extend = (*extend - step).max(0.);
            }
        }
        self.extend.retain(|_, extend| *extend > 0.);
    }

    pub fn are_animations_ongoing(&self) -> bool {
        let hovered = self.hovered();
        self.order.iter().any(|id| {
            let extend = self.extend.get(id).copied().unwrap_or(0.);
            if Some(*id) == hovered {
                extend < 1.
            } else {
                extend > 0.
            }
        })
    }

    /// Renders the cards on an output, for these windows in drawer order.
    pub fn render<R: NiriRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        output_size: Size<f64, Logical>,
        windows: &[&Mapped],
        placement: DrawerPlacement,
        scale: f64,
        push: &mut dyn FnMut(DrawerRenderElement<R>),
    ) {
        let items: Vec<_> = windows
            .iter()
            .map(|mapped| (mapped.id(), window_size(mapped)))
            .collect();
        let slots = self.slots(output_size, &items, placement);
        for (slot, mapped) in slots.into_iter().zip(windows) {
            push_window_thumbnail(
                ctx.r(),
                mapped,
                window_size(mapped),
                slot.card,
                scale,
                OPACITY,
                &mut |elem| push(DrawerRenderElement::Thumbnail(elem)),
            );

            let mut outline = slot.card;
            outline.loc -= Point::from((OUTLINE, OUTLINE));
            outline.size += Size::from((OUTLINE * 2., OUTLINE * 2.));
            let mut outlines = self.outlines.borrow_mut();
            let buffer = outlines
                .entry(slot.id)
                .or_insert_with(|| SolidColorBuffer::new(outline.size, OUTLINE_COLOR));
            buffer.resize(outline.size);
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
    items: &[(MappedId, Size<f64, Logical>)],
    placement: DrawerPlacement,
    extend: impl Fn(MappedId) -> f64,
) -> Vec<Slot> {
    if items.is_empty() {
        return Vec::new();
    }

    // At the left and right edges, cards stack top to bottom; at the bottom, left to right.
    let in_row = placement.edge == LimboEdge::Bottom;
    let along = |size: Size<f64, Logical>| if in_row { size.w } else { size.h };

    // Shrink the cards if they don't all fit.
    let gaps = GAP * (items.len() as f64 - 1.);
    let room = along(output_size) * MAX_STACK;
    let total = |card_h: f64| -> f64 {
        items
            .iter()
            .map(|(_, size)| along(card_size(*size, card_h)))
            .sum::<f64>()
            + gaps
    };
    let mut card_h = CARD_H;
    let full = total(card_h);
    if full > room {
        card_h = (card_h * (room - gaps) / (full - gaps)).max(1.);
    }

    let mut pos = ((along(output_size) - total(card_h)) / 2.).max(0.);
    let output = Rectangle::from_size(output_size);

    items
        .iter()
        .map(|&(id, size)| {
            let size = card_size(size, card_h);
            // How far into the screen the card reaches.
            let depth = if in_row { size.h } else { size.w };
            let shown = PEEK + extend(id).clamp(0., 1.) * (depth + placement.margin - PEEK);

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

    fn items(ids: &[MappedId]) -> Vec<(MappedId, Size<f64, Logical>)> {
        ids.iter()
            .map(|id| (*id, Size::from((800., 600.))))
            .collect()
    }

    #[test]
    fn right_cards_stack_centered_and_peek() {
        let ids = ids(2);
        let slots = slots(output(), &items(&ids), at(LimboEdge::Right), |_| 0.);

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

        let right = slots(output(), &items, at(LimboEdge::Right), |_| 1.)[0];
        assert_eq!(right.card.loc.x + right.card.size.w, 1920. - MARGIN);
        // The gap to the edge still counts as the card.
        assert_eq!(right.hit.loc.x, right.card.loc.x);
        assert_eq!(right.hit.loc.x + right.hit.size.w, 1920.);

        let left = slots(output(), &items, at(LimboEdge::Left), |_| 1.)[0];
        assert_eq!(left.card.loc.x, MARGIN);
        assert_eq!(left.hit.loc.x, 0.);
        assert_eq!(left.hit.size.w, MARGIN + left.card.size.w);

        let bottom = slots(output(), &items, at(LimboEdge::Bottom), |_| 1.)[0];
        assert_eq!(bottom.card.loc.y + bottom.card.size.h, 1080. - MARGIN);
        assert_eq!(bottom.hit.loc.y + bottom.hit.size.h, 1080.);
    }

    #[test]
    fn left_cards_peek_from_the_left() {
        let ids = ids(1);
        let slot = slots(output(), &items(&ids), at(LimboEdge::Left), |_| 0.)[0];
        assert_eq!(slot.card.loc.x + slot.card.size.w, PEEK);
        assert_eq!(slot.hit, Rectangle::new((0., slot.card.loc.y).into(), (PEEK, 120.).into()));
    }

    #[test]
    fn bottom_cards_form_a_centered_row() {
        let ids = ids(2);
        let slots = slots(output(), &items(&ids), at(LimboEdge::Bottom), |_| 0.);
        assert_eq!(slots[0].card.loc.y, 1080. - PEEK);
        assert_eq!(slots[0].card.loc.x, (1920. - 328.) / 2.);
        assert_eq!(slots[1].card.loc.x, slots[0].card.loc.x + 168.);
        assert_eq!(slots[0].hit.size.h, PEEK);
    }

    #[test]
    fn many_cards_shrink_to_fit() {
        let ids = ids(30);
        let items = items(&ids);

        let right = slots(output(), &items, at(LimboEdge::Right), |_| 0.);
        let last = right.last().unwrap().card;
        assert!(right[0].card.size.h < CARD_H);
        assert!(right[0].card.loc.y >= 0.);
        assert!(last.loc.y + last.size.h <= 1080.);

        let bottom = slots(output(), &items, at(LimboEdge::Bottom), |_| 0.);
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
