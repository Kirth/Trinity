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

Moving a window to another workspace leaves you where you are, like in i3: the window goes away and the focus stays on the current workspace.
To follow the window instead, set `focus=true` on that bind, or turn `pushaway` off for all of them:

```kdl
layout {
    layout-mode "i3"
    // Follow windows moved to another workspace.
    pushaway false
}

binds {
    // Moves the window away and stays here.
    Mod+Shift+1 { move-window-to-workspace 1; }
    // Always follows the window, even with pushaway on.
    Mod+Ctrl+1 { move-window-to-workspace 1 focus=true; }
}
```

`pushaway` can only be set in the top-level `layout {}` section, and has no effect in the scrolling layout.

#### Scratchpad

Workspace 0 is the scratchpad: workspaces are never numbered 0 on their own, so it sorts first and only holds the windows you send there.
To send a window there, move it all the way to the right with `move-column-right`, then press `move-column-right` three more times in quick succession (each within 400 ms of the last): the window flies off the right edge in a puff of smoke and lands on workspace 0.
Holding the key down doesn't count, and focus stays on the current workspace.
The fly-off and the puff play even with `animations { off; }`.

You can also throw a window to the scratchpad with the mouse: drag it and let go while it's moving quickly into the right edge of the screen.
A slow drag to the edge drops it there as usual.

Poofed one by accident? Press `move-column-left` within 2 seconds, and the window comes back exactly where it was, in the same container and at the same size.

Windows on workspace 0 peek out as thin live thumbnails at the right edge of the rightmost output, like a drawer, with the newest on top.
Hover one to slide it out, click it to bring the window next to the focused one, or drag it out to put it anywhere.
The drawer hides in the overview and behind a focused fullscreen window.

Like any other workspace, it goes away once it's empty.
Use `focus-workspace 0` to get to it, and the usual move actions to bring windows back.

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
    Mod+R { resize-mode; }

    Mod+Shift+Space { toggle-window-floating; }
    Mod+Space { switch-focus-between-floating-and-tiling; }
}
```

### Resize mode

`resize-mode` resizes the focused window, or the focused container after `focus-parent`, with plain keys, like i3's `mode "resize"`.
A "resize" label shows on what's being resized.

```kdl
binds {
    Mod+R { resize-mode; }
}
```

| Key | Action |
| --- | --- |
| <kbd>Left</kbd> or <kbd>h</kbd> | shrink the width by 10% of the screen |
| <kbd>Right</kbd> or <kbd>l</kbd> | grow the width |
| <kbd>Up</kbd> or <kbd>k</kbd> | shrink the height |
| <kbd>Down</kbd> or <kbd>j</kbd> | grow the height |
| <kbd>Escape</kbd>, <kbd>Return</kbd> or the `resize-mode` bind | leave resize mode |

Holding a key keeps resizing, and modifiers don't matter.
Other keys do nothing while in resize mode.
Locking the screen or opening the overview, screenshot UI or window switcher leaves it.

Like in i3, growing a window takes the space from the other windows in its container.
Resize mode also works for floating windows and in the scrolling layout.

### Jumping to windows

`focus-window-with-hints` shows a hint in a circle at the top-left of every visible window, on all outputs, tiled and floating.
Type a hint to focus its window: the hints use the keys <kbd>a</kbd> <kbd>s</kbd> <kbd>d</kbd> <kbd>f</kbd> <kbd>j</kbd> <kbd>k</kbd> <kbd>l</kbd>.
With more than seven windows, hints are two keys long, and the first key hides the hints that no longer match.
Windows hidden behind a fullscreen or maximized window are drawn faintly on top of it while the hints are shown, so you can see which window a hint would bring forward.
Any other key, <kbd>Escape</kbd> or a click cancels.

```kdl
binds {
    Mod+Semicolon { focus-window-with-hints; }
}
```

This action also works in the scrolling layout.

### Side view

`toggle-side-view` turns the focused window into a side-view window, picture-in-picture style: it floats in the bottom-right corner at a quarter of the screen width with 90% opacity.
Run it again to put the window back where it was, tiled or floating.

```kdl
binds {
    Mod+P { toggle-side-view; }
    Mod+Shift+P { side-view-next-corner; }
}
```

A side-view window:

- stays in its corner when the window or the screen changes size; `side-view-next-corner` and `side-view-prev-corner` move it clockwise and counter-clockwise, and dragging it snaps it to the nearest corner;
- follows you when you switch workspaces on its output;
- is skipped by directional focus and `switch-focus-between-floating-and-tiling`, so it doesn't take your keystrokes; click it or use `focus-window-with-hints` to focus it;
- leaves side view when it goes fullscreen or maximized, or is toggled out of floating.

Size, opacity and margin are set in [`layout { side-view {} }`](./Configuration:-Layout.md#side-view).
Side view also works in the scrolling layout.

### Seeing the containers

Containers are invisible, which can make the tree hard to follow.
To see them, turn on the debug outlines, either in the config with [`show-i3-containers`](./Configuration:-Debug-Options.md#show-i3-containers), or with a bind:

```kdl
binds {
    Mod+Shift+Ctrl+I { toggle-debug-i3-containers; }
}
```

Each container gets an outline and a label for its layout:

- `H`, blue: `splith`, children side by side.
- `V`, green: `splitv`, children on top of each other.
- `T`, orange: `tabbed`.
- `S`, purple: `stacking`.

Lines in the container's colour divide its children.
The focused container is drawn in white.
A new window opens right after the focused window, in the same container: to its right in an `H` container, below it in a `V` container.

### Differences from i3

- Tabs show as colored bars without window titles.
- The scratchpad is a normal workspace (workspace 0) rather than i3's floating `scratchpad show`.
- There are no marks or i3 IPC compatibility.
