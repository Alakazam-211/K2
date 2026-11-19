// K2's own form for a Postal request that carries a pairing code (SAS v2,
// prd-postal-in-k2-v1 S3): it shows THIS server's six digits, for the
// person to read out to the other side, and takes the code shown on the
// OTHER side's screen. The typed code goes straight to the decide route as
// this person; the daemon checks it and only then signs. Never on a card,
// the Thread or an agent's screen: the agent never sees either code.

import { useEffect, useState } from 'react'
import { Button, Input } from '@/components/ui'
import type { ServerScope } from '@/kessel/server-scope'
import { decidePostalRequest, fetchPostalCode, isPairingCode, postalErrorText } from './postal-api'

export function PostalPairCodeForm({
  scope,
  workspace,
  requestId,
  peer,
  onDone,
  onCancel,
}: {
  scope: ServerScope
  workspace: string
  requestId: string
  peer: string
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
      setError(postalErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <form
      data-testid="postal-code-form"
      className="flex flex-col gap-1.5"
      onSubmit={(e) => {
        e.preventDefault()
        void submit()
      }}
    >
      <div className="type-hint text-[var(--color-text-primary)]">
        Your code: <span className="font-mono" data-testid="postal-code-mine">{mine ?? '…'}</span>. Read it to {peer}’s person.
      </div>
      <Input
        data-testid="postal-code-input"
        aria-label="The code shown on their screen"
        inputMode="numeric"
        autoComplete="off"
        value={typed}
        maxLength={7}
        onChange={(e) => setTyped(e.target.value)}
        placeholder="The code shown on their screen"
      />
      <div className="type-hint text-[var(--color-text-secondary)]">
        The two codes match only if nobody sits between the two servers. A code that doesn’t match ends the request on both sides.
      </div>
      {error ? (
        <div className="type-hint text-[var(--color-status-warn-text)]" data-testid="postal-code-error">
          {error}
        </div>
      ) : null}
      <div className="flex gap-1.5">
        <Button variant="accent" type="submit" disabled={busy || mine === null}>
          Accept
        </Button>
        <Button variant="quiet" type="button" disabled={busy} onClick={onCancel}>
          Back
        </Button>
      </div>
    </form>
  )
}
