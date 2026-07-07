// Compact relative-time formatting shared by the Feedback page/item view
// and ProjectChatPanel. Originally lived in components/AgentOps/ops-api.ts;
// moved here when the Agent Ops fleet view was deleted in 0.40.31.

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
