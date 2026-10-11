// Settings → Tickets (0.45.4, Rosson 2026-10-10). Two groups:
//
//   - Server rules (the daemon's, `settings.json` → `tickets`): require an
//     HTML brief, require an assignee, the default assignee. Everyone can
//     see them; only the owner or an admin can change them (the daemon
//     says so with `canEdit`). Thin client over /cli/tickets/settings.
//   - Just for you: which tab the Tickets page opens on. A per-person view
//     preference kept on this computer (`lib/ticket-open-pref.ts`).

import React, { useCallback, useEffect, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import { SettingDropdown, SettingsGroup } from '../controls/SettingControls'
import {
  RULE_OPTIONS,
  fetchTicketSettings,
  saveTicketSettings,
  type TicketRule,
  type TicketSettings,
  type TicketSettingsChange,
} from './tickets-settings-api'
import {
  TICKET_OPEN_ON_OPTIONS,
  readTicketOpenOn,
  writeTicketOpenOn,
  type TicketOpenOn,
} from '@/lib/ticket-open-pref'

export const TICKETS_MANIFEST: SettingEntry[] = [
  {
    id: 'tickets.require-brief',
    section: 'tickets',
    label: 'Require an HTML Brief',
    description: 'Whether agent tickets must carry an HTML brief (off, warn, require)',
    keywords: ['ticket', 'tickets', 'brief', 'html', 'feedback', 'agent', 'policy', 'require'],
    group: 'Server rules',
  },
  {
    id: 'tickets.require-assignee',
    section: 'tickets',
    label: 'Require an Assignee',
    description: 'Whether agent tickets must name a user on this server',
    keywords: ['ticket', 'tickets', 'assign', 'assignee', 'feedback', 'agent', 'policy', 'require'],
    group: 'Server rules',
  },
  {
    id: 'tickets.default-assignee',
    section: 'tickets',
    label: 'Default Assignee',
    description: 'Who gets an agent ticket filed without an assignee',
    keywords: ['ticket', 'tickets', 'assign', 'assignee', 'default', 'owner', 'user'],
    group: 'Server rules',
  },
  {
    id: 'tickets.open-on',
    section: 'tickets',
    label: 'Open the Tickets Page On',
    description: 'Waiting on me, All, or the tab you used last',
    keywords: ['ticket', 'tickets', 'waiting', 'all', 'tab', 'default', 'open'],
    group: 'Just for you',
  },
]

const RULE_HELP: Record<TicketRule, string> = {
  off: 'No check.',
  warn: 'The ticket is filed and the agent is told what is missing.',
  require: 'The ticket is refused until the agent fixes it.',
}

const NO_DEFAULT = '__none__'

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

function Row({
  settingId,
  title,
  hint,
  children,
}: {
  settingId: string
  title: string
  hint: React.ReactNode
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <div
      className="flex items-center justify-between gap-4 py-2 border-b border-[var(--color-border)]"
      data-settings-id={settingId}
    >
      <div className="min-w-0">
        <div className="text-xs text-[var(--color-text-primary)]">{title}</div>
        <div className="text-[10px] text-[var(--color-text-muted)]">{hint}</div>
      </div>
      <div className="flex-shrink-0">{children}</div>
    </div>
  )
}

export function TicketsSection(): React.JSX.Element {
  const [settings, setSettings] = useState<TicketSettings | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [saveError, setSaveError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [openOn, setOpenOn] = useState<TicketOpenOn>(() => readTicketOpenOn())

  useEffect(() => {
    let cancelled = false
    fetchTicketSettings().then(
      (s) => {
        if (!cancelled) setSettings(s)
      },
      (e: unknown) => {
        if (!cancelled) setLoadError(errorText(e))
      },
    )
    return () => {
      cancelled = true
    }
  }, [])

  const save = useCallback(async (change: TicketSettingsChange): Promise<void> => {
    setBusy(true)
    setSaveError(null)
    try {
      setSettings(await saveTicketSettings(change))
    } catch (e) {
      setSaveError(errorText(e))
    } finally {
      setBusy(false)
    }
  }, [])

  const canEdit = settings?.canEdit === true
  const locked = !canEdit || busy

  const assigneeOptions = settings
    ? [
        { value: NO_DEFAULT, label: 'Nobody' },
        ...settings.people.map((p) => ({
          value: p.value,
          label: p.label,
          badge: p.disabled ? 'Disabled' : undefined,
        })),
        // A stored default that is no longer a user here still shows.
        ...(settings.defaultAssignee && !settings.people.some((p) => p.value === settings.defaultAssignee)
          ? [{ value: settings.defaultAssignee, label: settings.defaultAssignee, badge: 'Not a user here' }]
          : []),
      ]
    : []

  return (
    <div className="max-w-xl" data-testid="tickets-settings">
      <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">Tickets</h2>
      <p className="text-xs text-[var(--color-text-muted)] mb-6">
        Rules for the tickets agents file with <code>k2 tickets ask</code>, and how your Tickets page opens.
      </p>

      <div className="space-y-6">
        <SettingsGroup title="Server rules">
          {loadError && (
            <p role="alert" data-testid="tickets-settings-error" className="text-[11px] text-[var(--color-status-error)] py-2">
              Could not load the ticket rules: {loadError}
            </p>
          )}
          {!settings && !loadError && (
            <p className="text-[11px] text-[var(--color-text-muted)] py-2">Loading…</p>
          )}
          {settings && (
            <>
              {!canEdit && (
                <p data-testid="tickets-settings-readonly" className="text-[10px] text-[var(--color-text-muted)] py-1">
                  Only the owner or an admin can change these.
                </p>
              )}
              <Row
                settingId="tickets.require-brief"
                title="Require an HTML brief"
                hint={`On agent tickets (fyi is exempt). ${RULE_HELP[settings.briefPolicy]}`}
              >
                <SettingDropdown
                  ariaLabel="Require an HTML brief"
                  value={settings.briefPolicy}
                  options={RULE_OPTIONS}
                  disabled={locked}
                  onChange={(v) => save({ briefPolicy: v as TicketRule })}
                />
              </Row>
              <Row
                settingId="tickets.require-assignee"
                title="Require an assignee"
                hint={`Agent tickets must name a user on this server (fyi is exempt). ${RULE_HELP[settings.assigneePolicy]}`}
              >
                <SettingDropdown
                  ariaLabel="Require an assignee"
                  value={settings.assigneePolicy}
                  options={RULE_OPTIONS}
                  disabled={locked}
                  onChange={(v) => save({ assigneePolicy: v as TicketRule })}
                />
              </Row>
              <Row
                settingId="tickets.default-assignee"
                title="Default assignee"
                hint={
                  settings.defaultAssignee && !settings.defaultAssigneeKnown
                    ? 'This user is no longer on this server, so new tickets stay unassigned. Pick someone else.'
                    : 'An agent ticket filed without an assignee goes to this person.'
                }
              >
                <SettingDropdown
                  ariaLabel="Default assignee"
                  value={settings.defaultAssignee ?? NO_DEFAULT}
                  options={assigneeOptions}
                  disabled={locked}
                  onChange={(v) => save({ defaultAssignee: v === NO_DEFAULT ? '' : v })}
                />
              </Row>
              {saveError && (
                <p role="alert" data-testid="tickets-settings-save-error" className="text-[11px] text-[var(--color-status-error)] mt-1">
                  {saveError}
                </p>
              )}
            </>
          )}
        </SettingsGroup>

        <SettingsGroup title="Just for you">
          <Row
            settingId="tickets.open-on"
            title="Open the Tickets page on"
            hint="Kept on this computer for your login on this server."
          >
            <SettingDropdown
              ariaLabel="Open the Tickets page on"
              value={openOn}
              options={TICKET_OPEN_ON_OPTIONS}
              onChange={(v) => {
                const next = v as TicketOpenOn
                writeTicketOpenOn(next)
                setOpenOn(next)
              }}
            />
          </Row>
        </SettingsGroup>
      </div>
    </div>
  )
}
