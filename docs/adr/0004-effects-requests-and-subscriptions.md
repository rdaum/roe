# ADR 0004: distinct effects, external requests, and subscriptions

Status: accepted for Phase 3

## Context

Mica offers three host interaction paths with different semantics. Treating them as one generic
message would obscure acknowledgement, cancellation, backpressure, and redraw ordering.

## Decision

Use committed effects for observable changes that need no native return value: presentation
invalidation, echo/diagnostic publication, command completion notification, and quit intent. Effects
advance or invalidate the shared presentation stream; they never contain renderer handles or draw
commands.

Use external requests for native work that returns a value or failure: snapshots and validated text
mutations, clock reads, file/process/clipboard services, watcher registration, and layout mutation.
The handler resolves endpoint-local associations, enforces service and buffer authority, observes
the request timeout, races child work against `context.cancellation.cancelled()`, and refuses output
after cancellation. Large results obey the Phase 2 completion bound.

Use subscriptions for settled relation changes that should wake a session without polling: effective
keymap/mode/package changes, logical view-tree changes, and presentation-relevant facts. Each
session has an ephemeral subscription mailbox with budget 64. `SubscriptionReady` is only a
coalescible wake hint; the one host consumer drains the mailbox and requests a full resync if its
cursor or revision has a gap.

Ordering follows the driver contract: effects committed by a task precede its terminal lifecycle
event; subscription readiness caused by that outcome follows it. The host converts all three paths
to the Phase 2 session protocol before either frontend sees them.

## Consequences

Redraw is not coupled to terminal input, native results have explicit request/cancellation context,
and reactive policy changes do not require a polling loop. Queue saturation applies driver
backpressure and documented coalescing rather than allocating another host mailbox.
