import { useState, useEffect, useRef } from 'react'
import { useTimerStore, formatElapsed } from '@/stores/timer'

export default function MemoDialog(): React.JSX.Element | null {
  const showMemoDialog = useTimerStore((s) => s.showMemoDialog)
  const stoppedElapsed = useTimerStore((s) => s.stoppedElapsed)
  const saveEntry = useTimerStore((s) => s.saveEntry)
  const dismissMemoDialog = useTimerStore((s) => s.dismissMemoDialog)

  const [memo, setMemo] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)

  // Reset and focus on open
  useEffect(() => {
    if (showMemoDialog) {
      setMemo('')
      requestAnimationFrame(() => inputRef.current?.focus())
    }
  }, [showMemoDialog])

  // Keyboard shortcuts
  useEffect(() => {
    if (!showMemoDialog) return
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        dismissMemoDialog()
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [showMemoDialog, dismissMemoDialog])

  if (!showMemoDialog) return null

  const elapsed = stoppedElapsed ?? 0

  const handleSave = () => {
    saveEntry(memo.trim() || undefined)
  }

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.preventDefault()
      handleSave()
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm">
      <div
        className="w-[380px] border border-[var(--color-border)] bg-[var(--color-bg-surface)] p-6"
        style={{
          boxShadow: '0 8px 32px rgba(0, 0, 0, 0.6), 0 2px 8px rgba(0, 0, 0, 0.4)',
        }}
      >
        {/* Header */}
        <div className="flex items-center justify-between mb-1">
          <h3 className="text-sm font-semibold text-[var(--color-text-primary)]">
            Session Complete
          </h3>
          <span className="text-xs font-mono text-[var(--color-accent)] tabular-nums">
            {formatElapsed(elapsed)}
          </span>
        </div>

        <p className="text-xs text-[var(--color-text-muted)] mb-4">
          Add a note about what you worked on.
        </p>

        {/* Memo input */}
        <input
          ref={inputRef}
          type="text"
          value={memo}
          onChange={(e) => setMemo(e.target.value.slice(0, 200))}
          onKeyDown={handleKeyDown}
          placeholder="What did you work on?"
          maxLength={200}
          className="w-full px-3 py-2 text-sm bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none focus:border-[var(--color-accent)] transition-colors mb-1"
        />
        <div className="text-[10px] text-[var(--color-text-muted)] text-right mb-4">
          {memo.length}/200
        </div>

        {/* Actions */}
        <div className="flex items-center justify-between">
          <button
            onClick={dismissMemoDialog}
            className="text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors"
          >
            Skip
          </button>
          <button
            onClick={handleSave}
            className="px-4 py-1.5 text-xs font-medium bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity"
          >
            Save
          </button>
        </div>
      </div>
    </div>
  )
}
