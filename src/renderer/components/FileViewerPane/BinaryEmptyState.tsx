import { useCallback } from 'react'
import { daemonCliPost } from '@/lib/daemon-cli'

interface BinaryEmptyStateProps {
  filePath: string
  /** Optional override for the primary message. */
  message?: string
  /** Optional secondary detail line. */
  detail?: string
}

function revealLabel(): string {
  if (typeof navigator === 'undefined') return 'Reveal in file manager'
  const ua = navigator.userAgent
  if (/Mac|Darwin/i.test(ua) && !/iPhone|iPad|iPod/i.test(ua)) return 'Open in Finder'
  if (/Win/i.test(ua)) return 'Open in Explorer'
  if (/Linux|X11|CrOS/i.test(ua)) return 'Show in Files'
  return 'Reveal in file manager'
}

/**
 * Honest empty state for binary / non-text files — never dump into CodeMirror.
 */
export function BinaryEmptyState({
  filePath,
  message = "Binary file — can't edit as text",
  detail,
}: BinaryEmptyStateProps): React.JSX.Element {
  const fileName = filePath.split('/').pop() || filePath

  const reveal = useCallback(() => {
    void daemonCliPost('fs/open-finder', { target: filePath }).catch((err) => {
      console.warn('[file-viewer] open-finder failed:', err)
    })
  }, [filePath])

  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-3 bg-[var(--color-bg)] px-6">
      <div className="flex flex-col items-center gap-1.5 text-center max-w-md">
        <span className="text-sm text-[var(--color-text-primary)]">{message}</span>
        {detail ? (
          <span className="text-xs text-[var(--color-text-muted)]">{detail}</span>
        ) : (
          <span className="text-xs text-[var(--color-text-muted)] font-mono truncate max-w-full" title={filePath}>
            {fileName}
          </span>
        )}
      </div>
      <button
        type="button"
        className="px-3 py-1.5 text-xs font-medium border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors"
        onClick={reveal}
      >
        {revealLabel()}
      </button>
    </div>
  )
}
