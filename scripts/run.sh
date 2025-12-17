#!/usr/bin/env bash
#
# Run the Roe editor with Julia library path configured
#
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
JULIA_DIR="$PROJECT_ROOT/julia"

export LD_LIBRARY_PATH="$JULIA_DIR/lib:${LD_LIBRARY_PATH:-}"

exec cargo run --release --manifest-path "$PROJECT_ROOT/Cargo.toml" "$@"
