// Settings → Email Link: an account whose saved sign-in stopped working.
//
// Google or Microsoft can refuse a saved sign-in (a password change, a
// revoked grant, an expired token). The daemon says so in two places: the
// inbox row's status turns `error` when an agent's read or send failed,
// and the mail watcher marks the inbox `needsRelink` when its sign-in was
// refused (it then stops trying until the sign-in changes). Either way the
// fix is the same: sign in to the same link again with the features it
// already has. The daemon keeps the inbox's id, its workspace access and
// its calendar setting, and replaces only the saved tokens. Nothing here
// disconnects anything.
//
// An app-password account can't be signed in again this way; the page
// says what to do in words instead.

import React, { useEffect, useRef, useState } from 'react'
import { openUrl } from '@tauri-apps/plugin-opener'
import { Button, Callout } from '@/components/ui'
import { PRODUCT_NAME } from '@/lib/brand'
import type { Inbox, LinkFeatures } from './email-api'
import { fetchMailWatch } from './mail-watch-api'
import { ConsentFlowView, OAUTH_POLL_MS, useConsentUpgrade, type ConsentEnd } from './email-link-consent'

/** Refused inboxes, as the mail watcher last saw them: address → the time
 *  of its last check (so a sign-in made after that check can be told from
 *  a refusal that came after it). */
export type Relink = Map<string, string | null>

/** Why an account can't connect: `refused` = the provider refused the
 *  saved sign-in; `error` = the last check failed. Null = it's fine. */
export type Refusal = 'refused' | 'error' | null

/** What happened after a sign-in went through, once the lists are read
 *  again: `clear` = nothing refuses it now; `waiting` = the watcher hasn't
 *  checked since; `refused` = it checked and was refused again. */
export type AfterReconnect = 'clear' | 'waiting' | 'refused'

export function providerWord(inbox: Inbox): 'Google' | 'Microsoft' {
  return inbox.capabilities?.source === 'google' ? 'Google' : 'Microsoft'
}

const FEATURE_WORDS: readonly LinkFeatures[] = ['mail', 'calendar-read', 'calendar', 'mail,calendar-read', 'mail,calendar']

/** What to sign in for again: exactly what the link has now, so nothing is
 *  added or dropped. Null for an account that isn't a Google or Microsoft
 *  sign-in. */
export function reconnectFeatures(inbox: Inbox): LinkFeatures | null {
  const c = inbox.capabilities
  if (!c || (c.source !== 'google' && c.source !== 'microsoft')) return null
  const stored = c.account?.features
  if (stored) {
    const words = stored.split(',').map((w) => w.trim())
    const mail = words.includes('mail')
    const cal = words.includes('calendar') ? 'calendar' : words.includes('calendar-read') ? 'calendar-read' : null
    const joined = [mail ? 'mail' : null, cal].filter(Boolean).join(',')
    const known = FEATURE_WORDS.find((f) => f === joined)
    if (known) return known
  }
  // An older daemon leaves the stored features out: read them off what the
  // link can do (a calendar that's off for any reason isn't asked for).
  const missing = c.missing ?? []
  const mailOff = missing.some((m) => m.cap === 'mail.read' && m.reason === 'scope_not_granted')
  const calOn = c.calendar?.read === true
  const calWrite = c.calendar?.write === true
  const cal = calOn ? (calWrite ? 'calendar' : 'calendar-read') : null
  if (mailOff && cal) return cal
  if (!mailOff && cal) return cal === 'calendar' ? 'mail,calendar' : 'mail,calendar-read'
  return 'mail'
}

/** Reads which linked inboxes the watcher saw refused. The watcher's state
 *  is per inbox; it's read through the workspace that manages each one.
 *  A read that fails leaves its inboxes out (the row status still shows
 *  a failed check). */
export async function fetchRelink(rows: Inbox[]): Promise<Relink> {
  const wanted = new Set(rows.filter((r) => r.source === 'linked').map((r) => r.address))
  const projects = [...new Set(rows.filter((r) => r.source === 'linked').map((r) => r.primary.projectId))]
  const out: Relink = new Map()
  const reads = await Promise.allSettled(projects.map((p) => fetchMailWatch(p)))
  for (const r of reads) {
    if (r.status !== 'fulfilled' || !Array.isArray(r.value?.inboxes)) continue
    for (const i of r.value.inboxes) {
      if (i.source === 'linked' && i.needsRelink === true && wanted.has(i.address)) out.set(i.address, i.lastPollAt ?? null)
    }
  }
  return out
}

/** Whether `inbox` can't connect. `reconnected` holds, for each account
 *  signed in again here, the watcher's last check from before: a refusal
 *  stamped with that same check is the old one, not a new one. */
export function refusalOf(inbox: Inbox, relink: Relink, reconnected: Relink): Refusal {
  const seen = relink.get(inbox.address)
  const stale = relink.has(inbox.address) && reconnected.has(inbox.address) && reconnected.get(inbox.address) === seen
  if (relink.has(inbox.address) && !stale) return 'refused'
  if (inbox.status === 'error') return 'error'
  return null
}

type Failure = { text: string; adminConsentUrl?: string }

/** Words for a sign-in that ended without reconnecting, or null when the
 *  person closed or declined it (back to the notice, nothing to say). */
function endFailure(end: ConsentEnd, address: string): Failure | null {
  if (end.kind === 'failed') return { text: end.text }
  if (end.kind === 'timeout') return { text: 'The sign-in took too long and expired.' }
  const st = end.status
  if (st.state === 'denied') return null
  if (st.state === 'expired') return { text: 'The sign-in took too long and expired.' }
  if (st.code === 'account_mismatch') return { text: `That’s a different account. Sign in as ${address}.` }
  if (st.code === 'admin_consent_required') {
    return { text: `Your Microsoft 365 admin needs to approve ${PRODUCT_NAME} first.`, adminConsentUrl: st.adminConsentUrl }
  }
  if (st.code === 'scope_not_granted') return { text: 'Mail access wasn’t allowed on the sign-in page. Allow it when you sign in.' }
  return { text: st.hint ? `The sign-in didn’t finish: ${st.hint.replace(/[.\s]+$/, '')}.` : 'The sign-in didn’t finish.' }
}

/** The notice at the top of a refused account's page: what happened, the
 *  Reconnect button, the sign-in it starts, and how it went. */
export function ReconnectNotice({
  inbox,
  refusal,
  canMutate,
  autoStart = false,
  onAutoStarted,
  onReconnected,
  pollMs = OAUTH_POLL_MS,
}: {
  inbox: Inbox
  refusal: Refusal
  canMutate: boolean
  /** Start the sign-in as soon as the page opens (the list's Sign in again). */
  autoStart?: boolean
  onAutoStarted?: () => void
  /** Read the accounts and the watcher again once the sign-in went through. */
  onReconnected: (address: string) => Promise<AfterReconnect>
  pollMs?: number
}): React.JSX.Element | null {
  const features = reconnectFeatures(inbox)
  const provider = providerWord(inbox)
  const [failure, setFailure] = useState<Failure | null>(null)
  const [after, setAfter] = useState<'checking' | 'clear' | 'waiting' | null>(null)
  const onReconnectedRef = useRef(onReconnected)
  onReconnectedRef.current = onReconnected

  const { flow, busy, start, cancel } = useConsentUpgrade({
    inbox,
    features: features ?? 'mail',
    pollMs,
    onConnected: () => {
      setAfter('checking')
      void onReconnectedRef
        .current(inbox.address)
        .catch((): AfterReconnect => 'waiting')
        .then((r) => setAfter(r === 'refused' ? null : r))
    },
    onEnded: (end) => setFailure(endFailure(end, inbox.address)),
  })

  const begin = (): void => {
    setFailure(null)
    setAfter(null)
    void start()
  }

  // The list's Sign in again opens this page and starts at once.
  const autoStarted = useRef(false)
  useEffect(() => {
    if (!autoStart || autoStarted.current) return
    autoStarted.current = true
    onAutoStarted?.()
    if (features && refusal && canMutate) begin()
    // Once, when the page opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoStart])

  if (!features) return null

  if (after) {
    return (
      <Callout tone="ok" role="status" className="type-hint flex items-center justify-between gap-4" data-testid="reconnect-done">
        <span className="min-w-0">
          {after === 'checking'
            ? 'Reconnected. Checking mail…'
            : after === 'waiting'
              ? 'Signed in. Mail checks resume on the next check.'
              : 'Reconnected. Agents can use this account again.'}
        </span>
        {after !== 'checking' && (
          <Button variant="quiet" className="flex-shrink-0" onClick={() => setAfter(null)}>
            Dismiss
          </Button>
        )}
      </Callout>
    )
  }

  if (!refusal) return null
  const hint = `Sign in to ${provider} again`
  return (
    <Callout tone="warn" role="status" className="type-hint space-y-3" data-testid="reconnect-notice">
      <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
        <div className="min-w-0 space-y-0.5">
          <p className="font-medium">Can’t connect to {inbox.address}</p>
          <p className="text-[var(--color-text-secondary)] leading-relaxed max-w-[75ch]">
            {refusal === 'refused'
              ? `${provider} refused the saved sign-in.`
              : `${PRODUCT_NAME} couldn’t sign in to this account the last time it checked.`}{' '}
            Sign in again to reconnect. Workspace access and the calendar setting stay as they are.
          </p>
        </div>
        {!flow && (
          <Button
            variant="accent"
            className="flex-shrink-0"
            disabled={!canMutate || busy}
            title={canMutate ? hint : 'Not available while you view this server'}
            onClick={begin}
          >
            {busy ? 'Starting…' : failure ? 'Try again' : 'Reconnect'}
          </Button>
        )}
      </div>
      {failure && !flow && (
        <div className="space-y-2" role="alert" data-testid="reconnect-error">
          <p className="text-[var(--color-status-error-soft)] leading-relaxed max-w-[75ch]">{failure.text}</p>
          {failure.adminConsentUrl && (
            <Button variant="quiet" onClick={() => void openUrl(failure.adminConsentUrl as string)}>
              Open the admin consent page
            </Button>
          )}
        </div>
      )}
      {flow && (
        <ConsentFlowView
          flow={flow}
          address={inbox.address}
          testIdPrefix="reconnect"
          className=""
          lead={`Sign in as ${inbox.address} on Google’s page. It opened in your browser.`}
          waiting="Waiting for you to sign in…"
          onCancel={cancel}
        />
      )}
    </Callout>
  )
}
