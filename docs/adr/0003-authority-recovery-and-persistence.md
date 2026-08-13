# ADR 0003: relational authority, native recovery, in-memory persistence

Status: accepted for Phase 3

## Context

Programmability is useful only if a bad command or replacement cannot strand the editor, leak a
native resource, or silently persist a capability. Mica authority and Roe's native capability checks
must agree without treating an integer identity as permission.

## Decision

Mica relations describe authority:

- `CanInvoke(actor, selector)` controls command entry;
- `CanEffect(actor)` controls host-visible effects;
- `CanRead`/`CanWrite` control relation access;
- `roe/CanUseBuffer(actor, logical_buffer)` controls buffer association use; and
- `roe/CanRequestService(actor, service)` controls file, process, clipboard, clock, and watch
  requests.

At endpoint open, the host derives a `CapabilityGrants` projection for that endpoint. An external
request must pass both the Mica task context (endpoint, actor/principal, task, cancellation) and the
fresh native check. A `ResourceId` proves only generation, never authority. No grant token, endpoint
identity, or native resource association is durable.

The native recovery surface is deliberately non-programmable and small:

- boot the embedded safe recovery unit;
- show compiler, task, external-request, and package diagnostics;
- check then install/replace a named unit;
- disable a failing non-core package;
- fileout named programmable units;
- request a full presentation snapshot; and
- close the endpoint/driver cleanly.

A failed command produces a contextual diagnostic containing the task, selector, endpoint/session,
and failure class but not buffer contents. It does not close the endpoint. Fatal driver failure
switches the session to recovery-only mode and invalidates native associations.

Replacement is transactional from Roe's perspective: check source first, call `filein_unit` with
`FileinMode::Replace`, and publish the new active revision only after success. Check or replace
failure retains the last working named unit. Disabling a package retracts its activation and active
keymaps/hooks without deleting its last fileout-able source.

Phase 4 and Phase 5 use `DriverStorage::Memory`. Durable user/workspace facts are deferred until Roe
has exact-revision pinning, store backup, fileout/export, explicit migration, and rollback tests.
Compiled program bytes are never copied between Mica revisions.

## Consequences

Recovery remains available even when user policy is broken. Persisted editor description can later
grow deliberately, while session identities and native capabilities remain unambiguously volatile.
