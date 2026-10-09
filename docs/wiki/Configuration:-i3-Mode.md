### Overview

<sup>Since: next release</sup>

With `layout-mode "i3"`, niri lays out windows and workspaces like the i3 window manager instead of as scrollable columns.

```kdl
layout {
    layout-mode "i3"
}
```

The setting applies to all outputs and workspaces, so it can only be set in the top-level `layout {}` section.
You can switch it while niri is running: windows keep their arrangement as well as possible.
Going from scrolling to i3, each column becomes a vertical container (or a tabbed one for tabbed columns).
Going back, windows become columns in order.

### Windows

Windows are tiled in a tree of containers that fills the working area of the output.
Each container lays out its children in one of four ways:

- `splith`: side by side.
- `splitv`: on top of each other.
- `tabbed`: one at a time, with tabs along the top.
- `stacking`: one at a time, with tabs along the side.

New windows open next to the focused window, in the same container.
To open the next window below the focused one instead, `split "vertical"` first, like in i3.

Moving focus or windows works across containers following i3's rules: moving a window at the edge of its container takes it out into the surrounding container, and moving it towards a container puts it inside.
After `focus-parent`, moving and resizing act on the whole container.

You can drag windows with <kbd>Mod</kbd> and the left mouse button: dropping one on the side of another window places it on that side, splitting the other window if needed.
Dragging a window's edge with <kbd>Mod</kbd> and the right mouse button resizes it against its neighbour.

Floating windows work the same as in the scrolling layout.

### Workspaces

Workspaces are numbered like in i3:

- `focus-workspace 7` goes to workspace 7, creating it if it doesn't exist, even if there's no 3 to 6.
- If workspace 7 is on another output, that output gets focused.
- Workspace numbers are unique across all outputs, and workspaces stay sorted by number.
- An empty workspace goes away as soon as you switch away from it.
- Switching is instant: workspaces aren't laid out in a vertical strip, except in the overview.
- `focus-workspace-down` and `focus-workspace-up`, and the workspace switch gestures, go to the next and previous workspace on the output, wrapping around.
- `move-workspace-up`, `move-workspace-down` and `move-workspace-to-index` do nothing, since workspaces are always sorted.

`niri msg workspaces` and status bars show the workspace number as its index.

`empty-workspace-above-first` and `positional-workspaces` have no effect in i3 mode.
[Named workspaces](./Configuration:-Named-Workspaces.md) still work and always exist, after the numbered ones.

### Actions

These actions only do something in i3 mode:

```kdl
binds {
    // Split the focused window, so the next window opens beside or below it.
    Mod+B { split "horizontal"; }
    Mod+V { split "vertical"; }
    // Or "toggle", for the opposite of the current container.

    // Change the layout of the container the focused window is in.
    Mod+S { layout "stacking"; }
    Mod+W { layout "tabbed"; }
    Mod+E { layout "toggle-split"; }
    // Also "splith", "splitv" and "toggle-all".

    Mod+A { focus-parent; }
    Mod+Shift+A { focus-child; }
}
```

Existing actions keep working, with these meanings:

- `focus-column-left`/`right` and `focus-window-up`/`down` move focus in that direction.
- `move-column-left`/`right` and `move-window-up`/`down` move the focused window or container in that direction.
- `move-column-to-workspace` moves the focused window, like `move-window-to-workspace`.
- `set-column-width` and `set-window-height` resize the focused window or container along that axis.
- `toggle-column-tabbed-display` switches the container between tabbed and `splith`.
- `fullscreen-window` and `maximize-window-to-edges` work as usual.

Actions that only make sense for scrolling columns do nothing in i3 mode.
These include consuming and expelling windows, centering, preset widths, `expand-column-to-available-width`, `focus-column`, `move-column-to-index` and swapping windows.
The `default-column-width`, `open-maximized` and `default-column-display` window rules don't apply either.

Here's a set of binds close to i3's defaults:

```kdl
binds {
    Mod+Left  { focus-column-left; }
    Mod+Down  { focus-window-down; }
    Mod+Up    { focus-window-up; }
    Mod+Right { focus-column-right; }

    Mod+Shift+Left  { move-column-left; }
    Mod+Shift+Down  { move-window-down; }
    Mod+Shift+Up    { move-window-up; }
    Mod+Shift+Right { move-column-right; }

    Mod+1 { focus-workspace 1; }
    Mod+2 { focus-workspace 2; }
    Mod+3 { focus-workspace 3; }
    Mod+Shift+1 { move-window-to-workspace 1; }
    Mod+Shift+2 { move-window-to-workspace 2; }
    Mod+Shift+3 { move-window-to-workspace 3; }

    Mod+H { split "horizontal"; }
    Mod+V { split "vertical"; }
    Mod+S { layout "stacking"; }
    Mod+W { layout "tabbed"; }
    Mod+E { layout "toggle-split"; }
    Mod+F { fullscreen-window; }
    Mod+A { focus-parent; }

    Mod+Shift+Space { toggle-window-floating; }
    Mod+Space { switch-focus-between-floating-and-tiling; }
}
```

### Differences from i3

- Tabs show as colored bars without window titles.
- There are no marks, scratchpad, or i3 IPC compatibility.
