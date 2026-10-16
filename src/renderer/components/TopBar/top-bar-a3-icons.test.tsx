// @vitest-environment jsdom
//
// Icon style A3 (Rosson 2026-10-16, mock-up `mockup-icon-styles-v1.html`):
// the top bar's Run and drawer toggles draw the shared `K2Icons` (squared
// Lucide, 16px, 1.75 stroke). Buttons keep their size, hover, tooltip and
// accessible name; the drawer icon still shows open (solid divider) vs
// closed (dashed divider). The page tabs, Settings cog, server switcher and
// Admin profile are covered in their own test files.

import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    if (route === 'project-config/has-run-command') return { hasRunCommand: true }
    if (route === 'project-config/run-command') return { command: 'bun dev' }
    throw new Error(`unexpected route ${route}`)
  }),
}))
vi.mock('./ServerSwitcher', () => ({ default: () => <span data-stub="server" /> }))
vi.mock('./PageTabs', () => ({ default: () => <span data-stub="tabs" /> }))
vi.mock('./DesktopChromeLeft', () => ({ default: () => null }))
vi.mock('./DesktopChromeRight', () => ({
  default: ({ children }: { children?: React.ReactNode }) => <div data-stub="chrome-right">{children}</div>,
}))
vi.mock('./K2MarkButton', () => ({ default: () => <span data-stub="mark" /> }))
vi.mock('./TopBarUtilities', () => ({
  default: ({ leading, children }: { leading?: React.ReactNode; children?: React.ReactNode }) => (
    <div data-stub="utilities">
      {leading}
      {children}
    </div>
  ),
}))

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import TopBar from './TopBar'
import {
  AdminIcon,
  AgentsIcon,
  DrawerIcon,
  HomeIcon,
  K2_ICON_PX,
  K2_ICON_STROKE,
  ProjectsIcon,
  RunIcon,
  ServerIcon,
  SettingsIcon,
  TicketsIcon,
} from '@/components/icons/K2Icons'

afterEach(cleanup)

function svgIn(el: HTMLElement): SVGSVGElement {
  const svgs = el.querySelectorAll('svg')
  expect(svgs).toHaveLength(1)
  const svg = svgs[0]
  if (!(svg instanceof SVGSVGElement)) throw new Error('not an svg')
  return svg
}

function divider(svg: SVGSVGElement): Element {
  const path = svg.querySelector('path')
  if (!path) throw new Error('drawer icon has no divider')
  return path
}

describe('K2Icons (A3)', () => {
  it('one size, one stroke: 16px on a 24 grid at 1.75, squared Lucide except the gear', () => {
    expect([K2_ICON_PX, K2_ICON_STROKE]).toEqual([16, 1.75])
    const squared = [
      ['home', HomeIcon],
      ['agents', AgentsIcon],
      ['projects', ProjectsIcon],
      ['tickets', TicketsIcon],
      ['server', ServerIcon],
      ['admin', AdminIcon],
      ['run', RunIcon],
    ] as const
    for (const [name, Icon] of squared) {
      const { container } = render(<Icon />)
      const svg = svgIn(container)
      expect([
        name,
        svg.getAttribute('data-k2-icon'),
        svg.getAttribute('width'),
        svg.getAttribute('height'),
        svg.getAttribute('viewBox'),
        svg.getAttribute('fill'),
        svg.getAttribute('stroke'),
        svg.getAttribute('stroke-width'),
        svg.getAttribute('stroke-linecap'),
        svg.getAttribute('stroke-linejoin'),
        svg.getAttribute('aria-hidden'),
      ]).toEqual([name, name, '16', '16', '0 0 24 24', 'none', 'currentColor', '1.75', 'square', 'miter', 'true'])
      // No rounded corners anywhere.
      expect(svg.querySelectorAll('[rx]')).toHaveLength(0)
      cleanup()
    }
    const { container } = render(<SettingsIcon />)
    const gear = svgIn(container)
    expect([gear.getAttribute('stroke-linecap'), gear.getAttribute('stroke-linejoin'), gear.getAttribute('width')]).toEqual([
      'round',
      'round',
      '16',
    ])
  })

  it('a plain ServerIcon draws Lucide\'s dots; with ledColor they become the coloured LEDs', () => {
    const plain = render(<ServerIcon />)
    const a = svgIn(plain.container)
    expect(Array.from(a.querySelectorAll('path')).map((p) => p.getAttribute('d'))).toEqual(['M6 6h.01', 'M6 18h.01'])
    expect(a.querySelectorAll('[data-led]')).toHaveLength(0)
    expect(a.getAttribute('data-testid')).toBeNull()
    cleanup()
    const lit = render(<ServerIcon ledColor="#3fb950" label="Connection: connected" />)
    const b = svgIn(lit.container)
    expect(b.querySelectorAll('path')).toHaveLength(0)
    expect(Array.from(b.querySelectorAll('[data-led]')).map((l) => l.getAttribute('fill'))).toEqual(['#3fb950', '#3fb950'])
    expect([b.getAttribute('role'), b.getAttribute('aria-label'), b.getAttribute('aria-hidden')]).toEqual([
      'img',
      'Connection: connected',
      null,
    ])
  })

  it('DrawerIcon: Lucide panel-left squared, mirrored for the right, dashed divider when closed', () => {
    const cases = [
      { side: 'left', open: true, d: 'M9 3v18', dash: null },
      { side: 'left', open: false, d: 'M9 3v18', dash: '0.5 4' },
      { side: 'right', open: true, d: 'M15 3v18', dash: null },
      { side: 'right', open: false, d: 'M15 3v18', dash: '0.5 4' },
    ] as const
    for (const c of cases) {
      const { container } = render(<DrawerIcon side={c.side} open={c.open} />)
      const svg = svgIn(container)
      const frame = svg.querySelector('rect')
      expect([frame?.getAttribute('x'), frame?.getAttribute('y'), frame?.getAttribute('width'), frame?.getAttribute('rx')]).toEqual([
        '3',
        '3',
        '18',
        null,
      ])
      expect([divider(svg).getAttribute('d'), divider(svg).getAttribute('stroke-dasharray')]).toEqual([c.d, c.dash])
      cleanup()
    }
  })
})

describe('TopBar Run and drawer toggles draw A3 icons', () => {
  it('Run: the squared play triangle, same button, tooltip and hover colour', async () => {
    const onRun = vi.fn()
    render(<TopBar projectPath="/w/cortana" onRunCommand={onRun} />)
    const run = await waitFor(() => screen.getByTitle('Run workspace command'))
    const svg = svgIn(run)
    expect([svg.getAttribute('data-k2-icon'), svg.getAttribute('width')]).toEqual(['run', '16'])
    expect(svg.querySelector('path')?.getAttribute('d')).toBe('M5 3l14 9-14 9Z')
    for (const cls of ['h-6', 'w-6', 'hover:bg-[var(--color-bg-elevated)]', 'hover:text-[#4ec9b0]']) {
      expect([cls, run.classList.contains(cls)]).toEqual([cls, true])
    }
    fireEvent.click(run)
    await waitFor(() => expect(onRun).toHaveBeenCalledWith('bun dev'))
  })

  it('drawer toggles: A3 drawer icon per side, open vs closed, titles and clicks unchanged', () => {
    const onLeft = vi.fn()
    const onRight = vi.fn()
    const { rerender } = render(
      <TopBar leftPanelVisible rightPanelVisible={false} onToggleLeftPanel={onLeft} onToggleRightPanel={onRight} />,
    )
    const left = screen.getByTitle('Toggle left panel')
    const right = screen.getByTitle('Toggle right panel')
    for (const b of [left, right]) {
      for (const cls of ['h-6', 'w-6', 'hover:bg-[var(--color-bg-elevated)]', 'hover:text-[var(--color-text-primary)]']) {
        expect([cls, b.classList.contains(cls)]).toEqual([cls, true])
      }
    }
    const l = svgIn(left)
    const r = svgIn(right)
    expect([l.getAttribute('data-k2-icon'), l.getAttribute('width'), l.getAttribute('data-drawer-side'), l.getAttribute('data-drawer-open')]).toEqual([
      'drawer',
      '16',
      'left',
      'true',
    ])
    expect([r.getAttribute('data-k2-icon'), r.getAttribute('width'), r.getAttribute('data-drawer-side'), r.getAttribute('data-drawer-open')]).toEqual([
      'drawer',
      '16',
      'right',
      'false',
    ])
    expect(divider(l).getAttribute('stroke-dasharray')).toBeNull()
    expect(divider(r).getAttribute('stroke-dasharray')).toBe('0.5 4')

    fireEvent.click(left)
    fireEvent.click(right)
    expect([onLeft.mock.calls.length, onRight.mock.calls.length]).toEqual([1, 1])

    rerender(<TopBar leftPanelVisible={false} rightPanelVisible onToggleLeftPanel={onLeft} onToggleRightPanel={onRight} />)
    expect(svgIn(screen.getByTitle('Toggle left panel')).getAttribute('data-drawer-open')).toBe('false')
    expect(svgIn(screen.getByTitle('Toggle right panel')).getAttribute('data-drawer-open')).toBe('true')
  })

  it('no drawer toggles where the drawers open from edge handles', () => {
    render(<TopBar showDrawerToggles={false} />)
    expect(screen.queryByTitle('Toggle left panel')).toBeNull()
    expect(screen.queryByTitle('Toggle right panel')).toBeNull()
  })
})
