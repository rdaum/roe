#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"

grep -q '^compio = { version = "=0\.18\.0",' Cargo.toml

# compio-buf 0.8.2+ uses a standard-library API unavailable on Roe's Rust 1.88
# MSRV. Keep the compatible transitive selected until Compio's shared pin moves.
awk '
    $0 == "name = \"compio-buf\"" { in_compio_buf = 1; next }
    in_compio_buf && $0 == "version = \"0.8.1\"" { found = 1 }
    in_compio_buf && /^\[\[package\]\]$/ { in_compio_buf = 0 }
    END { exit(found ? 0 : 1) }
' Cargo.lock

direct_declarations="$(
    rg -n '^(arboard|compio|crossterm|notify|ropey|signal-hook|similar|slotmap|thiserror|tracing|tracing-subscriber)\s*=' \
        roe/Cargo.toml roe-core/Cargo.toml roe-terminal/Cargo.toml roe-vello/Cargo.toml \
        | rg -v '\{ workspace = true \}' || true
)"
if [[ -n "$direct_declarations" ]]; then
    printf '%s\n' "$direct_declarations" >&2
    printf '%s\n' 'direct dependency versions must be declared in [workspace.dependencies]' >&2
    exit 1
fi

cargo metadata --format-version 1 --locked --no-deps >/dev/null
