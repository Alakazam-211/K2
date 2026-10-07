// The legacy activity adapter: dots for a server WITHOUT the
// `daemon-activity` reported feature (0.44.x and older).
//
// prd-daemon-activity-and-thread-working-v1 S5 (RL9) moved "what is each
// agent doing" into the daemon, and the renderer only draws the daemon's
// rows (`stores/activity.ts`). A 0.44.x server has no rows to give: no
// `activity_changed`, no `GET /cli/activity/snapshot`. During a fleet hop the
// desktop app updates before its servers, so a new client must still light
// that server's tabs, Sidebar, Active bar, Home rows and rooms (RL13, Q5).
//
// This module is the ONLY place that turns an old server's signals into
// activity rows, the way a 0.44.x client did:
//   - `session_activity_changed` — the old daemon's title/bell observer
//     (0.40.39+), keyed by agent name: braille title = working, idle marker
//     or bell = idle, grok's `⚠ Action Required` title = permission.
//   - `agent_status_changed` — the old daemon's hook bucket (start / stop /
//     permission), keyed by the v2 session id (`paneId`).
//   - the visible pane's bottom rows, for harnesses with no title and no
//     hook (hermes, cursor-agent, gemini, aider …): a busy footer phrase.
//     Mounted panes only; it decays after `LEGACY_SCREEN_GRACE_MS`.
// Precedence is 0.44.x's: a hook permission wins; then the observer once it
// has spoken for the session; then a hook start or a busy footer.
//
// `stores/activity.ts` feeds this adapter only while its server is legacy,
// and its output goes into the same per-server view every surface reads —
// no surface has a second path. The ratchet (`lib/activity-ratchet.test.ts`)
// keeps these names inside this module.

import type { ActivityDisplay, ActivityRow, ActivityWorkspace } from '@/stores/session-events'

/** A pane's busy footer counts this long after it was last seen (0.44.x's
 *  1 s idle grace on a 500 ms watcher). */
export const LEGACY_SCREEN_GRACE_MS = 1_500

/** How many bottom rows of a pane the footer scan reads. */
export const LEGACY_SCREEN_ROWS = 15

/** Busy-footer phrases, matched case-insensitively (0.44.x's list: the
 *  stable hint text of each CLI's status line, not its rotating verb). */
export const LEGACY_BUSY_PHRASES: readonly string[] = [
  'esc to interrupt', // claude, codex
  'esc to cancel', // gemini
  'esc:cancel', // grok busy footer
  'starting session…', // grok startup
  'thinking…', // grok transcript
  'msg=interrupt', // hermes busy footer (no titles, no bell)
  'ctrl+c to stop', // cursor-agent
  'waiting for ', // aider, grok status row
  'thinking...', // goose, copilot, gemini fallback, pi, ollama
  'pondering...', // copilot
  'unravelling...', // copilot
  'working...', // opencode, pi
  'agent is working', // opencode
  ' is thinking...', // "<model> is thinking..."
  'planning next moves', // cursor-agent
  'taking longer than expected', // cursor-agent stall
  'loading...', // llm-tui-rs
  '🤖: waiting', // tenere
]

/** Does any of these bottom rows show a busy footer? */
export function legacyScreenShowsBusy(rows: readonly string[]): boolean {
  for (const text of rows) {
    if (!text) continue
    const lower = text.toLowerCase()
    for (const phrase of LEGACY_BUSY_PHRASES) if (lower.includes(phrase)) return true
  }
  return false
}

type ObserverStatus = 'working' | 'idle' | 'permission'
type HookStatus = 'start' | 'stop' | 'permission'

/** Everything one old server told this client. */
export interface LegacyFeed {
  /** agent name → the old daemon's title/bell observer. */
  observer: Map<string, { status: ObserverStatus; workspacePath: string | null }>
  /** v2 session id → the last hook bucket. */
  hooks: Map<string, { status: HookStatus; workspacePath: string | null }>
  /** agent name → a mounted pane shows a busy footer (until it decays). */
  screen: Map<string, { workspacePath: string | null }>
  /** agent name → v2 session id (`agents/running`, `session_added`). */
  sessionOf: Map<string, string>
  /** agent name → the session's cwd. */
  cwdOf: Map<string, string>
}

export function emptyLegacyFeed(): LegacyFeed {
  return { observer: new Map(), hooks: new Map(), screen: new Map(), sessionOf: new Map(), cwdOf: new Map() }
}

/** A project of the server, for attribution: the pinned Chat's agent name
 *  is its project id; any other session belongs to the deepest project
 *  whose path holds its cwd. */
export interface LegacyProject {
  id: string
  path?: string
}

function pathHolds(root: string, path: string): boolean {
  const norm = (p: string): string => p.replace(/\\/g, '/').replace(/\/+$/, '')
  const r = norm(root)
  const a = norm(path)
  return r.length > 0 && (a === r || a.startsWith(`${r}/`))
}

function projectFor(agentName: string, path: string | null, projects: readonly LegacyProject[]): string | null {
  if (agentName && projects.some((p) => p.id === agentName)) return agentName
  if (!path) return null
  let best: LegacyProject | null = null
  for (const p of projects) {
    if (!p.path || !pathHolds(p.path, path)) continue
    if (!best?.path || p.path.length > best.path.length) best = p
  }
  return best?.id ?? null
}

function displayFor(observer: ObserverStatus | undefined, hook: HookStatus | undefined, screenBusy: boolean): ActivityDisplay {
  if (hook === 'permission') return 'waiting'
  if (observer) return observer === 'permission' ? 'waiting' : observer
  if (hook === 'start' || screenBusy) return 'working'
  return 'idle'
}

interface Evidence {
  agentName: string
  sessionId: string | null
  display: ActivityDisplay
  workspacePath: string | null
  source: ActivityRow['evidenceSource']
}

function evidence(feed: LegacyFeed): Evidence[] {
  const out: Evidence[] = []
  const agents = new Set([...feed.observer.keys(), ...feed.screen.keys()])
  const agentOfSession = new Map<string, string>()
  for (const [agent, sid] of feed.sessionOf) {
    agentOfSession.set(sid, agent)
    if (feed.hooks.has(sid)) agents.add(agent)
  }
  for (const agent of agents) {
    const sid = feed.sessionOf.get(agent) ?? null
    const obs = feed.observer.get(agent)
    const hook = sid ? feed.hooks.get(sid) : undefined
    const screen = feed.screen.get(agent)
    out.push({
      agentName: agent,
      sessionId: sid,
      display: displayFor(obs?.status, hook?.status, screen !== undefined),
      workspacePath: obs?.workspacePath ?? hook?.workspacePath ?? feed.cwdOf.get(agent) ?? screen?.workspacePath ?? null,
      source: hook?.status === 'permission' ? 'hook' : obs ? 'title' : hook ? 'hook' : 'screen',
    })
  }
  // A hook for a session whose agent name this client hasn't learned yet:
  // its own row, found by the tab's `sessionId`.
  for (const [sid, hook] of feed.hooks) {
    if (agentOfSession.has(sid)) continue
    out.push({
      agentName: '',
      sessionId: sid,
      display: displayFor(undefined, hook.status, false),
      workspacePath: hook.workspacePath,
      source: 'hook',
    })
  }
  return out
}

/** The rows this feed shows, one per session. A session whose v2 id is
 *  known is keyed by it (a tab finds it by `sessionId`); otherwise by
 *  `legacy:<agentName>` (a tab finds it by its agent name). `prev` keeps
 *  each row's `rev` and turn start stable across recomputes. */
export function legacyRows(
  feed: LegacyFeed,
  prev: ReadonlyMap<string, ActivityRow>,
  projects: readonly LegacyProject[],
  now: number,
): Map<string, ActivityRow> {
  const rows = new Map<string, ActivityRow>()
  for (const e of evidence(feed)) {
    const sessionId = e.sessionId ?? `legacy:${e.agentName}`
    const before = prev.get(sessionId)
    const display = e.display
    const changed = before?.display !== display
    rows.set(sessionId, {
      sessionId,
      agentName: e.agentName,
      projectId: projectFor(e.agentName, e.workspacePath, projects),
      workspacePath: e.workspacePath,
      harness: 'unknown',
      display,
      lead: {
        state: display === 'working' ? 'working' : display === 'waiting' ? 'waiting' : 'idle',
        outcome: 'none',
        since: changed ? now : (before?.lead.since ?? now),
        promptId: null,
      },
      children: { subagents: 0, shells: 0, monitors: 0, crons: 0, unknown: 0, owed: 0, waiting: 0 },
      turnStartedAt: display === 'working' ? (before?.turnStartedAt ?? now) : null,
      evidenceAt: changed ? now : (before?.evidenceAt ?? now),
      evidenceSource: e.source,
      reason: '',
      staleSince: null,
      confirmed: true,
      rev: changed ? (before?.rev ?? 0) + 1 : (before?.rev ?? 1),
    })
  }
  return rows
}

/** RL1's rollup of legacy rows: per project when attributed, else per path. */
export function legacyRollups(
  rows: ReadonlyMap<string, ActivityRow>,
  key: (projectId: string | null, workspacePath: string | null) => string,
  highest: (list: Iterable<ActivityDisplay>) => ActivityDisplay,
): Map<string, ActivityWorkspace> {
  const groups = new Map<string, { projectId: string | null; workspacePath: string | null; list: ActivityRow[] }>()
  for (const r of rows.values()) {
    const k = key(r.projectId, r.workspacePath)
    const g = groups.get(k) ?? { projectId: r.projectId, workspacePath: r.workspacePath, list: [] }
    g.list.push(r)
    groups.set(k, g)
  }
  const out = new Map<string, ActivityWorkspace>()
  for (const [k, g] of groups) {
    const counts: Record<ActivityDisplay, number> = { working: 0, monitoring: 0, waiting: 0, idle: 0, unverifiable: 0 }
    for (const r of g.list) counts[r.display] += 1
    let since: number | null = null
    for (const r of g.list) {
      if (r.display === 'working' && r.turnStartedAt !== null && (since === null || r.turnStartedAt < since)) {
        since = r.turnStartedAt
      }
    }
    out.set(k, {
      projectId: g.projectId,
      workspacePath: g.workspacePath,
      display: highest(g.list.map((r) => r.display)),
      counts,
      since,
    })
  }
  return out
}
