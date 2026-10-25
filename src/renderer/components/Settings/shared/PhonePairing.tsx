// Phone pairing (phone-web-off, 2026-10-24; places per Rosson the same day):
//
//   - Settings → K2 Companion (`mode="all"`): the owner's view. "Pair a
//     phone" says which login the code pairs as (the owner's linked login
//     by default); with the owner token the owner may pick another login
//     (F2: owner token only, the daemon sends `pairAs.choices`). Lists every
//     paired phone with Revoke.
//   - The profile modal (`mode="mine"`): each signed-in person pairs their
//     own phone and sees and revokes their own phones. A Connect login types
//     its password first (F1); the local owner key does not (the daemon says
//     which with `pairAs.needsPassword`).
//
// Everything shown comes from the daemon (daemon-first): who a code pairs
// as, the choices, the password need, the link and every refusal hint. The
// QR is drawn here from the daemon's `pairUrl` (uqr, as the Companion
// section's store QRs). The code is valid for its 10 minutes; the panel
// counts down to the daemon's `expiresAt` and drops it after. Device names
// come from phones and are rendered as text only.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { renderSVG } from 'uqr'
import type { ServerScope } from '@/kessel/server-scope'
import {
  fetchPairedPhones,
  makePairCode,
  revokePairedPhone,
  secondsLeft,
  type DaemonError,
  type DevicesView,
  type PairCode,
} from './phone-pairing-api'

/** The QR for the daemon's pairing link (dark on white, 4-module border). */
export function PairQr({ url }: { url: string }): React.JSX.Element {
  const svg = useMemo(() => renderSVG(url, { border: 4, ecc: 'M', blackColor: '#000000', whiteColor: '#ffffff' }), [url])
  return (
    <div
      data-pair-qr=""
      role="img"
      aria-label="QR code that pairs the K2 phone app"
      className="bg-white border border-[var(--color-border)] [&>svg]:block [&>svg]:h-full [&>svg]:w-full"
      style={{ width: 176, height: 176 }}
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  )
}

function fmtWhen(ts: string): string {
  const d = new Date(ts)
  return Number.isNaN(d.getTime()) ? ts || '—' : d.toLocaleString()
}

function fmtLeft(secs: number): string {
  const m = Math.floor(secs / 60)
  const s = secs % 60
  return `${m}:${String(s).padStart(2, '0')}`
}

const defaultNow = (): number => Date.now()

export type PhonePairingMode = 'all' | 'mine'

export function PhonePairing({
  mode,
  scope,
  nowMs = defaultNow,
}: {
  mode: PhonePairingMode
  /** The server to talk to (the profile modal's window server); defaults
   *  to the primary one. */
  scope?: ServerScope
  /** Test seam for the countdown. */
  nowMs?: () => number
}): React.JSX.Element {
  const [view, setView] = useState<DevicesView | null>(null)
  const [loadError, setLoadError] = useState<DaemonError | null>(null)
  const [code, setCode] = useState<PairCode | null>(null)
  const [codeError, setCodeError] = useState<DaemonError | null>(null)
  const [password, setPassword] = useState('')
  const [picked, setPicked] = useState<string>('')
  const [busy, setBusy] = useState(false)
  const [revoking, setRevoking] = useState<string | null>(null)
  const [now, setNow] = useState(nowMs())

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

  useEffect(() => {
    if (!code) return
    const t = setInterval(() => setNow(nowMs()), 1000)
    return () => clearInterval(t)
  }, [code, nowMs])
  const left = code ? secondsLeft(code.expiresAt, now) : 0
  useEffect(() => {
    if (code && left === 0) setCode(null)
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

  const revoke = async (id: string): Promise<void> => {
    setRevoking(id)
    try {
      const r = await revokePairedPhone(id, scope)
      if (!r.ok) setLoadError(r.error)
      await reload()
    } finally {
      setRevoking(null)
    }
  }

  const canPair = target !== '' && !busy && (!needsPassword || password !== '')

  return (
    <div className="space-y-3" data-phone-pairing={mode}>
      {loadError && (
        <p className="m-0 text-[11px] text-[var(--color-status-error-soft)]" data-pair-error="">
          {loadError.hint}
        </p>
      )}

      {pairAs && !code && (
        <div className="space-y-2" data-pair-as="">
          {choices ? (
            <label className="flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)]">
              Pairs as
              <select
                aria-label="Pairs as"
                className="bg-transparent border border-[var(--color-border)] px-2 py-1 text-[11px] no-drag cursor-pointer"
                value={target}
                onChange={(e) => setPicked(e.target.value)}
              >
                {!pairAs.user && <option value="">Choose a login…</option>}
                {choices.map((c) => (
                  <option key={c} value={c}>
                    {c}
                    {c === pairAs.user ? ' (your linked login)' : ''}
                  </option>
                ))}
              </select>
            </label>
          ) : pairAs.user ? (
            <p className="m-0 text-[11px] text-[var(--color-text-secondary)]">
              A new code pairs as <span className="font-mono">{pairAs.user}</span>.
            </p>
          ) : null}
          {!pairAs.user && pairAs.error && (
            <p className="m-0 text-[11px] text-[var(--color-text-muted)]" data-pair-as-missing="">
              {pairAs.error.hint}
            </p>
          )}
          {target !== '' && needsPassword && (
            <label className="flex items-center gap-2 text-[11px] text-[var(--color-text-muted)]">
              Your password
              <input
                type="password"
                autoComplete="current-password"
                className="flex-1 min-w-0 bg-transparent border border-[var(--color-border)] px-2 py-1 text-[11px]"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
            </label>
          )}
          {(target !== '' || choices) && (
            <button
              type="button"
              className="px-3 py-1.5 text-[11px] border border-[var(--color-border)] hover:bg-[var(--color-bg-hover)] no-drag cursor-pointer disabled:opacity-50"
              disabled={!canPair}
              onClick={() => void pair()}
            >
              {busy ? 'Making a code…' : 'Pair a phone'}
            </button>
          )}
        </div>
      )}

      {codeError && (
        <p className="m-0 text-[11px] text-[var(--color-status-error-soft)]" role="alert" data-pair-code-error="">
          {codeError.hint}
        </p>
      )}

      {code && (
        <div className="border border-[var(--color-border)] p-3 space-y-2" data-pair-code="">
          <p className="m-0 text-[11px] text-[var(--color-text-secondary)]">
            Scan with the phone&apos;s camera, or type the code on the K2 app&apos;s Pair screen. Pairs as{' '}
            <span className="font-mono">{code.user}</span>.
          </p>
          {code.pairUrl && <PairQr url={code.pairUrl} />}
          <p className="m-0 font-mono text-[14px] tracking-wider select-all" data-pair-code-text="">
            {code.code}
          </p>
          {code.pairUrl ? (
            <p className="m-0 font-mono text-[10px] break-all text-[var(--color-text-muted)] select-all">{code.pairUrl}</p>
          ) : (
            <p className="m-0 text-[10px] text-[var(--color-text-muted)]">
              This server has no public address, so there is no link or QR: type the code in the app.
            </p>
          )}
          <div className="flex items-center justify-between text-[10px] text-[var(--color-text-muted)]">
            <span data-pair-countdown="">Works once, for {fmtLeft(left)}</span>
            <button
              type="button"
              className="underline hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
              onClick={() => {
                setCode(null)
                void reload()
              }}
            >
              Done
            </button>
          </div>
        </div>
      )}

      {view && view.devices.length === 0 && (
        <p className="m-0 text-[11px] text-[var(--color-text-muted)]" data-pair-empty="">
          No paired phones.
        </p>
      )}
      {view && view.devices.length > 0 && (
        <ul className="m-0 p-0 list-none divide-y divide-[var(--color-border)] border border-[var(--color-border)]" data-pair-list="">
          {view.devices.map((d) => (
            <li key={d.id} className="flex items-center gap-3 px-2.5 py-2" data-pair-device={d.id}>
              <div className="flex-1 min-w-0">
                <div className="text-[11px] text-[var(--color-text-primary)] truncate">
                  {d.name}
                  {d.current ? ' (this phone)' : ''}
                </div>
                <div className="text-[10px] text-[var(--color-text-muted)] truncate">
                  {mode === 'all' ? `${d.username} · ` : ''}paired {fmtWhen(d.createdAt)} · ends if unused {fmtWhen(d.expiresAt)}
                </div>
              </div>
              <button
                type="button"
                className="px-2 py-1 text-[10px] border border-[var(--color-border)] hover:bg-[var(--color-bg-hover)] no-drag cursor-pointer disabled:opacity-50"
                disabled={revoking === d.id}
                onClick={() => void revoke(d.id)}
              >
                {revoking === d.id ? 'Revoking…' : 'Revoke'}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
