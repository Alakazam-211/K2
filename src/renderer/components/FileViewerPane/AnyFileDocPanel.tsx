// The any-file panel: the multiplayer side of a file K2 can't co-edit live
// (PSD, CAD, 3D, .blend, images, PDFs...), beside the tab's usual view while
// the "Any other file" core is on. It renders the daemon's tier 0 truth
// (`docs/tier0.rs`) and sends gestures; the daemon decides everything.
//
// There is no live socket for these files, so the panel asks: status and
// who every few seconds while its tab is on screen (asking is also what keeps
// this person in the room; the daemon drops them a minute after they stop),
// versions and approvals when the version moves and now and then.
//
// The lock renews while the tab is open, even hidden: someone working in
// another program must not lose it. Closing the tab lets it run out.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { Button, Callout } from '@/components/ui'
import { formatBytes } from '@/components/Changes/viewer/diffQuery'
import type { ServerScope } from '@/kessel/server-scope'
import { isHostSwitchedError } from '@/lib/daemon-cli'
import { errorMessage, httpCodeOf, httpStatusOf } from '@/lib/daemon-http-error'
import {
  fetchTier0Approvals,
  fetchTier0Status,
  fetchTier0Versions,
  fetchTier0Who,
  postTier0Decision,
  postTier0Lease,
  postTier0Restore,
  type Tier0Approvals,
  type Tier0Person,
  type Tier0Status,
  type Tier0Version,
} from '@/lib/docs-tier0'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { isEffectivelyHidden } from '@/lib/workspace-switch-focus'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

export const POLL_MS = 4_000
/** Versions and approvals at least every this many polls. */
const FULL_EVERY = 4
export const RENEW_MS = 60_000
const MAX_VERSIONS_SHOWN = 50

export interface AnyFileDocPanelProps {
  scope: ServerScope
  /** The file, absolute (the daemon finds its workspace). */
  filePath: string
  /** The toolbar closed the panel: it keeps running, out of sight. */
  hidden?: boolean
  /** The daemon said this file's core is off: the tab asks again. */
  onCoreOff: () => void
}

const muted = 'text-[var(--color-text-muted)]'

/** "about 4m left", from the daemon's expiry. */
export function leftText(expiresMs: number, nowMs: number): string {
  const secs = Math.max(0, Math.round((expiresMs - nowMs) / 1000))
  if (secs < 60) return 'under a minute left'
  return `about ${Math.round(secs / 60)}m left`
}

/** A person's name from a pid: the roster's if they're here, else the key. */
export function pidName(pid: string, people: Tier0Person[]): string {
  return people.find((p) => p.pid === pid)?.name ?? pid.replace(/^(person|agent):/, '')
}

function Section({ title, children }: { title: string; children: React.ReactNode }): React.JSX.Element {
  return (
    <section className="border-b border-[var(--color-border)] px-3 py-2">
      <h3 className={`mb-1 text-[10px] font-semibold uppercase tracking-wider ${muted}`}>{title}</h3>
      {children}
    </section>
  )
}

function Empty({ children }: { children: React.ReactNode }): React.JSX.Element {
  return <p className={`text-[11px] ${muted}`}>{children}</p>
}

export function AnyFileDocPanel({ scope, filePath, hidden = false, onCoreOff }: AnyFileDocPanelProps): React.JSX.Element {
  const root = useRef<HTMLElement | null>(null)
  const [status, setStatus] = useState<Tier0Status | null>(null)
  const [people, setPeople] = useState<Tier0Person[]>([])
  const [versions, setVersions] = useState<Tier0Version[] | null>(null)
  const [approvals, setApprovals] = useState<Tier0Approvals>({ pending: [], mine: new Set() })
  // The daemon refuses this file (not in a workspace, gone, no rights...).
  const [refused, setRefused] = useState<string | null>(null)
  // It can't be reached right now; what's on screen stays.
  const [trouble, setTrouble] = useState<string | null>(null)
  const [lost, setLost] = useState<{ by: string | null } | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)

  const onCoreOffRef = useRef(onCoreOff)
  onCoreOffRef.current = onCoreOff
  // One read at a time. `epoch` moves around every gesture: a read that
  // started before one is stale and must not be taken for the truth (a read
  // from before "Take" would look like a lost lock).
  const run = useRef({ alive: true, reading: false, again: false, againFull: false, epoch: 0, version: -1, polls: 0, held: false })

  const failed = useCallback((e: unknown): void => {
    if (isHostSwitchedError(e)) return
    if (httpCodeOf(e) === 'core_off') {
      onCoreOffRef.current()
      return
    }
    const code = httpStatusOf(e)
    if (code === null) setTrouble('Can’t reach this server. Retrying…')
    else if (code >= 500) setTrouble(errorMessage(e))
    else setRefused(errorMessage(e))
  }, [])

  const refresh = useCallback(
    async (full: boolean): Promise<void> => {
      const r = run.current
      if (r.reading) {
        r.again = true
        r.againFull = r.againFull || full
        return
      }
      r.reading = true
      const epoch = r.epoch
      const stale = (): boolean => {
        if (epoch === r.epoch) return false
        r.again = true
        r.againFull = true
        return true
      }
      try {
        const [st, who] = await Promise.all([fetchTier0Status(scope, filePath), fetchTier0Who(scope, filePath)])
        if (!r.alive || stale()) return
        const more = full || st.version !== r.version
        const [vs, ap] = await Promise.all([
          more && st.rights.history ? fetchTier0Versions(scope, filePath) : Promise.resolve(null),
          more ? fetchTier0Approvals(scope, filePath) : Promise.resolve(null),
        ])
        if (!r.alive || stale()) return
        r.version = st.version
        const mineNow = st.lease !== null && st.lease.mine && !st.lease.auto
        if (r.held && !mineNow) setLost({ by: st.lease && !st.lease.mine ? st.lease.name : null })
        if (mineNow) setLost(null)
        r.held = mineNow
        setStatus(st)
        setPeople(who)
        if (more) setVersions(st.rights.history ? vs : null)
        if (ap) setApprovals(ap)
        setRefused(null)
        setTrouble(null)
      } catch (e) {
        if (r.alive) failed(e)
      } finally {
        r.reading = false
        if (r.alive && r.again) {
          const f = r.againFull
          r.again = false
          r.againFull = false
          void refresh(f)
        }
      }
    },
    [scope, filePath, failed],
  )

  useEffect(() => {
    const r = run.current
    r.alive = true
    void refresh(true)
    const t = setInterval(() => {
      // The tab is on screen (the panel's own toggle doesn't count).
      if (document.hidden || isEffectivelyHidden(root.current?.parentElement ?? null)) return
      r.polls += 1
      void refresh(r.polls % FULL_EVERY === 0)
    }, POLL_MS)
    return () => {
      r.alive = false
      clearInterval(t)
    }
  }, [refresh])

  const holding = status?.lease?.mine === true && !status.lease.auto && !scope.viewOnly
  useEffect(() => {
    if (!holding) return
    const t = setInterval(() => {
      postTier0Lease(scope, filePath, 'renew').catch((e: unknown) => {
        const code = httpCodeOf(e)
        // Gone or someone else's: the next read says so. Anything else
        // (a blip) waits for the next try.
        if (code === 'no_lease' || code === 'leased') void refresh(false)
      })
    }, RENEW_MS)
    return () => clearInterval(t)
  }, [holding, scope, filePath, refresh])

  const act = useCallback(
    async (key: string, fn: () => Promise<void>): Promise<void> => {
      const r = run.current
      setBusy(key)
      setActionError(null)
      setNotice(null)
      r.epoch += 1
      try {
        await fn()
      } catch (e) {
        if (!isHostSwitchedError(e)) {
          if (httpCodeOf(e) === 'core_off') onCoreOffRef.current()
          else setActionError(errorMessage(e))
        }
      } finally {
        r.epoch += 1
        setBusy(null)
        if (r.alive) await refresh(true)
      }
    },
    [refresh],
  )

  const take = (): void =>
    void act('lock', async () => {
      await postTier0Lease(scope, filePath, 'acquire')
      run.current.held = true
      setLost(null)
    })

  const release = (): void =>
    void act('lock', async () => {
      await postTier0Lease(scope, filePath, 'release')
      run.current.held = false
    })

  const restore = async (v: Tier0Version): Promise<void> => {
    const yes = await useConfirmDialogStore.getState().confirm({
      title: `Restore version ${v.id}?`,
      message: `Version ${v.id} becomes the newest version of this file. Nothing is deleted: the current version stays in the list and you can restore it later. A program that has the file open needs to reopen it.`,
      confirmLabel: 'Restore',
    })
    if (!yes) return
    await act(`restore:${v.id}`, async () => {
      const res = await postTier0Restore(scope, filePath, v.id)
      if (!res.changed) setNotice(`Version ${v.id} already matches the current file. Nothing changed.`)
      else if (!res.written) setNotice(`Version ${v.id} is restored in history. K2 will write the file as soon as it can.`)
      else setNotice(`Version ${v.id} is now the newest version.`)
    })
  }

  const decide = (change: number, state: 'approved' | 'declined'): void =>
    void act(`approval:${change}`, async () => {
      const d = await postTier0Decision(scope, filePath, change, state)
      if (state === 'approved') {
        setNotice(`Version ${change} approved.`)
      } else if (d.restoreTo === null) {
        setNotice(`Version ${change} declined. There is no earlier version to restore.`)
      } else {
        const also = d.later.length > 0 ? ` That also replaces ${d.later.map((v) => `version ${v}`).join(', ')}.` : ''
        setNotice(`Version ${change} declined. Nothing was restored for you. To undo it, restore version ${d.restoreTo}.${also}`)
      }
    })

  const canEdit = status?.rights.edit === true && !scope.viewOnly
  const lease = status?.lease ?? null
  const heldByOther = lease !== null && !lease.mine
  const now = Date.now()
  const nowSec = now / 1000
  const newestFirst = versions ? [...versions].sort((a, b) => b.id - a.id) : null
  const copies = status?.conflictCopies ?? null

  return (
    <aside
      ref={root}
      className={
        hidden
          ? 'hidden'
          : 'flex w-72 shrink-0 flex-col overflow-y-auto border-l border-[var(--color-border)] bg-[var(--color-bg-stripe)] text-xs text-[var(--color-text-primary)]'
      }
      aria-label="Multiplayer"
      data-testid="any-file-panel"
    >
      {refused ? (
        <div className={`p-3 text-[11px] ${muted}`} role="status" data-testid="any-file-refused">
          {refused}
        </div>
      ) : status === null ? (
        <div className={`p-3 text-[11px] ${muted}`} data-testid="any-file-loading">
          {trouble ?? 'Loading…'}
        </div>
      ) : (
        <>
          {trouble ? (
            <Callout tone="warn" className="m-2" role="status" data-testid="any-file-trouble">
              {trouble}
            </Callout>
          ) : null}
          {lost ? (
            <Callout tone="warn" className="m-2 space-y-1" role="alert" data-testid="any-file-lost">
              <div>{lost.by ? `You lost the lock. ${lost.by} holds it now.` : 'You lost the lock: it expired or someone released it.'}</div>
              <Button variant="quiet" onClick={() => setLost(null)}>
                Dismiss
              </Button>
            </Callout>
          ) : null}
          {actionError ? (
            <Callout tone="error" className="m-2" role="alert" data-testid="any-file-error">
              {actionError}
            </Callout>
          ) : null}
          {notice ? (
            <Callout tone="info" className="m-2" role="status" data-testid="any-file-notice">
              {notice}
            </Callout>
          ) : null}

          <Section title="Who’s here">
            {people.length === 0 ? (
              <Empty>Nobody’s here right now.</Empty>
            ) : (
              <ul className="space-y-1" data-testid="any-file-who">
                {people.map((p) => (
                  <li key={p.pid} className="flex min-w-0 items-center gap-1.5">
                    <span
                      className="inline-block h-2 w-2 shrink-0 rounded-[var(--radius-pill)] bg-[var(--color-accent)]"
                      style={p.colour ? { backgroundColor: p.colour } : undefined}
                      aria-hidden
                    />
                    <span className="truncate">{p.name}</span>
                    {p.kind === 'agent' ? <span className={`text-[10px] ${muted}`}>agent</span> : null}
                    {p.away ? <span className={`text-[10px] ${muted}`}>away</span> : null}
                    {p.region ? <span className={`truncate text-[10px] ${muted}`}>{p.region}</span> : null}
                  </li>
                ))}
              </ul>
            )}
          </Section>

          <Section title="Lock">
            <div data-testid="any-file-lock" data-holder={lease ? (lease.mine ? 'me' : 'other') : 'none'}>
              {lease === null ? (
                <Empty>Nobody holds the lock.</Empty>
              ) : lease.mine ? (
                <p className="text-[11px]">You hold the lock. It stays yours while this tab is open.</p>
              ) : (
                <p className="text-[11px]">
                  {lease.name} holds the lock, {leftText(lease.expiresMs, now)}.
                </p>
              )}
              {lease === null && canEdit ? (
                <Button variant="accent" className="mt-1" disabled={busy !== null} onClick={take}>
                  Take the lock
                </Button>
              ) : null}
              {lease?.mine && !scope.viewOnly ? (
                <Button variant="quiet" className="mt-1" disabled={busy !== null} onClick={release}>
                  Release
                </Button>
              ) : null}
            </div>
          </Section>

          <Section title="Versions">
            {newestFirst === null ? (
              <Empty>You can’t see this file’s versions.</Empty>
            ) : newestFirst.length === 0 ? (
              <Empty>No versions yet.</Empty>
            ) : (
              <ul className="space-y-1.5" data-testid="any-file-versions">
                {newestFirst.slice(0, MAX_VERSIONS_SHOWN).map((v) => (
                  <li key={v.id} className="flex items-start gap-2" data-testid={`any-file-version-${v.id}`}>
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-[11px]">
                        Version {v.id}
                        {v.name ? ` · ${v.name}` : ''}
                        {v.current ? <span className="text-[var(--color-accent)]"> · current</span> : null}
                      </div>
                      <div className={`text-[10px] ${muted}`}>
                        {v.authorLabel} · {formatRelativeTime(v.atMs / 1000, nowSec)} · {formatBytes(v.size)}
                        {v.restoredFrom !== null ? ` · from version ${v.restoredFrom}` : ''}
                        {v.redacted ? ' · content removed' : ''}
                      </div>
                    </div>
                    {!v.current && !v.redacted && canEdit && status.rights.history ? (
                      <Button
                        variant="quiet"
                        disabled={busy !== null || heldByOther}
                        title={heldByOther ? `${lease?.name} holds the lock` : undefined}
                        onClick={() => void restore(v)}
                      >
                        Restore
                      </Button>
                    ) : null}
                  </li>
                ))}
                {newestFirst.length > MAX_VERSIONS_SHOWN ? (
                  <li className={`text-[10px] ${muted}`}>{newestFirst.length - MAX_VERSIONS_SHOWN} older, not shown.</li>
                ) : null}
              </ul>
            )}
          </Section>

          <Section title="Conflict copies">
            {copies === null ? (
              <Empty>You can’t see this file’s conflict copies.</Empty>
            ) : copies.length === 0 ? (
              <Empty>No conflict copies.</Empty>
            ) : (
              <ul className="space-y-1" data-testid="any-file-copies">
                {copies.map((c) => (
                  <li key={c.path}>
                    <div className="truncate font-mono text-[10px]" title={c.path}>
                      {c.path}
                    </div>
                    <div className={`text-[10px] ${muted}`}>
                      Holds version {c.kept} · {formatRelativeTime(c.atMs / 1000, nowSec)}
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </Section>

          <Section title="Approvals">
            {approvals.pending.length === 0 ? (
              <Empty>Nothing waiting for approval.</Empty>
            ) : (
              <ul className="space-y-1.5" data-testid="any-file-approvals">
                {approvals.pending.map((a) => {
                  const mine = approvals.mine.has(a.change)
                  const by = versions?.find((v) => v.id === a.change)?.authorLabel ?? pidName(a.author, people)
                  return (
                    <li key={a.change} data-testid={`any-file-approval-${a.change}`}>
                      <div className="text-[11px]">
                        Version {a.change} by {by}, {mine ? 'for you' : `waiting for ${pidName(a.onBehalfOf, people)}`}
                      </div>
                      {mine && !scope.viewOnly ? (
                        <div className="mt-1 flex gap-1">
                          <Button variant="accent" disabled={busy !== null} onClick={() => decide(a.change, 'approved')}>
                            Approve
                          </Button>
                          <Button variant="quiet" disabled={busy !== null} onClick={() => decide(a.change, 'declined')}>
                            Decline
                          </Button>
                        </div>
                      ) : null}
                    </li>
                  )
                })}
              </ul>
            )}
          </Section>

          {scope.viewOnly ? (
            <p className={`px-3 py-2 text-[11px] ${muted}`} data-testid="any-file-view-only">
              This room is view-only.
            </p>
          ) : null}
        </>
      )}
    </aside>
  )
}
