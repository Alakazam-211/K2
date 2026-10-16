// @vitest-environment jsdom
//
// Icon style A3 (Rosson 2026-10-16, mock-up `mockup-icon-styles-v1.html`):
// the top bar's Run and drawer toggles draw the shared `K2Icons` (squared
// Lucide, 16px, 1.75 stroke). Buttons keep their size, hover, tooltip and
// accessible name; the drawer icon still shows open (solid divider) vs
// closed (dashed divider). The stopwatch (clock) and the K2 cheat sheet
// draw theirs too (Rosson: "the clock, mug, and cheat sheet"). The page
// tabs, Settings cog, server switcher, Admin profile and the Keep awake
// mug's fill levels are covered in their own test files.

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
import TimerButton from '@/components/Timer/TimerButton'
import K2NounsCheatSheet from '@/components/CheatSheet/K2NounsCheatSheet'
import { useTimerStore } from '@/stores/timer'
import {
  AdminIcon,
  AgentsIcon,
  CheatSheetIcon,
  ClockIcon,
  MugIcon,
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
      ['clock', ClockIcon],
      ['mug', () => <MugIcon fill="half" />],
      ['cheat-sheet', CheatSheetIcon],
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

describe('TopBar Run draws its A3 icon; no drawer toggles', () => {
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

  it('no drawer toggles: the drawers open from the room and drawer tab bars (0.46.0)', () => {
    render(<TopBar />)
    expect(screen.queryByTitle('Toggle left panel')).toBeNull()
    expect(screen.queryByTitle('Toggle right panel')).toBeNull()
    expect(document.querySelector('[data-k2-icon="drawer"]')).toBeNull()
  })
})

describe('the clock, mug and cheat sheet draw A3 icons', () => {
  it('Lucide clock / coffee / file-text bodies, squared', () => {
    const clock = svgIn(render(<ClockIcon />).container)
    expect(clock.querySelector('circle')?.getAttribute('r')).toBe('10')
    expect(clock.querySelector('path')?.getAttribute('d')).toBe('M12 6v6l4 2')
    cleanup()
    const mug = svgIn(render(<MugIcon fill="none" />).container)
    expect(Array.from(mug.querySelectorAll('path')).map((p) => p.getAttribute('d'))).toEqual([
      'M6 2v2',
      'M10 2v2',
      'M14 2v2',
      'M3 8h14v13H3Z',
      'M17 8h1a4 4 0 0 1 0 8h-1',
    ])
    cleanup()
    const sheet = svgIn(render(<CheatSheetIcon />).container)
    expect(Array.from(sheet.querySelectorAll('path')).map((p) => p.getAttribute('d'))).toEqual([
      'M4 2h10l6 6v14H4Z',
      'M14 2v6h6',
      'M10 9H8',
      'M16 13H8',
      'M16 17H8',
    ])
  })

  it('the stopwatch button draws the A3 clock; same button, tooltip and colours', () => {
    useTimerStore.setState({ visible: true, status: 'idle', showMemoDialog: false })
    render(<TimerButton />)
    const start = screen.getByTitle('Start stopwatch')
    const svg = svgIn(start)
    expect([svg.getAttribute('data-k2-icon'), svg.getAttribute('data-testid'), svg.getAttribute('width')]).toEqual([
      'clock',
      'timer-icon',
      '16',
    ])
    for (const cls of ['h-6', 'w-5', 'text-[var(--color-text-muted)]', 'hover:text-[var(--color-text-primary)]']) {
      expect([cls, start.classList.contains(cls)]).toEqual([cls, true])
    }
  })

  it('the cheat sheet button draws the A3 file-text; same label, tooltip and size', () => {
    render(<K2NounsCheatSheet />)
    const button = screen.getByRole('button', { name: 'K2 cheat sheet' })
    expect(button.getAttribute('title')).toBe('K2 cheat sheet')
    const svg = svgIn(button)
    expect([svg.getAttribute('data-k2-icon'), svg.getAttribute('width'), svg.getAttribute('aria-hidden')]).toEqual([
      'cheat-sheet',
      '16',
      'true',
    ])
    for (const cls of ['h-6', 'w-6', 'text-[var(--color-text-muted)]', 'hover:text-[var(--color-text-primary)]']) {
      expect([cls, button.classList.contains(cls)]).toEqual([cls, true])
    }
  })
})
