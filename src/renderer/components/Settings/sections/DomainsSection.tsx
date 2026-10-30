// Settings → Network → Custom Domains (prd-custom-domains-and-certs-v1 D4/D11).
// Thin client on GET /cli/domains. Attach ≠ NS transfer.

import React, { useCallback, useEffect, useState } from 'react'
import { getDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import type { SettingEntry } from '../searchManifest'
import { SettingDropdown } from '../controls/SettingControls'
import { primaryScope } from '@/kessel/server-scope'
import { onDomainsChanged } from '@/stores/session-events'

type CertState = { state: string; issuer?: string; expiresAt?: string | null }

/** 0.46.6: a custom App name's serving state (daemon `custom_domains::status_json`). */
export type AppNameStatus = {
  app: string
  state: string
  live: boolean
  notServedYet: boolean
  /** Grandfathered at 0.46.6: served by your own web server, not by K2. */
  external?: boolean
  words: string
  primary?: boolean
  served?: 'this_box' | 'elsewhere' | 'nothing'
  lastError?: string | null
  warning?: string
  error?: string
  txt?: { type: string; name: string; value: string }
}

type DomainNameRow = {
  hostname: string
  role: string
  cert?: CertState
  app?: AppNameStatus
}

/** The chip for an App name: never "issued" for a name nothing serves. */
export function appStateLabel(a: AppNameStatus): string {
  if (a.error === 'unknown_app') return `not served · no App ${a.app}`
  if (a.external) return `served by your own web server (not by K2)${a.primary ? ' · primary' : ''}`
  if (a.notServedYet) return `not served yet · ${a.state.replace('_', ' ')}`
  return a.primary ? 'live · primary' : 'live'
}

export type DomainRow = {
  apex: string
  zoneId: string | null
  dnsWrite: boolean
  /** A8.1: `active` | `pending_ns` from k2.dev bind; null for BYO rows. */
  status?: string | null
  /** Nameservers k2.dev wants at the registrar. */
  nameservers?: string[]
  /** k2.dev auto-added the zone to this account on bind. */
  created?: boolean
  /** `winddown` / `suspended` on k2.dev: records serve, no writes. */
  readOnly?: boolean
  /** The zone was not in k2.dev's last zones list for this server (DN12d). */
  zoneMissing?: boolean
  /** Unix seconds of the server's last check with k2.dev, if any since boot. */
  checkedAt?: number | null
  names: DomainNameRow[]
}

/** Copy for a 409 `zone_owned_elsewhere` attach. */
export const OWNED_ELSEWHERE_COPY = 'This domain belongs to another k2.dev account.'

/** Daemon error with its machine code kept (`zone_owned_elsewhere`, …). */
class DomainApiError extends Error {
  code: string | null
  constructor(message: string, code: string | null) {
    super(message)
    this.code = code
  }
}

function toApiError(status: number, body: unknown): DomainApiError {
  const err = (body as { error?: { hint?: string; code?: string } | string })?.error
  if (typeof err === 'string') return new DomainApiError(err, null)
  return new DomainApiError(err?.hint || `domains ${status}`, err?.code ?? null)
}

/** What the user reads for a failed domain call. */
export function domainErrorText(e: unknown): string {
  if (e instanceof DomainApiError && e.code === 'zone_owned_elsewhere') return OWNED_ELSEWHERE_COPY
  return e instanceof Error ? e.message : String(e)
}

export function isPendingNs(d: DomainRow): boolean {
  return d.status === 'pending_ns'
}

async function cliGet(path: string): Promise<unknown> {
  const creds = await getDaemonWs(primaryScope())
  const res = await fetch(`${daemonHttpBase(creds)}${path}?token=${creds.token}`, {
    method: 'GET',
  })
  const body = await res.json().catch(() => ({}))
  if (!res.ok) throw toApiError(res.status, body)
  return body
}

async function cliPost(path: string, payload: Record<string, string>): Promise<unknown> {
  const creds = await getDaemonWs(primaryScope())
  const res = await fetch(`${daemonHttpBase(creds)}${path}?token=${creds.token}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  })
  const body = await res.json().catch(() => ({}))
  if (!res.ok) throw toApiError(res.status, body)
  return body
}

/** One nameserver with a copy button (square, matches the section's buttons). */
function NameserverCopyRow({ ns }: { ns: string }): React.JSX.Element {
  const [copied, setCopied] = useState(false)
  const copy = () => {
    void navigator.clipboard.writeText(ns)
    setCopied(true)
    setTimeout(() => setCopied(false), 2000)
  }
  return (
    <div className="flex items-center justify-between gap-2 py-0.5">
      <span className="text-[11px] font-mono text-[var(--color-text-primary)] select-all">{ns}</span>
      <button
        type="button"
        onClick={copy}
        aria-label={`Copy ${ns}`}
        className="px-2 py-0.5 text-[10px] border border-[var(--color-border)] text-[var(--color-text-primary)] no-drag cursor-pointer"
      >
        {copied ? 'Copied' : 'Copy'}
      </button>
    </div>
  )
}

/** Pending-nameservers block for a `pending_ns` row (A8.1). */
export function PendingNameservers({
  domain,
  busy,
  onCheckAgain,
}: {
  domain: DomainRow
  busy: boolean
  onCheckAgain: (apex: string) => void
}): React.JSX.Element {
  const ns = domain.nameservers ?? []
  return (
    <div
      className="mt-1 ml-2 px-2 py-2 border border-[var(--color-border)]"
      data-testid={`pending-ns-${domain.apex}`}
    >
      <p className="text-[10px] text-[var(--color-text-muted)] mb-1 leading-relaxed">
        Point this domain&apos;s nameservers to these at your registrar. DNS records
        can&apos;t be changed until k2.dev sees them. This server checks with k2.dev
        by itself and updates here when it does.
      </p>
      {ns.length === 0 ? (
        <p className="text-[10px] text-[var(--color-text-muted)]">
          k2.dev didn&apos;t list nameservers. See your k2.dev dashboard.
        </p>
      ) : (
        ns.map((n) => <NameserverCopyRow key={n} ns={n} />)
      )}
      {domain.created && (
        <p className="text-[10px] text-[var(--color-text-muted)] mt-1">
          Auto-added to your k2.dev account
        </p>
      )}
      <button
        type="button"
        onClick={() => onCheckAgain(domain.apex)}
        disabled={busy}
        className="mt-2 px-2 py-1 text-xs border border-[var(--color-border)] text-[var(--color-text-primary)] disabled:opacity-40 no-drag cursor-pointer"
      >
        Check again
      </button>
    </div>
  )
}

function dnsLabel(d: DomainRow): string {
  if (isPendingNs(d)) return 'Pending nameservers'
  if (d.readOnly) return `K2-hosted DNS (read-only: ${d.status ?? 'on hold'} on k2.dev)`
  return d.dnsWrite ? 'K2-hosted DNS (writable)' : 'BYO DNS (inventory only)'
}

/** Copy for a zone missing from k2.dev's list for this server (DN12d). */
export const ZONE_MISSING_COPY =
  'Not found on k2.dev for this server. The zone may have been removed or moved to another account. Check again, or remove and attach it again.'

export const DOMAINS_MANIFEST: SettingEntry[] = [
  {
    id: 'domains.firewall',
    section: 'domains',
    label: 'Firewall',
    description: 'Public ports on this server, who may reach SSH, ports you open, the App guard',
    keywords: ['firewall', 'ports', 'ssh', 'open port', 'guard', 'rate limit', 'nft', 'ufw'],
  },
  {
    id: 'domains.inventory',
    section: 'domains',
    label: 'Custom domains',
    description: 'Names this machine terminates — attach an apex, then hostnames under it',
    keywords: ['domain', 'apex', 'hostname', 'dns', 'cert', 'tls', 'lztek', 'bind'],
  },
  {
    id: 'domains.add',
    section: 'domains',
    label: 'Add domain',
    description: 'Attach an apex to this server (does not transfer NS)',
    keywords: ['add', 'attach', 'apex', 'bind'],
  },
]

function certLabel(cert?: CertState): string {
  const state = cert?.state || 'missing'
  if (state === 'missing') return '—'
  if (cert?.issuer) return `${state} (${cert.issuer})`
  return state
}

export function DomainsSection(): React.JSX.Element {
  const [domains, setDomains] = useState<DomainRow[]>([])
  const [error, setError] = useState<string | null>(null)
  const [apex, setApex] = useState('')
  const [hostname, setHostname] = useState('')
  const [role, setRole] = useState('other')
  const [appId, setAppId] = useState('')
  const [apps, setApps] = useState<string[]>([])
  const [busy, setBusy] = useState(false)
  const [acmeEmail, setAcmeEmail] = useState('')
  const [acmeDir, setAcmeDir] = useState('')
  const [uploadHost, setUploadHost] = useState('')
  const [uploadCert, setUploadCert] = useState('')
  const [uploadKey, setUploadKey] = useState('')

  const refresh = useCallback(async () => {
    try {
      const body = (await cliGet('/cli/domains')) as { domains?: DomainRow[] }
      setDomains(body.domains ?? [])
      setError(null)
    } catch (e) {
      setError(domainErrorText(e))
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  // DN12e: the daemon re-checks pending zones by itself (and other clients
  // attach / remove); repaint when it says the inventory changed.
  useEffect(
    () =>
      onDomainsChanged(primaryScope(), () => {
        void refresh()
      }),
    [refresh],
  )

  const addApex = async () => {
    const v = apex.trim()
    if (!v || busy) return
    setBusy(true)
    try {
      await cliPost('/cli/domains', { apex: v })
      setApex('')
      await refresh()
    } catch (e) {
      setError(domainErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const checkAgain = async (value: string) => {
    if (busy) return
    setBusy(true)
    try {
      const body = (await cliPost('/cli/domains/refresh', { apex: value })) as {
        checked?: boolean
        hint?: string
      }
      await refresh()
      if (body.checked === false) setError(body.hint || 'Could not check with k2.dev')
    } catch (e) {
      setError(domainErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const removeApex = async (value: string) => {
    if (busy) return
    setBusy(true)
    try {
      await cliPost('/cli/domains/remove', { apex: value })
      await refresh()
    } catch (e) {
      setError(domainErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  // 0.46.6: the role picker's "App…" lists this server's Apps.
  useEffect(() => {
    if (role !== 'app' || apps.length > 0) return
    void (async () => {
      try {
        const body = (await cliGet('/cli/app/list')) as {
          apps?: { app?: string; appId?: string; id?: string }[]
        }
        const ids = (body.apps ?? [])
          .map((a) => a.app ?? a.appId ?? a.id ?? '')
          .filter((s) => s.length > 0)
        setApps(ids)
        if (ids.length > 0 && !appId) setAppId(ids[0])
      } catch (e) {
        setError(domainErrorText(e))
      }
    })()
  }, [role, apps.length, appId])

  const checkAppName = async (app: string, host: string) => {
    if (busy) return
    setBusy(true)
    try {
      await cliPost('/cli/app/domain/check', { app, host })
      await refresh()
    } catch (e) {
      setError(domainErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const addName = async () => {
    const v = hostname.trim()
    if (!v || busy) return
    if (role === 'app' && !appId) return
    setBusy(true)
    try {
      await cliPost('/cli/domains/names', { hostname: v, role: role === 'app' ? `app:${appId}` : role })
      setHostname('')
      await refresh()
    } catch (e) {
      setError(domainErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const issue = async (value: string) => {
    if (busy) return
    setBusy(true)
    try {
      await cliPost('/cli/certs/issue', { hostname: value })
      await refresh()
    } catch (e) {
      setError(domainErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const renew = async (value: string) => {
    if (busy) return
    setBusy(true)
    try {
      await cliPost('/cli/certs/renew', { hostname: value })
      await refresh()
    } catch (e) {
      setError(domainErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const saveAcme = async () => {
    if (busy) return
    setBusy(true)
    try {
      await cliPost('/cli/certs/config', { email: acmeEmail, directory: acmeDir })
    } catch (e) {
      setError(domainErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const uploadPem = async () => {
    if (busy || !uploadHost.trim()) return
    if (!window.confirm(`Replace the cert for ${uploadHost.trim()} with this PEM?`)) return
    setBusy(true)
    try {
      await cliPost('/cli/certs/upload', {
        hostname: uploadHost.trim(),
        certPem: uploadCert,
        keyPem: uploadKey,
      })
      setUploadCert('')
      setUploadKey('')
      await refresh()
    } catch (e) {
      setError(domainErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const removeName = async (value: string) => {
    if (busy) return
    setBusy(true)
    try {
      await cliPost('/cli/domains/names/remove', { hostname: value })
      await refresh()
    } catch (e) {
      setError(domainErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="max-w-2xl p-6">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">Domains</h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-4 leading-relaxed">
        Names this machine answers for. Adding an apex does not steal NS at the
        registrar — it attaches the name to this server for inventory, certs, and
        (only if K2 hosts the zone) DNS writes. Tunnel names like foo.k2.dev stay
        under Tunnel.
      </p>

      <div
        className="mb-4 px-3 py-3 border border-[var(--color-border)]"
        data-settings-id="domains.add"
      >
        <div className="text-xs text-[var(--color-text-primary)]">Add domain</div>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5 mb-2">
          Apex only (example.com). Then add hostnames such as mail.example.com.
        </p>
        <div className="flex gap-2">
          <input
            value={apex}
            onChange={(e) => setApex(e.target.value)}
            placeholder="example.com"
            className="flex-1 px-2 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
            onKeyDown={(e) => {
              if (e.key === 'Enter') void addApex()
            }}
          />
          <button
            type="button"
            onClick={() => void addApex()}
            disabled={busy || !apex.trim()}
            className="px-2 py-1 text-xs border border-[var(--color-border)] text-[var(--color-text-primary)] disabled:opacity-40 no-drag cursor-pointer"
          >
            Attach
          </button>
        </div>
      </div>

      <div
        className="mb-4 px-3 py-3 border border-[var(--color-border)]"
        data-settings-id="domains.inventory"
      >
        <div className="text-xs text-[var(--color-text-primary)] mb-2">Attached</div>
        {error && (
          <p className="text-[10px] text-[var(--color-status-error)] mb-2">{error}</p>
        )}
        {domains.length === 0 && !error && (
          <p className="text-[10px] text-[var(--color-text-muted)]">
            No custom domains on this server yet. Attach an apex to inventory names
            and certs. This is not a nameserver cutover.
          </p>
        )}
        {domains.map((d) => (
          <div key={d.apex} className="mb-3 last:mb-0">
            <div className="flex items-center justify-between gap-2">
              <div>
                <div className="text-xs font-mono text-[var(--color-text-primary)]">{d.apex}</div>
                <div className="text-[10px] text-[var(--color-text-muted)]">
                  {dnsLabel(d)}
                  {d.zoneId ? ` · zone ${d.zoneId}` : ''}
                </div>
              </div>
              <button
                type="button"
                onClick={() => void removeApex(d.apex)}
                className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
              >
                Remove
              </button>
            </div>
            {d.zoneMissing && (
              <div
                className="mt-1 ml-2 px-2 py-2 border border-[var(--color-border)]"
                data-testid={`zone-missing-${d.apex}`}
              >
                <p className="text-[10px] text-[var(--color-status-error)] leading-relaxed">
                  {ZONE_MISSING_COPY}
                </p>
                {!isPendingNs(d) && (
                  <button
                    type="button"
                    onClick={() => void checkAgain(d.apex)}
                    disabled={busy}
                    className="mt-2 px-2 py-1 text-xs border border-[var(--color-border)] text-[var(--color-text-primary)] disabled:opacity-40 no-drag cursor-pointer"
                  >
                    Check again
                  </button>
                )}
              </div>
            )}
            {isPendingNs(d) && (
              <PendingNameservers
                domain={d}
                busy={busy}
                onCheckAgain={(a) => void checkAgain(a)}
              />
            )}
            <div className="mt-1 ml-2">
              {d.names.length === 0 && (
                <div className="text-[10px] text-[var(--color-text-muted)]">No hostnames yet</div>
              )}
              {d.names.map((n) => (
                <div key={n.hostname} className="flex items-center justify-between gap-2 py-0.5">
                  <span className="text-[11px] font-mono text-[var(--color-text-secondary)]">
                    {n.hostname}
                    <span className="ml-2 text-[10px] text-[var(--color-text-muted)]">
                      {n.role} · cert {certLabel(n.cert)}
                    </span>
                    {n.app && (
                      <span
                        data-testid={`app-state-${n.hostname}`}
                        className={`ml-2 px-1 text-[10px] border border-[var(--color-border)] ${
                          n.app.notServedYet
                            ? 'text-[var(--color-text-muted)]'
                            : 'text-[var(--color-accent)]'
                        }`}
                      >
                        {appStateLabel(n.app)}
                      </span>
                    )}
                    {n.app && n.app.notServedYet && (
                      <span className="block mt-0.5 text-[10px] text-[var(--color-text-muted)] whitespace-normal">
                        {n.app.words}
                        {n.app.lastError ? ` — ${n.app.lastError}` : ''}
                        {n.app.txt && (
                          <span className="block font-mono">
                            TXT {n.app.txt.name} &quot;{n.app.txt.value}&quot; (DNS-only)
                          </span>
                        )}
                      </span>
                    )}
                    {n.app?.warning && (
                      <span className="block mt-0.5 text-[10px] text-[var(--color-status-error)]">
                        {n.app.warning}
                      </span>
                    )}
                  </span>
                  <span className="flex gap-2">
                    {n.app && !n.app.error && (
                      <button
                        type="button"
                        onClick={() => void checkAppName(n.app!.app, n.hostname)}
                        className="text-[10px] text-[var(--color-accent)] no-drag cursor-pointer"
                      >
                        Check now
                      </button>
                    )}
                    <button
                      type="button"
                      onClick={() => void issue(n.hostname)}
                      className="text-[10px] text-[var(--color-accent)] no-drag cursor-pointer"
                    >
                      Issue
                    </button>
                    <button
                      type="button"
                      onClick={() => void renew(n.hostname)}
                      className="text-[10px] text-[var(--color-accent)] no-drag cursor-pointer"
                    >
                      Renew
                    </button>
                    <button
                      type="button"
                      onClick={() => void removeName(n.hostname)}
                      className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
                    >
                      Remove
                    </button>
                  </span>
                </div>
              ))}
            </div>
          </div>
        ))}
      </div>

      <div className="px-3 py-3 border border-[var(--color-border)]">
        <div className="text-xs text-[var(--color-text-primary)]">Add hostname</div>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5 mb-2">
          Must be an attached apex or a label under one.
        </p>
        <div className="flex gap-2">
          <input
            value={hostname}
            onChange={(e) => setHostname(e.target.value)}
            placeholder="mail.example.com"
            className="flex-1 px-2 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
            onKeyDown={(e) => {
              if (e.key === 'Enter') void addName()
            }}
          />
          <SettingDropdown
            value={role}
            onChange={setRole}
            ariaLabel="Hostname role"
            options={[
              { value: 'other', label: 'other' },
              { value: 'mail', label: 'mail' },
              { value: 'publish', label: 'publish' },
              { value: 'direct', label: 'direct' },
              { value: 'app', label: 'App…' },
            ]}
          />
          {role === 'app' && (
            <SettingDropdown
              value={appId}
              onChange={setAppId}
              ariaLabel="App"
              options={apps.map((a) => ({ value: a, label: a }))}
            />
          )}
          <button
            type="button"
            onClick={() => void addName()}
            disabled={busy || !hostname.trim()}
            className="px-2 py-1 text-xs border border-[var(--color-border)] text-[var(--color-text-primary)] disabled:opacity-40 no-drag cursor-pointer"
          >
            Add
          </button>
        </div>
      </div>

      <div className="mt-4 px-3 py-3 border border-[var(--color-border)]">
        <div className="text-xs text-[var(--color-text-primary)]">ACME override</div>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5 mb-2">
          Optional email and directory URL (ZeroSSL / staging). Default is Let&apos;s Encrypt.
        </p>
        <div className="flex gap-2 mb-2">
          <input
            value={acmeEmail}
            onChange={(e) => setAcmeEmail(e.target.value)}
            placeholder="acme@example.com"
            className="flex-1 px-2 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)]"
          />
          <input
            value={acmeDir}
            onChange={(e) => setAcmeDir(e.target.value)}
            placeholder="https://acme-v02.api.letsencrypt.org/directory"
            className="flex-1 px-2 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)]"
          />
          <button
            type="button"
            onClick={() => void saveAcme()}
            className="px-2 py-1 text-xs border border-[var(--color-border)] no-drag cursor-pointer"
          >
            Save
          </button>
        </div>
        <div className="text-xs text-[var(--color-text-primary)] mt-3">Upload PEM</div>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5 mb-2">
          Owner only. Replaces the on-box key for one attached hostname.
        </p>
        <input
          value={uploadHost}
          onChange={(e) => setUploadHost(e.target.value)}
          placeholder="mail.example.com"
          className="w-full mb-2 px-2 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)]"
        />
        <textarea
          value={uploadCert}
          onChange={(e) => setUploadCert(e.target.value)}
          placeholder="-----BEGIN CERTIFICATE-----"
          className="w-full mb-2 px-2 py-1 text-[10px] font-mono h-16 bg-[var(--color-bg)] border border-[var(--color-border)]"
        />
        <textarea
          value={uploadKey}
          onChange={(e) => setUploadKey(e.target.value)}
          placeholder="-----BEGIN PRIVATE KEY-----"
          className="w-full mb-2 px-2 py-1 text-[10px] font-mono h-16 bg-[var(--color-bg)] border border-[var(--color-border)]"
        />
        <button
          type="button"
          onClick={() => void uploadPem()}
          disabled={busy || !uploadHost.trim() || !uploadCert.trim() || !uploadKey.trim()}
          className="px-2 py-1 text-xs border border-[var(--color-border)] disabled:opacity-40 no-drag cursor-pointer"
        >
          Upload
        </button>
      </div>
      <FirewallPanel />
    </div>
  )
}

// ── Settings → Network → Custom Domains → Firewall (0.46.6, prd-custom-domains-path-a-0466-v1 §4.5) ──
// Thin client on GET /cli/firewall (the daemon computes the view; never a
// scan). K2-owned rows are read-only and say which feature opened them.

export type FirewallPort = {
  port: number
  proto: 'tcp' | 'udp'
  source: string
  allowFrom?: string[]
  why: string
}

export type FirewallStatus = {
  view: {
    hash: string
    public: FirewallPort[]
    knownNotPublic: FirewallPort[]
    box: { label?: string | null; behindNat?: boolean | null }
    upstream: { managedBy: string; applied?: { provider: string; appliedAt: number; current: boolean } | null }
  }
  ssh: { port: number; allowFrom: string[] }
  applies: string
  guard: {
    settings: { enabled: boolean; connsPerIp: number; writesPerMin: number }
    counters: { perIpRefused: number; guardLimited: number }
  }
}

/** Plain words for a row's source (K2-owned rows say which feature). */
export function firewallSourceLabel(source: string): string {
  switch (source) {
    case 'ssh':
      return 'SSH'
    case 'front443':
      return 'K2: the :443 front'
    case 'skin-direct':
      return 'K2: Skin Direct'
    case 'hostmail':
      return 'K2: hosted mail'
    case 'sew-voice':
      return 'K2: Chat / Voice Hosting'
    case 'daemon':
      return 'K2: API (LAN)'
    case 'owner':
      return 'You'
    default:
      return source
  }
}

async function cliPostJson(path: string, payload: unknown): Promise<unknown> {
  const creds = await getDaemonWs(primaryScope())
  const res = await fetch(`${daemonHttpBase(creds)}${path}?token=${creds.token}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  })
  const body = await res.json().catch(() => ({}))
  if (!res.ok) throw toApiError(res.status, body)
  return body
}

export function FirewallPanel(): React.JSX.Element {
  const [st, setSt] = useState<FirewallStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [port, setPort] = useState('')
  const [label, setLabel] = useState('')
  const [sshFrom, setSshFrom] = useState('')

  const load = useCallback(async () => {
    try {
      setSt((await cliGet('/cli/firewall')) as FirewallStatus)
      setError(null)
    } catch (e) {
      setError(domainErrorText(e))
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  const act = async (path: string, payload: unknown) => {
    setBusy(true)
    try {
      await cliPostJson(path, payload)
      await load()
    } catch (e) {
      setError(domainErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const rows = st?.view.public ?? []
  const sshOnly = (st?.ssh.allowFrom ?? []).length > 0
  return (
    <div className="mt-6" data-testid="firewall-panel">
      <h3 className="text-xs font-semibold text-[var(--color-text-primary)] mb-1">Firewall</h3>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-2 leading-relaxed">
        Public ports on this server, worked out from what K2 runs. {st?.applies ?? ''}
      </p>
      {error && <p className="text-[10px] text-red-400 mb-2">{error}</p>}
      {st?.view.box.behindNat && (
        <p className="text-[10px] text-[var(--color-text-muted)] mb-2">
          This server is on a home or office connection: your own domains use K2 Connect (Pro, coming in 0.46.7).
        </p>
      )}
      <div className="border border-[var(--color-border)]">
        {rows.map((r) => (
          <div
            key={`${r.port}/${r.proto}`}
            className="flex items-center justify-between gap-2 px-2 py-1 border-b border-[var(--color-border)] last:border-b-0"
            data-testid={`fw-row-${r.port}-${r.proto}`}
          >
            <span className="text-[11px] font-mono w-20">
              {r.port}/{r.proto}
            </span>
            <span className="text-[10px] w-32 text-[var(--color-text-muted)]">{firewallSourceLabel(r.source)}</span>
            <span className="text-[10px] flex-1 truncate" title={r.why}>
              {r.why}
              {r.allowFrom && r.allowFrom[0] !== 'any' ? ` · only ${r.allowFrom.join(', ')}` : ''}
            </span>
            {r.source === 'owner' ? (
              <button
                type="button"
                disabled={busy}
                onClick={() => void act('/cli/firewall/close', { port: `${r.port}/${r.proto}` })}
                className="px-2 py-0.5 text-[10px] border border-[var(--color-border)] no-drag cursor-pointer disabled:opacity-40"
              >
                Remove
              </button>
            ) : (
              <span className="text-[10px] text-[var(--color-text-muted)] w-14 text-right">read-only</span>
            )}
          </div>
        ))}
      </div>
      <div className="mt-2 flex items-center gap-2">
        <span className="text-[10px] w-20">SSH from</span>
        <button
          type="button"
          disabled={busy || !sshOnly}
          onClick={() => void act('/cli/firewall/ssh', { any: true })}
          className="px-2 py-0.5 text-[10px] border border-[var(--color-border)] no-drag cursor-pointer disabled:opacity-40"
        >
          Anyone
        </button>
        <input
          value={sshFrom}
          onChange={(e) => setSshFrom(e.target.value)}
          placeholder={sshOnly ? (st?.ssh.allowFrom ?? []).join(', ') : '198.51.100.0/24'}
          className="flex-1 px-2 py-0.5 text-[10px] font-mono bg-[var(--color-bg)] border border-[var(--color-border)]"
        />
        <button
          type="button"
          disabled={busy || !sshFrom.trim()}
          onClick={() => void act('/cli/firewall/ssh', { allow: sshFrom.split(',').map((s) => s.trim()).filter(Boolean) })}
          className="px-2 py-0.5 text-[10px] border border-[var(--color-border)] no-drag cursor-pointer disabled:opacity-40"
        >
          Only these
        </button>
      </div>
      <div className="mt-2 flex items-center gap-2">
        <span className="text-[10px] w-20">Add a port</span>
        <input
          value={port}
          onChange={(e) => setPort(e.target.value)}
          placeholder="6000/udp"
          className="w-24 px-2 py-0.5 text-[10px] font-mono bg-[var(--color-bg)] border border-[var(--color-border)]"
        />
        <input
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          placeholder="what it's for"
          className="flex-1 px-2 py-0.5 text-[10px] bg-[var(--color-bg)] border border-[var(--color-border)]"
        />
        <button
          type="button"
          disabled={busy || !port.trim() || !label.trim()}
          onClick={() => {
            void act('/cli/firewall/open', { port: port.trim(), label: label.trim() })
            setPort('')
            setLabel('')
          }}
          className="px-2 py-0.5 text-[10px] border border-[var(--color-border)] no-drag cursor-pointer disabled:opacity-40"
        >
          Add
        </button>
      </div>
      {st && (
        <p className="mt-2 text-[10px] text-[var(--color-text-muted)]" data-testid="fw-guard">
          App guard {st.guard.settings.enabled ? 'on' : 'off'}: {st.guard.settings.connsPerIp} connections and{' '}
          {st.guard.settings.writesPerMin} writes a minute per visitor · refused {st.guard.counters.perIpRefused} ·
          limited {st.guard.counters.guardLimited}
        </p>
      )}
    </div>
  )
}
