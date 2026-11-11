// Settings → Tickets (0.45.4, Rosson 2026-10-10). Two card groups:
//
//   - For everyone on this server (the daemon's, `settings.json` →
//     `tickets`): require an HTML brief, require an assignee, the default
//     assignee. Everyone can see them; only the owner or an admin can change
//     them (the daemon says so with `canEdit`). Thin client over
//     /cli/tickets/settings. A person who can't change them sees the values
//     as plain text, with a line saying who can.
//   - Just for you: which tab the Tickets page opens on. A per-person view
//     preference kept on this computer (`lib/ticket-open-pref.ts`).
//
// The row labels "Require an HTML brief" and "Require an assignee" are the
// names the daemon's warnings to agents point at (`feedback_brief.rs`), so
// they stay as they are.

import React, { useCallback, useEffect, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import {
  SETTING_ROW_HINT,
  SaveNote,
  SettingDropdown,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
  useSaveStatus,
  type SaveState,
} from '../controls/SettingControls'
import { Button } from '@/components/ui/Button'
import { Tag } from '@/components/ui/Tag'
import { httpStatusOf } from '@/lib/daemon-http-error'
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
    description: 'Whether an agent must attach a short page saying what it needs (off, warn, require)',
    keywords: ['ticket', 'tickets', 'brief', 'html', 'feedback', 'agent', 'policy', 'require'],
    group: 'For everyone on this server',
  },
  {
    id: 'tickets.require-assignee',
    section: 'tickets',
    label: 'Require an Assignee',
    description: 'Whether an agent must name the person on this server who should answer',
    keywords: ['ticket', 'tickets', 'assign', 'assignee', 'feedback', 'agent', 'policy', 'require'],
    group: 'For everyone on this server',
  },
  {
    id: 'tickets.default-assignee',
    section: 'tickets',
    label: 'Default Assignee',
    description: 'Who gets an agent’s ticket that names nobody',
    keywords: ['ticket', 'tickets', 'assign', 'assignee', 'default', 'owner', 'user'],
    group: 'For everyone on this server',
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

/** What each rule does right now, in words, after the row's own line. */
const BRIEF_NOW: Record<TicketRule, string> = {
  off: 'Off: agents may leave it out.',
  warn: 'Warn: a ticket without one is still filed, and the agent is asked to add one.',
  require: 'Require: a ticket without one is turned back until the agent adds it.',
}

const ASSIGNEE_NOW: Record<TicketRule, string> = {
  off: 'Off: agents may leave it blank.',
  warn: 'Warn: a ticket without one is still filed, and the agent is asked to name someone.',
  require: 'Require: a ticket without one is turned back until the agent names someone.',
}

const NO_DEFAULT = '__none__'

const READ_ONLY_LINE = 'Only the server’s owner or an admin can change these.'

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

/** A value this person can see but not change: plain text on the control's
 *  edge, so it never reads as a broken dropdown. */
function ReadOnlyValue({ label, testId }: { label: string; testId: string }): React.JSX.Element {
  return (
    <span data-readonly-value="" data-testid={testId} className="block text-right type-row text-[var(--color-text-primary)] truncate">
      {label}
    </span>
  )
}

/** Why a change didn't save, under the row's hint: what failed in plain
 *  words and that nothing changed, then the server's own reason quietly. */
function RowSaveError({ state, status }: { state: SaveState; status: number | null }): React.JSX.Element | null {
  if (typeof state !== 'object') return null
  const reason = state.error.trim().replace(/[.\s]+$/, '')
  const lead =
    status === 403 || status === 401
      ? 'Couldn’t save: this login can’t change the server’s ticket rules. Nothing changed; ask the owner or an admin.'
      : status === null
        ? 'Couldn’t save: K2 couldn’t reach this server. Nothing changed; try again.'
        : 'Couldn’t save this change. Nothing changed; try again.'
  return (
    <div role="alert" data-save-error="" data-testid="tickets-settings-save-error" className="mt-1">
      <p className="type-hint text-[var(--color-status-error-soft)] leading-relaxed max-w-[75ch]">{lead}</p>
      {reason && <p className={SETTING_ROW_HINT}>The server said: {reason}.</p>}
    </div>
  )
}

/** One server rule: a dropdown for people who may change it, plain text for
 *  everyone else, "Saved" by the control after a write, and the reason under
 *  the hint when a write fails. */
function ServerRuleRow({
  settingId,
  label,
  hint,
  value,
  options,
  canEdit,
  busy,
  onSave,
}: {
  settingId: string
  label: string
  hint: React.ReactNode
  value: string
  options: { value: string; label: string }[]
  canEdit: boolean
  busy: boolean
  onSave: (value: string) => Promise<void>
}): React.JSX.Element {
  const save = useSaveStatus()
  const [status, setStatus] = useState<number | null>(null)
  const shown = options.find((o) => o.value === value)?.label ?? value
  return (
    <SettingRow
      settingId={settingId}
      label={<span className="inline-flex items-center gap-2">{label}<SaveNote state={save.state} /></span>}
      hint={
        <>
          {hint}
          <RowSaveError state={save.state} status={status} />
        </>
      }
      field
    >
      {canEdit ? (
        <SettingDropdown
          ariaLabel={label}
          value={value}
          options={options}
          disabled={busy}
          fullWidth
          onChange={async (v) => {
            if (v === value) return
            save.reset()
            try {
              await onSave(v)
              save.saved()
            } catch (e) {
              setStatus(httpStatusOf(e))
              save.failed(e)
            }
          }}
        />
      ) : (
        <ReadOnlyValue label={shown} testId={`${settingId}-value`} />
      )}
    </SettingRow>
  )
}

type Load = 'loading' | 'ready' | { error: string }

export function TicketsSection(): React.JSX.Element {
  const [settings, setSettings] = useState<TicketSettings | null>(null)
  const [load, setLoad] = useState<Load>('loading')
  const [busy, setBusy] = useState(false)
  const [openOn, setOpenOn] = useState<TicketOpenOn>(() => readTicketOpenOn())
  const openOnSave = useSaveStatus()

  const fetchRules = useCallback((isCancelled: () => boolean = () => false): void => {
    setLoad('loading')
    fetchTicketSettings().then(
      (s) => {
        if (isCancelled()) return
        setSettings(s)
        setLoad('ready')
      },
      (e: unknown) => {
        if (!isCancelled()) setLoad({ error: errorText(e) })
      },
    )
  }, [])

  useEffect(() => {
    let cancelled = false
    fetchRules(() => cancelled)
    return () => {
      cancelled = true
    }
  }, [fetchRules])

  /** Throws on failure; the row says what happened. */
  const save = useCallback(async (change: TicketSettingsChange): Promise<void> => {
    setBusy(true)
    try {
      setSettings(await saveTicketSettings(change))
    } finally {
      setBusy(false)
    }
  }, [])

  const canEdit = settings?.canEdit === true
  const ruleOptions = RULE_OPTIONS.map((o) => ({ value: o.value, label: o.label }))

  const assigneeOptions = settings
    ? [
        { value: NO_DEFAULT, label: 'Nobody' },
        ...settings.people.map((p) => ({
          value: p.value,
          label: p.disabled ? `${p.label} (account turned off)` : p.label,
        })),
        // A stored default that is no longer a user here still shows.
        ...(settings.defaultAssignee && !settings.people.some((p) => p.value === settings.defaultAssignee)
          ? [{ value: settings.defaultAssignee, label: `${settings.defaultAssignee} (no longer here)` }]
          : []),
      ]
    : []

  const defaultGone = !!settings?.defaultAssignee && settings.defaultAssigneeKnown === false
  const hasDefault = !!settings?.defaultAssignee && !defaultGone

  const rulesState =
    load === 'loading' ? (
      <SettingsLoadState kind="loading" title="Loading the ticket rules…" testId="tickets-settings-loading">
        Asking this server for its rules.
      </SettingsLoadState>
    ) : typeof load === 'object' ? (
      <SettingsLoadState
        kind="error"
        title="Couldn’t load the ticket rules"
        detail={load.error}
        testId="tickets-settings-error"
        action={<Button onClick={() => fetchRules()}>Try again</Button>}
      >
        K2 couldn’t read them from this server. Your own setting below still works.
      </SettingsLoadState>
    ) : undefined

  return (
    <div className="w-full max-w-[52rem] space-y-6 pb-6" data-testid="tickets-settings">
      <header className="min-w-0">
        <h2 className="type-title text-[var(--color-text-primary)]">Tickets</h2>
        <p className="type-hint text-[var(--color-text-secondary)] mt-1 max-w-[75ch]">
          Tickets are the questions and approvals agents send to people on this server. Set what agents must include, who
          gets a ticket that names nobody, and how your Tickets page opens.
        </p>
      </header>

      <SettingsGroup
        card
        title="For everyone on this server"
        testId="tickets-server-rules"
        badge={settings && !canEdit ? <Tag tone="muted">View only</Tag> : undefined}
        hint={
          <>
            These apply to every agent&rsquo;s questions and approvals. Notes marked FYI never need a brief or an
            assignee.
            {settings && !canEdit && (
              <span data-testid="tickets-settings-readonly" className="block mt-1 text-[var(--color-text-primary)]">
                {READ_ONLY_LINE}
              </span>
            )}
          </>
        }
        state={rulesState}
      >
        {settings && (
          <>
            <ServerRuleRow
              settingId="tickets.require-brief"
              label="Require an HTML brief"
              hint={
                <>
                  A short page from the agent: what it needs from you and the choices it offers.{' '}
                  {BRIEF_NOW[settings.briefPolicy]}
                </>
              }
              value={settings.briefPolicy}
              options={ruleOptions}
              canEdit={canEdit}
              busy={busy}
              onSave={(v) => save({ briefPolicy: v as TicketRule })}
            />
            <ServerRuleRow
              settingId="tickets.require-assignee"
              label="Require an assignee"
              hint={
                <>
                  Who on this server should answer. {ASSIGNEE_NOW[settings.assigneePolicy]}
                  {hasDefault && settings.assigneePolicy !== 'off' && ' A ticket that names nobody goes to the default assignee instead.'}
                </>
              }
              value={settings.assigneePolicy}
              options={ruleOptions}
              canEdit={canEdit}
              busy={busy}
              onSave={(v) => save({ assigneePolicy: v as TicketRule })}
            />
            <ServerRuleRow
              settingId="tickets.default-assignee"
              label="Default assignee"
              hint={
                defaultGone ? (
                  <span className="text-[var(--color-status-warn-text)]">
                    {settings.defaultAssignee} is no longer on this server, so tickets that name nobody stay unassigned.{' '}
                    {canEdit ? 'Pick someone else.' : 'Ask the owner or an admin to pick someone else.'}
                  </span>
                ) : hasDefault ? (
                  'Gets every agent ticket that names nobody, and its notification.'
                ) : (
                  'Nobody: an agent ticket that names nobody stays unassigned until someone takes it.'
                )
              }
              value={settings.defaultAssignee ?? NO_DEFAULT}
              options={assigneeOptions}
              canEdit={canEdit}
              busy={busy}
              onSave={(v) => save({ defaultAssignee: v === NO_DEFAULT ? '' : v })}
            />
          </>
        )}
      </SettingsGroup>

      <SettingsGroup card title="Just for you" hint="Kept on this computer, for your login on this server." testId="tickets-just-for-you">
        <SettingRow
          settingId="tickets.open-on"
          label={<span className="inline-flex items-center gap-2">Open the Tickets page on<SaveNote state={openOnSave.state} /></span>}
          hint="The list you see first each time you open Tickets."
          field
        >
          <SettingDropdown
            ariaLabel="Open the Tickets page on"
            value={openOn}
            options={TICKET_OPEN_ON_OPTIONS}
            fullWidth
            onChange={(v) => {
              const next = v as TicketOpenOn
              if (next === openOn) return
              writeTicketOpenOn(next)
              setOpenOn(next)
              openOnSave.saved()
            }}
          />
        </SettingRow>
      </SettingsGroup>
    </div>
  )
}
