# Code health review

Review date: 2026-09-06. Reviewed commit: `d1ca778`.

This review covers structure, factoring, sanitation, and code health across Rust, Mica, both frontends, tests, and CI configuration.
The findings come from source inspection. The review did not include builds, tests, or performance measurements.
The source links refer to the reviewed code. Later changes can move the referenced lines.

The Mica → session → frontend architecture is a good foundation.
Several boundaries depend on conventions, but large structs and dispatch functions still allow responsibilities to mix.
The main opportunities are explicit types, smaller components, shared mechanisms, and removal of obsolete paths.

## 1. Make the Mica bridge typed, ordered, and bounded

[MicaHostAction](../roe-core/src/mica_host.rs#L367) contains a string action name and 15 optional fields.
Each consumer must determine which field combinations are valid.
This representation permits missing operands and unrelated fields in the same action.

Native action names and capability requirements also have separate match expressions.
The `tab` action maps to `KeyAction::Tab`, which inserts text.
However, [mica_native_capabilities](../roe-core/src/session.rs#L4107) omits `tab` and defaults to no required capabilities.
This mismatch shows the cost of separate action definitions.

[MicaEventBatch](../roe-core/src/mica_host.rs#L418) separates events into collections by category.
The host applies native edits before host actions, regardless of their emission order.
The representation therefore loses ordering between event categories.
Boolean fields such as `prompt_close` also discard their position relative to updates.

Most batch collections have no explicit aggregate bound.
The bounded driver queue does not establish a bound on accumulated events across an invocation.
The policy-fact limit reports an error after retaining only part of the publication.
The host can then apply that partial projection after a policy reset.

Recommended changes:

- Decode each action into an enum variant with required operands.
- Return explicit errors for malformed effects.
- Put native capability requirements on the decoded action type.
- Preserve effect order in a bounded event sequence.
- Give policy replacement an explicit atomic boundary.
- Reject an incomplete policy publication without replacing the last complete projection.

These types describe native mechanisms and bridge messages.
Mica retains ownership of commands, bindings, and editor policy.

## 2. Give WorkspaceHost smaller components

[session.rs](../roe-core/src/session.rs#L633) contains 7,920 lines, including approximately 3,280 lines of tests.
[apply_mica_events](../roe-core/src/session.rs#L1483) alone exceeds 1,000 lines.
The main problem is the number of responsibilities that share access to workspace state.

The code already contains useful component boundaries:

| Component | Responsibility |
| --- | --- |
| Session protocol | Envelopes, messages, revisions, and errors |
| Attachment state | Input ordering, scroll, pointer state, and frontend requests |
| Presentation projection | Snapshots, styles, visible slices, and caches |
| Layout mechanism | Tree validation, geometry, and drag calculations |
| Mica effect application | Validated native actions and their outcomes |
| Recovery | Source validation, replacement, export, and package controls |

These components can remain modules within `roe-core`.
Smaller components can restrict which state each operation accesses and make their invariants explicit.
Moving functions without reducing their access to state gives less benefit.

The [live-buffer source provider](../roe-core/src/mica_host.rs#L114) also has a separate responsibility from driver lifecycle management.
Vello scene construction has a separate responsibility from the [application event loop](../roe-vello/src/lib.rs#L283).
Both are candidates for module extraction.

## 3. Remove remaining parallel editor-policy paths

Some production behavior still divides editor decisions between Mica and Rust.

Ordinary word movement uses [Mica-derived syntax](../roe-core/src/session.rs#L1717).
Word-selection actions instead reach the older [whitespace-based buffer methods](../roe-core/src/buffer.rs#L335).
As a result, word selection can disagree with word movement and word deletion.

During a buffer kill, [Rust chooses the replacement buffer](../roe-core/src/editor.rs#L440) from the first other SlotMap entry.
That choice is a logical target decision.
Mica can supply the replacement identity, while Rust validates and applies the change.

Prompt realization also recognizes a fixed set of command kinds.
[mica_prompt_content](../roe-core/src/session.rs#L4144) supplies labels for those kinds.
A generic prompt presentation can support new Mica interactions without additions to the Rust command vocabulary.

Recommended changes:

- Use one word-boundary mechanism for movement, selection, and deletion.
- Supply its effective syntax through Mica policy.
- Include the replacement buffer in the Mica decision for a buffer kill.
- Represent prompt content and selection through a generic native presentation.
- Keep command-specific labels and interaction choices in Mica.

## 4. Make presentation work depend on actual revisions

Every [key dispatch](../mica/roe-model.mica#L619) republishes policy.
The publication emits a reset, and Rust [increments the policy revision](../roe-core/src/session.rs#L1491).
Identical policy publications therefore receive different revisions.
This invalidates the highlight cache on cursor movement, even without a text change.

[capture_snapshot](../roe-core/src/session.rs#L3456) materializes the whole buffer to obtain a visible slice.
It also allocates all buffer lines to calculate the maximum line width.
Multiple views repeat this work for the same buffer.
Presentation deltas contain full snapshots, and consumers clone those snapshots.

Recommended changes:

- Advance the policy revision only after an effective policy change.
- Read visible text directly from Rope slices.
- Cache buffer metrics by text revision.
- Reuse unchanged buffer and view data across presentation updates.
- Measure the production session path before and after the changes.

The source establishes redundant work.
The review does not establish its contribution to latency or memory use.
Changes to wire-level deltas can remain separate from improvements to snapshot construction.

## 5. Separate document coordinates from display coordinates

[BufferInner::to_column_line](../roe-core/src/buffer.rs#L213) casts document line and column positions to `u16`.
A line or column value of 65,536 therefore wraps to zero.
[ViewScroll](../roe-core/src/session.rs#L439) also stores document offsets in `u16` fields.
Vello explicitly limits scrollbar positions to that range.

Document coordinates need their full range throughout buffer and session operations.
Conversion to bounded screen coordinates belongs at the viewport boundary.

Terminal realization also assumes that each Unicode scalar occupies one cell.
The [content path](../roe-terminal/src/terminal_renderer.rs#L544) prints text characters directly.
The echo, modeline, and typeout paths print whole strings without control-character escaping.
Embedded terminal controls can therefore affect terminal state instead of appearing as text.
Tabs, wide characters, and combining marks also require explicit display rules.

Recommended changes:

- Keep document positions separate from viewport dimensions and terminal cell coordinates.
- Preserve the full document range in scroll messages.
- Centralize terminal escaping, cell width, clipping, and character-to-cell mapping.
- Use that text-to-cells mechanism for buffer content, echo messages, modelines, and typeouts.
- Keep terminal realization in `roe-terminal`.
- Keep native buffer positions character-indexed.

This boundary provides one place for consistent cursor, selection, clipping, and text sanitation behavior.

## 6. Share startup construction and frontend service plumbing

Both binaries duplicate argument parsing, recovery flags, welcome content, file loading, and watcher registration.
They also construct `Editor` through extensive struct literals.
These callers need knowledge of almost every editor field.

Startup behavior already differs between frontends.
The [terminal frontend](../roe/src/main.rs#L273) creates a split for multiple files.
The [Vello frontend](../roe-vello/src/bin/roe-vello.rs#L187) creates one view.
A shared construction path can preserve the Mica ownership boundary for the initial logical layout.

Clipboard service implementations and output-draining loops also repeat across frontends.
The [terminal implementation](../roe-terminal/src/terminal_renderer.rs#L842) and [Vello implementation](../roe-vello/src/lib.rs#L232) perform the same basic service work.
A shared frontend component can retain attachment-local ownership of the clipboard.

Presentation-gap handling is another candidate for shared behavior.
Current frontend consumers propagate presentation errors into exit paths.
They do not request the full snapshot that the session contract requires after a revision gap.

Recommended changes:

- Introduce a shared startup configuration and workspace-construction path.
- Keep the initial logical layout decision in Mica.
- Reduce public mutable editor state after callers use the shared constructor.
- Share frontend service mechanics and output handling.
- Keep terminal and Vello event-loop integration in their respective frontends.
- Implement snapshot recovery once at the shared frontend boundary.

## 7. Consolidate native I/O ownership

[FileWatcher](../roe-core/src/file_watcher.rs#L115) and [NativeWatchService](../roe-core/src/native_kernel.rs#L303) independently implement similar backend machinery.
Both track registrations, parent-directory reference counts, bounded notification queues, and backend errors.
Their higher-level responsibilities differ.
The buffer watcher also owns synchronization baselines for external file changes.

A shared watcher backend can remove duplicate lifecycle logic while preserving those separate responsibilities.
This does not imply that every production buffer currently registers with both watchers.

[Native operations](../roe-core/src/native_kernel.rs#L469) mix immediate text mechanisms with blocking filesystem and process operations.
Production file opening and saving use the synchronous kernel path.
That work occurs while the caller holds the kernel mutex.

The process operation is outside production Mica dispatch.
However, it accumulates the complete process result before the session checks its size.
A result-size check after allocation does not bound the allocation itself.

Recommended changes:

- Share watcher backend ownership and cleanup mechanisms.
- Keep buffer synchronization policy separate from notification delivery.
- Separate immediate native operations from owned asynchronous I/O.
- Define cancellation and result bounds at the I/O operation boundary.
- Keep kernel lock scope separate from external I/O duration.

This factoring can clarify lifecycle ownership, cancellation, bounds, and frontend responsiveness.

## 8. Remove obsolete infrastructure and align tests with production

The workspace contains code that no longer contributes to the production architecture.
Some public APIs remain available despite the absence of workspace callers.

| Candidate | Evidence and proposed change |
| --- | --- |
| `FileSystem` and `SystemFileSystem` | The [implementation](../roe-core/src/native_services.rs#L34) has no workspace callers. Remove the unused abstraction. |
| Detailed `DirtyTracker` | [Vello](../roe-vello/src/renderer.rs#L28) uses it alongside a redraw boolean. The application rebuilds the scene. Its detailed span machinery has no production consumer. |
| `ChromeAction::Save` | The workspace has a consumer but no producer. Remove the obsolete variant and its dispatch branch. |
| `KillLine(bool)` | The implementation ignores the boolean. Remove the unused operand. |
| Old file-opening test path | [Test-only methods](../roe-core/src/editor.rs#L1749) retain a parallel open/watch/replace workflow. Move the regression cases onto production mechanisms. |

The large session test module also mixes protocol, recovery, policy, source-provider, agent, pointer, and clipboard cases.
Separate modules can make each behavior easier to inspect.
A shared fixture constructor can remove repeated editor initialization.
The existing Mica-test serialization remains necessary during that restructuring.

These changes need to retain the production `WorkspaceHost::open_with_mica` and `DirectSessionClient` coverage.
The shared terminal/Vello conformance fixture remains an important integration boundary.

## 9. Repair architectural documentation and dependency checks

[AGENTS.md](../AGENTS.md#L99) references a nonexistent `window.rs`.
It also names a missing policy-transfer document and ADRs 0005–0006, which are absent from the reviewed tree.
These references give readers an incorrect map of the architecture.

At `d1ca778`, the workspace manifest no longer contains Mica `rev` fields.
However, [check-dependencies.sh](../scripts/check-dependencies.sh#L8) still requires the previous exact declarations.
The required repository check cannot pass that guard as written.
`MICA-REVISION` and the documentation also describe manifest pinning.

The dependency guard matches complete manifest lines.
Formatting changes can therefore affect the result independently of dependency policy.

Recommended changes:

- Remove or correct references to absent architectural files.
- Choose and document the intended Mica dependency policy.
- Make the manifest, lockfile, revision record, and dependency documentation consistent with that policy.
- Validate parsed manifest and lockfile data.
- Report each dependency-policy mismatch with an actionable diagnostic.

The review does not choose between manifest revision pins and a lockfile-based policy.
That decision remains separate from the structural problems in the current guard.

## Suggested order of work

The bridge protocol and coordinate/display correctness are the first priorities.
Both contain structural problems with concrete behavioral consequences.
Shared construction and obsolete-code removal come next.
Those changes reduce the dependencies involved in the larger session refactor.

| Stage | Scope |
| --- | --- |
| 1 | Typed actions, capability requirements, event order, batch bounds, and atomic policy publication |
| 2 | Document coordinate range and consistent terminal text realization |
| 3 | Shared startup construction, frontend services, and removal of obsolete paths |
| 4 | Smaller session, bridge, presentation, layout, and recovery components |
| 5 | Policy revision accuracy, Rope slices, cached metrics, and measured presentation improvements |

Documentation and dependency-guard corrections can proceed independently.
Watcher and asynchronous I/O factoring need focused lifecycle and cancellation checks.

Each implementation change needs checks appropriate to its affected boundary.
The repository requires `./scripts/check.sh` before a commit.
This document records proposed work and source findings, not completed changes or new test results.
