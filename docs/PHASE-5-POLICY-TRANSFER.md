# Phase 5 Mica policy transfer

Phase 5 completes the policy transfer described in
[ROE-MICA-ROADMAP.md](../ROE-MICA-ROADMAP.md). Mica is now the sole production owner of editor
meaning: commands, bindings, prompts, completion, search state, modes, hooks, faces, syntax,
configuration, packages, and logical view decisions. Rust realizes bounded native mechanisms and
renderer-neutral presentation. Both the terminal and Vello frontends open the same Mica-enabled
`HostSession`.

This is an ownership boundary, not a claim that all of Roe is implemented in Mica. Rope storage,
filesystem and watcher operations, clipboard access, validated window-tree mutation, terminal
cells, glyph layout, Winit, WGPU, and Vello scenes remain native by design.

## Production path

```text
terminal or Vello input
  -> renderer-neutral InputEvent
  -> Mica prompt state or key dispatch
  -> Mica command/policy transaction
  -> bounded native action or host effect
  -> Rust resource/layout mechanism
  -> renderer-neutral PresentationUpdate and LifecycleEvent
  -> terminal cells or Vello scene
```

`HostSession::open_with_mica` is the production constructor. `HostSession::open` remains only as a
policy-free protocol/native-mechanism test harness; it has no Rust command or binding fallback.
`HostSession::initial_output` publishes the initial Mica policy before the first presentation, and
subsequent key transactions republish an atomic policy snapshot so replacement can remove facts
without leaving stale Rust projections.

## Transferred slices

### Commands, discovery, invocation, and keymaps

Mica owns `Command`, `CommandName`, `CommandSummary`, `CommandSelector`, `PackageCommand`,
`KeyBinding`, `NativeBinding`, `KeyPrefix`, `SessionKeymap`, `EffectiveSessionKeymap`, and
`EffectiveBinding`. `roe/dispatch_key` resolves combined keymap/binding precedence, detects equal
precedence ambiguity, invokes named commands, and selects bounded native editing actions. Printable
characters are also selected by this verb, so ordinary insertion has no production Rust binding
fallback.

`roe/DiscoverableCommand` filters M-x candidates through active packages and endpoint authority.
M-x invokes the selected Mica selector directly; commands do not require a shadow Rust registry or
host-action declaration to be discoverable.

Rust retains only the normalized platform key vocabulary and the native operation vocabulary used
to realize Mica's decision.

### Minibuffer, completion, files, buffers, and search

`PromptState`, `PromptLast`, `FileCandidate`, `ArgumentCandidate`, and the
`roe/prompt_key`, `roe/refresh_prompt`, and `roe/search_prompt_key` verbs own prompt text,
selection, cancellation, command argument acquisition, history, and filtering. Command, buffer,
and file candidates are computed in Mica and capped at 256 entries. Search state and selection live
in Mica; the native `text_search` request returns at most 1,024 character-indexed matches.

Rust retains directory enumeration, Rope searching, fallible file open/save, file watching, and a
passive prompt view used by both renderers. It receives the selected identity or path only after
Mica has applied actor, package, and prompt policy.

The deleted Rust owners are `CommandMode`, `SelectionMenu`, `BufferSwitchMode`,
`FileSelectorMode`, `IsearchMode`, `CommandRegistry`, and their candidate/action plumbing.

### Modes, hooks, faces, syntax, indentation, and configuration

Mica owns `BufferMajorMode`, `BufferMinorMode`, `ModeKeymap`, `ModeHook`, `Face`,
`FaceAttribute`, `FaceParent`, `SyntaxRule`, `Configuration`, and their `Effective*` rules.
`roe/publish_policy` emits a reset followed by a bounded projection of effective mode, face,
syntax, and configuration facts. `roe/dispatch_key` emits ordered effective hooks after editing.
Tab width comes from `EffectiveConfiguration`; word editing consumes Mica's effective syntax rule;
search highlighting consumes Mica face attributes. Hook invalidation reaches the common
presentation stream, not either renderer directly.

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
authority. `HostSession` exposes check-before-replace, named-unit replacement, fileout/export,
first-wave restore, and package enable/disable operations. Malformed replacement leaves the last
working unit live; a valid replacement atomically resets projected policy and removes stale
settings.

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

Authority is checked at the endpoint, service, logical-buffer, and native-resource layers. Native
failures are returned through typed completion/lifecycle results. Cancellation, endpoint close,
replacement failure, queue pressure, and failed watcher cleanup have focused tests.

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

## Verification and measurements

| Evidence | Result |
| -------- | ------ |
| `./scripts/check.sh` | Formatting, all-target checks, strict Clippy, dependency policy, and 150 tests pass. |
| `cargo test -p roe-core mica_ -- --test-threads=1` | 11 focused Mica authority, policy, prompt, lifecycle, replacement, and shutdown tests pass. |
| `./scripts/test-phase0-terminal-workflows.sh` | Release terminal workflows pass through the production Mica session. |
| `cargo build --release --bin roe-vello` | The production Vello frontend builds with the Mica session path; a display-host smoke remains an open platform obligation. |
| `./scripts/measure-phase0-baseline.sh` | Completes against the production Mica path. In the recorded run: 85.022 ms Mica-session readiness, 2.224 ms per Mica insert/delete pair, 267 us per snapshot/redraw, and 23,924 KiB idle terminal RSS. |

The measurements are coarse regression evidence, not optimization claims. Unlike the earlier
Phase 0 harness, the editing metric includes two complete Mica dispatch transactions and the redraw
metric requests a revisioned session snapshot before terminal realization.
