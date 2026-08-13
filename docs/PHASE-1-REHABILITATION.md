# Phase 1 rehabilitation

This document records the completed rehabilitation work for Phase 1 of
[ROE-MICA-ROADMAP.md](../ROE-MICA-ROADMAP.md). Phase 0 remains the historical baseline; this file
describes the renewed toolchain, error boundaries, queue ownership, event-loop progress, and
shutdown behavior that Phase 2 can build on.

Phase 1 was implemented on 2026-08-13 in these reviewable changes:

| Commit    | Change |
| --------- | ------ |
| `bbe6ed8` | Declared toolchain/dependency policy, upgraded compatible dependencies, added checks, and removed `async-trait`. |
| `e8c2de1` | Upgraded Vello, WGPU, and Parley as one coupled graphical migration. |
| `1b78e7b` | Migrated the workspace to Rust 2024 without functional changes. |
| `de6fcb8` | Removed detached buffer/mode actors and made terminal resource ownership explicit. |
| `ad93b1c` | Repaired background progress/wakeup, bounded watcher delivery, and external-failure reporting. |
| `cdd96b6` | Enforced the Rust 1.88-compatible dependency graph and repeatable advisory policy. |
| `609fc52` | Added tracing at native endpoint, request, mutation, redraw, and close boundaries. |
| `589dbbc` | Verified that cancellation returns a leased host for the next request. |
| `94209ec` | Kept the terminal idle regression probe in its pseudo-terminal foreground process group. |

## Toolchain and dependencies

The workspace now declares edition 2024, Rust 1.88 as its MSRV, and Rust 1.97.1 as its pinned
development/CI toolchain. [DEPENDENCY-POLICY.md](DEPENDENCY-POLICY.md) is the normative update and
advisory policy.

The completed dependency groups are:

| Area | Phase 0 | Phase 1 |
| ---- | ------- | ------- |
| Terminal/events | Crossterm 0.28 with Futures `EventStream` | Crossterm 0.29 with nonblocking polling; direct Futures dependency removed |
| Files/state | Notify 8.2, Similar 2.7, Slotmap 1.1 | Notify 8.2, Similar 3.1, Slotmap 1.1 |
| Graphics | Vello 0.6, WGPU 26, Parley 0.7, Pollster 0.4 | Vello 0.9, WGPU 29, Parley 0.11, Pollster 1.0 |
| Runtime | Compio 0.18 exact pin | Compio 0.18 exact pin retained for Mica compatibility |

`cargo outdated --workspace -R` reports all dependencies current after applying the compatibility
policy. Compio's transitive `compio-buf` is locked to 0.8.1 because 0.8.2 and 0.8.3 use a standard
library API unavailable on Rust 1.88; `scripts/check-dependencies.sh` prevents an accidental lockfile
advance. Ropey 2 and Winit 0.31 are prereleases and are not stable update targets.

The 2026-08-13 RustSec scan reports no vulnerabilities. Two unmaintained transitive crates have
documented temporary exceptions, exact advisory IDs, ownership paths, and a 2026-11-13 review
deadline in the dependency policy. `scripts/check-security.sh` ignores only those IDs and denies all
other audit warnings.

## Native ownership and delivery

The old buffer and mode actor tasks did not provide concurrency: frontends awaited every operation,
while the actors added unbounded Futures mailboxes and detached lifetimes. They are now one direct,
in-process `BufferHost` service per buffer.

| Boundary | Ordering and capacity | Overload/cancellation | Shutdown |
| -------- | --------------------- | --------------------- | -------- |
| Buffer host | Direct serialized call; no queue | An overlapping request returns typed `HostError::Busy`. `HostLease::drop` returns the service even when an awaiting caller is cancelled. | Dropping the last client drops its modes and buffer host; no task exists to join. |
| Mode chain | Major/minor modes run synchronously in declared order inside the host request; no queue | A mode consumes, annotates, or ignores the current request. | Modes are owned by and dropped with the host. |
| File notifications | Bounded FIFO of 256 path/buffer hints | Notify uses nonblocking `try_send`; a full queue drops the newest hint, logs the overload, and still wakes the frontend. Events carry no file contents; delivery rereads current disk state. | Dropping `FileWatcher` drops the notify watcher, sender, receiver, and wake handle. |
| Frontend input | Platform event order; no host mailbox | Terminal polls input after each Compio tick. Winit dispatches platform/user events on its UI thread. | Each frontend stops accepting input by leaving its event loop. |

There are no production `compio::runtime::spawn`, detached tasks, Futures MPSC queues, or other
host-facing unbounded channels in the workspace. The only remaining channel is the bounded native
file-notification FIFO above.

Buffer storage remains `Arc<RwLock<BufferInner>>` because renderer snapshots and file-merge logic
share buffers. Mutations are serialized by the frontend and direct buffer host, snapshots own their
text outside the lock, and poison failures remain internal-invariant assertions. Phase 2 will hide
this ownership behind the native-kernel API instead of exposing shared buffers to Mica.

## Event-loop progress and wakeup

The terminal loop now advances on a 20 ms Compio interval and polls Crossterm nonblockingly. Every
tick advances timers, signal observation, file delivery, and redraw decisions before checking for
input. The production workflow proves an external modification changes the visible frame while Roe
is idle; it no longer injects a keystroke to make the watcher progress.

The Vello loop uses a typed Winit user event and `EventLoopProxy` through the renderer-neutral
`FrontendWake` boundary. Native file callbacks send that event. `about_to_wait` also schedules a
20 ms deadline and drives ready Compio work, timers, watcher output, and redraw invalidation, so a
lost/coalesced platform wake cannot strand runtime work. `RedrawRequested` renders only when the
shared Vello redraw state is dirty.

Both frontends currently consume the same `ChromeAction`/`DirtyRegion` host outputs and differ only
in platform wakeup and realization. This is an interim compatibility boundary: Phase 2 replaces
the separate interpretation paths and per-event asynchronous calls with one revisioned
`HostSession` presentation stream.

## Error and shutdown model

Ordinary external failures no longer use internal assertions:

- save failures are `NativeOperationError::Save { path, source }` and are echoed while the editor
  remains usable;
- overlapping direct host operations are `HostError::Busy` rather than a borrow panic or hidden
  queue;
- clipboard reads and writes produce `ClipboardError`, preserve the internal kill ring, log the
  boundary failure, and add a visible echo action;
- only `NotFound` opens create a new empty buffer; permissions, directories, and other I/O errors
  retain path context and reach the command or startup caller;
- watcher deletion/read failures produce visible messages rather than disappearing after failed
  canonicalization;
- Winit window/surface and Vello renderer/render failures are logged and exit or return through the
  frontend boundary rather than panicking; and
- file-selector directory failures and watcher initialization failures retain resource context in
  tracing.

Remaining `expect`/`unwrap` sites in production code assert checked SlotMap membership, a live
window/buffer relationship, literal color validity, host-lease ownership, or lock non-poisoning.
These are internal invariants; Phase 2 adds broader window-tree and session validators rather than
turning invariant corruption into ordinary user errors.

Terminal state is owned by `TerminalSession`. Normal exit, I/O error, runtime error, SIGINT,
SIGTERM, and unwinding all run its idempotent cleanup. The signal path sets an atomic stop flag; the
next host tick stops input, returns from the runtime future, releases editor/native state, and then
restores terminal modes. The controlled workflow compares `stty -g` before and after SIGTERM.

Vello exits through Winit, after which the application, renderer surfaces, file watcher, editor
hosts, and Compio runtime are dropped by ownership. There are no detached Roe tasks to survive
either frontend. Explicit editor-endpoint closure and Mica-driver shutdown order do not exist yet;
they become mechanical `HostSession` lifecycle operations in Phases 2 and 3.

## Verification

The Phase 1 acceptance commands and results on the Phase 0 host are:

| Command | Result |
| ------- | ------ |
| `./scripts/check.sh` | Passes formatting, workspace/all-target checking, strict Clippy, all tests, and dependency policy. |
| `cargo +1.88.0 check --workspace --all-targets` | Passes on the declared MSRV after selecting `compio-buf 0.8.1`. |
| `cargo outdated --workspace -R` | Reports all dependencies up to date under the documented stable/compatibility policy. |
| `./scripts/check-security.sh` | Passes with no vulnerabilities and only the two documented advisory exceptions. |
| `./scripts/test-phase0-terminal-workflows.sh` | Passes production terminal workflows, idle watcher delivery, visible save failure, and terminal restoration after SIGTERM. |
| `cargo build --release --bin roe-vello` | Passes the renewed Vello/WGPU stack release build. |
| `./scripts/measure-phase0-baseline.sh` | Completes, including the one-second production terminal idle probe. |

The workspace suite contains 133 tests: 127 in `roe-core`, four terminal unit tests, and one shared
renderer-conformance test for each frontend. New focused evidence covers overlapping host requests,
host cancellation safety, bounded file-notification overload, real notify wakeup, clipboard
failure preservation/reporting, non-`NotFound` open errors, Unicode invariants, and external save
failure behavior.

The same-host coarse sample measured 1,025 ns per edit round trip, 266 microseconds per terminal
full redraw, 30.208 ms to the first welcome frame, and 3,596 KiB maximum RSS during the one-second
terminal idle probe. These are regression sentinels rather than acceptance thresholds. The idle RSS
sample is higher than Phase 0's 1,948 KiB observation and remains visible for later profiling; no
performance conclusion is drawn from one process sample.

## Phase 1 exit assessment and open obligations

Phase 1's mechanical exit criteria are met: checks pass, host queues are absent or explicitly
bounded, idle background work advances both frontend designs, owned Roe work has deterministic
cancellation/drop behavior, and normal external errors are returned or presented instead of
panicking.

The verification environment still has no X11 or Wayland display, so the production Vello event
loop cannot be observed end to end here. Its release build and renderer-neutral conformance suite
pass; a display-host smoke remains an explicit platform obligation. The 20 ms bridge and
`runtime.block_on` calls inside Winit input handling are deliberately transitional, not the Phase 2
architecture. Renderer realization is still frontend-specific, incomplete commands recorded in
the Phase 0 baseline remain incomplete, and Mica is not yet linked. Those items are routed to the
native kernel, session contract, and integration phases rather than hidden as Phase 1 successes.
