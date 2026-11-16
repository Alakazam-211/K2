// "New file editor (preview)" (Settings → Code, build-editors.md E1b): off
// by default and kept per computer. On, markdown and code files open in the
// editor frame on the live shell (components/Editor/EditorTab); off, or for
// any other kind of file, the file viewer is today's. The old path stays
// until the frame is proven (the retirement ledger); the default turns on
// after E5.

import type { FileCategory } from '@/components/FileViewerPane/fileCategory'
import { readLocalFlag, setLocalFlag, subscribeLocalFlags, useLocalFlag } from './local-flag'

export const NEW_EDITOR_PREVIEW_KEY = 'k2.newFileEditorPreview.v1'

export const readNewEditorPreview = (): boolean => readLocalFlag(NEW_EDITOR_PREVIEW_KEY)
export const setNewEditorPreview = (on: boolean): void => setLocalFlag(NEW_EDITOR_PREVIEW_KEY, on)
export const subscribeNewEditorPreview = subscribeLocalFlags
export const useNewEditorPreview = (): boolean => useLocalFlag(NEW_EDITOR_PREVIEW_KEY)[0]

export type EditorKind = 'markdown' | 'code'

/** Which files the frame opens: markdown, and the text files K2 edits as code. */
export function newEditorKind(category: FileCategory): EditorKind | null {
  if (category === 'markdown') return 'markdown'
  if (category === 'text') return 'code'
  return null
}
