// The new file editors' preview switches (Settings → Multiplayer, beside the
// live views' preview switches; build-editors.md E1b). Two editors on one
// frame, each with its own switch, off by default and kept per computer:
//   "New markdown editor": .md, .markdown and .mdx files (MarkdownEditor);
//   "New code editor": every other file K2 edits as text: code, config, plain
//   text and logs, and the source of CSV/TSV, HTML, Mermaid, diagram and DXF
//   files, whose current preview stays their Preview (CodeEditorTab).
// Off, or for any other kind of file, the file viewer is today's. The old
// path stays until the editors are proven (the retirement ledger); the
// defaults turn on after E5.
//
// One switch came first ("New file editor (preview)", `k2.newFileEditorPreview.v1`):
// on, it turns both new switches on once (when this module loads), and the old key goes.

import type { FileCategory } from '@/components/FileViewerPane/fileCategory'
import { readLocalFlag, setLocalFlag, subscribeLocalFlags, useLocalFlag } from './local-flag'

export const NEW_MARKDOWN_EDITOR_KEY = 'k2.newMarkdownEditor.v1'
export const NEW_CODE_EDITOR_KEY = 'k2.newCodeEditor.v1'
/** The single switch before the editors split (2026-11-15). Read only to migrate it. */
export const LEGACY_EDITOR_PREVIEW_KEY = 'k2.newFileEditorPreview.v1'

export type EditorKind = 'markdown' | 'code'

export const EDITOR_KEYS: Record<EditorKind, string> = { markdown: NEW_MARKDOWN_EDITOR_KEY, code: NEW_CODE_EDITOR_KEY }

/** The old single switch, on: both new switches on. Then the old key goes, so it migrates once. */
export function migrateLegacyEditorPreview(): void {
  try {
    const old = window.localStorage.getItem(LEGACY_EDITOR_PREVIEW_KEY)
    if (old === null) return
    window.localStorage.removeItem(LEGACY_EDITOR_PREVIEW_KEY)
    if (old === '1') for (const key of Object.values(EDITOR_KEYS)) setLocalFlag(key, true)
  } catch {
    // Blocked storage: nothing was kept to migrate.
  }
}

export function readEditorPreview(kind: EditorKind): boolean {
  migrateLegacyEditorPreview()
  return readLocalFlag(EDITOR_KEYS[kind])
}
export const setEditorPreview = (kind: EditorKind, on: boolean): void => setLocalFlag(EDITOR_KEYS[kind], on)
export const subscribeEditorPreview = subscribeLocalFlags
/** Follows the switch live, in this window and every other one. */
export const useEditorPreview = (kind: EditorKind): boolean => useLocalFlag(EDITOR_KEYS[kind])[0]

// Once per window, before anything reads a switch (the hook never writes while rendering).
migrateLegacyEditorPreview()

/** Text files with their own preview: the code editor shows it as Preview, beside Source (until E6 restyles them). */
export const PREVIEW_CATEGORIES: readonly FileCategory[] = ['csv', 'html', 'mermaid', 'diagram', 'cad2d']

/** Which editor a file opens in (switch on): markdown, or the text files K2 edits in CodeMirror. */
export function newEditorKind(category: FileCategory): EditorKind | null {
  if (category === 'markdown') return 'markdown'
  if (category === 'text' || PREVIEW_CATEGORIES.includes(category)) return 'code'
  return null
}
