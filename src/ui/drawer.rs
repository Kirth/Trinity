//! The limbo drawer: windows in limbo peek out as thin thumbnails at the right edge.
//!
//! Hovering one slides it out and shows its title.
//!
//! With limbo off, the same cards show the windows in the strip of the active workspace instead,
//! always out, in the band the layout leaves free for them. The slide times itself rather than
//! using an `Animation`, so it plays even with animations turned off.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use niri_config::{Color, CornerRadius, LimboEdge};
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::layout::workspace::STRIP_WIDTH;
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::RenderCtx;
pub use crate::ui::cards::{
    along_edge, insertion_point, is_past_card, window_size,
    CardRenderElement as DrawerRenderElement, Insert, Item, Slot,
};
use crate::ui::cards::{card_size, CardDecorations, CardInfo, GAP, MIN_CARD_H};
use crate::ui::mru::push_window_thumbnail;
use crate::window::mapped::MappedId;
use crate::window::Mapped;

/// Height of a card, in logical pixels, when they all fit.
const CARD_H: f64 = 120.;
/// Share of the output height (or width, at the bottom) the cards may take up.
const MAX_STACK: f64 = 0.7;
/// How much of a card peeks out when it isn't hovered, in logical pixels.
const PEEK: f64 = 10.;
/// How much of a card peeks out when its window wants attention.
const URGENT_PEEK: f64 = 24.;
/// How long a card takes to slide all the way out or back in.
const SLIDE: Duration = Duration::from_millis(150);
/// Opacity of the cards.
const OPACITY: f32 = 0.8;
/// Gap between a slid-out card and the edge of the screen, in millimetres.
pub const MARGIN_MM: f64 = 5.;
/// How far a card has to slide out before its title shows.
const CAPTION_EXTEND: f64 = 0.5;
/// Height of a card in the overview row, as a share of the height of the row.
const ROW_CARD_SHARE: f64 = 0.7;
/// Share of the width of the overview row the cards may take up.
const ROW_MAX_STACK: f64 = 0.9;
/// How much wider than high a card in the overview row gets, at most.
const ROW_MAX_ASPECT: f64 = 2.5;
/// How long the pointer has to stay against the edge beside the cards to slide them all out.
const EDGE_DWELL: Duration = Duration::from_millis(150);
/// Room between the cards in a strip and the edges of its band, in logical pixels.
const STRIP_PADDING: f64 = 14.;
/// How much of a card tucked behind another in a stack shows, as a share of its height.
const STACK_PEEK: f64 = 0.35;
/// Least of a tucked card that shows, in logical pixels.
const STACK_MIN_PEEK: f64 = 24.;
/// How much smaller each card further back in a stack is, as a share of its size.
const STACK_SHRINK: f64 = 0.04;
/// How long a stack takes to spread out or fold back.
const SPREAD: Duration = Duration::from_millis(180);
/// How much a card in a strip grows under the pointer, as a share of its size.
const GROW: f64 = 0.06;
/// How long a card takes to grow or shrink back.
const GROW_TIME: Duration = Duration::from_millis(120);
/// Width of the rim around the panel behind a strip, in logical pixels.
const PANEL_RIM: f32 = 1.;

/// Where the pointer is, as far as sliding out all the cards goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgePointer {
    /// Against the screen edge, beside the cards.
    AtEdge,
    /// Over the drawer: the cards and the gaps between and beside them.
    InDrawer,
    Away,
}

/// Where the drawer goes on an output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawerPlacement {
    pub edge: LimboEdge,
    /// Gap between a slid-out card and the edge, in logical pixels.
    pub margin: f64,
    /// In the overview: the band below the workspaces where the cards show in a row, in
    /// output-local coordinates.
    pub overview: Option<Rectangle<f64, Logical>>,
    /// For a workspace strip: the band at the right edge where the cards show in a column, in
    /// output-local coordinates.
    pub strip: Option<Rectangle<f64, Logical>>,
    /// For a workspace strip: where its panel goes, from the band to the right edge and from the
    /// top to the bottom of the screen, in output-local coordinates.
    pub strip_panel: Option<Rectangle<f64, Logical>>,
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
    /// The card being dragged along the drawer on an output, with where it starts along the
    /// edge.
    dragging: Option<(Output, MappedId, f64)>,
    /// How far each card has slid out on each output, from 0 to 1.
    extend: HashMap<(Output, MappedId), f64>,
    /// Outputs the cards animate on, as of the last tick.
    outputs: Vec<Output>,
    last_tick: Option<Duration>,
    /// Outlines, titles and app icons of the cards.
    decorations: CardDecorations,
    /// Panels behind strips, with their rims, by output and position, so that they keep their
    /// element ids.
    panels: RefCell<HashMap<(String, i64, i64), [BorderRenderElement; 2]>>,
    /// The output whose edge the pointer has been against, and since when, until the cards slide
    /// out.
    edge_since: Option<(Output, Duration)>,
    /// The output where pushing against the edge slid out all the cards.
    revealed: Option<Output>,
    /// The stack of cards spread out on an output, by app.
    spread: Option<(Output, u64)>,
    /// How far each stack has spread out on each output, from 0 to 1.
    spread_progress: HashMap<(Output, u64), f64>,
    /// How far each card in a strip has grown under the pointer on each output, from 0 to 1.
    grow: HashMap<(Output, MappedId), f64>,
}

impl Drawer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Matches the drawer to the windows in limbo, newest first.
    pub fn sync(&mut self, present: &[MappedId]) {
        // Show a newly poofed window, which goes first.
        if present.first().is_some_and(|id| !self.order.contains(id)) {
            self.offset = 0;
        }
        self.offset = self.offset.checked_rem(present.len()).unwrap_or(0);
        self.order = present.to_vec();
        self.extend.retain(|(_, id), _| present.contains(id));
        self.decorations.retain(|id| present.contains(&id));
        if self
            .hovered
            .as_ref()
            .is_some_and(|(_, id)| !present.contains(id))
        {
            self.hovered = None;
        }
        if self
            .dragging
            .as_ref()
            .is_some_and(|(_, id, _)| !present.contains(id))
        {
            self.dragging = None;
        }
        if present.is_empty() {
            self.edge_since = None;
            self.revealed = None;
        }
    }

    /// Updates where the pointer is, on this output, for sliding out all the cards. Returns
    /// whether anything changed.
    pub fn set_edge_pointer(&mut self, output: &Output, state: EdgePointer, now: Duration) -> bool {
        let before = (self.edge_since.clone(), self.revealed.clone());
        match state {
            EdgePointer::AtEdge => {
                if self.revealed.as_ref() != Some(output) {
                    self.revealed = None;
                    if self.edge_since.as_ref().is_none_or(|(o, _)| o != output) {
                        self.edge_since = Some((output.clone(), now));
                    }
                }
            }
            // Moving onto the cards keeps them out, but leaving the edge early calls it off.
            EdgePointer::InDrawer => {
                self.edge_since = None;
                if self.revealed.as_ref().is_some_and(|o| o != output) {
                    self.revealed = None;
                }
            }
            EdgePointer::Away => {
                self.edge_since = None;
                self.revealed = None;
            }
        }
        before.0 != self.edge_since || before.1 != self.revealed
    }

    /// The pointer isn't on any output.
    pub fn clear_edge_pointer(&mut self) -> bool {
        let changed = self.edge_since.is_some() || self.revealed.is_some();
        self.edge_since = None;
        self.revealed = None;
        changed
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

    /// Drags a card along the drawer on an output, to this position along the edge, or lets go
    /// of it.
    pub fn set_dragging(&mut self, dragging: Option<(Output, MappedId, f64)>) {
        self.dragging = dragging;
    }

    pub fn dragging(&self) -> Option<MappedId> {
        self.dragging.as_ref().map(|(_, id, _)| *id)
    }

    /// The card being dragged on this output.
    fn dragging_on(&self, output: &Output) -> Option<(MappedId, f64)> {
        self.dragging
            .as_ref()
            .filter(|(o, _, _)| o == output)
            .map(|(_, id, at)| (*id, *at))
    }

    /// The card under the pointer on this output.
    fn hovered_on(&self, output: &Output) -> Option<MappedId> {
        self.hovered
            .as_ref()
            .filter(|(o, _)| o == output)
            .map(|(_, id)| *id)
    }

    /// Whether this card should be slid out on this output.
    fn is_out(&self, output: &Output, id: MappedId) -> bool {
        self.popped
            || self.revealed.as_ref() == Some(output)
            || self.hovered_on(output) == Some(id)
            || self
                .dragging_on(output)
                .is_some_and(|(dragged, _)| dragged == id)
    }

    fn extend_of(&self, output: &Output, id: MappedId) -> f64 {
        self.extend
            .get(&(output.clone(), id))
            .copied()
            .unwrap_or(0.)
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
        output: &Output,
        output_size: Size<f64, Logical>,
        items: &[Item],
        placement: DrawerPlacement,
    ) -> Vec<Slot> {
        if let Some(band) = placement.overview {
            return overview_slots(band, items, self.offset, self.dragging_on(output));
        }
        if let Some(band) = placement.strip {
            return strip_slots(
                band,
                items,
                self.offset,
                self.dragging_on(output),
                |group| self.spread_of(output, group),
            );
        }
        slots(
            output_size,
            items,
            placement,
            self.offset,
            self.dragging_on(output),
            |id| self.extend_of(output, id),
        )
    }

    /// Whether only some of the cards fit on an output.
    pub fn overflows(
        &self,
        output: &Output,
        output_size: Size<f64, Logical>,
        items: &[Item],
        placement: DrawerPlacement,
    ) -> bool {
        self.slots(output, output_size, items, placement).len() < items.len()
    }

    /// The card under this point, in output-local coordinates.
    pub fn window_under(
        &self,
        output: &Output,
        output_size: Size<f64, Logical>,
        items: &[Item],
        placement: DrawerPlacement,
        pos: Point<f64, Logical>,
    ) -> Option<MappedId> {
        self.slots(output, output_size, items, placement)
            .into_iter()
            .find(|slot| slot.hit.contains(pos))
            .map(|slot| slot.id)
    }

    /// Spreads out a stack of cards on an output, or lets it fold back. Returns whether it
    /// changed.
    pub fn set_spread(&mut self, new: Option<(Output, u64)>) -> bool {
        if self.spread == new {
            return false;
        }
        self.spread = new;
        true
    }

    /// The stack spread out on this output.
    pub fn spread_on(&self, output: &Output) -> Option<u64> {
        self.spread
            .as_ref()
            .filter(|(o, _)| o == output)
            .map(|(_, group)| *group)
    }

    fn spread_of(&self, output: &Output, group: u64) -> f64 {
        self.spread_progress
            .get(&(output.clone(), group))
            .copied()
            .unwrap_or(0.)
    }

    /// Advances the slides of the cards on these outputs.
    pub fn advance_animations(&mut self, now: Duration, outputs: &[Output]) {
        let dt = self
            .last_tick
            .map_or(Duration::ZERO, |last| now.saturating_sub(last));
        self.last_tick = Some(now);
        let step = dt.as_secs_f64() / SLIDE.as_secs_f64();

        // Stacks spread out and fold back.
        if let Some(key) = &self.spread {
            self.spread_progress.entry(key.clone()).or_insert(0.);
        }
        let spread_step = dt.as_secs_f64() / SPREAD.as_secs_f64();
        for (key, progress) in &mut self.spread_progress {
            if self.spread.as_ref() == Some(key) {
                *progress = (*progress + spread_step).min(1.);
            } else {
                *progress = (*progress - spread_step).max(0.);
            }
        }
        let spread = &self.spread;
        self.spread_progress.retain(|key, progress| {
            (*progress > 0. || spread.as_ref() == Some(key)) && outputs.contains(&key.0)
        });

        // The card under the pointer grows, and the others shrink back.
        if let Some(key) = &self.hovered {
            self.grow.entry(key.clone()).or_insert(0.);
        }
        let grow_step = dt.as_secs_f64() / GROW_TIME.as_secs_f64();
        for (key, grow) in &mut self.grow {
            if self.hovered.as_ref() == Some(key) {
                *grow = (*grow + grow_step).min(1.);
            } else {
                *grow = (*grow - grow_step).max(0.);
            }
        }
        let hovered = &self.hovered;
        self.grow.retain(|key, grow| {
            (*grow > 0. || hovered.as_ref() == Some(key)) && outputs.contains(&key.0)
        });

        // Held against the edge long enough: slide all the cards out.
        if let Some((output, since)) = &self.edge_since {
            if now.saturating_sub(*since) >= EDGE_DWELL {
                self.revealed = Some(output.clone());
                self.edge_since = None;
            }
        }

        self.outputs = outputs.to_vec();
        for output in outputs {
            for &id in &self.order {
                let out = self.is_out(output, id);
                let extend = self.extend.entry((output.clone(), id)).or_insert(0.);
                if out {
                    *extend = (*extend + step).min(1.);
                } else {
                    *extend = (*extend - step).max(0.);
                }
            }
        }
        self.extend
            .retain(|(output, _), extend| *extend > 0. && outputs.contains(output));
    }

    /// Whether cards are still sliding on this output.
    pub fn are_animations_ongoing(&self, output: &Output) -> bool {
        // Keep frames coming to notice the end of the edge dwell.
        if self.edge_since.as_ref().is_some_and(|(o, _)| o == output) {
            return true;
        }
        if !self.outputs.contains(output) {
            return false;
        }
        let spreading = self.spread_progress.iter().any(|((o, group), progress)| {
            o == output
                && if self.spread_on(output) == Some(*group) {
                    *progress < 1.
                } else {
                    *progress > 0.
                }
        });
        if spreading
            || (self
                .spread_on(output)
                .is_some_and(|g| self.spread_of(output, g) < 1.))
        {
            return true;
        }
        let growing = self.grow.iter().any(|((o, id), grow)| {
            o == output
                && if self.hovered_on(output) == Some(*id) {
                    *grow < 1.
                } else {
                    *grow > 0.
                }
        });
        if growing {
            return true;
        }
        self.order.iter().any(|&id| {
            let extend = self.extend_of(output, id);
            if self.is_out(output, id) {
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
        output: &Output,
        output_size: Size<f64, Logical>,
        windows: &[&Mapped],
        placement: DrawerPlacement,
        urgent_color: Color,
        scale: f64,
        alpha: f32,
        push: &mut dyn FnMut(DrawerRenderElement<R>),
    ) {
        let items: Vec<_> = windows.iter().map(|mapped| Item::new(mapped)).collect();
        let slots = self.slots(output, output_size, &items, placement);

        // In the overview, the cards fade in with it and don't slide, and in a strip they're
        // always out.
        let in_overview = placement.overview.is_some() || placement.strip.is_some();
        let opacity = if in_overview { alpha } else { OPACITY * alpha };

        // The title of the hovered card, on top of everything.
        let hovered = self
            .hovered_on(output)
            .filter(|id| in_overview || self.extend_of(output, *id) > CAPTION_EXTEND);
        let hovered = hovered.and_then(|id| {
            let slot = slots.iter().find(|slot| slot.id == id)?;
            let mapped = windows.iter().find(|mapped| mapped.id() == id)?;
            Some((slot, mapped))
        });
        if let Some((slot, mapped)) = hovered {
            let info = CardInfo::new(mapped);
            if let Some(elem) = self.decorations.render_caption(
                ctx.renderer,
                mapped,
                info.text.as_deref(),
                slot.card,
                placement.edge,
                scale,
            ) {
                push(DrawerRenderElement::Texture(elem));
            }
        }

        // The dragged card goes first, to draw above the others, then the one under the
        // pointer.
        let mut slots = slots;
        let hovered = self.hovered_on(output);
        let dragged = self.dragging_on(output).map(|(id, _)| id);
        slots.sort_by_key(|slot| (Some(slot.id) != dragged, Some(slot.id) != hovered));
        for mut slot in slots {
            // In a strip, the card under the pointer grows a little, eased out.
            if placement.strip.is_some() && !slot.tucked {
                let grow = self
                    .grow
                    .get(&(output.clone(), slot.id))
                    .copied()
                    .unwrap_or(0.);
                slot.card = grown(slot.card, 1. - (1. - grow).powi(3));
            }
            let Some(mapped) = windows.iter().find(|mapped| mapped.id() == slot.id) else {
                continue;
            };
            let urgent = crate::layout::LayoutElement::is_urgent(*mapped);
            let info = CardInfo::new(mapped);
            // What providers want shown, above the card.
            if let Some(badge) = &info.badge {
                if let Some(elem) = self.decorations.render_badge(
                    ctx.renderer,
                    badge,
                    slot.card,
                    false,
                    scale,
                    opacity,
                ) {
                    push(DrawerRenderElement::Texture(elem));
                }
            }
            // How many cards a stack holds, on its front card.
            if slot.stack > 1 {
                if let Some(elem) = self.decorations.render_badge(
                    ctx.renderer,
                    &slot.stack.to_string(),
                    slot.card,
                    true,
                    scale,
                    opacity,
                ) {
                    push(DrawerRenderElement::Texture(elem));
                }
            }
            if let Some(progress) = info.progress {
                push(DrawerRenderElement::SolidColor(
                    self.decorations
                        .render_progress(slot.id, slot.card, progress, opacity),
                ));
            }
            // The app icon, above its card, once per stack.
            let icon = (!slot.tucked)
                .then(|| {
                    self.decorations.render_icon(
                        ctx.renderer,
                        mapped,
                        slot.card,
                        placement.edge,
                        scale,
                        opacity,
                    )
                })
                .flatten();
            if let Some(elem) = icon {
                push(DrawerRenderElement::Texture(elem));
            }
            push_window_thumbnail(
                ctx.r(),
                mapped,
                window_size(mapped),
                slot.card,
                scale,
                opacity,
                &mut |elem| push(DrawerRenderElement::Thumbnail(elem)),
            );

            push(DrawerRenderElement::SolidColor(
                self.decorations.render_outline(
                    slot.id,
                    slot.card,
                    urgent.then_some(urgent_color),
                    info.status,
                    opacity,
                ),
            ));
        }
    }

    /// Renders the panel behind a strip, with its rim on top. It goes below the top layer, so
    /// that a bar shows over it, but above the windows.
    #[allow(clippy::too_many_arguments)]
    pub fn render_panel(
        &self,
        renderer: &mut impl NiriRenderer,
        output: &Output,
        placement: DrawerPlacement,
        style: &niri_config::Strip,
        scale: f64,
        alpha: f32,
        push: &mut dyn FnMut(BorderRenderElement),
    ) {
        let Some(area) = placement.strip_panel else {
            return;
        };
        if style.off || !BorderRenderElement::has_shader(renderer) {
            return;
        }
        // Scaled down with the band, as in the overview.
        let zoom = area.size.w / STRIP_WIDTH;
        let panel = panel_rect(area, zoom)
            .to_physical_precise_round(scale)
            .to_logical(scale);

        // Round only the corners facing into the screen.
        let r = style.corner_radius as f32 * zoom as f32;
        let radius = CornerRadius {
            top_left: r,
            top_right: 0.,
            bottom_right: 0.,
            bottom_left: r,
        }
        .fit_to(panel.size.w as f32, panel.size.h as f32);
        let (from, to, angle, interpolation) = match style.background_gradient {
            Some(gradient) => (
                gradient.from,
                gradient.to,
                ((gradient.angle as f32) - 90.).to_radians(),
                gradient.in_,
            ),
            None => (
                style.background_color,
                style.background_color,
                0.,
                Default::default(),
            ),
        };
        let area = Rectangle::from_size(panel.size);

        let mut panels = self.panels.borrow_mut();
        // Positions change all the time while the overview opens; don't keep them all.
        if panels.len() > 32 {
            panels.clear();
        }
        let key = (
            output.name(),
            panel.loc.x.round() as i64,
            panel.loc.y.round() as i64,
        );
        let [background, rim] = panels
            .entry(key)
            .or_insert_with(|| [BorderRenderElement::empty(), BorderRenderElement::empty()]);
        background.update(
            panel.size,
            area,
            interpolation,
            from,
            to,
            angle,
            area,
            0.,
            radius,
            scale as f32,
            alpha,
        );
        rim.update(
            panel.size,
            area,
            Default::default(),
            style.rim_color,
            style.rim_color,
            0.,
            area,
            PANEL_RIM,
            radius,
            scale as f32,
            alpha,
        );
        push(rim.clone().with_location(panel.loc));
        push(background.clone().with_location(panel.loc));
    }
}

/// Where the pointer is, at this output-local position, for sliding out all the cards.
pub fn edge_pointer(
    edge: LimboEdge,
    output_size: Size<f64, Logical>,
    slots: &[Slot],
    pos: Point<f64, Logical>,
) -> EdgePointer {
    let Some(first) = slots.first() else {
        return EdgePointer::Away;
    };

    // Beside the cards: between the start of the first and the end of the last.
    let along = along_edge(edge, pos);
    let (start, end) = slots
        .iter()
        .fold((f64::MAX, f64::MIN), |(start, end), slot| {
            let card_start = along_edge(edge, slot.card.loc);
            let card_end = along_edge(edge, slot.card.loc + slot.card.size.to_point());
            (start.min(card_start), end.max(card_end))
        });
    let at_edge = match edge {
        LimboEdge::Right => pos.x >= output_size.w - 1.,
        LimboEdge::Left => pos.x < 1.,
        LimboEdge::Bottom => pos.y >= output_size.h - 1.,
    };
    if at_edge && (start..=end).contains(&along) {
        return EdgePointer::AtEdge;
    }

    let area = slots
        .iter()
        .fold(first.hit, |area, slot| area.merge(slot.hit));
    if area.contains(pos) {
        EdgePointer::InDrawer
    } else {
        EdgePointer::Away
    }
}

/// Where the cards go in the overview: in a row centred in the band below the workspaces, left
/// to right in drawer order.
fn overview_slots(
    band: Rectangle<f64, Logical>,
    items: &[Item],
    offset: usize,
    drag: Option<(MappedId, f64)>,
) -> Vec<Slot> {
    if items.is_empty() {
        return Vec::new();
    }

    let size_of = |item: &Item, card_h: f64| {
        let aspect = (item.size.w / item.size.h).min(ROW_MAX_ASPECT);
        Size::from(((aspect * card_h).max(1.), card_h))
    };
    let total = |items: &[&Item], card_h: f64| -> f64 {
        items
            .iter()
            .map(|item| size_of(item, card_h).w)
            .sum::<f64>()
            + GAP * (items.len() as f64 - 1.)
    };

    // Shrink the cards if they don't all fit, and past the smallest size, show as many as fit
    // starting at the offset, like the drawer does.
    let room = band.size.w * ROW_MAX_STACK;
    let mut shown_items: Vec<&Item> = items.iter().collect();
    let mut card_h = (band.size.h * ROW_CARD_SHARE).max(1.);
    let full = total(&shown_items, card_h);
    if full > room {
        let gaps = GAP * (items.len() as f64 - 1.);
        card_h = (card_h * (room - gaps) / (full - gaps)).max(1.);

        let min_h = MIN_CARD_H.min(band.size.h * ROW_CARD_SHARE);
        if card_h < min_h {
            card_h = min_h;
            let n = items.len();
            shown_items.clear();
            let mut used = 0.;
            for i in 0..n {
                let item = &items[(offset + i) % n];
                let len = size_of(item, card_h).w;
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

    let mut next = band.loc.x + ((band.size.w - total(&shown_items, card_h)) / 2.).max(0.);
    let y = band.loc.y + (band.size.h - card_h) / 2.;

    shown_items
        .into_iter()
        .map(|item| {
            let id = item.id;
            let size = size_of(item, card_h);

            // A dragged card follows the pointer, leaving its place in the order empty.
            let x = match drag {
                Some((dragged, at)) if dragged == id => at.clamp(
                    band.loc.x,
                    (band.loc.x + band.size.w - size.w).max(band.loc.x),
                ),
                _ => next,
            };

            let card = Rectangle::new(Point::from((x, y)), size);
            next += size.w + GAP;
            Slot {
                id,
                card,
                hit: card,
                group: item.group,
                stack: 0,
                tucked: false,
            }
        })
        .collect()
}

/// Where the panel behind a strip goes: over the whole panel area, and past its top, right and
/// bottom edges by the width of the rim, so that only the rim facing into the screen shows.
fn panel_rect(area: Rectangle<f64, Logical>, zoom: f64) -> Rectangle<f64, Logical> {
    let rim = f64::from(PANEL_RIM) * zoom;
    Rectangle::new(
        area.loc - Point::from((0., rim)),
        area.size + Size::from((rim, rim * 2.)),
    )
}

/// Where the cards go in a workspace strip: in a column from the top of the band, as wide as the
/// band allows. The cards of one app stack together, like a pile of paper: the newest in front,
/// the others tucked behind it showing their bottom.
///
/// A stack spreads out, by how far `spread` says, over the stacks below it rather than pushing
/// them away, so that nothing else moves.
fn strip_slots(
    band: Rectangle<f64, Logical>,
    items: &[Item],
    offset: usize,
    drag: Option<(MappedId, f64)>,
    spread: impl Fn(u64) -> f64,
) -> Vec<Slot> {
    if items.is_empty() {
        return Vec::new();
    }

    // Scaled down with the band, as in the overview.
    let zoom = band.size.w / STRIP_WIDTH;
    let pad = STRIP_PADDING * zoom;
    let w = (band.size.w - 2. * pad).max(1.);
    let room = (band.size.h - 2. * pad).max(1.);
    let top = band.loc.y + pad;
    let bottom_edge = band.loc.y + band.size.h - pad;

    // The cards of each app, in the order of their newest window, newest first within.
    let mut groups: Vec<Vec<&Item>> = Vec::new();
    for item in items {
        match groups.iter_mut().find(|g| g[0].group == item.group) {
            Some(group) => group.push(item),
            None => groups.push(vec![item]),
        }
    }

    // Each card is as wide as the band, at most as high as wide; further back in a stack, a bit
    // smaller.
    let size_of = |item: &Item, depth: usize, scale: f64| {
        let h = (w * item.size.h / item.size.w).clamp(1., w) * scale;
        let size: Size<f64, Logical> = Size::from(((h * item.size.w / item.size.h).min(w), h));
        let shrink = (1. - STACK_SHRINK * depth as f64).max(0.5);
        size.upscale(shrink)
    };
    let peek = |h: f64| (h * STACK_PEEK).max(STACK_MIN_PEEK * zoom).min(h);
    let group_len = |group: &[&Item], scale: f64| -> f64 {
        let mut len = size_of(group[0], 0, scale).h;
        for (depth, item) in group.iter().enumerate().skip(1) {
            len += peek(size_of(item, depth, scale).h);
        }
        len
    };
    let total = |groups: &[&Vec<&Item>], scale: f64| -> f64 {
        groups.iter().map(|g| group_len(g, scale)).sum::<f64>() + GAP * (groups.len() as f64 - 1.)
    };

    // Shrink the cards if they don't all fit, and past the smallest size, show as many stacks as
    // fit starting at the offset, like the drawer does.
    let mut shown: Vec<&Vec<&Item>> = groups.iter().collect();
    let mut scale = 1.;
    // The gaps stay, and the least a tucked card shows doesn't shrink linearly, so close in.
    let gaps = total(&shown, 0.);
    for _ in 0..8 {
        let len = total(&shown, scale);
        if len <= room + 0.001 {
            break;
        }
        scale *= ((room - gaps) / (len - gaps)).clamp(0., 1.);
    }
    let smallest = items
        .iter()
        .map(|item| size_of(item, 0, 1.).h)
        .fold(f64::MAX, f64::min);
    if smallest * scale < MIN_CARD_H {
        scale = (MIN_CARD_H / smallest).min(1.);
        let n = groups.len();
        shown.clear();
        let mut used = 0.;
        for i in 0..n {
            let group = &groups[(offset + i) % n];
            let len = group_len(group, scale);
            let next = if shown.is_empty() {
                len
            } else {
                used + GAP + len
            };
            if !shown.is_empty() && next > room {
                break;
            }
            used = next;
            shown.push(group);
        }
    }

    let card_at = |size: Size<f64, Logical>, y: f64| {
        let x = band.loc.x + (band.size.w - size.w) / 2.;
        Rectangle::new(Point::from((x, y)), size)
    };

    let mut spreading = Vec::new();
    let mut others = Vec::new();
    let mut next = top;
    for group in shown {
        let is_stack = group.len() > 1;
        let progress = if is_stack {
            spread(group[0].group).clamp(0., 1.)
        } else {
            0.
        };
        // Eased out: quick at first, settling gently.
        let eased = 1. - (1. - progress).powi(3);

        // Folded: the front card whole, the others tucked behind it.
        let mut folded = Vec::new();
        let mut prev_bottom = next;
        for (depth, item) in group.iter().enumerate() {
            let size = size_of(item, depth, scale);
            if depth == 0 {
                let card = card_at(size, next);
                folded.push((card, card));
                prev_bottom = card.loc.y + size.h;
            } else {
                let bottom = prev_bottom + peek(size.h);
                let card = card_at(size, bottom - size.h);
                let hit = Rectangle::new(
                    Point::from((card.loc.x, prev_bottom)),
                    Size::from((size.w, bottom - prev_bottom)),
                );
                folded.push((card, hit));
                prev_bottom = bottom;
            }
        }
        let folded_bottom = prev_bottom;

        // Spread: every card whole, from the front card down, kept inside the band.
        let mut spread_cards = Vec::new();
        if progress > 0. {
            let sizes: Vec<_> = group.iter().map(|item| size_of(item, 0, scale)).collect();
            let len =
                sizes.iter().map(|size| size.h).sum::<f64>() + GAP * (sizes.len() as f64 - 1.);
            let start = (next - (next + len - bottom_edge).max(0.)).max(top);
            let mut y = start;
            for size in &sizes {
                spread_cards.push(card_at(*size, y));
                y += size.h + GAP;
            }
        }

        let slots = if progress > 0. {
            &mut spreading
        } else {
            &mut others
        };
        for (depth, item) in group.iter().enumerate() {
            let id = item.id;
            let (folded_card, folded_hit) = folded[depth];
            let (mut card, mut hit) = if progress > 0. {
                let card = lerp_rect(folded_card, spread_cards[depth], eased);
                (card, card)
            } else {
                (folded_card, folded_hit)
            };
            let folded_view = progress < 0.5;
            let mut tucked = is_stack && depth > 0 && folded_view;

            // A dragged card follows the pointer, leaving its place in the order empty.
            if let Some((dragged, at)) = drag {
                if dragged == id {
                    let y = at.clamp(
                        band.loc.y,
                        (band.loc.y + band.size.h - card.size.h).max(band.loc.y),
                    );
                    card.loc.y = y;
                    hit = card;
                    tucked = false;
                }
            }

            slots.push(Slot {
                id,
                card,
                hit,
                group: item.group,
                stack: if is_stack && depth == 0 && folded_view {
                    group.len()
                } else {
                    0
                },
                tucked,
            });
        }
        next = folded_bottom + GAP;
    }

    // Spreading stacks go first, to draw above the others and get the pointer.
    spreading.extend(others);
    spreading
}

/// A card grown around its middle, by how far it's grown from 0 to 1.
fn grown(card: Rectangle<f64, Logical>, grow: f64) -> Rectangle<f64, Logical> {
    let factor = 1. + GROW * grow.clamp(0., 1.);
    let size = card.size.upscale(factor);
    let loc = card.loc - (size - card.size).downscale(2.).to_point();
    Rectangle::new(loc, size)
}

/// A rectangle part of the way from one to another.
fn lerp_rect(
    from: Rectangle<f64, Logical>,
    to: Rectangle<f64, Logical>,
    t: f64,
) -> Rectangle<f64, Logical> {
    let lerp = |a: f64, b: f64| a + (b - a) * t;
    Rectangle::new(
        Point::from((lerp(from.loc.x, to.loc.x), lerp(from.loc.y, to.loc.y))),
        Size::from((lerp(from.size.w, to.size.w), lerp(from.size.h, to.size.h))),
    )
}

/// Which stack of cards should be spread out, for the pointer at this position: the one spread
/// out already while the pointer stays on or between its cards, otherwise the stack of the card
/// under the pointer.
pub fn spread_target(
    slots: &[Slot],
    hovered: Option<MappedId>,
    current: Option<u64>,
    pos: Point<f64, Logical>,
) -> Option<u64> {
    if let Some(group) = current {
        let area = slots
            .iter()
            .filter(|slot| slot.group == group)
            .map(|slot| slot.card)
            .reduce(|a, b| a.merge(b));
        if let Some(mut area) = area {
            area.loc -= Point::from((GAP, GAP));
            area.size += Size::from((GAP * 2., GAP * 2.));
            if area.contains(pos) {
                return Some(group);
            }
        }
    }

    let slot = slots.iter().find(|slot| Some(slot.id) == hovered)?;
    let count = slots.iter().filter(|s| s.group == slot.group).count();
    (count > 1).then_some(slot.group)
}

fn slots(
    output_size: Size<f64, Logical>,
    items: &[Item],
    placement: DrawerPlacement,
    offset: usize,
    drag: Option<(MappedId, f64)>,
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

    let mut next = ((along(output_size) - total(&shown_items, card_h)) / 2.).max(0.);
    let output = Rectangle::from_size(output_size);

    shown_items
        .into_iter()
        .map(|item| {
            let id = item.id;
            let pos = next;
            let size = card_size(item.size, card_h);
            // How far into the screen the card reaches.
            let depth = if in_row { size.h } else { size.w };
            let peek = if item.urgent { URGENT_PEEK } else { PEEK };
            let shown = peek + extend(id).clamp(0., 1.) * (depth + placement.margin - peek);

            // A dragged card follows the pointer, leaving its place in the order empty.
            let pos = match drag {
                Some((dragged, at)) if dragged == id => {
                    at.clamp(0., (along(output_size) - along(size)).max(0.))
                }
                _ => pos,
            };

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

            next += along(size) + GAP;
            Slot {
                id,
                card,
                hit,
                group: item.group,
                stack: 0,
                tucked: false,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::cards::{caption_loc, icon_loc, truncate, CARD_MAX_W};

    fn ids(n: usize) -> Vec<MappedId> {
        (0..n).map(|_| MappedId::next()).collect()
    }

    fn output() -> Size<f64, Logical> {
        Size::from((1920., 1080.))
    }

    fn screen(name: &str) -> Output {
        Output::new(
            name.to_owned(),
            smithay::output::PhysicalProperties {
                size: (0, 0).into(),
                subpixel: smithay::output::Subpixel::Unknown,
                make: String::new(),
                model: String::new(),
                serial_number: String::new(),
            },
        )
    }

    const MARGIN: f64 = 19.;

    fn at(edge: LimboEdge) -> DrawerPlacement {
        DrawerPlacement {
            edge,
            margin: MARGIN,
            overview: None,
            strip: None,
            strip_panel: None,
        }
    }

    fn items(ids: &[MappedId]) -> Vec<Item> {
        ids.iter()
            .map(|id| Item {
                id: *id,
                size: Size::from((800., 600.)),
                urgent: false,
                group: id.get(),
            })
            .collect()
    }

    #[test]
    fn right_cards_stack_centered_and_peek() {
        let ids = ids(2);
        let slots = slots(
            output(),
            &items(&ids),
            at(LimboEdge::Right),
            0,
            None,
            |_| 0.,
        );

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

        let right = slots(output(), &items, at(LimboEdge::Right), 0, None, |_| 1.)[0];
        assert_eq!(right.card.loc.x + right.card.size.w, 1920. - MARGIN);
        // The gap to the edge still counts as the card.
        assert_eq!(right.hit.loc.x, right.card.loc.x);
        assert_eq!(right.hit.loc.x + right.hit.size.w, 1920.);

        let left = slots(output(), &items, at(LimboEdge::Left), 0, None, |_| 1.)[0];
        assert_eq!(left.card.loc.x, MARGIN);
        assert_eq!(left.hit.loc.x, 0.);
        assert_eq!(left.hit.size.w, MARGIN + left.card.size.w);

        let bottom = slots(output(), &items, at(LimboEdge::Bottom), 0, None, |_| 1.)[0];
        assert_eq!(bottom.card.loc.y + bottom.card.size.h, 1080. - MARGIN);
        assert_eq!(bottom.hit.loc.y + bottom.hit.size.h, 1080.);
    }

    #[test]
    fn left_cards_peek_from_the_left() {
        let ids = ids(1);
        let slot = slots(output(), &items(&ids), at(LimboEdge::Left), 0, None, |_| 0.)[0];
        assert_eq!(slot.card.loc.x + slot.card.size.w, PEEK);
        assert_eq!(
            slot.hit,
            Rectangle::new((0., slot.card.loc.y).into(), (PEEK, 120.).into())
        );
    }

    #[test]
    fn bottom_cards_form_a_centered_row() {
        let ids = ids(2);
        let slots = slots(
            output(),
            &items(&ids),
            at(LimboEdge::Bottom),
            0,
            None,
            |_| 0.,
        );
        assert_eq!(slots[0].card.loc.y, 1080. - PEEK);
        assert_eq!(slots[0].card.loc.x, (1920. - 328.) / 2.);
        assert_eq!(slots[1].card.loc.x, slots[0].card.loc.x + 168.);
        assert_eq!(slots[0].hit.size.h, PEEK);
    }

    #[test]
    fn many_cards_shrink_to_fit() {
        let ids = ids(30);
        let items = items(&ids);

        let right = slots(output(), &items, at(LimboEdge::Right), 0, None, |_| 0.);
        let last = right.last().unwrap().card;
        assert!(right[0].card.size.h < CARD_H);
        assert!(right[0].card.loc.y >= 0.);
        assert!(last.loc.y + last.size.h <= 1080.);

        let bottom = slots(output(), &items, at(LimboEdge::Bottom), 0, None, |_| 0.);
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
        let screen = screen("A");
        let y = 540.;
        assert_eq!(
            drawer.window_under(&screen, output(), &items, right, Point::from((1915., y))),
            Some(ids[0])
        );
        assert_eq!(
            drawer.window_under(&screen, output(), &items, right, Point::from((1900., y))),
            None
        );
        assert_eq!(
            drawer.window_under(&screen, output(), &items, right, Point::from((1915., 10.))),
            None
        );
    }

    #[test]
    fn popped_drawer_slides_every_card_out() {
        let mut drawer = Drawer::new();
        let ids = ids(2);
        let screens = [screen("A"), screen("B")];
        drawer.sync(&ids);
        drawer.advance_animations(Duration::ZERO, &screens);

        drawer.set_popped(true);
        assert!(screens.iter().all(|s| drawer.are_animations_ongoing(s)));
        drawer.advance_animations(SLIDE, &screens);
        assert!(screens
            .iter()
            .all(|s| ids.iter().all(|id| drawer.extend_of(s, *id) == 1.)));
        assert!(!screens.iter().any(|s| drawer.are_animations_ongoing(s)));

        drawer.set_popped(false);
        drawer.advance_animations(SLIDE * 2, &screens);
        assert!(drawer.extend.is_empty());
        assert!(!screens.iter().any(|s| drawer.are_animations_ongoing(s)));
    }

    #[test]
    fn urgent_cards_peek_further() {
        let ids = ids(2);
        let mut items = items(&ids);
        items[1].urgent = true;
        let slots = slots(output(), &items, at(LimboEdge::Right), 0, None, |_| 0.);
        assert_eq!(slots[0].card.loc.x, 1920. - PEEK);
        assert_eq!(slots[1].card.loc.x, 1920. - URGENT_PEEK);
        assert_eq!(slots[1].hit.size.w, URGENT_PEEK);
    }

    #[test]
    fn too_many_cards_show_a_window_onto_them() {
        let ids = ids(30);
        let items = items(&ids);

        let shown = slots(output(), &items, at(LimboEdge::Right), 0, None, |_| 0.);
        assert!(shown.len() < 30);
        assert!(shown.iter().all(|slot| slot.card.size.h == MIN_CARD_H));
        let last = shown.last().unwrap().card;
        assert!(shown[0].card.loc.y >= 0.);
        assert!(last.loc.y + last.size.h <= 1080.);
        assert_eq!(shown[0].id, ids[0]);

        // An offset starts further in, and wraps round to the start.
        let n = shown.len();
        let shifted = slots(output(), &items, at(LimboEdge::Right), 1, None, |_| 0.);
        assert_eq!(shifted[0].id, ids[1]);
        let wrapped = slots(output(), &items, at(LimboEdge::Right), 29, None, |_| 0.);
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
    fn dragged_card_follows_the_pointer() {
        let ids = ids(3);
        let items = items(&ids);
        let still = slots(output(), &items, at(LimboEdge::Right), 0, None, |_| 0.);
        let dragged = slots(
            output(),
            &items,
            at(LimboEdge::Right),
            0,
            Some((ids[1], 900.)),
            |_| 0.,
        );
        assert_eq!(dragged[1].card.loc.y, 900.);
        assert_eq!(dragged[1].hit.loc.y, 900.);
        assert_eq!(dragged[0], still[0]);
        assert_eq!(dragged[2], still[2]);

        // It stays on the output.
        let dragged = slots(
            output(),
            &items,
            at(LimboEdge::Right),
            0,
            Some((ids[1], 5000.)),
            |_| 0.,
        );
        assert_eq!(dragged[1].card.loc.y, 1080. - 120.);
    }

    #[test]
    fn dragged_card_stays_out() {
        let mut drawer = Drawer::new();
        let ids = ids(2);
        let screens = [screen("A"), screen("B")];
        drawer.sync(&ids);
        drawer.advance_animations(Duration::ZERO, &screens);
        drawer.set_dragging(Some((screens[0].clone(), ids[0], 0.)));
        drawer.advance_animations(SLIDE, &screens);
        assert_eq!(drawer.extend_of(&screens[0], ids[0]), 1.);
        assert_eq!(drawer.extend_of(&screens[0], ids[1]), 0.);
        // Only on the output it's dragged on.
        assert_eq!(drawer.extend_of(&screens[1], ids[0]), 0.);
    }

    #[test]
    fn reordering_keeps_the_scroll() {
        let mut drawer = Drawer::new();
        let ids = ids(3);
        drawer.sync(&ids);
        drawer.scroll(1);
        drawer.sync(&[ids[2], ids[0], ids[1]]);
        assert_eq!(drawer.offset, 1);
    }

    #[test]
    fn pulling_out_needs_to_clear_the_card() {
        let card = Rectangle::new(Point::from((1700., 500.)), Size::from((200., 100.)));
        let past = |edge, x, y| is_past_card(edge, card, Point::from((x, y)), 32.);
        assert!(!past(LimboEdge::Right, 1690., 550.));
        assert!(past(LimboEdge::Right, 1660., 550.));
        assert!(!past(LimboEdge::Left, 1910., 550.));
        assert!(past(LimboEdge::Left, 1940., 550.));
        assert!(!past(LimboEdge::Bottom, 1750., 480.));
        assert!(past(LimboEdge::Bottom, 1750., 460.));
    }

    #[test]
    fn insertion_point_goes_by_the_other_middles() {
        let ids = ids(3);
        let slots = slots(
            output(),
            &items(&ids),
            at(LimboEdge::Right),
            0,
            None,
            |_| 0.,
        );
        let middle = |i: usize| slots[i].card.loc.y + slots[i].card.size.h / 2.;
        let insert = |along| insertion_point(&slots, LimboEdge::Right, ids[0], along);
        assert_eq!(insert(0.), Some(Insert::Before(ids[1])));
        assert_eq!(insert(middle(1) + 1.), Some(Insert::Before(ids[2])));
        assert_eq!(insert(middle(2) + 1.), Some(Insert::After(ids[2])));
        // Alone in the drawer, there's nowhere to go.
        assert_eq!(
            insertion_point(&slots[..1], LimboEdge::Right, ids[0], 0.),
            None
        );
    }

    #[test]
    fn icon_sits_in_the_inner_bottom_corner() {
        let card = Rectangle::new(Point::from((1700., 500.)), Size::from((200., 100.)));
        let size = Size::from((32., 32.));
        assert_eq!(
            icon_loc(card, size, LimboEdge::Right),
            Point::from((1706., 562.))
        );
        assert_eq!(
            icon_loc(card, size, LimboEdge::Bottom),
            Point::from((1706., 562.))
        );
        assert_eq!(
            icon_loc(card, size, LimboEdge::Left),
            Point::from((1862., 562.))
        );
    }

    #[test]
    fn edge_pointer_needs_the_edge_beside_the_cards() {
        let ids = ids(2);
        let right = at(LimboEdge::Right);
        let shown = slots(output(), &items(&ids), right, 0, None, |_| 0.);
        let first = shown[0].card;
        let y = first.loc.y + 10.;
        let state = |x, y| edge_pointer(LimboEdge::Right, output(), &shown, Point::from((x, y)));

        assert_eq!(state(1919.5, y), EdgePointer::AtEdge);
        // Above the cards, the edge doesn't count.
        assert_eq!(state(1919.5, 10.), EdgePointer::Away);
        // On the peeking strip, but off the edge.
        assert_eq!(state(1915., y), EdgePointer::InDrawer);
        // In the gap between the cards.
        assert_eq!(
            state(1915., first.loc.y + first.size.h + GAP / 2.),
            EdgePointer::InDrawer
        );
        assert_eq!(state(1000., y), EdgePointer::Away);
        assert_eq!(
            edge_pointer(LimboEdge::Right, output(), &[], Point::from((1919.5, y))),
            EdgePointer::Away
        );

        let left = slots(output(), &items(&ids), at(LimboEdge::Left), 0, None, |_| 0.);
        let pos = Point::from((0.5, left[0].card.loc.y + 10.));
        assert_eq!(
            edge_pointer(LimboEdge::Left, output(), &left, pos),
            EdgePointer::AtEdge
        );
        let bottom = slots(
            output(),
            &items(&ids),
            at(LimboEdge::Bottom),
            0,
            None,
            |_| 0.,
        );
        let pos = Point::from((bottom[0].card.loc.x + 10., 1079.5));
        assert_eq!(
            edge_pointer(LimboEdge::Bottom, output(), &bottom, pos),
            EdgePointer::AtEdge
        );
    }

    #[test]
    fn holding_the_edge_slides_all_cards_out() {
        let mut drawer = Drawer::new();
        let ids = ids(2);
        let screens = [screen("A"), screen("B")];
        let a = &screens[0];
        drawer.sync(&ids);
        drawer.advance_animations(Duration::ZERO, &screens);

        // Not before the dwell.
        let ms = Duration::from_millis;
        assert!(drawer.set_edge_pointer(a, EdgePointer::AtEdge, ms(0)));
        assert!(drawer.are_animations_ongoing(a));
        // Only the output with the pointer against its edge keeps redrawing.
        assert!(!drawer.are_animations_ongoing(&screens[1]));
        drawer.advance_animations(ms(100), &screens);
        assert!(drawer.revealed.is_none());
        // Staying there doesn't restart the dwell.
        assert!(!drawer.set_edge_pointer(a, EdgePointer::AtEdge, ms(100)));
        drawer.advance_animations(ms(150), &screens);
        assert_eq!(drawer.revealed.as_ref(), Some(a));
        drawer.advance_animations(ms(150) + SLIDE, &screens);
        assert!(ids.iter().all(|id| drawer.extend_of(a, *id) == 1.));
        // Only on the output pushed against.
        assert!(ids
            .iter()
            .all(|id| drawer.extend_of(&screens[1], *id) == 0.));

        // Moving onto the cards keeps them out, leaving puts them back.
        drawer.set_edge_pointer(a, EdgePointer::InDrawer, ms(400));
        assert_eq!(drawer.revealed.as_ref(), Some(a));
        assert!(drawer.set_edge_pointer(a, EdgePointer::Away, ms(500)));
        assert!(drawer.revealed.is_none());

        // Leaving the edge before the dwell calls it off.
        drawer.set_edge_pointer(a, EdgePointer::AtEdge, ms(1000));
        drawer.set_edge_pointer(a, EdgePointer::InDrawer, ms(1050));
        drawer.advance_animations(ms(1200), &screens);
        assert!(drawer.revealed.is_none());
        assert!(drawer.edge_since.is_none());
    }

    #[test]
    fn sync_forgets_windows_that_left() {
        let mut drawer = Drawer::new();
        let ids = ids(2);
        drawer.sync(&ids);
        drawer.set_hovered(None);
        let a = screen("A");
        drawer.extend.insert((a.clone(), ids[0]), 0.5);
        drawer.sync(&ids[1..]);
        assert_eq!(drawer.order(), [ids[1]]);
        assert!(!drawer.extend.contains_key(&(a, ids[0])));
    }

    #[test]
    fn hovering_slides_out_only_on_that_output() {
        let mut drawer = Drawer::new();
        let ids = ids(2);
        let screens = [screen("A"), screen("B")];
        drawer.sync(&ids);
        drawer.advance_animations(Duration::ZERO, &screens);

        drawer.set_hovered(Some((screens[0].clone(), ids[0])));
        drawer.advance_animations(SLIDE, &screens);
        assert_eq!(drawer.extend_of(&screens[0], ids[0]), 1.);
        assert_eq!(drawer.extend_of(&screens[0], ids[1]), 0.);
        assert_eq!(drawer.extend_of(&screens[1], ids[0]), 0.);
        assert!(!screens.iter().any(|s| drawer.are_animations_ongoing(s)));

        let items = items(&ids);
        let right = at(LimboEdge::Right);
        let on_a = drawer.slots(&screens[0], output(), &items, right);
        let on_b = drawer.slots(&screens[1], output(), &items, right);
        assert!(on_a[0].card.loc.x < 1920. - PEEK);
        assert_eq!(on_b[0].card.loc.x, 1920. - PEEK);
    }

    #[test]
    fn strip_cards_keep_their_padding() {
        let band = Rectangle::new(Point::from((1704., 40.)), Size::from((STRIP_WIDTH, 1040.)));
        let ids = ids(2);
        let slots = strip_slots(band, &items(&ids), 0, None, |_| 0.);

        // As wide as the band minus the padding on both sides, starting the padding down.
        let card = slots[0].card;
        assert_eq!(card.size.w, STRIP_WIDTH - 2. * STRIP_PADDING);
        assert_eq!(card.loc.x, band.loc.x + STRIP_PADDING);
        assert_eq!(card.loc.y, band.loc.y + STRIP_PADDING);
        assert_eq!(slots[1].card.loc.y, card.loc.y + card.size.h + GAP);
    }

    #[test]
    fn strip_cards_shrink_to_fit_the_padded_band() {
        let band = Rectangle::new(Point::from((0., 0.)), Size::from((STRIP_WIDTH, 400.)));
        let ids = ids(4);
        let slots = strip_slots(band, &items(&ids), 0, None, |_| 0.);
        let last = slots.last().unwrap().card;
        assert!(last.loc.y + last.size.h <= band.size.h - STRIP_PADDING + 0.001);
    }

    #[test]
    fn panel_runs_past_the_screen_edges() {
        let area = Rectangle::new(Point::from((1704., 0.)), Size::from((216., 1080.)));
        let rim = f64::from(PANEL_RIM);
        let panel = panel_rect(area, 1.);
        // Starts at the strip, and goes past the top, right and bottom by the rim.
        assert_eq!(panel.loc, Point::from((1704., -rim)));
        assert_eq!(panel.loc.x + panel.size.w, 1920. + rim);
        assert_eq!(panel.loc.y + panel.size.h, 1080. + rim);
    }

    /// Items for these windows, all of the same app but the last.
    fn same_app_items(ids: &[MappedId]) -> Vec<Item> {
        let mut items = items(ids);
        let last = items.len() - 1;
        for (i, item) in items.iter_mut().enumerate() {
            item.group = if i == last { 2 } else { 1 };
        }
        items
    }

    #[test]
    fn cards_of_one_app_stack() {
        let band = Rectangle::new(Point::from((0., 0.)), Size::from((STRIP_WIDTH, 1000.)));
        let ids = ids(4);
        let slots = strip_slots(band, &same_app_items(&ids), 0, None, |_| 0.);

        // The front card shows whole, with the count of the stack.
        let front = slots[0];
        assert_eq!(front.stack, 3);
        assert!(!front.tucked);
        assert_eq!(front.hit, front.card);

        // The others tuck behind it, smaller, showing only their bottom.
        let tucked = slots[1];
        assert!(tucked.tucked);
        assert!(tucked.card.size.w < front.card.size.w);
        assert_eq!(tucked.hit.loc.y, front.card.loc.y + front.card.size.h);
        let shown = tucked.hit.size.h;
        let peek = (tucked.card.size.h * STACK_PEEK).max(STACK_MIN_PEEK);
        assert!((shown - peek).abs() < 1e-6);
        let bottom = tucked.card.loc.y + tucked.card.size.h;
        assert!((bottom - (tucked.hit.loc.y + shown)).abs() < 1e-6);
        assert!(slots[2].tucked);

        // The other app's card comes after the stack, on its own.
        let other = slots[3];
        assert_eq!(other.stack, 0);
        assert!(!other.tucked);
        assert_eq!(
            other.card.loc.y,
            slots[2].card.loc.y + slots[2].card.size.h + GAP
        );
    }

    #[test]
    fn a_spread_stack_lies_over_the_others() {
        let band = Rectangle::new(Point::from((0., 0.)), Size::from((STRIP_WIDTH, 1000.)));
        let ids = ids(4);
        let items = same_app_items(&ids);
        let folded = strip_slots(band, &items, 0, None, |_| 0.);
        let spread = strip_slots(band, &items, 0, None, |group| f64::from(group == 1));

        // The stack comes first, its cards whole and apart, from where its front card was.
        assert!(spread[..3]
            .iter()
            .all(|slot| slot.group == 1 && !slot.tucked));
        assert_eq!(spread[0].card, folded[0].card);
        assert_eq!(spread[1].card.size, spread[0].card.size);
        assert_eq!(
            spread[1].card.loc.y,
            spread[0].card.loc.y + spread[0].card.size.h + GAP
        );
        assert_eq!(spread[1].hit, spread[1].card);
        assert_eq!(spread[0].stack, 0);

        // The other app's card stays where it was, below the spread stack.
        assert_eq!(spread[3], folded[3]);
    }

    #[test]
    fn a_spread_stack_stays_inside_the_band() {
        // Tall cards: three of them spread out don't fit below the front card.
        let band = Rectangle::new(Point::from((0., 0.)), Size::from((STRIP_WIDTH, 500.)));
        let ids = ids(3);
        let mut items = items(&ids);
        for item in &mut items {
            item.group = 1;
        }
        let spread = strip_slots(band, &items, 0, None, |_| 1.);
        let bottom = spread
            .iter()
            .map(|slot| slot.card.loc.y + slot.card.size.h)
            .fold(f64::MIN, f64::max);
        let top = spread
            .iter()
            .map(|slot| slot.card.loc.y)
            .fold(f64::MAX, f64::min);
        assert!(bottom <= band.size.h - STRIP_PADDING + 0.001 || top <= STRIP_PADDING + 0.001);
        assert!(top >= STRIP_PADDING - 0.001);
    }

    #[test]
    fn spreading_moves_the_cards_part_of_the_way() {
        let band = Rectangle::new(Point::from((0., 0.)), Size::from((STRIP_WIDTH, 1000.)));
        let ids = ids(3);
        let items = same_app_items(&ids);
        let folded = strip_slots(band, &items, 0, None, |_| 0.);
        let half = strip_slots(band, &items, 0, None, |group| 0.5 * f64::from(group == 1));
        let spread = strip_slots(band, &items, 0, None, |group| f64::from(group == 1));
        let y = |slots: &[Slot]| slots[1].card.loc.y;
        assert!(y(&folded) < y(&half) && y(&half) < y(&spread));
    }

    #[test]
    fn the_pointer_keeps_a_stack_spread_between_its_cards() {
        let band = Rectangle::new(Point::from((0., 0.)), Size::from((STRIP_WIDTH, 1000.)));
        let ids = ids(4);
        let items = same_app_items(&ids);
        let folded = strip_slots(band, &items, 0, None, |_| 0.);

        // Hovering a card of a stack spreads it; a lone card spreads nothing.
        let on = |slot: &Slot| slot.card.loc + slot.card.size.downscale(2.).to_point();
        assert_eq!(
            spread_target(&folded, Some(ids[1]), None, on(&folded[1])),
            Some(1)
        );
        assert_eq!(
            spread_target(&folded, Some(ids[3]), None, on(&folded[3])),
            None
        );

        // In the gap between two spread cards, it stays spread, even over another card.
        let spread = strip_slots(band, &items, 0, None, |group| f64::from(group == 1));
        let gap = Point::from((
            spread[1].card.loc.x + 20.,
            spread[1].card.loc.y + spread[1].card.size.h + GAP / 2.,
        ));
        assert_eq!(spread_target(&spread, Some(ids[3]), Some(1), gap), Some(1));

        // Well away from it, it folds back.
        let away = Point::from((band.loc.x + 20., band.size.h - 2.));
        assert_eq!(spread_target(&spread, None, Some(1), away), None);
    }

    #[test]
    fn stacks_spread_over_time() {
        let mut drawer = Drawer::new();
        let screen = screen("A");
        let screens = [screen.clone()];
        drawer.advance_animations(Duration::ZERO, &screens);
        assert!(drawer.set_spread(Some((screen.clone(), 1))));
        drawer.advance_animations(Duration::from_millis(1), &screens);
        assert!(drawer.are_animations_ongoing(&screen));
        drawer.advance_animations(SPREAD * 2, &screens);
        assert_eq!(drawer.spread_of(&screen, 1), 1.);
        assert!(!drawer.are_animations_ongoing(&screen));

        // And fold back.
        drawer.set_spread(None);
        drawer.advance_animations(SPREAD * 2 + Duration::from_millis(1), &screens);
        assert!(drawer.are_animations_ongoing(&screen));
        drawer.advance_animations(SPREAD * 5, &screens);
        assert_eq!(drawer.spread_of(&screen, 1), 0.);
        assert!(!drawer.are_animations_ongoing(&screen));
    }

    #[test]
    fn stacks_keep_the_order_of_their_newest_window() {
        let band = Rectangle::new(Point::from((0., 0.)), Size::from((STRIP_WIDTH, 1000.)));
        let ids = ids(3);
        let mut items = items(&ids);
        // Newest first: app A, app B, app A again.
        items[0].group = 1;
        items[1].group = 2;
        items[2].group = 1;
        let order: Vec<_> = strip_slots(band, &items, 0, None, |_| 0.)
            .iter()
            .map(|slot| slot.id)
            .collect();
        assert_eq!(order, [ids[0], ids[2], ids[1]]);
    }

    #[test]
    fn cards_grow_around_their_middle() {
        let card = Rectangle::new(Point::from((100., 50.)), Size::from((200., 100.)));
        assert_eq!(grown(card, 0.), card);
        let big = grown(card, 1.);
        assert_eq!(big.size, card.size.upscale(1. + GROW));
        let middle = |r: Rectangle<f64, Logical>| r.loc + r.size.downscale(2.).to_point();
        assert_eq!(middle(big), middle(card));
    }

    #[test]
    fn hovered_card_grows_over_time() {
        let mut drawer = Drawer::new();
        let screen = screen("A");
        let screens = [screen.clone()];
        let ids = ids(1);
        drawer.sync(&ids);
        drawer.advance_animations(Duration::ZERO, &screens);
        drawer.set_hovered(Some((screen.clone(), ids[0])));
        drawer.advance_animations(Duration::from_millis(1), &screens);
        assert!(drawer.are_animations_ongoing(&screen));
        drawer.advance_animations(GROW_TIME * 2, &screens);
        assert_eq!(drawer.grow[&(screen.clone(), ids[0])], 1.);

        drawer.set_hovered(None);
        drawer.advance_animations(GROW_TIME * 2 + Duration::from_millis(1), &screens);
        assert!(drawer.are_animations_ongoing(&screen));
        drawer.advance_animations(GROW_TIME * 5, &screens);
        assert!(drawer.grow.is_empty());
    }
}
