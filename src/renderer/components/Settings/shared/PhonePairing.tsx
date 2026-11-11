// Phone pairing (phone-web-off, 2026-10-24; places per Rosson the same day):
//
//   - Settings → K2 Companion (`mode="all"`): the owner's view, as two
//     Settings cards. "Pair a phone" says which login the code pairs as
//     (the owner's linked login by default); with the owner token the owner
//     may pick another login (F2: owner token only, the daemon sends
//     `pairAs.choices`). "Paired phones" lists every paired phone with
//     Remove…, which asks first in the shared Settings dialog.
//   - The profile modal (`mode="mine"`): each signed-in person pairs their
//     own phone and sees and removes their own phones, in a compact layout
//     that fits the modal (remove asks first in the row). A Connect login
//     types its password first (F1); the local owner key does not (the
//     daemon says which with `pairAs.needsPassword`).
//
// Everything shown comes from the daemon (daemon-first): who a code pairs
// as, the choices, the password need, the link and every refusal hint. The
// QR is drawn here from the daemon's `pairUrl` (uqr, as the Companion
// section's store QRs). The code is valid for its 10 minutes; the panel
// counts down to the daemon's `expiresAt`. Device names come from phones
// and are rendered as text only.
//
// 0.46.6 pair success (Rosson): when the phone redeems the code the daemon
// sends `pair_code_changed` to the minter's sockets; the panel swaps the QR
// for a success state ("Paired iPhone as rosson", Done) and the phones list
// adds the phone at once, highlighted. If the code runs out unused (the
// frame, or the countdown) the panel says so and offers a new code. While
// a code shows, `GET …/pair-code/status` is polled as the fallback for a
// window whose events socket is down.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { renderSVG } from 'uqr'
import type { ServerScope } from '@/kessel/server-scope'
import { onPairCodeChanged } from '@/stores/session-events'
import { Button, Callout, Input } from '@/components/ui'
import {
  SETTING_FIELD_WIDTH,
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
  SettingDropdown,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
} from '../controls/SettingControls'
import { AccessSheet, DIALOG_MUTED, FailedNote } from '../sections/AccessDialogs'
import {
  fetchPairCodeStatus,
  fetchPairedPhones,
  makePairCode,
  revokePairedPhone,
  secondsLeft,
  type DaemonError,
  type DevicesView,
  type PairCode,
  type PairCodeStatus,
  type PairedPhone,
} from './phone-pairing-api'

/** How often the status fallback is read while a code shows. */
export const PAIR_STATUS_POLL_MS = 3000
/** How long a newly paired phone's row stays highlighted. */
export const NEW_PHONE_HIGHLIGHT_MS = 10_000

/** What the panel shows after a code finished. */
type Outcome =
  | { kind: 'paired'; deviceName: string; user: string }
  | { kind: 'expired' }
  | { kind: 'void' }

function CheckMark({ size = 20 }: { size?: number }): React.JSX.Element {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true" className="flex-shrink-0 text-[var(--color-status-ok)]">
      <circle cx="12" cy="12" r="11" fill="none" stroke="currentColor" strokeWidth="1.5" />
      <path d="M7 12.5l3.2 3.2L17 9" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  )
}

/** A QR code, dark on white whatever the theme (a phone camera reads it
 *  best that way). Its quiet zone is part of the drawing. */
export function QrCode({
  url,
  size = 176,
  label,
  border = 4,
  testId,
}: {
  url: string
  size?: number
  label: string
  border?: number
  testId?: string
}): React.JSX.Element {
  const svg = useMemo(() => renderSVG(url, { border, ecc: 'M', blackColor: 'black', whiteColor: 'white' }), [url, border])
  return (
    <div
      data-qr={testId}
      role="img"
      aria-label={label}
      className="flex-shrink-0 border border-[var(--color-border)] [&>svg]:block [&>svg]:h-full [&>svg]:w-full"
      style={{ width: size, height: size }}
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  )
}

/** The QR for the daemon's pairing link. */
export function PairQr({ url }: { url: string }): React.JSX.Element {
  return (
    <div data-pair-qr="">
      <QrCode url={url} label="QR code that pairs the K2 phone app" />
    </div>
  )
}

function fmtDay(ts: string): string {
  const d = new Date(ts)
  return Number.isNaN(d.getTime()) ? ts || '—' : d.toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' })
}

function fmtLeft(secs: number): string {
  const m = Math.floor(secs / 60)
  const s = secs % 60
  return `${m}:${String(s).padStart(2, '0')}`
}

const defaultNow = (): number => Date.now()

export type PhonePairingMode = 'all' | 'mine'

/** The panel's state and actions, shared by both layouts. */
function usePhonePairing(mode: PhonePairingMode, scope: ServerScope, nowMs: () => number) {
  const [view, setView] = useState<DevicesView | null>(null)
  const [loadError, setLoadError] = useState<DaemonError | null>(null)
  const [code, setCode] = useState<PairCode | null>(null)
  const [codeError, setCodeError] = useState<DaemonError | null>(null)
  const [password, setPassword] = useState('')
  const [picked, setPicked] = useState<string>('')
  const [busy, setBusy] = useState(false)
  const [now, setNow] = useState(nowMs())
  const [outcome, setOutcome] = useState<Outcome | null>(null)
  const [highlight, setHighlight] = useState<string | null>(null)

  const reload = useCallback(async () => {
    const r = await fetchPairedPhones({ mine: mode === 'mine', scope })
    if (r.ok) {
      setView(r.value)
      setLoadError(null)
    } else {
      setLoadError(r.error)
    }
  }, [mode, scope])

  useEffect(() => {
    void reload()
  }, [reload])

  // The code on screen, for the event handler (no resubscribe per code).
  const codeRef = useRef<PairCode | null>(null)
  codeRef.current = code

  // A phone paired (this panel's code or another of this person's): add it
  // to the list at once, highlighted, then refetch the real row.
  const addPhone = useCallback(
    (s: PairCodeStatus) => {
      if (!s.sessionId) return
      const id = s.sessionId
      setView((v) => {
        if (!v || v.devices.some((d) => d.id === id)) return v
        if (mode === 'mine' && v.pairAs.user && v.pairAs.user !== s.user) return v
        const row = {
          id,
          username: s.user,
          deviceId: '',
          name: s.deviceName ?? 'Phone',
          createdAt: new Date(nowMs()).toISOString(),
          expiresAt: '',
          maxExpiresAt: '',
          pairedBy: null,
          current: false,
        }
        return { ...v, devices: [row, ...v.devices] }
      })
      setHighlight(id)
      void reload()
    },
    [mode, nowMs, reload],
  )

  const settle = useCallback(
    (s: PairCodeStatus) => {
      if (s.state === 'paired') addPhone(s)
      const shown = codeRef.current
      if (!shown || !shown.codeId || shown.codeId !== s.codeId || s.state === 'pending') return
      setCode(null)
      if (s.state === 'paired') {
        setOutcome({ kind: 'paired', deviceName: s.deviceName ?? 'Phone', user: s.user || shown.user })
      } else {
        setOutcome({ kind: s.state })
      }
    },
    [addPhone],
  )

  useEffect(() => onPairCodeChanged(scope, (e) => settle(e)), [scope, settle])

  // The fallback: read the code's status while it shows.
  const codeId = code?.codeId ?? null
  useEffect(() => {
    if (!codeId) return
    let stopped = false
    const t = setInterval(() => {
      void fetchPairCodeStatus(codeId, scope).then((r) => {
        if (!stopped && r.ok) settle(r.value)
      })
    }, PAIR_STATUS_POLL_MS)
    return () => {
      stopped = true
      clearInterval(t)
    }
  }, [codeId, scope, settle])

  useEffect(() => {
    if (!highlight) return
    const t = setTimeout(() => setHighlight(null), NEW_PHONE_HIGHLIGHT_MS)
    return () => clearTimeout(t)
  }, [highlight])

  useEffect(() => {
    if (!code) return
    const t = setInterval(() => setNow(nowMs()), 1000)
    return () => clearInterval(t)
  }, [code, nowMs])
  const left = code ? secondsLeft(code.expiresAt, now) : 0
  useEffect(() => {
    if (code && left === 0) {
      setCode(null)
      setOutcome({ kind: 'expired' })
    }
  }, [code, left])

  const pairAs = view?.pairAs ?? null
  // The owner may pick another login only in the owner's view, and only
  // when the daemon offered choices (the owner token).
  const choices = mode === 'all' ? pairAs?.choices ?? null : null
  const target = choices ? picked || pairAs?.user || '' : pairAs?.user ?? ''
  const needsPassword = pairAs?.needsPassword === true

  const pair = async (): Promise<void> => {
    setBusy(true)
    setCodeError(null)
    setOutcome(null)
    try {
      const sendUser = choices && target && target !== pairAs?.user ? target : undefined
      const r = await makePairCode({ password: needsPassword ? password : undefined, user: sendUser, scope })
      setPassword('')
      if (r.ok) {
        setNow(nowMs())
        setCode(r.value)
      } else {
        setCodeError(r.error)
      }
    } finally {
      setBusy(false)
    }
  }

  /** Remove a phone; the answer says whether it worked. The list is read
   *  again either way. */
  const revoke = async (id: string): Promise<DaemonError | null> => {
    const r = await revokePairedPhone(id, scope)
    await reload()
    return r.ok ? null : r.error
  }

  const hideCode = (): void => {
    setCode(null)
    void reload()
  }

  /** "Make a new code" after one ran out: a login types its password again. */
  const again = (): void => {
    if (needsPassword || target === '') setOutcome(null)
    else void pair()
  }

  const canPair = target !== '' && !busy && (!needsPassword || password !== '')

  return {
    view,
    loadError,
    reload,
    code,
    codeError,
    password,
    setPassword,
    setPicked,
    busy,
    left,
    outcome,
    setOutcome,
    highlight,
    pairAs,
    choices,
    target,
    needsPassword,
    pair,
    revoke,
    hideCode,
    again,
    canPair,
  }
}

type Pairing = ReturnType<typeof usePhonePairing>

export function PhonePairing({
  mode,
  scope,
  nowMs = defaultNow,
}: {
  mode: PhonePairingMode
  /** The server to talk to (Settings: the window's primary server; the
   *  profile modal: the window's own server). */
  scope: ServerScope
  /** Test seam for the countdown. */
  nowMs?: () => number
}): React.JSX.Element {
  const p = usePhonePairing(mode, scope, nowMs)
  return mode === 'all' ? <PairingCards p={p} /> : <PairingCompact p={p} />
}

// ── Settings → K2 Companion: two cards ────────────────────────────────

/** The code on screen: QR, the code, its countdown and the link. */
function CodePanel({ code, left, onHide }: { code: PairCode; left: number; onHide: () => void }): React.JSX.Element {
  return (
    <div className="px-4 py-4" data-pair-code="">
      <div className="flex flex-wrap items-start gap-6">
        {code.pairUrl && <PairQr url={code.pairUrl} />}
        <div className="flex-1 min-w-[16rem] space-y-3">
          <div>
            <p className={SETTING_ROW_LABEL}>Scan this with your phone&apos;s camera</p>
            <p className={SETTING_ROW_HINT}>
              {code.pairUrl
                ? 'Or open the K2 app, tap Pair, and type the code below.'
                : 'This server has no public address yet, so there is no QR. Open the K2 app, tap Pair, and type the code below.'}{' '}
              Pairs as <span className="font-mono">{code.user}</span>.
            </p>
          </div>
          <p className="m-0 font-mono text-[length:var(--type-xl)] tracking-wider select-all text-[var(--color-text-primary)] break-all" data-pair-code-text="">
            {code.code}
          </p>
          <p className="type-hint text-[var(--color-text-secondary)]" data-pair-countdown="" aria-live="off">
            Works once · {fmtLeft(left)} left
          </p>
          {code.pairUrl && (
            <p className="type-hint font-mono break-all text-[var(--color-text-secondary)] select-all">{code.pairUrl}</p>
          )}
          <Button variant="quiet" onClick={onHide}>
            Hide code
          </Button>
        </div>
      </div>
    </div>
  )
}

function PairingCards({ p }: { p: Pairing }): React.JSX.Element {
  const [confirming, setConfirming] = useState<PairedPhone | null>(null)
  const [removing, setRemoving] = useState(false)
  const [removeError, setRemoveError] = useState<DaemonError | null>(null)
  const [removed, setRemoved] = useState<string | null>(null)
  const [retrying, setRetrying] = useState(false)

  const closeConfirm = useCallback(() => {
    setConfirming(null)
    setRemoveError(null)
  }, [])
  const confirmRemove = async (): Promise<void> => {
    if (!confirming) return
    setRemoving(true)
    setRemoveError(null)
    try {
      const err = await p.revoke(confirming.id)
      if (err) {
        setRemoveError(err)
      } else {
        setRemoved(confirming.name || 'The phone')
        setConfirming(null)
      }
    } finally {
      setRemoving(false)
    }
  }
  const retryLoad = async (): Promise<void> => {
    setRetrying(true)
    try {
      await p.reload()
    } finally {
      setRetrying(false)
    }
  }

  const { view, pairAs, code, outcome, choices, target } = p
  const loading = !view && !p.loadError

  // ── Pair a phone ──
  let pairBody: React.ReactNode
  if (code) {
    pairBody = <CodePanel code={code} left={p.left} onHide={p.hideCode} />
  } else if (outcome?.kind === 'paired') {
    pairBody = (
      <div className="px-4 py-3" data-pair-success="" role="status">
        <div className="flex items-center justify-between gap-6">
          <div className="flex items-start gap-2 min-w-0">
            <CheckMark />
            <div className="min-w-0">
              <p className={SETTING_ROW_LABEL} data-pair-success-text="">
                Paired {outcome.deviceName} as <span className="font-mono">{outcome.user}</span>
              </p>
              <p className={SETTING_ROW_HINT}>It&apos;s in Paired phones below and can use K2 now.</p>
            </div>
          </div>
          <Button className="flex-shrink-0" onClick={() => p.setOutcome(null)}>
            Done
          </Button>
        </div>
      </div>
    )
  } else if (outcome) {
    pairBody = (
      <div className="px-4 py-3" data-pair-expired={outcome.kind} role="status">
        <div className="flex items-center justify-between gap-6">
          <div className="min-w-0">
            <p className="type-row text-[var(--color-status-warn-text)]">
              {outcome.kind === 'expired' ? 'This code expired before a phone used it.' : 'This code can no longer be used.'}
            </p>
            <p className={SETTING_ROW_HINT}>
              {outcome.kind === 'expired'
                ? 'Make a new one and scan it within 10 minutes.'
                : 'A newer code replaced it, or the login changed. Make a new one to pair.'}
            </p>
          </div>
          <div className="flex items-center gap-2 flex-shrink-0">
            <Button variant="quiet" onClick={() => p.setOutcome(null)}>
              Close
            </Button>
            <Button variant="accent" disabled={p.busy} onClick={p.again}>
              {p.busy ? 'Making a code…' : 'Make a new code'}
            </Button>
          </div>
        </div>
      </div>
    )
  } else {
    pairBody = (
      <>
        {choices ? (
          <SettingRow
            label="Pairs as"
            hint={
              !pairAs?.user && pairAs?.error ? (
                <span data-pair-as-missing="">{pairAs.error.hint}</span>
              ) : (
                'The login the phone signs in as. It can do what that login can do.'
              )
            }
            field
          >
            <SettingDropdown
              fullWidth
              ariaLabel="Pairs as"
              placeholder={pairAs?.user ? undefined : 'Choose a login…'}
              value={target}
              options={choices.map((c) => ({ value: c, label: c === pairAs?.user ? `${c} (your linked login)` : c }))}
              onChange={p.setPicked}
            />
          </SettingRow>
        ) : pairAs?.user ? (
          <SettingRow label="Pairs as" hint="The login the phone signs in as. It can do what that login can do.">
            <span className="type-row font-mono text-[var(--color-text-primary)]">{pairAs.user}</span>
          </SettingRow>
        ) : pairAs?.error ? (
          <SettingRow label="Pairs as" hint={<span data-pair-as-missing="">{pairAs.error.hint}</span>}>
            {null}
          </SettingRow>
        ) : null}
        {target !== '' && p.needsPassword && (
          <SettingRow label="Your password" hint="Needed for each new code." field>
            <Input
              type="password"
              aria-label="Your password"
              autoComplete="current-password"
              className="w-full type-control"
              value={p.password}
              onChange={(e) => p.setPassword(e.target.value)}
            />
          </SettingRow>
        )}
        <SettingRow
          label="Pairing code"
          hint="A one-time code and QR that sign the K2 app in on your phone. It works once, for 10 minutes."
          below={
            p.codeError ? (
              <Callout tone="error" role="alert" className="type-hint space-y-0.5" data-pair-code-error="">
                <p className="font-medium">Couldn&apos;t make a pairing code.</p>
                <p>The server said: {p.codeError.hint.replace(/[.\s]+$/, '')}. Try again.</p>
              </Callout>
            ) : undefined
          }
        >
          <Button variant="accent" className="flex-shrink-0" disabled={!p.canPair} onClick={() => void p.pair()}>
            {p.busy ? 'Making a code…' : 'Pair a phone'}
          </Button>
        </SettingRow>
      </>
    )
  }

  const pairState = loading ? (
    <SettingsLoadState kind="loading" title="Loading…">
      Checking which login a new code pairs as.
    </SettingsLoadState>
  ) : !view ? (
    <SettingsLoadState
      kind="error"
      title="Couldn't load your paired phones"
      detail={p.loadError?.hint}
      action={
        <Button onClick={() => void retryLoad()} disabled={retrying}>
          {retrying ? 'Trying…' : 'Try again'}
        </Button>
      }
    >
      K2 couldn&apos;t read which phones are paired with this server, so it can&apos;t make a code yet.
    </SettingsLoadState>
  ) : undefined

  // ── Paired phones (left out while the read failed: the card above says so) ──
  const phones = view?.devices ?? []
  const phonesState = loading ? (
    <SettingsLoadState kind="loading" title="Loading paired phones…" />
  ) : phones.length === 0 ? (
    <div data-pair-empty="">
      <SettingsLoadState kind="empty" title="No phones paired yet">
        A phone you pair shows here, and you can remove it any time.
      </SettingsLoadState>
    </div>
  ) : undefined

  return (
    <div className="space-y-6" data-phone-pairing="all">
      <SettingsGroup
        title="Pair a phone"
        hint="Install the app first, then make a code here and scan it with your phone."
        card
        settingId="companion.pair"
        state={pairState}
      >
        {pairBody}
      </SettingsGroup>

      {(loading || view) && (
      <SettingsGroup
        title="Paired phones"
        hint={
          <>
            A phone stays signed in while it&apos;s used. It needs pairing again after 30 days unused, after its
            login&apos;s password changes, or when you remove it.
            {removed && (
              <span role="status" className="flex items-center gap-1 mt-1 text-[var(--color-status-ok-soft)]" data-pair-removed="">
                <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3 flex-shrink-0">
                  <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
                </svg>
                Removed {removed}. It&apos;s signed out of this server.
              </span>
            )}
          </>
        }
        card
        settingId="companion.phones"
        state={phonesState}
      >
        {view && p.loadError && (
          <div className="px-4 py-3">
            <Callout tone="error" role="alert" className="type-hint" data-pair-error="">
              Couldn&apos;t refresh the list. The server said: {p.loadError.hint.replace(/[.\s]+$/, '')}.
            </Callout>
          </div>
        )}
        <ul className="m-0 p-0 list-none divide-y divide-[var(--color-border)]" data-pair-list="">
          {phones.map((d) => (
            <li
              key={d.id}
              className={`px-4 py-3 flex items-center justify-between gap-6 transition-colors ${
                p.highlight === d.id ? 'bg-[var(--color-bg-hover)]' : ''
              }`}
              data-pair-device={d.id}
              data-pair-new={p.highlight === d.id ? '' : undefined}
            >
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2 min-w-0">
                  <span className={`${SETTING_ROW_LABEL} truncate`}>{d.name || 'Phone'}</span>
                  {d.current && <span className="type-hint text-[var(--color-text-secondary)] flex-shrink-0">This phone</span>}
                  {p.highlight === d.id && (
                    <span className="inline-flex items-center gap-1 type-hint text-[var(--color-status-ok-soft)] flex-shrink-0">
                      <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3">
                        <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
                      </svg>
                      Just paired
                    </span>
                  )}
                </div>
                <p className={`${SETTING_ROW_HINT} truncate`}>
                  {d.username} · paired {fmtDay(d.createdAt)}
                  {d.expiresAt ? ` · signs out if unused by ${fmtDay(d.expiresAt)}` : ''}
                </p>
              </div>
              <Button
                variant="danger-quiet"
                className="flex-shrink-0"
                aria-label={`Remove ${d.name || 'phone'}…`}
                onClick={() => {
                  setRemoved(null)
                  setRemoveError(null)
                  setConfirming(d)
                }}
              >
                Remove…
              </Button>
            </li>
          ))}
        </ul>
      </SettingsGroup>
      )}

      {confirming && (
        <AccessSheet title={`Remove ${confirming.name || 'this phone'}?`} onClose={closeConfirm} busy={removing} testId="pair-remove-dialog">
          <p className={DIALOG_MUTED}>
            It&apos;s signed out of this server right away. To use K2 on it again, pair it with a new code.
          </p>
          {removeError && (
            <FailedNote what="Couldn't remove this phone." next="It's still paired. Try again." detail={removeError.hint} />
          )}
          <div className="flex justify-end gap-2">
            <Button variant="ghost" size="md" onClick={closeConfirm} disabled={removing} autoFocus>
              Cancel
            </Button>
            <Button variant="danger-muted" size="md" onClick={() => void confirmRemove()} disabled={removing}>
              {removing ? 'Removing…' : 'Remove phone'}
            </Button>
          </div>
        </AccessSheet>
      )}
    </div>
  )
}

// ── Profile modal: compact ────────────────────────────────────────────

function PairingCompact({ p }: { p: Pairing }): React.JSX.Element {
  const [asking, setAsking] = useState<string | null>(null)
  const [removing, setRemoving] = useState<string | null>(null)
  const [removeError, setRemoveError] = useState<DaemonError | null>(null)
  const { view, pairAs, code, outcome, target } = p

  const remove = async (id: string): Promise<void> => {
    setRemoving(id)
    setRemoveError(null)
    try {
      const err = await p.revoke(id)
      if (err) setRemoveError(err)
      else setAsking(null)
    } finally {
      setRemoving(null)
    }
  }

  return (
    <div className="space-y-3" data-phone-pairing="mine">
      {p.loadError && (
        <p className="m-0 type-hint text-[var(--color-status-error-soft)]" data-pair-error="">
          {p.loadError.hint}
        </p>
      )}

      {outcome?.kind === 'paired' && (
        <div className="space-y-2" data-pair-success="" role="status">
          <div className="flex items-center gap-2">
            <CheckMark />
            <p className={`m-0 ${SETTING_ROW_LABEL}`} data-pair-success-text="">
              Paired {outcome.deviceName} as <span className="font-mono">{outcome.user}</span>
            </p>
          </div>
          <Button onClick={() => p.setOutcome(null)}>Done</Button>
        </div>
      )}

      {(outcome?.kind === 'expired' || outcome?.kind === 'void') && (
        <div className="space-y-2" data-pair-expired={outcome.kind} role="status">
          <p className="m-0 type-hint text-[var(--color-text-secondary)]">
            {outcome.kind === 'expired'
              ? 'This code expired before a phone used it.'
              : 'This code can no longer be used: a newer code replaced it, or the login changed.'}
          </p>
          <div className="flex items-center gap-2">
            <Button variant="accent" disabled={p.busy} onClick={p.again}>
              {p.busy ? 'Making a code…' : 'Make a new code'}
            </Button>
            <Button variant="quiet" onClick={() => p.setOutcome(null)}>
              Close
            </Button>
          </div>
        </div>
      )}

      {pairAs && !code && !outcome && (
        <div className="space-y-2" data-pair-as="">
          {pairAs.user ? (
            <p className="m-0 type-hint text-[var(--color-text-secondary)]">
              A new code pairs as <span className="font-mono">{pairAs.user}</span>.
            </p>
          ) : null}
          {!pairAs.user && pairAs.error && (
            <p className="m-0 type-hint text-[var(--color-text-secondary)]" data-pair-as-missing="">
              {pairAs.error.hint}
            </p>
          )}
          {target !== '' && p.needsPassword && (
            <label className="flex items-center gap-2 type-hint text-[var(--color-text-secondary)]">
              Your password
              <Input
                type="password"
                autoComplete="current-password"
                className={`${SETTING_FIELD_WIDTH} type-control`}
                value={p.password}
                onChange={(e) => p.setPassword(e.target.value)}
              />
            </label>
          )}
          {target !== '' && (
            <Button variant="accent" disabled={!p.canPair} onClick={() => void p.pair()}>
              {p.busy ? 'Making a code…' : 'Pair a phone'}
            </Button>
          )}
        </div>
      )}

      {p.codeError && (
        <p className="m-0 type-hint text-[var(--color-status-error-soft)]" role="alert" data-pair-code-error="">
          {p.codeError.hint}
        </p>
      )}

      {code && (
        <div className="space-y-2" data-pair-code="">
          <p className="m-0 type-hint text-[var(--color-text-secondary)]">
            Scan with the phone&apos;s camera, or type the code on the K2 app&apos;s Pair screen. Pairs as{' '}
            <span className="font-mono">{code.user}</span>.
          </p>
          {code.pairUrl && <PairQr url={code.pairUrl} />}
          <p className="m-0 font-mono text-[length:var(--type-xl)] tracking-wider select-all" data-pair-code-text="">
            {code.code}
          </p>
          {code.pairUrl ? (
            <p className="m-0 font-mono type-hint break-all text-[var(--color-text-secondary)] select-all">{code.pairUrl}</p>
          ) : (
            <p className="m-0 type-hint text-[var(--color-text-secondary)]">
              This server has no public address, so there is no link or QR: type the code in the app.
            </p>
          )}
          <div className="flex items-center justify-between gap-3 type-hint text-[var(--color-text-secondary)]">
            <span data-pair-countdown="">Works once · {fmtLeft(p.left)} left</span>
            <Button variant="quiet" onClick={p.hideCode}>
              Hide code
            </Button>
          </div>
        </div>
      )}

      {view && view.devices.length === 0 && (
        <p className="m-0 type-hint text-[var(--color-text-secondary)]" data-pair-empty="">
          No phones paired yet.
        </p>
      )}
      {view && view.devices.length > 0 && (
        <ul className="m-0 p-0 list-none divide-y divide-[var(--color-border)] border-y border-[var(--color-border)]" data-pair-list="">
          {view.devices.map((d) => (
            <li
              key={d.id}
              className={`py-2 transition-colors ${p.highlight === d.id ? 'bg-[var(--color-bg-hover)]' : ''}`}
              data-pair-device={d.id}
              data-pair-new={p.highlight === d.id ? '' : undefined}
            >
              <div className="flex items-center gap-3">
                <div className="flex-1 min-w-0">
                  <div className={`${SETTING_ROW_LABEL} truncate`}>
                    {d.name}
                    {d.current ? ' (this phone)' : ''}
                    {p.highlight === d.id ? ' · just paired' : ''}
                  </div>
                  <div className="type-hint text-[var(--color-text-secondary)] truncate">
                    paired {fmtDay(d.createdAt)}
                    {d.expiresAt ? ` · signs out if unused by ${fmtDay(d.expiresAt)}` : ''}
                  </div>
                </div>
                {asking === d.id ? (
                  <div className="flex items-center gap-2 flex-shrink-0" data-pair-remove-ask="">
                    <span className="type-hint text-[var(--color-text-secondary)]">Remove this phone?</span>
                    <Button variant="quiet" onClick={() => setAsking(null)} disabled={removing === d.id} autoFocus>
                      Cancel
                    </Button>
                    <Button variant="danger-muted" onClick={() => void remove(d.id)} disabled={removing === d.id}>
                      {removing === d.id ? 'Removing…' : 'Remove'}
                    </Button>
                  </div>
                ) : (
                  <Button
                    variant="danger-quiet"
                    className="flex-shrink-0"
                    onClick={() => {
                      setRemoveError(null)
                      setAsking(d.id)
                    }}
                  >
                    Remove…
                  </Button>
                )}
              </div>
              {asking === d.id && removeError && (
                <p className="mt-1 type-hint text-[var(--color-status-error-soft)]" role="alert">
                  Couldn&apos;t remove it: {removeError.hint.replace(/[.\s]+$/, '')}. It&apos;s still paired.
                </p>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
