# K2SO Code Editor Architecture

## Lessons from Zed's Editor

Zed implements a full text editor in Rust with a sophisticated architecture:

### Zed's Core Concepts

1. **Rope-based text storage** — Text is stored in a `SumTree<Chunk>` (balanced tree of 128-byte chunks). This enables O(log n) edits, character boundary detection, and UTF-8/UTF-16 conversions. Good for large files but complex to implement.

2. **Fragment model** — Every edit creates fragments that track authorship, visibility, and deletion timestamps. Text is never truly deleted — fragments are marked invisible. This enables undo by toggling visibility rather than replaying edits backward.

3. **Two-rope architecture** — Zed maintains a `visible_text` rope and a `deleted_text` rope. The fragment tree maps between them. This allows efficient undo without storing full history snapshots.

4. **Transaction-based undo/redo** — Edits are grouped into transactions with a 300ms auto-grouping interval. `undo_stack` and `redo_stack` hold `HistoryEntry` objects. Undo increments a count in an `UndoMap`; redo decrements it. Fragment visibility is recomputed from these counts.

5. **Display map pipeline** — Raw text goes through 6 transformation layers:
   - MultiBuffer → InlayMap → FoldMap → TabMap → WrapMap → BlockMap → DisplayMap
   - Each layer transforms coordinates and publishes edit patches upward

6. **Lamport clocks** — Each operation gets a monotonically increasing timestamp for CRDT-based collaborative editing ordering.

### What We Take From Zed

| Concept | Zed's Approach | K2SO's Approach |
|---------|---------------|-----------------|
| Text storage | Custom Rope | CodeMirror's Text (immutable tree) |
| Syntax highlighting | Tree-sitter (incremental) | CodeMirror language packages |
| Undo/Redo | Fragment visibility toggle | CodeMirror transactions + history |
| Save | Buffer dirty tracking | `isDirty` state + Tauri IPC |
| Key bindings | Action dispatch system | CodeMirror keymap extension |
| Display | GPU-rendered custom pipeline | DOM-based CodeMirror view |

### Why CodeMirror 6

Zed's approach is purpose-built for a native, GPU-rendered editor. K2SO runs its UI in a web view (Tauri), so we need a web-native solution. CodeMirror 6 implements the same architectural patterns adapted for the DOM:

- **Immutable state** — `EditorState` is immutable; changes create new states via transactions
- **Transaction grouping** — `history()` extension groups edits within a configurable interval
- **Extension system** — Language support, themes, keymaps are all composable extensions
- **Efficient updates** — Uses a persistent data structure for text, similar to Zed's rope

## K2SO Editor Implementation

### Component: `CodeEditor`

Replaces both `CodeViewer` (read-only Shiki) and the textarea fallback.

**Extensions used:**
- `@codemirror/lang-*` — Language support (syntax highlighting, auto-indent)
- `@codemirror/history` — Undo/redo with Cmd+Z / Cmd+Shift+Z
- `@codemirror/commands` — Standard editing commands
- `@codemirror/view` — Editor view + keymap for Cmd+S
- `@codemirror/theme-one-dark` — Dark theme matching K2SO's aesthetic

**Data flow:**
```
File opened → Tauri IPC (fs_read_file) → CodeMirror EditorState
    ↓
User edits → CodeMirror transactions → isDirty tracking
    ↓
Cmd+S → Tauri IPC (fs_write_file) → isDirty reset
    ↓
External change detected (polling) → State replacement (if not dirty)
```

**Key behaviors:**
- Cmd+S saves the file via Tauri IPC
- Cmd+Z / Cmd+Shift+Z for undo/redo (handled by CodeMirror's history extension)
- All standard text editing shortcuts work (Cmd+A, Cmd+C, Cmd+V, Cmd+X, etc.)
- Dirty state synced to tab store for visual indicator
- File polling continues when not dirty — external changes update the editor
- Read-only mode available for special files
