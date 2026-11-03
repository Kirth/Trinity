#!/usr/bin/env bash
# Shows the favicon of the page a Chromium window is on as the icon of its card in the niri strip.
#
# Runs for the whole session, e.g. from niri's spawn-at-startup. It finds the page by the window's
# title in Chromium's history, and its favicon in Chromium's favicon database. See README.md.

set -u

[ -n "${NIRI_SOCKET:-}" ] || exit 0
command -v jq >/dev/null || exit 0
command -v sqlite3 >/dev/null || exit 0

# Windows of these app ids are Chromium's (a regex, matched ignoring case).
APP_IDS=${CHROMIUM_FAVICON_APP_IDS:-'^(chromium|chromium-browser|google-chrome|brave-browser)$'}
config=${XDG_CONFIG_HOME:-$HOME/.config}
# Chromium's config directories, separated by colons; each holds a directory per profile.
DIRS=${CHROMIUM_FAVICON_DIRS:-$config/chromium:$config/google-chrome:$config/BraveSoftware/Brave-Browser}
CACHE=${XDG_CACHE_HOME:-$HOME/.cache}/niri/favicons
# Set to print the niri commands instead of running them.
DRY_RUN=${CHROMIUM_FAVICON_DRY_RUN:-}

# Chromium commits its history every few seconds, so a page that just opened may not be in it
# yet: look for it up to this many times, every RETRY_SECONDS.
RETRIES=6
RETRY_SECONDS=5

niri_action() {
    if [ -n "$DRY_RUN" ]; then
        echo niri msg action "$@"
    else
        niri msg action "$@" >/dev/null 2>&1
    fi
}

# Profile directories: those with both databases.
profiles=()
IFS=: read -ra dirs <<<"$DIRS"
for dir in "${dirs[@]}"; do
    for profile in "$dir"/*/; do
        profile=${profile%/}
        [ -f "$profile/History" ] && [ -f "$profile/Favicons" ] && profiles+=("$profile")
    done
done
[ "${#profiles[@]}" -gt 0 ] || exit 0

mkdir -p "$CACHE" || exit 0
find "$CACHE" -name '*.png' -mtime +30 -delete 2>/dev/null

# Quotes text for SQL.
sql_quote() {
    printf "'%s'" "${1//\'/\'\'}"
}

# Titles in history may start with a count, like "(4) ", that comes and goes with the page.
strip_count() {
    local col=$1
    echo "CASE WHEN $col GLOB '([0-9]*) *' THEN substr($col, instr($col, ') ') + 2) ELSE $col END"
}

# Prints the path of the favicon of the page with this title, written to the cache, if found.
# The databases are read as they are on disk, since Chromium keeps them locked.
favicon_for() {
    local title=$1 profile db url origin row bitmap updated path
    [[ $title =~ ^\(([0-9]+)\)\ (.*)$ ]] && title=${BASH_REMATCH[2]}
    for profile in "${profiles[@]}"; do
        db="file:$profile/History?immutable=1"
        url=$(sqlite3 "$db" "SELECT url FROM urls
            WHERE $(strip_count title) = $(sql_quote "$title")
            ORDER BY last_visit_time DESC LIMIT 1" 2>/dev/null) || continue
        [ -n "$url" ] || continue

        # Pages without an icon of their own get their site's, like most of YouTube's.
        origin=""
        [[ $url =~ ^(https?://[^/]+)/ ]] && origin=${BASH_REMATCH[1]}
        row=$(sqlite3 -separator ' ' "$db" "
            ATTACH $(sql_quote "file:$profile/Favicons?immutable=1") AS f;
            SELECT b.id, b.last_updated FROM f.icon_mapping m
            JOIN f.favicon_bitmaps b ON b.icon_id = m.icon_id
            WHERE m.page_url = $(sql_quote "$url")
                OR ($(sql_quote "$origin") != ''
                    AND substr(m.page_url, 1, $((${#origin} + 1))) = $(sql_quote "$origin/"))
            ORDER BY m.page_url = $(sql_quote "$url") DESC, b.width DESC, b.last_updated DESC
            LIMIT 1" 2>/dev/null) || continue
        read -r bitmap updated <<<"$row"
        [ -n "$bitmap" ] || continue

        # A new name whenever the image changes, since niri keeps what it loaded by path.
        path="$CACHE/${profile//[^A-Za-z0-9]/_}-$bitmap-$updated.png"
        if [ ! -s "$path" ]; then
            sqlite3 "$db" "
                ATTACH $(sql_quote "file:$profile/Favicons?immutable=1") AS f;
                SELECT writefile($(sql_quote "$path.tmp"), image_data)
                FROM f.favicon_bitmaps WHERE id = $bitmap" >/dev/null 2>&1 &&
                mv -f "$path.tmp" "$path" || continue
        fi
        echo "$path"
        return 0
    done
    return 1
}

# The title looked up and icon shown for each window, by window id, and how many more times to
# look for windows whose page wasn't found.
declare -A titles=() shown=() pending=()

update() {
    local id=$1 title=$2 page icon
    if [ "${titles[$id]-}" != "$title" ]; then
        titles[$id]=$title
        pending[$id]=$RETRIES
    elif [ -z "${pending[$id]:-}" ]; then
        return
    fi
    # The title ends with the browser's name, and maybe the profile's.
    page=${title% - *}
    icon=$(favicon_for "$page") || icon=$(favicon_for "${page% - *}") || icon=""

    pending[$id]=$((pending[$id] - 1))
    [ "${pending[$id]}" -gt 0 ] || unset "pending[$id]"

    if [ -n "$icon" ]; then
        unset "pending[$id]"
        if [ "${shown[$id]:-}" != "$icon" ]; then
            niri_action annotate-window --id "$id" --source chromium-favicon --icon "$icon"
            shown[$id]=$icon
        fi
        return
    fi

    # Better the app icon than the last page's.
    if [ -n "${shown[$id]:-}" ]; then
        niri_action clear-annotations --id "$id" --source chromium-favicon
        unset "shown[$id]"
    fi
}

forget() {
    unset "titles[$1]" "shown[$1]" "pending[$1]"
}

clear_all() {
    local id
    for id in "${!shown[@]}"; do
        niri_action clear-annotations --id "$id" --source chromium-favicon
    done
    exit 0
}
trap clear_all INT TERM HUP

# Lines of "open<US>id<US>title" for Chromium windows and "closed<US>id" for closed windows.
# shellcheck disable=SC2016
filter='
    def chromium: (.app_id // "") | test($apps; "i");
    def open: select(chromium) | ["open", .id, (.title // "" | gsub("\u001f"; " "))] | join("\u001f");
    if .WindowsChanged then .WindowsChanged.windows[] | open
    elif .WindowOpenedOrChanged then .WindowOpenedOrChanged.window
        | if chromium then open else ["closed", .id] | join("\u001f") end
    elif .WindowClosed then ["closed", .WindowClosed.id] | join("\u001f")
    else empty end'

handle() {
    local kind id title
    IFS=$'\x1f' read -r kind id title <<<"$1"
    case $kind in
        open) update "$id" "$title" ;;
        closed) forget "$id" ;;
    esac
}

if [ "${1:-}" = "--once" ]; then
    while IFS= read -r line; do
        handle "$line"
    done < <(niri msg --json windows 2>/dev/null |
        jq -c '{WindowsChanged: {windows: .}}' | jq -r --arg apps "$APP_IDS" "$filter")
    exit 0
fi

exec 3< <(niri msg --json event-stream 2>/dev/null |
    jq --unbuffered -r --arg apps "$APP_IDS" "$filter" 2>/dev/null)
next_retry=$((SECONDS + RETRY_SECONDS))
while true; do
    IFS= read -r -t "$RETRY_SECONDS" line <&3
    status=$?
    if [ "$status" -eq 0 ]; then
        handle "$line"
    elif [ "$status" -le 128 ]; then
        # The stream ended.
        break
    fi
    if [ "$SECONDS" -ge "$next_retry" ]; then
        for id in "${!pending[@]}"; do
            update "$id" "${titles[$id]}"
        done
        next_retry=$((SECONDS + RETRY_SECONDS))
    fi
done
clear_all
