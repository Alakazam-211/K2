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
import { withRemoteRetry } from '@/lib/remote-retry'
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
  // Retry-on-network-error so a remote restart (dead pooled WKWebView socket)
  // self-heals without an app relaunch. Non-2xx (404 federation-off / 403
  // not-owner) is authoritative and surfaces immediately.
  return withRemoteRetry(async () => {
    const search = new URLSearchParams()
    if (params) {
      for (const [k, v] of Object.entries(params)) {
        if (v !== undefined && v !== null) search.set(k, String(v))
      }
    }
    search.set('token', creds.token)
    const res = await fetch(`${creds.base}/cli/${route}?${search.toString()}`, { method: 'GET' })
    return parse<T>(res)
  })
}

/** POST `<base>/cli/<route>` (JSON body) against an explicit daemon. */
async function cliPost<T>(creds: DaemonCreds, route: string, body?: unknown): Promise<T> {
  return withRemoteRetry(async () => {
    const res = await fetch(`${creds.base}/cli/${route}?token=${encodeURIComponent(creds.token)}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    })
    return parse<T>(res)
  })
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
 *  send-gate — which reconstructs `<agent>::<subdomain>.k2.dev` — matching the
 *  literal `agent::host` we record. Off-zone hosts fall back to the daemon's
 *  self-reported subdomain (then the raw host). */
function subdomainForHost(host: string, reported: string): string {
  const SUFFIX = '.k2.dev'
  const h = host.trim()
  if (h.toLowerCase().endsWith(SUFFIX)) return h.slice(0, h.length - SUFFIX.length)
  return reported || h
}

/** Whether the RHS of a 2-part `::` address looks like a network host. */
function looksLikeHost(host: string): boolean {
  const h = host.trim()
  if (!h) return false
  return h.includes('.') || h.toLowerCase().endsWith('.k2.dev')
}

/**
 * Establish MUTUAL trust between the active (local) daemon and `host`, using
 * owner authority on both. Idempotent: if both directions are already Trusted
 * it returns immediately. Fails LOUD (and leaves no half-pair beyond what each
 * confirm already committed) if any step errors.
 */
export async function autoPairWithHost(host: string): Promise<void> {
  // Distinguish an AUTH failure (403 — not an owner/admin, or not signed in)
  // from federation-off / unreachable (404 / network) so the copy is accurate.
  const isAuthErr = (m: string) => /403|forbidden|invalid or missing auth token/i.test(m)

  const localC = await activeCreds()
  const remoteC = await remoteCreds(host)

  // 1. Read both identities. A federation-off peer 404s here; a 403 means the
  //    operator lacks owner/admin authority. Map each to clear copy.
  const localPub = await getPubkeyFor(localC).catch((e: unknown) => {
    const inner = e instanceof Error ? e.message : String(e)
    const lead = isAuthErr(inner)
      ? `You must be an owner or admin on this server to connect across servers.`
      : `This server isn't ready for cross-server connections — enable K2 Connect federation in Settings.`
    throw new Error(`${lead} (${inner})`)
  })
  const remotePub = await getPubkeyFor(remoteC).catch((e: unknown) => {
    const inner = e instanceof Error ? e.message : String(e)
    const lead = isAuthErr(inner)
      ? `You must be an owner or admin on "${host}" (and signed in there) to connect across servers.`
      : `"${host}" isn't ready for cross-server connections — federation may be off there.`
    throw new Error(`${lead} (${inner})`)
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

/**
 * Parse a remote **user** address into `{ agent, host }`.
 *
 * Accepts:
 * - canonical `agent::host` (host must look like a host)
 * - legacy `agent@host` (split on the LAST `@`)
 *
 * Rejects 3-part wire form `fp::ws::agent` and non-addresses.
 */
export function parseAgentAtHost(target: string): { agent: string; host: string } | null {
  const t = target.trim()
  if (!t) return null
  // 3-part wire form (two or more '::') — not a user connection address.
  if ((t.match(/::/g) || []).length >= 2) return null
  const colon = t.indexOf('::')
  if (colon > 0) {
    const agent = t.slice(0, colon).trim().toLowerCase()
    const host = t.slice(colon + 2).trim().toLowerCase()
    if (agent && host && looksLikeHost(host)) return { agent, host }
    return null
  }
  const at = t.lastIndexOf('@')
  if (at <= 0 || at >= t.length - 1) return null
  return {
    agent: t.slice(0, at).trim().toLowerCase(),
    host: t.slice(at + 1).trim().toLowerCase(),
  }
}

/** Canonical user form `agent::host` — both sides lowercase (PRD storage/display). */
export function formatAgentHost(agent: string, host: string): string {
  return `${agent.trim().toLowerCase()}::${host.trim().toLowerCase()}`
}

/** Workspace folder basename (the source agent's default name). Splits on / and \\. */
function workspaceBasename(path: string): string {
  const parts = path.replace(/[\\/]+$/, '').split(/[\\/]/)
  return (parts[parts.length - 1] ?? '').toLowerCase()
}

/** Resolve a remote `agent::host`'s workspace FILESYSTEM PATH on the peer, by
 *  joining the peer roster (agent→workspace UUID, via the LOCAL peer-roster seam)
 *  with the peer's projects/list (UUID→path). Never throws; returns {error} when
 *  it can't resolve unambiguously. */
async function resolveRemoteWorkspacePath(
  localC: DaemonCreds, remoteC: DaemonCreds, remoteFp: string, remoteAgent: string,
): Promise<{ path: string } | { error: string }> {
  const rosterBody = await cliGet<{ roster?: { agents?: RosterAgent[] } }>(
    localC, 'federation/peer-roster', { peer: remoteFp })
  const agents = rosterBody?.roster?.agents ?? []
  const want = remoteAgent.trim().toLowerCase()
  const matches = agents.filter((a) => a.agent.trim().toLowerCase() === want)
  if (matches.length === 0) return { error: `no workspace on the peer exposes agent "${remoteAgent}"` }
  if (matches.length > 1) return { error: `agent "${remoteAgent}" is ambiguous on the peer (${matches.length} workspaces)` }
  const wsId = matches[0].workspace_id
  const projects = await cliGet<Array<{ id: string; path: string }>>(remoteC, 'projects/list')
  const proj = (Array.isArray(projects) ? projects : []).find((p) => p?.id === wsId)
  if (!proj?.path) return { error: `peer has no project path for workspace ${wsId}` }
  return { path: proj.path }
}

/** Best-effort REVERSE row. Never throws — returns a human warning on soft failure, else null. */
async function tryAddReverseConnection(
  sourceWorkspacePath: string, remoteAgent: string, host: string,
): Promise<string | null> {
  try {
    const localC = await activeCreds()
    const remoteC = await remoteCreds(host)
    const [localPub, remotePub] = await Promise.all([getPubkeyFor(localC), getPubkeyFor(remoteC)])
    if (!localPub.subdomain) return 'Reverse connection skipped: this server has no tunnel subdomain (the peer could not reach it back).'
    // Always lowercase — federated handles are case-insensitive and stored
    // as `agent::host` (canonical). Capitalized basenames like "Cortana"
    // used to produce Cortana::z3thon.k2.dev and break older remotes that
    // only treated `@` as remote, and mismatched gates that lowercased.
    const sourceAgent = workspaceBasename(sourceWorkspacePath)
    if (!sourceAgent) return 'Reverse connection skipped: could not derive this workspace’s agent name.'
    const resolved = await resolveRemoteWorkspacePath(
      localC, remoteC, remotePub.fingerprint, remoteAgent.trim().toLowerCase(),
    )
    if ('error' in resolved) return `Reverse connection skipped: ${resolved.error}.`
    const reverseTarget = formatAgentHost(sourceAgent, `${localPub.subdomain}.k2.dev`)
    const reverseLegacy = `${sourceAgent}@${localPub.subdomain.toLowerCase()}.k2.dev`
    try {
      await cliGet(remoteC, 'connections', { project: resolved.path, action: 'add', target: reverseTarget })
    } catch (e) {
      // Older peer daemons only recognized agent@host as remote.
      const msg = e instanceof Error ? e.message : String(e)
      if (msg.includes('not found') || msg.includes('Workspace')) {
        await cliGet(remoteC, 'connections', { project: resolved.path, action: 'add', target: reverseLegacy })
      } else {
        throw e
      }
    }
    return null
  } catch (e) {
    return `Reverse connection skipped: ${e instanceof Error ? e.message : String(e)}`
  }
}

/**
 * Add a REMOTE (cross-daemon) connection `<agent>::<host>` for `sourceWorkspacePath`:
 * auto-pair the two daemons if needed, record the connection on the LOCAL
 * (active) daemon for the source workspace, then best-effort record the REVERSE
 * row on `host`. Accepts legacy `agent@host` on input; writes canonical `::`.
 *
 * The forward (local→remote) direction is the one the local daemon's send-gate
 * checks. The reverse row lets the remote agent message back: on `host`, for the
 * workspace exposing `<agent>`, it records `<sourceBasename>::<localSubdomain>.k2.dev`
 * (source agent name = source workspace folder basename; local subdomain from the
 * active daemon's federation pubkey). The reverse is FAIL-SOFT — it NEVER breaks
 * the forward connection or pairing; on any soft failure it returns a human
 * `reverseWarning` (else null) so the operator knows back-messaging isn't wired yet.
 */
export async function addRemoteConnection(
  sourceWorkspacePath: string,
  target: string,
): Promise<{ reverseWarning: string | null }> {
  const parsed = parseAgentAtHost(target)
  if (!parsed) {
    throw new Error(
      `"${target}" is not a valid remote agent address (expected <agent>::<host>).`,
    )
  }
  const canonical = formatAgentHost(parsed.agent, parsed.host)
  const legacy = `${parsed.agent}@${parsed.host}`
  // 1. Federation must be enabled on both + a mutual trust pin must exist.
  await autoPairWithHost(parsed.host)
  // 2. Record the connection on the local (active) side for the source ws.
  // Prefer `::`; fall back to legacy `@` for older peer daemons that only
  // recognized `agent@host` as remote (pre-PR2).
  const localC = await activeCreds()
  try {
    await cliGet(localC, 'connections', {
      project: sourceWorkspacePath,
      action: 'add',
      target: canonical,
    })
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e)
    if (msg.includes('not found') || msg.includes('Workspace')) {
      await cliGet(localC, 'connections', {
        project: sourceWorkspacePath,
        action: 'add',
        target: legacy,
      })
    } else {
      throw e
    }
  }
  // 3. Best-effort reverse row (never throws — soft warning only).
  const reverseWarning = await tryAddReverseConnection(sourceWorkspacePath, parsed.agent, parsed.host)
  return { reverseWarning }
}

/** Remove a REMOTE connection `<agent>::<host>` for `sourceWorkspacePath` from
 *  the local (active) daemon, then best-effort remove the mirrored REVERSE row
 *  on `host`. Accepts legacy `@` on input. (Trust pins are left intact.) The
 *  forward removal is authoritative; a remote failure in the reverse cleanup
 *  never rejects. */
export async function removeRemoteConnection(
  sourceWorkspacePath: string,
  target: string,
): Promise<void> {
  const parsed = parseAgentAtHost(target)
  const canonical = parsed ? formatAgentHost(parsed.agent, parsed.host) : target
  // Also try legacy `@` if the row was stored before `::` normalization
  // (older daemons / pre-PR2 rows).
  const legacy =
    parsed && !target.includes('@')
      ? `${parsed.agent}@${parsed.host}`
      : target.includes('@')
        ? target
        : null
  const localC = await activeCreds()
  const tryRemove = async (t: string) =>
    cliGet(localC, 'connections', {
      project: sourceWorkspacePath,
      action: 'remove',
      target: t,
    })
  try {
    await tryRemove(canonical)
  } catch (first) {
    if (legacy && legacy !== canonical) {
      try {
        await tryRemove(legacy)
      } catch {
        throw first
      }
    } else {
      throw first
    }
  }
  if (!parsed) return
  try {
    const remoteC = await remoteCreds(parsed.host)
    const [localPub, remotePub] = await Promise.all([getPubkey(), getPubkeyFor(remoteC)])
    if (!localPub.subdomain) return
    const resolved = await resolveRemoteWorkspacePath(await activeCreds(), remoteC, remotePub.fingerprint, parsed.agent)
    if ('error' in resolved) return
    const reverseTarget = formatAgentHost(
      workspaceBasename(sourceWorkspacePath),
      `${localPub.subdomain}.k2.dev`,
    )
    const reverseLegacy = `${workspaceBasename(sourceWorkspacePath)}@${localPub.subdomain}.k2.dev`
    try {
      await cliGet(remoteC, 'connections', { project: resolved.path, action: 'remove', target: reverseTarget })
    } catch {
      await cliGet(remoteC, 'connections', { project: resolved.path, action: 'remove', target: reverseLegacy }).catch(() => {})
    }
  } catch { /* best-effort reverse cleanup */ }
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
 * List EVERY cross-daemon `agent::host` connection configured on the ACTIVE
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
