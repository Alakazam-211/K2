// Settings → Workspaces → the agent → Permissions: "Secrets it may send to
// other servers" (0.46.11).
//
// A secret a person gave an agent goes to another server only on a person's
// say. That yes is a `vault.send` card (send once, or "from now on"), or a
// standing grant made here: "May send <secret> to <agent::host>", each with
// Revoke. Adding one asks for the same vault confirm as sending it
// (`handoff-send`, bound to that secret). Grants live in the vault (row-
// MACed) and are written through `/cli/vault/send-allow`, Owner or Admin
// only: the daemon refuses an agent and a Member.

import React, { useCallback, useEffect, useState } from 'react'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { useServerSupports } from '@/lib/server-capabilities'
import { daemonErrorText } from '@/lib/daemon-error'
import { primaryScope } from '@/kessel/server-scope'
import { handoffApi, shortDay, type SendAllowList } from './handoff-api'
import { ConfirmStep } from './DataSourceConfirm'
import { SETTING_FIELD_WIDTH, SettingDropdown, SettingRow } from '../controls/SettingControls'
import { Button, Input } from '@/components/ui'

const muted = 'text-[var(--color-text-secondary)]'

/** `agent::host` (or `agent@host`): an agent on another server. */
export function remoteAgentOk(s: string): boolean {
  const t = s.trim()
  if (!t || t.includes('*') || t.startsWith('ws:') || t.startsWith('team:')) return false
  return /^[A-Za-z0-9._-]+(::|@)[A-Za-z0-9.:_-]+$/.test(t) && t.split('::').length <= 2
}

type Adding = { secretId: string; name: string; to: string }

export function AgentHandoffSendAllow({
  project,
  readOnly = false,
}: {
  project: ProjectWithWorkspaces
  readOnly?: boolean
}): React.JSX.Element | null {
  const supported = useServerSupports('vault-send-allow')
  const [data, setData] = useState<SendAllowList | null>(null)
  const [pick, setPick] = useState('')
  const [to, setTo] = useState('')
  const [confirming, setConfirming] = useState<Adding | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      setData(await handoffApi.sendAllow(project.id))
      setError(null)
    } catch (e) {
      setError(daemonErrorText(e))
    }
  }, [project.id])

  useEffect(() => {
    if (supported) void load()
  }, [supported, load])

  // A POST answers the grants only: keep the picker's secrets.
  const keep = (next: SendAllowList): void => setData((prev) => ({ entries: next.entries, secrets: next.secrets.length ? next.secrets : prev?.secrets ?? [] }))

  const revoke = async (secretId: string, target: string): Promise<void> => {
    if (busy || readOnly) return
    setBusy(true)
    setError(null)
    try {
      keep(await handoffApi.editSendAllow({ workspace: project.id, secretId, to: target, action: 'remove' }))
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const add = async (a: Adding, confirmToken: string): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      keep(await handoffApi.editSendAllow({ workspace: project.id, secretId: a.secretId, to: a.to, action: 'add', confirmToken }))
      setPick('')
      setTo('')
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
      setConfirming(null)
    }
  }

  if (!supported) return null
  const entries = data?.entries ?? []
  const secrets = data?.secrets ?? []
  const picked = secrets.find((s) => s.id === pick)

  const below = (
    <div className="space-y-2">
      {data !== null && entries.length === 0 && (
        <div className={`type-hint ${muted}`} data-testid="send-allow-empty">
          None yet, so each one waits for a person on a card.
        </div>
      )}
      {entries.length > 0 && (
        <ul className="space-y-1">
          {entries.map((e) => (
            <li
              key={`${e.secretId}|${e.to}`}
              className="flex items-center gap-2 type-hint"
              data-testid="send-allow-entry"
              data-secret={e.name}
              data-to={e.to}
            >
              <span className="text-[var(--color-text-primary)]">
                May send {e.name} to {e.to}
              </span>
              <span className={`type-hint ${muted}`}>
                added by {e.addedBy}
                {shortDay(e.addedAt) ? `, ${shortDay(e.addedAt)}` : ''}
              </span>
              {!readOnly && (
                <Button
                  variant="quiet"
                  disabled={busy}
                  data-testid="send-allow-revoke"
                  aria-label={`Stop letting it send ${e.name} to ${e.to}`}
                  onClick={() => void revoke(e.secretId, e.to)}
                >
                  Revoke
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
      {!readOnly && confirming && (
        <div data-testid="send-allow-confirm" className="space-y-1">
          <div className={`type-hint ${muted}`}>
            Confirm it&rsquo;s you to let it send {confirming.name} to {confirming.to} from now on. It&rsquo;s the same
            confirm as sending it yourself.
          </div>
          <ConfirmStep
            scope={primaryScope()}
            action="handoff-send"
            secretId={confirming.secretId}
            label={`let ${project.handle || project.name} send ${confirming.name} to ${confirming.to}`}
            onToken={(token) => void add(confirming, token)}
            onCancel={() => setConfirming(null)}
          />
        </div>
      )}
      {!readOnly && !confirming && (
        <div className="flex flex-wrap items-center gap-2">
          <div className={`${SETTING_FIELD_WIDTH} min-w-0`}>
            <SettingDropdown
              menuAlign="left"
              fullWidth
              ariaLabel="A secret a person gave this agent"
              triggerData={{ 'data-testid': 'send-allow-pick' }}
              placeholder={secrets.length === 0 ? 'No secrets a person gave it' : 'A secret a person gave it'}
              disabled={secrets.length === 0}
              value={pick}
              options={secrets.map((s) => ({ value: s.id, label: s.name }))}
              onChange={setPick}
            />
          </div>
          <Input
            className={`${SETTING_FIELD_WIDTH} min-w-0 type-control`}
            aria-label="The agent on another server (agent::host)"
            data-testid="send-allow-to"
            placeholder="agent::host"
            value={to}
            onChange={(e) => setTo(e.target.value)}
          />
          <Button
            variant="quiet"
            disabled={busy || !picked || !remoteAgentOk(to)}
            data-testid="send-allow-add"
            onClick={() => picked && setConfirming({ secretId: picked.id, name: picked.name, to: to.trim() })}
          >
            Add
          </Button>
        </div>
      )}
      {error && (
        <div className="type-hint text-[var(--color-status-error-soft)]" role="alert">
          {error}
        </div>
      )}
    </div>
  )

  const hint =
    'A secret a person gave this agent goes to another server only when a person says so. These it may send without asking. Anything else asks on a card.'

  return (
    <div data-testid="send-allow">
      <SettingRow settingId="projects.handoff-send-allow" label="Secrets it may send to other servers" hint={hint} below={below}>
        {null}
      </SettingRow>
    </div>
  )
}
