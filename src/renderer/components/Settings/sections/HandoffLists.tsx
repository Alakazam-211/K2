// Settings → Vault → Incoming / Sent (prd-agent-secret-handoff-v1 §10.3).
//
// Secrets agents handed to each other, by name: who sent what to whom, its
// status, when, and the delivery id that matches the audit lines. A person
// decides pending incoming rows here (Accept / Accept as… / Decline), the
// same call as the Thread card. Never a value: the daemon's rows hold none.

import React, { useCallback, useEffect, useState } from 'react'
import { daemonErrorOf, daemonErrorText } from '@/lib/daemon-error'
import {
  handoffApi,
  handoffStatusWords,
  shortDelivery,
  splitPeer,
  type HandoffView,
} from './handoff-api'

const btn =
  'px-2 py-1 text-[10px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-text-muted)] cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed'
const muted = 'text-[var(--color-text-muted)]'
const danger = 'text-[var(--color-danger,#e5484d)]'
const input =
  'px-2 py-0.5 text-[11px] font-mono bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]'

const NAME_RE = /^[A-Za-z0-9_.-]{1,64}$/

function when(ts: string | null | undefined): string {
  return ts ? ts.slice(0, 16).replace('T', ' ') : ''
}

function peerWords(peer: string): string {
  const { agent, host } = splitPeer(peer)
  return host ? `${agent} on ${host}` : agent
}

function IncomingRow({ row, onChanged }: { row: HandoffView; onChanged: () => void }): React.JSX.Element {
  const [renaming, setRenaming] = useState(false)
  const [clash, setClash] = useState(row.reason === 'clash')
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const decide = async (body: { action: 'accept' | 'decline'; as?: string; replace?: boolean }): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await handoffApi.decide({ id: row.deliveryId, ...body })
      onChanged()
    } catch (e) {
      const info = daemonErrorOf(e)
      if (info?.code === 'name_taken') {
        setClash(true)
        setRenaming(true)
      }
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const pending = row.status === 'pending'
  return (
    <div className="p-2 text-[11px] space-y-1" data-testid="vault-incoming-row" data-delivery={row.deliveryId} data-status={row.status}>
      <div className="flex flex-wrap gap-x-3">
        <span className="font-mono text-[var(--color-text-primary)]">{row.landedName && row.status === 'landed' ? row.landedName : row.name}</span>
        <span className="text-[var(--color-text-secondary)]">
          from {peerWords(row.peer)} to {row.workspace}
        </span>
        <span className={muted}>{handoffStatusWords(row.status)}</span>
        <span className={muted}>{when(row.createdAt)}</span>
        <span className={`font-mono ${muted}`} title={row.deliveryId}>
          {shortDelivery(row.deliveryId)}
        </span>
      </div>
      {row.note && <div className={`${muted} whitespace-pre-wrap break-words`}>Note: {row.note}</div>}
      {row.status === 'landed' && row.ref && <div className="font-mono text-[10px] select-all">{row.ref}</div>}
      {row.decidedBy && !pending && <div className={muted}>decided by {row.decidedBy}</div>}
      {pending && (
        <div className="flex flex-wrap items-center gap-2">
          {renaming && (
            <>
              <input
                className={input}
                aria-label={`Accept ${row.name} as`}
                data-testid="vault-incoming-as"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder={`${row.name}_2`}
              />
              <button
                type="button"
                className={btn}
                disabled={busy || !NAME_RE.test(name.trim())}
                data-testid="vault-incoming-as-save"
                onClick={() => void decide({ action: 'accept', as: name.trim() })}
              >
                Accept as
              </button>
            </>
          )}
          {!renaming && (
            <button type="button" className={btn} disabled={busy} data-testid="vault-incoming-accept-as" onClick={() => setRenaming(true)}>
              Accept as…
            </button>
          )}
          {clash ? (
            <button type="button" className={btn} disabled={busy} data-testid="vault-incoming-replace" onClick={() => void decide({ action: 'accept', replace: true })}>
              Replace
            </button>
          ) : (
            <button type="button" className={btn} disabled={busy} data-testid="vault-incoming-accept" onClick={() => void decide({ action: 'accept' })}>
              Accept
            </button>
          )}
          <button type="button" className={btn} disabled={busy} data-testid="vault-incoming-decline" onClick={() => void decide({ action: 'decline' })}>
            Decline
          </button>
        </div>
      )}
      {error && (
        <p className={danger} role="alert">
          {error}
        </p>
      )}
    </div>
  )
}

function useRows(load: () => Promise<HandoffView[]>): {
  rows: HandoffView[] | null
  error: string | null
  refresh: () => void
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
  return { rows, error, refresh }
}

export function IncomingList(): React.JSX.Element {
  const load = useCallback(() => handoffApi.incoming(), [])
  const { rows, error, refresh } = useRows(load)
  return (
    <div className="space-y-1" data-testid="vault-incoming">
      <p className={`text-[11px] ${muted}`}>
        Secrets other agents sent to yours. Each waits for a person unless you chose &ldquo;Always accept from&rdquo; that
        sender (Settings → Projects → the agent). Accepting puts it in the agent&rsquo;s vault; it never sees the value.
      </p>
      {error && (
        <p className={`text-[11px] ${danger}`} role="alert">
          {error}
        </p>
      )}
      {rows === null && !error && <p className={`text-[11px] ${muted}`}>Loading…</p>}
      {rows !== null && rows.length === 0 && <p className={`text-[11px] ${muted}`}>Nothing sent to your agents yet.</p>}
      {rows !== null && rows.length > 0 && (
        <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
          {rows.map((r) => (
            <IncomingRow key={r.deliveryId} row={r} onChanged={refresh} />
          ))}
        </div>
      )}
    </div>
  )
}

export function SentList(): React.JSX.Element {
  const load = useCallback(() => handoffApi.sent(), [])
  const { rows, error } = useRows(load)
  return (
    <div className="space-y-1" data-testid="vault-sent">
      {error && (
        <p className={`text-[11px] ${danger}`} role="alert">
          {error}
        </p>
      )}
      {rows === null && !error && <p className={`text-[11px] ${muted}`}>Loading…</p>}
      {rows !== null && rows.length === 0 && <p className={`text-[11px] ${muted}`}>Your agents haven&rsquo;t sent any secrets.</p>}
      {rows !== null && rows.length > 0 && (
        <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
          {rows.map((r) => (
            <div key={r.deliveryId} className="p-2 text-[11px] space-y-0.5" data-testid="vault-sent-row" data-status={r.status}>
              <div className="flex flex-wrap gap-x-3">
                <span className="font-mono text-[var(--color-text-primary)]">{r.name}</span>
                <span className="text-[var(--color-text-secondary)]">
                  from {r.workspace} to {peerWords(r.peer)}
                </span>
                <span className={muted}>{handoffStatusWords(r.status)}</span>
                <span className={muted}>{when(r.createdAt)}</span>
                <span className={`font-mono ${muted}`} title={r.deliveryId}>
                  {shortDelivery(r.deliveryId)}
                </span>
              </div>
              {r.note && <div className={`${muted} whitespace-pre-wrap break-words`}>Note: {r.note}</div>}
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
