#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"

if ! command -v cargo-audit >/dev/null 2>&1; then
    printf '%s\n' 'cargo-audit is required: cargo install cargo-audit --locked' >&2
    exit 1
fi

# These two unmaintained transitive crates have no patched compatible release.
# Their ownership paths, rationale, and review deadline are recorded in
# docs/DEPENDENCY-POLICY.md. Any other warning or vulnerability fails the check.
cargo audit --deny warnings \
    --ignore RUSTSEC-2024-0436 \
    --ignore RUSTSEC-2026-0192
