use client::ClientId;
use insta::assert_snapshot;
use niri_config::Config;
use smithay::input::pointer::GrabStartData as PointerGrabStartData;
use smithay::utils::{Point, Size};
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;
use crate::input::drawer_grab::DrawerGrab;

fn i3_config() -> Config {
    Config::parse_mem(r#"layout { layout-mode "i3"; }"#).unwrap()
}

/// i3 mode with limbo in legacy mode, for the limbo tests.
fn limbo_config() -> Config {
    Config::parse_mem(r#"layout { layout-mode "i3"; }; limbo { legacy-mode; }"#).unwrap()
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

fn in_limbo(f: &mut Fixture, id: crate::window::mapped::MappedId) -> bool {
    let layout = &f.niri().layout;
    let in_limbo = layout.limbo_windows().any(|m| m.id() == id);
    if in_limbo {
        // Then it's on no workspace.
        assert!(workspace_number_of(f, id).is_none());
    }
    in_limbo
}

fn no_workspace_0(f: &mut Fixture) -> bool {
    f.niri()
        .layout
        .workspaces()
        .all(|(_, _, ws)| ws.number() != Some(0))
}

#[test]
fn poof_sends_window_to_limbo() {
    // The puff plays even with animations off.
    let config = r#"layout { layout-mode "i3"; }; limbo { legacy-mode; }; animations { off; }"#;
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

    assert!(in_limbo(&mut f, ids[1]));
    assert!(no_workspace_0(&mut f));
    assert_eq!(
        f.niri().layout.active_workspace().unwrap().number(),
        Some(1)
    );
    assert_eq!(focused_id(&mut f), Some(ids[0]));
}

#[test]
fn slow_edge_presses_dont_poof() {
    let mut f = Fixture::with_config(limbo_config());
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
    assert!(in_limbo(&mut f, ids[1]));
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
    f.niri_state().fling_to_limbo(window, &output, 300.)
}

#[test]
fn undo_poof_restores_place() {
    let mut f = Fixture::with_config(limbo_config());
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
    assert!(in_limbo(&mut f, ids[2]));
    assert_ne!(active_tree(&mut f), before);

    // Move-left right after puts it back in the same place, with the same sizes, focused.
    move_left_at(&mut f, 1500);
    f.double_roundtrip(id);
    assert_eq!(workspace_number_of(&mut f, ids[2]), Some(1));
    assert_eq!(active_tree(&mut f), before);
    assert_eq!(focused_id(&mut f), Some(ids[2]));
    // Limbo isn't a workspace.
    assert!(no_workspace_0(&mut f));

    // The undo is used up: another move-left moves as usual.
    move_left_at(&mut f, 1600);
    assert_eq!(workspace_number_of(&mut f, ids[2]), Some(1));
    assert_ne!(active_tree(&mut f), before);
}

#[test]
fn late_move_left_doesnt_undo() {
    let mut f = Fixture::with_config(limbo_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();

    for msec in [1000, 1100, 1200] {
        move_right_at(&mut f, msec);
    }
    assert!(in_limbo(&mut f, ids[1]));
    move_left_at(&mut f, 3500);
    assert!(in_limbo(&mut f, ids[1]));
}

#[test]
fn fling_poofs_dragged_window() {
    let mut f = Fixture::with_config(limbo_config());
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
    assert!(in_limbo(&mut f, id2));

    // Move-left puts it back where the drag started.
    move_left_at(&mut f, 1500);
    f.double_roundtrip(id);
    assert_eq!(workspace_number_of(&mut f, id2), Some(1));
    assert_eq!(active_tree(&mut f), before);
}

#[test]
fn undo_recreates_emptied_container() {
    let mut f = Fixture::with_config(limbo_config());
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
    assert!(in_limbo(&mut f, id3));
    assert!(!active_tree(&mut f).contains("SplitV["));

    // Undo recreates the container around 3.
    move_left_at(&mut f, 5500);
    f.double_roundtrip(id);
    assert_eq!(active_tree(&mut f), before);
}

fn drawer_ids(f: &mut Fixture) -> Vec<crate::window::mapped::MappedId> {
    let output = f.niri_output(1);
    let niri = f.niri();
    niri.advance_animations();
    niri.drawer_windows(&output)
        .into_iter()
        .map(|mapped| mapped.id())
        .collect()
}

#[test]
fn drawer_shows_poofed_windows() {
    let mut f = Fixture::with_config(limbo_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let _third = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();
    assert!(drawer_ids(&mut f).is_empty());

    // Poof 3, then 2: the newest goes first.
    for msec in [1000, 1100, 1200] {
        move_right_at(&mut f, msec);
    }
    assert_eq!(drawer_ids(&mut f), [ids[2]]);
    for msec in [3000, 3100, 3200] {
        move_right_at(&mut f, msec);
    }
    assert_eq!(drawer_ids(&mut f), [ids[1], ids[2]]);

    // The cards peek out at the right edge, and a point on one finds its window.
    let output = f.niri_output(1);
    let niri = f.niri();
    let mut found = Vec::new();
    for y in (0..1080).step_by(4) {
        if let Some(id) = niri.drawer_window_under(&output, Point::from((1915., y as f64))) {
            if !found.contains(&id) {
                found.push(id);
            }
        }
    }
    assert_eq!(found, [ids[1], ids[2]]);
    assert_eq!(
        niri.drawer_window_under(&output, Point::from((1800., 540.))),
        None
    );

    // In the overview, they show in a row below the workspaces instead.
    niri.layout.toggle_overview();
    niri.layout.refresh(true);
    assert_eq!(drawer_ids(&mut f), [ids[1], ids[2]]);

    let niri = f.niri();
    let band = niri
        .layout
        .monitor_for_output(&output)
        .unwrap()
        .overview_limbo_band()
        .unwrap();
    let slots = niri.drawer_slots(&output);
    assert_eq!(
        slots.iter().map(|slot| slot.id).collect::<Vec<_>>(),
        [ids[1], ids[2]]
    );
    // Left to right, newest first, inside the band.
    assert!(slots[0].card.loc.x + slots[0].card.size.w <= slots[1].card.loc.x);
    for slot in &slots {
        assert!(band.contains_rect(slot.card), "{slot:?} outside {band:?}");
        let center = slot.card.loc + slot.card.size.downscale(2.).to_point();
        assert_eq!(niri.drawer_window_under(&output, center), Some(slot.id));
    }
    // Nothing at the old right edge spot.
    assert_eq!(
        niri.drawer_window_under(&output, Point::from((1915., 540.))),
        None
    );
}

#[test]
fn pull_from_drawer_focuses_window() {
    let mut f = Fixture::with_config(limbo_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let output = f.niri_output(1);

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();

    for msec in [1000, 1100, 1200] {
        move_right_at(&mut f, msec);
    }
    assert_eq!(drawer_ids(&mut f), [ids[1]]);
    assert_eq!(focused_id(&mut f), Some(ids[0]));

    assert!(f.niri_state().pull_from_drawer(ids[1], &output).is_some());
    f.double_roundtrip(id);

    assert_eq!(workspace_number_of(&mut f, ids[1]), Some(1));
    assert_eq!(focused_id(&mut f), Some(ids[1]));
    assert!(drawer_ids(&mut f).is_empty());
    assert!(no_workspace_0(&mut f));

    // It's no longer undoable as a poof.
    move_left_at(&mut f, 1500);
    assert_eq!(workspace_number_of(&mut f, ids[1]), Some(1));
}

fn poof(f: &mut Fixture, start_msec: u64) {
    for msec in [start_msec, start_msec + 100, start_msec + 200] {
        move_right_at(f, msec);
    }
}

#[test]
fn poof_from_workspace_0() {
    let mut f = Fixture::with_config(limbo_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    // Workspace 0 is a normal workspace.
    f.niri_state().do_action(
        niri_config::Action::FocusWorkspace(niri_config::WorkspaceReference::Index(0)),
        false,
    );
    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();
    assert_eq!(workspace_number_of(&mut f, ids[0]), Some(0));

    poof(&mut f, 1000);
    assert!(in_limbo(&mut f, ids[1]));
    assert_eq!(workspace_number_of(&mut f, ids[0]), Some(0));

    // And undo puts it back on workspace 0.
    move_left_at(&mut f, 1500);
    assert_eq!(workspace_number_of(&mut f, ids[1]), Some(0));
}

#[test]
fn activating_pulls_out_of_limbo() {
    let mut f = Fixture::with_config(limbo_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();

    poof(&mut f, 1000);
    assert!(in_limbo(&mut f, ids[1]));

    f.niri_state()
        .do_action(niri_config::Action::FocusWindow(ids[1].get()), false);
    f.double_roundtrip(id);
    assert!(!in_limbo(&mut f, ids[1]));
    assert_eq!(workspace_number_of(&mut f, ids[1]), Some(1));
    assert_eq!(focused_id(&mut f), Some(ids[1]));
}

fn limbo_action(f: &mut Fixture, id: ClientId, action: niri_config::Action) {
    f.niri_state().do_action(action, false);
    f.double_roundtrip(id);
}

#[test]
fn move_window_to_limbo_action() {
    let mut f = Fixture::with_config(limbo_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let _third = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();

    // The focused window goes.
    limbo_action(&mut f, id, niri_config::Action::MoveWindowToLimbo);
    assert!(in_limbo(&mut f, ids[2]));

    // By id, a window that isn't focused goes, and the focus stays.
    let focus = focused_id(&mut f);
    limbo_action(
        &mut f,
        id,
        niri_config::Action::MoveWindowToLimboById(ids[0].get()),
    );
    assert!(in_limbo(&mut f, ids[0]));
    assert_eq!(focused_id(&mut f), focus);
    assert_eq!(drawer_ids(&mut f), [ids[0], ids[2]]);

    // A move-left right after undoes it like a poof.
    let window = f.niri().layout.focus().unwrap().window.clone();
    limbo_action(&mut f, id, niri_config::Action::MoveWindowToLimbo);
    assert!(f.niri().layout.is_in_limbo(&window));
    limbo_action(&mut f, id, niri_config::Action::MoveColumnLeft);
    assert!(!f.niri().layout.is_in_limbo(&window));
}

#[test]
fn limbo_actions_need_i3_mode() {
    let config = Config::parse_mem(r#"layout { layout-mode "scrolling"; }"#).unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    limbo_action(&mut f, id, niri_config::Action::MoveWindowToLimbo);
    limbo_action(&mut f, id, niri_config::Action::ToggleLimboWindow);
    assert_eq!(f.niri().layout.limbo_windows().count(), 0);
    assert_eq!(f.niri().layout.windows().count(), 1);
}

#[test]
fn pull_from_limbo_walks_the_stack() {
    let mut f = Fixture::with_config(limbo_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let _third = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();

    limbo_action(&mut f, id, niri_config::Action::MoveWindowToLimbo);
    limbo_action(&mut f, id, niri_config::Action::MoveWindowToLimbo);
    assert_eq!(drawer_ids(&mut f), [ids[1], ids[2]]);

    // Newest first, each next to the focused window and focused.
    limbo_action(&mut f, id, niri_config::Action::PullFromLimbo);
    assert!(!in_limbo(&mut f, ids[1]));
    assert_eq!(workspace_number_of(&mut f, ids[1]), Some(1));
    assert_eq!(focused_id(&mut f), Some(ids[1]));

    limbo_action(&mut f, id, niri_config::Action::PullFromLimbo);
    assert!(!in_limbo(&mut f, ids[2]));
    assert_eq!(workspace_number_of(&mut f, ids[2]), Some(1));
    assert_eq!(focused_id(&mut f), Some(ids[2]));

    // With limbo empty, nothing happens.
    limbo_action(&mut f, id, niri_config::Action::PullFromLimbo);
    assert_eq!(focused_id(&mut f), Some(ids[2]));
}

#[test]
fn toggle_limbo_window_shows_and_hides() {
    let mut f = Fixture::with_config(limbo_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let _third = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();

    limbo_action(&mut f, id, niri_config::Action::MoveWindowToLimbo);
    assert_eq!(drawer_ids(&mut f), [ids[2]]);

    // A focused window that didn't come from limbo stays: the toggle shows the newest instead.
    limbo_action(&mut f, id, niri_config::Action::ToggleLimboWindow);
    assert!(!in_limbo(&mut f, ids[2]));
    assert_eq!(focused_id(&mut f), Some(ids[2]));

    // Again hides it, and again shows it.
    limbo_action(&mut f, id, niri_config::Action::ToggleLimboWindow);
    assert!(in_limbo(&mut f, ids[2]));
    assert!(!in_limbo(&mut f, ids[1]));
    limbo_action(&mut f, id, niri_config::Action::ToggleLimboWindow);
    assert!(!in_limbo(&mut f, ids[2]));
    assert_eq!(focused_id(&mut f), Some(ids[2]));

    // By id, a window outside limbo goes in, and one in limbo comes out.
    let toggle =
        |id: crate::window::mapped::MappedId| niri_config::Action::ToggleLimboWindowById(id.get());
    limbo_action(&mut f, id, toggle(ids[0]));
    assert!(in_limbo(&mut f, ids[0]));
    limbo_action(&mut f, id, toggle(ids[0]));
    assert!(!in_limbo(&mut f, ids[0]));
    assert_eq!(focused_id(&mut f), Some(ids[0]));
}

#[test]
fn hints_label_limbo_windows_last() {
    let mut f = Fixture::with_config(limbo_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let output = f.niri_output(1);

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let _third = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();

    poof(&mut f, 1000);
    assert_eq!(drawer_ids(&mut f), [ids[2]]);

    // The visible windows get a and s, the card in the drawer comes after them.
    f.niri_state().open_ace_hints();
    let niri = f.niri();
    assert_eq!(niri.ace_hints.label_for(ids[0]), Some(("a", 0)));
    assert_eq!(niri.ace_hints.label_for(ids[1]), Some(("s", 0)));
    assert_eq!(niri.ace_hints.label_for(ids[2]), Some(("d", 0)));
    assert_eq!(
        niri.ace_hint_drawer_cards(&output)
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>(),
        [ids[2]]
    );

    // The drawer slides all the way out while the hints are shown.
    let niri = f.niri();
    niri.advance_animations();
    let now = niri.clock.now_unadjusted();
    niri.clock
        .set_unadjusted(now + std::time::Duration::from_secs(1));
    niri.advance_animations();
    let (_, loc) = niri.ace_hint_drawer_cards(&output)[0];
    // Further in than the 10 px it peeks out by otherwise.
    assert!(loc.x < 1920. - 30., "{loc:?}");

    // Typing its label brings the window out of limbo.
    tap_key(&mut f, "AC03"); // d
    f.double_roundtrip(id);
    assert!(!f.niri().ace_hints.is_open());
    assert!(!in_limbo(&mut f, ids[2]));
    assert_eq!(focused_id(&mut f), Some(ids[2]));
}

#[test]
fn drawer_closes_window() {
    let mut f = Fixture::with_config(limbo_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    let second = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();

    // A window that isn't in limbo isn't closed from the drawer.
    f.niri_state().close_drawer_window(ids[1]);
    f.double_roundtrip(id);
    assert!(!f.client(id).window(&second).close_requested);

    poof(&mut f, 1000);
    assert_eq!(drawer_ids(&mut f), [ids[1]]);
    f.niri_state().close_drawer_window(ids[1]);
    f.double_roundtrip(id);
    assert!(f.client(id).window(&second).close_requested);
}

#[test]
fn drawer_cycles_only_when_cards_overflow() {
    let mut f = Fixture::with_config(limbo_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let output = f.niri_output(1);

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    poof(&mut f, 1000);
    drawer_ids(&mut f);
    assert!(!f.niri().drawer_overflows(&output));
}

/// Starts dragging the card of a window in the drawer from the middle of its visible strip.
fn drag_card(f: &mut Fixture, id: crate::window::mapped::MappedId) -> DrawerGrab {
    let output = f.niri_output(1);
    let state = f.niri_state();
    let slot = state
        .niri
        .drawer_slots(&output)
        .into_iter()
        .find(|slot| slot.id == id)
        .unwrap();
    let location = slot.hit.loc + slot.hit.size.downscale(2.).to_point();
    let start_data = PointerGrabStartData {
        focus: None,
        button: 0x110,
        location,
    };
    DrawerGrab::new(state, start_data, output, id).unwrap()
}

fn three_in_limbo(f: &mut Fixture, id: ClientId) -> Vec<crate::window::mapped::MappedId> {
    for _ in 0..4 {
        open_window(f, id);
    }
    for _ in 0..3 {
        limbo_action(f, id, niri_config::Action::MoveWindowToLimbo);
    }
    let order = drawer_ids(f);
    assert_eq!(order.len(), 3);
    order
}

#[test]
fn dragging_a_card_along_the_drawer_rearranges() {
    let mut f = Fixture::with_config(limbo_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let order = three_in_limbo(&mut f, id);

    let mut grab = drag_card(&mut f, order[0]);
    // Down past the last card.
    let output = f.niri_output(1);
    let last = f.niri().drawer_slots(&output)[2].card;
    let pos = Point::from((1915., last.loc.y + last.size.h));
    assert!(grab.on_motion(f.niri_state(), pos, pos).is_none());
    assert_eq!(drawer_ids(&mut f), [order[1], order[2], order[0]]);
    assert_eq!(f.niri().drawer.dragging(), Some(order[0]));

    // Letting go leaves it there, still in limbo.
    grab.on_ungrab(f.niri_state());
    assert_eq!(drawer_ids(&mut f), [order[1], order[2], order[0]]);
    assert_eq!(f.niri().drawer.dragging(), None);
}

#[test]
fn dragging_a_card_into_the_screen_pulls_it_out() {
    let mut f = Fixture::with_config(limbo_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let order = three_in_limbo(&mut f, id);

    let mut grab = drag_card(&mut f, order[1]);
    let pos = Point::from((1000., 540.));
    assert!(grab.on_motion(f.niri_state(), pos, pos).is_some());
    grab.on_ungrab(f.niri_state());
    f.double_roundtrip(id);
    assert!(!in_limbo(&mut f, order[1]));
    assert_eq!(drawer_ids(&mut f), [order[0], order[2]]);
}

#[test]
fn clicking_a_card_brings_it_out() {
    let mut f = Fixture::with_config(limbo_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let order = three_in_limbo(&mut f, id);

    // A wobble smaller than the threshold is still a click.
    let mut grab = drag_card(&mut f, order[2]);
    let output = f.niri_output(1);
    let slot = f.niri().drawer_slots(&output)[2].hit;
    let pos = slot.loc + slot.size.downscale(2.).to_point() + Point::from((0., 3.));
    assert!(grab.on_motion(f.niri_state(), pos, pos).is_none());
    grab.on_ungrab(f.niri_state());
    f.double_roundtrip(id);
    assert!(!in_limbo(&mut f, order[2]));
    assert_eq!(focused_id(&mut f), Some(order[2]));
}

#[test]
fn pushing_against_the_edge_slides_all_cards_out() {
    use crate::ui::drawer::{edge_pointer, EdgePointer};

    let mut f = Fixture::with_config(limbo_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let output = f.niri_output(1);
    three_in_limbo(&mut f, id);

    // Against the right edge, level with the middle card.
    let niri = f.niri();
    let slots = niri.drawer_slots(&output);
    let pos = Point::from((1919.5, slots[1].card.loc.y + 10.));
    let state = edge_pointer(
        niri_config::LimboEdge::Right,
        Size::from((1920., 1080.)),
        &slots,
        pos,
    );
    assert_eq!(state, EdgePointer::AtEdge);

    niri.advance_animations();
    let now = niri.clock.now();
    niri.drawer.set_edge_pointer(&output, state, now);
    let start = niri.clock.now_unadjusted();
    // Past the dwell, then past the slide.
    for ms in [200, 400] {
        niri.clock
            .set_unadjusted(start + std::time::Duration::from_millis(ms));
        niri.advance_animations();
    }
    let slots = niri.drawer_slots(&output);
    assert_eq!(slots.len(), 3);
    for slot in slots {
        assert!(slot.card.loc.x < 1920. - 30., "{:?}", slot.card);
    }
}

#[test]
fn limbo_window_closes_cleanly() {
    let mut f = Fixture::with_config(limbo_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    let second = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();

    poof(&mut f, 1000);
    assert_eq!(drawer_ids(&mut f), [ids[1]]);

    // It commits while in limbo, then goes away.
    let window = f.client(id).window(&second);
    window.attach_new_buffer();
    window.commit();
    f.double_roundtrip(id);
    let window = f.client(id).window(&second);
    window.attach_null();
    window.commit();
    f.double_roundtrip(id);

    assert!(!f.niri().layout.windows().any(|(_, m)| m.id() == ids[1]));
    assert!(drawer_ids(&mut f).is_empty());
}

#[test]
fn limbo_windows_listed_without_workspace() {
    let mut f = Fixture::with_config(limbo_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();
    poof(&mut f, 1000);

    let mut listed = Vec::new();
    f.niri().layout.with_windows(|mapped, output, ws_id, _| {
        listed.push((mapped.id(), output.is_some(), ws_id.is_some()));
    });
    assert!(listed.contains(&(ids[1], false, false)));
    assert!(listed.contains(&(ids[0], true, true)));
}

#[test]
fn drawer_on_left_edge() {
    let config = r#"layout { layout-mode "i3"; }; limbo { legacy-mode; edge "left"; }"#;
    let mut f = Fixture::with_config(Config::parse_mem(config).unwrap());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let output = f.niri_output(1);

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();

    // The gesture still uses the right edge.
    poof(&mut f, 1000);
    assert_eq!(drawer_ids(&mut f), [ids[1]]);

    // The card peeks out on the left.
    let niri = f.niri();
    let hits = |x: f64| {
        (0..1080)
            .step_by(4)
            .any(|y| niri.drawer_window_under(&output, Point::from((x, y as f64))) == Some(ids[1]))
    };
    assert!(hits(5.));
    assert!(!hits(1915.));
}

fn strip_ids(f: &mut Fixture) -> Vec<crate::window::mapped::MappedId> {
    let output = f.niri_output(1);
    let niri = f.niri();
    niri.layout.strip_windows(&output).map(|m| m.id()).collect()
}

#[test]
fn poof_sends_window_to_the_strip() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();
    let _ = f.client(id).window(&first).format_recent_configures();

    poof(&mut f, 1000);
    f.double_roundtrip(id);
    assert_eq!(strip_ids(&mut f), [ids[1]]);
    assert!(f.niri().layout.limbo_windows().next().is_none());
    // The window stays on its workspace.
    let ws = f.niri().layout.active_workspace().unwrap();
    assert!(ws.strip_windows().any(|m| m.id() == ids[1]));
    assert_eq!(focused_id(&mut f), Some(ids[0]));
    // The drawer shows the strip.
    assert_eq!(drawer_ids(&mut f), [ids[1]]);

    // The first window takes the room, minus the strip.
    assert_snapshot!(
        f.client(id).window(&first).format_recent_configures(),
        @"size: 1672 × 1048, bounds: 1672 × 1048, states: [Activated]"
    );

    // Move-left right after undoes it.
    move_left_at(&mut f, 1400);
    assert!(strip_ids(&mut f).is_empty());
    assert_eq!(focused_id(&mut f), Some(ids[1]));
}

#[test]
fn clicking_a_strip_card_swaps_it_in() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let output = f.niri_output(1);

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();
    poof(&mut f, 1000);
    assert_eq!(focused_id(&mut f), Some(ids[0]));

    f.niri_state().click_drawer_card(ids[1], &output);
    f.double_roundtrip(id);
    assert_eq!(focused_id(&mut f), Some(ids[1]));
    assert_eq!(strip_ids(&mut f), [ids[0]]);
}

#[test]
fn the_only_window_can_go_to_the_strip() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();
    poof(&mut f, 1000);
    assert_eq!(strip_ids(&mut f), [ids[0]]);
    assert_eq!(focused_id(&mut f), None);

    limbo_action(&mut f, id, niri_config::Action::PullFromLimbo);
    assert!(strip_ids(&mut f).is_empty());
    assert_eq!(focused_id(&mut f), Some(ids[0]));
}

#[test]
fn limbo_actions_use_the_strip_with_limbo_off() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();

    limbo_action(&mut f, id, niri_config::Action::MoveWindowToLimbo);
    assert_eq!(strip_ids(&mut f), [ids[1]]);
    limbo_action(&mut f, id, niri_config::Action::PullFromLimbo);
    assert!(strip_ids(&mut f).is_empty());
    assert_eq!(focused_id(&mut f), Some(ids[1]));
}

#[test]
fn closing_a_strip_window_from_its_card() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    let second = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();
    poof(&mut f, 1000);

    f.niri_state().close_drawer_window(ids[1]);
    f.double_roundtrip(id);
    assert!(f.client(id).window(&second).close_requested);
}

#[test]
fn strip_actions_swap_windows_in() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    for _ in 0..3 {
        open_window(&mut f, id);
    }
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();
    // Set aside 3, then 2, so the strip is [2, 3] and 1 is focused.
    limbo_action(&mut f, id, niri_config::Action::MoveWindowToLimbo);
    limbo_action(&mut f, id, niri_config::Action::MoveWindowToLimbo);
    assert_eq!(strip_ids(&mut f), [ids[1], ids[2]]);
    assert_eq!(focused_id(&mut f), Some(ids[0]));

    // The newest goes back and forth.
    limbo_action(&mut f, id, niri_config::Action::SwapWithStrip(1));
    assert_eq!(focused_id(&mut f), Some(ids[1]));
    limbo_action(&mut f, id, niri_config::Action::SwapWithStrip(1));
    assert_eq!(focused_id(&mut f), Some(ids[0]));
    // Out of range does nothing.
    limbo_action(&mut f, id, niri_config::Action::SwapWithStrip(5));
    assert_eq!(focused_id(&mut f), Some(ids[0]));

    // Cycling walks through all of them.
    let mut seen = Vec::new();
    for _ in 0..3 {
        limbo_action(&mut f, id, niri_config::Action::CycleStrip);
        seen.push(focused_id(&mut f).unwrap());
    }
    seen.sort_by_key(|id| id.get());
    let mut all = ids.clone();
    all.sort_by_key(|id| id.get());
    assert_eq!(seen, all);
}

#[test]
fn overview_shows_each_workspace_strip() {
    let config = r#"layout { layout-mode "i3"; }; animations { off; }"#;
    let mut f = Fixture::with_config(Config::parse_mem(config).unwrap());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let output = f.niri_output(1);

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();
    poof(&mut f, 1000);

    f.niri_state()
        .do_action(niri_config::Action::OpenOverview, false);
    f.niri().layout.advance_animations();
    let niri = f.niri();
    let groups = niri.card_groups(&output);
    assert_eq!(groups.len(), 1);
    let (windows, placement) = &groups[0];
    assert_eq!(windows.iter().map(|m| m.id()).collect::<Vec<_>>(), [ids[1]]);

    // The strip sits inside the workspace's thumbnail, scaled down with it.
    let band = placement.strip.unwrap();
    let mon = niri.layout.monitor_for_output(&output).unwrap();
    let (_, geo) = mon
        .workspaces_with_render_geo()
        .find(|(ws, _)| ws.strip_windows().next().is_some())
        .unwrap();
    assert!(band.size.w < 216.);
    // Give or take rounding the thumbnail to pixels.
    let mut geo = geo;
    geo.size += Size::from((1., 1.));
    assert!(geo.contains_rect(band), "{geo:?} {band:?}");
    let slots = niri.drawer_slots(&output);
    assert_eq!(slots.len(), 1);
    assert!(band.contains_rect(slots[0].card));
}

#[test]
fn hints_label_strip_windows_last() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let _third = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();
    poof(&mut f, 1000);
    assert_eq!(strip_ids(&mut f), [ids[2]]);

    f.niri_state().open_ace_hints();
    let niri = f.niri();
    assert_eq!(niri.ace_hints.label_for(ids[0]), Some(("a", 0)));
    assert_eq!(niri.ace_hints.label_for(ids[1]), Some(("s", 0)));
    assert_eq!(niri.ace_hints.label_for(ids[2]), Some(("d", 0)));

    // Typing its label swaps the window in.
    tap_key(&mut f, "AC03"); // d
    assert_eq!(focused_id(&mut f), Some(ids[2]));
    assert_eq!(strip_ids(&mut f).len(), 1);
}

#[test]
fn strip_windows_get_the_full_size() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let _first = open_window(&mut f, id);
    let second = open_window(&mut f, id);
    let _ = f.client(id).window(&second).format_recent_configures();

    // Set aside from a half-width column, it gets the size of a lone window next to the strip.
    poof(&mut f, 1000);
    f.double_roundtrip(id);
    assert_snapshot!(
        f.client(id).window(&second).format_recent_configures(),
        @"size: 1672 × 1048, bounds: 1888 × 1048, states: []"
    );
}

fn annotations_of(
    f: &mut Fixture,
    id: crate::window::mapped::MappedId,
) -> std::collections::BTreeMap<String, niri_ipc::Annotation> {
    let layout = &f.niri().layout;
    let (_, mapped) = layout.windows().find(|(_, m)| m.id() == id).unwrap();
    mapped.ipc_annotations()
}

fn annotate(
    f: &mut Fixture,
    id: crate::window::mapped::MappedId,
    source: &str,
    annotation: niri_ipc::Annotation,
    ttl_ms: Option<u64>,
) {
    f.niri_state().do_action(
        niri_config::Action::AnnotateWindow {
            id: Some(id.get()),
            source: source.to_owned(),
            annotation,
            ttl_ms,
        },
        false,
    );
}

#[test]
fn providers_annotate_windows() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let _first = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();

    let tabs = niri_ipc::Annotation {
        badge: Some(String::from("12 tabs")),
        ..Default::default()
    };
    annotate(&mut f, ids[0], "chromium", tabs.clone(), None);
    let waiting = niri_ipc::Annotation {
        status: Some(niri_ipc::CardStatus::Waiting),
        // Cut down to size, and kept in range.
        text: Some("x".repeat(500)),
        progress: Some(7.),
        ..Default::default()
    };
    annotate(&mut f, ids[0], "claude", waiting, None);

    let annotations = annotations_of(&mut f, ids[0]);
    assert_eq!(annotations["chromium"], tabs);
    assert_eq!(annotations["claude"].text.as_ref().unwrap().len(), 120);
    assert_eq!(annotations["claude"].progress, Some(1.));

    // Clearing one source keeps the others.
    f.niri_state().do_action(
        niri_config::Action::ClearAnnotations {
            id: Some(ids[0].get()),
            source: Some(String::from("claude")),
        },
        false,
    );
    let annotations = annotations_of(&mut f, ids[0]);
    assert_eq!(annotations.keys().collect::<Vec<_>>(), ["chromium"]);

    // Without a source, everything goes.
    f.niri_state().do_action(
        niri_config::Action::ClearAnnotations {
            id: None,
            source: None,
        },
        false,
    );
    assert!(annotations_of(&mut f, ids[0]).is_empty());
}

#[test]
fn annotations_expire_unless_set_again() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let _first = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();
    let badge = |text: &str| niri_ipc::Annotation {
        badge: Some(text.to_owned()),
        ..Default::default()
    };

    annotate(&mut f, ids[0], "test", badge("1"), Some(0));
    std::thread::sleep(std::time::Duration::from_millis(5));
    f.dispatch_server();
    assert!(annotations_of(&mut f, ids[0]).is_empty());

    // Set again before the first one runs out: the newer one stays.
    annotate(&mut f, ids[0], "test", badge("1"), Some(0));
    annotate(&mut f, ids[0], "test", badge("2"), None);
    std::thread::sleep(std::time::Duration::from_millis(5));
    f.dispatch_server();
    assert_eq!(
        annotations_of(&mut f, ids[0])["test"].badge.as_deref(),
        Some("2")
    );
}

#[test]
fn too_many_sources_are_turned_away() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let _first = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();
    for n in 0..10 {
        annotate(&mut f, ids[0], &format!("p{n}"), Default::default(), None);
    }
    assert_eq!(annotations_of(&mut f, ids[0]).len(), 8);
}

#[test]
fn window_cards_follow_the_strip() {
    let mut f = Fixture::with_config(i3_config());
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let _first = open_window(&mut f, id);
    let _second = open_window(&mut f, id);
    let ids: Vec<_> = f.niri().layout.windows().map(|(_, m)| m.id()).collect();
    let cards = |f: &mut Fixture| -> Vec<(crate::window::mapped::MappedId, bool)> {
        f.niri()
            .layout
            .window_cards()
            .into_iter()
            .map(|(m, card)| (m.id(), card.visible))
            .collect()
    };
    assert!(cards(&mut f).is_empty());

    poof(&mut f, 1000);
    assert_eq!(cards(&mut f), [(ids[1], true)]);

    // On another workspace, the card is hidden, until the overview shows it.
    f.niri_state().do_action(
        niri_config::Action::FocusWorkspace(niri_config::WorkspaceReference::Index(2)),
        false,
    );
    assert_eq!(cards(&mut f), [(ids[1], false)]);
    f.niri_state()
        .do_action(niri_config::Action::OpenOverview, false);
    assert_eq!(cards(&mut f), [(ids[1], true)]);
}
