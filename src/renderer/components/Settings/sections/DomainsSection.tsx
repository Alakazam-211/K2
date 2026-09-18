// Settings → K2 Server → Domains (prd-custom-domains-and-certs-v1 D4/D11).
// Thin client on GET /cli/domains. Attach ≠ NS transfer.

import React, { useCallback, useEffect, useState } from 'react'
import { getDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import type { SettingEntry } from '../searchManifest'

type CertState = { state: string; issuer?: string; expiresAt?: string | null }

type DomainNameRow = {
  hostname: string
  role: string
  cert?: CertState
}

type DomainRow = {
  apex: string
  zoneId: string | null
  dnsWrite: boolean
  names: DomainNameRow[]
}

async function cliGet(path: string): Promise<unknown> {
  const creds = await getDaemonWs()
  const res = await fetch(`${daemonHttpBase(creds)}${path}?token=${creds.token}`, {
    method: 'GET',
  })
  const body = await res.json().catch(() => ({}))
  if (!res.ok) {
    const hint =
      (body as { error?: { hint?: string } | string })?.error
    throw new Error(
      typeof hint === 'string' ? hint : hint?.hint || `domains ${res.status}`,
    )
  }
  return body
}

async function cliPost(path: string, payload: Record<string, string>): Promise<unknown> {
  const creds = await getDaemonWs()
  const res = await fetch(`${daemonHttpBase(creds)}${path}?token=${creds.token}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  })
  const body = await res.json().catch(() => ({}))
  if (!res.ok) {
    const hint =
      (body as { error?: { hint?: string } | string })?.error
    throw new Error(
      typeof hint === 'string' ? hint : hint?.hint || `domains ${res.status}`,
    )
  }
  return body
}

export const DOMAINS_MANIFEST: SettingEntry[] = [
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
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const addApex = async () => {
    const v = apex.trim()
    if (!v || busy) return
    setBusy(true)
    try {
      await cliPost('/cli/domains', { apex: v })
      setApex('')
      await refresh()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
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
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const addName = async () => {
    const v = hostname.trim()
    if (!v || busy) return
    setBusy(true)
    try {
      await cliPost('/cli/domains/names', { hostname: v, role })
      setHostname('')
      await refresh()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
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
      setError(e instanceof Error ? e.message : String(e))
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
      setError(e instanceof Error ? e.message : String(e))
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
      setError(e instanceof Error ? e.message : String(e))
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
      setError(e instanceof Error ? e.message : String(e))
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
      setError(e instanceof Error ? e.message : String(e))
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
                  {d.dnsWrite ? 'K2-hosted DNS (writable)' : 'BYO DNS (inventory only)'}
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
                  </span>
                  <span className="flex gap-2">
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
          <select
            value={role}
            onChange={(e) => setRole(e.target.value)}
            className="px-1 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
          >
            <option value="other">other</option>
            <option value="mail">mail</option>
            <option value="publish">publish</option>
            <option value="direct">direct</option>
          </select>
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
    </div>
  )
}
