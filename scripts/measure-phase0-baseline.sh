#!/usr/bin/env bash
# Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.

set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
baseline_binary="$project_root/target/release/examples/phase0_baseline"
roe_binary="$project_root/target/release/roe"

cargo build \
    --release \
    --manifest-path "$project_root/Cargo.toml" \
    --package roe-terminal \
    --example phase0_baseline
cargo build --release --manifest-path "$project_root/Cargo.toml" --bin roe

if [[ -x /usr/bin/time ]]; then
    /usr/bin/time \
        --format='process_wall_seconds=%e\nprocess_max_rss_kib=%M' \
        "$baseline_binary"
else
    "$baseline_binary"
fi

if [[ -x /usr/bin/time ]]; then
    /usr/bin/time \
        --format='roe_cli_startup_seconds=%e\nroe_cli_max_rss_kib=%M' \
        "$roe_binary" --help >/dev/null
fi

if [[ -x /usr/bin/time ]] && command -v script >/dev/null && command -v timeout >/dev/null; then
    idle_metrics="$(mktemp)"
    TERM=xterm-256color script -qec \
        "/usr/bin/time --output=$idle_metrics --format='roe_terminal_idle_seconds=%e\nroe_terminal_idle_max_rss_kib=%M' timeout 1 $roe_binary" \
        /dev/null >/dev/null 2>&1 || true
    if [[ -s "$idle_metrics" ]]; then
        sed 's/^Command exited with non-zero status 124$//' "$idle_metrics"
    fi
    rm -f "$idle_metrics"
fi
