import { useCallback, useState } from 'react'
import { emit } from '@tauri-apps/api/event'
import { daemonCliPost } from '@/lib/daemon-cli'
import { useProjectsStore } from '@/stores/projects'
import { useRoom } from '@/components/Room/RoomContext'
import { Toggle } from '@/components/ui'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SaveError, SaveNote, SettingSwitchRow, useSaveStatus } from '@/components/Settings/controls/SettingControls'

/** Per-workspace hide-sessions: do not auto-surface API tabs. */
export function HideApiSessionsToggle({
  project,
}: {
  project: { id: string; path: string; hideApiSessions?: number }
}): React.JSX.Element {
  // Home M3 — the flag is written on the room's server, and the API tabs it
  // hides or re-hydrates are the room's.
  const room = useRoom()
  const [busy, setBusy] = useState(false)
  const save = useSaveStatus()
  const enabled = (project.hideApiSessions ?? 0) === 1

  const toggle = useCallback(async (): Promise<void> => {
    if (busy) return
    const next = !enabled
    setBusy(true)
    try {
      await daemonCliPost(room.scope, 'workspace/set', {
        project: project.path,
        fields: { hide_api_sessions: next ? '1' : '0' },
      })
      // The projects store is the PRIMARY room's list (MS4: never write
      // another server's record into it); `sync:projects` is this
      // computer's local broadcast.
      if (room.isPrimary) {
        useProjectsStore.setState((s) => ({
          projects: s.projects.map((p) =>
            p.id === project.id ? { ...p, hideApiSessions: next ? 1 : 0 } : p,
          ),
        }))
      }
      if (room.localCommands) void emit('sync:projects').catch(() => {})
      if (next) {
        room.tabs.room.minimizeApiSessionsForWorkspace(project.path)
      } else {
        void room.tabs.room.hydrateApiSandboxSessions()
      }
      save.saved()
    } catch (err) {
      console.error('[hide-api-sessions] write failed', err)
      save.failed(err)
    } finally {
      setBusy(false)
    }
  }, [room, busy, enabled, project.id, project.path, save])

  const hint = enabled
    ? 'API sessions stay off the tab strip. Open one from Chat history → API.'
    : 'API sessions open as tabs. Closing a tab hides it; the session keeps running.'
  return (
    <SettingSwitchRow
      control={
        <>
          <SaveNote state={save.state} />
          <Toggle
            checked={enabled}
            onChange={() => void toggle()}
            disabled={busy}
            aria-label="Hide API sessions"
            title={hint}
          />
        </>
      }
    >
      <div className={SETTING_ROW_LABEL}>Hide API sessions</div>
      <div className={SETTING_ROW_HINT}>{hint}</div>
      <SaveError state={save.state} />
    </SettingSwitchRow>
  )
}
