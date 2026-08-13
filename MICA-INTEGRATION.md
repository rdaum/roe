# Mica as Roe's implementation language

## Purpose

This is a handoff note for exploring Mica as Roe's embedded programming language. It records the
current direction, known problems, and places where Roe's present architecture may be at the wrong
level of abstraction. It is intentionally not a detailed implementation plan. The first integration
spike should be allowed to discover and reshape the boundary.

## Goal

The ambition is closer to GNU Emacs built over Emacs Lisp, or Lem built over Common Lisp, than to an
editor with a narrow plug-in API. Mica should become the live programming substrate in which much of
Roe is described and extended.

Rust should provide the small, efficient native substrate:

- terminal and graphical event-loop integration;
- rendering, text layout, and platform input;
- native text storage and operating-system resources;
- file watching, subprocesses, clipboard access, and similar services; and
- the embedded Mica runtime and the boundary between Mica and native resources.

Mica should eventually own most programmable editor policy and behaviour:

- commands and interactive commands;
- keymaps and keymap composition;
- major and minor modes;
- hooks, syntax rules, indentation, and configuration;
- minibuffer-like interactions and completion;
- packages and live replacement of editor behaviour;
- the live objects representing buffers, windows, sessions, and other editor concepts; and
- durable user and workspace state where persistence is appropriate.

The exact ownership of text, cursor state, and the editor object graph should be discovered through
the integration. Native storage can remain in Rust while Mica objects provide identity, policy, and
behaviour. Native handles and GPU resources must remain ephemeral; durable Mica relations should
store descriptions and policy, not live Rust capabilities.

## Current trajectory

Roe's Julia embedding has been removed. Built-in keybindings and commands temporarily live in Rust,
and both the terminal and Vello frontends now use Compio. Roe and Mica pin the same Compio release,
`0.18.0`, which removes the need to reconcile two asynchronous runtimes.

Mica already has most of the execution machinery needed for a prototype:

- a Compio-driven task driver and scheduler;
- transactional tasks over the relation store;
- suspension and resumption through input, timers, mailboxes, spawning, and external requests;
- effects and subscriptions for observable changes;
- endpoints for host sessions;
- in-memory and persistent worlds; and
- authority derived from durable policy.

The desired high-level flow is:

```text
native input or platform event
    -> invoke Mica behaviour
    -> Mica task reads or changes editor state
    -> Mica emits an effect or requests native work
    -> Roe applies the native operation
    -> completion or changed state wakes Mica and/or requests a redraw
```

There should be one process-long Mica driver with an explicit, modest resource budget. A daemon,
transport protocol, second scheduler, or compatibility layer for Julia is not required.

## Problems already identified in Roe

### Vello does not continuously drive Compio

The Vello frontend enters the Compio runtime only while handling a Winit `window_event`, while the
Winit loop uses `ControlFlow::Wait`. Compio work that becomes ready independently cannot wake Winit.
A detached task, timer, subscription, or completed Mica external request may therefore stall until
the user produces another window event.

This must be fixed before relying on background Mica work. The integration needs an explicit bridge
between Winit wakeups and Compio progress, likely involving Winit user events or an event-loop
proxy. Repeatedly wrapping individual callbacks in `block_on` is useful as a temporary porting
measure, but it is not the final event-loop architecture.

The terminal frontend is a simpler first host because its whole event loop already runs inside
Compio.

### Roe's actor mailboxes became unbounded

The Compio migration replaced bounded Tokio channels for buffer hosts and mode actors with unbounded
Futures channels. That removes backpressure and permits an input producer or Mica task to allocate
messages faster than the editor can process them. This is particularly undesirable in light of the
recent ENOMEM investigation.

Restore bounded delivery, or replace these actors as part of the Mica integration. No host-facing or
Mica-facing queue should have accidental unlimited growth.

### The Compio migration needs ordinary cleanup

The migration compiles and its focused converted tests pass, but the commit is not `rustfmt` clean.
The full Roe test suite also depends on the global system clipboard: kill-ring tests interfere with
one another and can fail based on external clipboard contents, even when run serially. This is
existing test-harness debt rather than evidence of a Compio failure, but it will make integration
work harder to validate.

### GPU dependency versions are not aligned

Roe currently gets WGPU 26 through Vello 0.6, while Mica currently declares WGPU 30 for relation
acceleration. Mica's driver makes its WGPU provider optional, but enables it by default. A first Roe
spike should disable Mica's WGPU feature and use CPU relation execution. Later work can either align
the dependency versions or define a deliberate host-provided GPU resource path. Version alignment
alone does not cause the renderer and relation accelerator to share a device.

## Mica's embedding boundary is ready for a Roe spike

The Mica cleanup originally identified for native embedding has been implemented. Roe can build its
first integration against the current `mica-driver` contract rather than waiting for another Mica
API pass:

- shutdown is shared and idempotent, cancels tracked asynchronous work, flushes persistence, and
  joins the dispatcher;
- individual tasks and endpoint-scoped work have explicit cancellation semantics;
- timers, mailbox waits, spawning, external requests, and background endpoint closure are tracked as
  driver-owned work;
- driver events, external-request admission, and subscription queues are bounded, with documented
  ordering, coalescing, and backpressure;
- checked filein, named-unit installation and replacement, include loading, and fileout are
  available after startup;
- `mica-driver` is the host-facing crate and provides a builder covering storage, resources, initial
  units, and external handlers;
- the driver allocates ephemeral host identities from one checked sequence;
- worker count, affinity, relation parallelism, task limits, queue budgets, persistence, and
  relation acceleration are explicit configuration choices; and
- `inner_runner()` is private to the driver crate, while the supported host operations are exposed
  directly.

The driver README and `examples/compio_host.rs` define and exercise the intended embedding contract.
Roe should use that surface first and propose Mica changes only when a concrete editor vertical
slice exposes a missing operation or a misshapen boundary.

There are operational constraints rather than missing facilities: a host must keep its Compio
runtime alive through driver shutdown, continue draining a full bounded event queue while shutdown
completes, and cooperate with cancellation for operating-system work started inside an external
handler. Persisted stores and compiled programs also do not yet have a cross-version compatibility
promise, so a host should pin an exact Mica revision.

## Roe abstractions that may be misplaced

The following are questions and likely pressure points, not conclusions. Prefer changing a misshapen
boundary cleanly over preserving it as a compatibility API.

### `Editor` currently owns too much policy

`Editor` coordinates buffers and windows, interprets commands, creates interactive selector windows,
processes action protocols, performs file operations, and mediates modes. Some of that is native
editor mechanism, but much of it is exactly the policy that Mica should own.

The eventual Rust editor core may be smaller: native state and operations with explicit invariants,
plus a host boundary through which Mica composes those operations.

### Frontends duplicate editor semantics

The terminal and Vello frontends both inspect and interpret `ChromeAction` variants. Renderers
should not independently know how commands, buffers, hooks, or modes work. Ideally they consume a
shared stream of native presentation changes and forward platform input through one host layer.

The current `ChromeAction`, `BufferResponse`, `ModeAction`, and `EditorAction` families may be
several overlapping protocols for one conceptual boundary. The spike should test whether they can be
collapsed into clearer native operations, Mica invocations, effects, and render invalidations.

### Roe's actor topology may duplicate Mica's scheduler

`BufferHost` and `ModeActor` give every buffer and mode a Rust task and mailbox. Mica already has
tasks, suspension, mailboxes, subscriptions, and transactional state. Retaining both models could
produce a scheduler within a scheduler and obscure cancellation, ordering, and ownership.

A native buffer service may still need serialization, but Rust mode actors should not be assumed to
be the permanent extension boundary. Modes are strong candidates to become Mica objects and methods.

### Rust registries are likely bootstrap scaffolding

`CommandRegistry` and `ConfigurableBindings` are useful for keeping Roe operational after Julia's
removal. They should not automatically become the public Mica API. Commands, keymaps, inheritance,
and composition can be modelled more naturally as live relations and behaviour installed in Mica.

Keep only the minimal native bootstrap commands needed to start, recover, diagnose, or reload the
Mica environment.

### Native IDs and Mica identities need a lifecycle boundary

Roe uses ephemeral `SlotMap` IDs for buffers, modes, and windows. Mica uses durable object
identities and relations. The integration needs an explicit mapping and lifetime rule rather than
leaking one identity system into the other.

It may be appropriate for a Mica buffer object to refer indirectly to an ephemeral native text
resource. Closing a buffer must then retract or invalidate that association and cancel related work
without making the native handle durable.

### File and platform operations need one host boundary

File reads and writes currently occur in several Rust locations. Mica should orchestrate editor
behaviour without receiving unrestricted access to arbitrary native resources. External requests are
the likely mechanism for operations that return a value; effects are appropriate for observable
one-way changes. The distinction, cancellation behaviour, and authority checks should be explicit.

### Syntax, faces, and hooks should not remain accidental globals

The current global face registry and placeholder after-change handling were inherited from the Julia
design. Mica should be able to define faces, syntax behaviour, and hooks as live editor state, while
the renderer retains an efficient native cache. Cache invalidation and snapshotting are a better
boundary than exposing renderer internals to Mica.

## Suggested trajectory

1. **Stabilize the Compio substrate in Roe.** Restore bounded mailboxes, format the migration, and
   design a real Compio/Winit wakeup path. Isolate clipboard-dependent tests.
2. **Build one narrow terminal-first spike.** Start one Mica driver with fixed small limits, GPU
   acceleration disabled, and initial units loaded before startup. Open one editor endpoint.
3. **Exercise one complete vertical slice.** Send a keystroke or command invocation into Mica, let
   Mica decide an editor operation, apply it to a native buffer, and redraw from the resulting
   effect. Include one native external request round trip.
4. **Move policy incrementally.** Commands and keymaps are a good first slice, followed by
   minibuffer interactions and modes, then hooks, syntax, configuration, and package loading.
5. **Reshape Roe around what the spike reveals.** Remove duplicated action handling and Rust actor
   or registry layers where Mica has taken responsibility. Do not build adapters to preserve
   temporary APIs.
6. **Exercise the existing lifecycle support.** Verify post-start unit replacement, endpoint
   cancellation, bounded event delivery, persistence, and deterministic shutdown in Roe's actual
   event loops. Change Mica only where this integration reveals a concrete gap.
7. **Revisit graphical and GPU integration.** Connect Compio wakeups to Winit, then decide whether
   Mica's relation acceleration remains disabled, owns a separate device, or shares host-provided
   WGPU resources after version alignment.

## Useful principles for the next session

- Treat Mica as Roe's language, not as a collection of callbacks.
- Keep the native substrate small, explicit, bounded, and allocation-conscious.
- Let durable relations describe policy; keep runtime capabilities and native handles ephemeral.
- Prefer invocation, effects, subscriptions, and external requests over editor-specific Mica
  builtins in the Mica core.
- Do not preserve Julia-shaped APIs merely because they already exist in Rust.
- Do not expose the entire Mica runtime to Roe to avoid designing a small host contract.
- Use a real vertical slice to choose abstractions before attempting a wholesale rewrite.
- Keep terminal and Vello behaviour behind one editor host boundary even if their event-loop
  integration differs.

## Questions the first spike should answer

- What is the smallest native buffer/window API that lets Mica implement a useful command?
- Which editor state belongs transactionally in Mica, and which state is an ephemeral native cache?
- How are native resource identities associated with Mica objects and invalidated on close?
- Do Roe's buffer actors add value once Mica owns behaviour, or should they disappear?
- What event wakes each frontend when Mica work completes independently of user input?
- How should a Mica task request UI-thread-only work without deadlocking either event loop?
- Which bootstrap and recovery operations must remain native when Mica code fails to load?
- What ordering and backpressure guarantees are required between input, task completion, effects,
  subscriptions, and redraws?

The immediate success criterion is modest: prove one useful editor behaviour can live in Mica and
round-trip through Roe without bypassing the driver, duplicating schedulers, or depending on
unbounded queues. That experiment should guide the larger redesign.
