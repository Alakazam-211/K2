// A file whose live open the daemon refused (a protected folder, a secret,
// not text…) opens the usual way, saying why, until the tab opens another
// file. The words name the real reason (closedWords).
import { useState, type ReactNode } from 'react'
import type { LiveDocClose } from '@/lib/editor/live/live-doc-source'
import { closedWords } from '@/lib/editor/live/live-notices'

export function useLiveRefused(filePath: string): {
  refusedWords: string | null
  onLiveRefused: (c: LiveDocClose) => void
  refusedBar: ReactNode
} {
  const [liveRefused, setLiveRefused] = useState<{ path: string; words: string } | null>(null)
  const refusedWords = liveRefused?.path === filePath ? liveRefused.words : null
  const refusedBar = refusedWords !== null && (
    <div role="status" data-testid="file-viewer-live-refused" className="border-b border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-3 py-1 text-[11px] text-[var(--color-text-secondary)] flex-shrink-0">
      {refusedWords} It opens the usual way.
    </div>
  )
  return { refusedWords, onLiveRefused: (c) => setLiveRefused({ path: filePath, words: closedWords(c) }), refusedBar }
}
