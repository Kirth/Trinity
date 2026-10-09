use std::collections::HashMap;

use niri_ipc::SizeChange;
use smithay::utils::{Logical, Point};

use super::*;
use crate::layout::tiling::{ContainerLayout, LayoutCommand, SplitCommand};

fn i3_options() -> Options {
    Options {
        layout: niri_config::Layout {
            layout_mode: LayoutMode::I3,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn monitors(layout: &Layout<TestWindow>) -> &[Monitor<TestWindow>] {
    let MonitorSet::Normal { monitors, .. } = &layout.monitor_set else {
        unreachable!()
    };
    monitors
}

fn numbers(layout: &Layout<TestWindow>, mon_idx: usize) -> Vec<Option<u8>> {
    monitors(layout)[mon_idx]
        .workspaces
        .iter()
        .map(|ws| ws.number)
        .collect()
}

fn active_number(layout: &Layout<TestWindow>) -> Option<u8> {
    layout.active_workspace().unwrap().number
}

const REFRESH: Op = Op::Refresh { is_active: true };

#[test]
fn active_workspace_gets_number_on_refresh() {
    let ops = [Op::AddOutput(1), REFRESH];
    let layout = check_ops_with_options(i3_options(), ops);
    assert_eq!(numbers(&layout, 0), [Some(1)]);
}

#[test]
fn focus_workspace_creates_it_instantly() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        REFRESH,
        Op::FocusI3Workspace(7),
    ];
    let layout = check_ops_with_options(i3_options(), ops);

    // No switch animation in i3 mode.
    assert!(monitors(&layout)[0].workspace_switch.is_none());
    assert_eq!(active_number(&layout), Some(7));
    assert_eq!(numbers(&layout, 0), [Some(1), Some(7), None]);
}

#[test]
fn leaving_empty_workspace_removes_it() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        REFRESH,
        Op::FocusI3Workspace(5),
        Op::FocusI3Workspace(1),
    ];
    let layout = check_ops_with_options(i3_options(), ops);
    assert_eq!(numbers(&layout, 0), [Some(1), None]);
}

#[test]
fn workspaces_stay_sorted() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        REFRESH,
        Op::FocusI3Workspace(7),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusI3Workspace(3),
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        REFRESH,
    ];
    let layout = check_ops_with_options(i3_options(), ops);
    assert_eq!(numbers(&layout, 0), [Some(1), Some(3), Some(7), None]);
    assert_eq!(active_number(&layout), Some(3));
}

#[test]
fn up_down_go_to_neighbours_and_wrap() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        REFRESH,
        Op::FocusI3Workspace(4),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusI3Workspace(9),
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
    ];
    let mut layout = check_ops_with_options(i3_options(), ops);

    // Down from the last workspace wraps around and skips the trailing empty one.
    check_ops_on_layout(&mut layout, [Op::FocusWorkspaceDown]);
    assert_eq!(active_number(&layout), Some(1));

    check_ops_on_layout(&mut layout, [Op::FocusWorkspaceUp]);
    assert_eq!(active_number(&layout), Some(9));

    check_ops_on_layout(&mut layout, [Op::FocusWorkspaceUp]);
    assert_eq!(active_number(&layout), Some(4));
}

#[test]
fn numbers_are_global_across_outputs() {
    let ops = [
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::FocusOutput(2),
        REFRESH,
    ];
    let mut layout = check_ops_with_options(i3_options(), ops);
    assert_eq!(numbers(&layout, 0), [Some(1)]);
    assert_eq!(numbers(&layout, 1), [Some(2)]);

    // Focusing a number that lives on another output focuses that output.
    check_ops_on_layout(&mut layout, [Op::FocusI3Workspace(1)]);
    assert_eq!(layout.active_output().unwrap().name(), "output1");
    assert_eq!(active_number(&layout), Some(1));
}

#[test]
fn move_column_to_new_workspace() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        REFRESH,
        Op::MoveColumnToI3Workspace(4, false),
    ];
    let layout = check_ops_with_options(i3_options(), ops);
    assert_eq!(numbers(&layout, 0), [Some(1), Some(4), None]);
    assert!(monitors(&layout)[0].workspaces[1].has_window(&2));
    assert_eq!(active_number(&layout), Some(1));
}

#[test]
fn moving_workspaces_by_hand_does_nothing() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        REFRESH,
        Op::FocusI3Workspace(2),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        REFRESH,
        Op::MoveWorkspaceUp,
    ];
    let layout = check_ops_with_options(i3_options(), ops);
    assert_eq!(numbers(&layout, 0), [Some(1), Some(2), None]);
}

#[test]
fn i3_mode_ignores_empty_workspace_above_first() {
    let mut options = i3_options();
    options.layout.empty_workspace_above_first = true;
    options.layout.positional_workspaces = true;

    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        REFRESH,
    ];
    let layout = check_ops_with_options(options, ops);
    assert_eq!(numbers(&layout, 0), [Some(1), None]);
}

fn add_window(id: usize) -> Op {
    Op::AddWindow {
        params: TestWindowParams::new(id),
    }
}

/// Position and visibility of each window on the active workspace.
fn positions(layout: &Layout<TestWindow>) -> HashMap<usize, (Point<f64, Logical>, bool)> {
    layout
        .active_workspace()
        .unwrap()
        .tiles_with_render_positions()
        .map(|(tile, pos, visible)| (*tile.window().id(), (pos, visible)))
        .collect()
}

fn focused(layout: &Layout<TestWindow>) -> usize {
    *layout.focus().unwrap().id()
}

fn tiling_layout(ops: impl IntoIterator<Item = Op>) -> Layout<TestWindow> {
    let mut all = vec![Op::AddOutput(1)];
    all.extend(ops);
    all.push(Op::CompleteAnimations);
    check_ops_with_options(i3_options(), all)
}

#[test]
fn new_windows_split_horizontally() {
    let layout = tiling_layout([add_window(1), add_window(2), add_window(3)]);
    let pos = positions(&layout);
    assert!(pos[&1].0.x < pos[&2].0.x);
    assert!(pos[&2].0.x < pos[&3].0.x);
    assert_eq!(pos[&1].0.y, pos[&3].0.y);
    assert_eq!(focused(&layout), 3);
}

#[test]
fn split_vertical_then_open() {
    let layout = tiling_layout([
        add_window(1),
        add_window(2),
        Op::Split(SplitCommand::Vertical),
        add_window(3),
    ]);
    let pos = positions(&layout);
    // 3 opens below 2, and both are to the right of 1.
    assert_eq!(pos[&2].0.x, pos[&3].0.x);
    assert!(pos[&2].0.y < pos[&3].0.y);
    assert!(pos[&1].0.x < pos[&2].0.x);
}

#[test]
fn directional_focus() {
    let mut layout = tiling_layout([
        add_window(1),
        add_window(2),
        Op::Split(SplitCommand::Vertical),
        add_window(3),
    ]);

    check_ops_on_layout(&mut layout, [Op::FocusWindowUp]);
    assert_eq!(focused(&layout), 2);

    check_ops_on_layout(&mut layout, [Op::FocusColumnLeft]);
    assert_eq!(focused(&layout), 1);

    // Going back right descends into the most recently focused window.
    check_ops_on_layout(&mut layout, [Op::FocusColumnRight]);
    assert_eq!(focused(&layout), 2);

    // Nothing further right.
    check_ops_on_layout(&mut layout, [Op::FocusColumnRight]);
    assert_eq!(focused(&layout), 2);
}

#[test]
fn move_swaps_with_neighbour() {
    let layout = tiling_layout([add_window(1), add_window(2), Op::MoveColumnLeft]);
    let pos = positions(&layout);
    assert!(pos[&2].0.x < pos[&1].0.x);
    assert_eq!(focused(&layout), 2);
}

#[test]
fn move_out_of_container() {
    // [1, splitv[2, 3]], then move 3 left: it leaves the vertical container.
    let layout = tiling_layout([
        add_window(1),
        add_window(2),
        Op::Split(SplitCommand::Vertical),
        add_window(3),
        Op::MoveColumnLeft,
    ]);
    let pos = positions(&layout);
    assert!(pos[&1].0.x < pos[&3].0.x);
    assert!(pos[&3].0.x < pos[&2].0.x);
    assert_eq!(pos[&1].0.y, pos[&3].0.y);
    assert_eq!(pos[&3].0.y, pos[&2].0.y);
}

#[test]
fn move_across_orientation_wraps_root() {
    // [1, 2] horizontally, move 2 down: the workspace becomes vertical with 2 below.
    let layout = tiling_layout([add_window(1), add_window(2), Op::MoveWindowDown]);
    let pos = positions(&layout);
    assert!(pos[&1].0.y < pos[&2].0.y);
    assert_eq!(pos[&1].0.x, pos[&2].0.x);
}

#[test]
fn move_at_edge_does_nothing() {
    let layout = tiling_layout([add_window(1), add_window(2), Op::MoveColumnRight]);
    let pos = positions(&layout);
    assert!(pos[&1].0.x < pos[&2].0.x);
}

#[test]
fn tabbed_shows_only_focused() {
    let mut layout = tiling_layout([
        add_window(1),
        add_window(2),
        add_window(3),
        Op::SetContainerLayout(LayoutCommand::Set(ContainerLayout::Tabbed)),
    ]);
    let pos = positions(&layout);
    assert!(!pos[&1].1);
    assert!(!pos[&2].1);
    assert!(pos[&3].1);

    // Left and right switch tabs.
    check_ops_on_layout(&mut layout, [Op::FocusColumnLeft]);
    let pos = positions(&layout);
    assert_eq!(focused(&layout), 2);
    assert!(pos[&2].1);
    assert!(!pos[&3].1);
}

#[test]
fn focus_parent_moves_container() {
    // [1, splitv[2, 3]]: focus parent on 3 selects the vertical container, moving it left swaps
    // it with 1.
    let layout = tiling_layout([
        add_window(1),
        add_window(2),
        Op::Split(SplitCommand::Vertical),
        add_window(3),
        Op::FocusParent,
        Op::MoveColumnLeft,
    ]);
    let pos = positions(&layout);
    assert!(pos[&2].0.x < pos[&1].0.x);
    assert_eq!(pos[&2].0.x, pos[&3].0.x);
}

#[test]
fn closing_windows_removes_empty_containers() {
    let layout = tiling_layout([
        add_window(1),
        add_window(2),
        Op::Split(SplitCommand::Vertical),
        add_window(3),
        Op::CloseWindow(2),
        Op::CloseWindow(3),
        add_window(4),
    ]);
    let pos = positions(&layout);
    assert!(pos[&1].0.x < pos[&4].0.x);
    assert_eq!(pos[&1].0.y, pos[&4].0.y);
}

#[test]
fn fullscreen_covers_output() {
    let layout = tiling_layout([add_window(1), add_window(2), Op::FullscreenWindow(1)]);
    let pos = positions(&layout);
    assert_eq!(pos[&1].0, Point::from((0., 0.)));
}

#[test]
fn resize_changes_share() {
    let layout = tiling_layout([
        add_window(1),
        add_window(2),
        Op::SetColumnWidth(SizeChange::SetProportion(75.)),
    ]);
    let pos = positions(&layout);
    let ws = layout.active_workspace().unwrap();
    let view_w = ws.view_size().w;
    // Window 2 takes about three quarters, so it starts in the left half.
    assert!(pos[&2].0.x < view_w / 2.);
}

#[test]
fn switching_mode_keeps_windows() {
    let mut layout = tiling_layout([add_window(1), add_window(2), add_window(3)]);

    let scrolling = niri_config::LayoutPart {
        layout_mode: Some(LayoutMode::Scrolling),
        ..Default::default()
    };
    check_ops_on_layout(
        &mut layout,
        [Op::UpdateConfig {
            layout_config: Box::new(scrolling),
        }],
    );
    let ws = layout.active_workspace().unwrap();
    assert!(!ws.is_i3());
    assert_eq!(ws.windows().count(), 3);
    assert_eq!(focused(&layout), 3);
}

fn drag(window: usize, x: f64, y: f64) -> [Op; 3] {
    [
        Op::InteractiveMoveBegin {
            window,
            output_idx: 1,
            px: 0.,
            py: 0.,
        },
        Op::InteractiveMoveUpdate {
            window,
            dx: 1000.,
            dy: 1000.,
            output_idx: 1,
            px: x,
            py: y,
        },
        Op::InteractiveMoveEnd { window },
    ]
}

#[test]
fn drop_on_side_of_window() {
    // The output is 1280 × 720. Dropping 1 on the right half of 3 puts it after 3.
    let mut ops = vec![add_window(1), add_window(2), add_window(3)];
    ops.extend(drag(1, 1200., 360.));
    let layout = tiling_layout(ops);
    let pos = positions(&layout);
    assert!(pos[&2].0.x < pos[&3].0.x);
    assert!(pos[&3].0.x < pos[&1].0.x);
    assert_eq!(focused(&layout), 1);
}

#[test]
fn drop_below_window_splits_it() {
    // Dropping 1 near the bottom edge of 2 stacks it below 2.
    let mut ops = vec![add_window(1), add_window(2)];
    ops.extend(drag(1, 950., 690.));
    let layout = tiling_layout(ops);
    let pos = positions(&layout);
    assert_eq!(pos[&1].0.x, pos[&2].0.x);
    assert!(pos[&2].0.y < pos[&1].0.y);
}

#[test]
fn interactive_resize_moves_boundary() {
    let before = tiling_layout([add_window(1), add_window(2)]);
    let x_before = positions(&before)[&2].0.x;

    let layout = tiling_layout([
        add_window(1),
        add_window(2),
        Op::InteractiveResizeBegin {
            window: 1,
            edges: ResizeEdge::RIGHT,
        },
        Op::InteractiveResizeUpdate {
            window: 1,
            dx: 200.,
            dy: 0.,
        },
        Op::InteractiveResizeEnd { window: 1 },
    ]);
    let x_after = positions(&layout)[&2].0.x;
    assert!(
        (x_after - x_before - 200.).abs() < 2.,
        "{x_before} -> {x_after}"
    );
}

#[test]
fn switching_to_i3_keeps_columns_together() {
    // In the scrolling layout: a column with 1 and 2, and 3 on its own.
    let ops = [
        Op::AddOutput(1),
        add_window(1),
        add_window(2),
        Op::ConsumeOrExpelWindowLeft { id: None },
        add_window(3),
        Op::UpdateConfig {
            layout_config: Box::new(niri_config::LayoutPart {
                layout_mode: Some(LayoutMode::I3),
                ..Default::default()
            }),
        },
        Op::CompleteAnimations,
    ];
    let layout = check_ops(ops);
    assert!(layout.active_workspace().unwrap().is_i3());

    let pos = positions(&layout);
    assert_eq!(pos[&1].0.x, pos[&2].0.x);
    assert!(pos[&1].0.y < pos[&2].0.y);
    assert!(pos[&1].0.x < pos[&3].0.x);
    assert_eq!(focused(&layout), 3);
}

fn debug_outline_edges(layout: &mut Layout<TestWindow>) -> usize {
    layout.update_render_elements(None);
    layout
        .active_workspace()
        .unwrap()
        .tiling()
        .unwrap()
        .debug_outline_edges()
}

#[test]
fn debug_outlines_show_containers() {
    let ops = [
        add_window(1),
        add_window(2),
        Op::Split(SplitCommand::Vertical),
        add_window(3),
    ];

    // Off by default.
    let mut layout = tiling_layout(ops.clone());
    assert_eq!(debug_outline_edges(&mut layout), 0);

    // The root and the vertical container each get four edges and a divider between their
    // two children.
    let mut options = i3_options();
    options.debug_containers = true;
    let mut all = vec![Op::AddOutput(1)];
    all.extend(ops);
    let mut layout = check_ops_with_options(options, all);
    assert_eq!(debug_outline_edges(&mut layout), 10);
    assert_eq!(
        debug_labels(&layout),
        [ContainerLayout::SplitH, ContainerLayout::SplitV]
    );
}

fn debug_labels(layout: &Layout<TestWindow>) -> Vec<ContainerLayout> {
    layout
        .active_workspace()
        .unwrap()
        .tiling()
        .unwrap()
        .debug_labels()
}

#[test]
fn debug_dividers_between_children() {
    let mut options = i3_options();
    options.debug_containers = true;
    let ops = [
        Op::AddOutput(1),
        add_window(1),
        add_window(2),
        add_window(3),
    ];
    let mut layout = check_ops_with_options(options, ops);
    // Four edges and two dividers.
    assert_eq!(debug_outline_edges(&mut layout), 6);
    assert_eq!(debug_labels(&layout), [ContainerLayout::SplitH]);

    // Tabbed containers have no dividers.
    check_ops_on_layout(
        &mut layout,
        [Op::SetContainerLayout(LayoutCommand::Set(
            ContainerLayout::Tabbed,
        ))],
    );
    assert_eq!(debug_outline_edges(&mut layout), 4);
    assert_eq!(debug_labels(&layout), [ContainerLayout::Tabbed]);
}
