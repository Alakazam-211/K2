// Settings → Email Link (K2 Mail, prd-email-server-v1 §17.5 + GH #28) —
// the CROSS-PLATFORM surface: the user connects their OWN external email
// accounts (Gmail app-password / Fastmail / IMAP) so a workspace's agents
// can READ the inbox and save reply DRAFTS into the account's real Drafts
// folder. The user reviews and sends from their own mail client — K2 never
// sends from a linked account (drafts only, no send code path).
//
// Linked inboxes are managed through the SAME unified access layer as
// hosted inboxes (GH #28): each has a **Primary** workspace plus grants,
// via the shared <InboxAccessPanel>. Source-aware: because these are
// linked, the panel caps access at Read + Draft and NEVER offers Send.
//
// NOT LINUX-GATED (BINDING): unlike Email Hosting, this is purely an IMAP
// client — nothing is installed or hosted — so the page NEVER reads GET
// /cli/mail/status and NEVER keys off `supported`. It works identically on
// Mac and Linux. Primary-gating on access/* is enforced server-side; the
// client only disables mutating controls in viewer mode.
//
// CREDENTIALS HYGIENE (§17.5): the app-password is WRITE-ONLY — sent ONCE
// in the add body, vaulted server-side, and never returned, listed, or
// persisted client-side. The connected-inbox list omits it (and even the
// username). This component holds it in local state only while the add
// form is open and clears it on submit/close.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { openUrl } from '@tauri-apps/plugin-opener'
import { useProjectsStore } from '@/stores/projects'
import { useToastStore } from '@/stores/toast'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useWindowModeStore } from '@/stores/window-mode'
import type { SettingEntry } from '../searchManifest'
import { SettingDropdown } from '../controls/SettingControls'
import { InboxAccessPanel } from './InboxAccessPanel'
import {
  addLinkedInbox,
  fetchInboxes,
  linkOauthStart,
  linkOauthStatus,
  mailErrorInfo,
  mailErrorMessage,
  removeLinkedInbox,
  type Inbox,
} from './email-api'

export const EMAIL_LINK_MANIFEST: SettingEntry[] = [
  { id: 'email-link.inboxes', section: 'email-link', label: 'Connected Inboxes', description: 'External email accounts your workspaces can read and draft replies for', keywords: ['imap', 'gmail', 'fastmail', 'inbox', 'external', 'connect', 'email', 'account', 'app password', 'link'] },
  { id: 'email-link.add', section: 'email-link', label: 'Connect an Inbox', description: 'Add a Gmail / Fastmail / IMAP account owned by one workspace', keywords: ['add', 'connect', 'imap', 'gmail', 'app password', 'drafts', 'external inbox', 'imap host', 'port', 'tls', 'primary', 'workspace'] },
  { id: 'email-link.oauth', section: 'email-link', label: 'Connect with Google or Microsoft', description: 'Link Gmail or Microsoft (Outlook / 365) via OAuth — no app-password', keywords: ['oauth', 'google', 'gmail', 'microsoft', 'outlook', '365', 'office', 'device code', 'sign in', 'connect', 'link', 'consent'] },
  { id: 'email-link.access', section: 'email-link', label: 'Inbox Access', description: 'Grant other workspaces read or read+draft access to a connected inbox', keywords: ['access', 'grant', 'revoke', 'share', 'read', 'draft', 'workspace', 'permission', 'primary', 'members', 'transfer'] },
]

const TLS_IMPLICIT = 'implicit-tls'
const TLS_STARTTLS = 'starttls'

const TLS_OPTIONS = [
  { value: TLS_IMPLICIT, label: 'Implicit TLS (993)' },
  { value: TLS_STARTTLS, label: 'STARTTLS (143)' },
]

type Selection = { kind: 'add' } | { kind: 'inbox'; address: string }

// ── Small shared bits ────────────────────────────────────────────────────

function SectionTitle({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
      {children}
    </h3>
  )
}

/** Connect-health → status-token color. */
function statusColor(status: string): string {
  switch (status) {
    case 'connected':
    case 'running':
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

/** Copy-to-clipboard with a transient "Copied" flash (mirrors the Email
 *  Hosting page's CopyButton). */
function CopyButton({ text, title }: { text: string; title?: string }): React.JSX.Element {
  const [copied, setCopied] = useState(false)
  return (
    <button
      type="button"
      title={title ?? 'Copy code'}
      onClick={() => {
        void navigator.clipboard
          .writeText(text)
          .then(() => {
            setCopied(true)
            window.setTimeout(() => setCopied(false), 1200)
          })
          .catch(() => {
            useToastStore.getState().addToast('Copy failed', 'error')
          })
      }}
      className="px-1.5 py-0.5 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-accent)] border border-[var(--color-border)] transition-colors cursor-pointer flex-shrink-0"
    >
      {copied ? 'Copied' : 'Copy'}
    </button>
  )
}

// ── OAuth connect (Gmail / Microsoft — no app-password, O4 flows) ─────────
//
// The provider-owned path: no host/port/password to type. `start` returns a
// DEVICE flow (Microsoft — show a code + verification URL, poll) or a
// LOOPBACK flow (Gmail — the daemon opened the local browser, poll). Gmail
// against a remote daemon 409s `remote_unsupported` — a teaching card, not a
// poll. Tokens/codes are NEVER rendered (only `userCode`/`verificationUrl`
// and the terminal `state`/`address`/`hint` ever come back).

const OAUTH_POLL_MS = 3000
const OAUTH_MAX_MS = 10 * 60 * 1000 // hard bound if the provider gives none

type OauthProvider = 'gmail' | 'microsoft'

type OauthFlow =
  | { kind: 'device'; provider: 'microsoft'; linkId: string; userCode: string; verificationUrl: string; deadlineMs: number }
  | { kind: 'loopback'; provider: 'gmail'; linkId: string; hint?: string; deadlineMs: number }

type OauthResult =
  | { kind: 'connected'; address?: string }
  | { kind: 'failed'; state: 'denied' | 'expired' | 'error'; hint?: string }
  | { kind: 'remote_unsupported'; message: string }
  | { kind: 'error'; message: string }

function OauthConnect({
  canMutate,
  onConnected,
}: {
  canMutate: boolean
  /** Refresh the connected-inbox list + select the new inbox (dismisses the card). */
  onConnected: (address: string) => void
}): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)

  const [project, setProject] = useState('')
  const [address, setAddress] = useState('')
  const [busy, setBusy] = useState<OauthProvider | null>(null)
  const [flow, setFlow] = useState<OauthFlow | null>(null)
  const [result, setResult] = useState<OauthResult | null>(null)

  const canStart = canMutate && busy === null && project.trim().length > 0 && address.trim().length > 0

  const start = useCallback(
    async (provider: OauthProvider): Promise<void> => {
      if (!canStart) return
      setBusy(provider)
      setFlow(null)
      setResult(null)
      try {
        const res = await linkOauthStart({
          address: address.trim(),
          provider,
          workspace: project.trim(),
        })
        if (res.flow === 'device') {
          const bound = Number.isFinite(res.expiresIn) && res.expiresIn > 0 ? res.expiresIn * 1000 : OAUTH_MAX_MS
          setFlow({
            kind: 'device',
            provider: 'microsoft',
            linkId: res.linkId,
            userCode: res.userCode,
            verificationUrl: res.verificationUrl,
            deadlineMs: Date.now() + Math.min(bound, OAUTH_MAX_MS),
          })
        } else {
          setFlow({
            kind: 'loopback',
            provider: 'gmail',
            linkId: res.linkId,
            hint: res.hint,
            deadlineMs: Date.now() + OAUTH_MAX_MS,
          })
        }
      } catch (e) {
        // Gmail on a remote/headless daemon → HTTP 409 remote_unsupported:
        // the daemon's browser opener can't spawn here. Teach, don't poll.
        const info = mailErrorInfo(e)
        if (info.code === 'remote_unsupported') {
          setResult({
            kind: 'remote_unsupported',
            message:
              info.hint ??
              'Gmail must be linked on the machine running this K2 daemon. You’re connected to a remote daemon, so the browser can’t open here.',
          })
        } else {
          setResult({ kind: 'error', message: mailErrorMessage(e) })
        }
      } finally {
        setBusy(null)
      }
    },
    [canStart, address, project],
  )

  // ── Poll link/oauth/status every 3s while a flow is live. Bounded by the
  //    provider's expiry (or 10 min). Terminal states clear the flow, which
  //    tears down the interval via cleanup — no leaked timers. ────────────
  const onConnectedRef = useRef(onConnected)
  onConnectedRef.current = onConnected
  useEffect(() => {
    if (!flow) return
    let cancelled = false
    const tick = async (): Promise<void> => {
      if (cancelled) return
      if (Date.now() > flow.deadlineMs) {
        setResult({ kind: 'failed', state: 'expired' })
        setFlow(null)
        return
      }
      try {
        const s = await linkOauthStatus(flow.linkId)
        if (cancelled) return
        if (s.state === 'connected') {
          setResult({ kind: 'connected', address: s.address })
          setFlow(null)
          onConnectedRef.current(s.address ?? address.trim())
        } else if (s.state === 'denied' || s.state === 'expired' || s.state === 'error') {
          setResult({ kind: 'failed', state: s.state, hint: s.hint })
          setFlow(null)
        }
        // 'pending' → keep polling.
      } catch (e) {
        if (cancelled) return
        setResult({ kind: 'error', message: mailErrorMessage(e) })
        setFlow(null)
      }
    }
    const id = window.setInterval(() => void tick(), OAUTH_POLL_MS)
    void tick() // fire immediately so the user isn't waiting 3s for the first read
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [flow, address])

  const reset = useCallback((): void => {
    setFlow(null)
    setResult(null)
  }, [])

  const inputCls =
    'px-2 py-1 text-[11px] font-mono bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)] no-drag disabled:opacity-50'

  const disabledField = busy !== null || flow !== null

  return (
    <div className="space-y-3" data-settings-id="email-link.oauth">
      <SectionTitle>Connect with Google or Microsoft</SectionTitle>
      <p className="text-[11px] text-[var(--color-text-muted)]">
        No app-password needed — approve access in your provider and K2 stores the connection in the
        daemon vault. Pick the <strong>Primary</strong> workspace and enter the account&rsquo;s email
        so K2 can label the pending link.
      </p>

      <div className="grid grid-cols-2 gap-3">
        <label className="flex flex-col gap-0.5 min-w-0">
          <span className="text-[10px] text-[var(--color-text-secondary)]">Primary workspace</span>
          <SettingDropdown
            value={project}
            placeholder="Pick a workspace…"
            options={projects.map((p) => ({ value: p.id, label: p.name }))}
            onChange={(v) => setProject(v)}
            menuAlign="left"
            className={disabledField || !canMutate ? 'opacity-50 pointer-events-none' : undefined}
          />
        </label>
        <label className="flex flex-col gap-0.5 min-w-0">
          <span className="text-[10px] text-[var(--color-text-secondary)]">Email address</span>
          <input
            type="email"
            value={address}
            disabled={!canMutate || disabledField}
            onChange={(e) => setAddress(e.target.value)}
            placeholder="you@gmail.com / you@outlook.com"
            className={inputCls}
          />
        </label>
      </div>

      <div className="flex items-center gap-2 flex-wrap">
        <button
          type="button"
          disabled={!canStart}
          onClick={() => void start('gmail')}
          title={canMutate ? 'Open Google consent on this machine' : 'Not available in viewer mode'}
          className="px-3 py-1.5 text-xs font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0"
        >
          {busy === 'gmail' ? 'Starting…' : 'Connect Gmail'}
        </button>
        <button
          type="button"
          disabled={!canStart}
          onClick={() => void start('microsoft')}
          title={canMutate ? 'Get a device code to approve in any browser' : 'Not available in viewer mode'}
          className="px-3 py-1.5 text-xs font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0"
        >
          {busy === 'microsoft' ? 'Starting…' : 'Connect Microsoft (Outlook / 365)'}
        </button>
      </div>

      {/* ── Live flow / result card ── */}
      {flow?.kind === 'device' && (
        <div className="border border-[var(--color-border)] px-3 py-3 space-y-2">
          <p className="text-[11px] text-[var(--color-text-secondary)]">
            Go to{' '}
            <button
              type="button"
              onClick={() => void openUrl(flow.verificationUrl)}
              className="font-mono text-[var(--color-accent)] hover:underline cursor-pointer"
            >
              {flow.verificationUrl}
            </button>{' '}
            and enter this code:
          </p>
          <div className="flex items-center gap-2">
            <span className="px-2 py-1 text-sm font-mono tracking-widest bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] select-all">
              {flow.userCode}
            </span>
            <CopyButton text={flow.userCode} title="Copy code" />
            <button
              type="button"
              onClick={() => void openUrl(flow.verificationUrl)}
              className="px-2 py-1 text-[10px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer flex-shrink-0"
            >
              Open page
            </button>
          </div>
          <p className="text-[10px] text-[var(--color-text-muted)]">
            Waiting for approval… this card updates automatically.
          </p>
        </div>
      )}

      {flow?.kind === 'loopback' && (
        <div className="border border-[var(--color-border)] px-3 py-3 space-y-1">
          <p className="text-[11px] text-[var(--color-text-secondary)]">
            A browser window opened on this machine — approve access there.
          </p>
          <p className="text-[10px] text-[var(--color-text-muted)]">
            {flow.hint ?? 'Waiting for approval… this card updates automatically.'}
          </p>
        </div>
      )}

      {result?.kind === 'connected' && (
        <div className="border border-[color-mix(in_srgb,var(--color-status-ok)_30%,transparent)] px-3 py-2 flex items-center gap-2">
          <p className="text-[11px] text-[var(--color-status-ok)]">
            Connected{result.address ? <span className="font-mono"> {result.address}</span> : ''}.
          </p>
        </div>
      )}

      {result?.kind === 'failed' && (
        <div className="border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] px-3 py-2 space-y-2">
          <p className="text-[11px] text-[var(--color-status-error-soft)]">
            {result.state === 'denied'
              ? 'Access was denied.'
              : result.state === 'expired'
                ? 'The request expired before it was approved.'
                : 'Something went wrong linking the account.'}
            {result.hint ? ` ${result.hint}` : ''}
          </p>
          <button
            type="button"
            onClick={reset}
            className="px-2 py-1 text-[10px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer"
          >
            Try again
          </button>
        </div>
      )}

      {result?.kind === 'remote_unsupported' && (
        <div className="border border-[color-mix(in_srgb,var(--color-status-warn)_30%,transparent)] px-3 py-2">
          <p className="text-[11px] text-[var(--color-text-secondary)]">{result.message}</p>
        </div>
      )}

      {result?.kind === 'error' && (
        <div className="border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] px-3 py-2 space-y-2">
          <p className="text-[11px] text-[var(--color-status-error-soft)] break-words">{result.message}</p>
          <button
            type="button"
            onClick={reset}
            className="px-2 py-1 text-[10px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer"
          >
            Try again
          </button>
        </div>
      )}
    </div>
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
      const res = await addLinkedInbox({
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
          Connect one of your email accounts and pick the workspace that will be its{' '}
          <strong>Primary</strong>. K2 connects over IMAP to verify the account now; the Primary
          manages the account and can later grant other workspaces read (or read + draft) access.
          Agents read the inbox and save reply drafts with <span className="font-mono">k2 mail</span>{' '}
          — you review and send from your own mail client. K2 never sends from the account.
        </p>
      </div>

      <OauthConnect canMutate={canMutate} onConnected={onAdded} />

      <div className="border-t border-[var(--color-border)] pt-4 space-y-3">
        <SectionTitle>Or connect manually over IMAP (app-password)</SectionTitle>
        <div className="grid grid-cols-2 gap-3">
          {field(
            'Primary workspace',
            <SettingDropdown
              value={project}
              placeholder="Pick a workspace…"
              options={projects.map((p) => ({ value: p.id, label: p.name }))}
              onChange={(v) => setProject(v)}
              menuAlign="left"
              className={!canMutate || busy ? 'opacity-50 pointer-events-none' : undefined}
            />,
            'The workspace that manages the account and who else can use it.',
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
              <SettingDropdown
                value={tls}
                options={TLS_OPTIONS}
                onChange={(v) => setTls(v)}
                className={!canMutate || busy ? 'opacity-50 pointer-events-none' : undefined}
              />,
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
  patchInbox,
}: {
  inbox: Inbox
  canMutate: boolean
  onRemoved: () => void
  /** Optimistically patch this inbox's row in the parent list. */
  patchInbox: (address: string, patch: (i: Inbox) => Inbox) => void
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
      await removeLinkedInbox(inbox.address)
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
          Primary{' '}
          <span className="text-[var(--color-text-secondary)]">
            {inbox.primary.workspace ?? 'unknown workspace'}
          </span>
          {' · '}the Primary manages the account and who can use it. Workspaces with access read it
          and draft replies with <span className="font-mono">k2 mail</span> — nobody sends from it.
        </p>
      </div>

      <div className="space-y-2">
        <SectionTitle>Connection</SectionTitle>
        <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
          {detailRow('Primary', inbox.primary.workspace ?? '—')}
          {inbox.host && detailRow('IMAP host', <span className="font-mono">{inbox.host}</span>)}
          {inbox.tls && detailRow('TLS', inbox.tls)}
          {detailRow('Status', inbox.status)}
        </div>
      </div>

      <InboxAccessPanel inbox={inbox} canMutate={canMutate} patchInbox={patchInbox} />

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

  const [inboxes, setInboxes] = useState<Inbox[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [revision, setRevision] = useState(0)
  const bump = useCallback(() => setRevision((r) => r + 1), [])
  const [selection, setSelection] = useState<Selection>({ kind: 'add' })

  // ── Load the connected-inbox table from the unified catalog, filtered
  //    to LINKED accounts. Cross-platform: no status/supported gate. ────
  useEffect(() => {
    let cancelled = false
    fetchInboxes()
      .then((r) => {
        if (cancelled) return
        setInboxes(r.filter((i) => i.source === 'linked'))
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

  // Optimistic patch of a single inbox row (primary/grants) — mutates the
  // already-loaded list in place, no refetch (no fetch-in-render).
  const patchInbox = useCallback(
    (address: string, patch: (i: Inbox) => Inbox): void => {
      setInboxes((prev) =>
        prev ? prev.map((i) => (i.address === address ? patch(i) : i)) : prev,
      )
    },
    [],
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
                    key={i.address}
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
                      {i.primary.workspace ?? 'unknown workspace'}
                      {i.grants.length > 0 ? ` +${i.grants.length}` : ''}
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
              patchInbox={patchInbox}
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
