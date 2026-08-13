# Phase 5 Mica policy transfer

This document records the Phase 5 transfer implemented after the first vertical slice in
[ROE-MICA-ROADMAP.md](../ROE-MICA-ROADMAP.md). Both production frontends now run the same
Mica-enabled `HostSession`. Mica owns global command identity, discovery, binding precedence, chord
prefixes, invocation, selection candidate policy, and logical window-operation choice. Rust owns
direct text mechanisms, native selectors, validated layout, presentation extraction, and renderer
realization.

Phase 5 was delivered as these reviewable changes:

| Commit | Change |
| ------ | ------ |
| `524b73e` | Moved global commands, Emacs chords, prefix recognition, and window-operation policy to Mica; enabled Mica in Vello. |
| `cd62ad4` | Made Mica discovery populate the command palette and Mica authority populate buffer selectors; removed production command-registry invocation. |
| `56060db` | Kept newline and tab as direct native text mechanisms rather than unresolved Rust command policy. |
| `91f51ee` | Corrected stale integration comments to state the implemented Mica/native ownership. |

## Authoritative path

`mica/roe-first-wave.mica` now defines the production global command package. Each command has a
Mica identity, user-visible name and summary, selector, package membership, role grant, optional
host-action relation, and key bindings. `roe/dispatch_key` in `mica/roe-model.mica` selects the
highest combined session/binding precedence and recognizes the Mica-owned `KeyPrefix("C-x")`.
Rust only retains the pending prefix bytes needed to join separately delivered platform key events.

The production route is:

```text
terminal or Vello input
  -> renderer-neutral InputEvent
  -> Mica EffectiveSessionKeymap / EffectiveBinding
  -> Mica command verb
  -> committed host_action and candidate effects
  -> HostSession native realization
  -> renderer-neutral PresentationUpdate / LifecycleEvent
  -> terminal cells or Vello scene
```

`HostSession::open_with_mica` replaces the editor's construction-time bindings with
`ConfigurableBindings::new_native_fallback`. That table contains no named command, global window,
redraw, save, file, buffer, search, or quit binding. It exists for character insertion, cursor
motion, region/kill/yank, undo, and other direct editing mechanisms that have not become Mica
commands. Therefore `C-x C-s`, `C-x C-c`, `C-x C-f`, `C-x C-v`, `C-x 2`, `C-x 3`, `C-x o`,
`C-x 0`, `C-x 1`, `C-x b`, `C-x k`, `M-x`, `C-s`, `C-r`, `C-l`, and `F12` have one production
owner: Mica.

The old complete `ConfigurableBindings` and `CommandRegistry` remain reachable only from the plain
`HostSession::open` compatibility constructor and direct legacy unit tests. Neither production
frontend uses that constructor. This preserves renderer-conformance fixtures while preventing
dual command ownership in a live Roe session; the compatibility surfaces can be deleted when those
historical tests are rewritten around Mica session fixtures.

## Command and minibuffer selection

The `execute_command` verb enumerates authority-filtered `DiscoverableCommand` facts and emits each
command's Mica-owned name and `CommandHostAction`. The host presents exactly that settled candidate
set in the existing command-window renderer. Selecting an entry returns only its name; `HostSession`
resolves it against the candidate map from the same Mica transaction. `Editor` no longer invokes
the Rust command registry when command mode confirms a selection.

This division keeps filtering, selection movement, text entry, and drawing in the native generic
selector mechanism while moving command existence, names, authority, package activation, and
meaning to Mica. A live replacement can therefore add, remove, rename, rebind, or disable a command
without changing either renderer or registering a Rust command object.

Buffer selection follows the same rule. The Mica `switch_buffer` and `kill_buffer` verbs enumerate
only `LogicalBuffer` identities for which the actor has `CanUseBuffer`, attach `BufferName`, and emit
the settled candidates. Rust's selector keeps only the ephemeral `BufferId` association required to
realize the chosen switch or kill. Temporary selector buffers/views are retired from Mica and the
native bridge by the Phase 4 synchronization rule.

File selection and incremental search are Mica commands and bindings. Filesystem enumeration,
character-indexed match computation, temporary selector buffers, and highlighting remain native
mechanisms. The chosen file operation is still capability-checked at the native boundary. This is
intentional: Mica decides when and why the interaction starts; Rust performs filesystem and Rope
work and renders its transient presentation.

## Modes, faces, configuration, packages, and windows

The durable policy relations established in Phase 3 remain authoritative for major/minor mode
composition, hooks, faces, syntax rules, configuration inheritance, packages, and named units.
Phase 5 does not introduce parallel Rust policy objects for these relations. The currently installed
production package uses `fundamental_mode`; native `Mode` implementations are retained as direct
text/selector mechanism adapters until additional Mica mode packages require distinct hook or
syntax behavior. Newline and tab now insert native text directly instead of attempting to invoke
nonexistent Rust `indent-line` or `newline-and-indent` commands.

Logical window-operation choice is Mica-owned. Split, select, delete, and delete-other commands emit
typed actions; Rust validates and realizes tree geometry, then returns the same presentation to both
frontends. Renderer windows, Winit handles, terminal state, and Vello/WGPU resources never enter
Mica.

Packages remain named replaceable units. Replacement is check-then-replace, malformed source keeps
the last working policy, failures are visible and recoverable, and the in-memory world avoids
persisting endpoint actors or native capabilities. Durable user/workspace state remains optional as
the roadmap specifies; it is not enabled without revision, backup, export, and migration policy.

## Deleted or bypassed policy

- Production frontends no longer use the Rust global binding table.
- Vello no longer opens a Rust-policy-only session.
- Mica command-palette confirmation no longer executes `CommandRegistry` handlers.
- Mica supplies buffer candidates; Rust no longer decides which buffers the actor may see in that
  interaction.
- Global window, file, search, buffer, save, redraw, and quit meanings no longer originate in
  `KeyAction` variants during production input.
- Tab/newline no longer pretend to be unresolved Rust named commands.

`ChromeAction`, `ModeAction`, `EditorAction`, `BufferResponse`, native selector modes, and direct
editing `KeyAction` variants still connect old editing mechanisms internally. They are not exposed
to either frontend or Mica. Removing them requires migrating the direct editing vocabulary to
native kernel operations; it is a later deletion optimization, not a second owner for the global
policy transferred here.

## Verification and measurements

| Command or evidence | Result |
| ------------------- | ------ |
| `./scripts/check.sh` | Passes formatting, all-target checks, strict Clippy, dependency policy, and 172 workspace tests. |
| `cargo +1.95.0 check --workspace --all-targets` | Passes at the pinned Mica MSRV. |
| `cargo test -p roe-core mica_ -- --test-threads=1` | Passes Mica chord/window policy, discovery-driven command invocation, authority, retirement, background progress, replacement, cancellation, and shutdown tests. |
| `./scripts/test-phase0-terminal-workflows.sh` | Passes real production editing, save, Mica F12, Mica command/buffer/file/search selection, window operations, idle file delivery, and terminal restoration. |
| `cargo build --release --bin roe-vello` | Passes with Vello constructing the Mica-enabled host; display-host smoke remains unavailable in this environment. |
| `./scripts/measure-phase0-baseline.sh` | Completes after the transfer: 1.607 us edit round trip, 298 us terminal full redraw, 91.334 ms terminal readiness, and 22,420 KiB idle terminal RSS in this run. |

The Mica runtime increases startup and idle memory relative to the pre-embedding baseline; the
recorded figures are regression evidence, not an optimization claim. Driver queues, task budgets,
external requests, subscriptions, selector candidates, presentation slices, and native resources
retain the explicit bounds and retirement rules from Phases 1 through 4.

Phase 5 has transferred the editor's programmable global policy and selection authority without
moving Rope, filesystem, layout, or renderer mechanisms out of Rust. Both frontends consume the
same Mica-owned decisions through `HostSession`, and live replacement can change the command layer
without a renderer fork or shadow Rust registration.
