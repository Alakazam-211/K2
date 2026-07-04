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
export type FeedbackStatus = 'waiting' | 'answered' | 'resolved' | 'dismissed'

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

/** POST /cli/feedback/answer — stores the answer + flips to `answered`
 *  (F1 stores only; session injection is F3). Author defaults to
 *  `owner` daemon-side. */
export async function answerFeedback(id: string, answer: string): Promise<void> {
  await daemonCliPost('feedback/answer', { id, answer })
}

/** POST /cli/feedback/comment — appends to the thread, status unchanged. */
export async function commentFeedback(id: string, body: string): Promise<void> {
  await daemonCliPost('feedback/comment', { id, body })
}

/** POST /cli/feedback/resolve — `resolved` or `dismissed`. */
export async function resolveFeedback(
  id: string,
  status: 'resolved' | 'dismissed',
): Promise<void> {
  await daemonCliPost('feedback/resolve', { id, status })
}

// ── Pure helpers (unit-tested in feedback-api.test.ts) ────────────────────

export function sortNewestFirst<T extends { createdAt: number }>(rows: T[]): T[] {
  return [...rows].sort((a, b) => b.createdAt - a.createdAt)
}

/** Page grouping: waiting items are the prominent section; answered and
 *  closed (resolved/dismissed) stay accessible below. */
export interface GroupedFeedback<T> {
  waiting: T[]
  answered: T[]
  closed: T[]
}

export function groupByStatus<T extends { status: FeedbackStatus }>(
  rows: T[],
): GroupedFeedback<T> {
  const grouped: GroupedFeedback<T> = { waiting: [], answered: [], closed: [] }
  for (const row of rows) {
    if (row.status === 'waiting') grouped.waiting.push(row)
    else if (row.status === 'answered') grouped.answered.push(row)
    else grouped.closed.push(row)
  }
  return grouped
}

/** What the reply box does for an item: a WAITING question/approval takes
 *  the reply as the ANSWER (answer route → status answered); everything
 *  else (fyi, or an already-answered/closed thread) posts a comment. */
export function replyMode(item: {
  status: FeedbackStatus
  kind: FeedbackKind
}): 'answer' | 'comment' {
  return item.status === 'waiting' && (item.kind === 'question' || item.kind === 'approval')
    ? 'answer'
    : 'comment'
}

/** One-tap option buttons are live only while the ask still waits. */
export function optionsActionable(item: {
  status: FeedbackStatus
  options: string[] | null
}): boolean {
  return item.status === 'waiting' && (item.options?.length ?? 0) > 0
}
