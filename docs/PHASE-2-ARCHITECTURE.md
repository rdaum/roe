# Phase 2 architecture renovation

This document records the implemented Phase 2 boundary from
[ROE-MICA-ROADMAP.md](../ROE-MICA-ROADMAP.md). Roe now has a policy-neutral native kernel, one
ordered host-session protocol, and renderer-specific realization behind a shared logical
presentation stream. The implementation is in process, but its public messages do not contain Rust
references, SlotMap keys, trait objects, renderer handles, or borrowed data.

Phase 2 was implemented in these reviewable changes:

| Commit | Change |
| ------ | ------ |
| `217f2cf` | Added the generation-checked native text/layout/service kernel and capability enforcement. |
| `5a4b537` | Defined the owned, serde-compatible session and revisioned presentation protocol. |
| `7e29cf3` | Prevented idle inputs from manufacturing unchanged presentation revisions. |
| `a695fd8` | Routed terminal and Vello input, policy execution, pointer/layout operations, presentation, and shutdown through `HostSession`. |

## Native kernel

`roe_core::native_kernel::NativeKernel` owns ephemeral native resources. A `ResourceId` is a
kernel-defined `(slot, generation)` pair; closing and reusing a slot advances its generation, so a
stale identity cannot acquire a new resource. These identities are volatile capabilities and must
never be persisted as editor facts.

The first native text resource wraps Roe's shared Rope-backed `Buffer`, so registration does not
copy the Rope and native operations affect the same storage observed by the session. The kernel
provides validated create/close, snapshot, character-indexed insert/delete/replace, selection,
undo, and redo operations. Replace is one explicit undo group. Invalid ranges fail before mutation.
Logical layouts use transport identities and reject duplicate views, missing active views,
non-finite ratios, ratios outside `(0, 1)`, and zero-sized frames.

Native service operations cover file read/write, clipboard read/write, wall-clock read, child
process execution, and watcher association. Every operation checks an explicit `CapabilityGrants`
set before looking up or disclosing a resource. Kernel errors distinguish denied authority, stale
resources, invalid ranges/layout, I/O, and clipboard failures. A kernel failure is returned as a
request completion; it does not tear down the session endpoint.

The kernel enforces mechanisms only. It contains no command names, key bindings, modes, completion
rules, hooks, packages, or renderer policy.

## Session protocol

`roe_core::session::HostSession` is the sole production frontend endpoint. Terminal and Vello
normalize platform input, send an `InputEnvelope`, apply `SessionOutput`, and realize its
`PresentationUpdate`. Neither frontend imports `ChromeAction`, calls an editor command, accesses a
mode, or borrows `Editor`.

The protocol has the four distinct families required by the roadmap:

- `InputEvent`: keys, text, pointer state, view scrolling, resize, focus, timer/native
  notifications, native requests, cancellation, heartbeat, resync, and close;
- `NativeOperation` plus `NativeCompletion`: capability-checked text/layout/native service work
  with request identities;
- `PresentationUpdate`: a full snapshot or a delta naming its exact base and resulting revision;
  and
- `LifecycleEvent`: ready/capability discovery, warning, error, quit request, heartbeat, and
  endpoint close.

Every envelope contains protocol version, session epoch, and input sequence. A live in-process
endpoint accepts exactly the next sequence; a duplicate, gap, stale epoch, or unsupported version
is rejected without dispatch. Accepted inputs are at-most-once. There is no session mailbox in
Phase 2: dispatch is a direct awaited call, so the caller itself supplies backpressure and only one
input is active per endpoint.

The direct adapter also enforces payload and presentation bounds:

- at most 64 normalized keys per input;
- at most 65,536 characters in text/native text payloads;
- frame dimensions from 1 through 1,000 cells on each axis; and
- at most 1,000,000 characters in one presented visible slice.

Native completion vectors are bounded by the input shape: one native request produces at most one
completion. Cancellation currently reports that no request is pending because native operations
are synchronous direct calls. Phase 3 external handlers introduce genuinely suspendable work and
must bind cancellation to endpoint/task authority rather than weakening this contract.

## Presentation and renderer ownership

The authoritative `PresentationSnapshot` contains only renderer-neutral values: session epoch and
revision, frame cells, transport view/resource identities, logical geometry and scroll offsets,
bounded visible text, cursor and selection, total-line/maximum-line metrics, modeline and echo
content, gutter intent, style definitions/references, and absolute styled ranges.

Each changed presentation advances its revision exactly once. A delta names the prior revision;
`PresentationStreamState`, shared by both renderers, rejects an epoch mismatch, missing base,
revision gap, snapshot/envelope mismatch, or non-unit advance. `RequestSnapshot` returns a complete
self-contained snapshot and is the recovery path after a slow consumer, reconnect, or detected
gap. Idle heartbeats and timer ticks with no logical change emit no presentation and consume no
revision.

Terminal owns cell mapping, ANSI/crossterm state, color realization, border glyphs, and cursor
visibility. Vello owns Parley shaping, GPU scenes, surfaces, scale factors, glyph styling, and
pixel-level scrollbar hit normalization. Both render chrome in Rust from the same Mica-ready
modeline, echo, style, geometry, and visible-slice values. Neither renderer infers commands or mode
policy from presentation data.

The older `Renderer<Editor>` trait and `renderer::PresentationSnapshot` remain only as the Phase 0
compatibility/conformance surface. Production frontend loops do not use them. They can be removed
after Phase 3 has replaced the remaining Rust editor driver and the Phase 0 historical tests have
been rewritten against the session transcript.

## State and authority location

| State or decision | Phase 2 owner | Process-boundary rule |
| ----------------- | ------------- | --------------------- |
| Rope text, spans, undo primitives | Native kernel/Rust | Referenced only by volatile `ResourceId`; operations require grants. |
| File, clipboard, clock, process, watcher mechanisms | Native kernel/Rust | Requested explicitly; host checks capability before work. |
| Window geometry and split invariants | Native kernel/session host | Views use `ViewId`; renderer handles never cross the boundary. |
| Input ordering and endpoint lifecycle | `HostSession` | Epoch plus exact sequence; close invalidates the endpoint. |
| Command/keymap/mode policy | Transitional Rust `Editor` behind `HostSession` | No frontend owns it; Phase 3 moves this driver state to Mica. |
| Logical presentation/chrome content | `HostSession` output | Full/delta revision stream; complete resync is always available. |
| Terminal cells and escape state | Terminal renderer | Never exposed to the driver. |
| Shaping, glyph cache, scene, WGPU surface | Vello renderer | Never exposed to the driver. |
| Durable user/package facts | None in Phase 2 | Phase 3 begins in-memory and defines persistence separately. |

`ChromeAction` is now an internal compatibility language between the transitional Rust modes and
the session host. Only `HostSession::resolve_actions` interprets it. Phase 3's Mica driver produces
native requests, presentation facts/effects, and lifecycle events directly; Phase 5 removes each
superseded Rust command/mode slice rather than creating dual ownership.

## Process-ready semantics

No network or subprocess transport is implemented in this phase. A later transport must preserve
the following semantics rather than exporting Rust internals:

1. negotiate `SESSION_PROTOCOL_VERSION` before accepting normal input;
2. allocate a fresh unpredictable session epoch and reset sequence state on open/reconnect;
3. discover the granted capability list through `LifecycleEvent::Ready`;
4. preserve one ordered input stream or reject gaps/duplicates;
5. apply transport backpressure before the documented payload bounds are exceeded;
6. correlate native work with `RequestId` and preserve timeout/cancellation authority context;
7. use heartbeat/lifecycle events to distinguish idle from a closed or crashed peer;
8. discard deltas after a gap and request a full snapshot;
9. invalidate all native associations on endpoint close or driver crash; and
10. log epoch, sequence, request, revision, capability decision, overload, resync, and close
    boundaries without logging buffer contents by default.

`SessionTranscript` proves that normalized inputs can be replayed through a renderer-free headless
host. The terminal/Vello session-conformance test applies one full snapshot and a delta to both
renderers, verifies identical logical state, and verifies that both reject the same artificial
revision gap.

## Verification and exit assessment

The Phase 2 acceptance commands are:

| Command | Result |
| ------- | ------ |
| `./scripts/check.sh` | Passes formatting, all-target checking, strict Clippy, workspace tests, and dependency policy. |
| `cargo +1.88.0 check --workspace --all-targets` | Passes on the declared MSRV with serde-derived protocol types. |
| `./scripts/test-phase0-terminal-workflows.sh` | Passes startup, interactive editing/opening, idle file delivery, visible save failure, quit, and SIGTERM restoration through `HostSession`. |
| `cargo build --release --bin roe-vello` | Passes with the session-driven Vello frontend and presentation renderer. |
| `./scripts/measure-phase0-baseline.sh` | Completes with the renovated terminal frontend. |
| `cargo test -p roe-vello --test session_conformance` | Proves shared presentation acceptance and gap detection. |

Focused tests cover stale resource generations, Unicode character ranges, atomic invalid-range
failure, grouped replace undo/redo, capability denial, layout invariants, exact input ordering,
bounded idle behavior, revision monotonicity, full resync, endpoint close, deterministic headless
transcript replay, nested split targeting, and cross-frontend presentation conformance.

Phase 2's exit criteria are met. Both production frontends use one session contract for input and
presentation; frontends no longer execute commands or manage modes; the native kernel is tested
without a renderer; headless transcripts replay through the real host; and the remaining
`ChromeAction`/Rust policy path has one explicitly transitional owner and deletion route. The
environment still lacks X11/Wayland, so the display-host Vello smoke remains the same explicit
platform obligation recorded in Phase 1 rather than being presented as a Phase 2 pass.
