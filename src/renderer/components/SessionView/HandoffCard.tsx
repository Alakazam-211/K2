// A secret another agent sent (prd-agent-secret-handoff-v1 §10.1, H18).
//
// The daemon posts this card on the receiving agent's Thread when a handoff
// waits for a person. It carries names, sites, the sender's note and the
// delivery id: never a value, and there is no value field here. A person
// decides with Accept / Accept as… / Decline (and "Always accept from"),
// which POSTs `/cli/vault/incoming/decide`; the daemon refuses a passport,
// so an agent can't accept its own card. A name clash (`name_taken`) offers
// a new name or Replace (a new version of the current secret).

import { useState, type JSX } from 'react'
import { daemonErrorOf, daemonErrorText } from '@/lib/daemon-error'
import { CHAMFER_CUT, chamferVars } from '@/components/Thread/chamfer'
import { cardExpiryLabel, ttlWords, type OverlayHandoff } from './overlayThread'
import { siteLabel } from './SecretCard'
import {
  handoffConsentWords,
  handoffKindWords,
  splitPeer,
  type HandoffDecideBody,
  type HandoffDecideResult,
} from '@/components/Settings/sections/handoff-api'

export interface HandoffCardProps {
  handoff: OverlayHandoff
  /** `cardAnswererLabel` of the card's `answeredBy`. */
  answeredBy?: string | null
  nowSec?: number
  /** POSTs `/cli/vault/incoming/decide`. Throws the daemon's refusal. */
  onDecide: (body: HandoffDecideBody) => Promise<HandoffDecideResult | void>
  /** The card sits in a room on another server, which this window can't
   *  decide for (a room never sends `vault/incoming/decide`): no buttons,
   *  just "Open <host> directly to accept this". */
  remoteHost?: string | null
}

const NAME_RE = /^[A-Za-z0-9_.-]{1,64}$/

export function HandoffCard({ handoff, answeredBy, nowSec, onDecide, remoteHost = null }: HandoffCardProps): JSX.Element {
  const { agent, host: peerHost } = splitPeer(handoff.from)
  const host = handoff.host || peerHost
  const [always, setAlways] = useState(false)
  const [renaming, setRenaming] = useState(false)
  const [newName, setNewName] = useState('')
  // A clash known up front (the card says so) or learned from `name_taken`.
  const [clash, setClash] = useState(handoff.clash)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // The decision as the daemon answered it, until the card itself updates.
  const [decided, setDecided] = useState<{ status: string; ref?: string | null } | null>(null)

  const muted = 'text-[var(--thread-card-muted,var(--color-text-muted))]'
  const text = 'text-[var(--thread-card-text,var(--color-text-primary))]'
  const border = 'border-[var(--thread-card-border,var(--color-border))]'
  const danger = 'text-[var(--color-danger,#e5484d)]'

  const status = decided?.status ?? handoff.status
  const reference = decided?.ref ?? handoff.reference ?? null

  if (status !== 'pending') {
    const words =
      status === 'landed' ? 'accepted' : status === 'declined' ? 'declined' : status === 'expired' ? 'expired' : status
    return (
      <div data-testid="thread-handoff-card" data-status={status} className={`mt-1 text-[11px] ${muted}`}>
        <span className="font-mono">{handoff.name}</span> from {agent} {words}
        {answeredBy && status !== 'expired' ? <span data-testid="thread-handoff-answered-by"> by {answeredBy}</span> : null}
        {status === 'landed' && reference ? (
          <>
            {' '}
            as <span className="font-mono select-all" data-testid="thread-handoff-ref">{reference}</span>
          </>
        ) : null}
      </div>
    )
  }

  const expiry = cardExpiryLabel(handoff.expiresAt, nowSec ?? Math.floor(Date.now() / 1000))
  // A doc from another server can come without `hosts`.
  const sites = (handoff.hosts ?? []).map(siteLabel).filter(Boolean)
  // Accepting is consent (2026-10-30): the person sees what the agent gets.
  const consent = handoffConsentWords(handoff.delivery, handoff.ceiling)

  const decide = async (body: Omit<HandoffDecideBody, 'id'>): Promise<void> => {
    if (busy) return
    setBusy(true)
    setError(null)
    try {
      const res = await onDecide({ id: handoff.deliveryId, ...body, ...(always && body.action === 'accept' ? { always: true } : {}) })
      const view = res && typeof res === 'object' ? res.delivery : undefined
      setDecided({ status: view?.status || (body.action === 'accept' ? 'landed' : 'declined'), ref: res?.ref ?? view?.ref ?? null })
    } catch (e) {
      const info = daemonErrorOf(e)
      if (info?.code === 'name_taken') {
        setClash(true)
        setRenaming(true)
        setError(info.hint || `This agent already has ${handoff.name}.`)
      } else {
        setError(daemonErrorText(e))
      }
    } finally {
      setBusy(false)
    }
  }

  const nameOk = NAME_RE.test(newName.trim())

  return (
    <div
      data-testid="thread-handoff-card"
      data-status={status}
      data-chamfer=""
      style={chamferVars({
        cut: CHAMFER_CUT.surface,
        fill: 'var(--thread-card-bg,var(--color-bg))',
        edge: 'var(--thread-card-border,var(--color-border))',
        border: 1,
      })}
      className={`mt-1 w-full box-border flex flex-col gap-2 rounded-[var(--thread-card-radius,0px)] border ${border} bg-[var(--thread-card-bg,var(--color-bg))] px-2.5 py-2 text-[11px] ${text}`}
    >
      <div data-testid="thread-handoff-header">
        <span className="font-medium">{agent}</span>
        {host ? <span className={muted}> ({host})</span> : null} sent{' '}
        <span className="font-mono font-medium">{handoff.name}</span>
        <span className={muted}>
          {' '}
          ({handoffKindWords(handoff.kind)}
          {sites.length ? `, for ${sites.join(', ')}` : ''})
        </span>
        {expiry && (
          <div className={`text-[10px] ${muted}`} data-testid="thread-handoff-expiry">
            {expiry}
          </div>
        )}
        {typeof handoff.ttlSecs === 'number' && handoff.ttlSecs > 0 && (
          <div className={`text-[10px] ${muted}`} data-testid="thread-handoff-ttl">
            the copy is kept {ttlWords(handoff.ttlSecs)} after it was sent, then destroyed
          </div>
        )}
        {consent && (
          <div className={`text-[10px] ${muted}`} data-testid="thread-handoff-consent">
            {consent}
          </div>
        )}
      </div>
      {(handoff.note ?? '').trim() && (
        <div data-testid="thread-handoff-note" className="whitespace-pre-wrap break-words">
          <span className={muted}>Note: </span>&lsquo;{(handoff.note ?? '').trim()}&rsquo;
        </div>
      )}
      {remoteHost && (
        <div className={`text-[11px] ${muted}`} data-testid="thread-handoff-remote">
          Open {remoteHost} directly to accept this.
        </div>
      )}
      {!remoteHost && clash && (
        <div className={`text-[10px] ${muted}`} data-testid="thread-handoff-clash">
          {handoff.workspace || 'This agent'} already has a secret named {handoff.name}. Accept it under another name, or
          replace the current one with this as a new version.
        </div>
      )}
      {!remoteHost && renaming && (
        <div className="flex gap-1" data-testid="thread-handoff-rename">
          <input
            data-testid="thread-handoff-as"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            placeholder={`${handoff.name}_${agent.toUpperCase().replace(/[^A-Z0-9]/g, '_')}`}
            aria-label="Accept as"
            className={`flex-1 min-w-0 bg-transparent ${text} outline-none border ${border} px-2 py-0.5 font-mono`}
          />
          <button
            type="button"
            data-testid="thread-handoff-as-save"
            disabled={!nameOk || busy}
            onClick={() => void decide({ action: 'accept', as: newName.trim() })}
            className={`px-2 py-0.5 border ${border} disabled:opacity-40 cursor-pointer disabled:cursor-not-allowed`}
          >
            Accept as
          </button>
        </div>
      )}
      {!remoteHost && (
      <label className={`flex items-center gap-1 cursor-pointer ${muted}`}>
        <input
          type="checkbox"
          data-testid="thread-handoff-always"
          checked={always}
          onChange={(e) => setAlways(e.target.checked)}
        />
        Always accept from {agent}
      </label>
      )}
      {error && (
        <div className={`text-[11px] ${danger}`} role="alert" data-testid="thread-handoff-error">
          {error}
        </div>
      )}
      {!remoteHost && (
      <div className="flex flex-wrap justify-end gap-2">
        <button
          type="button"
          data-testid="thread-handoff-decline"
          disabled={busy}
          onClick={() => void decide({ action: 'decline' })}
          className={`px-2 py-1 text-[11px] ${muted} cursor-pointer`}
        >
          Decline
        </button>
        {!renaming && (
          <button
            type="button"
            data-testid="thread-handoff-accept-as"
            disabled={busy}
            onClick={() => setRenaming(true)}
            className={`px-2 py-1 text-[11px] border ${border} cursor-pointer`}
          >
            Accept as…
          </button>
        )}
        {clash ? (
          <button
            type="button"
            data-testid="thread-handoff-replace"
            disabled={busy}
            onClick={() => void decide({ action: 'accept', replace: true })}
            className={`px-2 py-1 text-[11px] border ${border} cursor-pointer`}
          >
            Replace
          </button>
        ) : (
          <button
            type="button"
            data-testid="thread-handoff-accept"
            disabled={busy}
            onClick={() => void decide({ action: 'accept' })}
            className={`px-2 py-1 text-[11px] border ${border} cursor-pointer`}
          >
            Accept
          </button>
        )}
      </div>
      )}
    </div>
  )
}
