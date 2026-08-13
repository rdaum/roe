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
    printf -v command 'cd %q; exec %q ' "$project_root" "$roe_binary"
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
tmux -L "$tmux_socket" send-keys -t windows C-x o
tmux -L "$tmux_socket" send-keys -t windows C-x 0
finish_session windows

# A notify event must wake the terminal loop and update the buffer without input.
watch_file="$probe_dir/watch.txt"
printf 'before\n' >"$watch_file"
start_session watcher "$watch_file"
printf 'after\n' >"$watch_file"
sleep 1.2
tmux -L "$tmux_socket" send-keys -t watcher -l X
tmux -L "$tmux_socket" send-keys -t watcher C-x C-s
finish_session watcher
[[ "$(sed -n '1p' "$watch_file")" == 'Xafter' ]]

printf '%s\n' 'phase0_terminal_workflows=pass'
