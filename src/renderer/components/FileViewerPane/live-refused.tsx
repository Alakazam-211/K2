// A file whose live open or session the daemon declined (a protected
// folder, a secret, not text, another workspace…, before or after the
// welcome, from any server version) opens the usual way, saying why, until
// the tab opens another file. The words name the real reason (closedWords);
// a file in another workspace opens read only, as its words say.
import { useState, type ReactNode } from 'react'
import type { LiveDocClose } from '@/lib/editor/live/live-doc-source'
import { OTHER_WORKSPACE_WORDS, closedWords, isOtherWorkspace } from '@/lib/editor/live/live-notices'

export function useLiveRefused(filePath: string): {
  refusedWords: string | null
  /** Why the plain view is read only (another workspace), or null. */
  refusedReadOnly: string | null
  onLiveRefused: (c: LiveDocClose) => void
  refusedBar: ReactNode
} {
  const [liveRefused, setLiveRefused] = useState<{ path: string; words: string; readOnly: boolean } | null>(null)
  const here = liveRefused?.path === filePath ? liveRefused : null
  const refusedWords = here?.words ?? null
  const refusedBar = refusedWords !== null && (
    <div role="status" data-testid="file-viewer-live-refused" className="border-b border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-3 py-1 text-[11px] text-[var(--color-text-secondary)] flex-shrink-0">
      {here?.readOnly ? refusedWords : `${refusedWords} It opens the usual way.`}
    </div>
  )
  return {
    refusedWords,
    refusedReadOnly: here?.readOnly ? OTHER_WORKSPACE_WORDS : null,
    onLiveRefused: (c) => setLiveRefused({ path: filePath, words: closedWords(c), readOnly: isOtherWorkspace(c.code) }),
    refusedBar,
  }
}
