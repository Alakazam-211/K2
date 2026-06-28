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
import { getDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import { useConnectHostStore } from '@/stores/connect-host'

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

// ───────────────────────────────────────────────────────────────────────────
// GAP #3 — owner-driven cross-server CONNECTIONS (auto-pair + add).
//
// The blocker: typing `ai@rpm.k2.dev` into a workspace's "Connected
// Workspaces" editor only ever searched LOCAL workspaces, and the two daemons
// weren't paired. This is the renderer half of the fix: the owner — who holds
// an owner token for BOTH daemons (the active host + a saved remote in the
// connect-host address book) — can, in one gesture, (a) auto-pair the two
// daemons with MUTUAL trust and (b) record the connection on the source side.
//
// Why this is safe TOFU: it only works because ONE operator controls both
// daemons. The host-aware client reads each daemon's owner token from the
// keychain/store, so it can call BOTH daemons' owner-gated pair routes and
// read the SAS back programmatically — there is no human out-of-band step.
// Every step FAILS LOUD; a federation-off peer 404s the pubkey route and we
// abort before recording any half-state.
// ───────────────────────────────────────────────────────────────────────────

/** This daemon's federation identity (mirrors `GET /cli/federation/pubkey`). */
export interface FederationPubkey {
  public_key_pem: string
  fingerprint: string
  /** This daemon's tunnel subdomain, or `''` when none is configured. */
  subdomain: string
}

/** Resolved `{base,token}` for a single daemon. `base` is `<scheme>://<authority>`. */
interface DaemonCreds {
  base: string
  token: string
}

/** Creds for the ACTIVE host (the daemon the renderer is currently driving —
 *  the one that owns the source workspace). Host-aware via daemon-ws. */
async function activeCreds(): Promise<DaemonCreds> {
  const creds = await getDaemonWs()
  return { base: daemonHttpBase(creds), token: creds.token }
}

/**
 * Creds for a SPECIFIC remote host by hostname (e.g. `rpm.k2.dev`). The host
 * must be a saved K2 Connect server the operator has signed into — its token
 * lives in the connect-host store (resolved from the keychain on boot). Fails
 * LOUD with an actionable message when the host is unknown or has no token.
 */
async function remoteCreds(host: string): Promise<DaemonCreds> {
  const wanted = host.trim().toLowerCase()
  const match = useConnectHostStore
    .getState()
    .hosts.find((h) => h.hostname.trim().toLowerCase() === wanted)
  if (!match) {
    throw new Error(
      `"${host}" is not a saved server. Add it under Settings → Connections and sign in first.`,
    )
  }
  if (!match.token) {
    throw new Error(
      `Not signed in to "${host}". Pick it from the server switcher to sign in, then try again.`,
    )
  }
  const scheme = match.secure ? 'https' : 'http'
  const authority =
    match.secure && match.port === 443 ? match.hostname : `${match.hostname}:${match.port}`
  return { base: `${scheme}://${authority}`, token: match.token }
}

/** Parse the `{"error":"..."}`-aware body; throw on non-2xx (fail loud). */
async function parse<T>(res: Response): Promise<T> {
  const text = await res.text()
  if (!res.ok) {
    let msg = text
    try {
      const j = JSON.parse(text)
      if (j && typeof j.error === 'string') msg = j.error
    } catch {
      /* raw text */
    }
    throw new Error(msg || `daemon request failed (${res.status})`)
  }
  if (text.length === 0) return undefined as unknown as T
  try {
    return JSON.parse(text) as T
  } catch {
    return text as unknown as T
  }
}

/** GET `<base>/cli/<route>` against an explicit daemon. */
async function cliGet<T>(
  creds: DaemonCreds,
  route: string,
  params?: Record<string, string | number | boolean | undefined | null>,
): Promise<T> {
  const search = new URLSearchParams()
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      if (v !== undefined && v !== null) search.set(k, String(v))
    }
  }
  search.set('token', creds.token)
  const res = await fetch(`${creds.base}/cli/${route}?${search.toString()}`, { method: 'GET' })
  return parse<T>(res)
}

/** POST `<base>/cli/<route>` (JSON body) against an explicit daemon. */
async function cliPost<T>(creds: DaemonCreds, route: string, body?: unknown): Promise<T> {
  const res = await fetch(`${creds.base}/cli/${route}?token=${encodeURIComponent(creds.token)}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
  return parse<T>(res)
}

/** Read a daemon's federation identity. Throws if federation is off there
 *  (the route 404s) or the body is malformed. */
async function getPubkeyFor(creds: DaemonCreds): Promise<FederationPubkey> {
  const body = await cliGet<Partial<FederationPubkey>>(creds, 'federation/pubkey')
  if (!body?.public_key_pem || !body?.fingerprint) {
    throw new Error('server did not return a federation public key')
  }
  return {
    public_key_pem: body.public_key_pem,
    fingerprint: body.fingerprint,
    subdomain: body.subdomain ?? '',
  }
}

/** This (active) daemon's federation identity. */
export async function getPubkey(): Promise<FederationPubkey> {
  return getPubkeyFor(await activeCreds())
}

/** List a daemon's pinned peers (for the idempotency check). */
async function peersFor(creds: DaemonCreds): Promise<FederationPeer[]> {
  const body = await cliGet<{ peers?: FederationPeer[] }>(creds, 'federation/peers')
  return Array.isArray(body?.peers) ? body.peers : []
}

/** Drive `pair/request` then the owner `pair/confirm` on ONE daemon so it
 *  TRUSTS the presented key. The SAS is read from the request response and
 *  immediately echoed to confirm — safe only because the same owner controls
 *  both ends (no human SAS comparison). */
async function pairAndConfirm(
  creds: DaemonCreds,
  publicKeyPem: string,
  subdomain: string,
): Promise<void> {
  const req = await cliPost<{ fingerprint?: string; sas?: string }>(
    creds,
    'federation/pair/request',
    { public_key_pem: publicKeyPem, subdomain },
  )
  if (!req?.fingerprint || !req?.sas) {
    throw new Error('pair/request did not return a fingerprint + SAS')
  }
  await cliPost(creds, 'federation/pair/confirm', {
    fingerprint: req.fingerprint,
    sas: req.sas,
  })
}

/** A peer's pinned subdomain such that `<subdomain>.k2.dev === host`. Deriving
 *  it from the typed host (strip the `.k2.dev` zone) keeps the daemon-side
 *  send-gate — which reconstructs `<agent>@<subdomain>.k2.dev` — matching the
 *  literal `agent@host` we record. Off-zone hosts fall back to the daemon's
 *  self-reported subdomain (then the raw host). */
function subdomainForHost(host: string, reported: string): string {
  const SUFFIX = '.k2.dev'
  const h = host.trim()
  if (h.toLowerCase().endsWith(SUFFIX)) return h.slice(0, h.length - SUFFIX.length)
  return reported || h
}

/**
 * Establish MUTUAL trust between the active (local) daemon and `host`, using
 * owner authority on both. Idempotent: if both directions are already Trusted
 * it returns immediately. Fails LOUD (and leaves no half-pair beyond what each
 * confirm already committed) if any step errors.
 */
export async function autoPairWithHost(host: string): Promise<void> {
  const localC = await activeCreds()
  const remoteC = await remoteCreds(host)

  // 1. Read both identities. A federation-off peer 404s here → clear error.
  const localPub = await getPubkeyFor(localC).catch((e: unknown) => {
    throw new Error(
      `This server isn't ready for cross-server connections — enable K2 Connect federation in Settings. (${
        e instanceof Error ? e.message : String(e)
      })`,
    )
  })
  const remotePub = await getPubkeyFor(remoteC).catch((e: unknown) => {
    throw new Error(
      `"${host}" isn't ready for cross-server connections — federation may be off there. (${
        e instanceof Error ? e.message : String(e)
      })`,
    )
  })

  // 2. Idempotency: skip whichever direction is already Trusted.
  const [remotePeers, localPeers] = await Promise.all([peersFor(remoteC), peersFor(localC)])
  const remoteTrustsLocal = remotePeers.some(
    (p) => p.fingerprint === localPub.fingerprint && p.trust === 'trusted',
  )
  const localTrustsRemote = localPeers.some(
    (p) => p.fingerprint === remotePub.fingerprint && p.trust === 'trusted',
  )
  if (remoteTrustsLocal && localTrustsRemote) return

  // 3. Make the REMOTE trust LOCAL (pin local's key under local's subdomain).
  if (!remoteTrustsLocal) {
    await pairAndConfirm(remoteC, localPub.public_key_pem, localPub.subdomain)
  }
  // 4. Make LOCAL trust the REMOTE (pin remote's key under the host's
  //    subdomain so the send-gate + dial target line up).
  if (!localTrustsRemote) {
    await pairAndConfirm(localC, remotePub.public_key_pem, subdomainForHost(host, remotePub.subdomain))
  }
}

/** Split `<agent>@<host>` on the LAST `@`; both sides must be non-empty. */
export function parseAgentAtHost(target: string): { agent: string; host: string } | null {
  const t = target.trim()
  const at = t.lastIndexOf('@')
  if (at <= 0 || at >= t.length - 1) return null
  return { agent: t.slice(0, at), host: t.slice(at + 1) }
}

/**
 * Add a REMOTE (cross-daemon) connection `<agent>@<host>` for `sourceWorkspacePath`:
 * auto-pair the two daemons if needed, then record the connection on the LOCAL
 * (active) daemon for the source workspace. The local→remote direction is the
 * one the daemon's send-gate checks, so this is sufficient for the source
 * workspace's agent to message the remote agent.
 *
 * REVERSE-CONNECTION LIMITATION (TODO): the symmetric reverse — on `host`,
 * record `<sourceAgent>@<localSubdomain>` for some remote workspace — is NOT
 * done here. There is no single well-defined remote workspace to attach it to,
 * and resolving the source workspace's agent name on the peer is non-trivial.
 * Mutual TRUST is established (so the reverse is one `connections add` away),
 * but the reverse connection row is left for a follow-up.
 */
export async function addRemoteConnection(
  sourceWorkspacePath: string,
  target: string,
): Promise<void> {
  const parsed = parseAgentAtHost(target)
  if (!parsed) {
    throw new Error(`"${target}" is not a valid remote agent address (expected <agent>@<host>).`)
  }
  // 1. Federation must be enabled on both + a mutual trust pin must exist.
  await autoPairWithHost(parsed.host)
  // 2. Record the connection on the local (active) side for the source ws.
  const localC = await activeCreds()
  await cliGet(localC, 'connections', {
    project: sourceWorkspacePath,
    action: 'add',
    target,
  })
}

/** Remove a REMOTE connection `<agent>@<host>` for `sourceWorkspacePath` from
 *  the local (active) daemon. (Trust pins are left intact.) */
export async function removeRemoteConnection(
  sourceWorkspacePath: string,
  target: string,
): Promise<void> {
  const localC = await activeCreds()
  await cliGet(localC, 'connections', {
    project: sourceWorkspacePath,
    action: 'remove',
    target,
  })
}

/** A remote connection row as the `/cli/connections` list returns it. */
export interface RemoteConnectionEntry {
  remote: true
  address: string
  host: string
  agent: string
}

/** A daemon-wide remote connection: a `RemoteConnectionEntry` plus which of
 *  the daemon's workspaces it belongs to (so the overview can show `(ws: ai)`). */
export interface AggregatedRemoteConnection extends RemoteConnectionEntry {
  /** Source workspace display name on the ACTIVE daemon. */
  sourceWorkspace: string
  /** Source workspace path (stable key). */
  sourcePath: string
}

/**
 * List EVERY cross-daemon `agent@host` connection configured on the ACTIVE
 * daemon, across all of its workspaces — the daemon-global view for the K2
 * Connect overview. Host-aware: returns the connections of whichever daemon is
 * active (local or remote). Walks `projects/list` then each workspace's
 * `/cli/connections` (existing routes — no daemon change, so it works against
 * already-shipped remotes). Returns `[]` on any failure (overview degrades to
 * empty, never throws into the render).
 */
export async function listAllRemoteConnections(): Promise<AggregatedRemoteConnection[]> {
  try {
    const projects = await daemonCliGet<Array<{ id: string; name?: string; path: string }>>(
      'projects/list',
    )
    const list = Array.isArray(projects) ? projects : []
    const out: AggregatedRemoteConnection[] = []
    // Sequential (not Promise.all): the daemon serves one request per TCP
    // connection, and the connection counts are small — bursting would just
    // churn sockets. Each workspace failure degrades to none for that ws.
    for (const p of list) {
      if (!p?.path) continue
      const conns = await listRemoteConnections(p.path)
      for (const c of conns) {
        out.push({ ...c, sourceWorkspace: p.name || p.path, sourcePath: p.path })
      }
    }
    return out
  } catch {
    return []
  }
}

/** List the REMOTE (cross-daemon) connections recorded for a source
 *  workspace on the active daemon. Returns `[]` on any failure (the editor
 *  degrades to local-only). */
export async function listRemoteConnections(
  sourceWorkspacePath: string,
): Promise<RemoteConnectionEntry[]> {
  if (!sourceWorkspacePath) return []
  try {
    const localC = await activeCreds()
    const body = await cliGet<{ connections?: Array<Record<string, unknown>> }>(localC, 'connections', {
      project: sourceWorkspacePath,
      action: 'list',
    })
    const rows = Array.isArray(body?.connections) ? body.connections : []
    return rows
      .filter((c) => c?.remote === true && typeof c.address === 'string')
      .map((c) => ({
        remote: true,
        address: String(c.address),
        host: typeof c.host === 'string' ? c.host : '',
        agent: typeof c.agent === 'string' ? c.agent : '',
      }))
  } catch {
    return []
  }
}
