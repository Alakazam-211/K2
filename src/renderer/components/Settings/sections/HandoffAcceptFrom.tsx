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

const btn =
  'px-2 py-0.5 text-[10px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-text-muted)] cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed'
const muted = 'text-[var(--color-text-muted)]'
const input =
  'px-2 py-0.5 text-[11px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] min-w-0'

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

  return (
    <div
      className={compact ? 'py-1.5' : 'border border-[var(--color-border)] p-3 space-y-2'}
      data-testid="handoff-accept-from"
      data-settings-id="projects.handoff-accept-from"
    >
      <div className="flex items-center gap-2">
        <div className="text-xs font-medium text-[var(--color-text-primary)]">Secrets from other agents</div>
        {pending !== null && pending > 0 && (
          <button
            type="button"
            className={`text-[10px] underline ${muted} hover:text-[var(--color-text-primary)] cursor-pointer no-drag`}
            data-testid="handoff-pending-review"
            onClick={() => {
              requestVaultTab('incoming')
              useSettingsStore.getState().setSection('vault')
            }}
          >
            Pending: {pending} · Review
          </button>
        )}
      </div>
      <div className={`text-[11px] ${muted}`}>
        Secrets from these senders go straight into this agent&rsquo;s vault. Anything else waits for you (Settings →
        Vault → Incoming). A secret a person gave, or one that would replace a secret this agent has, always waits.
      </div>
      {entries !== null && entries.length === 0 && (
        <div className={`text-[11px] ${muted}`} data-testid="handoff-accept-empty">
          No one yet: every secret sent to this agent waits for you.
        </div>
      )}
      {entries !== null && entries.length > 0 && (
        <ul className="space-y-1">
          {entries.map((e) => (
            <li key={e.sender} className="flex items-center gap-2 text-[11px]" data-testid="handoff-accept-entry" data-sender={e.sender}>
              <span className="text-[var(--color-text-primary)]">{nameOf(e.sender)}</span>
              <span className={`text-[10px] ${muted}`}>added by {e.addedBy}</span>
              {!readOnly && (
                <button
                  type="button"
                  className={btn}
                  disabled={busy}
                  aria-label={`Stop accepting from ${e.sender}`}
                  onClick={() => void edit(e.sender, 'remove')}
                >
                  Remove
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      {!readOnly && (
        <div className="flex flex-wrap items-center gap-2">
          <select
            className={input}
            aria-label="Add an agent on this server"
            data-testid="handoff-accept-pick"
            value={pick}
            onChange={(e) => setPick(e.target.value)}
          >
            <option value="">An agent on this server…</option>
            {others.map((p) => (
              <option key={p.id} value={`ws:${p.id}`}>
                {p.handle || p.name}
              </option>
            ))}
          </select>
          <button type="button" className={btn} disabled={busy || !pick} data-testid="handoff-accept-pick-add" onClick={() => void edit(pick, 'add')}>
            Add
          </button>
          <input
            className={input}
            aria-label="Add a federated agent or a team"
            data-testid="handoff-accept-typed"
            placeholder="agent::host or team:<id>"
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
          />
          <button
            type="button"
            className={btn}
            disabled={busy || !typedSenderOk(typed)}
            data-testid="handoff-accept-typed-add"
            onClick={() => void edit(typed.trim(), 'add')}
          >
            Add
          </button>
        </div>
      )}
      {error && (
        <div className="text-[11px] text-[var(--color-danger,#e5484d)]" role="alert">
          {error}
        </div>
      )}
    </div>
  )
}
