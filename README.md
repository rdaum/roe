# Roe / ᚱᛟ / Ryan's Own Emacs

A minimalistic text editor in the spirit of the Emacs family of editors, built in Rust.

This editor follows the Emacs tradition in three key ways: (a) it's buffer-oriented rather than
file-oriented, (b) it uses the default GNU Emacs keybinding set, and (c) it's fully programmable via
an embedded scripting language. Unlike the current trend toward "modal" editors, this is a direct
manipulation editor and proud of it.

The editor is written in Rust on top of the [compio](https://github.com/compio-rs/compio) async
runtime. Its scripting layer is being migrated from Julia to [Mica](https://github.com/rdaum/mica),
a relation-first live programming environment; until that integration lands, the built-in
keybindings and commands are defined in Rust (`roe-core/src/keys.rs` and
`roe-core/src/command_registry.rs`).

## Screenshot

![Screenshot of Roe editor](screenshot.png)

## Renderers

Roe supports two rendering backends:

- **Terminal** (`roe`): Lightweight, runs in your terminal
- **Vello/GPU** (`roe-vello`): Native window with GPU-accelerated rendering via Vello/wgpu

Both renderers share the same core editor and keybindings.

## Features

- **Emacs-style keybindings**: Familiar keyboard shortcuts for Emacs users, customizable
- **Buffer-oriented editing**: Beyond embedding a scripting language and having macros, one of the
  core pieces that differentiates an "emacs" from other editors is working with "buffers" not just
  files and having buffer interaction as a fundamental tool use. Windows are views into buffers, and
  not all buffers need to be backed by files.
- **Window management**: Split windows horizontally/vertically, switch between them, same as emacs.
- **Mouse support**: Even in console mode, click to position cursor, drag window borders to resize,
  click to switch windows, etc.
- **Modular architecture**: Has an extensible mode system for different editing behaviors
- **Dual rendering**: Terminal or GPU-accelerated native window

## Key Bindings

Keybindings follow GNU Emacs conventions and are defined in Rust (`roe-core/src/keys.rs`); they can
be overridden at runtime via the `Bindings` interface. Once the Mica scripting runtime is
integrated, bindings and commands will be definable from scripts.

### Cursor Movement

#### Basic Movement

- Arrow keys or `C-f/b/n/p`: Move right/left/down/up
- `C-a`: Beginning of line
- `C-e`: End of line
- `Home/End`: Beginning/end of line

#### Word Movement

- `M-f` or `C-Right`: Move forward by word
- `M-b` or `C-Left`: Move backward by word

#### Paragraph Movement

- `M-{`: Move backward by paragraph
- `M-}`: Move forward by paragraph

#### Page Movement

- `C-v` or `Page Down`: Page down
- `M-v` or `Page Up`: Page up
- `M-Up`: Page up (alternative)
- `M-Down`: Page down (alternative)

#### Buffer Movement

- `C-Home`: Beginning of buffer
- `C-End`: End of buffer

### Window Management

- `C-x 2`: Split window horizontally
- `C-x 3`: Split window vertically
- `C-x o`: Switch to other window
- `C-x 0`: Delete current window
- `C-x 1`: Delete all other windows

### Buffer Management

- `C-x b`: Switch to another buffer
- `C-x k`: Kill (close) a buffer

### Mouse Operations

- **Click**: Position cursor at click location
- **Click in window**: Switch to clicked window
- **Drag window borders**: Resize windows by dragging their borders
- **Mouse events in modes**: Mouse events are forwarded to modes for future extensibility

### File Operations

- `C-x C-f`: Find file
- `C-x C-s`: Save file

### Editing

- Type to insert text
- `<Backspace>`: Delete character before cursor
- `<Delete>`: Delete character at cursor
- `<Enter>`: Insert newline

### Region Selection & Kill Ring

#### Region Selection

- `C-Space`: Set mark at cursor (start region selection)

#### Kill Ring Operations

- `C-w`: Kill (cut) region between mark and cursor
- `M-w`: Copy region to kill ring without deleting
- `C-k`: Kill (cut) from cursor to end of line
- `C-y`: Yank (paste) most recent kill
- `C-S-y`: Yank from kill-ring index 0

### Command & Control

- `M-x`: Command mode (interactive command execution)
- `C-g`: Cancel current operation (e.g., clear region selection)
- `C-x C-c`: Quit
- `Esc`: Escape

## Building and Running

```bash
# Build the project
cargo build --release

# Run terminal version
./scripts/run.sh [files...]

# Run Vello/GPU version
./scripts/run-vello.sh [files...]
```

## Configuration

Configuration (fonts, colours, custom keybindings) will be provided by the Mica scripting runtime
once it is integrated; for now the editor runs with its built-in defaults.

## Architecture

Roe is built with a clean separation of concerns:

- **roe-core**: Core editor logic, buffer management, window system
- **roe-terminal**: Terminal renderer using crossterm
- **roe-vello**: GPU renderer using Vello/wgpu with Parley for text layout

Key concepts:

- **Buffer**: Text storage using `ropey` for efficient editing
- **Window**: View into a buffer with cursor and scroll position
- **Mode**: Defines behavior and keybindings for different editing contexts
- **Editor**: Coordinates buffers, windows, and modes
- **Frame**: Represents available screen real estate

## Current Status

This is a work-in-progress editor. Currently implemented:

- **Text editing**: Basic insertion, deletion, cursor movement
- **Advanced movement**: Word-wise, paragraph-wise, and page navigation with Emacs key bindings
- **Window management**: Split windows horizontally/vertically, switch between windows
- **Buffer management**: Multiple buffers, switching, killing with interactive selection
- **Region selection**: Mark system with visual highlighting
- **Kill ring**: Cut, copy, paste with kill ring history
- **Command mode**: Interactive command execution (M-x) with completion
- **File operations**: Open and save files with interactive file selector
- **Mouse integration**: Click-to-position cursor, window switching, border dragging for resizing
- **Incremental search (isearch)**: Simple forward and backward incremental search.
- **Dual rendering**:
  - Terminal UI with efficient incremental rendering via crossterm
  - GPU-accelerated native window via Vello/wgpu with configurable fonts

## Next steps / not yet implemented

- **Scripting integration**: Wire up Mica as the extension language (replacing the former Julia
  integration) for commands, keybindings, modes, syntax highlighting, and configuration
- **Macro system**: Record and playback keystroke sequences
- **Search and replace**: Interactive search, query-replace functionality
- **LSP integration**: Language server protocol support for modern development features
- **Advanced editing**: Multiple cursors, rectangular selections, etc.
- **Undo/redo**: Currently partially implemented

## Contributing & Feedback

This editor is very much a work-in-progress and almost certainly has bugs. It also probably won't
meet your real editing needs yet. However, feedback and bug reports are very welcome!

If you encounter issues or have suggestions, please file them in the project's issue tracker. Even
if the editor isn't ready for daily use, your input helps guide development priorities and catch
problems early.

**Please report:**

- Crashes or unexpected behavior
- Missing features that are essential for your workflow
- Performance issues
- Ideas for improvements or missing Emacs functionality
