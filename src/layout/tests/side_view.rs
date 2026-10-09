use smithay::utils::{Logical, Point, Size};

use super::*;
use crate::layout::side_view::Corner;

fn options(i3: bool) -> Options {
    Options {
        layout: niri_config::Layout {
            layout_mode: if i3 {
                LayoutMode::I3
            } else {
                LayoutMode::Scrolling
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

fn add_window(id: usize) -> Op {
    Op::AddWindow {
        params: TestWindowParams::new(id),
    }
}

/// Lets the windows ack their size requests and finishes the animations.
fn settle() -> [Op; 4] {
    [
        Op::Communicate(1),
        Op::Communicate(2),
        Op::Communicate(3),
        Op::CompleteAnimations,
    ]
}

fn layout_with(i3: bool, ops: impl IntoIterator<Item = Op>) -> Layout<TestWindow> {
    // Refresh so that the first i3 workspace gets its number.
    let mut all = vec![Op::AddOutput(1), Op::Refresh { is_active: true }];
    all.extend(ops);
    all.extend(settle());
    check_ops_with_options(options(i3), all)
}

fn tile_pos_size(
    layout: &Layout<TestWindow>,
    id: usize,
) -> (Point<f64, Logical>, Size<f64, Logical>) {
    layout
        .active_workspace()
        .unwrap()
        .tiles_with_render_positions()
        .find(|(tile, _, _)| *tile.window().id() == id)
        .map(|(tile, pos, _)| (pos, tile.tile_size()))
        .unwrap()
}

fn is_side_view(layout: &Layout<TestWindow>, id: usize) -> bool {
    layout.active_workspace().unwrap().is_side_view(&id)
}

fn expected_pos(layout: &Layout<TestWindow>, id: usize, corner: Corner) -> Point<f64, Logical> {
    let ws = layout.active_workspace().unwrap();
    let area = ws.working_area();
    let gap = layout.options.layout.side_view.gap;
    let (_, size) = tile_pos_size(layout, id);
    let x = if corner.is_left() {
        area.loc.x + gap
    } else {
        area.loc.x + area.size.w - size.w - gap
    };
    let y = if corner.is_top() {
        area.loc.y + gap
    } else {
        area.loc.y + area.size.h - size.h - gap
    };
    Point::from((x, y))
}

fn assert_in_corner(layout: &Layout<TestWindow>, id: usize, corner: Corner) {
    let (pos, _) = tile_pos_size(layout, id);
    let expected = expected_pos(layout, id, corner);
    // Render positions are rounded to physical pixels.
    assert!(
        (pos.x - expected.x).abs() < 1. && (pos.y - expected.y).abs() < 1.,
        "{pos:?} not in {corner:?} corner, expected {expected:?}"
    );
}

#[test]
fn toggle_on_moves_to_bottom_right_at_side_view_width() {
    for i3 in [true, false] {
        let layout = layout_with(
            i3,
            [
                add_window(1),
                add_window(2),
                Op::ToggleSideView { id: None },
            ],
        );

        assert!(is_side_view(&layout, 2));
        assert_in_corner(&layout, 2, Corner::BottomRight);

        let ws = layout.active_workspace().unwrap();
        let expected_w = ws.working_area().size.w * layout.options.layout.side_view.width;
        let (_, size) = tile_pos_size(&layout, 2);
        assert!(
            (size.w - expected_w).abs() <= 1.,
            "{size:?} vs {expected_w}"
        );

        // The side-view window gives up the focus.
        assert_eq!(*layout.focus().unwrap().id(), 1);
    }
}

#[test]
fn toggle_off_restores_tiled_window() {
    for i3 in [true, false] {
        let before = layout_with(i3, [add_window(1), add_window(2)]);
        let (_, size_before) = tile_pos_size(&before, 2);

        let layout = layout_with(
            i3,
            [
                add_window(1),
                add_window(2),
                Op::ToggleSideView { id: Some(2) },
                Op::Communicate(2),
                Op::CompleteAnimations,
                Op::ToggleSideView { id: Some(2) },
            ],
        );

        assert!(!is_side_view(&layout, 2));
        let ws = layout.active_workspace().unwrap();
        assert!(ws.tiles().any(|tile| *tile.window().id() == 2));
        assert!(!ws
            .tiles_with_render_positions()
            .any(|(tile, _, _)| tile.is_side_view()));
        let (_, size) = tile_pos_size(&layout, 2);
        assert_eq!(size.h, size_before.h);
    }
}

#[test]
fn toggle_off_restores_floating_window() {
    let floating_window = Op::AddWindow {
        params: TestWindowParams {
            is_floating: true,
            ..TestWindowParams::new(1)
        },
    };
    let before = layout_with(true, [floating_window.clone()]);
    let (pos_before, size_before) = tile_pos_size(&before, 1);

    let layout = layout_with(
        true,
        [
            floating_window,
            Op::ToggleSideView { id: Some(1) },
            Op::Communicate(1),
            Op::CompleteAnimations,
            Op::ToggleSideView { id: Some(1) },
        ],
    );

    assert!(!is_side_view(&layout, 1));
    let (pos, size) = tile_pos_size(&layout, 1);
    assert_eq!(pos, pos_before);
    assert_eq!(size, size_before);
}

#[test]
fn cycle_corners() {
    let mut layout = layout_with(true, [add_window(1), Op::ToggleSideView { id: None }]);
    assert_in_corner(&layout, 1, Corner::BottomRight);

    for corner in [Corner::BottomLeft, Corner::TopLeft, Corner::TopRight] {
        check_ops_on_layout(
            &mut layout,
            [Op::SideViewCycleCorner(true), Op::CompleteAnimations],
        );
        assert_in_corner(&layout, 1, corner);
    }

    check_ops_on_layout(
        &mut layout,
        [Op::SideViewCycleCorner(false), Op::CompleteAnimations],
    );
    assert_in_corner(&layout, 1, Corner::TopLeft);
}

#[test]
fn stays_anchored_when_window_resizes() {
    let mut layout = layout_with(true, [add_window(1), Op::ToggleSideView { id: None }]);
    assert_in_corner(&layout, 1, Corner::BottomRight);

    check_ops_on_layout(
        &mut layout,
        [
            Op::SetWindowHeight {
                id: Some(1),
                change: SizeChange::SetFixed(300),
            },
            Op::Communicate(1),
            Op::CompleteAnimations,
        ],
    );
    let (_, size) = tile_pos_size(&layout, 1);
    assert_eq!(size.h, 300.);
    assert_in_corner(&layout, 1, Corner::BottomRight);
}

#[test]
fn follows_workspace_switches() {
    for i3 in [true, false] {
        let mut layout = layout_with(
            i3,
            [
                add_window(1),
                add_window(2),
                Op::ToggleSideView { id: Some(2) },
                Op::SideViewCycleCorner(true),
            ],
        );

        let switch = if i3 {
            Op::FocusI3Workspace(2)
        } else {
            Op::FocusWorkspaceDown
        };
        check_ops_on_layout(&mut layout, [switch, Op::CompleteAnimations]);

        assert!(is_side_view(&layout, 2), "i3: {i3}");
        assert_in_corner(&layout, 2, Corner::BottomLeft);
        // The side-view window doesn't take the focus on the new workspace.
        assert!(layout.focus().is_none());

        // And comes back.
        let switch = if i3 {
            Op::FocusI3Workspace(1)
        } else {
            Op::FocusWorkspaceUp
        };
        check_ops_on_layout(&mut layout, [switch, Op::CompleteAnimations]);
        assert!(is_side_view(&layout, 2));
        assert_eq!(
            layout.focus().map(|win| *win.id()),
            Some(1),
            "i3: {i3}, windows: {:?}",
            layout
                .active_workspace()
                .unwrap()
                .tiles()
                .map(|tile| *tile.window().id())
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn directional_focus_skips_side_view() {
    let floating_window = Op::AddWindow {
        params: TestWindowParams {
            is_floating: true,
            ..TestWindowParams::new(3)
        },
    };
    let mut layout = layout_with(
        true,
        [
            add_window(1),
            add_window(2),
            floating_window,
            Op::ToggleSideView { id: Some(2) },
        ],
    );

    // Floating focus moves never land on the side-view window.
    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusFloating,
            Op::FocusColumnLeft,
            Op::FocusColumnRight,
            Op::FocusWindowUp,
            Op::FocusWindowDown,
        ],
    );
    assert_eq!(*layout.focus().unwrap().id(), 3);

    // With only the side-view window floating, switching focus to floating does nothing.
    check_ops_on_layout(&mut layout, [Op::CloseWindow(3), Op::FocusFloating]);
    assert_eq!(*layout.focus().unwrap().id(), 1);
}

#[test]
fn focus_moves_from_side_view_to_tiling() {
    let mut layout = layout_with(true, [add_window(1), Op::ToggleSideView { id: None }]);
    check_ops_on_layout(&mut layout, [add_window(2)]);

    // Clicking focuses the side-view window.
    layout.activate_window(&1);
    assert_eq!(*layout.focus().unwrap().id(), 1);

    check_ops_on_layout(&mut layout, [Op::FocusColumnLeft]);
    assert_eq!(*layout.focus().unwrap().id(), 2);
}

#[test]
fn fullscreen_leaves_side_view() {
    for i3 in [true, false] {
        let layout = layout_with(
            i3,
            [
                add_window(1),
                Op::ToggleSideView { id: None },
                Op::FullscreenWindow(1),
            ],
        );
        assert!(!is_side_view(&layout, 1));
        let tile = layout
            .active_workspace()
            .unwrap()
            .tiles()
            .find(|tile| *tile.window().id() == 1)
            .unwrap();
        assert!(tile.window().pending_sizing_mode().is_fullscreen());
    }
}

#[test]
fn toggle_floating_leaves_side_view() {
    let layout = layout_with(
        true,
        [
            add_window(1),
            Op::ToggleSideView { id: None },
            Op::ToggleWindowFloating { id: Some(1) },
        ],
    );
    assert!(!is_side_view(&layout, 1));
}

#[test]
fn opacity_applies_only_in_side_view() {
    let layout = layout_with(true, [add_window(1), Op::ToggleSideView { id: None }]);
    let ws = layout.active_workspace().unwrap();
    let tile = ws.tiles().next().unwrap();
    assert!(tile.is_side_view());
    assert_eq!(layout.options.layout.side_view.opacity, 0.9);
}
