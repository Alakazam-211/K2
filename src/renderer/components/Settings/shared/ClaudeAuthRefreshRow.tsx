import React from 'react'
import { useCallback, useEffect } from 'react'
import { useSettingsStore } from '@/stores/settings'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useClaudeAuthStore } from '@/stores/claude-auth'
import type { ClaudeAuthState } from '@/stores/claude-auth'

export function ClaudeAuthRefreshRow(): React.JSX.Element {
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

  // Poll status every 60s while mounted
  useEffect(() => {
    fetchStatus()
    const interval = setInterval(fetchStatus, 60_000)
    return () => clearInterval(interval)
  }, [fetchStatus])

  const handleToggle = useCallback(async () => {
    if (!claudeAuthAutoRefresh) {
      // Enabling — show consent dialog
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
      // Disabling
      try {
        await uninstallScheduler()
        setClaudeAuthAutoRefresh(false)
        fetchStatus()
      } catch (e) {
        console.error('[settings] Failed to uninstall Claude auth scheduler:', e)
      }
    }
  }, [claudeAuthAutoRefresh, confirm, installScheduler, uninstallScheduler, setClaudeAuthAutoRefresh, fetchStatus])

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
      valid: { color: '#22c55e', text: `Valid (${minutes}m)` },
      expiring: { color: '#eab308', text: 'Expiring soon' },
      expired: { color: '#ef4444', text: 'Expired' },
      missing: { color: '#6b7280', text: 'No credentials' },
      unknown: { color: '#6b7280', text: '' },
    }

    const { color, text } = config[authState]
    statusIndicator = (
      <div className="flex items-center gap-1.5 mr-3">
        {statusDot(color)}
        <span className="text-[10px] text-[var(--color-text-muted)] whitespace-nowrap">{text}</span>
        {(authState === 'expiring' || authState === 'expired') && (
          <button
            onClick={handleRefreshNow}
            disabled={refreshing}
            className="text-[10px] text-[var(--color-accent)] hover:underline cursor-pointer no-drag disabled:opacity-50"
          >
            {refreshing ? '...' : 'Refresh'}
          </button>
        )}
      </div>
    )
  }

  return (
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">Auto-refresh Claude credentials</span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          Background scheduler keeps your Claude session alive
        </p>
      </div>
      <div className="flex items-center flex-shrink-0">
        {statusIndicator}
        <button
          onClick={handleToggle}
          className="no-drag cursor-pointer flex-shrink-0 relative"
          style={{
            width: 36,
            height: 20,
            backgroundColor: claudeAuthAutoRefresh ? 'var(--color-accent)' : '#333',
            border: 'none',
            transition: 'background-color 150ms',
          }}
        >
          <span
            style={{
              position: 'absolute',
              top: 2,
              left: claudeAuthAutoRefresh ? 18 : 2,
              width: 16,
              height: 16,
              backgroundColor: '#fff',
              transition: 'left 150ms',
            }}
          />
        </button>
      </div>
    </div>
  )
}
