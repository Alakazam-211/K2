// prd-zen-user-widgets-v2 UWA6, UW71, UWB11 — the guest projection: what a
// custom widget gets back from a verb, never K2's internal shapes.
//
// Every value that crosses the port is built here from scratch, key by key
// (closed shapes), so a new field on `ZenAgentRow` or a Thread item can't
// leak by accident (TUWA7 pins the keys):
//   - agent rows: `{address, label, index, server, state, stateLabel, detail,
//     working, needsYou, activity, counts, openable, selected, avatar, color,
//     image}`
//     (`counts` = `{subagents, tools, commands}` numbers or null), plus
//     `preview` with `thread:read` and `people` with `presence:read`.
//     Dropped: `hostKey`, `role`, `reach`, `auth`. `avatar` is a `data:` URL
//     or null (a workspace icon URL would leak a local address and can't
//     load in the frame anyway). `color` is `#rrggbb` or null; `image` is the
//     agent's picture only as a PNG/JPEG/WebP/GIF data: URL ≤ 256 KiB (never
//     SVG), else null — the App surface's rule. An offline server's rows read
//     `state: "unreachable"`, `activity: "unreachable"` (UWB8, UWB11);
//   - Thread items: `{seq, id, doc: {id, kind, from, to, created_at, body,
//     via, choice}}` for `text` and `choice`, plus `attachments` when the
//     message carries files (0.46.3): each the catalog's ThreadFile `{kind,
//     state, name?, size?, mime?, modifiedAt?, line?, lineEnd?}`, never its
//     workspace path or hash (UWA6); a `hidden` one is `{kind, state}` (no
//     name, type or size); a `secret` card keeps only
//     `{id, kind}` (widgets can't answer secrets); any other kind is left
//     out. `conversation_id` never crosses (it can be the pinned session id).
//     A post with a Board context (thread-context-v1, 0.46.3 r10) adds
//     `context: {text, widget, placement}` (never the Garden or the hash);
//   - a Thread view: `{address, phase, note, items, hasMore, turn?}` (`turn`
//     absent when the agent's server reports no Thread turns);
//   - a Thread turn (`thread.onActivity`, 0.46.5): `{address, turn, live,
//     source, fresh}`, where `turn` is the App guest's turn, key for key.

import type { OverlayChoice, OverlayThreadItem, ThreadChangesDoc, ThreadFileObject, ThreadTurn } from '@/components/SessionView/overlayThread'
import { coerceChanges } from '@/components/SessionView/overlayThread'
import type { UserWidgetCap } from '../k2-caps.generated'
import { agentLookColor, agentLookImage } from '../agent-look'
import type { ZenAgentRow, ZenPerson, ZenPreview, ZenThreadView } from './zen-data'
import { zenPermissionField, type ZenWidgetPermission } from './permission-sheet'

export type ZenWidgetActivity = 'working' | 'monitoring' | 'needs-you' | 'unverifiable' | 'idle' | 'unreachable'

export interface ZenWidgetPreview {
  text: string
  at: number | null
  from: string
  mine: boolean
  seq: number | null
}

export interface ZenWidgetPerson {
  user: string
  name: string
}

/** Live subagents and this turn's tool calls / shell commands. */
export interface ZenWidgetCounts {
  subagents: number
  tools: number
  commands: number
}

export interface ZenWidgetRow {
  address: string
  label: string
  index: number
  server: string | null
  state: string
  stateLabel: string | null
  detail: string | null
  working: boolean
  needsYou: boolean
  activity: ZenWidgetActivity | null
  counts: ZenWidgetCounts | null
  openable: boolean
  selected: boolean
  avatar: string | null
  /** The agent's color, `#rrggbb`, or null. */
  color: string | null
  /** The agent's picture (PNG/JPEG/WebP/GIF data: URL ≤ 256 KiB), or null. */
  image: string | null
  preview?: ZenWidgetPreview | null
  people?: ZenWidgetPerson[]
}

/** The row keys a widget gets with no optional cap (TUWA7). */
export const ZEN_WIDGET_ROW_KEYS = [
  'address',
  'label',
  'index',
  'server',
  'state',
  'stateLabel',
  'detail',
  'working',
  'needsYou',
  'activity',
  'counts',
  'openable',
  'selected',
  'avatar',
  'color',
  'image',
] as const

function projectPreview(p: ZenPreview | null): ZenWidgetPreview | null {
  if (!p) return null
  return { text: p.text, at: p.at, from: p.from, mine: p.mine, seq: p.seq }
}

function projectPerson(p: ZenPerson): ZenWidgetPerson {
  return { user: p.user, name: p.name }
}

/** One agent row as a widget sees it, by the widget's effective caps. */
export function projectZenRow(row: ZenAgentRow, caps: ReadonlySet<UserWidgetCap | string>): ZenWidgetRow {
  const unreachable = row.state === 'offline'
  const out: ZenWidgetRow = {
    address: row.address,
    label: row.label,
    index: row.index,
    server: row.server,
    state: unreachable ? 'unreachable' : row.state,
    stateLabel: row.stateLabel,
    detail: row.detail,
    working: row.working,
    needsYou: row.needsYou,
    activity: unreachable ? 'unreachable' : row.activity,
    // Built key by key: three numbers, nothing else crosses.
    counts:
      unreachable || !row.counts
        ? null
        : { subagents: row.counts.subagents, tools: row.counts.tools, commands: row.counts.commands },
    openable: row.openable,
    selected: row.selected,
    avatar: typeof row.avatarUrl === 'string' && row.avatarUrl.startsWith('data:') ? row.avatarUrl : null,
    // Checked again here: the port never trusts the row's own fields.
    color: agentLookColor(row.color),
    image: agentLookImage(row.image),
  }
  if (caps.has('thread:read')) out.preview = projectPreview(row.preview)
  if (caps.has('presence:read')) out.people = row.people.map(projectPerson)
  return out
}

export function projectZenRows(rows: readonly ZenAgentRow[], caps: ReadonlySet<string>): ZenWidgetRow[] {
  return rows.map((r) => projectZenRow(r, caps))
}

export function projectZenPeople(people: readonly ZenPerson[]): ZenWidgetPerson[] {
  return people.map(projectPerson)
}

export interface ZenWidgetCardAnswerer {
  kind: string
  login: string
  /** The owner's display name (owner answers only), else null. */
  name: string | null
}

export interface ZenWidgetChoice {
  prompt: string
  options: Array<{ label: string }>
  allow_custom: boolean
  status: string
  answer: string | null
  /** Who answered or dismissed it; null while pending / an agent's own answer. */
  answeredBy: ZenWidgetCardAnswerer | null
  /** Unix seconds a pending card expires; null when it never does. */
  expiresAt: number | null
  /** 0.46.3: what an answered settings card's action did (shown under the
   *  chosen option). Absent on every other card. */
  resultMessage?: string
  /** 0.46.3 permission cards (Beta): K2's words for the card, closed shape
   *  (`permission-sheet.ts`). The kit's one button opens K2's sheet outside
   *  the frame; a widget never answers it. Absent on every other card. */
  permission?: ZenWidgetPermission
}

/** One file on a message as a widget sees it (the catalog's ThreadFile). */
export interface ZenWidgetThreadFile {
  kind: string
  state: string
  name?: string
  size?: number
  mime?: string
  modifiedAt?: number
  line?: number
  lineEnd?: number
}

const FILE_STATES: ReadonlySet<string> = new Set(['ok', 'changed', 'gone', 'hidden'])

/** One attachment, key by key: no path, no sha256; hidden keeps kind and state. */
export function projectZenThreadFile(f: ThreadFileObject): ZenWidgetThreadFile {
  const kind = typeof f.kind === 'string' && f.kind ? f.kind : 'file'
  const state = typeof f.state === 'string' && FILE_STATES.has(f.state) ? f.state : 'ok'
  if (state === 'hidden') return { kind, state }
  const out: ZenWidgetThreadFile = { kind, state }
  if (typeof f.name === 'string') out.name = f.name
  if (typeof f.size === 'number' && Number.isFinite(f.size)) out.size = f.size
  if (typeof f.mime === 'string') out.mime = f.mime
  if (typeof f.modifiedAt === 'number' && Number.isFinite(f.modifiedAt)) out.modifiedAt = f.modifiedAt
  if (typeof f.line === 'number' && Number.isInteger(f.line)) out.line = f.line
  if (typeof f.lineEnd === 'number' && Number.isInteger(f.lineEnd)) out.lineEnd = f.lineEnd
  return out
}

export interface ZenWidgetThreadItem {
  seq: number
  id: string
  doc:
    | {
        id: string
        kind: 'text' | 'choice' | 'changes'
        from: string
        to: string | null
        created_at?: number
        body: string | null
        via: string | null
        choice: ZenWidgetChoice | null
        attachments?: ZenWidgetThreadFile[]
        /** thread-context-v1: the Board context that rode the person's post
         *  (a widget's line, never theirs). Absent on every other item. */
        context?: ZenWidgetThreadContext
        /** workbench-v1 B7: K2's changed-files card (kind `changes`, from
         *  `k2`), read from git. Absent on every other item. */
        changes?: ThreadChangesDoc
      }
    | { id: string; kind: 'secret' }
}

/** A post's Board context as a widget sees it: the text and which widget
 *  set it (its folder name and placement), key by key. */
export interface ZenWidgetThreadContext {
  text: string
  widget: string
  placement: string
}

function projectContext(c: unknown): ZenWidgetThreadContext | null {
  if (!c || typeof c !== 'object' || Array.isArray(c)) return null
  const r = c as Record<string, unknown>
  if (typeof r.text !== 'string' || typeof r.widget !== 'string' || typeof r.placement !== 'string') return null
  return { text: r.text, widget: r.widget, placement: r.placement }
}

function projectChoice(c: OverlayChoice | null | undefined, from = ''): ZenWidgetChoice | null {
  if (!c) return null
  return {
    prompt: c.prompt,
    options: c.options.map((o) => ({ label: o.label })),
    allow_custom: c.allow_custom,
    status: c.status,
    answer: c.answer ?? null,
    // Re-checked here: the port never trusts the doc's own fields.
    answeredBy: projectAnswerer(c.answeredBy),
    expiresAt: typeof c.expiresAt === 'number' && Number.isInteger(c.expiresAt) ? c.expiresAt : null,
    ...(typeof c.resultMessage === 'string' && c.resultMessage ? { resultMessage: c.resultMessage.slice(0, 300) } : {}),
    ...zenPermissionField(c, from, Math.floor(Date.now() / 1000)),
  }
}

function projectAnswerer(raw: unknown): ZenWidgetCardAnswerer | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const r = raw as Record<string, unknown>
  const str = (v: unknown): string => (typeof v === 'string' ? v.trim() : '')
  const kind = str(r.kind)
  const login = str(r.login)
  if (!kind || !login) return null
  const name = kind === 'owner' ? str(r.name) : ''
  return { kind, login, name: name || null }
}

/** One Thread item, or null for a kind widgets don't get. */
export function projectZenThreadItem(item: OverlayThreadItem): ZenWidgetThreadItem | null {
  const d = item.doc
  if (d.kind === 'secret') return { seq: item.seq, id: item.id, doc: { id: d.id, kind: 'secret' } }
  if (d.kind !== 'text' && d.kind !== 'choice' && d.kind !== 'changes') return null
  const context = d.kind === 'text' ? projectContext(d.context) : null
  // Re-read into the closed ThreadChanges shape: never the doc's own object.
  const changes = d.kind === 'changes' ? coerceChanges(d.changes) : null
  return {
    seq: item.seq,
    id: item.id,
    doc: {
      id: d.id,
      kind: d.kind,
      from: d.from,
      to: d.to ?? null,
      ...(typeof d.created_at === 'number' ? { created_at: d.created_at } : {}),
      body: d.body ?? null,
      via: d.via ?? null,
      choice: d.kind === 'choice' ? projectChoice(d.choice, d.from) : null,
      ...(Array.isArray(d.attachments) && d.attachments.length > 0
        ? { attachments: d.attachments.map(projectZenThreadFile) }
        : {}),
      ...(context ? { context } : {}),
      ...(changes ? { changes } : {}),
    },
  }
}

export function projectZenThreadItems(items: readonly OverlayThreadItem[]): ZenWidgetThreadItem[] {
  const out: ZenWidgetThreadItem[] = []
  for (const it of items) {
    const p = projectZenThreadItem(it)
    if (p) out.push(p)
  }
  return out
}

/** A thread-delta-v1 read (`thread.read` with `afterSeq`). */
export interface ZenWidgetThreadDelta {
  items: ZenWidgetThreadItem[]
  hasMore: boolean
  changed: ZenWidgetThreadItem[]
  removed: Array<{ id: string; seq: number }>
}

export function projectZenThreadDelta(d: {
  items: readonly OverlayThreadItem[]
  hasMore: boolean
  changed: readonly OverlayThreadItem[]
  removed: ReadonlyArray<{ id: string; seq: number }>
}): ZenWidgetThreadDelta {
  return {
    items: projectZenThreadItems(d.items),
    hasMore: d.hasMore === true,
    changed: projectZenThreadItems(d.changed),
    removed: d.removed.map((r) => ({ id: r.id, seq: r.seq })),
  }
}

export interface ZenWidgetThreadView {
  address: string
  phase: string
  note: string | null
  items: ZenWidgetThreadItem[]
  hasMore: boolean
  /** Absent when the agent's server doesn't report Thread turns (the
   *  catalog's ThreadView, `k2.d.ts`): never null for "unknown". */
  turn?: { state: string; since: number } | null
}

export function projectZenThreadView(v: ZenThreadView): ZenWidgetThreadView {
  return {
    address: v.address,
    phase: v.phase,
    note: v.note,
    items: projectZenThreadItems(v.items),
    hasMore: v.hasMore,
    // 0.46.5 (review B4): `undefined` (no turns on that server) stays
    // absent; only a server that reports turns says null.
    ...(v.turn === undefined ? {} : { turn: v.turn ? { state: v.turn.state, since: v.turn.since } : null }),
  }
}

// ── The Thread turn as an App guest sees it (0.46.5 activity SDK) ─────────
//
// `k2.thread.onActivity` hands a widget the SAME turn an App guest with
// `thread:read` + `activity:read` gets from `thread.activity` and its
// `thread_activity` frames: the daemon's `guest_turn_body`
// (thread_activity.rs), key for key, built fresh from an allow list.
// Strings only from fixed word lists; no tool line, child label, end
// detail, path or text. The shared fixture
// `tests/fixtures/activity/guest-turn-parity.json` holds both sides to it.

/** The exact keys of a guest turn (`GUEST_TURN_KEYS`, thread_activity.rs). */
export const ZEN_ACTIVITY_TURN_KEYS = [
  'turnId',
  'rev',
  'state',
  'phase',
  'phaseSince',
  'startedAt',
  'serverNow',
  'tally',
  'subagents',
  'subagentsDone',
  'background',
  'messages',
  'waitingOn',
  'end',
  // Frame schema 2 (0.46.5): closed words, counts and a coarse clock.
  'schema',
  'stepKind',
  'childCounts',
  'openBecause',
  'evidenceAt',
] as const

/** The daemon's `TURN_SCHEMA`: every guest turn says 2. */
export const ZEN_ACTIVITY_TURN_SCHEMA = 2
/** `GUEST_EVIDENCE_GRAIN_MS`: a guest's evidence clock is floored to 10 s. */
export const ZEN_ACTIVITY_EVIDENCE_GRAIN_MS = 10_000
const ZEN_STEP_KINDS = ['read', 'search', 'cmd', 'edit', 'web', 'subagent', 'tool'] as const
const ZEN_OPEN_BECAUSE = ['lead', 'children', 'waiting', 'stale'] as const

/** The catalog's AppThreadTurn. */
export interface ZenWidgetActivityTurn {
  schema: number
  stepKind: (typeof ZEN_STEP_KINDS)[number] | null
  childCounts: { subagent: number; shell: number; monitor: number; task: number }
  openBecause: (typeof ZEN_OPEN_BECAUSE)[number] | null
  evidenceAt: number | null
  turnId: string
  rev: number
  state: string
  phase: string
  phaseSince: number | null
  startedAt: number | null
  serverNow: number | null
  tally: { read: number; search: number; cmd: number; edit: number }
  subagents: number
  subagentsDone: number
  background: number
  messages: number
  waitingOn: 'owner' | null
  end: { reason: 'done' | 'stopped' | 'failed' } | null
}

/** The catalog's ThreadActivityState (`thread.onActivity`). */
export interface ZenWidgetActivityState {
  address: string
  turn: ZenWidgetActivityTurn | null
  live: boolean
  source: 'thread' | 'none'
  fresh: 'live' | 'catching-up' | 'retrying'
}

/** `coarse_end_reason` (thread_activity.rs). */
export function zenCoarseEndReason(reason: string | null | undefined): 'done' | 'stopped' | 'failed' {
  if (reason === 'reply' || reason === 'done') return 'done'
  if (reason === 'interrupted' || reason === 'superseded' || reason === 'stale') return 'stopped'
  return 'failed'
}

function whole(n: number | undefined): number {
  return typeof n === 'number' && Number.isFinite(n) && n > 0 ? Math.floor(n) : 0
}

function msOrNull(n: number | undefined): number | null {
  return typeof n === 'number' && Number.isFinite(n) ? Math.trunc(n) : null
}

/** One owner turn (the feed's, coerced by `coerceThreadTurn`) as an App
 *  guest reads it: `guest_turn_body`'s twin. */
export function projectZenActivityTurn(t: ThreadTurn): ZenWidgetActivityTurn {
  return {
    turnId: t.turnId,
    rev: Math.trunc(t.rev),
    state: t.state,
    phase: t.phase,
    phaseSince: msOrNull(t.phaseSince),
    startedAt: msOrNull(t.startedAt),
    serverNow: msOrNull(t.serverNow),
    tally: { read: whole(t.tally.read), search: whole(t.tally.search), cmd: whole(t.tally.cmd), edit: whole(t.tally.edit) },
    subagents: whole(t.subagents),
    subagentsDone: whole(t.subagentsDone),
    background: whole(t.background),
    messages: whole(t.messages),
    // Rosson 2026-10-21: "waiting on the owner", never which prompt.
    waitingOn: t.waitingOn !== null ? 'owner' : null,
    end: t.end ? { reason: zenCoarseEndReason(t.end.reason) } : null,
    schema: ZEN_ACTIVITY_TURN_SCHEMA,
    stepKind: ZEN_STEP_KINDS.find((k) => k === t.stepKind) ?? null,
    childCounts: {
      subagent: whole(t.childCounts?.subagent),
      shell: whole(t.childCounts?.shell),
      monitor: whole(t.childCounts?.monitor),
      task: whole(t.childCounts?.task),
    },
    openBecause: ZEN_OPEN_BECAUSE.find((k) => k === t.openBecause) ?? null,
    evidenceAt: floorTo(msOrNull(t.evidenceAt ?? undefined), ZEN_ACTIVITY_EVIDENCE_GRAIN_MS),
  }
}

/** `t - t.rem_euclid(grain)` (floors negative times too). */
function floorTo(t: number | null, grain: number): number | null {
  return t === null ? null : t - (((t % grain) + grain) % grain)
}

/** The turn id a subscriber must not show as an end frame: the turn that
 *  had already ended when it subscribed (its snapshot reads as no turn, as
 *  `thread.activity` answers null once a turn is over). Null when the
 *  first view had no ended turn. */
export function zenEndedAtSubscribe(v: ZenThreadView): string | null {
  const t = v.liveTurn
  return t && t.end ? t.turnId : null
}

/** What `k2.thread.onActivity` pushes for a Thread view. `endedBefore`
 *  ({@link zenEndedAtSubscribe} of the first view): that ended turn reads
 *  as none; any other end frame is pushed as is (the App socket's end
 *  frame), and the caller pushes a value only when it changed. */
export function projectZenActivityState(v: ZenThreadView, endedBefore: string | null): ZenWidgetActivityState {
  const reported = v.liveTurn !== undefined
  const raw = reported ? v.liveTurn ?? null : null
  const turn = raw && !(raw.end && raw.turnId === endedBefore) ? projectZenActivityTurn(raw) : null
  const fresh: ZenWidgetActivityState['fresh'] = v.error ? 'retrying' : v.phase === 'ready' && v.loaded ? 'live' : 'catching-up'
  return { address: v.address, turn, live: !!turn && turn.end === null, source: reported ? 'thread' : 'none', fresh }
}
