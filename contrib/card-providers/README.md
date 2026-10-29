# Card providers

Small programs that tell niri what to show on the cards of windows set aside in the strip, in i3 mode.
niri knows nothing about the apps: providers send it annotations with

```sh
niri msg action annotate-window --id <window> --source <name> \
    [--badge TEXT] [--status done|working|waiting|error] [--text TEXT] [--progress 0..1] [--ttl-ms MS]
niri msg action clear-annotations --id <window> [--source <name>]
```

Each provider uses its own `--source`, so several can annotate the same window.
`niri msg --json event-stream` tells providers when to act, without polling: `WindowCardChanged` when a window goes into or out of a strip, or its card is shown or hidden, and `WindowOpenedOrChanged` when a title changes.

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
