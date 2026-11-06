// Settings → Compute, package requests (prd-compute-self-serve-environments-v1
// §6, §12.3). Typed bindings for `/cli/compute/nodes/packages*`. Thin client:
// the daemon owns requests, the dry run, policies and who may decide; this
// file reads that truth, groups rows by batch for display and sends the
// owner's Allow / Deny.

import { listen } from '@tauri-apps/api/event'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import { controllerHost, type ComputeNode, type LocalStatus } from './compute-api'

export type PkgRequestState = 'pending' | 'approved' | 'installed' | 'denied' | 'failed' | 'expired'

/** What the node's dry run found (snake_case: the wire's own names). */
export interface PkgDrySummary {
  installs: string[]
  size_mb?: number | null
  resolved?: string[]
  already?: string[]
  /** A guard that would refuse it now (`package_denied: sudo`). */
  refusal?: string | null
}

export interface PkgRequestRow {
  id: string
  batchId: string
  /** own = filed here for our node; sent = we asked another server's node;
   *  fwd = another server asked, through our node. */
  origin: 'own' | 'sent' | 'fwd' | string
  node: string
  nodeRequest: string | null
  /** `own`, or the asking server's label. */
  server: string
  workspaceId: string | null
  jobId: string | null
  askedBy: string
  why: string | null
  names: string[]
  dry: PkgDrySummary | null
  /** One line: "Ubuntu 24.04.1 LTS · apt · …". */
  host: string | null
  state: PkgRequestState | string
  addToList: boolean
  decidedBy: string | null
  decidedAt?: number | null
  note: string | null
  createdAt: number
  expiresAt: number
}

export interface PkgRequestsList {
  requests: PkgRequestRow[]
  /** The caller is the owner or an Admin, in person. */
  canDecide: boolean
}

export interface NodePackagesView {
  node: string
  host: Record<string, unknown> | null
  hostLine: string | null
  packages: {
    manager: string | null
    level: string
    profiles: string[]
    maxInstallMb: number
    policy: string
    approved: string[]
    installed: string[]
    indexAgeSecs: number | null
    openRequests: number
  } | null
  takesRequests: boolean
  owner: string
  requests: PkgRequestRow[]
  note: string | null
}

export interface PkgDecideResult {
  request: string
  rows: { request: string; node: string; state: string; note?: string }[]
}

// ── Routes ──────────────────────────────────────────────────────────────

export async function fetchPkgRequests(
  state: 'pending' | 'all' = 'pending',
  opts: { node?: string; limit?: number } = {},
): Promise<PkgRequestsList> {
  const res = await daemonCliGet<Partial<PkgRequestsList>>(primaryScope(), 'compute/nodes/packages/requests', {
    state,
    node: opts.node,
    limit: opts.limit,
  })
  return {
    requests: Array.isArray(res?.requests) ? res.requests : [],
    canDecide: res?.canDecide === true,
  }
}

/** Allow (once, or `add` to the approved list) or deny one batch. */
export async function decidePkgRequest(
  batchId: string,
  approve: boolean,
  opts: { add?: boolean; note?: string } = {},
): Promise<PkgDecideResult> {
  const body: Record<string, unknown> = { request: batchId, approve }
  if (approve && opts.add) body.add = true
  if (opts.note) body.note = opts.note
  return daemonCliPost(primaryScope(), 'compute/nodes/packages/decide', body)
}

export async function fetchNodePackages(node: string): Promise<NodePackagesView> {
  return daemonCliGet<NodePackagesView>(primaryScope(), 'compute/nodes/packages', { node })
}

// ── Display helpers (pure) ──────────────────────────────────────────────

/** "agent tests" for our own agents, "acme.k2.dev's tests" for another server's. */
export function pkgAskerLabel(r: Pick<PkgRequestRow, 'server' | 'askedBy'>): string {
  return r.server === 'own' ? `agent ${r.askedBy}` : `${r.server}'s ${r.askedBy}`
}

export interface PkgBatch {
  batchId: string
  rows: PkgRequestRow[]
  /** Every name asked for, sorted, once each. */
  names: string[]
  /** One entry per node, with its host line when known. */
  nodes: { node: string; host: string | null }[]
  who: string
  why: string | null
  /** Packages the install pulls in (dependencies included), across nodes. */
  deps: number
  sizeMb: number
  refusals: string[]
  /** Asked by another server: the install is still host-wide. */
  otherServer: boolean
  createdAt: number
}

/** One card per batch, in the order the batches first appear. */
export function groupPkgBatches(rows: PkgRequestRow[]): PkgBatch[] {
  const byId = new Map<string, PkgRequestRow[]>()
  for (const r of rows) {
    const key = r.batchId || r.id
    const list = byId.get(key)
    if (list) list.push(r)
    else byId.set(key, [r])
  }
  const out: PkgBatch[] = []
  for (const [batchId, list] of byId) {
    const first = list[0]
    const names = [...new Set(list.flatMap((r) => r.names))].sort()
    const nodes: PkgBatch['nodes'] = []
    for (const r of list) {
      if (!nodes.some((n) => n.node === r.node)) nodes.push({ node: r.node, host: r.host })
    }
    const refusals = [...new Set(list.map((r) => r.dry?.refusal).filter((x): x is string => typeof x === 'string' && x !== ''))]
    out.push({
      batchId,
      rows: list,
      names,
      nodes,
      who: pkgAskerLabel(first),
      why: list.find((r) => r.why)?.why ?? null,
      deps: list.reduce((n, r) => n + (r.dry?.installs?.length ?? 0), 0),
      sizeMb: list.reduce((n, r) => n + (typeof r.dry?.size_mb === 'number' ? r.dry.size_mb : 0), 0),
      refusals,
      otherServer: list.some((r) => r.server !== 'own'),
      createdAt: Math.min(...list.map((r) => r.createdAt)),
    })
  }
  return out
}

/** "42 packages with dependencies, 180 MB", or '' before a dry run. */
export function pkgDepsLine(b: Pick<PkgBatch, 'deps' | 'sizeMb'>): string {
  if (b.deps <= 0) return ''
  return `${b.deps} ${b.deps === 1 ? 'package' : 'packages'} with dependencies${b.sizeMb > 0 ? `, ${b.sizeMb} MB` : ''}`
}

/** Which of the focused server's nodes is this machine: the node key's
 *  fingerprint first, then the same name under the same controller. Null
 *  when this machine isn't one of them (or a node that old doesn't say). */
export function findLocalNode(
  status: LocalStatus | null,
  nodes: ComputeNode[],
  controllerLabel: string | null,
): ComputeNode | null {
  const me = status?.node
  if (!me) return null
  const live = nodes.filter((n) => n.state !== 'revoked')
  if (me.node_fp) {
    const byFp = live.find((n) => n.fingerprint === me.node_fp)
    if (byFp) return byFp
  }
  const sameController =
    !!me.controller && !!controllerLabel && controllerHost(me.controller) === controllerHost(controllerLabel)
  if (sameController && me.name) return live.find((n) => n.name === me.name) ?? null
  return null
}

// ── Live updates ────────────────────────────────────────────────────────

/** True for a `sync:settings` payload about compute (`{compute: "…"}`). */
export function isComputeSync(payload: unknown): boolean {
  return !!payload && typeof payload === 'object' && 'compute' in (payload as Record<string, unknown>)
}

/** Re-read when the daemon says something on Compute changed (a package
 *  request filed or decided, an admin request…). Returns the unlisten. */
export function onComputeSync(fn: (kind: string) => void): () => void {
  let off: (() => void) | null = null
  let cancelled = false
  void listen<unknown>('sync:settings', (e) => {
    if (isComputeSync(e.payload)) fn(String((e.payload as { compute: unknown }).compute))
  })
    .then((u) => {
      if (cancelled) u()
      else off = u
    })
    .catch((err) => console.warn('[compute] sync:settings listen failed', err))
  return () => {
    cancelled = true
    off?.()
  }
}
