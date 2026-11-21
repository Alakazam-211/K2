// Settings → Email Link: the in-place sign-in an existing Google or
// Microsoft link goes through again. Two things use it: Add calendar (the
// link is consented for more) and Reconnect (the same features, after the
// provider refused the saved sign-in). Either way the daemon keeps the
// inbox's id, its workspace access and its levels; only the saved tokens
// and the consent change.
//
// Microsoft answers a device code; Google a consent page opened in the
// system browser, its redirect caught by the desktop's loopback listener
// and relayed to the daemon. Tokens never reach the renderer.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { openUrl } from '@tauri-apps/plugin-opener'
import { Button, Spinner } from '@/components/ui'
import { useToastStore } from '@/stores/toast'
import { PRODUCT_NAME } from '@/lib/brand'
import { SETTING_ROW_HINT } from '../controls/SettingControls'
import {
  linkOauthComplete,
  linkOauthStatus,
  linkOauthUpgrade,
  MailReplyError,
  mailErrorMessage,
  type Inbox,
  type LinkFeatures,
  type OauthLinkStatus,
} from './email-api'

export const OAUTH_POLL_MS = 3000
export const OAUTH_MAX_MS = 10 * 60 * 1000 // hard bound if the provider gives none

/** Copy with a short "Copied" on the button itself. */
export function CopyButton({ text, label = 'Copy' }: { text: string; label?: string }): React.JSX.Element {
  const [copied, setCopied] = useState(false)
  return (
    <Button
      variant="quiet"
      className="flex-shrink-0"
      onClick={() => {
        void navigator.clipboard
          .writeText(text)
          .then(() => {
            setCopied(true)
            window.setTimeout(() => setCopied(false), 1200)
          })
          .catch(() => {
            useToastStore.getState().addToast(`${PRODUCT_NAME} couldn’t copy to the clipboard`, 'error')
          })
      }}
    >
      {copied ? 'Copied' : label}
    </Button>
  )
}

/** The plain line and the server's reason for a failed read. A reply the
 *  app couldn't read gets its own line; the raw parser text never shows. */
export function readFailure(err: unknown): { line: string; detail: string | null } {
  if (err instanceof MailReplyError) {
    return {
      line: `This server sent an answer ${PRODUCT_NAME} couldn’t read. Try again; if it keeps happening, update ${PRODUCT_NAME} on the server.`,
      detail: null,
    }
  }
  return { line: `${PRODUCT_NAME} couldn’t reach this server’s email settings. Try again in a moment.`, detail: mailErrorMessage(err) }
}

/** One line for a step that failed: the server's words, or the plain line
 *  for a reply the app couldn't read. */
export function failureText(err: unknown): string {
  return err instanceof MailReplyError ? readFailure(err).line : mailErrorMessage(err)
}

/** Binds the desktop's loopback listener for a Google sign-in. Throws a
 *  plain error when this computer can't (the web build, an old app). */
export async function bindLoopback(): Promise<{ captureId: string; redirectUri: string }> {
  const bind = await invoke<{ captureId?: unknown; redirectUri?: unknown } | null>('oauth_loopback_bind')
  if (!bind || typeof bind.captureId !== 'string' || typeof bind.redirectUri !== 'string') {
    throw new Error(`${PRODUCT_NAME} couldn’t open a Google sign-in window on this computer. Use the ${PRODUCT_NAME} desktop app, or update it.`)
  }
  return { captureId: bind.captureId, redirectUri: bind.redirectUri }
}

export type UpgradeFlow =
  | { kind: 'device'; linkId: string; userCode: string; verificationUrl: string; deadlineMs: number }
  | { kind: 'browser'; linkId: string; authorizationUrl: string; deadlineMs: number }

/** How a sign-in ended without connecting. */
export type ConsentEnd =
  /** The daemon's verdict: denied, expired or error (with its code). */
  | { kind: 'status'; status: OauthLinkStatus }
  /** Nobody finished it before the deadline. */
  | { kind: 'timeout' }
  /** A step here failed; `text` says why. */
  | { kind: 'failed'; text: string }

/** Drives one in-place sign-in of `inbox` for `features`: start it, poll
 *  the daemon until it ends, and cancel it. `onConnected` runs once the
 *  daemon has the new sign-in; `onEnded` when it ended any other way. */
export function useConsentUpgrade({
  inbox,
  features,
  pollMs,
  onConnected,
  onEnded,
}: {
  inbox: Inbox
  features: LinkFeatures
  pollMs: number
  onConnected: () => void
  onEnded: (end: ConsentEnd) => void
}): { flow: UpgradeFlow | null; busy: boolean; start: () => Promise<void>; cancel: () => void } {
  const [busy, setBusy] = useState(false)
  const [flow, setFlow] = useState<UpgradeFlow | null>(null)
  const onConnectedRef = useRef(onConnected)
  onConnectedRef.current = onConnected
  const onEndedRef = useRef(onEnded)
  onEndedRef.current = onEnded
  // Google: the loopback capture waiting for the consent redirect.
  const captureRef = useRef<string | null>(null)
  const isGoogle = inbox.capabilities?.source === 'google'
  const address = inbox.address

  const cancelCapture = useCallback((): void => {
    const cap = captureRef.current
    captureRef.current = null
    if (cap) void invoke('oauth_loopback_cancel', { captureId: cap }).catch(() => undefined)
  }, [])
  useEffect(() => cancelCapture, [cancelCapture])

  const startGoogle = useCallback(async (): Promise<void> => {
    const bind = await bindLoopback()
    captureRef.current = bind.captureId
    try {
      const res = await linkOauthUpgrade({ inbox: address, features, clientCapture: true, redirectUri: bind.redirectUri })
      if (res.flow !== 'loopback') {
        throw new Error(`${PRODUCT_NAME} could not start a Google consent page. Try again.`)
      }
      if (!res.authorizationUrl || !res.state) {
        throw new Error(res.hint ?? `${PRODUCT_NAME} could not start a Google consent page. Try again.`)
      }
      const { linkId, authorizationUrl } = res
      const oauthState = res.state
      setFlow({ kind: 'browser', linkId, authorizationUrl, deadlineMs: Date.now() + OAUTH_MAX_MS })
      void openUrl(authorizationUrl).catch(() => undefined)
      // Relay the redirect's code in the background; the status poll
      // drives the UI.
      void (async () => {
        try {
          const capture = await invoke<{ code?: string | null; state?: string | null; error?: string | null } | null>(
            'oauth_loopback_wait',
            { captureId: bind.captureId, expectedState: oauthState, timeoutSecs: 300 },
          )
          if (captureRef.current !== bind.captureId) return
          await linkOauthComplete({
            linkId,
            code: capture?.code ?? undefined,
            state: capture?.state ?? oauthState,
            error: capture?.error ?? undefined,
          })
        } catch (e) {
          if (captureRef.current !== bind.captureId) return
          onEndedRef.current({ kind: 'failed', text: mailErrorMessage(e) })
          setFlow(null)
        } finally {
          if (captureRef.current === bind.captureId) captureRef.current = null
        }
      })()
    } catch (e) {
      cancelCapture()
      throw e
    }
  }, [address, features, cancelCapture])

  const start = useCallback(async (): Promise<void> => {
    setBusy(true)
    try {
      if (isGoogle) {
        await startGoogle()
        return
      }
      const res = await linkOauthUpgrade({ inbox: address, features })
      if (res.flow === 'device') {
        const bound = Number.isFinite(res.expiresIn) && res.expiresIn > 0 ? res.expiresIn * 1000 : OAUTH_MAX_MS
        setFlow({
          kind: 'device',
          linkId: res.linkId,
          userCode: res.userCode,
          verificationUrl: res.verificationUrl,
          deadlineMs: Date.now() + Math.min(bound, OAUTH_MAX_MS),
        })
      } else {
        onEndedRef.current({ kind: 'failed', text: res.hint ?? `${PRODUCT_NAME} could not start a Microsoft device sign-in. Try again.` })
      }
    } catch (e) {
      onEndedRef.current({ kind: 'failed', text: failureText(e) })
    } finally {
      setBusy(false)
    }
  }, [address, features, isGoogle, startGoogle])

  const cancel = useCallback((): void => {
    cancelCapture()
    setFlow(null)
  }, [cancelCapture])

  useEffect(() => {
    if (!flow) return
    let cancelled = false
    const tick = async (): Promise<void> => {
      if (cancelled) return
      if (Date.now() > flow.deadlineMs) {
        onEndedRef.current({ kind: 'timeout' })
        setFlow(null)
        cancelCapture()
        return
      }
      try {
        const st = await linkOauthStatus(flow.linkId)
        if (cancelled) return
        if (st.state === 'connected') {
          setFlow(null)
          onConnectedRef.current()
        } else if (st.state === 'denied' || st.state === 'expired' || st.state === 'error') {
          onEndedRef.current({ kind: 'status', status: st })
          setFlow(null)
          cancelCapture()
        }
      } catch (e) {
        if (cancelled) return
        onEndedRef.current({ kind: 'failed', text: failureText(e) })
        setFlow(null)
      }
    }
    const id = window.setInterval(() => void tick(), pollMs)
    void tick()
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [flow, pollMs, cancelCapture])

  return { flow, busy, start, cancel }
}

/** A sign-in that is waiting on the person: Google's page in their
 *  browser, or Microsoft's code to enter. `lead` says what to do on
 *  Google's page; `waiting` is the line beside the spinner. */
export function ConsentFlowView({
  flow,
  address,
  lead,
  waiting,
  testIdPrefix,
  onCancel,
  className = 'px-4 py-3',
}: {
  flow: UpgradeFlow
  address: string
  lead: React.ReactNode
  waiting: string
  testIdPrefix: string
  onCancel?: () => void
  className?: string
}): React.JSX.Element {
  const cancel = onCancel ? (
    <Button variant="quiet" className="flex-shrink-0" onClick={onCancel}>
      Cancel
    </Button>
  ) : null
  const spinner = (
    <span className="flex items-center gap-2 type-hint text-[var(--color-text-secondary)]">
      <Spinner />
      {waiting}
    </span>
  )
  if (flow.kind === 'browser') {
    return (
      <div className={`${className} space-y-2`} data-testid={`${testIdPrefix}-browser`} role="status">
        <p className={SETTING_ROW_HINT}>{lead}</p>
        <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-2">
          {spinner}
          <div className="flex items-center gap-2 flex-shrink-0">
            <CopyButton text={flow.authorizationUrl} label="Copy page link" />
            <Button variant="quiet" onClick={() => void openUrl(flow.authorizationUrl)}>
              Open Google’s page again
            </Button>
            {cancel}
          </div>
        </div>
      </div>
    )
  }
  return (
    <div className={`${className} space-y-2`} data-testid={`${testIdPrefix}-device`} role="status">
      <p className={SETTING_ROW_HINT}>
        Go to <span className="font-mono">{flow.verificationUrl}</span> and enter this code, signed in as {address}:
      </p>
      <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-2">
        <span className="type-title font-mono tracking-widest text-[var(--color-text-primary)] select-all">{flow.userCode}</span>
        <div className="flex items-center gap-2 flex-shrink-0">
          <CopyButton text={flow.userCode} label="Copy code" />
          <Button variant="quiet" onClick={() => void openUrl(flow.verificationUrl)}>
            Open sign-in page
          </Button>
        </div>
      </div>
      <div className="flex items-center justify-between gap-6">
        {spinner}
        {cancel}
      </div>
    </div>
  )
}
