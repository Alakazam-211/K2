import { useEffect, useCallback } from 'react'
import type { ActiveAgent } from '@/stores/active-agents'

interface AgentCloseDialogProps {
  agents: ActiveAgent[]
  mode: 'tab' | 'app'
  onConfirm: () => void
  onCancel: () => void
}

export default function AgentCloseDialog({
  agents,
  mode,
  onConfirm,
  onCancel
}: AgentCloseDialogProps): React.JSX.Element {
  // Escape to cancel
  useEffect(() => {
    const handler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        onCancel()
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [onCancel])

  const handleBackdropClick = useCallback(
    (e: React.MouseEvent) => {
      if (e.target === e.currentTarget) onCancel()
    },
    [onCancel]
  )

  const isSingle = agents.length === 1
  const title = mode === 'app'
    ? 'Active Agents Running'
    : isSingle
      ? `${agents[0].command} is Running`
      : 'Active Agents Running'

  return (
    <div
      className="fixed inset-0 z-[1000] flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.6)' }}
      onClick={handleBackdropClick}
    >
      <div
        className="max-w-md w-full mx-4"
        style={{
          backgroundColor: '#111',
          border: '1px solid var(--color-border)',
        }}
      >
        {/* Header */}
        <div className="px-5 pt-5 pb-3">
          <h3 className="text-sm font-medium text-[var(--color-text-primary)]">
            {title}
          </h3>
        </div>

        {/* Body */}
        <div className="px-5 pb-4">
          {mode === 'tab' && isSingle && (
            <p className="text-xs text-[var(--color-text-muted)] leading-relaxed">
              <span className="text-[var(--color-text-secondary)] font-medium">{agents[0].command}</span> is actively running in this terminal.
              Closing will terminate the session.
            </p>
          )}

          {(mode === 'app' || !isSingle) && (
            <>
              <p className="text-xs text-[var(--color-text-muted)] mb-3 leading-relaxed">
                {mode === 'app'
                  ? 'The following agents are still running. Quitting will terminate all of them.'
                  : 'The following agents are running in this tab. Closing will terminate them.'}
              </p>
              <div
                className="space-y-1 max-h-40 overflow-y-auto"
                style={{
                  backgroundColor: '#0a0a0a',
                  border: '1px solid var(--color-border)',
                  padding: '8px 12px',
                }}
              >
                {agents.map((agent) => (
                  <div key={agent.terminalId} className="flex items-center gap-2 text-xs">
                    <span className="flex-shrink-0 rounded-full" style={{ width: 6, height: 6, backgroundColor: '#22c55e' }} />
                    <span className="text-[var(--color-text-secondary)] font-medium">{agent.command}</span>
                    <span className="text-[var(--color-text-muted)]">in</span>
                    <span className="text-[var(--color-text-secondary)] truncate">{agent.tabTitle}</span>
                  </div>
                ))}
              </div>
            </>
          )}
        </div>

        {/* Actions */}
        <div className="px-5 pb-5 flex items-center justify-end gap-3">
          <button
            onClick={onCancel}
            className="px-4 py-1.5 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
          >
            Cancel
          </button>
          <button
            onClick={onConfirm}
            className="px-4 py-1.5 text-xs text-white bg-red-600 hover:bg-red-700 transition-colors no-drag cursor-pointer"
          >
            {mode === 'app' ? 'Quit Anyway' : 'Close Anyway'}
          </button>
        </div>
      </div>
    </div>
  )
}
