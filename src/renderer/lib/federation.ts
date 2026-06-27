// Renderer-side client for the Federation V1 (cross-server agent comms)
// daemon surface — `prd-cross-server-agent-comms.md` Phase 5.
//
// The daemon is authoritative; this is a thin convenience layer that lets the
// Workspace Connections UI populate a cross-server agent picker. It is
// DEFAULT-OFF and FAIL-CLOSED by construction:
//
//   - The whole `/cli/federation/*` surface 404s when the daemon's
//     `K2_FEDERATION` flag is off (the shipped default). Any error from these
//     calls (404 federation-off, 403 not-owner, network) collapses to a
//     "not available" result, so the picker simply renders nothing — no UI
//     change in a shipped build.
//   - `peers` / `peer-roster` are OWNER-gated on the daemon; a member's token
//     gets a 403 → "not available" here too (the component also role-gates up
//     front so members never even call).
//
// These hit the ACTIVE daemon via the host-aware `daemonCli*` layer. They list
// THAT daemon's pinned federation peers and fetch a paired peer's agent roster
// (the local daemon dials the peer's signed roster GET).

import { daemonCliGet } from '@/lib/daemon-cli'

/** Trust state of a pinned peer (mirrors `federation::PeerTrust`). */
export type PeerTrust = 'pending' | 'trusted' | 'blocked'

/** A locally-pinned federation peer (subset the picker needs; no secrets). */
export interface FederationPeer {
  fingerprint: string
  label: string
  subdomain: string
  trust: PeerTrust
  capabilities: string[]
}

/** One agent a peer exposes (mirrors `federation::RosterAgent`). */
export interface RosterAgent {
  /** Authoritative `projects.id` UUID on the peer (Risk H1 — exact). */
  workspace_id: string
  /** Peer-side workspace display name. */
  workspace_name: string
  /** The peer workspace's agent name. */
  agent: string
  /** `<workspace-uuid>::<agent>` — prefix the peer selector to address it. */
  address: string
}

/**
 * Result wrapper: `available:false` means the federation surface is off /
 * denied / unreachable — the caller renders NOTHING (fail-closed). `data` is
 * only present when the daemon actually answered.
 */
export type FederationResult<T> =
  | { available: true; data: T }
  | { available: false }

const UNAVAILABLE = { available: false } as const

/**
 * List the ACTIVE daemon's pinned federation peers. Returns
 * `{available:false}` if federation is off (404), the caller isn't the owner
 * (403), or the daemon is unreachable — so the picker stays hidden by default.
 */
export async function listFederationPeers(): Promise<FederationResult<FederationPeer[]>> {
  try {
    const body = await daemonCliGet<{ peers?: FederationPeer[] }>('federation/peers')
    return { available: true, data: Array.isArray(body?.peers) ? body.peers : [] }
  } catch {
    return UNAVAILABLE
  }
}

/** Only `trusted` peers are addressable; `pending`/`blocked` are filtered. */
export function trustedPeers(peers: FederationPeer[]): FederationPeer[] {
  return peers.filter((p) => p.trust === 'trusted')
}

/**
 * Fetch a paired peer's agent roster via the LOCAL daemon's owner-gated
 * `federation/peer-roster` seam (the daemon dials the peer's signed roster
 * GET). `peerSelector` is a fingerprint, label, or subdomain. Returns
 * `{available:false}` on any failure (fail-closed).
 */
export async function fetchPeerRoster(
  peerSelector: string,
): Promise<FederationResult<RosterAgent[]>> {
  if (!peerSelector) return UNAVAILABLE
  try {
    const body = await daemonCliGet<{ peer?: string; roster?: { agents?: RosterAgent[] } }>(
      'federation/peer-roster',
      { peer: peerSelector },
    )
    const agents = body?.roster?.agents
    return { available: true, data: Array.isArray(agents) ? agents : [] }
  } catch {
    return UNAVAILABLE
  }
}

/**
 * Build the cross-server target address `<peer>::<workspace>::<agent>` for a
 * selected peer + roster entry. The peer side is addressed by FINGERPRINT
 * (authoritative id), the workspace by its UUID (Risk H1) — both carried in
 * `agent.address` (already `<workspace-uuid>::<agent>`).
 */
export function crossServerTarget(peer: FederationPeer, agent: RosterAgent): string {
  return `${peer.fingerprint}::${agent.address}`
}
