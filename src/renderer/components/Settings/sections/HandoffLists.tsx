// Settings → Vault → Incoming / Sent (prd-agent-secret-handoff-v1 §10.3).
//
// Secrets agents handed to each other, by name: who sent what to whom, its
// status and when (the delivery id that matches the audit lines is the
// line's title). A person
// decides pending incoming rows here (Accept / Accept as… / Decline), the
// same call as the Thread card. Never a value: the daemon's rows hold none.
// Layout: the Settings card group, one row per delivery; Replace (which
// overwrites a secret already there) asks first.

import React, { useCallback, useEffect, useId, useState } from 'react'
import { daemonErrorOf, daemonErrorText } from '@/lib/daemon-error'
import { Button, Input, Tag, type TagTone } from '@/components/ui'
import { SETTING_ROW_HINT, SETTING_SERVER_REASON, SettingsGroup, SettingsLoadState } from '../controls/SettingControls'
import { ConfirmSheet } from './AccessDialogs'
import { handoffApi, splitPeer, type HandoffView } from './handoff-api'
import { agentUseWords, deliveryReadable, whenWords } from './vault-copy'
import type { ServerScope } from '@/kessel/server-scope'

const NAME_RE = /^[A-Za-z0-9_.-]{1,64}$/

/** States a person must notice get a tag (sentence case); the rest are
 *  words at the start of the row's quiet line. */
const IN_TAG: Record<string, { label: string; tone: TagTone }> = {
  pending: { label: 'Waiting for you', tone: 'warn' },
  held: { label: 'Held for the owner', tone: 'warn' },
  expired: { label: 'Expired', tone: 'muted' },
}
const OUT_TAG: Record<string, { label: string; tone: TagTone }> = {
  expired: { label: 'Expired', tone: 'muted' },
}

const STATUS_WORDS: Record<string, string> = {
  landed: 'Accepted',
  declined: 'Declined',
  unsent: 'Taken back',
  unknown: 'No answer yet',
  refused: 'Refused',
  pending: 'Not accepted yet',
  held: 'Held for their owner',
}

function StatusTag({ status, tags }: { status: string; tags: Record<string, { label: string; tone: TagTone }> }): React.JSX.Element | null {
  const t = tags[status]
  return t ? (
    <Tag tone={t.tone} data-case="as-written">
      {t.label}
    </Tag>
  ) : null
}

/** What accepting lets the receiving workspace's agents do (accepting is
 *  consent), and how far they may go later without asking again. */
function consentWords(workspace: string, delivery: string, ceiling: string): string {
  const more =
    ceiling && ceiling !== delivery && deliveryReadable(ceiling) && !deliveryReadable(delivery)
      ? ' Later they may read it without asking you again.'
      : ''
  return `If you accept, ${workspace}’s agents can ${agentUseWords(delivery)}.${more}`
}

function peerWords(peer: string): string {
  const { agent, host } = splitPeer(peer)
  return host ? `${agent} on ${host}` : agent
}

/** One quiet line: "Accepted by alex · from sales on lab.example.com to ops ·
 *  10 Nov 08:30". The delivery id (what audit lines quote) is its title. */
function WhoLine({
  from,
  to,
  at,
  id,
  status,
  tags,
  decidedBy,
}: {
  from: string
  to: string
  at: string
  id: string
  status: string
  tags: Record<string, { label: string; tone: TagTone }>
  decidedBy?: string | null
}): React.JSX.Element {
  const words = tags[status] ? '' : (STATUS_WORDS[status] ?? '')
  const lead = words ? `${words}${decidedBy && status !== 'pending' ? ` by ${decidedBy}` : ''} · from` : 'From'
  return (
    <p className="type-hint text-[var(--color-text-secondary)] mt-0.5 leading-relaxed truncate" title={`Delivery ${id}, as the audit log names it`}>
      {lead} {from} to {to}
      {at ? ` · ${whenWords(at)}` : ''}
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
  const [failed, setFailed] = useState<'accept' | 'decline'>('accept')
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
      setError(daemonErrorText(e).trim().replace(/[.\s]+$/, ''))
      setFailed(body.action)
    } finally {
      setBusy(false)
    }
  }

  const pending = row.status === 'pending'
  const shownName = row.landedName && row.status === 'landed' ? row.landedName : row.name
  const consent = pending && row.landsAs ? consentWords(row.workspace, row.landsAs.delivery, row.landsAs.ceiling) : null
  const nameOk = NAME_RE.test(name.trim())
  return (
    <div className="px-4 py-3" data-testid="vault-incoming-row" data-delivery={row.deliveryId} data-status={row.status}>
      <div className="flex items-start justify-between gap-6">
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <span className="type-name font-mono text-[var(--color-text-primary)]">{shownName}</span>
            <StatusTag status={row.status} tags={IN_TAG} />
          </div>
          <WhoLine
            from={peerWords(row.peer)}
            to={row.workspace}
            at={row.createdAt}
            id={row.deliveryId}
            status={row.status}
            tags={IN_TAG}
            decidedBy={row.decidedBy}
          />
          {row.note ? <p className={`${SETTING_ROW_HINT} whitespace-pre-wrap break-words`}>Their note: “{row.note}”</p> : null}
          {consent ? (
            <p className={SETTING_ROW_HINT} data-testid="vault-incoming-consent">
              {consent}
            </p>
          ) : null}
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
        <div role="alert" className="mt-2 max-w-[75ch]">
          <p className="type-hint text-[var(--color-status-error-soft)] leading-relaxed">
            Couldn’t {failed} {row.name}. Nothing changed{clash ? '; pick another name or replace the one there.' : '; you can try again.'}
          </p>
          <p className={SETTING_SERVER_REASON}>The server said: {error}.</p>
        </div>
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
          <div className="flex items-center gap-2 min-w-0">
            <span className="type-name font-mono text-[var(--color-text-primary)] truncate">{r.name}</span>
            <StatusTag status={r.status} tags={OUT_TAG} />
          </div>
          <WhoLine from={r.workspace} to={peerWords(r.peer)} at={r.createdAt} id={r.deliveryId} status={r.status} tags={OUT_TAG} decidedBy={r.decidedBy} />
          {r.note ? <p className={`${SETTING_ROW_HINT} whitespace-pre-wrap break-words`}>Note sent with it: “{r.note}”</p> : null}
        </div>
      ))}
    </SettingsGroup>
  )
}
