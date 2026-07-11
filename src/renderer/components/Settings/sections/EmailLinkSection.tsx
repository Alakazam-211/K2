// Settings → Email Link (K2 Mail S9, prd-email-server-v1 §17.5) — the
// CROSS-PLATFORM surface: the owner connects their OWN external email
// accounts (Gmail app-password / Fastmail / IMAP) so a workspace's
// agents can READ the inbox and save reply DRAFTS into the account's
// real Drafts folder. The user reviews and sends from their own mail
// client — K2 never sends from an external account (§17.5: drafts
// only, no send code path).
//
// NOT LINUX-GATED (BINDING): unlike Email Hosting, this is purely an
// IMAP client — nothing is installed or hosted — so the page NEVER
// reads GET /cli/mail/status and NEVER keys off `supported`. It works
// identically on Mac and Linux. Owner-gating on external/* is enforced
// server-side; the client only disables mutating controls in viewer
// mode.
//
// CREDENTIALS HYGIENE (§17.5): the app-password is WRITE-ONLY — sent
// ONCE in the add body, vaulted server-side, and never returned,
// listed, or persisted client-side. The connected-inbox list omits it
// (and even the username). This component holds it in local state only
// while the add form is open and clears it on submit/close.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useToastStore } from '@/stores/toast'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useWindowModeStore } from '@/stores/window-mode'
import type { SettingEntry } from '../searchManifest'
import {
  addExternalInbox,
  fetchExternalInboxes,
  mailErrorMessage,
  removeExternalInbox,
  type ExternalInbox,
} from './email-api'

export const EMAIL_LINK_MANIFEST: SettingEntry[] = [
  { id: 'email-link.inboxes', section: 'email-link', label: 'Connected Inboxes', description: 'External email accounts your agents can read and draft replies for', keywords: ['imap', 'gmail', 'fastmail', 'inbox', 'external', 'connect', 'email', 'account', 'app password', 'link'] },
  { id: 'email-link.add', section: 'email-link', label: 'Connect an Inbox', description: 'Add a Gmail / Fastmail / IMAP account bound to one workspace', keywords: ['add', 'connect', 'imap', 'gmail', 'app password', 'drafts', 'external inbox', 'imap host', 'port', 'tls'] },
]

const TLS_IMPLICIT = 'implicit-tls'
const TLS_STARTTLS = 'starttls'

type Selection = { kind: 'add' } | { kind: 'inbox'; address: string }

// ── Small shared bits ────────────────────────────────────────────────────

function SectionTitle({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
      {children}
    </h3>
  )
}

function fmtDate(unixSecs: number | null | undefined): string {
  if (typeof unixSecs !== 'number' || unixSecs <= 0) return '—'
  return new Date(unixSecs * 1000).toLocaleString()
}

/** Connect-health → status-token color. */
function statusColor(status: string): string {
  switch (status) {
    case 'connected':
      return 'var(--color-status-ok)'
    case 'error':
      return 'var(--color-status-error-soft)'
    default:
      return 'var(--color-text-muted)'
  }
}

function StatusChip({ status }: { status: string }): React.JSX.Element {
  const color = statusColor(status)
  const label = status === 'connected' ? 'Connected' : status === 'error' ? 'Error' : status
  return (
    <span
      className="text-[9px] font-semibold px-1.5 py-0.5 flex-shrink-0 uppercase tracking-wide"
      style={{ color, backgroundColor: `color-mix(in srgb, ${color} 14%, transparent)` }}
    >
      {label}
    </span>
  )
}

// ── Add-inbox form ───────────────────────────────────────────────────────

function AddInboxForm({
  canMutate,
  onAdded,
}: {
  canMutate: boolean
  onAdded: (address: string) => void
}): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)

  const [project, setProject] = useState('')
  const [address, setAddress] = useState('')
  const [host, setHost] = useState('')
  const [port, setPort] = useState('993')
  const [tls, setTls] = useState(TLS_IMPLICIT)
  const [username, setUsername] = useState('')
  const [displayName, setDisplayName] = useState('')
  const [draftsFolder, setDraftsFolder] = useState('')
  const [password, setPassword] = useState('')

  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const canSubmit =
    canMutate &&
    !busy &&
    project.trim().length > 0 &&
    address.trim().length > 0 &&
    host.trim().length > 0 &&
    password.length > 0

  const submit = useCallback(async (): Promise<void> => {
    if (!canSubmit) return
    setBusy(true)
    setError(null)
    const portNum = Number(port)
    try {
      const res = await addExternalInbox({
        project: project.trim(),
        address: address.trim(),
        host: host.trim(),
        port: Number.isFinite(portNum) && portNum > 0 ? portNum : undefined,
        tls,
        username: username.trim() || undefined,
        displayName: displayName.trim() || undefined,
        draftsFolder: draftsFolder.trim() || undefined,
        password,
      })
      // WRITE-ONLY: drop the secret from memory the instant it's sent.
      setPassword('')
      useToastStore.getState().addToast(`Connected ${res.address}`, 'success')
      onAdded(res.address)
    } catch (e) {
      // The daemon's structured hint (bad drafts folder → real folder
      // list, login/connect failure, workspace-not-found) verbatim.
      setError(mailErrorMessage(e))
    } finally {
      setBusy(false)
    }
  }, [canSubmit, project, address, host, port, tls, username, displayName, draftsFolder, password, onAdded])

  const field = (
    label: string,
    node: React.JSX.Element,
    hint?: string,
  ): React.JSX.Element => (
    <label className="flex flex-col gap-0.5 min-w-0">
      <span className="text-[10px] text-[var(--color-text-secondary)]">{label}</span>
      {node}
      {hint && <span className="text-[9px] text-[var(--color-text-muted)]">{hint}</span>}
    </label>
  )

  const inputCls =
    'px-2 py-1 text-[11px] font-mono bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)] no-drag disabled:opacity-50'

  return (
    <div className="grid gap-6 grid-cols-[minmax(0,42rem)]">
      <div className="min-w-0" data-settings-id="email-link.add">
        <h2 className="text-base font-medium text-[var(--color-text-primary)]">Connect an inbox</h2>
        <p className="text-[11px] text-[var(--color-text-muted)] mt-1">
          Bind one of your email accounts to a single workspace. K2 connects over IMAP to verify
          the account now, then that workspace&rsquo;s agents can read it and save reply drafts —
          you review and send from your own mail client.
        </p>
      </div>

      <div className="space-y-3">
        <SectionTitle>Account</SectionTitle>
        <div className="grid grid-cols-2 gap-3">
          {field(
            'Workspace',
            <select
              value={project}
              disabled={!canMutate || busy}
              onChange={(e) => setProject(e.target.value)}
              className="px-2 py-1 text-[11px] bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
            >
              <option value="">Pick a workspace…</option>
              {projects.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>,
            'The one workspace whose agents get read + draft access.',
          )}
          {field(
            'Email address',
            <input
              type="email"
              value={address}
              disabled={!canMutate || busy}
              onChange={(e) => setAddress(e.target.value)}
              placeholder="you@gmail.com"
              className={inputCls}
            />,
          )}
          {field(
            'IMAP host',
            <input
              type="text"
              value={host}
              disabled={!canMutate || busy}
              onChange={(e) => setHost(e.target.value)}
              placeholder="imap.gmail.com"
              className={inputCls}
            />,
          )}
          <div className="grid grid-cols-2 gap-2">
            {field(
              'Port',
              <input
                type="text"
                inputMode="numeric"
                value={port}
                disabled={!canMutate || busy}
                onChange={(e) => setPort(e.target.value)}
                placeholder="993"
                className={inputCls}
              />,
            )}
            {field(
              'TLS',
              <select
                value={tls}
                disabled={!canMutate || busy}
                onChange={(e) => setTls(e.target.value)}
                className="px-2 py-1 text-[11px] bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
              >
                <option value={TLS_IMPLICIT}>Implicit TLS (993)</option>
                <option value={TLS_STARTTLS}>STARTTLS (143)</option>
              </select>,
            )}
          </div>
          {field(
            'Username',
            <input
              type="text"
              value={username}
              disabled={!canMutate || busy}
              onChange={(e) => setUsername(e.target.value)}
              placeholder={address.trim() || 'defaults to the address'}
              className={inputCls}
            />,
          )}
          {field(
            'Display name (optional)',
            <input
              type="text"
              value={displayName}
              disabled={!canMutate || busy}
              onChange={(e) => setDisplayName(e.target.value)}
              placeholder="Rosson"
              className={inputCls}
            />,
          )}
          {field(
            'Drafts folder (optional)',
            <input
              type="text"
              value={draftsFolder}
              disabled={!canMutate || busy}
              onChange={(e) => setDraftsFolder(e.target.value)}
              placeholder="auto-detect"
              className={inputCls}
            />,
            'Blank = K2 finds it. A wrong name errors with the real folder list.',
          )}
        </div>
      </div>

      <div className="space-y-2">
        <SectionTitle>App password</SectionTitle>
        <input
          type="password"
          value={password}
          disabled={!canMutate || busy}
          autoComplete="new-password"
          onChange={(e) => setPassword(e.target.value)}
          placeholder="app-specific password"
          className={`w-full ${inputCls}`}
        />
        <p className="text-[10px] text-[var(--color-text-muted)]">
          Use an <span className="font-mono">app-specific password</span> (Gmail / Fastmail issue
          these), not your login password. It&rsquo;s stored securely in the daemon vault and never
          shown again — K2 never displays it back and never sends from this account.
        </p>
      </div>

      <div className="flex items-center gap-2 pb-6">
        <button
          type="button"
          disabled={!canSubmit}
          onClick={() => void submit()}
          title={canMutate ? 'Connect and verify over IMAP' : 'Not available in viewer mode'}
          className="px-3 py-1.5 text-xs font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0"
        >
          {busy ? 'Connecting…' : 'Connect inbox'}
        </button>
        {error && (
          <p className="text-[11px] text-[var(--color-status-error-soft)] break-words">{error}</p>
        )}
      </div>
    </div>
  )
}

// ── Inbox detail ─────────────────────────────────────────────────────────

function InboxDetail({
  inbox,
  canMutate,
  onRemoved,
}: {
  inbox: ExternalInbox
  canMutate: boolean
  onRemoved: () => void
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)

  const doRemove = useCallback(async (): Promise<void> => {
    const confirmed = await useConfirmDialogStore.getState().confirm({
      title: `Disconnect ${inbox.address}?`,
      message:
        'This removes the connection and permanently deletes its stored app-password from the ' +
        'vault. Agents lose read + draft access to this inbox. Nothing in the account itself is ' +
        'touched.',
      confirmLabel: 'Disconnect',
      destructive: true,
    })
    if (!confirmed) return
    setBusy(true)
    try {
      await removeExternalInbox(inbox.address)
      onRemoved()
    } catch (e) {
      useToastStore.getState().addToast(`Disconnect failed: ${mailErrorMessage(e)}`, 'error')
    } finally {
      setBusy(false)
    }
  }, [inbox.address, onRemoved])

  const detailRow = (label: string, value: React.ReactNode): React.JSX.Element => (
    <div className="flex items-center gap-3 px-3 py-2">
      <span className="text-[11px] text-[var(--color-text-secondary)] w-28 flex-shrink-0">
        {label}
      </span>
      <span className="text-xs text-[var(--color-text-primary)] truncate min-w-0">{value}</span>
    </div>
  )

  return (
    <div className="grid gap-6 grid-cols-[minmax(0,42rem)]">
      <div className="min-w-0">
        <div className="flex items-center gap-2 min-w-0">
          <h2 className="text-base font-medium text-[var(--color-text-primary)] font-mono truncate">
            {inbox.address}
          </h2>
          <StatusChip status={inbox.status} />
        </div>
        <p className="text-[11px] text-[var(--color-text-muted)] mt-1">
          Bound to <span className="text-[var(--color-text-secondary)]">{inbox.holderWorkspace ?? 'unknown workspace'}</span>
          {' · '}agents there read it and draft replies with{' '}
          <span className="font-mono">k2 mail draft</span>.
        </p>
      </div>

      {inbox.lastError && (
        <div className="border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] px-3 py-2">
          <p className="text-[11px] text-[var(--color-status-error-soft)] break-words">
            {inbox.lastError}
          </p>
        </div>
      )}

      <div className="space-y-2">
        <SectionTitle>Connection</SectionTitle>
        <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
          {detailRow('Workspace', inbox.holderWorkspace ?? '—')}
          {detailRow('IMAP host', <span className="font-mono">{inbox.host}:{inbox.port}</span>)}
          {detailRow('TLS', inbox.tls)}
          {detailRow('Drafts folder', inbox.draftsFolder ?? 'auto-detected')}
          {detailRow('Last checked', fmtDate(inbox.lastCheckedAt))}
          {detailRow('Connected', fmtDate(inbox.createdAt))}
        </div>
      </div>

      <div className="space-y-2 pb-6">
        <SectionTitle>Danger zone</SectionTitle>
        <div className="border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] px-3 py-2">
          <div className="flex items-center gap-3">
            <div className="flex-1 min-w-0">
              <p className="text-xs text-[var(--color-text-primary)]">Disconnect this inbox</p>
              <p className="text-[10px] text-[var(--color-text-muted)]">
                Deletes the stored app-password and revokes agent access. The account itself is
                untouched.
              </p>
            </div>
            <button
              type="button"
              disabled={!canMutate || busy}
              onClick={() => void doRemove()}
              className="px-2.5 py-1 text-[10px] font-medium text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error-soft)_10%,transparent)] transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0"
            >
              {busy ? 'Disconnecting…' : 'Disconnect'}
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}

// ── The section root (master-detail shell) ───────────────────────────────

export function EmailLinkSection(): React.JSX.Element {
  const viewerReadOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  const canMutate = !viewerReadOnly

  const [inboxes, setInboxes] = useState<ExternalInbox[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [revision, setRevision] = useState(0)
  const bump = useCallback(() => setRevision((r) => r + 1), [])
  const [selection, setSelection] = useState<Selection>({ kind: 'add' })

  // ── Load the connected-inbox table. Cross-platform: no status/
  //    supported gate — just the owner list route. ────────────────────
  useEffect(() => {
    let cancelled = false
    fetchExternalInboxes()
      .then((r) => {
        if (cancelled) return
        setInboxes(r)
        setError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setInboxes(null)
        setError(mailErrorMessage(e))
      })
    return () => {
      cancelled = true
    }
  }, [revision])

  // Selection healing: a removed inbox falls back to the add form.
  useEffect(() => {
    if (selection.kind !== 'inbox' || inboxes === null) return
    if (!inboxes.some((i) => i.address === selection.address)) {
      setSelection({ kind: 'add' })
    }
  }, [inboxes, selection])

  const selectedInbox = useMemo(() => {
    if (selection.kind !== 'inbox') return null
    return inboxes?.find((i) => i.address === selection.address) ?? null
  }, [selection, inboxes])

  const onAdded = useCallback(
    (address: string): void => {
      setSelection({ kind: 'inbox', address })
      bump()
    },
    [bump],
  )

  return (
    <div className="flex flex-col h-full min-h-0">
      <div className="flex flex-1 min-h-0">
        {/* ── Left column ── */}
        <div className="w-60 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col min-h-0">
          <div className="px-3 pt-3 pb-1">
            <SectionTitle>Connected inboxes</SectionTitle>
          </div>
          <div className="flex-1 overflow-y-auto px-1 py-1" data-settings-id="email-link.inboxes">
            {error ? (
              <div className="px-2 py-2 space-y-2">
                <p className="text-[11px] text-[var(--color-status-error-soft)] break-words">
                  {error}
                </p>
                <button
                  type="button"
                  onClick={bump}
                  className="px-2 py-1 text-[10px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer no-drag"
                >
                  Retry
                </button>
              </div>
            ) : inboxes === null ? (
              <p className="px-2 py-1 text-[11px] text-[var(--color-text-muted)]">Loading…</p>
            ) : inboxes.length === 0 ? (
              <p className="px-2 py-2 text-[11px] text-[var(--color-text-muted)] italic">
                No inboxes connected yet — link a personal account to give a workspace&rsquo;s
                agents read + draft access.
              </p>
            ) : (
              inboxes.map((i) => {
                const isSelected = selection.kind === 'inbox' && selection.address === i.address
                return (
                  <button
                    key={i.id}
                    type="button"
                    onClick={() => setSelection({ kind: 'inbox', address: i.address })}
                    className={`w-full flex flex-col gap-0.5 px-2 py-1.5 text-left transition-colors no-drag cursor-pointer min-w-0 ${
                      isSelected
                        ? 'bg-[var(--color-accent)]/15'
                        : 'hover:bg-[var(--color-bg-hover)]'
                    }`}
                  >
                    <div className="flex items-center gap-1.5 min-w-0">
                      <span className="text-xs font-mono truncate flex-1 text-[var(--color-text-primary)]">
                        {i.address}
                      </span>
                      <span
                        className="w-2 h-2 rounded-full flex-shrink-0"
                        style={{ backgroundColor: statusColor(i.status) }}
                        title={i.status}
                      />
                    </div>
                    <span className="text-[10px] text-[var(--color-text-muted)] truncate">
                      {i.holderWorkspace ?? 'unknown workspace'}
                    </span>
                  </button>
                )
              })
            )}
          </div>

          {/* Add inbox */}
          <div className="px-2 py-2 border-t border-[var(--color-border)] flex-shrink-0">
            <button
              type="button"
              disabled={!canMutate}
              onClick={() => setSelection({ kind: 'add' })}
              title={canMutate ? 'Connect a personal email account' : 'Not available in viewer mode'}
              className={`w-full flex items-center justify-center gap-1.5 px-2 py-1.5 text-xs transition-colors no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed ${
                selection.kind === 'add'
                  ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                  : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-hover)] hover:bg-[var(--color-bg-elevated)]'
              }`}
            >
              <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
              </svg>
              Add inbox
            </button>
          </div>
        </div>

        {/* ── Right panel ── */}
        <div className="flex-1 overflow-y-auto p-6 min-h-0">
          {selection.kind === 'add' || selectedInbox === null ? (
            <AddInboxForm canMutate={canMutate} onAdded={onAdded} />
          ) : (
            <InboxDetail
              inbox={selectedInbox}
              canMutate={canMutate}
              onRemoved={() => {
                setSelection({ kind: 'add' })
                bump()
              }}
            />
          )}
        </div>
      </div>
    </div>
  )
}
