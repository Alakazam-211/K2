// Where everything sits on the Org Chart canvas (prd-org-chart-v1 §6.6).
//
// The daemon lays the chart out (`org_chart::layout`) and sends the result
// as `nodes`: that is the truth, and `layoutFromNodes` draws it. An org's
// node is its bucket's canvas box; a member's x/y is relative to its bucket
// (so a bucket's members move with it); a loose agent's is on the canvas.
//
// A node the daemon hasn't placed yet (a card just dropped into a bucket, an
// org just made, both shown before the daemon answers) gets the same rules
// the daemon uses, from `layoutChart` below: the next free cell of its
// bucket, which grows to hold it, or the end of the row. The next
// `org_chart_changed` read replaces it with the daemon's spot.
//
// `layoutChart` is that rule set for a whole chart (the test daemon in
// org-api.fake.ts lays its charts out with it):
//   - cards are one size (220×72);
//   - a bucket grows with its members: cols = ceil(sqrt(n)) (at most 4),
//     rows follow; head first, then by name; a member a person dragged keeps
//     its spot inside the bucket, auto members fill the free cells around it;
//   - buckets stand in rows by their `below` lines (between the orgs, or
//     between agents in them), top to bottom, by name within a row; a
//     bucket with no lines goes in the top row, to the right;
//   - a bucket a person placed never moves; auto buckets step right past it;
//   - agents in no org sit in a "Not in an org" row under the buckets, and
//     agents on other servers in a row under that.

import type { OrgChart, OrgPlacement } from './org-api'

export const CARD_W = 220
export const CARD_H = 72
export const CARD_GAP = 16
export const BUCKET_PAD = 16
export const BUCKET_TITLE_H = 40
export const BUCKET_GAP = 64
export const ROW_GAP = 96
export const MAX_BUCKET_COLS = 4
export const LOOSE_COLS = 6
export const LABEL_H = 28

export interface LayoutBox {
  x: number
  y: number
  w: number
  h: number
}

export interface PlacedAgent extends LayoutBox {
  id: string
  orgId: string | null
  /** A person dragged it here. */
  pinned: boolean
}

export interface PlacedOrg extends LayoutBox {
  id: string
  pinned: boolean
}

export interface OrgLayout {
  orgs: Map<string, PlacedOrg>
  agents: Map<string, PlacedAgent>
  labels: { text: string; x: number; y: number }[]
  bounds: LayoutBox
}

export const LOOSE_LABEL = 'Not in an org'
export const REMOTE_LABEL = 'On other servers'

const overlaps = (a: LayoutBox, b: LayoutBox, pad = 0): boolean =>
  a.x < b.x + b.w + pad && a.x + a.w + pad > b.x && a.y < b.y + b.h + pad && a.y + a.h + pad > b.y

const byName = (names: Map<string, string>) => (a: string, b: string): number => {
  const na = (names.get(a) ?? a).toLowerCase()
  const nb = (names.get(b) ?? b).toLowerCase()
  return na < nb ? -1 : na > nb ? 1 : a < b ? -1 : a > b ? 1 : 0
}

/** The bucket's size for `n` members, before any person-placed member
 *  stretches it (an empty bucket keeps room for one card: "Drop agents here"). */
export function bucketSize(n: number): { w: number; h: number; cols: number } {
  const count = Math.max(1, n)
  const cols = Math.min(MAX_BUCKET_COLS, Math.ceil(Math.sqrt(count)))
  const rows = Math.ceil(count / cols)
  return {
    cols,
    w: BUCKET_PAD * 2 + cols * CARD_W + (cols - 1) * CARD_GAP,
    h: BUCKET_TITLE_H + BUCKET_PAD * 2 + rows * CARD_H + (rows - 1) * CARD_GAP,
  }
}

/** The free grid cell `i` of a bucket with `cols` columns, relative to it. */
function cell(i: number, cols: number): { x: number; y: number } {
  return {
    x: BUCKET_PAD + (i % cols) * (CARD_W + CARD_GAP),
    y: BUCKET_TITLE_H + BUCKET_PAD + Math.floor(i / cols) * (CARD_H + CARD_GAP),
  }
}

export function layoutChart(chart: OrgChart): OrgLayout {
  const names = new Map(chart.agents.map((a) => [a.id, a.name]))
  const orgNames = new Map(chart.orgs.map((o) => [o.id, o.name]))
  const person = new Map<string, OrgPlacement>()
  for (const n of chart.nodes) if (n.placedBy === 'person') person.set(`${n.kind}:${n.id}`, n)
  const agentIds = new Set(chart.agents.map((a) => a.id))
  const orgOf = new Map<string, string>()
  for (const o of chart.orgs) for (const m of o.memberIds) if (agentIds.has(m) && !orgOf.has(m)) orgOf.set(m, o.id)

  // 1. Each bucket's inside: members relative to the bucket, and its size.
  const inside = new Map<string, { w: number; h: number; members: PlacedAgent[] }>()
  for (const o of chart.orgs) {
    const members = o.memberIds.filter((m) => orgOf.get(m) === o.id)
    const order = [...members].sort((a, b) => (a === o.headId ? -1 : b === o.headId ? 1 : byName(names)(a, b)))
    const base = bucketSize(order.length)
    const placed: PlacedAgent[] = []
    for (const id of order) {
      const p = person.get(`agent:${id}`)
      if (p) placed.push({ id, orgId: o.id, x: Math.max(BUCKET_PAD, p.x), y: Math.max(BUCKET_TITLE_H, p.y), w: CARD_W, h: CARD_H, pinned: true })
    }
    let i = 0
    for (const id of order) {
      if (person.has(`agent:${id}`)) continue
      let c = cell(i, base.cols)
      while (placed.some((q) => overlaps({ ...c, w: CARD_W, h: CARD_H }, q))) c = cell(++i, base.cols)
      placed.push({ id, orgId: o.id, ...c, w: CARD_W, h: CARD_H, pinned: false })
      i++
    }
    let w = base.w
    let h = base.h
    for (const q of placed) {
      w = Math.max(w, q.x + q.w + BUCKET_PAD)
      h = Math.max(h, q.y + q.h + BUCKET_PAD)
    }
    const own = person.get(`org:${o.id}`)
    if (own?.w) w = Math.max(w, own.w)
    if (own?.h) h = Math.max(h, own.h)
    inside.set(o.id, { w, h, members: placed })
  }

  // 2. Bucket rows from `below` lines between orgs (an agent end counts as its org).
  const endOrg = (r: { kind: string; id: string }): string | null =>
    r.kind === 'org' ? (orgNames.has(r.id) ? r.id : null) : orgOf.get(r.id) ?? null
  const down = new Map<string, Set<string>>()
  const linked = new Set<string>()
  for (const l of chart.lines) {
    if (l.kind !== 'below') continue
    const a = endOrg(l.a)
    const b = endOrg(l.b)
    if (!a || !b || a === b) continue
    if (!down.has(a)) down.set(a, new Set())
    down.get(a)!.add(b)
    linked.add(a)
    linked.add(b)
  }
  const level = new Map<string, number>()
  // Longest path from a top bucket; a bounded walk so a stray loop can't hang it.
  const ids = chart.orgs.map((o) => o.id).sort(byName(orgNames))
  for (const id of ids) level.set(id, 0)
  for (let pass = 0; pass < ids.length; pass++) {
    let changed = false
    for (const a of ids) for (const b of down.get(a) ?? []) {
      const next = (level.get(a) ?? 0) + 1
      if (next > (level.get(b) ?? 0) && next < ids.length) {
        level.set(b, next)
        changed = true
      }
    }
    if (!changed) break
  }
  const rows: string[][] = []
  for (const id of ids.filter((x) => linked.has(x))) {
    const lv = level.get(id) ?? 0
    ;(rows[lv] ??= []).push(id)
  }
  ;(rows[0] ??= []).push(...ids.filter((x) => !linked.has(x)))

  const orgs = new Map<string, PlacedOrg>()
  for (const o of chart.orgs) {
    const p = person.get(`org:${o.id}`)
    const s = inside.get(o.id)!
    if (p) orgs.set(o.id, { id: o.id, x: p.x, y: p.y, w: s.w, h: s.h, pinned: true })
  }
  const pinnedOrgs = [...orgs.values()]
  let y = 0
  for (const row of rows) {
    if (!row || !row.length) continue
    let x = 0
    let rowH = 0
    for (const id of row) {
      if (orgs.has(id)) continue
      const s = inside.get(id)!
      let box = { x, y, w: s.w, h: s.h }
      for (let guard = 0; guard < pinnedOrgs.length + 1; guard++) {
        const hit = pinnedOrgs.find((q) => overlaps(box, q, BUCKET_GAP / 2))
        if (!hit) break
        box = { ...box, x: hit.x + hit.w + BUCKET_GAP }
      }
      orgs.set(id, { id, ...box, pinned: false })
      x = box.x + box.w + BUCKET_GAP
      rowH = Math.max(rowH, s.h)
    }
    if (rowH) y += rowH + ROW_GAP
  }

  // 3. Absolute boxes for members.
  const agents = new Map<string, PlacedAgent>()
  for (const o of chart.orgs) {
    const b = orgs.get(o.id)!
    for (const m of inside.get(o.id)!.members) agents.set(m.id, { ...m, x: b.x + m.x, y: b.y + m.y })
  }

  // 4. Agents in no org, then agents on other servers, each in a row of cards.
  const labels: OrgLayout['labels'] = []
  let bottom = 0
  for (const b of orgs.values()) bottom = Math.max(bottom, b.y + b.h)
  const taken = (): LayoutBox[] => [...orgs.values(), ...agents.values()]
  const flow = (list: string[], label: string): void => {
    if (!list.length) return
    const top = (bottom ? bottom + ROW_GAP : 0)
    labels.push({ text: label, x: 0, y: top })
    const start = top + LABEL_H
    let i = 0
    for (const id of list) {
      const p = person.get(`agent:${id}`)
      if (p && label === LOOSE_LABEL) {
        agents.set(id, { id, orgId: null, x: p.x, y: p.y, w: CARD_W, h: CARD_H, pinned: true })
        continue
      }
      const at = (k: number): LayoutBox => ({
        x: (k % LOOSE_COLS) * (CARD_W + CARD_GAP),
        y: start + Math.floor(k / LOOSE_COLS) * (CARD_H + CARD_GAP),
        w: CARD_W,
        h: CARD_H,
      })
      let box = at(i)
      while (taken().some((q) => overlaps(box, q, CARD_GAP / 2))) box = at(++i)
      agents.set(id, { id, orgId: null, ...box, pinned: false })
      i++
    }
    for (const a of agents.values()) bottom = Math.max(bottom, a.y + a.h)
  }
  const loose = chart.agents.filter((a) => a.server === null && !orgOf.has(a.id)).map((a) => a.id).sort(byName(names))
  const remote = chart.agents.filter((a) => a.server !== null && !orgOf.has(a.id)).map((a) => a.id).sort(byName(names))
  flow(loose, LOOSE_LABEL)
  flow(remote, REMOTE_LABEL)

  const all = [...orgs.values(), ...agents.values()]
  let x1 = 0
  let y1 = 0
  let x2 = 0
  let y2 = 0
  if (all.length) {
    x1 = Math.min(...all.map((b) => b.x))
    y1 = Math.min(...all.map((b) => b.y))
    x2 = Math.max(...all.map((b) => b.x + b.w))
    y2 = Math.max(...all.map((b) => b.y + b.h))
  }
  return { orgs, agents, labels, bounds: { x: x1, y: y1, w: x2 - x1, h: y2 - y1 } }
}

/** The bucket a world point falls in (the topmost when person-placed ones overlap). */
export function bucketAt(layout: OrgLayout, x: number, y: number): string | null {
  let hit: string | null = null
  for (const b of layout.orgs.values()) if (x >= b.x && x <= b.x + b.w && y >= b.y && y <= b.y + b.h) hit = b.id
  return hit
}

/** The chart as the daemon laid it out (`nodes`), plus the daemon's rules
 *  for any node it hasn't placed yet. */
export function layoutFromNodes(chart: OrgChart): OrgLayout {
  const cw = chart.card.w
  const ch = chart.card.h
  const node = new Map<string, OrgPlacement>()
  for (const n of chart.nodes) node.set(`${n.kind}:${n.id}`, n)
  const agentIds = new Set(chart.agents.map((a) => a.id))
  const orgOf = new Map<string, string>()
  for (const o of chart.orgs) for (const m of o.memberIds) if (agentIds.has(m) && !orgOf.has(m)) orgOf.set(m, o.id)

  // 1. Members relative to their bucket: placed ones as sent, new ones in the next free cell.
  const inside = new Map<string, PlacedAgent[]>()
  for (const o of chart.orgs) {
    const members = o.memberIds.filter((m) => orgOf.get(m) === o.id)
    const cols = bucketSize(members.length).cols
    const placed: PlacedAgent[] = []
    for (const id of members) {
      const n = node.get(`agent:${id}`)
      if (n) placed.push({ id, orgId: o.id, x: n.x, y: n.y, w: cw, h: ch, pinned: n.placedBy === 'person' })
    }
    let i = 0
    for (const id of members) {
      if (node.has(`agent:${id}`)) continue
      let c = cell(i, cols)
      while (placed.some((q) => overlaps({ ...c, w: cw, h: ch }, q))) c = cell(++i, cols)
      placed.push({ id, orgId: o.id, ...c, w: cw, h: ch, pinned: false })
      i++
    }
    inside.set(o.id, placed)
  }

  // 2. Buckets: the daemon's box, grown to hold anything not yet placed; a
  //    new org at the end of the top row.
  const orgs = new Map<string, PlacedOrg>()
  const fit = (id: string, x: number, y: number, w: number, h: number, pinned: boolean): void => {
    for (const q of inside.get(id) ?? []) {
      w = Math.max(w, q.x + q.w + BUCKET_PAD)
      h = Math.max(h, q.y + q.h + BUCKET_PAD)
    }
    orgs.set(id, { id, x, y, w, h, pinned })
  }
  for (const o of chart.orgs) {
    const n = node.get(`org:${o.id}`)
    if (!n) continue
    const size = bucketSize(o.memberIds.length)
    fit(o.id, n.x, n.y, n.w ?? size.w, n.h ?? size.h, n.placedBy === 'person')
  }
  for (const o of chart.orgs) {
    if (orgs.has(o.id)) continue
    const right = Math.max(0, ...[...orgs.values()].map((b) => b.x + b.w + BUCKET_GAP))
    const size = bucketSize(o.memberIds.length)
    fit(o.id, orgs.size ? right : 0, 0, size.w, size.h, false)
  }

  // 3. Absolute boxes for members; loose agents where the daemon put them.
  const agents = new Map<string, PlacedAgent>()
  for (const o of chart.orgs) {
    const b = orgs.get(o.id)!
    for (const m of inside.get(o.id)!) agents.set(m.id, { ...m, x: b.x + m.x, y: b.y + m.y })
  }
  const unplaced: string[] = []
  for (const a of chart.agents) {
    if (orgOf.has(a.id)) continue
    const n = node.get(`agent:${a.id}`)
    if (n) agents.set(a.id, { id: a.id, orgId: null, x: n.x, y: n.y, w: cw, h: ch, pinned: n.placedBy === 'person' })
    else unplaced.push(a.id)
  }
  // An agent the daemon hasn't placed (it just left its org with no drop
  // spot): a row under everything.
  if (unplaced.length) {
    const all = [...orgs.values(), ...agents.values()]
    const top = all.length ? Math.max(...all.map((b) => b.y + b.h)) + ROW_GAP : 0
    unplaced.forEach((id, k) =>
      agents.set(id, {
        id,
        orgId: null,
        x: (k % LOOSE_COLS) * (cw + CARD_GAP),
        y: top + Math.floor(k / LOOSE_COLS) * (ch + CARD_GAP),
        w: cw,
        h: ch,
        pinned: false,
      }),
    )
  }

  // 4. The two row labels sit over their cards.
  const labels: OrgLayout['labels'] = []
  const label = (ids: string[], text: string): void => {
    const boxes = ids.map((id) => agents.get(id)).filter((b): b is PlacedAgent => !!b)
    if (!boxes.length) return
    labels.push({ text, x: Math.min(...boxes.map((b) => b.x)), y: Math.min(...boxes.map((b) => b.y)) - LABEL_H })
  }
  label(chart.agents.filter((a) => a.server === null && !orgOf.has(a.id)).map((a) => a.id), LOOSE_LABEL)
  label(chart.agents.filter((a) => a.server !== null && !orgOf.has(a.id)).map((a) => a.id), REMOTE_LABEL)

  const all = [...orgs.values(), ...agents.values()]
  if (!all.length) return { orgs, agents, labels, bounds: { x: 0, y: 0, w: 0, h: 0 } }
  const x1 = Math.min(...all.map((b) => b.x))
  const y1 = Math.min(...all.map((b) => b.y))
  const x2 = Math.max(...all.map((b) => b.x + b.w))
  const y2 = Math.max(...all.map((b) => b.y + b.h))
  return { orgs, agents, labels, bounds: { x: x1, y: y1, w: x2 - x1, h: y2 - y1 } }
}

/** The daemon's `nodes` for a chart laid out by `layoutChart` (members
 *  relative to their bucket), keeping who placed each one. The test daemon
 *  answers with these. */
export function nodesOf(chart: OrgChart): OrgPlacement[] {
  const L = layoutChart(chart)
  const by = (kind: 'agent' | 'org', id: string): 'auto' | 'person' =>
    chart.nodes.find((n) => n.kind === kind && n.id === id && n.placedBy === 'person') ? 'person' : 'auto'
  const out: OrgPlacement[] = []
  for (const o of L.orgs.values()) out.push({ kind: 'org', id: o.id, x: o.x, y: o.y, w: o.w, h: o.h, placedBy: by('org', o.id) })
  for (const a of L.agents.values()) {
    const b = a.orgId ? L.orgs.get(a.orgId)! : null
    out.push({ kind: 'agent', id: a.id, x: b ? a.x - b.x : a.x, y: b ? a.y - b.y : a.y, w: null, h: null, placedBy: by('agent', a.id) })
  }
  return out
}
