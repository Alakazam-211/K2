// @vitest-environment jsdom
// The live workbook view over the sample source, stepped by hand: the banner,
// loading, the grid, follow and pause, claims, the activity line, the
// outside save and the not-live states.

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { SAMPLE_FILE_NAME, SampleWorkbookSource, type SampleScenario } from '@/lib/live-view/sample-source'
import { LiveWorkbookView } from './LiveWorkbookView'

function mount(scenario: SampleScenario = 'turn'): SampleWorkbookSource {
  const src = new SampleWorkbookSource({ scenario, autoplay: false, loop: false })
  render(<LiveWorkbookView source={src} fileName={SAMPLE_FILE_NAME} sample={{ source: src, scenario, onScenario: () => {} }} />)
  return src
}

/** Step the sample until the step with this label has been sent. */
function stepTo(src: SampleWorkbookSource, label: string): void {
  act(() => {
    for (let i = 0; i < 200 && src.status().label !== label; i++) src.step()
  })
  expect(src.status().label).toBe(label)
}

const line = (): string => screen.getByTestId('lv-activity-line').textContent ?? ''

afterEach(() => cleanup())

describe('LiveWorkbookView with the sample source', () => {
  it('always says it is sample data, and shows the loading card until the snapshot is in', () => {
    const src = mount()
    expect(screen.getByTestId('lv-sample-banner').textContent).toContain('Sample data, not this file')
    stepTo(src, 'snapshot')
    expect(screen.getByTestId('lv-loading')).toBeTruthy()
    stepTo(src, 'roster')
    expect(screen.queryByTestId('lv-loading')).toBeNull()
    expect(screen.getByTestId('lv-sample-banner')).toBeTruthy()
    expect(screen.getByTestId('lv-cell-A1').textContent).toBe('Team budget 2026')
    // A formula from the file shows its cached result, formatted by its number format.
    expect(screen.getByTestId('lv-cell-E4').textContent).toBe('20,000')
    src.close()
  })

  it('follows Atlas by default: its claim is drawn with its label, and the formula bar is its cell', () => {
    const src = mount()
    stepTo(src, 'claim_started_cells, focus, cells')
    expect(screen.getByTestId('lv-follow').textContent).toBe('AFollowing Atlas')
    expect(screen.getByTestId('lv-claim-claim-31')).toBeTruthy()
    expect(screen.getByTestId('lv-grid').textContent).toContain('Atlas: updating Q2 figures · held')
    expect(screen.getByTestId('lv-namebox').textContent).toBe('C4')
    expect(screen.getByTestId('lv-formula-bar').textContent).toContain('Atlas’s cell')
    stepTo(src, 'rev 21')
    expect(screen.getByTestId('lv-namebox').textContent).toBe('D5')
    expect(line()).toContain('Atlas: updating Q2 figures')
    expect(screen.getByTestId('lv-activity').textContent).toContain('Latest: Atlas set D5:D7 on Budget')
    expect(screen.getAllByTestId('lv-change').length).toBe(3)
    src.close()
  })

  it('a tab pauses following; "Follow Atlas again" brings it back', () => {
    const src = mount()
    stepTo(src, 'rev 22')
    const tabs = screen.getByTestId('lv-tabs')
    fireEvent.click(within(tabs).getByTestId('lv-tab-@sheet:2'))
    expect(screen.getByTestId('lv-follow').textContent).toBe('Follow Atlas')
    expect(screen.getByTestId('lv-tab-@sheet:2').getAttribute('aria-selected')).toBe('true')
    const resume = screen.getByTestId('lv-resume')
    expect(resume.textContent).toContain('Atlas is on Budget')
    fireEvent.click(within(resume).getByRole('button', { name: 'Follow Atlas again' }))
    expect(screen.getByTestId('lv-follow').textContent).toContain('Following Atlas')
    expect(screen.getByTestId('lv-tab-@sheet:1').getAttribute('aria-selected')).toBe('true')
    src.close()
  })

  it('a clicked cell is yours in the formula bar, and pauses following', () => {
    const src = mount()
    stepTo(src, 'rev 21')
    fireEvent.click(screen.getByTestId('lv-cell-A4'))
    expect(screen.getByTestId('lv-follow').textContent).toBe('Follow Atlas')
    expect(screen.getByTestId('lv-formula-bar').textContent).toContain('Your cell')
    src.close()
  })

  it('an insert flashes its band and notes the rows renumbered with no op', () => {
    const src = mount()
    stepTo(src, 'rev 23')
    expect(screen.getByTestId('lv-insert-band')).toBeTruthy()
    expect(screen.getByTestId('lv-renumbered').textContent).toMatch(/^Rows 11–\d+ renumbered · no op$/)
    // While the claim is held, its label leads; the change is the latest.
    expect(screen.getByTestId('lv-activity').textContent).toContain('Latest: Atlas inserted a row at 10 on Budget, +2 more')
    src.close()
  })

  it('a rename follows Atlas to FX rates; after turn_end the line says it is done; the outside save is worded by the file', () => {
    const src = mount()
    stepTo(src, 'rev 26')
    expect(screen.getByTestId('lv-tab-@sheet:2').textContent).toContain('FX rates')
    expect(screen.getByTestId('lv-tab-@sheet:2').getAttribute('aria-selected')).toBe('true')
    stepTo(src, 'turn_end')
    expect(line()).toContain('Atlas is done · deleted row 17 on Budget, a blank row, +7 more')
    stepTo(src, 'rev 29')
    expect(line()).toBe('budget-golden.xlsx was saved outside K2: C4 and 9 results changed')
    src.close()
  })

  it('history groups changes by turn', () => {
    const src = mount()
    stepTo(src, 'turn_end')
    fireEvent.click(screen.getByTestId('lv-history-toggle'))
    const h = screen.getByTestId('lv-history')
    expect(h.textContent).toContain('8 changes')
    expect(h.textContent).toContain('· done')
    expect(h.textContent).toContain('rev 23inserted a row at 10 on Budget')
    src.close()
  })

  it('an idle turn ends on the clock, with no turn_end', () => {
    const src = mount()
    stepTo(src, 'idle')
    expect(line()).toContain('Atlas is done')
    expect(screen.getByTestId('lv-activity').textContent).toContain('No word from Atlas for 20 s')
    src.close()
  })

  it('too large: the saved version, read-only, with no presence or follow', () => {
    const src = mount('blob')
    act(() => {
      for (let i = 0; i < 40; i++) src.step()
    })
    expect(screen.getByTestId('lv-blob-banner').textContent).toContain('Too large to watch live')
    expect(screen.getByTestId('lv-header').textContent).toContain('Saved version · read-only')
    expect(screen.queryByTestId('lv-follow')).toBeNull()
    expect(screen.getByTestId('lv-cell-A1').textContent).toBe('Team budget 2026')
    src.close()
  })

  it('not opened: the over-32 MiB card', () => {
    const src = mount('none')
    act(() => src.step())
    expect(screen.getByTestId('lv-none').textContent).toContain('too large for K2')
    src.close()
  })
})
