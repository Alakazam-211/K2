// One-time reminders: heartbeats that fire once and go away. The list comes
// from the daemon and refetches on its `heartbeat_roster_changed` push; the
// times shown are the daemon's, drawn in this machine's clock.

import React, { useCallback, useEffect, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { Button, Input } from '@/components/ui'
import { SettingDropdown } from '@/components/Settings/controls/SettingControls'
import { subscribeToWorkspaceTabEvents } from '@/stores/session-events'
import { daemonErrorText } from '@/lib/daemon-error'

export interface ReminderRow {
  name: string
  text: string | null
  fireAt: string | null
  tz: string | null
  state: string
  firedAt: string | null
}

const muted = 'text-[10px] text-[var(--color-text-muted)]'

export function fetchReminders(scope: ServerScope, project: string, all: boolean): Promise<ReminderRow[]> {
  return daemonCliGet<ReminderRow[]>(scope, 'heartbeat/reminder/list', all ? { project, all: 1 } : { project })
}

/** The daemon's time, in this machine's words. */
export function reminderWhen(iso: string | null): string {
  if (!iso) return ''
  const t = new Date(iso)
  return Number.isNaN(t.getTime()) ? iso : t.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
}

export function RemindersList({
  scope,
  projectPath,
  manage,
  refreshKey,
}: {
  scope: ServerScope
  projectPath: string
  /** Settings shows the finished ones and the Add form; the drawer doesn't. */
  manage: boolean
  /** The drawer's own heartbeat store refetches on the daemon's
   *  `heartbeat_roster_changed`, so it passes that here instead of this list
   *  opening a second subscription. Settings passes nothing and subscribes. */
  refreshKey?: unknown
}): React.JSX.Element | null {
  const [rows, setRows] = useState<ReminderRow[]>([])
  const [error, setError] = useState<string | null>(null)
  const [text, setText] = useState('')
  const [mode, setMode] = useState('in')
  const [when, setWhen] = useState('45m')
  const [zoneLine, setZoneLine] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      setRows(await fetchReminders(scope, projectPath, manage))
      setError(null)
    } catch (e) {
      setError(daemonErrorText(e))
    }
  }, [scope, projectPath, manage])

  useEffect(() => {
    void load()
    if (manage) {
      return subscribeToWorkspaceTabEvents(scope, projectPath, { onHeartbeatRosterChanged: () => void load() })
    }
    return undefined
  }, [load, scope, projectPath, manage, refreshKey])

  const pending = rows.filter((r) => r.state === 'pending')
  if (!manage && pending.length === 0) return null

  const add = async (): Promise<void> => {
    try {
      const body: Record<string, string> = { project: projectPath, text }
      body[mode] = when
      const res = await daemonCliPost<{ fires: string }>(scope, 'heartbeat/reminder/add', body)
      setZoneLine(`Fires ${res.fires}`)
      setText('')
      setError(null)
      await load()
    } catch (e) {
      setError(daemonErrorText(e))
    }
  }

  const cancel = async (name: string): Promise<void> => {
    try {
      await daemonCliPost(scope, 'heartbeat/reminder/cancel', { project: projectPath, name })
      await load()
    } catch (e) {
      setError(daemonErrorText(e))
    }
  }

  return (
    <div data-testid="reminders-list" className="space-y-1">
      <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)]">Reminders</div>
      {error && <div className={`${muted} !text-[var(--color-status-warn)]`}>{error}</div>}
      {rows.map((r) => (
        <div key={r.name} className="flex items-center justify-between gap-2 text-[11px]">
          <span className="min-w-0 truncate text-[var(--color-text-primary)]" title={r.text ?? ''}>
            {r.text}
          </span>
          <span className={`flex-shrink-0 ${muted}`}>
            {r.state === 'pending' ? reminderWhen(r.fireAt) : `${r.state}${r.firedAt ? ` ${reminderWhen(r.firedAt)}` : ''}`}
          </span>
          {manage && r.state === 'pending' && (
            <Button size="sm" onClick={() => void cancel(r.name)}>
              Cancel
            </Button>
          )}
        </div>
      ))}
      {manage && rows.length === 0 && <div className={muted}>No reminders. A reminder fires once and goes away.</div>}
      {manage && (
        <form
          className="flex items-center gap-2 pt-1"
          onSubmit={(e) => {
            e.preventDefault()
            if (text.trim() && when.trim()) void add()
          }}
        >
          <Input value={text} onChange={(e) => setText(e.target.value)} placeholder="Remind the agent to…" aria-label="Reminder text" />
          <SettingDropdown
            value={mode}
            options={[
              { value: 'in', label: 'In' },
              { value: 'at', label: 'At' },
              { value: 'on', label: 'On' },
            ]}
            ariaLabel="When"
            onChange={(v) => {
              setMode(v)
              setWhen(v === 'in' ? '45m' : v === 'at' ? '13:00' : '')
            }}
          />
          <Input
            className="w-28"
            value={when}
            onChange={(e) => setWhen(e.target.value)}
            placeholder={mode === 'on' ? '2026-10-30' : ''}
            aria-label="Time"
          />
          <Button size="sm" type="submit" disabled={!text.trim() || !when.trim()}>
            Add
          </Button>
        </form>
      )}
      {manage && zoneLine && <div className={muted}>{zoneLine}</div>}
    </div>
  )
}
