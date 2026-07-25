import React from 'react'
import { useCallback, useEffect } from 'react'
import { useSettingsStore } from '@/stores/settings'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { Toggle } from '@/components/ui'
import { useClaudeAuthStore } from '@/stores/claude-auth'
import type { ClaudeAuthState } from '@/stores/claude-auth'

/**
 * Claude credential auto-refresh control.
 * `embedded` = list-row mode (status + toggle only; parent supplies icon/title).
 */
export function ClaudeAuthRefreshRow({
  embedded = false,
}: {
  embedded?: boolean
} = {}): React.JSX.Element {
  const claudeAuthAutoRefresh = useSettingsStore((s) => s.claudeAuthAutoRefresh)
  const setClaudeAuthAutoRefresh = useSettingsStore((s) => s.setClaudeAuthAutoRefresh)
  const confirm = useConfirmDialogStore((s) => s.confirm)
  const {
    state: authState,
    secondsRemaining,
    refreshing,
    fetchStatus,
    refresh,
    installScheduler,
    uninstallScheduler,
  } = useClaudeAuthStore()

  useEffect(() => {
    fetchStatus()
    const interval = setInterval(fetchStatus, 60_000)
    return () => clearInterval(interval)
  }, [fetchStatus])

  const handleToggle = useCallback(async () => {
    if (!claudeAuthAutoRefresh) {
      const confirmed = await confirm({
        title: 'Install Background Token Refresh?',
        message:
          'K2 will install a background scheduler that refreshes your Claude authentication token every 20 minutes, preventing session expiry.\n\nThis runs independently of K2 and can be disabled at any time from Settings.',
        confirmLabel: 'Install',
      })
      if (!confirmed) return
      try {
        await installScheduler()
        setClaudeAuthAutoRefresh(true)
        fetchStatus()
      } catch (e) {
        console.error('[settings] Failed to install Claude auth scheduler:', e)
      }
    } else {
      try {
        await uninstallScheduler()
        setClaudeAuthAutoRefresh(false)
        fetchStatus()
      } catch (e) {
        console.error('[settings] Failed to uninstall Claude auth scheduler:', e)
      }
    }
  }, [
    claudeAuthAutoRefresh,
    confirm,
    installScheduler,
    uninstallScheduler,
    setClaudeAuthAutoRefresh,
    fetchStatus,
  ])

  const handleRefreshNow = useCallback(async () => {
    await refresh()
    fetchStatus()
  }, [refresh, fetchStatus])

  const statusDot = (color: string) => (
    <span className="w-1.5 h-1.5 flex-shrink-0" style={{ backgroundColor: color }} />
  )

  let statusIndicator: React.ReactNode = null
  if (authState !== 'unknown') {
    const remaining = secondsRemaining ?? 0
    const minutes = Math.floor(Math.abs(remaining) / 60)

    const config: Record<ClaudeAuthState, { color: string; text: string }> = {
      valid: { color: 'var(--color-status-ok)', text: `Valid (${minutes}m)` },
      expiring: { color: 'var(--color-status-warn-soft)', text: 'Expiring soon' },
      expired: { color: 'var(--color-status-error)', text: 'Expired' },
      missing: { color: 'var(--color-neutral)', text: 'No credentials' },
      unknown: { color: 'var(--color-neutral)', text: '' },
    }

    const { color, text } = config[authState]
    statusIndicator = (
      <div className="flex items-center gap-1.5 mr-3">
        {statusDot(color)}
        <span className="text-[10px] text-[var(--color-text-muted)] whitespace-nowrap">{text}</span>
        {(authState === 'expiring' || authState === 'expired') && (
          <button
            type="button"
            onClick={() => void handleRefreshNow()}
            disabled={refreshing}
            className="text-[10px] text-[var(--color-accent)] hover:underline cursor-pointer no-drag disabled:opacity-50"
          >
            {refreshing ? '...' : 'Refresh'}
          </button>
        )}
      </div>
    )
  }

  const toggle = (
    <Toggle
      checked={claudeAuthAutoRefresh}
      onChange={() => void handleToggle()}
      aria-label="Auto-refresh Claude credentials"
    />
  )

  if (embedded) {
    return (
      <div className="flex items-center flex-shrink-0">
        {statusIndicator}
        {toggle}
      </div>
    )
  }

  return (
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">
          Auto-refresh Claude credentials
        </span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          Background scheduler keeps your Claude session alive
        </p>
      </div>
      <div className="flex items-center flex-shrink-0">
        {statusIndicator}
        {toggle}
      </div>
    </div>
  )
}
