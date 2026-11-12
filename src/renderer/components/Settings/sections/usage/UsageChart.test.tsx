// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { applyAppZoomForTest, resetAppZoomForTest } from '@/test-utils/app-zoom'
import { AXIS_LABEL_SLOT_PX, UsageChart, axisDays } from './UsageChart'

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
