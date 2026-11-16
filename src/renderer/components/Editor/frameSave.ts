// Saving, as the shared frame sees it (build-editors.md E1c). EditorFile owns
// the save model and hands the frame what it shows and does through this
// context, so both editors (MarkdownEditor and CodeEditorTab) get the same
// save word, ⌘S in every mode, autosave on blur and Compare in the text's
// place, with no saving code of their own. Without a provider (tests, a view
// with no file behind it) the frame works as before.

import { createContext } from 'react'
import type { SaveStatus } from '@/lib/editor/save-model/reducer'

export interface FrameSave {
  /** The status line's save word; null for a file that can't be edited. */
  status: SaveStatus | null
  /** ⌘S, in every mode. */
  saveNow: () => void
  /** Focus left the tab. */
  onLeave: () => void
  /** In the text's place while it's set (Compare). */
  replaceBody: React.ReactNode | null
}

export const FrameSaveContext = createContext<FrameSave | null>(null)
