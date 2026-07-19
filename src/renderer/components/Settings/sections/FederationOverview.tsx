// FederationOverview — the host-aware "who is this daemon connected to" view
// for Settings → Connections. UNLIKE the saved-servers address book above it
// (a per-DEVICE client concept), this reflects the ACTIVE daemon's own
// relationships, sourced FROM that daemon:
//
//   1. Federated servers   — the daemon's pinned federation peers (the servers
//                            it can exchange messages / cross-agent connects
//                            with). `GET /cli/federation/peers`.
//   2. Cross-agent links   — every `agent::host` connection configured across
//                            the daemon's workspaces. Walks projects/list +
//                            /cli/connections.
//
// Both reads go through the host-aware `daemonCli*` layer, so when the active
// host is REMOTE this shows the REMOTE daemon's peers/connections — exactly the
// servers that could be sending it messages. Fail-soft: a federation-off or
// older daemon collapses each list to an empty/"off" state, never an error.

import React, { useEffect, useState } from 'react'
import { useConnectHostStore } from '@/stores/connect-host'
import {
  listFederationPeers,
  listAllRemoteConnections,
  type FederationPeer,
  type PeerTrust,
  type AggregatedRemoteConnection,
} from '@/lib/federation'

function trustBadge(trust: PeerTrust): { text: string; cls: string } {
  switch (trust) {
    case 'trusted':
      return { text: 'trusted', cls: 'text-emerald-300 border-emerald-500/40 bg-emerald-500/10' }
    case 'blocked':
      return { text: 'blocked', cls: 'text-[var(--color-status-error-bright)] border-[color-mix(in_srgb,var(--color-status-error)_40%,transparent)] bg-[color-mix(in_srgb,var(--color-status-error)_10%,transparent)]' }
    default:
      return { text: 'pending', cls: 'text-[var(--color-status-warn-amber-bright)] border-[color-mix(in_srgb,var(--color-status-warn)_40%,transparent)] bg-[color-mix(in_srgb,var(--color-status-warn)_10%,transparent)]' }
  }
}

function peerLabel(p: FederationPeer): string {
  return p.label || p.subdomain || p.fingerprint.slice(0, 12)
}

export function FederationOverview({
  /** Bump after pairing from a host tile so the peer list reloads. */
  refreshKey = 0,
}: {
  refreshKey?: number
} = {}): React.JSX.Element {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const isLocal = activeHost === 'local'
  const hostKey = isLocal ? 'local' : activeHost.id
  const hostLabel = isLocal ? 'This Mac' : activeHost.label || activeHost.hostname

  const [loading, setLoading] = useState(true)
  // `available` = the daemon's federation surface answered (federation on +
  // owner). When false we show an "off" hint instead of an empty list.
  const [available, setAvailable] = useState(true)
  const [peers, setPeers] = useState<FederationPeer[]>([])
  const [conns, setConns] = useState<AggregatedRemoteConnection[]>([])

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    void (async () => {
      const [peersRes, connsRes] = [await listFederationPeers(), await listAllRemoteConnections()]
      if (cancelled) return
      setAvailable(peersRes.available)
      setPeers(peersRes.available ? peersRes.data : [])
      setConns(connsRes)
      setLoading(false)
    })()
    return () => {
      cancelled = true
    }
  }, [hostKey, refreshKey])

  return (
    <div className="mt-6 space-y-4" data-settings-id="connections.federation-overview">
      {/* Federated servers */}
      <div>
        <h3 className="text-xs font-medium text-[var(--color-text-primary)]">
          Federated servers
          <span className="ml-1.5 text-[10px] font-normal text-[var(--color-text-muted)]">
            ({hostLabel})
          </span>
        </h3>
        <p className="text-[10px] text-[var(--color-text-muted)] mb-2">
          Servers paired with this one — they can exchange messages and cross-agent connects with it.
        </p>
        {loading ? (
          <div className="text-[10px] text-[var(--color-text-muted)] px-3 py-2">Loading…</div>
        ) : !available ? (
          <div className="text-[10px] text-[var(--color-text-muted)] px-3 py-2 border border-[var(--color-border)]">
            Federation is off on {hostLabel}. Enable it under Settings → K2 Connect.
          </div>
        ) : peers.length === 0 ? (
          <div className="text-[10px] text-[var(--color-text-muted)] px-3 py-2 border border-[var(--color-border)]">
            No federated servers yet. On a saved server tile above, click{' '}
            <span className="text-[var(--color-text-secondary)]">Pair as federated peer</span>
            {' '}(federation must be on for this Mac and that server).
          </div>
        ) : (
          <div className="space-y-2">
            {peers.map((p) => {
              const badge = trustBadge(p.trust)
              return (
                <div
                  key={p.fingerprint}
                  className="flex items-center gap-2 px-3 py-2 border border-[var(--color-border)]"
                >
                  <span className="w-2 h-2 flex-shrink-0 rounded-full bg-[var(--color-accent)]" />
                  <div className="flex flex-col min-w-0">
                    <span className="text-xs text-[var(--color-text-primary)] truncate">
                      {peerLabel(p)}
                    </span>
                    {p.subdomain && (
                      <span className="text-[10px] text-[var(--color-text-muted)] truncate">
                        {p.subdomain}.k2.dev
                      </span>
                    )}
                  </div>
                  <span
                    className={`ml-auto px-1.5 py-0.5 text-[10px] border ${badge.cls}`}
                    title={`Trust: ${badge.text}`}
                  >
                    {badge.text}
                  </span>
                </div>
              )
            })}
          </div>
        )}
      </div>

      {/* Cross-agent connections */}
      <div>
        <h3 className="text-xs font-medium text-[var(--color-text-primary)]">Cross-agent connections</h3>
        <p className="text-[10px] text-[var(--color-text-muted)] mb-2">
          Remote agents this server&apos;s workspaces are connected to (<code>agent::host</code>).
        </p>
        {loading ? (
          <div className="text-[10px] text-[var(--color-text-muted)] px-3 py-2">Loading…</div>
        ) : conns.length === 0 ? (
          <div className="text-[10px] text-[var(--color-text-muted)] px-3 py-2 border border-[var(--color-border)]">
            No cross-agent connections.
          </div>
        ) : (
          <div className="space-y-2">
            {conns.map((c) => (
              <div
                key={`${c.sourcePath}|${c.address}`}
                className="flex items-center gap-2 px-3 py-2 border border-[var(--color-border)]"
              >
                <span className="w-2 h-2 flex-shrink-0 rounded-full bg-[var(--color-accent)]" />
                <span className="text-xs text-[var(--color-text-primary)] truncate">{c.address}</span>
                <span className="ml-auto text-[10px] text-[var(--color-text-muted)] truncate">
                  ws: {c.sourceWorkspace}
                </span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
