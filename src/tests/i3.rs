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
