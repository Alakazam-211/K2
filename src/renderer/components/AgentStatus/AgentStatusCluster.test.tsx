// @vitest-environment jsdom
//
// plan-agent-status-unify-v1 §5: the shared cluster. DOM order is working →
// heartbeat → status → number on every row; toggling working, a status or
// live/reaped moves nothing (every slot that toggles is reserved, 14px);
// Q7 = B: no heartbeat → no heartbeat slot; aria labels on every glyph;
// reduced motion stills the spinner.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { agentStatus, type AgentRollupInput } from '@/lib/agent-status'
import { AgentStatusCluster, AgentStatusSlot, NumberColumn } from './AgentStatusCluster'

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

const COUNTS = { working: 0, waiting: 0, monitoring: 0, unverifiable: 0, idle: 0 }
function view(o: { working?: boolean; waiting?: boolean; monitoring?: boolean; unseen?: 'done' | 'failed' | null; hb?: boolean; live?: number }) {
  const rollup: AgentRollupInput = {
    display: o.waiting ? 'waiting' : o.working ? 'working' : 'idle',
    counts: { ...COUNTS, waiting: o.waiting ? 1 : 0, working: o.working ? 1 : 0, monitoring: o.monitoring ? 1 : 0 },
    busy: o.working ? 1 : 0,
    ...(o.live !== undefined ? { live: o.live } : {}),
  }
  return agentStatus({ rollup, unseen: o.unseen ?? null, heartbeatEnabled: !!o.hb })
}

function slots(container: HTMLElement): string[] {
  const cluster = container.querySelector('[data-agent-cluster]')
  if (!cluster) throw new Error('no cluster')
  return [...cluster.children].map((c) => c.getAttribute('data-slot') ?? '?')
}

function stubReducedMotion(on: boolean): void {
  vi.stubGlobal(
    'matchMedia',
    vi.fn((query: string) => ({
      matches: on && query.includes('prefers-reduced-motion'),
      media: query,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    })),
  )
  // `window.matchMedia` is what the hook reads.
  Object.defineProperty(window, 'matchMedia', { configurable: true, value: globalThis.matchMedia })
}

describe('AgentStatusCluster order and collapse (Q7 = B)', () => {
  it('with a heartbeat: working | heartbeat | status | number', () => {
    const { container } = render(<AgentStatusCluster view={view({ hb: true, live: 1 })} index={0} />)
    expect(slots(container)).toEqual(['working', 'heartbeat', 'status', 'number'])
  })

  it('without a heartbeat: working takes its column (working | status | number)', () => {
    const { container } = render(<AgentStatusCluster view={view({ live: 1 })} index={0} />)
    expect(slots(container)).toEqual(['working', 'status', 'number'])
  })

  it('a list without chords keeps a blank number column (presence still lines up)', () => {
    const { container } = render(<AgentStatusCluster view={view({})} index={null} />)
    expect(slots(container)).toEqual(['working', 'status', 'number'])
    expect(container.querySelector('[data-slot="number"]')?.textContent).toBe('')
  })

  it('turning working, a status or live on and off moves nothing', () => {
    const shapes = [
      view({ hb: true }),
      view({ hb: true, working: true }),
      view({ hb: true, waiting: true, working: true, live: 1 }),
      view({ hb: true, live: 0 }),
      view({ hb: true, unseen: 'done', live: 1 }),
    ]
    const layouts = shapes.map((v) => {
      const { container, unmount } = render(<AgentStatusCluster view={v} index={3} />)
      const out = [...container.querySelectorAll<HTMLElement>('[data-agent-cluster] > [data-slot]')].map(
        (el) => `${el.getAttribute('data-slot')}:${el.style.width}x${el.style.height}`,
      )
      unmount()
      return out
    })
    for (const l of layouts) expect(l).toEqual(layouts[0])
    expect(layouts[0]).toEqual(['working:14pxx14px', 'heartbeat:14pxx14px', 'status:14pxx14px', 'number:14pxx14px'])
  })
})

describe('glyphs: state, aria, shape', () => {
  it('every state has its own data-agent-status and an aria label', () => {
    const cases: Array<[ReturnType<typeof view>, string, string]> = [
      [view({ waiting: true }), 'needs-you', 'Needs you'],
      [view({ unseen: 'failed' }), 'failed', 'Failed · The turn failed (not viewed yet)'],
      [view({ unseen: 'done' }), 'done', 'Done · Finished the turn (not viewed yet)'],
      [view({ monitoring: true }), 'monitoring', 'Monitoring · Only background work is left'],
      [view({ live: 1 }), 'live', 'Live session (using RAM)'],
      [view({ live: 0 }), 'reaped', 'No live session (reaped to save RAM)'],
    ]
    for (const [v, status, label] of cases) {
      const { container, unmount } = render(<AgentStatusCluster view={v} index={0} />)
      const slot = container.querySelector('[data-slot="status"]')!
      expect(slot.getAttribute('data-agent-status')).toBe(status)
      expect(slot.getAttribute('role')).toBe('img')
      expect(slot.getAttribute('aria-label')).toBe(label)
      expect(slot.getAttribute('title')).toBe(label)
      unmount()
    }
  })

  it('idle with live unknown is an empty, hidden slot', () => {
    const { container } = render(<AgentStatusCluster view={view({})} index={0} />)
    const slot = container.querySelector('[data-slot="status"]')!
    expect(slot.getAttribute('data-agent-status')).toBe('idle')
    expect(slot.getAttribute('aria-hidden')).toBe('true')
    expect(slot.childElementCount).toBe(0)
  })

  it('live is a solid green square, reaped a solid grey one at 50%', () => {
    const live = render(<AgentStatusCluster view={view({ live: 1 })} />).container.querySelector<HTMLElement>('[data-live-square]')!
    expect(live.getAttribute('data-live-square')).toBe('live')
    expect(live.style.background).toBe('var(--color-status-ok)')
    cleanup()
    const reaped = render(<AgentStatusCluster view={view({ live: 0 })} />).container.querySelector<HTMLElement>('[data-live-square]')!
    expect(reaped.getAttribute('data-live-square')).toBe('reaped')
    expect(reaped.style.opacity).toBe('0.5')
  })

  it('needs-you is the only filled square, in the needs-you token', () => {
    const { container } = render(<AgentStatusCluster view={view({ waiting: true })} />)
    const slot = container.querySelector<HTMLElement>('[data-slot="status"]')!
    expect(slot.style.color).toBe('var(--color-status-needs-you)')
    expect(slot.querySelector('rect')?.getAttribute('fill')).toBe('currentColor')
  })

  it('working shows the 3×3 grid in the working token, with its label', () => {
    const { container } = render(<AgentStatusCluster view={view({ working: true })} />)
    const slot = container.querySelector<HTMLElement>('[data-slot="working"]')!
    expect(slot.style.color).toBe('var(--color-status-working)')
    expect(slot.getAttribute('aria-label')).toBe('Working')
    expect(slot.querySelectorAll('rect')).toHaveLength(9)
  })

  it('the heartbeat slot is labelled', () => {
    const { container } = render(<AgentStatusCluster view={view({ hb: true })} />)
    expect(container.querySelector('[data-slot="heartbeat"]')?.getAttribute('aria-label')).toBe(
      'Has an enabled heartbeat (it can run on its own)',
    )
  })
})

describe('reduced motion', () => {
  it('stills the working grid (static class)', () => {
    stubReducedMotion(true)
    const { container } = render(<AgentStatusCluster view={view({ working: true })} />)
    const svg = container.querySelector('svg.agent-working')!
    expect(svg.getAttribute('class')).toContain('is-still')
    expect(svg.getAttribute('data-agent-working')).toBe('still')
  })

  it('moves without the preference', () => {
    stubReducedMotion(false)
    const { container } = render(<AgentStatusCluster view={view({ working: true })} />)
    const svg = container.querySelector('svg.agent-working')!
    expect(svg.getAttribute('class')).not.toContain('is-still')
    expect(svg.getAttribute('data-agent-working')).toBe('moving')
  })
})

describe('NumberColumn', () => {
  it('1–9, 0, then a muted dash, in a 14px slot', () => {
    const labels = Array.from({ length: 12 }, (_, i) => {
      const { container, unmount } = render(<NumberColumn index={i} />)
      const el = container.querySelector<HTMLElement>('[data-slot="number"]')!
      const out = { text: el.textContent, width: el.style.width, dim: el.style.opacity }
      unmount()
      return out
    })
    expect(labels.map((l) => l.text)).toEqual(['1', '2', '3', '4', '5', '6', '7', '8', '9', '0', '-', '-'])
    for (const l of labels) expect(l.width).toBe('14px')
    expect(labels[10].dim).toBe('0.55')
    expect(labels[9].dim).toBe('')
  })
})

describe('AgentStatusSlot (one mark, for tabs and badges)', () => {
  it('a status wins, else working, else nothing', () => {
    const a = render(<AgentStatusSlot view={view({ waiting: true, working: true })} />).container
    expect(a.querySelector('[data-slot]')?.getAttribute('data-slot')).toBe('status')
    cleanup()
    const b = render(<AgentStatusSlot view={view({ working: true, live: 1 })} />).container
    expect(b.querySelector('[data-slot]')?.getAttribute('data-slot')).toBe('working')
    cleanup()
    const c = render(<AgentStatusSlot view={view({ live: 1 })} />).container
    expect(c.querySelector('[data-slot]')).toBeNull()
  })
})
