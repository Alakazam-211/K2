// @vitest-environment jsdom
// The timeline tab on its own sample: the panel's sections (Watching,
// Messages, Changes) and the notices show the sample turn's data; the
// content area stays a placeholder until the lanes and monitor (T3, T4).

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import TimelineSample from './TimelineSample'

beforeEach(() => vi.useFakeTimers({ shouldAdvanceTime: true }))
afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

async function mount(): Promise<void> {
  render(<TimelineSample width={1440} />)
  await act(async () => {})
}

const step = (n: number): void => {
  for (let i = 0; i < n; i++) fireEvent.click(screen.getByTestId('lv-sample-step'))
}

/** The banner's scenarios are a grouped menu (more than five): open it and pick one. */
function pick(id: string): void {
  fireEvent.click(within(screen.getByTestId('lv-sample-scenarios')).getByRole('button'))
  fireEvent.click(document.querySelector(`[data-setting-dropdown-menu] button[data-value="${id}"]`) as HTMLElement)
}

describe('the timeline tab on its sample', () => {
  it('opens on the watched edit under the banner: Watching has everyone, the content a placeholder', async () => {
    await mount()
    expect(screen.getByTestId('lv-sample-banner').textContent).toContain('Sample data, not this file')
    expect(screen.getByTestId('lv-sample-scenarios').textContent).toContain('Watching an edit')
    expect(screen.getByTestId('lv-state').textContent).toBe('Live')
    expect(screen.getByTestId('lv-watching-count').textContent).toBe('4')
    const w = screen.getByTestId('lv-watching')
    expect(within(w).getByText('Atlas')).toBeTruthy()
    expect(within(w).getAllByText('in the file, not editing')).toHaveLength(2)
    expect(screen.getByTestId('lv-hero').getAttribute('data-kind')).toBe('timeline')
    expect(screen.getByText('No changes yet')).toBeTruthy()
  })

  it('Changes, Messages and the hold fill in as the turn plays', async () => {
    await mount()
    step(18) // Atlas answers
    expect(screen.getByTestId('lv-messages-count').textContent).toBe('2')
    expect(screen.getByText('Is B-roll 03 better over this line?')).toBeTruthy()
    const rows = screen.getAllByTestId('lv-change').map((r) => r.textContent ?? '')
    expect(rows[rows.length - 1]).toContain('Atlas trimmed the start of Interview B by 1s')
    expect(rows.some((r) => r.includes('Atlas couldn’t trim Interview B: Brook is holding it'))).toBe(true)
    expect(screen.getByTestId('lv-person-agent:brook').textContent).toContain('holding 1 clip')
  })

  it('a refusal shows as one notice, with Ask Brook', async () => {
    await mount()
    step(14) // refused
    const n = screen.getByTestId('lv-notice')
    expect(n.textContent).toContain('Atlas couldn’t trim Interview B: Brook is holding it.')
    expect(within(n).getByRole('button', { name: 'Ask Brook' })).toBeTruthy()
  })

  it('outside saves and the connection reach the notice bar', async () => {
    await mount()
    pick('saves')
    expect(screen.getByTestId('lv-notice').textContent).toContain('Saved outside K2 · now in the live file')
    step(1)
    expect(screen.getByRole('button', { name: 'Open the copy' })).toBeTruthy()
    pick('connection')
    step(1)
    expect(screen.getByTestId('lv-notice').textContent).toContain('Reconnecting…')
    expect(screen.getByTestId('lv-watching-count').textContent).toBe('as you last saw')
  })
})
