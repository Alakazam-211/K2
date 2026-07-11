import { useCallback, useEffect, useState } from 'react'
import { getDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import { serverSupports } from '@/lib/server-capabilities'
import {
  onAppHello,
  onTunnelStatusChanged,
  onTunnelSubdomainsChanged,
} from '@/stores/session-events'
import { nestedPublicUrl, sortedTargets, urlCount } from './urls-ports'

// URLs & Ports — a collapsible Workspace-drawer section showing the K2
// Connect tunnel surface: the daemon's public URL / subdomain / tunnel
// local port / relay address, plus the NESTED subdomain map (`staging →
// localhost:3000` under `staging.<sub>.k2.dev`). Self-contained: fetches
// through the host-aware daemon HTTP layer (`getDaemonWs` — a remote
// daemon shows the REMOTE machine's tunnel, exactly what a fleet drawer
// should show) and stays live via the app-level `tunnel_status_changed` +
// `tunnel_subdomains_changed` broadcasts (the CompanionSection pattern),
// with a poll fallback for daemons that pre-date the broadcasts.

interface TunnelStatus {
  running: boolean
  public_url?: string | null
  subdomain?: string | null
  local_port?: number | null
  server_addr?: string | null
  frpc_installed: boolean
}

interface SubdomainsMap {
  primary: string
  targets: Record<string, string>
}

async function fetchTunnelStatus(): Promise<TunnelStatus> {
  const creds = await getDaemonWs()
  const res = await fetch(
    `${daemonHttpBase(creds)}/cli/tunnel/status?token=${creds.token}`,
    { method: 'GET' },
  )
  if (!res.ok) throw new Error(`tunnel status ${res.status}`)
  return (await res.json()) as TunnelStatus
}

async function fetchSubdomains(): Promise<SubdomainsMap> {
  const creds = await getDaemonWs()
  const res = await fetch(
    `${daemonHttpBase(creds)}/cli/tunnel/subdomains?token=${creds.token}`,
    { method: 'GET' },
  )
  // 404 = an older daemon without the route — surfaced as `null` state
  // ("unavailable"), distinct from an EMPTY map.
  if (!res.ok) throw new Error(`tunnel subdomains ${res.status}`)
  const data = (await res.json()) as Partial<SubdomainsMap>
  return {
    primary: typeof data.primary === 'string' ? data.primary : '',
    targets:
      data.targets && typeof data.targets === 'object' ? data.targets : {},
  }
}

// One label/value row, matching the Status block's identity rows above.
function InfoRow({ label, children }: { label: string; children: React.ReactNode }): React.JSX.Element {
  return (
    <div className="flex items-baseline gap-2 mt-1 first:mt-0">
      <span className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] flex-shrink-0 w-16">
        {label}
      </span>
      {children}
    </div>
  )
}

export function UrlsPortsSection({ projectId }: { projectId: string }): React.JSX.Element {
  const [status, setStatus] = useState<TunnelStatus | null>(null)
  // `undefined` = not fetched yet, `null` = route unavailable (older
  // daemon), object = the daemon's cached map (possibly empty — honest
  // "no nested URLs").
  const [subs, setSubs] = useState<SubdomainsMap | null | undefined>(undefined)

  // Collapse state, persisted per-workspace (the Worktrees idiom).
  const collapseKey = projectId ? `urls-ports.section-collapsed.${projectId}` : null
  const [open, setOpen] = useState<boolean>(() => {
    if (!collapseKey) return true
    return localStorage.getItem(collapseKey) !== 'closed'
  })
  useEffect(() => {
    if (!collapseKey) {
      setOpen(true)
      return
    }
    setOpen(localStorage.getItem(collapseKey) !== 'closed')
  }, [collapseKey])
  const toggle = useCallback((): void => {
    setOpen((cur) => {
      const next = !cur
      if (collapseKey) localStorage.setItem(collapseKey, next ? 'open' : 'closed')
      return next
    })
  }, [collapseKey])

  useEffect(() => {
    let cancelled = false
    const refreshStatus = async (): Promise<void> => {
      try {
        const s = await fetchTunnelStatus()
        if (!cancelled) setStatus(s)
      } catch {
        /* ignore — leave previous status / null */
      }
    }
    const refreshSubs = async (): Promise<void> => {
      try {
        const m = await fetchSubdomains()
        if (!cancelled) setSubs(m)
      } catch {
        // Route missing (older daemon) or transient failure — show the
        // honest "unavailable" state rather than a fake empty map.
        if (!cancelled) setSubs((prev) => prev ?? null)
      }
    }
    const refreshAll = (): void => {
      void refreshStatus()
      void refreshSubs()
    }
    refreshAll()

    if (serverSupports('daemon-broadcasts')) {
      const offHello = onAppHello(refreshAll)
      const offTunnel = onTunnelStatusChanged(() => {
        // The event only carries running + publicUrl; this section also
        // renders local_port / server_addr / subdomain, so re-snapshot the
        // full status (one cheap GET on a rare transition). A start/stop
        // can also change the map's relevance — refresh it too.
        refreshAll()
      })
      const offSubs = onTunnelSubdomainsChanged((e) => {
        // Whole-map replace (the ActiveChanged convention) — no GET needed.
        if (!cancelled) setSubs({ primary: e.primary, targets: e.targets })
      })
      // Slow safety poll: a daemon can support `daemon-broadcasts` but
      // pre-date `tunnel_subdomains_changed` (added with this section), so
      // a 30s re-snapshot keeps the map honest against such hosts.
      const slow = setInterval(refreshAll, 30000)
      return () => {
        cancelled = true
        offHello()
        offTunnel()
        offSubs()
        clearInterval(slow)
      }
    }

    // No broadcasts at all (old daemon) — the CompanionSection fallback.
    const interval = setInterval(refreshAll, 5000)
    return () => {
      cancelled = true
      clearInterval(interval)
    }
  }, [])

  const running = status?.running ?? false
  const publicUrl = status?.public_url ?? null
  const targets = subs && typeof subs === 'object' ? subs.targets : {}
  const primary = subs && typeof subs === 'object' ? subs.primary : ''
  const count = urlCount(running, publicUrl, targets)
  const nestedRows = sortedTargets(targets)

  return (
    <>
      {/* ── Header (the Worktrees collapsible idiom) ── */}
      <button
        onClick={toggle}
        className="w-full flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)] cursor-pointer no-drag hover:bg-white/[0.02] transition-colors"
        title={open ? 'Collapse URLs & Ports' : 'Expand URLs & Ports'}
      >
        <span className="flex items-center gap-1.5 text-[11px] text-[var(--color-text-secondary)]">
          <svg
            className={`w-2 h-2 text-[var(--color-text-muted)] transition-transform ${open ? 'rotate-90' : ''}`}
            viewBox="0 0 8 8"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M2 1 L6 4 L2 7" />
          </svg>
          {/* Globe glyph — accent-tinted to match the section-header
              weight of the Heartbeats/Worktrees icons. */}
          <svg
            className="w-3 h-3 text-[var(--color-accent)] flex-shrink-0"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.3"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <circle cx="8" cy="8" r="6.5" />
            <path d="M1.5 8h13M8 1.5c1.8 1.7 2.8 4 2.8 6.5S9.8 13.3 8 14.5C6.2 12.8 5.2 10.5 5.2 8S6.2 3.2 8 1.5z" />
          </svg>
          URLs &amp; Ports
          {count > 0 && (
            <span className="text-[9px] tabular-nums font-medium px-1 py-0.5 bg-[var(--color-wash-1)] text-[var(--color-text-muted)]">
              {count}
            </span>
          )}
        </span>
      </button>

      {open && (
        <div className="px-3 py-2 border-b border-[var(--color-border)]">
          {status === null ? (
            <p className="text-[10px] text-[var(--color-text-muted)]">
              Checking tunnel status…
            </p>
          ) : !running ? (
            <div className="flex items-center gap-2">
              <span
                className="w-2 h-2 flex-shrink-0 rounded-full"
                style={{ backgroundColor: 'var(--color-neutral)' }}
              />
              <span className="text-[11px] text-[var(--color-text-muted)]">
                K2 Connect tunnel isn&apos;t running
              </span>
            </div>
          ) : (
            <div>
              <InfoRow label="Public">
                {publicUrl ? (
                  <a
                    href={publicUrl}
                    target="_blank"
                    rel="noreferrer"
                    className="text-[11px] font-mono text-[var(--color-accent)] hover:underline truncate no-drag cursor-pointer"
                  >
                    {publicUrl}
                  </a>
                ) : (
                  <span className="text-[11px] text-[var(--color-text-muted)]">
                    (assigned by server)
                  </span>
                )}
              </InfoRow>
              {status.subdomain && (
                <InfoRow label="Subdomain">
                  <span className="text-[11px] font-mono text-[var(--color-text-primary)] truncate">
                    {status.subdomain}
                  </span>
                </InfoRow>
              )}
              {typeof status.local_port === 'number' && (
                <InfoRow label="Local port">
                  <span className="text-[11px] font-mono tabular-nums text-[var(--color-text-primary)]">
                    {status.local_port}
                  </span>
                </InfoRow>
              )}
              {status.server_addr && (
                <InfoRow label="Relay">
                  <span className="text-[11px] font-mono text-[var(--color-text-primary)] truncate">
                    {status.server_addr}
                  </span>
                </InfoRow>
              )}
            </div>
          )}

          {/* ── Nested subdomain map ── */}
          <div className="mt-2 pt-2 border-t border-[var(--color-border)]">
            <span className="text-[10px] font-medium text-[var(--color-text-muted)] uppercase tracking-wider">
              Nested URLs
            </span>
            {subs === null ? (
              <p className="text-[10px] text-[var(--color-text-muted)] mt-1">
                Not available on this daemon.
              </p>
            ) : nestedRows.length === 0 ? (
              <p className="text-[10px] text-[var(--color-text-muted)] mt-1">
                No nested URLs.
              </p>
            ) : (
              <div className="mt-1.5 space-y-1.5">
                {nestedRows.map(([label, target]) => {
                  const url = nestedPublicUrl(label, primary, publicUrl)
                  return (
                    <div key={label} className="min-w-0">
                      {url ? (
                        <a
                          href={url}
                          target="_blank"
                          rel="noreferrer"
                          className="block text-[11px] font-mono text-[var(--color-accent)] hover:underline truncate no-drag cursor-pointer"
                        >
                          {url}
                        </a>
                      ) : (
                        <span className="block text-[11px] font-mono text-[var(--color-text-primary)] truncate">
                          {label}
                        </span>
                      )}
                      <span className="block text-[10px] text-[var(--color-text-muted)] truncate">
                        → {target}
                      </span>
                    </div>
                  )
                })}
              </div>
            )}
          </div>
        </div>
      )}
    </>
  )
}
