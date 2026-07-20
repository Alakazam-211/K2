// Shared tunnel-URLs state — the fetch/subscribe half of the URLs
// surfaces, extracted so the workspace drawer (UrlsPortsSection) and the
// K2 Connect settings panel (TunnelUrlsPanel) render ONE truth instead of
// duplicating it. Self-contained: fetches through the host-aware daemon
// HTTP layer (`getDaemonWs` — a remote daemon shows the REMOTE machine's
// tunnel, exactly what a fleet surface should show) and stays live via
// the app-level `tunnel_status_changed` + `tunnel_subdomains_changed`
// broadcasts (the CompanionSection pattern), with a poll fallback for
// daemons that pre-date the broadcasts.

import { useEffect, useState } from 'react'
import { getDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import { withCliTokenQuery, withDaemonFetch } from '@/web/session-token'
import { serverSupports } from '@/lib/server-capabilities'
import {
  onAppHello,
  onTunnelStatusChanged,
  onTunnelSubdomainsChanged,
} from '@/stores/session-events'
import {
  normalizeTargets,
  type SubdomainTargetInfo,
} from '@/components/WorkspacePanel/urls-ports'

export interface TunnelStatus {
  running: boolean
  public_url?: string | null
  subdomain?: string | null
  local_port?: number | null
  server_addr?: string | null
  frpc_installed: boolean
}

export interface SubdomainsMap {
  primary: string
  /** Normalized (urls-ports.ts) — both the 0074 object shape and the
   *  pre-0074 bare-string shape land here as `{target, projectId}`. */
  targets: Record<string, SubdomainTargetInfo>
}

async function fetchTunnelStatus(): Promise<TunnelStatus> {
  const creds = await getDaemonWs()
  const res = await fetch(
    withCliTokenQuery(`${daemonHttpBase(creds)}/cli/tunnel/status`, creds.token),
    withDaemonFetch({ method: 'GET' }),
  )
  if (!res.ok) throw new Error(`tunnel status ${res.status}`)
  return (await res.json()) as TunnelStatus
}

async function fetchSubdomains(): Promise<SubdomainsMap> {
  const creds = await getDaemonWs()
  const res = await fetch(
    withCliTokenQuery(`${daemonHttpBase(creds)}/cli/tunnel/subdomains`, creds.token),
    withDaemonFetch({ method: 'GET' }),
  )
  // 404 = an older daemon without the route — surfaced as `null` state
  // ("unavailable"), distinct from an EMPTY map.
  if (!res.ok) throw new Error(`tunnel subdomains ${res.status}`)
  const data = (await res.json()) as { primary?: unknown; targets?: unknown }
  return {
    primary: typeof data.primary === 'string' ? data.primary : '',
    targets: normalizeTargets(data.targets),
  }
}

export interface TunnelUrlsState {
  /** `null` = not fetched yet / unreachable. */
  status: TunnelStatus | null
  /** `undefined` = not fetched yet, `null` = route unavailable (older
   *  daemon), object = the daemon's map (possibly empty — honest
   *  "no nested URLs"). */
  subs: SubdomainsMap | null | undefined
}

/** Live tunnel status + nested-subdomain map for the ACTIVE daemon.
 *  Fetches on mount, then converges via broadcasts (with the safety /
 *  fallback polls). One instance per consumer — the underlying GETs are
 *  cheap and the broadcast fan-out is shared anyway. */
export function useTunnelUrls(): TunnelUrlsState {
  const [status, setStatus] = useState<TunnelStatus | null>(null)
  const [subs, setSubs] = useState<SubdomainsMap | null | undefined>(undefined)

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
        // The event only carries running + publicUrl; consumers also
        // render local_port / server_addr / subdomain, so re-snapshot the
        // full status (one cheap GET on a rare transition). A start/stop
        // can also change the map's relevance — refresh it too.
        refreshAll()
      })
      const offSubs = onTunnelSubdomainsChanged((e) => {
        // Whole-map replace (the ActiveChanged convention) — no GET
        // needed. Normalize: older daemons broadcast bare-string targets.
        if (!cancelled) setSubs({ primary: e.primary, targets: normalizeTargets(e.targets) })
      })
      // Slow safety poll: a daemon can support `daemon-broadcasts` but
      // pre-date `tunnel_subdomains_changed`, so a 30s re-snapshot keeps
      // the map honest against such hosts.
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

  return { status, subs }
}
