// Settings → Workspaces → <agent> → Mail: which inboxes this agent can read,
// which of those wake it, and which senders are VIPs. Nothing wakes an agent
// until someone chooses here (or in an inbox's own panel; same record).
// Everything here is the daemon's truth: changes go through, then the answer
// redraws.

import React, { useCallback, useEffect, useState } from 'react'
import { Button, Input, Toggle } from '@/components/ui'
import { SettingDropdown, SettingRow, SettingsGroup } from '../controls/SettingControls'
import { mailErrorMessage, onWindowServerMailChanged } from './email-api'
import {
  addWatchVip,
  fetchMailWatch,
  pauseMailWatch,
  removeWatchVip,
  setMailWatch,
  setWatchInbox,
  wakeFromValue,
  wakeValue,
  WAKE_OPTIONS,
  WEEKDAYS,
  type MailWatchStatus,
  type WatchSetBody,
} from './mail-watch-api'

const muted = 'text-[10px] text-[var(--color-text-muted)]'
const WEEK = ['mon', 'tue', 'wed', 'thu', 'fri']

function Row({
  label,
  description,
  children,
}: {
  label: string
  description?: string
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <SettingRow
      label={
        <span>
          {label}
          {description && <span className={`block ${muted}`}>{description}</span>}
        </span>
      }
    >
      {children}
    </SettingRow>
  )
}

export function MailWatchPanel({ projectPath }: { projectPath: string }): React.JSX.Element {
  const [status, setStatus] = useState<MailWatchStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [newVip, setNewVip] = useState('')

  const load = useCallback(async () => {
    try {
      setStatus(await fetchMailWatch(projectPath))
      setError(null)
    } catch (e) {
      setError(mailErrorMessage(e))
    }
  }, [projectPath])

  useEffect(() => {
    void load()
    return onWindowServerMailChanged(() => void load())
  }, [load])

  const apply = async (fn: () => Promise<MailWatchStatus>): Promise<void> => {
    try {
      setStatus(await fn())
      setError(null)
    } catch (e) {
      setError(mailErrorMessage(e))
    }
  }

  const update = (body: WatchSetBody): Promise<void> => apply(() => setMailWatch(projectPath, body))

  if (!status) {
    return <div className={muted}>{error ?? 'Loading…'}</div>
  }

  const window = status.digest
  const days = window?.days ?? []
  const windowOn = Boolean(window?.start)
  const setDays = (next: string[]): Promise<void> =>
    update({ window: { start: window?.start ?? '09:00', end: window?.end ?? '17:00', days: next } })

  return (
    <div className="space-y-4">
      {error && <div className={`${muted} !text-[var(--color-status-warn)]`}>{error}</div>}

      <SettingsGroup title="Mail">
        <Row
          label="Tell this agent about mail"
          description="Nothing wakes this agent until you pick an inbox below. An inbox you don't pick isn't read for this at all."
        >
          <Toggle checked={status.enabled} onChange={(v) => void update({ on: v })} aria-label="Tell this agent about mail" />
        </Row>
      </SettingsGroup>

      <SettingsGroup title="Inboxes this agent can read">
        {status.inboxes.length === 0 && <div className={muted}>This agent can't read any inboxes yet.</div>}
        {status.inboxes.map((inbox) => (
          <Row
            key={inbox.address}
            label={inbox.address}
            description={
              inbox.needsRelink
                ? 'Its login was refused. Reconnect the inbox and notices start again.'
                : inbox.personMailbox
                  ? "A person's mailbox: only a person can choose this."
                  : inbox.primary
                    ? "This agent's own inbox"
                    : 'Shared with this agent'
            }
          >
            <SettingDropdown
              value={wakeValue(inbox.wake)}
              options={WAKE_OPTIONS}
              ariaLabel={`How ${inbox.address} wakes this agent`}
              onChange={(v) => apply(() => setWatchInbox(projectPath, inbox.address, wakeFromValue(v)))}
            />
          </Row>
        ))}
      </SettingsGroup>

      <SettingsGroup title="VIP senders">
        <div className={muted}>
          An address or a @domain. A VIP is told at once, only if mail authentication passes. Being a VIP changes when the
          agent hears, never whether it obeys.
        </div>
        <div className="flex flex-wrap items-center gap-1.5 pt-1">
          {status.vips.map((v) => (
            <Button key={v} size="sm" onClick={() => void apply(() => removeWatchVip(projectPath, v))} aria-label={`Remove ${v}`} title="Remove">
              {v} ×
            </Button>
          ))}
          {status.vips.length === 0 && <span className={muted}>No VIPs yet.</span>}
        </div>
        <form
          className="flex items-center gap-2 pt-1"
          onSubmit={(e) => {
            e.preventDefault()
            const entry = newVip.trim()
            if (entry) {
              setNewVip('')
              void apply(() => addWatchVip(projectPath, entry))
            }
          }}
        >
          <Input value={newVip} onChange={(e) => setNewVip(e.target.value)} placeholder="jane@acme.com or @acme.com" aria-label="Add a VIP sender" />
          <Button size="sm" type="submit" disabled={!newVip.trim()}>
            Add
          </Button>
        </form>
      </SettingsGroup>

      <SettingsGroup title="Check window and digest">
        <Row label="Check window" description="Digests only go out inside this window.">
          <Toggle
            checked={windowOn}
            onChange={(v) => void update({ window: v ? { start: '09:00', end: '17:00', days: WEEK } : { off: true } })}
            aria-label="Check window"
          />
        </Row>
        {windowOn && (
          <>
            <Row label="Hours">
              <div className="flex items-center gap-2">
                <Input
                  className="w-20"
                  defaultValue={window?.start ?? ''}
                  aria-label="Window start"
                  onBlur={(e) => void update({ window: { start: e.target.value, end: window?.end ?? '17:00', days } })}
                />
                <span className={muted}>to</span>
                <Input
                  className="w-20"
                  defaultValue={window?.end ?? ''}
                  aria-label="Window end"
                  onBlur={(e) => void update({ window: { start: window?.start ?? '09:00', end: e.target.value, days } })}
                />
              </div>
            </Row>
            <Row label="Days" description={window?.tz ? `Times in ${window.tz}` : "Times on this server's clock"}>
              <div className="flex flex-wrap items-center gap-1">
                {WEEKDAYS.map((d) => {
                  const on = days.includes(d.value)
                  return (
                    <Button
                      key={d.value}
                      size="sm"
                      variant={on ? 'primary' : undefined}
                      aria-pressed={on}
                      onClick={() => void setDays(on ? days.filter((x) => x !== d.value) : [...days, d.value])}
                    >
                      {d.label}
                    </Button>
                  )
                })}
              </div>
            </Row>
          </>
        )}
        <Row label="Digest every">
          <SettingDropdown
            value={String(window?.everySeconds ?? 3600)}
            options={[
              { value: '900', label: '15 minutes' },
              { value: '1800', label: '30 minutes' },
              { value: '3600', label: '1 hour' },
              { value: '7200', label: '2 hours' },
              { value: '14400', label: '4 hours' },
            ]}
            ariaLabel="Digest interval"
            onChange={(v) => update({ digestEverySecs: Number(v) })}
          />
        </Row>
        <Row label="VIP mail outside the window">
          <SettingDropdown
            value={status.vipOutsideWindow ? 'deliver' : 'hold'}
            options={[
              { value: 'deliver', label: 'Deliver now' },
              { value: 'hold', label: 'Hold for the next digest' },
            ]}
            ariaLabel="VIP mail outside the window"
            onChange={(v) => update({ vipOutsideWindow: v === 'deliver' })}
          />
        </Row>
      </SettingsGroup>

      <SettingsGroup title="More">
        <Row label="Also write VIP mail in the agent's session" description="Off: a card on the Thread only.">
          <Toggle checked={status.vipSessionLine} onChange={(v) => void update({ vipSessionLine: v })} aria-label="Write VIP mail in the session" />
        </Row>
        <Row label="Include a short quote" description="The first 160 characters on the Thread card.">
          <Toggle checked={status.snippets} onChange={(v) => void update({ snippets: v })} aria-label="Include a short quote" />
        </Row>
        <Row label="Pause" description={status.pausedUntil ? `Paused until ${status.pausedUntil}` : undefined}>
          {status.pausedUntil ? (
            <Button size="sm" onClick={() => void pauseMailWatch(projectPath, true).then(load)}>
              Resume
            </Button>
          ) : (
            <Button size="sm" onClick={() => void pauseMailWatch(projectPath, false, '1h').then(load)}>
              For 1 hour
            </Button>
          )}
        </Row>
      </SettingsGroup>

      <div className={muted}>
        Last 24 hours: {status.last24h.vipDelivered} VIP delivered · {status.last24h.digested} in digests ·{' '}
        {status.last24h.failedAuth} failed authentication
      </div>
    </div>
  )
}
