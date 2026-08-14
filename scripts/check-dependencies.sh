#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"

grep -q '^compio = { version = "=0\.18\.0",' Cargo.toml
grep -q '^mica-compiler = { git = "https://github.com/timbran-project/mica.git", rev = "bea4fbd786e2601af1a682bd723b527000979f70" }' Cargo.toml
grep -q '^mica-driver = { git = "https://github.com/timbran-project/mica.git", rev = "bea4fbd786e2601af1a682bd723b527000979f70", default-features = false }' Cargo.toml

direct_declarations="$(
    rg -n '^(arboard|compio|crossterm|mica-compiler|mica-driver|notify|ropey|signal-hook|similar|slotmap|thiserror|tracing|tracing-subscriber)\s*=' \
        roe/Cargo.toml roe-core/Cargo.toml roe-terminal/Cargo.toml roe-vello/Cargo.toml \
        | rg -v '\{ workspace = true \}' || true
)"
if [[ -n "$direct_declarations" ]]; then
    printf '%s\n' "$direct_declarations" >&2
    printf '%s\n' 'direct dependency versions must be declared in [workspace.dependencies]' >&2
    exit 1
fi

cargo metadata --format-version 1 --locked --no-deps >/dev/null
