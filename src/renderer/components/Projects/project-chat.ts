// Projects V1 P6 (prd-projects-v1 §6.4) — pure logic for the project
// chat drawer: page-size stepping for the load-earlier affordance,
// authoritative-tail message merging (live refetches must never drop
// earlier-loaded history), and the §6.4 delivered-indicator line.
// All view-free and unit-tested in project-chat.test.ts.

import type { ProjectGroupMemberInfo, ProjectGroupMessage } from './projects-api'

/** The drawer-open tail (the daemon's no-`after` default). */
export const MESSAGES_DEFAULT_LIMIT = 20
/** The daemon caps every page at 500 (`MESSAGES_AFTER_LIMIT`). */
export const MESSAGES_MAX_LIMIT = 500
/** Each "Show earlier" click widens the tail window by this much. */
export const EARLIER_PAGE_STEP = 100

/** The next no-`after` window size after a load-earlier click. The
 *  route has no `before` param — earlier history is read by re-reading
 *  the LATEST-N tail with a bigger N, capped at the daemon's 500. */
export function nextEarlierLimit(current: number): number {
  return Math.min(current + EARLIER_PAGE_STEP, MESSAGES_MAX_LIMIT)
}

/** Whether the load-earlier affordance still has anywhere to go:
 *  the last page was truncated AND the window can still widen. At the
 *  500 ceiling the drawer shows its "earliest not shown" note instead. */
export function canLoadEarlier(limit: number, truncated: boolean): boolean {
  return truncated && limit < MESSAGES_MAX_LIMIT
}

/** Merge a freshly-fetched page into what the drawer already renders.
 *
 *  `incoming` is an AUTHORITATIVE oldest-first tail (the daemon's
 *  latest-N window), so it wins verbatim; existing rows survive only as
 *  a PREFIX — rows that slid out of the fetched window (the user had
 *  "loaded earlier", then a live refetch used a smaller window). A row
 *  already in `incoming` is deduped by id; the createdAt boundary keeps
 *  the prefix strictly at-or-before the window's head (same-second
 *  boundary rows not in the window are older by rowid — they belong in
 *  the prefix). An anomalous empty page never wipes loaded history. */
export function mergeMessages(
  existing: ProjectGroupMessage[],
  incoming: ProjectGroupMessage[],
): ProjectGroupMessage[] {
  if (incoming.length === 0) return existing
  if (existing.length === 0) return incoming
  const ids = new Set(incoming.map((m) => m.id))
  const head = incoming[0]
  const prefix = existing.filter((m) => !ids.has(m.id) && m.createdAt <= head.createdAt)
  return [...prefix, ...incoming]
}

/** The PoC's display label for the delivered line: the member's agent
 *  display name, falling back to its workspace name; null when the
 *  group is memberless or the PoC row is missing/unregistered. */
export function pocLabel(
  members: ProjectGroupMemberInfo[],
  pocWorkspaceId: string | null,
): string | null {
  if (!pocWorkspaceId) return null
  const m = members.find((x) => x.workspaceId === pocWorkspaceId)
  if (!m) return null
  return m.agentName ?? m.name ?? null
}

/** The §6.4 line under a just-sent message, from the msg response's
 *  delivery outcome. `author-is-poc` shouldn't occur for owner posts —
 *  handled gracefully as "nothing to report" (the post itself is fine). */
export function deliveredLine(
  delivered: boolean,
  deliveryReason: string | null,
  poc: string | null,
): string | null {
  if (delivered) return `delivered to ${poc ?? 'the PoC'}`
  if (deliveryReason === 'author-is-poc') return null
  if (deliveryReason === 'no_poc') return 'stored — this project has no Point of Contact yet'
  return 'PoC session unreachable'
}
