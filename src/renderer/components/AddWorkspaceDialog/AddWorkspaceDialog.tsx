import React, { useEffect } from 'react'
import { useAddWorkspaceDialogStore } from '../../stores/add-workspace-dialog'

// The Skip / Start-Fresh / Adopt consent picker was removed with the
// canonical-agents PRD (§7 Removal Catalog): harness fan-out is now off by
// default, so adding a workspace no longer touches any harness file and
// needs no consent gate. The value-pitch WHY copy ("Tell K2 once, every
// AI tool listens…") moved verbatim into the K2 Canonical Agent skill
// briefing + the canonical button subtitle in the Agent section — that is
// where unification is actually opt-in. This dialog is now a plain confirm.
export default function AddWorkspaceDialog(): React.JSX.Element | null {
  const isOpen = useAddWorkspaceDialogStore((s) => s.isOpen)
  const isPending = useAddWorkspaceDialogStore((s) => s.isPending)
  const path = useAddWorkspaceDialogStore((s) => s.path)
  const error = useAddWorkspaceDialogStore((s) => s.error)
  const onConfirm = useAddWorkspaceDialogStore((s) => s.onConfirm)
  const close = useAddWorkspaceDialogStore((s) => s.close)
  const setIsPending = useAddWorkspaceDialogStore((s) => s.setIsPending)
  const setError = useAddWorkspaceDialogStore((s) => s.setError)

  // Close on Escape
  useEffect(() => {
    if (!isOpen) return
    const handler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' && !isPending) close()
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [isOpen, isPending, close])

  const handleAdd = async (): Promise<void> => {
    if (!onConfirm) return
    setIsPending(true)
    try {
      await onConfirm()
      close()
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err))
      setIsPending(false)
    }
  }

  if (!isOpen || !path) return null

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center no-drag"
      style={{ backgroundColor: 'rgba(0, 0, 0, 0.6)', backdropFilter: 'blur(4px)' }}
      onClick={isPending ? undefined : close}
    >
      <div
        className="w-[520px] max-h-[85vh] flex flex-col border border-[var(--color-border)] bg-[var(--color-bg-surface)] shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="px-5 pt-5 pb-2">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">
            Add Workspace to K2
          </h2>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-1 break-all font-mono">
            {path}
          </p>
        </div>

        {/* Plain-language reassurance — adding a workspace is non-destructive. */}
        <div className="px-5 pb-3">
          <div className="border border-[var(--color-border)] bg-[var(--color-bg)]/40 px-3 py-3 text-[11px] leading-relaxed text-[var(--color-text-secondary)] space-y-2">
            <p>
              K2 connects to this folder without touching any of your files. Your existing
              AI-tool notes (<span className="font-mono">CLAUDE.md</span>,{' '}
              <span className="font-mono">GEMINI.md</span>, …) stay exactly as they are.
            </p>
            <p>
              When you want every AI tool to share one set of project notes, run the{' '}
              <span className="text-[var(--color-text-primary)]">K2 Canonical Agent</span> from
              Workspace Settings → Agent — it unifies your harness files safely and reversibly.
            </p>
          </div>
        </div>

        {/* Error */}
        {error ? (
          <div className="px-5 pb-4">
            <div className="border border-red-500/30 bg-red-500/10 px-3 py-2">
              <p className="text-[11px] text-red-400 whitespace-pre-wrap">{error}</p>
            </div>
          </div>
        ) : null}

        {/* Actions */}
        <div className="px-5 pb-5 flex gap-2 justify-end border-t border-[var(--color-border)] pt-3">
          <button
            className="px-3 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] disabled:opacity-40"
            onClick={close}
            disabled={isPending}
          >
            Cancel
          </button>
          <button
            className="px-3 py-1.5 text-xs font-medium text-[var(--color-bg)] bg-[var(--color-text-primary)] hover:bg-[var(--color-text-secondary)] disabled:opacity-40"
            onClick={handleAdd}
            disabled={isPending}
          >
            {isPending ? 'Adding…' : 'Add workspace'}
          </button>
        </div>
      </div>
    </div>
  )
}
