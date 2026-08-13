# Phase 3 Mica editor architecture

This document records the Phase 3 design required by [ROE-MICA-ROADMAP.md](../ROE-MICA-ROADMAP.md).
The design is grounded in the Phase 2 session/kernel boundary and Mica's public `mica-driver` API at
exact revision `a13f479229b761bf45b7ef71802cd4ca6e588dd4`.

The pinned driver and its public dependency graph declare Rust 1.95. Roe's 1.88 check correctly
rejects them, so Phase 4 includes an explicit workspace/CI MSRV cutover to 1.95 alongside the Mica
dependency. No production Mica dependency is added during this design-only phase.

The ownership decisions are split into reviewable ADRs:

- [ADR 0001](adr/0001-mica-owns-editor-policy.md): Mica owns logical editor policy; Rust owns
  validated mechanisms and realization.
- [ADR 0002](adr/0002-one-process-driver-and-endpoint-lifecycle.md): one process-long driver and
  endpoint-scoped session lifecycle.
- [ADR 0003](adr/0003-authority-recovery-and-persistence.md): relational authority, a minimal native
  recovery surface, and an in-memory first world.
- [ADR 0004](adr/0004-effects-requests-and-subscriptions.md): distinct roles for effects, external
  requests, and subscriptions.
- [ADR 0005](adr/0005-policy-migration-and-deletion.md): vertical migration with deletion of
  superseded Rust policy.

## Checked object-model prototype

[`mica/roe-model.mica`](../mica/roe-model.mica) defines the proposed relations and two behaviors as
ordinary Mica source. [`mica/roe-model-demo.mica`](../mica/roe-model-demo.mica) supplies explicitly
non-installed fixture state and invokes the complete workflow:

```text
normalized "C-x o"
  -> roe/SessionKeymap + roe/EffectiveBinding
  -> roe/CommandSelector(:roe/other_window)
  -> invoke(:roe/other_window, actor + session)
  -> retract/assert roe/ActiveView
  -> committed presentation_invalidated effect
  -> shared Phase 2 presentation stream
```

The result is `#roe/demo_view_b` and the effect names the same logical view. No Rust command object,
binding registry, mode object, `ChromeAction`, renderer object, endpoint token, or native resource
ID participates in the decision. This demonstrates that the ontology can represent a real
keymap/command/window workflow without duplicating native policy.

The durable `roe/core` source contains commands, keymaps, modes, hooks, faces, syntax,
configuration, packages, authority description, relation definitions, derived rules, and behavior.
The non-installed demo source contains competing policy examples plus session/frame/view/buffer
fixtures. Phase 4 replaces the latter with endpoint-volatile tuples asserted by the host. Every
host-managed session, logical-object, and native-association relation is declared `:volatile` in
Mica source, which is required by `open_endpoint_with_context_and_volatile_tuples_named` and the
volatile assertion/retraction APIs. Durable `Delegates` facts classify durable policy identities
only; ephemeral identities are classified by explicit volatile relations and do not need a
delegation edge.

## Identity and invalidation rules

| Mica association                                     | Native correlate                           | Invalidation rule                                                                                               |
| ---------------------------------------------------- | ------------------------------------------ | --------------------------------------------------------------------------------------------------------------- |
| `roe/SessionEndpoint(session, endpoint)`             | driver endpoint identity                   | retract on normal close, driver shutdown, or fatal driver loss; reject all later submissions                    |
| `roe/NativeTextResource(buffer, resource)`           | generation-checked `ResourceId`            | retract before/with buffer removal or endpoint close; host revocation advances generation even if cleanup fails |
| `roe/NativeResourceGeneration(resource, generation)` | resource slot generation observation       | replace only after successful registration; retract on any stale-resource completion                            |
| `roe/NativeBufferRevision(buffer, revision)`         | native buffer observation                  | replace on successful mutation/snapshot; discard on resource invalidation or resync                             |
| `roe/PresentationRevision(session, revision)`        | Phase 2 presentation stream                | advance only with an accepted update; discard deltas after a gap and request full snapshot                      |
| subscription mailbox/cursor                          | driver mailbox capability                  | cancel before endpoint close; recreate after reconnect or a detected delivery gap                               |
| task/request association                             | Mica `TaskId` and external-request context | cancel on user request, endpoint close, timeout, unit disable where owned, or driver shutdown                   |

No row is eligible for durable fileout. Native integers are diagnostic values, not authority.

## Driver event mapping

| Driver event or interaction  | Roe host interpretation                                                       |
| ---------------------------- | ----------------------------------------------------------------------------- |
| named-role invocation result | command completion or contextual task diagnostic                              |
| `DriverEvent::Effect`        | renderer-neutral presentation/echo/quit invalidation                          |
| external request             | capability-checked `NativeOperation`; bounded completion resumes the task     |
| task failure/abort           | non-fatal diagnostic with task, selector, endpoint/session, and failure class |
| task cancellation            | explicit session lifecycle cancellation result                                |
| `SubscriptionReady`          | coalesced wake; drain session mailbox and translate settled relation changes  |
| endpoint close               | retract volatile tuples, cancel work, invalidate native associations          |
| fatal driver failure         | recovery-only mode, full diagnostics, native association invalidation         |

One host pump consumes driver events. Both terminal and Vello continue to consume only the shared
`SessionOutput` and `PresentationUpdate` contract.

## Startup, replacement, close, and recovery

The embedded recovery unit is the only builder-time unit. Roe checks the main `roe/core` source,
installs it as a named unit before opening normal endpoints, and otherwise exposes a recovery-only
session. Package replacement is check-then-`FileinMode::Replace`; the active unit revision changes
only after success. Malformed source therefore cannot erase the last working unit.

Normal endpoint close first stops new input, then awaits driver cancellation/retraction, invalidates
native associations, drains retained events, and releases frontend state. Process shutdown keeps the
Compio runtime and the sole event consumer alive while the idempotent driver shutdown future drains
its bounded queue.

Recovery can show diagnostics, check/reload/replace a named unit, disable a package, fileout units,
request a full snapshot, and close. It cannot edit native buffers or grant itself authority through
an unvalidated scripting shortcut.

## First-wave Rust bypass and deletion target

Phase 4 routes the selected Mica-owned binding before transitional Rust `Editor::key_event` and uses
Mica command discovery/selector facts. It does not register a shadow Rust command. Unclaimed keys
may continue through the current Rust path until their Phase 5 slice migrates.

The first wave bypasses `CommandRegistry`, `CommandMode`, and `ConfigurableBindings` for the
selected key. Phase 5 then removes those types, the remaining modes/selection menus, and finally
policy portions of `Editor`, `BufferHost`, `EditorAction`, and `ChromeAction` in the order and with
the acceptance gates recorded in ADR 0005. `NativeKernel`, Rope/undo mechanisms, session protocol,
presentation model, and renderer realization remain Rust.

## Verification and exit assessment

The ontology and workflow were checked through Mica's driver-backed runner at the pinned revision:

```sh
cargo run --manifest-path ../mica/Cargo.toml -p mica-runner --bin mica -- \
  eval --filein "$PWD/mica/roe-model.mica" --filein "$PWD/mica/roe-model-demo.mica" \
  --actor roe/demo_actor \
  'return roe/dispatch_key(#roe/demo_actor, #roe/demo_session, "C-x o")'
```

The runner opens a real non-root driver endpoint for `#roe/demo_actor`; the task returns
`#roe/demo_view_b` and emits `presentation_invalidated` for that view. The fixture composes a local
map inheriting the global binding with a competing lower-precedence map, so the result also checks
inheritance, composition, and precedence. Roe's workspace checks remain unchanged because Phase 3
adds no runtime dependency or production path.

The remaining policy composition is exercised through the same non-root endpoint:

```sh
cargo run --manifest-path ../mica/Cargo.toml -p mica-runner --bin mica -- \
  eval --filein "$PWD/mica/roe-model.mica" --filein "$PWD/mica/roe-model-demo.mica" \
  --actor roe/demo_actor \
  'return roe/demo_policy_probe(#roe/demo_actor, #roe/demo_session)'
```

The result is `:ok` only when command arguments, prompt/completion metadata, authority-filtered
discovery, major/minor hooks and syntax, face inheritance/override, and configuration
inheritance/override all resolve through their derived relations.

Phase 3 exit criteria are met: the checked ontology represents a real workflow without Rust policy,
every native association has an invalidation rule, effects/requests/subscriptions have distinct
editor uses, lifecycle and recovery paths are specified, and the first-wave bypass plus Phase 5
deletion map is explicit.
