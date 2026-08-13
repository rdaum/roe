# ADR 0005: migrate policy in vertical slices and delete superseded Rust

Status: accepted for Phase 3

## Context

Roe's current Rust editor path contains overlapping command, keymap, mode, selection-menu, action,
and buffer-host abstractions. A permanent bridge that keeps them synchronized with Mica would create
two editors.

## Decision

Phase 4 bypasses Rust command lookup for the selected Mica key binding and owns its complete policy
in the `roe/core` unit. It retains `HostSession`, `NativeKernel`, presentation types, renderers, and
the transitional Rust `Editor` only for unmigrated keys.

Phase 5 transfers and removes policy in this order:

| Slice                 | Mica becomes authoritative                                  | Rust path to delete or shrink after parity                                                                                                                              |
| --------------------- | ----------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| commands and keymaps  | command facts, discovery, invocation, bindings, precedence  | `CommandRegistry`, `Command`, `CommandMode`, `Bindings`, `DefaultBindings`, `ConfigurableBindings`, policy portions of `KeyState`                                       |
| minibuffer/completion | prompt state, argument acquisition, candidates, history     | `SelectionMenu`, command-window policy and corresponding `ChromeAction` variants                                                                                        |
| buffer/file/search    | logical buffer selection, file prompt policy, isearch state | `BufferSwitchMode`, `FileSelectorMode`, `IsearchMode`; retain native file/watch/search primitives only where justified                                                  |
| modes/hooks           | major/minor modes and ordered hook composition              | `Mode` policy trait, `ScratchMode`, `FileMode`, `MessagesMode`, `ModeAction`/`ModeResult`                                                                               |
| faces/syntax          | face and syntax facts, invalidation policy                  | global `FaceRegistry` policy; retain compact renderer-ready spans/cache                                                                                                 |
| editing/window policy | kill/yank policy, window commands, view decisions           | policy portions of `KillRing`, `Editor`, `EditorAction`, `ChromeAction`, and `BufferHost`; retain Rope, clipboard adapter, layout validation, session/presentation host |

Each slice must first pass a terminal transcript, native-kernel test, failure/replacement test, and
renderer-neutral presentation comparison. The old path is then removed in the same slice. A
temporary fallback is allowed only for keys not yet claimed by the active Mica keymaps.

## Consequences

There is one authority for every migrated behavior, and repository size/complexity falls as Mica
coverage grows. Phase 5 completion is measured by deletion and ownership, not merely by adding Mica
wrappers.
