# CLAUDE.md - Development Context for Red Editor

## Project Overview

Red is a minimalistic terminal-based text editor written in Rust, designed with Emacs-like keybindings and buffer management. The architecture emphasizes clean separation of concerns and extensibility.

## Architecture Overview

### Core Components

1. **Buffer** (`buffer.rs`): Text storage and manipulation
   - Uses `ropey::Rope` for efficient text operations
   - Provides cursor movement APIs (move_left, move_right, move_up, etc.)
   - Handles text insertion/deletion at arbitrary positions
   - Converts between character positions and line/column coordinates

2. **Editor** (`editor.rs`): Central coordinator
   - Manages SlotMaps of buffers, windows, and modes
   - Handles window layout using a tree-based system (WindowNode)
   - Processes key events and dispatches to appropriate handlers
   - Manages window splitting, deletion, and switching
   - Contains global kill-ring for cut/copy/paste operations

3. **Window** (`window.rs`): View into a buffer
   - Tracks position, size, cursor position, and scroll offset
   - Calculates physical cursor positions on screen
   - Each window references a buffer and has its own cursor

4. **Keys** (`keys.rs`): Input handling and keybindings
   - Defines LogicalKey enum for platform-independent key representation
   - KeyState tracks pressed keys and chord sequences
   - DefaultBindings implements Emacs-style key mappings
   - Supports modifier keys and multi-key chords (e.g., C-x C-c)

5. **Mode** (`mode.rs`): Extensible behavior system
   - ScratchMode currently handles basic text editing
   - Modes respond to KeyActions and return ModeActions
   - Designed for extensibility (syntax highlighting, language-specific features)

6. **Kill-ring** (`kill_ring.rs`): Emacs-style cut/copy/paste system
   - Circular buffer storing killed (cut/copied) text
   - Supports consecutive kill appending
   - Provides yank (paste) and yank-pop functionality
   - Configurable capacity (default 60 entries)

7. **Main** (`main.rs`): Terminal interface and rendering
   - Event loop processing keyboard input
   - Screen rendering with Unicode box drawing characters
   - Window borders and per-window modelines
   - Echo area for messages

### Key Data Structures

- `SlotMap` used for stable IDs (WindowId, BufferId, ModeId)
- `WindowNode` enum for tree-based window layout (Leaf vs Split)
- `Frame` represents available terminal space
- `KeyState` tracks modifier keys and chord sequences

### Dependencies

- `crossterm`: Terminal manipulation and input
- `ropey`: Efficient text buffer data structure
- `slotmap`: Memory-efficient collections with stable indices

## Development Guidelines

### Code Style
- Well-documented public APIs
- Comprehensive unit tests for core functionality
- Clean error handling with Result types
- No unsafe code currently used

### Key Design Patterns
- Event-driven architecture (KeyAction → ModeAction → ChromeAction)
- Immutable operations where possible
- Position calculations separate from rendering
- Tree-based recursive operations for window management

### Testing Strategy
- Unit tests for all core buffer operations
- Window layout and splitting tests
- Key binding and cursor movement tests
- Edge case handling (empty buffers, single windows)

## Common Development Tasks

### Adding New Keybindings
1. Add new variant to `KeyAction` enum in `keys.rs`
2. Update `DefaultBindings::keystroke()` method
3. Handle the action in `Editor::key_event()`
4. Add corresponding `ChromeAction` if needed

### Adding New Editor Features
1. Define new `ModeAction` variants in `mode.rs`
2. Implement handling in `Editor::insert_text()` or similar
3. Update mode implementations to emit new actions
4. Add UI handling in main event loop

### Window Management Changes
1. Window tree operations are in `editor.rs`
2. Layout calculation in `Editor::calculate_window_layout()`
3. Rendering logic in `main.rs` draw functions
4. Border drawing uses Unicode box characters

### Buffer Operations
1. Core text operations in `buffer.rs`
2. Character-position based API for cursor movement
3. Line/column conversions handled internally
4. Ropey provides efficient text manipulation

## Current Limitations & TODOs

### Not Yet Implemented
- File I/O (loading/saving files)
- Syntax highlighting system
- Undo/redo functionality
- Search and replace
- Configuration system
- Multiple buffers per window
- Plugin system
- Region selection (mark system)
- Yank-pop functionality (M-y)

### Known Issues
- Page up/down movement not implemented
- No scroll handling for long lines
- Echo area only shows temporary messages
- No persistent status display

### Future Architecture Considerations
- Plugin system design
- Syntax highlighting integration
- Language server protocol support
- Configuration file format
- Theme/color system
- Performance optimization for large files

## Build and Test Commands

```bash
# Build project
cargo build

# Run tests
cargo test

# Run with debug info
RUST_LOG=debug cargo run

# Check code style
cargo fmt
cargo clippy
```

## Debugging Tips

- Use `echo()` function for runtime debugging output
- Tests skip echo to avoid terminal issues
- Window tree integrity can be verified with test utilities
- SlotMap provides stable IDs for debugging references

## File Organization

- Each module has comprehensive tests at the bottom
- Public APIs are well-documented
- Implementation details are often in separate impl blocks
- Helper functions marked with appropriate visibility