# Phase 0 baseline

This document records the state against which the Roe renovation is measured. It is evidence for
Phase 0 of [ROE-MICA-ROADMAP.md](../ROE-MICA-ROADMAP.md), not a claim that the current architecture
should be preserved.

Baseline captured on 2026-08-13 at commit `b9fd85f`, plus the baseline harness introduced with this
document.

## Workspace and execution surfaces

Roe is a Cargo workspace with four members:

| Member         | Role                                                      | Executable surface |
| -------------- | --------------------------------------------------------- | ------------------ |
| `roe-core`     | Buffers, editor state, modes, commands, windows, and undo | Library            |
| `roe-terminal` | Crossterm presentation and terminal event loop            | Library            |
| `roe-vello`    | Winit/Vello graphical frontend                            | `roe-vello`        |
| `roe`          | Terminal application assembly                             | `roe`              |

There are no workspace feature flags. The Mica integration will add a deliberately selected
`mica-driver` feature set rather than inherit its defaults.

The project does not currently declare an MSRV or supported-platform policy. This baseline was
verified on:

- Linux `6.17.0-1026-nvidia`, `aarch64-unknown-linux-gnu`;
- 20 ARM CPU cores (10 Cortex-X925 and 10 Cortex-A725); and
- Rust/Cargo `1.97.1`.

Terminal use depends on Crossterm support. The Vello frontend depends on a Winit window system and a
Vello/WGPU-compatible graphics device. Other operating systems are plausible but unverified.

## Direct dependency inventory

Resolved direct versions at the baseline are:

| Area       | Dependencies                                                                                   |
| ---------- | ---------------------------------------------------------------------------------------------- |
| Core       | `arboard 3.6.1`, `async-trait 0.1.92`, `compio 0.18.0`, `futures 0.3.34`, `notify 8.2.0`       |
| Text/state | `ropey 1.6.1`, `similar 2.7.0`, `slotmap 1.1.1`                                                |
| Terminal   | `crossterm 0.28.1`                                                                             |
| Graphical  | `vello 0.6.0`, `winit 0.30.13`, `parley 0.7.0`, `pollster 0.4.0`; Vello resolves WGPU `26.0.1` |

Compio is pinned exactly at `0.18.0` and currently matches Mica. Mica's optional relation
acceleration uses WGPU 30, so the first integration must use `mica-driver` with default features
disabled and relation acceleration disabled.

The dependency-renewal phase must regenerate this inventory with `cargo tree --workspace --depth 1`
and an installed `cargo outdated`; this table does not assert that any version is current upstream.

## Verification commands

The stable baseline commands are:

```sh
cargo check --workspace
cargo test --workspace -- --test-threads=8
cargo test -p roe-terminal --test renderer_conformance
cargo test -p roe-vello --test renderer_conformance
./scripts/measure-phase0-baseline.sh
```

At capture time:

- `cargo check --workspace` passes;
- `cargo test --workspace -- --test-threads=8` passes 116 tests;
- both frontends pass the shared renderer dirty-lifecycle conformance test; and
- kill-ring and editor tests use an injected clipboard boundary and do not touch the user's system
  clipboard.

Tests that construct Compio runtimes are serialized inside the `roe-core` test process. Before that
isolation, a parallel workspace run could fail with `EPERM` while creating four runtimes
simultaneously.

## Representative workflows

The current automated suite covers the following renderer-neutral semantics:

| Workflow                         | Automated evidence                                                  |
| -------------------------------- | ------------------------------------------------------------------- |
| Buffer insert/delete and Unicode | `buffer::tests`, including position and movement edge cases         |
| Cursor and selection             | `editor::tests`, `window::tests`, region/mark tests                 |
| Kill/yank/clipboard              | injected `kill_ring::tests` and editor kill/yank integration tests  |
| Undo/redo                        | `undo::tests`                                                       |
| Window split/select/delete       | editor split-tree, spatial ordering, and geometry-restoration tests |
| Commands and selection menus     | `command_registry`, `command_mode`, and `selection_menu` tests      |
| Syntax spans and gutter          | `syntax::tests` and `gutter::tests`                                 |
| External file changes            | merge, conflict, reload, and diff tests in `file_watcher::tests`    |
| Frontend redraw invalidation     | shared terminal/Vello renderer conformance test                     |

Before a release or major frontend change, manually exercise these platform paths in both frontends
because they are not yet automated end to end:

1. start with no file, one file, and two files;
2. insert text, move by character/word/paragraph/page, select, kill, yank, undo, and redo;
3. run `M-x`, switch and kill buffers, find and save a file, and use incremental search;
4. split, select, resize, and delete windows;
5. modify an open file externally and exercise clean reload and conflict handling;
6. leave the editor idle long enough for timers/background events; and
7. close normally and force an external failure while confirming terminal restoration.

Phase 1 and Phase 2 must turn the important platform paths into headless session transcripts and
event-loop tests rather than keep this manual checklist indefinitely.

## Mechanical invariants

The renovation must preserve or deliberately replace these invariants:

### Buffer and positions

- Buffer text is UTF-8 stored in Ropey; public edit and cursor positions are character indices, not
  byte offsets.
- `(column, line)` conversion clamps or rejects out-of-range input consistently and round-trips for
  valid positions.
- An edit updates undo state and adjusts syntax spans as one serialized native operation.
- A region is the ordered range between mark and cursor; clearing or invalidating a mark cannot
  leave a stale range.
- Buffer snapshots used outside the buffer lock own their data.

### Windows and presentation

- Every normal window refers to a live buffer.
- Every leaf in the window tree refers to exactly one live window, and deleting a window removes the
  corresponding leaf without leaving a phantom selection target.
- Split ratios remain valid and layout gives each realized window its required minimum geometry.
- Renderer invalidation is monotonic until cleared: marking any region makes `needs_redraw()` true;
  clearing dirty state makes it false.
- Terminal and Vello may realize a frame differently but receive the same logical editor meaning.

### Tasks and external resources

- A request/reply channel has one reply or a detectable cancellation/disconnection.
- Work belonging to a buffer, mode, endpoint, or session does not outlive that owner without an
  explicit transfer.
- Closing a native resource invalidates all associations to it; a reused slot cannot validate a
  stale generation.
- Clipboard, clock, filesystem, process, renderer, and GPU state are native capabilities rather than
  durable editor data.
- Ordinary external failure is reported as an error and does not violate core state invariants.

These are baseline statements. Phase 1 must add explicit boundedness and shutdown invariants, and
Phase 2 must encode session ordering, revisions, and transport-neutral identity rules.

## Coarse performance baseline

Run `./scripts/measure-phase0-baseline.sh` to build and measure a release-mode headless editor
fixture. It uses a 2,000-line Unicode buffer, 10,000 insert/delete round trips, and 100 full
terminal redraws into a counting writer. The harness does not access a terminal, display, filesystem
fixture, or clipboard.

The 2026-08-13 aarch64 result was:

```text
fixture_lines=2000
fixture_startup_us=333
fixture_idle_rss_kib=2452
edit_iterations=10000
edit_round_trip_ns_per_iteration=1044
redraw_iterations=100
terminal_full_redraw_us_per_iteration=1841
terminal_bytes_per_full_redraw=31729
process_wall_seconds=0.19
process_max_rss_kib=2848
```

These numbers are coarse regression sentinels. Compare results only on the same build profile,
machine, and fixture. Production-shaped interactive measurements remain required before making
performance claims.

## Known failures and debt at the baseline

| Area                  | Current evidence and required follow-up                                                                                                  |
| --------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| Formatting            | `cargo fmt --all -- --check` fails after the Compio migration. Phase 1 makes the tree clean.                                             |
| Clippy                | `cargo clippy --workspace --all-targets -- -D warnings` fails, first reporting eight `roe-core` findings.                                |
| Buffer/mode actors    | Futures MPSC channels are unbounded and detached tasks lack an owned shutdown path.                                                      |
| Vello/Compio wakeup   | Winit waits for platform events while Compio only advances inside `window_event`; independent work can stall.                            |
| Frontend semantics    | Both frontends still interpret `ChromeAction`; renderer conformance currently covers dirty-state lifecycle only.                         |
| Error handling        | User/environment failures and internal invariants are mixed across `String` errors, ignored results, panics, and exits.                  |
| End-to-end UI testing | No automated terminal PTY or graphical event-loop workflow exists.                                                                       |
| Performance coverage  | The harness measures core editing and terminal full redraw only, not GPU redraw, input-to-presentation latency, or idle event-loop cost. |

Nothing in this table is waived. Each item is routed to a later phase of the roadmap.
