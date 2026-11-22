// Settings → Email Hosting (K2 Mail S7, prd-email-server-v1 §13) — the
// owner surface: the mail server, domains and their DNS records, agent
// addresses, and the outbound-approvals queue. The reference layout: a
// page header, a tab strip (Server / Domains / Addresses / Approvals) and
// card groups of rows; a domain opens its own detail inside Domains, with
// a way back to the list.
//
// CAPABILITY GATING (pre-mortem #15 — BINDING): the whole page keys off
// GET /cli/mail/status `supported` — the DAEMON's report, never
// navigator.platform (a Mac app driving a remote Linux daemon must see
// the real page). When unsupported the page renders fully from the
// static SAMPLE fixture (email-api.ts) under the shared Linux note, with
// every mutating control disabled and NO further network calls.
//
// LIVE UPDATES: the daemon mirrors its mail:* hook events onto the
// host-aware /cli/sessions/events bus as `mail_changed`; we subscribe on
// the window's server's app bus (`onWindowServerMailChanged`) and bump a
// revision that refetches. While an enable run is in flight we also poll
// /cli/mail/status (the route's documented enableProgress contract).
//
// UNTRUSTED CONTENT (§8.1/§8.4): approval items surface from/to/subject
// summaries only; the body is a bounded plain-text preview the daemon
// prepared, rendered ONLY behind an explicit expand, always as plain text
// (a pre-wrapped text box), never HTML, never auto-shown.

import React, { useCallback, useEffect, useId, useMemo, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useWindowModeStore } from '@/stores/window-mode'
import type { SettingEntry } from '../searchManifest'
import { InboxAccessPanel } from './InboxAccessPanel'
import { MailboxForwardingPanel } from './MailboxForwardingPanel'
import {
  ENABLE_STEPS,
  SAMPLE_ADDRESSES,
  SAMPLE_APPROVALS,
  SAMPLE_DOMAINS,
  SAMPLE_DOMAIN_DETAILS,
  SAMPLE_INBOXES,
  SAMPLE_PREFLIGHT,
  addDomain,
  approveOutbound,
  checkDomainNow,
  denyOutbound,
  disableServer,
  enableServer,
  fetchAllAddresses,
  type ApprovalFile,
  fetchApprovalFile,
  fetchApprovals,
  fetchApprovalText,
  fetchDomainDetail,
  fetchDomains,
  fetchInboxes,
  fetchMailStatus,
  onWindowServerMailChanged,
  fetchOutbox,
  fetchPreflight,
  isNotBuilt,
  mailErrorInfo,
  mailErrorMessage,
  removeDomain,
  retireAddress,
  rotateAddressPassword,
  setMailConfig,
  uninstallServer,
  type AddressRow,
  type ApprovalItem,
  type DnsRecordRow,
  type DomainDetail,
  type DomainSummary,
  type Inbox,
  type MailStatus,
  type OutboundItem,
  type PreflightReport,
  type RotatedMailboxPassword,
} from './email-api'
import {
  SETTING_LIST_ROW_HOVER,
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
  SETTING_SERVER_REASON,
  SaveError,
  SaveNote,
  SettingDropdown,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
  useSaveStatus,
} from '../controls/SettingControls'
import { Button, Callout, Checkbox, Input, Surface, Tag } from '@/components/ui'
import { AccessSheet, CopyLine, DIALOG_FIELD, DIALOG_MUTED, FailedNote, Field } from './AccessDialogs'
import { StatusDot, type HealthTone } from './ConnectionsParts'
import { LinuxOnlyNote } from './LinuxOnlyNote'
import { PRODUCT_NAME } from '@/lib/brand'

export const EMAIL_HOSTING_MANIFEST: SettingEntry[] = [
  { id: 'email-hosting.server', section: 'email-hosting', label: 'Email Server', description: 'Install and run your own mail server (Linux servers)', keywords: ['mail', 'email', 'smtp', 'stalwart', 'server', 'enable', 'preflight', 'hosting', 'install', 'uninstall'] },
  { id: 'email-hosting.domains', section: 'email-hosting', label: 'Email Domains', description: 'Add domains and check their DNS records', keywords: ['domain', 'dns', 'mx', 'spf', 'dkim', 'dmarc', 'ptr', 'records', 'verify', 'zone'] },
  { id: 'email-hosting.addresses', section: 'email-hosting', label: 'Email Addresses', description: 'Agent addresses on your domains: forwarding, passwords, retiring', keywords: ['address', 'mailbox', 'agent', 'cap', 'retire', 'mint', 'rotate', 'imap', 'smtp', 'password', 'forward', 'forwarding', 'gmail', 'redirect'] },
  { id: 'email-hosting.approvals', section: 'email-hosting', label: 'Send Approvals', description: 'Approve or deny mail your agents want to send', keywords: ['approval', 'approvals', 'outbox', 'send', 'deny', 'approve', 'queue', 'outbound'] },
  { id: 'email-hosting.send-mode', section: 'email-hosting', label: 'Send Mode & Relay', description: 'How mail leaves each domain: from this server or through a relay', keywords: ['relay', 'smtp relay', 'direct', 'send mode', 'smart host', 'receive-only'] },
]

type Tab = 'server' | 'domains' | 'addresses' | 'approvals'
const TABS: Tab[] = ['server', 'domains', 'addresses', 'approvals']
const TAB_LABEL: Record<Tab, string> = { server: 'Server', domains: 'Domains', addresses: 'Addresses', approvals: 'Approvals' }
const TAB_SETTINGS_ID: Record<Tab, string> = {
  server: 'email-hosting.server',
  domains: 'email-hosting.domains',
  addresses: 'email-hosting.addresses',
  approvals: 'email-hosting.approvals',
}

/** A page-level result under the header: what just happened, or what
 *  failed and why. Dismissable. */
type Notice = { tone: 'ok' | 'error'; text: string; detail?: string }

// ── Small shared bits ────────────────────────────────────────────────────

function fmtFileSize(n: number): string {
  if (n < 1024) return `${n} bytes`
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`
  return `${(n / (1024 * 1024)).toFixed(1)} MB`
}

function fmtDay(unixSecs: number | null | undefined): string {
  if (typeof unixSecs !== 'number' || unixSecs <= 0) return ''
  return new Date(unixSecs * 1000).toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' })
}

function fmtWhen(unixSecs: number | null | undefined): string {
  if (typeof unixSecs !== 'number' || unixSecs <= 0) return ''
  return new Date(unixSecs * 1000).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })
}

function reasonOf(e: unknown): string {
  return mailErrorInfo(e).hint ?? mailErrorMessage(e)
}

/** The daemon always sends `supported` and `state`; anything else is a
 *  reply this page can't read, said plainly instead of guessed at. */
function isMailStatus(s: unknown): s is MailStatus {
  const o = s as Partial<MailStatus> | null
  return !!o && typeof o === 'object' && typeof o.supported === 'boolean' && typeof o.state === 'string'
}

const STATE_WORD: Record<string, { word: string; tone: HealthTone; hint: string }> = {
  running: { word: 'Running', tone: 'ok', hint: 'Receiving and sending mail for your domains.' },
  degraded: { word: 'Running with problems', tone: 'warn', hint: 'Mail still flows, but something needs a look.' },
  installing: { word: 'Installing', tone: 'warn', hint: `${PRODUCT_NAME} is setting the server up. This page updates as each step finishes.` },
  error: { word: 'Install stopped', tone: 'error', hint: 'The install stopped part-way. Resume it to carry on from the last step that finished.' },
  stopped: { word: 'Stopped', tone: 'neutral', hint: 'The server isn’t running. Your mail is kept.' },
  disabled: { word: 'Turned off', tone: 'neutral', hint: 'The server is off and your mail is kept. Incoming mail bounces until you turn it back on.' },
  'not-installed': {
    word: 'Not installed',
    tone: 'neutral',
    hint: 'That’s fine: agents can still read and draft in any account you connect in Email Link. Install the server to give them addresses on your own domains that can send.',
  },
}

function stateInfo(state: string): { word: string; tone: HealthTone; hint: string } {
  return STATE_WORD[state] ?? { word: state ? state[0].toUpperCase() + state.slice(1) : 'Unknown', tone: 'neutral', hint: '' }
}

/** A DNS record's (or a domain's) check result, in words. */
function recordTag(status: string): { label: string; tone: HealthTone } {
  switch (status) {
    case 'valid':
      return { label: 'Found', tone: 'ok' }
    case 'verified':
      return { label: 'Verified', tone: 'ok' }
    case 'missing':
      return { label: 'Missing', tone: 'error' }
    case 'wrong':
      return { label: 'Wrong value', tone: 'error' }
    case 'unknown':
      return { label: 'Couldn’t check', tone: 'warn' }
    case 'unverifiable':
      return { label: 'Check by hand', tone: 'neutral' }
    case 'optional':
      return { label: 'Optional', tone: 'neutral' }
    case 'skip':
      // 0.46.1: an external zone can't give these names a certificate yet.
      return { label: 'Skip for now', tone: 'neutral' }
    case 'pending':
      return { label: 'Waiting for DNS', tone: 'warn' }
    default:
      return { label: 'Not checked yet', tone: 'neutral' }
  }
}

function domainTag(status: string): { label: string; tone: HealthTone } {
  if (status === 'verified') return { label: 'Verified', tone: 'ok' }
  if (status === 'pending') return { label: 'Waiting for DNS', tone: 'warn' }
  return recordTag(status)
}

const SEND_MODE_WORD: Record<string, string> = {
  'receive-only': 'Receive only',
  direct: 'Sends from this server',
  relay: 'Sends through a relay',
}

/** A button that copies `text`, then says Copied (or that it couldn't). */
function CopyButton({ text, label }: { text: string; label: string }): React.JSX.Element {
  const [state, setState] = useState<'idle' | 'done' | 'failed'>('idle')
  useEffect(() => {
    if (state === 'idle') return
    const t = window.setTimeout(() => setState('idle'), 1600)
    return () => window.clearTimeout(t)
  }, [state])
  return (
    <Button
      aria-label={`Copy ${label}`}
      onClick={() => {
        const clip = typeof navigator !== 'undefined' ? navigator.clipboard : undefined
        if (!clip) return setState('failed')
        clip.writeText(text).then(
          () => setState('done'),
          () => setState('failed'),
        )
      }}
    >
      {state === 'done' ? 'Copied' : state === 'failed' ? 'Couldn’t copy' : 'Copy'}
    </Button>
  )
}

/** A problem word's ink: the status text colour pulled 30% toward the
 *  primary text (the Tag's recipe), so it keeps 4.5:1 on a card and on the
 *  list-row hover band in all four looks (Paper and Veil amber was 3.8:1). */
const WORD_INK: Record<HealthTone, string> = {
  ok: 'text-[var(--color-text-primary)]',
  neutral: 'text-[var(--color-text-secondary)]',
  warn: 'text-[color-mix(in_srgb,var(--color-status-warn-text)_70%,var(--color-text-primary))]',
  error: 'text-[color-mix(in_srgb,var(--color-status-error-soft)_70%,var(--color-text-primary))]',
}

/** A status as a dot and a word: the words carry it, colour only on a problem. */
function StatusWord({ tone, children }: { tone: HealthTone; children: React.ReactNode }): React.JSX.Element {
  return (
    <span className="inline-flex items-center gap-1.5 type-hint whitespace-nowrap" data-status-word={tone}>
      <StatusDot tone={tone} />
      <span className={`font-medium ${WORD_INK[tone]}`}>{children}</span>
    </span>
  )
}

function NoticeBar({ notice, onDismiss }: { notice: Notice; onDismiss: () => void }): React.JSX.Element {
  return (
    <Callout
      tone={notice.tone}
      role={notice.tone === 'error' ? 'alert' : 'status'}
      className="type-hint flex items-center justify-between gap-4"
      data-testid="hosting-notice"
    >
      <div className="min-w-0">
        <p className="font-medium">{notice.text}</p>
        {notice.detail ? <p className="mt-0.5">The server said: {notice.detail.trim().replace(/[.\s]+$/, '')}.</p> : null}
      </div>
      <Button variant="quiet" className="flex-shrink-0" onClick={onDismiss}>
        Dismiss
      </Button>
    </Callout>
  )
}

/** One read-only fact in a card: label left, value right. */
function FactRow({ label, hint, children, testId }: { label: string; hint?: React.ReactNode; children: React.ReactNode; testId?: string }): React.JSX.Element {
  return (
    <SettingRow label={label} hint={hint} testId={testId}>
      {children}
    </SettingRow>
  )
}

// ── Server tab ───────────────────────────────────────────────────────────

const CHECK_MARK: Record<string, { tone: HealthTone; word: string }> = {
  pass: { tone: 'ok', word: 'Passed' },
  fail: { tone: 'error', word: 'Must fix' },
  warn: { tone: 'warn', word: 'Warning' },
  skipped: { tone: 'neutral', word: 'Skipped' },
}

function PreflightRows({ report }: { report: PreflightReport }): React.JSX.Element {
  return (
    <>
      {report.checks.map((c) => {
        const m = CHECK_MARK[c.status] ?? { tone: 'neutral' as HealthTone, word: 'Not run' }
        return (
          <div key={c.id} className="px-4 py-3 flex items-start justify-between gap-6" data-testid="hosting-check" data-check={c.status}>
            <div className="min-w-0 flex items-start gap-2.5">
              <span className="mt-1.5 flex">
                <StatusDot tone={m.tone} />
              </span>
              <div className="min-w-0">
                <p className={SETTING_ROW_LABEL}>{c.label}</p>
                {c.detail ? <p className={SETTING_ROW_HINT}>{c.detail}</p> : null}
              </div>
            </div>
            <span
              className={`type-hint whitespace-nowrap ${
                m.tone === 'error' ? 'text-[var(--color-status-error-soft)]' : m.tone === 'warn' ? 'text-[var(--color-status-warn-text)]' : 'text-[var(--color-text-secondary)]'
              }`}
            >
              {m.word}
            </span>
          </div>
        )
      })}
    </>
  )
}

/** The install steps in plain words (ids from `supervisor::ENABLE_STEPS`). */
const STEP_WORDS: Record<string, string> = {
  preflight: 'Check this server',
  download: 'Download the mail server',
  verify: 'Check the download is genuine',
  extract: 'Install the mail server',
  'system-user': 'Give it its own account on this server',
  dirs: 'Make folders for mail',
  config: 'Clear old settings',
  unit: 'Set it to start with the server',
  start: 'First start',
  bootstrap: 'Finish its first-time setup',
  'restart-normal': 'Restart it normally',
  'server-config': 'Turn on the mail ports',
  'service-account': `Create an account for ${PRODUCT_NAME}`,
  'api-key': `Create a key for ${PRODUCT_NAME}`,
  'recovery-off': 'Remove the setup password',
  restart: 'Restart with the final settings',
}

function InstallSteps({ status }: { status: MailStatus }): React.JSX.Element {
  const steps = status.enableProgress?.steps ?? {}
  const current = status.enableProgress?.current
  const [showDone, setShowDone] = useState(false)
  const [showLater, setShowLater] = useState(false)
  const done = ENABLE_STEPS.filter((s) => s.id in steps)
  const rest = ENABLE_STEPS.filter((s) => !(s.id in steps))
  // The step working now (or stopped at), then the rest folded away.
  const nowIdx = Math.max(0, rest.findIndex((s) => s.id === current))
  const now = rest.slice(0, nowIdx + 1)
  const later = rest.slice(nowIdx + 1)
  const row = (s: { id: string; label: string }, isDone: boolean): React.JSX.Element => {
    const active = current === s.id
    return (
      <div key={s.id} className="px-4 py-2.5 flex items-center justify-between gap-6" data-testid="hosting-step">
        <div className="flex items-center gap-2.5 min-w-0">
          <StatusDot tone={isDone ? 'ok' : active ? 'warn' : 'neutral'} />
          <span className={isDone || active ? SETTING_ROW_LABEL : 'type-row text-[var(--color-text-secondary)]'}>{STEP_WORDS[s.id] ?? s.label}</span>
        </div>
        <span className="type-hint text-[var(--color-text-secondary)] whitespace-nowrap">
          {isDone ? 'Done' : active ? (status.state === 'installing' ? 'Working…' : 'Stopped here') : 'Waiting'}
        </span>
      </div>
    )
  }
  return (
    <>
      {done.length > 0 ? (
        <button
          type="button"
          data-k2-focus=""
          aria-expanded={showDone}
          onClick={() => setShowDone((v) => !v)}
          className={`relative w-full text-left px-4 py-2.5 flex items-center justify-between gap-6 cursor-pointer outline-none ${SETTING_LIST_ROW_HOVER}`}
          data-testid="hosting-steps-done"
        >
          <span className="flex items-center gap-2.5 min-w-0">
            <StatusDot tone="ok" />
            <span className={SETTING_ROW_LABEL}>
              {done.length} of {ENABLE_STEPS.length} steps done
            </span>
          </span>
          <span className="type-hint text-[var(--color-text-secondary)] whitespace-nowrap">{showDone ? 'Hide' : 'Show'}</span>
        </button>
      ) : null}
      {showDone ? done.map((s) => row(s, true)) : null}
      {now.map((s) => row(s, false))}
      {later.length > 0 ? (
        <button
          type="button"
          data-k2-focus=""
          aria-expanded={showLater}
          onClick={() => setShowLater((v) => !v)}
          className={`relative w-full text-left px-4 py-2.5 flex items-center justify-between gap-6 cursor-pointer outline-none ${SETTING_LIST_ROW_HOVER}`}
          data-testid="hosting-steps-later"
        >
          <span className="flex items-center gap-2.5 min-w-0">
            <StatusDot tone="neutral" />
            <span className="type-row text-[var(--color-text-secondary)]">
              {later.length} more step{later.length === 1 ? '' : 's'} after this
            </span>
          </span>
          <span className="type-hint text-[var(--color-text-secondary)] whitespace-nowrap">{showLater ? 'Hide' : 'Show'}</span>
        </button>
      ) : null}
      {showLater ? later.map((s) => row(s, false)) : null}
    </>
  )
}

/** How mail apps reach the server, in words; the raw plan stays quiet. */
const PORT_PLAN_WORDS: Record<string, string> = {
  'tls-alpn': 'Secure connection on port 443',
  'http-01': 'Secure connection, certificate checked over port 80',
  'dns-01': 'Secure connection, certificate checked through your DNS',
}

/** A failure in plain words, from what the server said. The server's own
 *  reason still shows, quietly, under it. */
function plainFailure(fallback: string, detail: string): string {
  const d = detail.toLowerCase()
  if (/timed? ?out|timeout/.test(d) && /stop|systemctl/.test(d)) return 'Couldn’t stop the email server in time.'
  if (/no soa|does not exist|nxdomain|no such domain/.test(d)) return 'That domain doesn’t exist; check the spelling.'
  if (/dns|resolver|lookup/.test(d) && /timed? ?out|timeout|unreachable/.test(d)) return 'Couldn’t reach DNS to check the records.'
  if (/already exists|exists/.test(d)) return 'That domain is already on this server.'
  if (/locked|busy/.test(d)) return 'The server was busy.'
  return fallback
}

function ServerTab({
  status,
  sample,
  canMutate,
  onChanged,
  setNotice,
}: {
  status: MailStatus
  sample: boolean
  canMutate: boolean
  onChanged: () => void
  setNotice: (n: Notice | null) => void
}): React.JSX.Element {
  const [preflight, setPreflight] = useState<PreflightReport | null>(sample ? SAMPLE_PREFLIGHT : null)
  const [preflightError, setPreflightError] = useState<string | null>(null)
  const [preflightBusy, setPreflightBusy] = useState(false)
  const [hostname, setHostname] = useState('')
  const [busy, setBusy] = useState(false)
  const [enableError, setEnableError] = useState<string | null>(null)
  const [dialog, setDialog] = useState<'disable' | 'uninstall' | null>(null)
  const hostId = useId()

  const notInstalled = status.state === 'not-installed'
  const installing = status.state === 'installing'
  const hasProgress = status.enableProgress !== null && Object.keys(status.enableProgress?.steps ?? {}).length > 0
  const info = stateInfo(status.state)

  // The checks show before Install (§5.1) — read them for the
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
        if (!cancelled) setPreflightError(reasonOf(e))
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
      setPreflightError(reasonOf(e))
    } finally {
      setPreflightBusy(false)
    }
  }, [])

  const doEnable = useCallback(async (): Promise<void> => {
    const host = hostname.trim() || status.hostname || ''
    if (!host) {
      setEnableError('Enter the mail server’s name first, like mail.example.com.')
      return
    }
    setBusy(true)
    setEnableError(null)
    try {
      const res = await enableServer(host)
      if (res.ok === false) {
        // Failing checks return 200 + the report (§5.1 hard stops).
        setEnableError(res.error?.hint ?? 'Some checks must pass first. Fix the ones marked Must fix, then try again.')
        if (res.report) setPreflight(res.report)
      }
      onChanged()
    } catch (e) {
      setEnableError(reasonOf(e))
      onChanged()
    } finally {
      setBusy(false)
    }
  }, [hostname, status.hostname, onChanged])

  const versionText = status.version ? `Version ${status.version}` : null
  const mustFix = preflight ? preflight.checks.filter((c) => c.status === 'fail').length : 0

  return (
    <div className="space-y-6">
      <SettingsGroup title="Server" card testId="hosting-server-card">
        <FactRow
          label="Status"
          testId="hosting-status"
          hint={
            <>
              {info.hint}
              {status.lastError ? (
                <span className="block text-[var(--color-status-error-soft)] mt-1">
                  The server reported a problem: {status.lastError.trim().replace(/[.\s]+$/, '')}.
                </span>
              ) : null}
            </>
          }
        >
          <span className="inline-flex items-center gap-2 type-row text-[var(--color-text-primary)] whitespace-nowrap">
            <StatusDot tone={status.lastError && info.tone === 'ok' ? 'warn' : info.tone} />
            {info.word}
          </span>
        </FactRow>
        {versionText ? (
          <FactRow label="Version" hint={status.version !== status.pinnedVersion && status.pinnedVersion ? `${PRODUCT_NAME} expects version ${status.pinnedVersion}.` : undefined}>
            <span className="type-row text-[var(--color-text-secondary)] whitespace-nowrap">{versionText}</span>
          </FactRow>
        ) : null}
        {status.hostname ? (
          <FactRow label="Mail server name" hint="Mail apps and other mail servers reach this server by this name.">
            <span className="type-row text-[var(--color-text-secondary)] truncate max-w-[16rem]">{status.hostname}</span>
            <CopyButton text={status.hostname} label="mail server name" />
          </FactRow>
        ) : null}
        {status.portPlan ? (
          <FactRow
            label="How mail apps connect"
            hint={
              <>
                Chosen when the server installed.{' '}
                <span className="text-[var(--color-text-muted)]">{status.portPlan}</span>
              </>
            }
          >
            <span className="type-row text-[var(--color-text-secondary)] whitespace-nowrap">{PORT_PLAN_WORDS[status.portPlan] ?? 'Secure connection'}</span>
          </FactRow>
        ) : null}
      </SettingsGroup>

      {notInstalled ? (
        <SettingsGroup
          title="Install the email server"
          hint={`${PRODUCT_NAME} checks this server first. Anything marked Must fix stops the install; warnings don’t.`}
          card
          testId="hosting-install"
          action={
            <Button onClick={() => void runPreflight()} disabled={preflightBusy || sample}>
              {preflightBusy ? 'Checking…' : 'Check again'}
            </Button>
          }
          state={
            preflightError ? (
              <SettingsLoadState
                kind="error"
                title="Couldn’t check this server"
                detail={preflightError}
                action={<Button onClick={() => void runPreflight()}>Try again</Button>}
              >
                Nothing was installed. Try the checks again in a moment.
              </SettingsLoadState>
            ) : preflight === null ? (
              <SettingsLoadState kind="loading" title="Checking this server…">
                Ports, disk space, memory and DNS.
              </SettingsLoadState>
            ) : undefined
          }
        >
          {preflight ? <PreflightRows report={preflight} /> : null}
        </SettingsGroup>
      ) : null}

      {notInstalled ? (
        <SettingsGroup card testId="hosting-install-form">
          <SettingRow
            label="Mail server name"
            hint="Point this name at this server first, and set the server’s reverse DNS to it at your hosting provider."
            field
          >
            <Input
              id={hostId}
              aria-label="Mail server name"
              className="w-full"
              value={hostname}
              disabled={!canMutate || busy}
              onChange={(e) => {
                setHostname(e.target.value)
                setEnableError(null)
              }}
              placeholder="mail.example.com"
            />
          </SettingRow>
          <div className="px-4 py-3 flex items-center justify-end gap-4">
            {mustFix > 0 ? (
              <p className="type-hint text-[var(--color-text-secondary)]" data-testid="hosting-install-blocked">
                Fix the {mustFix === 1 ? 'check' : `${mustFix} checks`} marked Must fix above, then choose Check again.
              </p>
            ) : null}
            <Button variant="accent" size="md" disabled={!canMutate || busy || mustFix > 0 || preflightBusy} onClick={() => void doEnable()}>
              {busy ? 'Installing…' : 'Install email server'}
            </Button>
          </div>
          {enableError ? (
            <div className="px-4 py-3">
              <FailedNote what={plainFailure("The email server didn’t install.", enableError)} detail={enableError} next="Nothing changed on this server. Fix what it says, then try again." />
            </div>
          ) : null}
        </SettingsGroup>
      ) : null}

      {installing || (status.state === 'error' && hasProgress) ? (
        <SettingsGroup
          title={installing ? 'Installing' : 'Install steps'}
          hint={installing ? 'This takes a few minutes. You can leave this page; it carries on.' : 'The install stopped at the step marked below.'}
          card
          testId="hosting-steps"
          action={
            status.state === 'error' ? (
              <Button variant="accent" disabled={!canMutate || busy} onClick={() => void doEnable()}>
                {busy ? 'Resuming…' : 'Resume install'}
              </Button>
            ) : undefined
          }
        >
          <InstallSteps status={status} />
        </SettingsGroup>
      ) : null}

      {status.state === 'error' && !hasProgress ? (
        <SettingsGroup card>
          <SettingRow label="Resume the install" hint={`${PRODUCT_NAME} carries on from the last step that finished.`}>
            <Button variant="accent" disabled={!canMutate || busy} onClick={() => void doEnable()}>
              {busy ? 'Resuming…' : 'Resume install'}
            </Button>
          </SettingRow>
        </SettingsGroup>
      ) : null}

      {status.state === 'disabled' || status.state === 'stopped' ? (
        <SettingsGroup card testId="hosting-resume">
          <SettingRow label="Turn the server back on" hint="Your mail was kept. Your domains start receiving again.">
            <Button variant="accent" disabled={!canMutate || busy} onClick={() => void doEnable()}>
              {busy ? 'Turning on…' : 'Turn on'}
            </Button>
          </SettingRow>
        </SettingsGroup>
      ) : null}

      {enableError && !notInstalled ? (
        <FailedNote what={plainFailure("The email server didn’t start.", enableError)} detail={enableError} next="Nothing else changed. Try again in a moment." />
      ) : null}

      {!notInstalled && !installing ? (
        <SettingsGroup title={status.state === 'disabled' || status.state === 'stopped' ? 'Remove' : 'Turn off or remove'} card testId="hosting-danger">
          {status.state !== 'disabled' && status.state !== 'stopped' ? (
            <SettingRow label="Turn off the email server" hint="Stops the server and keeps all mail. Incoming mail bounces until you turn it back on.">
              <Button variant="danger-quiet" disabled={!canMutate || busy} onClick={() => setDialog('disable')}>
                Turn off…
              </Button>
            </SettingRow>
          ) : null}
          <SettingRow label="Uninstall the email server" hint="Removes the server program. Mail stays on this server unless you choose to delete it.">
            <Button variant="danger-quiet" disabled={!canMutate || busy} onClick={() => setDialog('uninstall')}>
              Uninstall…
            </Button>
          </SettingRow>
        </SettingsGroup>
      ) : null}

      {dialog === 'disable' ? (
        <DisableDialog
          onClose={() => setDialog(null)}
          onDone={(warning) => {
            setDialog(null)
            setNotice({ tone: 'ok', text: 'Email server turned off. Your mail is kept; turn it back on here.', detail: warning })
            onChanged()
          }}
        />
      ) : null}
      {dialog === 'uninstall' ? (
        <UninstallDialog
          hostname={status.hostname}
          onClose={() => setDialog(null)}
          onDone={(purged) => {
            setDialog(null)
            setNotice({ tone: 'ok', text: purged ? 'Email server uninstalled and its mail deleted.' : 'Email server uninstalled. Your mail is still on this server.' })
            onChanged()
          }}
        />
      ) : null}
    </div>
  )
}

function DisableDialog({ onClose, onDone }: { onClose: () => void; onDone: (warning?: string) => void }): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const run = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      const res = await disableServer()
      onDone(res.warning)
    } catch (e) {
      setError(reasonOf(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <AccessSheet
      title="Turn off the email server?"
      onClose={onClose}
      busy={busy}
      testId="hosting-disable-dialog"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy} autoFocus>
            Cancel
          </Button>
          <Button variant="danger-muted" size="md" onClick={() => void run()} disabled={busy}>
            {busy ? 'Turning off…' : 'Turn off'}
          </Button>
        </>
      }
    >
      <p className={DIALOG_MUTED}>
        Your mail is kept. Until you turn the server back on, mail sent to your domains bounces, and agents can’t send.
      </p>
      {error ? <FailedNote what={plainFailure("The email server is still on.", error)} detail={error} next="It’s still running and nothing changed. Try again in a moment." /> : null}
    </AccessSheet>
  )
}

function UninstallDialog({
  hostname,
  onClose,
  onDone,
}: {
  hostname: string | null
  onClose: () => void
  onDone: (purged: boolean) => void
}): React.JSX.Element {
  const [purge, setPurge] = useState(false)
  const [typed, setTyped] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const typedId = useId()
  const needs = hostname ?? ''
  const ready = !purge || (needs !== '' && typed.trim() === needs)
  const run = async (): Promise<void> => {
    if (!ready) return
    setBusy(true)
    setError(null)
    try {
      await uninstallServer(purge, purge ? typed.trim() : undefined)
      onDone(purge)
    } catch (e) {
      setError(reasonOf(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <AccessSheet
      title="Uninstall the email server?"
      onClose={onClose}
      busy={busy}
      testId="hosting-uninstall-dialog"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy} autoFocus>
            Cancel
          </Button>
          <Button variant="danger-muted" size="md" onClick={() => void run()} disabled={busy || !ready}>
            {busy ? 'Uninstalling…' : purge ? 'Uninstall and delete mail' : 'Uninstall'}
          </Button>
        </>
      }
    >
      <p className={DIALOG_MUTED}>{PRODUCT_NAME} removes the server program. Your domains and addresses stop working until you install it again.</p>
      <label className="flex items-start gap-2 cursor-pointer">
        <Checkbox className="mt-0.5" checked={purge} disabled={busy} onChange={(e) => setPurge(e.target.checked)} aria-label="Also delete all mail" />
        <span>
          <span className={SETTING_ROW_LABEL}>Also delete all mail</span>
          <span className={`block ${DIALOG_MUTED}`}>Every mailbox on this server, for good. This can’t be undone.</span>
        </span>
      </label>
      {purge ? (
        <Field label={`Type ${needs || 'the mail server name'} to confirm`} htmlFor={typedId}>
          <Input id={typedId} className={`${DIALOG_FIELD}`} value={typed} disabled={busy} onChange={(e) => setTyped(e.target.value)} placeholder={needs} />
        </Field>
      ) : null}
      {error ? <FailedNote what={plainFailure("The email server wasn’t uninstalled.", error)} detail={error} next="It’s still installed and nothing was deleted. Try again in a moment." /> : null}
    </AccessSheet>
  )
}

// ── Domains tab ──────────────────────────────────────────────────────────

function domainLine(d: DomainSummary): string {
  const broken = (d.records?.wrong ?? 0) + (d.records?.missing ?? 0)
  const mode = SEND_MODE_WORD[d.sendMode] ?? d.sendMode
  if (d.status === 'verified') return `${mode}${d.verifiedAt ? ` · verified ${fmtDay(d.verifiedAt)}` : ''}`
  if (broken > 0) return `${broken} DNS record${broken === 1 ? '' : 's'} to fix · ${mode.toLowerCase()}`
  return `Waiting for its DNS records · ${mode.toLowerCase()}`
}

function DomainList({
  domains,
  error,
  canMutate,
  onOpen,
  onAdd,
  onRetry,
}: {
  domains: DomainSummary[] | null
  error: string | null
  canMutate: boolean
  onOpen: (domain: string) => void
  onAdd: () => void
  onRetry: () => void
}): React.JSX.Element {
  const add = (
    <Button variant="accent" disabled={!canMutate} onClick={onAdd}>
      Add domain
    </Button>
  )
  return (
    <SettingsGroup
      title="Domains"
      hint="Agents get addresses on these domains. A domain starts working once its DNS records check out."
      action={domains && domains.length > 0 ? add : undefined}
      card
      testId="hosting-domains"
      state={
        error ? (
          <SettingsLoadState kind="error" title="Couldn’t load your domains" detail={error} action={<Button onClick={onRetry}>Try again</Button>}>
            Your domains are fine; this page just couldn’t read the list.
          </SettingsLoadState>
        ) : domains === null ? (
          <SettingsLoadState kind="loading" title="Loading domains…" />
        ) : domains.length === 0 ? (
          <SettingsLoadState kind="empty" title="No domains yet" action={add}>
            Add a domain you own. {PRODUCT_NAME} then shows the DNS records to add at your domain’s DNS provider.
          </SettingsLoadState>
        ) : undefined
      }
    >
      {(domains ?? []).map((d) => {
        const tag = domainTag(d.status)
        return (
          <button
            key={d.domain}
            type="button"
            data-k2-focus=""
            data-testid="hosting-domain-row"
            data-domain={d.domain}
            onClick={() => onOpen(d.domain)}
            className={`relative w-full text-left px-4 py-3 flex items-center justify-between gap-6 cursor-pointer outline-none ${SETTING_LIST_ROW_HOVER}`}
          >
            <div className="min-w-0">
              <p className="type-name text-[var(--color-text-primary)] truncate">{d.domain}</p>
              <p className="type-hint text-[var(--color-text-muted)] mt-0.5 truncate">{domainLine(d)}</p>
            </div>
            <span className="flex items-center gap-3 flex-shrink-0">
              <StatusWord tone={tag.tone}>{tag.label}</StatusWord>
              <svg aria-hidden="true" viewBox="0 0 16 16" className="w-3.5 h-3.5 text-[var(--color-text-secondary)]" fill="none" stroke="currentColor" strokeWidth={1.75}>
                <path d="M6 3.5 10.5 8 6 12.5" strokeLinecap="round" strokeLinejoin="round" />
              </svg>
            </span>
          </button>
        )
      })}
    </SettingsGroup>
  )
}

function AddDomainDialog({ onClose, onAdded, onExists }: { onClose: () => void; onAdded: (d: DomainDetail) => void; onExists: (domain: string) => void }): React.JSX.Element {
  const [value, setValue] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const id = useId()
  const formId = useId()
  const dom = value.trim().toLowerCase()
  const problem = dom && !/^[a-z0-9-]+(\.[a-z0-9-]+)+$/.test(dom) ? 'Enter a domain name like example.com, without http:// or an address.' : null
  const run = async (): Promise<void> => {
    if (!dom || problem || busy) return
    setBusy(true)
    setError(null)
    try {
      onAdded(await addDomain(dom))
    } catch (e) {
      const { code } = mailErrorInfo(e)
      if (code === 'exists') onExists(dom)
      else setError(reasonOf(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <AccessSheet
      title="Add a domain"
      intro={`Agents can get addresses on it once its DNS records check out. ${PRODUCT_NAME} shows you the records to add next.`}
      onClose={onClose}
      busy={busy}
      testId="hosting-add-domain"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" form={formId} variant="accent" size="md" disabled={!dom || !!problem || busy}>
            {busy ? 'Adding…' : 'Add domain'}
          </Button>
        </>
      }
    >
      <form
        id={formId}
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          void run()
        }}
      >
        <Field label="Domain" htmlFor={id} problem={problem} hint="A domain you own, like example.com. You’ll need access to its DNS settings.">
          <Input
            id={id}
            autoFocus
            className={`${DIALOG_FIELD}`}
            value={value}
            disabled={busy}
            aria-invalid={problem ? true : undefined}
            onChange={(e) => {
              setValue(e.target.value)
              setError(null)
            }}
            placeholder="example.com"
          />
        </Field>
        {error ? <FailedNote what={plainFailure(`Couldn’t add ${dom}.`, error)} detail={error} next="Nothing was added. Check the name, then try again." /> : null}
      </form>
    </AccessSheet>
  )
}

function RecordRow({ r }: { r: DnsRecordRow }): React.JSX.Element {
  const tag = recordTag(r.status)
  const live = r.live ?? []
  const instruction = r.category === 'instruction'
  return (
    <div className="px-4 py-2.5" data-testid="hosting-record" data-record={r.id}>
      <div className="flex items-baseline justify-between gap-6">
        <p className="min-w-0 truncate">
          <span className={SETTING_ROW_LABEL}>{r.purpose}</span>
          <span className="type-hint text-[var(--color-text-secondary)]">
            {' · '}
            {instruction ? 'at your hosting provider for ' : `${r.type} at `}
            {r.name}
          </span>
        </p>
        <StatusWord tone={tag.tone}>{tag.label}</StatusWord>
      </div>
      <div
        data-k2-chamfer="control"
        className="mt-1.5 flex items-center gap-2 bg-[var(--color-bg)] border border-[var(--color-border)] rounded-[var(--radius-field)] pl-2 pr-1 py-1"
      >
        <span className="flex-1 min-w-0 type-hint text-[var(--color-text-secondary)] break-all">{r.expectedDisplay}</span>
        {!instruction ? <CopyButton text={r.expected} label={`the ${r.purpose} value`} /> : null}
      </div>
      {r.status === 'wrong' || (r.status === 'missing' && live.length > 0) ? (
        <div className="mt-1.5" role="note">
          <p className="type-hint text-[var(--color-status-error-soft)]">
            {r.status === 'wrong'
              ? 'Your DNS has a different value here. Replace it with the one above, exactly as shown.'
              : 'Your DNS has other records at this name, but not this one. Add the one above.'}
          </p>
          <p className={`${SETTING_SERVER_REASON}`}>Now: {live.join(' · ')}</p>
        </div>
      ) : null}
    </div>
  )
}

function DomainDetailView({
  domain,
  sample,
  canMutate,
  revision,
  justAdded,
  onBack,
  onChanged,
  onRemoved,
}: {
  domain: string
  sample: boolean
  canMutate: boolean
  revision: number
  justAdded: boolean
  onBack: () => void
  onChanged: () => void
  onRemoved: (domain: string, retired: number, purged: boolean) => void
}): React.JSX.Element {
  const [detail, setDetail] = useState<DomainDetail | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [checking, setChecking] = useState(false)
  const [checkError, setCheckError] = useState<string | null>(null)
  const [checked, setChecked] = useState(false)
  const [advancedOpen, setAdvancedOpen] = useState(false)
  const [removing, setRemoving] = useState(false)
  const [load, setLoad] = useState(0)

  useEffect(() => {
    setAdvancedOpen(false)
    if (sample) {
      setDetail(SAMPLE_DOMAIN_DETAILS[domain] ?? null)
      setError(SAMPLE_DOMAIN_DETAILS[domain] ? null : 'no sample for this domain')
      return
    }
    let cancelled = false
    fetchDomainDetail(domain)
      .then((d) => {
        if (cancelled) return
        if (!d || !Array.isArray(d.records)) {
          setError('the reply had no DNS records')
          return
        }
        setDetail(d)
        setError(null)
      })
      .catch((e) => {
        if (!cancelled) setError(reasonOf(e))
      })
    return () => {
      cancelled = true
    }
  }, [domain, sample, revision, load])

  const checkNow = useCallback(async (): Promise<void> => {
    setChecking(true)
    setCheckError(null)
    setChecked(false)
    try {
      setDetail(await checkDomainNow(domain))
      setChecked(true)
    } catch (e) {
      setCheckError(reasonOf(e))
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

  const back = (
    <button
      type="button"
      data-k2-focus=""
      onClick={onBack}
      className="relative inline-flex items-center gap-1 px-1 -mx-1 type-hint text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] cursor-pointer outline-none"
      data-testid="hosting-domain-back"
    >
      <svg aria-hidden="true" viewBox="0 0 16 16" className="w-3 h-3" fill="none" stroke="currentColor" strokeWidth={1.75}>
        <path d="M10 3.5 5.5 8 10 12.5" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
      All domains
    </button>
  )

  if (error || detail === null || detail.domain !== domain) {
    return (
      <div className="space-y-4">
        {back}
        {error ? (
          <SettingsLoadState kind="error" title={`Couldn’t load ${domain}`} detail={error} action={<Button onClick={() => setLoad((n) => n + 1)}>Try again</Button>}>
            The domain is unchanged; this page just couldn’t read its records.
          </SettingsLoadState>
        ) : (
          <SettingsLoadState kind="loading" title={`Loading ${domain}…`} />
        )}
      </div>
    )
  }

  const tag = domainTag(detail.status)
  const main = detail.records.filter((r) => r.category !== 'advanced')
  const advanced = detail.records.filter((r) => r.category === 'advanced')
  const toFix = main.filter((r) => r.status === 'missing' || r.status === 'wrong').length

  return (
    <div className="space-y-6" data-testid="hosting-domain-detail" data-domain={domain}>
      <div className="space-y-2">
        {back}
        <div className="flex items-center gap-3 min-w-0">
          <h3 className="type-heading text-[var(--color-text-primary)] truncate">{detail.domain}</h3>
          <StatusWord tone={tag.tone}>{tag.label}</StatusWord>
        </div>
        <p className={SETTING_ROW_HINT}>
          {detail.status === 'verified'
            ? `Verified on ${fmtDay(detail.verifiedAt)}. ${PRODUCT_NAME} checks it again every day.`
            : 'It starts working once the mail routing, sender and signing records below check out.'}{' '}
          Plus addressing is on: mail to name+anything@{detail.domain} still reaches name@{detail.domain}, so an agent can hand
          out a tagged address without making a new one. Mail to names that don’t exist is refused.
        </p>
      </div>

      {justAdded && !checked ? (
        <Callout tone="ok" role="status" className="type-hint" data-testid="hosting-domain-added">
          Added {detail.domain}. Next, add the records below at your domain’s DNS provider, then choose Check now.
        </Callout>
      ) : null}
      {detail.dmarcNag ? (
        <Callout tone="warn" className="type-hint">
          Add the DMARC record below too. It doesn’t stop the domain verifying, but other mail servers trust your mail more with it.
        </Callout>
      ) : null}

      <SettingsGroup
        title="DNS records"
        hint={`Add these at your domain’s DNS provider, then choose Check now. Changes can take up to 48 hours to show up.${
          toFix > 0 ? ` ${toFix} to fix.` : ''
        }${checked ? ' Last checked just now.' : detail.lastCheckedAt ? ` Last checked ${fmtWhen(detail.lastCheckedAt)}.` : ''}`}
        card
        testId="hosting-records"
        action={
          <span className="flex items-center gap-2">
            <Button onClick={downloadZone}>Download zone file</Button>
            <Button variant="accent" disabled={sample || checking} onClick={() => void checkNow()}>
              {checking ? 'Checking…' : 'Check now'}
            </Button>
          </span>
        }
      >
        {checkError ? (
          <div className="px-4 py-3">
            <FailedNote what={plainFailure("Couldn’t check the records.", checkError)} detail={checkError} next="The results below are from the last check. Try again in a moment." />
          </div>
        ) : checked ? (
          <div className="px-4 py-3" role="status" data-testid="hosting-checked">
            <StatusWord tone={toFix > 0 ? 'warn' : detail.status === 'verified' ? 'ok' : 'neutral'}>
              {toFix > 0
                ? `Checked just now: ${toFix} record${toFix === 1 ? '' : 's'} still to fix`
                : detail.status === 'verified'
                  ? 'Checked just now: everything checks out'
                  : 'Checked just now: waiting for the rest to show up'}
            </StatusWord>
          </div>
        ) : null}
        {main.map((r) => (
          <RecordRow key={r.id} r={r} />
        ))}
        {advanced.length > 0 ? (
          <button
            type="button"
            data-k2-focus=""
            aria-expanded={advancedOpen}
            onClick={() => setAdvancedOpen((v) => !v)}
            className={`relative w-full text-left px-4 py-3 flex items-center justify-between gap-6 cursor-pointer outline-none ${SETTING_LIST_ROW_HOVER}`}
          >
            <span className="min-w-0">
              <span className={`block ${SETTING_ROW_LABEL}`}>Optional records ({advanced.length})</span>
              <span className={`block ${SETTING_ROW_HINT}`}>Help mail apps set themselves up and make delivery stricter. Add them when you’re ready.</span>
            </span>
            <span className="type-hint text-[var(--color-text-secondary)] whitespace-nowrap">{advancedOpen ? 'Hide' : 'Show'}</span>
          </button>
        ) : null}
        {advancedOpen ? advanced.map((r) => <RecordRow key={r.id} r={r} />) : null}
      </SettingsGroup>

      <SendingGroup detail={detail} canMutate={canMutate} onChanged={onChanged} />

      <SettingsGroup title="Remove" card testId="hosting-domain-danger">
        <SettingRow label={`Remove ${detail.domain}`} hint="Retires every address on it. Its mail is kept unless you choose to delete it.">
          <Button variant="danger-quiet" disabled={!canMutate} onClick={() => setRemoving(true)}>
            Remove…
          </Button>
        </SettingRow>
      </SettingsGroup>

      {removing ? (
        <RemoveDomainDialog
          domain={detail.domain}
          onClose={() => setRemoving(false)}
          onDone={(retired, purged) => {
            setRemoving(false)
            onRemoved(detail.domain, retired, purged)
          }}
        />
      ) : null}
    </div>
  )
}

const SEND_MODE_HINT: Record<string, string> = {
  'receive-only': 'Agents can receive mail on this domain but can’t send from it.',
  direct: 'Mail goes straight from this server. Works best when your hosting provider doesn’t block outgoing mail.',
  relay: 'Mail goes through a sending service you already use, such as your mail provider’s relay.',
}

function SendingGroup({ detail, canMutate, onChanged }: { detail: DomainDetail; canMutate: boolean; onChanged: () => void }): React.JSX.Element {
  const [modeDraft, setModeDraft] = useState<string | null>(null)
  const [relayHost, setRelayHost] = useState('')
  const [relayPort, setRelayPort] = useState('587')
  const [relayUser, setRelayUser] = useState('')
  const [relayPass, setRelayPass] = useState('')
  const [spfInclude, setSpfInclude] = useState('')
  const [busy, setBusy] = useState(false)
  const save = useSaveStatus()
  const [notBuilt, setNotBuilt] = useState(false)

  useEffect(() => {
    setModeDraft(null)
    setNotBuilt(false)
    save.reset()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [detail.domain])

  const mode = modeDraft ?? detail.sendMode
  const dirty = modeDraft !== null && modeDraft !== detail.sendMode
  const relayReady = mode !== 'relay' || relayHost.trim() !== ''

  const submit = async (): Promise<void> => {
    setBusy(true)
    setNotBuilt(false)
    try {
      const body: Record<string, unknown> = { domain: detail.domain, sendMode: mode }
      if (mode === 'relay') {
        body.relayHost = relayHost.trim()
        body.relayPort = Number(relayPort) || 587
        body.relayUser = relayUser.trim()
        if (relayPass) body.relayPass = relayPass
        if (spfInclude.trim()) body.spfInclude = spfInclude.trim()
      }
      await setMailConfig(body)
      save.saved()
      onChanged()
    } catch (e) {
      if (isNotBuilt(e)) setNotBuilt(true)
      save.failed(new Error(reasonOf(e)))
    } finally {
      setBusy(false)
    }
  }

  const relayField = (label: string, hint: string, value: string, set: (v: string) => void, placeholder: string, type = 'text'): React.JSX.Element => (
    <SettingRow label={label} hint={hint} field>
      <Input
        aria-label={label}
        type={type}
        className="w-full"
        value={value}
        disabled={!canMutate || busy}
        onChange={(e) => set(e.target.value)}
        placeholder={placeholder}
      />
    </SettingRow>
  )

  return (
    <SettingsGroup title="Sending" hint="How mail from this domain leaves the server." card settingId="email-hosting.send-mode" testId="hosting-sending">
      <SettingRow label="How mail leaves" hint={SEND_MODE_HINT[mode] ?? ''} field>
        <SettingDropdown
          ariaLabel="How mail leaves"
          value={mode}
          disabled={!canMutate || busy}
          onChange={(v) => {
            setModeDraft(v)
            save.reset()
          }}
          fullWidth
          options={[
            { value: 'receive-only', label: 'Receive only' },
            { value: 'direct', label: 'Send from this server' },
            { value: 'relay', label: 'Send through a relay' },
          ]}
        />
      </SettingRow>
      {mode === 'relay' ? (
        <>
          {relayField('Relay server', 'The sending service’s server name.', relayHost, setRelayHost, 'smtp.relay.example')}
          {relayField('Port', 'Usually 587.', relayPort, setRelayPort, '587')}
          {relayField('Username', 'The sign-in your relay gave you.', relayUser, setRelayUser, 'postmaster@example.com')}
          {relayField('Password', 'Kept on this server; never shown again.', relayPass, setRelayPass, '', 'password')}
          {relayField('SPF include', `From your relay’s setup page. ${PRODUCT_NAME} adds it to the SPF record above.`, spfInclude, setSpfInclude, 'include:relay.example')}
        </>
      ) : null}
      <div className="px-4 py-3">
        <div className="flex items-center justify-end gap-3">
          <SaveNote state={save.state} />
          <Button variant="accent" disabled={!canMutate || busy || !dirty || !relayReady} onClick={() => void submit()}>
            {busy ? 'Saving…' : 'Save'}
          </Button>
        </div>
        {notBuilt ? (
          <p role="alert" className="type-hint text-[var(--color-status-error-soft)] mt-1 leading-relaxed">
            This server can’t change how mail leaves yet. {detail.domain} stays {(SEND_MODE_WORD[detail.sendMode] ?? detail.sendMode).toLowerCase()} for now.
          </p>
        ) : (
          <SaveError state={save.state} />
        )}
      </div>
    </SettingsGroup>
  )
}

function RemoveDomainDialog({ domain, onClose, onDone }: { domain: string; onClose: () => void; onDone: (retired: number, purged: boolean) => void }): React.JSX.Element {
  const [purge, setPurge] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const run = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      const res = await removeDomain(domain, purge)
      onDone(typeof res?.retiredAddresses === 'number' ? res.retiredAddresses : 0, purge)
    } catch (e) {
      setError(reasonOf(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <AccessSheet
      title={`Remove ${domain}?`}
      onClose={onClose}
      busy={busy}
      testId="hosting-remove-domain"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy} autoFocus>
            Cancel
          </Button>
          <Button variant="danger-muted" size="md" onClick={() => void run()} disabled={busy}>
            {busy ? 'Removing…' : 'Remove domain'}
          </Button>
        </>
      }
    >
      <p className={DIALOG_MUTED}>Every address on {domain} is retired and stops receiving mail. You can add the domain again later.</p>
      <label className="flex items-start gap-2 cursor-pointer">
        <Checkbox className="mt-0.5" checked={purge} disabled={busy} onChange={(e) => setPurge(e.target.checked)} aria-label="Also delete this domain’s mail" />
        <span>
          <span className={SETTING_ROW_LABEL}>Also delete this domain’s mail</span>
          <span className={`block ${DIALOG_MUTED}`}>For good. Leave it off to keep the mail on this server.</span>
        </span>
      </label>
      {error ? <FailedNote what={plainFailure(`${domain} wasn’t removed.`, error)} detail={error} next="Nothing changed. Try again in a moment." /> : null}
    </AccessSheet>
  )
}

// ── Addresses tab ────────────────────────────────────────────────────────

function AddressesTab({
  sample,
  canMutate,
  revision,
  onChanged,
  setNotice,
}: {
  sample: boolean
  canMutate: boolean
  revision: number
  onChanged: () => void
  setNotice: (n: Notice | null) => void
}): React.JSX.Element {
  const [rows, setRows] = useState<AddressRow[] | null>(sample ? SAMPLE_ADDRESSES : null)
  // The unified catalog (hosted rows only) — the access SSOT per address.
  const [inboxes, setInboxes] = useState<Inbox[] | null>(sample ? SAMPLE_INBOXES : null)
  const [error, setError] = useState<string | null>(null)
  const [load, setLoad] = useState(0)
  const [dialog, setDialog] = useState<{ kind: 'retire' | 'rotate'; row: AddressRow } | null>(null)
  const [onceCreds, setOnceCreds] = useState<RotatedMailboxPassword | null>(null)
  // FW4: which mailbox's Forwarding panel is open (one at a time; the
  // panel loads only when opened, never in sample mode).
  const [forwardOpen, setForwardOpen] = useState<string | null>(null)

  useEffect(() => {
    if (sample) {
      setRows(SAMPLE_ADDRESSES)
      setInboxes(SAMPLE_INBOXES)
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
        if (!cancelled) setError(reasonOf(e))
      })
    // Access rows for each hosted address (Primary + grants). A failure
    // just hides the access panels; it doesn't fail the address list.
    fetchInboxes()
      .then((all) => {
        if (!cancelled) setInboxes(all.filter((i) => i.source === 'hosted'))
      })
      .catch(() => {
        if (!cancelled) setInboxes([])
      })
    return () => {
      cancelled = true
    }
  }, [sample, revision, load])

  const patchInbox = useCallback((address: string, patch: (i: Inbox) => Inbox): void => {
    setInboxes((prev) => (prev ? prev.map((i) => (i.address === address ? patch(i) : i)) : prev))
  }, [])

  const inboxByAddress = useMemo(() => {
    const m = new Map<string, Inbox>()
    for (const i of inboxes ?? []) m.set(i.address, i)
    return m
  }, [inboxes])

  const groups = useMemo(() => {
    const byDomain = new Map<string, AddressRow[]>()
    for (const r of rows ?? []) {
      const dom = r.address.split('@')[1] ?? 'Other'
      const list = byDomain.get(dom) ?? []
      list.push(r)
      byDomain.set(dom, list)
    }
    return [...byDomain.entries()].sort(([a], [b]) => a.localeCompare(b))
  }, [rows])

  const intro = (
    <p className={SETTING_ROW_HINT} data-settings-id="email-hosting.addresses">
      Agents make their own addresses on verified domains, up to five each. Plus addressing gives them more without new addresses.
    </p>
  )

  if (error || rows === null || groups.length === 0) {
    return (
      <div className="space-y-4">
        {intro}
        {error ? (
          <SettingsLoadState kind="error" title="Couldn’t load addresses" detail={error} action={<Button onClick={() => setLoad((n) => n + 1)}>Try again</Button>}>
            Your addresses are fine; this page just couldn’t read the list.
          </SettingsLoadState>
        ) : rows === null ? (
          <SettingsLoadState kind="loading" title="Loading addresses…" />
        ) : (
          <SettingsLoadState kind="empty" title="No addresses yet">
            Agents make addresses once a domain is verified. Add a domain on the Domains tab first.
          </SettingsLoadState>
        )}
      </div>
    )
  }

  return (
    <div className="space-y-6">
      {intro}
      {onceCreds ? <PasswordShownOnce creds={onceCreds} onClose={() => setOnceCreds(null)} /> : null}
      {groups.map(([dom, list]) => (
        <SettingsGroup key={dom} title={dom} card testId="hosting-address-group">
          {list.map((r) => {
            const inbox = r.status === 'active' ? inboxByAddress.get(r.address) : undefined
            const retired = r.status === 'retired'
            return (
              <div key={r.id} className={`px-4 py-3 ${SETTING_LIST_ROW_HOVER}`} data-testid="hosting-address-row" data-address={r.address}>
                <div className="flex items-center justify-between gap-6">
                  <div className="min-w-0">
                    <p className={`type-name truncate ${retired ? 'text-[var(--color-text-secondary)]' : 'text-[var(--color-text-primary)]'}`}>{r.address}</p>
                    <p className="type-hint text-[var(--color-text-muted)] mt-0.5 truncate">
                      {r.holderWorkspace ? `Held by ${r.holderWorkspace}` : 'Its workspace was removed'}
                      {r.createdAt ? ` · made ${fmtDay(r.createdAt)}` : ''}
                      {r.retiredAt ? ` · retired ${fmtDay(r.retiredAt)}` : ''}
                    </p>
                  </div>
                  <div className="flex items-center gap-2 flex-shrink-0">
                    {retired ? (
                      <StatusWord tone="neutral">Retired</StatusWord>
                    ) : (
                      <>
                        {!sample ? (
                          <Button aria-expanded={forwardOpen === r.id} onClick={() => setForwardOpen((cur) => (cur === r.id ? null : r.id))}>
                            Forwarding
                          </Button>
                        ) : null}
                        <Button disabled={!canMutate} onClick={() => setDialog({ kind: 'rotate', row: r })}>
                          New password…
                        </Button>
                        <Button variant="danger-quiet" disabled={!canMutate} onClick={() => setDialog({ kind: 'retire', row: r })}>
                          Retire…
                        </Button>
                      </>
                    )}
                  </div>
                </div>
                {!sample && !retired && forwardOpen === r.id ? (
                  <div className="mt-3">
                    <MailboxForwardingPanel address={r.address} canMutate={canMutate} />
                  </div>
                ) : null}
                {inbox ? (
                  <div className="mt-3 pt-2 border-t border-[var(--color-border)]">
                    <InboxAccessPanel inbox={inbox} canMutate={canMutate} patchInbox={patchInbox} />
                  </div>
                ) : null}
              </div>
            )
          })}
        </SettingsGroup>
      ))}

      {dialog?.kind === 'retire' ? (
        <RetireDialog
          row={dialog.row}
          onClose={() => setDialog(null)}
          onDone={(row) => {
            setDialog(null)
            setRows((prev) => prev?.map((x) => (x.id === row.id ? { ...x, status: 'retired', retiredAt: Math.floor(Date.now() / 1000) } : x)) ?? null)
            setNotice({ tone: 'ok', text: `Retired ${row.address}. It no longer receives mail; its mailbox is kept for a while, then deleted.` })
            onChanged()
          }}
        />
      ) : null}
      {dialog?.kind === 'rotate' ? (
        <RotateDialog
          row={dialog.row}
          onClose={() => setDialog(null)}
          onDone={(creds) => {
            setDialog(null)
            setOnceCreds(creds)
          }}
        />
      ) : null}
    </div>
  )
}

function RetireDialog({ row, onClose, onDone }: { row: AddressRow; onClose: () => void; onDone: (row: AddressRow) => void }): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const run = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await retireAddress(row.holderProjectId, row.address)
      onDone(row)
    } catch (e) {
      setError(reasonOf(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <AccessSheet
      title={`Retire ${row.address}?`}
      onClose={onClose}
      busy={busy}
      testId="hosting-retire-dialog"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy} autoFocus>
            Cancel
          </Button>
          <Button variant="danger-muted" size="md" onClick={() => void run()} disabled={busy}>
            {busy ? 'Retiring…' : 'Retire address'}
          </Button>
        </>
      }
    >
      <p className={DIALOG_MUTED}>
        It stops receiving mail straight away. Its mailbox is kept for a while, then deleted. {row.holderWorkspace ?? 'The agent'} can make another address in its place.
      </p>
      {error ? <FailedNote what={plainFailure(`${row.address} wasn’t retired.`, error)} detail={error} next="It still works. Try again in a moment." /> : null}
    </AccessSheet>
  )
}

function RotateDialog({ row, onClose, onDone }: { row: AddressRow; onClose: () => void; onDone: (c: RotatedMailboxPassword) => void }): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const run = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      onDone(await rotateAddressPassword(row.address))
    } catch (e) {
      setError(reasonOf(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <AccessSheet
      title={`New password for ${row.address}?`}
      onClose={onClose}
      busy={busy}
      testId="hosting-rotate-dialog"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy} autoFocus>
            Cancel
          </Button>
          <Button variant="danger-muted" size="md" onClick={() => void run()} disabled={busy}>
            {busy ? 'Making…' : 'Make new password'}
          </Button>
        </>
      }
    >
      <p className={DIALOG_MUTED}>
        Mail apps signed in with the old password are signed out, and its app passwords stop working. {PRODUCT_NAME} shows the new one once.
      </p>
      {error ? <FailedNote what={plainFailure("No new password was made.", error)} detail={error} next="The old password still works. Try again in a moment." /> : null}
    </AccessSheet>
  )
}

function PasswordShownOnce({ creds, onClose }: { creds: RotatedMailboxPassword; onClose: () => void }): React.JSX.Element {
  return (
    <AccessSheet
      title={`New password for ${creds.address}`}
      onClose={onClose}
      testId="hosting-password-once"
      footer={
        <Button variant="accent" size="md" onClick={onClose} autoFocus>
          Done
        </Button>
      }
    >
      <p className={DIALOG_MUTED}>{creds.note ?? 'Use these in a mail app. Mail apps on the old password are signed out.'}</p>
      <Surface chamfer material="tile" className="divide-y divide-[var(--color-border)]">
        <CopyLine mono={false} label="Server" value={creds.imap?.host || creds.submission?.host || ''} />
        <CopyLine mono={false} label="Username" value={creds.username || creds.address} />
        <CopyLine mono={false} label="Password" value={creds.password} testId="hosting-once-password" />
      </Surface>
      <p className={DIALOG_MUTED}>
        Incoming mail (IMAP) on port {creds.imap?.port ?? 443}, sending (SMTP) on port {creds.submission?.port ?? 465}. {PRODUCT_NAME} won’t show this password again.
      </p>
    </AccessSheet>
  )
}

// ── Approvals tab ────────────────────────────────────────────────────────

function ApprovalRow({ item, canMutate, onDecided }: { item: ApprovalItem; canMutate: boolean; onDecided: (id: string, text: string, failed?: string) => void }): React.JSX.Element {
  const [expanded, setExpanded] = useState(false)
  const [denying, setDenying] = useState(false)
  const [busy, setBusy] = useState(false)
  const [fullText, setFullText] = useState<string | null>(null)
  const [rowError, setRowError] = useState<{ what: string; detail: string } | null>(null)

  const loadFullText = async (): Promise<void> => {
    try {
      setFullText(await fetchApprovalText(item.id))
    } catch (e) {
      setRowError({ what: 'Couldn’t load the whole message.', detail: reasonOf(e) })
    }
  }

  const saveFile = async (file: ApprovalFile): Promise<void> => {
    try {
      const url = URL.createObjectURL(await fetchApprovalFile(item.id, file.n))
      const a = document.createElement('a')
      a.href = url
      a.download = file.name
      a.click()
      URL.revokeObjectURL(url)
    } catch (e) {
      setRowError({ what: `Couldn’t save ${file.name}.`, detail: reasonOf(e) })
    }
  }

  const approve = async (): Promise<void> => {
    setBusy(true)
    setRowError(null)
    try {
      await approveOutbound(item.id)
      onDecided(item.id, `Approved “${item.subject || 'No subject'}”. It’s on its way.`)
    } catch (e) {
      // `engine` failures mean approved-but-not-submitted: the row leaves
      // the queue either way, and the page says it didn't go out.
      onDecided(item.id, `Approved “${item.subject || 'No subject'}”, but it didn’t send.`, reasonOf(e))
    } finally {
      setBusy(false)
    }
  }

  const to = [...item.to, ...item.cc.map((c) => `${c} (cc)`)].join(', ')
  return (
    <div className="px-4 py-3 space-y-2" data-testid="hosting-approval" data-id={item.id}>
      <div className="flex items-start justify-between gap-6">
        <div className="min-w-0">
          <p className="type-name text-[var(--color-text-primary)] truncate" title={item.subject}>
            {item.subject || 'No subject'}
          </p>
          <p className={`${SETTING_ROW_HINT} break-words`}>
            From {item.from}
            {item.fromName ? ` shown as “${item.fromName}”` : ''} to {to}
          </p>
          <p className="type-hint text-[var(--color-text-muted)] mt-0.5">
            {item.agentName ?? 'An agent'}
            {item.workspace && item.workspace !== item.agentName ? ` in ${item.workspace}` : ''} · {fmtWhen(item.createdAt)}
            {item.expiresAt ? ` · denied for you on ${fmtDay(item.expiresAt)} if you don’t decide` : ''}
          </p>
        </div>
        <div className="flex items-center gap-2 flex-shrink-0">
          <Button variant="danger-quiet" disabled={!canMutate || busy} onClick={() => setDenying(true)}>
            Deny…
          </Button>
          <Button variant="accent" disabled={!canMutate || busy} onClick={() => void approve()}>
            {busy ? 'Sending…' : 'Approve and send'}
          </Button>
        </div>
      </div>

      {(item.bcc?.length ?? 0) > 0 || (item.policyBcc?.length ?? 0) > 0 ? (
        <p className="type-hint text-[var(--color-text-secondary)] break-words">
          Hidden copies to {[...(item.bcc ?? []), ...(item.policyBcc ?? [])].join(', ')}
          {(item.policyBcc?.length ?? 0) > 0 ? ' (some from your always-copy setting)' : ''}.
        </p>
      ) : null}
      {(item.replyTo?.length ?? 0) > 0 ? (
        <p className="type-hint text-[var(--color-status-warn-text)] break-words">
          Replies go to {item.replyTo?.join(', ')}, not to the sender.
        </p>
      ) : null}
      {item.sourceCheck ? <p className="type-hint text-[var(--color-status-warn-text)] break-words">The message it answers {item.sourceCheck}. Its sender may be forged.</p> : null}
      {item.sourceDmarc ? (
        <p className={`type-hint break-words ${item.sourceDmarc === 'fail' ? 'text-[var(--color-status-warn-text)]' : 'text-[var(--color-text-secondary)]'}`}>
          The message it answers {item.sourceDmarc === 'fail' ? 'failed its sender check, so its sender may be forged.' : `had a sender check result of “${item.sourceDmarc}”.`}
        </p>
      ) : null}
      {item.forwardOf ? (
        <p className="type-hint text-[var(--color-text-secondary)] break-words">
          Forwards a message in {item.forwardOf.mailbox} from {item.forwardOf.sender} (not checked)
          {item.forwardOf.asEml ? ', attached whole' : ''}.
        </p>
      ) : null}

      {(item.files?.length ?? 0) > 0 ? (
        <div className="space-y-1">
          <p className="type-hint text-[var(--color-text-secondary)]">Files that go with it, as they were when the agent sent this:</p>
          {item.files?.map((f) => (
            <div key={f.n} className="flex items-center justify-between gap-4 min-w-0">
              <span className="type-hint min-w-0 truncate">
                <span className="text-[var(--color-text-primary)]">{f.name}</span>
                <span className="text-[var(--color-text-secondary)]">
                  {' '}
                  · {fmtFileSize(f.size)}
                  {f.scanned ? '' : ' · not checked for secrets'}
                </span>
              </span>
              <Button variant="quiet" onClick={() => void saveFile(f)}>
                Save a copy
              </Button>
            </div>
          ))}
        </div>
      ) : null}

      {/* Body — NEVER auto-rendered; explicit expand, plain text only
          (untrusted external content, §8.1). */}
      <div>
        <Button variant="quiet" aria-expanded={expanded} onClick={() => setExpanded((v) => !v)}>
          {expanded ? 'Hide message' : 'Show message'}
        </Button>
        {expanded ? (
          <div className="mt-2 space-y-1">
            <p className="type-hint text-[var(--color-text-secondary)]">Written by the agent. Shown as plain text.</p>
            <div
              data-k2-chamfer="control"
              className="type-hint text-[var(--color-text-secondary)] whitespace-pre-wrap break-words bg-[var(--color-bg)] border border-[var(--color-border)] rounded-[var(--radius-field)] px-2 py-1.5 max-h-48 overflow-y-auto"
            >
              {fullText ?? item.bodyPreview ?? 'This message has no text preview.'}
            </div>
            {fullText === null && item.bodyPreview !== null && item.bodyPreview.length >= 280 ? (
              <Button variant="quiet" onClick={() => void loadFullText()}>
                Show the whole message
              </Button>
            ) : null}
          </div>
        ) : null}
      </div>
      {rowError ? (
        <p role="alert" className="type-hint text-[var(--color-status-error-soft)]">
          {rowError.what} <span className="text-[var(--color-text-muted)]">The server said: {rowError.detail.trim().replace(/[.\s]+$/, '')}.</span>
        </p>
      ) : null}

      {denying ? (
        <DenyDialog
          item={item}
          onClose={() => setDenying(false)}
          onDone={() => {
            setDenying(false)
            onDecided(item.id, `Denied “${item.subject || 'No subject'}”. ${item.agentName ?? 'The agent'} gets your reason.`)
          }}
        />
      ) : null}
    </div>
  )
}

function DenyDialog({ item, onClose, onDone }: { item: ApprovalItem; onClose: () => void; onDone: () => void }): React.JSX.Element {
  const [note, setNote] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const id = useId()
  const formId = useId()
  const run = async (): Promise<void> => {
    if (!note.trim() || busy) return
    setBusy(true)
    setError(null)
    try {
      await denyOutbound(item.id, note.trim())
      onDone()
    } catch (e) {
      setError(reasonOf(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <AccessSheet
      title="Deny this message?"
      intro={`“${item.subject || 'No subject'}” won’t be sent. ${item.agentName ?? 'The agent'} sees your reason.`}
      onClose={onClose}
      busy={busy}
      testId="hosting-deny-dialog"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" form={formId} variant="danger-muted" size="md" disabled={busy || !note.trim()}>
            {busy ? 'Denying…' : 'Deny'}
          </Button>
        </>
      }
    >
      <form
        id={formId}
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          void run()
        }}
      >
        <Field label="Reason" htmlFor={id} hint="Say what to change, like “Send it to the vendor contact only.”">
          <Input id={id} autoFocus className={DIALOG_FIELD} value={note} disabled={busy} onChange={(e) => setNote(e.target.value)} />
        </Field>
        {error ? <FailedNote what={plainFailure("The message wasn’t denied.", error)} detail={error} next="It’s still waiting for you. Try again in a moment." /> : null}
      </form>
    </AccessSheet>
  )
}

const OUTBOUND_TAG: Record<string, { label: string; tone: HealthTone }> = {
  pending_approval: { label: 'Waiting', tone: 'warn' },
  approved: { label: 'Approved', tone: 'ok' },
  submitted: { label: 'Sent', tone: 'ok' },
  rejected: { label: 'Denied', tone: 'error' },
  failed: { label: 'Didn’t send', tone: 'error' },
}

const SAMPLE_HISTORY: OutboundItem[] = [
  {
    id: 'out_11ab', from: 'research-bot@example.org', to: ['noreply@service.example'], cc: [],
    subject: 'Re: Verify your account', status: 'submitted', statusNote: 'accepted for delivery', note: null,
    agentName: 'research-bot', decidedBy: 'owner', createdAt: 1751700000, decidedAt: 1751700120, sentAt: 1751700121,
  },
  {
    id: 'out_09cd', from: 'signup-runner@example.org', to: ['everyone@company.example'], cc: [],
    subject: 'Bulk outreach draft', status: 'rejected', statusNote: 'denied', note: 'Too many people. Send it to the vendor contact only.',
    agentName: 'signup-runner', decidedBy: 'owner', createdAt: 1751600000, decidedAt: 1751600500, sentAt: null,
  },
]

function ApprovalsTab({
  pending,
  pendingError,
  sample,
  canMutate,
  revision,
  onDecided,
  onRetry,
}: {
  pending: ApprovalItem[] | null
  pendingError: string | null
  sample: boolean
  canMutate: boolean
  revision: number
  onDecided: (id: string, text: string, failed?: string) => void
  onRetry: () => void
}): React.JSX.Element {
  // Decided history: per-workspace outbox (no owner-wide history route
  // yet — the picker selects which workspace's outbox to show).
  const projects = useProjectsStore((s) => s.projects)
  const [historyWs, setHistoryWs] = useState('')
  const [history, setHistory] = useState<OutboundItem[] | null>(null)
  const [historyError, setHistoryError] = useState<string | null>(null)
  const [historyLoading, setHistoryLoading] = useState(false)
  const [historyLoad, setHistoryLoad] = useState(0)

  useEffect(() => {
    if (sample) {
      setHistory(SAMPLE_HISTORY)
      return
    }
    if (!historyWs) {
      setHistory(null)
      setHistoryError(null)
      return
    }
    let cancelled = false
    setHistoryLoading(true)
    fetchOutbox(historyWs)
      .then((r) => {
        if (cancelled) return
        setHistory(r)
        setHistoryError(null)
      })
      .catch((e) => {
        if (!cancelled) setHistoryError(reasonOf(e))
      })
      .finally(() => {
        if (!cancelled) setHistoryLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [sample, historyWs, revision, historyLoad])

  const wsName = projects.find((p) => p.id === historyWs)?.name ?? 'this workspace'

  return (
    <div className="space-y-6">
      <SettingsGroup
        title="Waiting for you"
        hint="Mail your agents want to send from workspaces that need your approval. Anything you don’t decide within 7 days is denied for you."
        card
        testId="hosting-pending"
        state={
          pendingError ? (
            <SettingsLoadState kind="error" title="Couldn’t load the messages waiting for you" detail={pendingError} action={<Button onClick={onRetry}>Try again</Button>}>
              Nothing was sent or denied. Try again in a moment.
            </SettingsLoadState>
          ) : pending === null ? (
            <SettingsLoadState kind="loading" title="Loading…" />
          ) : pending.length === 0 ? (
            <SettingsLoadState kind="empty" title="Nothing waiting">
              When a workspace needs your approval to send, its agents’ mail waits here. Choose that in the workspace’s email settings.
            </SettingsLoadState>
          ) : undefined
        }
      >
        {(pending ?? []).map((item) => (
          <ApprovalRow key={item.id} item={item} canMutate={canMutate} onDecided={onDecided} />
        ))}
      </SettingsGroup>

      <SettingsGroup
        title="Sent and decided"
        hint="What a workspace’s agents sent, and what you approved or denied."
        card
        testId="hosting-history"
        action={
          !sample ? (
            <SettingDropdown
              ariaLabel="Workspace"
              value={historyWs}
              onChange={setHistoryWs}
              placeholder="Pick a workspace"
              options={projects.map((p) => ({ value: p.id, label: p.name }))}
            />
          ) : undefined
        }
        state={
          historyError ? (
            <SettingsLoadState kind="error" title={`Couldn’t load what ${wsName} sent`} detail={historyError} action={<Button onClick={() => setHistoryLoad((n) => n + 1)}>Try again</Button>}>
              Nothing changed. Try again in a moment.
            </SettingsLoadState>
          ) : historyLoading && history === null ? (
            <SettingsLoadState kind="loading" title="Loading…" />
          ) : history === null ? (
            <SettingsLoadState kind="empty" title="Pick a workspace">
              Choose a workspace above to see the mail its agents sent.
            </SettingsLoadState>
          ) : history.length === 0 ? (
            <SettingsLoadState kind="empty" title="Nothing sent yet">
              {wsName}’s agents haven’t sent any mail.
            </SettingsLoadState>
          ) : undefined
        }
      >
        {(history ?? []).map((o) => {
          const tag = OUTBOUND_TAG[o.status] ?? { label: o.status, tone: 'neutral' as HealthTone }
          return (
            <div key={o.id} className="px-4 py-3 flex items-start justify-between gap-6" data-testid="hosting-history-row">
              <div className="min-w-0">
                <p className="type-name text-[var(--color-text-primary)] truncate">{o.subject || 'No subject'}</p>
                <p className={`${SETTING_ROW_HINT} break-words`}>
                  From {o.from} to {o.to.join(', ')}
                </p>
                {o.note ? <p className="type-hint text-[var(--color-text-secondary)] mt-0.5">Your reason: {o.note}</p> : null}
              </div>
              <div className="flex flex-col items-end gap-1 flex-shrink-0">
                <StatusWord tone={tag.tone}>{tag.label}</StatusWord>
                <span className="type-hint text-[var(--color-text-muted)] whitespace-nowrap">{fmtWhen(o.decidedAt ?? o.createdAt)}</span>
              </div>
            </div>
          )
        })}
      </SettingsGroup>
    </div>
  )
}

// ── The section root ─────────────────────────────────────────────────────

export function EmailHostingSection(): React.JSX.Element {
  const viewerReadOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')

  const [status, setStatus] = useState<MailStatus | null>(null)
  const [statusError, setStatusError] = useState<string | null>(null)
  const [domains, setDomains] = useState<DomainSummary[] | null>(null)
  const [domainsError, setDomainsError] = useState<string | null>(null)
  const [approvals, setApprovals] = useState<ApprovalItem[] | null>(null)
  const [approvalsError, setApprovalsError] = useState<string | null>(null)
  const [tab, setTab] = useState<Tab>('server')
  const [openDomain, setOpenDomain] = useState<string | null>(null)
  const [justAdded, setJustAdded] = useState<string | null>(null)
  const [adding, setAdding] = useState(false)
  const [notice, setNotice] = useState<Notice | null>(null)
  // Bumped by mail:* events + local mutations → every loader refetches.
  const [revision, setRevision] = useState(0)
  const bump = useCallback(() => setRevision((r) => r + 1), [])

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
        if (!isMailStatus(s)) {
          setStatusError('the reply didn’t say whether this server can host mail')
          return
        }
        setStatus(s)
        setStatusError(null)
      })
      .catch((e) => {
        if (!cancelled) setStatusError(reasonOf(e))
      })
    return () => {
      cancelled = true
    }
  }, [revision])

  // ── Domains + the approvals queue. Sample mode reads fixtures and
  //    stays network-silent. ────────────────────────────────────────
  useEffect(() => {
    if (supported === null) return
    if (!supported) {
      setDomains(SAMPLE_DOMAINS)
      setApprovals(SAMPLE_APPROVALS)
      setApprovalsError(null)
      setDomainsError(null)
      return
    }
    let cancelled = false
    fetchDomains()
      .then((d) => {
        if (cancelled) return
        setDomains(d)
        setDomainsError(null)
      })
      .catch((e) => {
        if (!cancelled) setDomainsError(reasonOf(e))
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
        // the queue says it couldn't load, the rest of the page works.
        setApprovals(null)
        setApprovalsError(reasonOf(e))
      })
    return () => {
      cancelled = true
    }
  }, [supported, revision])

  // ── Live updates: the daemon's `mail_changed` broadcast on the window's
  //    server's app bus (Home 0.43.2, Q7). Refetch signal only. ──────
  useEffect(() => onWindowServerMailChanged(() => setRevision((r) => r + 1)), [])

  // ── Install-progress poll: only while an install is in flight. ─────
  useEffect(() => {
    if (!installing) return
    const t = window.setInterval(() => {
      fetchMailStatus()
        .then((s) => {
          if (isMailStatus(s)) setStatus(s)
        })
        .catch(() => {
          /* transient — next tick retries */
        })
    }, 2000)
    return () => window.clearInterval(t)
  }, [installing])

  // A removed domain falls back to the list.
  useEffect(() => {
    if (openDomain === null || domains === null || justAdded === openDomain) return
    if (!domains.some((d) => d.domain === openDomain)) setOpenDomain(null)
  }, [domains, openDomain, justAdded])

  const onDecided = useCallback(
    (id: string, text: string, failed?: string): void => {
      setApprovals((prev) => prev?.filter((a) => a.id !== id) ?? null)
      setNotice(failed ? { tone: 'error', text, detail: failed } : { tone: 'ok', text })
      bump()
    },
    [bump],
  )

  const header = (
    <header className="min-w-0">
      <h2 className="type-title text-[var(--color-text-primary)]">Email hosting</h2>
      <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed max-w-[75ch]">
        Run your own mail server so agents get addresses on your domains. They read their mail here, and send only the way you allow.
      </p>
    </header>
  )

  const frame = (body: React.ReactNode): React.JSX.Element => (
    <div className="h-full min-h-0 overflow-y-auto" data-testid="email-hosting">
      <div className="p-6">
        <div className="w-full max-w-[52rem] space-y-5 pb-6">{body}</div>
      </div>
    </div>
  )

  if (statusError) {
    return frame(
      <>
        {header}
        <SettingsLoadState kind="error" title="Couldn’t read the email server’s status" detail={statusError} action={<Button onClick={bump}>Try again</Button>} testId="hosting-error">
          Your mail is unaffected; this page just couldn’t reach the server. Try again in a moment.
        </SettingsLoadState>
      </>,
    )
  }
  if (status === null) {
    return frame(
      <>
        {header}
        <SettingsLoadState kind="loading" title="Checking the email server…" testId="hosting-loading">
          Reading its status, domains and addresses.
        </SettingsLoadState>
      </>,
    )
  }

  const pendingCount = approvals?.length ?? 0
  const pickTab = (t: Tab): void => {
    setTab(t)
    if (t !== 'domains') setOpenDomain(null)
  }

  return frame(
    <>
      {header}
      {sample ? <LinuxOnlyNote what="Email hosting" /> : null}
      {notice ? <NoticeBar notice={notice} onDismiss={() => setNotice(null)} /> : null}

      <div role="tablist" aria-label="Email hosting" className="flex flex-wrap gap-0.5 border-b border-[var(--color-border)] w-full">
        {TABS.map((t) => {
          const active = tab === t
          return (
            <button
              key={t}
              type="button"
              role="tab"
              aria-selected={active}
              onClick={() => pickTab(t)}
              data-settings-id={TAB_SETTINGS_ID[t]}
              data-k2-focus=""
              className={`relative px-3 py-2 type-control transition-colors no-drag cursor-pointer border-b-2 -mb-px outline-none inline-flex items-center gap-1.5 ${
                active ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]' : 'border-transparent text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
              }`}
            >
              {TAB_LABEL[t]}
              {t === 'approvals' && pendingCount > 0 ? (
                <Tag tone="warn" aria-label={`${pendingCount} waiting`}>
                  {pendingCount}
                </Tag>
              ) : null}
            </button>
          )
        })}
      </div>

      <div className="pt-1">
        {tab === 'server' ? <ServerTab status={status} sample={sample} canMutate={canMutate} onChanged={bump} setNotice={setNotice} /> : null}
        {tab === 'domains' && openDomain === null ? (
          <DomainList
            domains={domains}
            error={domainsError}
            canMutate={canMutate}
            onOpen={(d) => {
              setJustAdded(null)
              setOpenDomain(d)
            }}
            onAdd={() => setAdding(true)}
            onRetry={bump}
          />
        ) : null}
        {tab === 'domains' && openDomain !== null ? (
          <DomainDetailView
            domain={openDomain}
            sample={sample}
            canMutate={canMutate}
            revision={revision}
            justAdded={justAdded === openDomain}
            onBack={() => {
              setOpenDomain(null)
              setJustAdded(null)
            }}
            onChanged={bump}
            onRemoved={(d, retired, purged) => {
              setOpenDomain(null)
              setJustAdded(null)
              setDomains((prev) => prev?.filter((x) => x.domain !== d) ?? null)
              setNotice({
                tone: 'ok',
                text: `Removed ${d}. ${retired} address${retired === 1 ? '' : 'es'} retired${purged ? ' and its mail deleted' : '; its mail is kept'}.`,
              })
              bump()
            }}
          />
        ) : null}
        {tab === 'addresses' ? <AddressesTab sample={sample} canMutate={canMutate} revision={revision} onChanged={bump} setNotice={setNotice} /> : null}
        {tab === 'approvals' ? (
          <ApprovalsTab
            pending={approvals}
            pendingError={approvalsError}
            sample={sample}
            canMutate={canMutate}
            revision={revision}
            onDecided={onDecided}
            onRetry={bump}
          />
        ) : null}
      </div>

      {adding ? (
        <AddDomainDialog
          onClose={() => setAdding(false)}
          onAdded={(d) => {
            setAdding(false)
            setNotice(null)
            setJustAdded(d.domain)
            setOpenDomain(d.domain)
            setTab('domains')
            bump()
          }}
          onExists={(dom) => {
            setAdding(false)
            setJustAdded(null)
            setNotice({ tone: 'ok', text: `${dom} is already on this server. Here are its records.` })
            setOpenDomain(dom)
            setTab('domains')
          }}
        />
      ) : null}
    </>,
  )
}
