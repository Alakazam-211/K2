// WorkspaceConnectionsDropdown — the Federation V1 (Phase 5) cross-server
// agent picker for the Workspace Connections UI.
//
// `prd-cross-server-agent-comms.md` Phase 5. When the active daemon has PAIRED
// federation peers AND the viewer is owner/admin, this lists those peers'
// agents as selectable options; choosing one sets the cross-server target
// `<peer>::<workspace>::<agent>` (via the federation-target store).
//
// DEFAULT-OFF + FAIL-CLOSED + ROLE-GATED, three independent ways:
//   1. ROLE-GATE — members render NOTHING (return null before any fetch). The
//      daemon ALSO owner-gates `federation/peers`/`peer-roster`; this is the
//      renderer-side defense-in-depth the task asks for.
//   2. FEATURE-GATE — `federation/*` 404s when the daemon's K2_FEDERATION flag
//      is off (the shipped default); `listFederationPeers` collapses that (and
//      any 403/network error) to "unavailable" → render nothing. So a shipped
//      build shows zero UI change.
//   3. EMPTY-GATE — no trusted peers, or no agents → render nothing.
//
// MAIN LOOP / Rosson eyes-on: placement + the "whose agents" UX model (this
// lists the active daemon's PEERS' agents) are the bits to review.

import { useEffect, useMemo, useState } from 'react'
import {
  listFederationPeers,
  fetchPeerRoster,
  trustedPeers,
  crossServerTarget,
  type FederationPeer,
  type RosterAgent,
} from '@/lib/federation'
import { useFederationTargetStore } from '@/stores/federation-target'

/** The viewer's resolved role on the active host (from `/cli/auth/whoami`). */
export type ViewerRole = 'owner' | 'admin' | 'member' | null

/** owner/admin may use the cross-server picker; members may not. */
function canUseFederation(role: ViewerRole): boolean {
  return role === 'owner' || role === 'admin'
}

interface PeerAgentOption {
  peer: FederationPeer
  agent: RosterAgent
  target: string
  label: string
}

export interface WorkspaceConnectionsDropdownProps {
  /** Viewer role on the active host. Members see nothing. */
  role: ViewerRole
  /** Optional notify on selection (in addition to the federation-target store). */
  onSelectTarget?: (target: string, label: string) => void
}

export default function WorkspaceConnectionsDropdown({
  role,
  onSelectTarget,
}: WorkspaceConnectionsDropdownProps): React.JSX.Element | null {
  const setTarget = useFederationTargetStore((s) => s.setTarget)
  const activeTarget = useFederationTargetStore((s) => s.target)
  const [options, setOptions] = useState<PeerAgentOption[] | null>(null)

  // ROLE-GATE: members (and unauthenticated) never fetch and never render.
  const allowed = canUseFederation(role)

  useEffect(() => {
    if (!allowed) {
      setOptions(null)
      return
    }
    let cancelled = false
    void (async () => {
      const peersRes = await listFederationPeers()
      if (cancelled || !peersRes.available) {
        setOptions(null)
        return
      }
      const peers = trustedPeers(peersRes.data)
      if (peers.length === 0) {
        setOptions(null)
        return
      }
      // Fetch each trusted peer's roster (the local daemon dials the peer).
      const rosters = await Promise.all(
        peers.map(async (peer) => {
          const r = await fetchPeerRoster(peer.fingerprint || peer.subdomain)
          return r.available ? { peer, agents: r.data } : { peer, agents: [] }
        }),
      )
      if (cancelled) return
      const opts: PeerAgentOption[] = []
      for (const { peer, agents } of rosters) {
        const peerLabel = peer.label || peer.subdomain || peer.fingerprint.slice(0, 8)
        for (const agent of agents) {
          opts.push({
            peer,
            agent,
            target: crossServerTarget(peer, agent),
            label: `${peerLabel} · ${agent.agent}`,
          })
        }
      }
      setOptions(opts.length > 0 ? opts : null)
    })()
    return () => {
      cancelled = true
    }
  }, [allowed])

  const grouped = useMemo(() => {
    const by = new Map<string, { peer: FederationPeer; items: PeerAgentOption[] }>()
    for (const o of options ?? []) {
      const key = o.peer.fingerprint
      if (!by.has(key)) by.set(key, { peer: o.peer, items: [] })
      by.get(key)!.items.push(o)
    }
    return [...by.values()]
  }, [options])

  // FEATURE / EMPTY gate: nothing to show → render nothing.
  if (!allowed || !options || options.length === 0) return null

  const pick = (o: PeerAgentOption): void => {
    setTarget(o.target, o.label)
    onSelectTarget?.(o.target, o.label)
  }

  return (
    <div
      data-testid="federation-connections"
      className="px-3 py-2 text-[12px]"
      role="group"
      aria-label="Cross-server agents"
    >
      <div className="text-[10px] uppercase tracking-wide text-[var(--color-text-muted)] mb-1">
        Cross-server agents
      </div>
      {grouped.map(({ peer, items }) => (
        <div key={peer.fingerprint} className="mb-1">
          <div className="text-[10px] text-[var(--color-text-muted)] truncate">
            {peer.label || peer.subdomain}
          </div>
          {items.map((o) => (
            <button
              key={o.target}
              data-testid="federation-agent-option"
              onClick={() => pick(o)}
              title={o.target}
              className={`w-full text-left px-2 py-1 rounded transition-colors truncate ${
                activeTarget === o.target
                  ? 'text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)]'
                  : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)]'
              }`}
            >
              {o.agent.workspace_name} · {o.agent.agent}
            </button>
          ))}
        </div>
      ))}
    </div>
  )
}
