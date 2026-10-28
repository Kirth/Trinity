use std::collections::HashMap;

use niri_ipc::SizeChange;
use smithay::utils::{Logical, Point, Size};

use super::*;
use crate::layout::tiling::{ContainerLayout, LayoutCommand, SplitCommand};
use crate::layout::workspace::STRIP_WIDTH;

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

fn add_window_with_min(id: usize, w: i32, h: i32) -> Op {
    let mut params = TestWindowParams::new(id);
    params.min_max_size = (Size::from((w, h)), Size::default());
    Op::AddWindow { params }
}

#[test]
fn min_width_takes_space_from_siblings() {
    let layout = tiling_layout([add_window_with_min(1, 900, 0), add_window(2), add_window(3)]);
    let pos = positions(&layout);
    // Window 1 gets its min width, and the other two share the rest.
    assert!(pos[&2].0.x >= pos[&1].0.x + 900., "{pos:?}");
    assert!(pos[&3].0.x > pos[&2].0.x, "{pos:?}");
}

#[test]
fn min_width_inside_nested_container() {
    let layout = tiling_layout([
        add_window(1),
        add_window(2),
        Op::Split(SplitCommand::Vertical),
        add_window_with_min(3, 900, 0),
    ]);
    let pos = positions(&layout);
    // 2 and 3 are stacked in the right column, which is as wide as 3 needs.
    assert_eq!(pos[&2].0.x, pos[&3].0.x);
    let ws = layout.active_workspace().unwrap();
    assert!(ws.view_size().w - pos[&3].0.x >= 900., "{pos:?}");
}

#[test]
fn mins_that_dont_fit_fall_back_to_shares() {
    let layout = tiling_layout([
        add_window_with_min(1, 900, 0),
        add_window_with_min(2, 900, 0),
    ]);
    let pos = positions(&layout);
    let ws = layout.active_workspace().unwrap();
    let half = ws.view_size().w / 2.;
    assert!((pos[&2].0.x - half).abs() < 16., "{pos:?}");
}

#[test]
fn window_larger_than_slot_is_clipped_for_input() {
    let mut layout = tiling_layout([
        add_window_with_min(1, 900, 0),
        add_window_with_min(2, 900, 0),
        // Window 1 refuses its slot and stays 900 wide, over window 2's slot.
        Op::SetForcedSize {
            id: 1,
            size: Some(Size::from((900, 200))),
        },
        Op::Communicate(1),
    ]);
    let pos = positions(&layout);
    let x = pos[&2].0.x + 10.;
    let output = layout.active_output().unwrap().clone();
    layout.refresh(true);

    // The part of window 1 over window 2's slot is clipped: clicks there go to window 2.
    let (win, _) = layout
        .window_under(&output, Point::from((x, 100.)))
        .unwrap();
    assert_eq!(*win.id(), 2);
}

#[test]
fn resize_grows_window_at_its_min() {
    let before = tiling_layout([add_window_with_min(1, 900, 0), add_window(2), add_window(3)]);
    let x_before = positions(&before)[&2].0.x;

    let layout = tiling_layout([
        add_window_with_min(1, 900, 0),
        add_window(2),
        add_window(3),
        Op::FocusColumnLeft,
        Op::FocusColumnLeft,
        Op::SetColumnWidth(SizeChange::AdjustProportion(5.)),
    ]);
    assert_eq!(focused(&layout), 1);
    let x_after = positions(&layout)[&2].0.x;
    assert!(x_after > x_before + 30., "{x_before} -> {x_after}");
}

/// Three workspaces with a window each, plus the trailing empty one.
fn three_workspaces() -> Vec<Op> {
    vec![
        Op::AddOutput(1),
        add_window(1),
        REFRESH,
        Op::FocusI3Workspace(2),
        add_window(2),
        REFRESH,
        Op::FocusI3Workspace(3),
        add_window(3),
        REFRESH,
    ]
}

fn workspace_geos(layout: &Layout<TestWindow>) -> Vec<Rectangle<f64, Logical>> {
    monitors(layout)[0]
        .workspaces_with_render_geo_cull(false)
        .map(|(_, geo)| geo)
        .collect()
}

#[test]
fn overview_lays_out_workspaces_in_a_grid() {
    let mut ops = three_workspaces();
    ops.extend([Op::ToggleOverview, Op::CompleteAnimations]);
    let layout = check_ops_with_options(i3_options(), ops);

    let geos = workspace_geos(&layout);
    assert_eq!(geos.len(), 4);

    // 2×2 on a 1280×720 output.
    assert_eq!(geos[0].loc.y, geos[1].loc.y);
    assert_eq!(geos[2].loc.y, geos[3].loc.y);
    assert_eq!(geos[0].loc.x, geos[2].loc.x);
    assert_eq!(geos[1].loc.x, geos[3].loc.x);
    assert!(geos[0].loc.x < geos[1].loc.x);
    assert!(geos[0].loc.y < geos[2].loc.y);

    let output = Rectangle::from_size(Size::from((1280., 720.)));
    for (i, geo) in geos.iter().enumerate() {
        assert!(output.contains_rect(*geo), "{geo:?} must be on the output");
        for other in &geos[i + 1..] {
            assert!(!geo.overlaps(*other), "{geo:?} overlaps {other:?}");
        }
    }

    // The grid is centered.
    let left = geos[0].loc.x;
    let right = 1280. - (geos[1].loc.x + geos[1].size.w);
    assert!((left - right).abs() <= 1., "{left} vs {right}");
}

#[test]
fn overview_grid_single_workspace_uses_configured_zoom() {
    let ops = [
        Op::AddOutput(1),
        REFRESH,
        Op::ToggleOverview,
        Op::CompleteAnimations,
    ];
    let layout = check_ops_with_options(i3_options(), ops);

    let mon = &monitors(&layout)[0];
    let zoom = layout.options.overview.zoom;
    assert_eq!(mon.overview_zoom(), zoom);

    let geos = workspace_geos(&layout);
    assert_eq!(geos.len(), 1);
    let center = geos[0].loc + geos[0].size.downscale(2.).to_point();
    assert!((center.x - 640.).abs() <= 1. && (center.y - 360.).abs() <= 1.);
}

#[test]
fn closed_overview_grid_shows_active_workspace_fullscreen() {
    let mut ops = three_workspaces();
    ops.extend([Op::FocusI3Workspace(2), Op::CompleteAnimations]);
    let layout = check_ops_with_options(i3_options(), ops);

    let mon = &monitors(&layout)[0];
    let mut visible = mon.workspaces_with_render_geo();
    let (ws, geo) = visible.next().unwrap();
    assert!(visible.next().is_none());
    assert_eq!(ws.number, Some(2));
    assert_eq!(geo, Rectangle::from_size(Size::from((1280., 720.))));
}

#[test]
fn overview_grid_hit_testing() {
    let mut ops = three_workspaces();
    ops.extend([Op::ToggleOverview, Op::CompleteAnimations]);
    let layout = check_ops_with_options(i3_options(), ops);

    let mon = &monitors(&layout)[0];
    let geos = workspace_geos(&layout);

    // Inside the second column of the first row.
    let pos = geos[1].loc + geos[1].size.downscale(2.).to_point();
    let (ws, _) = mon.workspace_under(pos).unwrap();
    assert_eq!(ws.number, Some(2));

    // In the gap between the first and the second row, closer to the first.
    let gap_top = geos[0].loc.y + geos[0].size.h;
    let pos = Point::from((geos[0].loc.x + 10., gap_top + 1.));
    let (insert_ws, _) = mon.insert_position(pos);
    assert_eq!(insert_ws, InsertWorkspace::Existing(mon.workspaces[0].id()));

    // Far outside the grid goes to the nearest cell.
    let (insert_ws, _) = mon.insert_position(Point::from((1279., 719.)));
    assert_eq!(insert_ws, InsertWorkspace::Existing(mon.workspaces[3].id()));
}

#[test]
fn overview_grid_leaves_room_for_limbo() {
    let mut ops = three_workspaces();
    ops.extend([
        add_window(4),
        Op::MoveToLimbo(4),
        REFRESH,
        Op::ToggleOverview,
        Op::CompleteAnimations,
    ]);
    let layout = check_ops_with_options(i3_options(), ops);

    let mon = &monitors(&layout)[0];
    let band = mon.overview_limbo_band().unwrap();
    assert_eq!(band.loc.y + band.size.h, 720.);
    for geo in workspace_geos(&layout) {
        assert!(
            geo.loc.y + geo.size.h <= band.loc.y,
            "{geo:?} must be above the limbo band {band:?}"
        );
    }
}

#[test]
fn overview_grid_has_no_limbo_band_without_limbo() {
    let mut ops = three_workspaces();
    ops.extend([Op::ToggleOverview, Op::CompleteAnimations]);
    let layout = check_ops_with_options(i3_options(), ops);
    assert!(monitors(&layout)[0].overview_limbo_band().is_none());
}

/// Right edge of the rightmost window on the active workspace.
fn right_edge(layout: &Layout<TestWindow>) -> f64 {
    layout
        .active_workspace()
        .unwrap()
        .tiles_with_render_positions()
        .map(|(tile, pos, _)| pos.x + tile.tile_expected_or_current_size().w)
        .fold(f64::MIN, f64::max)
}

fn strip(layout: &Layout<TestWindow>) -> Vec<usize> {
    let ws = layout.active_workspace().unwrap();
    ws.strip_windows().map(|win| *win.id()).collect()
}

#[test]
fn stashing_makes_room_for_the_strip() {
    let before = right_edge(&tiling_layout([
        add_window(1),
        add_window(2),
        add_window(3),
    ]));
    let layout = tiling_layout([add_window(1), add_window(2), add_window(3), Op::Stash(3)]);
    assert_eq!(strip(&layout), [3]);
    assert!(!positions(&layout).contains_key(&3));
    assert!(right_edge(&layout) <= before - STRIP_WIDTH + 1.);

    // Without windows in the strip, the layout gets the room back.
    let layout = tiling_layout([
        add_window(1),
        add_window(2),
        add_window(3),
        Op::Stash(3),
        Op::Unstash {
            id: 3,
            to_place: true,
        },
    ]);
    assert!(strip(&layout).is_empty());
    assert_eq!(right_edge(&layout), before);
}

#[test]
fn last_window_stays_out_of_the_strip() {
    let layout = tiling_layout([add_window(1), add_window(2), Op::Stash(1), Op::Stash(2)]);
    assert_eq!(strip(&layout), [1]);
    assert_eq!(focused(&layout), 2);
}

#[test]
fn unstash_puts_window_back_in_place() {
    let ops = [
        add_window(1),
        add_window(2),
        Op::Split(SplitCommand::Vertical),
        add_window(3),
    ];
    let before = positions(&tiling_layout(ops.clone()));
    let mut all = ops.to_vec();
    all.extend([
        Op::Stash(2),
        Op::Unstash {
            id: 2,
            to_place: true,
        },
    ]);
    let after = positions(&tiling_layout(all));
    assert_eq!(before, after);
}

#[test]
fn swap_takes_the_focused_windows_place() {
    let ops = [
        add_window(1),
        add_window(2),
        add_window(3),
        Op::Stash(3),
        Op::FocusWindow(2),
    ];
    let before = positions(&tiling_layout(ops.clone()));
    let mut all = ops.to_vec();
    // Swapping 3 back in puts it where the focused 2 was.
    all.push(Op::SwapWithStrip(3));
    let layout = tiling_layout(all);
    assert_eq!(strip(&layout), [2]);
    assert_eq!(focused(&layout), 3);
    let after = positions(&layout);
    assert_eq!(after[&3].0.x, before[&2].0.x);
}

#[test]
fn activating_a_strip_window_swaps_it_in() {
    let layout = tiling_layout([
        add_window(1),
        add_window(2),
        Op::Stash(1),
        Op::FocusWindow(1),
    ]);
    assert_eq!(strip(&layout), [2]);
    assert_eq!(focused(&layout), 1);
}

#[test]
fn closing_the_last_laid_out_window_brings_back_the_strip() {
    let layout = tiling_layout([
        add_window(1),
        add_window(2),
        add_window(3),
        Op::Stash(1),
        Op::Stash(2),
        Op::CloseWindow(3),
    ]);
    assert_eq!(strip(&layout), [1]);
    assert_eq!(focused(&layout), 2);
}

#[test]
fn strip_is_per_workspace() {
    let layout = tiling_layout([
        add_window(1),
        add_window(2),
        Op::Stash(1),
        REFRESH,
        Op::FocusI3Workspace(2),
        add_window(3),
    ]);
    assert!(strip(&layout).is_empty());
    assert!(layout.is_in_strip(&1));
}

#[test]
fn closing_a_strip_window_removes_it() {
    let layout = tiling_layout([
        add_window(1),
        add_window(2),
        Op::Stash(1),
        Op::CloseWindow(1),
    ]);
    assert!(strip(&layout).is_empty());
    assert!(!layout.has_window(&1));
}

fn strip_op() -> impl Strategy<Value = Op> {
    let id = 1..=5usize;
    prop_oneof![
        3 => id.clone().prop_map(add_window),
        1 => id.clone().prop_map(Op::CloseWindow),
        3 => id.clone().prop_map(Op::Stash),
        1 => (id.clone(), any::<bool>()).prop_map(|(id, to_place)| Op::Unstash { id, to_place }),
        2 => id.clone().prop_map(Op::SwapWithStrip),
        1 => (id.clone(), 0..=5usize).prop_map(|(id, idx)| Op::MoveInStrip { id, idx }),
        2 => id.clone().prop_map(Op::FocusWindow),
        1 => id.clone().prop_map(Op::FullscreenWindow),
        1 => id.clone().prop_map(|id| Op::MaximizeWindowToEdges { id: Some(id) }),
        1 => id.clone().prop_map(|id| Op::ToggleWindowFloating { id: Some(id) }),
        1 => (proptest::option::of(id.clone()), 0..=4usize).prop_map(|(window_id, workspace_idx)| {
            Op::MoveWindowToWorkspace {
                window_id,
                workspace_idx,
            }
        }),
        1 => (1..=4u8).prop_map(Op::FocusI3Workspace),
        1 => Just(Op::Split(SplitCommand::Vertical)),
        1 => Just(Op::CloseWindow(1)),
        1 => Just(REFRESH),
        1 => (1..=2usize).prop_map(Op::AddOutput),
        1 => (1..=2usize).prop_map(Op::RemoveOutput),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: std::env::var("STRIP_CASES").ok().and_then(|n| n.parse().ok()).unwrap_or(64),
        ..ProptestConfig::default()
    })]

    #[test]
    fn random_strip_operations_dont_panic(ops in proptest::collection::vec(strip_op(), 1..40)) {
        let mut all = vec![Op::AddOutput(1)];
        all.extend(ops);
        check_ops_with_options(i3_options(), all);
    }
}
