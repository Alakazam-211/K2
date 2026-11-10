// @vitest-environment jsdom
// The Org Chart tab's canvas against a stand-in daemon (org-api.fake.ts):
//   - a card dropped in a bucket calls the membership route (add; from
//     another bucket, move after a confirm; out onto the canvas, remove);
//   - lines: bottom → top posts `below` in the right order; top → top and a
//     loop snap back with the daemon's sentence and post nothing;
//   - the dotted layer toggles and is kept for this viewer;
//   - a dragged position is sent and kept; Tidy up clears it;
//   - `org_chart_changed` refetches; an older read never undoes a write;
//   - the org view shows purpose and bulletin; the 5-line limit holds;
//   - a Member gets the view only;
//   - it renders in Terminal and Glass (light and dark).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const confirm = vi.hoisted(() => ({ answer: true, calls: [] as { title: string }[] }))
vi.mock('@/stores/confirm-dialog', () => ({
  useConfirmDialogStore: {
    getState: () => ({
      confirm: async (o: { title: string }) => {
        confirm.calls.push(o)
        return confirm.answer
      },
    }),
  },
}))

import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { OrgChartBoard, LS_ORG_CHART_DOTTED, DOTTED_LABEL } from './OrgChartBoard'
import { fakeOrgApi, type FakeOrgApi } from './org-api.fake'
import { orgChartFixture } from './org-chart-fixture'
import { layoutChart } from './org-layout'
import { TOP_TO_TOP_MESSAGE } from './org-rules'
import type { OrgChart } from './org-api'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

let emit: ((revision: number) => void) | null = null
const subscribe = (fn: (revision: number) => void): (() => void) => {
  emit = fn
  return () => {
    emit = null
  }
}

async function mount(chart: OrgChart = orgChartFixture()): Promise<FakeOrgApi> {
  const api = fakeOrgApi(chart)
  render(<OrgChartBoard api={api} subscribe={subscribe} />)
  await screen.findByTestId('org-chart-board')
  return api
}

const card = (id: string): HTMLElement => document.querySelector(`[data-org-agent="${id}"]`) as HTMLElement
const dot = (id: string, side: string): HTMLElement => card(id).querySelector(`[data-org-dot="${side}"]`) as HTMLElement

/** Drag from `el` (pressed at world x/y; the camera starts at 0,0 ×1 in jsdom) to x2/y2. */
function drag(el: HTMLElement, x1: number, y1: number, x2: number, y2: number): void {
  fireEvent.pointerDown(el, { clientX: x1, clientY: y1, button: 0 })
  fireEvent.pointerMove(window, { clientX: (x1 + x2) / 2, clientY: (y1 + y2) / 2 })
  fireEvent.pointerMove(window, { clientX: x2, clientY: y2 })
  fireEvent.pointerUp(window, { clientX: x2, clientY: y2 })
}

/** Drag the card `id` so its centre lands on (x, y). */
function dragCardTo(chart: OrgChart, id: string, x: number, y: number): void {
  const a = layoutChart(chart).agents.get(id)!
  const sx = a.x + 20
  const sy = a.y + 20
  drag(card(id), sx, sy, x - a.w / 2 + 20, y - a.h / 2 + 20)
}

beforeEach(() => {
  localStorage.clear()
  confirm.answer = true
  confirm.calls = []
})

afterEach(() => {
  cleanup()
  document.documentElement.removeAttribute('data-style')
  document.documentElement.removeAttribute('data-scheme')
})

describe('membership by drag and drop', () => {
  it('dropping a loose agent in a bucket calls member/add, and the card is in it at once', async () => {
    const chart = orgChartFixture()
    const api = await mount(chart)
    const billing = layoutChart(chart).orgs.get('org-billing')!
    dragCardTo(chart, 'ada', billing.x + billing.w / 2, billing.y + billing.h / 2)
    expect(api.callsTo('member/add')).toEqual([{ route: 'member/add', body: { org: 'org-billing', agent: 'ada' } }])
    expect(api.callsTo('member/move')).toEqual([])
    await waitFor(() => expect(api.state.orgs.find((o) => o.id === 'org-billing')!.memberIds).toEqual(['ada']))
    // The bucket's count says so.
    const title = document.querySelector('[data-org-bucket-title="org-billing"]')!
    await waitFor(() => expect(title.textContent).toContain('1'))
  })

  it('dropping a member into another bucket asks once, then calls member/move', async () => {
    const chart = orgChartFixture()
    const api = await mount(chart)
    const billing = layoutChart(chart).orgs.get('org-billing')!
    dragCardTo(chart, 'bo', billing.x + billing.w / 2, billing.y + billing.h / 2)
    await waitFor(() => expect(api.callsTo('member/move')).toEqual([{ route: 'member/move', body: { agent: 'bo', org: 'org-billing' } }]))
    expect(confirm.calls.map((c) => c.title)).toEqual(['Move Bo to Billing?'])
    expect(api.callsTo('member/add')).toEqual([])
  })

  it('saying no to the move sends nothing', async () => {
    confirm.answer = false
    const chart = orgChartFixture()
    const api = await mount(chart)
    const billing = layoutChart(chart).orgs.get('org-billing')!
    dragCardTo(chart, 'bo', billing.x + billing.w / 2, billing.y + billing.h / 2)
    await waitFor(() => expect(confirm.calls).toHaveLength(1))
    expect(api.callsTo('member/move')).toEqual([])
  })

  it('dragging a member out onto empty canvas calls member/remove', async () => {
    const chart = orgChartFixture()
    const api = await mount(chart)
    dragCardTo(chart, 'bo', 4000, 4000)
    expect(api.callsTo('member/remove')).toEqual([{ route: 'member/remove', body: { org: 'org-support', agent: 'bo' } }])
  })

  it("a refusal puts the daemon's chart back and says its sentence", async () => {
    const chart = orgChartFixture()
    const api = await mount(chart)
    api.failNext('member/add', 'agent_in_another_org', 'Ada is already in Support.')
    const billing = layoutChart(chart).orgs.get('org-billing')!
    dragCardTo(chart, 'ada', billing.x + billing.w / 2, billing.y + billing.h / 2)
    expect(await screen.findByTestId('org-chart-notice')).toHaveProperty('textContent', 'Ada is already in Support.')
    expect(document.querySelector('[data-org-bucket-title="org-billing"]')!.textContent).toContain('0')
  })

  it('an agent on another server is not draggable', async () => {
    const chart = orgChartFixture()
    const api = await mount(chart)
    const billing = layoutChart(chart).orgs.get('org-billing')!
    dragCardTo(chart, 'far', billing.x + 10, billing.y + 60)
    expect(api.calls.filter((c) => c.route !== 'chart')).toEqual([])
    expect(card('far').querySelector('[data-org-dot]')).toBeNull()
  })
})

describe('lines', () => {
  it("from one card's bottom dot to another's top: posts below, the first above", async () => {
    const chart = orgChartFixture()
    const api = await mount(chart)
    const L = layoutChart(chart)
    const ada = L.agents.get('ada')!
    const cy = L.agents.get('cy')!
    drag(dot('ada', 'bottom'), ada.x + ada.w / 2, ada.y + ada.h, cy.x + cy.w / 2, cy.y + 2)
    expect(api.callsTo('line/add')).toEqual([
      { route: 'line/add', body: { kind: 'below', a: { kind: 'agent', id: 'ada' }, b: { kind: 'agent', id: 'cy' } } },
    ])
    await waitFor(() => expect(document.querySelectorAll('[data-org-line]')).toHaveLength(1))
    expect(document.querySelector('[data-org-line]')!.getAttribute('data-kind')).toBe('below')
  })

  it('from a top dot to a bottom: the same line, reversed (the end card on top)', async () => {
    const chart = orgChartFixture()
    const api = await mount(chart)
    const L = layoutChart(chart)
    const ada = L.agents.get('ada')!
    const cy = L.agents.get('cy')!
    drag(dot('ada', 'top'), ada.x + ada.w / 2, ada.y, cy.x + cy.w / 2, cy.y + cy.h - 2)
    expect(api.callsTo('line/add')[0]!.body).toEqual({ kind: 'below', a: { kind: 'agent', id: 'cy' }, b: { kind: 'agent', id: 'ada' } })
  })

  it('side to side posts beside', async () => {
    const chart = orgChartFixture()
    const api = await mount(chart)
    const L = layoutChart(chart)
    const ada = L.agents.get('ada')!
    const cy = L.agents.get('cy')!
    drag(dot('ada', 'right'), ada.x + ada.w, ada.y + ada.h / 2, cy.x + 2, cy.y + cy.h / 2)
    expect(api.callsTo('line/add')[0]!.body).toMatchObject({ kind: 'beside' })
  })

  it('top to top snaps back with the sentence and posts nothing', async () => {
    const chart = orgChartFixture()
    const api = await mount(chart)
    const L = layoutChart(chart)
    const ada = L.agents.get('ada')!
    const cy = L.agents.get('cy')!
    drag(dot('ada', 'top'), ada.x + ada.w / 2, ada.y, cy.x + cy.w / 2, cy.y + 2)
    expect(api.callsTo('line/add')).toEqual([])
    expect(screen.getByTestId('org-chart-notice').textContent).toBe(TOP_TO_TOP_MESSAGE)
    expect(document.querySelector('[data-org-line]')).toBeNull()
    expect(document.querySelector('[data-org-wire]')).toBeNull()
  })

  it('a loop is refused in the UI: Ada above Cy already, Cy above Ada posts nothing', async () => {
    const chart = orgChartFixture({ lines: [{ id: 'l1', kind: 'below', a: { kind: 'agent', id: 'ada' }, b: { kind: 'agent', id: 'cy' } }] })
    const api = await mount(chart)
    const L = layoutChart(chart)
    const ada = L.agents.get('ada')!
    const cy = L.agents.get('cy')!
    drag(dot('cy', 'bottom'), cy.x + cy.w / 2, cy.y + cy.h, ada.x + ada.w / 2, ada.y + 2)
    expect(api.callsTo('line/add')).toEqual([])
    expect(screen.getByTestId('org-chart-notice').textContent).toBe("Ada is already above Cy, so Cy can't be above Ada.")
  })

  it('a selected line is removed with its button', async () => {
    const chart = orgChartFixture({ lines: [{ id: 'l1', kind: 'beside', a: { kind: 'agent', id: 'ada' }, b: { kind: 'agent', id: 'cy' } }] })
    const api = await mount(chart)
    const hit = document.querySelector('[data-org-line="l1"] path') as SVGPathElement
    fireEvent.pointerDown(hit, { button: 0 })
    fireEvent.click(await waitFor(() => document.querySelector('[data-org-line-remove="l1"]') as HTMLElement))
    expect(api.callsTo('line/remove')).toEqual([{ route: 'line/remove', body: { id: 'l1' } }])
  })
})

describe('the dotted layer (who can message whom)', () => {
  it('is off at first, the toggle shows it, and the choice is kept for this viewer', async () => {
    await mount()
    expect(document.querySelectorAll('[data-org-dotted]')).toHaveLength(0)
    const toggle = screen.getByRole('switch', { name: DOTTED_LABEL })
    fireEvent.click(toggle)
    expect(document.querySelectorAll('[data-org-dotted]')).toHaveLength(1)
    expect(document.querySelector('[data-org-dotted]')!.getAttribute('stroke-dasharray')).toBe('0.1 5')
    expect(localStorage.getItem(LS_ORG_CHART_DOTTED)).toBe('1')
    fireEvent.click(toggle)
    expect(document.querySelectorAll('[data-org-dotted]')).toHaveLength(0)
    expect(localStorage.getItem(LS_ORG_CHART_DOTTED)).toBe('0')
  })

  it('a viewer who turned it on finds it on next time', async () => {
    localStorage.setItem(LS_ORG_CHART_DOTTED, '1')
    await mount()
    expect(document.querySelectorAll('[data-org-dotted]')).toHaveLength(1)
  })
})

describe('positions', () => {
  it('a dragged card is sent with the revision and stays where it was put after the refetch; Tidy up clears it', async () => {
    const chart = orgChartFixture()
    const api = await mount(chart)
    const auto = layoutChart(chart).agents.get('cy')!
    const tidy = screen.getByRole('button', { name: 'Tidy up' }) as HTMLButtonElement
    expect(tidy.disabled).toBe(true)
    drag(card('cy'), auto.x + 10, auto.y + 10, auto.x + 10 + 500, auto.y + 10 + 300)
    expect(api.callsTo('layout/move')).toEqual([
      { route: 'layout/move', body: { kind: 'agent', id: 'cy', x: auto.x + 500, y: auto.y + 300, revision: 1 } },
    ])
    await waitFor(() => expect(api.calls.filter((c) => c.route === 'chart')).toHaveLength(2))
    expect(card('cy').style.left).toBe(`${auto.x + 500}px`)
    expect(card('cy').style.top).toBe(`${auto.y + 300}px`)

    await waitFor(() => expect(tidy.disabled).toBe(false))
    fireEvent.click(tidy)
    expect(api.callsTo('layout/tidy')).toEqual([{ route: 'layout/tidy', body: { revision: 2 } }])
    expect(card('cy').style.left).toBe(`${auto.x}px`)
    expect(card('cy').style.top).toBe(`${auto.y}px`)
  })

  it('a drag on a chart that changed underneath (stale revision) is read again and sent once more', async () => {
    const chart = orgChartFixture()
    const api = await mount(chart)
    // Someone else wrote: the daemon is at revision 2, the tab still shows 1.
    api.state = { ...api.state, revision: 2 }
    const auto = layoutChart(chart).agents.get('cy')!
    drag(card('cy'), auto.x + 10, auto.y + 10, auto.x + 110, auto.y + 10)
    await waitFor(() => expect(api.callsTo('layout/move').map((c) => c.body.revision)).toEqual([1, 2]))
    expect(api.state.nodes).toEqual([{ kind: 'agent', id: 'cy', x: auto.x + 100, y: auto.y, w: null, h: null, placedBy: 'person' }])
  })
})

describe('live updates', () => {
  it('org_chart_changed with a newer revision refetches; the same revision does not', async () => {
    const api = await mount()
    const reads = (): number => api.calls.filter((c) => c.route === 'chart').length
    expect(reads()).toBe(1)
    act(() => emit!(1))
    expect(reads()).toBe(1)
    api.state = { ...api.state, revision: 5, orgs: [...api.state.orgs, { id: 'org-new', name: 'Research', purpose: '', headId: null, memberIds: [], bulletin: null }] }
    act(() => emit!(5))
    await waitFor(() => expect(document.querySelector('[data-org-bucket="org-new"]')).not.toBeNull())
    expect(reads()).toBe(2)
  })
})

describe('the org view', () => {
  it('a click on a bucket opens its purpose and bulletin; an edit over 5 lines is refused before sending; 5 lines go', async () => {
    const api = await mount()
    const title = document.querySelector('[data-org-bucket-title="org-support"]') as HTMLElement
    fireEvent.pointerDown(title, { clientX: 5, clientY: 5, button: 0 })
    fireEvent.pointerUp(window, { clientX: 5, clientY: 5 })
    const panel = await screen.findByTestId('org-panel')
    expect((screen.getByLabelText('Purpose') as HTMLTextAreaElement).value).toBe('Help for customers.')
    const bulletin = screen.getByLabelText('Bulletin') as HTMLTextAreaElement
    expect(bulletin.value).toBe('Tickets go here.\nThis week: the new inbox.')
    expect(panel.textContent).toContain('Bo')

    fireEvent.change(bulletin, { target: { value: '1\n2\n3\n4\n5\n6\n7' } })
    expect(screen.getByTestId('org-panel-bulletin-count').textContent).toBe(
      'An org bulletin is at most 5 lines; this one has 7. Cut 2 lines and send it again.',
    )
    const save = screen.getByRole('button', { name: 'Save bulletin' }) as HTMLButtonElement
    expect(save.disabled).toBe(true)

    fireEvent.change(bulletin, { target: { value: 'one\ntwo\nthree\nfour\nfive' } })
    expect(screen.getByTestId('org-panel-bulletin-count').textContent).toBe('5 of 5 lines')
    fireEvent.click(save)
    await waitFor(() =>
      expect(api.callsTo('bulletin/set')).toEqual([{ route: 'bulletin/set', body: { org: 'org-support', lines: ['one', 'two', 'three', 'four', 'five'] } }]),
    )

    fireEvent.change(screen.getByLabelText('Purpose'), { target: { value: 'Help, kindly.' } })
    // One save at a time: Save purpose waits for the bulletin's to finish.
    const savePurpose = screen.getByRole('button', { name: 'Save purpose' }) as HTMLButtonElement
    await waitFor(() => expect(savePurpose.disabled).toBe(false))
    fireEvent.click(savePurpose)
    await waitFor(() => expect(api.callsTo('purpose')).toEqual([{ route: 'purpose', body: { org: 'org-support', purpose: 'Help, kindly.' } }]))
  })

  it('Make head in the org view calls the head route', async () => {
    const chart = orgChartFixture()
    chart.orgs[1]!.memberIds.push('cy')
    const api = await mount(chart)
    const title = document.querySelector('[data-org-bucket-title="org-support"]') as HTMLElement
    fireEvent.pointerDown(title, { clientX: 5, clientY: 5, button: 0 })
    fireEvent.pointerUp(window, { clientX: 5, clientY: 5 })
    await screen.findByTestId('org-panel')
    fireEvent.click(screen.getByRole('button', { name: 'Make head' }))
    expect(api.callsTo('head')).toEqual([{ route: 'head', body: { org: 'org-support', agent: 'cy' } }])
    await waitFor(() => expect(card('cy').hasAttribute('data-head')).toBe(true))
  })
})

describe('a Member login: the view only', () => {
  it('shows View only, no dots, no drags, no Tidy up or New org; the org view reads', async () => {
    const chart = orgChartFixture({ canEdit: false })
    const api = await mount(chart)
    expect(screen.getByTestId('org-chart-view-only').textContent).toBe('View only')
    expect(document.querySelector('[data-org-dot]')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Tidy up' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'New org' })).toBeNull()
    const billing = layoutChart(chart).orgs.get('org-billing')!
    dragCardTo(chart, 'ada', billing.x + billing.w / 2, billing.y + billing.h / 2)
    expect(api.calls.filter((c) => c.route !== 'chart')).toEqual([])
    fireEvent.pointerDown(document.querySelector('[data-org-bucket-title="org-support"]')!, { button: 0 })
    await screen.findByTestId('org-panel')
    expect(screen.getByTestId('org-panel-bulletin').textContent).toContain('Tickets go here.')
    expect(screen.queryByLabelText('Bulletin')).toBeNull()
  })
})

describe('both Styles', () => {
  for (const [style, scheme] of [
    ['terminal', 'dark'],
    ['terminal', 'light'],
    ['glass', 'dark'],
    ['glass', 'light'],
  ] as const) {
    it(`renders buckets, cards and lines in ${style} / ${scheme}, from K2's tokens`, async () => {
      document.documentElement.setAttribute('data-style', style)
      document.documentElement.setAttribute('data-scheme', scheme)
      localStorage.setItem(LS_ORG_CHART_DOTTED, '1')
      await mount(orgChartFixture({ lines: [{ id: 'l1', kind: 'below', a: { kind: 'agent', id: 'bo' }, b: { kind: 'agent', id: 'cy' } }] }))
      const ada = card('ada')
      // Cards and buckets are the Style's own surfaces: Terminal cuts their
      // corners (data-k2-chamfer), Glass paints its Tile material.
      expect(ada.getAttribute('data-k2-chamfer')).toBe('surface')
      expect(ada.getAttribute('data-material')).toBe('tile')
      expect(ada.matches('html[data-style="terminal"] [data-k2-chamfer]')).toBe(style === 'terminal')
      expect(document.querySelector('[data-org-bucket="org-support"]')!.getAttribute('data-k2-chamfer')).toBe('surface')
      // Lines and dots draw in tokens, never a fixed colour.
      const line = document.querySelector('[data-org-line="l1"] path:last-child') as SVGPathElement
      expect(line.style.stroke).toBe('var(--color-text-secondary)')
      expect((document.querySelector('[data-org-dotted]') as SVGPathElement).style.stroke).toBe('var(--color-text-muted)')
      expect(screen.getByTestId('org-chart-canvas').className).toContain('bg-[var(--color-bg-canvas)]')
    })
  }
})
