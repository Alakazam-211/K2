// Settings → Email (K2 Mail S7, prd-email-server-v1 §13) — the owner
// surface: server lifecycle, domains + DNS record tables, addresses,
// and the outbound-approvals queue. Master-detail (the
// Projects/ProjectSettings shell idiom): left column = server status
// card + Approvals/Addresses nav + the domain list + [Add domain];
// right panel = the selection's management surface.
//
// CAPABILITY GATING (pre-mortem #15 — BINDING): the whole page keys off
// GET /cli/mail/status `supported` — the DAEMON's report, never
// navigator.platform (a Mac app driving a remote Linux daemon must see
// the real page). When unsupported the page renders fully from the
// static SAMPLE fixture (email-api.ts) under the D3 banner, with every
// mutating control disabled and NO further network calls.
//
// LIVE UPDATES: the daemon emits mail:* hook events
// (server-state-changed / domain-status-changed /
// send-approval-requested / send-decided) on the legacy /events WS,
// which src-tauri re-emits as Tauri events — we listen and bump a
// revision that refetches. NOTE: those hooks are not yet mirrored onto
// the host-aware /cli/sessions/events bus (the feedback_changed
// pattern), so against a REMOTE host live updates rely on the manual
// refresh affordances ([Check now], panel switches) until a daemon
// slice adds the mirror. While an enable run is in flight we
// additionally poll /cli/mail/status (the route's documented
// enableProgress contract).
//
// UNTRUSTED CONTENT (§8.1/§8.4): approval items surface from/to/
// subject summaries only; the body is a bounded plain-text preview the
// daemon prepared, rendered ONLY behind an explicit expand, always as
// plain text (<pre>), never HTML, never auto-shown.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useProjectsStore } from '@/stores/projects'
import { useToastStore } from '@/stores/toast'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useWindowModeStore } from '@/stores/window-mode'
import type { SettingEntry } from '../searchManifest'
import {
  ENABLE_STEPS,
  SAMPLE_ADDRESSES,
  SAMPLE_APPROVALS,
  SAMPLE_DOMAINS,
  SAMPLE_DOMAIN_DETAILS,
  SAMPLE_PREFLIGHT,
  addDomain,
  approveOutbound,
  checkDomainNow,
  denyOutbound,
  disableServer,
  enableServer,
  fetchAllAddresses,
  fetchApprovals,
  fetchDoctor,
  fetchDomainDetail,
  fetchDomains,
  fetchMailConfig,
  fetchMailStatus,
  fetchOutbox,
  fetchPreflight,
  isNotBuilt,
  mailErrorInfo,
  mailErrorMessage,
  removeDomain,
  retireAddress,
  setMailConfig,
  uninstallServer,
  type AddressRow,
  type ApprovalItem,
  type DnsRecordRow,
  type DomainDetail,
  type DomainSummary,
  type MailStatus,
  type OutboundItem,
  type PreflightReport,
} from './email-api'

export const EMAIL_MANIFEST: SettingEntry[] = [
  { id: 'email.server', section: 'email', label: 'Email Server', description: 'Enable and supervise the mail server (Linux deployments)', keywords: ['mail', 'email', 'smtp', 'stalwart', 'server', 'enable', 'preflight'] },
  { id: 'email.domains', section: 'email', label: 'Email Domains', description: 'Add domains and verify MX / SPF / DKIM / DMARC records', keywords: ['domain', 'dns', 'mx', 'spf', 'dkim', 'dmarc', 'ptr', 'records', 'verify', 'zone'] },
  { id: 'email.addresses', section: 'email', label: 'Email Addresses', description: 'Agent mailboxes per domain — caps, holders, retirement', keywords: ['address', 'mailbox', 'agent', 'cap', 'retire', 'mint'] },
  { id: 'email.approvals', section: 'email', label: 'Send Approvals', description: 'Approve or deny agent outbound email', keywords: ['approval', 'approvals', 'outbox', 'send', 'deny', 'approve', 'queue', 'outbound'] },
  { id: 'email.send-mode', section: 'email', label: 'Send Mode & Relay', description: 'Direct send or smart-host relay, per domain', keywords: ['relay', 'smtp relay', 'direct', 'send mode', 'smart host', 'receive-only'] },
]

// The D3 banner — exact copy is a locked product decision.
const MAC_BANNER =
  'THIS FEATURE ONLY WORKS ON LINUX DEPLOYMENTS, THIS PAGE IS JUST HERE FOR EXAMPLE PURPOSES.'

type Selection =
  | { kind: 'server' }
  | { kind: 'approvals' }
  | { kind: 'addresses' }
  | { kind: 'domain'; domain: string }

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

/** Server-state → status-token color. */
function stateColor(state: string): string {
  switch (state) {
    case 'running':
      return 'var(--color-status-ok)'
    case 'degraded':
      return 'var(--color-status-warn)'
    case 'installing':
      return 'var(--color-status-working)'
    case 'error':
      return 'var(--color-status-error-soft)'
    default:
      // not-installed / stopped / disabled
      return 'var(--color-text-muted)'
  }
}

/** Per-record (and per-domain) verification chip. */
function recordChip(status: string): { label: string; color: string } {
  switch (status) {
    case 'valid':
    case 'verified':
      return { label: status === 'verified' ? 'Verified' : 'Valid', color: 'var(--color-status-ok)' }
    case 'missing':
      return { label: 'Missing', color: 'var(--color-status-error-soft)' }
    case 'wrong':
      return { label: 'Wrong', color: 'var(--color-status-error-soft)' }
    case 'unknown':
      return { label: 'Unknown', color: 'var(--color-status-warn)' }
    case 'unverifiable':
      return { label: 'Manual', color: 'var(--color-status-warn)' }
    case 'optional':
      return { label: 'Optional', color: 'var(--color-text-muted)' }
    default:
      return { label: 'Pending', color: 'var(--color-text-muted)' }
  }
}

function StatusChip({ status }: { status: string }): React.JSX.Element {
  const { label, color } = recordChip(status)
  return (
    <span
      className="text-[9px] font-semibold px-1.5 py-0.5 flex-shrink-0 uppercase tracking-wide"
      style={{ color, backgroundColor: `color-mix(in srgb, ${color} 14%, transparent)` }}
    >
      {label}
    </span>
  )
}

/** Copy-to-clipboard with a transient "Copied" flash. */
function CopyButton({ text, title }: { text: string; title?: string }): React.JSX.Element {
  const [copied, setCopied] = useState(false)
  return (
    <button
      type="button"
      title={title ?? 'Copy value'}
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

// ── Server panel ─────────────────────────────────────────────────────────

function PreflightChecklist({ report }: { report: PreflightReport }): React.JSX.Element {
  const mark = (status: string): { glyph: string; color: string } => {
    switch (status) {
      case 'pass':
        return { glyph: '✓', color: 'var(--color-status-ok)' }
      case 'fail':
        return { glyph: '✖', color: 'var(--color-status-error-soft)' }
      case 'warn':
        return { glyph: '!', color: 'var(--color-status-warn)' }
      case 'skipped':
        return { glyph: '·', color: 'var(--color-text-muted)' }
      default:
        return { glyph: '?', color: 'var(--color-text-muted)' }
    }
  }
  return (
    <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
      {report.checks.map((c) => {
        const m = mark(c.status)
        return (
          <div key={c.id} className="flex items-start gap-2 px-3 py-1.5">
            <span className="w-3 flex-shrink-0 text-xs font-bold" style={{ color: m.color }}>
              {m.glyph}
            </span>
            <div className="flex-1 min-w-0">
              <span className="text-xs text-[var(--color-text-primary)]">{c.label}</span>
              {c.detail && (
                <p className="text-[10px] text-[var(--color-text-muted)] break-words">{c.detail}</p>
              )}
            </div>
          </div>
        )
      })}
      {report.portPlan && (
        <div className="px-3 py-1.5 text-[10px] text-[var(--color-text-muted)]">
          TLS/port plan: <span className="text-[var(--color-text-secondary)]">{report.portPlan}</span>
        </div>
      )}
    </div>
  )
}

function EnableProgress({ status }: { status: MailStatus }): React.JSX.Element {
  const steps = status.enableProgress?.steps ?? {}
  const current = status.enableProgress?.current
  return (
    <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
      {ENABLE_STEPS.map((s) => {
        const done = s.id in steps
        const active = current === s.id
        return (
          <div key={s.id} className="flex items-center gap-2 px-3 py-1">
            <span
              className="w-3 flex-shrink-0 text-xs font-bold"
              style={{
                color: done
                  ? 'var(--color-status-ok)'
                  : active
                    ? 'var(--color-status-working)'
                    : 'var(--color-text-muted)',
              }}
            >
              {done ? '✓' : active ? '›' : '·'}
            </span>
            <span
              className={`text-[11px] ${
                done || active
                  ? 'text-[var(--color-text-primary)]'
                  : 'text-[var(--color-text-muted)]'
              }`}
            >
              {s.label}
              {active && status.state === 'installing' ? '…' : ''}
            </span>
          </div>
        )
      })}
    </div>
  )
}

function ServerPanel({
  status,
  sample,
  canMutate,
  onChanged,
}: {
  status: MailStatus
  sample: boolean
  canMutate: boolean
  onChanged: () => void
}): React.JSX.Element {
  const [preflight, setPreflight] = useState<PreflightReport | null>(sample ? SAMPLE_PREFLIGHT : null)
  const [preflightError, setPreflightError] = useState<string | null>(null)
  const [preflightBusy, setPreflightBusy] = useState(false)
  const [hostname, setHostname] = useState('')
  const [busy, setBusy] = useState(false)
  const [enableError, setEnableError] = useState<string | null>(null)
  // Uninstall danger-zone locals.
  const [purge, setPurge] = useState(false)
  const [confirmHost, setConfirmHost] = useState('')

  const notInstalled = status.state === 'not-installed'
  const installing = status.state === 'installing'
  const hasProgress =
    status.enableProgress !== null &&
    Object.keys(status.enableProgress?.steps ?? {}).length > 0

  // Preflight renders before [Enable] (§5.1) — fetch it for the
  // not-installed view on a live daemon. One-shot per mount/state.
  useEffect(() => {
    if (sample || !notInstalled) return
    let cancelled = false
    setPreflightBusy(true)
    fetchPreflight()
      .then((r) => {
        if (cancelled) return
        setPreflight(r)
        setPreflightError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setPreflightError(mailErrorMessage(e))
      })
      .finally(() => {
        if (!cancelled) setPreflightBusy(false)
      })
    return () => {
      cancelled = true
    }
  }, [sample, notInstalled])

  const runPreflight = useCallback(async (): Promise<void> => {
    setPreflightBusy(true)
    try {
      setPreflight(await fetchPreflight())
      setPreflightError(null)
    } catch (e) {
      setPreflightError(mailErrorMessage(e))
    } finally {
      setPreflightBusy(false)
    }
  }, [])

  const doEnable = useCallback(async (): Promise<void> => {
    const host = hostname.trim() || status.hostname || ''
    if (!host) {
      setEnableError('Enter the mail hostname first (e.g. mail.acme.dev).')
      return
    }
    setBusy(true)
    setEnableError(null)
    try {
      const res = await enableServer(host)
      if (res.ok === false) {
        // Failing preflight returns 200 + the report (§5.1 hard stops).
        setEnableError(res.error?.hint ?? 'preflight found hard stops — fix them and re-enable')
        if (res.report) setPreflight(res.report)
      }
      onChanged()
    } catch (e) {
      setEnableError(mailErrorMessage(e))
      onChanged()
    } finally {
      setBusy(false)
    }
  }, [hostname, status.hostname, onChanged])

  const doDisable = useCallback(async (): Promise<void> => {
    const confirmed = await useConfirmDialogStore.getState().confirm({
      title: 'Disable the email server?',
      message:
        'Mail data is kept, but every verified domain’s MX record will point at a ' +
        'STOPPED server — inbound mail bounces until you re-enable or update DNS.',
      confirmLabel: 'Disable',
      destructive: true,
    })
    if (!confirmed) return
    setBusy(true)
    try {
      const res = await disableServer()
      if (res.warning) useToastStore.getState().addToast(res.warning, 'error', 10000)
    } catch (e) {
      useToastStore.getState().addToast(`Disable failed: ${mailErrorMessage(e)}`, 'error')
    } finally {
      setBusy(false)
      onChanged()
    }
  }, [onChanged])

  const doUninstall = useCallback(async (): Promise<void> => {
    const confirmed = await useConfirmDialogStore.getState().confirm({
      title: 'Uninstall the email server?',
      message: purge
        ? 'This removes the server AND permanently deletes all mail data.'
        : 'This removes the server binary and unit. Mail data stays on disk.',
      confirmLabel: 'Uninstall',
      destructive: true,
    })
    if (!confirmed) return
    setBusy(true)
    try {
      await uninstallServer(purge, purge ? confirmHost.trim() : undefined)
      setPurge(false)
      setConfirmHost('')
    } catch (e) {
      useToastStore.getState().addToast(`Uninstall failed: ${mailErrorMessage(e)}`, 'error')
    } finally {
      setBusy(false)
      onChanged()
    }
  }, [purge, confirmHost, onChanged])

  return (
    <div className="grid gap-6 grid-cols-[minmax(0,42rem)]">
      <div className="min-w-0" data-settings-id="email.server">
        <h2 className="text-base font-medium text-[var(--color-text-primary)]">Email server</h2>
        <p className="text-[11px] text-[var(--color-text-muted)] mt-1">
          K2 installs and supervises a mail server on this deployment. Agents mint addresses on
          your domains, read their mail, and send under your governance.
        </p>
      </div>

      {/* ── Status ── */}
      <div className="space-y-2">
        <SectionTitle>Status</SectionTitle>
        <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
          <div className="flex items-center gap-2 px-3 py-2">
            <span
              className="w-2 h-2 rounded-full flex-shrink-0"
              style={{ backgroundColor: stateColor(status.state) }}
            />
            <span className="text-xs text-[var(--color-text-primary)]">{status.state}</span>
            <span className="flex-1" />
            <span className="text-[10px] text-[var(--color-text-muted)]">
              {status.version ? `v${status.version}` : `pinned v${status.pinnedVersion}`}
            </span>
          </div>
          {status.hostname && (
            <div className="flex items-center gap-3 px-3 py-2">
              <span className="text-[11px] text-[var(--color-text-secondary)] w-24 flex-shrink-0">
                Mail hostname
              </span>
              <span className="text-xs font-mono text-[var(--color-text-primary)] truncate">
                {status.hostname}
              </span>
            </div>
          )}
          {status.portPlan && (
            <div className="flex items-center gap-3 px-3 py-2">
              <span className="text-[11px] text-[var(--color-text-secondary)] w-24 flex-shrink-0">
                TLS/port plan
              </span>
              <span className="text-xs text-[var(--color-text-primary)]">{status.portPlan}</span>
            </div>
          )}
          {status.lastError && (
            <div className="px-3 py-2">
              <p className="text-[11px] text-[var(--color-status-error-soft)] break-words">
                {status.lastError}
              </p>
            </div>
          )}
        </div>
      </div>

      {/* ── Enable / progress ── */}
      {(notInstalled || installing || status.state === 'error' || status.state === 'disabled' || status.state === 'stopped') && (
        <div className="space-y-2">
          <SectionTitle>{notInstalled ? 'Enable' : installing ? 'Installing' : 'Resume'}</SectionTitle>

          {notInstalled && (
            <>
              <p className="text-[11px] text-[var(--color-text-muted)]">
                Preflight runs first — hard stops block the install; warnings don&rsquo;t.
              </p>
              {preflightError ? (
                <p className="text-[11px] text-[var(--color-status-error-soft)]">
                  Preflight failed to run: {preflightError}
                </p>
              ) : preflight === null ? (
                <p className="text-[11px] text-[var(--color-text-muted)]">Running preflight…</p>
              ) : (
                <PreflightChecklist report={preflight} />
              )}
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  value={hostname}
                  disabled={!canMutate || busy}
                  onChange={(e) => setHostname(e.target.value)}
                  placeholder="mail.acme.dev"
                  className="flex-1 min-w-0 px-2 py-1.5 text-xs font-mono bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)] no-drag disabled:opacity-50"
                />
                <button
                  type="button"
                  disabled={!canMutate || busy}
                  onClick={() => void doEnable()}
                  title={canMutate ? 'Preflight-gate, then install' : 'Not available on this deployment'}
                  className="px-3 py-1.5 text-xs font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0"
                >
                  {busy ? 'Starting…' : 'Enable Email Server'}
                </button>
                <button
                  type="button"
                  disabled={preflightBusy || sample}
                  onClick={() => void runPreflight()}
                  className="px-2 py-1.5 text-[10px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer disabled:opacity-50 flex-shrink-0"
                >
                  {preflightBusy ? 'Checking…' : 'Re-run preflight'}
                </button>
              </div>
              <p className="text-[10px] text-[var(--color-text-muted)] opacity-70">
                Before anything works, point <span className="font-mono">A &lt;hostname&gt; → &lt;box IP&gt;</span> at
                this box and set its reverse DNS (PTR) to the hostname at your VPS provider.
              </p>
            </>
          )}

          {(installing || status.state === 'error') && (
            <>
              {(installing || hasProgress) && <EnableProgress status={status} />}
              {status.state === 'error' && (
                <div className="flex items-center gap-2">
                  <button
                    type="button"
                    disabled={!canMutate || busy}
                    onClick={() => void doEnable()}
                    className="px-3 py-1.5 text-xs font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer disabled:opacity-50"
                  >
                    Resume install
                  </button>
                  <span className="text-[10px] text-[var(--color-text-muted)]">
                    Re-enabling resumes from the last completed step.
                  </span>
                </div>
              )}
            </>
          )}

          {(status.state === 'disabled' || status.state === 'stopped') && (
            <div className="flex items-center gap-2">
              <button
                type="button"
                disabled={!canMutate || busy}
                onClick={() => void doEnable()}
                className="px-3 py-1.5 text-xs font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer disabled:opacity-50"
              >
                Re-enable
              </button>
              <span className="text-[10px] text-[var(--color-text-muted)]">
                Mail data was kept — re-enabling brings existing domains back online.
              </span>
            </div>
          )}

          {enableError && (
            <p className="text-[11px] text-[var(--color-status-error-soft)] break-words">{enableError}</p>
          )}
        </div>
      )}

      {/* ── Danger zone (installed states only) ── */}
      {!notInstalled && !installing && (
        <div className="space-y-2 pb-6">
          <SectionTitle>Danger zone</SectionTitle>
          <div className="border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] divide-y divide-[var(--color-border)]">
            <div className="flex items-center gap-3 px-3 py-2">
              <div className="flex-1 min-w-0">
                <p className="text-xs text-[var(--color-text-primary)]">Disable</p>
                <p className="text-[10px] text-[var(--color-text-muted)]">
                  Stops the server, keeps all mail data. MX records will point at a dead port.
                </p>
              </div>
              <button
                type="button"
                disabled={!canMutate || busy}
                onClick={() => void doDisable()}
                className="px-2.5 py-1 text-[10px] font-medium text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error-soft)_10%,transparent)] transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0"
              >
                Disable
              </button>
            </div>
            <div className="px-3 py-2 space-y-1.5">
              <div className="flex items-center gap-3">
                <div className="flex-1 min-w-0">
                  <p className="text-xs text-[var(--color-text-primary)]">Uninstall</p>
                  <p className="text-[10px] text-[var(--color-text-muted)]">
                    Removes the server binary and unit.
                  </p>
                </div>
                <button
                  type="button"
                  disabled={
                    !canMutate ||
                    busy ||
                    (purge && confirmHost.trim() !== (status.hostname ?? ''))
                  }
                  onClick={() => void doUninstall()}
                  className="px-2.5 py-1 text-[10px] font-medium text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error-soft)_10%,transparent)] transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0"
                >
                  Uninstall
                </button>
              </div>
              <label className="flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)] cursor-pointer">
                <input
                  type="checkbox"
                  checked={purge}
                  disabled={!canMutate || busy}
                  onChange={(e) => setPurge(e.target.checked)}
                  className="no-drag"
                />
                Also delete all mail data (permanent)
              </label>
              {purge && (
                <input
                  type="text"
                  value={confirmHost}
                  disabled={!canMutate || busy}
                  onChange={(e) => setConfirmHost(e.target.value)}
                  placeholder={`Type ${status.hostname ?? 'the mail hostname'} to confirm`}
                  className="w-full px-2 py-1 text-[11px] font-mono bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-status-error-soft)] no-drag disabled:opacity-50"
                />
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  )
}

// ── Domain panel ─────────────────────────────────────────────────────────

function DnsRecordTable({
  records,
  advancedOpen,
  onToggleAdvanced,
}: {
  records: DnsRecordRow[]
  advancedOpen: boolean
  onToggleAdvanced: () => void
}): React.JSX.Element {
  const main = records.filter((r) => r.category !== 'advanced')
  const advanced = records.filter((r) => r.category === 'advanced')
  const row = (r: DnsRecordRow): React.JSX.Element => (
    <div key={r.id} className="px-3 py-2 space-y-1">
      <div className="flex items-center gap-2 min-w-0">
        <span className="text-[10px] font-semibold text-[var(--color-text-secondary)] w-12 flex-shrink-0">
          {r.type}
        </span>
        <span className="text-[11px] font-mono text-[var(--color-text-primary)] truncate flex-1" title={r.name}>
          {r.name}
        </span>
        <StatusChip status={r.status} />
        <CopyButton text={r.expected} title={`Copy the ${r.type} value`} />
      </div>
      <p className="text-[10px] text-[var(--color-text-muted)]">{r.purpose}</p>
      <div className="text-[10px] font-mono text-[var(--color-text-secondary)] break-all bg-[var(--color-bg)] border border-[var(--color-border)] px-2 py-1">
        {r.expectedDisplay}
      </div>
      {(r.status === 'wrong' || (r.status === 'missing' && (r.live?.length ?? 0) > 0)) && (
        <div className="text-[10px] border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] px-2 py-1 space-y-0.5">
          <p className="text-[var(--color-status-error-soft)] font-semibold">
            {r.status === 'wrong' ? 'DNS serves a different value:' : 'DNS has other records at this name:'}
          </p>
          {(r.live ?? []).map((v, i) => (
            <p key={i} className="font-mono text-[var(--color-text-secondary)] break-all">
              {v}
            </p>
          ))}
          <p className="text-[var(--color-text-muted)]">
            Expected value above — copy it exactly (watch for a registrar auto-appending your
            zone, or a DKIM value split incorrectly).
          </p>
        </div>
      )}
      {r.checkedAt != null && (
        <p className="text-[9px] text-[var(--color-text-muted)] opacity-70">
          checked {fmtDate(r.checkedAt)}
        </p>
      )}
    </div>
  )
  return (
    <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
      {main.map(row)}
      {advanced.length > 0 && (
        <>
          <button
            type="button"
            onClick={onToggleAdvanced}
            className="w-full text-left px-3 py-1.5 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer"
          >
            {advancedOpen ? '▾' : '▸'} Advanced, optional records ({advanced.length}) —
            autoconfig / MTA-STS / TLS-RPT / TLSA
          </button>
          {advancedOpen && advanced.map(row)}
        </>
      )}
    </div>
  )
}

function DomainPanel({
  domain,
  sample,
  canMutate,
  revision,
  onChanged,
  onRemoved,
}: {
  domain: string
  sample: boolean
  canMutate: boolean
  revision: number
  onChanged: () => void
  onRemoved: () => void
}): React.JSX.Element {
  const [detail, setDetail] = useState<DomainDetail | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [checking, setChecking] = useState(false)
  const [advancedOpen, setAdvancedOpen] = useState(false)
  const [purgeMail, setPurgeMail] = useState(false)
  const [busy, setBusy] = useState(false)

  // Sending block (S5-config routes may still be 501 — graceful state).
  const [modeDraft, setModeDraft] = useState<string | null>(null)
  const [relayHost, setRelayHost] = useState('')
  const [relayPort, setRelayPort] = useState('587')
  const [relayUser, setRelayUser] = useState('')
  const [relayPass, setRelayPass] = useState('')
  const [spfInclude, setSpfInclude] = useState('')
  const [configNote, setConfigNote] = useState<string | null>(null)
  const [configBusy, setConfigBusy] = useState(false)

  // Doctor card — 501 until S6.
  const [doctorNote, setDoctorNote] = useState<string | null>(null)

  useEffect(() => {
    setModeDraft(null)
    setConfigNote(null)
    setAdvancedOpen(false)
    if (sample) {
      setDetail(SAMPLE_DOMAIN_DETAILS[domain] ?? null)
      setError(null)
      setDoctorNote('Deliverability doctor ships with a later mail slice (S6).')
      return
    }
    let cancelled = false
    fetchDomainDetail(domain)
      .then((d) => {
        if (cancelled) return
        setDetail(d)
        setError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setError(mailErrorMessage(e))
      })
    fetchDoctor()
      .then(() => {
        if (!cancelled) setDoctorNote(null)
      })
      .catch((e) => {
        if (cancelled) return
        setDoctorNote(
          isNotBuilt(e)
            ? 'Deliverability doctor ships with a later mail slice (S6).'
            : `Doctor unavailable: ${mailErrorMessage(e)}`,
        )
      })
    return () => {
      cancelled = true
    }
  }, [domain, sample, revision])

  const checkNow = useCallback(async (): Promise<void> => {
    setChecking(true)
    try {
      setDetail(await checkDomainNow(domain))
      setError(null)
    } catch (e) {
      useToastStore.getState().addToast(`Check failed: ${mailErrorMessage(e)}`, 'error')
    } finally {
      setChecking(false)
      onChanged()
    }
  }, [domain, onChanged])

  const downloadZone = useCallback((): void => {
    if (!detail) return
    const blob = new Blob([detail.zoneFile], { type: 'text/plain' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `${detail.domain}.zone`
    a.click()
    URL.revokeObjectURL(url)
  }, [detail])

  const doRemove = useCallback(async (): Promise<void> => {
    const confirmed = await useConfirmDialogStore.getState().confirm({
      title: `Remove ${domain}?`,
      message:
        'This retires every address on the domain and destroys the domain in the mail server.' +
        (purgeMail ? ' Mail data will ALSO be purged.' : ' Mail data is kept unless purged.'),
      confirmLabel: 'Remove domain',
      destructive: true,
    })
    if (!confirmed) return
    setBusy(true)
    try {
      const res = await removeDomain(domain, purgeMail)
      useToastStore
        .getState()
        .addToast(
          `Removed ${domain} (${res.retiredAddresses} address${res.retiredAddresses === 1 ? '' : 'es'} retired)`,
          'success',
        )
      onRemoved()
    } catch (e) {
      useToastStore.getState().addToast(`Remove failed: ${mailErrorMessage(e)}`, 'error')
    } finally {
      setBusy(false)
    }
  }, [domain, purgeMail, onRemoved])

  const saveSendConfig = useCallback(async (): Promise<void> => {
    if (!detail) return
    const mode = modeDraft ?? detail.sendMode
    setConfigBusy(true)
    setConfigNote(null)
    try {
      const body: Record<string, unknown> = { domain, sendMode: mode }
      if (mode === 'relay') {
        body.relayHost = relayHost.trim()
        body.relayPort = Number(relayPort) || 587
        body.relayUser = relayUser.trim()
        if (relayPass) body.relayPass = relayPass
        if (spfInclude.trim()) body.spfInclude = spfInclude.trim()
      }
      await setMailConfig(body)
      setConfigNote('Saved.')
      onChanged()
    } catch (e) {
      if (isNotBuilt(e)) {
        setConfigNote(
          'Send-mode & relay configuration isn’t wired up yet (the mail config routes ' +
            'ship with a later slice) — this domain stays ' +
            `${detail.sendMode} for now.`,
        )
      } else {
        setConfigNote(mailErrorMessage(e))
      }
    } finally {
      setConfigBusy(false)
    }
  }, [detail, domain, modeDraft, relayHost, relayPort, relayUser, relayPass, spfInclude, onChanged])

  if (error) {
    return <p className="text-[11px] text-[var(--color-status-error-soft)]">Failed to load {domain}: {error}</p>
  }
  if (detail === null || detail.domain !== domain) {
    return <p className="text-xs text-[var(--color-text-muted)]">Loading…</p>
  }

  const effectiveMode = modeDraft ?? detail.sendMode

  return (
    <div className="grid gap-6 grid-cols-[minmax(0,44rem)]">
      {/* ── Header ── */}
      <div className="min-w-0" data-settings-id="email.domains">
        <div className="flex items-center gap-2 min-w-0">
          <h2 className="text-base font-medium text-[var(--color-text-primary)] truncate">
            {detail.domain}
          </h2>
          <StatusChip status={detail.status} />
        </div>
        <p className="text-[11px] text-[var(--color-text-muted)] mt-1">
          {detail.status === 'verified'
            ? `Verified ${fmtDate(detail.verifiedAt)} · re-checked daily`
            : 'Verifies when MX + SPF + at least one DKIM record are valid.'}
          {' · '}last checked {fmtDate(detail.lastCheckedAt)}
        </p>
        {/* Catch-all / plus-addressing indicators (§6.1 V1 defaults —
            per-domain values aren't surfaced by the daemon yet). */}
        <div className="flex items-center gap-1.5 mt-2">
          <span className="text-[9px] font-semibold px-1.5 py-0.5 uppercase tracking-wide text-[var(--color-status-ok)] bg-[color-mix(in_srgb,var(--color-status-ok)_12%,transparent)]">
            Plus-addressing on
          </span>
          <span className="text-[9px] font-semibold px-1.5 py-0.5 uppercase tracking-wide text-[var(--color-text-muted)] bg-[var(--color-bg-elevated)]">
            Catch-all off
          </span>
          {detail.dmarcNag && (
            <span
              className="text-[9px] font-semibold px-1.5 py-0.5 uppercase tracking-wide text-[var(--color-status-warn)] bg-[color-mix(in_srgb,var(--color-status-warn)_12%,transparent)]"
              title="DMARC is strongly recommended — it never blocks verification, but set it."
            >
              Set DMARC
            </span>
          )}
        </div>
      </div>

      {/* ── DNS records ── */}
      <div className="space-y-2">
        <div className="flex items-center gap-2">
          <SectionTitle>DNS records</SectionTitle>
          <span className="flex-1" />
          <button
            type="button"
            onClick={downloadZone}
            className="px-2 py-0.5 text-[10px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer"
          >
            Download zone file
          </button>
          <button
            type="button"
            disabled={sample || checking}
            onClick={() => void checkNow()}
            title={sample ? 'Not available on this deployment' : 'Re-verify every record now'}
            className="px-2 py-0.5 text-[10px] text-[var(--color-accent)] border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/10 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {checking ? 'Checking…' : 'Check now'}
          </button>
        </div>
        <DnsRecordTable
          records={detail.records}
          advancedOpen={advancedOpen}
          onToggleAdvanced={() => setAdvancedOpen((v) => !v)}
        />
        <p className="text-[10px] text-[var(--color-text-muted)] opacity-70">{detail.note}.</p>
      </div>

      {/* ── Sending (send mode + relay, §8.3) ── */}
      <div className="space-y-2" data-settings-id="email.send-mode">
        <SectionTitle>Sending</SectionTitle>
        <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
          <div className="flex items-center gap-3 px-3 py-2">
            <span className="text-[11px] text-[var(--color-text-secondary)] w-24 flex-shrink-0">
              Send mode
            </span>
            <select
              value={effectiveMode}
              disabled={!canMutate || configBusy}
              onChange={(e) => setModeDraft(e.target.value)}
              className="px-2 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
            >
              <option value="receive-only">Receive only</option>
              <option value="direct">Direct (from this box)</option>
              <option value="relay">Relay (smart host)</option>
            </select>
            {effectiveMode === 'direct' && (
              <span className="text-[10px] text-[var(--color-text-muted)]">
                Requires a passing deliverability-doctor grade.
              </span>
            )}
          </div>
          {effectiveMode === 'relay' && (
            <div className="px-3 py-2 grid grid-cols-2 gap-2">
              {(
                [
                  ['Host', relayHost, setRelayHost, 'smtp.mailgun.org'],
                  ['Port', relayPort, setRelayPort, '587'],
                  ['Username', relayUser, setRelayUser, 'postmaster@acme.dev'],
                ] as const
              ).map(([label, value, setter, placeholder]) => (
                <label key={label} className="flex flex-col gap-0.5 min-w-0">
                  <span className="text-[10px] text-[var(--color-text-secondary)]">{label}</span>
                  <input
                    type="text"
                    value={value}
                    disabled={!canMutate || configBusy}
                    onChange={(e) => setter(e.target.value)}
                    placeholder={placeholder}
                    className="px-2 py-1 text-[11px] font-mono bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)] no-drag disabled:opacity-50"
                  />
                </label>
              ))}
              <label className="flex flex-col gap-0.5 min-w-0">
                <span className="text-[10px] text-[var(--color-text-secondary)]">Password</span>
                <input
                  type="password"
                  value={relayPass}
                  disabled={!canMutate || configBusy}
                  onChange={(e) => setRelayPass(e.target.value)}
                  className="px-2 py-1 text-[11px] font-mono bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)] no-drag disabled:opacity-50"
                />
              </label>
              <label className="flex flex-col gap-0.5 min-w-0 col-span-2">
                <span className="text-[10px] text-[var(--color-text-secondary)]">
                  SPF include (from your relay provider&rsquo;s setup page)
                </span>
                <input
                  type="text"
                  value={spfInclude}
                  disabled={!canMutate || configBusy}
                  onChange={(e) => setSpfInclude(e.target.value)}
                  placeholder="include:mailgun.org"
                  className="px-2 py-1 text-[11px] font-mono bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)] no-drag disabled:opacity-50"
                />
              </label>
              <p className="col-span-2 text-[10px] text-[var(--color-text-muted)]">
                The relay&rsquo;s own DNS records (its DKIM CNAMEs etc.) must also be set at the
                provider — the SPF row above updates to include the relay once saved.
              </p>
            </div>
          )}
          <div className="flex items-center gap-2 px-3 py-2">
            <button
              type="button"
              disabled={!canMutate || configBusy}
              onClick={() => void saveSendConfig()}
              className="px-2.5 py-1 text-[10px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {configBusy ? 'Saving…' : 'Save sending config'}
            </button>
            {configNote && (
              <span className="text-[10px] text-[var(--color-text-muted)]">{configNote}</span>
            )}
          </div>
        </div>
      </div>

      {/* ── Deliverability doctor (S6) ── */}
      <div className="space-y-2">
        <SectionTitle>Deliverability doctor</SectionTitle>
        {doctorNote ? (
          <p className="text-[10px] text-[var(--color-text-muted)] italic">{doctorNote}</p>
        ) : (
          <p className="text-[10px] text-[var(--color-text-muted)]">No doctor runs yet.</p>
        )}
      </div>

      {/* ── Danger zone ── */}
      <div className="space-y-2 pb-6">
        <SectionTitle>Danger zone</SectionTitle>
        <div className="border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] px-3 py-2 space-y-1.5">
          <div className="flex items-center gap-3">
            <div className="flex-1 min-w-0">
              <p className="text-xs text-[var(--color-text-primary)]">Remove this domain</p>
              <p className="text-[10px] text-[var(--color-text-muted)]">
                Retires all its addresses and destroys the domain in the mail server.
              </p>
            </div>
            <button
              type="button"
              disabled={!canMutate || busy}
              onClick={() => void doRemove()}
              className="px-2.5 py-1 text-[10px] font-medium text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error-soft)_10%,transparent)] transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0"
            >
              Remove domain
            </button>
          </div>
          <label className="flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)] cursor-pointer">
            <input
              type="checkbox"
              checked={purgeMail}
              disabled={!canMutate || busy}
              onChange={(e) => setPurgeMail(e.target.checked)}
              className="no-drag"
            />
            Also purge the domain&rsquo;s mail data
          </label>
        </div>
      </div>
    </div>
  )
}

// ── Addresses panel ──────────────────────────────────────────────────────

function AddressesPanel({
  sample,
  canMutate,
  revision,
  onChanged,
}: {
  sample: boolean
  canMutate: boolean
  revision: number
  onChanged: () => void
}): React.JSX.Element {
  const [rows, setRows] = useState<AddressRow[] | null>(sample ? SAMPLE_ADDRESSES : null)
  const [error, setError] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [capNote, setCapNote] = useState<string | null>(null)

  useEffect(() => {
    if (sample) {
      setRows(SAMPLE_ADDRESSES)
      setCapNote(null)
      return
    }
    let cancelled = false
    fetchAllAddresses()
      .then((r) => {
        if (cancelled) return
        setRows(r)
        setError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setError(mailErrorMessage(e))
      })
    // Cap configuration rides GET /cli/mail/config — 501 until the
    // config sub-slice lands; render the default policy instead.
    fetchMailConfig()
      .then(() => {
        if (!cancelled) setCapNote(null)
      })
      .catch((e) => {
        if (cancelled) return
        setCapNote(
          isNotBuilt(e)
            ? 'Per-workspace cap controls arrive with the mail config slice.'
            : `Config unavailable: ${mailErrorMessage(e)}`,
        )
      })
    return () => {
      cancelled = true
    }
  }, [sample, revision])

  const retire = useCallback(
    async (row: AddressRow): Promise<void> => {
      const confirmed = await useConfirmDialogStore.getState().confirm({
        title: `Retire ${row.address}?`,
        message:
          'The address stops receiving immediately; its mailbox data is kept for the ' +
          'retention window, then purged. The cap slot frees immediately.',
        confirmLabel: 'Retire address',
        destructive: true,
      })
      if (!confirmed) return
      setBusyId(row.id)
      try {
        await retireAddress(row.holderProjectId, row.address)
        // Optimistic: flip the row locally; the refetch confirms.
        setRows(
          (prev) =>
            prev?.map((r) =>
              r.id === row.id
                ? { ...r, status: 'retired', retiredAt: Math.floor(Date.now() / 1000) }
                : r,
            ) ?? null,
        )
        onChanged()
      } catch (e) {
        useToastStore.getState().addToast(`Retire failed: ${mailErrorMessage(e)}`, 'error')
      } finally {
        setBusyId(null)
      }
    },
    [onChanged],
  )

  // Group by domain (the address's suffix — the owner list is flat).
  const groups = useMemo(() => {
    const byDomain = new Map<string, AddressRow[]>()
    for (const r of rows ?? []) {
      const dom = r.address.split('@')[1] ?? '(unknown)'
      const list = byDomain.get(dom) ?? []
      list.push(r)
      byDomain.set(dom, list)
    }
    return [...byDomain.entries()].sort(([a], [b]) => a.localeCompare(b))
  }, [rows])

  return (
    <div className="grid gap-6 grid-cols-[minmax(0,44rem)]">
      <div className="min-w-0" data-settings-id="email.addresses">
        <h2 className="text-base font-medium text-[var(--color-text-primary)]">Addresses</h2>
        <p className="text-[11px] text-[var(--color-text-muted)] mt-1">
          Agent mailboxes across every domain. Agents mint their own with{' '}
          <span className="font-mono">k2 mail create</span> (default cap: 5 active addresses per
          agent, 0 = unlimited); plus-addressing gives unlimited tags without new addresses.
        </p>
        {capNote && <p className="text-[10px] text-[var(--color-text-muted)] italic mt-1">{capNote}</p>}
      </div>

      {error ? (
        <p className="text-[11px] text-[var(--color-status-error-soft)]">Failed to load addresses: {error}</p>
      ) : rows === null ? (
        <p className="text-xs text-[var(--color-text-muted)]">Loading…</p>
      ) : groups.length === 0 ? (
        <p className="text-[11px] text-[var(--color-text-muted)] italic">
          No addresses yet — agents mint them on verified domains with{' '}
          <span className="font-mono">k2 mail create &lt;name&gt;@&lt;domain&gt;</span>, or add a
          domain first.
        </p>
      ) : (
        groups.map(([dom, list]) => (
          <div key={dom} className="space-y-2">
            <SectionTitle>{dom}</SectionTitle>
            <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
              {list.map((r) => (
                <div key={r.id} className="flex items-center gap-2 px-3 py-2 min-w-0">
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2 min-w-0">
                      <span
                        className={`text-xs font-mono truncate ${
                          r.status === 'retired'
                            ? 'text-[var(--color-text-muted)] line-through'
                            : 'text-[var(--color-text-primary)]'
                        }`}
                      >
                        {r.address}
                      </span>
                      {r.status === 'retired' && (
                        <span className="text-[9px] font-semibold px-1.5 py-0.5 uppercase tracking-wide text-[var(--color-text-muted)] bg-[var(--color-bg-elevated)] flex-shrink-0">
                          Retired
                        </span>
                      )}
                    </div>
                    <p className="text-[10px] text-[var(--color-text-muted)] truncate">
                      {r.holderWorkspace ?? 'unknown workspace'} · created {fmtDate(r.createdAt)}
                      {r.retiredAt != null ? ` · retired ${fmtDate(r.retiredAt)}` : ''}
                    </p>
                  </div>
                  {r.status === 'active' && (
                    <button
                      type="button"
                      disabled={!canMutate || busyId === r.id}
                      onClick={() => void retire(r)}
                      className="px-2 py-0.5 text-[10px] text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error-soft)_10%,transparent)] transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0"
                    >
                      {busyId === r.id ? 'Retiring…' : 'Retire'}
                    </button>
                  )}
                </div>
              ))}
            </div>
          </div>
        ))
      )}
    </div>
  )
}

// ── Approvals panel ──────────────────────────────────────────────────────

function ApprovalCard({
  item,
  canMutate,
  onDecided,
}: {
  item: ApprovalItem
  canMutate: boolean
  onDecided: (id: string) => void
}): React.JSX.Element {
  const [expanded, setExpanded] = useState(false)
  const [denying, setDenying] = useState(false)
  const [note, setNote] = useState('')
  const [busy, setBusy] = useState<'approve' | 'deny' | null>(null)

  const approve = async (): Promise<void> => {
    setBusy('approve')
    try {
      await approveOutbound(item.id)
      onDecided(item.id)
    } catch (e) {
      // `engine` failures mean approved-but-not-submitted — surfaced
      // loudly; the row leaves the pending queue either way.
      useToastStore.getState().addToast(mailErrorMessage(e), 'error', 10000)
      onDecided(item.id)
    } finally {
      setBusy(null)
    }
  }

  const deny = async (): Promise<void> => {
    if (!note.trim()) return
    setBusy('deny')
    try {
      await denyOutbound(item.id, note.trim())
      onDecided(item.id)
    } catch (e) {
      useToastStore.getState().addToast(`Deny failed: ${mailErrorMessage(e)}`, 'error')
    } finally {
      setBusy(null)
    }
  }

  return (
    <div className="border border-[var(--color-border)] px-3 py-2 space-y-1.5">
      <div className="flex items-center gap-2 min-w-0">
        <span className="text-xs text-[var(--color-text-primary)] truncate flex-1" title={item.subject}>
          {item.subject || '(no subject)'}
        </span>
        <span className="text-[10px] text-[var(--color-text-muted)] flex-shrink-0">
          {fmtDate(item.createdAt)}
        </span>
      </div>
      <p className="text-[10px] text-[var(--color-text-muted)] break-words">
        <span className="font-mono">{item.from}</span>
        {' → '}
        <span className="font-mono">{item.to.join(', ')}</span>
        {item.cc.length > 0 && <span className="font-mono"> · cc {item.cc.join(', ')}</span>}
      </p>
      <p className="text-[10px] text-[var(--color-text-muted)]">
        {item.agentName ?? 'agent'}
        {item.workspace ? ` · ${item.workspace}` : ''} · expires {fmtDate(item.expiresAt)} (auto-deny)
      </p>

      {/* Body — NEVER auto-rendered; explicit expand, plain text only
          (untrusted external content, §8.1). */}
      <button
        type="button"
        onClick={() => setExpanded((v) => !v)}
        className="text-[10px] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer"
      >
        {expanded ? '▾ Hide message body' : '▸ Show message body'}
      </button>
      {expanded && (
        <div className="space-y-1">
          <p className="text-[9px] text-[var(--color-status-warn)] uppercase tracking-wide font-semibold">
            Agent-composed content — plain-text preview
          </p>
          <pre className="text-[10px] font-mono text-[var(--color-text-secondary)] whitespace-pre-wrap break-words bg-[var(--color-bg)] border border-[var(--color-border)] px-2 py-1.5 max-h-48 overflow-y-auto">
            {item.bodyPreview ?? '(body preview unavailable)'}
          </pre>
          {item.bodyPreview !== null && item.bodyPreview.length >= 280 && (
            <p className="text-[9px] text-[var(--color-text-muted)]">
              Preview truncated at 280 characters — the full message is what gets sent on
              approval.
            </p>
          )}
        </div>
      )}

      <div className="flex items-center gap-2 pt-0.5">
        <button
          type="button"
          disabled={!canMutate || busy !== null}
          onClick={() => void approve()}
          className="px-2.5 py-1 text-[10px] font-medium text-[var(--color-status-ok)] border border-[color-mix(in_srgb,var(--color-status-ok)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-ok)_10%,transparent)] transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {busy === 'approve' ? 'Approving…' : 'Approve & send'}
        </button>
        {!denying ? (
          <button
            type="button"
            disabled={!canMutate || busy !== null}
            onClick={() => setDenying(true)}
            className="px-2.5 py-1 text-[10px] font-medium text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error-soft)_10%,transparent)] transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
          >
            Deny…
          </button>
        ) : (
          <div className="flex items-center gap-1.5 flex-1 min-w-0">
            <input
              autoFocus
              type="text"
              value={note}
              disabled={busy !== null}
              onChange={(e) => setNote(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void deny()
                else if (e.key === 'Escape') {
                  e.stopPropagation()
                  setDenying(false)
                  setNote('')
                }
              }}
              placeholder="Reason (flows back to the agent — required)"
              className="flex-1 min-w-0 px-2 py-1 text-[10px] bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-status-error-soft)] no-drag"
            />
            <button
              type="button"
              disabled={!canMutate || busy !== null || note.trim().length === 0}
              onClick={() => void deny()}
              className="px-2 py-1 text-[10px] font-medium text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error-soft)_10%,transparent)] transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0"
            >
              {busy === 'deny' ? 'Denying…' : 'Deny'}
            </button>
          </div>
        )}
      </div>
    </div>
  )
}

/** Wire status → chip color for outbox history rows. */
function outboundChip(status: string): { label: string; color: string } {
  switch (status) {
    case 'pending_approval':
      return { label: 'Pending', color: 'var(--color-status-warn)' }
    case 'approved':
      return { label: 'Approved', color: 'var(--color-status-ok)' }
    case 'submitted':
      return { label: 'Submitted', color: 'var(--color-status-ok)' }
    case 'rejected':
      return { label: 'Denied', color: 'var(--color-status-error-soft)' }
    case 'failed':
      return { label: 'Failed', color: 'var(--color-status-error-soft)' }
    default:
      return { label: status, color: 'var(--color-text-muted)' }
  }
}

function ApprovalsPanel({
  pending,
  pendingError,
  sample,
  canMutate,
  revision,
  onDecided,
}: {
  pending: ApprovalItem[] | null
  pendingError: string | null
  sample: boolean
  canMutate: boolean
  revision: number
  onDecided: (id: string) => void
}): React.JSX.Element {
  // Decided history: per-workspace outbox (no owner-wide history route
  // yet — the picker selects which workspace's outbox to show).
  const projects = useProjectsStore((s) => s.projects)
  const [historyWs, setHistoryWs] = useState('')
  const [history, setHistory] = useState<OutboundItem[] | null>(null)
  const [historyError, setHistoryError] = useState<string | null>(null)

  useEffect(() => {
    if (sample) {
      setHistory([
        {
          id: 'out_11ab',
          from: 'research-bot@example.org',
          to: ['noreply@saas.example'],
          cc: [],
          subject: 'Re: Verify your account',
          status: 'submitted',
          statusNote: 'accepted-for-delivery (final delivery is the receiving server’s business)',
          note: null,
          agentName: 'research-bot',
          decidedBy: 'owner',
          createdAt: 1751700000,
          decidedAt: 1751700120,
          sentAt: 1751700121,
        },
        {
          id: 'out_09cd',
          from: 'signup-runner@example.org',
          to: ['all-staff@bigcorp.example'],
          cc: [],
          subject: 'Bulk outreach draft',
          status: 'rejected',
          statusNote: 'denied by your human — see the note',
          note: 'Too broad a recipient list — narrow it to the vendor contact.',
          agentName: 'signup-runner',
          decidedBy: 'owner',
          createdAt: 1751600000,
          decidedAt: 1751600500,
          sentAt: null,
        },
      ])
      return
    }
    if (!historyWs) {
      setHistory(null)
      setHistoryError(null)
      return
    }
    let cancelled = false
    fetchOutbox(historyWs)
      .then((r) => {
        if (cancelled) return
        setHistory(r)
        setHistoryError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setHistoryError(mailErrorMessage(e))
      })
    return () => {
      cancelled = true
    }
  }, [sample, historyWs, revision])

  return (
    <div className="grid gap-6 grid-cols-[minmax(0,44rem)]">
      <div className="min-w-0" data-settings-id="email.approvals">
        <h2 className="text-base font-medium text-[var(--color-text-primary)]">Approvals</h2>
        <p className="text-[11px] text-[var(--color-text-muted)] mt-1">
          Agent sends queued for your decision (workspaces in approval mode). Approving submits
          the message; denying returns your note to the agent. Pending items auto-deny after 7
          days.
        </p>
      </div>

      <div className="space-y-2">
        <SectionTitle>Pending</SectionTitle>
        {pendingError ? (
          <p className="text-[11px] text-[var(--color-status-error-soft)]">
            Failed to load the queue: {pendingError}
          </p>
        ) : pending === null ? (
          <p className="text-xs text-[var(--color-text-muted)]">Loading…</p>
        ) : pending.length === 0 ? (
          <p className="text-[11px] text-[var(--color-text-muted)] italic">
            Nothing waiting. Agent sends land here when a workspace&rsquo;s send mode is
            &ldquo;approval&rdquo; (the default is off — enable it per workspace with{' '}
            <span className="font-mono">k2 mail config --agent-send approval</span>).
          </p>
        ) : (
          <div className="space-y-2">
            {pending.map((item) => (
              <ApprovalCard key={item.id} item={item} canMutate={canMutate} onDecided={onDecided} />
            ))}
          </div>
        )}
      </div>

      <div className="space-y-2 pb-6">
        <div className="flex items-center gap-2">
          <SectionTitle>History</SectionTitle>
          <span className="flex-1" />
          {!sample && (
            <select
              value={historyWs}
              onChange={(e) => setHistoryWs(e.target.value)}
              className="px-2 py-1 text-[11px] bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)] no-drag cursor-pointer"
            >
              <option value="">Pick a workspace…</option>
              {projects.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          )}
        </div>
        {historyError ? (
          <p className="text-[11px] text-[var(--color-status-error-soft)]">{historyError}</p>
        ) : history === null ? (
          <p className="text-[10px] text-[var(--color-text-muted)] italic">
            Decided and sent messages live in each workspace&rsquo;s outbox — pick a workspace to
            view its history (agents see their own with{' '}
            <span className="font-mono">k2 mail outbox</span>).
          </p>
        ) : history.length === 0 ? (
          <p className="text-[11px] text-[var(--color-text-muted)] italic">
            No outbound messages in this workspace.
          </p>
        ) : (
          <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
            {history.map((o) => {
              const chip = outboundChip(o.status)
              return (
                <div key={o.id} className="px-3 py-2 space-y-0.5 min-w-0">
                  <div className="flex items-center gap-2 min-w-0">
                    <span
                      className="text-[9px] font-semibold px-1.5 py-0.5 uppercase tracking-wide flex-shrink-0"
                      style={{
                        color: chip.color,
                        backgroundColor: `color-mix(in srgb, ${chip.color} 14%, transparent)`,
                      }}
                    >
                      {chip.label}
                    </span>
                    <span className="text-xs text-[var(--color-text-primary)] truncate flex-1">
                      {o.subject || '(no subject)'}
                    </span>
                    <span className="text-[10px] text-[var(--color-text-muted)] flex-shrink-0">
                      {fmtDate(o.decidedAt ?? o.createdAt)}
                    </span>
                  </div>
                  <p className="text-[10px] text-[var(--color-text-muted)] break-words">
                    <span className="font-mono">{o.from}</span>
                    {' → '}
                    <span className="font-mono">{o.to.join(', ')}</span>
                  </p>
                  {o.note && (
                    <p className="text-[10px] text-[var(--color-text-secondary)]">
                      Note: {o.note}
                    </p>
                  )}
                </div>
              )
            })}
          </div>
        )}
      </div>
    </div>
  )
}

// ── The section root (master-detail shell) ───────────────────────────────

export function EmailSection(): React.JSX.Element {
  const viewerReadOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')

  const [status, setStatus] = useState<MailStatus | null>(null)
  const [statusError, setStatusError] = useState<string | null>(null)
  const [domains, setDomains] = useState<DomainSummary[] | null>(null)
  const [approvals, setApprovals] = useState<ApprovalItem[] | null>(null)
  const [approvalsError, setApprovalsError] = useState<string | null>(null)
  const [selection, setSelection] = useState<Selection>({ kind: 'server' })
  // Bumped by mail:* events + local mutations → every loader refetches.
  const [revision, setRevision] = useState(0)
  const bump = useCallback(() => setRevision((r) => r + 1), [])

  // Add-domain inline form.
  const [addingDomain, setAddingDomain] = useState(false)
  const [newDomain, setNewDomain] = useState('')
  const [addError, setAddError] = useState<string | null>(null)
  const [addBusy, setAddBusy] = useState(false)

  const sample = status !== null && !status.supported
  const canMutate = status !== null && status.supported && !viewerReadOnly
  // Stable primitives for effect deps — `status` is a fresh object on
  // every poll tick, and keying loaders off the object would refetch
  // the world every 2 s during an install.
  const supported: boolean | null = status === null ? null : status.supported
  const installing = status !== null && status.supported && status.state === 'installing'

  // ── Capability read + status (the ONE call made before we know the
  //    supported flag; pre-mortem #15). ──────────────────────────────
  useEffect(() => {
    let cancelled = false
    fetchMailStatus()
      .then((s) => {
        if (cancelled) return
        setStatus(s)
        setStatusError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setStatusError(mailErrorMessage(e))
      })
    return () => {
      cancelled = true
    }
  }, [revision])

  // ── Left-column data: domains + the approvals queue. Sample mode
  //    reads fixtures and stays network-silent. ─────────────────────
  useEffect(() => {
    if (supported === null) return
    if (!supported) {
      setDomains(SAMPLE_DOMAINS)
      setApprovals(SAMPLE_APPROVALS)
      setApprovalsError(null)
      return
    }
    let cancelled = false
    fetchDomains()
      .then((d) => {
        if (!cancelled) setDomains(d)
      })
      .catch(() => {
        if (!cancelled) setDomains([])
      })
    fetchApprovals()
      .then((a) => {
        if (cancelled) return
        setApprovals(a)
        setApprovalsError(null)
      })
      .catch((e) => {
        if (cancelled) return
        // Non-admin remote principals get the owner-or-admin gate here;
        // show the queue as unavailable rather than erroring the page.
        setApprovals(null)
        setApprovalsError(mailErrorMessage(e))
      })
    return () => {
      cancelled = true
    }
  }, [supported, revision])

  // ── Live updates: the daemon's mail:* hook events (Tauri-forwarded
  //    from the legacy /events WS). Refetch signals only. ────────────
  useEffect(() => {
    let disposed = false
    const unlisteners: UnlistenFn[] = []
    void (async () => {
      for (const ev of [
        'mail:server-state-changed',
        'mail:domain-status-changed',
        'mail:send-approval-requested',
        'mail:send-decided',
      ]) {
        try {
          const un = await listen(ev, () => setRevision((r) => r + 1))
          if (disposed) un()
          else unlisteners.push(un)
        } catch {
          // Non-Tauri context (tests) — the poll/manual paths cover it.
        }
      }
    })()
    return () => {
      disposed = true
      for (const un of unlisteners) un()
    }
  }, [])

  // ── Enable-progress poll: only while an install is in flight (the
  //    route's documented contract — poll GET /cli/mail/status). ─────
  useEffect(() => {
    if (!installing) return
    const t = window.setInterval(() => {
      fetchMailStatus()
        .then((s) => setStatus(s))
        .catch(() => {
          /* transient — next tick retries */
        })
    }, 2000)
    return () => window.clearInterval(t)
  }, [installing])

  // Selection healing: a removed domain falls back to the server panel.
  useEffect(() => {
    if (selection.kind !== 'domain' || domains === null) return
    if (!domains.some((d) => d.domain === selection.domain)) {
      setSelection({ kind: 'server' })
    }
  }, [domains, selection])

  const submitAddDomain = useCallback(async (): Promise<void> => {
    const dom = newDomain.trim().toLowerCase()
    if (!dom || addBusy) return
    setAddBusy(true)
    setAddError(null)
    try {
      const detail = await addDomain(dom)
      setAddingDomain(false)
      setNewDomain('')
      setSelection({ kind: 'domain', domain: detail.domain })
      bump()
    } catch (e) {
      const { code, hint } = mailErrorInfo(e)
      setAddError(hint ?? mailErrorMessage(e))
      if (code === 'exists') setSelection({ kind: 'domain', domain: dom })
    } finally {
      setAddBusy(false)
    }
  }, [newDomain, addBusy, bump])

  const onDecided = useCallback(
    (id: string): void => {
      // Optimistic removal from the pending queue; the event/refetch
      // converges the rest.
      setApprovals((prev) => prev?.filter((a) => a.id !== id) ?? null)
      bump()
    },
    [bump],
  )

  const pendingCount = approvals?.length ?? 0

  if (statusError) {
    return (
      <div className="p-6 space-y-2">
        <p className="text-[11px] text-[var(--color-status-error-soft)]">
          Couldn&rsquo;t read the mail server status: {statusError}
        </p>
        <button
          type="button"
          onClick={bump}
          className="px-2.5 py-1 text-xs text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer"
        >
          Retry
        </button>
      </div>
    )
  }
  if (status === null) {
    return <p className="p-6 text-xs text-[var(--color-text-muted)]">Loading…</p>
  }

  return (
    <div className="flex flex-col h-full min-h-0">
      {/* ── D3 banner — pinned on top of the whole page when the DAEMON
          reports unsupported (never navigator.platform). ── */}
      {sample && (
        <div className="flex-shrink-0 px-4 py-2 text-[10px] font-bold tracking-wide text-center text-[var(--color-status-warn-text)] bg-[color-mix(in_srgb,var(--color-status-warn)_18%,transparent)] border-b border-[color-mix(in_srgb,var(--color-status-warn)_40%,transparent)]">
          {MAC_BANNER}
        </div>
      )}

      <div className="flex flex-1 min-h-0">
        {/* ── Left column ── */}
        <div className="w-60 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col min-h-0">
          {/* Server status card */}
          <button
            type="button"
            onClick={() => setSelection({ kind: 'server' })}
            className={`text-left px-3 py-2.5 border-b border-[var(--color-border)] transition-colors no-drag cursor-pointer ${
              selection.kind === 'server'
                ? 'bg-[var(--color-accent)]/10'
                : 'hover:bg-[var(--color-bg-hover)]'
            }`}
          >
            <div className="flex items-center gap-2">
              <span
                className="w-2 h-2 rounded-full flex-shrink-0"
                style={{ backgroundColor: stateColor(status.state) }}
              />
              <span className="text-xs font-medium text-[var(--color-text-primary)]">
                Email server
              </span>
              <span className="flex-1" />
              <span className="text-[9px] text-[var(--color-text-muted)]">
                {status.version ? `v${status.version}` : ''}
              </span>
            </div>
            <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5 truncate">
              {status.state}
              {status.hostname ? ` · ${status.hostname}` : ''}
              {status.portPlan ? ` · ${status.portPlan}` : ''}
            </p>
          </button>

          {/* Approvals + Addresses nav */}
          <button
            type="button"
            onClick={() => setSelection({ kind: 'approvals' })}
            className={`flex items-center gap-2 text-left px-3 py-2 text-xs transition-colors no-drag cursor-pointer ${
              selection.kind === 'approvals'
                ? 'bg-[var(--color-accent)]/10 text-[var(--color-text-primary)]'
                : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]'
            }`}
          >
            Approvals
            {pendingCount > 0 && (
              <span className="text-[9px] font-bold px-1.5 py-0.5 text-[var(--color-status-warn-text)] bg-[color-mix(in_srgb,var(--color-status-warn)_20%,transparent)] flex-shrink-0">
                {pendingCount}
              </span>
            )}
          </button>
          <button
            type="button"
            onClick={() => setSelection({ kind: 'addresses' })}
            className={`text-left px-3 py-2 text-xs transition-colors no-drag cursor-pointer ${
              selection.kind === 'addresses'
                ? 'bg-[var(--color-accent)]/10 text-[var(--color-text-primary)]'
                : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]'
            }`}
          >
            Addresses
          </button>

          {/* Domain list */}
          <div className="px-3 pt-3 pb-1 border-t border-[var(--color-border)] mt-1">
            <SectionTitle>Domains</SectionTitle>
          </div>
          <div className="flex-1 overflow-y-auto px-1 py-1">
            {domains === null ? (
              <p className="px-2 py-1 text-[11px] text-[var(--color-text-muted)]">Loading…</p>
            ) : domains.length === 0 ? (
              <p className="px-2 py-1 text-[11px] text-[var(--color-text-muted)] italic">
                No domains yet.
              </p>
            ) : (
              domains.map((d) => {
                const isSelected = selection.kind === 'domain' && selection.domain === d.domain
                const broken = d.records.wrong + d.records.missing
                return (
                  <button
                    key={d.domain}
                    type="button"
                    onClick={() => setSelection({ kind: 'domain', domain: d.domain })}
                    className={`w-full flex items-center gap-2 px-2 py-1.5 text-left transition-colors no-drag cursor-pointer min-w-0 ${
                      isSelected
                        ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                        : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)]'
                    }`}
                  >
                    <span className="text-xs font-mono truncate flex-1">{d.domain}</span>
                    {broken > 0 && d.status !== 'verified' && (
                      <span
                        className="text-[9px] text-[var(--color-status-error-soft)] flex-shrink-0 tabular-nums"
                        title={`${broken} record(s) missing or wrong`}
                      >
                        {broken}✖
                      </span>
                    )}
                    <StatusChip status={d.status} />
                  </button>
                )
              })
            )}
          </div>

          {/* Add domain */}
          <div className="px-2 py-2 border-t border-[var(--color-border)] flex-shrink-0">
            {addingDomain ? (
              <div className="space-y-1">
                <div className="flex items-center gap-1">
                  <input
                    autoFocus
                    type="text"
                    value={newDomain}
                    disabled={addBusy}
                    onChange={(e) => {
                      setNewDomain(e.target.value)
                      setAddError(null)
                    }}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') void submitAddDomain()
                      else if (e.key === 'Escape') {
                        e.stopPropagation()
                        setAddingDomain(false)
                        setNewDomain('')
                        setAddError(null)
                      }
                    }}
                    placeholder="acme.dev"
                    className="flex-1 min-w-0 px-2 py-1 text-[11px] font-mono bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)] no-drag disabled:opacity-50"
                  />
                  <button
                    type="button"
                    disabled={addBusy || newDomain.trim().length === 0}
                    onClick={() => void submitAddDomain()}
                    className="px-2 py-1 text-[10px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer disabled:opacity-50 flex-shrink-0"
                  >
                    {addBusy ? '…' : 'Add'}
                  </button>
                </div>
                {addError && (
                  <p className="text-[10px] text-[var(--color-status-error-soft)] break-words">{addError}</p>
                )}
              </div>
            ) : (
              <button
                type="button"
                disabled={!canMutate}
                onClick={() => setAddingDomain(true)}
                title={
                  canMutate
                    ? 'Add a domain — K2 shows the DNS records to set at your registrar'
                    : 'Not available on this deployment'
                }
                className="w-full flex items-center justify-center gap-1.5 px-2 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-hover)] hover:bg-[var(--color-bg-elevated)] transition-colors no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
              >
                <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                  <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
                </svg>
                Add domain
              </button>
            )}
          </div>
        </div>

        {/* ── Right panel ── */}
        <div className="flex-1 overflow-y-auto p-6 min-h-0">
          {selection.kind === 'server' && (
            <ServerPanel status={status} sample={sample} canMutate={canMutate} onChanged={bump} />
          )}
          {selection.kind === 'domain' && (
            <DomainPanel
              domain={selection.domain}
              sample={sample}
              canMutate={canMutate}
              revision={revision}
              onChanged={bump}
              onRemoved={() => {
                setSelection({ kind: 'server' })
                bump()
              }}
            />
          )}
          {selection.kind === 'addresses' && (
            <AddressesPanel sample={sample} canMutate={canMutate} revision={revision} onChanged={bump} />
          )}
          {selection.kind === 'approvals' && (
            <ApprovalsPanel
              pending={approvals}
              pendingError={approvalsError}
              sample={sample}
              canMutate={canMutate}
              revision={revision}
              onDecided={onDecided}
            />
          )}
        </div>
      </div>
    </div>
  )
}
