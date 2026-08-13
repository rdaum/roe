# AGENTS.md - Agent Guide for Roe Editor

This guide describes the current Roe architecture and the conventions agents should preserve.

## Project Overview

**Roe** (Ryan's Own Emacs) is a minimal text editor in the Emacs tradition, built in Rust. It is
buffer-oriented rather than file-oriented, uses Emacs-style keys, and is programmable through an
embedded [Mica](https://github.com/timbran-project/mica) driver.

Roe uses [Compio](https://github.com/compio-rs/compio) for completion-based asynchronous I/O. The
former Julia integration, Rust command registry, Rust keybinding tables, mode actors, selection
modes, and syntax/face registries have been removed. Mica is now the authoritative production owner
of editor policy; Rust owns bounded native mechanisms, the session boundary, and renderer-specific
realization.

The workspace uses Rust edition 2024, declares Rust 1.95 as its MSRV, and pins Rust 1.97.1 for
development and CI. `mica-driver` is pinned to the exact revision in `Cargo.toml` with default
features disabled.

## Architectural Boundary

The production path is:

```text
terminal or Vello platform event
  -> renderer-neutral InputEvent
  -> ordered attachment envelope
  -> Mica prompt/key/command/policy transaction
  -> bounded native action, external request, or effect
  -> Rust text/file/layout/resource mechanism
  -> revisioned PresentationUpdate plus LifecycleEvent
  -> terminal cells or Vello scene
```

### Mica owns meaning

Mica owns:

- commands, discovery, argument acquisition, and invocation;
- keymaps, prefixes, binding precedence, and ordinary text-action selection;
- prompts, completion, buffer/file selection, and incremental search state;
- major/minor modes, hooks, faces, syntax rules, indentation, and configuration;
- packages and effective policy composition; and
- logical active-view and window-target decisions.

The core ontology and generic behaviors live in `mica/roe-model.mica`. Shipped editor policy and
bindings live in `mica/roe-first-wave.mica`. `mica/roe-model-demo.mica` is a non-production Phase 3
fixture.

### Rust owns mechanisms

Rust owns:

- Rope-backed buffer storage, character-indexed mutation, selection primitives, and undo/redo;
- file I/O, file watching, clipboard, clock, process, and directory mechanisms;
- generation-checked native resources and capability enforcement;
- validated logical layout mutation and geometry;
- ordered session envelopes, lifecycle delivery, bounds, and presentation revisioning; and
- terminal state/cells and Vello/Winit/WGPU/Parley resources.

Rust may validate invariants and enforce authority, but it must not choose commands, bindings,
modes, hooks, candidates, packages, or logical targets in the production path.

### Production constructors

Both shipped frontends create a `WorkspaceHost` with `WorkspaceHost::open_with_mica`, then attach a
`DirectSessionClient`. `WorkspaceHost::open` is intentionally a policy-free workspace/native-
mechanism test harness. Do not add a Rust policy fallback to either constructor.

## Workspace Structure

The Cargo workspace has four crates:

- **`roe`**: terminal binary and Crossterm event loop;
- **`roe-core`**: buffers, native editor mechanisms, Mica bridge, kernel, and session protocol;
- **`roe-terminal`**: terminal presentation realization; and
- **`roe-vello`**: Winit/Vello GPU frontend and presentation realization.

Important paths:

```text
roe/
├── Cargo.toml
├── rust-toolchain.toml
├── mica/
│   ├── MICA-REVISION
│   ├── roe-model.mica           # ontology, derived rules, generic behaviors
│   ├── roe-first-wave.mica      # shipped commands, bindings, modes, faces, policy
│   └── roe-model-demo.mica      # non-production model fixture
├── roe-core/src/
│   ├── buffer.rs                # Rope storage, marks, gutter intent, undo primitives
│   ├── editor.rs                # buffers/windows plus native mechanism realization
│   ├── file_watcher.rs          # transactional, bounded external file watching
│   ├── keys.rs                  # normalized key and native-action vocabulary only
│   ├── kill_ring.rs             # Emacs-style kill ring and clipboard integration
│   ├── mica_host.rs             # Mica driver lifecycle and native bridge
│   ├── native_kernel.rs         # capabilities, resources, native operations, layouts
│   ├── native_services.rs       # injectable platform service traits
│   ├── renderer.rs              # shared renderer utility types
│   ├── session.rs               # transport-neutral input/output/presentation protocol
│   ├── undo.rs
│   └── window.rs
├── roe-terminal/src/terminal_renderer.rs
├── roe-vello/src/
├── roe/src/main.rs
├── docs/                        # phase records, ADRs, and dependency policy
└── scripts/
```

The definitive ownership summary is `docs/PHASE-5-POLICY-TRANSFER.md`. Architectural decisions are
recorded in `docs/adr/0001` through `0006`.

## Essential Commands

### Required verification

Run the repository check before committing:

```bash
./scripts/check.sh
```

It runs:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=8
./scripts/check-dependencies.sh
```

Useful focused checks:

```bash
# Declared MSRV
cargo +1.95.0 check --workspace --all-targets

# Mica/session tests; serialize focused driver tests
cargo test -p roe-core mica_ -- --test-threads=1

# Shared production presentation stream
cargo test -p roe-vello --test session_conformance

# Headless production Mica-to-Vello scene
cargo test -p roe-vello production_mica_session_builds_a_vello_scene_without_a_display

# Production terminal workflows
./scripts/test-phase0-terminal-workflows.sh

# Coarse readiness/edit/redraw/RSS regression measurements
./scripts/measure-phase0-baseline.sh

# Security policy; requires cargo-audit
./scripts/check-security.sh
```

### Building and running

```bash
cargo build --release
cargo build --release -p roe
cargo build --release -p roe-vello

./scripts/run.sh [files...]
./scripts/run-vello.sh [files...]
```

Both binaries expose the same pre-policy recovery operations:

```text
--mica-check FILE
--mica-replace UNIT FILE
--mica-export UNIT FILE
--mica-restore-first-wave
--mica-enable-package PACKAGE
--mica-disable-package PACKAGE
--mica-inspect
```

## Core Concepts and Invariants

### Buffers, windows, and identities

- A `Buffer` wraps `Arc<RwLock<BufferInner>>`; text is stored in `ropey::Rope`.
- Buffer positions and native text ranges are **character indices**, not byte offsets.
- A `Window` is an Emacs-style view into a buffer, not an operating-system window.
- `WindowNode` is the validated logical split tree; `Frame` is available screen real estate.
- `BufferId` and `WindowId` are stable SlotMap keys local to Rust.
- `ViewId` is the transport-neutral logical presentation identity.
- `ResourceId` is a volatile `(slot, generation)` capability reference. A stale generation must
  never acquire a reused resource.
- Native IDs, endpoint identities, grants, and resource generations are ephemeral and must never be
  persisted as Mica policy facts.

Buffers remain fundamental: scratch and prompt buffers need not have files, and core operations
should use `BufferId` rather than treating file paths as buffer identity.

### Host session protocol

`SessionClient` is the frontend contract. `DirectSessionClient` is its in-process implementation;
a remote implementation must preserve the same messages and lifecycle rather than wrapping a
different editor API. Every `InputEnvelope` carries protocol version, attachment epoch, and exact
sequence number. Accepted inputs are at-most-once; duplicates, gaps, stale epochs, and unsupported
versions are rejected.

`WorkspaceHost` owns durable editor state, Mica, native resources, watchers, and processes.
`Attachment` owns viewport/focus, input ordering, presentation revision, pointer and scroll state,
and frontend-service grants. Attach, detach, resume, close-attachment, and terminate-workspace are
distinct operations. Transport loss must detach an attachment and must not terminate its workspace.
Server-originated output carries no acknowledged input sequence.

`SessionOutput` contains:

- zero or one full/delta `PresentationUpdate`;
- bounded native completions; and
- lifecycle events such as warnings, errors, overload, recovery results, invalidation, quit, and
  endpoint closure.

Presentation deltas name their base and resulting revisions. A consumer that detects a gap must
discard deltas and request a full snapshot. Idle work that changes no logical presentation must not
advance the presentation revision.

Frontends may realize presentation differently, but must not infer editor meaning from presentation
data. Terminal incremental rendering should repaint changed views/rows without clearing the screen;
layout changes may require a complete clear. Vello owns pixel geometry, shaping, scrollbars, and GPU
scenes.

### Mica host and authority

`MicaHost` embeds the CPU-only driver and owns one logical consumer of driver events. Do not add a
second event reader or an unbounded bridge queue.

Authority is layered:

1. Mica relations decide whether an actor may invoke/effect/request a service.
2. The endpoint bridge checks actor, endpoint, service, and logical-buffer associations.
3. `NativeKernel` checks the native capability and resource generation.
4. The native operation validates ranges, layouts, and payload bounds before mutation.

A numeric or logical identity is never authority by itself. Failed commands and native requests are
recoverable diagnostics; they should not tear down a healthy endpoint. Replacement is check-first
and must retain the last working unit on failure.

Current important bounds include:

- 256 queued Mica driver events;
- 16 concurrent external requests;
- 64 subscription events;
- 64 logical keys per input;
- 65,536 text characters per input;
- 256 prompt/policy candidates or facts;
- 1,024 search matches;
- 64 logical views; and
- 1 MiB variable-size native completion data.

Preserve or deliberately revise bounds when adding work; do not introduce hidden unbounded queues,
histories, task sets, caches, directory accumulation, or diagnostics.

### Compio lifecycle

- Compio runtime ownership stays on its owning thread; do not move a runtime across threads.
- Frontends drive asynchronous session work through their existing Compio/Winit loops.
- Roe deliberately removed its detached buffer/mode actors and unbounded Futures mailboxes.
- Do not casually add `compio::runtime::spawn`; every task needs explicit owner, cancellation,
  backpressure, error delivery, and shutdown behavior.
- Keep the runtime and the sole Mica event consumer alive while closing endpoints and draining the
  bounded driver queue.
- File I/O uses `compio::buf::BufResult`; check the operation result before using the returned
  buffer.

### File watching and shutdown

File-watcher ownership is transactional:

- publish a watch association only after the backend watch succeeds;
- preserve logical ownership when a final backend unwatch fails so it can be retried;
- use the bounded 256-hint queue and latest-value backend-error slot;
- reread current disk state when handling a hint rather than putting file contents in the queue;
- unregister killed/replaced buffers; and
- attempt every native cleanup during shutdown even after an earlier cleanup error.

Terminal state cleanup must remain idempotent and attempt raw-mode restoration on normal exit,
signals, I/O failure, and unwinding. Vello errors should remain typed and returned rather than
log-only exits.

## Code Patterns

### Buffer access

Prefer the `Buffer` closure helpers, which keep lock scope explicit:

```rust
let content = buffer.with_read(|inner| inner.content());
buffer.with_write(|inner| inner.insert_pos(fragment, position));
```

The direct wrapper methods are also appropriate for small operations. Use a read lock for
observation and a write lock only for mutation. Do not retain a guard across an await or renderer
call.

### Native operations

When adding a genuinely native mechanism:

1. define the capability and bounded request/result vocabulary in `native_kernel.rs`;
2. authorize before resolving or disclosing a resource;
3. validate all ranges/layout/payloads before mutation;
4. bridge only the minimal service and logical identities in `mica_host.rs`;
5. translate outcomes into `SessionOutput` in `session.rs`; and
6. test denial, stale generations, invalid input, bounds, cancellation/close, and recovery.

Never expose a Rope, Rust SlotMap key, renderer handle, OS handle, or `ResourceId` directly to Mica.

### Mica policy changes

For commands, bindings, prompts, modes, hooks, faces, syntax, configuration, or packages:

1. change the relation/behavior model in `mica/roe-model.mica` when the ontology changes;
2. add shipped policy facts and command implementations in `mica/roe-first-wave.mica`;
3. keep command discovery, argument typing, precedence, and package membership relational;
4. use effects for no-result observable changes, external requests for result-bearing native work,
   and subscriptions for settled relation changes;
5. synchronize endpoint-volatile context through `MicaHost`, not durable source; and
6. exercise the real `WorkspaceHost::open_with_mica` plus `DirectSessionClient` path and both
   frontend presentation consumers.

Do not create a parallel Rust command, keybinding, mode, face, or syntax owner. `KeyAction` and the
reduced `ChromeAction` are mechanism/effect vocabularies, not policy registries.

### Renderer changes

Renderer changes consume `PresentationSnapshot`/`PresentationUpdate` and must preserve shared
revision semantics. Put terminal cell/escape behavior in `roe-terminal`; put shaping, pixel
geometry, scrollbar, surface, and scene behavior in `roe-vello`. Do not route commands or Mica
driver events directly into a renderer.

When changing the shared presentation model, update both renderers and the
`roe-vello/tests/session_conformance.rs` fixture. Test full snapshots, deltas, revision gaps, layout
changes, cursor/selection state, styles, modeline/echo content, and incremental redraw behavior as
appropriate.

### Error handling and tracing

- Ordinary external failures should become typed errors, lifecycle diagnostics, or visible echo
  messages, not panics.
- `expect`/`unwrap` are reserved for checked internal invariants such as SlotMap membership or lock
  poisoning.
- Diagnostics should include task/selector/endpoint/session/failure class where useful, but should
  not log buffer contents by default.
- Preserve tracing at endpoint, request, native mutation, redraw, invalidation, recovery, and close
  boundaries.

### Tests and shared resources

Use `KillRing::with_capacity` in core tests. The workspace kill ring never accesses the system
clipboard; clipboard access belongs to an attachment-local frontend service. Frontend clipboard
tests share process-wide state and can interfere when run in parallel. Focused Mica driver tests
should use one test thread when they share driver/recovery state.

## Dependencies and Tooling

- `compio` is exactly pinned at 0.18.0 for Mica compatibility.
- `mica-driver` is pinned by Git revision with default features disabled.
- Mica relation acceleration and persistent storage are intentionally disabled.
- Vello, its WGPU graph, Parley, Winit, and Pollster are a coupled upgrade group.
- Dependency policy and temporary advisory exceptions live in `docs/DEPENDENCY-POLICY.md`.
- `scripts/check-dependencies.sh` enforces the runtime pins and centralized workspace dependencies.
- Rust formatting is checked with `cargo fmt`; strict Clippy is part of `scripts/check.sh`.

Do not update Compio, Mica, or the graphics stack as an incidental change. A Mica revision change
requires rechecking source compatibility, lifecycle, authority, replacement, recovery, backpressure,
shutdown, terminal workflows, and both frontend presentation paths.

## Known Deliberate Limits

- Durable Mica user/workspace persistence is not enabled. Adding it requires explicit schema,
  migration, backup, export, rollback, and recovery policy.
- No network, daemon, or subprocess session transport exists yet. `SessionClient`, attachment
  lifecycle, independently pushable output, and frontend-service request/results are the transport-
  neutral boundary on which one can be built.
- A real display-host Vello smoke remains an environment-dependent obligation; headless scene and
  conformance tests do not replace it.
- Production Mica dispatch is materially slower than direct Rope mutation. Preserve the honest
  session-path benchmark when optimizing it.

## Copyright and License

Roe is GPL-3.0. Preserve the copyright/SPDX header style of the surrounding source and run the
repository formatter and checks for new files.
