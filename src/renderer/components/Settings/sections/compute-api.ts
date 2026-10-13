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
}

export interface GrantLimits {
  maxJobSecs?: number
  maxDiskGb?: number
  maxParallel?: number
  maxQueued?: number
}

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
}

/** Without `controller` + (`enroll` | `ticket`) the daemon only installs. */
export interface LocalEnableBody {
  controller?: string
  enroll?: string
  ticket?: string
  name?: string
  /** Always sent from Settings: an open home with no choice is refused. */
  privateHome: boolean
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
  limits: GrantLimits = {},
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
