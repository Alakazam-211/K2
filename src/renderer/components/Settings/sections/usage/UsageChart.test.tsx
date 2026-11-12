// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { applyAppZoomForTest, resetAppZoomForTest } from '@/test-utils/app-zoom'
import { UsageChart } from './UsageChart'

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
