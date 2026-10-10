//! The scratchpad drawer: windows on workspace 0 peek out as thin thumbnails at the right edge.
//!
//! Hovering one slides it out. The slide times itself rather than using an `Animation`, so it
//! plays even with animations turned off.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

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
/// Share of the output height the cards may take up.
const MAX_STACK: f64 = 0.7;
/// How much of a card peeks out when it isn't hovered, in logical pixels.
const PEEK: f64 = 10.;
/// How long a card takes to slide all the way out or back in.
const SLIDE: Duration = Duration::from_millis(150);
/// Outline drawn behind each card, so it stands out on any background.
const OUTLINE: f64 = 1.;
const OUTLINE_COLOR: [f32; 4] = [0., 0., 0., 0.6];

/// Where a card goes on the output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    pub id: MappedId,
    /// The whole card, partly past the right edge.
    pub card: Rectangle<f64, Logical>,
    /// The part of the card on the output.
    pub visible: Rectangle<f64, Logical>,
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

    /// Puts a window that just went into the scratchpad first.
    pub fn remember(&mut self, id: MappedId) {
        self.order.retain(|x| *x != id);
        self.order.insert(0, id);
    }

    /// Forgets a window taken out of the scratchpad.
    pub fn forget(&mut self, id: MappedId) {
        self.order.retain(|x| *x != id);
        self.extend.remove(&id);
        if self.hovered.as_ref().is_some_and(|(_, x)| *x == id) {
            self.hovered = None;
        }
    }

    /// Matches the drawer to the windows on workspace 0, in tree order.
    ///
    /// Windows that left are dropped, and windows that got there some other way go last.
    pub fn sync(&mut self, present: &[MappedId]) {
        self.order.retain(|id| present.contains(id));
        for id in present {
            if !self.order.contains(id) {
                self.order.push(*id);
            }
        }
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
    ) -> Vec<Slot> {
        slots(output_size, items, |id| {
            self.extend.get(&id).copied().unwrap_or(0.)
        })
    }

    /// The card under this point, in output-local coordinates.
    pub fn window_under(
        &self,
        output_size: Size<f64, Logical>,
        items: &[(MappedId, Size<f64, Logical>)],
        pos: Point<f64, Logical>,
    ) -> Option<MappedId> {
        self.slots(output_size, items)
            .into_iter()
            .find(|slot| slot.visible.contains(pos))
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
        scale: f64,
        push: &mut dyn FnMut(DrawerRenderElement<R>),
    ) {
        let items: Vec<_> = windows
            .iter()
            .map(|mapped| (mapped.id(), window_size(mapped)))
            .collect();
        for (slot, mapped) in self.slots(output_size, &items).into_iter().zip(windows) {
            push_window_thumbnail(
                ctx.r(),
                mapped,
                window_size(mapped),
                slot.card,
                scale,
                1.,
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
                SolidColorRenderElement::from_buffer(buffer, outline.loc, 1., Kind::Unspecified),
            ));
        }
    }
}

/// Size of a window for its card.
pub fn window_size(mapped: &Mapped) -> Size<f64, Logical> {
    let size = crate::layout::LayoutElement::size(mapped).to_f64();
    Size::from((size.w.max(1.), size.h.max(1.)))
}

fn slots(
    output_size: Size<f64, Logical>,
    items: &[(MappedId, Size<f64, Logical>)],
    extend: impl Fn(MappedId) -> f64,
) -> Vec<Slot> {
    if items.is_empty() {
        return Vec::new();
    }

    // Shrink the cards if they don't all fit.
    let n = items.len() as f64;
    let room = output_size.h * MAX_STACK;
    let card_h = f64::min(CARD_H, (room - GAP * (n - 1.)) / n).max(1.);

    let total = card_h * n + GAP * (n - 1.);
    let mut y = ((output_size.h - total) / 2.).max(0.);

    items
        .iter()
        .map(|&(id, size)| {
            let card_w = (size.w * card_h / size.h).clamp(PEEK, CARD_MAX_W);
            let shown = PEEK + extend(id).clamp(0., 1.) * (card_w - PEEK);
            let x = output_size.w - shown;

            let card = Rectangle::new(Point::from((x, y)), Size::from((card_w, card_h)));
            let visible = Rectangle::new(Point::from((x, y)), Size::from((shown, card_h)));
            y += card_h + GAP;
            Slot { id, card, visible }
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

    #[test]
    fn cards_stack_centered_and_peek() {
        let ids = ids(2);
        let items = [
            (ids[0], Size::from((800., 600.))),
            (ids[1], Size::from((800., 600.))),
        ];
        let slots = slots(output(), &items, |_| 0.);

        // 120 high, 160 wide, 8 apart, centered.
        assert_eq!(slots[0].card.size, Size::from((160., 120.)));
        assert_eq!(slots[0].card.loc.y, (1080. - 248.) / 2.);
        assert_eq!(slots[1].card.loc.y, slots[0].card.loc.y + 128.);

        // Only PEEK shows.
        assert_eq!(slots[0].card.loc.x, 1920. - PEEK);
        assert_eq!(slots[0].visible.size.w, PEEK);
    }

    #[test]
    fn hovered_card_slides_out() {
        let ids = ids(1);
        let items = [(ids[0], Size::from((800., 600.)))];
        let slots = slots(output(), &items, |_| 1.);
        assert_eq!(slots[0].card.loc.x, 1920. - 160.);
        assert_eq!(slots[0].visible, slots[0].card);
    }

    #[test]
    fn many_cards_shrink_to_fit() {
        let ids = ids(20);
        let items: Vec<_> = ids
            .iter()
            .map(|id| (*id, Size::from((800., 600.))))
            .collect();
        let slots = slots(output(), &items, |_| 0.);
        let last = slots.last().unwrap().card;
        assert!(slots[0].card.size.h < CARD_H);
        assert!(slots[0].card.loc.y >= 0.);
        assert!(last.loc.y + last.size.h <= 1080.);
    }

    #[test]
    fn window_under_hits_only_the_visible_strip() {
        let mut drawer = Drawer::new();
        let ids = ids(1);
        drawer.sync(&ids);
        let items = [(ids[0], Size::from((800., 600.)))];
        let y = 540.;
        assert_eq!(
            drawer.window_under(output(), &items, Point::from((1915., y))),
            Some(ids[0])
        );
        assert_eq!(
            drawer.window_under(output(), &items, Point::from((1900., y))),
            None
        );
        assert_eq!(
            drawer.window_under(output(), &items, Point::from((1915., 10.))),
            None
        );
    }

    #[test]
    fn sync_keeps_order() {
        let mut drawer = Drawer::new();
        let ids = ids(3);
        drawer.remember(ids[0]);
        drawer.remember(ids[1]);
        assert_eq!(drawer.order(), [ids[1], ids[0]]);

        // 2 arrived some other way and goes last; 0 left.
        drawer.sync(&[ids[2], ids[1]]);
        assert_eq!(drawer.order(), [ids[1], ids[2]]);
    }
}
