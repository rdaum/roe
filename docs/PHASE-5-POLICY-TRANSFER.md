# Phase 5 Mica policy transfer

Phase 5 completes the policy transfer described in
[ROE-MICA-ROADMAP.md](../ROE-MICA-ROADMAP.md). Mica is now the sole production owner of editor
meaning: commands, bindings, prompts, completion, search state, modes, hooks, faces, syntax,
configuration, packages, and logical view decisions. Rust realizes bounded native mechanisms and
renderer-neutral presentation. Both terminal and Vello create a Mica-enabled `WorkspaceHost` and
attach through the `SessionClient` contract.

This is an ownership boundary, not a claim that all of Roe is implemented in Mica. Rope storage,
filesystem and watcher operations, validated window-tree mutation, terminal cells, glyph layout,
Winit, WGPU, and Vello scenes remain native by design. Platform clipboard access is native but
attachment-local; it is not a workspace or native-kernel service.

## Production path

```text
terminal or Vello input
  -> renderer-neutral InputEvent
  -> ordered Attachment
  -> Mica prompt state or key dispatch
  -> Mica command/policy transaction
  -> bounded native action or host effect
  -> Rust resource/layout mechanism
  -> renderer-neutral PresentationUpdate and LifecycleEvent
  -> terminal cells or Vello scene
```

`WorkspaceHost::open_with_mica` is the production constructor. `WorkspaceHost::open` remains only
as a policy-free workspace/native-mechanism test harness; it has no Rust command or binding
fallback. `DirectSessionClient::initial_output` publishes the initial Mica policy before the first
presentation. Subsequent key transactions republish an atomic policy snapshot so replacement can
remove facts without leaving stale Rust projections.

`WorkspaceHost` owns the long-running editor, buffers, Mica driver, native kernel, watchers, and
processes. An `Attachment` owns viewport and focus, exact input sequence, presentation revision,
pointer/scroll state, and frontend-service capabilities. Attach, detach, resume, close-attachment,
and terminate-workspace are separate lifecycle operations. A detached or closed frontend does not
terminate the workspace. Background work emits server-originated `SessionOutput` with no client
input acknowledgement and therefore consumes no input sequence number.

`SessionClient` is the transport-independent frontend interface. `DirectSessionClient` implements
it in process without serialization. A future remote implementation can encode the same owned,
Serde-compatible protocol over CBOR and ZeroMQ without retaining a second session API. Bounded,
correlated frontend-service requests cover attachment-local clipboard and notification work;
files, processes, and watches remain backend-local.

## Transferred slices

### Commands, discovery, invocation, and keymaps

Mica owns `Command`, `CommandName`, `CommandSummary`, `CommandSelector`, `PackageCommand`,
`CommandArgument`, `ArgumentPrompt`, `ArgumentCompletion`, `CommandImplementation`, `KeyBinding`,
`NativeBinding`, `KeyPrefix`, `SessionKeymap`, `EffectiveSessionKeymap`, and `EffectiveBinding`.
`roe/dispatch_key` resolves command and native actions in one combined precedence comparison,
detects equal-precedence ambiguity, invokes named commands, and selects bounded native editing
actions. Printable characters are also selected by this verb, so ordinary insertion has no
production Rust binding fallback.

`roe/DiscoverableCommand` filters M-x candidates through active packages and endpoint authority.
M-x invokes the selected Mica selector directly; commands do not require a shadow Rust registry or
host-action declaration to be discoverable.

Rust retains only the normalized platform key vocabulary and the native operation vocabulary used
to realize Mica's decision.

### Minibuffer, completion, files, buffers, and search

`PromptState`, `PromptLast`, `FileCandidate`, `ArgumentCandidate`, and the
`roe/prompt_key`, `roe/refresh_prompt`, and `roe/search_prompt_key` verbs own prompt text,
selection, cancellation, command argument acquisition, history, and filtering. The generic
argument path is exercised by `select-window`: its declared `:window`/`:logical_view` argument
opens a completion prompt, validates the chosen logical view, and invokes the declared command
implementation. `ArgumentCandidateKind` makes each provider declare its value kind; acquisition
rejects a provider/argument mismatch, acceptance validates the logical identity, and the host
decodes the candidate from Mica's emitted kind instead of guessing from the raw value. Prompt
position, label, completion selector, and candidate value type therefore remain Mica data; the
exercised command deliberately uses a nonzero position so the bridge cannot silently special-case
position zero. Command, buffer, view, and file candidates are computed in Mica and capped at 256
entries. Search state and selection live in Mica; the native `text_search` request returns at most
1,024 character-indexed matches.

Rust retains directory enumeration, Rope searching, fallible file open/save, file watching, and a
passive prompt view used by both renderers. It receives the selected identity or path only after
Mica has applied actor, package, and prompt policy.

The deleted Rust owners are `CommandMode`, `SelectionMenu`, `BufferSwitchMode`,
`FileSelectorMode`, `IsearchMode`, `CommandRegistry`, and their candidate/action plumbing.

### Modes, hooks, faces, syntax, indentation, and configuration

Mica owns `DefaultMajorMode`, `BufferMajorMode`, `BufferMinorMode`, `ModeKeymap`, `ModeHook`,
`Face`, `FaceAttribute`, `FaceParent`, `SyntaxRule`, `Configuration`, and their `Effective*` rules.
The host publishes logical buffers but does not assign `fundamental`; Mica derives the configured
default unless an explicit buffer major mode overrides it.
`roe/publish_policy` emits a reset followed by a bounded projection of effective mode, face,
syntax, and configuration facts. `roe/dispatch_key` emits ordered effective hooks after editing.
Higher-precedence hook selectors run first and execute inside Mica; the built-in invalidation hook
emits only the renderer-neutral invalidation mechanism it needs. Tab width comes from
`EffectiveConfiguration`; word movement and deletion interpret the highest-precedence Mica
character-class rule and visibly reject unsupported or
ambiguous rules rather than using a Rust fallback; search highlighting consumes Mica face
attributes. Hook invalidation reaches the common presentation stream, not either renderer
directly.

Rust retains renderer-ready style records, character ranges, native text mutation, and redraw cache
invalidation. It no longer stores a buffer major mode or owns a mode trait, mode actor, face
registry, or span policy. The deleted owners are `Mode`, `ScratchMode`, `FileMode`,
`MessagesMode`, `BufferHost`, and the Rust syntax/face registry.

### Logical frames, windows, and views

The host publishes `SessionFrame`, `FrameRootView`, `ViewFirstChild`, `ViewSecondChild`,
`ViewSplitAxis`, `ViewSplitRatio`, `ViewBuffer`, `ViewCursor`, `NextView`, and `ActiveView` for the
complete logical tree. `SessionLeafView` and `VisibleBuffer` derive the usable leaves. Mica window
verbs choose and identify the exact target view; Rust validates and realizes split/delete geometry.
Neither renderer interprets a window command.

### Packages, replacement, and recovery

`PackageEnabled`, `PackageDisabled`, and `Package*` membership determine which commands, keymaps,
and modes participate. Package disable is a volatile endpoint overlay, so it cannot persist native
authority. `WorkspaceHost` exposes check-before-replace, named-unit replacement, fileout/export,
first-wave restore, and package enable/disable operations. Malformed replacement leaves the last
working unit live; a valid replacement atomically resets projected policy and removes stale
settings.

Both shipped binaries expose the native bootstrap surface before their normal event loops through
`--mica-check`, `--mica-replace`, `--mica-export`, `--mica-restore-first-wave`, package
enable/disable, and `--mica-inspect`. Inspection reports endpoint/session identities and the
bounded live-object state. These options do not depend on user Mica policy, so invalid replacement
can be diagnosed and repaired from either frontend. Exporting the built-in first-wave unit ensures
that unit is loaded before fileout, including at process startup, and missing CLI operands produce
a usage error with exit status 2 rather than a panic.

Durable user/workspace state is intentionally not enabled in Phase 5. It was optional in the
roadmap, and enabling it requires an explicit schema revision, migration, backup, and recovery
policy. Ephemeral identities, resource generations, endpoints, and capability grants remain
non-durable.

## Deletion gate

The following superseded paths are absent from the production tree:

- Rust command registry, command mode, global binding tables, and key-state policy;
- Rust selection, buffer-selection, file-selection, and isearch modes;
- Rust mode traits/actors, buffer-host actors, face registry, syntax registry, and highlight store;
- the old renderer-over-`Editor` interface and its duplicate terminal/Vello conformance fixtures;
- the editor's policy fields and broad command/action fallback path.

The remaining `KeyAction` and reduced `ChromeAction` enums name native mechanisms and session
effects; they do not bind keys, discover commands, filter candidates, or choose logical targets.
The only renderer conformance surface is the revisioned `SessionOutput` presentation stream.

## Bounds and failure behaviour

- Mica driver events: 256 queued events with one logical consumer.
- External native requests: 16 in flight.
- Subscription delivery: 64 queued events per subscription budget.
- Prompt candidates: 256.
- Search matches: 1,024.
- Effective policy facts per publication: 256; overflow produces a visible lifecycle error.
- Native resources: fixed-capacity, generation-checked slots with explicit invalidation.
- Logical views: 64, with native minimum-geometry and topology validation before split/delete commit.
- Messages: 65,536 characters, retaining the newest diagnostics.
- Directory results: the lexically first 256 paths, retained with bounded memory during enumeration.

Authority is checked at the endpoint, service, logical-buffer, and native-resource layers. In
particular, `copy_region` requires Mica `text_read` plus logical-buffer authority and native
`TextRead` in addition to clipboard-write authority. Native failures are returned through typed
completion/lifecycle results; failed task diagnostics include task, selector, endpoint/session,
and failure class without buffer contents. Cancellation, endpoint close, replacement failure,
queue pressure, and failed watcher cleanup have focused tests.

## Delivery history

The initial four commits established global-policy ownership, common-frontend routing, and native
editing separation. A strict end-of-phase review rejected that checkpoint because prompt/mode/view
policy was still declarative-only and the legacy stack remained reachable. The corrective series
closed those gaps and passed the deletion gate.

| Commit | Change |
| ------ | ------ |
| `524b73e` | Move global editor policy to Mica. |
| `cd62ad4` | Drive editor selection from Mica. |
| `56060db` | Keep direct text editing native. |
| `91f51ee` | Clarify the initial Mica/native boundary. |
| `7fb8f4d` | Record the initial, subsequently rejected Phase 5 checkpoint. |
| `edaa0be` | Move complete key dispatch, including native binding choice, into Mica. |
| `8f0fe35` | Move prompt, completion, file/buffer selection, and isearch state into Mica. |
| `a085d58` | Publish the complete logical view tree and make Mica choose window targets. |
| `7aa1f64` | Make mode, hook, face, syntax, indentation, and configuration policy authoritative. |
| `8e1caf5` | Add check, replacement, export, restore, and package recovery operations. |
| `ad98565` | Delete the superseded Rust policy stack and old renderer path. |
| `8ed36b8` | Measure editing and redraw through the production Mica session. |
| `d12aa81` | Remove Rust insertion fallback from Mica-owned input. |
| `f023522` | Move pointer/view decisions into Mica and enforce native effect authority. |
| `3dd8f77` | Bound native resources and expose pre-policy recovery operations. |
| `45fd49d` | Exercise generic command argument acquisition through logical-view selection. |
| `1c89b1f` | Make default modes, binding precedence, hook order, and syntax policy authoritative. |
| `8c088ba` | Exercise both frontend realizations with the production Mica session stream. |
| `03731c9` | Authorize and test relational layout dragging through Mica. |
| `d91b014` | Close recovery, argument, hook, syntax, authority, and retention gaps from re-review. |
| `4789b37` | Close final copy authority, argument typing, export, diagnostics, and CLI gaps. |

## Verification and measurements

| Evidence | Result |
| -------- | ------ |
| `./scripts/check.sh` | Formatting, all-target checks, strict Clippy, dependency policy, and 157 tests pass. |
| `cargo test -p roe-core mica_ -- --test-threads=1` | 14 focused Mica authority, precedence, syntax, prompt, lifecycle, replacement, and shutdown tests pass. |
| `./scripts/test-phase0-terminal-workflows.sh` | Release terminal workflows pass through the production Mica session. |
| `cargo test -p roe-vello production_mica_session_builds_a_vello_scene_without_a_display` | A real Mica session produces the headless Vello scene before and after an edit. |
| `cargo test -p roe-vello --test session_conformance` | Terminal and Vello consume the same real full/delta Mica session stream. |
| `cargo build --release --bin roe-vello` | The production Vello frontend builds with the Mica session path; a display-host smoke remains an open platform obligation. |
| `./scripts/measure-phase0-baseline.sh` | Completes against the production Mica path. In the recorded run: 194.150 ms Mica-session readiness, 2.308 ms per Mica insert/delete pair, 270 us per snapshot/redraw, 33,816 KiB idle terminal RSS, and 0 KiB measured RSS growth across the edit/redraw workload. |

The measurements are coarse regression evidence, not optimization claims. Unlike the earlier
Phase 0 harness, the editing metric includes two complete Mica dispatch transactions and the redraw
metric requests a revisioned session snapshot before terminal realization.
