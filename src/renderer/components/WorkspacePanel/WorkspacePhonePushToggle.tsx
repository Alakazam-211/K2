// The caller's own Thread push for one workspace
// (prd-thread-push-notifications-v1 §9 shortcut, TP23), in two places:
//
//   - `WorkspacePhonePushToggle`: Workspaces → ws → Agent tab, "Phone
//     notifications (you)" (the window's server).
//   - `WorkspacePhonePushBell`: the Workspace drawer header, beside the
//     completion bell (0.46.0, Rosson 2026-10-16), on the room's server.
//
// Both edit only the caller's own row for this workspace (POST
// /cli/notify/prefs), never the workspace default, and read the daemon's
// one truth through `useNotifyPrefs` (refetch on `notify_prefs_changed`), so
// they and Settings → Notifications agree. Unrelated to the completion
// chime, which is a shared desktop sound. Hidden on a server without
// `thread-push-v1`.

import { useCallback, useState } from 'react'
import { Toggle } from '@/components/ui'
import { useSettingsStore } from '@/stores/settings'
import { useRoom } from '@/components/Room/RoomContext'
import { SmartphoneIcon, SmartphoneOffIcon } from '@/components/icons/K2Icons'
import {
  setMyWorkspacePush,
  sourceLabel,
  type NotifyPrefs,
  type NotifyWorkspacePref,
} from '@/components/Settings/sections/notify-api'
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

/** This workspace's effective switch. A workspace the daemon has not listed
 *  yet (just added) follows the §4 fallback, like the daemon would. */
function effectiveRow(prefs: NotifyPrefs, projectId: string): Pick<NotifyWorkspacePref, 'threadPush' | 'source'> {
  const row = prefs.workspaces.find((w) => w.projectId === projectId)
  return row ?? { threadPush: prefs.defaultOn, source: 'fallback' }
}

/** `prefs` with the caller's own row for `projectId` set to `threadPush`
 *  (the optimistic picture until the POST answers). */
function withOwnRow(prefs: NotifyPrefs, projectId: string, threadPush: boolean): NotifyPrefs {
  const mine = { threadPush, source: 'user' as const, updatedBy: prefs.principal || null, updatedAt: null }
  const has = prefs.workspaces.some((w) => w.projectId === projectId)
  return {
    ...prefs,
    workspaces: has
      ? prefs.workspaces.map((w) => (w.projectId === projectId ? { ...w, ...mine } : w))
      : [...prefs.workspaces, { projectId, name: projectId, handle: '', ...mine }],
  }
}

/** The drawer toggle's tooltip, one fact per line. */
export function phonePushTitle(
  prefs: Pick<NotifyPrefs, 'server' | 'push'>,
  row: Pick<NotifyWorkspacePref, 'threadPush' | 'source'>,
  readOnly: boolean,
): string {
  const lines: string[] = []
  if (!prefs.server.enabled) lines.push('Your phone pushes are off for this server (Settings → Notifications)')
  const inherited = row.source === 'default' ? ' (workspace default)' : row.source === 'fallback' ? ' (default)' : ''
  lines.push(`Thread pushes to your phone: ${row.threadPush ? 'on' : 'off'}${inherited}`)
  if (prefs.push === 'dormant') lines.push("This server isn't connected to the push service yet")
  if (prefs.push === 'airgap') lines.push('Air-gapped: no pushes')
  if (readOnly) lines.push('View only: you cannot change it from this room')
  return lines.join('\n')
}

/** Drawer header phone: accent = my Thread pushes on for this workspace,
 *  muted grey + slash = off; dimmed while my server bell is off. A click
 *  flips my own row (optimistic, rolled back on error). */
export function WorkspacePhonePushBell({ project }: { project: { id: string } }): React.JSX.Element | null {
  // Home M4: the drawer's room's server, like the completion bell.
  const room = useRoom()
  const scope = room.scope
  const available = scope.serverSupports('thread-push-v1')
  const { prefs, adopt } = useNotifyPrefs(available, scope)
  const [busy, setBusy] = useState(false)

  const toggle = useCallback(async (): Promise<void> => {
    if (busy || room.readOnly || !prefs) return
    const before = prefs
    const next = !effectiveRow(prefs, project.id).threadPush
    adopt(withOwnRow(prefs, project.id, next))
    setBusy(true)
    try {
      adopt(await setMyWorkspacePush(project.id, next, scope))
    } catch (err) {
      console.error('[thread-push] drawer toggle write failed', err)
      adopt(before)
    } finally {
      setBusy(false)
    }
  }, [busy, room.readOnly, prefs, project.id, scope, adopt])

  // An older server (no notify routes), or prefs not read: nothing to show.
  if (!available || !prefs) return null
  const row = effectiveRow(prefs, project.id)
  const on = row.threadPush
  const bellOff = !prefs.server.enabled

  return (
    <button
      type="button"
      onClick={() => void toggle()}
      role="switch"
      aria-checked={on}
      aria-readonly={room.readOnly || undefined}
      disabled={busy}
      data-testid="workspace-phone-push-bell"
      data-server-bell={bellOff ? 'off' : 'on'}
      className={`flex-shrink-0 p-0.5 no-drag disabled:opacity-50 ${
        room.readOnly ? 'cursor-default' : 'cursor-pointer'
      } ${on ? 'text-[var(--color-accent)]' : 'text-[var(--color-text-muted)]'} ${bellOff ? 'opacity-40' : ''}`}
      title={phonePushTitle(prefs, row, room.readOnly)}
      aria-label={
        on
          ? 'Turn off Thread pushes to your phone for this workspace'
          : 'Turn on Thread pushes to your phone for this workspace'
      }
    >
      {on ? <SmartphoneIcon /> : <SmartphoneOffIcon />}
    </button>
  )
}
