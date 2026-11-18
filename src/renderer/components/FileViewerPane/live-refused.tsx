// A file whose live open or session the daemon declined (a protected
// folder, a secret, not text, another workspace…, before or after the
// welcome, from any server version) opens the usual way, saying why, until
// the tab opens another file. The words name the real reason (closedWords);
// a file in another workspace opens read only, as its words say. Reload
// (on the bar) reads the file again and tries the live open again, in the
// same tab: refused again, the bar comes back with the reason.
import { useState, type ReactNode } from 'react'
import type { LiveDocClose } from '@/lib/editor/live/live-doc-source'
import { OTHER_WORKSPACE_WORDS, closedWords, isOtherWorkspace } from '@/lib/editor/live/live-notices'

export function useLiveRefused(
  filePath: string,
  /** Read the file again and ask for its live open again (the host's). */
  onReload?: () => void,
): {
  refusedWords: string | null
  /** Why the plain view is read only (another workspace), or null. */
  refusedReadOnly: string | null
  onLiveRefused: (c: LiveDocClose) => void
  /** Forget the refusal, read the file again and retry the live open. */
  reload: () => void
  refusedBar: ReactNode
} {
  const [liveRefused, setLiveRefused] = useState<{ path: string; words: string; readOnly: boolean } | null>(null)
  const here = liveRefused?.path === filePath ? liveRefused : null
  const refusedWords = here?.words ?? null
  const reload = (): void => {
    setLiveRefused(null)
    onReload?.()
  }
  const refusedBar = refusedWords !== null && (
    <div role="status" data-testid="file-viewer-live-refused" className="flex items-center gap-2 border-b border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-3 py-1 text-[11px] text-[var(--color-text-secondary)] flex-shrink-0">
      <span className="min-w-0 flex-1" data-testid="file-viewer-live-refused-words">{here?.readOnly ? refusedWords : `${refusedWords} It opens the usual way.`}</span>
      {onReload && (
        <button
          type="button"
          data-testid="file-viewer-live-reload"
          title="Read the file again and try live editing again"
          className="shrink-0 rounded-[var(--radius-field)] border border-[var(--color-border)] px-2 py-0.5 text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]"
          onClick={reload}
        >
          Reload
        </button>
      )}
    </div>
  )
  return {
    refusedWords,
    refusedReadOnly: here?.readOnly ? OTHER_WORKSPACE_WORDS : null,
    onLiveRefused: (c) => setLiveRefused({ path: filePath, words: closedWords(c), readOnly: isOtherWorkspace(c.code) }),
    reload,
    refusedBar,
  }
}
