// Settings → Email Hosting → a mailbox → Forwarding
// (prd-hostmail-external-forwarding-v1 §4.2, FW4).
//
// One mailbox's forward: destination, keep a copy, state, the confirm
// link's state and expiry, the last 24 hours, the audit events, and the
// gestures (set / change, pause, resume, remove). It only calls the routes
// (`mail-forward-api.ts`); every rule lives in the daemon.
//
// WHO IS LOOKING decides the door, and the daemon decides who that is: the
// staff route (`/cli/mail/forward`, owner / Admin) first; when it refuses,
// the person route (`/cli/mail/self/forward`, the mailbox's own linked
// person). Neither → a plain "not yours to manage" line. Resume is offered
// only where it will work (`resumeRule` mirrors the daemon's check).
//
// STYLES: built from the ui primitives (Surface, Button, Input,
// SquareCheckbox, Callout) and theme tokens only, so Square (chamfered),
// Glass and Bezel restyle it through the style contract.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { Button, Callout, Input, SquareCheckbox, Surface } from '@/components/ui'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useToastStore } from '@/stores/toast'
import type { ServerScope } from '@/kessel/server-scope'
import { mailErrorMessage } from './email-api'
import {
  eventWords,
  forwardApiOn,
  isStaffRefusal,
  resumeRule,
  stateLabel,
  type Forward,
  type ForwardShow,
  type ForwardViewer,
} from './mail-forward-api'

const muted = 'text-[var(--color-text-muted)]'

function fmt(unix: number | null | undefined): string {
  return typeof unix === 'number' && unix > 0 ? new Date(unix * 1000).toLocaleString() : '—'
}

function Chip({ text, tone }: { text: string; tone: 'ok' | 'warn' | 'error' | 'info' | 'muted' }): React.JSX.Element {
  const color = {
    ok: 'var(--color-status-ok)',
    warn: 'var(--color-status-warn)',
    error: 'var(--color-status-error-soft)',
    info: 'var(--color-accent)',
    muted: 'var(--color-text-muted)',
  }[tone]
  return (
    <span
      className="text-[9px] font-semibold px-1.5 py-0.5 uppercase tracking-wide border rounded-[var(--radius-field)] flex-shrink-0"
      style={{ color, borderColor: `color-mix(in oklab, ${color} 45%, transparent)` }}
    >
      {text}
    </span>
  )
}

function stateTone(f: Forward): 'ok' | 'warn' | 'error' | 'info' {
  if (f.state === 'active') return 'ok'
  if (f.state === 'pending_verify') return f.verify?.state === 'expired' ? 'warn' : 'info'
  if (f.state === 'paused') return f.pauseReason === 'manual' ? 'warn' : 'error'
  return 'info'
}

/** The set / change form. `keep` is undefined until the viewer touches it,
 *  so the daemon's default applies (outside: keep, same box: no copy). */
function SetForm({
  initialTo,
  initialKeep,
  busy,
  onSubmit,
  onCancel,
}: {
  initialTo: string
  initialKeep: boolean | undefined
  busy: boolean
  onSubmit: (to: string, keep: boolean | undefined) => void
  onCancel?: () => void
}): React.JSX.Element {
  const [to, setTo] = useState(initialTo)
  const [keep, setKeep] = useState<boolean | undefined>(initialKeep)
  const shownKeep = keep ?? true
  return (
    <form
      className="space-y-2"
      onSubmit={(e) => {
        e.preventDefault()
        if (to.trim()) onSubmit(to.trim(), keep)
      }}
    >
      <div className="flex items-center gap-2 min-w-0">
        <Input
          aria-label="Forward to"
          className="flex-1 min-w-0 font-mono"
          placeholder="name@gmail.com, or a mailbox on this server"
          value={to}
          onChange={(e) => setTo(e.target.value)}
          disabled={busy}
        />
        <Button type="submit" variant="accent" disabled={busy || !to.trim()}>
          {busy ? 'Saving…' : 'Forward'}
        </Button>
        {onCancel && (
          <Button variant="ghost" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
        )}
      </div>
      <label className="flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)] cursor-pointer">
        <SquareCheckbox
          aria-label="Keep a copy in this mailbox"
          checked={shownKeep}
          onChange={(e) => setKeep(e.target.checked)}
          disabled={busy}
        />
        Keep a copy in this mailbox
      </label>
      <p className={`text-[10px] ${muted}`}>
        An outside address gets a confirm link first; forwarding starts when it’s clicked (the link works for 24
        hours). Spam, mail failing DMARC and bounces always stay here.
        {keep === undefined && ' A mailbox on this server keeps no copy unless you tick the box.'}
      </p>
    </form>
  )
}

export function MailboxForwardingPanel({
  address,
  canMutate,
  scope,
}: {
  address: string
  canMutate: boolean
  /** The server this page shows (default: the window's server). */
  scope?: ServerScope
}): React.JSX.Element {
  const api = useMemo(() => forwardApiOn(scope), [scope])
  const [viewer, setViewer] = useState<ForwardViewer | null>(null)
  const [data, setData] = useState<ForwardShow | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [denied, setDenied] = useState(false)
  const [busy, setBusy] = useState(false)
  const [editing, setEditing] = useState(false)
  const [showEvents, setShowEvents] = useState(false)
  const [revision, setRevision] = useState(0)
  const reload = useCallback(() => setRevision((r) => r + 1), [])

  // Load through the door the daemon lets this viewer use.
  useEffect(() => {
    let cancelled = false
    const load = async (): Promise<void> => {
      try {
        const d = await api.show(viewer ?? 'staff', address)
        if (cancelled) return
        setData(d)
        setViewer(viewer ?? 'staff')
        setError(null)
        return
      } catch (e) {
        if (cancelled) return
        if (viewer !== null || !isStaffRefusal(e)) {
          setError(mailErrorMessage(e))
          return
        }
      }
      try {
        const d = await api.show('person', address)
        if (cancelled) return
        setData(d)
        setViewer('person')
        setError(null)
      } catch {
        if (!cancelled) setDenied(true)
      }
    }
    void load()
    return () => {
      cancelled = true
    }
    // `viewer` is settled on the first load; later loads reuse it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, address, revision])

  const act = useCallback(
    async (fn: () => Promise<{ note?: string | null; message?: string }>, done?: string): Promise<boolean> => {
      setBusy(true)
      try {
        const r = await fn()
        const toast = useToastStore.getState()
        if (r.note) toast.addToast(r.note, 'info')
        else if (done) toast.addToast(done, 'success')
        reload()
        return true
      } catch (e) {
        useToastStore.getState().addToast(mailErrorMessage(e), 'error')
        return false
      } finally {
        setBusy(false)
      }
    },
    [reload],
  )

  const f = data?.forward ?? null
  const door: ForwardViewer = viewer ?? 'staff'

  const onSet = useCallback(
    async (to: string, keep: boolean | undefined): Promise<void> => {
      setBusy(true)
      try {
        const r = await api.set(door, address, to, keep)
        const toast = useToastStore.getState()
        if (r.state === 'pending_verify') {
          toast.addToast(`A confirm link was sent to ${r.to}; the forward starts once it’s clicked.`, 'info')
        } else {
          toast.addToast(`${address} now forwards to ${r.to}.`, 'success')
        }
        if (r.note) toast.addToast(r.note, 'info')
        setEditing(false)
        reload()
      } catch (e) {
        useToastStore.getState().addToast(mailErrorMessage(e), 'error')
      } finally {
        setBusy(false)
      }
    },
    [api, door, address, reload],
  )

  const onRemove = useCallback(async (): Promise<void> => {
    if (!f) return
    const ok = await useConfirmDialogStore.getState().confirm({
      title: `Stop forwarding ${address}?`,
      message: `New mail stays in ${address} and no longer goes to ${f.to}. You can set a forward again later (an outside address confirms again).`,
      confirmLabel: 'Remove forward',
      destructive: true,
    })
    if (!ok) return
    await act(() => api.unset(door, address), 'Forward removed.')
  }, [f, address, act, api, door])

  if (denied) {
    return (
      <p className={`text-[11px] ${muted}`} data-testid="forward-denied">
        Forwarding for this mailbox is managed by the owner, an Admin, or the person it’s linked to.
      </p>
    )
  }
  if (error) {
    return <Callout tone="error">Couldn’t load forwarding: {error}</Callout>
  }
  if (!data) {
    return <p className={`text-[11px] ${muted}`}>Loading forwarding…</p>
  }

  const rule = f && f.state === 'paused' ? resumeRule(door, f.pauseReason) : null
  const counts = f?.last24h
  const showCounts = !!f && (f.kind === 'external' || !!(counts && (counts.forwarded || counts.refused)))
  const events = data.events ?? []

  return (
    <Surface role2="inset" className="p-3 space-y-2 min-w-0" data-testid="mailbox-forwarding">
      <div className="flex items-center gap-2 min-w-0">
        <h4 className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]">Forwarding</h4>
        {door === 'person' && <span className={`text-[10px] ${muted}`}>your mailbox</span>}
      </div>

      {!f ? (
        canMutate ? (
          <>
            <p className={`text-[11px] ${muted}`}>Not forwarded. New mail stays in this mailbox.</p>
            <SetForm initialTo="" initialKeep={undefined} busy={busy} onSubmit={(to, keep) => void onSet(to, keep)} />
          </>
        ) : (
          <p className={`text-[11px] ${muted}`}>Not forwarded.</p>
        )
      ) : (
        <>
          <div className="flex items-center gap-2 min-w-0 flex-wrap">
            <span className={`text-[11px] ${muted}`}>To</span>
            <span className="text-xs font-mono text-[var(--color-text-primary)] truncate" data-testid="forward-dest">
              {f.to}
            </span>
            <Chip text={f.kind === 'external' ? 'Outside' : 'This server'} tone="muted" />
            <Chip text={stateLabel(f)} tone={stateTone(f)} />
            <span className={`text-[10px] ${muted}`}>{f.keep ? 'Keeps a copy here' : 'No copy kept here'}</span>
          </div>

          {f.state === 'pending_verify' &&
            (f.verify?.state === 'expired' ? (
              <Callout tone="warn">The confirm link to {f.to} expired. Set the forward again to send a new one.</Callout>
            ) : (
              <Callout tone="info" data-testid="forward-pending">
                A confirm link was sent to {f.to}; the forward starts once it’s clicked
                {f.verify?.expiresAt ? ` (the link expires ${fmt(f.verify.expiresAt)})` : ''}.
              </Callout>
            ))}

          {f.state === 'paused' && (
            <Callout tone={f.pauseReason === 'manual' ? 'warn' : 'error'} data-testid="forward-paused">
              Paused{f.pausedAt ? ` ${fmt(f.pausedAt)}` : ''}: {f.pauseWords ?? f.pauseReason}. New mail stays in this
              mailbox.{rule?.why ? ` ${rule.why}` : ''}
            </Callout>
          )}

          {f.state === 'active' && f.kind === 'external' && f.verify?.verifiedAt && (
            <p className={`text-[10px] ${muted}`}>Confirmed by the destination {fmt(f.verify.verifiedAt)}.</p>
          )}

          {showCounts && counts && (
            <dl className="grid grid-cols-3 gap-2 text-[11px]" data-testid="forward-counts">
              {(
                [
                  ['Forwarded', counts.forwarded],
                  ['Delivered', counts.delivered],
                  ['Refused', counts.refused],
                ] as const
              ).map(([label, n]) => (
                <div key={label} className="min-w-0">
                  <dt className={`text-[10px] ${muted}`}>{label} · 24 h</dt>
                  <dd
                    className={`font-mono ${
                      label === 'Refused' && n > 0 ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-text-primary)]'
                    }`}
                  >
                    {n ?? 0}
                  </dd>
                </div>
              ))}
            </dl>
          )}

          {editing ? (
            <SetForm
              initialTo={f.to}
              initialKeep={f.keep}
              busy={busy}
              onSubmit={(to, keep) => void onSet(to, keep)}
              onCancel={() => setEditing(false)}
            />
          ) : (
            canMutate && (
              <div className="flex items-center gap-2 flex-wrap">
                {f.state === 'paused' && rule?.can && (
                  <Button variant="accent" disabled={busy} onClick={() => void act(() => api.resume(door, address), 'Forward resumed.')}>
                    Resume
                  </Button>
                )}
                {f.state === 'active' && (
                  <Button variant="quiet" disabled={busy} onClick={() => void act(() => api.pause(door, address), 'Forward paused.')}>
                    Pause
                  </Button>
                )}
                <Button variant="quiet" disabled={busy} onClick={() => setEditing(true)}>
                  {f.state === 'paused' && f.pauseReason === 'revoked' ? 'Set again…' : 'Change…'}
                </Button>
                <Button
                  variant="ghost"
                  disabled={busy}
                  className="text-[var(--color-status-error-soft)] hover:text-[var(--color-status-error-soft)]"
                  onClick={() => void onRemove()}
                >
                  Remove
                </Button>
              </div>
            )
          )}
          {f.setBy && <p className={`text-[10px] ${muted}`}>Set by {f.setBy} · {fmt(f.createdAt)}</p>}
        </>
      )}

      {events.length > 0 && (
        <div className="pt-1 border-t border-[var(--color-border)]">
          <button
            type="button"
            className={`text-[10px] ${muted} hover:text-[var(--color-text-primary)] cursor-pointer`}
            aria-expanded={showEvents}
            onClick={() => setShowEvents((v) => !v)}
          >
            {showEvents ? '▾' : '▸'} History ({events.length})
          </button>
          {showEvents && (
            <ul className="mt-1 space-y-0.5" data-testid="forward-events">
              {events.map((e) => (
                <li key={e.id} className="flex items-center gap-2 text-[10px] min-w-0">
                  <span className={`${muted} font-mono flex-shrink-0`}>{fmt(e.at)}</span>
                  <span className="text-[var(--color-text-secondary)] truncate">{eventWords(e)}</span>
                  <span className={`${muted} ml-auto flex-shrink-0`}>{e.actor}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </Surface>
  )
}
