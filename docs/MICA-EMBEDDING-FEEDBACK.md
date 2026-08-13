# Mica embedding feedback from Roe

This is a handoff to an agent working on Mica. It records the friction encountered while turning
Roe from an editor with Rust-owned policy into an editor whose commands, keymaps, prompts, modes,
hooks, faces, syntax, configuration, packages, and logical window decisions are owned by Mica.

This is not a general review of Mica and it is not a claim about Mica `main`. The observations are
from Roe at commit `1fbfc1b4d5fcf4f6677975e6ddf63fd4cec0d3fa`, using the public `mica-driver` API
at exact revision `a13f479229b761bf45b7ef71802cd4ca6e588dd4`. Some items below may already have
been improved elsewhere. Where it was unclear whether a feature was absent or merely difficult to
discover, this note says so.

The most useful Roe-side evidence is:

- [`roe-core/src/mica_host.rs`](../roe-core/src/mica_host.rs), the driver, endpoint, event,
  volatile-state, recovery, and value-decoding bridge;
- [`mica/roe-model.mica`](../mica/roe-model.mica), the durable ontology, authority rules, and
  editor-policy verbs;
- [`mica/roe-first-wave.mica`](../mica/roe-first-wave.mica), the installed commands, prompts,
  completion, search, and policy projection;
- [`docs/PHASE-4-MICA-INTEGRATION.md`](PHASE-4-MICA-INTEGRATION.md) and
  [`docs/PHASE-5-POLICY-TRANSFER.md`](PHASE-5-POLICY-TRANSFER.md), the implementation records; and
- [`MICA-INTEGRATION.md`](../MICA-INTEGRATION.md), the expectations before the integration began.

## Executive summary

The integration succeeded. Mica proved capable of owning real editor semantics instead of acting as
a callback or configuration language. Transactions, derived relations, endpoint authority,
external requests, effects, checked filein, volatile facts, bounded resources, and explicit
shutdown were all valuable foundations.

The main difficulty was that a production embedder still has to assemble a substantial amount of
lifecycle and protocol machinery around those foundations. Roe had to implement its own task-event
correlator, sole-consumer discipline, foreign-event-loop wake path, volatile-object synchronizer,
effect schema and decoder, layered authority mapping, recovery choreography, and contextual
diagnostic formatting. These are not editor-specific problems. A smaller high-level embedding layer,
built on the existing driver primitives, would make Mica much easier and safer to adopt.

On the language side, the relational model was expressive, but common policy operations became
more imperative and shape-dependent than expected. The sharpest edges were `one` changing result
shape based on projection count, query variables not becoming lexical bindings, untyped map/symbol
protocols, the lack of semantic or structural types, missing collection/query conveniences, manual
functional-relation updates, dynamic invocation without a checked signature, and limited runtime
diagnostic context.

## What worked well

These should be preserved while improving the higher-level surface:

- `mica-driver` is a real host boundary; Roe did not need to reach into runtime internals.
- Fixed resource budgets made task, event, external-request, subscription, and shutdown behaviour
  reviewable. Backpressure is much better than an accidental unbounded host bridge.
- `ExternalRequestContext` carries task, actor/principal, endpoint, and cancellation information.
  That was enough to enforce native authorization and stop work on cancellation.
- Checked filein and named Add/Replace operations let Roe keep the last working program after a bad
  replacement.
- Endpoint-volatile tuple installation permits session state and authority to be present atomically
  when an endpoint becomes usable.
- Effects, external requests, and subscriptions have deliberately different semantics. That
  separation is useful even though the host plumbing could be easier.
- Exact value-kind annotations, the centralized builtin catalogue, and the language book's
  current explanation of relational query values are good direction. In particular, the builtin
  reference clearly states that string indexing is by Unicode scalar value; Roe depends on that.

## Embedding and integration pain points

### 1. A minimal embedding profile is needed

Roe must pin a Git revision, disable all default features, explicitly select CPU relation
execution, and raise its MSRV from Rust 1.88 to 1.95. Mica's defaults include Cranelift, Fjall,
source providers, and WGPU. The WGPU feature also brought a WGPU version incompatible with Roe's
Vello renderer even though Roe did not need relation acceleration.

The existing feature controls made a CPU-only integration possible, which is good. The remaining
problem is discoverability and dependency footprint. Consider:

- a documented `minimal-embedder` or `cpu-memory` feature profile;
- publishing the host-facing crate independently, if the internal crate graph permits it;
- a compatibility table covering Rust MSRV, storage format, program-artifact format, Compio, and
  optional WGPU versions; and
- keeping the minimal driver graph free of unrelated source-provider and GPU dependencies by
  construction, not only by carefully disabling defaults.

Exact revision pinning is reasonable while persisted formats are unstable, but it makes an upgrade
an application migration rather than an ordinary dependency update. A concise upgrade checklist or
machine-readable compatibility marker would help.

### 2. The low-level lifecycle is correct but easy to assemble incorrectly

A realistic host has to perform all of the following in the correct order:

1. choose resource budgets and build the driver;
2. check and install initial named units;
3. allocate endpoint, actor, session, frame, buffer, native-resource, and view identities;
4. open an endpoint with actor context and a large initial volatile tuple set;
5. submit invocations and correlate their `TaskId`s with terminal events;
6. retain or route unrelated effects, failures, cancellations, and subscription wakes;
7. keep draining events during endpoint close and driver shutdown so bounded producers can finish;
8. retract every volatile association and cancel native work; and
9. keep the Compio runtime alive until shutdown completes.

The minimal `compio_host` example shows the happy path, but it does not demonstrate named units,
volatile endpoint context, multiple in-flight tasks, subscriptions, queue saturation, replacement,
or deadlock-safe close. Roe's resulting bridge is large partly because the public API exposes all of
these pieces separately.

A higher-level session/task API could preserve the low-level primitives while making the safe path
shorter. For example, an endpoint session object could own a scoped volatile fact set and return an
invocation handle whose completion method drains the shared event stream without losing unrelated
events. A second, production-shaped example should cover the complete lifecycle above.

### 3. The one-consumer event contract is a convention rather than an abstraction

`wait_events` and `drain_events` divide one stream, so the host must arrange exactly one logical
consumer. Roe has one custom loop that correlates the awaited task while translating unrelated
events into a retained batch. Dispatch, idle progress, replacement, endpoint close, and shutdown
must all cooperate with it.

This is subtle when an application has multiple sessions or when one component wants to await a
specific task while another pumps background work. It is easy to consume another task's terminal
event, discard an effect, or deadlock shutdown behind a full event queue.

Possible improvements include:

- a driver-owned event demultiplexer with task handles plus a separate ordered host-event stream;
- an `invoke_and_wait` API that preserves unrelated events through the canonical consumer;
- an explicit event-pump object that cannot be cloned into competing consumers; and
- a shutdown helper that drains the queue as part of the contract instead of requiring the host to
  spawn shutdown and poll it while draining.

Whichever design is chosen, effect-before-terminal-event and subscription-after-commit ordering
must remain explicit.

### 4. Foreign event loops need a first-class wake integration

Mica runs on Compio, while Roe's graphical frontend is owned by Winit. A Mica timer, completed
external request, or subscription can become ready without any keyboard or window event. Roe had
to build a coalesced `EventLoopProxy` wake bridge and a periodic Compio pump; the terminal frontend
uses a 20 ms idle tick for the same general progress requirement.

Mica should not depend on Winit, but `mica-driver` could expose a small readiness/waker contract and
document how to connect it to a foreign loop. Useful deliverables would be:

- a callback or edge-triggered readiness registration with one-outstanding-wake semantics;
- explicit rules for acknowledging and rearming readiness without losing a wake;
- examples for Winit and a poll/select-style terminal loop; and
- a test adapter that proves a timer or external completion progresses without user input.

### 5. Effects and external-request payloads need schemas or typed codecs

Mica-to-host communication is made of general `Value`s. Roe chose maps with a `:kind` discriminator
and then wrote hand decoders for presentation invalidation, prompt updates, search updates, policy
facts, native actions, and host actions. Native completions use another hand-built
`{:status -> :ok/:error, ...}` convention.

This is flexible, but malformed or version-skewed messages are hard to distinguish from unrelated
effects. Roe's decoders return `Option`; a missing field, wrong kind, or unknown identity can make an
effect silently fail to decode. The same issue appeared when command-argument values needed a new
`:value_kind` field: both Mica production code and Rust decoding had to change together without a
checked interface.

Mica would benefit from a host protocol facility that can generate or validate both sides of a
message boundary. It need not be a Rust-specific IDL. Structural value schemas, tagged variants,
or declared effect/request signatures with a Rust codec generator would all help. At minimum, the
driver should make “unrecognized effect” and “effect failed schema validation” observable rather
than encouraging `Option`-based dropping.

### 6. Volatile identity and tuple lifetime needs a scoped abstraction

Roe mirrors a changing Rust object graph into Mica relations. Each buffer or view addition allocates
identities and asserts several tuples; removal must retract them and retire every Rust-side mapping.
Layout changes currently retract and rebuild a tuple set. Endpoint close reconstructs the complete
set of live volatile tuples so it can pass them to the close-and-retract operation.

Two rules were especially easy to get wrong:

- `:volatile` controls persistence, not automatic in-process expiry; hosts still own retraction; and
- an endpoint-created identity cannot safely participate in durable grant/delegation facts merely
  because the endpoint itself is ephemeral.

The language book documents the first rule, but the embedding API makes the host manually enforce
both. A scoped volatile fact-set or lease could own the tuples added for a session/resource and
retract them on update or close. Useful operations would include atomic replace of a named volatile
set, diff application, and a close guarantee that logically revokes the set even if native cleanup
reports a separate error.

### 7. Authority is powerful but difficult to audit across the native boundary

Roe performs several distinct checks for one native operation:

- endpoint actor/role authority (`CanInvoke`, `CanRead`, `CanWrite`, `CanEffect`);
- permission to request a named native service;
- permission to use a particular logical buffer; and
- a generation-checked native capability grant in Rust.

This layered design is defensible, but every action must be classified correctly in every layer.
For example, `copy_region` initially acquired clipboard-write authority but omitted the logical
buffer/text-read checks even though Rust then read buffer contents. Direct native requests also had
to be explicitly rejected once Mica owned the session.

Consider an external-service declaration that names its required authority dimensions and a driver
helper that hands the native bridge an already-admitted resource binding. The host must retain the
final native capability check, but Mica could make incomplete service policy easier to detect.
Static or runtime diagnostics for a service used without declared `CanRequestService` policy would
also be valuable.

### 8. Unit replacement and fileout need stronger discovery and failure semantics

Roe implements replacement as `check_filein`, then Add or Replace based on host-maintained knowledge
of whether the unit is loaded. It also embeds source with `include_str!` and tracks the first-wave
unit's loaded state itself.

The most concrete API surprise was `fileout_unit` for a named unit that had not yet been installed:
it returned an empty string rather than an explicit missing-unit error. A production
`--mica-export roe/first-wave FILE` therefore succeeded while writing a zero-byte file. Roe worked
around this by ensuring that particular unit is loaded before export.

Helpful changes would be:

- distinguish a missing or empty unit from a valid empty fileout;
- expose unit existence, revision, source ownership, and active/replaced state;
- provide checked transactional replace as one operation; and
- provide a package/unit inventory suitable for recovery tooling.

### 9. Embedders need bounded diagnostic and introspection APIs

Driver terminal events contain the task and error, but they do not by themselves retain the selector
and application context needed for a useful production diagnostic. Roe passes the selector string
separately through its wait loop and formats task, selector, endpoint, session, and failure class.
Its `--mica-inspect` output can report only the host's current identity counts and endpoint state,
not a durable catalogue of recent compiler, task, external-request, subscription, or package
failures.

Useful read-only APIs would expose bounded snapshots of:

- installed units/packages and their revisions;
- active, suspended, completed, failed, and cancelled tasks with selector and endpoint context;
- outstanding external requests and subscriptions;
- event/subscription queue occupancy and overload history; and
- the last filein/check diagnostics with source spans.

An invocation result or task handle should retain its selector and endpoint context so every failure
does not require host-side string bookkeeping.

## Language, runtime, and documentation pain points

### 1. `one` has a surprising shape discontinuity

`one Relation(..., ?value)` returns the scalar value when there is one free variable, but a binding
map when there are multiple free variables. Zero answers return `nothing`; multiple answers raise
`E_AMBIGUOUS`. Roe uses both forms extensively.

The current language book does document this accurately. Nevertheless, it remained a practical
source of errors: treating a one-column result like `candidate_kind[:value_kind]` raises `E_INDEX`
because the value is the symbol itself. The runtime error reports an invalid index but not the query
shape that produced the scalar.

Potential improvements are a uniform row result, explicit scalar/row operators, destructuring query
syntax, or at least an error that says “this `one` result is a scalar because the query projected one
variable.” The documentation should cross-link this rule from every introductory `one` example,
because the happy-path example does not make the discontinuity visible.

### 2. Query variables and lexical variables look more alike than they are

In `one Relation(..., ?value_kind)`, `?value_kind` names a projected column; it does not create a
lexical local called `value_kind`. Trying to use the latter produces `UnknownValue`. In a `for`
query, the projected names are accessed through the loop row map.

Again, the language book now states this. The syntax still invites the wrong interpretation,
especially for people coming from Prolog, Datalog, or pattern matching. Destructuring such as
`let {value_kind} = one ...`, or a distinct projection syntax, would make the binding boundary
clearer. Compiler suggestions for an unknown local matching a query-variable name would be a cheap
improvement.

### 3. General maps and symbols are doing the work of result and variant types

Several Roe verbs return either a symbol such as `:invalid_completion_type` or a map such as
`{:kind -> :argument_required, ...}`. A caller that indexes the result before checking the symbol
case gets `E_INDEX`. Effect messages have the same map-plus-tag convention.

Exact value-kind annotations help at outer boundaries, but Mica currently has no union, nullable,
collection-element, function-signature, relation-heading, or user-defined structural type syntax.
It was therefore awkward to express “this is one of these result variants, and this field exists in
this variant.” Tagged structural variants and pattern matching would remove a large class of
defensive map indexing and protocol drift.

### 4. Semantic types are harder than runtime value kinds

Roe needed command arguments whose values were logical views, logical buffers, selectors, paths, or
opaque host values. All identities share the same runtime kind, so an `identity` annotation cannot
say that a value is a live leaf view in this session. Roe introduced
`ArgumentCandidateKind`, repeated provider/argument compatibility checks, and relation membership
checks before emitting the type tag to Rust.

This is not necessarily a request for nominal classes. Possible relational answers include refined
annotations, predicates/contracts on parameters and returns, or a standard way to associate a
schema/refinement with a tagged value. What matters is that dynamic data crossing a host boundary
can be validated once and decoded without guessing.

A small `value_kind(value)` or `is_identity` family would also be useful for genuinely dynamic
code. Such functions were not apparent in the builtin catalogue at the pinned revision.

### 5. Common collection and bounded-query operations are missing or hard to find

Roe defines `roe/list_length` by iterating and incrementing a counter. Candidate construction then
calls it repeatedly and manually stops appending after 256 entries. The builtin catalogue at the
pinned revision has `sort`, but no general collection length, `take`, bounded query iteration, or
overflow-reporting limit that was apparent during the work.

For a runtime that treats resource bounds as part of correctness, this should be a first-class
pattern. Useful primitives include:

- `len` for list, map, relation, string, and bytes with clearly distinct Unicode semantics;
- `take`/`limit` with a way to know that more answers existed;
- bounded collection builders; and
- query APIs that stop execution at the bound instead of materializing or scanning everything
  before the Mica code discards it.

### 6. Precedence and aggregation require verbose imperative loops

Key dispatch, prompt-global dispatch, command discovery, and ordered hooks need “choose the highest
precedence, reject an equal-precedence conflict, and preserve deterministic order.” Roe implements
this with nested relation loops, a sentinel minimum integer, mutable winner/precedence variables,
and an ambiguity flag. Hook ordering constructs sortable pairs and calls `sort`.

Relational aggregation primitives such as `argmax`, `max_by`, grouping, ordered iteration, or a
standard precedence combinator would make this policy shorter and easier to audit. An ambiguity-
detecting `unique_max_by` would exactly fit keymap composition without silently hiding ties.

### 7. Updating functional state is repetitive outside the narrow dot-sugar case

Prompt state, cursor state, active view, history, and layout observations are functional relations.
Roe frequently queries the old tuple, retracts the exact old value, and asserts the new tuple.
Binary functional relations have dot assignment, but composite-key and wider functional relations
still need manual retract/assert choreography.

An `upsert`/`replace` form based on a relation's declared key would reduce boilerplate and prevent
stale-state mistakes. A batch form would also make it clearer when several functional facts are one
logical state transition.

### 8. Dynamic invocation lacks a discoverable checked signature

Commands and hooks are selected from relation data and invoked with `invoke(selector, role_map)`.
This is central to Roe's extensibility, but role names, required roles, annotations, result shape,
and emitted protocol are checked only when the invocation executes. A misspelled selector or role,
or a replacement whose command signature changes, fails late.

Mica already stores method facts, so expose a convenient supported introspection/check API for
callable signatures. Filein could optionally validate relation-declared selectors against expected
roles and result/effect schemas. Runtime dispatch errors should include the selector, supplied roles,
applicable candidates, and the reason each candidate was rejected.

### 9. Ambiguity diagnostics need provenance

Raising `E_AMBIGUOUS` is preferable to choosing an arbitrary fact or method. In editor policy,
however, resolving the problem requires knowing which bindings, rules, packages, keymaps, or methods
competed and where they were installed. A bare ambiguity message forces the application author to
reconstruct the candidate set manually.

Attach bounded candidate/provenance information to ambiguity errors: relation and projected values,
method identities, owning units, and source spans where available. Roe's precedence code also needs
this for equal-precedence conflicts that it detects itself.

### 10. Unit and package composition is under-tooled for a large application

Roe's two Mica files are already large and have cross-file assumptions about relations, selectors,
authority, and emitted values. Named units provide replacement and fileout boundaries, but there was
no obvious module/import interface, exported-signature check, or unit dependency graph for safely
splitting the application further.

For “Mica as the application's implementation language,” units need tooling beyond loading source:
declared imports/exports, dependency and replacement validation, package status, and a way to ask
what a replacement would invalidate. This is particularly important when persisted user packages
are eventually enabled.

### 11. Runtime and policy observability are not yet enough for performance work

Roe's measured Mica insert/delete pair is around milliseconds, far above a direct Rope edit, but the
current measurement includes a complete policy transaction and policy republishing. It does not
identify time in dispatch, relation queries, rule evaluation, transaction commit/retry, external
request handling, event delivery, or host decoding.

Per-task tracing and profiling should expose bounded timings and counts for those stages, including
relation scans, derived-rule work, retries, emitted values, and queue waits. Without that, an
embedder cannot tell whether to change its data model, cache a derived fact, subscribe to changes,
or optimize the runtime.

### 12. Static analysis could catch declarative policy that has no production consumer

During the migration it was possible to declare modes, hooks, faces, syntax, and configuration and
have model-level tests pass while the production Rust path still ignored them. This was partly a Roe
acceptance problem, but Mica tooling could help identify:

- relations that are asserted but never queried;
- verbs/selectors that are declared but unreachable under current authority;
- effects that no host schema accepts;
- package facts that do not affect any derived relation; and
- host-required relations or selectors missing from a replacement unit.

A relation/rule dependency graph and an optional unused-policy lint would be especially useful for
large embedded applications.

## Documentation improvements with the highest leverage

The Mica language book at the pinned revision is substantially better than the minimal embedding
README alone suggests. It already documents `one` result shapes, query-variable scope, volatile
fact lifetime, exact value-kind limitations, Unicode scalar string operations, event ordering, and
the one-consumer rule. The problem is partly that these facts are spread between language and
runtime chapters and are not assembled into a production embedding narrative.

The most valuable documentation additions would be:

1. A production-shaped embedder example: named units, endpoint context plus volatile tuples,
   multiple tasks, external requests, a subscription, a foreign-loop wake, replacement, and
   deadlock-safe close/shutdown.
2. A “sharp edges” page showing `one` scalar versus map results, query variables versus locals,
   `nothing` versus an empty list/map, durable versus volatile identity lifetime, and dynamic
   invocation failure modes.
3. A generated, searchable API reference for every `mica-driver` method with lifecycle state,
   cancellation, event ordering, failure, and backpressure semantics.
4. A host-protocol guide with versioned tagged values, strict decoding, compatibility rules, and
   examples of mapping Mica values into Rust types.
5. An authority walkthrough for one external request, showing endpoint admission, relational
   service permission, logical-resource permission, native capability validation, cancellation,
   and revocation.
6. A recovery guide covering unit discovery, check/add/replace/fileout, failed-task inspection,
   persistent-format migration, and recovery when the programmable unit cannot load.

## Suggested Mica backlog

In priority order:

1. Add a canonical high-level endpoint/task/event-pump abstraction and production embedding example.
2. Add typed or schema-validated effect and external-request protocols.
3. Add scoped volatile fact sets with atomic replace/diff and reliable logical revocation.
4. Make missing-unit fileout explicit; add unit/package discovery and checked replace.
5. Add bounded task, unit, request, subscription, queue, and diagnostic introspection.
6. Add a foreign-event-loop readiness/waker contract and examples.
7. Add collection length, bounded query/collection operations, and useful aggregation primitives.
8. Add structural variants/pattern matching or another checked tagged-result facility.
9. Add functional-relation upsert/replace syntax beyond binary dot assignment.
10. Improve dispatch and ambiguity errors with selector, candidate, unit, and source provenance.
11. Add task/query profiling and a relation/rule dependency inspector.
12. Document and test an upgrade path for the minimal embedding dependency profile.

## Roe-owned complexity that Mica should not absorb

Some complexity exposed by this work belongs in Roe, not Mica:

- Rope storage, character-index invariants, undo, and validated native mutation;
- Winit, terminal, WGPU, Vello, layout realization, and renderer damage tracking;
- filesystem, watcher, clipboard, and subprocess implementations;
- native resource generations and final platform capability enforcement; and
- the editor-specific ontology and presentation protocol.

The requested Mica improvements are generic mechanisms for safely hosting a live Mica application.
They should not turn `mica-driver` into an editor framework or introduce Roe-specific builtins.
