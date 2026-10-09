//! i3-style tiling layout: a tree of containers with windows as leaves.
//!
//! Containers arrange their children side by side (split horizontal), stacked (split vertical),
//! or show one child at a time (tabbed and stacked, with a tab indicator). Focus, move, split and
//! layout commands follow i3's semantics.

use std::iter::zip;
use std::rc::Rc;

use niri_config::utils::MergeWith as _;
use niri_config::TabIndicatorPosition;
use niri_ipc::{SizeChange, WindowLayout};
use ordered_float::NotNan;
use smithay::backend::renderer::element::utils::CropRenderElement;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Point, Rectangle, Scale, Serial, Size};

use super::closing_window::{ClosingWindow, ClosingWindowRenderElement};
use super::scrolling::ColumnWidth;
use super::tab_indicator::{TabIndicator, TabIndicatorRenderElement, TabInfo};
use super::tile::{Tile, TileRenderElement, TileRenderSnapshot};
use super::{ConfigureIntent, HitType, LayoutElement, Options, RemovedTile, SizingMode};
use crate::animation::{Animation, Clock};
use crate::layout::RenderLayer;
use crate::niri_render_elements;
use crate::render_helpers::label::{render_pill, LabelCache};
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::RenderCtx;
use crate::utils::transaction::{Transaction, TransactionBlocker};
use crate::window::ResolvedWindowRules;

/// Smallest share of a split container that a child can be resized to.
const MIN_SHARE: f64 = 0.05;

const DEBUG_LABEL_FONT: &str = "sans bold 12px";
/// Radius of the debug layout label, in logical pixels.
const DEBUG_LABEL_RADIUS: f64 = 11.;

/// How a container arranges its children.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerLayout {
    /// Children side by side.
    SplitH,
    /// Children on top of each other.
    SplitV,
    /// One child visible at a time, with tabs along the top.
    Tabbed,
    /// One child visible at a time, with tabs along the side.
    Stacked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Argument of the `split` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitCommand {
    Horizontal,
    Vertical,
    Toggle,
}

/// Argument of the `layout` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutCommand {
    Set(ContainerLayout),
    /// Toggle between split horizontal and split vertical.
    ToggleSplit,
    /// Cycle through all layouts.
    ToggleAll,
}

impl ContainerLayout {
    /// Axis along which directional focus and move go through the children.
    fn orientation(self) -> Orientation {
        match self {
            ContainerLayout::SplitH | ContainerLayout::Tabbed => Orientation::Horizontal,
            ContainerLayout::SplitV | ContainerLayout::Stacked => Orientation::Vertical,
        }
    }

    fn is_split(self) -> bool {
        matches!(self, ContainerLayout::SplitH | ContainerLayout::SplitV)
    }

    fn split(orientation: Orientation) -> Self {
        match orientation {
            Orientation::Horizontal => ContainerLayout::SplitH,
            Orientation::Vertical => ContainerLayout::SplitV,
        }
    }
}

impl Orientation {
    fn flip(self) -> Self {
        match self {
            Orientation::Horizontal => Orientation::Vertical,
            Orientation::Vertical => Orientation::Horizontal,
        }
    }
}

impl Direction {
    fn orientation(self) -> Orientation {
        match self {
            Direction::Left | Direction::Right => Orientation::Horizontal,
            Direction::Up | Direction::Down => Orientation::Vertical,
        }
    }

    /// Whether this direction goes towards higher child indices.
    fn is_forward(self) -> bool {
        matches!(self, Direction::Right | Direction::Down)
    }
}

/// Space for i3-style tiled windows.
#[derive(Debug)]
pub struct TilingSpace<W: LayoutElement> {
    /// The workspace container. It has no children when the space is empty.
    root: Container<W>,

    /// How many levels above the focused window the focus is, after `focus parent`.
    focus_up: usize,

    /// Windows in the closing animation.
    closing_windows: Vec<ClosingWindow>,

    /// Ongoing interactive resize.
    interactive_resize: Option<TilingResize<W>>,

    /// View size for this space.
    view_size: Size<f64, Logical>,

    /// Working area for this space.
    working_area: Rectangle<f64, Logical>,

    /// Scale of the output the space is on (and rounds its sizes to).
    scale: f64,

    /// Clock for driving animations.
    clock: Clock,

    /// Configurable properties of the layout.
    options: Rc<Options>,

    /// Debug layout label textures by layout, focus and scale.
    debug_labels: LabelCache<(ContainerLayout, bool, NotNan<f64>)>,
}

niri_render_elements! {
    TilingSpaceRenderElement<R> => {
        Tile = TileRenderElement<R>,
        ClippedTile = CropRenderElement<TileRenderElement<R>>,
        ClosingWindow = ClosingWindowRenderElement,
        TabIndicator = TabIndicatorRenderElement,
        SolidColor = SolidColorRenderElement,
        Texture = PrimaryGpuTextureRenderElement,
    }
}

/// An interactive resize of a window by dragging its edges.
#[derive(Debug)]
struct TilingResize<W: LayoutElement> {
    window: W::Id,
    edges: crate::utils::ResizeEdge,
    /// The boundary being moved along each axis, if any.
    horizontal: Option<ResizeBoundary>,
    vertical: Option<ResizeBoundary>,
}

/// A boundary between two children of a split container.
#[derive(Debug, Clone)]
struct ResizeBoundary {
    /// Path to the split container.
    container: Vec<usize>,
    /// Number of children of the container when the resize started.
    count: usize,
    /// The child that contains the window, and its neighbour across the boundary.
    child: usize,
    neighbour: usize,
    /// Shares of the child and its neighbour when the resize started.
    start_shares: (f64, f64),
    /// Size of the container along the axis, minus gaps, when the resize started.
    available: f64,
}

#[derive(Debug)]
enum Node<W: LayoutElement> {
    // Boxed since tiles and tab indicators are large.
    Leaf(Box<Leaf<W>>),
    Container(Box<Container<W>>),
}

#[derive(Debug)]
struct Leaf<W: LayoutElement> {
    tile: Tile<W>,

    /// Tile position in the space, as last laid out.
    pos: Point<f64, Logical>,

    /// Tile size in the space, as last laid out.
    size: Size<f64, Logical>,

    /// Whether the tile has been laid out at least once.
    is_laid_out: bool,

    /// Whether the tile is shown, i.e. not a hidden tab.
    visible: bool,

    /// Requested sizing mode of the tile.
    sizing: SizingMode,

    /// Size and sizing mode last requested from the window.
    ///
    /// Only windows whose request changes take part in a resize transaction. A window that keeps
    /// its size doesn't send a configure, so it would hold the transaction until its deadline.
    requested: Option<(Size<f64, Logical>, SizingMode)>,
}

#[derive(Debug)]
struct Container<W: LayoutElement> {
    layout: ContainerLayout,

    children: Vec<Node<W>>,

    /// Size proportions of the children in split layouts. They sum up to 1.
    shares: Vec<f64>,

    /// Index of the most recently focused child.
    focused: usize,

    /// Area of the container in the space, as last laid out.
    rect: Rectangle<f64, Logical>,

    /// Tabs for the tabbed and stacked layouts.
    tab_indicator: TabIndicator,

    /// Edges of the debug outline and dividers between children, with their positions, when
    /// container debugging is on.
    debug_outline: Vec<(SolidColorBuffer, Point<f64, Logical>)>,

    /// Whether the debug layout label is shown, whether it's focused, and the top-centre point
    /// it hangs from.
    debug_label: Option<(bool, Point<f64, Logical>)>,
}

/// Colour of a container's debug outline, dividers and label.
fn debug_color(layout: ContainerLayout) -> [f32; 3] {
    match layout {
        ContainerLayout::SplitH => [0.3, 0.55, 1.],
        ContainerLayout::SplitV => [0.3, 0.9, 0.4],
        ContainerLayout::Tabbed => [1., 0.6, 0.2],
        ContainerLayout::Stacked => [0.75, 0.4, 1.],
    }
}

impl<W: LayoutElement> Leaf<W> {
    /// Whether the window is bigger than its slot, usually because it can't get as small as its
    /// share, and so has to be clipped to not cover its neighbours.
    ///
    /// Not while it's animating a resize, to keep the animation intact.
    fn is_clipped(&self) -> bool {
        if !self.sizing.is_normal() || self.tile.resize_animation().is_some() {
            return false;
        }
        let size = self.tile.tile_size();
        size.w > self.size.w + 0.5 || size.h > self.size.h + 0.5
    }
}

impl<W: LayoutElement> Node<W> {
    /// Area of the node in the space, as last laid out.
    fn rect(&self) -> Rectangle<f64, Logical> {
        match self {
            Node::Container(container) => container.rect,
            Node::Leaf(leaf) => Rectangle::new(leaf.pos, leaf.size),
        }
    }

    /// Smallest size the node can take without its windows growing past their slots.
    fn min_size(&self, gaps: f64, scale: f64) -> Size<f64, Logical> {
        match self {
            Node::Container(container) => container.min_size(gaps, scale),
            Node::Leaf(leaf) => leaf.tile.min_size_nonfullscreen(),
        }
    }

    fn as_container(&self) -> Option<&Container<W>> {
        match self {
            Node::Container(container) => Some(container),
            Node::Leaf(_) => None,
        }
    }

    fn as_container_mut(&mut self) -> Option<&mut Container<W>> {
        match self {
            Node::Container(container) => Some(container),
            Node::Leaf(_) => None,
        }
    }

    /// Returns the leaf that focus goes to when focusing this node.
    fn focused_leaf(&self) -> &Leaf<W> {
        match self {
            Node::Leaf(leaf) => leaf,
            Node::Container(container) => container.children[container.focused].focused_leaf(),
        }
    }

    fn leaves(&self) -> Box<dyn Iterator<Item = &Leaf<W>> + '_> {
        match self {
            Node::Leaf(leaf) => Box::new(std::iter::once(&**leaf)),
            Node::Container(container) => Box::new(container.leaves()),
        }
    }

    fn leaves_mut(&mut self) -> Box<dyn Iterator<Item = &mut Leaf<W>> + '_> {
        match self {
            Node::Leaf(leaf) => Box::new(std::iter::once(&mut **leaf)),
            Node::Container(container) => Box::new(container.leaves_mut()),
        }
    }
}

impl<W: LayoutElement> Container<W> {
    fn new(layout: ContainerLayout, options: &Options) -> Self {
        let mut rv = Self {
            layout,
            children: Vec::new(),
            shares: Vec::new(),
            focused: 0,
            rect: Rectangle::default(),
            tab_indicator: TabIndicator::new(options.layout.tab_indicator),
            debug_outline: Vec::new(),
            debug_label: None,
        };
        rv.update_tab_indicator_config(options);
        rv
    }

    /// Tabbed containers show tabs along the top and stacked ones along the side, regardless of
    /// the configured tab indicator position.
    fn update_tab_indicator_config(&mut self, options: &Options) {
        let mut config = options.layout.tab_indicator;
        config.position = match self.layout {
            ContainerLayout::Stacked => TabIndicatorPosition::Left,
            _ => TabIndicatorPosition::Top,
        };
        self.tab_indicator.update_config(config);
    }

    fn set_layout(&mut self, layout: ContainerLayout, options: &Options) {
        self.layout = layout;
        self.update_tab_indicator_config(options);
    }

    fn leaves(&self) -> impl Iterator<Item = &Leaf<W>> + '_ {
        self.children.iter().flat_map(|child| child.leaves())
    }

    fn leaves_mut(&mut self) -> impl Iterator<Item = &mut Leaf<W>> + '_ {
        self.children
            .iter_mut()
            .flat_map(|child| child.leaves_mut())
    }

    /// Returns the container at this path relative to this one.
    fn container_at(&self, path: &[usize]) -> &Container<W> {
        let mut container = self;
        for &idx in path {
            container = container.children[idx].as_container().unwrap();
        }
        container
    }

    fn container_at_mut(&mut self, path: &[usize]) -> &mut Container<W> {
        let mut container = self;
        for &idx in path {
            container = container.children[idx].as_container_mut().unwrap();
        }
        container
    }

    /// Returns the node at this non-empty path relative to this container.
    fn node_at(&self, path: &[usize]) -> &Node<W> {
        let (&last, parent) = path.split_last().unwrap();
        &self.container_at(parent).children[last]
    }

    fn node_at_mut(&mut self, path: &[usize]) -> &mut Node<W> {
        let (&last, parent) = path.split_last().unwrap();
        &mut self.container_at_mut(parent).children[last]
    }

    /// Inserts a child, giving it an equal share and shrinking the others proportionally.
    fn insert(&mut self, idx: usize, node: Node<W>) {
        let n = self.children.len() as f64;
        let share = 1. / (n + 1.);
        for s in &mut self.shares {
            *s *= n / (n + 1.);
        }
        self.children.insert(idx, node);
        self.shares.insert(idx, share);
        if self.children.len() > 1 && idx <= self.focused {
            self.focused += 1;
        }
    }

    /// Removes a child, giving its share to the remaining children proportionally.
    fn remove(&mut self, idx: usize) -> Node<W> {
        let node = self.children.remove(idx);
        self.shares.remove(idx);
        normalize(&mut self.shares);

        if idx < self.focused || (idx == self.focused && self.focused > 0) {
            self.focused -= 1;
        }

        node
    }

    /// Finds the path to the leaf with this window, relative to this container.
    fn find(&self, id: &W::Id) -> Option<Vec<usize>> {
        for (idx, child) in self.children.iter().enumerate() {
            match child {
                Node::Leaf(leaf) => {
                    if leaf.tile.window().id() == id {
                        return Some(vec![idx]);
                    }
                }
                Node::Container(container) => {
                    if let Some(mut path) = container.find(id) {
                        path.insert(0, idx);
                        return Some(path);
                    }
                }
            }
        }
        None
    }

    /// Path to the leaf that has focus, following the focus history.
    fn focused_leaf_path(&self) -> Vec<usize> {
        let mut path = Vec::new();
        let mut container = self;
        while !container.children.is_empty() {
            path.push(container.focused);
            match &container.children[container.focused] {
                Node::Leaf(_) => break,
                Node::Container(child) => container = child,
            }
        }
        path
    }

    /// Area for the children of tabbed and stacked containers, below or beside the tabs.
    fn tabbed_content_rect(&self, scale: f64) -> Rectangle<f64, Logical> {
        let count = self.children.len();
        let extra = self.tab_indicator.extra_size(count, scale);
        let offset = self.tab_indicator.content_offset(count, scale);
        let mut rect = self.rect;
        rect.loc += offset;
        rect.size -= extra;
        rect.size.w = f64::max(rect.size.w, 1.);
        rect.size.h = f64::max(rect.size.h, 1.);
        rect
    }

    /// Smallest size the container can take: its children's mins along the split axis add up,
    /// the largest one counts across it and in tabbed and stacked layouts.
    fn min_size(&self, gaps: f64, scale: f64) -> Size<f64, Logical> {
        let mins = self
            .children
            .iter()
            .map(|child| child.min_size(gaps, scale));
        let max = |a: Size<f64, Logical>, b: Size<f64, Logical>| {
            Size::from((f64::max(a.w, b.w), f64::max(a.h, b.h)))
        };
        let gaps = gaps * self.children.len().saturating_sub(1) as f64;

        match self.layout {
            ContainerLayout::SplitH => mins.fold(Size::from((gaps, 0.)), |acc, min| {
                Size::from((acc.w + min.w, f64::max(acc.h, min.h)))
            }),
            ContainerLayout::SplitV => mins.fold(Size::from((0., gaps)), |acc, min| {
                Size::from((f64::max(acc.w, min.w), acc.h + min.h))
            }),
            ContainerLayout::Tabbed | ContainerLayout::Stacked => {
                let extra = self.tab_indicator.extra_size(self.children.len(), scale);
                mins.fold(Size::default(), max) + extra
            }
        }
    }

    /// Lays out this container's children inside `self.rect`.
    fn layout_children(&mut self, ctx: &LayoutCtx, visible: bool) {
        let count = self.children.len();
        if count == 0 {
            return;
        }

        match self.layout {
            ContainerLayout::SplitH | ContainerLayout::SplitV => {
                let horizontal = self.layout == ContainerLayout::SplitH;
                let total = if horizontal {
                    self.rect.size.w
                } else {
                    self.rect.size.h
                };
                let available = f64::max(total - ctx.gaps * (count - 1) as f64, 0.);

                let mins: Vec<f64> = self
                    .children
                    .iter()
                    .map(|child| {
                        let min = child.min_size(ctx.gaps, ctx.scale);
                        if horizontal {
                            min.w
                        } else {
                            min.h
                        }
                    })
                    .collect();
                let lens = split_lengths(&self.shares, &mins, available);

                let mut offset = 0.;
                for (idx, child) in self.children.iter_mut().enumerate() {
                    // The last child takes the remaining space to avoid rounding drift.
                    let len = if idx == count - 1 {
                        f64::max(total - offset, 0.)
                    } else {
                        lens[idx].round()
                    };

                    let mut rect = self.rect;
                    if horizontal {
                        rect.loc.x += offset;
                        rect.size.w = len;
                    } else {
                        rect.loc.y += offset;
                        rect.size.h = len;
                    }
                    offset += len + ctx.gaps;

                    layout_node(child, rect, ctx, visible);
                }
            }
            ContainerLayout::Tabbed | ContainerLayout::Stacked => {
                let rect = self.tabbed_content_rect(ctx.scale);
                let focused = self.focused;
                for (idx, child) in self.children.iter_mut().enumerate() {
                    layout_node(child, rect, ctx, visible && idx == focused);
                }
            }
        }
    }

    #[cfg(test)]
    fn verify_invariants(&self, is_root: bool) {
        if !is_root {
            assert!(
                !self.children.is_empty(),
                "non-root containers can't be empty"
            );
        }
        assert_eq!(self.children.len(), self.shares.len());
        if !self.children.is_empty() {
            assert!(self.focused < self.children.len());
            let sum: f64 = self.shares.iter().sum();
            assert!(
                (sum - 1.).abs() < 1e-6,
                "shares must sum to 1: {:?}",
                self.shares
            );
            assert!(self.shares.iter().all(|s| *s > 0.));
        }

        for child in &self.children {
            if let Node::Container(container) = child {
                container.verify_invariants(false);
            }
        }
    }
}

/// Parameters shared during a layout pass.
struct LayoutCtx {
    gaps: f64,
    scale: f64,
    animate: bool,
    transaction: Transaction,
    view_size: Size<f64, Logical>,
    working_area: Rectangle<f64, Logical>,
}

fn layout_node<W: LayoutElement>(
    node: &mut Node<W>,
    rect: Rectangle<f64, Logical>,
    ctx: &LayoutCtx,
    visible: bool,
) {
    match node {
        Node::Container(container) => {
            container.rect = rect;
            container.layout_children(ctx, visible);
        }
        Node::Leaf(leaf) => {
            let (pos, size) = match leaf.sizing {
                SizingMode::Normal => (rect.loc, rect.size),
                SizingMode::Maximized => (ctx.working_area.loc, ctx.working_area.size),
                SizingMode::Fullscreen => (Point::from((0., 0.)), ctx.view_size),
            };

            // Round the position to physical pixels.
            let pos = pos
                .to_physical_precise_round(ctx.scale)
                .to_logical(ctx.scale);

            if leaf.is_laid_out && ctx.animate && leaf.pos != pos {
                leaf.tile.animate_move_from(leaf.pos - pos);
            }
            leaf.pos = pos;
            leaf.size = size;
            leaf.is_laid_out = true;
            leaf.visible = visible || !leaf.sizing.is_normal();

            // Hidden tabs and windows that keep their size don't participate in the transaction.
            let request = (size, leaf.sizing);
            let changed = leaf.requested != Some(request);
            leaf.requested = Some(request);
            let transaction = (visible && changed).then(|| ctx.transaction.clone());
            match leaf.sizing {
                SizingMode::Normal => leaf.tile.request_tile_size(size, ctx.animate, transaction),
                SizingMode::Maximized => {
                    leaf.tile.request_maximized(size, ctx.animate, transaction)
                }
                SizingMode::Fullscreen => leaf.tile.request_fullscreen(ctx.animate, transaction),
            }
        }
    }
}

/// Splits `available` between children by their shares, giving each at least its min.
///
/// Children whose share falls below their min get the min, and the others share the rest by
/// their shares. When the mins don't all fit, the plain shares are used: windows then get
/// clipped to their slots.
fn split_lengths(shares: &[f64], mins: &[f64], available: f64) -> Vec<f64> {
    let by_shares = || shares.iter().map(|share| available * share).collect();
    if mins.iter().sum::<f64>() > available {
        return by_shares();
    }

    let mut pinned = vec![false; shares.len()];
    loop {
        let rest = available
            - zip(mins, &pinned)
                .filter(|(_, pinned)| **pinned)
                .map(|(min, _)| min)
                .sum::<f64>();
        let rest_shares: f64 = zip(shares, &pinned)
            .filter(|(_, pinned)| !**pinned)
            .map(|(share, _)| share)
            .sum();

        let lens: Vec<f64> = (0..shares.len())
            .map(|i| {
                if pinned[i] {
                    mins[i]
                } else if rest_shares > 0. {
                    rest * shares[i] / rest_shares
                } else {
                    0.
                }
            })
            .collect();

        let mut changed = false;
        for i in 0..shares.len() {
            if !pinned[i] && lens[i] < mins[i] {
                pinned[i] = true;
                changed = true;
            }
        }
        if !changed {
            return lens;
        }
    }
}

fn normalize(shares: &mut [f64]) {
    let sum: f64 = shares.iter().sum();
    if sum > 0. {
        for s in shares {
            *s /= sum;
        }
    }
}

impl<W: LayoutElement> TilingSpace<W> {
    pub fn new(
        view_size: Size<f64, Logical>,
        working_area: Rectangle<f64, Logical>,
        scale: f64,
        clock: Clock,
        options: Rc<Options>,
    ) -> Self {
        Self {
            root: Container::new(ContainerLayout::SplitH, &options),
            focus_up: 0,
            closing_windows: Vec::new(),
            interactive_resize: None,
            view_size,
            working_area,
            scale,
            clock,
            options,
            debug_labels: LabelCache::new(),
        }
    }

    pub fn update_config(
        &mut self,
        view_size: Size<f64, Logical>,
        working_area: Rectangle<f64, Logical>,
        scale: f64,
        options: Rc<Options>,
    ) {
        for leaf in self.root.leaves_mut() {
            leaf.tile.update_config(view_size, scale, options.clone());
            // Borders or scale may have changed the window size for the same tile size.
            leaf.requested = None;
        }
        update_containers_config(&mut self.root, &options);

        self.view_size = view_size;
        self.working_area = working_area;
        self.scale = scale;
        self.options = options;

        self.update_layout(false, None);
    }

    pub fn update_shaders(&mut self) {
        for leaf in self.root.leaves_mut() {
            leaf.tile.update_shaders();
        }
        for_each_container_mut(&mut self.root, &mut |c| c.tab_indicator.update_shaders());
    }

    pub fn advance_animations(&mut self) {
        for leaf in self.root.leaves_mut() {
            leaf.tile.advance_animations();
        }
        for_each_container_mut(&mut self.root, &mut |c| {
            c.tab_indicator.advance_animations()
        });

        self.closing_windows.retain_mut(|closing| {
            closing.advance_animations();
            closing.are_animations_ongoing()
        });
    }

    pub fn are_animations_ongoing(&self) -> bool {
        let mut tabs = false;
        for_each_container(&self.root, &mut |c| {
            tabs |= c.tab_indicator.are_animations_ongoing()
        });
        tabs || self
            .root
            .leaves()
            .any(|leaf| leaf.tile.are_animations_ongoing())
            || !self.closing_windows.is_empty()
    }

    pub fn are_transitions_ongoing(&self) -> bool {
        self.root
            .leaves()
            .any(|leaf| leaf.tile.are_transitions_ongoing())
            || !self.closing_windows.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.root.children.is_empty()
    }

    pub fn has_window(&self, id: &W::Id) -> bool {
        self.root.find(id).is_some()
    }

    pub fn tiles(&self) -> impl Iterator<Item = &Tile<W>> + '_ {
        self.root.leaves().map(|leaf| &leaf.tile)
    }

    pub fn tiles_mut(&mut self) -> impl Iterator<Item = &mut Tile<W>> + '_ {
        self.root.leaves_mut().map(|leaf| &mut leaf.tile)
    }

    fn focused_leaf(&self) -> Option<&Leaf<W>> {
        if self.is_empty() {
            return None;
        }
        match self.root.node_at(&self.root.focused_leaf_path()) {
            Node::Leaf(leaf) => Some(leaf),
            Node::Container(_) => unreachable!(),
        }
    }

    fn focused_leaf_mut(&mut self) -> Option<&mut Leaf<W>> {
        if self.is_empty() {
            return None;
        }
        let path = self.root.focused_leaf_path();
        match self.root.node_at_mut(&path) {
            Node::Leaf(leaf) => Some(leaf),
            Node::Container(_) => unreachable!(),
        }
    }

    pub fn active_window(&self) -> Option<&W> {
        self.focused_leaf().map(|leaf| leaf.tile.window())
    }

    pub fn active_window_mut(&mut self) -> Option<&mut W> {
        self.focused_leaf_mut().map(|leaf| leaf.tile.window_mut())
    }

    pub fn active_tile_mut(&mut self) -> Option<&mut Tile<W>> {
        self.focused_leaf_mut().map(|leaf| &mut leaf.tile)
    }

    /// Path to the focused node, which is a container after `focus parent`.
    fn focused_path(&self) -> Option<Vec<usize>> {
        if self.is_empty() {
            return None;
        }
        let mut path = self.root.focused_leaf_path();
        let keep = path.len().saturating_sub(self.focus_up).max(1);
        path.truncate(keep);
        Some(path)
    }

    /// Focuses the node at this path, descending to its most recently focused leaf.
    fn focus_path(&mut self, path: &[usize]) {
        let mut container = &mut self.root;
        for &idx in path {
            container.focused = idx;
            match &mut container.children[idx] {
                Node::Container(child) => container = child,
                Node::Leaf(_) => break,
            }
        }
        self.focus_up = 0;
    }

    pub fn activate_window(&mut self, id: &W::Id) -> bool {
        let Some(path) = self.root.find(id) else {
            return false;
        };
        self.focus_path(&path);
        // A different tab may have become visible.
        self.update_layout(false, None);
        true
    }

    /// Recomputes the position and size of every tile.
    fn update_layout(&mut self, animate: bool, transaction: Option<Transaction>) {
        let gaps = self.options.layout.gaps;
        let mut rect = self.working_area;
        rect.loc += Point::from((gaps, gaps));
        rect.size.w = f64::max(rect.size.w - gaps * 2., 1.);
        rect.size.h = f64::max(rect.size.h - gaps * 2., 1.);

        let ctx = LayoutCtx {
            gaps,
            scale: self.scale,
            animate,
            transaction: transaction.unwrap_or_else(Transaction::new),
            view_size: self.view_size,
            working_area: self.working_area,
        };

        self.root.rect = rect;
        self.root.layout_children(&ctx, true);
    }

    /// The layout an empty workspace starts with: horizontal on landscape outputs.
    fn default_layout(&self) -> ContainerLayout {
        if self.view_size.w >= self.view_size.h {
            ContainerLayout::SplitH
        } else {
            ContainerLayout::SplitV
        }
    }

    fn new_leaf(&self, mut tile: Tile<W>) -> Leaf<W> {
        tile.update_config(self.view_size, self.scale, self.options.clone());
        let sizing = tile.window().pending_sizing_mode();
        Leaf {
            tile,
            pos: Point::default(),
            size: Size::default(),
            is_laid_out: false,
            requested: None,
            visible: true,
            sizing,
        }
    }

    /// Adds a tile next to the focused node, like i3 places new windows.
    pub fn add_tile(&mut self, tile: Tile<W>, activate: bool) {
        let leaf = self.new_leaf(tile);
        let path = match self.focused_path() {
            Some(path) => path,
            None => {
                let layout = self.default_layout();
                self.root.set_layout(layout, &self.options);
                vec![]
            }
        };
        self.insert_leaf_after(&path, leaf, activate);
    }

    /// Adds a column of tiles from the scrolling layout at the end: a single tile on its own, or
    /// several in a vertical or tabbed container.
    pub fn append_column(&mut self, tiles: Vec<Tile<W>>, tabbed: bool) {
        if tiles.is_empty() {
            return;
        }

        if self.is_empty() {
            let layout = self.default_layout();
            self.root.set_layout(layout, &self.options);
        }

        let node = if tiles.len() == 1 {
            Node::Leaf(Box::new(self.new_leaf(tiles.into_iter().next().unwrap())))
        } else {
            let layout = if tabbed {
                ContainerLayout::Tabbed
            } else {
                ContainerLayout::SplitV
            };
            let mut container = Container::new(layout, &self.options);
            for tile in tiles {
                let idx = container.children.len();
                container.insert(idx, Node::Leaf(Box::new(self.new_leaf(tile))));
            }
            Node::Container(Box::new(container))
        };

        let idx = self.root.children.len();
        self.root.insert(idx, node);
        self.update_layout(false, None);
    }

    /// Takes all tiles out, in tree order.
    pub fn into_tiles(self) -> Vec<Tile<W>> {
        fn collect<W: LayoutElement>(container: Container<W>, out: &mut Vec<Tile<W>>) {
            for child in container.children {
                match child {
                    Node::Leaf(leaf) => out.push(leaf.tile),
                    Node::Container(inner) => collect(*inner, out),
                }
            }
        }

        let mut tiles = Vec::new();
        collect(self.root, &mut tiles);
        tiles
    }

    /// Adds a tile right after the tile of another window.
    pub fn add_tile_next_to(&mut self, next_to: &W::Id, tile: Tile<W>, activate: bool) {
        let leaf = self.new_leaf(tile);
        let path = self.root.find(next_to).unwrap();
        self.insert_leaf_after(&path, leaf, activate);
    }

    /// Inserts a leaf after the node at `path`, or into the empty root if `path` is empty.
    fn insert_leaf_after(&mut self, path: &[usize], leaf: Leaf<W>, activate: bool) {
        let new_path = match path.split_last() {
            None => {
                self.root.insert(0, Node::Leaf(Box::new(leaf)));
                vec![0]
            }
            Some((&idx, parent_path)) => {
                let parent = self.root.container_at_mut(parent_path);
                parent.insert(idx + 1, Node::Leaf(Box::new(leaf)));
                let mut new_path = parent_path.to_vec();
                new_path.push(idx + 1);
                new_path
            }
        };

        if activate {
            self.focus_path(&new_path);
        }

        self.update_layout(true, None);
    }

    pub fn remove_tile(&mut self, id: &W::Id, transaction: Transaction) -> RemovedTile<W> {
        let path = self.root.find(id).unwrap();
        let node = self.remove_node(&path);
        let Node::Leaf(leaf) = node else {
            unreachable!()
        };

        // Removing a window changes the tree under any ongoing resize.
        self.interactive_resize = None;

        self.update_layout(true, Some(transaction));

        let tile = leaf.tile;
        let width = ColumnWidth::Fixed(tile.tile_expected_or_current_size().w);
        RemovedTile {
            tile,
            width,
            is_full_width: false,
            is_floating: false,
        }
    }

    /// Removes the node at `path` and any containers left empty by that.
    fn remove_node(&mut self, path: &[usize]) -> Node<W> {
        let (&idx, parent_path) = path.split_last().unwrap();
        let node = self.root.container_at_mut(parent_path).remove(idx);

        // Remove containers that became empty, but never the root.
        let mut parent_path = parent_path.to_vec();
        while let Some(&idx) = parent_path.last() {
            if !self.root.container_at(&parent_path).children.is_empty() {
                break;
            }
            parent_path.pop();
            self.root.container_at_mut(&parent_path).remove(idx);
        }

        if self.is_empty() {
            self.focus_up = 0;
        }

        node
    }

    /// Replaces split containers that have a single split container child with that child, like
    /// i3 does after moves.
    fn flatten(&mut self) {
        fn flatten_container<W: LayoutElement>(container: &mut Container<W>) {
            for child in &mut container.children {
                if let Node::Container(inner) = child {
                    flatten_container(inner);

                    if inner.children.len() == 1
                        && matches!(&inner.children[0], Node::Container(c) if c.layout.is_split())
                    {
                        let grandchild = inner.children.pop().unwrap();
                        *child = grandchild;
                    }
                }
            }
        }
        flatten_container(&mut self.root);
    }

    /// Moves focus in a direction, like i3's `focus <direction>`.
    pub fn focus(&mut self, direction: Direction) -> bool {
        let Some(mut path) = self.focused_path() else {
            return false;
        };

        while let Some(idx) = path.pop() {
            let parent = self.root.container_at(&path);
            if parent.layout.orientation() != direction.orientation() {
                continue;
            }

            let new_idx = if direction.is_forward() {
                idx + 1
            } else if idx > 0 {
                idx - 1
            } else {
                continue;
            };
            if new_idx >= parent.children.len() {
                continue;
            }

            path.push(new_idx);
            self.focus_path(&path);
            self.update_layout(false, None);
            return true;
        }

        false
    }

    /// Moves the focused node in a direction, like i3's `move <direction>`.
    pub fn move_(&mut self, direction: Direction) -> bool {
        let Some(path) = self.focused_path() else {
            return false;
        };
        let (&idx, parent_path) = path.split_last().unwrap();
        let parent = self.root.container_at(parent_path);
        let orientation = direction.orientation();

        // Within the parent: swap with a neighbouring window, or enter a neighbouring container.
        if parent.layout.orientation() == orientation {
            let neighbour = if direction.is_forward() {
                (idx + 1 < parent.children.len()).then_some(idx + 1)
            } else {
                idx.checked_sub(1)
            };

            if let Some(neighbour) = neighbour {
                let mut neighbour_path = parent_path.to_vec();
                neighbour_path.push(neighbour);

                match &parent.children[neighbour] {
                    Node::Leaf(_) => {
                        let parent = self.root.container_at_mut(parent_path);
                        parent.children.swap(idx, neighbour);
                        parent.focused = neighbour;
                    }
                    Node::Container(target) => {
                        // Enter the container next to its focused child, or at the near end
                        // if it's arranged along the same axis.
                        let insert_idx = if target.layout.orientation() == orientation {
                            if direction.is_forward() {
                                0
                            } else {
                                target.children.len()
                            }
                        } else {
                            target.focused + 1
                        };

                        let node = self.root.container_at_mut(parent_path).remove(idx);
                        // Removing shifted the neighbour if it came after us.
                        let neighbour = if neighbour > idx {
                            neighbour - 1
                        } else {
                            neighbour
                        };
                        let mut target_path = parent_path.to_vec();
                        target_path.push(neighbour);
                        self.root
                            .container_at_mut(&target_path)
                            .insert(insert_idx, node);
                        target_path.push(insert_idx);
                        self.focus_path(&target_path);
                    }
                }

                self.update_layout(true, None);
                return true;
            }
        }

        // At the edge of the parent: move out into the nearest ancestor along the same axis.
        let mut ancestor_path = parent_path.to_vec();
        let ancestor = loop {
            let Some(child_idx) = ancestor_path.pop() else {
                break None;
            };
            if self.root.container_at(&ancestor_path).layout.orientation() == orientation {
                break Some(child_idx);
            }
        };

        let node = match ancestor {
            Some(child_idx) => {
                // The ancestor is a strict ancestor of the node's parent, so removing the node
                // can only remove containers below the ancestor's child, which keeps child_idx
                // valid unless that child itself goes away.
                // That child goes away if every container from it down to the node's parent has
                // the node as its only descendant.
                let child_path_len = ancestor_path.len() + 1;
                let child_removed = (child_path_len..path.len())
                    .all(|len| self.root.container_at(&path[..len]).children.len() == 1);

                let node = self.remove_node(&path);
                let mut insert_idx = if direction.is_forward() {
                    child_idx + 1
                } else {
                    child_idx
                };
                if child_removed && direction.is_forward() {
                    insert_idx -= 1;
                }
                let ancestor = self.root.container_at_mut(&ancestor_path);
                ancestor.insert(insert_idx, node);
                ancestor_path.push(insert_idx);
                ancestor_path
            }
            None => {
                // Already at the edge of the root along this axis, or the only child of the root:
                // there's nowhere to go.
                if self.root.layout.orientation() == orientation
                    || (path.len() == 1 && self.root.children.len() == 1)
                {
                    return false;
                }

                // Wrap the root's children into a container with the root's old layout, then
                // make the root split along the move axis with the node beside the old content.
                let node = self.remove_node(&path);
                if !self.root.children.is_empty() {
                    let mut wrapper = Container::new(self.root.layout, &self.options);
                    wrapper.children = std::mem::take(&mut self.root.children);
                    wrapper.shares = std::mem::take(&mut self.root.shares);
                    wrapper.focused = self.root.focused;
                    self.root.focused = 0;
                    self.root.insert(0, Node::Container(Box::new(wrapper)));
                }
                self.root
                    .set_layout(ContainerLayout::split(orientation), &self.options);
                let insert_idx = if direction.is_forward() {
                    self.root.children.len()
                } else {
                    0
                };
                self.root.insert(insert_idx, node);
                vec![insert_idx]
            }
        };

        self.focus_path(&node);
        // Flattening keeps the focus history of the remaining containers.
        self.flatten();
        self.update_layout(true, None);
        true
    }

    /// Splits the focused node, like i3's `split`.
    pub fn split(&mut self, command: SplitCommand) {
        let Some(path) = self.focused_path() else {
            return;
        };
        let (&idx, parent_path) = path.split_last().unwrap();
        let parent = self.root.container_at_mut(parent_path);

        let orientation = match command {
            SplitCommand::Horizontal => Orientation::Horizontal,
            SplitCommand::Vertical => Orientation::Vertical,
            SplitCommand::Toggle => parent.layout.orientation().flip(),
        };
        let layout = ContainerLayout::split(orientation);

        // An only child doesn't need a new container: changing the parent's layout is enough.
        if parent.children.len() == 1 {
            parent.set_layout(layout, &self.options);
        } else {
            let mut wrapper = Container::new(layout, &self.options);
            let placeholder = Node::Container(Box::new(Container::new(layout, &self.options)));
            let node = std::mem::replace(&mut parent.children[idx], placeholder);
            wrapper.children.push(node);
            wrapper.shares.push(1.);
            parent.children[idx] = Node::Container(Box::new(wrapper));
        }

        self.update_layout(true, None);
    }

    /// Changes the layout of the focused node's parent, like i3's `layout`.
    pub fn set_layout(&mut self, command: LayoutCommand) {
        let Some(path) = self.focused_path() else {
            return;
        };
        let parent = self.root.container_at_mut(&path[..path.len() - 1]);

        let layout = match command {
            LayoutCommand::Set(layout) => layout,
            LayoutCommand::ToggleSplit => match parent.layout {
                ContainerLayout::SplitH => ContainerLayout::SplitV,
                _ => ContainerLayout::SplitH,
            },
            LayoutCommand::ToggleAll => match parent.layout {
                ContainerLayout::SplitH => ContainerLayout::SplitV,
                ContainerLayout::SplitV => ContainerLayout::Stacked,
                ContainerLayout::Stacked => ContainerLayout::Tabbed,
                ContainerLayout::Tabbed => ContainerLayout::SplitH,
            },
        };
        parent.set_layout(layout, &self.options);

        self.update_layout(true, None);
    }

    /// Toggles the parent of the focused node between tabbed and split.
    pub fn toggle_tabbed(&mut self) {
        let Some(path) = self.focused_path() else {
            return;
        };
        let parent = self.root.container_at(&path[..path.len() - 1]);
        let layout = if parent.layout == ContainerLayout::Tabbed {
            ContainerLayout::SplitH
        } else {
            ContainerLayout::Tabbed
        };
        self.set_layout(LayoutCommand::Set(layout));
    }

    pub fn focus_parent(&mut self) {
        if let Some(path) = self.focused_path() {
            if path.len() > 1 {
                self.focus_up += 1;
            }
        }
    }

    pub fn focus_child(&mut self) {
        self.focus_up = self.focus_up.saturating_sub(1);
    }

    /// Resizes a window, or the focused node, along an axis, taking space from its siblings.
    pub fn resize(&mut self, id: Option<&W::Id>, orientation: Orientation, change: SizeChange) {
        let path = match id {
            Some(id) => self.root.find(id),
            None => self.focused_path(),
        };
        let Some(mut path) = path else {
            return;
        };

        // Find the nearest split ancestor along this axis with more than one child.
        let idx = loop {
            let Some(idx) = path.pop() else {
                return;
            };
            let parent = self.root.container_at(&path);
            if parent.layout == ContainerLayout::split(orientation) && parent.children.len() > 1 {
                break idx;
            }
        };

        let gaps = self.options.layout.gaps;
        let working = match orientation {
            Orientation::Horizontal => self.working_area.size.w,
            Orientation::Vertical => self.working_area.size.h,
        };
        let parent = self.root.container_at_mut(&path);
        let count = parent.children.len();
        let total = match orientation {
            Orientation::Horizontal => parent.rect.size.w,
            Orientation::Vertical => parent.rect.size.h,
        };
        let available = f64::max(total - gaps * (count - 1) as f64, 1.);

        // Start from the laid out size, which can be above the share for a window at its min.
        let rect = parent.children[idx].rect();
        let current = match orientation {
            Orientation::Horizontal => rect.size.w,
            Orientation::Vertical => rect.size.h,
        };
        let target = match change {
            SizeChange::SetFixed(px) => f64::from(px),
            SizeChange::SetProportion(prop) => working * prop / 100.,
            SizeChange::AdjustFixed(px) => current + f64::from(px),
            SizeChange::AdjustProportion(prop) => current + working * prop / 100.,
        };

        let max_share = 1. - MIN_SHARE * (count - 1) as f64;
        let new_share = (target / available).clamp(MIN_SHARE, max_share);

        // Scale the other children to fill the rest.
        let others: f64 = 1. - parent.shares[idx];
        let rest = 1. - new_share;
        for (i, share) in parent.shares.iter_mut().enumerate() {
            if i == idx {
                *share = new_share;
            } else if others > 0. {
                *share = f64::max(*share / others * rest, MIN_SHARE / 2.);
            }
        }
        normalize(&mut parent.shares);

        self.update_layout(true, None);
    }

    pub fn set_fullscreen(&mut self, id: &W::Id, is_fullscreen: bool) -> bool {
        self.set_sizing(id, is_fullscreen, SizingMode::Fullscreen, |sizing| {
            sizing.is_fullscreen()
        })
    }

    pub fn set_maximized(&mut self, id: &W::Id, maximize: bool) -> bool {
        self.set_sizing(id, maximize, SizingMode::Maximized, |sizing| {
            sizing.is_maximized()
        })
    }

    fn set_sizing(
        &mut self,
        id: &W::Id,
        enable: bool,
        mode: SizingMode,
        is_mode: impl Fn(SizingMode) -> bool,
    ) -> bool {
        let Some(path) = self.root.find(id) else {
            return false;
        };
        let Node::Leaf(leaf) = self.root.node_at_mut(&path) else {
            unreachable!()
        };

        let sizing = if enable {
            mode
        } else if is_mode(leaf.sizing) {
            SizingMode::Normal
        } else {
            return false;
        };
        if leaf.sizing == sizing {
            return false;
        }
        leaf.sizing = sizing;

        self.update_layout(true, None);
        true
    }

    /// Requested sizing mode of a window.
    pub fn sizing_mode(&self, id: &W::Id) -> SizingMode {
        self.root
            .leaves()
            .find(|leaf| leaf.tile.window().id() == id)
            .map_or(SizingMode::Normal, |leaf| leaf.sizing)
    }

    pub fn is_active_pending_fullscreen(&self) -> bool {
        self.focused_leaf()
            .is_some_and(|leaf| leaf.sizing.is_fullscreen())
    }

    pub fn update_window(&mut self, id: &W::Id, serial: Option<Serial>) -> bool {
        let Some(path) = self.root.find(id) else {
            return false;
        };
        let Node::Leaf(leaf) = self.root.node_at_mut(&path) else {
            unreachable!()
        };

        if let Some(serial) = serial {
            leaf.tile.window_mut().on_commit(serial);
        }
        leaf.tile.update_window();
        true
    }

    pub fn start_open_animation(&mut self, id: &W::Id) -> bool {
        let Some(path) = self.root.find(id) else {
            return false;
        };
        let Node::Leaf(leaf) = self.root.node_at_mut(&path) else {
            unreachable!()
        };
        leaf.tile.start_open_animation();
        true
    }

    pub fn start_close_animation_for_window(
        &mut self,
        renderer: &mut GlesRenderer,
        id: &W::Id,
        blocker: TransactionBlocker,
    ) {
        let Some(path) = self.root.find(id) else {
            return;
        };
        let Node::Leaf(leaf) = self.root.node_at_mut(&path) else {
            unreachable!()
        };

        let tile_pos = leaf.pos + leaf.tile.render_offset();
        let tile_size = leaf.tile.tile_size();
        let Some(snapshot) = leaf.tile.take_unmap_snapshot() else {
            return;
        };

        self.start_close_animation_for_tile(renderer, snapshot, tile_size, tile_pos, blocker);
    }

    pub fn start_close_animation_for_tile(
        &mut self,
        renderer: &mut GlesRenderer,
        snapshot: TileRenderSnapshot,
        tile_size: Size<f64, Logical>,
        tile_pos: Point<f64, Logical>,
        blocker: TransactionBlocker,
    ) {
        let anim = Animation::new(
            self.clock.clone(),
            0.,
            1.,
            0.,
            self.options.animations.window_close.anim,
        );

        let blocker = if self.options.disable_transactions {
            TransactionBlocker::completed()
        } else {
            blocker
        };

        let scale = Scale::from(self.scale);
        let res = ClosingWindow::new(
            renderer, snapshot, scale, tile_size, tile_pos, blocker, anim,
        );
        match res {
            Ok(closing) => self.closing_windows.push(closing),
            Err(err) => warn!("error creating a closing window animation: {err:?}"),
        }
    }

    /// Size that a new window will get when it opens next to the focused node.
    pub fn new_window_size(&self, rules: &ResolvedWindowRules) -> Size<i32, Logical> {
        let gaps = self.options.layout.gaps;
        let full = Size::from((
            f64::max(self.working_area.size.w - gaps * 2., 1.),
            f64::max(self.working_area.size.h - gaps * 2., 1.),
        ));

        let size = match self.focused_path() {
            None => full,
            Some(path) => {
                let parent = self.root.container_at(&path[..path.len() - 1]);
                let count = (parent.children.len() + 1) as f64;
                let mut size = parent.rect.size;
                match parent.layout {
                    ContainerLayout::SplitH => {
                        size.w = (size.w - gaps * (count - 1.)) / count;
                    }
                    ContainerLayout::SplitV => {
                        size.h = (size.h - gaps * (count - 1.)) / count;
                    }
                    ContainerLayout::Tabbed | ContainerLayout::Stacked => {
                        size = parent.tabbed_content_rect(self.scale).size;
                    }
                }
                size
            }
        };

        let border = self.options.layout.border.merged_with(&rules.border);
        let mut size = size;
        if !border.off {
            size.w -= border.width * 2.;
            size.h -= border.width * 2.;
        }
        Size::from((f64::max(size.w, 1.), f64::max(size.h, 1.))).to_i32_floor()
    }

    pub fn new_window_toplevel_bounds(&self, rules: &ResolvedWindowRules) -> Size<i32, Logical> {
        let border = self.options.layout.border.merged_with(&rules.border);
        compute_toplevel_bounds(border, self.working_area.size, self.options.layout.gaps)
    }

    pub fn refresh(&mut self, is_active: bool, is_focused: bool) {
        let focused_id = self
            .focused_leaf()
            .map(|leaf| leaf.tile.window().id().clone());
        let options = self.options.clone();
        let working_area = self.working_area;
        let resize = self
            .interactive_resize
            .as_ref()
            .map(|resize| (resize.window.clone(), resize.edges));

        for leaf in self.root.leaves_mut() {
            let win = leaf.tile.window_mut();

            win.set_active_in_column(true);
            win.set_floating(false);

            let mut active = is_active && Some(win.id()) == focused_id.as_ref();
            if options.deactivate_unfocused_windows {
                active &= is_focused;
            }
            win.set_activated(active);

            let resize_data = resize
                .as_ref()
                .filter(|(window, _)| window == win.id())
                .map(|(_, edges)| super::InteractiveResizeData { edges: *edges });
            win.set_interactive_resize(resize_data);

            let border = options.layout.border.merged_with(&win.rules().border);
            win.set_bounds(compute_toplevel_bounds(
                border,
                working_area.size,
                options.layout.gaps,
            ));

            // If transactions are disabled, also disable combined throttling, for more
            // intuitive behavior.
            let intent = if options.disable_resize_throttling {
                ConfigureIntent::CanSend
            } else {
                win.configure_intent()
            };
            if matches!(
                intent,
                ConfigureIntent::CanSend | ConfigureIntent::ShouldSend
            ) {
                win.send_pending_configure();
            }

            win.refresh();
        }
    }

    /// Tiles with their positions, including hidden tabs, and whether they're visible.
    pub fn tiles_with_render_positions(
        &self,
    ) -> impl Iterator<Item = (&Tile<W>, Point<f64, Logical>, bool)> {
        let scale = self.scale;
        self.root.leaves().map(move |leaf| {
            let pos = leaf.pos + leaf.tile.render_offset();
            // Round to physical pixels.
            let pos = pos.to_physical_precise_round(scale).to_logical(scale);
            (&leaf.tile, pos, leaf.visible)
        })
    }

    pub fn tiles_with_render_positions_mut(
        &mut self,
        round: bool,
    ) -> impl Iterator<Item = (&mut Tile<W>, Point<f64, Logical>)> {
        let scale = self.scale;
        self.root.leaves_mut().map(move |leaf| {
            let mut pos = leaf.pos + leaf.tile.render_offset();
            if round {
                pos = pos.to_physical_precise_round(scale).to_logical(scale);
            }
            (&mut leaf.tile, pos)
        })
    }

    pub fn tiles_with_ipc_layouts(&self) -> impl Iterator<Item = (&Tile<W>, WindowLayout)> {
        self.root.leaves().map(|leaf| {
            let layout = WindowLayout {
                tile_pos_in_workspace_view: Some(leaf.pos.into()),
                ..leaf.tile.ipc_layout_template()
            };
            (&leaf.tile, layout)
        })
    }

    pub fn active_window_visual_rectangle(&self) -> Option<Rectangle<f64, Logical>> {
        let leaf = self.focused_leaf()?;
        let window_pos = leaf.pos + leaf.tile.window_loc();
        let window_rect = Rectangle::new(window_pos, leaf.tile.window_size());
        let area = if leaf.sizing.is_fullscreen() {
            Rectangle::from_size(self.view_size)
        } else {
            self.working_area
        };
        area.intersection(window_rect)
    }

    /// Area of the focused node: the container after `focus-parent`, otherwise the window.
    pub fn focused_node_rect(&self) -> Option<Rectangle<f64, Logical>> {
        let path = self.focused_path()?;
        match self.root.node_at(&path) {
            Node::Container(container) => Some(container.rect),
            Node::Leaf(_) => self.active_window_visual_rectangle(),
        }
    }

    pub fn popup_target_rect(&self, id: &W::Id) -> Option<Rectangle<f64, Logical>> {
        let leaf = self
            .root
            .leaves()
            .find(|leaf| leaf.tile.window().id() == id)?;
        let mut target = self.working_area;
        target.loc -= leaf.pos;
        target.loc -= leaf.tile.window_loc();
        Some(target)
    }

    pub fn render_above_top_layer(&self) -> bool {
        self.focused_leaf()
            .is_some_and(|leaf| leaf.tile.sizing_mode().is_fullscreen())
    }

    pub fn update_render_elements(
        &mut self,
        is_active: bool,
        view_rect: Rectangle<f64, Logical>,
        layer: RenderLayer,
    ) {
        let focused_id = self
            .focused_leaf()
            .map(|leaf| leaf.tile.window().id().clone());
        for leaf in self.root.leaves_mut() {
            // Skip tiles belonging to a different render layer.
            if layer.is_normal() == leaf.tile.is_moving_between_workspaces() {
                continue;
            }

            let is_active = is_active && Some(leaf.tile.window().id()) == focused_id.as_ref();
            let mut tile_view_rect = view_rect;
            tile_view_rect.loc -= leaf.pos + leaf.tile.render_offset();
            leaf.tile.update_render_elements(is_active, tile_view_rect);
        }

        if layer.is_normal() {
            let scale = self.scale;
            update_tab_render_elements(&mut self.root, is_active, view_rect, scale, true);
            self.update_debug_outlines();
        }
    }

    /// Updates the debug outlines of containers: coloured by layout, nested ones inset, and the
    /// focused container in white. Split containers also get dividers between their children
    /// and a label naming their layout.
    fn update_debug_outlines(&mut self) {
        // The focused container is the focused node after `focus parent`, otherwise the parent
        // of the focused window.
        let focused = self
            .focused_path()
            .map(|path| match self.root.node_at(&path) {
                Node::Container(_) => path,
                Node::Leaf(_) => path[..path.len() - 1].to_vec(),
            });
        let enabled = self.options.debug_containers && !self.is_empty();

        fn walk<W: LayoutElement>(
            container: &mut Container<W>,
            path: &mut Vec<usize>,
            visible: bool,
            enabled: bool,
            focused: Option<&[usize]>,
        ) {
            if enabled && visible {
                let is_focused = focused == Some(path.as_slice());
                let (width, color) = if is_focused {
                    (4., [1., 1., 1., 1.])
                } else {
                    let [r, g, b] = debug_color(container.layout);
                    // Premultiplied alpha.
                    let a = 0.8;
                    (2., [r * a, g * a, b * a, a])
                };

                // Inset nested containers so their outlines don't cover their parents'.
                let inset = path.len() as f64 * 3.;
                let mut rect = container.rect;
                rect.loc += Point::from((inset, inset));
                rect.size.w = f64::max(rect.size.w - inset * 2., width * 2.);
                rect.size.h = f64::max(rect.size.h - inset * 2., width * 2.);

                let (x, y, w, h) = (rect.loc.x, rect.loc.y, rect.size.w, rect.size.h);
                let mut edges = vec![
                    (Point::from((x, y)), Size::from((w, width))),
                    (Point::from((x, y + h - width)), Size::from((w, width))),
                    (Point::from((x, y)), Size::from((width, h))),
                    (Point::from((x + w - width, y)), Size::from((width, h))),
                ];

                // Dividers in the middle of the gaps between children.
                let horizontal = container.layout == ContainerLayout::SplitH;
                if container.layout.is_split() {
                    for pair in container.children.windows(2) {
                        let (a, b) = (pair[0].rect(), pair[1].rect());
                        if horizontal {
                            let mid = (a.loc.x + a.size.w + b.loc.x) / 2.;
                            edges
                                .push((Point::from((mid - width / 2., y)), Size::from((width, h))));
                        } else {
                            let mid = (a.loc.y + a.size.h + b.loc.y) / 2.;
                            edges
                                .push((Point::from((x, mid - width / 2.)), Size::from((w, width))));
                        }
                    }
                }

                container.debug_outline.resize_with(edges.len(), || {
                    (SolidColorBuffer::default(), Point::default())
                });
                for ((buffer, pos), (loc, size)) in container.debug_outline.iter_mut().zip(edges) {
                    buffer.update(size, color);
                    *pos = loc;
                }

                let anchor = Point::from((x + w / 2., y + width + 4.));
                container.debug_label = Some((is_focused, anchor));
            } else {
                container.debug_outline.clear();
                container.debug_label = None;
            }

            let is_tabbed = !container.layout.is_split();
            let focused_child = container.focused;
            for (idx, child) in container.children.iter_mut().enumerate() {
                if let Node::Container(inner) = child {
                    path.push(idx);
                    let visible = visible && (!is_tabbed || idx == focused_child);
                    walk(inner, path, visible, enabled, focused);
                    path.pop();
                }
            }
        }

        walk(
            &mut self.root,
            &mut Vec::new(),
            true,
            enabled,
            focused.as_deref(),
        );
    }

    /// Layouts of the containers with a debug label, in tree order.
    #[cfg(test)]
    pub fn debug_labels(&self) -> Vec<ContainerLayout> {
        let mut labels = Vec::new();
        for_each_container(&self.root, &mut |c| {
            if c.debug_label.is_some() {
                labels.push(c.layout);
            }
        });
        labels
    }

    /// Number of debug outline edges currently drawn.
    #[cfg(test)]
    pub fn debug_outline_edges(&self) -> usize {
        let mut count = 0;
        for_each_container(&self.root, &mut |c| count += c.debug_outline.len());
        count
    }

    fn render_debug_label<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        container: &Container<W>,
    ) -> Option<PrimaryGpuTextureRenderElement> {
        let (is_focused, anchor) = container.debug_label?;
        let layout = container.layout;
        let scale = self.scale;

        let key = (layout, is_focused, NotNan::new(scale).unwrap());
        let buffer = self.debug_labels.get(key, || {
            let text = match layout {
                ContainerLayout::SplitH => "H",
                ContainerLayout::SplitV => "V",
                ContainerLayout::Tabbed => "T",
                ContainerLayout::Stacked => "S",
            };
            let [r, g, b] = debug_color(layout);
            let alpha = if is_focused { 1. } else { 0.75 };
            let fill = [f64::from(r), f64::from(g), f64::from(b), alpha];
            render_pill(text, DEBUG_LABEL_FONT, DEBUG_LABEL_RADIUS, fill, scale)
        })?;
        let texture =
            TextureBuffer::from_memory_buffer(renderer.as_gles_renderer(), &buffer).ok()?;

        let loc = anchor - Point::from((texture.logical_size().w / 2., 0.));
        let loc = loc.to_physical_precise_round(scale).to_logical(scale);
        let elem = TextureRenderElement::from_texture_buffer(
            texture,
            loc,
            1.,
            None,
            None,
            Kind::Unspecified,
        );
        Some(PrimaryGpuTextureRenderElement(elem))
    }

    pub fn render<R: NiriRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        xray_pos: XrayPos,
        focus_ring: bool,
        layer: RenderLayer,
        push: &mut dyn FnMut(TilingSpaceRenderElement<R>),
    ) {
        let scale = Scale::from(self.scale);

        // Draw the debug outlines on top of everything.
        if layer.is_normal() {
            for_each_container(&self.root, &mut |container| {
                for (buffer, pos) in &container.debug_outline {
                    let pos = pos.to_physical_precise_round(scale).to_logical(scale);
                    let elem =
                        SolidColorRenderElement::from_buffer(buffer, pos, 1., Kind::Unspecified);
                    push(elem.into());
                }
            });

            for_each_container(&self.root, &mut |container| {
                if let Some(elem) = self.render_debug_label(ctx.renderer, container) {
                    push(elem.into());
                }
            });
        }

        // Draw the closing windows on top of the other windows.
        if layer.is_normal() {
            let view_rect = Rectangle::from_size(self.view_size);
            for closing in self.closing_windows.iter().rev() {
                let elem = closing.render(ctx.as_gles(), view_rect, scale);
                push(elem.into());
            }
        }

        // Draw the tabs on top of the tiles, unless a tile covers the whole view.
        let any_fullscreen = self.root.leaves().any(|leaf| leaf.sizing.is_fullscreen());
        if layer.is_normal() && !any_fullscreen {
            for_each_container(&self.root, &mut |container| {
                if !container.layout.is_split() {
                    let pos = container.rect.loc;
                    let pos = pos.to_physical_precise_round(scale).to_logical(scale);
                    container
                        .tab_indicator
                        .render(ctx.renderer, pos, &mut |elem| push(elem.into()));
                }
            });
        }

        let focused_id = self
            .focused_leaf()
            .map(|leaf| leaf.tile.window().id().clone());
        let tiles: Vec<_> = self.root.leaves().collect();

        // Maximized and fullscreen tiles go on top of the others, and the focused one on top of
        // everything else.
        let raised = |leaf: &Leaf<W>| {
            if Some(leaf.tile.window().id()) == focused_id.as_ref() {
                2
            } else if !leaf.sizing.is_normal() {
                1
            } else {
                0
            }
        };
        let mut order: Vec<usize> = (0..tiles.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(raised(tiles[i])));

        for i in order {
            let leaf = tiles[i];

            // Skip tiles belonging to a different render layer.
            if layer.is_normal() == leaf.tile.is_moving_between_workspaces() {
                continue;
            }

            if !leaf.visible {
                continue;
            }

            let tile_pos = leaf.pos + leaf.tile.render_offset();
            let tile_pos = tile_pos.to_physical_precise_round(scale).to_logical(scale);

            let focus_ring = focus_ring && Some(leaf.tile.window().id()) == focused_id.as_ref();
            let xray_pos = xray_pos.offset(tile_pos);
            if leaf.is_clipped() {
                let crop = Rectangle::new(tile_pos, leaf.size).to_physical_precise_round(scale);
                leaf.tile
                    .render(ctx.r(), tile_pos, xray_pos, focus_ring, &mut |elem| {
                        if let Some(elem) = CropRenderElement::from_element(elem, scale, crop) {
                            push(elem.into());
                        }
                    });
            } else {
                leaf.tile
                    .render(ctx.r(), tile_pos, xray_pos, focus_ring, &mut |elem| {
                        push(elem.into())
                    });
            }
        }
    }

    pub fn window_under(&self, pos: Point<f64, Logical>) -> Option<(&W, HitType)> {
        // Tabs first, they're drawn on top, unless a tile covers the whole view.
        let any_fullscreen = self.root.leaves().any(|leaf| leaf.sizing.is_fullscreen());
        if !any_fullscreen {
            if let Some(rv) = tab_hit(&self.root, pos, self.scale) {
                return Some(rv);
            }
        }

        // Then raised tiles, then the rest, matching the render order.
        let focused_id = self
            .focused_leaf()
            .map(|leaf| leaf.tile.window().id().clone());
        let mut leaves: Vec<_> = self.root.leaves().filter(|leaf| leaf.visible).collect();
        leaves.sort_by_key(|leaf| {
            let raised = if Some(leaf.tile.window().id()) == focused_id.as_ref() {
                2
            } else if !leaf.sizing.is_normal() {
                1
            } else {
                0
            };
            std::cmp::Reverse(raised)
        });

        let scale = self.scale;
        for leaf in leaves {
            let tile_pos = leaf.pos + leaf.tile.render_offset();
            let tile_pos = tile_pos.to_physical_precise_round(scale).to_logical(scale);
            // The clipped off part of a window doesn't take input.
            if leaf.is_clipped() && !Rectangle::new(tile_pos, leaf.size).contains(pos) {
                continue;
            }
            if let Some(rv) = HitType::hit_tile(&leaf.tile, tile_pos, pos) {
                return Some(rv);
            }
        }

        None
    }

    /// Finds where a window dropped at `pos` goes: next to a visible tile, on the side of the
    /// nearest edge. Returns the index of the tile in tree order and the side.
    pub fn insert_position(&self, pos: Point<f64, Logical>) -> Option<(usize, Direction)> {
        let distance_to = |leaf: &Leaf<W>| {
            let rect = Rectangle::new(leaf.pos, leaf.size);
            let dx = f64::max(
                f64::max(rect.loc.x - pos.x, pos.x - (rect.loc.x + rect.size.w)),
                0.,
            );
            let dy = f64::max(
                f64::max(rect.loc.y - pos.y, pos.y - (rect.loc.y + rect.size.h)),
                0.,
            );
            dx * dx + dy * dy
        };

        let (idx, leaf) = self
            .root
            .leaves()
            .enumerate()
            .filter(|(_, leaf)| leaf.visible && leaf.sizing.is_normal())
            .min_by(|(_, a), (_, b)| distance_to(a).total_cmp(&distance_to(b)))?;

        // Pick the side by the nearest edge, relative to the tile size.
        let rel_x = (pos.x - leaf.pos.x) / f64::max(leaf.size.w, 1.);
        let rel_y = (pos.y - leaf.pos.y) / f64::max(leaf.size.h, 1.);
        let edges = [
            (rel_x, Direction::Left),
            (1. - rel_x, Direction::Right),
            (rel_y, Direction::Up),
            (1. - rel_y, Direction::Down),
        ];
        let (_, side) = edges
            .into_iter()
            .min_by(|(a, _), (b, _)| a.total_cmp(b))
            .unwrap();

        Some((idx, side))
    }

    /// Area to highlight for dropping a window next to a tile.
    pub fn insert_hint_area(
        &self,
        leaf_idx: usize,
        side: Direction,
    ) -> Option<Rectangle<f64, Logical>> {
        let leaf = self.root.leaves().nth(leaf_idx)?;
        let mut rect = Rectangle::new(leaf.pos, leaf.size);
        match side {
            Direction::Left => rect.size.w /= 2.,
            Direction::Right => {
                rect.size.w /= 2.;
                rect.loc.x += rect.size.w;
            }
            Direction::Up => rect.size.h /= 2.,
            Direction::Down => {
                rect.size.h /= 2.;
                rect.loc.y += rect.size.h;
            }
        }
        Some(rect)
    }

    /// Inserts a tile next to the tile at `leaf_idx` in tree order, on the given side.
    ///
    /// If the tile's container goes along the other axis, the tile gets wrapped in a new split
    /// container first, like dropping a window in i3.
    pub fn insert_next_to(&mut self, leaf_idx: usize, side: Direction, tile: Tile<W>) {
        let leaf = self.new_leaf(tile);

        let Some(id) = self
            .root
            .leaves()
            .nth(leaf_idx)
            .map(|leaf| leaf.tile.window().id().clone())
        else {
            // The target went away, just add the tile normally.
            let path = self.focused_path().unwrap_or_default();
            self.insert_leaf_after(&path, leaf, true);
            return;
        };
        let mut path = self.root.find(&id).unwrap();

        let (&idx, parent_path) = path.split_last().unwrap();
        let parent = self.root.container_at_mut(parent_path);
        let orientation = side.orientation();

        if parent.layout.orientation() != orientation || !parent.layout.is_split() {
            if parent.children.len() == 1 && parent.layout.is_split() {
                parent.set_layout(ContainerLayout::split(orientation), &self.options);
            } else {
                // Wrap the target in a split container along the drop axis.
                let layout = ContainerLayout::split(orientation);
                let mut wrapper = Container::new(layout, &self.options);
                let placeholder = Node::Container(Box::new(Container::new(layout, &self.options)));
                let node = std::mem::replace(&mut parent.children[idx], placeholder);
                wrapper.children.push(node);
                wrapper.shares.push(1.);
                parent.children[idx] = Node::Container(Box::new(wrapper));
                path.push(0);
            }
        }

        let (&idx, parent_path) = path.split_last().unwrap();
        let insert_idx = if side.is_forward() { idx + 1 } else { idx };
        self.root
            .container_at_mut(parent_path)
            .insert(insert_idx, Node::Leaf(Box::new(leaf)));

        let mut new_path = parent_path.to_vec();
        new_path.push(insert_idx);
        self.focus_path(&new_path);

        self.update_layout(true, None);
    }

    /// Finds the boundary on one side of the child of `path` that is a split along `side`'s
    /// axis, walking up the tree.
    fn resize_boundary(&self, path: &[usize], side: Direction) -> Option<ResizeBoundary> {
        let mut path = path.to_vec();
        while let Some(idx) = path.pop() {
            let container = self.root.container_at(&path);
            if container.layout != ContainerLayout::split(side.orientation()) {
                continue;
            }

            let neighbour = if side.is_forward() {
                idx + 1
            } else if idx > 0 {
                idx - 1
            } else {
                continue;
            };
            if neighbour >= container.children.len() {
                continue;
            }

            let count = container.children.len();
            let total = match side.orientation() {
                Orientation::Horizontal => container.rect.size.w,
                Orientation::Vertical => container.rect.size.h,
            };
            let available = f64::max(total - self.options.layout.gaps * (count - 1) as f64, 1.);

            return Some(ResizeBoundary {
                container: path,
                count,
                child: idx,
                neighbour,
                start_shares: (container.shares[idx], container.shares[neighbour]),
                available,
            });
        }
        None
    }

    pub fn interactive_resize_begin(
        &mut self,
        window: W::Id,
        edges: crate::utils::ResizeEdge,
    ) -> bool {
        use crate::utils::ResizeEdge;

        if self.interactive_resize.is_some() {
            return false;
        }
        let Some(path) = self.root.find(&window) else {
            return false;
        };

        let horizontal = if edges.contains(ResizeEdge::LEFT) {
            self.resize_boundary(&path, Direction::Left)
        } else if edges.contains(ResizeEdge::RIGHT) {
            self.resize_boundary(&path, Direction::Right)
        } else {
            None
        };
        let vertical = if edges.contains(ResizeEdge::TOP) {
            self.resize_boundary(&path, Direction::Up)
        } else if edges.contains(ResizeEdge::BOTTOM) {
            self.resize_boundary(&path, Direction::Down)
        } else {
            None
        };

        if horizontal.is_none() && vertical.is_none() {
            return false;
        }

        self.interactive_resize = Some(TilingResize {
            window,
            edges,
            horizontal,
            vertical,
        });
        true
    }

    pub fn interactive_resize_update(
        &mut self,
        window: &W::Id,
        delta: Point<f64, Logical>,
    ) -> bool {
        let Some(resize) = &self.interactive_resize else {
            return false;
        };
        if &resize.window != window {
            return false;
        }

        let boundaries = [
            (resize.horizontal.clone(), delta.x),
            (resize.vertical.clone(), delta.y),
        ];
        for (boundary, delta) in boundaries {
            let Some(b) = boundary else {
                continue;
            };

            // Stop if the tree changed under the resize.
            let valid = self.root.find(window).is_some_and(|path| {
                path.starts_with(&b.container)
                    && path.len() > b.container.len()
                    && path[b.container.len()] == b.child
            });
            if !valid || self.root.container_at(&b.container).children.len() != b.count {
                self.interactive_resize = None;
                return false;
            }

            // Moving the boundary towards the neighbour grows the child.
            let grows = if b.neighbour > b.child { delta } else { -delta };
            let (child, neighbour) = b.start_shares;
            let pair = child + neighbour;
            let new_child = (child + grows / b.available).clamp(MIN_SHARE, pair - MIN_SHARE);

            let container = self.root.container_at_mut(&b.container);
            container.shares[b.child] = new_child;
            container.shares[b.neighbour] = pair - new_child;
        }

        self.update_layout(false, None);
        true
    }

    pub fn interactive_resize_end(&mut self, window: Option<&W::Id>) {
        let Some(resize) = &self.interactive_resize else {
            return;
        };
        if window.is_none_or(|window| window == &resize.window) {
            self.interactive_resize = None;
        }
    }

    pub fn view_size(&self) -> Size<f64, Logical> {
        self.view_size
    }

    pub fn working_area(&self) -> Rectangle<f64, Logical> {
        self.working_area
    }

    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    pub fn options(&self) -> &Rc<Options> {
        &self.options
    }

    #[cfg(test)]
    pub fn verify_invariants(&self) {
        assert!(self.scale > 0.);
        assert!(self.scale.is_finite());

        self.root.verify_invariants(true);

        if let Some(path) = self.focused_path() {
            assert!(!path.is_empty(), "the root can't be focused");
        }

        if let Some(resize) = &self.interactive_resize {
            assert!(
                self.has_window(&resize.window),
                "interactive resize window must be present in the tree"
            );
        }

        let mut ids = Vec::new();
        for leaf in self.root.leaves() {
            let tile = &leaf.tile;
            assert!(Rc::ptr_eq(&self.options, &tile.options));
            assert_eq!(self.view_size, tile.view_size());
            assert_eq!(self.clock, tile.clock);
            assert_eq!(self.scale, tile.scale());
            tile.verify_invariants();

            let id = tile.window().id();
            assert!(!ids.contains(&id), "windows must be unique");
            ids.push(id);
        }
    }
}

fn for_each_container<W: LayoutElement>(
    container: &Container<W>,
    f: &mut dyn FnMut(&Container<W>),
) {
    f(container);
    for child in &container.children {
        if let Node::Container(inner) = child {
            for_each_container(inner, f);
        }
    }
}

fn for_each_container_mut<W: LayoutElement>(
    container: &mut Container<W>,
    f: &mut dyn FnMut(&mut Container<W>),
) {
    f(container);
    for child in &mut container.children {
        if let Node::Container(inner) = child {
            for_each_container_mut(inner, f);
        }
    }
}

fn update_containers_config<W: LayoutElement>(container: &mut Container<W>, options: &Options) {
    for_each_container_mut(container, &mut |c| c.update_tab_indicator_config(options));
}

/// Area of the tabs, relative to the container.
fn tab_area<W: LayoutElement>(container: &Container<W>, scale: f64) -> Rectangle<f64, Logical> {
    let content = container.tabbed_content_rect(scale);
    Rectangle::new(content.loc - container.rect.loc, content.size)
}

fn update_tab_render_elements<W: LayoutElement>(
    container: &mut Container<W>,
    is_active: bool,
    view_rect: Rectangle<f64, Logical>,
    scale: f64,
    visible: bool,
) {
    let is_tabbed = !container.layout.is_split();

    if is_tabbed {
        let area = tab_area(container, scale);
        let config = container.tab_indicator.config();
        let focused = container.focused;
        let origin = container.rect.loc;
        let tabs: Vec<TabInfo> = container
            .children
            .iter()
            .enumerate()
            .map(|(idx, child)| {
                let leaf = child.focused_leaf();
                let is_urgent = leaf.tile.window().is_urgent();
                TabInfo::from_tile(
                    &leaf.tile,
                    leaf.pos - origin,
                    idx == focused,
                    is_urgent,
                    &config,
                )
            })
            .collect();

        // Hide the tabs when a tile in this container is fullscreen or maximized.
        let enabled = visible && container.leaves().all(|leaf| leaf.sizing.is_normal());

        let mut area_view_rect = view_rect;
        area_view_rect.loc -= origin;
        let count = tabs.len();
        container.tab_indicator.update_render_elements(
            enabled,
            area,
            area_view_rect,
            count,
            tabs.into_iter(),
            is_active,
            scale,
        );
    } else {
        // Clear the tabs of containers that used to be tabbed.
        container.tab_indicator.update_render_elements(
            false,
            Rectangle::default(),
            Rectangle::default(),
            0,
            std::iter::empty(),
            false,
            scale,
        );
    }

    let focused = container.focused;
    for (idx, child) in container.children.iter_mut().enumerate() {
        if let Node::Container(inner) = child {
            let visible = visible && (!is_tabbed || idx == focused);
            update_tab_render_elements(inner, is_active, view_rect, scale, visible);
        }
    }
}

/// Finds the window whose tab is under `pos`.
fn tab_hit<W: LayoutElement>(
    container: &Container<W>,
    pos: Point<f64, Logical>,
    scale: f64,
) -> Option<(&W, HitType)> {
    if !container.layout.is_split() && container.leaves().all(|leaf| leaf.sizing.is_normal()) {
        let origin = container.rect.loc;
        let origin = origin.to_physical_precise_round(scale).to_logical(scale);
        if let Some(idx) = container.tab_indicator.hit(
            tab_area(container, scale),
            container.children.len(),
            scale,
            pos - origin,
        ) {
            let leaf = container.children[idx].focused_leaf();
            let hit = HitType::Activate {
                is_tab_indicator: true,
            };
            return Some((leaf.tile.window(), hit));
        }
    }

    let is_tabbed = !container.layout.is_split();
    for (idx, child) in container.children.iter().enumerate() {
        if is_tabbed && idx != container.focused {
            continue;
        }
        if let Node::Container(inner) = child {
            if let Some(rv) = tab_hit(inner, pos, scale) {
                return Some(rv);
            }
        }
    }

    None
}

fn compute_toplevel_bounds(
    border_config: niri_config::Border,
    working_area_size: Size<f64, Logical>,
    gaps: f64,
) -> Size<i32, Logical> {
    let mut border = 0.;
    if !border_config.off {
        border = border_config.width * 2.;
    }

    Size::from((
        f64::max(working_area_size.w - gaps * 2. - border, 1.),
        f64::max(working_area_size.h - gaps * 2. - border, 1.),
    ))
    .to_i32_floor()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_lengths_follow_shares_without_mins() {
        assert_eq!(split_lengths(&[0.5, 0.5], &[0., 0.], 100.), [50., 50.]);
    }

    #[test]
    fn split_lengths_give_mins_and_share_the_rest() {
        let lens = split_lengths(&[0.25, 0.25, 0.5], &[60., 0., 0.], 100.);
        // 40 left for the others, in a 1:2 ratio.
        assert_eq!(lens[0], 60.);
        assert!((lens[1] - 40. / 3.).abs() < 1e-9);
        assert!((lens[2] - 80. / 3.).abs() < 1e-9);
    }

    #[test]
    fn split_lengths_pin_mins_that_appear_after_others() {
        // After pinning the first child, the second one falls below its min too.
        let lens = split_lengths(&[0.2, 0.3, 0.5], &[50., 25., 0.], 100.);
        assert_eq!(lens, [50., 25., 25.]);
    }

    #[test]
    fn split_lengths_fall_back_to_shares_when_mins_dont_fit() {
        assert_eq!(split_lengths(&[0.5, 0.5], &[80., 80.], 100.), [50., 50.]);
    }
}
