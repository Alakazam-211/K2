// Workspaces → ws → Agent tab: "Phone notifications (you)"
// (prd-thread-push-notifications-v1 §9 shortcut, TP23).
//
// Edits only the caller's own row for this workspace (POST
// /cli/notify/prefs), never the workspace default. Unrelated to the
// completion chime above it, which is a shared desktop sound. Hidden on a
// server without `thread-push-v1`.

import { useCallback, useState } from 'react'
import { Toggle } from '@/components/ui'
import { useSettingsStore } from '@/stores/settings'
import { setMyWorkspacePush, sourceLabel } from '@/components/Settings/sections/notify-api'
import { threadPushAvailable, useNotifyPrefs } from '@/components/Settings/sections/useNotifyPrefs'

export function WorkspacePhonePushToggle({ project }: { project: { id: string; name?: string } }): React.JSX.Element | null {
  const available = threadPushAvailable()
  const { prefs, error, adopt } = useNotifyPrefs(available)
  const [busy, setBusy] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)

  const write = useCallback(
    async (next: boolean | null): Promise<void> => {
      setBusy(true)
      setSaveError(null)
      try {
        adopt(await setMyWorkspacePush(project.id, next))
      } catch (e) {
        setSaveError(e instanceof Error ? e.message : String(e))
      } finally {
        setBusy(false)
      }
    },
    [project.id, adopt],
  )

  if (!available) return null
  const row = prefs?.workspaces.find((w) => w.projectId === project.id) ?? null

  return (
    <div className="border border-[var(--color-border)] p-3" data-testid="workspace-phone-push">
      <div className="flex items-start gap-3">
        <div className="flex-1 min-w-0">
          <div className="text-xs font-medium text-[var(--color-text-primary)]">Phone notifications (you)</div>
          <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5 leading-relaxed">
            Push your phone when this agent posts in a Thread. Only your own setting; others keep theirs.
          </div>
          {row && (
            <div data-testid="workspace-phone-push-source" className="text-[10px] text-[var(--color-text-secondary)] mt-1">
              {sourceLabel(row, prefs?.principal ?? '')}
            </div>
          )}
          {prefs && !prefs.server.enabled && (
            <div className="text-[10px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
              Your pushes from this server are off in{' '}
              <button
                type="button"
                className="underline cursor-pointer no-drag"
                onClick={() => useSettingsStore.getState().setSection('notifications')}
              >
                Settings → Notifications
              </button>
              .
            </div>
          )}
          {error && (
            <div role="alert" className="text-[10px] text-[var(--color-status-error)] mt-1">
              Could not load your notifications: {error}
            </div>
          )}
          {saveError && (
            <div role="alert" className="text-[10px] text-[var(--color-status-error)] mt-1">
              {saveError}
            </div>
          )}
        </div>
        <div className="flex items-center gap-2 flex-shrink-0">
          {row?.source === 'user' && (
            <button
              type="button"
              onClick={() => void write(null)}
              disabled={busy}
              aria-label="Reset phone notifications to the workspace default"
              className="text-[10px] px-1.5 py-0.5 border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer disabled:opacity-50"
            >
              Reset
            </button>
          )}
          <Toggle
            aria-label="Phone notifications from this workspace"
            checked={row?.threadPush ?? false}
            disabled={busy || !row}
            onChange={(next) => void write(next)}
          />
        </div>
      </div>
    </div>
  )
}
