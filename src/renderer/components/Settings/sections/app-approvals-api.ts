// Settings → Cores → App Approvals (prd-k2-app-runtime-v1 §5.6, 0.46.1
// door). Typed bindings for `/cli/app/approvals|approve|deny` plus the
// plain-words helpers the cards use. Thin client: the daemon owns every
// held request, its expiry and what approving applies; this file reads that
// truth and sends the owner's gestures. Approving always takes a fresh vault
// owner confirm (action `app-approve`, bound to the request id) through
// `vaultApi.confirmStart/confirmFinish`, the same flow Vault and node admin use.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'

/** The vault confirm action the daemon checks on approve. */
export const APP_APPROVE_CONFIRM_ACTION = 'app-approve'

export type AppRequestKind =
  | 'config'
  | 'publish_run'
  | 'app_key'
  | 'clean_mode'
  | 'egress'
  | 'egress_mode'
  | 'deploy'
  | 'install'
  | 'db_raw'
  | 'overcommit'
  | 'manage'
  | 'quota'

export type AppRequestStatus = 'pending' | 'approved' | 'denied' | 'withdrawn' | 'expired' | 'failed'

export interface AppRequest {
  id: string
  kind: AppRequestKind | string
  appId: string | null
  serviceId: string | null
  appLabel: string
  workspaceId: string
  workspaceHandle?: string | null
  body: unknown
  /** Plain-word lines, or a kind-shaped object (see `diffLines`). */
  diff: unknown
  summary: string
  /** Why it was held (the classifier's words). */
  reasons: string[]
  /** The requester's own why. */
  reason: string | null
  requestedBy: string
  requesterWorkspace: string | null
  /** Unix seconds. */
  requestedAt: number
  /** Unix seconds. */
  expiresAt: number
  status: AppRequestStatus | string
  decidedBy: string | null
  decidedAt: number | null
  confirmVia: string | null
  decisionReason: string | null
  result: unknown
  /** Egress kinds: the hosts asked for, for display. */
  hosts?: string[]
}

export interface OwnerConfirmStatus {
  available: boolean
  methods: string[]
  hint?: string
}

export interface AppApprovalsList {
  requests: AppRequest[]
  ownerConfirm: OwnerConfirmStatus
}

export interface AppApproveResult {
  ok: boolean
  request: AppRequest
  status: string
  result?: unknown
  note?: string
  error?: { code: string; hint: string }
}

export async function fetchAppApprovals(state: 'pending' | 'all'): Promise<AppApprovalsList> {
  const res = await daemonCliGet<Partial<AppApprovalsList>>(primaryScope(), 'app/approvals', { state })
  const oc = res?.ownerConfirm
  return {
    requests: Array.isArray(res?.requests) ? res.requests : [],
    ownerConfirm: {
      available: oc?.available === true,
      methods: Array.isArray(oc?.methods) ? oc.methods : [],
      ...(typeof oc?.hint === 'string' ? { hint: oc.hint } : {}),
    },
  }
}

export async function approveAppRequest(request: string, confirmToken: string): Promise<AppApproveResult> {
  return daemonCliPost<AppApproveResult>(primaryScope(), 'app/approve', { request, confirmToken })
}

export async function denyAppRequest(request: string, reason?: string): Promise<{ ok: boolean; status: string }> {
  return daemonCliPost(primaryScope(), 'app/deny', reason ? { request, reason } : { request })
}

// ── plain words ──────────────────────────────────────────────────────

export const KIND_LABELS: Record<string, string> = {
  config: 'Config',
  publish_run: 'Publish run',
  app_key: 'App key',
  clean_mode: 'Clean mode',
  egress: 'Egress',
  egress_mode: 'Egress mode',
  deploy: 'Deploy',
  install: 'Install',
  db_raw: 'Raw database',
  overcommit: 'Overcommit',
  manage: 'Manage',
  quota: 'Quota',
}

export function kindLabel(kind: string): string {
  return KIND_LABELS[kind] ?? kind
}

function strList(v: unknown): string[] {
  return Array.isArray(v) ? v.map((x) => (typeof x === 'string' ? x : JSON.stringify(x))) : []
}

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

/** The change in plain words, one line each. Deploy/install: "+ cap X"
 *  lines (and "First install" when `diff.first`); egress: "+ host X";
 *  quota: the raised limits; anything else: its `lines`. */
export function diffLines(r: Pick<AppRequest, 'diff' | 'hosts'>): string[] {
  const d = r.diff
  if (Array.isArray(d)) return strList(d)
  if (typeof d === 'string') return d ? [d] : []
  if (!isObj(d)) return []
  const out: string[] = []
  if (d.first === true) out.push('First install')
  const given = strList(d.lines)
  const caps = isObj(d.caps) ? strList(d.caps.add) : []
  if (given.length > 0) out.push(...given)
  else if (caps.length > 0) out.push(...caps.map((c) => `+ cap ${c}`))
  if (isObj(d.egress)) {
    const hosts = Array.isArray(r.hosts) && r.hosts.length > 0 ? r.hosts : strList(d.egress.add)
    out.push(...hosts.map((h) => `+ host ${h}`))
  }
  if (typeof d.egressMode === 'string') out.push(`Egress mode → ${d.egressMode}`)
  if (given.length === 0 && isObj(d.quotas) && isObj(d.quotas.from) && isObj(d.quotas.to)) {
    const from = d.quotas.from
    for (const [k, to] of Object.entries(d.quotas.to)) {
      if (JSON.stringify(from[k]) !== JSON.stringify(to)) out.push(`${k}: ${String(from[k])} → ${String(to)}`)
    }
  }
  if (out.length === 0 && Object.keys(d).length > 0) out.push(JSON.stringify(d))
  return out
}

/** Deploy/install: every cap the App holds once this is approved. */
export function capsAfter(r: Pick<AppRequest, 'diff'>): string[] {
  const d = r.diff
  return isObj(d) && isObj(d.caps) ? strList(d.caps.approvedAfter) : []
}

/** "expires in 6d" / "expires in 3h" / "expired". Both args unix seconds. */
export function expiryText(expiresAt: number, nowSec: number): string {
  const left = expiresAt - nowSec
  if (left <= 0) return 'expired'
  if (left < 3600) return `expires in ${Math.max(1, Math.round(left / 60))}m`
  if (left < 86400) return `expires in ${Math.round(left / 3600)}h`
  return `expires in ${Math.round(left / 86400)}d`
}

/** The error a failed apply left on the request (`result.{code,hint}`). */
export function resultError(r: Pick<AppRequest, 'result'>): string | null {
  const res = r.result
  if (!isObj(res)) return null
  if (typeof res.hint === 'string' && res.hint) return res.hint
  if (typeof res.code === 'string' && res.code) return res.code
  return null
}
