// Settings → Vault → Incoming / Sent (prd-agent-secret-handoff-v1 §10.3).
//
// Secrets agents handed to each other, by name: who sent what to whom, its
// status, when, and the delivery id that matches the audit lines. A person
// decides pending incoming rows here (Accept / Accept as… / Decline), the
// same call as the Thread card. Never a value: the daemon's rows hold none.
// Layout: the Settings card group, one row per delivery; Replace (which
// overwrites a secret already there) asks first.

import React, { useCallback, useEffect, useId, useState } from 'react'
import { daemonErrorOf, daemonErrorText } from '@/lib/daemon-error'
import { Button, Input, Tag, type TagTone } from '@/components/ui'
import { SETTING_ROW_HINT, SettingsGroup, SettingsLoadState } from '../controls/SettingControls'
import { ConfirmSheet } from './AccessDialogs'
import { handoffApi, shortDelivery, splitPeer, type HandoffView } from './handoff-api'
import { deliveryLabel, whenWords } from './vault-copy'
import type { ServerScope } from '@/kessel/server-scope'

const NAME_RE = /^[A-Za-z0-9_.-]{1,64}$/

const STATUS: Record<string, { label: string; tone: TagTone }> = {
  pending: { label: 'Waiting for you', tone: 'warn' },
  held: { label: 'Held for the owner', tone: 'warn' },
  landed: { label: 'In the vault', tone: 'ok' },
  declined: { label: 'Declined', tone: 'muted' },
  expired: { label: 'Expired', tone: 'muted' },
  unsent: { label: 'Taken back', tone: 'muted' },
  unknown: { label: 'No answer yet', tone: 'muted' },
  refused: { label: 'Refused', tone: 'danger' },
}

function StatusTag({ status }: { status: string }): React.JSX.Element {
  const s = STATUS[status] ?? { label: status, tone: 'muted' as TagTone }
  return <Tag tone={s.tone}>{s.label}</Tag>
}

/** What accepting gives the receiving agent (accepting is consent): how it
 *  gets it, and the most its agents may take it as without asking again. */
function consentWords(delivery: string, ceiling: string): string {
  const more = ceiling && ceiling !== delivery ? ` Later its agents may take it as far as “${deliveryLabel(ceiling)}” without asking you again.` : ''
  return `If you accept, the agent gets it this way: ${deliveryLabel(delivery)}.${more}`
}

function peerWords(peer: string): string {
  const { agent, host } = splitPeer(peer)
  return host ? `${agent} on ${host}` : agent
}

/** "From sales on lab.example.com to ops · 10 Nov 08:30 · hd_7f3c…" */
function WhoLine({ from, to, at, id }: { from: string; to: string; at: string; id: string }): React.JSX.Element {
  return (
    <p className={SETTING_ROW_HINT}>
      From {from} to {to}
      {at ? ` · ${whenWords(at)}` : ''} ·{' '}
      <span className="font-mono" title={`Delivery ${id}: the id in the audit log`}>
        {shortDelivery(id)}
      </span>
    </p>
  )
}

function IncomingRow({
  row,
  scope,
  onChanged,
}: {
  row: HandoffView
  scope: ServerScope
  onChanged: () => void
}): React.JSX.Element {
  const [renaming, setRenaming] = useState(false)
  const [clash, setClash] = useState(row.reason === 'clash')
  const [confirmReplace, setConfirmReplace] = useState(false)
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const nameId = useId()

  const decide = async (body: { action: 'accept' | 'decline'; as?: string; replace?: boolean }): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      // Settings → Vault lists this window's server's handoffs (Settings passes its scope).
      await handoffApi.decide({ id: row.deliveryId, ...body }, scope)
      onChanged()
    } catch (e) {
      const info = daemonErrorOf(e)
      if (info?.code === 'name_taken') {
        setClash(true)
        setRenaming(true)
      }
      const what = body.action === 'decline' ? 'Couldn’t decline it' : 'Couldn’t accept it'
      setError(`${what}. The server said: ${daemonErrorText(e).trim().replace(/[.\s]+$/, '')}.`)
    } finally {
      setBusy(false)
    }
  }

  const pending = row.status === 'pending'
  const shownName = row.landedName && row.status === 'landed' ? row.landedName : row.name
  const consent = pending && row.landsAs ? consentWords(row.landsAs.delivery, row.landsAs.ceiling) : null
  const nameOk = NAME_RE.test(name.trim())
  return (
    <div className="px-4 py-3" data-testid="vault-incoming-row" data-delivery={row.deliveryId} data-status={row.status}>
      <div className="flex items-start justify-between gap-6">
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <span className="type-name font-mono text-[var(--color-text-primary)]">{shownName}</span>
            <StatusTag status={row.status} />
          </div>
          <WhoLine from={peerWords(row.peer)} to={row.workspace} at={row.createdAt} id={row.deliveryId} />
          {row.note ? <p className={`${SETTING_ROW_HINT} whitespace-pre-wrap break-words`}>Their note: “{row.note}”</p> : null}
          {consent ? (
            <p className={SETTING_ROW_HINT} data-testid="vault-incoming-consent">
              {consent}
            </p>
          ) : null}
          {row.status === 'landed' && row.ref ? <p className={`${SETTING_ROW_HINT} font-mono select-all`}>{row.ref}</p> : null}
          {row.decidedBy && !pending ? <p className={SETTING_ROW_HINT}>Decided by {row.decidedBy}</p> : null}
        </div>
        {pending ? (
          <div className="flex items-center gap-2 flex-shrink-0">
            {!renaming ? (
              <Button variant="quiet" disabled={busy} data-testid="vault-incoming-accept-as" onClick={() => setRenaming(true)}>
                Accept as…
              </Button>
            ) : null}
            <Button variant="ghost" disabled={busy} data-testid="vault-incoming-decline" onClick={() => void decide({ action: 'decline' })}>
              Decline
            </Button>
            {clash ? (
              <Button disabled={busy} data-testid="vault-incoming-replace" onClick={() => setConfirmReplace(true)}>
                Replace…
              </Button>
            ) : (
              <Button variant="accent" disabled={busy} data-testid="vault-incoming-accept" onClick={() => void decide({ action: 'accept' })}>
                Accept
              </Button>
            )}
          </div>
        ) : null}
      </div>
      {pending && renaming ? (
        <form
          className="mt-3"
          onSubmit={(e) => {
            e.preventDefault()
            if (nameOk && !busy) void decide({ action: 'accept', as: name.trim() })
          }}
        >
          <label htmlFor={nameId} className="block type-row text-[var(--color-text-primary)] mb-1">
            Accept it under another name
          </label>
          <div className="flex items-center gap-2">
            <Input
              id={nameId}
              className="w-60 type-control font-mono"
              aria-label={`Accept ${row.name} as`}
              data-testid="vault-incoming-as"
              value={name}
              autoFocus
              onChange={(e) => setName(e.target.value)}
              placeholder={`${row.name}_2`}
            />
            <Button type="submit" disabled={busy || !nameOk} data-testid="vault-incoming-as-save">
              Accept as
            </Button>
            <Button variant="ghost" disabled={busy} onClick={() => setRenaming(false)}>
              Cancel
            </Button>
          </div>
          <p className={SETTING_ROW_HINT}>
            {clash ? `${row.workspace} already has a secret called ${row.name}. Pick another name, or replace that one.` : 'Letters, numbers, dots, dashes and underscores.'}
          </p>
        </form>
      ) : null}
      {error ? (
        <p role="alert" className="type-hint text-[var(--color-status-error-soft)] mt-2 leading-relaxed max-w-[75ch]">
          {error} Nothing changed; try again.
        </p>
      ) : null}
      {confirmReplace ? (
        <ConfirmSheet
          title={`Replace ${row.name} in ${row.workspace}?`}
          message={`The ${row.name} already there is replaced by the one ${peerWords(row.peer)} sent. ${row.workspace}’s agents use the new one from their next call.`}
          confirmLabel="Replace"
          destructive
          testId="vault-incoming-replace-confirm"
          onCancel={() => setConfirmReplace(false)}
          onConfirm={() => {
            setConfirmReplace(false)
            void decide({ action: 'accept', replace: true })
          }}
        />
      ) : null}
    </div>
  )
}

function useRows(load: () => Promise<HandoffView[]>): {
  rows: HandoffView[] | null
  error: string | null
  refresh: () => void
  retry: () => void
} {
  const [rows, setRows] = useState<HandoffView[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [nonce, setNonce] = useState(0)
  useEffect(() => {
    let live = true
    load()
      .then((r) => {
        if (!live) return
        setRows(r)
        setError(null)
      })
      .catch((e) => live && setError(daemonErrorText(e)))
    return () => {
      live = false
    }
  }, [load, nonce])
  const refresh = useCallback(() => setNonce((n) => n + 1), [])
  const retry = useCallback(() => {
    setError(null)
    setRows(null)
    setNonce((n) => n + 1)
  }, [])
  return { rows, error, refresh, retry }
}

export function IncomingList({ scope }: { scope: ServerScope }): React.JSX.Element {
  const load = useCallback(() => handoffApi.incoming(undefined, scope), [scope])
  const { rows, error, refresh, retry } = useRows(load)
  return (
    <SettingsGroup
      title="Sent to your agents"
      hint="Secrets other agents sent to yours. Accepting puts it in that agent’s vault without showing it the value. To stop asking for one sender, turn on Always accept from on the agent’s page in Workspaces / Agents."
      card
      testId="vault-incoming"
      state={
        error ? (
          <SettingsLoadState kind="error" title="Couldn’t load incoming secrets" detail={error} action={<Button onClick={retry}>Try again</Button>}>
            Nothing was accepted or declined. Try again in a moment.
          </SettingsLoadState>
        ) : rows === null ? (
          <SettingsLoadState kind="loading" title="Loading incoming secrets…" />
        ) : rows.length === 0 ? (
          <SettingsLoadState kind="empty" title="Nothing sent to your agents yet">
            When another agent sends one of yours a secret, it waits here for you.
          </SettingsLoadState>
        ) : undefined
      }
    >
      {rows?.map((r) => <IncomingRow key={r.deliveryId} row={r} scope={scope} onChanged={refresh} />)}
    </SettingsGroup>
  )
}

export function SentList({ scope }: { scope: ServerScope }): React.JSX.Element {
  const load = useCallback(() => handoffApi.sent(undefined, scope), [scope])
  const { rows, error, retry } = useRows(load)
  return (
    <SettingsGroup
      title="Sent by your agents"
      hint="Secrets your agents sent to other agents, and whether they were accepted. The value never shows here."
      card
      testId="vault-sent"
      state={
        error ? (
          <SettingsLoadState kind="error" title="Couldn’t load sent secrets" detail={error} action={<Button onClick={retry}>Try again</Button>}>
            Try again in a moment.
          </SettingsLoadState>
        ) : rows === null ? (
          <SettingsLoadState kind="loading" title="Loading sent secrets…" />
        ) : rows.length === 0 ? (
          <SettingsLoadState kind="empty" title="Your agents haven’t sent any secrets">
            When one of your agents sends a secret to another agent, it shows here.
          </SettingsLoadState>
        ) : undefined
      }
    >
      {rows?.map((r) => (
        <div key={r.deliveryId} className="px-4 py-3" data-testid="vault-sent-row" data-status={r.status}>
          <div className="flex items-center gap-2 flex-wrap">
            <span className="type-name font-mono text-[var(--color-text-primary)]">{r.name}</span>
            <StatusTag status={r.status} />
          </div>
          <WhoLine from={r.workspace} to={peerWords(r.peer)} at={r.createdAt} id={r.deliveryId} />
          {r.note ? <p className={`${SETTING_ROW_HINT} whitespace-pre-wrap break-words`}>Note sent with it: “{r.note}”</p> : null}
        </div>
      ))}
    </SettingsGroup>
  )
}
