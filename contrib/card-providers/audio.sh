#!/usr/bin/env bash
# Shows which windows are playing sound on their cards in the niri strip.
#
# Runs for the whole session, e.g. from niri's spawn-at-startup. It sleeps until PipeWire reports
# a change, then badges the windows of the streams that are playing. See README.md.

set -u

[ -n "${NIRI_SOCKET:-}" ] || exit 0
command -v jq >/dev/null || exit 0
command -v pw-dump >/dev/null || exit 0

PLAYING_BADGE=${AUDIO_PLAYING_BADGE:-🔊}
MUTED_BADGE=${AUDIO_MUTED_BADGE:-🔇}
# Set to print the niri commands instead of running them.
DRY_RUN=${AUDIO_DRY_RUN:-}

niri_action() {
    if [ -n "$DRY_RUN" ]; then
        echo "niri msg action $*"
    else
        niri msg action "$@" >/dev/null 2>&1
    fi
}

# The badge shown on each window, by window id, as of the last update.
declare -A shown=()

# The parent of a process.
parent_of() {
    local stat
    stat=$(<"/proc/$1/stat") 2>/dev/null || return 1
    # The name in brackets may hold spaces; the parent comes right after the state.
    stat=${stat##*) }
    set -- $stat
    echo "$2"
}

update() {
    local windows streams
    windows=$(niri msg --json windows 2>/dev/null) || return
    # Playing streams: process, media title, and whether muted.
    # The process is on the stream's client, or on the stream itself.
    streams=$(pw-dump 2>/dev/null | jq -r '
        (map(select(.type == "PipeWire:Interface:Client")
            | {key: (.id | tostring), value: .info.props["application.process.id"]})
            | from_entries) as $clients
        | .[]
        | select(.type == "PipeWire:Interface:Node")
        | select(.info.props["media.class"] == "Stream/Output/Audio")
        | select(.info.state == "running")
        | [
            ((.info.props["application.process.id"]
                // $clients[(.info.props["client.id"] // "") | tostring]
                // "") | tostring),
            ((.info.params.Props // []) | map(.mute // false) | any | tostring),
            (.info.props["media.name"] // "" | gsub("\u001f"; " "))
          ]
        | join("\u001f")') || return

    declare -A playing=() unmuted=()
    local pid muted media ids id
    while IFS=$'\x1f' read -r pid muted media; do
        [ -n "$pid" ] || continue
        # The nearest process up the tree that owns windows.
        ids=""
        while [ -n "$pid" ] && [ "$pid" -gt 1 ]; do
            ids=$(jq -r --argjson pid "$pid" --arg media "$media" '
                map(select(.pid == $pid)) as $own
                # Several windows of one process: prefer those showing what plays.
                | ($own | map(select($media != ""
                    and ((.title // "") | ascii_downcase | contains($media | ascii_downcase)))))
                    as $titled
                | (if ($titled | length) > 0 then $titled else $own end)
                | .[].id' <<<"$windows")
            [ -n "$ids" ] && break
            pid=$(parent_of "$pid") || break
        done
        for id in $ids; do
            playing[$id]=1
            [ "$muted" = "true" ] || unmuted[$id]=1
        done
    done <<<"$streams"

    local badge
    for id in "${!playing[@]}"; do
        if [ -n "${unmuted[$id]:-}" ]; then badge=$PLAYING_BADGE; else badge=$MUTED_BADGE; fi
        if [ "${shown[$id]:-}" != "$badge" ]; then
            niri_action annotate-window --id "$id" --source audio --badge "$badge"
            shown[$id]=$badge
        fi
    done
    for id in "${!shown[@]}"; do
        if [ -z "${playing[$id]:-}" ]; then
            niri_action clear-annotations --id "$id" --source audio
            unset "shown[$id]"
        fi
    done
}

clear_all() {
    local id
    for id in "${!shown[@]}"; do
        niri_action clear-annotations --id "$id" --source audio
    done
    exit 0
}
trap clear_all INT TERM HUP

if [ "${1:-}" = "--once" ]; then
    update
    exit 0
fi

update
# Wake on PipeWire changes; let a burst of them settle, then look once.
exec 3< <(pw-dump --monitor --no-colors 2>/dev/null)
while IFS= read -r _ <&3; do
    while IFS= read -r -t 0.2 _ <&3; do :; done
    update
done
clear_all
