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

export type FeedbackKind = 'question' | 'approval' | 'fyi'
export type FeedbackStatus =
  | 'waiting'
  | 'answered'
  | 'resolved'
  | 'dismissed'
  | 'planned'
  | 'needs_discussion'

export interface FeedbackItem {
  id: string
  projectId: string
  sessionId: string | null
  sessionKind: 'canonical' | 'sandbox' | null
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

/** A list row tagged with the workspace it was fetched for (the list
 *  route is project-scoped; the page shows the workspace name). */
export interface FeedbackListRow extends FeedbackItem {
  projectPath: string
  projectName: string
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
}

/** Minimal slice of the projects store the fan-out needs. */
export interface FeedbackProjectRef {
  id: string
  name: string
  path: string
}

/** GET /cli/feedback/list?project=<path>&all=1 for every registered
 *  workspace, tag rows with their host project, merge newest-first.
 *  Per-project failures are logged and skipped (one unreachable
 *  workspace must not blank the whole page); a fully-failed fan-out
 *  throws so the page shows a real error instead of a fake empty. */
export async function fetchAllFeedback(
  projects: FeedbackProjectRef[],
): Promise<FeedbackListRow[]> {
  if (projects.length === 0) return []
  let failures = 0
  let lastError: unknown = null
  const results = await Promise.all(
    projects.map(async (p) => {
      try {
        const res = await daemonCliGet<{ ok: boolean; items: FeedbackItem[] }>(
          'feedback/list',
          { project: p.path, all: 1 },
        )
        return (res.items ?? []).map((item) => ({
          ...item,
          assignees: item.assignees ?? [],
          projectPath: p.path,
          projectName: p.name,
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

/** Waiting-count fan-out for the top-bar badge (status=waiting only). */
export async function fetchWaitingCount(
  projects: FeedbackProjectRef[],
): Promise<number> {
  const counts = await Promise.all(
    projects.map(async (p) => {
      try {
        const res = await daemonCliGet<{ ok: boolean; items: FeedbackItem[] }>(
          'feedback/list',
          { project: p.path, status: 'waiting' },
        )
        return res.items?.length ?? 0
      } catch {
        return 0
      }
    }),
  )
  return counts.reduce((n, c) => n + c, 0)
}

/** GET /cli/feedback/show?id=<id> — one item + its full thread. */
export async function fetchFeedbackShow(id: string): Promise<FeedbackShow> {
  return daemonCliGet<FeedbackShow>('feedback/show', { id })
}

/** POST /cli/feedback/comment — it's just a comment thread. The
 *  renderer posts author-less (= `owner`, a HUMAN comment): the daemon
 *  injects it into the asking session, and the FIRST human comment on
 *  a waiting question/approval doubles as the answer behind the scenes
 *  (status → answered, `ask --wait` unblocks). fyi never auto-answers.
 *  (The legacy answer route still exists for API compat; the renderer
 *  no longer uses it.) */
export async function commentFeedback(id: string, body: string): Promise<void> {
  await daemonCliPost('feedback/comment', { id, body })
}

/** POST /cli/feedback/resolve — `resolved`, `dismissed`, `planned`,
 *  `needs_discussion`, or `waiting` (reopen). `answered` is NOT manually
 *  settable. */
export async function resolveFeedback(
  id: string,
  status: 'resolved' | 'dismissed' | 'waiting' | 'planned' | 'needs_discussion',
): Promise<void> {
  await daemonCliPost('feedback/resolve', { id, status })
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
  return daemonCliPost<{ assignees: string[] }>('feedback/assign', {
    id,
    usernames,
  })
}

// ── Pure helpers (unit-tested in feedback-api.test.ts) ────────────────────

export function sortNewestFirst<T extends { createdAt: number }>(rows: T[]): T[] {
  return [...rows].sort((a, b) => b.createdAt - a.createdAt)
}

/** Page grouping: waiting / needs discussion are open sections; answered
 *  and closed (resolved/dismissed) stay accessible below. */
export interface GroupedFeedback<T> {
  waiting: T[]
  needs_discussion: T[]
  answered: T[]
  planned: T[]
  closed: T[]
}

export function groupByStatus<T extends { status: FeedbackStatus }>(
  rows: T[],
): GroupedFeedback<T> {
  const grouped: GroupedFeedback<T> = {
    waiting: [],
    needs_discussion: [],
    answered: [],
    planned: [],
    closed: [],
  }
  for (const row of rows) {
    if (row.status === 'waiting') grouped.waiting.push(row)
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
