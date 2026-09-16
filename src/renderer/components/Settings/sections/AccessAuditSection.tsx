// Settings → Logs → Access Audit. Thin owner-only table over
// GET /cli/users/audit on the active host. No last-ok column on Users /
// Access; no daemon schema. See prd-login-history-settings-v1.md.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { useConnectHostStore } from '@/stores/connect-host'
import { useServerSupports } from '@/lib/server-capabilities'
import type { SettingEntry } from '../searchManifest'
import {
  AUDIT_TAIL_CHOICES,
  AUDIT_TAIL_DEFAULT,
  fetchUsersAudit,
  fetchWhoamiRole,
  formatAuditTimeUtc,
  isSuccessLogin,
  newestFirst,
  type AuthAuditEvent,
} from './access-audit-api'

export const ACCESS_AUDIT_MANIFEST: SettingEntry[] = [
  {
    id: 'k2-connect.access-audit',
    section: 'access-audit',
    label: 'Access Audit',
    description: 'Login history for this host — who got in, from where, when',
    keywords: ['access audit', 'login history', 'audit', 'sign-in', 'compromised'],
  },
]

const EMPTY_COPY =
  'No login records yet on this host. Logging started in 0.40.144 — earlier sign-ins are not in this log.'
const OLD_HOST_COPY = 'This host predates login history — update it to v0.40.144.'
const OWNER_ONLY_COPY = 'Login history is visible to the server owner.'

type FilterMode = 'successes' | 'all'

type ViewState =
  | { kind: 'loading' }
  | { kind: 'old-host' }
  | { kind: 'owner-only' }
  | { kind: 'empty' }
  | { kind: 'table'; events: AuthAuditEvent[] }
  | { kind: 'error'; message: string }
  | { kind: 'unauthorized' }

function outcomeClass(outcome: string): string {
  return outcome === 'ok'
    ? 'text-[var(--color-success, #3d9a5f)]'
    : 'text-[var(--color-text-muted)]'
}

function isAbort(err: unknown): boolean {
  return err instanceof Error && err.name === 'AbortError'
}

export function AccessAuditSection(): React.JSX.Element {
  const supportsAudit = useServerSupports('users-audit')
  const hostKey = useConnectHostStore((s) => (s.activeHost === 'local' ? 'local' : s.activeHost.id))
  const [tail, setTail] = useState<number>(AUDIT_TAIL_DEFAULT)
  const [filter, setFilter] = useState<FilterMode>('successes')
  const [userFilter, setUserFilter] = useState('')
  const [refreshNonce, setRefreshNonce] = useState(0)
  const [view, setView] = useState<ViewState>({ kind: 'loading' })

  useEffect(() => {
    if (!supportsAudit) {
      setView({ kind: 'old-host' })
      return
    }
    const ac = new AbortController()
    setView({ kind: 'loading' })
    void (async () => {
      try {
        const role = await fetchWhoamiRole({ signal: ac.signal })
        if (ac.signal.aborted) return
        if (role !== 'owner') {
          setView({ kind: 'owner-only' })
          return
        }
        const result = await fetchUsersAudit({ tail, signal: ac.signal })
        if (ac.signal.aborted) return
        if (result.kind === 'not-found') {
          setView({ kind: 'old-host' })
          return
        }
        if (result.kind === 'forbidden') {
          setView({ kind: 'owner-only' })
          return
        }
        if (result.kind === 'unauthorized') {
          setView({ kind: 'unauthorized' })
          return
        }
        if (result.kind === 'error') {
          setView({ kind: 'error', message: result.message })
          return
        }
        if (result.events.length === 0) {
          setView({ kind: 'empty' })
          return
        }
        setView({ kind: 'table', events: newestFirst(result.events) })
      } catch (err) {
        if (ac.signal.aborted || isAbort(err)) return
        setView({
          kind: 'error',
          message: err instanceof Error ? err.message : 'Could not load login history.',
        })
      }
    })()
    return () => ac.abort()
  }, [supportsAudit, tail, hostKey, refreshNonce])

  const onRefresh = useCallback(() => {
    setRefreshNonce((n) => n + 1)
  }, [])

  const rows = useMemo(() => {
    if (view.kind !== 'table') return []
    const q = userFilter.trim().toLowerCase()
    return view.events.filter((ev) => {
      if (filter === 'successes' && !isSuccessLogin(ev)) return false
      if (q && !ev.user.toLowerCase().includes(q)) return false
      return true
    })
  }, [view, filter, userFilter])

  return (
    <div data-settings-id="access-audit">
      <div
        className="flex items-start justify-between gap-3 mb-1"
        data-settings-id="k2-connect.access-audit"
      >
        <div>
          <h2 className="text-lg font-semibold text-[var(--color-text-primary)]">Access Audit</h2>
          <p className="text-xs text-[var(--color-text-muted)] mt-1">
            Login history for this host. A successful sign-in is the account that got in —
            passwords are never stored in this log.
          </p>
        </div>
      </div>

      {view.kind === 'old-host' && (
        <p className="text-xs text-[var(--color-text-muted)] mt-6">{OLD_HOST_COPY}</p>
      )}
      {view.kind === 'owner-only' && (
        <p className="text-xs text-[var(--color-text-muted)] mt-6">{OWNER_ONLY_COPY}</p>
      )}
      {view.kind === 'unauthorized' && (
        <p className="text-xs text-[var(--color-text-muted)] mt-6">
          Could not load login history — sign in again.
        </p>
      )}
      {view.kind === 'error' && (
        <p className="text-xs text-[var(--color-text-muted)] mt-6">{view.message}</p>
      )}
      {view.kind === 'loading' && (
        <p className="text-xs text-[var(--color-text-muted)] mt-6">Loading login history…</p>
      )}

      {(view.kind === 'empty' || view.kind === 'table') && (
        <>
          <div className="flex flex-wrap items-end justify-between gap-3 mt-6 mb-3">
            <div className="flex flex-wrap items-center gap-2">
              <span className="text-sm font-semibold text-[var(--color-text-primary)]">
                Login history
              </span>
              <div className="flex gap-1">
                <button
                  type="button"
                  aria-pressed={filter === 'successes'}
                  onClick={() => setFilter('successes')}
                  className={`text-[10px] px-2 py-0.5 border cursor-pointer no-drag ${
                    filter === 'successes'
                      ? 'border-[var(--color-accent)] text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)]'
                      : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)]'
                  }`}
                >
                  Successes
                </button>
                <button
                  type="button"
                  aria-pressed={filter === 'all'}
                  onClick={() => setFilter('all')}
                  className={`text-[10px] px-2 py-0.5 border cursor-pointer no-drag ${
                    filter === 'all'
                      ? 'border-[var(--color-accent)] text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)]'
                      : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)]'
                  }`}
                >
                  All
                </button>
              </div>
              <label className="text-[10px] text-[var(--color-text-muted)] flex items-center gap-1">
                User
                <input
                  type="text"
                  value={userFilter}
                  onChange={(e) => setUserFilter(e.target.value)}
                  aria-label="Filter by user"
                  className="text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] px-2 py-0.5 outline-none w-28"
                />
              </label>
            </div>
            <div className="flex items-center gap-2">
              <label className="text-[10px] text-[var(--color-text-muted)] flex items-center gap-1">
                tail
                <select
                  aria-label="Audit tail"
                  value={tail}
                  onChange={(e) => setTail(Number(e.target.value))}
                  className="text-[11px] bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] px-1.5 py-0.5 outline-none cursor-pointer no-drag"
                >
                  {AUDIT_TAIL_CHOICES.map((n) => (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  ))}
                </select>
              </label>
              <button
                type="button"
                onClick={onRefresh}
                className="text-[10px] px-2 py-0.5 border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer no-drag"
              >
                Refresh
              </button>
            </div>
          </div>

          {view.kind === 'empty' ? (
            <p className="text-xs text-[var(--color-text-muted)]">{EMPTY_COPY}</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-xs text-left border-collapse min-w-[640px]">
                <thead>
                  <tr className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] border-b border-[var(--color-border)]">
                    <th className="font-medium py-1.5 pr-3">Time (UTC)</th>
                    <th className="font-medium py-1.5 pr-3">Event</th>
                    <th className="font-medium py-1.5 pr-3">Outcome</th>
                    <th className="font-medium py-1.5 pr-3">User</th>
                    <th className="font-medium py-1.5 pr-3">Ingress</th>
                    <th className="font-medium py-1.5">IP</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.length === 0 ? (
                    <tr>
                      <td
                        colSpan={6}
                        className="py-3 text-[var(--color-text-muted)]"
                      >
                        No matching records in this window.
                      </td>
                    </tr>
                  ) : (
                    rows.map((ev, i) => (
                      <tr
                        key={`${ev.ts}-${ev.user}-${ev.outcome}-${i}`}
                        className="border-b border-[var(--color-border)] text-[var(--color-text-primary)]"
                      >
                        <td className="py-1.5 pr-3 font-mono whitespace-nowrap">
                          {formatAuditTimeUtc(ev.ts)}
                        </td>
                        <td className="py-1.5 pr-3">{ev.event || '—'}</td>
                        <td className={`py-1.5 pr-3 font-mono ${outcomeClass(ev.outcome)}`}>
                          {ev.outcome || '—'}
                        </td>
                        <td className="py-1.5 pr-3">{ev.user || '—'}</td>
                        <td className="py-1.5 pr-3 font-mono">{ev.ingress || '—'}</td>
                        <td className="py-1.5 font-mono">{ev.ip || '—'}</td>
                      </tr>
                    ))
                  )}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}
    </div>
  )
}
