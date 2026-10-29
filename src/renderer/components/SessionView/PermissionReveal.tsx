// The password-reset card's reveal (0.46.6, `apps.user-password-reset`;
// prd-password-reset-card-v1 §5): the password or one-time link the daemon
// handed THIS person in their own answer (`forYou`), shown once.
//
// - It lives in this component's state only: never a store, localStorage,
//   sessionStorage or a log. Unmounting forgets it.
// - Copy puts it on the clipboard; it is wiped 10 s after a copy, when the
//   countdown ends, or on Close, whichever comes first. A copied value is
//   taken off the clipboard on wipe if the clipboard still holds it (the
//   clipboard history of other apps is out of K2's reach: the panel says so).
// - The card and the agent never see it; the card's own line says only who
//   reset it and when.

import { useCallback, useEffect, useRef, useState, type JSX } from 'react'
import type { RevealedSecret } from './permissionCardView'

const TEXT = 'var(--thread-card-text,var(--color-text-primary))'
const TEXT2 = 'var(--thread-card-text-secondary,var(--color-text-secondary))'
const MUTED = 'var(--thread-card-muted,var(--color-text-muted))'
const EDGE = 'var(--thread-card-border,var(--color-border))'
const ACCENT = 'var(--thread-card-accent,var(--color-accent))'

/** Seconds a copied value stays on the card. */
export const AFTER_COPY_SECS = 10

/** `0:58`. */
export function clock(secs: number): string {
  const s = Math.max(0, Math.ceil(secs))
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`
}

function title(r: RevealedSecret): string {
  if (r.kind === 'password') return `New password for ${r.username}`
  if (r.kind === 'code') return `One-time code for ${r.username}`
  return `Set-password link for ${r.username}`
}

function hint(r: RevealedSecret): string {
  if (r.kind === 'password') return `Give it to ${r.username} yourself. The next time they sign in, K2 asks them to choose a new password.`
  if (r.kind === 'code') return `This box has no web address for K2's set-password page yet: give ${r.username} this code with the password they should set.`
  return `Give it to ${r.username} yourself. It works once, for 24 hours; they choose their own password.`
}

async function clearClipboardIfStill(value: string): Promise<void> {
  try {
    const clip = navigator.clipboard
    if (!clip?.readText || !clip.writeText) return
    if ((await clip.readText()) === value) await clip.writeText('')
  } catch {
    // Reading the clipboard can be refused; the value stays wherever the
    // person pasted it, as the panel says.
  }
}

const DATE_NOW = (): number => Date.now()

/** The one-time reveal. `onGone` tells the card to drop it (its state). */
export function PermissionReveal({
  secret,
  onGone,
  now = DATE_NOW,
}: {
  secret: RevealedSecret
  onGone: () => void
  /** For tests. */
  now?: () => number
}): JSX.Element {
  const [deadline, setDeadline] = useState(() => now() + secret.secs * 1000)
  const [left, setLeft] = useState(secret.secs)
  const [copied, setCopied] = useState(false)
  const copiedRef = useRef(false)
  const valueRef = useRef(secret.value)
  const goneRef = useRef(false)

  const gone = useCallback((): void => {
    if (goneRef.current) return
    goneRef.current = true
    if (copiedRef.current) void clearClipboardIfStill(valueRef.current)
    valueRef.current = ''
    onGone()
  }, [onGone])

  useEffect(() => {
    const tick = (): void => {
      const s = (deadline - now()) / 1000
      setLeft(s)
      if (s <= 0) gone()
    }
    tick()
    const t = setInterval(tick, 250)
    return () => clearInterval(t)
  }, [deadline, gone, now])

  // Unmounting (the Thread closes, the card scrolls away and is dropped):
  // forget it, and take it off the clipboard.
  useEffect(
    () => () => {
      if (!goneRef.current && copiedRef.current) void clearClipboardIfStill(valueRef.current)
      valueRef.current = ''
    },
    [],
  )

  const copy = async (): Promise<void> => {
    try {
      await navigator.clipboard.writeText(secret.value)
      copiedRef.current = true
      setCopied(true)
      setDeadline((d) => Math.min(d, now() + AFTER_COPY_SECS * 1000))
    } catch {
      setCopied(false)
    }
  }

  return (
    <div
      data-testid="permission-reveal"
      data-kind={secret.kind}
      className="flex flex-col gap-1 px-2 py-1.5"
      style={{ border: `1px solid ${ACCENT}`, background: `color-mix(in srgb, ${ACCENT} 8%, transparent)` }}
    >
      <div className="text-[11px] font-medium" style={{ color: TEXT }}>
        {title(secret)}
        {secret.app ? <span style={{ color: MUTED }}> ({secret.app})</span> : null}
      </div>
      <div className="flex items-center gap-1.5">
        <code
          data-testid="permission-reveal-value"
          className="select-all break-all px-1.5 py-0.5 font-mono text-[12px]"
          style={{ color: TEXT, border: `1px solid ${EDGE}` }}
        >
          {secret.value}
        </code>
        <button
          type="button"
          data-testid="permission-reveal-copy"
          onClick={() => void copy()}
          className="border px-2 py-0.5 text-[11px] cursor-pointer"
          style={{ borderColor: ACCENT, color: TEXT, background: 'transparent' }}
        >
          {copied ? 'Copied' : 'Copy'}
        </button>
        <button
          type="button"
          data-testid="permission-reveal-close"
          onClick={gone}
          className="border px-2 py-0.5 text-[11px] cursor-pointer"
          style={{ borderColor: EDGE, color: TEXT2, background: 'transparent' }}
        >
          Close
        </button>
      </div>
      <div data-testid="permission-reveal-hint" className="text-[10px]" style={{ color: TEXT2 }}>
        {hint(secret)}
      </div>
      <div data-testid="permission-reveal-clock" className="text-[10px]" style={{ color: MUTED }}>
        Shown once, only to you. Gone in {clock(left)}, or when you close it{copied ? '' : ' or 10 s after you copy it'}. Your clipboard
        may keep a copy. Lost it? Reset them again in Settings → User Access.
      </div>
    </div>
  )
}
