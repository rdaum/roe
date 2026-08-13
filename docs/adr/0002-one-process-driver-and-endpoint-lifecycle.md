# ADR 0002: one process-long Mica driver with endpoint-scoped sessions

Status: accepted for Phase 3

## Context

Mica already supplies task scheduling, suspension, bounded driver events, external-request
admission, subscriptions, cancellation, and endpoint cleanup. Recreating those facilities in Roe
would repeat the actor/mailbox problems repaired in Phase 1.

## Decision

Roe pins Mica revision `a13f479229b761bf45b7ef71802cd4ca6e588dd4` with `default-features = false`.
One `CompioTaskDriver` lives for the process-long Compio runtime. Initial Phase 4 resource policy is
explicit:

| Resource                     | Initial budget |
| ---------------------------- | -------------- |
| dispatcher workers           | 2              |
| relation parallelism         | 1              |
| task instruction budget      | 250,000        |
| task retry limit             | 4              |
| task call depth              | 32             |
| driver event queue           | 256            |
| concurrent external requests | 16             |
| subscription mailbox budget  | 64             |
| relation acceleration        | disabled       |
| storage                      | memory         |

Startup order:

1. create the Compio runtime;
2. construct the driver with the compiled-in minimal recovery unit;
3. run `check_filein` on the Roe core unit and install it as a named unit before accepting input;
4. if checking or installation fails, keep the recovery unit alive, expose diagnostics, and do not
   claim the normal unit is active;
5. allocate a process-local, non-durable ephemeral identity per Roe session;
6. open the endpoint with actor/principal context plus volatile session/resource tuples; and
7. begin one host event-consumer pump before submitting normal editor input.

Normalized input becomes a named-role invocation or endpoint input. Exactly one logical consumer
owns `wait_events`/`drain_events`; frontend loops consume translated `SessionOutput`, not raw Mica
events.

Close order:

1. reject new session input;
2. `close_endpoint_and_retract_volatile_tuples_named`;
3. await cancellation of endpoint tasks and external work;
4. invalidate every native association and advance resource generations;
5. drain retained terminal events/effects; and
6. on process exit, poll `shutdown` while continuing to drain the bounded event queue, then drop
   native frontends and the Compio runtime.

Shutdown and endpoint close are idempotent. A frontend drop guard may start background close, but
the normal path awaits it.

## Consequences

There is no second Roe task scheduler or unbounded Mica bridge queue. Vello and terminal share the
same endpoint semantics. The host can later move across a process boundary without changing the Mica
ontology or exporting Rust references.
