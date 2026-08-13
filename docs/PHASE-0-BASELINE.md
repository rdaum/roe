# Phase 0 baseline

This document records the state against which the Roe renovation is measured. It is evidence for
Phase 0 of [ROE-MICA-ROADMAP.md](../ROE-MICA-ROADMAP.md), not a claim that the current architecture
should be preserved.

Baseline work began on 2026-08-13 at commit `b9fd85f`. The recorded evidence includes the invariant
and isolation fixes made during Phase 0 and is verified at the Phase 0 completion commit.

## Workspace and execution surfaces

Roe is a Cargo workspace with four members:

| Member         | Role                                                      | Executable surface        |
| -------------- | --------------------------------------------------------- | ------------------------- |
| `roe-core`     | Buffers, editor state, modes, commands, windows, and undo | Library                   |
| `roe-terminal` | Crossterm presentation and terminal event loop            | Library                   |
| `roe-vello`    | Winit/Vello graphical frontend                            | `roe-vello`               |
| `roe`          | Terminal application assembly                             | `roe`                     |
| `roe-terminal` | Headless Phase 0 regression fixture                       | `phase0_baseline` example |

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
cargo test -p roe-core editor::tests
cargo test -p roe-terminal --test renderer_conformance
cargo test -p roe-vello --test renderer_conformance
cargo build --release --bin roe-vello
./scripts/test-phase0-terminal-workflows.sh
./scripts/measure-phase0-baseline.sh
```

At capture time:

- `cargo check --workspace` passes;
- `cargo test --workspace -- --test-threads=8` passes 129 tests;
- terminal rendering and the logical-observation/redraw component used by production Vello pass the
  shared dirty-lifecycle and presentation-snapshot conformance test;
- the production terminal adapter passes controlled startup, movement, Unicode region/undo/yank,
  edit/save, command/buffer/file selection, search, window resize, recorded watcher event-loop
  behavior, and clean-shutdown workflows; and
- kill-ring and editor tests use an injected clipboard boundary and do not touch the user's system
  clipboard.

Tests that construct Compio runtimes are serialized inside the `roe-core` test process. Before that
isolation, a parallel workspace run could fail with `EPERM` while creating four runtimes
simultaneously.

## Representative workflows and current evidence

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

The file-watcher suite includes an actual `notify` round trip through a uniquely owned temporary
directory. File selection uses an injected filesystem listing in tests, and editor/watcher timeout
tests use injected clocks.

The following platform smoke status was recorded on 2026-08-13:

| Workflow                           | Terminal                                                                                                      | Vello                                                                                |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| Release build                      | Passed with `cargo build --release --bin roe`                                                                 | Passed with `cargo build --release --bin roe-vello`                                  |
| No-, one-, and two-file start      | Passed in isolated 80x24 tmux terminals                                                                       | Not runnable: no X11 or Wayland display is available in the verification environment |
| Insert and save                    | Passed against a uniquely owned temporary file                                                                | Platform observation unavailable                                                     |
| Incremental search                 | Passed on a buffer with a multibyte prefix; insertion occurred at the selected match                          | Renderer-neutral semantics pass; platform observation unavailable                    |
| Split/select/resize/delete windows | Passed through production key and SGR mouse input                                                             | Renderer-neutral semantics pass; platform observation unavailable                    |
| External modification              | Notify was not processed while idle; the next input delivered it, reloaded the buffer, and saved the new text | Renderer-neutral watcher round trip passes; platform observation unavailable         |
| Clean shutdown                     | Passed via `C-x C-c` for every scripted terminal workflow                                                     | Platform observation unavailable                                                     |
| Forced signal shutdown             | Reproduced in an owned tmux pane; after SIGTERM Roe left the pseudo-terminal raw and without echo             | Not run                                                                              |

`./scripts/test-phase0-terminal-workflows.sh` reproduces the terminal observations without using the
user's active terminal. This records both results and what the environment could not exercise; an
unavailable Vello platform observation is an explicit open obligation, not a pass. The production
Vello application now captures the same renderer-neutral logical observation before production scene
construction and gates `RedrawRequested` with the tested redraw-state component. The shared suite
proves both frontends observe the same windows, buffer text, cursor, geometry, and active state
before renderer-specific realization. Actual glyph shaping, terminal-cell output, Vello scene
construction, and GPU presentation remain outside the suite until Phase 2 introduces the versioned
presentation contract.

Before a release or major frontend change, exercise these remaining platform paths in Vello and the
terminal variations not covered by the controlled workflow script:

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

The buffer-position statements below are current mechanical invariants covered by tests. Window,
presentation, and lifecycle statements distinguish what is enforced now from the target that later
phases must make mechanically true.

### Buffer and positions

- Buffer text is UTF-8 stored in Ropey; public edit and cursor positions are character indices, not
  byte offsets.
- `(column, line)` conversion clamps columns to line content and lines past the buffer to EOF, and
  round-trips for valid positions, including non-ASCII text.
- Window `(column, line)` coordinates are currently `u16`, so they cannot represent a line or column
  above 65,535. Direct buffer character offsets are `usize`; replacing the narrow window coordinates
  is explicit Phase 2 debt rather than an unstated invariant.
- An edit updates undo state and adjusts syntax spans as one serialized native operation.
- A region is the ordered range between mark and cursor; clearing or invalidating a mark cannot
  leave a stale range.
- Buffer snapshots used outside the buffer lock own their data.

### Windows and presentation

- Normal editor construction and the tested split/delete paths give every normal window a live
  buffer. A general runtime tree validator is still a Phase 2 requirement.
- Split/delete tests compare the live-window set with the tree leaves and cover the historical
  phantom-window failure. They do not yet enforce uniqueness at every mutation boundary.
- Split construction normalizes non-finite and out-of-range ratios. Minimum-geometry validation is
  not yet a general kernel invariant.
- Renderer invalidation is monotonic until cleared: marking any region makes `needs_redraw()` true;
  clearing dirty state makes it false. Full and incremental rendering capture the same logical
  presentation fields in both frontends. The shared suite does not claim renderer-realization
  equality or end-to-end delivery of every dirty-region variant.
- Terminal and Vello should realize the same logical presentation. They do not yet receive one
  shared presentation object and still interpret `ChromeAction` separately.

### Required task and external-resource invariants

- A request/reply channel must have one reply or a detectable cancellation/disconnection.
- Work belonging to a buffer, mode, endpoint, or session must not outlive that owner without an
  explicit transfer. Current detached buffer/mode tasks violate this target.
- Closing a native resource invalidates all associations to it; a reused slot cannot validate a
  stale generation.
- Clipboard, clock, filesystem, process, renderer, and GPU state are native capabilities rather than
  durable editor data.
- Ordinary external failure must be reported as an error and must not violate core state invariants.
  Current save/watcher paths do not yet satisfy this consistently.

These are baseline statements. Phase 1 must add explicit boundedness and shutdown invariants, and
Phase 2 must encode session ordering, revisions, and transport-neutral identity rules.

## Coarse performance baseline

Run `./scripts/measure-phase0-baseline.sh` to build and measure a release-mode headless editor
fixture. It measures a 2,000-line Unicode fixture, 10,000 insert/delete round trips, and 100 full
terminal redraws into a counting writer. It also measures the real `roe --help` process and starts
the real terminal application for one idle second inside an isolated pseudo-terminal. No user's
terminal or clipboard is modified.

The 2026-08-13 aarch64 sample was:

```text
fixture_lines=2000
fixture_construction_us=184
post_fixture_rss_kib=2448
edit_iterations=10000
edit_round_trip_ns_per_iteration=1040
redraw_iterations=100
terminal_full_redraw_us_per_iteration=246
terminal_bytes_per_full_redraw=31729
process_wall_seconds=0.03
process_max_rss_kib=2624
roe_help_wall_seconds=0.00
roe_help_max_rss_kib=2304
roe_terminal_ready_ms=25.391
roe_terminal_idle_seconds=1.00
roe_terminal_idle_max_rss_kib=1948
```

`roe_terminal_ready_ms` measures process launch through the first captured `*Welcome*` frame in a
fresh tmux server, and therefore includes tmux server startup overhead. The idle probe uses a real
80x24 pseudo-terminal rather than the previous zero-sized terminal, which crashed before editor
construction. These numbers are coarse regression sentinels. Compare results only on the same build
profile, machine, and fixture. Production-shaped interactive measurements remain required before
making performance claims.

## Known failures and debt at the baseline

| Area                  | Current evidence and required follow-up                                                                                                   |
| --------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| Formatting            | Phase 0 review normalized the Compio migration; `cargo fmt --all -- --check` now passes.                                                  |
| Clippy                | `cargo clippy --workspace --all-targets -- -D warnings` fails, first reporting eight `roe-core` findings.                                 |
| Buffer/mode actors    | Futures MPSC channels are unbounded and detached tasks lack an owned shutdown path.                                                       |
| Vello/Compio wakeup   | Winit waits for platform events while Compio only advances inside `window_event`; independent work can stall.                             |
| Frontend semantics    | Both frontends still interpret `ChromeAction`; shared conformance covers logical observations and redraw state, not realization equality. |
| Error handling        | User/environment failures and internal invariants are mixed across `String` errors, ignored results, panics, and exits.                   |
| End-to-end UI testing | Controlled terminal workflows pass in tmux; no graphical event-loop workflow can run without an X11/Wayland host.                         |
| Watcher deletion      | Modification delivery is covered, but deletion is currently dropped after canonicalization of the now-missing path.                       |
| Incomplete operations | `revert-buffer`, `write-file`, and `ActionPosition::End` insert/delete/kill remain explicit implementation stubs.                         |
| Mode command surface  | `Mode::available_commands` is not registered or consumed, so its mode-local command lists are currently dead code.                        |
| Signal shutdown       | The tmux probe verifies that SIGTERM leaves its owned terminal raw and without echo; signal-aware restoration is required in Phase 1.     |
| Performance coverage  | The harness measures core editing and terminal full redraw only, not GPU redraw, input-to-presentation latency, or idle event-loop cost.  |

Nothing in this table is waived. Each item is routed to a later phase of the roadmap.
