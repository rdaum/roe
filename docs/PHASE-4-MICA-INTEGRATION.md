# Phase 4 first Mica integration wave

This document records the implemented first production slice from
[ROE-MICA-ROADMAP.md](../ROE-MICA-ROADMAP.md). The terminal frontend now starts a CPU-only Mica
driver behind `HostSession`; `F12` is interpreted by a Mica keymap and runs the Mica-defined
`roe/insert_current_time` command. Rust still owns the Rope, native clock, validated mutation, and
terminal rendering.

Phase 4 was implemented as four conventional changes:

| Commit | Change |
| ------ | ------ |
| `040b4e9` | Pinned `mica-driver`, disabled its default features, and raised the workspace/CI MSRV to Rust 1.95. |
| `2ef6e8d` | Embedded the driver, implemented the command/request/effect path, lifecycle handling, and production terminal probe. |
| `1d21d8c` | Proved that the configured event queue backpressures a producer and remains drainable during shutdown. |
| `2148927` | Enforced native-service authority, retired stale volatile identities, and added the idle driver-event pump. |

## End-to-end slice

[`mica/roe-first-wave.mica`](../mica/roe-first-wave.mica) owns the command identity, description,
package membership, `F12` binding, selector, authorization declaration, native-request sequence,
cursor update, and presentation effect. There is no shadow Rust command or binding.

```text
terminal F12
  -> InputEvent::Keys([Function(12)])
  -> normalized sequence "F12"
  -> endpoint invocation roe/dispatch_key
  -> EffectiveSessionKeymap + EffectiveBinding
  -> roe/insert_current_time
  -> external_request(:clock_millis)
  -> external_request(:text_insert, character offset + text)
  -> committed presentation_invalidated effect
  -> SessionOutput::PresentationUpdate
  -> terminal redraw
```

The command reads `ActiveView`, `ViewBuffer`, and `ViewCursor` from endpoint-volatile relations. It
must have both the relational `CanRequestService` grants and `CanUseBuffer(actor, buffer)` before it
can request native work. The Rust external handler independently checks that the request actor owns
the endpoint, that its endpoint grant mirror contains the service-specific `clock_read` or
`text_write` authority, and that the logical buffer appears in the host's `CanUseBuffer` mirror
before mapping it to a generation-checked `ResourceId`. The first-wave mirror is initialized from
the fixed `roe/editor_role` policy installed in `roe/core`; Phase 5 must synchronize it when roles
become live-editable. `NativeKernel` then checks its native capability grant and validates the
character offset before mutation. Logical service authority, host association, and native
capability are distinct checks; none is persisted. A focused denial test revokes the bridge's clock
grant and proves that Mica's effect permission alone cannot reach the native clock.

The native clock is injectable. The deterministic integration test uses a fixed millisecond value,
including after a Unicode edit and after creating a new Rust buffer/window following endpoint open.
The production tmux workflow sends a real `F12`, saves the result, and verifies the inserted clock
line in the file.

## Host and lifecycle boundary

`MicaHost` is private implementation behind the renderer-neutral `HostSession` contract. In the
terminal process there is one driver and one editor endpoint for the lifetime of its one session.
The driver uses only public `mica-driver` APIs and has fixed resources:

| Resource | Bound or policy |
| -------- | --------------- |
| driver workers | 2 |
| relation parallelism | 1 |
| task instructions | 250,000 |
| task retries | 4 |
| call depth | 32 |
| driver event queue | 256, producer backpressure at capacity |
| concurrent external requests | 16 |
| subscription queue budget | 64 |
| relation acceleration | disabled |

One host consumer drains `DriverEvent`. Normal key dispatch awaits that same stream, so a completed
external request resumes and redraws without another keyboard, pointer, or timer event. Independent
work is drained by `HostSession` on the terminal's existing 20 ms idle timer; effects, task errors,
cancellations, and subscription-ready notices therefore reach a `SessionOutput` without incidental
user input. Events arriving in the same batch after another task's completion are still processed,
and replacement leaves events for this consumer rather than discarding them. Close keeps the
Compio runtime and consumer active while it retracts endpoint tuples, cancels suspended tasks,
drains a full event queue, and awaits driver shutdown. Cancelled Mica tasks become typed
`LifecycleEvent::MicaTaskCancelled` values before `EndpointClosed`.

The endpoint starts atomically with ephemeral identities and its initial volatile session, actor,
buffer, resource-association, view, cursor, and keymap tuples. Before each Mica key dispatch the host
synchronizes the active Rust view, buffer, and character cursor. Changed functional tuples are
retracted before their replacements are asserted. New buffers and views receive fresh ephemeral
identities and native-resource associations. Removed Rust views and buffers have their complete
volatile tuple sets retracted; their bridge grants and every host identity map entry are removed in
the same synchronization pass. Endpoint close reconstructs and retracts every remaining tuple the
host installed.

Named source replacement is check-then-`FileinMode::Replace`. A malformed candidate leaves the last
working unit installed. A runtime command failure becomes a visible echo-area diagnostic and
`LifecycleEvent::Error`; a later valid replacement can run in the same session.

## Permanent decisions from the spike

- Mica policy enters Roe through `HostSession`, not through either renderer. Effects become shared
  presentation invalidations; they never call terminal or Vello code.
- The native boundary is a small service vocabulary plus logical identities. Mica never receives a
  Rope, `ResourceId`, Rust `BufferId`, renderer handle, or operating-system handle.
- Authorization is intentionally layered: Mica relations decide policy, the endpoint bridge limits
  logical associations, and `NativeKernel` enforces native capabilities and mechanical invariants.
- Endpoint-volatile facts are the synchronization format for logical context. Durable named units
  contain program and policy only.
- The driver event stream has one consumer. Key dispatch, idle progress, replacement, and close all
  feed the same event-to-session translation; Phase 5 subscription handlers must reuse it rather
  than introduce a second reader.
- Transitional Rust key handling is fallback-only: Mica receives normalized keys first, and only
  `:unbound` sequences reach `Editor::key_event`.
- `HostSession::open_with_mica` is the production integration constructor. Plain `open` remains a
  deliberate headless/conformance constructor until Phase 5 migrates Vello and the remaining policy
  slices through the same Mica-enabled path.

## Deliberate first-wave limits

The terminal is the only Mica-enabled frontend in this phase, as required by the roadmap. Vello
continues to use the same `SessionOutput` and presentation model but starts a Rust-policy session;
Phase 5 must move it to the Mica constructor as each policy slice becomes shared. The first wave has
no durable world, relation subscription, remote transport, or Mica-owned minibuffer. It also does
not remove the Rust registry used by unbound commands. Those are Phase 5 migration work, not hidden
alternate implementations of the `F12` behavior.

## Verification and exit assessment

| Command or test | Result |
| --------------- | ------ |
| `./scripts/check.sh` | Passes formatting, all-target checking, strict Clippy, workspace tests at eight threads, and dependency policy. |
| `cargo +1.95.0 check --workspace --all-targets` | Passes on the declared Mica-compatible MSRV. |
| `./scripts/test-phase0-terminal-workflows.sh` | Passes the existing production workflows plus the real Mica `F12` insertion/save/redraw path and terminal restoration. |
| `cargo test -p roe-core mica_ -- --test-threads=1` | Passes normalized input, deterministic round trip, dynamic context and retirement, host service denial, idle background completion, replacement/failure/recovery, backpressure, cancellation, full-queue close, and shutdown. |
| `cargo tree -p mica-driver -e features` | Contains no WGPU, Fjall, SQLite, persistence, or GPU feature. |

The Phase 4 exit criteria are met. A useful behavior is Mica-owned end to end through the public
driver; Rust owns native text and rendering; queues and shutdown are bounded and exercised; command
failure and replacement recover without losing the session; and the resulting host output is the
same renderer-neutral contract Vello already consumes. Phase 5 can now transfer policy in vertical
slices and delete each displaced Rust path.
