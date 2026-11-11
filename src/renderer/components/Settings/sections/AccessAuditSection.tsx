// Settings → Logs → Access Audit. Owner-only list over GET /cli/users/audit
// on the active host: sign-ins, vault handoffs and permission cards. Each
// record reads as a sentence (who did what, from where, when) and opens to
// the raw line. See prd-login-history-settings-v1.md.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { useConnectHostStore } from '@/stores/connect-host'
import { useServerSupports } from '@/lib/server-capabilities'
import { Button, Callout, SearchField, Segmented } from '@/components/ui'
import type { SettingEntry } from '../searchManifest'
import {
  SETTING_ROW_HINT,
  SettingDropdown,
  SettingsGroup,
  SettingsLoadState,
} from '../controls/SettingControls'
import {
  AUDIT_TAIL_CHOICES,
  AUDIT_TAIL_DEFAULT,
  fetchUsersAudit,
  fetchWhoamiRole,
  formatAuditTimeUtc,
  isAgentVaultExport,
  isFailedLogin,
  isFailedRecord,
  isPermissionCardsEvent,
  isSuccessLogin,
  isVaultHandoff,
  newestFirst,
  type AuthAuditEvent,
} from './access-audit-api'

export const ACCESS_AUDIT_MANIFEST: SettingEntry[] = [
  {
    id: 'k2-connect.access-audit',
    section: 'access-audit',
    label: 'Access Audit',
    description: 'Who signed in to this server, from where and when, and which secrets agents sent each other',
    keywords: ['access audit', 'login history', 'audit', 'sign-in', 'failed sign-in', 'compromised', 'vault handoff', 'secret sent', 'permission cards', 'permission request'],
  },
]

const CONTENT_WIDTH = 'max-w-[52rem]'

type FilterMode = 'successes' | 'failed' | 'handoffs' | 'permission' | 'all'

const FILTERS: { mode: FilterMode; label: string; title: string }[] = [
  { mode: 'successes', label: 'Signed in', title: 'Sign-ins' },
  { mode: 'failed', label: 'Failed sign-ins', title: 'Failed sign-ins' },
  { mode: 'handoffs', label: 'Vault handoffs', title: 'Vault handoffs' },
  // Permission cards (Beta, prd-agent-settings-cards-v1 §6.5): asks,
  // answers, notes, Turn off again and the switches.
  { mode: 'permission', label: 'Permission cards', title: 'Permission cards' },
  { mode: 'all', label: 'All', title: 'All records' },
]

type ViewState =
  | { kind: 'loading' }
  | { kind: 'old-host' }
  | { kind: 'owner-only' }
  | { kind: 'empty' }
  | { kind: 'table'; events: AuthAuditEvent[] }
  | { kind: 'error'; message: string }
  | { kind: 'unauthorized' }

function isAbort(err: unknown): boolean {
  return err instanceof Error && err.name === 'AbortError'
}

function matchesFilter(ev: AuthAuditEvent, filter: FilterMode): boolean {
  switch (filter) {
    // An agent's vault export always shows: it is why this page is open.
    case 'successes':
      return isSuccessLogin(ev) || isAgentVaultExport(ev)
    case 'failed':
      return isFailedLogin(ev)
    case 'handoffs':
      return isVaultHandoff(ev)
    case 'permission':
      return isPermissionCardsEvent(ev)
    case 'all':
      return true
  }
}

/** Who did it, in plain words: an agent's stamp reads "Agent atlas/s1". */
export function auditWho(user: string): string {
  const u = user.trim()
  if (!u || u === '-') return 'Someone'
  if (u.startsWith('agent:')) return `Agent ${u.slice('agent:'.length)}`
  if (u.startsWith('connect:')) return u.slice('connect:'.length)
  return u
}

const SIGN_IN_FAILURES: Record<string, string> = {
  bad_creds: 'couldn’t sign in: wrong username or password',
  locked: 'couldn’t sign in: locked out after too many tries',
  rate_limited: 'couldn’t sign in: too many tries, so K2 is waiting',
  blocked_ingress: 'was blocked: came in by a route this server doesn’t accept',
}

const PERMISSION_WHAT: Record<string, string> = {
  'permission-request': 'asked for a permission',
  'permission-answer': 'answered a permission request',
  'permission-note': 'added a note to a permission request',
  'permission-undo': 'turned a permission off again',
  'permission-switch': 'changed a permission switch',
}

/** What happened, as the rest of a sentence after the person's name. */
export function auditWhat(ev: AuthAuditEvent): string {
  const failed = isFailedRecord(ev)
  const word = ev.outcome.trim().split(/\s+/)[1] ?? ''
  switch (ev.event) {
    case 'login':
      if (!failed) return 'signed in'
      if (ev.outcome.startsWith('bad_attest')) return 'was blocked: the sign-in couldn’t be verified'
      return SIGN_IN_FAILURES[ev.outcome] ?? 'couldn’t sign in'
    case 'logout':
      return 'signed out'
    case 'change-password':
      return failed ? 'couldn’t change their password' : 'changed their password'
    case 'set-password':
      return failed ? 'couldn’t set a password' : 'had a password set'
    case 'add-user':
      return 'added a person'
    case 'vault-handoff':
      if (failed) return 'was refused a secret handoff'
      if (word === 'send') return 'sent a secret'
      if (word === 'accept') return 'accepted a secret'
      if (word === 'decline') return 'declined a secret'
      if (word === 'expire') return 'let a secret handoff expire'
      return 'handed off a secret'
    case 'vault-carry':
      return word === 'export' ? 'exported secrets from this server’s vault' : 'moved secrets in the vault'
    default:
      return PERMISSION_WHAT[ev.event] ?? (ev.event || 'did something')
  }
}

/** Where it came from: the address and the route, in plain words. */
export function auditWhere(ev: AuthAuditEvent): string {
  const ing = ev.ingress.trim()
  const route =
    ing === 'loopback'
      ? 'on this computer'
      : ing === 'lan'
        ? 'local network'
        : ing === 'tunnel'
          ? 'through the tunnel'
          : ing.startsWith('edge:')
            ? 'web sign-in'
            : ''
  const ip = ev.ip && ev.ip !== '-' ? ev.ip : ''
  return [ip, route].filter(Boolean).join(' · ')
}

/** Local date and time; the full UTC stamp is in the details. */
function localWhen(ts: string): string {
  const d = new Date(ts)
  if (Number.isNaN(d.getTime())) return ts || '—'
  return d.toLocaleString([], { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })
}

function RecordRow({ ev, open, onToggle, rowId }: { ev: AuthAuditEvent; open: boolean; onToggle: () => void; rowId: string }): React.JSX.Element {
  const failed = isFailedRecord(ev)
  const exported = isAgentVaultExport(ev)
  const where = auditWhere(ev)
  const showOutcome = !(ev.event === 'login' && (ev.outcome === 'ok' || SIGN_IN_FAILURES[ev.outcome]))
  const dot = failed
    ? 'bg-[var(--color-status-error)]'
    : exported
      ? 'bg-[var(--color-status-warn)]'
      : ev.event === 'login'
        ? 'bg-[var(--color-status-ok)]'
        : 'bg-[var(--color-text-muted)]'
  const fields: Array<[string, string | undefined]> = [
    ['Time (UTC)', formatAuditTimeUtc(ev.ts)],
    ['Record', ev.event],
    ['Result', ev.outcome],
    ['Account', ev.user],
    ['Route', ev.ingress],
    ['Address', ev.ip],
    ['App', ev.client],
  ]
  return (
    <li data-audit-row="" data-failed={failed ? 'true' : undefined} data-agent-vault-export={exported ? 'true' : undefined}>
      <button
        type="button"
        data-k2-focus=""
        aria-expanded={open}
        aria-controls={rowId}
        onClick={onToggle}
        className="group relative w-full grid grid-cols-[auto_minmax(0,1fr)_auto_auto] items-start gap-x-3 px-4 py-3 text-left no-drag cursor-pointer outline-none transition-colors hover:bg-[var(--color-bg-hover)]"
      >
        <span aria-hidden="true" className={`mt-1.5 inline-block h-2 w-2 rounded-[var(--radius-status)] ${dot}`} />
        <span className="min-w-0">
          <span className="block type-row text-[var(--color-text-primary)]">
            <span className="type-name">{auditWho(ev.user)}</span>{' '}
            <span className={failed ? 'text-[var(--color-status-error-soft)]' : undefined}>{auditWhat(ev)}</span>
          </span>
          {where && <span className="block type-hint text-[var(--color-text-secondary)] mt-0.5">{where}</span>}
          {showOutcome && ev.outcome && (
            <span className="block type-hint font-mono text-[var(--color-text-secondary)] mt-0.5 whitespace-pre-wrap break-words">
              {ev.outcome}
            </span>
          )}
        </span>
        <span className="type-hint text-[var(--color-text-secondary)] tabular-nums whitespace-nowrap" title={`${formatAuditTimeUtc(ev.ts)} UTC`}>
          {localWhen(ev.ts)}
        </span>
        <span aria-hidden="true" className={`type-row text-[var(--color-text-muted)] group-hover:text-[var(--color-text-primary)] transition-transform ${open ? 'rotate-90' : ''}`}>
          ›
        </span>
      </button>
      {open && (
        <dl id={rowId} className="grid grid-cols-[8rem_minmax(0,1fr)] gap-x-4 gap-y-1 px-4 pb-3 pl-9">
          {fields
            .filter(([, v]) => v !== undefined && v !== '')
            .map(([k, v]) => (
              <React.Fragment key={k}>
                <dt className="type-hint text-[var(--color-text-secondary)]">{k}</dt>
                <dd className="type-hint font-mono text-[var(--color-text-primary)] break-words">{v}</dd>
              </React.Fragment>
            ))}
        </dl>
      )}
    </li>
  )
}

export function AccessAuditSection(): React.JSX.Element {
  const supportsAudit = useServerSupports('users-audit')
  const hostKey = useConnectHostStore((s) => (s.activeHost === 'local' ? 'local' : s.activeHost.id))
  const [tail, setTail] = useState<number>(AUDIT_TAIL_DEFAULT)
  const [filter, setFilter] = useState<FilterMode>('successes')
  const [userFilter, setUserFilter] = useState('')
  const [refreshNonce, setRefreshNonce] = useState(0)
  const [view, setView] = useState<ViewState>({ kind: 'loading' })
  const [openRow, setOpenRow] = useState<string | null>(null)

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
          message: err instanceof Error ? err.message : '',
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
      if (!matchesFilter(ev, filter)) return false
      if (q && !ev.user.toLowerCase().includes(q)) return false
      return true
    })
  }, [view, filter, userFilter])

  const agentExports = useMemo(
    () => (view.kind === 'table' ? view.events.filter(isAgentVaultExport).length : 0),
    [view],
  )

  const tryAgain = <Button onClick={onRefresh}>Try again</Button>
  const gate =
    view.kind === 'old-host' ? (
      <SettingsLoadState kind="empty" title="This server can’t show its sign-in history yet" testId="audit-old-host">
        Update K2 on this server, then come back here to see who signed in.
      </SettingsLoadState>
    ) : view.kind === 'owner-only' ? (
      <SettingsLoadState kind="empty" title="Only the server’s owner can see this" testId="audit-owner-only">
        Sign-in history is private to the owner. Ask them if you need to know who signed in.
      </SettingsLoadState>
    ) : view.kind === 'unauthorized' ? (
      <SettingsLoadState kind="error" title="Your sign-in to this server has expired" action={tryAgain} testId="audit-unauthorized">
        Sign in to this server again, then try again.
      </SettingsLoadState>
    ) : view.kind === 'error' ? (
      <SettingsLoadState kind="error" title="Sign-in history couldn’t load" detail={view.message} action={tryAgain} testId="audit-error">
        K2 couldn’t read this server’s access log. Nothing is lost; try again in a moment.
      </SettingsLoadState>
    ) : view.kind === 'loading' ? (
      <SettingsLoadState kind="loading" title="Loading sign-in history…" testId="audit-loading">
        The newest records on this server.
      </SettingsLoadState>
    ) : view.kind === 'empty' ? (
      <SettingsLoadState kind="empty" title="No sign-ins recorded yet" action={<Button onClick={onRefresh}>Refresh</Button>} testId="audit-empty">
        Each sign-in to this server, and each secret an agent hands to another, shows here as it happens.
      </SettingsLoadState>
    ) : null

  const current = FILTERS.find((f) => f.mode === filter) ?? FILTERS[0]
  const narrowed = filter !== 'all' || userFilter.trim() !== ''
  const total = view.kind === 'table' ? view.events.length : 0
  const who = userFilter.trim()

  return (
    <div data-settings-id="access-audit" className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`}>
      <header className="min-w-0" data-settings-id="k2-connect.access-audit">
        <h2 className="type-title text-[var(--color-text-primary)]">Access Audit</h2>
        <p className={SETTING_ROW_HINT}>
          Who signed in to this server, from where and when, and which secrets agents handed each other. Passwords and
          secret values are never kept here.
        </p>
      </header>

      {gate ?? (
        <>
          {agentExports > 0 && (
            <Callout tone="warn" role="status" data-testid="agent-vault-export-banner" className="type-row">
              {agentExports === 1
                ? 'An agent exported secrets from this server’s vault. It’s marked below; the log keeps names only, never values.'
                : `Agents exported secrets from this server’s vault ${agentExports} times. They’re marked below; the log keeps names only, never values.`}
            </Callout>
          )}

          <div className="flex flex-wrap items-center justify-between gap-3" data-testid="audit-filters">
            <Segmented<FilterMode>
              aria-label="Show"
              value={filter}
              onChange={(next) => {
                setFilter(next)
                setOpenRow(null)
              }}
              options={FILTERS.map((f) => ({ value: f.mode, label: f.label, buttonProps: { 'data-testid': `audit-filter-${f.mode}` } }))}
            />
            <SearchField
              className="w-60"
              value={userFilter}
              onChange={(e) => setUserFilter(e.target.value)}
              placeholder="Filter by person"
              aria-label="Filter by person"
            />
          </div>

          <SettingsGroup
            card
            title={current.title}
            hint={
              who
                ? rows.length === 0
                  ? `Nothing for “${who}” here.`
                  : `${rows.length === 1 ? 'One record' : `${rows.length} records`} for “${who}”, newest first.`
                : 'Newest first. Open a record for the full line.'
            }
            action={<Button onClick={onRefresh}>Refresh</Button>}
            testId="audit-records"
          >
            {rows.length === 0 ? (
              <div className="px-4 py-6 flex items-center justify-between gap-6" data-testid="audit-no-match">
                <div className="min-w-0">
                  <p className="type-row text-[var(--color-text-primary)]">
                    {filter === 'failed' && !who ? 'No failed sign-ins' : 'Nothing matches'}
                  </p>
                  <p className={SETTING_ROW_HINT}>
                    {filter === 'failed' && !who
                      ? `No one failed to sign in within the newest ${total} records.`
                      : `No ${current.title.toLowerCase()} ${who ? `for “${who}” ` : ''}in the newest ${total} records.`}
                  </p>
                </div>
                {narrowed && (
                  <Button
                    onClick={() => {
                      setFilter('all')
                      setUserFilter('')
                    }}
                  >
                    Show all records
                  </Button>
                )}
              </div>
            ) : (
              <ul className="max-h-[40rem] overflow-y-auto divide-y divide-[var(--color-border)]" data-testid="audit-rows">
                {rows.map((ev, i) => {
                  const key = `${ev.ts}-${ev.user}-${ev.event}-${i}`
                  return (
                    <RecordRow
                      key={key}
                      ev={ev}
                      rowId={`audit-row-${i}`}
                      open={openRow === key}
                      onToggle={() => setOpenRow((k) => (k === key ? null : key))}
                    />
                  )
                })}
              </ul>
            )}
            <div className="px-4 py-2.5 flex items-center justify-between gap-4">
              <span className="type-hint text-[var(--color-text-secondary)]" data-testid="audit-count">
                Showing {rows.length} of the newest {total} records
              </span>
              <span className="flex items-center gap-2">
                <span className="type-hint text-[var(--color-text-secondary)]">Read</span>
                <SettingDropdown
                  ariaLabel="How many records to read"
                  value={String(tail)}
                  onChange={(value) => setTail(Number(value))}
                  options={AUDIT_TAIL_CHOICES.map((n) => ({
                    value: String(n),
                    label: `Newest ${n.toLocaleString()}`,
                  }))}
                />
              </span>
            </div>
          </SettingsGroup>
        </>
      )}
    </div>
  )
}
