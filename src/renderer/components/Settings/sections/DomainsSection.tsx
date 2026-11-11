// Settings → Network → Custom Domains (prd-custom-domains-and-certs-v1 D4/D11).
// Thin client on GET /cli/domains and GET /cli/firewall; every answer is
// checked in `domains-parse.ts`, so a bad answer is a failed read, never a
// crash. Adding a domain attaches it to this server; it never moves the
// domain's nameservers by itself.
//
// The page follows Settings → Workspaces / Agents: a title with one line
// under it, then card groups of rows at one content width. A person comes
// here to point their own domain at this server and see whether it works:
// Your domains (add, status, nameservers to set), Names (what each address
// is for, its certificate, the DNS records an App name still needs),
// Certificates, then Firewall.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { getDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import type { SettingEntry } from '../searchManifest'
import {
  SaveError,
  SaveNote,
  SettingDropdown,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
  useSaveStatus,
  SETTING_LIST_ROW_HOVER,
} from '../controls/SettingControls'
import { Button, Callout, Input, OverflowMenu, Textarea, Surface, Tag, Toggle, type TagTone } from '@/components/ui'
import { primaryScope } from '@/kessel/server-scope'
import { onDomainsChanged } from '@/stores/session-events'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import {
  DomainsParseError,
  parseDomainAnswer,
  parseDomainList,
  parseFirewallStatus,
  type AppNameStatus,
  type DomainNameRow,
  type DomainRow,
  type FirewallPort,
  type FirewallStatus,
} from './domains-parse'

export type { AppNameStatus, DomainRow, FirewallPort, FirewallStatus } from './domains-parse'

const CONTENT_WIDTH = 'max-w-[52rem]'

// ── Talking to the daemon ───────────────────────────────────────────

/** Daemon error with its machine code kept (`zone_owned_elsewhere`, …). */
class DomainApiError extends Error {
  code: string | null
  constructor(message: string, code: string | null) {
    super(message)
    this.code = code
  }
}

function toApiError(status: number, body: unknown): DomainApiError {
  const err = (body as { error?: { hint?: string; code?: string } | string } | null)?.error
  if (typeof err === 'string') return new DomainApiError(err, null)
  return new DomainApiError(err?.hint || `the server answered ${status}`, err?.code ?? null)
}

async function cliCall(path: string, payload?: unknown): Promise<unknown> {
  const creds = await getDaemonWs(primaryScope())
  const res = await fetch(`${daemonHttpBase(creds)}${path}?token=${creds.token}`, payload === undefined
    ? { method: 'GET' }
    : { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(payload) })
  const body = await res.json().catch(() => ({}))
  if (!res.ok) throw toApiError(res.status, body)
  return body
}

/** Copy for a 409 `zone_owned_elsewhere` attach. */
export const OWNED_ELSEWHERE_COPY = 'This domain belongs to another k2.dev account.'

/** The server's reason for a failed call, in its own words (shown quietly). */
export function domainErrorText(e: unknown): string {
  if (e instanceof DomainApiError && e.code === 'zone_owned_elsewhere') return OWNED_ELSEWHERE_COPY
  return e instanceof Error ? e.message : String(e)
}

/** What failed, in plain words, and the server's reason after it. */
type ActionFailure = { title: string; detail: string }

function failure(title: string, e: unknown): ActionFailure {
  return { title, detail: domainErrorText(e) }
}

function ActionError({ error, onDismiss }: { error: ActionFailure | null; onDismiss(): void }): React.JSX.Element | null {
  if (!error) return null
  const plain = error.detail === OWNED_ELSEWHERE_COPY
  return (
    <div className="px-4 py-3">
      <Callout tone="error" role="alert" className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <p className="type-row">{error.title}</p>
          <p className="type-hint text-[var(--color-text-secondary)] mt-0.5 break-words">
            {plain ? error.detail : `The server said: ${error.detail.replace(/[.\s]+$/, '')}.`} Nothing changed; try again.
          </p>
        </div>
        <Button variant="quiet" className="flex-shrink-0" onClick={onDismiss}>
          Dismiss
        </Button>
      </Callout>
    </div>
  )
}

function CopyButton({ value, label }: { value: string; label: string }): React.JSX.Element {
  const [copied, setCopied] = useState(false)
  useEffect(() => {
    if (!copied) return
    const t = window.setTimeout(() => setCopied(false), 2000)
    return () => window.clearTimeout(t)
  }, [copied])
  return (
    <Button
      variant="quiet"
      className="flex-shrink-0"
      aria-label={`Copy ${label}`}
      onClick={() => {
        void navigator.clipboard.writeText(value)
        setCopied(true)
      }}
    >
      {copied ? 'Copied' : 'Copy'}
    </Button>
  )
}

const DAY = new Intl.DateTimeFormat('en-GB', { day: 'numeric', month: 'short', year: 'numeric' })
const day = (unix: number): string => DAY.format(new Date(unix * 1000))

// ── Words ───────────────────────────────────────────────────────────

export function isPendingNs(d: Pick<DomainRow, 'status'>): boolean {
  return d.status === 'pending_ns'
}

/** Copy for a zone missing from k2.dev's list for this server (DN12d). */
export const ZONE_MISSING_COPY =
  'k2.dev no longer lists this domain for this server. It may have been removed or moved to another account. Check again, or remove it and add it again.'

/** A domain's status chip and the one line under its name. */
export function domainStatus(d: DomainRow): { tone: TagTone; label: string; hint: string } {
  if (d.zoneMissing) return { tone: 'danger', label: 'Not found', hint: ZONE_MISSING_COPY }
  if (isPendingNs(d)) {
    return { tone: 'warn', label: 'Waiting for nameservers', hint: 'Set its nameservers at your registrar. K2 checks by itself and updates this page.' }
  }
  if (d.readOnly) {
    return {
      tone: 'warn',
      label: 'On hold',
      hint: `Its records still work, but your k2.dev plan is ${d.status ?? 'on hold'}, so they can’t change. Check your plan on k2.dev.`,
    }
  }
  if (d.dnsWrite) return { tone: 'ok', label: 'Working', hint: 'K2 hosts its DNS, so K2 adds the records each name needs.' }
  return { tone: 'neutral', label: 'Your DNS', hint: 'Its DNS stays where you manage it. Add the records each name needs there.' }
}

const ROLE_OPTIONS = [
  { value: 'mail', label: 'Mail' },
  { value: 'publish', label: 'Published pages' },
  { value: 'app', label: 'An App' },
  { value: 'direct', label: 'Direct to this server' },
  { value: 'other', label: 'Something else' },
]

function roleWords(role: string): string {
  if (role.startsWith('app:')) return `For the App ${role.slice(4)}`
  if (role === 'mail') return 'For mail'
  if (role === 'publish') return 'For published pages'
  if (role === 'direct') return 'Direct to this server'
  return 'Other use'
}

/** An App name's chip (never "ready" for a name nothing serves). */
export function appStatus(a: AppNameStatus): { tone: TagTone; label: string } {
  if (a.error === 'unknown_app') return { tone: 'danger', label: 'No such App' }
  if (a.external) return { tone: 'neutral', label: 'Your web server' }
  if (a.live && !a.notServedYet) return { tone: 'ok', label: 'Live' }
  if (a.state === 'verifying') return { tone: 'warn', label: 'Checking DNS' }
  if (a.state === 'issuing') return { tone: 'warn', label: 'Getting a certificate' }
  if (a.state === 'failing') return { tone: 'danger', label: 'Not working' }
  return { tone: 'warn', label: 'Waiting for DNS' }
}

/** A name's chip and the line under it. */
export function nameStatus(n: DomainNameRow): { tone: TagTone; label: string; hint: string } {
  const role = roleWords(n.role)
  if (n.app) {
    const s = appStatus(n.app)
    if (n.app.error === 'unknown_app') return { ...s, hint: `There’s no App called ${n.app.app} on this server. Remove this name, or add it again for another App.` }
    if (n.app.external) return { ...s, hint: `${role}. Your own web server answers for it, not K2.` }
    const extra = n.app.viaRelay ? ' Reaches this server through K2 Connect.' : ''
    const main = n.app.primary ? ' Its main address.' : ''
    return { ...s, hint: `${role}.${main}${extra}` }
  }
  const c = n.cert
  const uploaded = n.renewal?.state === 'uploaded'
  if (c.state === 'expired') {
    return { tone: 'danger', label: 'Certificate expired', hint: `${role}. Its certificate expired${c.expiresAt ? ` on ${day(c.expiresAt)}` : ''}, so browsers warn visitors.` }
  }
  if (c.state === 'self-signed') {
    return { tone: 'warn', label: 'Temporary certificate', hint: `${role}. Browsers warn visitors until K2 gets a real certificate.` }
  }
  if (c.state === 'issued') {
    const until = c.expiresAt ? ` until ${day(c.expiresAt)}` : ''
    const how = uploaded ? 'Your own certificate' : 'Certificate'
    const renew = uploaded ? '' : ', renews by itself'
    return { tone: 'ok', label: 'Working', hint: `${role}. ${how}${until}${renew}.` }
  }
  return { tone: 'muted', label: 'No certificate', hint: `${role}. No certificate yet: get one from its menu.` }
}

/** Plain words for a firewall row: who opened it and what it's for. */
export function firewallPortWords(r: FirewallPort): { who: 'K2' | 'You'; what: string } {
  const only = r.allowFrom.length > 0 ? `, only from ${r.allowFrom.join(', ')}` : ''
  switch (r.source) {
    case 'ssh':
      return { who: 'You', what: `SSH, to sign in to this server${only}` }
    case 'front443':
      return { who: 'K2', what: 'Secure web: Apps on your domains, mail on the web and calls' }
    case 'front80':
      return { who: 'K2', what: 'Web: sends visitors to the secure address and proves your domains for certificates' }
    case 'skin-direct':
      return { who: 'K2', what: 'Apps served straight from this server' }
    case 'hostmail': {
      const mail: Record<number, string> = {
        25: 'Mail: receives mail from other servers',
        465: 'Mail: mail apps send through it',
        587: 'Mail: mail apps send through it',
        993: 'Mail: mail apps read through it',
        143: 'Mail: mail apps read through it',
        80: 'Mail: proves the domain for its certificate',
        443: 'Mail on the web',
      }
      return { who: 'K2', what: mail[r.port] ?? 'Mail' }
    }
    case 'sew-voice':
      return { who: 'K2', what: 'Chat / Voice: calls' }
    case 'daemon':
      return { who: 'K2', what: 'K2 on your local network' }
    case 'owner':
    {
      const label = r.why.replace(/^owner:\s*/, '')
      return { who: 'You', what: `${label ? label[0].toUpperCase() + label.slice(1) : 'Opened by you'}${only}` }
    }
    default:
      return { who: 'K2', what: r.why || r.source }
  }
}

export const DOMAINS_MANIFEST: SettingEntry[] = [
  {
    id: 'domains.inventory',
    section: 'domains',
    label: 'Your domains',
    description: 'Domains you own, pointed at this server, and whether each one works',
    keywords: ['domain', 'apex', 'hostname', 'dns', 'nameservers', 'cert', 'tls', 'bind'],
  },
  {
    id: 'domains.add',
    section: 'domains',
    label: 'Add domain',
    description: 'Point a domain you own at this server',
    keywords: ['add', 'attach', 'apex', 'bind', 'domain'],
  },
  {
    id: 'domains.names',
    section: 'domains',
    label: 'Names',
    description: 'Addresses on your domains and what each is for: mail, pages or an App',
    keywords: ['hostname', 'subdomain', 'mail', 'app', 'records', 'certificate'],
  },
  {
    id: 'domains.certificates',
    section: 'domains',
    label: 'Certificates',
    description: 'The certificate service K2 uses, or upload your own certificate',
    keywords: ['certificate', 'acme', 'lets encrypt', 'zerossl', 'pem', 'upload', 'tls'],
  },
  {
    id: 'domains.firewall',
    section: 'domains',
    label: 'Firewall',
    description: 'Ports open to the internet on this server, who can reach SSH, ports you open',
    keywords: ['firewall', 'ports', 'ssh', 'open port', 'guard', 'rate limit', 'nft', 'ufw'],
  },
]

// ── The page ────────────────────────────────────────────────────────

type Load<T> = { kind: 'loading' } | { kind: 'error'; detail: string; unreadable: boolean } | { kind: 'ready'; data: T }

/** A failed read: the server's reason, or an answer K2 can't read. */
function readFailure(e: unknown): { kind: 'error'; detail: string; unreadable: boolean } {
  return { kind: 'error', detail: domainErrorText(e), unreadable: e instanceof DomainsParseError }
}

export function DomainsSection(): React.JSX.Element {
  const [state, setState] = useState<Load<DomainRow[]>>({ kind: 'loading' })
  /** The domain just added: its row says what to do next. */
  const [justAdded, setJustAdded] = useState<string | null>(null)

  const refresh = useCallback(async (): Promise<void> => {
    try {
      setState({ kind: 'ready', data: parseDomainList(await cliCall('/cli/domains')) })
    } catch (e) {
      setState(readFailure(e))
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  // DN12e: the daemon re-checks pending zones by itself (and other clients
  // add or remove); repaint when it says the list changed.
  useEffect(() => onDomainsChanged(primaryScope(), () => void refresh()), [refresh])

  const domains = state.kind === 'ready' ? state.data : []
  return (
    <div className="h-full min-h-0 overflow-y-auto p-6 [scrollbar-gutter:stable]" data-testid="domains-page">
      <div className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`}>
        <header className="min-w-0">
          <h2 className="type-title text-[var(--color-text-primary)]">Custom Domains</h2>
          <p className={SETTING_ROW_HINT}>
            Use domains you own for this server’s Apps, mail and pages. Add a domain, point it here, and K2 gets and
            renews the certificates.
          </p>
        </header>
        <DomainsGroup
          state={state}
          justAdded={justAdded}
          onAdded={(apex) => {
            setJustAdded(apex)
            void refresh()
          }}
          refresh={refresh}
        />
        {state.kind === 'ready' && domains.length > 0 && <NamesGroup domains={domains} refresh={refresh} />}
        {state.kind === 'ready' && domains.length > 0 && <CertificatesGroup domains={domains} refresh={refresh} />}
        <FirewallPanel />
      </div>
    </div>
  )
}

// ── Your domains ────────────────────────────────────────────────────

function DomainsGroup({
  state,
  justAdded,
  onAdded,
  refresh,
}: {
  state: Load<DomainRow[]>
  justAdded: string | null
  onAdded(apex: string): void
  refresh(): Promise<void>
}): React.JSX.Element {
  const [adding, setAdding] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<ActionFailure | null>(null)
  const addRef = useRef<HTMLButtonElement>(null)

  const run = async (title: string, fn: () => Promise<unknown>): Promise<boolean> => {
    if (busy) return false
    setBusy(true)
    setError(null)
    try {
      await fn()
      await refresh()
      return true
    } catch (e) {
      setError(failure(title, e))
      return false
    } finally {
      setBusy(false)
    }
  }

  const checkAgain = (apex: string): void =>
    void run(`K2 couldn’t check ${apex} with k2.dev.`, async () => {
      const body = (await cliCall('/cli/domains/refresh', { apex })) as { checked?: boolean; hint?: string } | null
      if (body?.checked === false) throw new DomainApiError(body.hint || 'k2.dev didn’t answer', null)
    })

  const remove = async (d: DomainRow): Promise<void> => {
    const yes = await useConfirmDialogStore.getState().confirm({
      title: `Remove ${d.apex}?`,
      message: `This server stops answering for ${d.apex}${d.names.length > 0 ? ` and its ${d.names.length === 1 ? 'name' : `${d.names.length} names`}` : ''}. Its nameservers at your registrar stay as they are.`,
      confirmLabel: 'Remove',
      destructive: true,
    })
    if (yes) void run(`K2 couldn’t remove ${d.apex}.`, () => cliCall('/cli/domains/remove', { apex: d.apex }))
  }

  const domains = state.kind === 'ready' ? state.data : []
  const canAdd = state.kind === 'ready' && !adding
  const openForm = (): void => {
    setError(null)
    setAdding(true)
  }

  let body: React.ReactNode
  if (state.kind === 'loading') {
    body = null
  } else if (state.kind === 'error') {
    body = null
  } else if (domains.length === 0 && !adding) {
    body = null
  } else {
    body = (
      <>
        {adding && (
          <AddDomainForm
            onCancel={() => {
              setAdding(false)
              requestAnimationFrame(() => addRef.current?.focus())
            }}
            onAdded={(apex) => {
              setAdding(false)
              onAdded(apex)
            }}
            onError={setError}
          />
        )}
        {domains.map((d) => (
          <DomainItem
            key={d.apex}
            domain={d}
            busy={busy}
            justAdded={justAdded === d.apex}
            onCheckAgain={() => checkAgain(d.apex)}
            onRemove={() => void remove(d)}
          />
        ))}
      </>
    )
  }

  const group = (
    <SettingsGroup
      title="Your domains"
      hint="Each domain you own that points at this server. Adding one doesn’t change anything at your registrar."
      settingId="domains.inventory"
      card
      action={
        canAdd && domains.length > 0 ? (
          <Button ref={addRef} variant="accent" data-settings-id="domains.add" onClick={openForm}>
            Add domain
          </Button>
        ) : undefined
      }
    >
      <ActionError error={error} onDismiss={() => setError(null)} />
      {body}
    </SettingsGroup>
  )

  if (state.kind === 'loading') {
    return (
      <section data-settings-id="domains.inventory" className="space-y-2">
        <GroupHeading title="Your domains" />
        <SettingsLoadState kind="loading" title="Loading your domains…" testId="domains-loading" />
      </section>
    )
  }
  if (state.kind === 'error') {
    return (
      <section data-settings-id="domains.inventory" className="space-y-2">
        <GroupHeading title="Your domains" />
        <SettingsLoadState
          kind="error"
          title="K2 couldn’t read your domains from this server"
          detail={state.unreadable ? null : state.detail}
          testId="domains-load-error"
          action={<Button variant="quiet" onClick={() => void refresh()}>Try again</Button>}
        >
          {state.unreadable
            ? 'Its answer wasn’t a list of domains K2 can read. Your domains still work. Try again, or update K2 on the server.'
            : 'Your domains still work. Try again in a moment.'}
        </SettingsLoadState>
      </section>
    )
  }
  if (domains.length === 0 && !adding) {
    return (
      <section data-settings-id="domains.inventory" className="space-y-2">
        <GroupHeading title="Your domains" />
        <SettingsLoadState
          kind="empty"
          title="No domains yet"
          testId="domains-empty"
          action={
            <Button ref={addRef} variant="accent" data-settings-id="domains.add" onClick={openForm}>
              Add domain
            </Button>
          }
        >
          Add a domain you own, like example.com, to use it for this server’s Apps, mail and pages.
        </SettingsLoadState>
      </section>
    )
  }
  return group
}

function GroupHeading({ title }: { title: string }): React.JSX.Element {
  return <h3 className="type-heading text-[var(--color-text-primary)] min-h-[1.5rem] flex items-center">{title}</h3>
}

function AddDomainForm({
  onCancel,
  onAdded,
  onError,
}: {
  onCancel(): void
  onAdded(apex: string): void
  onError(e: ActionFailure | null): void
}): React.JSX.Element {
  const [apex, setApex] = useState('')
  const [saving, setSaving] = useState(false)
  const ref = useRef<HTMLInputElement>(null)
  useEffect(() => {
    requestAnimationFrame(() => ref.current?.focus())
  }, [])
  const value = apex.trim().toLowerCase()
  const looksLikeOne = /^[a-z0-9-]+(\.[a-z0-9-]+)+$/.test(value)

  const save = async (): Promise<void> => {
    if (!looksLikeOne || saving) return
    setSaving(true)
    onError(null)
    try {
      const body = await cliCall('/cli/domains', { apex: value })
      onAdded(parseDomainAnswer(body)?.apex ?? value)
    } catch (e) {
      onError(failure(`K2 couldn’t add ${value}.`, e))
      setSaving(false)
    }
  }

  return (
    <div
      className="bg-[var(--color-wash-1)] divide-y divide-[var(--color-border)]"
      role="group"
      aria-label="New domain"
      data-testid="add-domain-form"
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.preventDefault()
          e.stopPropagation()
          onCancel()
        } else if (e.key === 'Enter' && (e.target as HTMLElement).tagName === 'INPUT') {
          e.preventDefault()
          void save()
        }
      }}
    >
      <SettingRow label="Domain name" hint="The name you bought, like example.com. Add names under it, like mail.example.com, once it’s here." field>
        <Input
          ref={ref}
          aria-label="Domain name"
          value={apex}
          placeholder="example.com"
          spellCheck={false}
          autoCapitalize="off"
          onChange={(e) => setApex(e.target.value)}
          className="w-full type-control"
        />
      </SettingRow>
      <div className="px-4 py-3 flex items-center justify-end gap-2">
        {!looksLikeOne && (
          <span className="type-hint text-[var(--color-text-secondary)] mr-auto">
            {value === '' ? 'Enter a domain name to add it.' : 'Enter a full domain name, like example.com.'}
          </span>
        )}
        <Button variant="quiet" onClick={onCancel}>
          Cancel
        </Button>
        <Button variant="accent" disabled={!looksLikeOne || saving} onClick={() => void save()}>
          {saving ? 'Adding…' : 'Add domain'}
        </Button>
      </div>
    </div>
  )
}

function DomainItem({
  domain: d,
  busy,
  justAdded,
  onCheckAgain,
  onRemove,
}: {
  domain: DomainRow
  busy: boolean
  justAdded: boolean
  onCheckAgain(): void
  onRemove(): void
}): React.JSX.Element {
  const s = domainStatus(d)
  const pending = isPendingNs(d)
  const onK2 = d.zoneId !== null
  return (
    <div className={`px-4 py-3 ${SETTING_LIST_ROW_HOVER}`} data-testid={`domain-${d.apex}`}>
      <div className="flex items-start justify-between gap-6">
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 min-w-0">
            <span className="type-name text-[var(--color-text-primary)] truncate">{d.apex}</span>
            <Tag tone={s.tone} data-testid={`domain-status-${d.apex}`}>{s.label}</Tag>
            {justAdded && <SaveNoteText text="Added" />}
          </div>
          <p className={SETTING_ROW_HINT} data-testid={d.zoneMissing ? `zone-missing-${d.apex}` : undefined}>
            {s.hint}
          </p>
          {d.checkError && <p className={SETTING_ROW_HINT}>The last check didn’t finish. The server said: {d.checkError.replace(/[.\s]+$/, '')}.</p>}
        </div>
        <div className="flex items-center gap-2 flex-shrink-0">
          {(d.zoneMissing || pending) && (
            <Button variant="quiet" disabled={busy} onClick={onCheckAgain}>
              Check again
            </Button>
          )}
          <OverflowMenu
            label={`More actions for ${d.apex}`}
            items={[
              ...(onK2 && !d.zoneMissing && !pending
                ? [{ id: 'check', label: 'Check with k2.dev', onSelect: onCheckAgain, disabled: busy }]
                : []),
              { id: 'remove', label: 'Remove…', onSelect: onRemove, danger: true, group: onK2 && !d.zoneMissing && !pending, disabled: busy },
            ]}
          />
        </div>
      </div>
      {pending && <PendingNameservers domain={d} />}
    </div>
  )
}

/** "Added" with a check, beside a new domain's name. */
function SaveNoteText({ text }: { text: string }): React.JSX.Element {
  return (
    <span role="status" className="inline-flex items-center gap-1 type-hint text-[var(--color-status-ok-soft)] whitespace-nowrap">
      <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3">
        <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
      </svg>
      {text}
    </span>
  )
}

/** The nameservers to set for a domain still waiting for them (A8.1). */
export function PendingNameservers({ domain }: { domain: DomainRow }): React.JSX.Element {
  const ns = domain.nameservers
  return (
    <Surface chamfer material="tile" className="mt-3 p-3 space-y-2" data-testid={`pending-ns-${domain.apex}`}>
      <p className={SETTING_ROW_HINT}>
        Next: where you bought {domain.apex}, replace its nameservers with {ns.length === 1 ? 'this one' : `these ${ns.length === 2 ? 'two' : ns.length}`}.
        Changes can take a few hours to reach everyone.
      </p>
      {ns.length === 0 ? (
        <p className={SETTING_ROW_HINT}>k2.dev didn’t list the nameservers. They’re on your k2.dev dashboard.</p>
      ) : (
        <ul className="space-y-1">
          {ns.map((n) => (
            <li key={n} className="flex items-center justify-between gap-4">
              <span className="type-row font-mono text-[var(--color-text-primary)] select-all">{n}</span>
              <CopyButton value={n} label={n} />
            </li>
          ))}
        </ul>
      )}
      {domain.created && <p className={SETTING_ROW_HINT}>K2 also added {domain.apex} to your k2.dev account.</p>}
    </Surface>
  )
}

// ── Names ───────────────────────────────────────────────────────────

function NamesGroup({ domains, refresh }: { domains: DomainRow[]; refresh(): Promise<void> }): React.JSX.Element {
  const [adding, setAdding] = useState(false)
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<ActionFailure | null>(null)
  const [done, setDone] = useState<{ host: string; text: string } | null>(null)
  const addRef = useRef<HTMLButtonElement>(null)
  const names = domains.flatMap((d) => d.names)

  useEffect(() => {
    if (!done) return
    const t = window.setTimeout(() => setDone(null), 2500)
    return () => window.clearTimeout(t)
  }, [done])

  const run = async (host: string, title: string, ok: string, fn: () => Promise<unknown>): Promise<void> => {
    if (busy) return
    setBusy(host)
    setError(null)
    try {
      await fn()
      await refresh()
      setDone({ host, text: ok })
    } catch (e) {
      setError(failure(title, e))
    } finally {
      setBusy(null)
    }
  }

  const certificate = (n: DomainNameRow): void => {
    const renew = n.cert.state === 'issued' || n.cert.state === 'expired'
    void run(
      n.hostname,
      `K2 couldn’t get a certificate for ${n.hostname}.`,
      renew ? 'Renewed' : 'Certificate ready',
      () => cliCall(renew ? '/cli/certs/renew' : '/cli/certs/issue', { hostname: n.hostname }),
    )
  }
  const checkNow = (n: DomainNameRow): void =>
    void run(n.hostname, `K2 couldn’t check ${n.hostname}.`, 'Checked', () => cliCall('/cli/app/domain/check', { app: n.app?.app, host: n.hostname }))
  const remove = async (n: DomainNameRow): Promise<void> => {
    const yes = await useConfirmDialogStore.getState().confirm({
      title: `Remove ${n.hostname}?`,
      message: `This server stops answering for ${n.hostname}. Its DNS records stay where you manage them.`,
      confirmLabel: 'Remove',
      destructive: true,
    })
    if (yes) void run(n.hostname, `K2 couldn’t remove ${n.hostname}.`, 'Removed', () => cliCall('/cli/domains/names/remove', { hostname: n.hostname }))
  }

  return (
    <SettingsGroup
      title="Names"
      hint="The addresses on your domains, like mail.example.com, and what each one is for."
      settingId="domains.names"
      card
      action={
        !adding ? (
          <Button
            ref={addRef}
            variant="quiet"
            onClick={() => {
              setError(null)
              setAdding(true)
            }}
          >
            Add name
          </Button>
        ) : undefined
      }
    >
      <ActionError error={error} onDismiss={() => setError(null)} />
      {adding && (
        <AddNameForm
          domains={domains}
          onCancel={() => {
            setAdding(false)
            requestAnimationFrame(() => addRef.current?.focus())
          }}
          onAdded={(host) => {
            setAdding(false)
            setDone({ host, text: 'Added' })
            void refresh()
          }}
          onError={setError}
        />
      )}
      {names.length === 0 && !adding ? (
        <div className="px-4 py-3" data-testid="names-empty">
          <p className={SETTING_ROW_LABEL}>No names yet</p>
          <p className={SETTING_ROW_HINT}>Add a name, like mail.example.com or the domain itself, and say what it’s for.</p>
        </div>
      ) : (
        names.map((n) => (
          <NameItem
            key={n.hostname}
            name={n}
            busy={busy === n.hostname}
            done={done?.host === n.hostname ? done.text : null}
            onCertificate={() => certificate(n)}
            onCheckNow={() => checkNow(n)}
            onRemove={() => void remove(n)}
          />
        ))
      )}
    </SettingsGroup>
  )
}

function NameItem({
  name: n,
  busy,
  done,
  onCertificate,
  onCheckNow,
  onRemove,
}: {
  name: DomainNameRow
  busy: boolean
  done: string | null
  onCertificate(): void
  onCheckNow(): void
  onRemove(): void
}): React.JSX.Element {
  const s = nameStatus(n)
  const a = n.app
  const waiting = a !== null && a.notServedYet && !a.external && a.error !== 'unknown_app'
  const hasCert = n.cert.state === 'issued' || n.cert.state === 'expired'
  return (
    <div className={`px-4 py-3 ${SETTING_LIST_ROW_HOVER}`} data-testid={`domain-name-${n.hostname}`}>
      <div className="flex items-start justify-between gap-6">
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 min-w-0">
            <span className="type-name text-[var(--color-text-primary)] truncate">{n.hostname}</span>
            <Tag tone={s.tone} data-testid={`name-status-${n.hostname}`}>{s.label}</Tag>
            {done && <SaveNoteText text={done} />}
          </div>
          <p className={SETTING_ROW_HINT}>{s.hint}</p>
          {n.renewal?.lastError && n.cert.state !== 'issued' && (
            <p className={SETTING_ROW_HINT}>K2 is trying again. The server said: {n.renewal.lastError.replace(/[.\s]+$/, '')}.</p>
          )}
          {a?.warning && (
            <p className={`${SETTING_ROW_HINT} text-[var(--color-status-warn-amber-soft)]`}>
              It’s marked live, but this server isn’t the one answering for it. Its DNS may point somewhere else.
            </p>
          )}
        </div>
        <div className="flex items-center gap-2 flex-shrink-0">
          {n.cert.state === 'expired' && !a && (
            <Button variant="quiet" disabled={busy} onClick={onCertificate}>
              {busy ? 'Renewing…' : 'Renew'}
            </Button>
          )}
          <OverflowMenu
            label={`More actions for the name ${n.hostname}`}
            items={[
              ...(a && !a.error ? [{ id: 'check', label: 'Check now', onSelect: onCheckNow, disabled: busy }] : []),
              { id: 'cert', label: hasCert ? 'Renew certificate' : 'Get a certificate', onSelect: onCertificate, disabled: busy },
              { id: 'remove', label: 'Remove…', onSelect: onRemove, danger: true, group: true, disabled: busy },
            ]}
          />
        </div>
      </div>
      {waiting && a && <AppRecords host={n.hostname} app={a} busy={busy} onCheckNow={onCheckNow} />}
    </div>
  )
}

/** What an App name still needs: the DNS records to set, and Check now. */
function AppRecords({ host, app: a, busy, onCheckNow }: { host: string; app: AppNameStatus; busy: boolean; onCheckNow(): void }): React.JSX.Element {
  const records = a.records.length > 0 ? a.records : a.txt ? [a.txt] : []
  const failing = a.state === 'failing'
  return (
    <Surface chamfer material="tile" className="mt-3 p-3 space-y-2" data-testid={`app-records-${host}`}>
      <div className="flex items-start justify-between gap-4">
        <p className={SETTING_ROW_HINT}>
          {failing
            ? 'These records stopped pointing here. Check them where you manage this domain’s DNS.'
            : 'Next: add these records where you manage this domain’s DNS. K2 checks by itself, or check now.'}
          {a.recordsPath === 'connect' ? ' Visitors reach this server through K2 Connect.' : ''}
        </p>
        <Button variant="quiet" className="flex-shrink-0" disabled={busy} onClick={onCheckNow}>
          {busy ? 'Checking…' : 'Check now'}
        </Button>
      </div>
      {records.length > 0 && (
        <ul className="divide-y divide-[var(--color-border)]">
          {records.map((r) => (
            <li key={`${r.type} ${r.name}`} className="flex items-center gap-3 py-1.5" data-testid={`app-record-${r.type}`}>
              <span className="w-14 flex-shrink-0 type-hint font-mono text-[var(--color-text-secondary)]">{r.type}</span>
              <span className="min-w-0 flex-1 type-row font-mono text-[var(--color-text-primary)] truncate" title={r.name}>{r.name}</span>
              <span className="min-w-0 flex-1 type-row font-mono text-[var(--color-text-primary)] truncate select-all" title={r.value}>{r.value}</span>
              <CopyButton value={r.value} label={`the ${r.type} value for ${r.name}`} />
            </li>
          ))}
        </ul>
      )}
      {a.txtPending && <p className={SETTING_ROW_HINT} data-testid={`app-txt-pending-${host}`}>{a.txtPending}</p>}
      {a.pathB?.available && a.pathB.meter && <p className={SETTING_ROW_HINT} data-testid={`app-path-b-meter-${host}`}>{a.pathB.meter}</p>}
      {a.pathB && !a.pathB.available && a.pathB.why && (
        <p className={SETTING_ROW_HINT}>
          Through K2 Connect: {a.pathB.why}
          {a.pathB.upgrade && (
            <>
              {' '}
              <a href={a.pathB.upgrade} target="_blank" rel="noreferrer" data-testid={`app-path-b-upgrade-${host}`} data-k2-focus="" className="relative px-0.5 -mx-0.5 text-[var(--color-accent)] underline">
                Upgrade
              </a>
            </>
          )}
        </p>
      )}
      {a.lastError && <p className={SETTING_ROW_HINT}>Last check: {a.lastError.replace(/[.\s]+$/, '')}.</p>}
    </Surface>
  )
}

function AddNameForm({
  domains,
  onCancel,
  onAdded,
  onError,
}: {
  domains: DomainRow[]
  onCancel(): void
  onAdded(host: string): void
  onError(e: ActionFailure | null): void
}): React.JSX.Element {
  const [host, setHost] = useState('')
  const [role, setRole] = useState('mail')
  const [apps, setApps] = useState<string[] | null>(null)
  const [appsError, setAppsError] = useState<string | null>(null)
  const [appId, setAppId] = useState('')
  const [saving, setSaving] = useState(false)
  const ref = useRef<HTMLInputElement>(null)
  useEffect(() => {
    requestAnimationFrame(() => ref.current?.focus())
  }, [])

  // "An App" lists this server's Apps.
  useEffect(() => {
    if (role !== 'app' || apps !== null) return
    let live = true
    cliCall('/cli/app/list')
      .then((body) => {
        if (!live) return
        const list = (body as { apps?: unknown } | null)?.apps
        const ids = Array.isArray(list)
          ? list
              .map((a) => (a && typeof a === 'object' ? ((a as Record<string, unknown>).app ?? (a as Record<string, unknown>).appId ?? (a as Record<string, unknown>).id) : null))
              .filter((s): s is string => typeof s === 'string' && s !== '')
          : []
        setApps(ids)
        if (ids.length > 0) setAppId((cur) => cur || ids[0])
      })
      .catch((e: unknown) => live && setAppsError(domainErrorText(e)))
    return () => {
      live = false
    }
  }, [role, apps])

  const value = host.trim().toLowerCase()
  const under = domains.find((d) => value === d.apex || value.endsWith(`.${d.apex}`))
  const needApp = role === 'app' && !appId
  const valid = value !== '' && under !== undefined && !needApp

  const save = async (): Promise<void> => {
    if (!valid || saving) return
    setSaving(true)
    onError(null)
    try {
      await cliCall('/cli/domains/names', { hostname: value, role: role === 'app' ? `app:${appId}` : role })
      onAdded(value)
    } catch (e) {
      onError(failure(`K2 couldn’t add ${value}.`, e))
      setSaving(false)
    }
  }

  return (
    <div
      className="bg-[var(--color-wash-1)] divide-y divide-[var(--color-border)]"
      role="group"
      aria-label="New name"
      data-testid="add-name-form"
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.preventDefault()
          e.stopPropagation()
          onCancel()
        } else if (e.key === 'Enter' && (e.target as HTMLElement).tagName === 'INPUT') {
          e.preventDefault()
          void save()
        }
      }}
    >
      <SettingRow label="Name" hint={`An address on one of your domains, like mail.${domains[0]?.apex ?? 'example.com'}.`} field>
        <Input
          ref={ref}
          aria-label="Name"
          value={host}
          placeholder={`mail.${domains[0]?.apex ?? 'example.com'}`}
          spellCheck={false}
          autoCapitalize="off"
          onChange={(e) => setHost(e.target.value)}
          className="w-full type-control"
        />
      </SettingRow>
      <SettingRow label="Used for" hint="Tells K2 what to serve on it and which certificate it needs." field>
        <SettingDropdown fullWidth ariaLabel="Used for" value={role} options={ROLE_OPTIONS} onChange={setRole} />
      </SettingRow>
      {role === 'app' && (
        <SettingRow
          label="App"
          hint={appsError ? `K2 couldn’t list this server’s Apps. The server said: ${appsError.replace(/[.\s]+$/, '')}.` : 'The App this name opens.'}
          field
        >
          <SettingDropdown
            fullWidth
            ariaLabel="App"
            value={appId}
            options={(apps ?? []).map((a) => ({ value: a, label: a }))}
            placeholder={apps === null ? (appsError ? 'Apps didn’t load' : 'Loading…') : 'No Apps on this server'}
            disabled={!apps || apps.length === 0}
            onChange={setAppId}
          />
        </SettingRow>
      )}
      <div className="px-4 py-3 flex items-center justify-end gap-2">
        {!valid && (
          <span className="type-hint text-[var(--color-text-secondary)] mr-auto">
            {value === ''
              ? 'Enter a name to add it.'
              : !under
                ? `Use a name on one of your domains: ${domains.map((d) => d.apex).join(', ')}.`
                : 'Pick the App this name opens.'}
          </span>
        )}
        <Button variant="quiet" onClick={onCancel}>
          Cancel
        </Button>
        <Button variant="accent" disabled={!valid || saving} onClick={() => void save()}>
          {saving ? 'Adding…' : 'Add name'}
        </Button>
      </div>
    </div>
  )
}

// ── Certificates ────────────────────────────────────────────────────

function CertificatesGroup({ domains, refresh }: { domains: DomainRow[]; refresh(): Promise<void> }): React.JSX.Element {
  const [uploading, setUploading] = useState(false)
  const names = domains.flatMap((d) => d.names.map((n) => n.hostname))
  return (
    <SettingsGroup
      title="Certificates"
      hint="K2 gets free certificates from Let’s Encrypt and renews them before they expire. Change these only to use another service."
      settingId="domains.certificates"
      card
    >
      <CertConfigRow
        field="email"
        label="Contact email"
        hint="Where the certificate service sends notices. Saves when you press Return or leave the field."
        placeholder="you@example.com"
      />
      <CertConfigRow
        field="directory"
        label="Certificate service"
        hint="Another service’s address, like ZeroSSL’s. Leave it empty for Let’s Encrypt."
        placeholder="Let’s Encrypt"
      />
      <SettingRow
        label="Your own certificate"
        hint="Use a certificate you already have for one of your names. K2 stops renewing that name."
        below={uploading ? <UploadForm names={names} onDone={(ok) => { setUploading(false); if (ok) void refresh() }} /> : undefined}
      >
        {!uploading && (
          <Button variant="quiet" disabled={names.length === 0} title={names.length === 0 ? 'Add a name first' : undefined} onClick={() => setUploading(true)}>
            Upload…
          </Button>
        )}
      </SettingRow>
    </SettingsGroup>
  )
}

/** One certificate-service field, saved on Return or blur (`POST /cli/certs/config`
 *  with that field only, so the other keeps its value). */
function CertConfigRow({ field, label, hint, placeholder }: { field: 'email' | 'directory'; label: string; hint: string; placeholder: string }): React.JSX.Element {
  const [value, setValue] = useState('')
  const [savedValue, setSavedValue] = useState('')
  const save = useSaveStatus()
  const commit = async (): Promise<void> => {
    const v = value.trim()
    if (v === '' || v === savedValue) return
    save.reset()
    try {
      await cliCall('/cli/certs/config', { [field]: v })
      setSavedValue(v)
      save.saved()
    } catch (e) {
      save.failed(domainErrorText(e))
    }
  }
  return (
    <SettingRow
      label={<span className="inline-flex items-center gap-2">{label}<SaveNote state={save.state} /></span>}
      hint={hint}
      field
      below={typeof save.state === 'object' ? <SaveError state={save.state} /> : undefined}
    >
      <Input
        aria-label={label}
        type={field === 'email' ? 'email' : 'text'}
        value={value}
        placeholder={placeholder}
        spellCheck={false}
        onChange={(e) => setValue(e.target.value)}
        onBlur={() => void commit()}
        onKeyDown={(e) => {
          if (e.key === 'Enter') void commit()
        }}
        className="w-full type-control"
      />
    </SettingRow>
  )
}

function UploadForm({ names, onDone }: { names: string[]; onDone(ok: boolean): void }): React.JSX.Element {
  const [host, setHost] = useState(names[0] ?? '')
  const [cert, setCert] = useState('')
  const [key, setKey] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<ActionFailure | null>(null)
  const valid = host !== '' && cert.trim() !== '' && key.trim() !== ''

  const upload = async (): Promise<void> => {
    if (!valid || busy) return
    const yes = await useConfirmDialogStore.getState().confirm({
      title: `Replace the certificate for ${host}?`,
      message: 'K2 uses this certificate and its key from now on, and stops renewing the name by itself.',
      confirmLabel: 'Replace',
      destructive: true,
    })
    if (!yes) return
    setBusy(true)
    setError(null)
    try {
      await cliCall('/cli/certs/upload', { hostname: host, certPem: cert, keyPem: key })
      onDone(true)
    } catch (e) {
      setError(failure(`K2 couldn’t use this certificate for ${host}.`, e))
      setBusy(false)
    }
  }

  return (
    <Surface chamfer material="tile" className="p-3 space-y-3" data-testid="upload-cert-form">
      <div className="flex items-center justify-between gap-6">
        <span className={SETTING_ROW_LABEL}>For</span>
        <div className="w-60 flex-shrink-0">
          <SettingDropdown fullWidth ariaLabel="Name for the certificate" value={host} options={names.map((n) => ({ value: n, label: n }))} onChange={setHost} />
        </div>
      </div>
      <Textarea
        aria-label="Certificate"
        value={cert}
        rows={3}
        spellCheck={false}
        placeholder="Paste the certificate, BEGIN and END lines included"
        onChange={(e) => setCert(e.target.value)}
        className="font-mono"
      />
      <Textarea
        aria-label="Private key"
        value={key}
        rows={3}
        spellCheck={false}
        placeholder="Paste its private key, BEGIN and END lines included"
        onChange={(e) => setKey(e.target.value)}
        className="font-mono"
      />
      {error && (
        <Callout tone="error" role="alert">
          <p className="type-row">{error.title}</p>
          <p className="type-hint text-[var(--color-text-secondary)] mt-0.5 break-words">The server said: {error.detail.replace(/[.\s]+$/, '')}.</p>
        </Callout>
      )}
      <div className="flex items-center justify-end gap-2">
        <Button variant="quiet" onClick={() => onDone(false)}>
          Cancel
        </Button>
        <Button variant="accent" disabled={!valid || busy} onClick={() => void upload()}>
          {busy ? 'Uploading…' : 'Upload'}
        </Button>
      </div>
    </Surface>
  )
}

// ── Firewall (0.46.6, prd-custom-domains-path-a-0466-v1 §4.5) ────────
// Thin client on GET /cli/firewall (the daemon works the view out; never a
// scan). Ports K2 opened say which feature; only yours can be closed here.

export function FirewallPanel(): React.JSX.Element {
  const [state, setState] = useState<Load<FirewallStatus>>({ kind: 'loading' })
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<ActionFailure | null>(null)
  const [opening, setOpening] = useState(false)
  const [sshOpen, setSshOpen] = useState(false)
  const openRef = useRef<HTMLButtonElement>(null)

  const load = useCallback(async (): Promise<void> => {
    try {
      setState({ kind: 'ready', data: parseFirewallStatus(await cliCall('/cli/firewall')) })
    } catch (e) {
      setState(readFailure(e))
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  const act = async (title: string, path: string, payload: unknown): Promise<boolean> => {
    setBusy(true)
    setError(null)
    try {
      await cliCall(path, payload)
      await load()
      return true
    } catch (e) {
      setError(failure(title, e))
      return false
    } finally {
      setBusy(false)
    }
  }

  const close = async (r: FirewallPort): Promise<void> => {
    const what = firewallPortWords(r).what
    const yes = await useConfirmDialogStore.getState().confirm({
      title: `Close port ${r.port}/${r.proto}?`,
      message: `Nothing on the internet can reach ${what.toLowerCase()} on this port any more.`,
      confirmLabel: 'Close port',
      destructive: true,
    })
    if (yes) void act(`K2 couldn’t close port ${r.port}/${r.proto}.`, '/cli/firewall/close', { port: `${r.port}/${r.proto}` })
  }

  const heading = (
    <div className="mb-2">
      <GroupHeading title="Firewall" />
    </div>
  )
  if (state.kind === 'loading') {
    return (
      <section data-testid="firewall-panel" data-settings-id="domains.firewall">
        {heading}
        <SettingsLoadState kind="loading" title="Loading the firewall…" />
      </section>
    )
  }
  if (state.kind === 'error') {
    return (
      <section data-testid="firewall-panel" data-settings-id="domains.firewall">
        {heading}
        <SettingsLoadState
          kind="error"
          title="K2 couldn’t read the firewall from this server"
          detail={state.unreadable ? null : state.detail}
          testId="firewall-load-error"
          action={<Button variant="quiet" onClick={() => void load()}>Try again</Button>}
        >
          {state.unreadable
            ? 'Its answer wasn’t a list of ports K2 can read. The ports stay as they were. Try again, or update K2 on the server.'
            : 'The ports stay as they were. Try again in a moment.'}
        </SettingsLoadState>
      </section>
    )
  }

  const st = state.data
  const sshOnly = st.ssh.allowFrom.length > 0
  const front = st.front
  const httpShown = front !== null && (front.enabled || front.relayNames > 0)
  const httpOn = front !== null && front.httpOn && (front.httpServing || !front.enabled)
  return (
    <div data-testid="firewall-panel">
      <SettingsGroup
        title="Firewall"
        hint={`The ports open to the internet on this server, worked out from what K2 runs. ${
          st.managedBy === 'k2' ? 'K2 sets the same list at your provider.' : 'Nothing sets this list at your provider: set the same rules in its firewall.'
        }`}
        settingId="domains.firewall"
        card
        action={
          !opening ? (
            <Button
              ref={openRef}
              variant="quiet"
              onClick={() => {
                setError(null)
                setOpening(true)
              }}
            >
              Open a port
            </Button>
          ) : undefined
        }
      >
        <ActionError error={error} onDismiss={() => setError(null)} />
        {st.behindNat && (
          <div className="px-4 py-3">
            <Callout tone="info">
              <p className="type-hint text-[var(--color-text-primary)]">
                This server is on a home or office connection, so your domains reach it through K2 Connect (Pro).
              </p>
            </Callout>
          </div>
        )}
        {opening && (
          <OpenPortForm
            busy={busy}
            onCancel={() => {
              setOpening(false)
              requestAnimationFrame(() => openRef.current?.focus())
            }}
            onOpen={async (port, label) => {
              if (await act(`K2 couldn’t open port ${port}.`, '/cli/firewall/open', { port, label })) setOpening(false)
            }}
          />
        )}
        {st.ports.map((r) => {
          const w = firewallPortWords(r)
          return (
            <div
              key={`${r.port}/${r.proto}`}
              className={`px-4 py-3 flex items-center justify-between gap-6 ${SETTING_LIST_ROW_HOVER}`}
              data-testid={`fw-row-${r.port}-${r.proto}`}
            >
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <span className="type-name font-mono text-[var(--color-text-primary)]">
                    {r.port}/{r.proto}
                  </span>
                  <Tag tone={w.who === 'K2' ? 'muted' : 'neutral'}>{w.who === 'K2' ? 'K2' : 'You'}</Tag>
                </div>
                <p className={SETTING_ROW_HINT}>{w.what}</p>
              </div>
              {r.source === 'owner' ? (
                <Button variant="danger-quiet" className="flex-shrink-0" disabled={busy} onClick={() => void close(r)}>
                  Close…
                </Button>
              ) : (
                <span className="type-hint text-[var(--color-text-secondary)] flex-shrink-0">
                  {r.source === 'ssh' ? 'Set below' : 'Set by K2'}
                </span>
              )}
            </div>
          )
        })}
        <SettingRow
          label="Who can reach SSH"
          hint={sshOnly ? `Only ${st.ssh.allowFrom.join(', ')}, on port ${st.ssh.port}.` : `Anyone on the internet, on port ${st.ssh.port}. K2 records this; it doesn’t move SSH.`}
          testId="fw-ssh"
          below={
            sshOpen ? (
              <SshEditor
                current={st.ssh.allowFrom}
                busy={busy}
                onCancel={() => setSshOpen(false)}
                onSave={async (payload) => {
                  if (await act('K2 couldn’t change who can reach SSH.', '/cli/firewall/ssh', payload)) setSshOpen(false)
                }}
              />
            ) : undefined
          }
        >
          {!sshOpen && (
            <Button variant="quiet" onClick={() => setSshOpen(true)}>
              Change…
            </Button>
          )}
        </SettingRow>
        {httpShown && front && (
          <SettingRow
            label="Send http to https"
            hint={`Visitors who type http:// go to the secure address.${
              front.enabled && front.httpOn && !front.httpServing ? ' Something else holds port 80, so this isn’t working yet.' : ''
            }`}
            testId="fw-http"
          >
            <Toggle
              checked={httpOn}
              disabled={busy}
              aria-label="Send http to https"
              onChange={(next) => void act('K2 couldn’t change the http redirect.', '/cli/front/http', { on: next })}
            />
          </SettingRow>
        )}
        {st.guard && (
          <SettingRow
            label="Visitor limits"
            testId="fw-guard"
            hint={
              st.guard.enabled
                ? `Each visitor can hold ${st.guard.connsPerIp ?? '–'} connections and make ${st.guard.writesPerMin ?? '–'} changes a minute to your Apps. Turned away ${st.guard.refused} · slowed ${st.guard.limited} since K2 started.`
                : 'Off: visitors to your Apps have no limits.'
            }
          >
            <Tag tone={st.guard.enabled ? 'ok' : 'muted'}>{st.guard.enabled ? 'On' : 'Off'}</Tag>
          </SettingRow>
        )}
      </SettingsGroup>
    </div>
  )
}

function OpenPortForm({ busy, onCancel, onOpen }: { busy: boolean; onCancel(): void; onOpen(port: string, label: string): void }): React.JSX.Element {
  const [port, setPort] = useState('')
  const [label, setLabel] = useState('')
  const ref = useRef<HTMLInputElement>(null)
  useEffect(() => {
    requestAnimationFrame(() => ref.current?.focus())
  }, [])
  const p = port.trim().toLowerCase()
  const portOk = /^\d{1,5}(\/(tcp|udp))?$/.test(p) && Number(p.split('/')[0]) > 0 && Number(p.split('/')[0]) < 65536
  const valid = portOk && label.trim() !== ''
  return (
    <div
      className="bg-[var(--color-wash-1)] divide-y divide-[var(--color-border)]"
      role="group"
      aria-label="Open a port"
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.preventDefault()
          e.stopPropagation()
          onCancel()
        } else if (e.key === 'Enter' && valid) {
          e.preventDefault()
          onOpen(p, label.trim())
        }
      }}
    >
      <SettingRow label="Port" hint="A number, with /udp for UDP. TCP if you leave it off." field>
        <Input ref={ref} aria-label="Port" value={port} placeholder="6000/udp" onChange={(e) => setPort(e.target.value)} className="w-full type-control font-mono" />
      </SettingRow>
      <SettingRow label="What it’s for" hint="So you know later why it’s open." field>
        <Input aria-label="What it’s for" value={label} placeholder="Game server" onChange={(e) => setLabel(e.target.value)} className="w-full type-control" />
      </SettingRow>
      <div className="px-4 py-3 flex items-center justify-end gap-2">
        {!valid && (
          <span className="type-hint text-[var(--color-text-secondary)] mr-auto">
            {p !== '' && !portOk ? 'Enter a port from 1 to 65535, like 6000 or 6000/udp.' : 'Enter a port and what it’s for.'}
          </span>
        )}
        <Button variant="quiet" onClick={onCancel}>
          Cancel
        </Button>
        <Button variant="accent" disabled={!valid || busy} onClick={() => onOpen(p, label.trim())}>
          Open port
        </Button>
      </div>
    </div>
  )
}

function SshEditor({
  current,
  busy,
  onCancel,
  onSave,
}: {
  current: string[]
  busy: boolean
  onCancel(): void
  onSave(payload: { any: true } | { allow: string[] }): void
}): React.JSX.Element {
  const [value, setValue] = useState(current.join(', '))
  const list = value.split(',').map((s) => s.trim()).filter(Boolean)
  return (
    <Surface chamfer material="tile" className="p-3 space-y-3" data-testid="fw-ssh-editor">
      <div className="flex items-center justify-between gap-6">
        <div className="min-w-0">
          <p className={SETTING_ROW_LABEL}>Only these addresses</p>
          <p className={SETTING_ROW_HINT}>IP addresses or ranges, separated by commas.</p>
        </div>
        <Input aria-label="Addresses that can reach SSH" value={value} placeholder="198.51.100.0/24" onChange={(e) => setValue(e.target.value)} className="w-60 flex-shrink-0 type-control font-mono" />
      </div>
      <div className="flex items-center justify-end gap-2">
        <Button variant="quiet" className="mr-auto" disabled={busy || current.length === 0} onClick={() => onSave({ any: true })}>
          Let anyone in
        </Button>
        <Button variant="quiet" onClick={onCancel}>
          Cancel
        </Button>
        <Button variant="accent" disabled={busy || list.length === 0} onClick={() => onSave({ allow: list })}>
          Only these
        </Button>
      </div>
    </Surface>
  )
}
