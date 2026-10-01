// Feedback F2 — renderer-side client for the daemon's `/cli/feedback/*`
// routes (F1, feedback_routes.rs) + the pure list/reply helpers the page
// renders from.
//
// Wire shapes mirror k2-core's `FeedbackItem`/`FeedbackComment` (camelCase
// serde) — see crates/k2-core/src/feedback.rs. The list route is
// PER-WORKSPACE (`?project=<path>`), so the page's "All" view fans out one
// GET per registered project and tags each row with its host workspace;
// the fan-out reads the ALREADY-LOADED projects store, never fetchProjects
// (feedback_dev_mode_performance).

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'

export type FeedbackKind = 'question' | 'approval' | 'fyi'
export type FeedbackStatus =
  | 'waiting'
  | 'answered'
  | 'resolved'
  | 'dismissed'
  | 'planned'
  | 'needs_discussion'

/** Stamp on the asking session. Unknown strings are honest JSON, not canonical. */
export type FeedbackSessionKind = 'canonical' | 'sandbox' | 'sidecar' | 'api' | (string & {}) | null

export interface FeedbackItem {
  id: string
  projectId: string
  sessionId: string | null
  sessionKind: FeedbackSessionKind
  agentName: string
  kind: FeedbackKind
  title: string
  body: string | null
  options: string[] | null
  priority: number
  status: FeedbackStatus
  answer: string | null
  createdAt: number
  updatedAt: number
  answeredAt: number | null
  commentCount: number
  /** Username snapshots for push targeting. */
  assignees: string[]
}

/** A list row tagged with the workspace it lives on. `linked: false` =
 *  its workspace was removed (or re-added under a new id): no workspace
 *  name or path is recorded, so both are null (prd-tickets-badge-orphans). */
export interface FeedbackListRow extends FeedbackItem {
  projectPath: string | null
  projectName: string | null
  linked: boolean
}

/** `list-all` wire row: the item plus where it lives (k2-core
 *  `HostFeedbackItem`). */
interface HostFeedbackWireRow extends FeedbackItem {
  projectPath: string | null
  projectName: string | null
  linked: boolean
}

export interface FeedbackComment {
  author: string
  body: string
  at: number
}

/** The `show` wire shape: the item flat + workspace/projectPath + thread. */
export interface FeedbackShow extends FeedbackItem {
  workspace: string | null
  projectPath: string | null
  comments: FeedbackComment[]
  /** That project's `workspace_sessions.session_id` (pinned conversation). */
  canonicalSessionId?: string | null
}

/** Minimal slice of the projects store the fan-out needs. */
export interface FeedbackProjectRef {
  id: string
  name: string
  path: string
}

/** Every ticket the page shows. A server that reports `tickets-list-all`
 *  answers in ONE GET (`list-all?all=1`), including tickets from removed
 *  workspaces (`linked: false`). An older server keeps the per-workspace
 *  fan-out, where every row is linked. */
export async function fetchAllFeedback(
  projects: FeedbackProjectRef[],
): Promise<FeedbackListRow[]> {
  if (primaryScope().serverSupports('tickets-list-all')) return fetchHostFeedback()
  return fetchFeedbackFanOut(projects)
}

/** GET /cli/feedback/list-all?all=1 — every ticket on the host. Throws on
 *  any failure (the page shows the error). */
export async function fetchHostFeedback(): Promise<FeedbackListRow[]> {
  const res = await daemonCliGet<{ ok: boolean; items: HostFeedbackWireRow[] }>(
    primaryScope(),
    'feedback/list-all',
    { all: 1 },
  )
  if (!Array.isArray(res.items)) throw new Error('feedback/list-all: response has no items')
  return sortNewestFirst(
    res.items.map((item) => ({
      ...item,
      assignees: item.assignees ?? [],
      projectPath: item.linked ? item.projectPath : null,
      projectName: item.linked ? item.projectName : null,
      linked: item.linked === true,
    })),
  )
}

/** GET /cli/feedback/list?project=<path>&all=1 for every registered
 *  workspace, tag rows with their host project, merge newest-first.
 *  Per-project failures are logged and skipped (one unreachable
 *  workspace must not blank the whole page); a fully-failed fan-out
 *  throws so the page shows a real error instead of a fake empty. */
async function fetchFeedbackFanOut(
  projects: FeedbackProjectRef[],
): Promise<FeedbackListRow[]> {
  if (projects.length === 0) return []
  let failures = 0
  let lastError: unknown = null
  const results = await Promise.all(
    projects.map(async (p) => {
      try {
        const res = await daemonCliGet<{ ok: boolean; items: FeedbackItem[] }>(primaryScope(),
          'feedback/list',
          { project: p.path, all: 1 },
        )
        return (res.items ?? []).map((item) => ({
          ...item,
          assignees: item.assignees ?? [],
          projectPath: p.path,
          projectName: p.name,
          linked: true,
        }))
      } catch (err) {
        failures++
        lastError = err
        console.warn('[feedback] list failed for', p.path, err)
        return [] as FeedbackListRow[]
      }
    }),
  )
  if (failures === projects.length) {
    throw lastError instanceof Error ? lastError : new Error(String(lastError))
  }
  return sortNewestFirst(results.flat())
}

/** Waiting-count for the top-bar badge: one host-wide GET. Throws on ANY
 *  failure — there is no per-workspace fallback, because that turned a dead
 *  host into a count of 0 (prd-tickets-badge-orphans TB8). The store keeps
 *  the last number and marks it stale. */
export async function fetchWaitingCount(): Promise<number> {
  const res = await daemonCliGet<{ ok: boolean; count: number }>(primaryScope(),
    'feedback/waiting-count',
  )
  if (typeof res.count !== 'number') {
    throw new Error('feedback/waiting-count: response has no count')
  }
  return res.count
}

/** GET /cli/feedback/show?id=<id> — one item + its full thread. */
export async function fetchFeedbackShow(id: string): Promise<FeedbackShow> {
  return daemonCliGet<FeedbackShow>(primaryScope(), 'feedback/show', { id })
}

/** POST /cli/feedback/comment — it's just a comment thread. The
 *  renderer posts author-less (= `owner`, a HUMAN comment): the daemon
 *  injects it into the asking session, and the FIRST human comment on
 *  a waiting question/approval doubles as the answer behind the scenes
 *  (status → answered, `ask --wait` unblocks). fyi never auto-answers.
 *  (The legacy answer route still exists for API compat; the renderer
 *  no longer uses it.) */
export interface FeedbackCommentResult {
  ok: boolean
  id: string
  commentId?: string
  author?: string
  answered?: boolean
  status?: FeedbackStatus
  delivered?: boolean
  deliveryReason?: string | null
  deliveredSessionId?: string | null
}

export async function commentFeedback(
  id: string,
  body: string,
): Promise<FeedbackCommentResult> {
  return daemonCliPost<FeedbackCommentResult>(primaryScope(), 'feedback/comment', { id, body })
}

/** Agent-tab wake plan (D6). Never `sandbox/reopen` a pinned conversation id. */
export type AskingSessionWakeAction =
  | 'none'
  | 'checking'
  | 'attach-live'
  | 'ensure-pinned-chat'
  | 'sandbox/reopen'
  | 'dormant-unwakeable'

export function askingSessionWakeAction(args: {
  sessionId: string | null
  sessionKind: FeedbackSessionKind
  /** `undefined` = show payload not loaded yet; `null` = no pinned id. */
  canonicalSessionId: string | null | undefined
  liveById: boolean
}): AskingSessionWakeAction {
  if (!args.sessionId) return 'none'
  if (args.liveById) return 'attach-live'
  const d6 =
    typeof args.canonicalSessionId === 'string' &&
    args.canonicalSessionId.length > 0 &&
    args.sessionId === args.canonicalSessionId
  if (d6) return 'ensure-pinned-chat'
  if (args.sessionKind === 'canonical') return 'ensure-pinned-chat'
  if (args.canonicalSessionId === undefined) return 'checking'
  if (args.sessionKind === 'sandbox') return 'sandbox/reopen'
  return 'dormant-unwakeable'
}

/** POST /cli/feedback/resolve — `resolved`, `dismissed`, `planned`,
 *  `needs_discussion`, or `waiting` (reopen). `answered` is NOT manually
 *  settable. */
export async function resolveFeedback(
  id: string,
  status: 'resolved' | 'dismissed' | 'waiting' | 'planned' | 'needs_discussion',
): Promise<void> {
  await daemonCliPost(primaryScope(), 'feedback/resolve', { id, status })
}

/** Human-readable status label for chips / badges. */
export function statusLabel(status: FeedbackStatus | 'all'): string {
  if (status === 'all') return 'All'
  if (status === 'needs_discussion') return 'Needs discussion'
  return status.charAt(0).toUpperCase() + status.slice(1)
}

/** POST /cli/feedback/assign — replace assignee set (username snapshots). */
export async function assignFeedback(
  id: string,
  usernames: string[],
): Promise<{ assignees: string[] }> {
  return daemonCliPost<{ assignees: string[] }>(primaryScope(), 'feedback/assign', {
    id,
    usernames,
  })
}

// ── Pure helpers (unit-tested in feedback-api.test.ts) ────────────────────

export function sortNewestFirst<T extends { createdAt: number }>(rows: T[]): T[] {
  return [...rows].sort((a, b) => b.createdAt - a.createdAt)
}

/** Page grouping: waiting / needs discussion are open sections; answered
 *  and closed (resolved/dismissed) stay accessible below. An OPEN ticket
 *  whose workspace is gone (`linked === false`) goes in `unlinked`, never
 *  `waiting`, so "Waiting on you" still matches the badge; a closed one
 *  goes in `closed`. */
export interface GroupedFeedback<T> {
  waiting: T[]
  needs_discussion: T[]
  answered: T[]
  planned: T[]
  unlinked: T[]
  closed: T[]
}

/** True for a row whose workspace was removed (only `list-all` sets it). */
export function isUnlinked(row: { linked?: boolean }): boolean {
  return row.linked === false
}

export function groupByStatus<T extends { status: FeedbackStatus; linked?: boolean }>(
  rows: T[],
): GroupedFeedback<T> {
  const grouped: GroupedFeedback<T> = {
    waiting: [],
    needs_discussion: [],
    answered: [],
    planned: [],
    unlinked: [],
    closed: [],
  }
  for (const row of rows) {
    const closed = row.status === 'resolved' || row.status === 'dismissed'
    if (isUnlinked(row) && !closed) grouped.unlinked.push(row)
    else if (row.status === 'waiting') grouped.waiting.push(row)
    else if (row.status === 'needs_discussion') grouped.needs_discussion.push(row)
    else if (row.status === 'answered') grouped.answered.push(row)
    else if (row.status === 'planned') grouped.planned.push(row)
    else grouped.closed.push(row)
  }
  return grouped
}

/** Per-status counts for the page's status-filter chips (AFSROW-style:
 *  every status shows its count, plus the total for "All"). Counted
 *  AFTER the workspace + search filters so the chips describe exactly
 *  what toggling them would reveal. */
export interface StatusCounts {
  all: number
  waiting: number
  needs_discussion: number
  answered: number
  resolved: number
  dismissed: number
  planned: number
}

export function countByStatus<T extends { status: FeedbackStatus }>(rows: T[]): StatusCounts {
  const counts: StatusCounts = {
    all: rows.length,
    waiting: 0,
    needs_discussion: 0,
    answered: 0,
    resolved: 0,
    dismissed: 0,
    planned: 0,
  }
  for (const row of rows) counts[row.status]++
  return counts
}

/** One-tap option buttons are live only while the ask still waits. */
export function optionsActionable(item: {
  status: FeedbackStatus
  options: string[] | null
}): boolean {
  return item.status === 'waiting' && (item.options?.length ?? 0) > 0
}

/** Tokenized, order-independent AND search over the list. The query is
 *  split on whitespace; a row matches only if EVERY term is a substring
 *  of its combined lowercased haystack (title/agent/workspace/kind/
 *  status/id), so each term can hit a different field. Empty query = no
 *  filter. Substring-only — no fuzzy matching. */
export function filterBySearch<
  T extends Pick<FeedbackListRow, 'id' | 'title' | 'agentName' | 'projectName' | 'kind' | 'status'>,
>(rows: T[], query: string): T[] {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean)
  if (terms.length === 0) return rows
  return rows.filter((r) => {
    const haystack = [r.title, r.agentName, r.projectName, r.kind, r.status, r.id]
      .join(' ')
      .toLowerCase()
    return terms.every((t) => haystack.includes(t))
  })
}

/** The label an unlinked ticket's workspace shows. */
export const UNLINKED_WORKSPACE_LABEL = 'Unlinked workspace'

/** The statuses a card's menu offers. An unlinked ticket gets only
 *  Resolve and Dismiss (TB18): no agent is left to discuss or plan with,
 *  and these may be legal or money items, so they close one at a time. */
export function selectableStatusesFor(row: { linked?: boolean }): readonly SelectableStatus[] {
  return isUnlinked(row) ? UNLINKED_STATUSES : SELECTABLE_STATUSES
}

export const SELECTABLE_STATUSES = [
  'waiting',
  'needs_discussion',
  'planned',
  'resolved',
  'dismissed',
] as const
export type SelectableStatus = (typeof SELECTABLE_STATUSES)[number]
const UNLINKED_STATUSES: readonly SelectableStatus[] = ['resolved', 'dismissed']

/** What identifies an unlinked ticket when its workspace name is gone
 *  (Appa A1): title, the date it was filed, the agent it was filed as (the
 *  only name the row still records), and a short id for `k2 tickets show`. */
export function unlinkedDetails(row: {
  id: string
  title: string
  agentName: string
  createdAt: number
}): { title: string; filed: string; agent: string; shortId: string } {
  return {
    title: row.title,
    filed: formatFiledDate(row.createdAt),
    agent: row.agentName,
    shortId: row.id.slice(0, 8),
  }
}

/** `createdAt` (unix seconds) as a fixed UTC date + time, so the same
 *  ticket reads the same on every machine: `2026-09-30 14:05 UTC`. */
export function formatFiledDate(createdAtSec: number): string {
  const iso = new Date(createdAtSec * 1000).toISOString()
  return `${iso.slice(0, 10)} ${iso.slice(11, 16)} UTC`
}

/** Assignee filter values for the board people dropdown. */
export type AssigneeFilter = 'all' | 'unassigned' | string

/** Unique assignee usernames across rows, sorted A–Z. */
export function collectAssignees<T extends { assignees?: string[] | null }>(rows: T[]): string[] {
  const set = new Set<string>()
  for (const row of rows) {
    for (const name of row.assignees ?? []) {
      const t = name.trim()
      if (t) set.add(t)
    }
  }
  return [...set].sort((a, b) => a.localeCompare(b))
}

/** Filter rows by assignee. `all` = no filter; `unassigned` = empty
 *  assignee set; otherwise the username must appear on the ticket. */
export function filterByAssignee<T extends { assignees?: string[] | null }>(
  rows: T[],
  assignee: AssigneeFilter,
): T[] {
  if (assignee === 'all') return rows
  if (assignee === 'unassigned') {
    return rows.filter((r) => (r.assignees?.length ?? 0) === 0)
  }
  return rows.filter((r) => (r.assignees ?? []).includes(assignee))
}
