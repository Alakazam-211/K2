// A live file, as the shared frame sees it (build-editors.md E4a). The live
// editor (live/LiveEditorFile) hands the frame its header (the state chip, the
// sentence, the avatar stack), its side panel and what the root says about
// the connection, through this context, the way EditorFile hands it saving
// (frameSave.ts). Without a provider the frame is a plain file's: no chip, no
// sentence, no panel.

import { createContext } from 'react'
import type { HeaderSentence } from '@/components/LiveView/shell/LiveHeader'
import type { LiveTabState } from '@/lib/live-view/shared/state'

export interface FrameLive {
  /** None: no chip (alone, connected). */
  state?: LiveTabState
  sentence: HeaderSentence | null
  /** The avatar stack (and E4b's Follow). */
  right?: React.ReactNode
  panel: React.ReactNode
  panelOpen: boolean
  onTogglePanel: () => void
  onClosePanel: () => void
  /** The panel's words for its toggle ("Live"). */
  panelLabel: string
  /** Not connected: others' carets freeze in grey. */
  offline: boolean
}

export const FrameLiveContext = createContext<FrameLive | null>(null)
