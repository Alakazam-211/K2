// The chat's "this session can't start" view when the folder is the reason
// (Rosson 2026-11-09): the Thread card can't be reached when the agent can't
// be opened, so the decision shows where the session would be, instead of
// "Chat session ended / Retry". It is Settings' folder-trust row in another
// place: same status, same three choices, same daemon decision. The daemon
// says who may decide (`can_decide`, its own check), and decides.

import { useEffect, useRef } from 'react'
import { Button, Callout } from '@/components/ui'
import {
  FOLDER_TRUST_CHOICES,
  FolderTrustFindings,
  folderTrustLine,
  useFolderTrust,
  type FolderTrustControls,
  type FolderTrustState,
} from '@/components/Settings/sections/FolderTrustRow'
import type { ServerScope } from '@/kessel/server-scope'

/** States that keep the gate shut: waiting for a person, or "Don't trust". */
const HOLDING: readonly FolderTrustState[] = ['pending', 'changed', 'undecided', 'denied']

export interface FolderTrustHold {
  trust: FolderTrustControls
  /** The folder is why this session can't start. */
  held: boolean
  /** The first read is out: show neither the panel nor the failure yet. */
  checking: boolean
}

/** Reads the folder while `enabled` (the session isn't running). When a held
 *  folder stops being held (a choice here, the card, Settings, the CLI),
 *  `onCleared` starts the session. A folder that was never held calls
 *  nothing: a chat that simply ended stays ended until someone retries. */
export function useFolderTrustHold(scope: ServerScope, projectPath: string, enabled: boolean, onCleared: () => void): FolderTrustHold {
  const trust = useFolderTrust(scope, projectPath, { enabled, pollDenied: true })
  const { status, loaded } = trust
  const held = enabled && status !== null && HOLDING.includes(status.state)
  const wasHeld = useRef(false)
  const onClearedRef = useRef(onCleared)
  onClearedRef.current = onCleared
  useEffect(() => {
    if (held) {
      wasHeld.current = true
      return
    }
    if (!enabled) {
      wasHeld.current = false
      return
    }
    if (wasHeld.current && status !== null) {
      wasHeld.current = false
      onClearedRef.current()
    }
  }, [enabled, held, status])
  return { trust, held, checking: enabled && !loaded }
}

export function FolderTrustHoldPanel({ hold, inline = false }: { hold: FolderTrustHold; inline?: boolean }): React.JSX.Element | null {
  const { status, busy, notice, error, decide, reset } = hold.trust
  if (!status) return null
  const canDecide = status.can_decide === true
  const denied = status.state === 'denied'
  const title = denied
    ? "K2 won't start this chat in this folder"
    : canDecide
      ? "This chat can't start until you decide about its folder"
      : "This chat can't start until the owner trusts its folder"
  const line = denied
    ? `${folderTrustLine(status)}.${canDecide ? '' : " Only this server's owner can change that."}`
    : canDecide
      ? 'K2 found settings in this folder that run commands when the agent starts:'
      : "Only this server's owner can decide. The chat starts on its own once they do."
  return (
    <div
      data-testid="folder-trust-hold"
      // Thread view: a strip above the messages, clear of the floating header.
      className={
        inline
          ? 'flex-shrink-0 max-h-[50%] overflow-y-auto border-b border-[var(--color-border)] px-4 py-3'
          : 'flex-1 min-h-0 overflow-y-auto flex px-6 py-6'
      }
      style={inline ? { marginTop: 'var(--thread-float-inset, 0px)' } : undefined}
    >
      <div className={inline ? 'w-full' : 'm-auto w-full max-w-[56ch]'}>
        <div className="type-name text-[var(--color-text-primary)]">{title}</div>
        <div className="type-hint text-[var(--color-text-muted)] mt-1 leading-relaxed" data-testid="folder-trust-hold-line">
          {line}
        </div>
        {canDecide ? <FolderTrustFindings status={status} /> : null}
        {canDecide && !denied ? (
          <div className="flex flex-wrap gap-1.5 mt-3" data-testid="folder-trust-hold-choices">
            {FOLDER_TRUST_CHOICES.map((c) => (
              <Button key={c.decision} variant="quiet" onClick={() => void decide(c.decision)} disabled={busy}>
                {c.label}
              </Button>
            ))}
          </div>
        ) : null}
        {canDecide && denied ? (
          <div className="flex flex-wrap gap-1.5 mt-3">
            <Button variant="quiet" onClick={() => void reset()} disabled={busy}>
              Change this decision
            </Button>
          </div>
        ) : null}
        {notice ? <div className="type-hint text-[var(--color-text-secondary)] mt-2 leading-relaxed">{notice}</div> : null}
        {error ? (
          <Callout tone="error" role="alert" className="mt-2">
            {error}
          </Callout>
        ) : null}
      </div>
    </div>
  )
}
