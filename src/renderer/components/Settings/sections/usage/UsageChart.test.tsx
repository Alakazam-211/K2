// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { applyAppZoomForTest, resetAppZoomForTest } from '@/test-utils/app-zoom'
import { AXIS_LABEL_SLOT_PX, ChartLegend, UsageChart, axisDays } from './UsageChart'

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  resetAppZoomForTest()
})

const day = (d: string, n: number) => ({ day: d, input_tokens: n, output_tokens: 0, cache_read_tokens: 0 })

describe('UsageChart under the app zoom', () => {
  it('at 150%, draws at the box width in CSS px, not its on-screen width', () => {
    applyAppZoomForTest(1.5)
    // The box is 600 CSS px wide, 900 on screen.
    vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(600)
    vi.spyOn(Element.prototype, 'getBoundingClientRect').mockReturnValue({ left: 0, top: 0, right: 900, bottom: 300, width: 900, height: 300 } as DOMRect)
    render(
      <UsageChart
        series={[{ harness: 'claude', days: [day('2026-11-10', 1200), day('2026-11-11', 800)] }]}
        slots={['2026-11-10', '2026-11-11']}
        measure="input"
        loading={false}
      />,
    )
    const svg = screen.getByTestId('usage-chart').querySelector('svg')
    expect(svg?.getAttribute('width')).toBe('600')
  })
})

const SLOTS = ['2026-11-09', '2026-11-10', '2026-11-11']
const TWO = [
  { harness: 'codex', days: [day('2026-11-10', 300), day('2026-11-11', 90)] },
  { harness: 'claude', days: [day('2026-11-09', 1000), day('2026-11-11', 2400)] },
]

function renderTwo(slots = SLOTS, series = TWO): HTMLElement {
  vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(640)
  render(<UsageChart series={series} slots={slots} measure="input" loading={false} />)
  return screen.getByTestId('usage-chart')
}

describe('the line graph', () => {
  it('draws one line and one soft area per harness, in its own colour', () => {
    const chart = renderTwo()
    const lines = chart.querySelectorAll('path[data-line]')
    expect([...lines].map((l) => l.getAttribute('data-line'))).toEqual(['claude', 'codex'])
    for (const l of lines) {
      expect(l.getAttribute('stroke-width')).toBe('2')
      expect(l.getAttribute('stroke-linejoin')).toBe('round')
      expect(l.getAttribute('stroke-linecap')).toBe('round')
      expect(l.getAttribute('fill')).toBe('none')
    }
    expect(lines[0].getAttribute('stroke')).toBe('var(--color-chart-1, var(--color-accent))')
    const areas = chart.querySelectorAll('path[data-area]')
    expect(areas).toHaveLength(2)
    expect(Number(areas[0].getAttribute('fill-opacity'))).toBeLessThanOrEqual(0.12)
    // No bars any more.
    expect(chart.querySelector('rect[data-harness]')).toBeNull()
  })

  it('hovering anywhere in a day shows a crosshair, a dot per series and a card with every series plus the total', () => {
    const chart = renderTwo()
    expect(screen.queryByTestId('usage-chart-tooltip')).toBeNull()
    fireEvent.mouseEnter(screen.getByTestId('usage-chart-hit-2026-11-10'))
    const cross = screen.getByTestId('usage-chart-crosshair')
    const hit = screen.getByTestId('usage-chart-hit-2026-11-10')
    const x = Number(cross.getAttribute('x1'))
    expect(x).toBeGreaterThan(Number(hit.getAttribute('x')))
    expect(x).toBeLessThan(Number(hit.getAttribute('x')) + Number(hit.getAttribute('width')))
    expect(chart.querySelectorAll('[data-testid="usage-chart-hover-dot"]')).toHaveLength(2)
    const tip = screen.getByTestId('usage-chart-tooltip')
    // Claude had nothing that day: it is listed as 0, not left out.
    expect(tip.textContent).toBe('Tue, Nov 10Claude0Codex300Total300')
    expect(tip.querySelectorAll('[data-testid="usage-series-key"]')).toHaveLength(2)
  })

  it('left and right arrows step the day while the chart has focus', () => {
    renderTwo()
    const plot = screen.getByRole('group', { name: /Input per day/ })
    plot.focus()
    fireEvent.keyDown(plot, { key: 'ArrowLeft' })
    expect(screen.getByTestId('usage-chart-tooltip').textContent).toContain('Wed, Nov 11')
    fireEvent.keyDown(plot, { key: 'ArrowLeft' })
    expect(screen.getByTestId('usage-chart-tooltip').textContent).toContain('Tue, Nov 10')
    fireEvent.keyDown(plot, { key: 'ArrowLeft' })
    fireEvent.keyDown(plot, { key: 'ArrowLeft' })
    expect(screen.getByTestId('usage-chart-tooltip').textContent).toContain('Mon, Nov 9')
    fireEvent.keyDown(plot, { key: 'ArrowRight' })
    expect(screen.getByTestId('usage-chart-tooltip').textContent).toContain('Tue, Nov 10')
    fireEvent.keyDown(plot, { key: 'Escape' })
    expect(screen.queryByTestId('usage-chart-tooltip')).toBeNull()
  })

  it('names each line at its right end in text colour, not the series colour', () => {
    renderTwo()
    const labels = screen.getAllByTestId(/^usage-chart-end-label-/)
    expect(labels.map((l) => l.textContent)).toEqual(['Claude', 'Codex'])
    for (const l of labels) {
      expect(l.className).toContain('text-[var(--color-text-secondary)]')
      expect(l.getAttribute('style') ?? '').not.toContain('chart')
    }
  })

  it('drops the end labels when the line ends crowd', () => {
    renderTwo(SLOTS, [
      { harness: 'claude', days: [day('2026-11-11', 1000)] },
      { harness: 'codex', days: [day('2026-11-11', 990)] },
    ])
    expect(screen.queryAllByTestId(/^usage-chart-end-label-/)).toEqual([])
  })

  it('one day is a dot with its number beside it, not a line', () => {
    const chart = renderTwo(['2026-11-11'], [{ harness: 'claude', days: [day('2026-11-11', 2400)] }])
    expect(chart.querySelector('path[data-line]')).toBeNull()
    expect(chart.querySelectorAll('[data-testid="usage-chart-dot"]')).toHaveLength(1)
    expect(screen.getByTestId('usage-chart-dot-label-claude').textContent).toBe('2.4K')
  })

  it('keeps the numbers table, one row per day with use', () => {
    renderTwo()
    fireEvent.click(screen.getByTestId('usage-chart-table-toggle'))
    const rows = screen.getByTestId('usage-chart-table').querySelectorAll('tbody tr')
    expect([...rows].map((r) => r.textContent)).toEqual(['Mon, Nov 91,00001,000', 'Tue, Nov 100300300', 'Wed, Nov 112,400902,490'])
  })

  it('pins the first and last day labels inside the plot, since the lines run edge to edge', () => {
    renderTwo()
    const axis = screen.getByText('Nov 9').parentElement as HTMLElement
    const labels = [...axis.children] as HTMLElement[]
    expect(labels.map((l) => l.textContent)).toEqual(['Nov 9', 'Nov 10', 'Nov 11'])
    expect(labels[0].style.left).toBe('48px')
    expect(labels[0].style.transform).toBe('')
    expect(labels[1].style.transform).toBe('translateX(-50%)')
    // The end labels' room sits right of the plot; the last day ends at the plot's edge.
    expect(Number.parseFloat(labels[2].style.right)).toBeGreaterThan(0)
  })
})

describe('the legend', () => {
  it('shows line keys for two or more series, and nothing for one', () => {
    const { unmount } = render(<ChartLegend harnesses={['claude', 'codex']} />)
    const keys = screen.getByTestId('usage-chart-legend').querySelectorAll('[data-testid="usage-series-key"]')
    expect(keys).toHaveLength(2)
    expect(keys[0].getAttribute('data-shape')).toBe('line')
    unmount()
    render(<ChartLegend harnesses={['claude']} />)
    expect(screen.queryByTestId('usage-chart-legend')).toBeNull()
  })
})

describe('the day axis on a narrow chart', () => {
  it('keeps about five labels with room, and every day for a short range', () => {
    expect(axisDays(30)).toEqual([0, 8, 16, 24, 29])
    expect(axisDays(7)).toEqual([0, 1, 2, 3, 4, 5, 6])
  })

  it('drops the middle labels first as the room shrinks, then the first; the last day stays', () => {
    expect(axisDays(30, 3)).toEqual([0, 15, 29])
    expect(axisDays(30, 2)).toEqual([0, 29])
    expect(axisDays(30, 1)).toEqual([29])
    expect(axisDays(7, 4)).toEqual([0, 2, 4, 6])
  })

  it('at 175 % in a small window (a 180 CSS px plot), labels fit their slots without colliding', () => {
    const shown = axisDays(30, 180 / AXIS_LABEL_SLOT_PX)
    expect(shown.length).toBeLessThanOrEqual(3)
    expect(shown[shown.length - 1]).toBe(29)
    // Each gap between shown days is at least one label slot wide.
    const px = (i: number): number => (i / 29) * 180
    for (let k = 1; k < shown.length; k++) expect(px(shown[k]) - px(shown[k - 1])).toBeGreaterThanOrEqual(AXIS_LABEL_SLOT_PX)
  })
})
