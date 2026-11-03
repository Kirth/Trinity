# Card providers

Small programs that tell niri what to show on the cards of windows set aside in the strip, in i3 mode.
niri knows nothing about the apps: providers send it annotations with

```sh
niri msg action annotate-window --id <window> --source <name> \
    [--badge TEXT] [--status done|working|waiting|error] [--text TEXT] [--progress 0..1] [--icon PATH] [--ttl-ms MS]
niri msg action clear-annotations --id <window> [--source <name>]
```

Each provider uses its own `--source`, so several can annotate the same window.
`niri msg --json event-stream` tells providers when to act, without polling: `WindowCardChanged` when a window goes into or out of a strip, or its card is shown or hidden, and `WindowOpenedOrChanged` when a title changes.

Some apps annotate their own windows and need nothing from here: slk-gio (a Slack client) shows unread counts and the last message on its cards.

## Claude Code

`claude-code.sh` shows what a Claude Code session is doing on its terminal's card: working (with the tool it's using), waiting for you, or done.
It needs `jq`, and finds the terminal's window by walking up from Claude Code's process to the nearest process that owns a niri window.

Add it to the hooks in `~/.claude/settings.json`, with the path to the script:

```json
{
  "hooks": {
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "/path/to/claude-code.sh" }] }],
    "PreToolUse": [{ "matcher": "*", "hooks": [{ "type": "command", "command": "/path/to/claude-code.sh" }] }],
    "Notification": [{ "hooks": [{ "type": "command", "command": "/path/to/claude-code.sh" }] }],
    "Stop": [{ "hooks": [{ "type": "command", "command": "/path/to/claude-code.sh" }] }],
    "SessionEnd": [{ "hooks": [{ "type": "command", "command": "/path/to/claude-code.sh" }] }]
  }
}
```

It does nothing outside niri, and never prints anything, so it can't get in Claude Code's way.

## Audio

`audio.sh` badges the cards of windows that are playing sound: 🔊, or 🔇 if they're muted.
It needs `jq` and PipeWire's `pw-dump`, and runs for the whole session, so start it from the niri config:

```kdl
spawn-at-startup "/path/to/audio.sh"
```

It sleeps until PipeWire reports a change, then looks once at the streams that are playing and finds the window of each by walking up from the stream's process to the nearest process that owns a window (so a player started from a terminal badges that terminal).
Browsers play from one process for all their windows; the script then prefers the windows whose title contains what's playing, and otherwise badges all of them.
Set `AUDIO_PLAYING_BADGE` and `AUDIO_MUTED_BADGE` to change the badges, and `AUDIO_DRY_RUN=1` to print the niri commands instead of running them; `--once` looks once and exits.

## Chromium favicons

`chromium-favicon.sh` shows the favicon of the page each Chromium window is on as the icon of its card, instead of Chromium's.
It needs `jq` and `sqlite3`, and runs for the whole session, so start it from the niri config:

```kdl
spawn-at-startup "/path/to/chromium-favicon.sh"
```

Chromium doesn't tell niri what page a window is on, only its title, so the script looks the title up in each profile's history, then the page's favicon in the profile's favicon database, and writes it to `~/.cache/niri/favicons`.
A page without an icon of its own gets its site's; one that can't be found keeps Chromium's icon.
Chromium writes its history only every few seconds, so a page you just opened may take a few seconds to get its icon, and two pages with the same title may get each other's.
It reads Chromium, Google Chrome and Brave profiles; set `CHROMIUM_FAVICON_DIRS` (directories separated by colons) and `CHROMIUM_FAVICON_APP_IDS` (a regex) for other Chromium-based browsers, and `CHROMIUM_FAVICON_DRY_RUN=1` to print the niri commands instead of running them; `--once` looks once and exits.
