// Garden v3 data sources — wire types (contract-sources-v1 §3.3, §4.3,
// §4.4, §6). TS mirrors of the daemon's JSON (camelCase). Written in C0;
// read-only for B2/B3 (changes go through the integrator).
//
// Times are integer SECONDS since the Unix epoch (UTC); durations are
// integer seconds. Ages on screen come from `serverNow - updatedAt`, never
// the viewer's clock.

export type Health = 'ok' | 'warn' | 'crit' | 'unknown'
export type Phase = 'pending' | 'live' | 'stale' | 'paused'
/** `pending` | `paused` | `stale` | the health. */
export type Display = 'pending' | 'paused' | 'stale' | Health
export type SourceKind = 'http-json'
export type SourceStatus = 'proposed' | 'active' | 'paused' | 'removed'
export type Viewers = 'member' | 'admin'
export type AgentAccess = 'values' | 'shape'
export type Keep = 'values' | 'states'
export type OnTargetError = 'crit' | 'unknown'
export type SimulateState = 'ok' | 'warn' | 'crit' | 'stale' | 'unknown' | 'slow'
export type GapKind = 'host_slept' | 'daemon_restarted'
export type Approval = 'one_click' | 'needs_confirm' | 'owner_only'

/** Fixed reasons; never derived from response text. */
export type Reason =
  | 'http_auth'
  | 'http_404'
  | 'http_4xx'
  | 'http_5xx'
  | 'timeout'
  | 'connect'
  | 'dns'
  | 'tls'
  | 'blocked_address'
  | 'airgap'
  | 'host_slept'
  | 'daemon_restarted'
  | 'too_large'
  | 'not_json'
  | 'rate_limited'
  | 'secret_missing'
  | 'secret_disabled'
  | 'vault_unavailable'
  | 'redirect_other_origin'
  | 'budget'
  | 'paused_by_person'

export type JsonValue = string | number | boolean | null | JsonValue[] | { [key: string]: JsonValue }

export interface Soft {
  count: number
  of: number
}

export interface Counts {
  ok: number
  warn: number
  crit: number
  unknown: number
}

export interface SourceGap {
  kind: GapKind
  /** Seconds. */
  from: number
  /** Seconds. */
  to: number
}

export interface SourceMuted {
  /** Seconds. */
  until: number
  by: string
  reason: string | null
}

export interface SourceSimulated {
  state: SimulateState
  /** Seconds. */
  until: number
  by: string | null
}

export interface ItemView {
  key: string
  health: Health
  reason: Reason | null
  /** Seconds: when this item went hard. */
  since: number | null
  soft: Soft
  fields: Record<string, JsonValue>
}

/** The card view: `sources.read`, `k2 sources read --json`, `show`'s
 *  `view`, `changes`. */
export interface SourceResult {
  name: string
  label: string
  kind: SourceKind
  status: SourceStatus
  health: Health
  phase: Phase
  display: Display
  reason: Reason | null
  reasonText: string | null
  httpStatus: number | null
  soft: Soft
  /** Seconds: the server's clock when it answered. */
  serverNow: number
  /** Seconds: last success. */
  updatedAt: number | null
  /** Seconds: last attempt. */
  checkedAt: number | null
  /** Seconds. */
  nextAt: number | null
  /** Seconds. */
  hardSince: number | null
  /** Seconds. */
  recoveredAt: number | null
  gap: SourceGap | null
  muted: SourceMuted | null
  simulated: SourceSimulated | null
  pinned: boolean
  keep: Keep
  /** Values replaced for this caller. */
  masked: boolean
  link: string | null
  /** `{}` for `keep: states`. */
  fields: Record<string, JsonValue>
  /** At most 200. */
  items: ItemView[]
  counts: Counts
  rev: number
}

// ── definitions (the export shape) ───────────────────────────────────

export interface FetchTarget {
  key: string
  url: string
}

export type FetchTargets =
  | { url: string }
  | { targets: FetchTarget[] }
  | { urlSecret: { name: string; origin: string } }

export type Cond =
  | { eq: JsonValue }
  | { ne: JsonValue }
  | { in: JsonValue[] }
  | { lt: number }
  | { lte: number }
  | { gt: number }
  | { gte: number }
  | { exists: boolean }

export interface Rule {
  field: string
  crit?: Cond | null
  warn?: Cond | null
  ok?: Cond | null
}

export type Aggregate =
  | { op: 'count' }
  | { op: 'countWhere'; field: string; cond: Cond }
  | { op: 'sum'; field: string }
  | { op: 'min'; field: string }
  | { op: 'max'; field: string }
  | { op: 'distinct'; field: string }

export interface ItemsSpec {
  pointer: string | null
  itemKey: string | null
  itemFields?: Record<string, string>
}

export interface Extract {
  fields?: Record<string, string>
  items?: ItemsSpec | null
  aggregates?: Record<string, Aggregate>
  trend?: string[]
}

/** `GET /cli/sources/export`; the body of `propose` / `add`. */
export interface SourceExport {
  name: string
  label: string
  kind?: SourceKind
  preset?: string | null
  fetch: FetchTargets
  headers?: Array<[string, string]>
  secret?: string | null
  /** Seconds. */
  everySecs: number
  /** Seconds. */
  timeoutSecs?: number
  maxBytes?: number
  extract?: Extract
  rules?: Rule[]
  failAfter?: number
  onTargetError?: OnTargetError
  link?: string | null
  agentAccess?: AgentAccess | null
  viewers?: Viewers | null
  keep?: Keep
}

/** A preset-based `propose` / `add` body. */
export interface SourcePresetBody {
  preset: string
  presetArgs?: Record<string, string>
  name: string
  label: string
  [override: string]: unknown
}

export interface SpecWarning {
  code: 'token_in_path' | 'root_key'
  field: string
  hint: string
}

/** `show`'s `source`: the export shape plus who/when. */
export interface SourceDef extends SourceExport {
  status: SourceStatus
  madeBy: string
  approvedBy: string | null
  /** Seconds. */
  approvedAt: number | null
  specHash: string
  /** Seconds. */
  createdAt: number
  /** Seconds. */
  updatedAt: number
  pinned: boolean
  secretBound: boolean
  /** Persons only: the vault confirm `secretId` for `source-secret`. */
  id?: string
}

// ── responses (§4.3) ──────────────────────────────────────────────────

export interface SourceListRow {
  name: string
  label: string
  status: SourceStatus
  madeBy: string
  /** The secret's NAME, never a value. */
  secret: string | null
  pinned: boolean
  keep: Keep
  viewers: Viewers
  agentAccess: AgentAccess
  /** Seconds. */
  everySecs: number
  targets: number
  triggers: number
  view: SourceResult
}

export interface SourcesListResponse {
  sources: SourceListRow[]
  /** Seconds. */
  serverNow: number
  rev: number
}

export interface SourceShowResponse {
  source: SourceDef
  view: SourceResult
  warnings: SpecWarning[]
}

export interface HistoryPoint {
  /** Seconds. */
  t: number
  /** null = a gap. */
  v: number | null
}

export interface SourceHistoryResponse {
  field: string
  res: 'raw' | 'hour'
  points: HistoryPoint[]
}

export interface SourcesChangesResponse {
  rev: number
  full: boolean
  changed: SourceResult[]
  removed: string[]
}

export interface ParkedOrigin {
  origin: string
  /** Seconds. */
  untilAt: number
  reason: Reason
}

export interface SourcesStatusResponse {
  sources: number
  active: number
  targets: number
  inFlight: number
  /** Seconds. */
  behindSecs: number
  parkedOrigins: ParkedOrigin[]
  lastGap: SourceGap | null
  airgap: boolean
}

export interface SourceProposeResponse {
  name: string
  status: SourceStatus
  specHash: string
  warnings: SpecWarning[]
  approval: Approval
}

export interface SourceEditResponse {
  name: string
  status: SourceStatus
  specHash: string
  warnings: SpecWarning[]
  unbound: boolean
}

export interface SourceRemoveResponse {
  removed: string
  /** Seconds. */
  heldUntilAt: number
  triggersRemoved: number
}

export interface SourceStatusChange {
  name: string
  status: SourceStatus
}

export interface SourceMuteResponse {
  name: string
  muted: SourceMuted | null
}

export interface SourceRefreshResponse {
  queued: boolean
  /** Seconds. */
  nextAt: number
}

export interface TryTreeNode {
  pointer: string
  type: 'object' | 'array' | 'string' | 'number' | 'bool' | 'null'
  len?: number
  sample?: JsonValue
}

export interface TryTargetResult {
  key: string
  status: number | null
  reason: Reason | null
  elapsedMs: number
}

export interface RootKeyHint {
  code: 'stripe_secret' | 'supabase_service' | 'cloudflare_global' | 'github_classic'
  hint: string
}

export interface SourceTryResponse {
  ok: boolean
  status: number | null
  elapsedMs: number
  reason: Reason | null
  truncated: boolean
  tree: TryTreeNode[]
  treeTruncated: boolean
  view: SourceResult | null
  warnings: SpecWarning[]
  rootKey: RootKeyHint | null
  /** Fan-out drafts: the first 3 targets. */
  targets?: TryTargetResult[]
}

export interface SourceSimulateResponse {
  name: string
  simulated: SourceSimulated | null
}

export interface PendingSource {
  name: string
  label: string
  madeBy: string
  origins: string[]
  /** Seconds. */
  everySecs: number
  targets: number
  secret: string | null
  urlSecret: string | null
  lan: boolean
  specHash: string
  approval: Approval
  /** Persons only: the vault confirm `secretId`. */
  id?: string
}

export interface PendingTrigger {
  id: string
  source: string
  on: TriggerOn
  agent: { projectId: string; handle: string }
  madeBy: string
}

export interface SourcesPendingResponse {
  sources: PendingSource[]
  triggers: PendingTrigger[]
}

export interface SourceApproveBody {
  name: string
  specHash: string
  secret?: string
  confirmToken?: string
  viewers?: Viewers
  agentAccess?: AgentAccess
  keep?: Keep
  useAnyway?: boolean
}

export interface SourceApproveResponse {
  name: string
  status: SourceStatus
  secretBound: boolean
  /** Seconds. */
  nextAt: number | null
}

export interface SourceBatchApproveBody {
  items: Array<{ name: string; specHash: string; secret?: string }>
  triggers: string[]
  confirmToken?: string
}

export interface SourceBatchApproveResponse {
  approved: string[]
  triggersApproved: string[]
}

export interface SourceSecretResponse {
  name: string
  secret: string | null
  secretBound: boolean
}

export interface SourcePinResponse {
  name: string
  pinned: boolean
}

// ── triggers ──────────────────────────────────────────────────────────

export type TriggerOn =
  | { kind: 'went_crit' }
  | { kind: 'went_warn' }
  | { kind: 'recovered' }
  | { kind: 'stale_for'; secs: number }
  | { kind: 'item_went_crit' }

export type TriggerStatus = 'proposed' | 'active' | 'paused' | 'undeliverable'

export interface TriggerView {
  id: string
  source: string
  on: TriggerOn
  agent: { projectId: string; handle: string; missing: boolean }
  note: string | null
  /** Seconds. */
  cooldownSecs: number
  dailyCap: number
  status: TriggerStatus
  madeBy: string
  /** Seconds. */
  lastFireAt: number | null
  firesToday: number
}

export type FireOutcome = 'pending' | 'delivered' | 'inbox' | 'undeliverable' | 'suppressed' | 'coalesced' | 'test'

export type SuppressReason =
  | 'cooldown'
  | 'daily_cap'
  | 'agent_cap'
  | 'muted'
  | 'simulated'
  | 'paused'
  | 'gap'
  | 'deduped'
  | 'draining'

/** Never note text. */
export interface FireRow {
  id: string
  /** Seconds. */
  at: number
  incidentId: string
  itemKey: string | null
  outcome: FireOutcome
  suppressReason: SuppressReason | null
  /** Seconds. */
  deliveredAt: number | null
  woke: boolean | null
  noteBytes: number | null
  detail: string | null
}

export interface TriggerProposeBody {
  source: string
  on: TriggerOn
  /** Name, handle, path or project id. */
  agent: string
  note?: string
}

export interface TriggerAddBody extends TriggerProposeBody {
  /** Seconds. */
  cooldownSecs?: number
  dailyCap?: number
}

/** `triggers/edit|approve|pause|resume|remove`. The contract fixes only
 *  the `{id}` request; B2's handlers may add fields. */
export interface TriggerChangeResponse {
  id: string
  status?: TriggerStatus
  [field: string]: unknown
}

export interface TriggerTestResponse {
  outcome: 'delivered' | 'inbox' | 'undeliverable'
  woke: boolean
}

// ── errors (§4.4) ─────────────────────────────────────────────────────

export type SourcesErrorCode =
  | 'bad_request'
  | 'bad_spec'
  | 'reason_required'
  | 'secret_origin_mismatch'
  | 'invalid_token'
  | 'apps_not_allowed'
  | 'role_required'
  | 'cap_not_granted'
  | 'person_only'
  | 'owner_only'
  | 'lan_owner_only'
  | 'not_own'
  | 'field_person_only'
  | 'other_agent'
  | 'too_long'
  /** The one confirm code (0.46.0): a vault/person confirm is needed. */
  | 'vault_confirm_required'
  /** The same, from a 0.45.x server (kept for one release). */
  | 'confirm_required'
  | 'confirm_failed'
  | 'source_not_found'
  | 'trigger_not_found'
  | 'agent_not_found'
  | 'secret_not_found'
  | 'method_not_allowed'
  | 'name_taken'
  | 'name_held'
  | 'too_many_sources'
  | 'too_many_targets'
  | 'spec_changed'
  | 'batch_changed'
  | 'root_key'
  | 'no_confirm_method'
  | 'no_history'
  | 'trigger_exists'
  | 'removed'
  | 'source_pinned'
  | 'garden_locked'
  | 'rate_limited'
  | 'not_built'
  | 'vault_unavailable'

/** `{"error": {"code", "hint", ...extra}}`. */
export interface SourcesErrorEnvelope {
  error: {
    code: SourcesErrorCode
    hint: string
    field?: string
    specCode?: string
    /** Seconds. */
    untilAt?: number
    /** Seconds. */
    removedAt?: number
    /** Seconds. */
    retryAfterSecs?: number
    role?: string
    rootKey?: RootKeyHint
  }
}

// ── events and the Garden bridge (§5, §6) ─────────────────────────────

/** The app-level event (`onSourcesChanged`). */
export const SOURCES_CHANGED_KIND = 'sources_changed' as const

/** The widget cap that lets a card read sources. */
export const SOURCES_READ_CAP = 'sources:read' as const

/** The Garden bridge verbs (catalog rows are B3's). */
export const SOURCES_BRIDGE_VERBS = [
  'sources.list',
  'sources.read',
  'sources.history',
  'sources.subscribe',
  'sources.refresh',
  'sources.ask',
] as const
export type SourcesBridgeVerb = (typeof SOURCES_BRIDGE_VERBS)[number]

/** `sources.list` / reader reach states. */
export type SourceReach = 'ok' | 'unreachable' | 'server_not_saved' | 'cap_not_granted' | 'removed'

/** Vault confirm actions (`vault/confirm/start`). */
export const SOURCE_CONFIRM_SECRET = 'source-secret' as const
export const SOURCE_CONFIRM_BATCH = 'source-batch' as const

/** Renderer limits mirrored from the daemon (§3.6). */
export const REMOTE_POLL_SECS = 20
export const MAX_SOURCE_SERVERS_PER_GARDEN = 8
