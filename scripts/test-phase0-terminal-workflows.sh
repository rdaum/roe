#!/usr/bin/env bash
# Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.

set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
roe_binary="$project_root/target/release/roe"
probe_dir="$(mktemp -d)"
tmux_socket="roe-phase0-$$"
cleanup() {
    tmux -L "$tmux_socket" kill-server 2>/dev/null || true
    rm -rf "$probe_dir"
}
trap cleanup EXIT

cargo build --release --manifest-path "$project_root/Cargo.toml" --bin roe

start_session() {
    local name="$1"
    shift

    local command
    printf -v command 'cd %q; exec env -u DISPLAY -u WAYLAND_DISPLAY %q ' \
        "$probe_dir" "$roe_binary"
    local argument quoted_argument
    for argument in "$@"; do
        printf -v quoted_argument '%q' "$argument"
        command+="$quoted_argument "
    done
    tmux -L "$tmux_socket" new-session -d -s "$name" -x 80 -y 24 "$command"
    sleep 0.5
}

finish_session() {
    local name="$1"
    sleep 0.3
    tmux -L "$tmux_socket" send-keys -t "$name" C-x C-c
    for _ in $(seq 1 30); do
        if ! tmux -L "$tmux_socket" has-session -t "$name" 2>/dev/null; then
            return 0
        fi
        sleep 0.1
    done
    tmux -L "$tmux_socket" capture-pane -p -t "$name" >&2 || true
    return 1
}

# Startup and clean C-x C-c shutdown with no file.
start_session no-file
finish_session no-file

# One-file startup, insertion, save, and clean shutdown.
one_file="$probe_dir/one.txt"
printf 'alpha\n' >"$one_file"
start_session one-file "$one_file"
tmux -L "$tmux_socket" send-keys -t one-file -l Z
tmux -L "$tmux_socket" send-keys -t one-file C-x C-s
finish_session one-file
[[ "$(sed -n '1p' "$one_file")" == 'Zalpha' ]]

# Movement, a multibyte region kill, undo, yank, and insertion must all retain
# character-index cursor semantics. Display access is removed above so this
# cannot touch the user's system clipboard.
unicode_file="$probe_dir/unicode.txt"
printf 'éx\n' >"$unicode_file"
start_session unicode-edit "$unicode_file"
tmux -L "$tmux_socket" send-keys -t unicode-edit C-Space Right C-w C-_ End C-y
tmux -L "$tmux_socket" send-keys -t unicode-edit -l Z
tmux -L "$tmux_socket" send-keys -t unicode-edit C-x C-s
finish_session unicode-edit
[[ "$(sed -n '1p' "$unicode_file")" == 'éxéZ' ]]

# Two-file startup, window selection, edit of the selected buffer, and save.
first_file="$probe_dir/first.txt"
second_file="$probe_dir/second.txt"
printf 'first\n' >"$first_file"
printf 'second\n' >"$second_file"
start_session two-files "$first_file" "$second_file"
tmux -L "$tmux_socket" send-keys -t two-files C-x o
tmux -L "$tmux_socket" send-keys -t two-files -l Q
tmux -L "$tmux_socket" send-keys -t two-files C-x C-s
finish_session two-files
[[ "$(sed -n '1p' "$first_file")" == 'first' ]]
[[ "$(sed -n '1p' "$second_file")" == 'Qsecond' ]]

# Buffer selection filters the production menu and switches the active buffer.
printf 'first\n' >"$first_file"
printf 'second\n' >"$second_file"
start_session buffer-select "$first_file" "$second_file"
tmux -L "$tmux_socket" send-keys -t buffer-select C-x b
tmux -L "$tmux_socket" send-keys -t buffer-select -l second.txt
tmux -L "$tmux_socket" send-keys -t buffer-select Enter
tmux -L "$tmux_socket" send-keys -t buffer-select -l B
tmux -L "$tmux_socket" send-keys -t buffer-select C-x C-s
finish_session buffer-select
[[ "$(sed -n '1p' "$second_file")" == 'Bsecond' ]]

# File selection opens a listed file from the controlled working directory.
selected_file="$probe_dir/selected.txt"
printf 'selected\n' >"$selected_file"
start_session file-select
tmux -L "$tmux_socket" send-keys -t file-select C-x C-f
tmux -L "$tmux_socket" send-keys -t file-select -l selected.txt
tmux -L "$tmux_socket" send-keys -t file-select Enter End
tmux -L "$tmux_socket" send-keys -t file-select -l F
tmux -L "$tmux_socket" send-keys -t file-select C-x C-s
finish_session file-select
[[ "$(sed -n '1p' "$selected_file")" == 'selectedF' ]]

# Incremental search moves to a Unicode-safe character position before editing.
search_file="$probe_dir/search.txt"
printf 'λ alpha beta\n' >"$search_file"
start_session isearch "$search_file"
tmux -L "$tmux_socket" send-keys -t isearch C-s
tmux -L "$tmux_socket" send-keys -t isearch -l beta
tmux -L "$tmux_socket" send-keys -t isearch Enter
tmux -L "$tmux_socket" send-keys -t isearch -l X
tmux -L "$tmux_socket" send-keys -t isearch C-x C-s
finish_session isearch
[[ "$(sed -n '1p' "$search_file")" == 'λ alpha Xbeta' ]]

# Split, select, and delete a window through the production terminal adapter.
window_file="$probe_dir/window.txt"
printf 'window\n' >"$window_file"
start_session windows "$window_file"
tmux -L "$tmux_socket" send-keys -t windows C-x 2
sleep 0.2
before_resize_row="$(
    tmux -L "$tmux_socket" capture-pane -p -t windows \
        | awk '/window.txt/ { print NR; exit }'
)"
tmux -L "$tmux_socket" send-keys -t windows -l \
    $'\e[<0;40;11M\e[<32;40;19M\e[<0;40;19m'
sleep 0.2
after_resize_row="$(
    tmux -L "$tmux_socket" capture-pane -p -t windows \
        | awk '/window.txt/ { print NR; exit }'
)"
[[ -n "$before_resize_row" && -n "$after_resize_row" ]]
((after_resize_row > before_resize_row))
tmux -L "$tmux_socket" send-keys -t windows C-x o
tmux -L "$tmux_socket" send-keys -t windows C-x 0
finish_session windows

# Command selection executes a named command through M-x.
command_file="$probe_dir/command.txt"
printf 'command\n' >"$command_file"
start_session command-select "$command_file"
tmux -L "$tmux_socket" send-keys -t command-select M-x
tmux -L "$tmux_socket" send-keys -t command-select -l split-window-horizontally
tmux -L "$tmux_socket" send-keys -t command-select Enter C-x o End
tmux -L "$tmux_socket" send-keys -t command-select -l M
tmux -L "$tmux_socket" send-keys -t command-select C-x C-s
finish_session command-select
[[ "$(sed -n '1p' "$command_file")" == 'commandM' ]]

# Record the current notify/event-loop behavior. The delivered event remains
# unapplied while idle and is processed when the next terminal event arrives.
watch_file="$probe_dir/watch.txt"
printf 'before\n' >"$watch_file"
start_session watcher "$watch_file"
printf 'after\n' >"$watch_file"
sleep 1.2
watcher_pane="$(tmux -L "$tmux_socket" capture-pane -p -t watcher)"
[[ "$watcher_pane" == *before* ]]
tmux -L "$tmux_socket" send-keys -t watcher -l X
tmux -L "$tmux_socket" send-keys -t watcher C-x C-s
finish_session watcher
[[ "$(sed -n '1p' "$watch_file")" == 'Xafter' ]]

# Record the current forced-shutdown behavior without risking the user's
# terminal. A wrapper remains in the owned pane after Roe receives SIGTERM and
# records the pseudo-terminal state Roe left behind.
forced_state="$probe_dir/forced-stty.txt"
forced_initial_state="$probe_dir/forced-stty-initial.txt"
forced_marker="$probe_dir/forced-stty-ready"
printf -v forced_command \
    'cd %q; stty -g >%q; env -u DISPLAY -u WAYLAND_DISPLAY %q; stty -g >%q; : >%q; sleep 5' \
    "$probe_dir" "$forced_initial_state" "$roe_binary" "$forced_state" "$forced_marker"
tmux -L "$tmux_socket" new-session -d -s forced-shutdown -x 80 -y 24 "$forced_command"
sleep 0.5
forced_shell_pid="$(
    tmux -L "$tmux_socket" display-message -p -t forced-shutdown '#{pane_pid}'
)"
forced_roe_pid="$(pgrep -P "$forced_shell_pid" -x roe | head -n 1)"
[[ -n "$forced_roe_pid" ]]
kill -TERM "$forced_roe_pid"
for _ in $(seq 1 30); do
    if [[ -f "$forced_marker" ]]; then
        break
    fi
    sleep 0.1
done
[[ -f "$forced_marker" ]]
cmp "$forced_initial_state" "$forced_state"
tmux -L "$tmux_socket" kill-session -t forced-shutdown

printf '%s\n' 'phase0_terminal_workflows=pass'
printf '%s\n' 'phase1_forced_shutdown=terminal_restored'
