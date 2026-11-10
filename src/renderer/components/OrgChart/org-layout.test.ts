// The canvas lays itself out (prd-org-chart-v1 §6.6): buckets grow with
// their members, a person's drag is kept, Tidy up puts everything back.
import { describe, expect, it } from 'vitest'
import {
  bucketSize,
  BUCKET_PAD,
  BUCKET_TITLE_H,
  CARD_GAP,
  CARD_H,
  CARD_W,
  layoutChart,
  LOOSE_LABEL,
  REMOTE_LABEL,
} from './org-layout'
import { orgChartFixture } from './org-chart-fixture'
import * as ops from './org-ops'
import type { OrgAgent, OrgChart } from './org-api'

function withAgents(n: number): OrgChart {
  const agents: OrgAgent[] = Array.from({ length: n }, (_, i) => ({
    id: `a${i}`,
    name: `Agent ${String(i).padStart(2, '0')}`,
    color: null,
    iconUrl: null,
    server: null,
    role: null,
  }))
  return { ...orgChartFixture(), agents, orgs: [{ id: 'o', name: 'Ops', purpose: '', headId: null, memberIds: [], bulletin: null }], connections: [] }
}

describe('bucket size', () => {
  it('grows with its members: 1 → one card, 4 → 2×2, 5 → 3 wide, capped at 4 columns', () => {
    const w = (cols: number): number => BUCKET_PAD * 2 + cols * CARD_W + (cols - 1) * CARD_GAP
    const h = (rows: number): number => BUCKET_TITLE_H + BUCKET_PAD * 2 + rows * CARD_H + (rows - 1) * CARD_GAP
    expect(bucketSize(1)).toEqual({ cols: 1, w: w(1), h: h(1) })
    expect(bucketSize(4)).toEqual({ cols: 2, w: w(2), h: h(2) })
    expect(bucketSize(5)).toEqual({ cols: 3, w: w(3), h: h(2) })
    expect(bucketSize(30).cols).toBe(4)
  })

  it('the layout follows: adding members makes the bucket bigger, removing makes it smaller', () => {
    let chart = withAgents(5)
    chart = ops.joinOrg(chart, 'o', 'a0')
    const one = layoutChart(chart).orgs.get('o')!
    for (const id of ['a1', 'a2', 'a3']) chart = ops.joinOrg(chart, 'o', id)
    const four = layoutChart(chart).orgs.get('o')!
    chart = ops.joinOrg(chart, 'o', 'a4')
    const five = layoutChart(chart).orgs.get('o')!
    expect([one.w, one.h]).toEqual([bucketSize(1).w, bucketSize(1).h])
    expect([four.w, four.h]).toEqual([bucketSize(4).w, bucketSize(4).h])
    expect([five.w, five.h]).toEqual([bucketSize(5).w, bucketSize(5).h])
    chart = ops.leaveOrg(chart, 'a4')
    expect(layoutChart(chart).orgs.get('o')!.w).toBe(four.w)
  })
})

describe('inside a bucket', () => {
  it('the head comes first, then by name', () => {
    let chart = withAgents(3)
    for (const id of ['a2', 'a0', 'a1']) chart = ops.joinOrg(chart, 'o', id)
    chart = ops.makeHead(chart, 'o', 'a2')
    const L = layoutChart(chart)
    const b = L.orgs.get('o')!
    const order = ['a2', 'a0', 'a1'].map((id) => L.agents.get(id)!)
    expect(order[0]).toMatchObject({ x: b.x + BUCKET_PAD, y: b.y + BUCKET_TITLE_H + BUCKET_PAD })
    expect(order[1]!.x).toBe(b.x + BUCKET_PAD + CARD_W + CARD_GAP)
    expect(order[2]!.y).toBeGreaterThan(order[0]!.y)
  })

  it("a member a person dragged keeps its spot; the others fill around it", () => {
    let chart = withAgents(2)
    chart = ops.joinOrg(chart, 'o', 'a0')
    chart = ops.joinOrg(chart, 'o', 'a1')
    chart = ops.place(chart, { kind: 'agent', id: 'a0' }, BUCKET_PAD, BUCKET_TITLE_H + BUCKET_PAD)
    chart = ops.place(chart, { kind: 'agent', id: 'a1' }, 300, 200)
    const L = layoutChart(chart)
    const b = L.orgs.get('o')!
    expect(L.agents.get('a1')).toMatchObject({ x: b.x + 300, y: b.y + 200, pinned: true })
    // The bucket stretches to hold it.
    expect(b.w).toBeGreaterThanOrEqual(300 + CARD_W + BUCKET_PAD)
    expect(b.h).toBeGreaterThanOrEqual(200 + CARD_H + BUCKET_PAD)
  })
})

describe('on the canvas', () => {
  it('a bucket a person placed stays put; Tidy up puts it back on auto', () => {
    const chart = ops.place(orgChartFixture(), { kind: 'org', id: 'org-support' }, 900, 40)
    const placed = layoutChart(chart).orgs.get('org-support')!
    expect(placed).toMatchObject({ x: 900, y: 40, pinned: true })
    const auto = layoutChart(orgChartFixture()).orgs.get('org-support')!
    const tidied = layoutChart(ops.tidy(chart)).orgs.get('org-support')!
    expect(tidied).toEqual(auto)
    expect(tidied.pinned).toBe(false)
  })

  it('an auto bucket that would land on a person-placed one steps right past it; the placed one never moves', () => {
    const base = orgChartFixture()
    const auto = layoutChart(base)
    const billing = auto.orgs.get('org-billing')!
    // Put Support where Billing would go.
    const chart = ops.place(base, { kind: 'org', id: 'org-support' }, billing.x, billing.y)
    const L = layoutChart(chart)
    const s = L.orgs.get('org-support')!
    const b = L.orgs.get('org-billing')!
    expect([s.x, s.y]).toEqual([billing.x, billing.y])
    expect(b.x).toBeGreaterThanOrEqual(s.x + s.w)
  })

  it('below lines put buckets in rows, top to bottom', () => {
    const chart = orgChartFixture({
      lines: [{ id: 'l', kind: 'below', a: { kind: 'org', id: 'org-support' }, b: { kind: 'org', id: 'org-billing' } }],
    })
    const L = layoutChart(chart)
    expect(L.orgs.get('org-billing')!.y).toBeGreaterThan(L.orgs.get('org-support')!.y + L.orgs.get('org-support')!.h)
  })

  it('agents in no org sit in a row under the buckets; agents on other servers under that', () => {
    const L = layoutChart(orgChartFixture())
    const bottom = Math.max(...[...L.orgs.values()].map((b) => b.y + b.h))
    expect(L.labels.map((l) => l.text)).toEqual([LOOSE_LABEL, REMOTE_LABEL])
    expect(L.agents.get('ada')!.y).toBeGreaterThan(bottom)
    expect(L.agents.get('far')!.y).toBeGreaterThan(L.agents.get('ada')!.y)
    expect(L.agents.get('bo')!.orgId).toBe('org-support')
  })

  it('a loose card a person placed keeps its spot; Tidy up returns it to the row', () => {
    const chart = ops.place(orgChartFixture(), { kind: 'agent', id: 'cy' }, 1500, -300)
    expect(layoutChart(chart).agents.get('cy')).toMatchObject({ x: 1500, y: -300, pinned: true })
    expect(layoutChart(ops.tidy(chart)).agents.get('cy')).toEqual(layoutChart(orgChartFixture()).agents.get('cy'))
  })
})
