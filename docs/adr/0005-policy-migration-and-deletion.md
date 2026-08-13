# ADR 0005: migrate policy in vertical slices and delete superseded Rust

Status: accepted and implemented in Phase 5

## Context

Roe's old Rust path contained overlapping command, keymap, mode, selection-menu, action,
buffer-host, syntax, and renderer abstractions. Keeping those owners synchronized with Mica would
have created two editors and made live replacement unreliable.

## Decision

Mica is authoritative for editor policy and logical choices. Rust is authoritative for native
resources, validated mechanisms, session ordering, presentation extraction, and rendering.

The migration proceeded as vertical slices: commands/keymaps; prompt/completion and arguments;
buffer/file/search interactions; modes/hooks; faces/syntax/configuration; logical views; and
packages/recovery. A slice was complete only after both frontends consumed it through
`HostSession`, its bounded authority/failure paths were exercised, and the displaced Rust owner was
deleted or reduced to a mechanism vocabulary.

The production input path has no Rust command, keymap, prompt, mode, syntax, or face-policy
fallback. Ordinary character insertion and named native editing actions are selected by Mica and
realized by Rust. `HostSession::open` is retained solely as a policy-free mechanism/protocol test
harness; both applications use `HostSession::open_with_mica`.

Durable user/workspace state remains disabled. It is an optional later decision requiring schema
versioning and recovery design, and must never persist native capabilities.

## Consequences

- A live behaviour has one production owner.
- Mica replacement can add, remove, or alter policy without synchronizing a Rust registry.
- Terminal and Vello receive the same ordered presentation stream and cannot fork editor meaning.
- The native boundary stays small: text/files/watchers/clipboard/layout/rendering remain Rust
  mechanisms with capability and generation checks.
- The old command registry, binding tables, prompt/search modes, mode and buffer-host actors,
  syntax/face registries, and renderer-over-`Editor` path have been deleted.
- Coarse editing latency now includes Mica transactions and is materially higher than direct Rope
  mutation; it is tracked as an honest optimization target rather than hidden by the old benchmark.

The detailed ownership matrix, bounds, commits, and acceptance evidence are recorded in
[PHASE-5-POLICY-TRANSFER.md](../PHASE-5-POLICY-TRANSFER.md).
