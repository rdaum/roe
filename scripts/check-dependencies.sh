#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"

PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_dependency_policy.py'
PYTHONDONTWRITEBYTECODE=1 python3 scripts/dependency_policy.py
cargo metadata --format-version 1 --locked --no-deps >/dev/null
