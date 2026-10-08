// prd-zen-user-widgets-v2 UWA6, UW71, UWB11 — the guest projection: what a
// custom widget gets back from a verb, never K2's internal shapes.
//
// Every value that crosses the port is built here from scratch, key by key
// (closed shapes), so a new field on `ZenAgentRow` or a Thread item can't
// leak by accident (TUWA7 pins the keys):
//   - agent rows: `{address, label, index, server, state, stateLabel, detail,
//     working, needsYou, activity, openable, selected, avatar}`, plus
//     `preview` with `thread:read` and `people` with `presence:read`.
//     Dropped: `hostKey`, `role`, `reach`, `auth`. `avatar` is a `data:` URL
//     or null (a workspace icon URL would leak a local address and can't
//     load in the frame anyway). An offline server's rows read
//     `state: "unreachable"`, `activity: "unreachable"` (UWB8, UWB11);
//   - Thread items: `{seq, id, doc: {id, kind, from, to, created_at, body,
//     via, choice}}` for `text` and `choice`; a `secret` card keeps only
//     `{id, kind}` (widgets can't answer secrets); any other kind is left
//     out. `conversation_id` never crosses (it can be the pinned session id);
//   - a Thread view: `{address, phase, note, items, hasMore, turn}`.

import type { OverlayChoice, OverlayThreadItem } from '@/components/SessionView/overlayThread'
import type { UserWidgetCap } from '../k2-caps.generated'
import type { ZenAgentRow, ZenPerson, ZenPreview, ZenThreadView } from './zen-data'

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
  openable: boolean
  selected: boolean
  avatar: string | null
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
  'openable',
  'selected',
  'avatar',
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
    openable: row.openable,
    selected: row.selected,
    avatar: typeof row.avatarUrl === 'string' && row.avatarUrl.startsWith('data:') ? row.avatarUrl : null,
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

export interface ZenWidgetChoice {
  prompt: string
  options: Array<{ label: string }>
  allow_custom: boolean
  status: string
  answer: string | null
}

export interface ZenWidgetThreadItem {
  seq: number
  id: string
  doc:
    | {
        id: string
        kind: 'text' | 'choice'
        from: string
        to: string | null
        created_at: number | null
        body: string | null
        via: string | null
        choice: ZenWidgetChoice | null
      }
    | { id: string; kind: 'secret' }
}

function projectChoice(c: OverlayChoice | null | undefined): ZenWidgetChoice | null {
  if (!c) return null
  return {
    prompt: c.prompt,
    options: c.options.map((o) => ({ label: o.label })),
    allow_custom: c.allow_custom,
    status: c.status,
    answer: c.answer ?? null,
  }
}

/** One Thread item, or null for a kind widgets don't get. */
export function projectZenThreadItem(item: OverlayThreadItem): ZenWidgetThreadItem | null {
  const d = item.doc
  if (d.kind === 'secret') return { seq: item.seq, id: item.id, doc: { id: d.id, kind: 'secret' } }
  if (d.kind !== 'text' && d.kind !== 'choice') return null
  return {
    seq: item.seq,
    id: item.id,
    doc: {
      id: d.id,
      kind: d.kind,
      from: d.from,
      to: d.to ?? null,
      created_at: typeof d.created_at === 'number' ? d.created_at : null,
      body: d.body ?? null,
      via: d.via ?? null,
      choice: d.kind === 'choice' ? projectChoice(d.choice) : null,
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

export interface ZenWidgetThreadView {
  address: string
  phase: string
  note: string | null
  items: ZenWidgetThreadItem[]
  hasMore: boolean
  turn: { state: string; since: number } | null
}

export function projectZenThreadView(v: ZenThreadView): ZenWidgetThreadView {
  return {
    address: v.address,
    phase: v.phase,
    note: v.note,
    items: projectZenThreadItems(v.items),
    hasMore: v.hasMore,
    turn: v.turn ? { state: v.turn.state, since: v.turn.since } : null,
  }
}
