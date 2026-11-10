// The line rules the canvas checks before posting (Rosson 11-10: top to
// bottom or side to side, never top to top, no loops).
import { describe, expect, it } from 'vitest'
import { checkLine, TOP_TO_TOP_MESSAGE, wireKind } from './org-rules'
import { orgChartFixture } from './org-chart-fixture'
import type { OrgRef } from './org-api'

const agent = (id: string): OrgRef => ({ kind: 'agent', id })

describe('wireKind: which dots make which line', () => {
  it('bottom → top is below, the start on top; top → bottom is below, the end on top', () => {
    expect(wireKind('bottom', 'top')).toEqual({ ok: true, kind: 'below', upper: 'from' })
    expect(wireKind('top', 'bottom')).toEqual({ ok: true, kind: 'below', upper: 'to' })
  })

  it('side → side is beside, either way round', () => {
    for (const [a, b] of [['left', 'right'], ['right', 'left'], ['right', 'right'], ['left', 'left']] as const) {
      expect(wireKind(a, b)).toEqual({ ok: true, kind: 'beside' })
    }
  })

  it('top → top, bottom → bottom and top/bottom → a side are refused with the daemon sentence', () => {
    for (const [a, b] of [['top', 'top'], ['bottom', 'bottom'], ['top', 'left'], ['right', 'bottom']] as const) {
      expect(wireKind(a, b)).toEqual({ ok: false, refusal: { code: 'line_top_to_top', message: TOP_TO_TOP_MESSAGE } })
    }
  })
})

describe('checkLine', () => {
  it('a line to itself is refused', () => {
    expect(checkLine(orgChartFixture(), 'below', agent('ada'), agent('ada'))?.code).toBe('line_self')
  })

  it('a second line between the same two cards is refused, whichever way it is drawn', () => {
    const chart = orgChartFixture({ lines: [{ id: 'l1', kind: 'beside', a: agent('ada'), b: agent('cy') }] })
    expect(checkLine(chart, 'beside', agent('cy'), agent('ada'))?.code).toBe('line_exists')
    expect(checkLine(chart, 'below', agent('ada'), agent('cy'))?.code).toBe('line_exists')
  })

  it('B below A, then A below B: a loop, said the daemon way', () => {
    const chart = orgChartFixture({ lines: [{ id: 'l1', kind: 'below', a: agent('ada'), b: agent('bo') }] })
    expect(checkLine(chart, 'below', agent('bo'), agent('ada'))).toEqual({
      code: 'line_loop',
      message: "Ada is already above Bo, so Bo can't be above Ada.",
    })
  })

  it('a loop down a chain (Ada > Bo > Cy, then Cy above Ada) is refused too', () => {
    const chart = orgChartFixture({
      lines: [
        { id: 'l1', kind: 'below', a: agent('ada'), b: agent('bo') },
        { id: 'l2', kind: 'below', a: agent('bo'), b: agent('cy') },
      ],
    })
    expect(checkLine(chart, 'below', agent('cy'), agent('ada'))?.code).toBe('line_loop')
  })

  it('several lines up to one card are fine (no loop): Ada and Cy both above Bo', () => {
    const chart = orgChartFixture({ lines: [{ id: 'l1', kind: 'below', a: agent('ada'), b: agent('bo') }] })
    expect(checkLine(chart, 'below', agent('cy'), agent('bo'))).toBeNull()
  })

  it('an org end works like an agent end', () => {
    const org = (id: string): OrgRef => ({ kind: 'org', id })
    const chart = orgChartFixture({ lines: [{ id: 'l1', kind: 'below', a: org('org-billing'), b: org('org-support') }] })
    expect(checkLine(chart, 'below', org('org-support'), org('org-billing'))?.message).toBe(
      "Billing is already above Support, so Support can't be above Billing.",
    )
  })
})
