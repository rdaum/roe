# Toolchain and dependency policy

The workspace declares Rust 1.96 as its minimum supported Rust version (MSRV).
The development and CI toolchain is Rust 1.97.1.
An MSRV change requires matching changes to the workspace metadata, toolchain configuration, CI, and this document.

## Mica and Compio

The workspace pins Compio to `=0.18.0`.
The Mica manifests track `https://github.com/timbran-project/mica.git` without `rev`, `branch`, or `tag` selectors.
`Cargo.lock` fixes the resolved Mica commit.
`mica/MICA-REVISION` records that same full commit ID.

This lockfile policy preserves the manifest choice at commit `d1ca778`.
The code-health changes do not update the locked Mica commit.
All locked Mica crates must use the recorded commit.

An intentional Mica update includes the lockfile and revision record in the same change.
Its verification covers source compatibility, lifecycle, authority, replacement, recovery, bounds, cancellation, shutdown, terminal workflows, and both presentation consumers.
The dependency guard uses `--locked` metadata validation, so a stale lockfile fails the check.

The `mica-driver` declaration disables default features and enables only `source-provider`.
Member crates cannot re-enable default features or add driver acceleration features.
Roe keeps Mica WGPU acceleration, Fjall persistence, and Cranelift compilation disabled.

The source-provider feature includes the bounded provider contract and its Git/JJ, tree-sitter, and Tokio dependencies.
Roe configures local-worktree access and its live-buffer overlay.
The `mica-external-http` crate supplies the HTTP and streaming handlers for the workspace agent.

## Update groups

| Group | Dependencies | Verification |
| --- | --- | --- |
| Native and terminal libraries | Arboard, Crossterm, Notify, Ropey, Similar, SlotMap | MSRV, native mechanisms, terminal workflows |
| Runtime | Compio and the locked Mica crates | Mica lifecycle, authority, replacement, cancellation, and session tests |
| Graphics | Vello, its WGPU graph, Parley, Winit, Pollster | A coordinated upgrade, Vello build, and frontend conformance |
| Text realization | Unicode Segmentation, Unicode Width | Terminal controls, width, clipping, cursor, and pointer regressions |

External dependency declarations belong in `Cargo.toml` under `workspace.dependencies`.
Member manifests inherit them with `workspace = true`.
This requirement includes development, build, and target-specific dependencies.
Local Roe crate relationships retain path declarations.

## Required checks

Run `./scripts/check.sh` before a commit.
The dependency check uses Python 3.11 or later and its built-in TOML parser.
It reads manifests and the lockfile, compares the revision record, and reports each policy mismatch.
The same check runs its parser-policy regression tests.

For the advisory check, install `cargo-audit`.
Then run `./scripts/check-security.sh`.
CI runs that command too.
An advisory exception requires a documented reason and review deadline.

## Advisory exceptions

The recorded audit on 2026-08-13 found no vulnerabilities.
It found two unmaintained-crate warnings without a compatible patched release.
The security check ignores only these advisory IDs and denies other warning categories.
The exception review deadline is 2026-11-13.

| Advisory | Dependency path | Recorded reason |
| --- | --- | --- |
| `RUSTSEC-2024-0436` | Compio Driver → Paste 1.0.15 | Compio 0.18 requires the final Paste release. |
| `RUSTSEC-2026-0192` | Winit → Ab Glyph → TTF Parser 0.25.1 | The Winit 0.30 graph has no maintained compatible replacement. |
