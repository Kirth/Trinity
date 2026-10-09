use client::ClientId;
use insta::assert_snapshot;
use niri_config::Config;
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
