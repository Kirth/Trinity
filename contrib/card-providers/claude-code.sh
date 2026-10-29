#!/usr/bin/env bash
# Shows what a Claude Code session is up to on its terminal's card in the niri strip.
#
# Run it from Claude Code hooks: it reads the hook's JSON on stdin. See README.md.

set -u
exec >/dev/null 2>&1

[ -n "${NIRI_SOCKET:-}" ] || exit 0
command -v jq >/dev/null || exit 0

input=$(cat)
event=$(jq -r '.hook_event_name // empty' <<<"$input")
session=$(jq -r '.session_id // "unknown"' <<<"$input")
[ -n "$event" ] || exit 0

cache_dir="${XDG_RUNTIME_DIR:-/tmp}/niri-claude-cards"
cache="$cache_dir/$session"

# The niri window of the terminal running this session: the nearest ancestor process that owns
# a window.
find_window() {
    local windows pid id
    windows=$(niri msg --json windows) || return 1
    pid=$PPID
    while [ "$pid" -gt 1 ]; do
        id=$(jq -r --argjson pid "$pid" 'map(select(.pid == $pid)) | first | .id // empty' \
            <<<"$windows")
        if [ -n "$id" ]; then
            echo "$id"
            return 0
        fi
        pid=$(awk '{print $4}' "/proc/$pid/stat" 2>/dev/null) || return 1
    done
    return 1
}

window=""
[ -r "$cache" ] && window=$(cat "$cache")
if [ -z "$window" ] || ! niri msg --json windows | jq -e --argjson id "$window" \
    'any(.id == $id)' >/dev/null; then
    window=$(find_window) || exit 0
    mkdir -p "$cache_dir" && echo "$window" >"$cache"
fi

annotate() {
    niri msg action annotate-window --id "$window" --source claude "$@"
}

case "$event" in
    UserPromptSubmit)
        annotate --status working --text "Working"
        ;;
    PreToolUse)
        tool=$(jq -r '.tool_name // "a tool"' <<<"$input")
        annotate --status working --text "Using $tool"
        ;;
    Notification)
        message=$(jq -r '.message // "Needs you"' <<<"$input")
        annotate --status waiting --text "$message"
        ;;
    Stop)
        annotate --status done --text "Done"
        ;;
    SessionEnd)
        niri msg action clear-annotations --id "$window" --source claude
        rm -f "$cache"
        ;;
esac

exit 0
