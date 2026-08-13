# Roe renovation and Mica integration roadmap

## Vision

Roe should become a **Mica Emacs**: a live, inspectable, programmable environment in the lineage of
Emacs and the Symbolics Lisp machines. A useful north star is **Genera for Mica with an Emacs-style
UI**.

Mica is a relation-first live programming environment. Objects have durable identities described by
facts rather than fixed record layouts; behaviour is installed as verbs and methods dispatched
through named roles and prototype delegation; and rules derive new relations from existing state.
Mica tasks execute transactionally over the live relation store and can suspend for input, timers,
mailboxes, spawned work, or native external requests. Effects and subscriptions connect committed
changes back to a host application. Its embedding driver runs on Compio and provides bounded task
scheduling, endpoint lifecycles, cancellation, authority derived from durable policy, and optional
in-memory or persistent worlds.

That means more than embedding a scripting language or exposing a plug-in API. Mica should be the
language in which editor behaviour, policy, configuration, and much of the live editor object graph
are expressed. Rust should provide a small, fast native substrate for text storage, platform access,
event loops, rendering, and other capabilities that are intrinsically native.

Roe began as a handwritten application and later accumulated LLM-assisted code produced by earlier,
less capable models. The result contains good original ideas, but its implementation is uneven:
responsibilities have drifted, some protocols overlap, error handling is inconsistent, tests are not
fully isolated, and recent runtime migration work is unfinished. We should renovate the existing
system without treating every current abstraction as permanent.

This roadmap expands on [MICA-INTEGRATION.md](MICA-INTEGRATION.md). That document is the handoff for
the embedding problem; this one defines the larger sequence of rehabilitation, architectural
renovation, integration, and incremental transfer of responsibility to Mica.

## Intended character

The completed system should have these properties:

- Mica is Roe's implementation language for programmable editor behaviour, not a collection of
  callbacks around a Rust editor.
- Rust owns native resources and mechanisms: rope storage, files, processes, clipboard access,
  platform events, text layout, GPU resources, terminal cells, and drawing.
- Logical editor objects and policy can live in Mica: commands, keymaps, modes, hooks, packages,
  configuration, logical buffers, logical windows, sessions, and durable workspace state.
- The view layer consumes a renderer-neutral presentation model. Terminal and Vello frontends do not
  interpret editor commands independently.
- Logical window management and input are not tied to a particular renderer. The same session model
  can support a terminal, a native window, tests, remote display, or a future separate process.
- Embedded operation is the first target. The architecture permits a process boundary without
  requiring a daemon or wire protocol in the first implementation.
- Every queue, task, native handle, endpoint, and identity has explicit ownership, bounds, and
  shutdown behaviour.
- A broken user program can be diagnosed and recovered from without making the editor unusable.

## Architectural principles

### Separate meaning from realization

Mica should decide what an input means and how editor policy composes. Rust should efficiently
realize the resulting native operations and presentation.

For example, Mica may decide that a key sequence invokes a command, that a command splits a logical
window, or that a modeline contains a particular set of fields. Rust may own the rope mutation,
layout algorithm, terminal cell production, glyph shaping, and Vello scene construction used to
realize those decisions.

### Keep the native boundary small and capable

Mica should reach the native substrate through a coherent editor host API rather than through layers
modelled after Roe's current `ModeAction`, `EditorAction`, `BufferResponse`, and `ChromeAction`
types. The boundary should consist of a small vocabulary of operations, events, snapshots, and
effects with documented invariants.

“Closer to the metal” does not mean putting renderer or operating-system handles into Mica. It means
that Mica policy reaches the native mechanisms directly through this boundary, without a second Rust
policy framework standing between them.

### Preserve view/logic separation

The original Roe goal of separating the view layer from editor logic remains sound. We should make
the separation sharper:

```text
platform input                     Mica policy and live objects
      |                                      |
      v                                      v
frontend adapter -> session input -> editor host boundary
                                          |
                           +--------------+--------------+
                           |                             |
                           v                             v
                  native editor kernel          Mica driver/events
                           |
                           v
               presentation snapshot or delta
                           |
              +------------+-------------+
              |                          |
              v                          v
      terminal renderer            Vello renderer
```

The in-process implementation may use Rust enums and direct calls, but messages crossing the session
boundary should be owned data with explicit identities and ordering. A future transport can encode
the same semantics rather than reverse-engineering them from UI code.

### Separate durable description from ephemeral capability

Mica relations may durably describe a buffer, window, package, keymap, or file association. Native
rope instances, file descriptors, WGPU devices, Winit windows, subprocess handles, and capability
tokens remain ephemeral. Associations between the two need explicit creation, invalidation, and
generation checks.

### Earn abstractions with vertical slices

Do not design the entire editor ontology before running Mica inside Roe. Establish the smallest
useful end-to-end path, observe which current abstractions help or obstruct it, and reshape the
boundary. Avoid compatibility layers for Julia-era or temporary Rust APIs.

## Phase 0: establish the renovation baseline

### Goals

- Record current behaviour before structural work begins.
- Define the invariants that rehabilitation and integration must preserve.
- Make failures reproducible without relying on a user's terminal, GPU, filesystem, or clipboard.

### Work

1. Inventory the workspace, dependency graph, build targets, feature flags, and supported platforms.
2. Record representative workflows for both frontends. Exercise the production terminal adapter in a
   controlled pseudo-terminal and the production Vello adapter on a display-capable host. When a
   required platform is unavailable, record that limitation explicitly and retain the workflow as an
   open platform-smoke obligation; cover its renderer-neutral semantics headlessly rather than
   inventing a successful observation:
   - startup with no files and with one or more files;
   - text insertion, movement, region operations, undo, and save;
   - command selection, buffer selection, file selection, and incremental search;
   - splitting, selecting, resizing, and deleting windows;
   - external file change detection; and
   - clean shutdown and failure during shutdown.
3. Add or identify renderer-neutral tests for editor semantics before moving those semantics.
4. Introduce injectable clipboard, clock, filesystem, and native-service boundaries where global
   state currently makes tests interfere.
5. Capture coarse baselines for startup time, idle memory, basic editing latency, and redraw cost.
   These are regression guards, not an optimization programme.
6. Document explicit invariants for buffer positions, UTF-8/character indexing, window-tree
   validity, dirty-region delivery, and task/endpoint lifetime.

### Exit criteria

- The normal workspace check and focused editor tests have documented commands and stable results.
- Clipboard-dependent tests do not mutate or depend on the user's global clipboard.
- Terminal and Vello behaviour have a small shared conformance suite.
- Known failures and intentional omissions are written down rather than hidden by broad ignores.

## Phase 1: rehabilitate Roe

This phase improves the current system before asking it to host another runtime. It should use
small, reviewable changes rather than combine dependency migration, error-model changes, and
architecture work in one patch.

### Phase 1A: dependency and toolchain renewal

1. Add a declared Rust toolchain/MSRV policy and workspace-level package metadata.
2. Use `cargo outdated`, the upstream changelogs, and a lockfile audit to classify dependencies:
   - straightforward compatible updates;
   - major updates needing local migration;
   - coupled graphical updates such as Vello, WGPU, Winit, and Parley; and
   - deliberately pinned dependencies such as the Compio version shared with Mica.
3. Update low-risk dependencies first, one coherent group at a time.
4. Upgrade the rendering stack as a dedicated change with terminal and Vello smoke tests.
5. Evaluate Rust 2024 edition migration after the dependency updates. Do not mix an edition
   migration with functional changes.
6. Add repeatable checks for formatting, clippy, dependency policy, and security advisories.
7. Remove dependencies and features that are no longer used after Julia's removal.

“Latest” is constrained by compatibility and the integration target. Roe and Mica should continue to
use the same Compio release, and the first Mica wave should disable Mica's default WGPU feature
until the renderer and relation-acceleration stacks have an intentional device/version strategy.

### Phase 1B: code health and error model

1. Make the tree `rustfmt` clean and establish formatting as a required check.
2. Audit `unwrap`, `expect`, panic, and ignored-result sites by boundary:
   - retain assertions only for genuine internal invariants;
   - return structured errors for user input, files, clipboard, renderer surfaces, and runtime work;
   - attach operation and resource context once, near the failing boundary; and
   - ensure terminal restoration and driver shutdown happen on every exit path.
3. Replace ad hoc `String` errors in core protocols with focused error types where callers need to
   distinguish recovery, cancellation, invalid state, and fatal failure.
4. Add tracing at lifecycle boundaries: frontend startup, endpoint creation, task submission, native
   request, buffer mutation, redraw request, endpoint close, and shutdown.
5. Break up long methods when the extracted unit has a clear responsibility. Do not create generic
   managers or services merely to reduce line count.
6. Remove dead Julia-era concepts and comments, while retaining only explicitly chosen bootstrap
   infrastructure.
7. Review buffer locking and shared ownership. Establish where serialization is required and where
   the current `Arc<RwLock<_>>` model permits surprising cross-task mutation.

### Phase 1C: Compio and event-loop repair

1. Replace unbounded buffer-host and mode-actor mailboxes with bounded delivery, or remove an actor
   layer when a direct serialized service is simpler.
2. Specify queue capacities, ordering, overload behaviour, cancellation, and shutdown for every
   remaining queue.
3. Give detached work an owner. Buffer hosts, modes, file watching, and future Mica tasks must be
   joined or cancelled through a session lifecycle.
4. Build a real Winit/Compio bridge:
   - use a Winit user event or event-loop proxy to wake the UI thread;
   - allow Compio work, timers, file events, and Mica driver events to request progress/redraw;
   - avoid treating `runtime.block_on` inside each `window_event` as the final architecture; and
   - test completion when no keyboard or mouse event arrives.
5. Make the terminal loop and graphical loop consume the same host/session outputs even though their
   platform wakeup mechanisms differ.
6. Define shutdown order: stop accepting input, close the editor endpoint, cancel native work, drain
   required events, shut down Mica, release renderer/platform resources, and restore the terminal.

### Exit criteria

- Current Roe behaviour remains usable in both frontends.
- Workspace formatting, checking, clippy, and isolated tests pass.
- No host-facing queue is accidentally unbounded.
- Background work wakes both frontends without requiring incidental input.
- Runtime and actor work has deterministic cancellation and shutdown.
- Errors from normal external failures reach the user without panics.

## Phase 2: renovate the Roe architecture

The goal is not to perfect a Rust editor before Mica arrives. The goal is to expose a native kernel
and presentation boundary that Mica can use directly.

### Phase 2A: identify the native editor kernel

Define the smallest native mechanisms that benefit from remaining in Rust:

- text resources backed by Ropey, including mutation, snapshots, spans, and undo primitives;
- ephemeral resource allocation with generation-checked identifiers;
- validated window-tree/layout primitives;
- file, clipboard, clock, process, and watcher operations;
- presentation extraction needed by renderers; and
- lifecycle and cancellation for native resources.

The kernel should enforce mechanical invariants but should not decide command names, keybindings,
major modes, completion policy, package policy, or hook composition.

Create explicit operation and result types. Candidate operations include buffer snapshot, insert,
delete, replace, set selection, create/close resource, mutate logical view layout, and request
native services. Final names and granularity should come from the first vertical slices.

### Phase 2B: create one presentation boundary

1. Replace frontend interpretation of `ChromeAction` with a shared host/session layer.
2. Define a renderer-neutral presentation snapshot or versioned delta containing only what a view
   needs: logical windows, geometry, visible buffer slices, selections, faces, modeline/chrome data,
   echo area, cursor state, and invalidations.
3. Keep shaping, terminal-cell mapping, glyph caches, surfaces, and drawing inside each renderer.
4. Keep chrome **rendering** in Rust. Allow Mica to describe chrome content and policy without
   exposing renderer internals.
5. Give snapshots/deltas monotonically increasing revisions so a slow or remote view can detect a
   gap and request a fresh snapshot.
6. Run terminal and Vello against the same scripted sequence and compare their logical presentation
   output before renderer-specific realization.

### Phase 2C: define a transport-neutral session boundary

The first implementation remains in process, but its semantics should survive a process boundary.

Define four categories rather than one catch-all action enum:

- **Input events:** normalized keys, text input, pointer actions, resize, focus, and native
  notifications.
- **Editor operations:** validated requests to native resources and logical view primitives.
- **Presentation output:** snapshots, deltas, invalidations, messages, and cursor/chrome updates.
- **Lifecycle events:** session open/close, resource invalidation, cancellation, overload, and fatal
  failure.

Requirements:

- owned, serializable data without Rust references or renderer handles;
- explicit session, object, resource, and revision identities;
- documented ordering and backpressure;
- request IDs where acknowledgement matters;
- idempotence or duplicate detection where retries may later be possible; and
- capability checks at the host boundary, not implicit trust based on an integer ID.

Do not implement a network transport in this phase. Prove the boundary with an in-process adapter
and a deterministic test harness. A loopback/process test can follow once the semantics stabilize.

### Exit criteria

- Terminal and Vello forward input and render presentation through the same session contract.
- Frontends no longer execute editor commands or manage modes.
- The native kernel can be tested without either renderer.
- A session transcript can be replayed through a headless host.
- Current overlapping action families have either a clear temporary role or a removal plan.

## Phase 3: design the Mica editor architecture

This phase turns the session boundary into a Mica-native editor model. Produce a short architecture
decision record for each ownership choice; do not encode undecided ownership accidentally in public
APIs.

### Phase 3A: define the live object model

Prototype the Mica relations and behaviours for:

- editor sessions and actors;
- logical buffers and their association with ephemeral native text resources;
- frames, logical windows, view trees, active views, cursors, marks, and selections;
- commands, interactive argument acquisition, and command discovery;
- keymaps, key sequences, precedence, inheritance, and composition;
- major and minor modes;
- hooks, faces, syntax rules, and configuration; and
- packages, named units, and live replacement.

Classify each fact as durable, session-volatile, derived, or native-cached. In particular, native
resource IDs and endpoint relations must never become durable capabilities.

### Phase 3B: map the driver lifecycle

Use the current `mica-driver` embedding contract directly:

1. Pin one exact Mica revision with `default-features = false`.
2. Construct one process-long `CompioTaskDriver` with explicit small budgets and relation
   acceleration disabled.
3. Install checked initial Roe units before startup.
4. Allocate and open one endpoint per editor session using ephemeral identities.
5. Translate normalized input and host notifications into named-role invocations or endpoint input.
6. Consume `DriverEvent` from one logical event consumer.
7. Use effects for committed observable changes and redraw/presentation invalidation.
8. Use external requests for native work that must return a result. Handlers must honour endpoint,
   task, authority, timeout, and cancellation context.
9. Use subscriptions where changes to relations should wake a session without polling.
10. On close, cancel endpoint work and invalidate its native associations. Keep the Compio runtime
    alive while draining required events and awaiting idempotent driver shutdown.

### Phase 3C: define authority and recovery

1. Define relations controlling which actors may invoke editor behaviours, emit host-visible
   effects, access buffers, and request file/process/clipboard services.
2. Map authority to native service checks without persisting capability values.
3. Keep a minimal native recovery surface:
   - start with a safe built-in unit;
   - show diagnostics;
   - reload or replace a named unit;
   - disable a failing package;
   - export/fileout programmable state; and
   - close cleanly.
4. Define task failure presentation so a bad command reports context without killing the session.
5. Define persistence policy. Begin with an in-memory world; introduce durable user/workspace state
   only with revision pinning, backup, export, and migration expectations.

### Exit criteria

- The proposed ontology can represent one real command/keymap/window workflow without native policy
  objects duplicating it.
- Every Mica identity associated with a native resource has an invalidation rule.
- Effects, external requests, and subscriptions each have a defined editor use.
- Startup, failed initial unit, package replacement, endpoint close, and shutdown are specified.
- The design identifies which Rust actors, registries, and action types the first integration wave
  will remove or bypass.

## Phase 4: first Mica integration wave

The first wave should run in the terminal frontend and prove a complete useful behaviour. It should
not attempt to migrate every command or model the whole editor.

### Phase 4A: embed the driver behind the session host

1. Add the exactly pinned CPU-only `mica-driver` dependency.
2. Start the driver inside the existing process-long Compio runtime.
3. Load a small Roe bootstrap unit and open an editor endpoint.
4. Add the driver event stream to the terminal session loop with bounded delivery.
5. Implement cancellation-aware external handlers for the minimal native kernel operations.
6. Route effects into shared presentation invalidation rather than terminal-specific rendering.
7. Implement deterministic endpoint close and driver shutdown tests.

### Phase 4B: prove one command and keymap vertical slice

Choose one useful, testable Mica-defined command whose entire policy lives in Mica. A good initial
slice is a command bound by a Mica keymap that:

1. receives normalized input through a named-role invocation;
2. reads the active logical buffer/window context;
3. requests one native value or operation through an external request;
4. applies a validated native buffer operation;
5. commits an observable effect or relation change; and
6. causes the terminal to redraw through the common presentation path.

An `insert-current-date` or similarly bounded command is a useful harness because it exercises a
cancellation-aware native clock request and a text mutation while remaining deterministic under an
injected test clock. The exact user-facing command can change if the integration reveals a better
slice; the required round trip should not be weakened.

The Mica unit must define both command discovery and the key binding. Do not register a shadow Rust
command merely to make the demonstration work.

### Phase 4C: exercise lifecycle, replacement, and failure

Before calling the wave complete, verify:

- the named Roe unit can be replaced after startup and the next invocation uses it;
- a malformed replacement is rejected without losing the last working unit;
- a command failure appears in Roe diagnostics and the editor remains usable;
- queue saturation applies defined backpressure rather than growing memory;
- endpoint closure cancels a suspended external request;
- shutdown completes with a full event queue while the host continues draining it; and
- no input event is required to wake completed Mica work.

### Exit criteria

- One useful editor behaviour is implemented in Mica end to end.
- The path uses the public driver API, not Mica runtime internals or editor-specific Mica builtins.
- There is one process-long driver, one endpoint lifecycle, and no accidental unbounded queue.
- Rust owns the native buffer/resource operation and rendering; Mica owns the command and binding.
- The same host-level output is suitable for Vello even though Vello is not yet enabled for the
  first wave.
- The spike produces concrete decisions about the permanent editor host boundary.

## Phase 5: transfer editor policy to Mica

Move responsibility in coherent vertical slices. Each slice should delete or shrink the superseded
Rust policy path rather than create permanent dual ownership.

Suggested order:

1. command definitions, discovery, invocation, and keymaps;
2. minibuffer interactions, completion, and command argument acquisition;
3. buffer selection, file selection, and incremental search;
4. major/minor modes and hook composition;
5. faces, syntax rules, indentation, and renderer cache invalidation;
6. logical frames/windows and view policy;
7. configuration, packages, live replacement, and recovery tooling; and
8. optional durable user and workspace state.

For each slice:

- name the Mica-owned relations and behaviours;
- name the Rust mechanisms that remain;
- migrate terminal and Vello through the common session boundary;
- test replacement, cancellation, authority, and failure;
- remove the displaced Rust registry/actor/action path; and
- remeasure editing latency, redraw behaviour, and memory growth.

## Phase 6: grow from editor to live environment

Once both frontends use the Mica-owned editor model, pursue the broader Lisp-machine character:

- first-class inspectors for objects, relations, tasks, packages, and authority;
- live browsing and replacement of editor and application behaviour;
- integrated source, diagnostics, task, and relation views;
- recoverable workspaces and session state;
- multiple simultaneous views over one live world;
- remote or separate-process frontends using the established session semantics;
- application surfaces beyond text editing that reuse the same chrome and interaction model; and
- deliberate GPU sharing or separate-device policy after Roe and Mica's WGPU stacks align.

These are product directions, not requirements for the initial embedding. The earlier phases should
avoid choices that make them impossible, but should not build unused distributed infrastructure in
advance.

## Cross-cutting acceptance rules

Every phase must preserve these rules:

- **No hidden unbounded growth.** Queues, histories, caches, and task sets have bounds or explicit
  retention policy.
- **No accidental dual ownership.** A behaviour has one authoritative implementation during normal
  operation.
- **No renderer policy forks.** Terminal and Vello may realize presentation differently but do not
  assign different meanings to editor actions.
- **No durable native capabilities.** Persist descriptions and policy, never live handles or
  authority tokens.
- **No panic for ordinary failure.** External failures are contextual errors; internal invariant
  failures remain loud.
- **No integration without lifecycle tests.** Startup, cancellation, endpoint closure, overload, and
  shutdown are part of the feature.
- **No architecture claims from compilation alone.** Exercise the real event loops and one complete
  behaviour round trip.
- **No premature transport.** Keep messages transport-neutral, but stabilize semantics in process
  before adding a daemon or remote frontend.
- **No permanent bootstrap sprawl.** Native recovery commands stay intentionally small.

## Immediate next work

The next implementation session should begin with Phase 0 and Phase 1C evidence gathering, then
produce small commits in this order:

1. isolate clipboard-dependent tests and record the baseline checks;
2. make the current Compio migration format-clean;
3. inventory and group dependency updates;
4. bound or replace the buffer/mode actor channels;
5. introduce an owned session lifecycle for background work; and
6. design and test the Winit/Compio wakeup bridge.

In parallel at the design level, sketch the native text-resource operations and renderer-neutral
presentation snapshot needed by the first Mica command. Do not perform a wholesale `Editor` rewrite
before that command has exercised the boundary.
