// Thread view's slide-out panels: Chatter and Heartbeats (with the other
// outside input). They use the terminal reveal's column (`threadReveal.ts`,
// `SplitSessionColumns`): the same place, size and slide. One drawer at a
// time: opening one closes the others.

import type { OverlayDoc, OverlayThreadItem } from './overlayThread'

export type ThreadPanelKind = 'chatter' | 'heartbeats'
/** What fills Thread view's slide-out column. */
export type ThreadDrawerKind = 'terminal' | ThreadPanelKind

export const THREAD_PANEL_KINDS: readonly ThreadPanelKind[] = ['chatter', 'heartbeats']

export const THREAD_PANEL_LABEL: Record<ThreadPanelKind, string> = {
  chatter: 'Chatter',
  heartbeats: 'Heartbeats',
}

/** One line under the panel's title: what it holds. */
export const THREAD_PANEL_HINT: Record<ThreadPanelKind, string> = {
  chatter: 'Messages between this agent and other agents',
  heartbeats: 'Heartbeats and other outside input that woke this workspace',
}

/** The open drawer: a panel wins over the remembered terminal (opening a
 *  panel closes the terminal, so both are only set for one render). */
export function threadDrawerOpen(terminalRevealed: boolean, panel: ThreadPanelKind | null): ThreadDrawerKind | null {
  if (panel) return panel
  return terminalRevealed ? 'terminal' : null
}

// ── Chatter ─────────────────────────────────────────────────────────────

export const CHATTER_LINE_MAX = 140

/** The row's one line: the sender's summary, else the body's first line. */
export function chatterLine(doc: OverlayDoc): string {
  const summary = (doc.summary ?? '').trim()
  const raw = summary || firstLine(doc.body ?? '')
  if (!raw) return '(no text)'
  return raw.length > CHATTER_LINE_MAX ? `${raw.slice(0, CHATTER_LINE_MAX - 1).trimEnd()}…` : raw
}

function firstLine(text: string): string {
  for (const line of text.split('\n')) {
    const t = line.trim()
    if (t) return t
  }
  return ''
}

/** Sender → recipient, as the rest of K2 writes it. */
export function chatterRoute(doc: OverlayDoc): { from: string; to: string | null } {
  const from = doc.from.trim() || 'unknown'
  const to = (doc.to ?? '').trim()
  return { from, to: to || null }
}

/** Oldest first, so the newest sits at the bottom, by the daemon's order
 *  (seq), then time. */
export function chatterInOrder(items: readonly OverlayThreadItem[]): OverlayThreadItem[] {
  return items.slice().sort((a, b) => a.seq - b.seq || (a.doc.created_at ?? 0) - (b.doc.created_at ?? 0))
}

// ── Heartbeats and outside input ────────────────────────────────────────

/** `/cli/heartbeat/fires-list`'s page, as Settings → Heartbeats reads it. */
export const HEARTBEAT_FIRES_LIMIT = 50
/** No live stream for fires: re-read this often while the panel is open
 *  (Settings → Heartbeats' Run history does the same). */
export const HEARTBEAT_FIRES_REFRESH_MS = 15_000

/** A `heartbeat_fires` row (camelCase, as the daemon sends it). */
export interface HeartbeatFireRow {
  id: number
  agentName?: string | null
  scheduleName?: string | null
  firedAt: string
  decision: string
  reason?: string | null
}

/** Kinds of outside input. Only heartbeats have a source today; reminders
 *  and VIP mail have no route yet (the panel says so). */
export type ThreadInputKind = 'heartbeat' | 'reminder' | 'mail'

export type ThreadInputTone = 'ok' | 'warn' | 'error' | 'muted'

export interface ThreadInputRow {
  id: string
  kind: ThreadInputKind
  /** Seconds since the epoch. */
  at: number
  /** Who or what it came from. */
  source: string
  /** What happened, in a few words. */
  status: string
  tone: ThreadInputTone
  /** The rest, when there is any. */
  detail: string | null
}

export const THREAD_INPUT_KIND_LABEL: Record<ThreadInputKind, string> = {
  heartbeat: 'Heartbeat',
  reminder: 'Reminder',
  mail: 'Mail',
}

const DECISION_LABEL: Record<string, string> = {
  fired: 'Ran',
  fired_catchup: 'Ran late',
  no_work: 'Nothing to do',
  error: 'Failed',
  wakeup_file_missing: 'Wakeup file missing',
  schedule_invalid: 'Schedule invalid',
  auto_disabled: 'Turned off',
  auto_disabled_failing: 'Turned off after failures',
  tick_gap: 'Missed while asleep',
}

export function heartbeatDecisionLabel(decision: string): string {
  if (DECISION_LABEL[decision]) return DECISION_LABEL[decision]
  if (decision.startsWith('skipped_')) return 'Skipped'
  const plain = decision.replace(/_/g, ' ').trim()
  return plain ? plain.charAt(0).toUpperCase() + plain.slice(1) : 'Unknown'
}

export function heartbeatDecisionTone(decision: string): ThreadInputTone {
  if (decision === 'fired') return 'ok'
  if (decision === 'fired_catchup') return 'warn'
  if (decision.startsWith('skipped_') || decision === 'no_work') return 'muted'
  if (DECISION_LABEL[decision]) return 'error'
  return 'muted'
}

/** `firedAt` (ISO, or SQLite's `YYYY-MM-DD HH:MM:SS` in UTC) → seconds. */
export function firedAtSeconds(firedAt: string): number | null {
  const raw = firedAt.trim()
  if (!raw) return null
  const iso = /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}/.test(raw) && !/[zZ]|[+-]\d{2}:?\d{2}$/.test(raw)
    ? `${raw.replace(' ', 'T')}Z`
    : raw
  const ms = Date.parse(iso)
  return Number.isFinite(ms) ? Math.floor(ms / 1000) : null
}

/** Heartbeat fires as input rows, newest first. A `changed` row is an edit
 *  to a heartbeat, not a wake, so it stays in Settings' history only. */
export function heartbeatInputRows(fires: readonly HeartbeatFireRow[]): ThreadInputRow[] {
  const rows: ThreadInputRow[] = []
  for (const f of fires) {
    if (!f || typeof f.decision !== 'string' || f.decision === 'changed') continue
    const at = firedAtSeconds(typeof f.firedAt === 'string' ? f.firedAt : '')
    if (at === null) continue
    const agent = (f.agentName ?? '').trim()
    const schedule = (f.scheduleName ?? '').trim()
    const source = agent && schedule ? `${agent} / ${schedule}` : schedule || agent || 'Workspace'
    const reason = (f.reason ?? '').trim()
    rows.push({
      id: `heartbeat:${f.id}`,
      kind: 'heartbeat',
      at,
      source,
      status: heartbeatDecisionLabel(f.decision),
      tone: heartbeatDecisionTone(f.decision),
      detail: reason || null,
    })
  }
  return newestFirst(rows)
}

/** One list from every source, newest first (ties keep a stable order). */
export function newestFirst(rows: readonly ThreadInputRow[]): ThreadInputRow[] {
  return rows.slice().sort((a, b) => b.at - a.at || a.id.localeCompare(b.id))
}

/** The full local time, for a row's tooltip. */
export function fullLocalTime(sec: number): string {
  try {
    return new Date(sec * 1000).toLocaleString(undefined, {
      weekday: 'short',
      month: 'short',
      day: 'numeric',
      hour: 'numeric',
      minute: '2-digit',
    })
  } catch {
    return ''
  }
}
