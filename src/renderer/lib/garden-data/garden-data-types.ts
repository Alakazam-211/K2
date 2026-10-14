// Garden data — wire types for `/cli/zen/data/*` and `/cli/zen/call`
// (prd-gardens-local-services-v1 1A; contract-garden-data-v1 §4.5, §6, §7).
// Written in C0; read-only for B1/B2/B3 (changes go through the integrator).
//
// Times are seconds since the Unix epoch. `by` on the wire is `you`,
// `you via <widget>` or `agent:<handle>` (contract §5.3).

/** Any JSON value a widget or agent stores (≤ 60 KiB as JSON text). */
export type GardenJson = unknown

export type GardenLevel = 'read' | 'write'
export type GardenSubjectKind = 'workspace' | 'project'

/** One live key. */
export interface GardenEntry {
  key: string
  rev: number
  value: GardenJson
  updatedAt: number
  by: string
}

/** One write in a batch, a transaction part or an import plan. */
export type GardenOp =
  | { op: 'put'; key: string; value: GardenJson; ifRev?: number }
  | { op: 'delete'; key: string; ifRev?: number }

export interface GardenGetResponse {
  entry: GardenEntry | null
}

export interface GardenRevResponse {
  rev: number
}

export interface GardenDeleteResponse {
  rev: number
  deleted: boolean
}

export interface GardenListResponse {
  items: GardenEntry[]
  next: string | null
}

/** Rows strictly after `(since, after)` in `(rev, key)` order. */
export interface GardenCursor {
  since: number
  after?: string
}

export interface GardenChange {
  key: string
  rev: number
  deleted: boolean
  value: GardenJson
  by: string
  at: number
}

export interface GardenChangesResponse {
  head: number
  items: GardenChange[]
  next: GardenCursor | null
  /** The page ends inside a rev: fetch `next` before applying. */
  partial: boolean
}

export interface GardenBatchResponse {
  rev: number
  ops: number
}

export interface GardenTxBeginResponse {
  txId: string
  expiresAt: number
}

export interface GardenTxStatus {
  ops: number
  bytes: number
  expiresAt: number
}

export interface GardenTxAbortResponse {
  aborted: boolean
}

/** One `zen_data_grants` row. */
export interface GardenGrantRow {
  id: number
  space: string
  prefix: string
  subjectKind: GardenSubjectKind
  subjectId: string
  level: GardenLevel
  admin: boolean
  viaGrant: number | null
  /** `owner` | `agent:<projects.id>` */
  grantedBy: string
  grantedAt: number
  /** Present on `show`: the agent handle or project name. */
  subjectName?: string
}

export type GardenGrantFrom =
  | { kind: 'agent' }
  | { kind: 'project'; id: string; name: string }
  | { kind: 'delegation' }

export interface GardenEffectiveGrant {
  prefix: string
  level: GardenLevel
  from: GardenGrantFrom
  admin: boolean
  grantId: number
}

export interface GardenGrantsResponse {
  space: string
  /** An agent's effective grants, or (owner) every row. */
  grants: Array<GardenEffectiveGrant | GardenGrantRow>
  /** Grants the caller made under its delegations. */
  made: GardenGrantRow[]
}

export interface GardenGrantBody {
  widget: string
  agent?: string
  project?: string
  level: GardenLevel
  prefix?: string
}

export interface GardenGrantResponse {
  grant: GardenGrantRow
  created: boolean
  /** Every project grant: projects are a convenience, not a wall. */
  warning?: 'project_grant_convenience'
}

export interface GardenRevokeBody {
  widget: string
  agent?: string
  project?: string
  prefix?: string
}

export interface GardenRevokeResponse {
  revoked: number
}

export interface GardenAdminBody {
  widget: string
  agent: string
  level: GardenLevel
  prefix?: string
}

export interface GardenAdminRevokeBody {
  widget: string
  agent: string
  prefix?: string
  dryRun?: boolean
}

export interface GardenAdminRevokeResponse {
  revoked: number
  cascaded: number
}

/** The live hash's maker, as Settings shows it. */
export type GardenMadeBy =
  | { kind: 'owner' }
  | { kind: 'k2' }
  | { kind: 'agent'; handle: string }
  | { kind: 'unclaimed' }

export interface GardenSpaceRow {
  widget: string
  space: string
  bytes: number
  keys: number
  rev: number
  updatedAt: number
  orphanedAt: number | null
  grants: number
  admins: string[]
  /** null when the widget folder is gone. */
  madeBy: GardenMadeBy | null
}

export interface GardenSaveRow {
  widget: string
  bytes: number
  keys: number
}

export interface GardenSpacesResponse {
  spaces: GardenSpaceRow[]
  saves: GardenSaveRow[]
}

export interface GardenSpaceInfo {
  space: string
  widget: string
  rev: number
  reapedRev: number
  bytes: number
  keys: number
  createdAt: number
  updatedAt: number
  orphanedAt: number | null
}

export interface GardenShowResponse {
  space: GardenSpaceInfo
  grants: GardenGrantRow[]
  delegations: Array<{ grant: GardenGrantRow; madeUnder: number }>
  madeBy: GardenMadeBy | null
  liveHash: string | null
}

export interface GardenSpaceDeleteResponse {
  deleted: string
  snapshot: string
  grantsRemoved: number
}

export interface GardenSpaceMoveResponse {
  moved: boolean
  grantsMoved: number
}

export interface GardenExportHeader {
  space: string
  rev: number
  exportedAt: number
  snapshot: string | null
}

export interface GardenExportResponse {
  header: GardenExportHeader
  items: GardenEntry[]
  next: string | null
}

export interface GardenImportBody {
  widget: string
  mode: 'merge' | 'replace'
  items: Array<{ key: string; value: GardenJson }>
}

export interface GardenImportResponse {
  rev: number
  written: number
  deleted: number
}

/** `POST /cli/zen/call` (the widget door, contract §7.1). */
export interface GardenCallBody {
  garden: string
  placement: string
  /** The live bundle hash the frame runs. */
  hash: string
  /** A `garden-local` catalog verb. */
  verb: string
  args: GardenJson[]
}

export interface GardenCallResponse<T = GardenJson> {
  ok: true
  result: T
}

/** Every Garden data error code (contract §4.6). */
export type GardenErrorCode =
  | 'bad_request'
  | 'bad_key'
  | 'confirm_required'
  | 'invalid_token'
  | 'apps_not_allowed'
  | 'zen_local_only'
  | 'person_only'
  | 'owner_only'
  | 'not_granted'
  | 'author_unknown'
  | 'self_grant'
  | 'level_too_high'
  | 'not_own'
  | 'origin_refused'
  | 'cap_not_granted'
  | 'space_not_found'
  | 'agent_not_found'
  | 'project_not_found'
  | 'grant_not_found'
  | 'widget_not_found'
  | 'bad_tx'
  | 'no_snapshot'
  | 'method_not_allowed'
  | 'conflict'
  | 'space_full'
  | 'resync'
  | 'grant_exists'
  | 'use_admin_revoke'
  | 'space_exists'
  | 'stale_widget'
  | 'tx_expired'
  | 'value_too_large'
  | 'too_large'
  | 'paused'
  | 'rate_limited'
  | 'busy'
  | 'not_built'
  | 'store_unavailable'
  | 'store_full'

export interface GardenErrorEnvelope {
  error: {
    code: GardenErrorCode
    hint: string
    key?: string
    rev?: number
    bytes?: number
    what?: 'bytes' | 'keys'
    reapedRev?: number
    retryAfterSecs?: number
    oldest?: string | null
  }
}
