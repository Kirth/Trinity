use client::ClientId;
use insta::assert_snapshot;
use niri_config::Config;
use smithay::utils::Point;
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;

fn i3_config() -> Config {
    Config::parse_mem(r#"layout { layout-mode "i3"; }"#).unwrap()
}

/// Opens a window, commits a buffer at the size niri asked for, and returns its surface.
fn open_window(f: &mut Fixture, id: ClientId) -> WlSurface {
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    let (w, h) = window.configures_received.last().unwrap().1.size;
    window.set_size(w.max(1) as u16, h.max(1) as u16);
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    surface
}

#[test]
fn windows_split_the_output() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    // The first window takes the whole working area, minus gaps.
    let first = open_window(&mut f, id);
    assert_snapshot!(
        f.client(id).window(&first).format_recent_configures(),
        @r"
    size: 1888 × 1048, bounds: 1888 × 1048, states: []
    size: 1888 × 1048, bounds: 1888 × 1048, states: [Activated]
    "
    );

    // The second window opens at half the width, and the first one shrinks to make room.
    let second = open_window(&mut f, id);
    assert_snapshot!(
        f.client(id).window(&second).format_recent_configures(),
        @r"
    size: 936 × 1048, bounds: 1888 × 1048, states: []
    size: 936 × 1048, bounds: 1888 × 1048, states: [Activated]
    "
    );
    assert_snapshot!(
        f.client(id).window(&first).format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: []"
    );
}

#[test]
fn split_vertical_stacks_new_window() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    let second = open_window(&mut f, id);
    // Only look at the configures after the split.
    let _ = f.client(id).window(&second).format_recent_configures();

    f.niri()
        .layout
        .split(crate::layout::tiling::SplitCommand::Vertical);
    let third = open_window(&mut f, id);

    // The third window goes below the second, sharing its half of the output.
    assert_snapshot!(
        f.client(id).window(&third).format_recent_configures(),
        @r"
    size: 936 × 516, bounds: 1888 × 1048, states: []
    size: 936 × 516, bounds: 1888 × 1048, states: [Activated]
    "
    );
    assert_snapshot!(
        f.client(id).window(&second).format_recent_configures(),
        @"size: 936 × 516, bounds: 1888 × 1048, states: []"
    );
}

fn format_window_sizes(f: &mut Fixture) -> String {
    let mut sizes: Vec<_> = f
        .niri()
        .layout
        .windows()
        .map(|(_, mapped)| {
            let size = crate::layout::LayoutElement::size(mapped);
            format!("{} × {}", size.w, size.h)
        })
        .collect();
    sizes.sort();
    sizes.join("\n")
}

#[test]
fn closing_doesnt_wait_for_unchanged_windows() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    // 1 on the left, 2 and 3 stacked on the right.
    let first = open_window(&mut f, id);
    let second = open_window(&mut f, id);
    f.niri()
        .layout
        .split(crate::layout::tiling::SplitCommand::Vertical);
    let third = open_window(&mut f, id);

    // Commit the sizes from opening 3.
    for surface in [&first, &second, &third] {
        let window = f.client(id).window(surface);
        let (w, h) = window.configures_received.last().unwrap().1.size;
        window.set_size(w as u16, h as u16);
        window.ack_last_and_commit();
    }
    f.double_roundtrip(id);
    let _ = f.client(id).window(&first).format_recent_configures();
    let _ = f.client(id).window(&second).format_recent_configures();

    // Close 3: 2 grows back to the full height, 1 keeps its size.
    let window = f.client(id).window(&third);
    window.attach_null();
    window.commit();
    f.double_roundtrip(id);

    assert_snapshot!(f.client(id).window(&first).format_recent_configures(), @"");
    let window = f.client(id).window(&second);
    assert_snapshot!(
        window.format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: [Activated]"
    );

    // 2 commits its new size, which must apply right away, since 1 isn't part of the resize.
    window.set_size(936, 1048);
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    assert_snapshot!(format_window_sizes(&mut f), @r"
    936 × 1048
    936 × 1048
    ");
}

/// Presses and releases a key, by its xkb name, through the normal input path.
fn tap_key(f: &mut Fixture, name: &str) {
    use smithay::backend::input::{InputEvent, InputTime, KeyState};

    use crate::tests::test_input_backend::{TestInputBackend, TestKeyboardKeyEvent};

    let state = f.niri_state();
    let keyboard = state.niri.seat.get_keyboard().unwrap();
    let code = keyboard.with_xkb_state(state, |xkb| {
        let xkb = xkb.xkb().lock().unwrap();
        let keymap = unsafe { xkb.keymap() };
        keymap.key_by_name(name).unwrap()
    });

    for key_state in [KeyState::Pressed, KeyState::Released] {
        f.niri_state()
            .process_input_event(InputEvent::<TestInputBackend>::Keyboard {
                event: TestKeyboardKeyEvent {
                    time: InputTime::from_micros(0),
                    code,
                    state: key_state,
                    count: 1,
                },
            });
    }
}

fn focused_id(f: &mut Fixture) -> Option<crate::window::mapped::MappedId> {
    f.niri().layout.focus().map(|m| m.id())
}

#[test]
fn hints_focus_typed_window() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    // Three windows side by side get the hints a, s, d from left to right.
    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let _third = open_window(&mut f, id);
    // Windows in tree order, which is left to right here.
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();
    assert_eq!(ids.len(), 3);

    f.niri_state().open_ace_hints();
    assert!(f.niri().ace_hints.is_open());

    tap_key(&mut f, "AC02"); // s
    f.double_roundtrip(id);
    assert!(!f.niri().ace_hints.is_open());
    assert_eq!(focused_id(&mut f), Some(ids[1]));

    // A key that isn't a hint closes the hints and leaves the focus alone.
    f.niri_state().open_ace_hints();
    tap_key(&mut f, "AD01"); // q
    f.double_roundtrip(id);
    assert!(!f.niri().ace_hints.is_open());
    assert_eq!(focused_id(&mut f), Some(ids[1]));

    // Escape cancels too.
    f.niri_state().open_ace_hints();
    tap_key(&mut f, "ESC");
    assert!(!f.niri().ace_hints.is_open());

    // And picking the first window works.
    f.niri_state().open_ace_hints();
    tap_key(&mut f, "AC01"); // a
    f.double_roundtrip(id);
    assert_eq!(focused_id(&mut f), Some(ids[0]));
}

#[test]
fn hints_show_ghosts_of_windows_behind_fullscreen() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let first = open_window(&mut f, id);
    let second = open_window(&mut f, id);
    let third = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();
    let output = f.niri_output(1);

    // Without a fullscreen window, nothing is hidden.
    f.niri_state().open_ace_hints();
    assert!(f.niri().ace_hint_ghosts(&output).is_empty());
    f.niri().ace_hints.close();

    // Fullscreen the third window, and let all windows commit their new sizes.
    f.niri_state()
        .do_action(niri_config::Action::FullscreenWindow, false);
    f.double_roundtrip(id);
    configured_widths(&mut f, id, &[&first, &second, &third]);
    f.niri_complete_animations();

    // The two windows behind it get a ghost, the fullscreen one doesn't.
    f.niri_state().open_ace_hints();
    let ghosts: Vec<_> = f
        .niri()
        .ace_hint_ghosts(&output)
        .into_iter()
        .map(|(tile, _)| tile.window().id())
        .collect();
    assert_eq!(ghosts.len(), 2);
    assert!(ghosts.contains(&ids[0]));
    assert!(ghosts.contains(&ids[1]));

    // Closing the hints removes the ghosts, since they need a label.
    f.niri().ace_hints.close();
    assert!(f.niri().ace_hint_ghosts(&output).is_empty());

    // Maximized to the edges instead, it hides the other two windows just the same.
    f.niri_state()
        .do_action(niri_config::Action::FullscreenWindow, false);
    f.double_roundtrip(id);
    configured_widths(&mut f, id, &[&first, &second, &third]);
    f.niri_complete_animations();
    f.niri_state()
        .do_action(niri_config::Action::MaximizeWindowToEdges, false);
    f.double_roundtrip(id);
    configured_widths(&mut f, id, &[&first, &second, &third]);
    f.niri_complete_animations();

    f.niri_state().open_ace_hints();
    let ghosts: Vec<_> = f
        .niri()
        .ace_hint_ghosts(&output)
        .into_iter()
        .map(|(tile, _)| tile.window().id())
        .collect();
    assert_eq!(ghosts.len(), 2);
    assert!(ghosts.contains(&ids[0]));
    assert!(ghosts.contains(&ids[1]));
}

/// Commits the last configured size of each window, and returns their widths.
fn configured_widths(f: &mut Fixture, id: ClientId, surfaces: &[&WlSurface]) -> Vec<i32> {
    let widths = surfaces
        .iter()
        .map(|surface| {
            let window = f.client(id).window(surface);
            let (w, h) = window.configures_received.last().unwrap().1.size;
            if window.configures_looked_at < window.configures_received.len() {
                window.configures_looked_at = window.configures_received.len();
                window.set_size(w as u16, h as u16);
                window.ack_last_and_commit();
            }
            w
        })
        .collect();
    f.double_roundtrip(id);
    widths
}

#[test]
fn resize_mode_resizes_until_escape() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    // Two windows side by side, the second one focused.
    let first = open_window(&mut f, id);
    let second = open_window(&mut f, id);
    assert_eq!(
        configured_widths(&mut f, id, &[&first, &second]),
        [936, 936]
    );

    f.niri_state()
        .do_action(niri_config::Action::ResizeMode, false);
    assert!(f.niri().resize_mode.is_active());

    // Right grows the focused window, taking the space from its neighbour.
    tap_key(&mut f, "RGHT");
    f.double_roundtrip(id);
    let [w1, w2] = configured_widths(&mut f, id, &[&first, &second])[..] else {
        unreachable!()
    };
    assert!(w2 > 936, "{w2}");
    assert_eq!(w1 + w2, 936 * 2);

    // l does the same.
    tap_key(&mut f, "AC09");
    f.double_roundtrip(id);
    let grown = configured_widths(&mut f, id, &[&first, &second]);
    assert!(grown[1] > w2, "{grown:?}");

    // h shrinks it back.
    tap_key(&mut f, "AC06");
    f.double_roundtrip(id);
    assert_eq!(configured_widths(&mut f, id, &[&first, &second]), [w1, w2]);

    // Other keys are swallowed and keep the mode.
    let _ = f.client(id).state.recent_keyboard_events(&second).count();
    tap_key(&mut f, "AD01"); // q
    f.double_roundtrip(id);
    assert!(f.niri().resize_mode.is_active());
    let key_events = f
        .client(id)
        .state
        .recent_keyboard_events(&second)
        .filter(|event| matches!(event, client::KeyboardEvent::Key { .. }))
        .count();
    assert_eq!(key_events, 0);
    assert_eq!(configured_widths(&mut f, id, &[&first, &second]), [w1, w2]);

    // Escape leaves, and then arrows no longer resize.
    tap_key(&mut f, "ESC");
    assert!(!f.niri().resize_mode.is_active());
    tap_key(&mut f, "RGHT");
    f.double_roundtrip(id);
    assert_eq!(configured_widths(&mut f, id, &[&first, &second]), [w1, w2]);

    // Return leaves too.
    f.niri_state()
        .do_action(niri_config::Action::ResizeMode, false);
    tap_key(&mut f, "RTRN");
    assert!(!f.niri().resize_mode.is_active());
}

#[test]
fn resize_mode_bind_toggles() {
    let config = Config::parse_mem(
        r#"
        layout { layout-mode "i3"; }
        binds {
            R { resize-mode; }
        }
        "#,
    )
    .unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);

    tap_key(&mut f, "AD04"); // r
    assert!(f.niri().resize_mode.is_active());
    tap_key(&mut f, "AD04");
    assert!(!f.niri().resize_mode.is_active());
}

fn active_workspace_number(f: &mut Fixture) -> Option<u8> {
    f.niri().layout.active_workspace().unwrap().number()
}

fn move_to_workspace(n: u8, focus: Option<bool>) -> niri_config::Action {
    niri_config::Action::MoveWindowToWorkspace(niri_config::WorkspaceReference::Index(n), focus)
}

#[test]
fn pushaway_keeps_focus_on_current_workspace() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    f.niri_state().do_action(
        niri_config::Action::FocusWorkspace(niri_config::WorkspaceReference::Index(1)),
        false,
    );
    assert_eq!(active_workspace_number(&mut f), Some(1));

    // The window goes to workspace 3, and we stay on 1 with the other window focused.
    let moved = focused_id(&mut f);
    f.niri_state().do_action(move_to_workspace(3, None), false);
    f.double_roundtrip(id);
    assert_eq!(active_workspace_number(&mut f), Some(1));
    assert_ne!(focused_id(&mut f), moved);
    assert!(f
        .niri()
        .layout
        .windows()
        .any(|(_, m)| Some(m.id()) == moved));

    // focus=true on the action still follows the window.
    f.niri_state()
        .do_action(move_to_workspace(4, Some(true)), false);
    f.double_roundtrip(id);
    assert_eq!(active_workspace_number(&mut f), Some(4));
}

#[test]
fn pushaway_off_follows_the_window() {
    let config = Config::parse_mem(
        r#"
        layout {
            layout-mode "i3"
            pushaway false
        }
        "#,
    )
    .unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let _first = open_window(&mut f, id);

    f.niri_state().do_action(move_to_workspace(3, None), false);
    f.double_roundtrip(id);
    assert_eq!(active_workspace_number(&mut f), Some(3));
}

/// Presses move-right at the given time, in milliseconds.
fn move_right_at(f: &mut Fixture, msec: u64) {
    f.niri()
        .clock
        .set_unadjusted(std::time::Duration::from_millis(msec));
    f.niri_state()
        .do_action(niri_config::Action::MoveColumnRight, false);
}

fn workspace_number_of(f: &mut Fixture, id: crate::window::mapped::MappedId) -> Option<u8> {
    let ws = f
        .niri()
        .layout
        .workspaces()
        .find(|(_, _, ws)| ws.windows().any(|m| m.id() == id))
        .map(|(_, _, ws)| ws.number());
    ws.flatten()
}

#[test]
fn poof_sends_window_to_workspace_0() {
    // The puff plays even with animations off.
    let config = r#"layout { layout-mode "i3"; }; animations { off; }"#;
    let mut f = Fixture::with_config(Config::parse_mem(config).unwrap());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let output = f.niri_output(1);

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();
    // The second window is focused, at the right edge.
    assert_eq!(focused_id(&mut f), Some(ids[1]));

    // Three presses against the edge in quick succession poof it.
    move_right_at(&mut f, 1000);
    move_right_at(&mut f, 1200);
    assert_eq!(workspace_number_of(&mut f, ids[1]), Some(1));
    assert!(!f.niri().poof.are_animations_ongoing(&output));
    move_right_at(&mut f, 1400);

    // There's a puff, and it's still going a bit later.
    assert!(f.niri().poof.are_animations_ongoing(&output));
    f.niri()
        .clock
        .set_unadjusted(std::time::Duration::from_millis(1900));
    f.niri().advance_animations();
    assert!(f.niri().poof.are_animations_ongoing(&output));
    f.double_roundtrip(id);

    assert_eq!(workspace_number_of(&mut f, ids[1]), Some(0));
    assert_eq!(
        f.niri().layout.active_workspace().unwrap().number(),
        Some(1)
    );
    assert_eq!(focused_id(&mut f), Some(ids[0]));
}

#[test]
fn slow_edge_presses_dont_poof() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();

    // Too far apart.
    move_right_at(&mut f, 1000);
    move_right_at(&mut f, 1200);
    move_right_at(&mut f, 2000);
    assert_eq!(workspace_number_of(&mut f, ids[1]), Some(1));

    // A move that does something in between resets the gesture.
    move_right_at(&mut f, 3000);
    move_right_at(&mut f, 3100);
    f.niri_state()
        .do_action(niri_config::Action::MoveColumnLeft, false);
    move_right_at(&mut f, 3200);
    assert_eq!(workspace_number_of(&mut f, ids[1]), Some(1));
    // That one moved it back to the edge, so these are the first two presses against it.
    move_right_at(&mut f, 3300);
    move_right_at(&mut f, 3400);
    assert_eq!(workspace_number_of(&mut f, ids[1]), Some(1));
    // And the third one poofs it.
    move_right_at(&mut f, 3500);
    assert_eq!(workspace_number_of(&mut f, ids[1]), Some(0));
}

/// The i3 tree of the active workspace, as text.
fn active_tree(f: &mut Fixture) -> String {
    let ws = f.niri().layout.active_workspace().unwrap();
    ws.tiling().unwrap().tree_string()
}

/// Opens 1 | (2 / 3), with 3 focused, and returns the surfaces.
fn open_nested(f: &mut Fixture, id: ClientId) -> [WlSurface; 3] {
    let first = open_window(f, id);
    let second = open_window(f, id);
    f.niri()
        .layout
        .split(crate::layout::tiling::SplitCommand::Vertical);
    let third = open_window(f, id);
    configured_widths(f, id, &[&first, &second, &third]);
    [first, second, third]
}

fn move_left_at(f: &mut Fixture, msec: u64) {
    f.niri()
        .clock
        .set_unadjusted(std::time::Duration::from_millis(msec));
    f.niri_state()
        .do_action(niri_config::Action::MoveColumnLeft, false);
}

/// Drags a window to the right edge of output 1, and flings it.
fn fling(f: &mut Fixture, window: &smithay::desktop::Window, msec: u64) -> bool {
    let output = f.niri_output(1);
    let niri = f.niri();
    niri.layout
        .interactive_move_begin(window.clone(), &output, Point::from((1000., 300.)));
    niri.layout.interactive_move_update(
        window,
        Point::from((919., 0.)),
        output.clone(),
        Point::from((1919., 300.)),
    );
    assert!(niri.layout.interactive_move_origin(window).is_some());

    f.niri()
        .clock
        .set_unadjusted(std::time::Duration::from_millis(msec));
    f.niri_state().fling_to_scratchpad(window, &output, 300.)
}

#[test]
fn undo_poof_restores_place() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _surfaces = open_nested(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();
    assert_eq!(focused_id(&mut f), Some(ids[2]));

    // The first press moves 3 out of its container, to the right edge: 1 | 2 | 3.
    move_right_at(&mut f, 500);
    f.double_roundtrip(id);
    let before = active_tree(&mut f);

    for msec in [1000, 1100, 1200] {
        move_right_at(&mut f, msec);
    }
    assert_eq!(workspace_number_of(&mut f, ids[2]), Some(0));
    assert_ne!(active_tree(&mut f), before);

    // Move-left right after puts it back in the same place, with the same sizes, focused.
    move_left_at(&mut f, 1700);
    f.double_roundtrip(id);
    assert_eq!(workspace_number_of(&mut f, ids[2]), Some(1));
    assert_eq!(active_tree(&mut f), before);
    assert_eq!(focused_id(&mut f), Some(ids[2]));
    // Workspace 0 went away with it.
    assert!(f
        .niri()
        .layout
        .workspaces()
        .all(|(_, _, ws)| ws.number() != Some(0)));

    // The undo is used up: another move-left moves as usual.
    move_left_at(&mut f, 1800);
    assert_eq!(workspace_number_of(&mut f, ids[2]), Some(1));
    assert_ne!(active_tree(&mut f), before);
}

#[test]
fn late_move_left_doesnt_undo() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();

    for msec in [1000, 1100, 1200] {
        move_right_at(&mut f, msec);
    }
    assert_eq!(workspace_number_of(&mut f, ids[1]), Some(0));
    move_left_at(&mut f, 3500);
    assert_eq!(workspace_number_of(&mut f, ids[1]), Some(0));
}

#[test]
fn fling_poofs_dragged_window() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let output = f.niri_output(1);

    let _surfaces = open_nested(&mut f, id);
    let windows: Vec<_> = f
        .niri()
        .layout
        .windows()
        .map(|(_, m)| (m.id(), m.window.clone()))
        .collect();
    let before = active_tree(&mut f);

    // Fling 2, from inside the vertical container.
    let (id2, window2) = windows[1].clone();
    assert!(fling(&mut f, &window2, 1000));
    assert!(f.niri().poof.are_animations_ongoing(&output));
    f.double_roundtrip(id);
    assert_eq!(workspace_number_of(&mut f, id2), Some(0));

    // Move-left puts it back where the drag started.
    move_left_at(&mut f, 1500);
    f.double_roundtrip(id);
    assert_eq!(workspace_number_of(&mut f, id2), Some(1));
    assert_eq!(active_tree(&mut f), before);
}

#[test]
fn undo_recreates_emptied_container() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _surfaces = open_nested(&mut f, id);
    let windows: Vec<_> = f
        .niri()
        .layout
        .windows()
        .map(|(_, m)| (m.id(), m.window.clone()))
        .collect();

    // Fling 2 and 3 one after the other, so the vertical container empties and goes away.
    let (_, window2) = windows[1].clone();
    assert!(fling(&mut f, &window2, 1000));
    f.double_roundtrip(id);
    let before = active_tree(&mut f);
    assert!(before.contains("SplitV["), "{before}");

    let (id3, window3) = windows[2].clone();
    assert!(fling(&mut f, &window3, 5000));
    f.double_roundtrip(id);
    assert_eq!(workspace_number_of(&mut f, id3), Some(0));
    assert!(!active_tree(&mut f).contains("SplitV["));

    // Undo recreates the container around 3.
    move_left_at(&mut f, 5500);
    f.double_roundtrip(id);
    assert_eq!(active_tree(&mut f), before);
}
