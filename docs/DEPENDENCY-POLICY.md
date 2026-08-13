# Toolchain and dependency policy

Roe's declared minimum supported Rust version (MSRV) is 1.95. The repository pins Rust 1.97.1 in
`rust-toolchain.toml` for reproducible development and CI. Phase 4 raised the MSRV from 1.88 in the
same dedicated cutover that pinned the Mica driver; later raises must likewise update this document,
workspace metadata, and CI together.

## Update groups

The Phase 1 inventory was captured on 2026-08-13 with `cargo outdated`, `cargo tree -d`, crate
metadata, and upstream release notes.

| Group              | Dependencies                                                    | Policy                                                                                                         |
| ------------------ | --------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| Routine compatible | `arboard`, `crossterm`, `notify`, `ropey`, `similar`, `slotmap` | Follow current stable releases compatible with the MSRV.                                                       |
| Runtime pins       | `compio = 0.18.0`, exact `mica-driver` revision                 | Change together with Mica lifecycle, replacement, cancellation, and terminal workflow tests.                   |
| Mica features      | `mica-driver` with `default-features = false`                   | Keep CPU relation execution; do not initialize Mica WGPU, Fjall, Cranelift, or source-provider feature graphs. |
| Coupled graphics   | `vello`, its WGPU graph, `parley`, `winit`, `pollster`          | Upgrade as one reviewed group with Vello build and frontend conformance checks.                                |
| Removed            | `async-trait`, direct `futures`                                 | Unused actor/event-stream dependencies removed in Phase 1.                                                     |

Ropey 2 is currently a prerelease and is not treated as the current stable target. Winit 0.31 is
also prerelease. Mica's first integration must use `default-features = false`, leaving its WGPU
relation accelerator disabled until Roe and Mica intentionally choose a device/version strategy.

## Required checks

Run `./scripts/check.sh` before committing. `./scripts/check-dependencies.sh` verifies the exact
Compio and Mica pins and rejects duplicate direct dependency declarations outside workspace policy.
Install `cargo-audit` and run `./scripts/check-security.sh` for the advisory check; CI runs the same
command. Exceptions require a documented reason and review deadline.

## Advisory exceptions

The 2026-08-13 audit found no vulnerabilities. It found two unmaintained-crate warnings with no
patched compatible release. The security check ignores exactly these advisory IDs and denies every
other warning category. Review both exceptions no later than 2026-11-13.

| Advisory            | Dependency path                              | Reason for temporary exception                                      |
| ------------------- | -------------------------------------------- | ------------------------------------------------------------------- |
| `RUSTSEC-2024-0436` | `compio-driver` -> `paste 1.0.15`            | Upstream Compio 0.18 transitively requires the final paste release. |
| `RUSTSEC-2026-0192` | `winit` -> `ab_glyph` -> `ttf-parser 0.25.1` | Current Winit 0.30 graph has no maintained compatible replacement.  |
