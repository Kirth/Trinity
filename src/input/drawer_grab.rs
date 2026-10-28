//! Dragging a card in the limbo drawer: along the drawer to rearrange the cards, into the screen
//! to pull its window out, or just a click to bring it next to the focused window.

use smithay::input::pointer::{
    AxisFrame, ButtonEvent, CursorIcon, CursorImageStatus, Focus, GestureHoldBeginEvent,
    GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
    GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
    GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerInnerHandle,
    RelativeMotionEvent,
};
use smithay::input::SeatHandler;
use smithay::output::Output;
use smithay::utils::{Logical, Point};

use crate::input::move_grab::MoveGrab;
use crate::input::AnyStartData;
use crate::niri::State;
use crate::ui::drawer::{along_edge, insertion_point, is_past_card, Insert};
use crate::window::mapped::MappedId;

/// How far along the drawer, in logical pixels, the pointer has to move to start rearranging.
const DRAG_THRESHOLD: f64 = 8.;
/// How far past the card into the screen, in logical pixels, the pointer has to move to pull the
/// window out.
const PULL_DISTANCE: f64 = 32.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Not moved far enough to tell yet; letting go is a click.
    Pending,
    /// Rearranging the cards.
    Reorder,
    /// Over: pulled out, or the card went away.
    Done,
}

pub struct DrawerGrab {
    start_data: PointerGrabStartData<State>,
    output: Output,
    id: MappedId,
    /// Where the press was along the drawer's edge.
    start_along: f64,
    /// Where the press was along the edge relative to the start of the card.
    grab_offset: f64,
    phase: Phase,
}

impl DrawerGrab {
    pub fn new(
        state: &State,
        start_data: PointerGrabStartData<State>,
        output: Output,
        id: MappedId,
    ) -> Option<Self> {
        let edge = state.niri.drawer_placement(&output).edge;
        let output_loc = state
            .niri
            .global_space
            .output_geometry(&output)?
            .loc
            .to_f64();
        let pos = start_data.location - output_loc;
        let slot = state
            .niri
            .drawer_slots(&output)
            .into_iter()
            .find(|slot| slot.id == id)?;

        let start_along = along_edge(edge, pos);
        Some(Self {
            grab_offset: start_along - along_edge(edge, slot.card.loc),
            start_along,
            start_data,
            output,
            id,
            phase: Phase::Pending,
        })
    }

    /// Handles the pointer moving to this output-local position. Returns a grab to hand over to
    /// if the window was pulled out, or `None` with the phase `Done` if the drag is over.
    pub(crate) fn on_motion(
        &mut self,
        data: &mut State,
        pos: Point<f64, Logical>,
        location: Point<f64, Logical>,
    ) -> Option<MoveGrab> {
        let edge = data.niri.drawer_placement(&self.output).edge;
        let slots = data.niri.drawer_slots(&self.output);
        let Some(slot) = slots.iter().find(|slot| slot.id == self.id) else {
            // The card went away.
            self.phase = Phase::Done;
            return None;
        };

        // Into the screen: pull the window out and keep dragging it.
        if is_past_card(edge, slot.card, pos, PULL_DISTANCE) {
            self.phase = Phase::Done;
            let window = data.pull_from_drawer(self.id, &self.output)?;
            let start_data = PointerGrabStartData {
                focus: None,
                button: self.start_data.button,
                location,
            };
            let start_data = AnyStartData::Pointer(start_data);
            return MoveGrab::new(data, start_data, window, false, Some(CursorIcon::Grabbing));
        }

        let along = along_edge(edge, pos);
        if self.phase == Phase::Pending {
            if (along - self.start_along).abs() <= DRAG_THRESHOLD {
                return None;
            }
            self.phase = Phase::Reorder;
            data.niri
                .cursor_manager
                .set_cursor_image(CursorImageStatus::Named(CursorIcon::Grabbing));
        }

        // Rearranging: the card follows the pointer, and moves in limbo to where it's dropped.
        data.niri.drawer.set_dragging(Some((
            self.output.clone(),
            self.id,
            along - self.grab_offset,
        )));
        if let Some(insert) = insertion_point(&slots, edge, self.id, along) {
            reorder(data, self.id, insert, &self.output);
        }
        data.niri.queue_redraw_all();
        None
    }

    pub(crate) fn on_ungrab(&mut self, data: &mut State) {
        data.niri.drawer.set_dragging(None);
        data.niri
            .cursor_manager
            .set_cursor_image(CursorImageStatus::default_named());

        // Let go without moving: a click brings the window out, leaving the overview.
        if self.phase == Phase::Pending {
            data.click_drawer_card(self.id, &self.output);
            if data.niri.layout.is_overview_open() {
                data.niri.layout.close_overview();
            }
        }
        self.phase = Phase::Done;
        data.niri.queue_redraw_all();
    }
}

/// Moves a window in limbo or a strip next to another, as the drawer shows it.
fn reorder(data: &mut State, id: MappedId, insert: Insert, output: &Output) {
    let layout = &mut data.niri.layout;
    let in_limbo = layout.limbo_windows().any(|mapped| mapped.id() == id);
    let order: Vec<_> = if in_limbo {
        layout.limbo_windows().map(|mapped| mapped.id()).collect()
    } else {
        layout
            .strip_windows(output)
            .map(|mapped| mapped.id())
            .collect()
    };
    let others: Vec<_> = order.iter().copied().filter(|x| *x != id).collect();
    let idx = match insert {
        Insert::Before(other) => others.iter().position(|x| *x == other),
        Insert::After(other) => others.iter().position(|x| *x == other).map(|i| i + 1),
    };
    let window = layout
        .windows()
        .find(|(_, mapped)| mapped.id() == id)
        .map(|(_, mapped)| mapped.window.clone());
    if let (Some(idx), Some(window)) = (idx, window) {
        let current = order.iter().position(|x| *x == id);
        if current != Some(idx) {
            if in_limbo {
                layout.move_in_limbo(&window, idx);
            } else {
                layout.move_in_strip(&window, idx);
            }
        }
    }
}

impl PointerGrab<State> for DrawerGrab {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(<State as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        // While the grab is active, no client has pointer focus.
        handle.motion(data, None, event);

        let Some(output_geo) = data.niri.global_space.output_geometry(&self.output) else {
            self.phase = Phase::Done;
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        };
        let pos = event.location - output_geo.loc.to_f64();

        if let Some(grab) = self.on_motion(data, pos, event.location) {
            handle.set_grab(self, data, event.serial, Focus::Clear, grab);
            data.niri
                .cursor_manager
                .set_cursor_image(CursorImageStatus::Named(CursorIcon::Grabbing));
        } else if self.phase == Phase::Done {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn relative_motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(<State as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, None, event);
    }

    fn button(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);

        if !handle.current_pressed().contains(&self.start_data.button) {
            // The button that started the drag was released.
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &PointerGrabStartData<State> {
        &self.start_data
    }

    fn unset(&mut self, data: &mut State) {
        self.on_ungrab(data);
    }
}
