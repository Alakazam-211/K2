// K2's own form for a Postal request that carries a pairing code (SAS v2,
// prd-postal-in-k2-v1 S3): it shows THIS server's six digits, for the
// person to read out to the other side, and takes the code shown on the
// OTHER side's screen. The typed code goes straight to the decide route as
// this person; the daemon checks it and only then signs. Never on a card,
// the Thread or an agent's screen: the agent never sees either code.
//
// Two looks: `settings` (the Postal tab's row: Settings fields and
// buttons) and `card` (inside a Thread permission card: the card's own
// field and buttons, sized to the card).

import { useEffect, useState } from 'react'
import { Button, Input } from '@/components/ui'
import type { ServerScope } from '@/kessel/server-scope'
import { CARD_DANGER_TEXT, CARD_TEXT, CARD_TEXT2, CardButton, CardField } from '@/components/SessionView/cardControls'
import { decidePostalRequest, fetchPostalCode, isPairingCode, postalErrorText, postalPlainError } from './postal-api'

const MEASURE = 'max-w-[var(--measure-text,75ch)]'

/** The code as two groups of three with one small, even gap (a space in
 *  the code face is a whole digit wide). The space stays in the text, for
 *  copying and screen readers. */
function PairCode({ code }: { code: string | null }): React.JSX.Element {
  const digits = (code ?? '').replace(/\D/g, '')
  if (digits.length !== 6) return <span className="font-mono" data-testid="postal-code-mine">{code ?? '…'}</span>
  return (
    <span className="inline-flex gap-1 font-mono" data-testid="postal-code-mine">
      <span>{digits.slice(0, 3)}</span>
      <span className="sr-only"> </span>
      <span>{digits.slice(3)}</span>
    </span>
  )
}

/** Who to read the code to: the owner by name, else the peer's person. */
export function readCodeTo(peer: string, owner: string | null | undefined): string {
  const name = owner?.trim()
  return name ? `${name}, ${peer}’s owner` : `${peer}’s person`
}

export function PostalPairCodeForm({
  scope,
  workspace,
  requestId,
  peer,
  owner = null,
  variant = 'settings',
  onDone,
  onCancel,
}: {
  scope: ServerScope
  workspace: string
  requestId: string
  peer: string
  /** The peer's owner, by name (the friend row's ownerName). */
  owner?: string | null
  variant?: 'settings' | 'card'
  onDone: (result: unknown) => void
  onCancel: () => void
}): React.JSX.Element {
  const [mine, setMine] = useState<string | null>(null)
  const [typed, setTyped] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    let live = true
    fetchPostalCode(scope, workspace, requestId)
      .then((c) => live && setMine(c))
      .catch((e: unknown) => live && setError(postalErrorText(e)))
    return () => {
      live = false
    }
  }, [scope, workspace, requestId])

  const submit = async (): Promise<void> => {
    if (!isPairingCode(typed)) {
      setError('Type the 6 digits shown on their screen.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const r = await decidePostalRequest(scope, workspace, requestId, 'accept', typed.replace(/[\s-]/g, ''))
      setTyped('')
      onDone(r)
    } catch (e) {
      setError(postalPlainError(e))
    } finally {
      setBusy(false)
    }
  }

  const card = variant === 'card'
  const lead = (
    <>
      Your code: <PairCode code={mine} />. Read it to {readCodeTo(peer, owner)}.
    </>
  )
  // The card's own words already say why the codes match; it keeps the consequence.
  const why = card
    ? 'A code that doesn’t match ends the request on both sides.'
    : 'The two codes match only if nobody sits between the two servers. A code that doesn’t match ends the request on both sides.'
  const field = {
    'aria-label': 'The code shown on their screen',
    inputMode: 'numeric' as const,
    autoComplete: 'off',
    value: typed,
    maxLength: 7,
    onChange: (e: React.ChangeEvent<HTMLInputElement>) => setTyped(e.target.value),
    placeholder: 'The code shown on their screen',
  }

  return (
    <form
      data-testid="postal-code-form"
      className={`flex flex-col min-w-0 max-w-full ${card ? 'gap-1' : 'gap-1.5'}`}
      onSubmit={(e) => {
        e.preventDefault()
        void submit()
      }}
    >
      {card ? (
        <>
          <div className="break-words" style={{ color: CARD_TEXT }}>
            {lead}
          </div>
          <CardField testId="postal-code-input" className="max-w-56" {...field} />
          <div className="text-[10px] leading-relaxed break-words" style={{ color: CARD_TEXT2 }}>
            {why}
          </div>
          {error ? (
            <div role="alert" className="text-[10px] break-words" style={{ color: CARD_DANGER_TEXT }} data-testid="postal-code-error">
              {error}
            </div>
          ) : null}
          <div className="flex flex-wrap gap-1.5">
            <CardButton label="Accept" testId="postal-code-accept" type="submit" primary disabled={busy || mine === null} />
            <CardButton label="Back" testId="postal-code-back" disabled={busy} onClick={onCancel} />
          </div>
        </>
      ) : (
        <>
          <div className={`type-hint text-[var(--color-text-primary)] break-words ${MEASURE}`}>{lead}</div>
          <Input data-testid="postal-code-input" className="w-72 max-w-full" {...field} />
          <div className={`type-hint text-[var(--color-text-secondary)] break-words ${MEASURE}`}>{why}</div>
          {error ? (
            <div role="alert" className={`type-hint text-[var(--color-status-error-soft)] break-words ${MEASURE}`} data-testid="postal-code-error">
              {error}
            </div>
          ) : null}
          <div className="flex flex-wrap gap-1.5">
            <Button variant="accent" type="submit" disabled={busy || mine === null}>
              Accept
            </Button>
            <Button variant="quiet" type="button" disabled={busy} onClick={onCancel}>
              Back
            </Button>
          </div>
        </>
      )}
    </form>
  )
}
