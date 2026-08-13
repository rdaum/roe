# Toolchain and dependency policy

Roe's declared minimum supported Rust version (MSRV) is 1.88. The repository pins Rust 1.97.1 in
`rust-toolchain.toml` for reproducible development and CI. A dependency update may raise the MSRV
only in a dedicated commit that updates both this document and workspace metadata.

## Update groups

The Phase 1 inventory was captured on 2026-08-13 with `cargo outdated`, `cargo tree -d`, crate
metadata, and upstream release notes.

| Group              | Dependencies                                                               | Policy                                                                                              |
| ------------------ | -------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| Routine compatible | `arboard`, `crossterm`, `futures`, `notify`, `ropey`, `similar`, `slotmap` | Follow current stable releases compatible with the MSRV.                                            |
| Runtime pin        | `compio = 0.18.0`                                                          | Exact pin shared with Mica. Change only together with the pinned Mica revision and lifecycle tests. |
| Coupled graphics   | `vello`, its WGPU graph, `parley`, `winit`, `pollster`                     | Upgrade as one reviewed group with Vello build and frontend conformance checks.                     |
| Removed            | `async-trait`                                                              | Unused Julia-era dependency removed in Phase 1.                                                     |

Ropey 2 is currently a prerelease and is not treated as the current stable target. Winit 0.31 is
also prerelease. Mica's first integration must use `default-features = false`, leaving its WGPU
relation accelerator disabled until Roe and Mica intentionally choose a device/version strategy.

## Required checks

Run `./scripts/check.sh` before committing. `./scripts/check-dependencies.sh` verifies the exact
Compio pin and rejects duplicate direct dependency declarations outside workspace policy. CI also
runs `cargo audit` against the lockfile. Advisory exceptions require a documented reason and expiry;
none are accepted by default.
