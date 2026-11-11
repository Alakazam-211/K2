// @vitest-environment jsdom
// Settings → Logs → Timer: settings in one card with save feedback; History
// filters by dates and workspace, has designed empty / failed states, and
// asks before deleting an entry.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({
  fetchEntries: vi.fn(async () => undefined),
  deleteEntry: vi.fn(async () => true),
  restoreEntry: vi.fn(async () => true),
  exportEntries: vi.fn(async () => 'id\n'),
  updateTimerSetting: vi.fn(async () => true),
  state: {} as Record<string, unknown>,
}))

vi.mock('@/stores/timer', async (orig) => {
  const actual = await orig<typeof import('@/stores/timer')>()
  const use = (sel: (s: Record<string, unknown>) => unknown) => sel(h.state)
  return { ...actual, useTimerStore: use }
})
vi.mock('@/stores/projects', () => ({
  useProjectsStore: (sel: (s: { projects: Array<{ id: string; name: string }> }) => unknown) =>
    sel({ projects: [{ id: 'p1', name: 'atlas' }, { id: 'p2', name: 'billing' }] }),
}))

import { TimerSection, formatSpent, rangeBounds } from './TimerSection'

const ENTRY = { id: 't1', projectId: 'p1', startTime: 1_794_000_000, endTime: 1_794_003_600, durationSeconds: 3600, memo: 'Design review', createdAt: 1 }

function setState(extra: Record<string, unknown> = {}): void {
  h.state = {
    visible: true,
    skipMemo: false,
    timezone: 'UTC',
    entries: [ENTRY],
    entriesStatus: 'ready',
    entriesError: null,
    fetchEntries: h.fetchEntries,
    deleteEntry: h.deleteEntry,
    restoreEntry: h.restoreEntry,
    exportEntries: h.exportEntries,
    updateTimerSetting: h.updateTimerSetting,
    ...extra,
  }
}

beforeEach(() => {
  for (const f of [h.fetchEntries, h.deleteEntry, h.restoreEntry, h.exportEntries, h.updateTimerSetting]) f.mockClear()
  setState()
})
afterEach(() => cleanup())

describe('rangeBounds', () => {
  it('turns a range into first-day start and last-day end', () => {
    const now = new Date(2026, 10, 15, 12, 0)
    expect(rangeBounds('all', '', '', now)).toEqual({})
    expect(rangeBounds('month', '', '', now).start).toBe(Math.floor(new Date(2026, 10, 1).getTime() / 1000))
    const last = rangeBounds('last-month', '', '', now)
    expect(last.start).toBe(Math.floor(new Date(2026, 9, 1).getTime() / 1000))
    expect(last.end).toBe(Math.floor(new Date(2026, 10, 1).getTime() / 1000) - 1)
    expect(rangeBounds('custom', '2026-11-02', '', now)).toEqual({ start: Math.floor(new Date('2026-11-02T00:00:00').getTime() / 1000), end: undefined })
  })
})

describe('formatSpent', () => {
  it('reads in hours and minutes', () => {
    expect(formatSpent(20)).toBe('under a minute')
    expect(formatSpent(48 * 60)).toBe('48m')
    expect(formatSpent(3600)).toBe('1h')
    expect(formatSpent(95 * 60)).toBe('1h 35m')
  })
})

describe('TimerSection', () => {
  it('a setting change confirms; a failed one says so', async () => {
    render(<TimerSection />)
    fireEvent.click(screen.getByRole('switch', { name: 'Skip the note when you stop' }))
    await waitFor(() => expect(screen.getByText('Saved')).toBeTruthy())
    expect(h.updateTimerSetting).toHaveBeenCalledWith('skipMemo', true)

    h.updateTimerSetting.mockResolvedValueOnce(false)
    fireEvent.click(screen.getByRole('switch', { name: 'Show the timer in the top bar' }))
    await waitFor(() => expect(screen.getByText(/Couldn’t save: the server didn’t accept the change/)).toBeTruthy())
  })

  it('filters by workspace and offers a way back when nothing matches', async () => {
    render(<TimerSection />)
    expect(h.fetchEntries).toHaveBeenLastCalledWith(undefined, undefined, undefined)
    fireEvent.click(screen.getByRole('button', { name: 'Workspace' }))
    fireEvent.click(document.querySelector('[data-setting-dropdown-menu] button[data-value="p2"]') as HTMLElement)
    await waitFor(() => expect(h.fetchEntries).toHaveBeenLastCalledWith(undefined, undefined, 'p2'))
    expect(screen.getByTestId('timer-total').textContent).toBe('One entry in billing, 1h in all.')
    cleanup()
    setState({ entries: [] })
    render(<TimerSection />)
    expect(screen.getByText('No time entries yet')).toBeTruthy()
  })

  it('a failed read shows the error card with the reason and Try again', () => {
    setState({ entries: [], entriesStatus: 'error', entriesError: 'database is locked' })
    render(<TimerSection />)
    expect(screen.getByText('Your time entries couldn’t load')).toBeTruthy()
    expect(screen.getByText('The server said: database is locked.')).toBeTruthy()
    const calls = h.fetchEntries.mock.calls.length
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    expect(h.fetchEntries.mock.calls.length).toBe(calls + 1)
  })

  it('asks before deleting, and says so when the delete fails', async () => {
    render(<TimerSection />)
    fireEvent.click(screen.getByRole('button', { name: 'Delete “Design review”' }))
    expect(screen.getByText('Delete this entry?')).toBeTruthy()
    expect(h.deleteEntry).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Keep' }))
    expect(screen.queryByText('Delete this entry?')).toBeNull()

    h.deleteEntry.mockResolvedValueOnce(false)
    fireEvent.click(screen.getByRole('button', { name: 'Delete “Design review”' }))
    fireEvent.click(screen.getByRole('button', { name: 'Delete the entry “Design review”' }))
    await waitFor(() => expect(screen.getByText('Couldn’t delete this entry. It’s still here; try again.')).toBeTruthy())
    expect(h.deleteEntry).toHaveBeenCalledWith('t1')
  })

  it('a delete says what was deleted and Undo puts it back', async () => {
    render(<TimerSection />)
    fireEvent.click(screen.getByRole('button', { name: 'Delete “Design review”' }))
    fireEvent.click(screen.getByRole('button', { name: 'Delete the entry “Design review”' }))
    await waitFor(() => expect(screen.getByTestId('timer-deleted')).toBeTruthy())
    expect(screen.getByRole('status').textContent).toBe('Deleted the entry “Design review” (1h, atlas).')
    fireEvent.click(screen.getByRole('button', { name: 'Undo' }))
    await waitFor(() => expect(screen.queryByTestId('timer-deleted')).toBeNull())
    expect(h.restoreEntry).toHaveBeenCalledWith(ENTRY)
  })

  it('an Undo that fails says so and keeps Undo', async () => {
    h.restoreEntry.mockResolvedValueOnce(false)
    render(<TimerSection />)
    fireEvent.click(screen.getByRole('button', { name: 'Delete “Design review”' }))
    fireEvent.click(screen.getByRole('button', { name: 'Delete the entry “Design review”' }))
    await waitFor(() => expect(screen.getByTestId('timer-deleted')).toBeTruthy())
    fireEvent.click(screen.getByRole('button', { name: 'Undo' }))
    await waitFor(() => expect(screen.getByText('Couldn’t bring the entry back. Try Undo again in a moment.')).toBeTruthy())
    expect(screen.getByRole('button', { name: 'Undo' })).toBeTruthy()
  })

  it('an export with no file says it failed', async () => {
    h.exportEntries.mockResolvedValueOnce('')
    render(<TimerSection />)
    fireEvent.click(screen.getByRole('button', { name: 'Export CSV' }))
    await waitFor(() => expect(screen.getByText(/Couldn’t export/)).toBeTruthy())
  })
})
