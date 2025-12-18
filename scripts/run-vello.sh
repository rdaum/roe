#!/usr/bin/env bash
#
# Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
#
# Run the Roe editor (Vello/GPU version) with Julia library path configured
#
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
JULIA_DIR="$PROJECT_ROOT/julia"

export LD_LIBRARY_PATH="$JULIA_DIR/lib:${LD_LIBRARY_PATH:-}"

# Quick check for required Julia packages (only installs if missing)
MARKER_FILE="$PROJECT_ROOT/.julia-packages-installed"
if [[ ! -f "$MARKER_FILE" ]]; then
    echo "First run: checking Julia packages..."
    "$JULIA_DIR/bin/julia" --project="$PROJECT_ROOT" -e '
        using Pkg
        try
            using JuliaSyntaxHighlighting
        catch
            println("Installing JuliaSyntaxHighlighting...")
            Pkg.add("JuliaSyntaxHighlighting")
        end
    ' && touch "$MARKER_FILE"
fi

exec cargo run --release --bin roe-vello --manifest-path "$PROJECT_ROOT/Cargo.toml" "$@"
