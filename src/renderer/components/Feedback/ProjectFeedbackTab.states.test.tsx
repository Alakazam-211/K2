// @vitest-environment jsdom
//
// The Projects page's Feedback tab when its read fails or finds nothing
// (0.46.12): a failure with nothing loaded is the shared state card (plain
// words, the reason quietly, Try again), never a route name; the empty
// board's hint names no CLI command.

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
    feed.all.mockRejectedValueOnce(new Error('the reply had no ticket list')).mockResolvedValue([])
    render(<ProjectFeedbackTab members={MEMBERS} />)
    const card = await screen.findByTestId('project-feedback-error')
    expect(card.textContent).toContain('Couldn’t load this project’s tickets')
    expect(card.textContent).toContain('The server said: the reply had no ticket list.')
    expect(card.textContent).not.toMatch(/feedback\/|list-all|Failed to load/)
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(screen.getByTestId('board').getAttribute('data-rows')).toBe('0'))
    expect(screen.queryByTestId('project-feedback-error')).toBeNull()
  })

  it('empty: the hint says what shows up here, with no CLI command', async () => {
    feed.all.mockResolvedValue([])
    render(<ProjectFeedbackTab members={MEMBERS} />)
    const board = await screen.findByTestId('board')
    expect(board.textContent).toBe('When an agent in this project asks you something, it shows up here.')
    expect(board.textContent).not.toMatch(/k2 |`/)
  })
})
