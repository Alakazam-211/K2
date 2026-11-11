// @vitest-environment jsdom
//
// The Projects page's Feedback tab when its read fails or finds nothing
// (0.46.12): a failure with nothing loaded is the shared state card (plain
// words, the reason quietly, Try again busy while it reads), never a route
// name; nothing yet is the same card in place of the board (no CLI, no
// "Select a ticket" detail pane).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
const feed = vi.hoisted(() => ({ all: vi.fn() }))
vi.mock('./feedback-api', () => ({ fetchAllFeedback: feed.all }))
vi.mock('./TicketBoard', () => ({
  TicketBoard: ({ rows, emptyHint }: { rows: unknown[] | null; emptyHint?: string }) => (
    <div data-testid="board" data-rows={rows === null ? 'null' : String(rows.length)}>
      {emptyHint}
    </div>
  ),
}))

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { ProjectFeedbackTab } from './ProjectFeedbackTab'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const MEMBERS = [{ workspaceId: 'w1', name: 'ada', path: '/w/ada', agentName: 'Ada', createdAt: 1 }]

beforeEach(() => {
  feed.all.mockReset()
})
afterEach(() => cleanup())

describe('Feedback tab states', () => {
  it('a failed read: the state card with the reason and Try again, no route name; Try again loads the board', async () => {
    let answer: (rows: unknown[]) => void = () => {}
    feed.all
      .mockRejectedValueOnce(new Error('the reply had no item list'))
      .mockImplementationOnce(() => new Promise((r) => (answer = r)))
    render(<ProjectFeedbackTab members={MEMBERS} />)
    const card = await screen.findByTestId('project-feedback-error')
    expect(card.textContent).toContain('Couldn’t load your agents’ questions')
    expect(card.querySelector('[data-load-detail]')?.textContent).toBe('the reply had no item list')
    expect(card.textContent).not.toMatch(/feedback\/|list-all|Failed to load|ticket/)
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    const busy = (await screen.findByRole('button', { name: 'Trying again…' })) as HTMLButtonElement
    expect(busy.disabled).toBe(true)
    answer([{ id: 't1', projectId: 'w1' }])
    await waitFor(() => expect(screen.getByTestId('board').getAttribute('data-rows')).toBe('1'))
    expect(screen.queryByTestId('project-feedback-error')).toBeNull()
  })

  it('empty: one card in place of the board, saying what shows up here, in body text', async () => {
    feed.all.mockResolvedValue([])
    render(<ProjectFeedbackTab members={MEMBERS} />)
    const card = await screen.findByTestId('project-feedback-empty')
    expect(card.textContent).toContain('Nothing to answer yet')
    const line = screen.getByText('When an agent in this project asks you something, it shows up here.')
    // The state card's body text: the secondary text colour, no fade.
    expect(line.className).toContain('text-[var(--color-text-secondary)]')
    expect(line.className).not.toMatch(/opacity|text-muted/)
    expect(card.textContent).not.toMatch(/k2 |`|ticket/i)
    expect(screen.queryByTestId('board')).toBeNull()
  })
})
