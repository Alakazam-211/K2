import React from 'react'
import { useSettingsStore } from '@/stores/settings'
// Phase 2 Unit 7a — settings live in the daemon.
import { settingsUpdate } from '@/lib/daemon-settings'
import { Toggle } from '@/components/ui'

export function AgenticSystemsToggle(): React.JSX.Element {
  const enabled = useSettingsStore((s) => s.agenticSystemsEnabled)

  const toggle = async () => {
    const newVal = !enabled
    useSettingsStore.setState({ agenticSystemsEnabled: newVal })
    await settingsUpdate({ agenticSystemsEnabled: newVal }).catch(console.error)
  }

  return (
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)] inline-flex items-center gap-2">
          Agentic Systems
          <span
            className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)]"
            title="Agentic systems are in beta — interface and behavior may change"
          >
            beta
          </span>
        </span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          {enabled
            ? 'AI agents, workspace manager, heartbeat, and review queue are active'
            : 'Enable to unlock AI agent orchestration across workspaces'}
        </p>
      </div>
      <Toggle checked={enabled} onChange={() => void toggle()} aria-label="Agentic Systems" />
    </div>
  )
}
