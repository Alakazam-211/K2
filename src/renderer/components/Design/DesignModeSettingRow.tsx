// Workspace settings → "Design mode" switch (late decision Q-F2), next to
// the completion sound toggle. The caller passes the server it writes to
// (MS79: no `primaryScope()` here).

import { useCallback, useState } from 'react'
import { emit } from '@tauri-apps/api/event'
import { useProjectsStore } from '@/stores/projects'
import type { ServerScope } from '@/kessel/server-scope'
import { isDesignModeEnabled, setDesignMode } from './design-mode-setting'

export type DesignModeProject = { id: string; path: string; designModeEnabled?: number }

export async function writeDesignMode(scope: ServerScope, project: DesignModeProject, next: boolean): Promise<void> {
  await setDesignMode(scope, project.path, next)
  // The window's projects store mirrors only the window's server.
  if (scope.isPrimary) {
    useProjectsStore.setState((s) => ({
      projects: s.projects.map((p) => (p.id === project.id ? { ...p, designModeEnabled: next ? 1 : 0 } : p)),
    }))
  }
  if (scope.isPrimary || !scope.isRemote) void emit('sync:projects').catch(() => {})
}

export function DesignModeSettingRow({
  project,
  scope,
}: {
  project: DesignModeProject
  /** The server the workspace lives on. */
  scope: ServerScope
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const enabled = isDesignModeEnabled(project)

  const toggle = useCallback(async (): Promise<void> => {
    if (busy) return
    setBusy(true)
    setError(null)
    try {
      await writeDesignMode(scope, project, !enabled)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }, [busy, enabled, project, scope])

  return (
    <div className="border border-[var(--color-border)] p-3">
      <div className="flex items-start gap-3">
        <button
          type="button"
          onClick={() => void toggle()}
          role="switch"
          aria-checked={enabled}
          aria-label="Design mode"
          disabled={busy}
          data-settings-id="projects.design-mode"
          className={`mt-0.5 w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 ${
            enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
          }`}
        >
          <span
            className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
              enabled ? 'translate-x-3.5' : 'translate-x-0.5'
            }`}
          />
        </button>
        <div className="flex-1 min-w-0">
          <div className="text-xs font-medium text-[var(--color-text-primary)]">Design mode</div>
          <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5 leading-relaxed">
            Shows the pinned Design tab and Design in the + menu for this workspace.
          </div>
          {error ? (
            <div role="alert" className="text-[10px] text-[var(--color-danger,#c33)] mt-1">
              {error}
            </div>
          ) : null}
        </div>
      </div>
    </div>
  )
}
