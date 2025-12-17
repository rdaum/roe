#!/usr/bin/env bash
# Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
#
#
# Run the Roe editor with Julia library path configured
#
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
JULIA_DIR="$PROJECT_ROOT/julia"

export LD_LIBRARY_PATH="$JULIA_DIR/lib:${LD_LIBRARY_PATH:-}"

exec cargo run --release --bin roe --manifest-path "$PROJECT_ROOT/Cargo.toml" "$@"
