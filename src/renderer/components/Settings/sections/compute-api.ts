// Settings → Compute (prd-k2-compute-nodes-v1 §15.1, §16.1) — typed renderer
// bindings for `/cli/compute/*`. Thin client: the daemon owns nodes, grants,
// jobs and this computer's node state; this file only reads that truth and
// sends the owner's gestures.
//
// Two scopes:
//   - the CONTROLLER is the server the window is switched to
//     (`primaryScope()`): its nodes, grants, jobs and audit lines.
//   - THIS COMPUTER (`scopeForHost('local')`, desktop only): whether this
//     machine is itself a node, and the machine owner's word over it.
//
// Compute refusals are `{"error":{"code","hint"}}`. `daemon-cli` only lifts a
// STRING `error` into the thrown message, so an object error arrives as the
// raw JSON body text; `computeErrorInfo` parses it back out.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope, scopeForHost, type ServerScope } from '@/kessel/server-scope'
import type { AdminRequest } from './compute-admin-api'

// ── Types (camelCase from the daemon unless noted) ──────────────────────

export type NodeState = 'pending' | 'active' | 'revoked'
export type ControllerPause = 'paused' | 'draining' | null

export interface NodeOffer {
  control: string
  controlBy?: string | null
  available: boolean
  unavailable: string[]
  refusal?: string | null
  /** snake_case: the wire frame's own field names. */
  caps: { cpu_millis: number; mem_bytes: number; disk_budget_bytes: number; hard: boolean }
  free: { cpu_millis: number; mem_bytes: number; disk_free_bytes: number }
  parallelMax: number
  tools: Record<string, string>
  slots?: unknown
  protocol?: number
  features?: string[]
}

export interface ComputeNode {
  id: string
  name: string
  fingerprint: string
  os: string | null
  arch: string | null
  labels: Record<string, string>
  state: NodeState
  enrolledVia: string
  enrolledBy?: string | null
  enrolledAt: number
  confirmedBy: string | null
  controllerPause: ControllerPause
  lastSeen: number | null
  lastRoute: string | null
  protocol: number | null
  nodeVersion: string | null
  routes: string[]
  online: boolean
  liveRoute: string | null
  queued: number
  running: number
  offer?: NodeOffer
  /** Owner only, while pending: "123 456". */
  sas?: string | null
  /** Switched off in Settings: draining, removed once its jobs end. */
  retiring?: boolean
  /** Node sharing (0.46.3+): only on shared nodes and tenants. `tenant` =
   *  another server owns the node (no admin from here). */
  role?: 'owner' | 'tenant'
  share?: NodeShareJson
  /** Job tiers (0.46.5): this server's queued jobs on the node, by tier.
   *  Absent on older daemons. */
  queueTiers?: TierCounts
}

/** A compute job's tier (prd-compute-job-tiers-v1): CI, then Priority (the
 *  critical path), then Normal. The daemon sets and orders them; this page
 *  only shows them. */
export type JobTier = 'ci' | 'priority' | 'normal'

/** Queued jobs by tier, as the daemon counts them. */
export interface TierCounts {
  ci: number
  priority: number
  normal: number
}

/** A submitter with Priority work queued or running on this server
 *  (a courtesy note, never enforced). */
export interface CriticalPathEntry {
  submitter: string
  workspace: string
  jobs: number
}

/** The tier's short tag: `CI`, `P`, or nothing for Normal. */
export function tierTag(tier: JobTier | string | null | undefined): string {
  if (tier === 'ci') return 'CI'
  if (tier === 'priority') return 'P'
  return ''
}

/** `CI 1 · P 0 · N 3`, or '' when nothing is queued. */
export function tierCountsLine(c: TierCounts | null | undefined): string {
  if (!c) return ''
  const ci = c.ci ?? 0
  const p = c.priority ?? 0
  const n = c.normal ?? 0
  if (ci + p + n === 0) return ''
  return `CI ${ci} · P ${p} · N ${n}`
}

/** One server's share (the wire's own snake_case names, `ShareLimits`). */
export interface ShareLimitsJson {
  weight: number
  top_class: 'batch' | 'interactive' | 'gate'
  gate: boolean
  exclusive: boolean
  max_running: number
  max_queued: number
  cpu_hours_day: number
  max_job_secs: number
  max_cpus: number
  max_mem_gb: number
  disk_gb: number
  network: 'on' | 'no-lan' | 'none'
}

/** `share` in the nodes JSON (prd-compute-shared-queue-v1 §4.2): who shares
 *  the node and this server's own share there. */
export interface NodeShareJson {
  shared: boolean
  role: 'owner' | 'tenant'
  ownerLabel: string | null
  servers: number | null
  state: string | null
  expiresAt: number | null
  expiresInSecs: number | null
  limits: ShareLimitsJson | null
  running: number | null
  queued: number | null
  cpuSecsToday: number | null
  /** The daemon's one-line summary ("running 1/1 · queued 2/10 · …"). */
  line: string | null
  /** Job tiers (0.46.5): does this server's Priority count on the node?
   *  null = the node is too old to say. */
  priority?: boolean | null
}

/** One server on a node this server owns (`GET compute/nodes/servers`). */
export interface NodeServerRow {
  slot: number
  label: string
  /** The full key fingerprint once enrolled, else the code's 16 hex. */
  fp?: string | null
  /** pending | active | paused | expired | revoked | refused */
  state: string
  trust: 'own' | 'team'
  role: 'owner' | 'tenant'
  limits?: ShareLimitsJson
  /** The node's runtime facts (snake_case). */
  usage?: { running?: number; queued?: number; cpu_secs_today?: number; online?: boolean } | null
  expiresAt?: number | null
  expiresInSecs?: number
  weight?: number
  /** "482 913" while pending: read it to that server's owner. */
  sas?: string | null
  /** Why its attach is refused (`server_too_old`), when `refused`. */
  refusal?: string | null
  addedAt?: number | null
  addedBy?: string | null
  /** A solo node: just this server. */
  solo?: boolean
  /** Job tiers (0.46.5): its Priority counts on this node (the node's
   *  `queue.toml`). null = the node runs a k2-node without tiers. */
  priority?: boolean | null
}

export interface NodeServersOwner {
  node: string
  role: 'owner'
  shared: boolean
  maxServers: number | null
  solo: boolean | null
  servers: NodeServerRow[]
  /** Open `servers.*` requests (adds, widenings, owner changes) on this node. */
  requests: AdminRequest[]
  note: string | null
  /** The caller may revoke and change the owner (the server's owner in person). */
  ownerActs?: boolean
}

export interface NodeServersTenant {
  node: string
  role: 'tenant'
  owner: string | null
  shared: boolean
  share: unknown
  line: string | null
}

export type NodeServers = NodeServersOwner | NodeServersTenant

/** What a `servers.*` change answered: held for approval, or done at once. */
export interface ServersChangeResult {
  ok?: boolean
  held?: boolean
  request?: AdminRequest
  hint?: string
  changed?: boolean
  class?: string
  note?: string
  warning?: string
}

/** One row of a node's queue (`GET compute/queue`): other servers' jobs
 *  carry only class, state, size and ETA. */
export interface NodeQueueEntry {
  pos: number
  mine: boolean
  class: string
  state: string
  cpuMillis?: number
  memBytes?: number
  etaSecs?: number | null
  blockedBy?: string | null
  /** The owner server sees other servers' labels. */
  server?: string | null
  short?: string
  workspace?: string
  argv?: string
  otherWorkspace?: boolean
  /** The tier the node counts (0.46.5 nodes; never who sent it). */
  tier?: JobTier | null
}

export interface NodeQueue {
  node: string
  shared: boolean
  from: 'node' | 'server'
  entries: NodeQueueEntry[]
  totals?: { slot: number; label: string; running: number; queued: number; cpuSecsToday: number }[]
}

export interface WaitingCode {
  name: string
  expiresAt: number
}

export interface ComputeNodesList {
  nodes: ComputeNode[]
  waitingCodes: WaitingCode[]
  /** This server's label as nodes see it, e.g. `rosson.k2.dev`. */
  controller: string
  /** This server trusts a one-click ticket key (absent on older daemons). */
  ticketSupported?: boolean
  /** Submitters with Priority work on this server (empty = nobody; absent
   *  on daemons without tiers). */
  criticalPath?: CriticalPathEntry[]
}

/** What the Agent tab's "Allow compute nodes" write did. */
export interface ComputeAgentSetResult {
  ok: boolean
  agentsCanUseCompute?: boolean
  granted?: string[]
  kept?: string[]
  grantsRemoved?: number
  note?: string
}

export interface ComputeGrant {
  nodeId: string
  workspaceId: string
  scopes: string
  maxJobSecs: number
  maxDiskGb: number
  maxParallel: number
  maxQueued: number
  warmAt: string | null
  expiresAt: number | null
  grantedBy: string
  grantedAt: number
  node: string | null
  workspace: string
  /** Job tiers (0.46.5): this workspace may send CI jobs to the node (set
   *  by the owner or an Admin only). Absent on older daemons. */
  allowCi?: boolean
}

export interface GrantLimits {
  maxJobSecs?: number
  maxDiskGb?: number
  maxParallel?: number
  maxQueued?: number
}

/** A grant write: its limits (absent ones reset to the defaults) and,
 *  optionally, `allowCi` (absent keeps the stored value; only the owner or
 *  an Admin may set it). */
export type GrantSet = GrantLimits & { allowCi?: boolean }

export interface ComputeJob {
  id: string
  short: string
  node: string | null
  workspace: string
  argv: string[]
  state: string
  reason: string | null
  detail?: string | null
  exitCode: number | null
  /** What the job declared it needs (toolchains, packages). Not on every daemon. */
  needs?: unknown
  /** Job tiers (0.46.5): the tier it runs as, what was asked when the daemon
   *  lowered it (a guest share without Priority), and who sent it. */
  tier?: JobTier
  tierAsked?: string | null
  submitter?: string | null
  createdAt: number
  startedAt?: number | null
  endedAt?: number | null
}

export interface ComputeEvent {
  id: number
  at: number
  actor: string
  kind: string
  nodeId?: string | null
  workspaceId?: string | null
  jobId?: string | null
  detail: unknown
}

export interface EnrollCode {
  name: string
  code: string
  controllerFp: string
  enroll: string
  expiresAt: number
  controllerUrl: string
  install: { linux: string; macos: string; enrollOnly: string }
  confirm: string
}

/** `k2-node`'s own status file (snake_case — written by the node binary). */
export interface LocalNodeStatus {
  version?: string
  protocol?: number
  node_fp?: string
  name?: string | null
  controller?: string | null
  /** unenrolled | pending | connecting | online | offline | revoked */
  state: string
  route?: string | null
  last_error?: string | null
  running?: string[]
  control?: string
  sas?: string | null
  unavailable?: string[]
  caps_hard?: boolean
}

export interface LocalHomeCheck {
  home: string
  mode: string
  private: boolean
  readable: string[]
}

/** The last enroll the node answered (code or ticket). */
export interface LocalEnrollResult {
  id: string
  ok: boolean
  message: string | null
  sas: string | null
  name: string | null
  at: number
}

export interface LocalStatus {
  installed: boolean
  home: string
  configDir: string
  node: LocalNodeStatus | null
  control: Record<string, unknown> | null
  policy: Record<string, unknown> | null
  // Newer daemons only — every one of these may be absent.
  platform?: 'linux' | 'macos'
  binary?: { path: string; version: string | null; source: string } | null
  /** The node binary is installed system-wide (not just found). */
  installedBinary?: boolean
  /** How this daemon gets root: a root helper, the OS admin prompt, or a command for the owner. */
  elevation?: 'helper' | 'prompt' | 'command'
  rootHelper?: { present: boolean; node: boolean; protocol?: number }
  homeCheck?: LocalHomeCheck | null
  packages?: { manager: string | null; approved: string[]; installer?: boolean } | null
  /** The LOCAL daemon's ticket keys. One-click depends on the controller's,
   *  so the page keeps its 404 fallback regardless of this value. */
  ticketSupported?: boolean
  enrollResult?: LocalEnrollResult | null
  enrollRequestPending?: boolean
  /** The node's remote-admin level (admin.toml); null = never set up. */
  remoteAdmin?: RemoteAdminLevel | null
  /** Present when K2 can't update this node (level off or unset): the one
   *  command that turns it on at the machine. */
  remoteAdminCommand?: string
}

/** `k2-node admin level`: off = K2 can't update the node; policy = K2 keeps
 *  it updated and healthy; full = also root actions, each owner-approved. */
export type RemoteAdminLevel = 'off' | 'policy' | 'full'

/** Without `controller` + (`enroll` | `ticket`) the daemon only installs. */
export interface LocalEnableBody {
  controller?: string
  enroll?: string
  ticket?: string
  name?: string
  /** Always sent from Settings: an open home with no choice is refused. */
  privateHome: boolean
  /** Set by the same root step as the install (no second prompt). */
  remoteAdmin?: RemoteAdminLevel
}

export interface LocalEnableResult {
  ok?: boolean
  step?: 'enrolling' | 'needs_root' | 'waiting' | 'installed' | 'ready'
  mode?: string
  command?: string
  message?: string
  sas?: string | null
  /** `waiting`: the enroll request the node will answer. */
  requestId?: string
  /** `installed` / `ready`: the node key, once the service wrote it. */
  nodeFp?: string | null
  rootNeeded?: boolean
}

export interface LocalDisableResult {
  done?: boolean
  rootNeeded?: boolean
  mode?: string
  command?: string
  message?: string
}

export type PolicyKey =
  | 'cpu_cap_percent'
  | 'mem_cap_gb'
  | 'disk_budget_gb'
  | 'availability'
  | 'on_owner_return'
  | 'repo_toolchains'

// ── Errors ──────────────────────────────────────────────────────────────

export function computeErrorInfo(err: unknown): { code?: string; hint?: string } {
  const msg = err instanceof Error ? err.message : String(err)
  try {
    const parsed = JSON.parse(msg)
    const e = parsed?.error
    if (e && typeof e === 'object') {
      return {
        code: typeof e.code === 'string' ? e.code : undefined,
        hint: typeof e.hint === 'string' ? e.hint : undefined,
      }
    }
  } catch {
    /* not the compute refusal shape */
  }
  return {}
}

/** "hint (code)" for a compute refusal, else the plain error text. */
export function computeErrorMessage(err: unknown): string {
  const { code, hint } = computeErrorInfo(err)
  if (hint && code) return `${hint} (${code})`
  if (hint) return hint
  if (code) return code
  return err instanceof Error ? err.message : String(err)
}

/** Compute routes 404 with `compute_disabled` while the preview is off. */
export function isComputeDisabled(err: unknown): boolean {
  return computeErrorInfo(err).code === 'compute_disabled'
}

/** Node names: 1–32 of a-z, 0-9 and '-' (the daemon's `valid_name`). */
export function validNodeName(name: string): boolean {
  return /^[a-z0-9-]{1,32}$/.test(name)
}

// ── Scopes ──────────────────────────────────────────────────────────────

/** This computer's daemon (the "This computer" part; desktop only). */
export function computeLocalScope(): ServerScope {
  return scopeForHost('local')
}

// ── Controller (the focused server) ─────────────────────────────────────

export async function fetchComputeNodes(): Promise<ComputeNodesList> {
  const res = await daemonCliGet<Partial<ComputeNodesList>>(primaryScope(), 'compute/nodes')
  return {
    nodes: Array.isArray(res?.nodes) ? res.nodes : [],
    waitingCodes: Array.isArray(res?.waitingCodes) ? res.waitingCodes : [],
    controller: typeof res?.controller === 'string' ? res.controller : '',
    ticketSupported: res?.ticketSupported === true,
    criticalPath: Array.isArray(res?.criticalPath) ? res.criticalPath : [],
  }
}

/** "Use compute from this machine" off: drain, then remove once idle. */
export async function retireComputeNode(
  node: string,
): Promise<{ ok: boolean; state?: 'revoked' | 'draining'; running?: number; jobsCancelled?: number }> {
  return daemonCliPost(primaryScope(), 'compute/nodes/retire', { node })
}

/** Grants on one node, or every grant on this server when `node` is omitted
 *  (one request for the whole page instead of one per node). */
export async function fetchComputeGrants(node?: string): Promise<ComputeGrant[]> {
  const res = await daemonCliGet<{ grants?: ComputeGrant[] }>(
    primaryScope(),
    'compute/grants',
    node ? { node } : undefined,
  )
  return Array.isArray(res?.grants) ? res.grants : []
}

export async function setComputeGrant(
  node: string,
  workspace: string,
  limits: GrantSet = {},
): Promise<{ ok: boolean; agentsCanUseCompute?: boolean; note?: string }> {
  return daemonCliPost(primaryScope(), 'compute/grants/set', { node, workspace, ...limits })
}

export async function revokeComputeGrant(node: string, workspace: string): Promise<{ ok: boolean; removed?: boolean }> {
  return daemonCliPost(primaryScope(), 'compute/grants/revoke', { node, workspace })
}

export async function confirmComputeNode(node: string, sas: string): Promise<{ ok: boolean; state?: string; next?: string }> {
  return daemonCliPost(primaryScope(), 'compute/nodes/confirm', { node, sas })
}

export async function pauseComputeNode(
  node: string,
  action: 'pause' | 'drain' | 'resume',
): Promise<{ ok: boolean; controllerPause?: ControllerPause }> {
  return daemonCliPost(primaryScope(), 'compute/nodes/pause', { node, action })
}

export async function renameComputeNode(node: string, name: string): Promise<{ ok: boolean; node?: string }> {
  return daemonCliPost(primaryScope(), 'compute/nodes/rename', { node, name })
}

export async function removeComputeNode(node: string): Promise<{ ok: boolean; jobsCancelled?: number }> {
  return daemonCliPost(primaryScope(), 'compute/nodes/remove', { node })
}

export async function mintEnrollCode(name: string): Promise<EnrollCode> {
  return daemonCliPost(primaryScope(), 'compute/nodes/enroll-code', { name })
}

export async function fetchComputeJobs(limit = 20): Promise<ComputeJob[]> {
  const res = await daemonCliGet<{ jobs?: ComputeJob[] }>(primaryScope(), 'compute/jobs', { limit })
  return Array.isArray(res?.jobs) ? res.jobs : []
}

export async function fetchComputeEvents(limit = 30): Promise<ComputeEvent[]> {
  const res = await daemonCliGet<{ events?: ComputeEvent[] }>(primaryScope(), 'compute/events', { limit })
  return Array.isArray(res?.events) ? res.events : []
}

/** The Agent tab's "Allow compute nodes" (Owner/Admin; the single
 *  control, Rosson 2026-10-11): on = the switch plus a grant on every
 *  machine switched on here, default limits (machines added later are
 *  granted by the daemon); off = switch off and every grant removed. */
export async function setWorkspaceComputeSwitch(
  workspace: string,
  value: boolean,
): Promise<ComputeAgentSetResult> {
  return daemonCliPost(primaryScope(), 'agent-access/set', { toggle: 'compute', value, workspace })
}

/** "Allow this agent to operate compute nodes" (node admin Q14, 0156):
 *  the owner's standing approval for the operate class only (narrow,
 *  pause, update, restart). Root actions still wait for the owner each
 *  time. Same Admin route as the CLI (`k2 compute operate on|off`). */
export interface ComputeOperateSetResult {
  ok: boolean
  workspace?: string
  toggle?: string
  agentsCanOperateCompute?: boolean
}

export async function setWorkspaceComputeOperate(
  workspace: string,
  value: boolean,
): Promise<ComputeOperateSetResult> {
  return daemonCliPost(primaryScope(), 'agent-access/set', { toggle: 'compute_operate', value, workspace })
}

/** "Allow managing compute nodes" (`compute_manage`, 0194): a local agent
 *  from the workspace may add, confirm, rename, pause/drain/resume, retire
 *  and route nodes and grant/revoke any workspace, each act audited. The
 *  daemon lets only the owner flip it (an Admin login is refused). Same
 *  route as the CLI (`k2 compute manage on|off`). */
export interface ComputeManageSetResult {
  ok: boolean
  workspace?: string
  toggle?: string
  agentsCanManageCompute?: boolean
}

export async function setWorkspaceComputeManage(
  workspace: string,
  value: boolean,
): Promise<ComputeManageSetResult> {
  return daemonCliPost(primaryScope(), 'agent-access/set', { toggle: 'compute_manage', value, workspace })
}

// ── Node sharing: the servers on a node (prd-compute-shared-queue-v1 §9,
//    prd-macos-tenant-compute-v1 §5) ──────────────────────────────────────
// The daemon decides everything (narrow vs widen, who may revoke, which
// server may become the owner); these only send the gesture and read back.

/** Every server on a node this server owns, or this server's own share on a
 *  node another server owns. `refresh` asks the node now (else the mirror). */
export async function fetchNodeServers(node: string, refresh = false): Promise<NodeServers> {
  return daemonCliGet<NodeServers>(primaryScope(), 'compute/nodes/servers', refresh ? { node, refresh: 1 } : { node })
}

export interface AddServerBody {
  url: string
  /** `CODE.FP` as the new server's `k2 compute node add` printed it. */
  code: string
  trust: 'own' | 'team'
  /** End date from now; omit for the default (7 days for team, none for own). */
  forSecs?: number
  set?: Partial<ShareLimitsJson>
  reason?: string
}

/** Always held for the owner's approval (Approvals / the card). */
export async function addNodeServer(node: string, body: AddServerBody): Promise<ServersChangeResult> {
  return daemonCliPost(primaryScope(), 'compute/nodes/servers/add', { node, ...body })
}

/** A narrowing applies at once; a widening (or a later end date) is held. */
export async function setServerLimits(
  node: string,
  server: string,
  set: Partial<ShareLimitsJson>,
  forSecs?: number,
): Promise<ServersChangeResult> {
  return daemonCliPost(primaryScope(), 'compute/nodes/servers/limits', forSecs === undefined ? { node, server, set } : { node, server, set, forSecs })
}

/** Job tiers (0.46.5): does this server's Priority count on the node? Off
 *  applies at once; on is a widening the daemon holds for approval. Sent
 *  only to nodes with tiers (the daemon answers `node_too_old` otherwise). */
export async function setServerPriority(node: string, server: string, priority: boolean): Promise<ServersChangeResult> {
  return daemonCliPost(primaryScope(), 'compute/nodes/servers/limits', { node, server, priority })
}

export async function pauseServer(node: string, server: string, action: 'pause' | 'resume'): Promise<ServersChangeResult> {
  return daemonCliPost(primaryScope(), 'compute/nodes/servers/pause', { node, server, action })
}

/** The owner only. `now` stops its running jobs; `wipe` wipes its files now (else in 24 h). */
export async function revokeServer(node: string, server: string, opts: { now: boolean; wipe: boolean }): Promise<ServersChangeResult> {
  return daemonCliPost(primaryScope(), 'compute/nodes/servers/revoke', { node, server, ...opts })
}

export interface OwnerChangeOptions {
  /** keep: this server stays on the node as one of yours; revoke: it leaves. */
  old: 'keep' | 'revoke'
  /** move: each server keeps its warm caches; wipe: cold builds once. */
  caches: 'move' | 'wipe'
  /** Stop running jobs instead of waiting for them. */
  now: boolean
  reason?: string
}

/** The owner only; held for the owner's approval, then the node drains and
 *  swaps (prd-macos-tenant-compute-v1 §6). */
export async function changeNodeOwner(node: string, server: string, opts: OwnerChangeOptions): Promise<ServersChangeResult> {
  return daemonCliPost(primaryScope(), 'compute/nodes/servers/owner', { node, server, ...opts })
}

/** The node's whole queue as this server may see it (others redacted). */
export async function fetchNodeQueue(node: string): Promise<NodeQueue> {
  const res = await daemonCliGet<Partial<NodeQueue>>(primaryScope(), 'compute/queue', { node })
  return {
    node: typeof res?.node === 'string' ? res.node : node,
    shared: res?.shared === true,
    from: res?.from === 'node' ? 'node' : 'server',
    entries: Array.isArray(res?.entries) ? res.entries : [],
    totals: Array.isArray(res?.totals) ? res.totals : undefined,
  }
}

/** Servers that may become this node's owner (MT28): one of your own, active,
 *  enrolled (a full key) and online. The daemon checks again. */
export function ownerTargets(rows: NodeServerRow[]): NodeServerRow[] {
  return rows.filter(
    (r) => r.slot !== 0 && r.trust === 'own' && r.state === 'active' && r.usage?.online === true && typeof r.fp === 'string' && /^[0-9a-f]{64}$/.test(r.fp),
  )
}

/** Can this node take other people's servers (`team`)? From the node's own
 *  offer: the `tenant-wall` feature, and why not when it's missing. */
export function teamPossible(node: Pick<ComputeNode, 'os' | 'offer'>): { ok: boolean; why: string | null } {
  const features = Array.isArray(node.offer?.features) ? node.offer.features : []
  if (features.includes('tenant-wall')) return { ok: true, why: null }
  const mac = node.os === 'macos' || node.os === 'darwin'
  return { ok: false, why: mac ? 'a Mac without per-tenant users yet: your own servers only' : 'its tenant wall isn’t on: your own servers only' }
}

/** A share in one line (the daemon's wire fields). */
export function limitsLine(l: ShareLimitsJson | undefined | null): string {
  if (!l) return '—'
  const hours = l.max_job_secs % 3600 === 0 ? `${l.max_job_secs / 3600}h` : `${Math.round(l.max_job_secs / 60)}m`
  return `${l.max_running} running · ${l.max_queued} queued · ${l.cpu_hours_day} CPU-h/day · ${hours}/job · ${l.max_cpus} CPUs · ${l.max_mem_gb} GB · ${l.disk_gb} GB disk · network ${l.network} · weight ${l.weight}`
}

// ── This computer (local daemon) ────────────────────────────────────────

export async function fetchLocalStatus(): Promise<LocalStatus> {
  return daemonCliGet<LocalStatus>(computeLocalScope(), 'compute/local/status')
}

/** Pause after current jobs (default) or now (`now: true` stops them). */
export async function pauseLocalNode(now = false): Promise<{ ok: boolean; control?: string }> {
  return daemonCliPost(computeLocalScope(), 'compute/local/pause', now ? { now: true } : {})
}

export async function drainLocalNode(): Promise<{ ok: boolean; control?: string }> {
  return daemonCliPost(computeLocalScope(), 'compute/local/drain', {})
}

export async function resumeLocalNode(): Promise<{ ok: boolean; control?: string }> {
  return daemonCliPost(computeLocalScope(), 'compute/local/resume', {})
}

export async function setLocalPolicy(
  set: Partial<Record<PolicyKey, string | number | boolean>>,
): Promise<{ ok: boolean; policy?: Record<string, unknown> }> {
  return daemonCliPost(computeLocalScope(), 'compute/local/policy', { set })
}

/** Settings always sends `elevate: true`: the daemon may raise the OS admin
 *  prompt for the one-time setup (the CLI never does). */
export async function enableLocalNode(body: LocalEnableBody): Promise<LocalEnableResult> {
  return daemonCliPost(computeLocalScope(), 'compute/local/enable', { ...body, elevate: true })
}

export interface LocalRemoteAdminResult {
  ok?: boolean
  done?: boolean
  rootNeeded?: boolean
  mode?: string
  command?: string
  message?: string
  remoteAdmin?: RemoteAdminLevel
}

/** The remote-admin toggles, changed after the node is on: the daemon runs
 *  `k2-node admin level` through root (the OS admin prompt from Settings,
 *  or a command to copy). */
export async function setLocalRemoteAdmin(level: RemoteAdminLevel): Promise<LocalRemoteAdminResult> {
  return daemonCliPost(computeLocalScope(), 'compute/local/remote-admin', { level, elevate: true })
}

/** The two toggles → one level. Full needs policy on. */
export function remoteAdminLevel(keepUpdated: boolean, allowRoot: boolean): RemoteAdminLevel {
  if (!keepUpdated) return 'off'
  return allowRoot ? 'full' : 'policy'
}

export async function disableLocalNode(): Promise<LocalDisableResult> {
  return daemonCliPost(computeLocalScope(), 'compute/local/disable', { elevate: true })
}

// ── Formatting ──────────────────────────────────────────────────────────

export function formatBytes(n: number | null | undefined): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return '—'
  const gb = n / 2 ** 30
  if (gb >= 1) return `${gb >= 100 ? Math.round(gb) : Math.round(gb * 10) / 10} GB`
  const mb = n / 2 ** 20
  return `${Math.round(mb)} MB`
}

export function formatCores(cpuMillis: number | null | undefined): string {
  if (cpuMillis === null || cpuMillis === undefined) return '—'
  const c = cpuMillis / 1000
  return `${Number.isInteger(c) ? c : Math.round(c * 10) / 10} cores`
}

export type NodeDot = 'online' | 'offline' | 'pending' | 'paused' | 'draining'

export function nodeDot(n: Pick<ComputeNode, 'state' | 'online' | 'controllerPause'>): NodeDot {
  if (n.state === 'pending') return 'pending'
  if (n.controllerPause === 'paused') return 'paused'
  if (n.controllerPause === 'draining') return 'draining'
  return n.online ? 'online' : 'offline'
}

/** Host part of a controller URL or label, for "is this the window's server". */
export function controllerHost(s: string | null | undefined): string {
  if (!s) return ''
  return s
    .trim()
    .toLowerCase()
    .replace(/^[a-z]+:\/\//, '')
    .replace(/\/.*$/, '')
    .replace(/:\d+$/, '')
}
