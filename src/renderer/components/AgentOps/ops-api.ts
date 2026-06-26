// Agent Ops read API — wire types + PURE logic.
//
// Phase D of `.k2/prds/prd-observability-agent-ops.md`. This module owns:
//   - the wire shapes the daemon's `/cli/ops/*` routes return (verbatim,
//     camelCase — see `crates/k2-daemon/src/ops_routes.rs`),
//   - the pure helpers the <AgentOps /> view renders with (relative time,
//     status normalization, basename/short-address), and
//   - the stream-event → row-update interpretation (no React, no I/O).
//
// Everything here is side-effect-free so it unit-tests without a daemon,
// a browser, or a WebSocket (ops-api.test.ts).

// ── Overview (GET /cli/ops/overview) ──────────────────────────────────────

/** Normalized agent status the overview exposes. `working`/`idle` come from
 *  the daemon's start/stop bucket; `permission` passes through. */
export type AgentStatus = 'working' | 'idle' | 'permission'

/** One live session, exactly as `/cli/ops/overview` serializes it
 *  (OverviewSession in ops_routes.rs, `rename_all = "camelCase"`). */
export interface OverviewSession {
  sessionId: string
  workspacePath: string
  agentAddress: string
  active: boolean
  /** `null` until an AgentStatusChanged has been observed since boot. */
  agentStatus: AgentStatus | null
  /** `'live'` when the PTY backs a heartbeat's active terminal, else null. */
  heartbeatState: 'live' | null
  /** Unix SECONDS of the last observed status change, or null. */
  lastActivityAt: number | null
}

// ── Activity (GET /cli/ops/activity) ──────────────────────────────────────

/** One persistent `activity_feed` row, as the daemon serializes it
 *  (ActivityFeedEntry in schema.rs, camelCase). `createdAt` is unix seconds. */
export interface ActivityRow {
  id: number
  projectId: string
  actor: string | null
  eventType: string
  fromWorkspace: string | null
  toWorkspace: string | null
  toProjectId: string | null
  summary: string | null
  metadata: string | null
  createdAt: number
}

// ── Stream (GET /cli/ops/stream WS) ───────────────────────────────────────

/** The hello frame the ops stream sends first (distinct from the envelope). */
export interface OpsHelloFrame {
  kind: 'hello'
  subscriber_id: number
}

/** The additive envelope every non-hello frame is wrapped in. `event` is a
 *  verbatim `SessionEvent` (still discriminated by its own `kind`) or an
 *  `AgentSignal`, depending on `source`. */
export interface OpsStreamEnvelope {
  source: 'session' | 'awareness'
  event: Record<string, unknown>
}

/** What a single stream frame means for the overview, decided purely so the
 *  view applies the common case (a status flip) with NO refetch and only
 *  asks for a coalesced refetch on structural / active-set changes. */
export type StreamAction =
  | { type: 'status'; sessionId: string; status: AgentStatus; at: number }
  | { type: 'refetch' }
  | { type: 'ignore' }

/** Map the daemon's raw start/stop/permission bucket to the AgentStatus
 *  vocabulary the overview uses (twin of `normalize_status` in ops_routes.rs,
 *  kept in lockstep so the live stream and the snapshot never disagree). */
export function normalizeStatus(raw: string): AgentStatus {
  switch (raw) {
    case 'start':
      return 'working'
    case 'stop':
      return 'idle'
    case 'permission':
      return 'permission'
    default:
      // Unknown bucket — treat as idle for display rather than inventing a
      // status the badge can't render.
      return 'idle'
  }
}

/** Interpret one parsed stream frame against `nowSec` (unix seconds).
 *
 *  - session / `agent_status_changed` → a precise `status` delta keyed on
 *    `paneId` (== the overview's `sessionId`). The common case; applied
 *    in-place, no refetch.
 *  - session / `session_added` | `session_removed` | `active_changed` →
 *    `refetch`: the fleet's shape (or active set) changed and a partial row
 *    can't be synthesized faithfully — re-pull the one-shot overview
 *    (coalesced by the caller).
 *  - everything else (awareness signals, hello-as-envelope, unknown kinds)
 *    → `ignore`. */
export function interpretStreamEvent(
  env: OpsStreamEnvelope,
  nowSec: number,
): StreamAction {
  if (env.source !== 'session') return { type: 'ignore' }
  const ev = env.event
  const kind = typeof ev.kind === 'string' ? ev.kind : ''
  switch (kind) {
    case 'agent_status_changed': {
      const sessionId = typeof ev.paneId === 'string' ? ev.paneId : ''
      const raw = typeof ev.status === 'string' ? ev.status : ''
      if (!sessionId || !raw) return { type: 'ignore' }
      return { type: 'status', sessionId, status: normalizeStatus(raw), at: nowSec }
    }
    case 'session_added':
    case 'session_removed':
    case 'active_changed':
      return { type: 'refetch' }
    default:
      return { type: 'ignore' }
  }
}

/** Apply a `status` delta to the overview rows immutably. Returns the SAME
 *  array reference when nothing matched (so React can skip the re-render). */
export function applyStatusToRows(
  rows: OverviewSession[],
  sessionId: string,
  status: AgentStatus,
  at: number,
): OverviewSession[] {
  let changed = false
  const next = rows.map((r) => {
    if (r.sessionId !== sessionId) return r
    changed = true
    return { ...r, agentStatus: status, lastActivityAt: at }
  })
  return changed ? next : rows
}

// ── Display helpers ───────────────────────────────────────────────────────

/** Last path segment of a workspace cwd — the human-facing workspace name.
 *  Tolerates trailing slashes and empty input. */
export function workspaceBasename(path: string): string {
  if (!path) return '(unknown)'
  const trimmed = path.replace(/\/+$/, '')
  const idx = trimmed.lastIndexOf('/')
  const base = idx === -1 ? trimmed : trimmed.slice(idx + 1)
  return base || '(root)'
}

/** A compact form of the agent address for the row. The v2 map key can be a
 *  path-ish or `a::b` composite; show the most specific trailing segment. */
export function shortAddress(address: string): string {
  if (!address) return '—'
  const parts = address.split(/::|\//).filter(Boolean)
  return parts.length ? parts[parts.length - 1] : address
}

/** Compact relative time, e.g. "just now", "2m ago", "3h ago", "5d ago".
 *  Both args are unix SECONDS. A null/absent timestamp renders as "—".
 *  Future timestamps (clock skew) clamp to "just now". */
export function formatRelativeTime(
  thenSec: number | null | undefined,
  nowSec: number,
): string {
  if (thenSec === null || thenSec === undefined) return '—'
  const delta = Math.max(0, nowSec - thenSec)
  if (delta < 45) return 'just now'
  if (delta < 90) return '1m ago'
  const mins = Math.round(delta / 60)
  if (mins < 60) return `${mins}m ago`
  const hours = Math.round(delta / 3600)
  if (hours < 24) return `${hours}h ago`
  const days = Math.round(delta / 86400)
  return `${days}d ago`
}
