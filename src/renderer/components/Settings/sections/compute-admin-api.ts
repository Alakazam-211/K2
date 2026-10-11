// Settings → Compute, node admin (prd-k2-node-admin-v1 §10, §13, S10) —
// typed bindings for `/cli/compute/admin/*` and `/cli/compute/nodes/{health,
// update,rollback,alerts}`. Thin client: the daemon owns requests, health,
// alerts and the signed commands; this file reads that truth and sends the
// owner's gestures. Approving always takes a fresh owner confirm (vault
// confirm action `node-admin`, bound to the request id): Touch ID / login
// password on the Mac, the Connect or vault password elsewhere.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'

export type AdminRequestState = 'pending' | 'approved' | 'denied' | 'withdrawn' | 'expired' | 'sent' | 'done' | 'failed'

export interface AdminStep {
  verb: string
  args?: Record<string, unknown>
}

export interface AdminRequest {
  id: string
  nodeId: string
  node: string | null
  kind: 'command' | 'plan' | 'service'
  class: 'operate' | 'privileged'
  /** `{verb, args}` or `{steps: [{verb, args}]}`. */
  body: { verb?: string; args?: Record<string, unknown>; steps?: AdminStep[] }
  digest: string
  summary: string
  reason: string | null
  proposedBy: string
  proposerWorkspace: string | null
  proposerWorkspaceName?: string
  proposedAt: number
  state: AdminRequestState
  decidedBy: string | null
  decidedVia: string | null
  decidedAt: number | null
  denyReason: string | null
  expiresAt: number
  cmdIds: string[]
  nextStep: number
  result: Record<string, unknown> | null
}

export interface AdminRequestsList {
  requests: AdminRequest[]
  /** The caller is the owner (only the owner approves). */
  canApprove: boolean
}

/** Health facts as the node reports them (`k2_node_proto::health::Health`,
 *  snake_case: the wire's names). A fact the node couldn't read is null. */
export interface NodeHealthFacts {
  at?: number
  boot_id?: string
  uptime_secs?: number | null
  load_milli?: [number, number, number] | null
  cores?: number
  mem_total_bytes?: number
  mem_available_bytes?: number | null
  swap_used_bytes?: number | null
  /** `normal` | `warn` | `critical`. */
  memory_pressure?: string | null
  disks?: { name: string; path: string; total_bytes: number; free_bytes: number }[]
  updates?: { pending?: number | null; security?: number | null; reboot_required?: boolean | null; checked_at?: number; source?: string } | null
  sleep?: { sleep?: number | null; autorestart?: number | null; targets_masked?: boolean | null } | null
  power?: { has_battery: boolean; on_battery: boolean }
  thermal?: { cpu_speed_limit?: number | null; max_zone_milli_c?: number | null } | null
  /** `on` | `off` | `unknown` | `luks`. */
  disk_encryption?: string | null
  os?: { name: string; version: string; kernel: string; arch: string }
  executor?: { reachable: boolean; version?: string | null; remote_admin?: string | null; approval_key_fps?: string[]; auto_update?: string | null; last_update?: string | null }
  stuck_jobs?: { job_id: string; generation: number; quiet_secs: number }[]
}

export interface NodeUpdateState {
  state: string
  version?: string | null
  reason?: string | null
  running?: string | null
  hasPrev?: boolean | null
  at?: number | null
}

export interface NodeHealthView {
  node: string
  nodeId: string
  online: boolean
  lastSeen: number | null
  nodeVersion: string | null
  at: number | null
  facts: NodeHealthFacts | null
  remoteAdmin: string | null
  approvalKeyFp: string | null
  alertAfterSecs: number | null
  updateRing: string
  update: NodeUpdateState | null
  alertsTo: string | null
  pendingApprovals: number
  features: string[]
}

export async function fetchAdminRequests(state: 'open' | 'pending' | 'all' = 'open'): Promise<AdminRequestsList> {
  const res = await daemonCliGet<Partial<AdminRequestsList>>(primaryScope(), 'compute/admin/requests', { state })
  return {
    requests: Array.isArray(res?.requests) ? res.requests : [],
    canApprove: res?.canApprove === true,
  }
}

export async function denyAdminRequest(request: string, why?: string): Promise<{ ok: boolean; request: AdminRequest }> {
  return daemonCliPost(primaryScope(), 'compute/admin/deny', why ? { request, why } : { request })
}

export async function approveAdminRequest(
  request: string,
  confirmToken: string,
): Promise<{ ok: boolean; request: AdminRequest; sent: boolean; note: string }> {
  return daemonCliPost(primaryScope(), 'compute/admin/approve', { request, confirmToken })
}

/** Which confirm methods this caller has (`local-auth` on the Mac). */
export async function startAdminConfirm(request: string): Promise<{ methods: string[] }> {
  return daemonCliPost(primaryScope(), 'vault/confirm/start', { action: 'node-admin', secretId: request })
}

/** A one-use, 60-second confirm token bound to `request`. */
export async function finishAdminConfirm(
  request: string,
  method: string,
  label: string,
  password?: string,
): Promise<{ token: string }> {
  return daemonCliPost(primaryScope(), 'vault/confirm/finish', {
    action: 'node-admin',
    secretId: request,
    method,
    label: label.slice(0, 64),
    ...(password !== undefined ? { password } : {}),
  })
}

export async function fetchNodeHealth(node: string): Promise<NodeHealthView> {
  return daemonCliGet<NodeHealthView>(primaryScope(), 'compute/nodes/health', { node })
}

export async function updateNode(node: string, canary = false): Promise<{ ok: boolean; update: NodeUpdateState; note: string }> {
  return daemonCliPost(primaryScope(), 'compute/nodes/update', canary ? { node, canary: true } : { node })
}

export async function rollbackNode(node: string): Promise<{ ok: boolean; update: NodeUpdateState; note: string }> {
  return daemonCliPost(primaryScope(), 'compute/nodes/rollback', { node })
}

/** Offline alert after `afterSecs` (null = off) for one node. */
export async function setNodeOfflineAlert(node: string, afterSecs: number | null): Promise<{ ok: boolean; alertAfterSecs: number | null }> {
  return daemonCliPost(primaryScope(), 'compute/nodes/alerts', { node, after: afterSecs === null ? 'off' : afterSecs })
}

/** The workspace (an IT agent) alerts go to; null = nobody. */
export async function setAlertsTo(workspace: string | null): Promise<{ ok: boolean; alertsTo: string | null }> {
  return daemonCliPost(primaryScope(), 'compute/nodes/alerts', { to: workspace ?? 'none' })
}

/** Steps of a request, for the plain-words view. */
export function requestSteps(r: Pick<AdminRequest, 'body'>): AdminStep[] {
  if (Array.isArray(r.body?.steps)) return r.body.steps
  if (r.body?.verb) return [{ verb: r.body.verb, args: r.body.args ?? {} }]
  return []
}

/** The full script text of any `script` step (shown before approving). */
export function requestScripts(r: Pick<AdminRequest, 'body'>): { interpreter: string; text: string }[] {
  return requestSteps(r)
    .filter((s) => s.verb === 'script')
    .map((s) => ({
      interpreter: typeof s.args?.interpreter === 'string' ? s.args.interpreter : '/bin/sh',
      text: typeof s.args?.text === 'string' ? s.args.text : '',
    }))
}

/** Who proposed it, for people: `agent:it` → "agent it". */
export function proposerLabel(r: Pick<AdminRequest, 'proposedBy' | 'proposerWorkspaceName'>): string {
  if (r.proposerWorkspaceName) return `agent ${r.proposerWorkspaceName}`
  return r.proposedBy.replace(/^login:/, '').replace(/^agent:/, 'agent ')
}

/** Health one-liners ("9 GB free of 500 GB on /"). */
export function healthLines(f: NodeHealthFacts | null): string[] {
  if (!f) return []
  const gb = (b: number | null | undefined): string => (typeof b === 'number' ? `${Math.floor(b / 2 ** 30)} GB` : '?')
  const out: string[] = []
  if (typeof f.uptime_secs === 'number') {
    const d = Math.floor(f.uptime_secs / 86400)
    const h = Math.floor((f.uptime_secs % 86400) / 3600)
    out.push(`Up ${d}d ${h}h`)
  }
  for (const d of f.disks ?? []) out.push(`${gb(d.free_bytes)} free of ${gb(d.total_bytes)} on ${d.path}`)
  if (typeof f.mem_total_bytes === 'number' && f.mem_total_bytes > 0) {
    out.push(`${gb(f.mem_available_bytes)} of ${gb(f.mem_total_bytes)} memory available${f.memory_pressure ? ` (pressure ${f.memory_pressure})` : ''}`)
  }
  if (f.disk_encryption) {
    out.push(`Disk encryption ${f.disk_encryption}${f.disk_encryption === 'on' ? ': after a power cut someone types the password at the machine' : ''}`)
  }
  const autorestart = f.sleep?.autorestart
  if (typeof autorestart === 'number') out.push(`Power back on after a cut: ${autorestart === 1 ? 'yes' : 'no'}`)
  if (f.power?.has_battery) out.push(f.power.on_battery ? 'On battery' : 'On AC power')
  const u = f.updates
  if (u && typeof u.pending === 'number') out.push(`${u.pending} update(s) pending${u.security ? `, ${u.security} security` : ''}`)
  if (u?.reboot_required) out.push('Reboot required')
  const limit = f.thermal?.cpu_speed_limit
  if (typeof limit === 'number' && limit < 100) out.push(`CPU held to ${limit}% by heat`)
  if (f.executor && !f.executor.reachable) out.push("The admin executor isn't answering")
  for (const j of f.stuck_jobs ?? []) out.push(`Job ${j.job_id.slice(0, 8)} looks stuck (quiet ${Math.round(j.quiet_secs / 60)} min)`)
  return out
}
