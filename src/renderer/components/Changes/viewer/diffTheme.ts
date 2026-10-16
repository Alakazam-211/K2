// K2 colours for @codemirror/merge. The editor's own theme (background,
// gutters, syntax) comes from the user's code-editor theme (CodeEditor's
// getThemeExtension / getHighlightExtension); this layer only paints the
// diff marks, from the app's style tokens so every K2 style (dark or
// light) gets matching adds/removes.
//
// The merge package's base theme uses `&light` / `&dark` selectors, which
// out-rank a plain theme's selectors, so the colour rules carry !important.

import { EditorView } from '@codemirror/view'

const ok = 'var(--color-status-ok)'
const err = 'var(--color-status-error)'
const mix = (c: string, pct: number): string => `color-mix(in srgb, ${c} ${pct}%, transparent) !important`

export const k2DiffTheme = EditorView.theme({
  // Whole changed lines.
  '&.cm-merge-a .cm-changedLine, .cm-deletedChunk': { backgroundColor: mix(err, 12) },
  '&.cm-merge-b .cm-changedLine, .cm-inlineChangedLine': { backgroundColor: mix(ok, 12) },
  // The changed characters inside them.
  '&.cm-merge-a .cm-changedText, .cm-deletedChunk .cm-deletedText': { background: mix(err, 32) },
  '&.cm-merge-b .cm-changedText': { background: mix(ok, 32) },
  '&.cm-merge-b .cm-deletedText': { background: mix(err, 32) },
  '.cm-deletedChunk': { paddingLeft: '6px' },
  '.cm-insertedLine, .cm-deletedLine, .cm-deletedLine del': { textDecoration: 'none' },
  // Gutter bars.
  '.cm-changeGutter': { width: '3px', paddingLeft: '1px' },
  '&.cm-merge-a .cm-changedLineGutter, .cm-deletedLineGutter': { background: `${err} !important` },
  '&.cm-merge-b .cm-changedLineGutter': { background: `${ok} !important` },
  '.cm-inlineChangedLineGutter': { background: 'var(--color-accent) !important' },
  // "⦚ N unchanged lines ⦚" bars.
  '.cm-collapsedLines': {
    color: 'var(--color-text-muted) !important',
    background: 'var(--color-bg-surface) !important',
    borderTop: '1px solid var(--color-border)',
    borderBottom: '1px solid var(--color-border)',
    fontSize: '11px',
    padding: '3px 5px 3px 10px',
  },
  '.cm-collapsedLines:hover': { color: 'var(--color-text-primary) !important' },
  // A read-only viewer: no blinking caret over the whole doc, but keep
  // the selection visible so F7 shows where it landed.
  '.cm-cursor': { borderLeftColor: 'var(--color-accent)' },
  '.cm-content': { padding: '4px 0' },
  '&.cm-focused': { outline: 'none' },
})

/** Stacked ("view all") editors grow with their content; the list scrolls. */
export const autoHeightTheme = EditorView.theme({
  '&': { height: 'auto !important' },
  '.cm-scroller': { overflow: 'visible !important' },
})

/** Single view: the editor fills the pane and scrolls itself. */
export const fillHeightTheme = EditorView.theme({
  '&': { height: '100%' },
  '.cm-scroller': { overflow: 'auto' },
})
