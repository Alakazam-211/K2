// Garden v3 data sources — one typed call per `/cli/sources/*` route
// (contract-sources-v1 §4.2, §4.3). Every call takes the `ServerScope` of
// the server that owns the source (Settings passes `primaryScope()`, the
// Garden reader the scope for an address's host). Written in C0; read-only
// for B2/B3 (changes go through the integrator).
//
// GET rows go through `daemonCliGet`, POST rows through `daemonCliPost`
// (JSON body). Errors throw like every daemon call; the envelope is
// `SourcesErrorEnvelope`.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import type {
  AgentAccess,
  FireRow,
  Keep,
  SimulateState,
  SourceApproveBody,
  SourceApproveResponse,
  SourceBatchApproveBody,
  SourceBatchApproveResponse,
  SourceEditResponse,
  SourceExport,
  SourceHistoryResponse,
  SourceMuteResponse,
  SourcePinResponse,
  SourcePresetBody,
  SourceProposeResponse,
  SourceRefreshResponse,
  SourceRemoveResponse,
  SourceResult,
  SourceSecretResponse,
  SourceShowResponse,
  SourceSimulateResponse,
  SourceStatusChange,
  SourceTryResponse,
  SourcesChangesResponse,
  SourcesListResponse,
  SourcesPendingResponse,
  SourcesStatusResponse,
  TriggerAddBody,
  TriggerChangeResponse,
  TriggerProposeBody,
  TriggerTestResponse,
  TriggerView,
  Viewers,
} from './sources-types'

// ── reads (Member) ────────────────────────────────────────────────────

export function listSources(scope: ServerScope): Promise<SourcesListResponse> {
  return daemonCliGet<SourcesListResponse>(scope, 'sources/list')
}

export function showSource(scope: ServerScope, name: string): Promise<SourceShowResponse> {
  return daemonCliGet<SourceShowResponse>(scope, 'sources/show', { name })
}

export function readSource(scope: ServerScope, name: string): Promise<SourceResult> {
  return daemonCliGet<SourceResult>(scope, 'sources/read', { name })
}

/** `fromAt` / `toAt` in seconds; default the last 48 h. */
export function sourceHistory(
  scope: ServerScope,
  name: string,
  field: string,
  opts: { fromAt?: number; toAt?: number } = {},
): Promise<SourceHistoryResponse> {
  return daemonCliGet<SourceHistoryResponse>(scope, 'sources/history', {
    name,
    field,
    from: opts.fromAt,
    to: opts.toAt,
  })
}

/** Card views changed since `rev` (0 = everything readable). */
export function sourcesChanges(scope: ServerScope, since: number): Promise<SourcesChangesResponse> {
  return daemonCliGet<SourcesChangesResponse>(scope, 'sources/changes', { since })
}

export function exportSource(scope: ServerScope, name: string): Promise<SourceExport> {
  return daemonCliGet<SourceExport>(scope, 'sources/export', { name })
}

export function sourcesStatus(scope: ServerScope): Promise<SourcesStatusResponse> {
  return daemonCliGet<SourcesStatusResponse>(scope, 'sources/status')
}

// ── definitions ───────────────────────────────────────────────────────

export function proposeSource(scope: ServerScope, body: SourceExport | SourcePresetBody): Promise<SourceProposeResponse> {
  return daemonCliPost<SourceProposeResponse>(scope, 'sources/propose', body)
}

/** People only. With `viewers` / `agentAccess` / `keep` from the add form. */
export function addSource(
  scope: ServerScope,
  body: (SourceExport | SourcePresetBody) & { viewers?: Viewers; agentAccess?: AgentAccess; keep?: Keep },
): Promise<SourceProposeResponse> {
  return daemonCliPost<SourceProposeResponse>(scope, 'sources/add', body)
}

export function editSource(
  scope: ServerScope,
  name: string,
  set: Partial<Omit<SourceExport, 'name'>>,
): Promise<SourceEditResponse> {
  return daemonCliPost<SourceEditResponse>(scope, 'sources/edit', { name, set })
}

export function removeSource(scope: ServerScope, name: string): Promise<SourceRemoveResponse> {
  return daemonCliPost<SourceRemoveResponse>(scope, 'sources/remove', { name })
}

export function pauseSource(scope: ServerScope, name: string): Promise<SourceStatusChange> {
  return daemonCliPost<SourceStatusChange>(scope, 'sources/pause', { name })
}

export function resumeSource(scope: ServerScope, name: string): Promise<SourceStatusChange> {
  return daemonCliPost<SourceStatusChange>(scope, 'sources/resume', { name })
}

/** `forSecs` in seconds (a person: up to 86400). */
export function muteSource(scope: ServerScope, name: string, forSecs: number, reason?: string): Promise<SourceMuteResponse> {
  return daemonCliPost<SourceMuteResponse>(scope, 'sources/mute', { name, forSecs, reason })
}

export function unmuteSource(scope: ServerScope, name: string): Promise<SourceMuteResponse> {
  return daemonCliPost<SourceMuteResponse>(scope, 'sources/unmute', { name })
}

/** At most once per 15 s per source (429 `rate_limited`). */
export function refreshSource(scope: ServerScope, name: string): Promise<SourceRefreshResponse> {
  return daemonCliPost<SourceRefreshResponse>(scope, 'sources/refresh', { name })
}

// ── Try and simulate ──────────────────────────────────────────────────

/** A draft (export shape, `name`/`label` optional) or `{ name }` of a saved source. */
export function trySource(
  scope: ServerScope,
  body: Partial<SourceExport> | SourcePresetBody | { name: string },
): Promise<SourceTryResponse> {
  return daemonCliPost<SourceTryResponse>(scope, 'sources/try', body)
}

/** `state: 'off'` clears it. `forSecs` in seconds (default 3600). */
export function simulateSource(
  scope: ServerScope,
  name: string,
  state: SimulateState | 'off',
  forSecs?: number,
): Promise<SourceSimulateResponse> {
  return daemonCliPost<SourceSimulateResponse>(scope, 'sources/simulate', { name, state, forSecs })
}

// ── approvals, secrets, pins (people) ─────────────────────────────────

export function pendingSources(scope: ServerScope, names?: string[]): Promise<SourcesPendingResponse> {
  return daemonCliGet<SourcesPendingResponse>(scope, 'sources/pending', names?.length ? { names: names.join(',') } : undefined)
}

export function approveSource(scope: ServerScope, body: SourceApproveBody): Promise<SourceApproveResponse> {
  return daemonCliPost<SourceApproveResponse>(scope, 'sources/approve', body)
}

export function batchApproveSources(scope: ServerScope, body: SourceBatchApproveBody): Promise<SourceBatchApproveResponse> {
  return daemonCliPost<SourceBatchApproveResponse>(scope, 'sources/batch-approve', body)
}

/** `secret: null` unbinds (no confirm). Binding needs a `source-secret` confirm. */
export function setSourceSecret(
  scope: ServerScope,
  name: string,
  secret: string | null,
  opts: { confirmToken?: string; useAnyway?: boolean } = {},
): Promise<SourceSecretResponse> {
  return daemonCliPost<SourceSecretResponse>(scope, 'sources/secret', { name, secret, ...opts })
}

export function pinSource(scope: ServerScope, name: string): Promise<SourcePinResponse> {
  return daemonCliPost<SourcePinResponse>(scope, 'sources/pin', { name })
}

export function unpinSource(scope: ServerScope, name: string): Promise<SourcePinResponse> {
  return daemonCliPost<SourcePinResponse>(scope, 'sources/unpin', { name })
}

// ── triggers ──────────────────────────────────────────────────────────

/** A source's triggers, or every trigger without `source`. */
export async function listTriggers(scope: ServerScope, source?: string): Promise<TriggerView[]> {
  const r = await daemonCliGet<{ triggers?: TriggerView[] }>(scope, 'sources/triggers/list', source ? { source } : undefined)
  return r.triggers ?? []
}

export function showTrigger(scope: ServerScope, id: string): Promise<{ trigger: TriggerView; fires: FireRow[] }> {
  return daemonCliGet<{ trigger: TriggerView; fires: FireRow[] }>(scope, 'sources/triggers/show', { id })
}

export async function triggerLog(scope: ServerScope, id: string, limit?: number): Promise<FireRow[]> {
  const r = await daemonCliGet<{ fires?: FireRow[] }>(scope, 'sources/triggers/log', { id, limit })
  return r.fires ?? []
}

export function proposeTrigger(scope: ServerScope, body: TriggerProposeBody): Promise<{ id: string; status: 'proposed' }> {
  return daemonCliPost<{ id: string; status: 'proposed' }>(scope, 'sources/triggers/propose', body)
}

export function addTrigger(scope: ServerScope, body: TriggerAddBody): Promise<{ id: string; status: 'active' }> {
  return daemonCliPost<{ id: string; status: 'active' }>(scope, 'sources/triggers/add', body)
}

export function editTrigger(
  scope: ServerScope,
  id: string,
  set: Partial<Omit<TriggerAddBody, 'source'>>,
): Promise<TriggerChangeResponse> {
  return daemonCliPost<TriggerChangeResponse>(scope, 'sources/triggers/edit', { id, set })
}

export function approveTrigger(scope: ServerScope, id: string): Promise<TriggerChangeResponse> {
  return daemonCliPost<TriggerChangeResponse>(scope, 'sources/triggers/approve', { id })
}

export function pauseTrigger(scope: ServerScope, id: string): Promise<TriggerChangeResponse> {
  return daemonCliPost<TriggerChangeResponse>(scope, 'sources/triggers/pause', { id })
}

export function resumeTrigger(scope: ServerScope, id: string): Promise<TriggerChangeResponse> {
  return daemonCliPost<TriggerChangeResponse>(scope, 'sources/triggers/resume', { id })
}

export function removeTrigger(scope: ServerScope, id: string): Promise<TriggerChangeResponse> {
  return daemonCliPost<TriggerChangeResponse>(scope, 'sources/triggers/remove', { id })
}

/** At most 5 an hour per trigger (429). */
export function testTrigger(scope: ServerScope, id: string): Promise<TriggerTestResponse> {
  return daemonCliPost<TriggerTestResponse>(scope, 'sources/triggers/test', { id })
}
