// Settings → Projects → the agent: "Secrets from other agents"
// (prd-agent-secret-handoff-v1 §7.3, §10.4, H19).
//
// The accept-from list: secrets from these senders land straight in this
// agent's vault; everything else waits for a person (Settings → Vault →
// Incoming, or the Thread card). The list lives in the vault (row-MACed),
// so it is written through `/cli/vault/accept-from`, never through
// `/cli/agent-access/set`. People only: the daemon refuses an agent.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { useServerSupports } from '@/lib/server-capabilities'
import { daemonErrorText } from '@/lib/daemon-error'
import { handoffApi, requestVaultTab, type AcceptFromEntry } from './handoff-api'
import { SETTING_FIELD_WIDTH, SETTING_ROW_LABEL, SettingDropdown, SettingRow } from '../controls/SettingControls'
import { Button, Input } from '@/components/ui'

const muted = 'text-[var(--color-text-secondary)]'

/** A typed sender: `agent::host`, `agent@host`, `team:<id>` or `ws:<id>`. */
export function typedSenderOk(s: string): boolean {
  const t = s.trim()
  if (!t || t.includes('*')) return false
  if (/^(ws|team):[A-Za-z0-9._-]+$/.test(t)) return true
  return /^[A-Za-z0-9._-]+(::|@)[A-Za-z0-9.:_-]+$/.test(t)
}

export function AgentHandoffAcceptFrom({
  project,
  compact = false,
  readOnly = false,
}: {
  project: ProjectWithWorkspaces
  compact?: boolean
  readOnly?: boolean
}): React.JSX.Element | null {
  const supported = useServerSupports('vault-handoff')
  const projects = useProjectsStore((s) => s.projects)
  const [entries, setEntries] = useState<AcceptFromEntry[] | null>(null)
  const [pending, setPending] = useState<number | null>(null)
  const [pick, setPick] = useState('')
  const [typed, setTyped] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const [list, incoming] = await Promise.all([
        handoffApi.acceptFrom(project.id),
        handoffApi.incoming(project.id),
      ])
      setEntries(list)
      setPending(incoming.filter((r) => r.status === 'pending').length)
      setError(null)
    } catch (e) {
      setError(daemonErrorText(e))
    }
  }, [project.id])

  useEffect(() => {
    if (supported) void load()
  }, [supported, load])

  const others = useMemo(
    () => projects.filter((p) => p.id !== project.id && !(entries ?? []).some((e) => e.sender === `ws:${p.id}`)),
    [projects, project.id, entries],
  )

  const nameOf = (sender: string): string => {
    if (sender.startsWith('ws:')) {
      const id = sender.slice(3)
      const p = projects.find((x) => x.id === id)
      return p ? `${p.handle || p.name} (this server)` : `${id} (this server)`
    }
    if (sender.startsWith('team:')) return `team ${sender.slice(5)}`
    return sender
  }

  const edit = async (sender: string, action: 'add' | 'remove'): Promise<void> => {
    if (busy || readOnly) return
    setBusy(true)
    setError(null)
    try {
      const list = await handoffApi.editAcceptFrom({ workspace: project.id, sender, action })
      setEntries(list)
      if (action === 'add') {
        setPick('')
        setTyped('')
      }
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  if (!supported) return null

  const pendingLink = pending !== null && pending > 0 && (
    <Button
      variant="quiet"
      data-testid="handoff-pending-review"
      onClick={() => {
        requestVaultTab('incoming')
        useSettingsStore.getState().setSection('vault')
      }}
    >
      Review {pending} waiting
    </Button>
  )

  // The row reads like the others: label and hint on the left, the waiting
  // count on the right. Who's on the list, and the two ways to add someone,
  // sit on the line below.
  const below = (
    <div className="space-y-2">
      {entries !== null && entries.length === 0 && (
        <div className={`type-hint ${muted}`} data-testid="handoff-accept-empty">
          No one yet, so every secret sent to this agent waits for you.
        </div>
      )}
      {entries !== null && entries.length > 0 && (
        <ul className="space-y-1">
          {entries.map((e) => (
            <li key={e.sender} className="flex items-center gap-2 type-hint" data-testid="handoff-accept-entry" data-sender={e.sender}>
              <span className="text-[var(--color-text-primary)]">{nameOf(e.sender)}</span>
              <span className={`type-hint ${muted}`}>added by {e.addedBy}</span>
              {!readOnly && (
                <Button
                  variant="quiet"
                  disabled={busy}
                  aria-label={`Stop accepting from ${e.sender}`}
                  onClick={() => void edit(e.sender, 'remove')}
                >
                  Remove
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
      {!readOnly && (
        <>
          <div className="flex items-center gap-2">
            <div className={`${SETTING_FIELD_WIDTH} min-w-0`}>
            <SettingDropdown
              menuAlign="left"
              fullWidth
              ariaLabel="Add an agent on this server"
              triggerData={{ 'data-testid': 'handoff-accept-pick' }}
              placeholder={others.length === 0 ? 'No other agents on this server' : 'An agent on this server'}
              disabled={others.length === 0}
              value={pick}
              options={others.map((p) => ({ value: `ws:${p.id}`, label: p.handle || p.name }))}
              onChange={setPick}
            />
            </div>
            <Button variant="quiet" disabled={busy || !pick} data-testid="handoff-accept-pick-add" onClick={() => void edit(pick, 'add')}>
              Add
            </Button>
          </div>
          <div className="flex items-center gap-2">
            <Input
              className={`${SETTING_FIELD_WIDTH} min-w-0 type-control`}
              aria-label="Add an agent on another server, or a team"
              data-testid="handoff-accept-typed"
              placeholder="Address from another server"
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
            />
            <Button
              variant="quiet"
              disabled={busy || !typedSenderOk(typed)}
              data-testid="handoff-accept-typed-add"
              onClick={() => void edit(typed.trim(), 'add')}
            >
              Add
            </Button>
          </div>
          <div className={`type-hint ${muted}`}>
            An agent&rsquo;s address is under Handle on its Agent tab. For a whole team, type{' '}
            <code>team:</code> and the team&rsquo;s ID.
          </div>
        </>
      )}
      {error && (
        <div className="type-hint text-[var(--color-status-error-soft)]" role="alert">
          {error}
        </div>
      )}
    </div>
  )

  const hint = 'Secrets these agents send go straight into this agent\u2019s vault. Anything else waits for you in Vault \u2192 Incoming.'

  if (compact) {
    return (
      <div className="py-1.5 space-y-2" data-testid="handoff-accept-from" data-settings-id="projects.handoff-accept-from">
        <div className="flex items-center gap-2">
          <div className={SETTING_ROW_LABEL}>Secrets from other agents</div>
          {pendingLink}
        </div>
        {below}
      </div>
    )
  }

  return (
    <div data-testid="handoff-accept-from">
      <SettingRow settingId="projects.handoff-accept-from" label="Secrets from other agents" hint={hint} below={below}>
        {pendingLink || null}
      </SettingRow>
    </div>
  )
}
