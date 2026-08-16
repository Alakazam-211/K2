import { useCallback, useState } from 'react'
import { emit } from '@tauri-apps/api/event'
import { daemonCliPost } from '@/lib/daemon-cli'
import { useProjectsStore } from '@/stores/projects'
import {
  hydrateApiSandboxSessions,
  minimizeApiSessionsForWorkspace,
} from '@/stores/tabs'

/** Per-workspace hide-sessions: do not auto-surface API tabs. */
export function HideApiSessionsToggle({
  project,
}: {
  project: { id: string; path: string; hideApiSessions?: number }
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const enabled = (project.hideApiSessions ?? 0) === 1

  const toggle = useCallback(async (): Promise<void> => {
    if (busy) return
    const next = !enabled
    setBusy(true)
    try {
      await daemonCliPost('workspace/set', {
        project: project.path,
        fields: { hide_api_sessions: next ? '1' : '0' },
      })
      useProjectsStore.setState((s) => ({
        projects: s.projects.map((p) =>
          p.id === project.id ? { ...p, hideApiSessions: next ? 1 : 0 } : p,
        ),
      }))
      void emit('sync:projects').catch(() => {})
      if (next) {
        minimizeApiSessionsForWorkspace(project.path)
      } else {
        void hydrateApiSandboxSessions()
      }
    } catch (err) {
      console.error('[hide-api-sessions] write failed', err)
    } finally {
      setBusy(false)
    }
  }, [busy, enabled, project.id, project.path])

  return (
    <div className="flex items-start gap-3">
      <button
        type="button"
        onClick={() => void toggle()}
        role="switch"
        aria-checked={enabled}
        disabled={busy}
        className={`mt-0.5 w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 ${
          enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
        }`}
        title={
          enabled
            ? 'API sessions stay off the tab strip. Open them from Chat history → API.'
            : 'API sessions appear as tabs. Closing a tab minimizes it (does not kill the session).'
        }
      >
        <span
          className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
            enabled ? 'translate-x-3.5' : 'translate-x-0.5'
          }`}
        />
      </button>
      <div className="text-[11px] font-medium text-[var(--color-text-primary)]">Hide sessions</div>
    </div>
  )
}
