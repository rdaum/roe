# Code health implementation

Scope: all recommendations in [CODE-HEALTH-REVIEW.md](CODE-HEALTH-REVIEW.md).

All nine review areas are implemented. This record maps the changes, verification, measurements, and operational limits.

## Requirements

- [x] 1. Typed bridge actions, explicit decoding errors, and action-owned native capability requirements.
- [x] 1. Ordered, bounded events and atomic policy publication with retention of the last complete projection.
- [x] 2. Session protocol, attachment, presentation, layout, effects, and recovery components with restricted state access.
- [x] 2. Separate source-provider and Vello scene components.
- [x] 3. One Mica-controlled word mechanism for movement, selection, and deletion.
- [x] 3. Mica-selected replacement buffers and generic prompt presentation.
- [x] 4. Effective policy revisions, direct Rope slices, cached metrics, and reuse of unchanged presentation data.
- [x] 4. Comparable production session measurements before and after implementation.
- [x] 5. Full-range document and scroll coordinates with separate display coordinates.
- [x] 5. Shared terminal text sanitation, width, clipping, cursor, and selection mapping.
- [x] 6. Shared startup configuration and construction, Mica-owned initial layout, and reduced public editor state.
- [x] 6. Shared attachment-local frontend services, output handling, and snapshot recovery.
- [x] 7. Shared watcher backend with separate buffer synchronization state.
- [x] 7. Owned asynchronous I/O, cancellation, bounded results, and no kernel lock across external I/O.
- [x] 8. Remove unused filesystem abstractions, detailed dirty tracking, obsolete save actions, and unused kill-line operands.
- [x] 8. Replace obsolete test workflows and split session tests around a shared fixture and serialization boundary.
- [x] 9. Correct architectural references and align dependency policy, manifest, lockfile, and revision record.
- [x] 9. Parse dependency data and report actionable policy errors.

## Verification

- [x] Focused regression checks for changed mechanisms and policy behavior.
- [x] Shared terminal/Vello session conformance and headless scene checks.
- [x] Production terminal workflows.
- [x] Full repository check and declared MSRV check.
- [x] Final requirement-by-requirement audit against the current code.

The final `./scripts/check.sh` passed formatting, workspace checks, strict Clippy, 223 Rust tests, and eight dependency-policy tests.
The Rust total includes 195 core tests, 16 terminal tests, 10 Vello tests, one binary test, and shared conformance.
The Vello suite includes both headless production scenes.
The declared MSRV check passed with `cargo +1.96.0 check --workspace --all-targets`.
The production terminal workflow script passed, including forced-shutdown terminal restoration.
`git diff --check` passed.

## Measurements

The baseline uses `scripts/measure-phase0-baseline.sh` at commit `d1ca778` before code changes.
The fixture contains 2,000 lines. The edit and redraw loops each contain 100 iterations.

| Measurement | Before | After |
| --- | --- | --- |
| Mica session ready | 496,398 µs | 505,352 µs |
| Mica insert/delete pair | 2,312,010 ns | 2,507,528 ns |
| Terminal snapshot/redraw | 236 µs | 136 µs |
| Full redraw output | 17,050 bytes | 17,050 bytes |
| RSS after session workload | 53,144 KiB | 54,024 KiB |
| RSS growth during workload | 0 KiB | 64 KiB |
| Terminal ready | 538.311 ms | 533.513 ms |
| Terminal idle maximum RSS | 54,440 KiB | 54,436 KiB |

These are single-run measurements, not a statistical performance claim.
Both runs use the same fixture, iteration counts, production Mica path, and terminal output workload.
The after-run had no concurrent repository tests or builds.

Snapshot/redraw time decreased by approximately 42%. Edit time increased by approximately 8.5%.
The result does not establish an overall performance improvement.
The remaining edit regression needs profiling before any claim of faster editing.

An intermediate version recounted the policy list before each append.
A bounded counter replaced those repeated scans.
Edit time decreased from 3,461,839 ns to 2,837,325 ns in the corresponding intermediate runs.
The final run measured 2,507,528 ns. This variation reinforces the limits of single-run comparisons.
A regression test covers publisher overflow and retention of the last complete projection.

## Completed boundaries

The bridge uses required operands in typed host and native actions.
Native actions own their capability requirements. The Tab action requires `TextWrite`.
One ordered event sequence replaces the former collections for each event category.
Admission limits each batch to 256 events and 4 MiB of retained event data.
Effect application also limits nested work to 256 events and a depth of 32.

Mica publishes the complete policy projection as one effect.
The decoder rejects oversized or malformed projections.
The separate policy component rejects conflicting functional facts before publication.
Equivalent facts retain the policy revision, regardless of order or duplicate facts.

Mica supplies prompt prefixes and candidate labels. Rust no longer recognizes prompt command kinds or candidate identities.
Mica selects the replacement buffer for a buffer kill. Its policy prefers scratch, then the sorted selection candidates.
Word movement, selection, and deletion use the same effective Mica syntax.

The live-buffer source provider now has a separate module.
Its read operation checks the byte limit before text allocation and captures the text revision under the same read lock.
Driver shutdown sends events directly into the bounded batch.

## Regression coverage

Bridge cases cover ordered effects, typed operands, capability denial, complete policy publication, overflow, and recovery.
Policy cases cover replacement targets, word syntax, prompt presentation, and stable effective revisions.
Coordinate cases cover 65,535, 65,536, and 70,000, plus renderer hit validation.
Terminal cases cover every text surface, display-cell mapping, incremental rows, and layout clears.

Startup cases cover both viewport configurations and Mica replacement of the initial layout.
Frontend cases cover service correlation, bounded output, and full-snapshot recovery after a missing delta.
Layout cases cover rollback after failed validation.
Watcher and I/O cases cover ownership, bounds, cancellation, cleanup errors, and shutdown.

The publisher-overflow test exposed a discarded invocation error.
Policy publication now uses the shared invocation-result handler and reports the error without replacing the previous projection.

Two tests also failed in a clean copy of commit `d1ca778`.
The command-discovery test rejected a valid replacement through an additive precheck.
Replacement now uses the staged validation in Mica before its atomic commit.
The global-chord test encountered two writers of realized active-view context.
Mica now emits its selected view, and the host republishes the realized volatile context.
Both existing tests passed after these changes.

## Document and terminal coordinates

Document positions and scroll offsets use `usize`. Viewport dimensions and terminal cell locations remain bounded screen values.
Absolute mutations use named line and column fields.

The terminal text component escapes controls and handles tabs, wide graphemes, combining marks, clipping, and padding.
Cursor placement, horizontal realization, selection, and mouse hits share that component.
The session validates renderer text hits against the view, resource generation, text revision, and character range.
Buffer content, echo messages, modelines, and typeout text use the safe text path.

The unused filesystem abstraction, detailed dirty tracker, unused save action, and kill-line operand are removed.
Vello retains one redraw flag. Chrome-action resolution no longer needs an attachment, lifecycle sink, asynchronous call, or pending-action queue.

## Dependency and documentation corrections

The dependency policy preserves the current manifest choice: Mica tracks its repository, and the lockfile fixes its commit.
The revision record now matches the existing lockfile. No locked Mica code changed.
All external member dependencies now inherit workspace declarations, including the graphics group.

The dependency guard parses TOML instead of matching manifest lines.
Its checks cover target-specific and development dependencies, driver features, local paths, runtime versions, and the recorded Mica revision.
The architecture guide now points to existing source files and ADRs 0001–0004.

## Shared construction and frontend processing

Both binaries use the same argument parser, help text, welcome content, file loading, watcher registration, and recovery path.
The native constructor creates a seed view. Mica chooses the initial buffer and logical layout.
The shipped policy displays the first two requested files and keeps the first file active.
Other requested files remain available as buffers.
A replacement-policy test changes the layout without a Rust change.

Editor fields now have crate-local visibility. Fixtures and frontends use the shared constructor.
Frontend clipboard instances remain attachment-local. The workspace kill ring does not access the clipboard.
One bounded output processor handles service responses, lifecycle diagnostics, and presentation recovery.
A missing delta triggers a full-snapshot request through the existing session.
The terminal and Vello loops retain their platform-specific responsibilities.

## Presentation and component boundaries

The protocol module owns wire types and input bounds.
The attachment module owns transport validation, request correlation, and attach/detach/resume/close state.
Behavior-focused session test modules share one fixture and the existing Mica serialization lock.

The presentation projector receives read-only editor, policy, and identity data.
Its mutable inputs contain only display state and bounded caches.
Each buffer observation captures metadata, text revision, and the persistent Rope root under one read lock.
Visible text uses Rope slices. Unchanged slices share immutable text across views, updates, and renderer clones.
Buffer metrics use text revisions. Highlight spans use both text and effective policy revisions.

The Vello scene builder receives presentation data, text resources, theme data, and a scene.
It has no session client, event loop, or editor access.

The layout component accepts typed mutations and restores its previous state after invalid geometry.
The recovery component receives only the Mica host.
Native action realization receives only editor mechanisms and effective policy.
The effect coordinator retains cross-component sequencing, authorization, frontend services, and lifecycle delivery.

## Watcher and I/O ownership

The shared watcher backend owns registrations, parent reference counts, bounded hints, wake delivery, and cleanup.
Buffer synchronization baselines remain in `file_watcher.rs`.
Native resource generations remain in `native_kernel.rs`.
Registration succeeds before logical publication. A failed final unwatch preserves ownership for retry.
Resource revocation removes the logical owner and retains failed backend cleanup for shutdown.

External I/O admission checks capabilities before filesystem or process work.
The kernel releases its mutex before the request starts external work.
Compio handles file and pipe I/O on the owning runtime.
A cooperative directory worker retains an ownership lease.
Workspace shutdown cancels admitted work and waits for every lease.
A recovery error during startup also closes the workspace.

The I/O tests cover denied admission, concurrency limits, bounded reads, pre-cancelled writes, sorted directory results, combined process output, cancellation, future-drop cleanup, and shutdown.
The kernel-lock test observes an active child without waiting for the child to exit.

## Explicit limits and compatibility

These changes introduce explicit operational limits:

| Operation | Limit |
| --- | --- |
| Admitted native I/O | 16 requests |
| Native I/O deadline | 30 seconds |
| File contents, startup loads, recovery file transfers, watcher rereads | 1 MiB per transfer |
| Process stdout and stderr | 1 MiB combined |
| Process arguments | 256 arguments and 65,536 aggregate bytes |
| Directory result | 256 sorted entries and 1 MiB |
| Directory scan | 65,536 entries |
| Watch registrations | 1,024 logical owners and parent directories |

An oversized read fails before full collection. An oversized write fails before destination creation.
An interrupted write can leave a partial destination. Atomic file replacement is not part of this change.
A directory OS call can delay cooperative cancellation and shutdown until that call returns.

Process cancellation kills and reaps the direct child.
It does not manage descendant process groups.
Normal process cleanup is asynchronous. Future-drop cleanup uses a synchronous kill-and-wait fallback.
Process spawning remains outside production Mica command dispatch.

The direct session still awaits each accepted input operation.
This change does not add concurrent frontend input or a process-job UI.
The immediate `NativeKernel::execute` method rejects external I/O with `IoRequired`.
The session and Mica bridge use the authorized asynchronous entry.

Editor fields now have crate-local visibility.
The shared constructor and read-only accessors replace external struct literals.
Visible presentation text uses shared immutable storage and still serializes as a string.
Document positions use `usize`. Terminal cell coordinates retain their viewport bounds.

A real display-host Vello smoke remains unverified because this session has no display connection.
Headless scene and shared-conformance tests do not replace that check.

## Requirement-to-code map

| Review section | Main implementation paths |
| --- | --- |
| 1. Bridge contracts | [events](../roe-core/src/mica_host/events.rs), [decoder](../roe-core/src/mica_host/decode.rs), [policy projection](../roe-core/src/session/policy.rs) |
| 2. Components | [session modules](../roe-core/src/session), [source provider](../roe-core/src/mica_host/source_provider.rs), [Vello scene](../roe-vello/src/scene.rs) |
| 3. Policy ownership | [shipped Mica policy](../mica/roe-first-wave.mica), [native actions](../roe-core/src/session/native_actions.rs) |
| 4. Presentation revisions | [projector](../roe-core/src/session/presentation.rs), [cache](../roe-core/src/session/presentation_cache.rs), [baseline workload](../roe-terminal/examples/phase0_baseline.rs) |
| 5. Coordinates and sanitation | [buffer](../roe-core/src/buffer.rs), [protocol](../roe-core/src/session/protocol.rs), [terminal text](../roe-terminal/src/text_cells.rs) |
| 6. Shared frontends | [startup](../roe-core/src/startup.rs), [frontend services](../roe-core/src/frontend.rs) |
| 7. Native ownership | [watch backend](../roe-core/src/watch_backend.rs), [native I/O](../roe-core/src/native_io.rs), [I/O tests](../roe-core/src/native_io/tests.rs) |
| 8. Obsolete paths and tests | [editor](../roe-core/src/editor.rs), [renderer utilities](../roe-core/src/renderer.rs), [session tests](../roe-core/src/session/tests) |
| 9. Dependency consistency | [policy](DEPENDENCY-POLICY.md), [parsed guard](../scripts/dependency_policy.py), [guard tests](../scripts/test_dependency_policy.py) |
